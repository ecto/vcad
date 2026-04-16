-- Phase 1: usernames, public profiles, and document slugs.
--
-- profiles: globally unique usernames, public-readable, owner-writable.
-- documents gains slug (per-user unique) + visibility + published_at.
-- get_public_document(username, slug): SECURITY DEFINER RPC for /@user/slug.
-- share_redirects: maps old /view/<token> → (username, slug) for 308s.

-- ─── profiles ────────────────────────────────────────────────────────────

create table profiles (
  id uuid primary key references auth.users(id) on delete cascade,
  username text unique not null,
  display_name text,
  bio text,
  avatar_url text,
  created_at timestamptz default now(),
  updated_at timestamptz default now(),
  constraint username_format check (username ~ '^[a-z0-9]([a-z0-9-]*[a-z0-9])?$'),
  constraint username_length check (char_length(username) between 2 and 24),
  constraint username_not_reserved check (username not in (
    'admin', 'api', 'help', 'docs', 'view', 'share', 'settings',
    'login', 'signup', 'about', 'legal', 'embed', 'app', 'www',
    'root', 'me', 'public', 'static', 'new', 'edit', 'delete',
    'search', 'explore', 'discover', 'trending', 'popular',
    'og', 'embed', 'assets', 'favicon', 'robots', 'sitemap'
  ))
);

alter table profiles enable row level security;

create policy "Profiles are world-readable"
  on profiles for select using (true);

create policy "Users insert own profile"
  on profiles for insert with check (auth.uid() = id);

create policy "Users update own profile"
  on profiles for update using (auth.uid() = id);

-- Auto-update updated_at on profiles
create trigger profiles_touch_updated_at
  before update on profiles
  for each row execute function update_updated_at();

-- ─── documents: slug + visibility ────────────────────────────────────────

alter table documents
  add column slug text,
  add column visibility text default 'private'
    check (visibility in ('private', 'unlisted', 'public')),
  add column published_at timestamptz;

create unique index idx_documents_user_slug
  on documents(user_id, slug)
  where slug is not null;

-- Public documents are readable by anyone (supplements the existing
-- owner-only "Users can manage own documents" policy).
create policy "Public documents are world-readable"
  on documents for select
  using (visibility = 'public');

-- ─── get_public_document RPC ─────────────────────────────────────────────

create or replace function get_public_document(p_username text, p_slug text)
returns table (
  id uuid,
  name text,
  content jsonb,
  version int,
  updated_at timestamptz,
  owner_username text,
  owner_display_name text,
  owner_avatar_url text
)
language sql
security definer
set search_path = public
stable
as $$
  select
    d.id, d.name, d.content, d.version, d.updated_at,
    p.username, p.display_name, p.avatar_url
  from documents d
  join profiles p on p.id = d.user_id
  where p.username = p_username
    and d.slug = p_slug
    and d.visibility in ('public', 'unlisted');
$$;

grant execute on function get_public_document(text, text) to anon, authenticated;

-- ─── list_public_documents: for profile pages ────────────────────────────

create or replace function list_public_documents(p_username text)
returns table (
  id uuid,
  name text,
  slug text,
  updated_at timestamptz,
  published_at timestamptz
)
language sql
security definer
set search_path = public
stable
as $$
  select d.id, d.name, d.slug, d.updated_at, d.published_at
  from documents d
  join profiles p on p.id = d.user_id
  where p.username = p_username
    and d.visibility = 'public'
    and d.slug is not null
  order by d.published_at desc nulls last, d.updated_at desc;
$$;

grant execute on function list_public_documents(text) to anon, authenticated;

-- ─── share_redirects: /view/<token> → /@user/slug ────────────────────────

create table share_redirects (
  token uuid primary key references document_shares(token) on delete cascade,
  username text not null,
  slug text not null,
  created_at timestamptz default now()
);

alter table share_redirects enable row level security;

-- Anyone can read redirects (they're public URL mappings).
create policy "Redirects are world-readable"
  on share_redirects for select using (true);

-- Only the share owner can insert/update redirects (enforced via the
-- document_shares FK — if you own the share token, you own the redirect).
create policy "Share owners manage redirects"
  on share_redirects for all
  using (
    token in (select s.token from document_shares s where s.created_by = auth.uid())
  );
