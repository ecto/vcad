-- Billing + usage metering for vcad chat.
--
-- Design:
--  * subscriptions: one row per paying user, mirrors Stripe subscription state.
--    Free users have no row.
--  * usage_periods: denormalized token counter keyed by (user_id, period_start).
--    One row per billing period per user; upserted atomically by record_chat_usage.
--  * Paid users' periods align to Stripe current_period_start/end (not calendar
--    months), so upgrading mid-month starts a fresh window on the billing cycle.
--  * Free users' periods align to the calendar month UTC.
--
-- Rate limiting is a single indexed PK lookup on usage_periods instead of
-- scanning inference_logs (previous approach was O(N) per chat request).

-- ---------------------------------------------------------------------------
-- subscriptions
-- ---------------------------------------------------------------------------

create table if not exists subscriptions (
  user_id uuid primary key references auth.users(id) on delete cascade,
  stripe_customer_id text not null,
  stripe_subscription_id text,
  tier text not null default 'free'
    check (tier in ('free', 'pro', 'max')),
  status text not null default 'active'
    check (status in (
      'active', 'trialing', 'past_due', 'canceled',
      'incomplete', 'incomplete_expired', 'unpaid', 'paused'
    )),
  current_period_start timestamptz,
  current_period_end timestamptz,
  cancel_at_period_end boolean not null default false,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create unique index if not exists subscriptions_stripe_customer_id_idx
  on subscriptions (stripe_customer_id);

create index if not exists subscriptions_stripe_subscription_id_idx
  on subscriptions (stripe_subscription_id)
  where stripe_subscription_id is not null;

alter table subscriptions enable row level security;

create policy "Users can view own subscription"
  on subscriptions for select
  using (auth.uid() = user_id);

grant select, insert, update, delete on subscriptions to service_role;

create or replace function touch_subscriptions_updated_at()
returns trigger as $$
begin
  new.updated_at = now();
  return new;
end;
$$ language plpgsql;

drop trigger if exists subscriptions_touch_updated_at on subscriptions;
create trigger subscriptions_touch_updated_at
  before update on subscriptions
  for each row execute function touch_subscriptions_updated_at();

-- ---------------------------------------------------------------------------
-- usage_periods — denormalized counter, one row per user per period
-- ---------------------------------------------------------------------------

create table if not exists usage_periods (
  user_id uuid not null references auth.users(id) on delete cascade,
  period_start timestamptz not null,
  period_end timestamptz not null,
  tier text not null,
  input_tokens bigint not null default 0,
  output_tokens bigint not null default 0,
  message_count integer not null default 0,
  updated_at timestamptz not null default now(),
  primary key (user_id, period_start)
);

create index if not exists usage_periods_user_period_end_idx
  on usage_periods (user_id, period_end desc);

alter table usage_periods enable row level security;

create policy "Users can view own usage"
  on usage_periods for select
  using (auth.uid() = user_id);

grant select, insert, update on usage_periods to service_role;

-- Atomic upsert: inserts a new row for the period or increments the existing
-- one. Called once per successful chat stream. Running under service_role via
-- security definer so RLS doesn't block the write.
create or replace function record_chat_usage(
  p_user_id uuid,
  p_period_start timestamptz,
  p_period_end timestamptz,
  p_tier text,
  p_input_tokens bigint,
  p_output_tokens bigint
) returns usage_periods as $$
declare
  r usage_periods;
begin
  insert into usage_periods (
    user_id, period_start, period_end, tier,
    input_tokens, output_tokens, message_count, updated_at
  )
  values (
    p_user_id, p_period_start, p_period_end, p_tier,
    p_input_tokens, p_output_tokens, 1, now()
  )
  on conflict (user_id, period_start) do update
    set input_tokens  = usage_periods.input_tokens  + excluded.input_tokens,
        output_tokens = usage_periods.output_tokens + excluded.output_tokens,
        message_count = usage_periods.message_count + 1,
        period_end    = greatest(usage_periods.period_end, excluded.period_end),
        tier          = excluded.tier,
        updated_at    = now()
  returning * into r;
  return r;
end;
$$ language plpgsql security definer;

revoke all on function record_chat_usage(uuid, timestamptz, timestamptz, text, bigint, bigint) from public;
grant execute on function record_chat_usage(uuid, timestamptz, timestamptz, text, bigint, bigint) to service_role;

-- ---------------------------------------------------------------------------
-- inference_logs: split tokens into input/output for accurate cost accounting
-- ---------------------------------------------------------------------------

alter table inference_logs add column if not exists input_tokens integer;
alter table inference_logs add column if not exists output_tokens integer;
