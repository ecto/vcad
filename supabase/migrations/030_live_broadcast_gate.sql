-- Gate the live Realtime broadcast on the share state, so the event STREAM
-- honors the same "private by default, revocable" contract as the HTTP routes
-- (migration 029). Before this, broadcast_session_event() (028) fanned EVERY
-- session_events insert to the public topic session:<id> unconditionally —
-- independent of live_shares — so a never-shared session still streamed, and
-- unshare_session didn't stop new live events. It also shipped the full kernel
-- payload, including raw tool args (loon source / parameters).
--
-- Now: only sessions with an ACTIVE live_shares row broadcast (never-shared →
-- silent; after unshare → no new events), and the broadcast payload drops
-- `args` so viewers get tool/type/changed, not the construction source. The
-- durable session_events row is unchanged — it keeps full args for
-- fold/replay/the Receipt; only the fan-out is slimmed.
--
-- Residual (documented in the share tooling): an already-connected subscriber's
-- websocket is not force-closed on unshare; it simply receives no further
-- events. Force-termination needs a private channel + RLS on realtime.messages,
-- a deferred follow-up.

create or replace function broadcast_session_event()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  -- Private by default: no fan-out unless the session is actively shared.
  if not exists (select 1 from live_shares where session_id = new.session_id) then
    return new;
  end if;
  begin
    perform realtime.send(
      jsonb_build_object(
        'id', new.id,
        'seq', new.seq,
        'session_id', new.session_id,
        'author', new.author,
        'kind', new.kind,
        'type', new.type,
        'payload', (new.payload - 'args'), -- strip raw tool args from the fan-out
        'created_at', new.created_at
      ),
      'session_event',                  -- event name
      'session:' || new.session_id,     -- topic
      false                             -- public topic; share gate is the control
    );
  exception when others then
    null; -- best-effort fan-out; the durable append already succeeded
  end;
  return new;
end;
$$;
