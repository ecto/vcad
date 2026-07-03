/**
 * TypeScript wrapper for the WASM atomic/molecular simulation.
 *
 * Mirrors {@link ./physics.ts}: a clean async API over the feature-gated
 * `vcad-kernel-atoms` bindings compiled into `@vcad/kernel-wasm`. Structure
 * I/O, energy minimization, molecular dynamics, inspection, and reproducibility
 * receipts. All numbers are in the atomic unit system (Å / eV / amu / fs / e /
 * K) — not the millimeter CAD convention.
 */

import type { MoleculeSystem } from "@vcad/ir";

/** Structural report from {@link inspectMolecule}. */
export interface MoleculeReport {
  atom_count: number;
  formula: string;
  species_counts: Record<string, number>;
  mass_amu: number;
  center_of_mass: [number, number, number];
  radius_of_gyration: number;
  bbox: [[number, number, number], [number, number, number]];
  bond_count: number;
  periodic: boolean;
}

/** Observation from a molecular-dynamics step. */
export interface MdObservation {
  step: number;
  potential_energy: number;
  kinetic_energy: number;
  total_energy: number;
  temperature: number;
  max_force: number;
}

/** Result of an energy minimization. */
export interface MinimizeResult {
  converged: boolean;
  iters: number;
  energy: number;
  maxForce: number;
}

/** Force-field / integrator configuration. */
export interface MdConfig {
  /**
   * "auto" (default), "lj", "bonds", or "mlip-stub". "auto" uses harmonic
   * bonds for a bonded molecule and Lennard-Jones for an unbonded system.
   */
  forceField?: "auto" | "lj" | "bonds" | "mlip-stub";
  /** LJ well depth (eV). */
  epsilon?: number;
  /** LJ sigma (Å). */
  sigma?: number;
  /** LJ / Coulomb cutoff (Å). */
  cutoff?: number;
  /** Include harmonic bonds from the molecule's bond list. */
  useBonds?: boolean;
  /** Harmonic bond force constant (eV/Å²). */
  bondK?: number;
  /** Harmonic bond equilibrium length (Å). */
  bondR0?: number;
  /** Include direct Coulomb over partial charges. */
  useCoulomb?: boolean;
  /** Timestep (fs). */
  dt?: number;
  /** Thermostat target temperature (K); omit or <=0 for NVE. */
  thermostatK?: number;
  /** Thermostat coupling time (fs). */
  thermostatTau?: number;
  /** Homogenization strain amplitude for the second differences (default 2e-3). */
  strain?: number;
  /** Re-relax internal coordinates under each strained cell (default true). */
  relaxInternal?: boolean;
}

/** Bulk material properties from {@link homogenizeMaterial}. */
export interface MaterialCard {
  /** Mass density (kg/m³). */
  density_kg_m3: number;
  /** Cubic elastic constant C11 (GPa). */
  c11_gpa: number;
  /** Cubic elastic constant C12 (GPa). */
  c12_gpa: number;
  /** Cubic elastic constant C44 (GPa). */
  c44_gpa: number;
  /** Bulk modulus K = (C11 + 2 C12)/3 (GPa). */
  bulk_gpa: number;
  /** Isotropic (VRH) shear modulus (GPa). */
  shear_gpa: number;
  /** Isotropic Young's modulus E = 9KG/(3K + G) (GPa). */
  youngs_gpa: number;
  /** Isotropic Poisson ratio ν = (3K − 2G)/(2(3K + G)). */
  poisson: number;
  /** Potential energy per atom at the reference state (eV). */
  energy_ev_atom: number;
  /** Number of atoms in the supercell. */
  atoms: number;
  /** Reference cell volume (Å³). */
  volume_a3: number;
}

/**
 * Minimal structural view of the `vcad-kernel-atoms` WASM surface.
 *
 * These exports are produced by the feature-gated bindings in
 * `vcad-kernel-atoms` and appear in `@vcad/kernel-wasm` after a `wasm-pack`
 * rebuild. We describe them structurally here (rather than importing the
 * generated names) so this wrapper typechecks against any committed kernel
 * build; {@link isAtomsAvailable} feature-detects them at runtime.
 */
interface WasmMdSim {
  run(steps: number): string;
  observe(): string;
  reset(): string;
  moleculeJson(): string;
  free(): void;
}
interface AtomsWasm {
  atoms_parse_xyz(text: string): string;
  atoms_write_xyz(json: string): string;
  atoms_inspect(json: string): string;
  atoms_minimize(molJson: string, cfgJson: string, maxIters: number, forceTol: number): string;
  atoms_homogenize(molJson: string, cfgJson: string): string;
  atoms_build_receipt(m: string, ff: string, run: string, p: string, o: string): string;
  MdSim: new (moleculeJson: string, configJson: string) => WasmMdSim;
}

