-- session_events — the per-session append-only event log ("the spine").
--
-- Every durable, attributable thing that happens in an MCP session is a row
-- here: a kernel mutation, an overlay annotation (pin/flag), a control event
-- (propose_order/approve). State = fold(log); the content snapshot in
-- `documents` / `mcp_sessions` is a derived materialization of that fold.
--
-- Design mirrors the money plane (migration 027):
--   1. Append-only, bigserial total order; per-session monotonic `seq` assigned
--      inside the RPC under an advisory lock so concurrent serverless instances
--      can't collide.
--   2. Idempotency is namespaced PER SESSION (session_id, idempotency_key) — one
--      session's key can never collide with another's, and a retried append is a
--      safe no-op replay.
--   3. Writes happen ONLY inside the SECURITY DEFINER `append_session_event` RPC;
--      service_role gets SELECT only on the table, so even a leaked service key
--      can't forge history outside the audited append path.
--   4. append = persist = broadcast: an after-insert trigger fans the row out via
--      Supabase Realtime (`realtime.send`) to topic `session:<session_id>`. The
--      INSERT is the one write; the broadcast is a DB-side consequence, not an
--      app-side dual write. Best-effort — a missing realtime schema never blocks
--      the durable append.
--
-- High-frequency liveness (cursor/camera) is NOT stored here — it rides a plain
-- ephemeral Realtime broadcast. The test for what belongs in this table: "would
-- you want it in the Receipt?"

