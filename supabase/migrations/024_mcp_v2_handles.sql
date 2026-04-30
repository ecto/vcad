-- MCP v2 — versioned doc handles, short-link table, render cache, telemetry.
--
-- The goal of this migration is to give the v2 MCP server a persistent
-- store for the in-process handle store (`packages/mcp/src/handles.ts`)
-- without breaking the web app. Anonymous docs (created by an unauth'd
-- MCP connection) are TTL'd; authenticated docs land in the user's
-- existing library and `created_via='mcp'` makes them filterable.

-- ── documents: TTL + provenance ────────────────────────────────────
alter table documents
  add column if not exists ttl_at        timestamptz,
  add column if not exists created_via   text,
  add column if not exists source_meta   jsonb;

create index if not exists documents_ttl_at_idx on documents (ttl_at)
  where ttl_at is not null;

-- ── document_versions: content dedup ───────────────────────────────
alter table document_versions
  add column if not exists content_sha256 text;

create index if not exists document_versions_content_sha256_idx
  on document_versions (content_sha256);

-- Anonymous docs are world-readable until expiry; the existing RLS
-- (owner-only) already protects authenticated rows, so we only need
-- a permissive read path for anon.
do $$
begin
  if not exists (
    select 1 from pg_policies
    where schemaname = 'public'
      and tablename = 'documents'
      and policyname = 'documents_anon_read_until_ttl'
  ) then
    create policy documents_anon_read_until_ttl on documents
      for select
      using (
        owner_id is null
        and (ttl_at is null or ttl_at > now())
      );
  end if;
end
$$;

-- ── mcp_short_links: short URLs for `share` ────────────────────────
create table if not exists mcp_short_links (
  short_id          text primary key,
  document_id       uuid not null references documents(id) on delete cascade,
  document_version  int,
  created_at        timestamptz not null default now(),
  expires_at        timestamptz,
  access            text not null default 'view-link',  -- public | view-link | private
  created_by        uuid references auth.users
);
create index if not exists mcp_short_links_document_id_idx
  on mcp_short_links (document_id);

alter table mcp_short_links enable row level security;

do $$
begin
  if not exists (
    select 1 from pg_policies
    where schemaname = 'public'
      and tablename = 'mcp_short_links'
      and policyname = 'mcp_short_links_public_read'
  ) then
    create policy mcp_short_links_public_read on mcp_short_links
      for select
      using (
        access = 'public'
        or access = 'view-link'
        or created_by = auth.uid()
      );
  end if;

  if not exists (
    select 1 from pg_policies
    where schemaname = 'public'
      and tablename = 'mcp_short_links'
      and policyname = 'mcp_short_links_owner_write'
  ) then
    create policy mcp_short_links_owner_write on mcp_short_links
      for all
      using (created_by = auth.uid())
      with check (created_by = auth.uid());
  end if;
end
$$;

-- ── mcp_render_cache: keyed PNGs from the render tool ──────────────
create table if not exists mcp_render_cache (
  cache_key         text primary key,
  document_id       uuid not null references documents(id) on delete cascade,
  document_version  int  not null,
  image_url         text not null,
  byte_size         int  not null,
  created_at        timestamptz not null default now(),
  hit_count         int  not null default 0
);
create index if not exists mcp_render_cache_document_id_idx
  on mcp_render_cache (document_id);

-- ── mcp_tool_calls: rolled-up telemetry ────────────────────────────
create table if not exists mcp_tool_calls (
  id            bigserial primary key,
  ts            timestamptz not null default now(),
  tool          text not null,
  user_id       uuid references auth.users,
  input_doc_id  uuid,
  output_doc_id uuid,
  output_version int,
  elapsed_ms    int,
  status        text,                                   -- ok | error
  error_kind    text,
  bytes_in      int,
  bytes_out     int
);
create index if not exists mcp_tool_calls_ts_idx
  on mcp_tool_calls (ts desc);
create index if not exists mcp_tool_calls_user_ts_idx
  on mcp_tool_calls (user_id, ts desc);

-- ── Sweep helper: drop expired anonymous docs ──────────────────────
-- Schedule this via Supabase scheduled functions (`select cron.schedule(...)`)
-- once the `pg_cron` extension is enabled; until then operators run it
-- manually or via an external job.
create or replace function mcp_sweep_expired_anonymous_docs()
returns int
language plpgsql
security definer
as $$
declare
  removed int;
begin
  delete from documents
  where owner_id is null
    and ttl_at is not null
    and ttl_at < now()
  returning 1 into removed;
  -- `delete returning 1` returns one row per delete; the count we want
  -- is `found`'s row count. PL/pgSQL exposes that via `get diagnostics`.
  get diagnostics removed = ROW_COUNT;
  return removed;
end
$$;

comment on function mcp_sweep_expired_anonymous_docs is
  'Daily sweeper for MCP-v2 anonymous documents past their TTL. Returns rows removed.';
