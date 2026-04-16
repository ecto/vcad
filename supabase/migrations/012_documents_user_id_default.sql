-- Fix: default user_id to the authenticated user so inserts that omit
-- user_id (e.g. the sync client) don't violate the RLS policy.
alter table documents alter column user_id set default auth.uid();
