-- Harden sweep_orphaned_streams so it can never touch another user's rows.
--
-- The original version (014_chat_threads.sql) only enforced ownership when
-- the caller passed a non-null thread_id_filter. When the filter was null
-- it updated chat_messages across every user's chat_threads — so any
-- authenticated caller could mark all users' streaming messages as
-- interrupted. Rewrite the function to always scope to auth.uid().

create or replace function sweep_orphaned_streams(thread_id_filter uuid default null)
returns integer
language plpgsql
security definer
set search_path = public
as $$
declare
  swept integer;
  caller uuid := auth.uid();
begin
  if caller is null then
    raise exception 'must be signed in to sweep streams';
  end if;

  with stale as (
    select m.id
    from chat_messages m
    join chat_threads t on t.id = m.thread_id
    where m.status = 'streaming'
      and t.user_id = caller
      and (thread_id_filter is null or m.thread_id = thread_id_filter)
      and (
        select coalesce(max(d.created_at), m.created_at)
        from chat_message_deltas d where d.message_id = m.id
      ) < now() - interval '2 minutes'
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

-- migrate_chat_threads_to_user already verifies auth.uid() == to_user_id, but
-- nothing stops a caller from pointing from_user_id at any anonymous session
-- they can guess the UUID of. Refuse to migrate from a user that has already
-- been upgraded (is_anonymous = false); that prevents the "steal permanent
-- user's threads" escalation path if anon UUID guessing ever becomes
-- tractable.
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
  from_is_anon boolean;
begin
  if auth.uid() is null or auth.uid() <> to_user_id then
    raise exception 'must be signed in as destination user';
  end if;
  if from_user_id = to_user_id then
    return 0;
  end if;

  select coalesce(u.is_anonymous, false) into from_is_anon
  from auth.users u where u.id = from_user_id;
  if not from_is_anon then
    raise exception 'from_user_id must be an anonymous session';
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
