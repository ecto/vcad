/**
 * Strict top-level argument validation for tool dispatch.
 *
 * Field report (torr session): a `style: "raytrace"` argument on render_view
 * was silently accepted and ignored — the caller only discovered it by
 * byte-comparing outputs. Handlers read args by hand, so an unrecognized key
 * never surfaces. This module closes that gap at the dispatch layer: any
 * top-level argument key not declared in the tool's advertised input schema
 * is a loud, structured error instead of a silent no-op.
 *
 * Only object schemas with declared `properties` and without a truthy
 * `additionalProperties` are enforced — schemas that deliberately accept
 * free-form maps keep working.
 */

/** Minimal shape of the JSON schemas the tool defs advertise. */
interface ObjectishSchema {
  type?: string;
  properties?: Record<string, unknown>;
  additionalProperties?: unknown;
}

/**
 * Return the top-level keys of `args` that the schema does not declare, or
 * an empty array when everything is declared (or the schema opts out of
 * strictness via `additionalProperties`).
 */
export function unknownArgKeys(
  schema: unknown,
  args: Record<string, unknown>,
): string[] {
  const s = schema as ObjectishSchema | null | undefined;
  if (!s || s.type !== "object" || !s.properties) return [];
  if (s.additionalProperties) return [];
  const declared = new Set(Object.keys(s.properties));
  return Object.keys(args).filter((k) => !declared.has(k));
}

/**
 * Build the rejection error text for unknown argument keys: names the
 * offenders and lists what the tool actually accepts, so the caller can
 * self-correct in one step.
 */
export function unknownArgsError(
  toolName: string,
  unknown: string[],
  schema: unknown,
): string {
  const props = (schema as ObjectishSchema | null | undefined)?.properties ?? {};
  return JSON.stringify({
    error: `Unknown argument${unknown.length > 1 ? "s" : ""} for ${toolName}: ${unknown
      .map((k) => `'${k}'`)
      .join(", ")}. Unrecognized arguments are rejected rather than silently ignored.`,
    accepted_arguments: Object.keys(props),
  });
}
