/**
 * create_cad_loon tool — evaluate loon source to produce a CAD document.
 */

import { readFileSync } from "node:fs";
import { isAbsolute, join, resolve, sep } from "node:path";
import type { Engine } from "@vcad/engine";
import { toVCode } from "@vcad/ir";
import { appendIntegrity, computeIntegrity } from "./integrity.js";
import { hydrateMacros, macroPrelude, type InlineLoon } from "./loon-macros.js";
import { documents, getSession, recordTriangles } from "./session-core.js";
import { behavior, type ToolDef } from "./tool-def.js";
import type { ToolResult } from "./tool-result.js";

/** JSON Schema for create_cad_loon input. */
export const createCadLoonSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "Existing session (from open_document) to write the evaluated " +
        "document into, so the open → author workflow stays on one session. " +
        "Omitted: a fresh session is minted.",
    },
    source: {
      type: "string" as const,
      description: "Loon source code defining CAD geometry",
    },
    use_loons: {
      type: "array" as const,
      items: { type: "string" as const },
      description:
        "Stored macro names (see list_loons) to prepend as a library — " +
        "their [let <name> [fn ...]] definitions become callable from " +
        "`source`, exactly like the stdlib. List dependencies before " +
        "dependents.",
    },
    loons: {
      type: "array" as const,
      description:
        "STATELESS macro library: macros passed by value (the `macro` " +
        "records define_loon returns: {name, source}). Prepended like " +
        "use_loons but with no server-side registry dependency — immune " +
        "to serverless cold starts. Names here also satisfy use_loons.",
      items: {
        type: "object" as const,
        required: ["name", "source"],
        properties: {
          name: { type: "string" as const },
          source: { type: "string" as const },
          params: { type: "array" as const },
        },
      },
    },
    modules: {
      type: "object" as const,
      description:
        "Multi-file projects, by value: a { \"<module name>\": \"<loon " +
        "source>\" } map that `[use <name>]` in `source` resolves against. " +
        "`pub` controls what a module exports; `[use m :as alias]` and " +
        "`[use m [a b]]` work as in the language. Entries passed via " +
        "`loons` are importable by name too.",
      additionalProperties: { type: "string" as const },
    },
    base_dir: {
      type: "string" as const,
      description:
        "Server-side directory that `[use <name>]` resolves against — the " +
        "server reads <base_dir>/<name>.loon (dots are path separators) and " +
        "hands the sources to the kernel, following nested imports. Reads " +
        "are confined to this directory. Explicit `modules` entries win.",
    },
    format: {
      type: "string" as const,
      enum: ["vcode", "json"],
      description: "Output format (default: compact)",
    },
  },
  required: ["source"],
};

interface CreateLoonInput {
  document_id?: string;
  source: string;
  use_loons?: string[];
  loons?: InlineLoon[];
  modules?: Record<string, string>;
  base_dir?: string;
  format?: "vcode" | "json";
}

/** Compose the effective program: macro prelude (inline `loons` win over
 *  the registry) + user source. Inline macros not named in use_loons are
 *  prepended too — passing `loons` alone is sufficient. */
export function composeLoonProgram(input: unknown): string {
  const { source, use_loons, loons } = input as CreateLoonInput;
  const names = [
    ...(use_loons ?? []),
    ...(loons ?? []).map((m) => m.name).filter((n) => !use_loons?.includes(n)),
  ];
  if (!names.length) return source;
  return `${macroPrelude(names, loons)}\n\n${source}`;
}

