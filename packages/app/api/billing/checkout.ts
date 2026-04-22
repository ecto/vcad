// POST /api/billing/checkout
//
// Body: { tier: "pro" | "max" }
// Returns: { url }
//
// Creates (or reuses) a Stripe Customer for the caller, then opens a
// Checkout session for the requested tier. The tier → price mapping uses
// Stripe `price.lookup_key`, which must match `TIERS[x].stripeLookupKey` in
// @vcad/core. The user creates the products manually in the Stripe dashboard
// with matching lookup keys.

import type { VercelRequest, VercelResponse } from "@vercel/node";
import type { SupabaseClient } from "@supabase/supabase-js";
import { TIERS, type PaidTierId } from "@vcad/core";
import { applyCors, getSupabaseAdmin, getUserIdFromAuth } from "../_lib/supabase.js";
import { getStripe, getAppOrigin } from "../_lib/stripe.js";

function isPaidTier(v: unknown): v is PaidTierId {
  return v === "pro" || v === "max";
}

async function getOrCreateStripeCustomer(
  admin: SupabaseClient,
  stripe: ReturnType<typeof getStripe>,
  userId: string,
): Promise<string> {
  if (!stripe) throw new Error("Stripe not configured");

  const { data: existing } = await admin
    .from("subscriptions")
    .select("stripe_customer_id")
    .eq("user_id", userId)
    .maybeSingle();
  if (existing?.stripe_customer_id) return existing.stripe_customer_id as string;

  // Pull the user's email from auth.users so the Stripe customer has a real
  // identity — makes dashboards readable and lets customers use passwordless
  // portal access.
  const { data: authUser } = await admin.auth.admin.getUserById(userId);
  const email = authUser?.user?.email ?? undefined;

  const customer = await stripe.customers.create({
    email,
    metadata: { vcad_user_id: userId },
  });

  // Seed a free-tier row so subsequent lookups are O(1) and the webhook has
  // a target to update when the subscription is created.
  const { error } = await admin.from("subscriptions").upsert(
    {
      user_id: userId,
      stripe_customer_id: customer.id,
      tier: "free",
      status: "active",
    },
    { onConflict: "user_id" },
  );
  if (error) console.error("[checkout] seed subscriptions row failed:", error);

  return customer.id;
}

export default async function handler(req: VercelRequest, res: VercelResponse) {
  applyCors(res, req);

  if (req.method === "OPTIONS") {
    res.status(200).end();
    return;
  }
  if (req.method !== "POST") {
    res.status(405).json({ error: "Method not allowed" });
    return;
  }

  const admin = getSupabaseAdmin();
  const stripe = getStripe();
  if (!admin || !stripe) {
    res.status(503).json({ error: "Billing not configured" });
    return;
  }

  const userId = await getUserIdFromAuth(req, admin);
  if (!userId) {
    res.status(401).json({ error: "Unauthorized" });
    return;
  }

  let body: { tier?: unknown };
  if (typeof req.body === "string") {
    try { body = JSON.parse(req.body); } catch { res.status(400).json({ error: "invalid json" }); return; }
  } else if (req.body && typeof req.body === "object") {
    body = req.body as { tier?: unknown };
  } else {
    let raw = "";
    for await (const chunk of req) raw += chunk;
    try { body = JSON.parse(raw); } catch { res.status(400).json({ error: "invalid json" }); return; }
  }

  if (!isPaidTier(body.tier)) {
    res.status(400).json({ error: "tier must be 'pro' or 'max'" });
    return;
  }

  const tier = body.tier;
  const lookupKey = TIERS[tier].stripeLookupKey;
  if (!lookupKey) {
    res.status(500).json({ error: `No Stripe lookup key configured for tier ${tier}` });
    return;
  }

  try {
    // Resolve the Stripe Price from its lookup key. We fetch at request time
    // (instead of storing IDs in env) so renaming / replacing the price in
    // Stripe doesn't require a redeploy.
    const prices = await stripe.prices.list({
      lookup_keys: [lookupKey],
      active: true,
      limit: 1,
    });
    const price = prices.data[0];
    if (!price) {
      res.status(500).json({
        error: `No active Stripe price found for lookup_key ${lookupKey}`,
      });
      return;
    }

    const customerId = await getOrCreateStripeCustomer(admin, stripe, userId);
    const origin = getAppOrigin();

    const session = await stripe.checkout.sessions.create({
      mode: "subscription",
      customer: customerId,
      line_items: [{ price: price.id, quantity: 1 }],
      allow_promotion_codes: true,
      client_reference_id: userId,
      success_url: `${origin}/?billing=success&session_id={CHECKOUT_SESSION_ID}`,
      cancel_url: `${origin}/?billing=canceled`,
      subscription_data: {
        metadata: { vcad_user_id: userId, vcad_tier: tier },
      },
    });

    res.status(200).json({ url: session.url });
  } catch (err) {
    console.error("[checkout] Stripe error:", err);
    res.status(500).json({
      error: err instanceof Error ? err.message : "Checkout failed",
    });
  }
}
