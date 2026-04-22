-- Optional expiry on document_shares.
--
-- Before: shares lived forever until explicitly revoked. A forgotten
-- share link could be used months after the owner intended access to
-- end, and there was no way to cap exposure in bulk (e.g., an HR-policy
-- "no links older than 30 days").
--
-- After: document_shares carries an optional expires_at timestamp.
-- get_shared_document() and any other reader that joins on
-- document_shares must filter out expired rows.

alter table document_shares
    add column if not exists expires_at timestamptz;

create index if not exists idx_document_shares_expires_at
    on document_shares (expires_at)
    where expires_at is not null;

-- Rewrite get_shared_document to respect expires_at.
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
  where s.token = p_token
    and (s.expires_at is null or s.expires_at > now());
$$;

grant execute on function get_shared_document(uuid) to anon, authenticated;

-- Opportunistic sweep of expired rows; the pg_cron schedule is best-effort
-- and keeps the shares table tidy even when no reader ever touches an
-- expired row.
do $$
begin
    if exists (select 1 from pg_extension where extname = 'pg_cron') then
        perform cron.schedule(
            'document_shares_expiry_sweep',
            '17 * * * *',
            $sql$
                delete from document_shares
                where expires_at is not null and expires_at < now()
            $sql$
        );
    end if;
end
$$;