create table if not exists session_events (
  id              bigserial primary key,        -- global monotonic order
  -- The MCP session/document id (text — MCP ids aren't uuids). Possession of an
  -- unguessable id is the capability, same model as mcp_sessions.
  session_id      text not null,
  -- The owning user, or null for an anonymous capability session.
  user_id         uuid references auth.users(id) on delete cascade,
  -- Per-session 1..N, assigned in append_session_event() under an advisory lock.
  seq             integer not null,
  -- Who emitted it: a user sub, the literal 'agent', or 'human'.
  author          text not null,
  -- Coarse class. kernel = folds into geometry (driver-only later); overlay =
  -- annotations (open to viewers); control = lifecycle (propose/approve).
  kind            text not null check (kind in ('kernel', 'overlay', 'control')),
  -- Fine type: the tool name for kernel, 'pin'/'flag'/'stroke' for overlay,
  -- 'propose_order'/'approve'/… for control.
  type            text not null,
  -- kernel: {tool, args(slim), changed?}; overlay: {anchor, text, …}.
  payload         jsonb not null default '{}'::jsonb,
  idempotency_key text not null,
  created_at      timestamptz not null default now(),
  unique (session_id, idempotency_key),
  unique (session_id, seq)
);

create index if not exists session_events_session_idx
  on session_events (session_id, seq);

-- ─── guard: the append RPC is server-only (mirrors assert_money_caller) ───────
create or replace function assert_session_caller()
returns void
language plpgsql
security definer
set search_path = public
as $$
declare
  v_claims text := current_setting('request.jwt.claims', true);
begin
  if v_claims is not null
     and (v_claims::jsonb ->> 'role') in ('authenticated', 'anon') then
    raise exception 'forbidden: session_events functions are service-role only';
  end if;
end;
$$;

-- ─── append_session_event() — atomic, per-session-ordered, idempotent ────────
-- The ONLY way a row enters session_events. Returns jsonb:
--   { ok:true,  id, seq }                 on append
--   { ok:true,  id, seq, idempotent:true} on replay of the same (session, key)
--   { ok:false, reason }                  on a guarded failure (no raise)
create or replace function append_session_event(
  p_session_id text,
  p_user uuid,
  p_author text,
  p_kind text,
  p_type text,
  p_payload jsonb,
  p_idempotency_key text
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  v_existing session_events%rowtype;
  v_seq integer;
  v_id bigint;
begin
  perform assert_session_caller();

  if p_session_id is null or length(p_session_id) = 0 then
    return jsonb_build_object('ok', false, 'reason', 'missing_session_id');
  end if;
  if p_kind not in ('kernel', 'overlay', 'control') then
    return jsonb_build_object('ok', false, 'reason', 'invalid_kind');
  end if;
  if p_type is null or length(p_type) = 0 then
    return jsonb_build_object('ok', false, 'reason', 'missing_type');
  end if;
  if p_idempotency_key is null or length(p_idempotency_key) = 0 then
    return jsonb_build_object('ok', false, 'reason', 'missing_idempotency_key');
  end if;

  -- Serialize same-session appends so seq assignment + idempotency check-then-
  -- insert is race-free (the loser sees the winner's committed row).
  perform pg_advisory_xact_lock(hashtext(p_session_id));

  -- Idempotent replay (per-session key).
  select * into v_existing
    from session_events
    where session_id = p_session_id and idempotency_key = p_idempotency_key;
  if found then
    return jsonb_build_object('ok', true, 'idempotent', true,
                             'id', v_existing.id, 'seq', v_existing.seq);
  end if;

  select coalesce(max(seq), 0) + 1 into v_seq
    from session_events where session_id = p_session_id;

  insert into session_events
    (session_id, user_id, seq, author, kind, type, payload, idempotency_key)
    values (p_session_id, p_user, v_seq, p_author, p_kind, p_type,
            coalesce(p_payload, '{}'::jsonb), p_idempotency_key)
    returning id into v_id;

  return jsonb_build_object('ok', true, 'id', v_id, 'seq', v_seq);
end;
$$;

-- ─── broadcast trigger — append = persist = broadcast ────────────────────────
-- Fans each appended row out to topic `session:<session_id>`. Wrapped so a
-- missing realtime schema or a transient broadcast error can NEVER roll back
-- the durable insert. v1 uses a public topic (the unguessable session_id is the
-- capability); signed-in sessions can move to private channels + an RLS policy
-- on realtime.messages later without touching this table.
create or replace function broadcast_session_event()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  begin
    perform realtime.send(
      jsonb_build_object(
        'id', new.id,
        'seq', new.seq,
        'session_id', new.session_id,
        'author', new.author,
        'kind', new.kind,
        'type', new.type,
        'payload', new.payload,
        'created_at', new.created_at
      ),
      'session_event',                  -- event name
      'session:' || new.session_id,     -- topic
      false                             -- public topic; secrecy = capability (v1)
    );
  exception when others then
    null; -- best-effort fan-out; the durable append already succeeded
  end;
  return new;
end;
$$;

drop trigger if exists session_events_broadcast on session_events;
create trigger session_events_broadcast
  after insert on session_events
  for each row execute function broadcast_session_event();

-- ─── stale-event sweep (cron wiring is a follow-up; callable manually) ───────
create or replace function cleanup_stale_session_events(max_age interval default '30 days')
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  swept integer;
begin
  delete from session_events where created_at < now() - max_age;
  get diagnostics swept = row_count;
  return swept;
end;
$$;

-- ─── RLS — users read their own events; the RPC is the SOLE writer ───────────
alter table session_events enable row level security;

drop policy if exists "Users read own session events" on session_events;
create policy "Users read own session events"
  on session_events for select using (auth.uid() = user_id);
-- Anonymous capability sessions have user_id = null → no direct table read;
-- the server (service_role, bypasses RLS) and the broadcast topic serve them.

-- service_role: SELECT only on the table — the SECURITY DEFINER RPC is the sole
-- writer, mirroring wallets/wallet_ledger.
grant select on session_events to service_role;
grant usage, select on all sequences in schema public to service_role;

-- The append RPC is server-only. Never grant execute to authenticated/anon.
revoke all on function append_session_event(text, uuid, text, text, text, jsonb, text) from public;
revoke all on function cleanup_stale_session_events(interval) from public;
grant execute on function append_session_event(text, uuid, text, text, text, jsonb, text) to service_role;
grant execute on function cleanup_stale_session_events(interval) to service_role;
