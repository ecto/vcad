-- vcad Fabricate — Phase 1 money plane (schema + atomic primitives only).
--
-- Lands the DURABLE, REVOCABLE money primitives the adversarial review flagged
-- as must-fix, with NO tool yet wired to spend. Nothing here charges anyone —
-- place_order / authorize_spend / Stripe land in a later slice (test-mode +
-- flag-gated first).
--
-- Guardrails encoded here:
--   1. Spend authorizations are DB rows with a real lifecycle
--      (pending_human → authorized → consumed/revoked/expired) + a kill switch.
--      A signed JWT (app-side) is only a pointer; this table is the truth, and
--      EVERY field (max_amount, daily_cap, process/fab allowlist, doc_hash,
--      quote binding, expiry, ownership) is enforced in debit_wallet().
--   2. wallet_ledger is the append-only source of truth; wallets.balance is a
--      cached mirror. Wallet writes happen ONLY inside the SECURITY DEFINER
--      RPCs — service_role gets SELECT only on wallets/ledger.
--   3. debit_wallet() is one atomic, balance-floored, per-user-idempotent txn
--      (advisory-lock serialized on (user, key)) that also consumes a one_time
--      authorization — no double-spend, no replay double-charge, no cross-user
--      key collision.
--
-- Isolation: correctness assumes the default READ COMMITTED (FOR UPDATE
-- re-reads the latest committed row). A caller that bumps to REPEATABLE READ /
-- SERIALIZABLE must treat SQLSTATE 40001 as a retry, not a debit failure.
--
-- Money is integer MINOR units (USD cents) everywhere.

-- ─── wallets — cached balance mirror (source of truth is wallet_ledger) ───────
create table if not exists wallets (
  user_id uuid primary key references auth.users(id) on delete cascade,
  credit_balance_minor bigint not null default 0 check (credit_balance_minor >= 0),
  currency text not null default 'USD',
  updated_at timestamptz not null default now()
);

-- ─── wallet_ledger — append-only; balance = sum(delta_minor) ──────────────────
create table if not exists wallet_ledger (
  id bigserial primary key,
  user_id uuid not null references auth.users(id) on delete cascade,
  -- Signed: +topup / +refund, -order_debit / -metered_api.
  delta_minor bigint not null,
  reason text not null check (
    reason in ('topup', 'order_debit', 'refund', 'metered_api', 'adjustment')
  ),
  order_id uuid references orders(id) on delete set null,
  -- Idempotency key, namespaced PER USER (not global) so one tenant's key can
  -- never collide with another's.
  idempotency_key text not null,
  -- Snapshot of the resulting balance for audit / reconciliation / replay.
  balance_after_minor bigint not null,
  created_at timestamptz not null default now(),
  unique (user_id, idempotency_key)
);

create index if not exists wallet_ledger_user_idx
  on wallet_ledger (user_id, created_at desc);

-- ─── spend_authorizations — DB-backed, revocable (NOT a stateless JWT) ────────
create table if not exists spend_authorizations (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  -- null for a standing budget; required for a one_time quote authorization.
  quote_id uuid references quotes(id) on delete cascade,
  kind text not null check (kind in ('one_time', 'standing')),
  max_amount_minor bigint not null check (max_amount_minor > 0),
  daily_cap_minor bigint check (daily_cap_minor is null or daily_cap_minor > 0),
  process_allowlist text[],
  fab_allowlist text[],
  doc_hash text,
  status text not null default 'pending_human' check (
    status in ('pending_human', 'authorized', 'consumed', 'revoked', 'expired')
  ),
  approved_by uuid references auth.users(id),
  approved_at timestamptz,
  revoked_at timestamptz,
  consumed_at timestamptz,
  expires_at timestamptz not null,
  created_at timestamptz not null default now(),
  -- A one_time authorization must bind to a specific quote.
  check (kind <> 'one_time' or quote_id is not null)
);

create index if not exists spend_authorizations_user_idx
  on spend_authorizations (user_id, created_at desc);

-- ledger ← authorization link (added after spend_authorizations exists so the
-- FK resolves regardless of table order). Enables per-authorization daily-cap
-- accounting + audit (which authz funded which debit).
alter table wallet_ledger
  add column if not exists authorization_id uuid references spend_authorizations(id) on delete set null;

