/**
 * Hierarchical schematic sheets — flatten a SchematicSheet carrying a
 * `hierarchy` into one flat sheet (components + explicit nets + power flags).
 *
 * Mirrors `crates/vcad-ecad-schematic/src/hierarchy.rs` (same scoping rules,
 * ref mangling, auto-placement and diagnostic messages); the kernel copy
 * serves Rust consumers (CLI, KiCad writer, kernel ERC), this one serves the
 * MCP tools without waiting on a WASM rebuild.
 *
 * Scoping: inside a child sheet a net name is LOCAL (flattens to
 * `<path>/<name>`, path = instance names joined by `/`) unless it is a PORT
 * (declared in `ports`, or a Hierarchical label), which resolves through the
 * instance's `connections` to the parent's net, or GLOBAL (`global_nets`, or a
 * Global label in any sheet), which keeps its name everywhere.
 */
import type {
  PowerFlag,
  SchematicComponent,
  SchematicSheet,
  SheetDefinition,
  SheetInstance,
} from "@vcad/ir";

/** Spacing between auto-placed instances (matches the kernel). */
export const AUTO_INSTANCE_STRIDE = 100_000;

export interface HierarchyDiagnostic {
  severity: "Error" | "Warning";
  message: string;
}

export interface FlatInstance {
  path: string;
  sheet: string;
  refs: Record<string, string>;
}

export interface FlattenResult {
  sheet: SchematicSheet;
  diagnostics: HierarchyDiagnostic[];
  instances: FlatInstance[];
}

/** Child-sheet connectivity: net name → pin refs ("U1.3"). */
export type Netlister = (sheet: SchematicSheet) => Promise<Map<string, string[]>>;

/** `U1` + 100 → `U101`; a ref without trailing digits gets the offset appended. */
export function offsetRef(ref: string, offset: number): string {
  const m = /^(.*?)(\d+)$/.exec(ref);
  if (!m) return `${ref}${offset}`;
  return `${m[1]}${Number(m[2]) + offset}`;
}

const isAutoName = (n: string) => /^NET-\d+$/.test(n);

function portsOf(def: SheetDefinition): Set<string> {
  const ports = new Set((def.ports ?? []).map((p) => p.name));
  for (const l of def.body.labels ?? []) if (l.scope === "Hierarchical") ports.add(l.name);
  return ports;
}

export function isHierarchical(sheet: SchematicSheet | undefined | null): boolean {
  return !!sheet?.hierarchy;
}

