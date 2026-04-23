-- Phase: named versions.
--
-- `document_versions` already captures every content change as a row via the
-- trigger in 002_versions.sql. That's great for reconstructive history but
-- too noisy to scroll through. Named versions let a user promote specific
-- rows to first-class waypoints ("v1 pre-review", "manufacturable") without
-- changing how auto-versioning works.
--
-- Additive: unlabeled rows remain the full auto-save history.

alter table public.document_versions
  add column if not exists label text,
  add column if not exists labeled_by uuid references auth.users(id) on delete set null,
  add column if not exists labeled_at timestamptz;

create index if not exists document_versions_labeled_idx
  on public.document_versions(document_id, labeled_at desc)
  where label is not null;

-- ─── RPC: label_version ───────────────────────────────────────────────────
-- Promotes a single auto-version to a named one. Enforces ownership via the
-- underlying documents RLS by joining through documents.user_id.
create or replace function public.label_version(p_version_id uuid, p_label text)
returns void
language plpgsql
security definer
set search_path = public
as $$
begin
  if p_label is null or length(btrim(p_label)) = 0 then
    raise exception 'label cannot be empty';
  end if;

  update document_versions v
  set
    label = btrim(p_label),
    labeled_by = auth.uid(),
    labeled_at = now()
  from documents d
  where v.id = p_version_id
    and v.document_id = d.id
    and d.user_id = auth.uid();

  if not found then
    raise exception 'version not found or not owned by caller';
  end if;
end;
$$;

grant execute on function public.label_version(uuid, text) to authenticated;

-- ─── RPC: unlabel_version ─────────────────────────────────────────────────
-- Removes a named-version label (auto row survives).
create or replace function public.unlabel_version(p_version_id uuid)
returns void
language plpgsql
security definer
set search_path = public
as $$
begin
  update document_versions v
  set label = null, labeled_by = null, labeled_at = null
  from documents d
  where v.id = p_version_id
    and v.document_id = d.id
    and d.user_id = auth.uid();

  if not found then
    raise exception 'version not found or not owned by caller';
  end if;
end;
$$;

grant execute on function public.unlabel_version(uuid) to authenticated;

-- ─── RPC: list_named_versions ─────────────────────────────────────────────
-- Returns labeled versions for a document, newest label first. RLS still
-- applies on the underlying table — this is just a convenience view.
create or replace function public.list_named_versions(p_document_id uuid)
returns table (
  id uuid,
  version_number int,
  label text,
  labeled_by uuid,
  labeled_at timestamptz,
  device_modified_at bigint,
  created_at timestamptz
)
language sql
security definer
set search_path = public
stable
as $$
  select v.id, v.version_number, v.label, v.labeled_by, v.labeled_at,
         v.device_modified_at, v.created_at
  from document_versions v
  join documents d on d.id = v.document_id
  where d.user_id = auth.uid()
    and v.document_id = p_document_id
    and v.label is not null
  order by v.labeled_at desc;
$$;

grant execute on function public.list_named_versions(uuid) to authenticated;
