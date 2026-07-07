/**
 * Edge middleware for mcp.vcad.io — U.S. export-control / sanctions block.
 *
 * This deploy uses the Build Output API (build.sh writes .vercel/output
 * directly), so Vercel's automatic middleware.ts detection does not apply.
 * build.sh bundles this file into functions/_middleware.func (edge runtime)
 * and routes every request through it via a `middlewarePath` route with
 * `continue: true`. Block list + citations live in shared/geo-block.ts.
 *
 * The request handler in entry.ts runs the same check as defense-in-depth.
 */

import { isRequestGeoBlocked, geoBlockResponse } from "../../shared/geo-block.js";

export default function middleware(request: Request): Response {
  if (isRequestGeoBlocked(request)) return geoBlockResponse();
  // Build Output API middleware signals "continue to routing" explicitly.
  return new Response(null, { headers: { "x-middleware-next": "1" } });
}