/** Flatten; a sheet without a hierarchy is returned unchanged. */
export async function flattenSchematic(
  sheet: SchematicSheet,
  netlister: Netlister,
): Promise<FlattenResult> {
  const h = sheet.hierarchy;
  if (!h) return { sheet, diagnostics: [], instances: [] };

  const diagnostics: HierarchyDiagnostic[] = [];
  const err = (message: string) => diagnostics.push({ severity: "Error", message });
  const warn = (message: string) => diagnostics.push({ severity: "Warning", message });

  const defs = new Map<string, SheetDefinition>();
  for (const def of h.sheets ?? []) {
    if (defs.has(def.name)) err(`Duplicate sheet definition '${def.name}'`);
    else defs.set(def.name, def);
  }

  const globals = new Set(h.global_nets ?? []);
  for (const l of [...(sheet.labels ?? []), ...(h.sheets ?? []).flatMap((d) => d.body.labels ?? [])]) {
    if (l.scope === "Global") globals.add(l.name);
  }
  const rootNames = new Set<string>([
    ...(sheet.labels ?? []).map((l) => l.name),
    ...Object.keys(sheet.nets ?? {}),
  ]);

  const useCount = new Map<string, number>();
  const countUses = (instances: SheetInstance[], stack: string[]) => {
    for (const inst of instances) {
      useCount.set(inst.sheet, (useCount.get(inst.sheet) ?? 0) + 1);
      if (stack.includes(inst.sheet)) continue;
      const def = defs.get(inst.sheet);
      if (def) countUses(def.instances ?? [], [...stack, def.name]);
    }
  };
  countUses(h.instances ?? [], []);

  const outComponents: SchematicComponent[] = [];
  const outNets = new Map<string, string[]>(Object.entries(sheet.nets ?? {}).map(([k, v]) => [k, [...v]]));
  const outFlags: PowerFlag[] = [];
  const instances: FlatInstance[] = [];
  const unusedPortReported = new Set<string>();
  let autoSlot = 0;

  const elaborateAll = async (
    insts: SheetInstance[],
    parentPath: string,
    parentOffset: { x: number; y: number },
    resolveParent: (n: string) => string,
    stack: string[],
  ) => {
    const seen = new Set<string>();
    const where = parentPath === "" ? "the root sheet" : `sheet instance '${parentPath}'`;
    for (const inst of insts) {
      if (!inst.name || inst.name.includes("/")) {
        err(`Sheet instance name '${inst.name}' in ${where} must be non-empty and contain no '/'`);
        continue;
      }
      if (seen.has(inst.name)) {
        err(`Duplicate sheet instance name '${inst.name}' in ${where}`);
        continue;
      }
      seen.add(inst.name);
      const def = defs.get(inst.sheet);
      if (!def) {
        err(`Sheet instance '${inst.name}' references unknown sheet '${inst.sheet}'`);
        continue;
      }
      if (stack.includes(def.name)) {
        err(`Sheet recursion: instance '${inst.name}' places sheet '${def.name}' inside itself`);
        continue;
      }
      const path = parentPath === "" ? inst.name : `${parentPath}/${inst.name}`;
      const binding = new Map<string, string>();
      for (const [port, net] of Object.entries(inst.connections ?? {})) binding.set(port, resolveParent(net));
      autoSlot += 1;
      const offset = inst.position
        ? { x: parentOffset.x + inst.position.x, y: parentOffset.y + inst.position.y }
        : { x: 0, y: AUTO_INSTANCE_STRIDE * autoSlot };
      await elaborate(def, inst, path, binding, offset, [...stack, def.name]);
    }
  };

  const elaborate = async (
    def: SheetDefinition,
    inst: SheetInstance,
    path: string,
    binding: Map<string, string>,
    offset: { x: number; y: number },
    stack: string[],
  ) => {
    const ports = portsOf(def);
    for (const port of [...ports].sort()) {
      if (globals.has(port)) {
        err(`Net scope collision: port '${port}' of sheet '${def.name}' is also a global net`);
      }
    }
    for (const port of Object.keys(inst.connections ?? {}).sort()) {
      if (!ports.has(port)) {
        err(`Sheet instance '${path}' connects '${port}', which is not a port of sheet '${def.name}'`);
      }
    }
    for (const port of [...ports].sort()) {
      if (!binding.has(port)) warn(`Hierarchical port '${port}' of sheet instance '${path}' is not connected`);
    }
    if (def.body.hierarchy) {
      err(`Sheet '${def.name}' body has its own hierarchy — nest sheets with \`instances\` instead (ignored)`);
    }

    const resolve = (name: string): string => {
      if (ports.has(name)) {
        const bound = binding.get(name);
        if (bound !== undefined) return bound;
      } else if (globals.has(name)) {
        return name;
      }
      return `${path}/${name}`;
    };

    const repeated = (useCount.get(def.name) ?? 0) > 1;
    const refs: Record<string, string> = {};
    for (const comp of def.body.components ?? []) {
      const mapped = inst.ref_map?.[comp.ref];
      const flatRef =
        mapped !== undefined
          ? mapped
          : inst.ref_offset != null
            ? offsetRef(comp.ref, inst.ref_offset)
            : repeated
              ? `${comp.ref}_${inst.name}`
              : comp.ref;
      refs[comp.ref] = flatRef;
      outComponents.push({
        ...comp,
        ref: flatRef,
        position: { x: comp.position.x + offset.x, y: comp.position.y + offset.y },
      });
    }

    const body: SchematicSheet = { ...def.body, hierarchy: undefined };
    const nets = await netlister(body);
    const usedNames = new Set<string>();
    for (const [name, pins] of [...nets.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
      if (pins.length === 0) continue;
      if (isAutoName(name) && pins.length < 2) continue;
      usedNames.add(name);
      const flat = resolve(name);
      if (!ports.has(name) && !globals.has(name) && rootNames.has(flat)) {
        err(
          `Net scope collision: local net '${name}' of sheet instance '${path}' flattens to '${flat}', which the root sheet also uses`,
        );
      }
      const entry = outNets.get(flat) ?? [];
      for (const p of pins) {
        const dot = p.indexOf(".");
        const ref = p.slice(0, dot);
        entry.push(`${refs[ref] ?? ref}${p.slice(dot)}`);
      }
      outNets.set(flat, entry);
    }
    for (const child of def.instances ?? []) {
      for (const n of Object.values(child.connections ?? {})) usedNames.add(n);
    }
    for (const port of [...ports].sort()) {
      const key = `${def.name}\0${port}`;
      if (!usedNames.has(port) && !unusedPortReported.has(key)) {
        unusedPortReported.add(key);
        warn(`Hierarchical port '${port}' of sheet '${def.name}' connects to nothing inside the sheet`);
      }
    }
    for (const flag of def.body.power_flags ?? []) outFlags.push({ ...flag, net: resolve(flag.net) });

    instances.push({ path, sheet: def.name, refs });
    await elaborateAll(def.instances ?? [], path, offset, resolve, stack);
  };

  await elaborateAll(h.instances ?? [], "", { x: 0, y: 0 }, (n) => n, []);

  const nets: Record<string, string[]> = {};
  for (const [name, pins] of [...outNets.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    nets[name] = [...new Set(pins)];
  }
  const { hierarchy: _h, ...rest } = sheet;
  const flatSheet: SchematicSheet = {
    ...rest,
    components: [...(sheet.components ?? []), ...outComponents],
    ...(Object.keys(nets).length > 0 ? { nets } : { nets: undefined }),
    power_flags: [...(sheet.power_flags ?? []), ...outFlags],
  };
  if (flatSheet.nets === undefined) delete flatSheet.nets;
  if ((flatSheet.power_flags ?? []).length === 0) delete flatSheet.power_flags;
  return { sheet: flatSheet, diagnostics, instances };
}
