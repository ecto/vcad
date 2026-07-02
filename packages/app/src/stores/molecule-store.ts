/**
 * Molecule store — the atomic/molecular viewport layer.
 *
 * Molecular visualization is orthogonal to BRep/CAD editing, so it lives in its
 * own store rather than the CRDT document. The viewport's {@link AtomInstances}
 * renders whatever is here; demos and (later) MCP pushes populate it. Structures
 * are built the same way the Rust kernel does — covalent-radius bond perception,
 * CPK colors — so what you see here matches `vcad-kernel-atoms`.
 *
 * (Persisting a molecule inside a saved `.vcad` uses `document.molecule` + the
 * CRDT `Feature::Molecule` path; this store is the live view layer.)
 */

import { create } from "zustand";
import type { MoleculeSystem, Species } from "@vcad/ir";
import type { AtomRepresentation } from "../components/AtomInstances";

interface Atom {
  s: string;
  x: number;
  y: number;
  z: number;
}

// Element reference: [covalent radius Å, atomic number, mass amu, CPK sRGB 0..1].
const ELEMENTS: Record<string, [number, number, number, [number, number, number]]> = {
  H: [0.31, 1, 1.008, [1, 1, 1]],
  C: [0.76, 6, 12.011, [0.34, 0.34, 0.34]],
  N: [0.71, 7, 14.007, [0.19, 0.31, 0.97]],
  O: [0.66, 8, 15.999, [1, 0.05, 0.05]],
  Na: [1.66, 11, 22.99, [0.67, 0.36, 0.95]],
  Cl: [1.02, 17, 35.45, [0.12, 0.94, 0.12]],
  Au: [1.36, 79, 196.97, [1, 0.82, 0.14]],
};
function elem(s: string) {
  return ELEMENTS[s] ?? [0.75, 6, 12.011, [0.85, 0.4, 0.85] as [number, number, number]];
}

/** Assemble a MoleculeSystem from raw atoms: dedup species, perceive bonds. */
function makeMolecule(name: string, atoms: Atom[], periodic = false): MoleculeSystem {
  const species: Species[] = [];
  const speciesIdx: number[] = [];
  const seen = new Map<string, number>();
  for (const a of atoms) {
    let idx = seen.get(a.s);
    if (idx === undefined) {
      const [, num, mass] = elem(a.s);
      idx = species.length;
      species.push({ element: a.s, atomicNumber: num, mass, charge: 0 });
      seen.set(a.s, idx);
    }
    speciesIdx.push(idx);
  }
  const positions = atoms.map((a) => [a.x, a.y, a.z] as [number, number, number]);
  // Covalent-radius bond perception (io.rs::perceive_bonds, tol 1.2).
  const bonds: { a: number; b: number; order: number }[] = [];
  const tol = 1.2;
  for (let i = 0; i < atoms.length; i++) {
    for (let j = i + 1; j < atoms.length; j++) {
      const dx = atoms[i]!.x - atoms[j]!.x;
      const dy = atoms[i]!.y - atoms[j]!.y;
      const dz = atoms[i]!.z - atoms[j]!.z;
      const r2 = dx * dx + dy * dy + dz * dz;
      const cut = (elem(atoms[i]!.s)[0] + elem(atoms[j]!.s)[0]) * tol;
      if (r2 > 0.16 && r2 < cut * cut) bonds.push({ a: i, b: j, order: 1 });
    }
  }
  const cell = periodic ? centerCell(atoms) : undefined;
  return { species, positions, speciesIdx, bonds, name, ...(cell ? { cell } : {}) };
}

function centerCell(_atoms: Atom[]) {
  return undefined; // demos are display-only; no PBC needed for rendering
}

function centered(atoms: Atom[]): Atom[] {
  let cx = 0,
    cy = 0,
    cz = 0;
  for (const a of atoms) {
    cx += a.x;
    cy += a.y;
    cz += a.z;
  }
  const n = atoms.length || 1;
  cx /= n;
  cy /= n;
  cz /= n;
  return atoms.map((a) => ({ s: a.s, x: a.x - cx, y: a.y - cy, z: a.z - cz }));
}

