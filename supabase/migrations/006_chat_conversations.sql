-- Chat conversation storage for SFT / training data.
-- Anonymous conversations are always stored (required for the free tier).
-- Authenticated conversations are stored by default but users can opt out
-- via user_preferences.share_chat_conversations.

-- 1. Per-user preferences (lightweight, keyed on auth.users).
create table if not exists user_preferences (
  user_id uuid primary key references auth.users(id) on delete cascade,
  share_chat_conversations boolean not null default true,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

alter table user_preferences enable row level security;

create policy "Users can view own preferences"
  on user_preferences for select
  using (auth.uid() = user_id);

create policy "Users can upsert own preferences"
  on user_preferences for insert
  with check (auth.uid() = user_id);

create policy "Users can update own preferences"
  on user_preferences for update
  using (auth.uid() = user_id);

-- 2. Full chat conversation log (SFT / training corpus).
create table if not exists chat_conversations (
  id uuid primary key default gen_random_uuid(),
  user_id uuid references auth.users(id) on delete cascade,
  ip_hash text,
  -- JSONB message array as sent to the model: [{role, content}, ...]
  -- content may be a string or an array of content blocks (text, tool_use, tool_result)
  messages jsonb not null,
  -- Tool schemas (AnthropicTool[]) at the time of the request
  tools jsonb,
  -- SHA-256 hash of the system prompt so we can group by prompt version
  system_prompt_hash text,
  -- Aggregate stats
  tokens integer,
  tool_call_count integer,
  duration_ms integer,
  -- Safety classifier output
  safety_verdict text not null default 'safe',  -- 'safe' | 'flagged' | 'error'
  safety_reason text,
  -- Opt-out flag (always true for anon; respects user_preferences for logged-in)
  consented boolean not null default true,
  created_at timestamptz not null default now(),

  constraint chat_conversations_user_or_ip_check
    check (user_id is not null or ip_hash is not null),
  constraint chat_conversations_verdict_check
    check (safety_verdict in ('safe', 'flagged', 'error'))
);

-- Indexes for common query patterns
create index if not exists chat_conversations_user_id_idx
  on chat_conversations (user_id, created_at desc)
  where user_id is not null;

create index if not exists chat_conversations_ip_hash_idx
  on chat_conversations (ip_hash, created_at desc)
  where ip_hash is not null;

create index if not exists chat_conversations_verdict_idx
  on chat_conversations (safety_verdict, created_at desc);

-- RLS: users can see and delete their own conversations (GDPR-friendly).
alter table chat_conversations enable row level security;

create policy "Users can view own conversations"
  on chat_conversations for select
  using (auth.uid() = user_id);

create policy "Users can delete own conversations"
  on chat_conversations for delete
  using (auth.uid() = user_id);

-- Only the service role (API routes) writes rows.
create policy "Service role can insert conversations"
  on chat_conversations for insert
  with check (true);

grant insert on chat_conversations to service_role;
grant insert, select, update on user_preferences to service_role;

-- Touch updated_at on every update of user_preferences.
create or replace function touch_user_preferences_updated_at()
returns trigger as $$
begin
  new.updated_at = now();
  return new;
end;
$$ language plpgsql;

create trigger user_preferences_touch_updated_at
  before update on user_preferences
  for each row
  execute function touch_user_preferences_updated_at();
