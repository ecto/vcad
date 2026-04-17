-- Per-document AI chat threads — production-grade persistence.
--
-- Schema rationale:
--   chat_threads        — thread metadata, one per (user, document)
--   chat_messages       — append-only DAG (parent_id supports branching)
--   chat_message_deltas — streaming sidecar; replayable mid-stream resume
--   chat_tool_calls     — normalized tool invocations for analytics + the
--                         client-side execution loop (tools run in browser,
--                         results POSTed back to /api/chat)
--
-- Distinct from chat_conversations (existing SFT/training audit log;
-- per-call snapshot, opt-out via user_preferences.share_chat_conversations).
-- chat_threads is always-on user-owned working data. SFT pipeline migration
-- to read from these tables is a follow-up.

-- ---------------------------------------------------------------------------
-- chat_threads
-- ---------------------------------------------------------------------------

create table if not exists chat_threads (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references auth.users(id) on delete cascade,
  -- Client-generated document id. Text (not uuid) because some local-only
  -- ids predate the cloud-sync uuid scheme.
  document_id text not null,
  title text,
  model_id text,
  -- Leaf of the active branch — the path from this message back to a root
  -- via parent_id is what the UI renders by default.
  head_message_id text,
  status text not null default 'active' check (status in ('active', 'archived')),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  last_activity_at timestamptz not null default now(),
  unique (user_id, document_id)
);

create index if not exists chat_threads_user_activity_idx
  on chat_threads (user_id, last_activity_at desc);

-- ---------------------------------------------------------------------------
-- chat_messages
-- ---------------------------------------------------------------------------

create table if not exists chat_messages (
  -- Client-generated id (matches the in-memory ChatMessage.id from
  -- chat-store makeId()) so the client can optimistically render before the
  -- server roundtrip. Server writes win on conflict via upsert.
  id text primary key,
  thread_id uuid not null references chat_threads(id) on delete cascade,
  -- Nullable parent for the DAG. Roots have parent_id is null.
  parent_id text references chat_messages(id) on delete cascade,
  role text not null check (role in ('user', 'assistant')),
  -- Anthropic-format content blocks: [{type:'text',text}, {type:'image',...},
  -- {type:'tool_use',id,name,input}, {type:'tool_result',tool_use_id,content}].
  -- This is what's sent to the model; render-only metadata is in adjacent columns.
  content_blocks jsonb not null default '[]'::jsonb,
  -- Render-only: data URLs + filenames for user attachments. Not sent to model
  -- (the data is encoded into content_blocks as base64 image blocks).
  attachments jsonb,
  -- Render-only: selected-part pills shown above the user bubble.
  context jsonb,
  status text not null default 'complete' check (
    status in ('pending', 'streaming', 'complete', 'interrupted', 'error')
  ),
  input_tokens integer,
  output_tokens integer,
  duration_ms integer,
  model_id text,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now(),
  completed_at timestamptz
);

create index if not exists chat_messages_thread_idx
  on chat_messages (thread_id, created_at);

create index if not exists chat_messages_parent_idx
  on chat_messages (parent_id)
  where parent_id is not null;

create index if not exists chat_messages_streaming_idx
  on chat_messages (thread_id, updated_at)
  where status = 'streaming';

-- ---------------------------------------------------------------------------
-- chat_message_deltas
-- ---------------------------------------------------------------------------

create table if not exists chat_message_deltas (
  id bigserial primary key,
  message_id text not null references chat_messages(id) on delete cascade,
  -- Monotonic per-message sequence; client orders deltas by this.
  sequence integer not null,
  delta_type text not null check (
    delta_type in ('text', 'tool_start', 'tool_input_json', 'block_stop', 'done')
  ),
  payload jsonb,
  created_at timestamptz not null default now(),
  unique (message_id, sequence)
);

create index if not exists chat_message_deltas_message_idx
  on chat_message_deltas (message_id, sequence);

-- ---------------------------------------------------------------------------
-- chat_tool_calls
-- ---------------------------------------------------------------------------

create table if not exists chat_tool_calls (
  -- Anthropic tool_use id (e.g. "toolu_01AbC..."). Globally unique per turn.
  id text primary key,
  message_id text not null references chat_messages(id) on delete cascade,
  -- Denormalized for cheap thread-scoped queries ("any pending tools in this
  -- thread?") without joining through messages.
  thread_id uuid not null references chat_threads(id) on delete cascade,
  name text not null,
  args jsonb not null default '{}'::jsonb,
  result jsonb,
  status text not null default 'pending' check (
    status in ('pending', 'success', 'error')
  ),
  -- Render-only: human-readable execution display payload (rendered by
  -- VcadToolCard). Not sent back to the model.
  display jsonb,
  -- Render-only: data URL for tools that produce an image (e.g. screenshot).
  image_data_url text,
  started_at timestamptz not null default now(),
  completed_at timestamptz,
  duration_ms integer
);

create index if not exists chat_tool_calls_message_idx
  on chat_tool_calls (message_id);

create index if not exists chat_tool_calls_pending_idx
  on chat_tool_calls (thread_id, started_at)
  where status = 'pending';

-- ---------------------------------------------------------------------------
-- Triggers — touch updated_at
-- ---------------------------------------------------------------------------

create or replace function touch_updated_at()
returns trigger as $$
begin
  new.updated_at = now();
  return new;
end;
$$ language plpgsql;

create trigger chat_threads_touch_updated_at
  before update on chat_threads
  for each row execute function touch_updated_at();

create trigger chat_messages_touch_updated_at
  before update on chat_messages
  for each row execute function touch_updated_at();

-- ---------------------------------------------------------------------------
-- RLS
-- ---------------------------------------------------------------------------

alter table chat_threads        enable row level security;
alter table chat_messages       enable row level security;
alter table chat_message_deltas enable row level security;
alter table chat_tool_calls     enable row level security;

create policy "Users manage own chat threads"
  on chat_threads for all
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);

