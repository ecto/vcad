-- Per-user durable storage for the agent loon-macro library.
--
-- WHY: macros defined via define_loon live in a process-warm Map plus local
-- JSON files (packages/mcp/src/tools/loon-macros.ts) — on the serverless
-- deploy a macro defined on one instance is invisible everywhere else, and
-- dies with the instance. Same failure class — and same fix — as sessions
-- (SupabaseSessionStore) and artifacts (migration 033): a durable table the
-- MCP server hydrates on miss. Unlike artifacts, macros are user-scoped and
-- permanent (a library, not a cache): keyed (user_id, name), no TTL.
--
-- The MCP server writes with the service role (bypasses RLS) always scoping
-- user_id to the verified caller; RLS mirrors `documents` so a future
-- signed-in web UI can read/manage the user's own library directly.

create table if not exists mcp_macros (
  user_id uuid not null references auth.users (id) on delete cascade,
  -- kebab-case macro name; also the loon function the source defines.
  name text not null,
  -- Monotone version, bumped by the server on redefinition.
  version integer not null default 1,
  description text not null default '',
  -- [{name, description?, example, unit?}] — ordered parameter docs.
  params jsonb not null default '[]'::jsonb,
  -- The loon source: [let <name> [fn [params...] ...]] (+ helpers).
  source text not null,
  -- Reserved for the certify_loon rung: a DesignReceipt (vcad.receipt/1)
  -- whose claims cover the macro's parameter range at verify tier. Null =
  -- uncertified (smoke-tested only).
  receipt jsonb,
  updated_at timestamptz not null default now(),
  primary key (user_id, name)
);

alter table mcp_macros enable row level security;

create policy "Users can view their own macros" on mcp_macros
  for select using (auth.uid() = user_id);

create policy "Users can insert their own macros" on mcp_macros
  for insert with check (auth.uid() = user_id);

create policy "Users can update their own macros" on mcp_macros
  for update using (auth.uid() = user_id);

create policy "Users can delete their own macros" on mcp_macros
  for delete using (auth.uid() = user_id);
