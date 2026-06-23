-- live_shares — explicit, revocable per-session sharing for the live window.
--
-- Sessions are PRIVATE by default. The /live/<id>/* routes (events replay,
-- geometry, annotations, the viewer page) are gated on a row here: no row →
-- 404, even when the global VCAD_LIVE_WINDOW flag is on and the caller has a
-- valid (unguessable) session id. The driver opts in explicitly via the
-- share_session MCP tool (which warns that the link is public) and can revoke
-- with unshare_session. This narrows the capability surface: a leaked or
-- guessed session id is inert unless the session was deliberately shared.
--
-- Keyed by session_id (the MCP document/session id, text — not a uuid), so one
-- table covers both anonymous (mcp_sessions) and signed-in (documents) sessions
-- uniformly. RLS on with no anon/authenticated policies → only the server
-- (service role, which bypasses RLS) reads/writes; clients can't enumerate it.

create table if not exists live_shares (
  session_id text primary key,
  -- The driver who shared it (the documents/mcp owner), or null for an
  -- anonymous capability session. Audit only — the gate is presence of the row.
  shared_by uuid references auth.users(id) on delete set null,
  created_at timestamptz not null default now()
);

create index if not exists live_shares_created_idx on live_shares (created_at);

alter table live_shares enable row level security;

grant all on live_shares to service_role;

-- Stale-share sweep — a shared link shouldn't live forever. Cron wiring is a
-- follow-up; callable manually or from a scheduled job for now.
create or replace function cleanup_stale_live_shares(max_age interval default '30 days')
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  swept integer;
begin
  delete from live_shares where created_at < now() - max_age;
  get diagnostics swept = row_count;
  return swept;
end;
$$;

revoke all on function cleanup_stale_live_shares(interval) from public;
grant execute on function cleanup_stale_live_shares(interval) to service_role;
