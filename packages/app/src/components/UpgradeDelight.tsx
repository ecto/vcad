import { useEffect } from "react";
import { refreshUsage } from "@/lib/billing-api";

/**
 * Detects a successful return from Stripe Checkout (`?billing=success`) and
 * fires the upgrade celebration: confetti burst + billing store refresh.
 * Cleans the URL params so refreshing doesn't re-trigger.
 *
 * Mount once at the App root alongside `<CelebrationOverlay />`.
 */
export function UpgradeDelight() {
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    if (params.get("billing") !== "success") return;

    // Clean the URL immediately so a refresh doesn't re-trigger.
    params.delete("billing");
    params.delete("session_id");
    const remaining = params.toString();
    window.history.replaceState(
      {},
      "",
      remaining ? `${window.location.pathname}?${remaining}` : window.location.pathname,
    );

    // Short delay so the page settles before confetti fires — feels more
    // intentional than blasting particles mid-hydration.
    const timer = setTimeout(() => {
      window.dispatchEvent(new CustomEvent("vcad:celebrate-upgrade"));
    }, 400);

    // Refresh the billing store so the meter shows the new tier.
    void refreshUsage();

    return () => clearTimeout(timer);
  }, []);

  return null;
}