// --- Generators (mirror builder.rs recipes) --------------------------------

function water(): Atom[] {
  return [
    { s: "O", x: 0, y: 0, z: 0 },
    { s: "H", x: 0.757, y: 0.586, z: 0 },
    { s: "H", x: -0.757, y: 0.586, z: 0 },
  ];
}

function benzene(): Atom[] {
  const a: Atom[] = [];
  for (let k = 0; k < 6; k++) {
    const t = (k * Math.PI) / 3;
    a.push({ s: "C", x: 1.39 * Math.cos(t), y: 1.39 * Math.sin(t), z: 0 });
    a.push({ s: "H", x: 2.46 * Math.cos(t), y: 2.46 * Math.sin(t), z: 0 });
  }
  return a;
}

/** Buckminsterfullerene C60 — truncated icosahedron vertices via golden ratio. */
function buckyball(): Atom[] {
  const phi = (1 + Math.sqrt(5)) / 2;
  const base: [number, number, number][] = [
    [0, 1, 3 * phi],
    [2, 1 + 2 * phi, phi],
    [1, 2 + phi, 2 * phi],
  ];
  const verts: [number, number, number][] = [];
  for (const [p, q, r] of base) {
    for (const sp of [p, -p]) {
      for (const sq of [q, -q]) {
        for (const sr of [r, -r]) {
          if (p === 0 && sp < 0) continue; // avoid ±0 duplicates
          // three cyclic (even) permutations
          verts.push([sp, sq, sr], [sr, sp, sq], [sq, sr, sp]);
        }
      }
    }
  }
  // Dedup (numerical) and scale so edges ≈ 1.42 Å (aromatic C-C).
  const uniq: [number, number, number][] = [];
  for (const v of verts) {
    if (!uniq.some((u) => Math.hypot(u[0] - v[0], u[1] - v[1], u[2] - v[2]) < 1e-6)) uniq.push(v);
  }
  const scale = 1.42 / 2; // construction edge length is 2
  return uniq.map(([x, y, z]) => ({ s: "C", x: x * scale, y: y * scale, z: z * scale }));
}

/** Armchair (n,n) carbon nanotube segment. */
function nanotube(n = 6, rings = 8): Atom[] {
  const acc = 1.42; // C-C
  const a = acc * Math.sqrt(3); // lattice constant
  const radius = (a * n) / (2 * Math.PI);
  const atoms: Atom[] = [];
  const dz = (1.5 * acc) / 1; // rise per half-ring
  for (let r = 0; r < rings; r++) {
    for (let i = 0; i < n; i++) {
      // two-atom armchair unit per angular step
      const th0 = (2 * Math.PI * i) / n;
      const th1 = th0 + Math.PI / n;
      const z0 = r * 3 * acc;
      atoms.push({ s: "C", x: radius * Math.cos(th0), y: radius * Math.sin(th0), z: z0 });
      atoms.push({ s: "C", x: radius * Math.cos(th0), y: radius * Math.sin(th0), z: z0 + acc });
      atoms.push({ s: "C", x: radius * Math.cos(th1), y: radius * Math.sin(th1), z: z0 + acc + dz });
      atoms.push({
        s: "C",
        x: radius * Math.cos(th1),
        y: radius * Math.sin(th1),
        z: z0 + 2 * acc + dz,
      });
    }
  }
  return atoms;
}

function fcc(sym: string, aa: number, n: number): Atom[] {
  const basis = [
    [0, 0, 0],
    [0.5, 0.5, 0],
    [0.5, 0, 0.5],
    [0, 0.5, 0.5],
  ];
  const a: Atom[] = [];
  for (let i = 0; i < n; i++)
    for (let j = 0; j < n; j++)
      for (let k = 0; k < n; k++)
        for (const b of basis)
          a.push({ s: sym, x: (i + b[0]!) * aa, y: (j + b[1]!) * aa, z: (k + b[2]!) * aa });
  return a;
}