create policy "Users manage own chat messages"
  on chat_messages for all
  using (
    exists (
      select 1 from chat_threads t
      where t.id = chat_messages.thread_id and t.user_id = auth.uid()
    )
  )
  with check (
    exists (
      select 1 from chat_threads t
      where t.id = chat_messages.thread_id and t.user_id = auth.uid()
    )
  );

create policy "Users read own message deltas"
  on chat_message_deltas for select
  using (
    exists (
      select 1 from chat_messages m
      join chat_threads t on t.id = m.thread_id
      where m.id = chat_message_deltas.message_id and t.user_id = auth.uid()
    )
  );

-- Service role inserts deltas during the stream; clients only read.
create policy "Service role writes deltas"
  on chat_message_deltas for insert to service_role
  with check (true);

create policy "Users manage own tool calls"
  on chat_tool_calls for all
  using (
    exists (
      select 1 from chat_threads t
      where t.id = chat_tool_calls.thread_id and t.user_id = auth.uid()
    )
  )
  with check (
    exists (
      select 1 from chat_threads t
      where t.id = chat_tool_calls.thread_id and t.user_id = auth.uid()
    )
  );

grant all on chat_threads, chat_messages, chat_message_deltas, chat_tool_calls
  to service_role;
grant usage, select on all sequences in schema public to service_role;

-- ---------------------------------------------------------------------------
-- Realtime publication
-- ---------------------------------------------------------------------------

alter publication supabase_realtime add table chat_threads;
alter publication supabase_realtime add table chat_messages;
alter publication supabase_realtime add table chat_message_deltas;
alter publication supabase_realtime add table chat_tool_calls;

-- ---------------------------------------------------------------------------
-- sweep_orphaned_streams — flip stale streaming rows to interrupted.
-- A streaming message is "orphaned" if it has no delta activity for >2 min.
-- The serverless function probably died (Vercel kills on disconnect). Run
-- opportunistically on hydrate; eventual cron job is a follow-up.
-- ---------------------------------------------------------------------------

create or replace function sweep_orphaned_streams(thread_id_filter uuid default null)
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  swept integer;
begin
  with stale as (
    select m.id
    from chat_messages m
    where m.status = 'streaming'
      and (thread_id_filter is null or m.thread_id = thread_id_filter)
      and (
        select coalesce(max(d.created_at), m.created_at)
        from chat_message_deltas d where d.message_id = m.id
      ) < now() - interval '2 minutes'
      and (
        thread_id_filter is null
        or exists (
          select 1 from chat_threads t
          where t.id = m.thread_id and t.user_id = auth.uid()
        )
      )
  )
  update chat_messages m
    set status = 'interrupted',
        completed_at = now()
    from stale
    where m.id = stale.id;
  get diagnostics swept = row_count;
  return swept;
end;
$$;

grant execute on function sweep_orphaned_streams(uuid) to authenticated;

-- ---------------------------------------------------------------------------
-- migrate_chat_threads_to_user — re-parent anonymous threads on sign-in.
-- Called by the client when an anonymous Supabase session is upgraded to a
-- permanent (Google/GitHub) one. Caller must already be the destination user.
-- Threads whose document_id collides with one the destination user already
-- owns are dropped (destination wins; merging would require id reconciliation
-- across messages and isn't worth the complexity for the rare case).
-- ---------------------------------------------------------------------------

create or replace function migrate_chat_threads_to_user(
  from_user_id uuid,
  to_user_id uuid
) returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  moved integer;
begin
  if auth.uid() is null or auth.uid() <> to_user_id then
    raise exception 'must be signed in as destination user';
  end if;

  update chat_threads
    set user_id = to_user_id
    where user_id = from_user_id
      and not exists (
        select 1 from chat_threads dest
        where dest.user_id = to_user_id
          and dest.document_id = chat_threads.document_id
      );
  get diagnostics moved = row_count;

  delete from chat_threads where user_id = from_user_id;

  return moved;
end;
$$;

grant execute on function migrate_chat_threads_to_user(uuid, uuid) to authenticated;
