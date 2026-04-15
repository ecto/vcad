-- One-time backfill: populate usage_periods for the current calendar month
-- from inference_logs. Pre-007 rows only carry the aggregate `tokens` column,
-- so we put the whole sum into output_tokens. input + output = total either
-- way, which is all the rate-limit check reads; going forward
-- record_chat_usage will split the two correctly.
--
-- Idempotent via ON CONFLICT — safe to re-run if the migration is replayed
-- against a database that already has the backfilled rows.

insert into usage_periods (
  user_id, period_start, period_end, tier,
  input_tokens, output_tokens, message_count, updated_at
)
select
  user_id,
  date_trunc('month', now() at time zone 'utc') as period_start,
  (date_trunc('month', now() at time zone 'utc') + interval '1 month') as period_end,
  'free' as tier,
  coalesce(sum(input_tokens), 0) as input_tokens,
  coalesce(sum(coalesce(output_tokens, tokens)), 0) as output_tokens,
  count(*) as message_count,
  now()
from inference_logs
where kind = 'chat'
  and user_id is not null
  and created_at >= date_trunc('month', now() at time zone 'utc')
group by user_id
on conflict (user_id, period_start) do update
  set input_tokens  = excluded.input_tokens,
      output_tokens = excluded.output_tokens,
      message_count = excluded.message_count,
      updated_at    = now();
