-- Fix: chat thread INSERT failing with "operator does not exist: uuid = text".
--
-- 022 introduced a document lookup in notify_discord_on_chat_thread_create
-- that compared documents.id (uuid) against new.document_id (text). The
-- comparison errored on every INSERT into chat_threads, breaking thread
-- creation for all users.
--
-- chat_threads.document_id stores the client-generated local doc id
-- (matches documents.local_id, not documents.id). Look up by local_id
-- scoped to the same user. The "shared doc" branch is dropped because
-- shared docs get their own per-user local_id, so it never fired anyway.

create or replace function notify_discord_on_chat_thread_create()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
declare
  owner_username text;
  owner_label text;
  doc_name text;
  doc_label text;
  fields jsonb;
begin
  select username into owner_username from profiles where id = new.user_id;
  owner_label := coalesce(owner_username, 'user:' || substr(new.user_id::text, 1, 8));

  select name into doc_name
    from documents
    where local_id = new.document_id and user_id = new.user_id
    limit 1;
  doc_label := coalesce(nullif(doc_name, ''), 'doc:' || substr(new.document_id, 1, 8));

  fields := jsonb_build_array(
    jsonb_build_object('name', 'user',     'value', owner_label,                        'inline', true),
    jsonb_build_object('name', 'document', 'value', doc_label,                          'inline', true),
    jsonb_build_object('name', 'thread',   'value', substr(new.id::text, 1, 8),         'inline', true)
  );

  perform notify_discord(jsonb_build_object(
    'title', '💬 New chat thread',
    'color', 10937650, -- #a6e22e
    'timestamp', new.created_at,
    'fields', fields
  ));

  return new;
end;
$$;
