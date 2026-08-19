/**
 * create_cad_loon tool — evaluate loon source to produce a CAD document.
 */

import { readFileSync } from "node:fs";
import { delimiter, isAbsolute, join, resolve, sep } from "node:path";
import type { Engine } from "@vcad/engine";
import { toVCode } from "@vcad/ir";
import type { Document } from "@vcad/ir";
import { appendIntegrity, computeIntegrity } from "./integrity.js";
import { hydrateMacros, macroPrelude, type InlineLoon } from "./loon-macros.js";
import { documents, getSession, recordTriangles } from "./session-core.js";
import { attachLoonSource } from "./source-provenance.js";
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

/** The loon lib path: `$VCAD_LOON_PATH` directories, searched — in order —
 *  for a module that is not beside the importing file. Same variable and
 *  same order as the native `vcad` / `vcad-render` resolvers
 *  (`crates/vcad-loon/src/modules.rs`). */
export function loonLibDirs(env: NodeJS.ProcessEnv = process.env): string[] {
  const raw = env.VCAD_LOON_PATH;
  if (!raw) return [];
  return raw.split(delimiter).filter((d) => d.length > 0);
}

/**
 * The in-memory module map `[use ...]` resolves against: explicit `modules`,
 * plus inline `loons` (a macro passed by value is also an importable
 * module), plus — when `base_dir` is given — files read from disk, following
 * nested imports transitively. A module not found beside the importer is
 * looked for in each `$VCAD_LOON_PATH` directory (the lib path), so a
 * project and the CLI resolve the same `[use]` the same way.
 *
 * A name the server can't find is simply left out; the kernel then reports
 * the missing module with loon's own error, rather than the server guessing.
 */
