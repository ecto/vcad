-- Durable MCP sessions for ANONYMOUS callers (capability-keyed).
--
-- Authenticated MCP sessions persist to the user-owned `documents` table
-- (SupabaseSessionStore, migration 001+). Anonymous callers have no user_id, so
-- their open documents lived only in a per-instance in-memory map and were lost
-- on a Vercel cold start or cross-instance routing ("Unknown document_id").
--
-- This table gives anonymous sessions durable, capability-scoped storage: the
-- unguessable document_id (random 72-bit suffix from nextSessionId) IS the
-- capability. RLS is enabled with NO anon/authenticated policies, so only the
-- MCP server (service role, which bypasses RLS) ever reads or writes — clients
-- cannot enumerate or read sessions directly.

create table if not exists mcp_sessions (
  -- The session/document id (text — MCP ids aren't uuids). Possession = access.
  document_id text primary key,
  -- Raw Document IR (same shape SupabaseSessionStore stores in documents.content).
  content jsonb not null,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists mcp_sessions_updated_idx on mcp_sessions (updated_at);

-- touch_updated_at() is defined in migration 014.
create trigger mcp_sessions_touch_updated_at
  before update on mcp_sessions
  for each row execute function touch_updated_at();

-- RLS on + no policies = deny all to anon/authenticated; service_role bypasses
-- RLS, so only the server touches this table.
alter table mcp_sessions enable row level security;

grant all on mcp_sessions to service_role;

-- Anonymous sessions are transient — sweep stale rows. Cron wiring is a
-- follow-up; callable manually or from a scheduled job for now.
create or replace function cleanup_stale_mcp_sessions(max_age interval default '7 days')
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  swept integer;
begin
  delete from mcp_sessions where updated_at < now() - max_age;
  get diagnostics swept = row_count;
  return swept;
end;
$$;
