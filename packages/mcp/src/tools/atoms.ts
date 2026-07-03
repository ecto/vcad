/**
 * Atomic / molecular design & simulation tools.
 *
 * Structure import, inspection, energy minimization, molecular dynamics, a
 * property-target inverse-design loop, and a ball-and-stick renderer — the
 * atomic-domain analog of the CAD tools. Backed by the `vcad-kernel-atoms`
 * WASM bindings via `@vcad/engine`.
 *
 * Units are Å / eV / amu / fs / e / K, not the millimeter CAD convention.
 */

import type { MoleculeSystem } from "@vcad/ir";
import {
  parseXyz,
  inspectMolecule,
  minimizeEnergy,
  homogenizeMaterial,
  buildMoleculeReceipt,
  MdEnv,
  type MdConfig,
} from "@vcad/engine";

type ToolResult = { content: Array<{ type: "text"; text: string }> };

function ok(data: unknown): ToolResult {
  return { content: [{ type: "text", text: JSON.stringify(data, null, 2) }] };
}
function fail(err: unknown): ToolResult {
  const message = err instanceof Error ? err.message : String(err);
  return { content: [{ type: "text", text: JSON.stringify({ error: message }) }] };
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

export const loadStructureSchema = {
  type: "object" as const,
  properties: {
    xyz: {
      type: "string" as const,
      description: "Structure in XYZ or extended-XYZ format (Lattice=\"...\" for periodic cells).",
    },
    molecule: {
      type: "object" as const,
      description: "A MoleculeSystem object (alternative to xyz).",
    },
  },
};

export const inspectMoleculeSchema = {
  type: "object" as const,
  properties: {
    molecule: { type: "object" as const, description: "MoleculeSystem to analyze." },
  },
  required: ["molecule"],
};

const configSchema = {
  type: "object" as const,
  description:
    "Force-field / integrator config: forceField ('auto'|'lj'|'bonds'|'mlip-stub', default auto), epsilon, sigma, cutoff, useBonds, bondK, bondR0, useCoulomb, dt, thermostatK, thermostatTau.",
};

export const minimizeEnergySchema = {
  type: "object" as const,
  properties: {
    molecule: { type: "object" as const, description: "MoleculeSystem to relax." },
    config: configSchema,
    max_iters: { type: "number" as const, description: "Max FIRE iterations (default 2000)." },
    force_tol: { type: "number" as const, description: "Force tolerance eV/Å (default 1e-4)." },
  },
  required: ["molecule"],
};

export const mdRunSchema = {
  type: "object" as const,
  properties: {
    molecule: { type: "object" as const, description: "Starting MoleculeSystem." },
    config: configSchema,
    steps: { type: "number" as const, description: "Number of velocity-Verlet steps." },
  },
  required: ["molecule", "steps"],
};

export const designMaterialSchema = {
  type: "object" as const,
  properties: {
    molecule: { type: "object" as const, description: "Base MoleculeSystem to reshape." },
    property: {
      type: "string" as const,
      enum: ["nn_distance", "radius_of_gyration"],
      description: "Property to drive to the target.",
    },
    target: { type: "number" as const, description: "Target property value (Å)." },
    scale_bounds: {
      type: "array" as const,
      items: { type: "number" as const },
      description: "[min, max] isotropic scale factor to search (default [0.5, 2.0]).",
    },
  },
  required: ["molecule", "property", "target"],
};

export const homogenizeMaterialSchema = {
  type: "object" as const,
  properties: {
    molecule: {
      type: "object" as const,
      description: "MoleculeSystem to homogenize — must have a periodic cell.",
    },
    force_field: {
      type: "string" as const,
      enum: ["auto", "lj", "bonds", "mlip-stub"],
      description: "Force field (default auto: bonds for bonded molecules, LJ otherwise).",
    },
    epsilon: { type: "number" as const, description: "LJ well depth (eV)." },
    sigma: { type: "number" as const, description: "LJ sigma (Å)." },
    cutoff: { type: "number" as const, description: "LJ cutoff (Å)." },
    bond_k: {
      type: "number" as const,
      description:
        "Harmonic bond force constant (eV/Å²) for the 'bonds'/'auto' force field (default 20). Moduli scale linearly with it.",
    },
    strain: {
      type: "number" as const,
      description: "Strain amplitude for the second differences (default 2e-3).",
    },
    relax_internal: {
      type: "boolean" as const,
      description: "Re-relax internal coordinates under each strained cell (default true).",
    },
  },
  required: ["molecule"],
};

export const renderMoleculeSchema = {
  type: "object" as const,
  properties: {
    molecule: { type: "object" as const, description: "MoleculeSystem to render." },
    width_px: { type: "number" as const, description: "Image width in px (default 640, max 1600)." },
    representation: {
      type: "string" as const,
      enum: ["ball_and_stick", "space_filling"],
      description: "Rendering style (default ball_and_stick).",
    },
  },
  required: ["molecule"],
};

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async function resolveMolecule(args: {
  xyz?: string;
  molecule?: MoleculeSystem;
}): Promise<MoleculeSystem> {
  if (args.xyz) return parseXyz(args.xyz);
  if (args.molecule) return args.molecule;
  throw new Error("provide either `xyz` or `molecule`");
}

/** Import a structure and return the molecule plus a summary. */
export async function loadStructure(input: unknown): Promise<ToolResult> {
  try {
    const args = input as { xyz?: string; molecule?: MoleculeSystem };
    const molecule = await resolveMolecule(args);
    const report = await inspectMolecule(molecule);
    return ok({ molecule, report });
  } catch (err) {
    return fail(err);
  }
}

/** Structural analysis of a molecule. */
export async function inspectMoleculeTool(input: unknown): Promise<ToolResult> {
  try {
    const args = input as { molecule: MoleculeSystem };
    return ok(await inspectMolecule(args.molecule));
  } catch (err) {
    return fail(err);
  }
}

/** Relax a structure to a local energy minimum. */
export async function minimizeEnergyTool(input: unknown): Promise<ToolResult> {
  try {
    const args = input as {
      molecule: MoleculeSystem;
      config?: MdConfig;
      max_iters?: number;
      force_tol?: number;
    };
    const { result, molecule } = await minimizeEnergy(
      args.molecule,
      args.config ?? {},
      args.max_iters ?? 2000,
      args.force_tol ?? 1e-4,
    );
    const receipt = await buildMoleculeReceipt(
      args.molecule,
      args.config?.forceField ?? "lj",
      "minimize",
      { max_iters: args.max_iters ?? 2000, force_tol: args.force_tol ?? 1e-4 },
      [
        ["energy", result.energy],
        ["max_force", result.maxForce],
      ],
    );
    return ok({ result, molecule, receipt });
  } catch (err) {
    return fail(err);
  }
}

/** Run molecular dynamics and return the trajectory endpoint. */
export async function mdRun(input: unknown): Promise<ToolResult> {
  try {
    const args = input as { molecule: MoleculeSystem; config?: MdConfig; steps: number };
    const env = await MdEnv.create(args.molecule, args.config ?? {});
    try {
      const observation = env.run(args.steps);
      const molecule = env.molecule();
      return ok({ observation, molecule });
    } finally {
      env.free();
    }
  } catch (err) {
    return fail(err);
  }
}

function scaleMolecule(mol: MoleculeSystem, s: number): MoleculeSystem {
  const com: [number, number, number] = [0, 0, 0];
  for (const p of mol.positions) {
    com[0] += p[0];
    com[1] += p[1];
    com[2] += p[2];
  }
  const n = Math.max(1, mol.positions.length);
  com[0] /= n;
  com[1] /= n;
  com[2] /= n;
  const positions = mol.positions.map(
    (p) =>
      [
        com[0] + (p[0] - com[0]) * s,
        com[1] + (p[1] - com[1]) * s,
        com[2] + (p[2] - com[2]) * s,
      ] as [number, number, number],
  );
  const cell = mol.cell
    ? {
        ...mol.cell,
        a: mol.cell.a.map((x) => x * s) as [number, number, number],
        b: mol.cell.b.map((x) => x * s) as [number, number, number],
        c: mol.cell.c.map((x) => x * s) as [number, number, number],
      }
    : mol.cell;
  return { ...mol, positions, cell };
}

/**
 * Inverse design over a single isotropic scale DOF: bisection-search the scale
 * factor that drives a geometric property to a target. This is the MCP-layer,
 * geometry-objective realization of the loop; the energy-objective inverse
 * design (gradients through the simulation) lives in the Rust `inverse` module.
 */
export async function designMaterial(input: unknown): Promise<ToolResult> {
  try {
    const args = input as {
      molecule: MoleculeSystem;
      property: "nn_distance" | "radius_of_gyration";
      target: number;
      scale_bounds?: [number, number];
    };
    const [lo0, hi0] = args.scale_bounds ?? [0.5, 2.0];

    const measure = async (mol: MoleculeSystem): Promise<number> => {
      const r = await inspectMolecule(mol);
      if (args.property === "radius_of_gyration") return r.radius_of_gyration;
      // nearest-neighbor distance via bbox-independent min pair distance.
      let min = Infinity;
      const P = mol.positions;
      for (let i = 0; i < P.length; i++) {
        for (let j = i + 1; j < P.length; j++) {
          const dx = P[i][0] - P[j][0];
          const dy = P[i][1] - P[j][1];
          const dz = P[i][2] - P[j][2];
          min = Math.min(min, Math.hypot(dx, dy, dz));
        }
      }
      return min;
    };

    // Property scales monotonically with the isotropic scale factor, so bisect.
    let lo = lo0;
    let hi = hi0;
    const base = args.molecule;
    const pLo = await measure(scaleMolecule(base, lo));
    const pHi = await measure(scaleMolecule(base, hi));
    const increasing = pHi >= pLo;
    let s = 1.0;
    let prop = await measure(base);
    for (let it = 0; it < 60; it++) {
      s = 0.5 * (lo + hi);
      prop = await measure(scaleMolecule(base, s));
      const tooSmall = increasing ? prop < args.target : prop > args.target;
      if (tooSmall) lo = s;
      else hi = s;
      if (Math.abs(prop - args.target) < 1e-4) break;
    }
    const molecule = scaleMolecule(base, s);
    const receipt = await buildMoleculeReceipt(
      base,
      "geometry",
      "design_material",
      { property: args.property, target: args.target },
      [
        ["scale", s],
        ["property", prop],
      ],
    );
    return ok({ scale: s, property: prop, target: args.target, molecule, receipt });
  } catch (err) {
    return fail(err);
  }
}

/**
 * Homogenize a periodic crystal into bulk material properties — density
 * (kg/m³), cubic elastic constants C11/C12/C44 and VRH isotropic moduli (GPa)
 * — the atoms-to-continuum bridge. Requires a fully periodic cell.
 */
export async function homogenizeMaterialTool(input: unknown): Promise<ToolResult> {
  try {
    const args = input as {
      molecule: MoleculeSystem;
      force_field?: "auto" | "lj" | "bonds" | "mlip-stub";
      epsilon?: number;
      sigma?: number;
      cutoff?: number;
      bond_k?: number;
      strain?: number;
      relax_internal?: boolean;
    };
    const config: MdConfig = {};
    if (args.force_field !== undefined) config.forceField = args.force_field;
    if (args.epsilon !== undefined) config.epsilon = args.epsilon;
    if (args.sigma !== undefined) config.sigma = args.sigma;
    if (args.cutoff !== undefined) config.cutoff = args.cutoff;
    if (args.bond_k !== undefined) config.bondK = args.bond_k;
    if (args.strain !== undefined) config.strain = args.strain;
    if (args.relax_internal !== undefined) config.relaxInternal = args.relax_internal;
    const card = await homogenizeMaterial(args.molecule, config);
    return ok(card);
  } catch (err) {
    return fail(err);
  }
}

// CPK fallback colors (sRGB 0..255) for common elements; species.color wins.
const CPK: Record<string, [number, number, number]> = {
  H: [255, 255, 255],
  C: [143, 143, 143],
  N: [48, 80, 248],
  O: [255, 13, 13],
  F: [144, 224, 80],
  Na: [171, 92, 242],
  Mg: [138, 255, 0],
  P: [255, 128, 0],
  S: [255, 255, 48],
  Cl: [31, 240, 31],
  Fe: [224, 102, 51],
  Au: [255, 209, 35],
};
const DEFAULT_RADIUS: Record<string, number> = {
  H: 0.31,
  C: 0.76,
  N: 0.71,
  O: 0.66,
  S: 1.05,
  P: 1.07,
};

function elementColor(mol: MoleculeSystem, i: number): string {
  const sp = mol.species[mol.speciesIdx[i]];
  if (sp?.color) {
    const [r, g, b] = sp.color;
    return `rgb(${Math.round(r * 255)},${Math.round(g * 255)},${Math.round(b * 255)})`;
  }
  const c = CPK[sp?.element ?? "C"] ?? [200, 100, 200];
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}

/**
 * Render a molecule as an isometric ball-and-stick SVG (depth-sorted, CPK
 * colors, radial-shaded atoms). This is the self-contained interim renderer;
 * the world-class path is the WebGPU ray tracer's impostor-sphere buffer plus a
 * headless-wgpu harness returning PNG (Track B).
 */
export async function renderMolecule(input: unknown): Promise<ToolResult> {
  try {
    const args = input as {
      molecule: MoleculeSystem;
      width_px?: number;
      representation?: "ball_and_stick" | "space_filling";
    };
    const mol = args.molecule;
    const W = Math.min(1600, Math.max(200, Math.round(args.width_px ?? 640)));
    const H = W;
    const spaceFilling = args.representation === "space_filling";

    // Isometric projection (Z-up): rotate so the structure reads in 3/4 view.
    const project = (p: [number, number, number]): [number, number, number] => {
      const ax = Math.PI / 6; // 30° tilt
      const az = Math.PI / 4; // 45° yaw
      const x1 = p[0] * Math.cos(az) - p[1] * Math.sin(az);
      const y1 = p[0] * Math.sin(az) + p[1] * Math.cos(az);
      const y2 = y1 * Math.cos(ax) - p[2] * Math.sin(ax);
      const depth = y1 * Math.sin(ax) + p[2] * Math.cos(ax);
      return [x1, y2, depth];
    };

    const proj = mol.positions.map(project);
    let minX = Infinity,
      minY = Infinity,
      maxX = -Infinity,
      maxY = -Infinity;
    for (const [x, y] of proj) {
      minX = Math.min(minX, x);
      minY = Math.min(minY, y);
      maxX = Math.max(maxX, x);
      maxY = Math.max(maxY, y);
    }
    const pad = 0.12 * Math.max(maxX - minX, maxY - minY, 1);
    minX -= pad;
    minY -= pad;
    maxX += pad;
    maxY += pad;
    const span = Math.max(maxX - minX, maxY - minY, 1);
    const scale = (W * 0.9) / span;
    const sx = (x: number) => (x - minX) * scale + (W - (maxX - minX) * scale) / 2;
    const sy = (y: number) => H - ((y - minY) * scale + (H - (maxY - minY) * scale) / 2);

    const order = proj.map((_, i) => i).sort((a, b) => proj[a][2] - proj[b][2]);

    const parts: string[] = [];
    parts.push(
      `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">`,
    );
    parts.push(`<rect width="${W}" height="${H}" fill="#0d1117"/>`);
    parts.push(`<defs>`);
    parts.push(
      `<radialGradient id="shade" cx="35%" cy="35%" r="65%"><stop offset="0%" stop-color="white" stop-opacity="0.55"/><stop offset="100%" stop-color="black" stop-opacity="0.25"/></radialGradient>`,
    );
    parts.push(`</defs>`);

    // Bonds first (behind atoms) unless space-filling.
    if (!spaceFilling) {
      for (const b of mol.bonds ?? []) {
        const a = proj[b.a];
        const c = proj[b.b];
        if (!a || !c) continue;
        parts.push(
          `<line x1="${sx(a[0]).toFixed(1)}" y1="${sy(a[1]).toFixed(1)}" x2="${sx(c[0]).toFixed(1)}" y2="${sy(c[1]).toFixed(1)}" stroke="#8b949e" stroke-width="${Math.max(1, scale * 0.12)}" stroke-linecap="round"/>`,
        );
      }
    }

    for (const i of order) {
      const p = proj[i];
      const sp = mol.species[mol.speciesIdx[i]];
      const baseR = sp?.radius ?? DEFAULT_RADIUS[sp?.element ?? "C"] ?? 0.7;
      const r = (spaceFilling ? baseR * 1.6 : baseR * 0.55) * scale;
      const cx = sx(p[0]);
      const cy = sy(p[1]);
      const col = elementColor(mol, i);
      parts.push(`<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="${r.toFixed(1)}" fill="${col}"/>`);
      parts.push(
        `<circle cx="${cx.toFixed(1)}" cy="${cy.toFixed(1)}" r="${r.toFixed(1)}" fill="url(#shade)"/>`,
      );
    }
    parts.push(`</svg>`);
    const svg = parts.join("");
    return {
      content: [{ type: "text", text: svg }],
    };
  } catch (err) {
    return fail(err);
  }
}
