-- Phase 0: public read-only share links for documents.
--
-- A share is a thin pointer: (token, document_id, created_by, created_at).
-- No duplicated state, no permissions, no expiry. Revoke = DELETE.
-- Anonymous reads go through get_shared_document(token) only; the documents
-- table RLS stays owner-only.

create table document_shares (
  token uuid primary key default gen_random_uuid(),
  document_id uuid not null references documents(id) on delete cascade,
  created_by uuid not null references auth.users(id) on delete cascade,
  created_at timestamptz default now()
);

create index idx_document_shares_document_id on document_shares(document_id);

alter table document_shares enable row level security;

create policy "Owners manage their shares"
  on document_shares for all
  using (auth.uid() = created_by);
-- No public SELECT policy. Anon reads go through get_shared_document() below.

create or replace function get_shared_document(p_token uuid)
returns table (
  id uuid,
  name text,
  content jsonb,
  version int,
  updated_at timestamptz
)
language sql
security definer
set search_path = public
stable
as $$
  select d.id, d.name, d.content, d.version, d.updated_at
  from document_shares s
  join documents d on d.id = s.document_id
  where s.token = p_token;
$$;

grant execute on function get_shared_document(uuid) to anon, authenticated;
