-- Runtime tool-pack switching (issue #432).
--
-- The MCP server can enable/disable optional tool packs at runtime via
-- `set_tool_packs`. The hosted HTTP transport is stateless (a fresh Server per
-- request), so the choice can't live in process memory — it's persisted here,
-- keyed by the authenticated user, and re-read at the top of the next request.
--
-- One row per user; `packs` is the enabled-pack list (empty = core only). The
-- server writes with the service-role key, so RLS is enabled with no policies
-- (service role bypasses RLS; no anon/authenticated access is granted).

create table if not exists public.mcp_tool_packs (
  user_id    text primary key,
  packs      text[] not null default '{}',
  updated_at timestamptz not null default now()
);

alter table public.mcp_tool_packs enable row level security;
