/**
 * Agent tool-making layer: define, list, and call parametric loon macros.
 *
 * A macro is named loon source — `[let <name> [fn [params…] …]]` — exactly
 * the idiom the stdlib itself is written in (lib/src/lib.loon). Definitions
 * are prepended to programs the same way the stdlib is, so no engine or
 * language change is involved: the macro layer turns vcad from a stateless
 * kernel into an accumulating library.
 *
 * The trust ladder starts at definition time: `define_loon` refuses source
 * that does not compile, and refuses a macro whose smoke call (with the
 * declared example arguments) does not evaluate to a non-empty scene. What
 * enters the library is known-good by construction; receipt-certified
 * macros (claims over the parameter range) are the planned next rung.
 *
 * Storage v1: process-warm registry + file persistence under
 * VCAD_MCP_STATE_DIR for local/stdio use. Hosted durability (a per-user
 * mcp_macros table) is a follow-up — the MacroStore seam is already shaped
 * for it.
 */

import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Engine } from "@vcad/engine";
import { toVCode } from "@vcad/ir";
import { registerSession } from "./session.js";
import { resolveWithinRoot } from "./safe-path.js";
import { behavior, type ToolDef } from "./tool-def.js";

/** One stored macro. */
export interface LoonMacro {
  /** kebab-case name; also the loon function it must define. */
  name: string;
  /** What the macro builds; shown in list_loons. */
  description: string;
  /** Ordered parameter docs (names must match the fn's parameter list). */
  params: Array<{
    name: string;
    description?: string;
    /** Example value used for the definition-time smoke call. */
    example: number;
    unit?: string;
  }>;
  /** Loon source containing `[let <name> [fn [...] ...]]`. May define
   *  helpers; everything is prepended together at call time. */
  source: string;
  /** Monotone version, bumped on redefinition. */
  version: number;
}

const MACRO_NAME = /^[a-z][a-z0-9-]{1,63}$/;

/** Names the stdlib already claims — a macro may not shadow them. */
const RESERVED = new Set([
  "cube", "cylinder", "sphere", "cone", "torus", "wedge", "prism",
  "union", "difference", "intersection", "translate", "rotate", "scale",
  "mirror", "extrude", "revolve", "shell", "fillet", "chamfer",
  "sweep-line", "sweep-helix", "loft", "loft-closed", "linear-pattern",
  "circular-pattern", "sketch", "line", "arc", "root", "pipe", "let",
  "fn", "type", "assembly", "part", "instance",
]);

/** Process-warm registry. Hosted instances keep this for their lifetime;
 *  local/stdio instances also persist to disk (see load/persist). */
const registry = new Map<string, LoonMacro>();
let hydrated = false;
let diskEnabled = true;

function macroDir(): string {
  return join(process.env.VCAD_MCP_STATE_DIR ?? process.cwd(), "loon-macros");
}

function hydrateFromDisk(): void {
  if (hydrated) return;
  hydrated = true;
  const dir = macroDir();
  if (!existsSync(dir)) return;
  for (const f of readdirSync(dir)) {
    if (!f.endsWith(".json")) continue;
    try {
      const m = JSON.parse(readFileSync(join(dir, f), "utf8")) as LoonMacro;
      if (MACRO_NAME.test(m.name) && typeof m.source === "string") {
        registry.set(m.name, m);
      }
    } catch {
      // A corrupt file never blocks the library; it is simply skipped.
    }
  }
}

function persistToDisk(m: LoonMacro): void {
  if (!diskEnabled) return;
  try {
    const dir = macroDir();
    mkdirSync(dir, { recursive: true });
    const path = resolveWithinRoot(`${m.name}.json`, dir);
    writeFileSync(path, JSON.stringify(m, null, 2));
  } catch {
    // Warm registry still holds it; disk persistence is best-effort.
  }
}

/** An inline (pass-by-value) macro: the stateless alternative to the warm
 *  registry. `define_loon` returns this exact shape so agents can carry
 *  macros across instances/sessions without any server state. */
export interface InlineLoon {
  name: string;
  source: string;
  params?: LoonMacro["params"];
}

/** Look up macros: inline definitions win, then the warm registry. */
export function getMacros(
  names: string[],
  inline?: InlineLoon[],
): LoonMacro[] {
  hydrateFromDisk();
  const byValue = new Map(
    (inline ?? []).map((m) => [
      m.name,
      { description: "", params: m.params ?? [], version: 0, ...m } as LoonMacro,
    ]),
  );
  return names.map((n) => {
    const m = byValue.get(n) ?? registry.get(n);
    if (!m) {
      const known = [...registry.keys()].sort().join(", ") || "(none defined)";
      throw new Error(
        `unknown loon macro "${n}" — defined macros: ${known}. ` +
          `Stateless alternative: pass the macro by value via \`loons\`.`,
      );
    }
    return m;
  });
}

