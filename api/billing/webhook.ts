// POST /api/billing/webhook
//
// Stripe sends subscription lifecycle events here. We verify the signature
// with STRIPE_WEBHOOK_SECRET, then mirror the state into the `subscriptions`
// table so /api/chat's entitlement check sees the latest tier.
//
// Important: we disable Vercel's JSON body parser so Stripe's signature can
// be verified against the exact bytes it sent. The generic body parser would
// re-encode the JSON and invalidate the signature.

import type { VercelRequest, VercelResponse } from "@vercel/node";
import type Stripe from "stripe";
import type { SupabaseClient } from "@supabase/supabase-js";
import { tierFromStripeLookupKey, type TierId } from "@vcad/core";
import { getSupabaseAdmin } from "../_lib/supabase.js";
import { getStripe } from "../_lib/stripe.js";
import { sendEmail, upgradeWelcomeEmail } from "../_lib/email.js";

export const config = {
  api: {
    bodyParser: false,
  },
};

async function readRawBody(req: VercelRequest): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : (chunk as Buffer));
  }
  return Buffer.concat(chunks);
}

/**
 * Resolve the tier id from a Stripe subscription by inspecting the first
 * item's price lookup_key. Falls back to the subscription metadata that we
 * set in Checkout, then to "free".
 */
function tierFromSubscription(sub: Stripe.Subscription): TierId {
  const firstItem = sub.items.data[0];
  const lookup = firstItem?.price?.lookup_key ?? null;
  const resolved = tierFromStripeLookupKey(lookup);
  if (resolved) return resolved;
  const metaTier = sub.metadata?.vcad_tier;
  if (metaTier === "pro" || metaTier === "max") return metaTier;
  return "free";
}

/**
 * Resolve the vcad user id for a Stripe customer. Prefer the subscription
 * metadata we set during checkout; fall back to the `subscriptions` row
 * keyed on customer id. Returns null if neither is available (malformed
 * state — we log and skip).
 */
async function resolveUserId(
  admin: SupabaseClient,
  sub: Stripe.Subscription,
): Promise<string | null> {
  const meta = sub.metadata?.vcad_user_id;
  if (typeof meta === "string" && meta.length > 0) return meta;

  const customerId = typeof sub.customer === "string" ? sub.customer : sub.customer.id;
  const { data } = await admin
    .from("subscriptions")
    .select("user_id")
    .eq("stripe_customer_id", customerId)
    .maybeSingle();
  return (data?.user_id as string | undefined) ?? null;
}

function toIso(ts: number | null | undefined): string | null {
  if (!ts) return null;
  return new Date(ts * 1000).toISOString();
}

async function upsertSubscription(
  admin: SupabaseClient,
  sub: Stripe.Subscription,
): Promise<void> {
  const userId = await resolveUserId(admin, sub);
  if (!userId) {
    console.warn("[webhook] no user id for subscription", sub.id);
    return;
  }
  const customerId = typeof sub.customer === "string" ? sub.customer : sub.customer.id;
  const tier = tierFromSubscription(sub);

  const { error } = await admin.from("subscriptions").upsert(
    {
      user_id: userId,
      stripe_customer_id: customerId,
      stripe_subscription_id: sub.id,
      tier,
      status: sub.status,
      current_period_start: toIso(sub.current_period_start),
      current_period_end: toIso(sub.current_period_end),
      cancel_at_period_end: sub.cancel_at_period_end,
    },
    { onConflict: "user_id" },
  );
  if (error) console.error("[webhook] upsert failed:", error);
}

async function markSubscriptionCanceled(
  admin: SupabaseClient,
  sub: Stripe.Subscription,
): Promise<void> {
  const userId = await resolveUserId(admin, sub);
  if (!userId) return;
  const { error } = await admin
    .from("subscriptions")
    .update({
      tier: "free",
      status: "canceled",
      stripe_subscription_id: null,
      current_period_end: toIso(sub.current_period_end),
      cancel_at_period_end: false,
    })
    .eq("user_id", userId);
  if (error) console.error("[webhook] cancel update failed:", error);
}

export default async function handler(req: VercelRequest, res: VercelResponse) {
  if (req.method !== "POST") {
    res.status(405).json({ error: "Method not allowed" });
    return;
  }

  const admin = getSupabaseAdmin();
  const stripe = getStripe();
  const webhookSecret = process.env.STRIPE_WEBHOOK_SECRET;
  if (!admin || !stripe || !webhookSecret) {
    console.error("[webhook] missing config — skipping event");
    res.status(503).json({ error: "Billing not configured" });
    return;
  }

  const sig = req.headers["stripe-signature"];
  if (typeof sig !== "string") {
    res.status(400).json({ error: "Missing stripe-signature header" });
    return;
  }

  let event: Stripe.Event;
  try {
    const raw = await readRawBody(req);
    event = stripe.webhooks.constructEvent(raw, sig, webhookSecret);
  } catch (err) {
    console.error("[webhook] signature verification failed:", err);
    res.status(400).json({
      error: err instanceof Error ? err.message : "Invalid signature",
    });
    return;
  }

  try {
    switch (event.type) {
      case "checkout.session.completed": {
        const session = event.data.object as Stripe.Checkout.Session;
        if (session.subscription) {
          const subId =
            typeof session.subscription === "string"
              ? session.subscription
              : session.subscription.id;
          const sub = await stripe.subscriptions.retrieve(subId);
          // Carry through the client_reference_id so subsequent events can
          // find the user even if metadata propagation is delayed.
          if (session.client_reference_id && !sub.metadata?.vcad_user_id) {
            sub.metadata = { ...sub.metadata, vcad_user_id: session.client_reference_id };
          }
          await upsertSubscription(admin, sub);

          // Send the "Welcome to Pro/Max" email on first checkout.
          const tier = tierFromSubscription(sub);
          if (tier !== "free") {
            const userId = session.client_reference_id ?? (await resolveUserId(admin, sub));
            if (userId) {
              void (async () => {
                try {
                  const { data: authUser } = await admin.auth.admin.getUserById(userId);
                  const email = authUser?.user?.email;
                  if (!email) return;
                  const firstName = (() => {
                    const full = authUser?.user?.user_metadata?.full_name ?? authUser?.user?.user_metadata?.name;
                    if (full) return String(full).split(" ")[0] ?? "there";
                    return email.split("@")[0] ?? "there";
                  })();
                  const msg = upgradeWelcomeEmail({ firstName, tier });
                  await sendEmail({ to: email, ...msg });
                } catch (err) {
                  console.error("[webhook] welcome email failed:", err);
                }
              })();
            }
          }
        }
        break;
      }
      case "customer.subscription.created":
      case "customer.subscription.updated": {
        const sub = event.data.object as Stripe.Subscription;
        await upsertSubscription(admin, sub);
        break;
      }
      case "customer.subscription.deleted": {
        const sub = event.data.object as Stripe.Subscription;
        await markSubscriptionCanceled(admin, sub);
        break;
      }
      case "invoice.payment_failed": {
        // Stripe flips subscription.status to past_due and emits a
        // subscription.updated event, so this mostly exists for logging.
        console.warn("[webhook] payment_failed for invoice", (event.data.object as Stripe.Invoice).id);
        break;
      }
      default:
        // Ignore events we don't care about. Stripe retries on non-2xx so we
        // always return 200 for unhandled types.
        break;
    }
    res.status(200).json({ received: true });
  } catch (err) {
    console.error("[webhook] handler error:", err);
    res.status(500).json({
      error: err instanceof Error ? err.message : "Webhook handler failed",
    });
  }
}
