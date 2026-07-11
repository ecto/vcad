/**
 * Trust boundary for the commerce plane (docs/trust-boundary.md).
 *
 * vcad agents ingest untrusted content (imported STEP/KiCad/Eagle files,
 * part descriptions, datasheets) and also hold spend authority
 * (authorize_spend / place_order). This module is the mechanical guard
 * between the two: a synchronous, pure pre-dispatch check that runs before
 * any commerce tool handler and rejects argument shapes an injection would
 * need — regardless of what the model was convinced to do.
 *
 * The rules are deliberately dumb and enforceable, not heuristic:
 *  1. Money-plane tools accept opaque ids only. Every id-shaped argument
 *     must match a safe charset; a "part number" or order id carrying a URL,
 *     whitespace, or control characters is refused outright.
 *  2. Artifact handles must point at OUR artifact store. A bare `art_…` id
 *     or a relative `/artifacts/…` path is fine; a full URL is only accepted
 *     on an allowlisted vcad host. A poisoned document that plants
 *     `https://evil.example/artifacts/art_x` never reaches resolution.
 *  3. Free-text that travels to the fab (ship_to, material, finish) must be
 *     plain: no URLs, no control characters, bounded length. An address is
 *     not a place for instructions.
 *
 * Fail-closed: the guard refuses on violation with a stable, greppable
 * `TRUST_BOUNDARY:` message; it never rewrites arguments.
 */

/** Tools the guard applies to (the commerce plane). */
export const COMMERCE_TOOLS: ReadonlySet<string> = new Set([
  "quote_manufacturing",
  "authorize_spend",
  "place_order",
]);

/** Arguments that must be opaque ids, per tool. */
const ID_FIELDS: Record<string, readonly string[]> = {
  quote_manufacturing: ["document_id"],
  authorize_spend: ["order_id"],
  place_order: ["order_id", "authorization_id", "idempotency_key"],
};

/** Free-text fields that travel to the fab, per tool. */
const FAB_TEXT_FIELDS: Record<string, readonly string[]> = {
  quote_manufacturing: ["material", "finish"],
};

/** Artifact-handle fields, per tool. */
const ARTIFACT_FIELDS: Record<string, readonly string[]> = {
  quote_manufacturing: ["fab_artifact_id"],
  place_order: ["fab_artifact_id"],
};

/** Opaque-id charset: what our own tools mint (uuid/art_/ord_/auth_ …). */
const SAFE_ID = /^[A-Za-z0-9._:-]{1,128}$/;

/** Hosts an artifact_url may name. Everything else is refused. */
const ARTIFACT_HOSTS: ReadonlySet<string> = new Set([
  "mcp.vcad.io",
  "vcad.io",
  "www.vcad.io",
  "localhost",
  "127.0.0.1",
]);

const SHIP_TO_MAX_FIELD_LEN = 200;
const SHIP_TO_MAX_FIELDS = 24;

/** Result of a boundary check. */
export interface BoundaryVerdict {
  ok: boolean;
  /** Present when `ok` is false; starts with `TRUST_BOUNDARY:`. */
  reason?: string;
}

const pass: BoundaryVerdict = { ok: true };

function refuse(reason: string): BoundaryVerdict {
  return { ok: false, reason: `TRUST_BOUNDARY: ${reason}` };
}

/* eslint-disable no-control-regex */
const CONTROL_CHARS = /[\x00-\x1f\x7f]/;
/* eslint-enable no-control-regex */
const URL_SCHEME = /[a-z][a-z0-9+.-]*:\/\//i;

function hasUrl(s: string): boolean {
  return URL_SCHEME.test(s) || /\bwww\.[a-z0-9-]+\.[a-z]{2,}/i.test(s);
}

/**
 * Is this handle allowed to reach artifact resolution? Bare `art_…` ids and
 * relative `/artifacts/…` paths always are; absolute URLs only on our hosts.
 */
