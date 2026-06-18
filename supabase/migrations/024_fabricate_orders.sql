-- vcad Fabricate — Phase 0 (the spine, zero money).
--
-- Lets an agent quote a custom-manufactured part from a live CAD/PCB session
-- and persists the quote + a nascent order so the design→DFM→binding-quote
-- loop is visible end-to-end. NO money moves in Phase 0: the money plane
-- (spend_authorizations, wallets, wallet_ledger, processed_events) and the
-- ordering/payment transitions land in the Phase 1 migration, gated on the
-- three critical fixes (DB-backed revocable authz, idempotent outbox worker,
-- atomic debit RPC).
--
-- Tables:
--   fab_partners — per-fab catalog + due-diligence row (the routing source).
--   quotes       — a priced, DFM-checked cart for one design at a point in time.
--   orders       — the lifecycle row; in Phase 0 it starts (and stays) at
--                  QUOTED. place_order (Phase 1) advances it past AUTHORIZED.
--
-- Money is stored in integer MINOR units (USD cents) — never floats. The
-- per-fab cost (fab_cost_minor) is server-only and NEVER returned to the agent;
-- the agent sees only the margin-inclusive total. Two ledgers the fab can't
-- bridge.

-- ---------------------------------------------------------------------------
-- fab_partners — manufacturer catalog + per-seller due diligence
-- ---------------------------------------------------------------------------

create table if not exists fab_partners (
  id uuid primary key default gen_random_uuid(),
  -- Stable adapter key (matches ManufacturerAdapter.key in code).
  key text not null unique,
  name text not null,
  -- Processes this fab serves: pcb | cnc | 3dprint | sheet_metal | cast_metal.
  processes text[] not null default '{}',
  region text,
  supports_ddp boolean not null default false,
  -- US-only lane flag — controlled/sensitive parts route only to is_us_only fabs.
  is_us_only boolean not null default false,
  -- Per-seller due-diligence (the Paddle/FTC lesson): track dispute rate +
  -- require an insurance cert before a fab is order-eligible (Phase 1 gate).
  dispute_rate numeric not null default 0,
  insurance_cert_url text,
  -- active = advertised for quoting. order_eligible (Phase 1) is a stricter gate.
  active boolean not null default true,
  created_at timestamptz not null default now()
);

-- ---------------------------------------------------------------------------
-- quotes — a priced, DFM-checked cart for one design at one moment
-- ---------------------------------------------------------------------------

create table if not exists quotes (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  -- Client/session document id (text — MCP session ids aren't all uuids).
  document_id text not null,
  -- Hash of the design at quote time. Phase 1 binds this into the spend
  -- authorization so the receipt proves WHAT was ordered.
  doc_hash text,
  process text not null check (
    process in ('pcb', 'cnc', '3dprint', 'sheet_metal', 'cast_metal')
  ),
  material text,
  quantity integer not null check (quantity > 0),
  -- Normalized, margin-INCLUSIVE options shown to the agent (fab cost hidden).
  fab_options jsonb not null default '[]'::jsonb,
  dfm jsonb,
  -- Server-only economics. fab_cost_minor + margin_minor never leave the server.
  fab_cost_minor bigint,
  margin_minor bigint,
  landed_cost jsonb,
  total_amount_minor bigint not null,
  currency text not null default 'USD',
  expires_at timestamptz,
  created_at timestamptz not null default now()
);

create index if not exists quotes_user_created_idx
  on quotes (user_id, created_at desc);

-- ---------------------------------------------------------------------------
-- orders — the lifecycle row (Phase 0: QUOTED only)
-- ---------------------------------------------------------------------------

create table if not exists orders (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  document_id text not null,
  quote_id uuid references quotes(id) on delete set null,
  -- Full lifecycle enumerated now so Phase 1 needs no state migration.
  state text not null default 'QUOTED' check (
    state in (
      'DRAFT', 'QUOTED', 'EXPIRED', 'AUTHORIZED', 'PENDING_PAYMENT',
      'PAYMENT_FAILED', 'PAID', 'SUBMITTED', 'SUBMIT_FAILED', 'RECONCILING',
      'IN_PRODUCTION', 'SHIPPED', 'DELIVERED', 'CANCELED', 'CANCELED_BY_FAB',
      'REFUNDED'
    )
  ),
  fab text,
  fab_order_ref text,
  amount_total_minor bigint,
  -- Server-only; never returned to the agent.
  fab_cost_minor bigint,
  currency text not null default 'USD',
  ship_to jsonb,
  -- Append-only lifecycle timeline: [{state, at, note}].
  events jsonb not null default '[]'::jsonb,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists orders_user_created_idx
  on orders (user_id, created_at desc);

create index if not exists orders_quote_idx
  on orders (quote_id);

-- ---------------------------------------------------------------------------
-- Triggers — touch updated_at (touch_updated_at() defined in migration 014)
-- ---------------------------------------------------------------------------

create trigger orders_touch_updated_at
  before update on orders
  for each row execute function touch_updated_at();

-- ---------------------------------------------------------------------------
-- RLS — users own their quotes/orders; fab_partners is a read-only catalog
-- ---------------------------------------------------------------------------

alter table fab_partners enable row level security;
alter table quotes       enable row level security;
alter table orders       enable row level security;

-- Catalog: any authenticated user may read active partners; only service_role
-- writes (vetting a partner is an operator action, never a client one).
create policy "Anyone reads fab partners"
  on fab_partners for select
  using (true);

create policy "Users manage own quotes"
  on quotes for all
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);

create policy "Users manage own orders"
  on orders for all
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);

grant all on fab_partners, quotes, orders to service_role;
grant usage, select on all sequences in schema public to service_role;

-- ---------------------------------------------------------------------------
-- Seed the launch fab catalog (idempotent)
-- ---------------------------------------------------------------------------

insert into fab_partners (key, name, processes, region, supports_ddp, is_us_only, active)
values
  ('jlcpcb',      'JLCPCB',       array['pcb','3dprint'], 'CN', true,  false, true),
  ('digitalmetal','Digital Metal', array['cast_metal'],    'US', true,  true,  true)
on conflict (key) do update set
  name = excluded.name,
  processes = excluded.processes,
  region = excluded.region,
  supports_ddp = excluded.supports_ddp,
  is_us_only = excluded.is_us_only,
  active = excluded.active;
