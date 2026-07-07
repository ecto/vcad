/**
 * Vercel Edge Middleware for the vcad.io web app (packages/app deploy config).
 *
 * Blocks the entire hosted surface for embargoed jurisdictions — see
 * shared/geo-block.ts for the block list and the legal citations. A copy of
 * this wrapper exists at the repo root so the block applies regardless of
 * which directory the Vercel project uses as its root; both import the same
 * shared module.
 */

import { isRequestGeoBlocked, geoBlockResponse } from "../../shared/geo-block";

export default function middleware(request: Request): Response | undefined {
  if (isRequestGeoBlocked(request)) return geoBlockResponse();
  return undefined; // fall through to the app
}
