-- Discord activity notifications.
--
-- Fires webhooks to Discord on three events that happen client-direct-to-DB
-- (so they can't be trivially caught by a server-side API handler):
--
--   * auth.users INSERT        → new user signup
--   * documents INSERT         → new cloud document
--   * chat_threads INSERT      → first message in a new chat thread
--
-- Uses the pg_net extension to POST asynchronously so nothing about the
-- user's request depends on Discord being reachable. The webhook URL lives
-- in vault.decrypted_secrets under name 'discord_webhook_url'. Set it with:
--
--   select vault.create_secret(
--     'https://discord.com/api/webhooks/...', 'discord_webhook_url');
--
-- When the secret isn't set, the helper function returns without calling
-- Discord — so this migration is safe to apply before the webhook is wired.
-- Subscription events (upgrades/cancels) are handled in the billing webhook
-- handler directly (packages/app/api/billing/webhook.ts), not here.

create extension if not exists pg_net;

-- ─── notify_discord() — shared helper ────────────────────────────────────

create or replace function notify_discord(embed jsonb)
returns void
language plpgsql
security definer
set search_path = public, vault
as $$
declare
  webhook_url text;
begin
  select decrypted_secret
    into webhook_url
    from vault.decrypted_secrets
    where name = 'discord_webhook_url'
    limit 1;

  if webhook_url is null or webhook_url = '' then
    return;
  end if;

  perform net.http_post(
    url := webhook_url,
    headers := jsonb_build_object('Content-Type', 'application/json'),
    body := jsonb_build_object('embeds', jsonb_build_array(embed))
  );
exception when others then
  -- Never let a Discord failure roll back a user-facing write.
  raise warning 'notify_discord failed: %', sqlerrm;
end;
$$;

-- ─── new user signup ─────────────────────────────────────────────────────

create or replace function notify_discord_on_user_signup()
returns trigger
language plpgsql
security definer
set search_path = public, auth
as $$
declare
  masked_email text;
  provider text;
begin
  -- Skip anonymous Supabase sessions — users bounce in and out of those
  -- constantly; we only care about real signups.
  if coalesce(new.is_anonymous, false) then
    return new;
  end if;

  -- "a***@example.com"
  if new.email is not null and position('@' in new.email) > 0 then
    masked_email := substr(new.email, 1, 1)
      || repeat('*', greatest(1, position('@' in new.email) - 2))
      || substr(new.email, position('@' in new.email));
  else
    masked_email := 'unknown';
  end if;

  provider := coalesce(
    new.raw_app_meta_data ->> 'provider',
    'email'
  );

  perform notify_discord(jsonb_build_object(
    'title', '👋 New user signup',
    'color', 6750207, -- #66d9ef
    'timestamp', new.created_at,
    'fields', jsonb_build_array(
      jsonb_build_object('name', 'email',    'value', masked_email, 'inline', true),
      jsonb_build_object('name', 'provider', 'value', provider,     'inline', true)
    )
  ));

  return new;
end;
$$;

drop trigger if exists discord_notify_user_signup on auth.users;
create trigger discord_notify_user_signup
  after insert on auth.users
  for each row execute function notify_discord_on_user_signup();

-- ─── new cloud document ──────────────────────────────────────────────────

create or replace function notify_discord_on_document_create()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
declare
  owner_username text;
  owner_label text;
begin
  select username into owner_username from profiles where id = new.user_id;
  owner_label := coalesce(owner_username, 'user:' || substr(new.user_id::text, 1, 8));

  perform notify_discord(jsonb_build_object(
    'title', '📐 New document',
    'color', 16328818, -- #f92672
    'timestamp', new.created_at,
    'fields', jsonb_build_array(
      jsonb_build_object('name', 'name',  'value', coalesce(new.name, 'untitled'), 'inline', true),
      jsonb_build_object('name', 'owner', 'value', owner_label,                    'inline', true)
    )
  ));

  return new;
end;
$$;

drop trigger if exists discord_notify_document_create on public.documents;
create trigger discord_notify_document_create
  after insert on public.documents
  for each row execute function notify_discord_on_document_create();

-- ─── new chat thread (first message) ─────────────────────────────────────

create or replace function notify_discord_on_chat_thread_create()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
declare
  owner_username text;
  owner_label text;
begin
  select username into owner_username from profiles where id = new.user_id;
  owner_label := coalesce(owner_username, 'user:' || substr(new.user_id::text, 1, 8));

  perform notify_discord(jsonb_build_object(
    'title', '💬 New chat thread',
    'color', 10937650, -- #a6e22e
    'timestamp', new.created_at,
    'fields', jsonb_build_array(
      jsonb_build_object('name', 'user',     'value', owner_label,     'inline', true),
      jsonb_build_object('name', 'document', 'value', new.document_id, 'inline', true)
    )
  ));

  return new;
end;
$$;

drop trigger if exists discord_notify_chat_thread_create on public.chat_threads;
create trigger discord_notify_chat_thread_create
  after insert on public.chat_threads
  for each row execute function notify_discord_on_chat_thread_create();
