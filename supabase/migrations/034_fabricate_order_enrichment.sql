-- vcad Fabricate — order enrichment for the agent-native factory loop (M2/M4).
--
-- Closes the store.ts round-trip gaps the ordering gates and the order feed
-- need durable:
--   orders.fab_artifact     — the fab-bundle HANDLE ({artifact_id, artifact_url,
--                             bytes, manifest[{file, bytes, sha256}]}). Metadata
--                             only, never bytes — files stay in the artifact
--                             store; the manifest sha256s pin the exact bytes
--                             the fab receives (and kerf's upload-hash oracle
--                             verifies). Fixes the 024-era gap where the handle
--                             survived only in memory + the order_placed event.
--   orders.receipt_status   — design-receipt verdict recorded by place_order's
--                             fail-closed gate at place time. 'stale'/'violated'
--                             never reach a placed order (the gate refuses);
--                             they are enumerated so a later re-verification
--                             sweep can downgrade a stored status.
--   orders.kerf_intent_hash — kerf intent hash the order's quote was bound to:
--                             sha256 of the canonical ConfiguratorIntent
--                             (vendor + process + file sha256s + config + qty).
--                             Geometry/config/quantity edit ⇒ new hash ⇒ the
--                             vendor quote is dead ⇒ place_order refuses.
--   quotes.kerf_intent_hash — the same hash, recorded at quote time.
--   quotes.kerf_job_id      — kerf quote-job id, the handle for job-state and
--                             evidence-bundle lookups on the kerf rail.
--
-- All five columns are SERVER-ONLY writes: the MCP server (service role)
-- records them; clients read their own rows via the existing RLS policies
-- ("Users manage own quotes/orders", migration 024) but have no reason to
-- write them, and no client-side path does. Additive + idempotent — safe to
-- run against a live database; pre-migration servers keep working (the store
-- retries writes without these keys on column skew).

-- ── orders ───────────────────────────────────────────────────────────────────

alter table orders
  add column if not exists fab_artifact jsonb;

alter table orders
  add column if not exists receipt_status text
    check (receipt_status in ('holds', 'stale', 'violated', 'unverified'));

alter table orders
  add column if not exists kerf_intent_hash text;

comment on column orders.fab_artifact is
  'Fab-bundle handle (artifact_id/url/bytes/manifest sha256s) — metadata only, never file bytes. Server-written (place_order / quote_manufacturing).';
comment on column orders.receipt_status is
  'Design-receipt verdict recorded by place_order''s fail-closed gate. Server-written; holds = all clearance claims re-verified at place time, unverified = no claims to check.';
comment on column orders.kerf_intent_hash is
  'kerf intent hash the order''s quote was bound to (geometry-edit tripwire). Server-written.';

-- ── quotes ───────────────────────────────────────────────────────────────────

alter table quotes
  add column if not exists kerf_intent_hash text;

alter table quotes
  add column if not exists kerf_job_id text;

comment on column quotes.kerf_intent_hash is
  'sha256 of the canonical kerf ConfiguratorIntent this quote priced — the identity the vendor quote (and any spend mandate) binds to. Server-written.';
comment on column quotes.kerf_job_id is
  'kerf quote-job id for job-state / evidence-bundle lookups. Server-written.';
