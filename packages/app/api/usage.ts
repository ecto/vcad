// GET /api/usage — returns the caller's current tier, period window, and
// usage counter. Used by the sidebar meter and the upgrade modal.
//
// Requires a Supabase access token (Authorization: Bearer ...). Anon users
// get a 401 and should consult the local anon counter instead.

import type { VercelRequest, VercelResponse } from "@vercel/node";
import { applyCors, getSupabaseAdmin, getUserIdFromAuth } from "./_lib/supabase.js";
import { getEntitlement, getPeriodUsage } from "./_lib/entitlements.js";

export default async function handler(req: VercelRequest, res: VercelResponse) {
  applyCors(res);

  if (req.method === "OPTIONS") {
    res.status(200).end();
    return;
  }

  if (req.method !== "GET") {
    res.status(405).json({ error: "Method not allowed" });
    return;
  }

  const admin = getSupabaseAdmin();
  if (!admin) {
    res.status(503).json({ error: "Billing not configured" });
    return;
  }

  const userId = await getUserIdFromAuth(req, admin);
  if (!userId) {
    res.status(401).json({ error: "Unauthorized" });
    return;
  }

  const entitlement = await getEntitlement(admin, userId);
  const usage = await getPeriodUsage(admin, userId, entitlement.periodStart);

  res.setHeader("Cache-Control", "no-store");
  res.status(200).json({
    tier: entitlement.tier,
    status: entitlement.status,
    periodStart: entitlement.periodStart.toISOString(),
    periodEnd: entitlement.periodEnd.toISOString(),
    inputTokens: usage.inputTokens,
    outputTokens: usage.outputTokens,
    messageCount: usage.messageCount,
    limit: entitlement.limit,
    cancelAtPeriodEnd: entitlement.cancelAtPeriodEnd,
  });
}