/** Resolve the kernel WASM module through the shared singleton. */
async function ensureWasm(): Promise<AtomsWasm> {
  const { getKernelWasm } = await import("./wasm-singleton.js");
  return (await getKernelWasm()) as unknown as AtomsWasm;
}

/** True if the WASM bundle was compiled with the `atoms` feature. */
export async function isAtomsAvailable(): Promise<boolean> {
  try {
    const wasm = (await ensureWasm()) as unknown as Record<string, unknown>;
    return typeof wasm.atoms_inspect === "function";
  } catch {
    return false;
  }
}

/** Parse XYZ / extended-XYZ text into a {@link MoleculeSystem}. */
export async function parseXyz(text: string): Promise<MoleculeSystem> {
  const wasm = await ensureWasm();
  return JSON.parse(wasm.atoms_parse_xyz(text)) as MoleculeSystem;
}

/** Serialize a {@link MoleculeSystem} to XYZ text. */
export async function writeXyz(mol: MoleculeSystem): Promise<string> {
  const wasm = await ensureWasm();
  return wasm.atoms_write_xyz(JSON.stringify(mol));
}

/** Compute a structural report (formula, Rg, bbox, …). */
export async function inspectMolecule(mol: MoleculeSystem): Promise<MoleculeReport> {
  const wasm = await ensureWasm();
  return JSON.parse(wasm.atoms_inspect(JSON.stringify(mol))) as MoleculeReport;
}

/** Minimize a structure; returns the relaxed molecule and a result summary. */
export async function minimizeEnergy(
  mol: MoleculeSystem,
  config: MdConfig = {},
  maxIters = 2000,
  forceTol = 1e-4,
): Promise<{ result: MinimizeResult; molecule: MoleculeSystem }> {
  const wasm = await ensureWasm();
  const out = wasm.atoms_minimize(JSON.stringify(mol), JSON.stringify(config), maxIters, forceTol);
  return JSON.parse(out) as { result: MinimizeResult; molecule: MoleculeSystem };
}

/**
 * Homogenize a periodic crystal into bulk material properties (Å / eV / amu
 * in; SI density plus GPa moduli out). Rejects non-periodic structures.
 */
export async function homogenizeMaterial(
  mol: MoleculeSystem,
  config: MdConfig = {},
): Promise<MaterialCard> {
  const wasm = await ensureWasm();
  if (typeof wasm.atoms_homogenize !== "function") {
    throw new Error("atoms_homogenize is not in this kernel build — rebuild vcad-kernel-wasm");
  }
  return JSON.parse(wasm.atoms_homogenize(JSON.stringify(mol), JSON.stringify(config))) as MaterialCard;
}

/** Build a reproducible, tamper-evident simulation receipt. */
export async function buildReceipt(
  mol: MoleculeSystem,
  forceField: string,
  run: string,
  params: unknown,
  outputs: Array<[string, number]>,
): Promise<unknown> {
  const wasm = await ensureWasm();
  const out = wasm.atoms_build_receipt(
    JSON.stringify(mol),
    forceField,
    run,
    JSON.stringify(params ?? null),
    JSON.stringify(outputs),
  );
  return JSON.parse(out);
}

/**
 * A stateful molecular-dynamics environment wrapping the WASM `MdSim`.
 *
 * Usage: `const env = await MdEnv.create(mol, cfg); env.run(100);`
 */
export class MdEnv {
  private constructor(private sim: WasmMdSim) {}

  /** Create an environment from a molecule and config. */
  static async create(mol: MoleculeSystem, config: MdConfig = {}): Promise<MdEnv> {
    const wasm = await ensureWasm();
    return new MdEnv(new wasm.MdSim(JSON.stringify(mol), JSON.stringify(config)));
  }

  /** Run `steps` MD steps; returns the final observation. */
  run(steps: number): MdObservation {
    return JSON.parse(this.sim.run(steps)) as MdObservation;
  }

  /** Current observation without stepping. */
  observe(): MdObservation {
    return JSON.parse(this.sim.observe()) as MdObservation;
  }

  /** Reset to the initial structure. */
  reset(): MdObservation {
    return JSON.parse(this.sim.reset()) as MdObservation;
  }

  /** Current structure as a {@link MoleculeSystem}. */
  molecule(): MoleculeSystem {
    return JSON.parse(this.sim.moleculeJson()) as MoleculeSystem;
  }

  /** Free the underlying WASM object. */
  free(): void {
    this.sim.free();
  }
}
