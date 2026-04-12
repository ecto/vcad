-- Extend inference_logs to track chat usage alongside generate-ir usage.
-- Supports both authenticated users (tracked by user_id, monthly token budget)
-- and anonymous users (tracked by hashed IP, daily message count).

-- 1. Allow user_id to be NULL for anonymous chats.
alter table inference_logs
  alter column user_id drop not null;

-- 2. Add kind column to distinguish chat vs generate-ir usage.
-- Existing rows were all generate-ir so default accordingly.
alter table inference_logs
  add column if not exists kind text not null default 'generate-ir';

-- 3. Hashed IP (SHA-256 of IP + salt) for anonymous rate limiting.
alter table inference_logs
  add column if not exists ip_hash text;

-- 4. Tool call count — agentic chat runs can produce many tool calls per message.
alter table inference_logs
  add column if not exists tool_calls integer;

-- 5. At least one of user_id or ip_hash must be set (we know who made the request).
alter table inference_logs
  add constraint inference_logs_user_or_ip_check
  check (user_id is not null or ip_hash is not null);

-- 6. Index for anon rate limiting: count rows for a given ip_hash in the last 24h.
create index if not exists inference_logs_ip_hash_kind_created_at_idx
  on inference_logs (ip_hash, kind, created_at desc)
  where ip_hash is not null;

-- 7. Index for monthly token budget: sum tokens for a user in the current month.
create index if not exists inference_logs_user_kind_created_at_idx
  on inference_logs (user_id, kind, created_at desc)
  where user_id is not null;

-- 8. RLS: allow the service role full access for insertion from the API.
-- The existing "Service role can insert inference logs" policy already covers this.

-- 9. RLS policy for users to see only their own logs already exists and still applies
-- because anonymous rows have user_id = null, so auth.uid() = user_id is false.