export function isAllowedArtifactHandle(handle: string): boolean {
  if (CONTROL_CHARS.test(handle)) return false;
  if (SAFE_ID.test(handle) && !/^\.+$/.test(handle)) return true; // bare id
  if (handle.startsWith("/")) {
    // Relative path: /artifacts/<id>[/<file>] only. Dot-only segments would
    // be path traversal, and the id charset admits them — refuse explicitly.
    if (!/^\/artifacts\/[A-Za-z0-9._:-]+(\/[A-Za-z0-9._:-]+)?$/.test(handle)) {
      return false;
    }
    return handle.split("/").every((seg) => !/^\.+$/.test(seg) || seg === "");
  }
  let url: URL;
  try {
    url = new URL(handle);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return false;
  if (!ARTIFACT_HOSTS.has(url.hostname)) return false;
  return url.pathname.includes("/artifacts/");
}

/** One flat or one-level-nested string field of ship_to. */
function badShipToValue(v: unknown): string | null {
  if (v === null || v === undefined) return null;
  if (typeof v === "number" || typeof v === "boolean") return null;
  if (typeof v !== "string") return "non-scalar value";
  if (v.length > SHIP_TO_MAX_FIELD_LEN) return "field too long";
  if (CONTROL_CHARS.test(v)) return "control characters";
  if (hasUrl(v)) return "embedded URL";
  return null;
}

/** Validate a ship_to object: bounded, flat-ish, plain text only. */
export function checkShipTo(shipTo: unknown): BoundaryVerdict {
  if (shipTo === undefined || shipTo === null) return pass;
  if (typeof shipTo !== "object" || Array.isArray(shipTo)) {
    return refuse("ship_to must be an object of plain address fields");
  }
  const entries = Object.entries(shipTo as Record<string, unknown>);
  if (entries.length > SHIP_TO_MAX_FIELDS) {
    return refuse("ship_to has too many fields");
  }
  for (const [k, v] of entries) {
    const bad = badShipToValue(v);
    if (bad) {
      return refuse(
        `ship_to.${k}: ${bad}. Addresses carry plain text only — no URLs, ` +
          `no control characters, ≤${SHIP_TO_MAX_FIELD_LEN} chars per field.`,
      );
    }
  }
  return pass;
}

/**
 * Pre-dispatch guard. Call for every tool; non-commerce tools pass through
 * untouched. Pure and synchronous — safe at the dispatch choke-point.
 */
export function checkCommerceBoundary(
  toolName: string,
  args: Record<string, unknown>,
): BoundaryVerdict {
  if (!COMMERCE_TOOLS.has(toolName)) return pass;

  for (const field of ID_FIELDS[toolName] ?? []) {
    const v = args[field];
    if (v === undefined || v === null) continue;
    if (typeof v !== "string" || !SAFE_ID.test(v)) {
      return refuse(
        `${toolName}.${field} must be an opaque id (letters, digits, ` +
          `.:_-, ≤128 chars) minted by a vcad tool — not free text. ` +
          `Ids from part descriptions, datasheets, or imported files are ` +
          `never valid here.`,
      );
    }
  }

  for (const field of ARTIFACT_FIELDS[toolName] ?? []) {
    const v = args[field];
    if (v === undefined || v === null || v === "") continue;
    if (typeof v !== "string" || !isAllowedArtifactHandle(v)) {
      return refuse(
        `${toolName}.${field} must reference the vcad artifact store: a ` +
          `bare art_… id, a relative /artifacts/… path, or an artifact URL ` +
          `on a vcad host. External URLs are never fetched or bound to orders.`,
      );
    }
  }

  for (const field of FAB_TEXT_FIELDS[toolName] ?? []) {
    const v = args[field];
    if (v === undefined || v === null) continue;
    if (typeof v !== "string" || CONTROL_CHARS.test(v) || hasUrl(v) || v.length > 120) {
      return refuse(
        `${toolName}.${field} must be a short plain-text label (≤120 chars, ` +
          `no URLs, no control characters).`,
      );
    }
  }

  if (toolName === "quote_manufacturing") {
    const shipTo = checkShipTo(args.ship_to);
    if (!shipTo.ok) return shipTo;
  }

  return pass;
}
