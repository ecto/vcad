-- Minimal Supabase-ish deps so migration 027 applies in a plain Postgres.
create role service_role;
create role authenticated;
create role anon;
create schema if not exists auth;
create table auth.users (id uuid primary key default gen_random_uuid(), email text, is_anonymous boolean default false);
create or replace function auth.uid() returns uuid language sql stable as $$ select nullif(current_setting('test.uid', true),'')::uuid $$;
create table profiles (id uuid primary key references auth.users(id), username text);
create or replace function touch_updated_at() returns trigger language plpgsql as $$ begin new.updated_at = now(); return new; end $$;
-- subset of migration 024 columns that 027 references
create table quotes (id uuid primary key default gen_random_uuid(), user_id uuid, doc_hash text, process text, total_amount_minor bigint, created_at timestamptz default now());
create table orders (id uuid primary key default gen_random_uuid(), user_id uuid, quote_id uuid references quotes(id), state text default 'QUOTED', fab text, created_at timestamptz default now());
