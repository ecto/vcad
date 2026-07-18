/**
 * Tolerant scalar coercion for tool arguments.
 *
 * Field report: `check_clearance {allow_contact: true}` had no effect on the
 * hosted server — the boolean arrived as something other than a strict `true`
 * (some MCP clients/transports serialize booleans as the strings "true"/
 * "false"), so the handler's `args.x === true` check silently read false and
 * the flag vanished. Numbers survive the same transport, so the failure is
 * specific to booleans read by identity.
 *
 * `asBool` accepts the honest spellings of a boolean flag — the real boolean,
 * and the case-insensitive strings "true"/"false" / "1"/"0" — and returns the
 * schema default for anything else. Strict on unknown *keys* (see
 * validate-args), lenient on the *shape* of a known scalar.
 */

/** Coerce a boolean-ish argument to a boolean, falling back to `dflt`. */
export function asBool(v: unknown, dflt = false): boolean {
  if (typeof v === "boolean") return v;
  if (typeof v === "number") return v === 1 ? true : v === 0 ? false : dflt;
  if (typeof v === "string") {
    switch (v.trim().toLowerCase()) {
      case "true":
      case "1":
        return true;
      case "false":
      case "0":
        return false;
      default:
        return dflt;
    }
  }
  return dflt;
}
