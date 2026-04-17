// POST /api/billing/portal
//
// Returns a Stripe Customer Portal URL for the signed-in user. The portal is
// Stripe's hosted self-service UI for updating payment methods, canceling
// subscriptions, viewing invoices, etc. — meaning vcad never has to build
// any of that surface.

import type { VercelRequest, VercelResponse } from "@vercel/node";
import { applyCors, getSupabaseAdmin, getUserIdFromAuth } from "../_lib/supabase.js";
import { getStripe, getAppOrigin } from "../_lib/stripe.js";

export default async function handler(req: VercelRequest, res: VercelResponse) {
  applyCors(res);

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

  const { data } = await admin
    .from("subscriptions")
    .select("stripe_customer_id")
    .eq("user_id", userId)
    .maybeSingle();
  const customerId = data?.stripe_customer_id as string | undefined;
  if (!customerId) {
    res.status(404).json({
      error: "No subscription found. Start a checkout first.",
    });
    return;
  }

  try {
    const origin = getAppOrigin();
    const session = await stripe.billingPortal.sessions.create({
      customer: customerId,
      return_url: `${origin}/?billing=portal-return`,
    });
    res.status(200).json({ url: session.url });
  } catch (err) {
    console.error("[portal] Stripe error:", err);
    res.status(500).json({
      error: err instanceof Error ? err.message : "Portal session failed",
    });
  }
}
