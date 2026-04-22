-- Harden RLS on documents: the FOR ALL policy defined in 001_documents.sql
-- only set USING, which left INSERT/UPDATE free to write rows with an
-- arbitrary user_id. Replace it with an explicit WITH CHECK so the row
-- after INSERT/UPDATE is always owned by the caller.

drop policy if exists "Users can manage own documents" on documents;

create policy "Users can manage own documents"
  on documents for all
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);