function diamond(aa: number, n: number): Atom[] {
  const base = [
    [0, 0, 0],
    [0.5, 0.5, 0],
    [0.5, 0, 0.5],
    [0, 0.5, 0.5],
  ];
  const basis: number[][] = [];
  for (const b of base) {
    basis.push(b);
    basis.push([b[0]! + 0.25, b[1]! + 0.25, b[2]! + 0.25]);
  }
  const a: Atom[] = [];
  for (let i = 0; i < n; i++)
    for (let j = 0; j < n; j++)
      for (let k = 0; k < n; k++)
        for (const b of basis)
          a.push({ s: "C", x: (i + b[0]!) * aa, y: (j + b[1]!) * aa, z: (k + b[2]!) * aa });
  return a;
}

function rocksalt(aa: number, n: number): Atom[] {
  const a: Atom[] = [];
  const h = aa / 2;
  for (let i = 0; i <= n; i++)
    for (let j = 0; j <= n; j++)
      for (let k = 0; k <= n; k++)
        a.push({ s: (i + j + k) % 2 === 0 ? "Na" : "Cl", x: i * h, y: j * h, z: k * h });
  return a;
}

/** A demo definition surfaced in the UI. */
export interface MoleculeDemo {
  id: string;
  name: string;
  blurb: string;
  build: () => MoleculeSystem;
}

export const MOLECULE_DEMOS: MoleculeDemo[] = [
  {
    id: "buckyball",
    name: "Buckyball C₆₀",
    blurb: "Truncated icosahedron, generated from the golden ratio",
    build: () => makeMolecule("Buckminsterfullerene", centered(buckyball())),
  },
  {
    id: "nanotube",
    name: "Carbon nanotube",
    blurb: "Armchair (6,6) — a rolled graphene sheet",
    build: () => makeMolecule("Carbon nanotube (6,6)", centered(nanotube(6, 9))),
  },
  {
    id: "diamond",
    name: "Diamond lattice",
    blurb: "Diamond-cubic carbon, 2×2×2 cells",
    build: () => makeMolecule("Diamond — C", centered(diamond(3.57, 2)), true),
  },
  {
    id: "gold",
    name: "Gold crystal",
    blurb: "Face-centered cubic Au, 3×3×3 cells",
    build: () => makeMolecule("Gold — FCC", centered(fcc("Au", 4.08, 3)), true),
  },
  {
    id: "salt",
    name: "Rock salt",
    blurb: "NaCl, interpenetrating FCC sublattices",
    build: () => makeMolecule("Rock salt — NaCl", centered(rocksalt(5.64, 3)), true),
  },
  {
    id: "benzene",
    name: "Benzene",
    blurb: "Aromatic C₆H₆ ring",
    build: () => makeMolecule("Benzene", centered(benzene())),
  },
  {
    id: "water",
    name: "Water",
    blurb: "The bent H₂O molecule",
    build: () => makeMolecule("Water", centered(water())),
  },
];

interface MoleculeState {
  molecule: MoleculeSystem | null;
  representation: AtomRepresentation;
  activeDemoId: string | null;
  setMolecule: (mol: MoleculeSystem | null, demoId?: string | null) => void;
  loadDemo: (id: string) => void;
  setRepresentation: (rep: AtomRepresentation) => void;
  clear: () => void;
}

export const useMoleculeStore = create<MoleculeState>((set) => ({
  molecule: null,
  representation: "ball_and_stick",
  activeDemoId: null,
  setMolecule: (molecule, demoId = null) => set({ molecule, activeDemoId: demoId }),
  loadDemo: (id) => {
    const demo = MOLECULE_DEMOS.find((d) => d.id === id);
    if (demo) set({ molecule: demo.build(), activeDemoId: id });
  },
  setRepresentation: (representation) => set({ representation }),
  clear: () => set({ molecule: null, activeDemoId: null }),
}));
