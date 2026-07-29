-- Durable Tasks-extension records (capability-keyed) — the backing for
-- protocol-2026.ts's task store (packages/mcp/src/task-store.ts).
--
-- WHY: Tasks-extension records (io.modelcontextprotocol/tasks, SEP-2663) live
-- in a process-local Map, so on the serverless deploy a restart made every
-- outstanding taskId report "Unknown taskId" — including tasks that FINISHED
-- before the restart. Terminal records (completed / failed / cancelled) are
-- perfectly serializable, so they persist here and `tasks/get` hydrates from
-- this table on a cache miss. In-flight tasks cannot resume (the in-process
-- bridge dies with the instance) and are reported as failed-by-restart via the
-- boot token embedded in the taskId — no row needed.
--
-- Same model as mcp_artifacts (migration 033): service-role-only table keyed
-- by the unguessable id; possession of the id IS the capability.

create table if not exists mcp_tasks (
  -- The task id ("task_<boot-token>_<random>"). Possession = access.
  task_id text primary key,
  -- The full serialized StoredTask (taskId, status, statusMessage,
  -- timestamps, toolName, result, error). Kept as one blob so the wire shape
  -- round-trips without column drift.
  record jsonb not null,
  created_at timestamptz not null default now(),
  -- TTL, mirrored from the in-memory registry (TASK_TTL_MS, 30 min). Reads
  -- treat an expired row as absent.
  expires_at timestamptz not null
);

create index if not exists mcp_tasks_expires_idx on mcp_tasks (expires_at);

-- RLS on + no policies = deny all to anon/authenticated; service_role
-- bypasses RLS, so only the MCP server ever reads or writes rows.
alter table mcp_tasks enable row level security;

grant all on mcp_tasks to service_role;

-- Tasks are transient by design — sweep expired rows. Callable manually or
-- from a scheduled job (same follow-up wiring as cleanup_expired_mcp_artifacts,
-- migration 033).
create or replace function cleanup_expired_mcp_tasks()
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  swept integer;
begin
  delete from mcp_tasks where expires_at < now();
  get diagnostics swept = row_count;
  return swept;
end;
$$;
