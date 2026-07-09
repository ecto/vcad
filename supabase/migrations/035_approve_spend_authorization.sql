-- vcad Fabricate — human approval write-path for spend authorizations.
--
-- Migration 027 minted the authorization lifecycle (pending_human → authorized
-- → consumed/revoked/expired) but left users READ-ONLY on their own rows: RLS
-- grants SELECT, and every write ran through service-role-only RPCs. The agent
-- can PROPOSE a spend (authorize_spend → status pending_human) and deep-link
-- the human to vcad.io/authorize/<id>, but there was no way for the human to
-- actually flip the row. These two functions are that missing write-path.
--
-- Guardrails:
--   1. These are the HUMAN's own actions, so unlike the money RPCs they are
--      granted to `authenticated` — but only under RLS-equivalent guards
--      enforced INSIDE the function: auth.uid() must be non-null and equal to
--      the row's user_id, and the only legal transition is FROM pending_human.
--      A caller can never approve someone else's authorization, re-approve a
--      consumed one, or resurrect a revoked/expired one.
--   2. Not-owner is deliberately indistinguishable from not-found — the
--      response never confirms that a foreign authorization id exists.
--   3. No wallet/ledger touches. Money still moves ONLY through debit_wallet()
--      (service-role, migration 027), which independently re-checks status,
--      expiry, ownership, caps, and allowlists at consume time. Approving here
--      unlocks at most ONE bounded, already-proposed spend.
--   4. The row is locked (FOR UPDATE) before the status check, so a concurrent
--      approve/decline race resolves to exactly one winner; the loser gets
--      {ok:false, reason:'not_pending'}.
--
-- Returns jsonb, never raises for guarded failures:
--   { ok:true,  status }            on success
--   { ok:false, reason, status? }   reasons: not_found | not_pending | expired

-- ─── approve_spend_authorization() — pending_human → authorized ──────────────
create or replace function approve_spend_authorization(p_authorization_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  v_uid uuid := auth.uid();
  v_authz spend_authorizations%rowtype;
begin
  -- Unauthenticated (no JWT uid) folds into not_found: the caller learns
  -- nothing about whether the id exists.
  if v_uid is null then
    return jsonb_build_object('ok', false, 'reason', 'not_found');
  end if;

  -- Ownership is part of the lookup, so a foreign id is indistinguishable
  -- from a missing one. Lock the row to serialize concurrent decisions.
  select * into v_authz
    from spend_authorizations
    where id = p_authorization_id and user_id = v_uid
    for update;
  if not found then
    return jsonb_build_object('ok', false, 'reason', 'not_found');
  end if;

  if v_authz.status <> 'pending_human' then
    return jsonb_build_object('ok', false, 'reason', 'not_pending',
                              'status', v_authz.status);
  end if;

  if v_authz.expires_at <= now() then
    return jsonb_build_object('ok', false, 'reason', 'expired');
  end if;

  update spend_authorizations
    set status = 'authorized', approved_by = v_uid, approved_at = now()
    where id = v_authz.id;

  return jsonb_build_object('ok', true, 'status', 'authorized');
end;
$$;

-- ─── decline_spend_authorization() — pending_human → revoked ─────────────────
create or replace function decline_spend_authorization(p_authorization_id uuid)
returns jsonb
language plpgsql
security definer
set search_path = public
as $$
declare
  v_uid uuid := auth.uid();
  v_authz spend_authorizations%rowtype;
begin
  if v_uid is null then
    return jsonb_build_object('ok', false, 'reason', 'not_found');
  end if;

  select * into v_authz
    from spend_authorizations
    where id = p_authorization_id and user_id = v_uid
    for update;
  if not found then
    return jsonb_build_object('ok', false, 'reason', 'not_found');
  end if;

  if v_authz.status <> 'pending_human' then
    return jsonb_build_object('ok', false, 'reason', 'not_pending',
                              'status', v_authz.status);
  end if;

  if v_authz.expires_at <= now() then
    return jsonb_build_object('ok', false, 'reason', 'expired');
  end if;

  update spend_authorizations
    set status = 'revoked', revoked_at = now()
    where id = v_authz.id;

  return jsonb_build_object('ok', true, 'status', 'revoked');
end;
$$;

-- ─── grants — the human's own action, under the in-function guards ───────────
-- authenticated only: anon (pre-session) callers have no uid to own a row, and
-- the service role already has its own authorization write-paths. This does
-- NOT weaken the migration-027 money plane — debit_wallet remains service-role
-- only and re-verifies everything at consume time.
revoke all on function approve_spend_authorization(uuid) from public;
revoke all on function decline_spend_authorization(uuid) from public;
grant execute on function approve_spend_authorization(uuid) to authenticated;
grant execute on function decline_spend_authorization(uuid) to authenticated;
