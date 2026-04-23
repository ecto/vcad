-- Enrich the "💬 New chat thread" Discord notification.
--
-- Privacy stance: we intentionally do NOT include message content, email, or
-- full user/document UUIDs. The document name is the only new human-readable
-- field, and it is already broadcast by the document-create trigger in 021.

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
  doc_owner_id uuid;
  doc_label text;
  fields jsonb;
begin
  select username into owner_username from profiles where id = new.user_id;
  owner_label := coalesce(owner_username, 'user:' || substr(new.user_id::text, 1, 8));

  select name, user_id into doc_name, doc_owner_id from documents where id = new.document_id;
  doc_label := coalesce(nullif(doc_name, ''), 'doc:' || substr(new.document_id, 1, 8));

  fields := jsonb_build_array(
    jsonb_build_object('name', 'user',     'value', owner_label,                        'inline', true),
    jsonb_build_object('name', 'document', 'value', doc_label,                          'inline', true),
    jsonb_build_object('name', 'thread',   'value', substr(new.id::text, 1, 8),         'inline', true)
  );

  if doc_owner_id is not null and doc_owner_id <> new.user_id then
    fields := fields || jsonb_build_array(
      jsonb_build_object('name', 'context', 'value', 'shared doc', 'inline', true)
    );
  end if;

  perform notify_discord(jsonb_build_object(
    'title', '💬 New chat thread',
    'color', 10937650, -- #a6e22e
    'timestamp', new.created_at,
    'fields', fields
  ));

  return new;
end;
$$;