export function composeLoonModules(input: unknown): Record<string, string> {
  const { source, loons, modules, base_dir } = input as CreateLoonInput;
  const map: Record<string, string> = {};
  for (const m of loons ?? []) map[m.name] = m.source;
  Object.assign(map, modules ?? {});

  const dirs = [...(base_dir ? [base_dir] : []), ...loonLibDirs()];
  if (dirs.length) {
    const pending = [
      ...importedNames(source),
      ...Object.values(map).flatMap(importedNames),
    ];
    const seen = new Set<string>();
    while (pending.length) {
      const name = pending.pop() as string;
      if (seen.has(name) || map[name]) continue;
      seen.add(name);
      let src: string | null = null;
      for (const dir of dirs) {
        src = readModule(dir, name);
        if (src !== null) break;
      }
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
  const { format = "vcode", base_dir } = input as CreateLoonInput;
  const source = composeLoonProgram(input);

  const modules = composeLoonModules(input);
  const doc = engine.evalVcadSourceWithModules(source, modules);
  // Record what made this document. The COMPOSED program is stored rather
  // than the bare `source` argument: it inlines the macro prelude, so the
  // stored text re-evaluates to the same geometry with no dependency on a
  // macro registry that a restart may have emptied.
  if (doc) attachLoonSource(doc, { text: source, modules, base_dir });
  if (!doc) {
    // Distinguish "no loon at all" from "loon, but a kernel too old to
    // resolve modules" — otherwise a stale kernel reads as a broken program.
    const text = Object.keys(modules).length
      ? "Error: this kernel build cannot resolve loon modules ([use ...]) — " +
        "update the kernel, or inline the modules into `source`"
      : "Error: Loon evaluation not supported by this engine build";
    return { content: [{ type: "text", text }] };
  }

  const { text, fellBack } = serializeDocument(doc, format);
  const content: Array<{ type: "text"; text: string }> = [{ type: "text", text }];
  if (fellBack) {
    content.push({
      type: "text",
      text:
        `Note: returned the JSON IR instead of VCode — this document uses ops VCode ` +
        `cannot express (${fellBack}). The document itself is complete and every ` +
        `downstream tool works on it; only the compact text form is unavailable.`,
    });
  }

  const note = parametricNote(source, modules, engine, doc);
  if (note) content.push({ type: "text", text: note });

  return { content };
}

/**
 * Render the document as the caller asked, falling back to the JSON IR when
 * VCode cannot express what the program built.
 *
 * VCode is a deliberately compact subset — it has no opcode for a sheet-metal
 * bend graph, an imported vendor part, or a PCB. Failing the whole call over
 * the *display* format would mean a loon program that models a perfectly good
 * part cannot be authored at all, even though the document is fine and every
 * downstream tool (unfold, export, inspect) works on it. So degrade the text
 * and say so, rather than refusing the geometry.
 */
function serializeDocument(
  doc: Document,
  format: "vcode" | "json",
): { text: string; fellBack: string | null } {
  if (format === "json") return { text: JSON.stringify(doc, null, 2), fellBack: null };
  try {
    return { text: toVCode(doc), fellBack: null };
  } catch (e) {
    const why = e instanceof Error ? e.message : String(e);
    return { text: JSON.stringify(doc, null, 2), fellBack: why };
  }
}

/**
 * Report the parametric surface a program declared, and anything the bridge
 * could not preserve.
 *
 * Without this an author has no way to tell that `pitch_axis_x` reached the
 * document as a live parameter rather than being inlined — the geometry looks
 * the same either way. Silent on programs that declare nothing, and silent on
 * kernels too old to answer.
 */
function parametricNote(
  source: string,
  modules: Record<string, string>,
  engine: Engine,
  doc: Document,
): string | null {
  const names = Object.keys(doc.parameters ?? {}).sort();
  if (!names.length) return null;

  const lines: string[] = [];
  // Only base (numeric) parameters are knobs. Derived ones follow from them —
  // they never appear in a binding formula, so flagging them as driving
  // nothing would be noise, and set_parameters refuses them anyway.
  const value = (n: string) => (doc.parameters as Record<string, { value: number | string }>)[n].value;
  const bound = new Set(
    Object.values(doc.bindings ?? {}).flatMap((expr) =>
      typeof expr === "string" ? (expr.match(/[A-Za-z_]\w*/g) ?? []) : [],
    ),
  );
  const base = names.filter((n) => typeof value(n) === "number");
  const derived = names.filter((n) => typeof value(n) === "string");

  if (base.length) {
    lines.push(
      `Parameters (${base.length}, settable): ${base
        .map((n) => (bound.has(n) ? n : `${n} (drives nothing)`))
        .join(", ")}`,
    );
  }
  if (derived.length) {
    lines.push(
      `Derived (${derived.length}, follow from the above): ${derived
        .map((n) => `${n} = "${value(n)}"`)
        .join(", ")}`,
    );
  }
  const datums = Object.keys(doc.datums ?? {}).sort();
  if (datums.length) lines.push(`Datums (${datums.length}): ${datums.join(", ")}`);
  if (base.length) {
    lines.push("Change any settable parameter with set_parameters — no re-authoring needed.");
  }

  let warnings: string[] = [];
  try {
    warnings = engine.evalVcadSourceParametric(source, modules)?.warnings ?? [];
  } catch {
    // Best-effort: never fail authoring over diagnostics.
  }
  for (const w of warnings) lines.push(`- ${w}`);
  return lines.join("\n");
}

export const toolDefs: ToolDef[] = [
  {
    name: "create_cad_loon",
    pack: null,
    description:
      "The preferred authoring tool for whole parts and multi-feature models — one call, full vocabulary. Create a CAD document from loon source code. Loon is a Lisp-like language for parametric CAD — the FULL modeling vocabulary (patterns, sketches, extrude/revolve/sweep/loft, assemblies) is available here even where no dedicated MCP tool exists. For incremental single-node edits to an open session, use create/update/delete instead.\n\n" +
      "Primitives: [cube x y z], [cylinder r h], [sphere r], [cone r-bottom r-top h], [torus major-r minor-r], [wedge x y z], [prism sides radius height]. Segment count is the kernel default (32); pin it with [cylinder-n r h n] / [sphere-n r n] / [cone-n rb rt h n] / [torus-n R r n] where the facets are load-bearing (a bore that must accept a real shaft)\n" +
      "Imports — place a purchased part instead of approximating it, so fit checks test the real envelope and not one you invented: [import-step \"vendor/x6-60.step\"], [import-step-body path index] for a multi-body file, [import-mesh \"part.stl\"], [import-mesh-scaled sx sy sz path]. Relative paths resolve against base_dir / the module source\n" +
      "Sheet metal (subject-last, so it threads through pipe) — author a cut-and-bent part as a bend chain and it KEEPS its bends, so sheet_metal_unfold is exact instead of inferred back out of a solid by flat_pattern_from_solid: [sheet-base-flange-rect width depth thickness \"al-soft\"] (or [sheet-base-flange #[x0 y0 …] #[holes] t material] for an arbitrary outline) then [sheet-edge-flange edge length angle-deg s], [sheet-jog edge offset length s], [sheet-hem edge length s], [sheet-bend-relief s]. `edge` is an outline index, or \"south\"/\"east\"/\"north\"/\"west\" on a rectangular base flange (edge 0 is the -Y edge, CCW from there). Each has an -at form taking panel id, radius, direction (\"up\"/\"down\") and K-factor, where 0.0 means the default; -shop variants resolve every bend through a shop table. Sheet nodes are a bend graph, NOT solids — never union, transform or fillet them\n" +
      "Booleans (subject-last): [difference tool subject], [union other subject], [intersection other subject]\n" +
      "Transforms (subject-last): [translate x y z s], [rotate rx ry rz s], [scale sx sy sz s], [mirror ox oy oz nx ny nz s] (plane through the origin point with that normal) — plus the axis sugar [mirror-x s] / [mirror-y s] / [mirror-z s], which mirror through the origin, negating that one coordinate. NEVER hand-mirror by negating coordinates; use these.\n" +
      "Features: [fillet r s], [chamfer d s], [shell t s]\n" +
      "Patterns (subject-last): [linear-pattern dx dy dz count spacing s], [circular-pattern ox oy oz ax ay az count angle s] — e.g. a bolt circle is [circular-pattern 0 0 0 0 0 1 6 360 bolt-hole]\n" +
      "Symmetric patterns (subject-last): [mirror-pattern nx ny nz s] and its sugar [mirror-pattern-x s] / -y / -z union a solid with its mirror image (a left/right pair in one expression, so the halves can't drift apart); [quad-pattern s] is the 4-fold X-and-Y version — a quadruped's legs, a 4-post frame, a vehicle chassis\n" +
      "Sketches: [sketch ox oy oz xx xy xz yx yy yz #[segments]] with [line x1 y1 x2 y2] and [arc x1 y1 x2 y2 cx cy ccw]\n" +
      "Sketch ops (sketch-last): [extrude dx dy dz sk], [revolve aox aoy aoz adx ady adz angle sk], [sweep-line sx sy sz ex ey ez sk], [sweep-helix radius pitch height turns sk], [loft #[sk1 sk2 …]], [loft-closed #[sk1 sk2 …]]\n" +
      "Assemblies: [assembly #[parts] #[instances] #[joints] ground-id] with [part name solid \"material\"], [instance name part-name x y z], [revolute-joint …], [prismatic-joint …], [fixed-joint …], [ball-joint …]\n" +
      "Assembly symmetry: author ONE side as an assembly, then [mirror-group-x \"-r\" side] (or -y / -z) returns it plus a mirrored, suffixed copy — parts reflected, placements and joint anchors mirrored, and joint axes flipped by the correct rule (a hinge across its own mirror normal keeps its axis; the other two flip), so the same joint state drives both sides symmetrically. Splice it back with [assembly-join chassis mirrored]. NEVER hand-mirror an assembly.\n" +
      "Pipe: [pipe [cube 50 30 5] [difference [cylinder 3 10]] [fillet 1.0]]\n" +
      "Let bindings: [let body [cube 50 30 5]] — note these are inlined and do NOT survive into the document; use [defparam ...] for a value you want to change later\n" +
      "Parameters (survive into the document and are settable afterwards with set_parameters, and differentiable with parameter_gradient): [defparam pitch_axis_x 310.0], [defparam wall \"bore * 0.2\"] for derived values, plus optional :unit/:min/:max/:description. Names must be identifier-safe (underscores, not dashes)\n" +
      "Datums — named reference geometry, so two parts cannot each hold their own copy of a shared plane: [datum-plane \"femur_inner\" y 131.0], [datum-axis \"pitch\" x 0 0 310], [datum-point \"hip\" 0 0 310], read back with [datum \"femur_inner\"], [datum+ \"femur_inner\" 3.0] (3 mm outboard), [datum-x/-y/-z \"pitch\"]\n" +
      "Stacks — declarative packing, where each running clearance is a named value instead of an arbitrary number: [stack y \"leg\" 131.0 [lane \"femur_inner\" 5.0] [gap \"idler_run\" 1.0] [lane \"idler_boss\" 3.0]] declares datum planes leg_femur_inner_lo/_hi, leg_idler_boss_lo/_hi and leg_end; widening leg_idler_run slides everything outboard of it\n" +
      "Scene: [root solid \"material-name\"], with [material name r g b metallic roughness] to define one\n" +
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
        const composed = composeLoonProgram(args);
        const modules = composeLoonModules(args);
        const doc = ctx.engine.evalVcadSourceWithModules(composed, modules);
        if (doc) {
          attachLoonSource(doc, {
            text: composed,
            modules,
            base_dir:
              typeof args.base_dir === "string" ? args.base_dir : undefined,
          });
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
