/**
 * U.S. export-control / sanctions geo-block — single source of truth.
 *
 * vcad's hosted surfaces (vcad.io, mcp.vcad.io) are operated by a U.S.
 * person and may not be supplied to the jurisdictions below:
 *
 * - RU — OFAC determination of 2024-06-12 under E.O. 14071 (effective
 *   2024-09-12): prohibits supplying "IT support and cloud-based services
 *   for … design and manufacturing software" (CAD is explicitly named) to
 *   any person located in the Russian Federation.
 * - BY — parallel BIS EAR restrictions (15 C.F.R. § 746.8, Russia/Belarus).
 * - IR, CU, KP, SY — comprehensively sanctioned (OFAC country programs).
 * - Crimea, Sevastopol, Donetsk, Luhansk (so-called DNR/LNR) regions of
 *   Ukraine — embargoed under E.O. 13685 and E.O. 14065. Blocked
 *   best-effort via the region header when the platform provides one.
 *
 * This module is imported by every deploy's edge middleware and by the MCP
 * server's request handler — keep it dependency-free and runtime-agnostic
 * (it runs on the Edge runtime and on Node). The public GitHub repo is NOT
 * affected: published open-source software is not subject to the EAR
 * (15 C.F.R. §§ 734.3(b)(3), 734.7).
 */

/** ISO 3166-1 alpha-2 country codes for which the hosted service is blocked. */
export const BLOCKED_COUNTRIES: ReadonlySet<string> = new Set([
  "RU",
  "BY",
  "IR",
  "CU",
  "KP",
  "SY",
]);

/**
 * Blocked ISO 3166-2:UA subdivision codes (the part after "UA-"):
 * 43 = Crimea, 40 = Sevastopol, 14 = Donetsk oblast, 09 = Luhansk oblast.
 */
export const BLOCKED_UA_REGIONS: ReadonlySet<string> = new Set([
  "43",
  "40",
  "14",
  "09",
]);

/** HTTP status for blocked requests (RFC 7725 — Unavailable For Legal Reasons). */
export const GEO_BLOCK_STATUS = 451;

/** Human-readable message returned to blocked requests. */
export const GEO_BLOCK_MESSAGE =
  "vcad's hosted service is unavailable in your region due to U.S. export controls and sanctions. " +
  "The open-source code remains available at github.com/ecto/vcad.";

/** JSON body returned with the 451. */
export const GEO_BLOCK_BODY = JSON.stringify({
  error: "unavailable_for_legal_reasons",
  message: GEO_BLOCK_MESSAGE,
});

/**
 * Decide whether a request from `country` (ISO 3166-1 alpha-2, e.g. "RU")
 * and optional `region` (ISO 3166-2 subdivision — either "43" or "UA-43"
 * form) must be blocked.
 *
 * Fails open: a missing/empty country header allows the request. Geo headers
 * are absent in local dev and some internal invocations, and blocking those
 * would take the service down for everyone — IP geolocation is best-effort
 * screening, not the sole compliance control.
 */
export function isGeoBlocked(
  country: string | null | undefined,
  region?: string | null,
): boolean {
  if (!country) return false;
  const cc = country.trim().toUpperCase();
  if (BLOCKED_COUNTRIES.has(cc)) return true;
  if (cc === "UA" && region) {
    const sub = region.trim().toUpperCase().replace(/^UA-/, "");
    if (BLOCKED_UA_REGIONS.has(sub)) return true;
  }
  return false;
}

/** Vercel attaches `geo` to the request in some runtimes; honor it when present. */
interface GeoRequest extends Request {
  geo?: { country?: string; countryRegion?: string };
}

/**
 * Extract geo from a fetch `Request` (Vercel's `x-vercel-ip-country` /
 * `x-vercel-ip-country-region` headers, falling back to `request.geo` where
 * the platform provides it) and run the block check.
 */
export function isRequestGeoBlocked(request: Request): boolean {
  const geo = (request as GeoRequest).geo;
  const country =
    request.headers.get("x-vercel-ip-country") ?? geo?.country ?? null;
  const region =
    request.headers.get("x-vercel-ip-country-region") ??
    geo?.countryRegion ??
    null;
  return isGeoBlocked(country, region);
}

/** The 451 response served to blocked requests (Edge runtime / fetch API). */
export function geoBlockResponse(): Response {
  return new Response(GEO_BLOCK_BODY, {
    status: GEO_BLOCK_STATUS,
    headers: { "Content-Type": "application/json" },
  });
}