/** Concatenated source of the given macros, dependency-blind (macros may
 *  reference each other; callers list dependencies first). */
export function macroPrelude(names: string[], inline?: InlineLoon[]): string {
  return getMacros(names, inline)
    .map((m) => `; macro ${m.name} v${m.version}\n${m.source}`)
    .join("\n\n");
}

const loonNum = (v: number): string =>
  Number.isFinite(v) ? String(v) : (() => { throw new Error(`non-finite argument ${v}`); })();

/** Compose `[root [<name> args…] material]` call site. */
function callSite(m: LoonMacro, args: number[], material: string): string {
  const argSrc = args.map(loonNum).join(" ");
  return `[root [${m.name} ${argSrc}] "${material}"]`;
}

// ── define_loon ────────────────────────────────────────────────────────

export const defineLoonSchema = {
  type: "object" as const,
  required: ["name", "description", "params", "source"],
  properties: {
    name: {
      type: "string" as const,
      description:
        "kebab-case macro name (2–64 chars). The source must define a loon " +
        "function of this exact name via [let <name> [fn [...] ...]].",
    },
    description: {
      type: "string" as const,
      description: "One sentence: what the macro builds.",
    },
    params: {
      type: "array" as const,
      description:
        "Ordered parameter docs matching the fn's parameter list. `example` " +
        "values are used for the definition-time smoke call.",
      items: {
        type: "object" as const,
        required: ["name", "example"],
        properties: {
          name: { type: "string" as const },
          description: { type: "string" as const },
          example: {
            type: "number" as const,
            description: "A representative value; the smoke call uses it.",
          },
          unit: { type: "string" as const, description: "e.g. mm, deg" },
        },
      },
    },
    source: {
      type: "string" as const,
      description:
        "Loon source defining [let <name> [fn [<params...>] <Solid-expr>]]. " +
        "May include helper lets/types; the whole block is prepended to " +
        "calling programs, exactly like the stdlib.",
    },
  },
};

interface DefineArgs {
  name: string;
  description: string;
  params: LoonMacro["params"];
  source: string;
}

export function defineLoonTool(
  args: Record<string, unknown>,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  hydrateFromDisk();
  const a = args as unknown as DefineArgs;
  if (!MACRO_NAME.test(a.name)) {
    throw new Error(
      `define_loon: name must be kebab-case ([a-z][a-z0-9-]{1,63}), got "${a.name}"`,
    );
  }
  if (RESERVED.has(a.name)) {
    throw new Error(`define_loon: "${a.name}" shadows a stdlib name`);
  }
  if (!a.source.includes(`[let ${a.name} `) && !a.source.includes(`[let ${a.name}\n`)) {
    throw new Error(
      `define_loon: source must define the macro via [let ${a.name} [fn ...]]`,
    );
  }
  if (!Array.isArray(a.params)) {
    throw new Error("define_loon: params must be an array (may be empty)");
  }

  // Definition-time smoke call: the macro must compile AND its example
  // instantiation must evaluate to a non-empty scene. Known-good by
  // construction or not in the library.
  const candidate: LoonMacro = {
    name: a.name,
    description: String(a.description ?? ""),
    params: a.params,
    source: a.source,
    version: (registry.get(a.name)?.version ?? 0) + 1,
  };
  const examples = candidate.params.map((p) => {
    if (typeof p.example !== "number" || !Number.isFinite(p.example)) {
      throw new Error(`define_loon: param "${p.name}" needs a finite example value`);
    }
    return p.example;
  });
  const smoke = `${candidate.source}\n\n${callSite(candidate, examples, "default")}`;
  let doc;
  try {
    doc = engine.evalVcadSource(smoke);
  } catch (e) {
    throw new Error(
      `define_loon: smoke call [${a.name} ${examples.join(" ")}] failed to ` +
        `evaluate — macro NOT stored. Loon error: ${e instanceof Error ? e.message : e}`,
    );
  }
  if (!doc || !doc.roots?.length || !Object.keys(doc.nodes ?? {}).length) {
    throw new Error(
      `define_loon: smoke call produced an empty scene — macro NOT stored`,
    );
  }

  registry.set(candidate.name, candidate);
  persistToDisk(candidate);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            name: candidate.name,
            version: candidate.version,
            smoke_call: `[${candidate.name} ${examples.join(" ")}]`,
            verified: "compiles + example instantiation yields geometry",
            usage:
              `call_loon {name: "${candidate.name}", args: [...]} or ` +
              `create_cad_loon with use_loons: ["${candidate.name}"]`,
            // Pass-by-value record: carry this across sessions/instances and
            // replay via `loons` — no server state required.
            macro: {
              name: candidate.name,
              source: candidate.source,
              params: candidate.params,
            },
          },
          null,
          2,
        ),
      },
    ],
  };
}

