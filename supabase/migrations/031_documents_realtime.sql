-- Live "Continue in Claude" reflection.
--
-- When a signed-in user hands a part off to Claude, the model's continued
-- session persists to the `documents` table under local_id = mcp:cont_<token>
-- (same owner / user_id as the web user). Publishing `documents` to Realtime
-- lets the vcad.io tab subscribe to that row and reflect the model's edits live
-- — the "alive in both surfaces" beat.
--
-- Security: `documents` RLS already restricts every row to its owner
-- (auth.uid() = user_id, migration 001), and Realtime enforces RLS per
-- subscriber using their JWT, so a user only ever receives changes to their OWN
-- documents — never anyone else's. Default replica identity (the primary key)
-- is sufficient: subscribers get the new row on INSERT/UPDATE, which is all the
-- reflection needs.

do $$
begin
  if not exists (
    select 1
    from pg_publication_tables
    where pubname = 'supabase_realtime'
      and schemaname = 'public'
      and tablename = 'documents'
  ) then
    alter publication supabase_realtime add table documents;
  end if;
end $$;
