-- Harden RLS on inference_logs: the UPDATE policy from 004_add_ratings.sql
-- defined only USING, so a user could update their own row and change
-- user_id to another user's id (reassigning ownership). Add an explicit
-- WITH CHECK so the row's user_id is still the caller after update.

drop policy if exists "Users can rate own inference logs" on inference_logs;

create policy "Users can rate own inference logs"
  on inference_logs for update
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);
