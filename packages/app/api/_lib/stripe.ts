// Stripe SDK client factory. Keeps secret-key access in one place so the
// rest of the billing endpoints don't duplicate configuration.

import Stripe from "stripe";

let cached: Stripe | null = null;

export function getStripe(): Stripe | null {
  if (cached) return cached;
  const key = process.env.STRIPE_SECRET_KEY;
  if (!key) return null;
  cached = new Stripe(key, {
    // Pin the API version so upgrades are explicit and don't silently break
    // the webhook schema.
    apiVersion: "2025-02-24.acacia",
  });
  return cached;
}

/** Absolute origin used for Checkout / Portal return URLs. Falls back to
 *  vcad.io in production when VERCEL_URL isn't set explicitly. */
export function getAppOrigin(): string {
  const explicit = process.env.APP_ORIGIN;
  if (explicit) return explicit;
  const vercel = process.env.VERCEL_URL;
  if (vercel) return `https://${vercel}`;
  return "https://vcad.io";
}