-- ─── processed_events — durable webhook idempotency (NOT an in-memory Map) ────
create table if not exists processed_events (
  event_id text primary key,
  source text not null default 'stripe',
  created_at timestamptz not null default now()
);

-- ─── orders: money-plane columns (table from migration 024) ──────────────────
alter table orders add column if not exists authorization_id uuid references spend_authorizations(id);
alter table orders add column if not exists idempotency_key text;
alter table orders add column if not exists stripe_payment_intent_id text;
-- One order per quote — server-derivable double-order guard. Phase-0 already
-- writes exactly one order per (fresh) quote_id, so this builds cleanly.
create unique index if not exists orders_quote_unique on orders (quote_id) where quote_id is not null;

-- ─── touch_updated_at (defined in migration 014); re-runnable ────────────────
drop trigger if exists wallets_touch_updated_at on wallets;
create trigger wallets_touch_updated_at
  before update on wallets
  for each row execute function touch_updated_at();

-- ─── guard: money RPCs are server-only, even if a future migration re-grants ─
-- Blocks a PostgREST 'authenticated'/'anon' caller; allows service_role and
-- internal (no-JWT) contexts like migrations / pg_cron.
create or replace function assert_money_caller()
returns void
language plpgsql
security definer
set search_path = public
as $$
declare
  v_claims text := current_setting('request.jwt.claims', true);
begin
  if v_claims is not null
     and (v_claims::jsonb ->> 'role') in ('authenticated', 'anon') then
    raise exception 'forbidden: money functions are service-role only';
  end if;
end;
$$;

-- Per-call sanity ceiling so a fat-fingered/abusive amount can't move $millions.
-- ($1,000,000 in minor units.)

