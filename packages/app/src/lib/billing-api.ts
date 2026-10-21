import { useAuthStore } from "@vcad/auth";
import {
  useBillingStore,
  parseUsageResponse,
  type PaidTierId,
} from "@vcad/core";

// ---------------------------------------------------------------------------
// Client helpers for the billing endpoints. Kept separate from the rest of
// chat-api so the sidebar meter can be refreshed without dragging in chat
// streaming code.
// ---------------------------------------------------------------------------

function authHeaders(): Record<string, string> {
  const session = useAuthStore.getState().session;
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (session?.access_token) {
    headers.Authorization = `Bearer ${session.access_token}`;
  }
  return headers;
}

/**
 * Fetch the current usage snapshot and push it into the billing store. Safe
 * to call repeatedly — unauthenticated calls short-circuit and clear the
 * store. Errors are stored on the store so the meter can show a retry state.
 */
export async function refreshUsage(): Promise<void> {
  const store = useBillingStore.getState();
  const session = useAuthStore.getState().session;

  if (!session?.access_token) {
    store.reset();
    return;
  }

  store.setLoading(true);
  try {
    const res = await fetch("/api/usage", {
      method: "GET",
      headers: authHeaders(),
    });
    if (!res.ok) {
      // 401 just means the token is no longer valid — silently clear.
      if (res.status === 401) {
        store.reset();
        return;
      }
      store.setError(`Usage fetch failed: ${res.status}`);
      return;
    }
    const raw = await res.json();
    const snapshot = parseUsageResponse(raw);
    if (snapshot) store.setSnapshot(snapshot);
  } catch (err) {
    store.setError(err instanceof Error ? err.message : "Usage fetch failed");
  } finally {
    store.setLoading(false);
  }
}

/**
 * Kick off Stripe Checkout for the requested tier. Returns the Checkout URL
 * so the caller can navigate. Throws on any non-OK response so the upgrade
 * modal can surface the error inline.
 */
export async function startCheckout(tier: PaidTierId): Promise<string> {
  const res = await fetch("/api/billing/checkout", {
    method: "POST",
    headers: authHeaders(),
    body: JSON.stringify({ tier }),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `Checkout failed: ${res.status}`);
  }
  const { url } = (await res.json()) as { url?: string };
  if (!url) throw new Error("Checkout response missing URL");
  return url;
}

/**
 * Open Stripe's Customer Portal in the current tab — used for managing
 * payment methods, invoices, and cancellation.
 */
export async function openCustomerPortal(): Promise<string> {
  const res = await fetch("/api/billing/portal", {
    method: "POST",
    headers: authHeaders(),
  });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || `Portal session failed: ${res.status}`);
  }
  const { url } = (await res.json()) as { url?: string };
  if (!url) throw new Error("Portal response missing URL");
  const a = document.createElement("a");
  a.href = url;
  a.rel = "noopener";
  a.click();
  return url;
}