/** Module names a program imports: every `[use <name> …]` head. */
function importedNames(source: string): string[] {
  const names: string[] = [];
  const re = /\[\s*use\s+([A-Za-z_][\w.-]*)/g;
  for (let m = re.exec(source); m; m = re.exec(source)) names.push(m[1]);
  return names;
}

/** Read `<base>/<a>/<b>.loon` for a dotted module name, refusing to escape
 *  `base`. Returns null when the module isn't there. */
function readModule(base: string, name: string): string | null {
  if (isAbsolute(name) || name.split(".").includes("..")) return null;
  const root = resolve(base);
  for (const ext of [".loon", ".oo"]) {
    const file = resolve(join(root, ...name.split(".")) + ext);
    if (file !== root && !file.startsWith(root + sep)) continue;
    try {
      return readFileSync(file, "utf8");
    } catch {
      // try the next extension
    }
  }
  return null;
}

/**
 * The in-memory module map `[use ...]` resolves against: explicit `modules`,
 * plus inline `loons` (a macro passed by value is also an importable
 * module), plus — when `base_dir` is given — files read from disk, following
 * nested imports transitively.
 *
 * A name the server can't find is simply left out; the kernel then reports
 * the missing module with loon's own error, rather than the server guessing.
 */
export function composeLoonModules(input: unknown): Record<string, string> {
  const { source, loons, modules, base_dir } = input as CreateLoonInput;
  const map: Record<string, string> = {};
  for (const m of loons ?? []) map[m.name] = m.source;
  Object.assign(map, modules ?? {});

  if (base_dir) {
    const pending = [
      ...importedNames(source),
      ...Object.values(map).flatMap(importedNames),
    ];
    const seen = new Set<string>();
    while (pending.length) {
      const name = pending.pop() as string;
      if (seen.has(name) || map[name]) continue;
      seen.add(name);
      const src = readModule(base_dir, name);
      if (src === null) continue;
      map[name] = src;
      pending.push(...importedNames(src));
    }
  }
  return map;
}

/** Evaluate loon source and return a CAD document. */
export function createCadLoon(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const { format = "vcode" } = input as CreateLoonInput;
  const source = composeLoonProgram(input);

  const modules = composeLoonModules(input);
  const doc = engine.evalVcadSourceWithModules(source, modules);
  if (!doc) {
    // Distinguish "no loon at all" from "loon, but a kernel too old to
    // resolve modules" — otherwise a stale kernel reads as a broken program.
    const text = Object.keys(modules).length
      ? "Error: this kernel build cannot resolve loon modules ([use ...]) — " +
        "update the kernel, or inline the modules into `source`"
      : "Error: Loon evaluation not supported by this engine build";
    return { content: [{ type: "text", text }] };
  }

  const text = format === "json" ? JSON.stringify(doc, null, 2) : toVCode(doc);

  return {
    content: [{ type: "text", text }],
  };
}

export const toolDefs: ToolDef[] = [
  {
    name: "create_cad_loon",
    pack: null,
    description:
      "The preferred authoring tool for whole parts and multi-feature models — one call, full vocabulary. Create a CAD document from loon source code. Loon is a Lisp-like language for parametric CAD — the FULL modeling vocabulary (patterns, sketches, extrude/revolve/sweep/loft, assemblies) is available here even where no dedicated MCP tool exists. For incremental single-node edits to an open session, use create/update/delete instead.\n\n" +
      "Primitives: [cube x y z], [cylinder r h], [sphere r], [cone r-bottom r-top h]\n" +
      "Booleans (subject-last): [difference tool subject], [union other subject], [intersection other subject]\n" +
      "Transforms (subject-last): [translate x y z s], [rotate rx ry rz s], [scale sx sy sz s]\n" +
      "Features: [fillet r s], [chamfer d s], [shell t s]\n" +
      "Patterns (subject-last): [linear-pattern dx dy dz count spacing s], [circular-pattern ox oy oz ax ay az count angle s] — e.g. a bolt circle is [circular-pattern 0 0 0 0 0 1 6 360 bolt-hole]\n" +
      "Sketches: [sketch ox oy oz xx xy xz yx yy yz #[segments]] with [line x1 y1 x2 y2] and [arc x1 y1 x2 y2 cx cy ccw]\n" +
      "Sketch ops (sketch-last): [extrude dx dy dz sk], [revolve aox aoy aoz adx ady adz angle sk], [sweep-line sx sy sz ex ey ez sk], [sweep-helix radius pitch height turns sk], [loft #[sk1 sk2 …]]\n" +
      "Assemblies: [assembly #[parts] #[instances] #[joints] ground-id] with [part name solid \"material\"], [instance name part-name x y z], [revolute-joint …], [prismatic-joint …], [fixed-joint …], [ball-joint …]\n" +
      "Pipe: [pipe [cube 50 30 5] [difference [cylinder 3 10]] [fillet 1.0]]\n" +
      "Let bindings: [let body [cube 50 30 5]]\n" +
      "Scene: [root solid \"material-name\"]\n" +
      "Modules: [use bracket] then [bracket.plate] — multi-file projects work here, with sources passed in `modules` (or read from `base_dir`); [use bracket :as b] aliases, [use bracket [plate]] imports selectively, and `pub` in a module picks what it exports",
    inputSchema: createCadLoonSchema,
    handler: async (args, ctx) => {
      // Hydrate any by-name macros from the durable per-user store before
      // composing (cold serverless instances start with an empty registry).
      const useLoons = Array.isArray(args.use_loons)
        ? (args.use_loons as string[])
        : undefined;
      if (useLoons?.length) {
        await hydrateMacros(ctx.user, useLoons).catch(() => {});
      }
      const result = createCadLoon(args, ctx.engine) as ToolResult;
      // Session-targeted authoring: write the evaluated document into the
      // caller's open session instead of minting a fresh one, so
      // open_document → create_cad_loon stays on one document_id. Validated
      // via getSession so an unknown id fails loudly, not silently forks.
      const targetId =
        typeof args.document_id === "string" ? args.document_id : null;
      if (targetId) getSession(targetId); // unknown id fails loudly
      // Attach the integrity certificate to the largest mutation of all:
      // authoring a whole document. The loon evaluation is cheap relative to
      // the mesh evaluation computeIntegrity runs anyway.
      try {
        const doc = ctx.engine.evalVcadSourceWithModules(
          composeLoonProgram(args),
          composeLoonModules(args),
        );
        if (doc) {
          if (targetId) documents.set(targetId, doc);
          const integrity = computeIntegrity(doc, ctx.engine);
          if (integrity) {
            appendIntegrity(result, integrity);
            if (targetId) recordTriangles(targetId, integrity.triangles);
          }
        }
      } catch {
        // Best-effort: never fail the authoring call over accounting.
      }
      return result;
    },
    behavior: behavior({ writesDoc: true, geometry: true, mount: true }),
  },
];