-- ─── debit_wallet() — atomic, balance-floored, idempotent, authz-consuming ───
-- The ONLY way credits leave a wallet. Returns jsonb:
--   { ok:true,  balance_minor, idempotent? }   on success / replay
--   { ok:false, reason, balance_minor? }        on a guarded failure (no raise)
-- Signature verification of the authorization JWT happens APP-SIDE; here we
-- enforce DB-state truth and every authz field.
create or replace function debit_wallet(
  p_user uuid,
  p_amount_minor bigint,
  p_order_id uuid,
  p_authorization_id uuid,
  p_idempotency_key text
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  c_max_minor constant bigint := 100000000; -- $1M ceiling
  v_authz spend_authorizations%rowtype;
  v_existing wallet_ledger%rowtype;
  v_balance bigint;
  v_spent_today bigint;
  v_fab text;
  v_process text;
  v_order_quote uuid;
  v_quote_hash text;
begin
  perform assert_money_caller();

  if p_amount_minor is null or p_amount_minor <= 0 then
    return jsonb_build_object('ok', false, 'reason', 'invalid_amount');
  end if;
  if p_amount_minor > c_max_minor then
    return jsonb_build_object('ok', false, 'reason', 'amount_over_ceiling');
  end if;
  if p_idempotency_key is null or length(p_idempotency_key) = 0 then
    return jsonb_build_object('ok', false, 'reason', 'missing_idempotency_key');
  end if;

  -- Serialize same-(user,key) calls so the idempotency check-then-insert is
  -- race-free (the loser sees the winner's committed row, not a unique error).
  perform pg_advisory_xact_lock(hashtext(p_user::text || ':' || p_idempotency_key));

  -- Idempotent replay (per-user key). Must match the original request.
  select * into v_existing
    from wallet_ledger
    where user_id = p_user and idempotency_key = p_idempotency_key;
  if found then
    if v_existing.reason <> 'order_debit'
       or v_existing.order_id is distinct from p_order_id
       or v_existing.authorization_id is distinct from p_authorization_id
       or v_existing.delta_minor <> -p_amount_minor then
      return jsonb_build_object('ok', false, 'reason', 'idempotency_key_reused');
    end if;
    return jsonb_build_object('ok', true, 'idempotent', true,
                             'balance_minor', v_existing.balance_after_minor);
  end if;

  -- Lock + validate the authorization against DB truth.
  select * into v_authz from spend_authorizations where id = p_authorization_id for update;
  if not found or v_authz.user_id <> p_user then
    return jsonb_build_object('ok', false, 'reason', 'authz_not_found');
  end if;
  if v_authz.revoked_at is not null or v_authz.status = 'revoked' then
    return jsonb_build_object('ok', false, 'reason', 'authz_revoked');
  end if;
  if v_authz.expires_at <= now() then
    return jsonb_build_object('ok', false, 'reason', 'authz_expired');
  end if;
  if v_authz.status <> 'authorized' then
    return jsonb_build_object('ok', false, 'reason', 'authz_not_authorized');
  end if;
  if p_amount_minor > v_authz.max_amount_minor then
    return jsonb_build_object('ok', false, 'reason', 'amount_exceeds_authz');
  end if;

  -- Fetch the order (ownership-scoped) + its quote for binding/allowlist checks.
  select o.fab, o.quote_id, q.process, q.doc_hash
    into v_fab, v_order_quote, v_process, v_quote_hash
    from orders o
    left join quotes q on q.id = o.quote_id
    where o.id = p_order_id and o.user_id = p_user;
  if not found then
    return jsonb_build_object('ok', false, 'reason', 'order_not_found');
  end if;

  if v_authz.kind = 'one_time' and v_order_quote is distinct from v_authz.quote_id then
    return jsonb_build_object('ok', false, 'reason', 'authz_quote_mismatch');
  end if;
  if v_authz.doc_hash is not null and v_quote_hash is distinct from v_authz.doc_hash then
    return jsonb_build_object('ok', false, 'reason', 'doc_hash_mismatch');
  end if;
  if v_authz.fab_allowlist is not null
     and not (v_fab = any(v_authz.fab_allowlist)) then
    return jsonb_build_object('ok', false, 'reason', 'fab_not_allowed');
  end if;
  if v_authz.process_allowlist is not null
     and not (v_process = any(v_authz.process_allowlist)) then
    return jsonb_build_object('ok', false, 'reason', 'process_not_allowed');
  end if;

  -- Daily cumulative cap (standing budgets).
  if v_authz.daily_cap_minor is not null then
    select coalesce(sum(-delta_minor), 0) into v_spent_today
      from wallet_ledger
      where authorization_id = v_authz.id and reason = 'order_debit'
        and created_at >= date_trunc('day', now() at time zone 'UTC');
    if v_spent_today + p_amount_minor > v_authz.daily_cap_minor then
      return jsonb_build_object('ok', false, 'reason', 'daily_cap_exceeded');
    end if;
  end if;

  -- Lock the wallet and enforce the balance floor.
  select credit_balance_minor into v_balance from wallets where user_id = p_user for update;
  if not found then
    v_balance := 0;
  end if;
  if v_balance < p_amount_minor then
    return jsonb_build_object('ok', false, 'reason', 'insufficient_funds', 'balance_minor', v_balance);
  end if;

  v_balance := v_balance - p_amount_minor;

  insert into wallet_ledger (user_id, delta_minor, reason, order_id, authorization_id, idempotency_key, balance_after_minor)
    values (p_user, -p_amount_minor, 'order_debit', p_order_id, p_authorization_id, p_idempotency_key, v_balance);

  update wallets set credit_balance_minor = v_balance where user_id = p_user;

  -- Consume one_time authorizations; standing budgets stay 'authorized' and are
  -- bounded by daily_cap + expiry.
  if v_authz.kind = 'one_time' then
    update spend_authorizations set status = 'consumed', consumed_at = now() where id = v_authz.id;
  end if;

  return jsonb_build_object('ok', true, 'balance_minor', v_balance);
end;
$$;

-- ─── credit_wallet() — top-ups + refunds (per-user idempotent) ───────────────
create or replace function credit_wallet(
  p_user uuid,
  p_amount_minor bigint,
  p_reason text,
  p_idempotency_key text,
  p_order_id uuid default null
)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  c_max_minor constant bigint := 100000000; -- $1M ceiling
  v_existing wallet_ledger%rowtype;
  v_balance bigint;
begin
  perform assert_money_caller();

  if p_amount_minor is null or p_amount_minor <= 0 then
    return jsonb_build_object('ok', false, 'reason', 'invalid_amount');
  end if;
  if p_amount_minor > c_max_minor then
    return jsonb_build_object('ok', false, 'reason', 'amount_over_ceiling');
  end if;
  if p_reason not in ('topup', 'refund', 'adjustment') then
    return jsonb_build_object('ok', false, 'reason', 'invalid_reason');
  end if;
  if p_reason = 'refund' and p_order_id is null then
    return jsonb_build_object('ok', false, 'reason', 'refund_requires_order');
  end if;
  if p_idempotency_key is null or length(p_idempotency_key) = 0 then
    return jsonb_build_object('ok', false, 'reason', 'missing_idempotency_key');
  end if;

  perform pg_advisory_xact_lock(hashtext(p_user::text || ':' || p_idempotency_key));

  select * into v_existing
    from wallet_ledger
    where user_id = p_user and idempotency_key = p_idempotency_key;
  if found then
    if v_existing.reason <> p_reason
       or v_existing.order_id is distinct from p_order_id
       or v_existing.delta_minor <> p_amount_minor then
      return jsonb_build_object('ok', false, 'reason', 'idempotency_key_reused');
    end if;
    return jsonb_build_object('ok', true, 'idempotent', true,
                             'balance_minor', v_existing.balance_after_minor);
  end if;

  insert into wallets (user_id, credit_balance_minor) values (p_user, 0)
    on conflict (user_id) do nothing;

  select credit_balance_minor into v_balance from wallets where user_id = p_user for update;
  v_balance := coalesce(v_balance, 0) + p_amount_minor;

  insert into wallet_ledger (user_id, delta_minor, reason, order_id, idempotency_key, balance_after_minor)
    values (p_user, p_amount_minor, p_reason, p_order_id, p_idempotency_key, v_balance);

  update wallets set credit_balance_minor = v_balance where user_id = p_user;

  return jsonb_build_object('ok', true, 'balance_minor', v_balance);
end;
$$;

-- ─── revoke_user_authorizations() — kill switch ──────────────────────────────
create or replace function revoke_user_authorizations(p_user uuid)
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  n integer;
begin
  perform assert_money_caller();
  update spend_authorizations
    set status = 'revoked', revoked_at = now()
    where user_id = p_user and status in ('pending_human', 'authorized');
  get diagnostics n = row_count;
  return n;
end;
$$;

-- ─── fn_wallet_drift() — reconciliation: cached balance vs ledger sum ─────────
create or replace function fn_wallet_drift()
returns table (user_id uuid, cached_minor bigint, ledger_minor bigint)
language sql
security definer
set search_path = public
as $$
  select w.user_id,
         w.credit_balance_minor,
         coalesce((select sum(l.delta_minor) from wallet_ledger l where l.user_id = w.user_id), 0)
  from wallets w
  where w.credit_balance_minor
        <> coalesce((select sum(l.delta_minor) from wallet_ledger l where l.user_id = w.user_id), 0);
$$;

-- ─── RLS — users read their own money rows; ALL writes via the RPCs ───────────
alter table wallets               enable row level security;
alter table wallet_ledger         enable row level security;
alter table spend_authorizations  enable row level security;
alter table processed_events      enable row level security;

drop policy if exists "Users read own wallet" on wallets;
drop policy if exists "Users read own ledger" on wallet_ledger;
drop policy if exists "Users read own authorizations" on spend_authorizations;
create policy "Users read own wallet"         on wallets              for select using (auth.uid() = user_id);
create policy "Users read own ledger"         on wallet_ledger        for select using (auth.uid() = user_id);
create policy "Users read own authorizations" on spend_authorizations for select using (auth.uid() = user_id);
-- processed_events: no policies → only service_role (bypasses RLS) touches it.

-- wallets + ledger: service_role gets SELECT only — the SECURITY DEFINER RPCs
-- (running as owner) are the SOLE writers, so even a leaked service key can't
-- rewrite a balance outside the audited RPC path.
grant select on wallets, wallet_ledger to service_role;
-- authorizations + events are server-managed directly (mint/approve/dedupe).
grant all on spend_authorizations, processed_events to service_role;
grant usage, select on all sequences in schema public to service_role;

-- Money-moving RPCs are server-only. Never grant execute to authenticated/anon.
revoke all on function debit_wallet(uuid, bigint, uuid, uuid, text) from public;
revoke all on function credit_wallet(uuid, bigint, text, text, uuid) from public;
revoke all on function revoke_user_authorizations(uuid) from public;
revoke all on function fn_wallet_drift() from public;
grant execute on function debit_wallet(uuid, bigint, uuid, uuid, text) to service_role;
grant execute on function credit_wallet(uuid, bigint, text, text, uuid) to service_role;
grant execute on function revoke_user_authorizations(uuid) to service_role;
grant execute on function fn_wallet_drift() to service_role;