// ── call_loon ──────────────────────────────────────────────────────────

export const callLoonSchema = {
  type: "object" as const,
  required: ["name", "args"],
  properties: {
    name: { type: "string" as const, description: "Macro to instantiate." },
    args: {
      type: "array" as const,
      items: { type: "number" as const },
      description: "Positional arguments, in the macro's declared order.",
    },
    material: {
      type: "string" as const,
      description: "Material for the instantiated part. Default \"default\".",
    },
    macro: {
      type: "object" as const,
      description:
        "STATELESS alternative: the macro passed by value (the `macro` " +
        "record define_loon returned: {name, source, params}). Wins over " +
        "the server-side registry; immune to serverless cold starts.",
      properties: {
        name: { type: "string" as const },
        source: { type: "string" as const },
        params: { type: "array" as const },
      },
      required: ["name", "source"],
    },
    format: {
      type: "string" as const,
      enum: ["vcode", "json"],
      description: "Document output format (default vcode).",
    },
  },
};

interface CallArgs {
  name: string;
  args: number[];
  material?: string;
  macro?: InlineLoon;
  format?: "vcode" | "json";
}

export function callLoonTool(
  args: Record<string, unknown>,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = args as unknown as CallArgs;
  const [m] = getMacros([String(a.name)], a.macro ? [a.macro] : undefined);
  if (!Array.isArray(a.args)) throw new Error("call_loon: `args` must be an array");
  // Arity is only checkable when the macro declares params (an inline macro
  // may omit them — loon itself then reports any mismatch).
  const declaredArity = a.macro && !a.macro.params ? undefined : m.params.length;
  if (declaredArity !== undefined && a.args.length !== declaredArity) {
    throw new Error(
      `call_loon: ${m.name} takes ${declaredArity} args ` +
        `(${m.params.map((p) => p.name).join(", ")}), got ${a.args.length}`,
    );
  }
  const source = `${m.source}\n\n${callSite(m, a.args, a.material ?? "default")}`;
  const doc = engine.evalVcadSource(source);
  if (!doc) {
    throw new Error("call_loon: loon evaluation not supported by this engine build");
  }
  const documentId = registerSession(doc);
  const text = a.format === "json" ? JSON.stringify(doc, null, 2) : toVCode(doc);
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(
          {
            document_id: documentId,
            macro: m.name,
            version: m.version,
            document: text,
          },
          null,
          2,
        ),
      },
    ],
  };
}

// ── list_loons ─────────────────────────────────────────────────────────

export function listLoonsTool(): { content: Array<{ type: "text"; text: string }> } {
  hydrateFromDisk();
  const macros = [...registry.values()]
    .sort((x, y) => x.name.localeCompare(y.name))
    .map((m) => ({
      name: m.name,
      version: m.version,
      description: m.description,
      params: m.params.map((p) => ({
        name: p.name,
        ...(p.unit ? { unit: p.unit } : {}),
        ...(p.description ? { description: p.description } : {}),
        example: p.example,
      })),
    }));
  return {
    content: [
      { type: "text", text: JSON.stringify({ count: macros.length, macros }, null, 2) },
    ],
  };
}

/** Test seam: empty warm registry, no disk reads or writes. */
export function clearMacrosForTest(): void {
  registry.clear();
  hydrated = true; // skip disk hydration
  diskEnabled = false;
}

export const toolDefs: ToolDef[] = [
  {
    name: "define_loon",
    pack: null,
    description:
      "Add a reusable parametric macro to the loon library. Provide loon source defining " +
      "[let <name> [fn [params...] <Solid-expr>]] plus parameter docs with example values. " +
      "The macro is smoke-tested at definition time (must compile and the example call must " +
      "yield geometry) — only known-good macros enter the library. Once defined, instantiate " +
      "with call_loon or compose inside any create_cad_loon program via use_loons. Redefining " +
      "a name bumps its version. Prefer macros over re-writing the same geometry each session.",
    inputSchema: defineLoonSchema,
    handler: (args, ctx) => defineLoonTool(args, ctx.engine),
    behavior: behavior({}),
  },
  {
    name: "call_loon",
    pack: null,
    description:
      "Instantiate a stored loon macro into a new document: positional numeric args in the " +
      "macro's declared order (see list_loons). Returns the document and a document_id session.",
    inputSchema: callLoonSchema,
    handler: (args, ctx) => callLoonTool(args, ctx.engine),
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
  {
    name: "list_loons",
    pack: null,
    description:
      "List the stored loon macro library: names, versions, parameter docs with units and " +
      "example values. Use before call_loon or create_cad_loon with use_loons.",
    inputSchema: { type: "object" as const, properties: {} },
    handler: () => listLoonsTool(),
    behavior: behavior({}),
  },
];
