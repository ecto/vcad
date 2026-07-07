# Export controls and sanctions

vcad's **hosted services** — the web app at [vcad.io](https://vcad.io) and the
MCP server at `mcp.vcad.io` — are operated by a U.S. person and are therefore
subject to U.S. export controls and sanctions. They are **not available** in:

- **Russia** — the OFAC determination of June 12, 2024 under Executive Order
  14071 (effective September 12, 2024) prohibits supplying IT support and
  cloud-based services for design and manufacturing software (CAD is
  explicitly named) to any person located in the Russian Federation.
- **Belarus** — parallel BIS Export Administration Regulations restrictions
  (15 C.F.R. § 746.8).
- **Iran, Cuba, North Korea, Syria** — comprehensively sanctioned under the
  respective OFAC country programs.
- **The Crimea, Sevastopol, Donetsk, and Luhansk regions of Ukraine** —
  embargoed under Executive Orders 13685 and 14065. Blocked best-effort by
  region where geolocation provides one; the rest of Ukraine is unaffected.

Requests from these jurisdictions receive **HTTP 451** (Unavailable For Legal
Reasons). The block list lives in [`shared/geo-block.ts`](../shared/geo-block.ts)
and is enforced by Vercel Edge Middleware on both deployments, plus a
request-layer check in the MCP server.

## The open-source code is not affected

This repository is published open-source software, which is not subject to
the EAR (15 C.F.R. §§ 734.3(b)(3), 734.7). Anyone may clone, build, and run
vcad locally under the [Apache License 2.0](../LICENSE).

## Your responsibility

By using the hosted services you represent that you are not located in any of
the jurisdictions above and are not on a U.S. restricted-party list. You are
responsible for complying with all export control and sanctions laws that
apply to you.
