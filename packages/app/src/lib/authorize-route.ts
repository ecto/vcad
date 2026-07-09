/**
 * /authorize/<authorization_id> — spend-authorization approval deep link.
 *
 * The MCP `authorize_spend` tool proposes a spend (status pending_human) and
 * points the human at `https://vcad.io/authorize/<id>`. Like `/cli-auth`,
 * this page is mounted from `main.tsx` by pathname match — it never shares
 * the App render path, so it can't break the normal editor load.
 */

const AUTHORIZE_ROUTE_RE =
  /^\/authorize\/([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\/?$/;

/**
 * Parse an /authorize/<uuid> pathname into the authorization id.
 * Returns null for anything else (including malformed ids — the page only
 * ever queries by uuid, so non-uuid paths fall through to the editor).
 */
export function parseAuthorizeRoute(pathname: string): string | null {
  const match = pathname.match(AUTHORIZE_ROUTE_RE);
  return match?.[1] ?? null;
}

/** Authorization id from the current URL, or null when not on /authorize/. */
export function getAuthorizeRouteId(): string | null {
  if (typeof window === "undefined") return null;
  return parseAuthorizeRoute(window.location.pathname);
}
