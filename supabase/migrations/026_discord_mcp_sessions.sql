-- Discord pings for MCP sessions — reuses notify_discord() + the vault webhook
-- (migration 021); no new webhook config or env var.
--
-- Two cases:
--   * Authenticated MCP sessions persist to `documents` with local_id
--     'mcp:<id>' (SupabaseSessionStore), so they currently fire the generic
--     "📐 New document" ping. Route those to a distinct "🟢 New MCP session"
--     embed instead, so MCP usage is separable from real web-app documents.
--   * Anonymous MCP sessions live in `mcp_sessions` (migration 025) and had no
--     ping at all — add one.

-- ─── documents: route MCP rows to an MCP-session embed ───────────────────────
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

  -- MCP-authored sessions (local_id 'mcp:<id>') → MCP-session embed, not a doc.
  if new.local_id like 'mcp:%' then
    perform notify_discord(jsonb_build_object(
      'title', '🟢 New MCP session',
      'color', 3066993, -- #2ecc71
      'timestamp', new.created_at,
      'fields', jsonb_build_array(
        jsonb_build_object('name', 'session', 'value', substr(new.local_id, 5), 'inline', true),
        jsonb_build_object('name', 'who',     'value', owner_label,             'inline', true)
      )
    ));
    return new;
  end if;

  -- Real web-app document (unchanged from migration 021).
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

-- ─── mcp_sessions: anonymous MCP-session pings ───────────────────────────────
create or replace function notify_discord_on_mcp_session_create()
returns trigger
language plpgsql
security definer
set search_path = public
as $$
begin
  perform notify_discord(jsonb_build_object(
    'title', '🟢 New MCP session',
    'color', 3066993, -- #2ecc71
    'timestamp', new.created_at,
    'fields', jsonb_build_array(
      jsonb_build_object('name', 'session', 'value', new.document_id, 'inline', true),
      jsonb_build_object('name', 'who',     'value', 'anonymous',     'inline', true)
    )
  ));
  return new;
end;
$$;

drop trigger if exists discord_notify_mcp_session_create on public.mcp_sessions;
create trigger discord_notify_mcp_session_create
  after insert on public.mcp_sessions
  for each row execute function notify_discord_on_mcp_session_create();
