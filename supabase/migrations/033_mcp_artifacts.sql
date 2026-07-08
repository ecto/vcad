-- Durable MCP artifact store (capability-keyed) — the backing for the
-- /artifacts/* channel that keeps large export/fab bundles out of model
-- context.
--
-- WHY: artifacts lived only in a per-instance in-memory registry
-- (packages/mcp/src/tools/artifact-store.ts), so on the serverless deploy a
-- handle minted by export_gerber on one instance was unreadable everywhere
-- else — the artifact_url 404'd from any other instance and
-- quote_manufacturing rejected a fab_artifact_id minted minutes earlier
-- ("Unknown or expired"). Same failure class - and same fix - as anonymous
-- sessions (migration 025): a service-role-only table keyed by the
-- unguessable id; possession of the id IS the capability.

create table if not exists mcp_artifacts (
  -- The artifact id ("art_<random>", 96-bit suffix). Possession = access.
  artifact_id text primary key,
  -- Total bundle size across all files (pre-base64 bytes).
  bytes bigint not null,
  -- [{file, bytes, sha256}] — the verification manifest the tool returned.
  manifest jsonb not null,
  -- [{name, content_type, b64}] — file contents, base64-encoded (jsonb can't
  -- hold raw bytes). Bundles are bounded by the tools that produce them
  -- (Gerber sets / meshes, ~100 KB–10 MB), well inside row limits.
  files jsonb not null,
  created_at timestamptz not null default now(),
  -- TTL, mirrored from the in-memory registry (MCP_ARTIFACT_TTL_MS, 24 h
  -- default). Reads treat an expired row as absent and delete it.
  expires_at timestamptz not null
);

create index if not exists mcp_artifacts_expires_idx on mcp_artifacts (expires_at);

-- RLS on + no policies = deny all to anon/authenticated; service_role
-- bypasses RLS, so only the MCP server ever reads or writes rows — clients
-- cannot enumerate or read artifacts except through the /artifacts route,
-- which requires the unguessable id.
alter table mcp_artifacts enable row level security;

grant all on mcp_artifacts to service_role;

-- Artifacts are transient by design — sweep expired rows. Reads already
-- delete-on-expiry lazily; this catches never-read rows. Callable manually
-- or from a scheduled job (same follow-up wiring as
-- cleanup_stale_mcp_sessions, migration 025).
create or replace function cleanup_expired_mcp_artifacts()
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  swept integer;
begin
  delete from mcp_artifacts where expires_at < now();
  get diagnostics swept = row_count;
  return swept;
end;
$$;
