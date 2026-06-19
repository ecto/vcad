\set ON_ERROR_STOP on
-- ── seed ──
insert into auth.users (id, email) values ('11111111-1111-1111-1111-111111111111','a@x.test');
insert into profiles (id, username) values ('11111111-1111-1111-1111-111111111111','alice');
insert into quotes (id, user_id, doc_hash, process, total_amount_minor)
  values ('22222222-2222-2222-2222-222222222222','11111111-1111-1111-1111-111111111111','hash_abc','pcb',30000);
insert into orders (id, user_id, quote_id, fab)
  values ('33333333-3333-3333-3333-333333333333','11111111-1111-1111-1111-111111111111','22222222-2222-2222-2222-222222222222','jlcpcb');

do $$
declare u uuid := '11111111-1111-1111-1111-111111111111';
        o uuid := '33333333-3333-3333-3333-333333333333';
        q uuid := '22222222-2222-2222-2222-222222222222';
        a uuid; r jsonb; n int;
begin
  -- topup
  r := credit_wallet(u, 1000, 'topup', 'topup-1'); assert (r->>'ok')='true' and (r->>'balance_minor')='1000', 'topup '||r::text;
  -- idempotent topup
  r := credit_wallet(u, 1000, 'topup', 'topup-1'); assert (r->>'idempotent')='true' and (r->>'balance_minor')='1000', 'topup-replay '||r::text;
  -- reused key, different amount
  r := credit_wallet(u, 5, 'topup', 'topup-1'); assert (r->>'reason')='idempotency_key_reused', 'reuse '||r::text;

  -- one_time authz: authorized, matches quote, fab+process allow, hash match, max 500
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,process_allowlist,fab_allowlist,doc_hash,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',500,array['pcb'],array['jlcpcb'],'hash_abc','authorized', now()+interval '1 day')
    returning id into a;
  -- happy debit 300
  r := debit_wallet(u,300,o,a,'deb-1'); assert (r->>'ok')='true' and (r->>'balance_minor')='700', 'debit '||r::text;
  -- idempotent debit replay
  r := debit_wallet(u,300,o,a,'deb-1'); assert (r->>'idempotent')='true' and (r->>'balance_minor')='700', 'debit-replay '||r::text;
  -- authz now consumed
  assert (select status from spend_authorizations where id=a)='consumed', 'authz not consumed';
  -- reusing consumed authz (new key) fails
  r := debit_wallet(u,100,o,a,'deb-2'); assert (r->>'reason')='authz_not_authorized', 'consumed-reuse '||r::text;

  -- insufficient funds
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,doc_hash,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',999999,'hash_abc','authorized',now()+interval '1 day') returning id into a;
  r := debit_wallet(u,5000,o,a,'deb-3'); assert (r->>'reason')='insufficient_funds' and (r->>'balance_minor')='700','insufficient '||r::text;

  -- amount exceeds authz
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',50,'authorized',now()+interval '1 day') returning id into a;
  r := debit_wallet(u,100,o,a,'deb-4'); assert (r->>'reason')='amount_exceeds_authz','exceeds '||r::text;

  -- expired authz
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',500,'authorized',now()-interval '1 minute') returning id into a;
  r := debit_wallet(u,100,o,a,'deb-5'); assert (r->>'reason')='authz_expired','expired '||r::text;

  -- revoked via kill switch
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',500,'authorized',now()+interval '1 day') returning id into a;
  n := revoke_user_authorizations(u); assert n>=1, 'revoke count';
  r := debit_wallet(u,100,o,a,'deb-6'); assert (r->>'reason')='authz_revoked','revoked '||r::text;

  -- wrong user
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',500,'authorized',now()+interval '1 day') returning id into a;
  r := debit_wallet('44444444-4444-4444-4444-444444444444',100,o,a,'deb-7'); assert (r->>'reason')='authz_not_found','wronguser '||r::text;

  -- fab not allowed
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,fab_allowlist,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',500,array['pcbway'],'authorized',now()+interval '1 day') returning id into a;
  r := debit_wallet(u,100,o,a,'deb-8'); assert (r->>'reason')='fab_not_allowed','fab '||r::text;

  -- doc_hash mismatch
  insert into spend_authorizations (id,user_id,quote_id,kind,max_amount_minor,doc_hash,status,expires_at)
    values (gen_random_uuid(),u,q,'one_time',500,'WRONG','authorized',now()+interval '1 day') returning id into a;
  r := debit_wallet(u,100,o,a,'deb-9'); assert (r->>'reason')='doc_hash_mismatch','hash '||r::text;

  -- standing authz + daily cap: balance 700, cap 250
  insert into spend_authorizations (id,user_id,kind,max_amount_minor,daily_cap_minor,status,expires_at)
    values (gen_random_uuid(),u,'standing',500,250,'authorized',now()+interval '1 day') returning id into a;
  r := debit_wallet(u,200,o,a,'deb-10'); assert (r->>'ok')='true' and (r->>'balance_minor')='500','standing1 '||r::text;
  assert (select status from spend_authorizations where id=a)='authorized','standing stays authorized'; -- not consumed
  r := debit_wallet(u,100,o,a,'deb-11'); assert (r->>'reason')='daily_cap_exceeded','dailycap '||r::text; -- 200+100>250

  -- refund requires order
  r := credit_wallet(u,50,'refund','ref-1'); assert (r->>'reason')='refund_requires_order','refund-noorder '||r::text;
  r := credit_wallet(u,50,'refund','ref-2',o); assert (r->>'ok')='true' and (r->>'balance_minor')='550','refund-ok '||r::text;

  -- reconciliation: no drift
  assert (select count(*) from fn_wallet_drift())=0, 'drift detected';

  raise notice 'ALL LOGIC TESTS PASSED';
end $$;

-- service-role guard: an authenticated caller must be rejected
set request.jwt.claims = '{"role":"authenticated"}';
do $$
declare ok boolean := false;
begin
  begin
    perform credit_wallet('11111111-1111-1111-1111-111111111111', 1, 'topup', 'should-fail');
  exception when others then ok := true;
  end;
  assert ok, 'service-role guard did NOT block authenticated caller';
  raise notice 'SERVICE-ROLE GUARD PASSED';
end $$;
reset request.jwt.claims;
