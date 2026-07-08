/**
 * simulate_strike — the receipt you can hear, before the metal exists.
 *
 * Models a mallet strike on a flat free-free bar (a glockenspiel /
 * vibraphone bar) and returns what the aluminum will do: the modal
 * frequency table, a synthesized WAV of the strike, and a verdict from
 * FFT peak extraction against the target pitch, in cents.
 *
 * Two frequency models, both reported:
 *
 * - **Closed form** (Euler–Bernoulli, uniform section):
 *   fₙ = (βₙL)²/(2π L²)·√(EI/ρA), cosh(βL)·cos(βL) = 1.
 * - **Hole-aware FEM**: 1-D Hermite beam elements with the exact
 *   material width through each cord hole, w_eff(x) = w − 2√(r²−(x−x₀)²),
 *   feeding A(x) and I(x) — so the "suspension holes aren't in the beam
 *   model" caveat is retired. Synthesis uses the FEM frequencies.
 *
 * Strike physics: modal gains are the mode shape at the strike point
 * filtered by a half-sine mallet contact pulse (hard mallet ≈ 0.5 ms —
 * shorter contact, brighter strike). Decay is Q-based: material Q plus a
 * suspension-damping heuristic ∝ φₙ² at the cord holes — which is the
 * audible reason the holes sit on the fundamental's nodal lines.
 *
 * The verdict path is a real round trip: synthesize → Hann window → FFT →
 * parabolic peak interpolation → cents vs target. Material E and ρ come
 * from the kernel's sheet-metal registry, not constants in this file.
 *
 * The returned `modes` table is also the data contract for client-side
 * Web Audio synthesis in the viewer widget (tap a bar, hear the note) —
 * a few hundred bytes instead of an audio blob.
 */

import type { Engine } from "@vcad/engine";
import { writeFileSync } from "node:fs";
import { getSession } from "./session.js";
import { resolveWithinRoot } from "./safe-path.js";
import { isRemoteDeployment, maxInlineExportBytes } from "./remote.js";

// ─── Free-free beam modal math ────────────────────────────────────────────

/** Roots of cosh(x)·cos(x) = 1 (free-free beam), Newton-refined. */
export function freeFreeBetaL(count: number): number[] {
  const roots: number[] = [];
  for (let n = 1; n <= count; n++) {
    // Asymptotic start: (2n+1)π/2.
    let x = ((2 * n + 1) * Math.PI) / 2;
    for (let i = 0; i < 50; i++) {
      const f = Math.cosh(x) * Math.cos(x) - 1;
      const df = Math.sinh(x) * Math.cos(x) - Math.cosh(x) * Math.sin(x);
      const step = f / df;
      x -= step;
      if (Math.abs(step) < 1e-13) break;
    }
    roots.push(x);
  }
  return roots;
}

/**
 * Free-free mode shape φₙ(ξ), ξ = x/L in [0,1], normalized to max |φ| = 1.
 * φ = cosh(βx) + cos(βx) − σ(sinh(βx) + sin(βx)).
 */
export function modeShape(betaL: number, xi: number): number {
  const sigma =
    (Math.cosh(betaL) - Math.cos(betaL)) / (Math.sinh(betaL) - Math.sin(betaL));
  const raw = (t: number) =>
    Math.cosh(betaL * t) +
    Math.cos(betaL * t) -
    sigma * (Math.sinh(betaL * t) + Math.sin(betaL * t));
  // Max |φ| is at the free ends (= 2 in this normalization).
  return raw(xi) / Math.abs(raw(0));
}

export interface BarSpec {
  length_mm: number;
  width_mm: number;
  thickness_mm: number;
  /** Hole centers along the bar axis (mm from one end). */
  holes_mm: number[];
  hole_dia_mm: number;
  /** Young's modulus (GPa) and density (kg/m³) — from the kernel registry. */
  modulus_gpa: number;
  density_kg_m3: number;
}

/** Closed-form modal frequencies (Hz) for the uniform bar. */
export function closedFormHz(bar: BarSpec, count: number): number[] {
  const L = bar.length_mm / 1000;
  const t = bar.thickness_mm / 1000;
  const E = bar.modulus_gpa * 1e9;
  // I/A = t²/12 → √(EI/ρA) = t·√(E/12ρ).
  const c = t * Math.sqrt(E / (12 * bar.density_kg_m3));
  return freeFreeBetaL(count).map((bl) => (bl * bl * c) / (2 * Math.PI * L * L));
}

// ─── Hole-aware 1-D FEM (Hermite beam elements) ───────────────────────────

/** Material width through the cord holes at axial station x (mm). */
function effectiveWidthMm(bar: BarSpec, xMm: number): number {
  let w = bar.width_mm;
  const r = bar.hole_dia_mm / 2;
  for (const h of bar.holes_mm) {
    const d = Math.abs(xMm - h);
    if (d < r) w -= 2 * Math.sqrt(r * r - d * d);
  }
  return Math.max(w, 1e-6);
}

/**
 * Lowest `count` elastic modal frequencies (Hz) from a free-free Hermite
 * beam FEM with per-Gauss-point section properties. The two rigid-body
 * modes are discarded. Dense symmetric eigensolve (Cholesky + Jacobi) —
 * fine at the ~200-DOF meshes this uses.
 */
export function femHz(bar: BarSpec, count: number, nel = 96): number[] {
  const L = bar.length_mm / 1000;
  const t = bar.thickness_mm / 1000;
  const E = bar.modulus_gpa * 1e9;
  const rho = bar.density_kg_m3;
  const nDof = 2 * (nel + 1);
  const K = zeros(nDof, nDof);
  const M = zeros(nDof, nDof);
  const le = L / nel;

  // 4-point Gauss on [-1, 1].
  const gp = [-0.8611363115940526, -0.3399810435848563, 0.3399810435848563, 0.8611363115940526];
  const gw = [0.3478548451374538, 0.6521451548625461, 0.6521451548625461, 0.3478548451374538];

  for (let e = 0; e < nel; e++) {
    const x0 = e * le;
    const ke = zeros(4, 4);
    const me = zeros(4, 4);
    for (let g = 0; g < 4; g++) {
      const xi = gp[g]; // element coordinate in [-1, 1]
      const x = x0 + ((xi + 1) / 2) * le; // global (m)
      const wEff = effectiveWidthMm(bar, x * 1000) / 1000; // m
      const A = wEff * t;
      const I = (wEff * t * t * t) / 12;
      // Hermite shape functions on [-1,1] (rotational DOFs scaled by le/2).
      const s = le / 2;
      const N = [
        0.25 * (1 - xi) * (1 - xi) * (2 + xi),
        s * 0.25 * (1 - xi) * (1 - xi) * (1 + xi),
        0.25 * (1 + xi) * (1 + xi) * (2 - xi),
        s * 0.25 * (1 + xi) * (1 + xi) * (xi - 1),
      ];
      // Second derivatives d²N/dx² = d²N/dξ² · (2/le)².
      const c2 = (2 / le) * (2 / le);
      const B = [
        1.5 * xi * c2,
        s * (1.5 * xi - 0.5) * c2,
        -1.5 * xi * c2,
        s * (1.5 * xi + 0.5) * c2,
      ];
      const wJ = gw[g] * s; // Gauss weight × Jacobian
      for (let i = 0; i < 4; i++) {
        for (let j = 0; j < 4; j++) {
          ke[i][j] += E * I * B[i] * B[j] * wJ;
          me[i][j] += rho * A * N[i] * N[j] * wJ;
        }
      }
    }
    const dof = [2 * e, 2 * e + 1, 2 * e + 2, 2 * e + 3];
    for (let i = 0; i < 4; i++) {
      for (let j = 0; j < 4; j++) {
        K[dof[i]][dof[j]] += ke[i][j];
        M[dof[i]][dof[j]] += me[i][j];
      }
    }
  }

  // Generalized symmetric eigenproblem K φ = ω² M φ via M = LLᵀ,
  // C = L⁻¹ K L⁻ᵀ (symmetric), then cyclic Jacobi.
  const Lc = cholesky(M);
  const C = congruence(K, Lc);
  const lambda = jacobiEigenvalues(C);
  lambda.sort((a, b) => a - b);
  // Drop rigid-body modes (λ ≈ 0; threshold at (2π·1 Hz)²).
  const elastic = lambda.filter((l) => l > (2 * Math.PI) ** 2);
  return elastic.slice(0, count).map((l) => Math.sqrt(l) / (2 * Math.PI));
}

function zeros(r: number, c: number): number[][] {
  return Array.from({ length: r }, () => new Array<number>(c).fill(0));
}

/** Lower-triangular Cholesky factor of an SPD matrix. */
function cholesky(A: number[][]): number[][] {
  const n = A.length;
  const L = zeros(n, n);
  for (let i = 0; i < n; i++) {
    for (let j = 0; j <= i; j++) {
      let s = A[i][j];
      for (let k = 0; k < j; k++) s -= L[i][k] * L[j][k];
      if (i === j) {
        if (s <= 0) throw new Error("mass matrix not positive definite");
        L[i][i] = Math.sqrt(s);
      } else {
        L[i][j] = s / L[j][j];
      }
    }
  }
  return L;
}

/** C = L⁻¹ K L⁻ᵀ (forward/back substitution, keeps symmetry). */
function congruence(K: number[][], L: number[][]): number[][] {
  const n = K.length;
  // Y = L⁻¹ K  (solve L·Y = K column-wise via forward substitution on rows).
  const Y = zeros(n, n);
  for (let c = 0; c < n; c++) {
    for (let i = 0; i < n; i++) {
      let s = K[i][c];
      for (let k = 0; k < i; k++) s -= L[i][k] * Y[k][c];
      Y[i][c] = s / L[i][i];
    }
  }
  // C = Y L⁻ᵀ  → Cᵀ = L⁻¹ Yᵀ, and C symmetric.
  const C = zeros(n, n);
  for (let r = 0; r < n; r++) {
    for (let i = 0; i < n; i++) {
      let s = Y[r][i];
      for (let k = 0; k < i; k++) s -= L[i][k] * C[r][k];
      C[r][i] = s / L[i][i];
    }
  }
  // Symmetrize against round-off.
  for (let i = 0; i < n; i++)
    for (let j = 0; j < i; j++) {
      const v = 0.5 * (C[i][j] + C[j][i]);
      C[i][j] = v;
      C[j][i] = v;
    }
  return C;
}

/** Eigenvalues of a symmetric matrix by cyclic Jacobi rotations. */
function jacobiEigenvalues(A: number[][]): number[] {
  const n = A.length;
  const a = A.map((row) => row.slice());
  for (let sweep = 0; sweep < 30; sweep++) {
    let off = 0;
    for (let i = 0; i < n; i++)
      for (let j = i + 1; j < n; j++) off += a[i][j] * a[i][j];
    if (off < 1e-18 * n * n) break;
    for (let p = 0; p < n - 1; p++) {
      for (let q = p + 1; q < n; q++) {
        if (Math.abs(a[p][q]) < 1e-30) continue;
        const theta = (a[q][q] - a[p][p]) / (2 * a[p][q]);
        const t =
          Math.sign(theta) / (Math.abs(theta) + Math.sqrt(theta * theta + 1));
        const c = 1 / Math.sqrt(t * t + 1);
        const s = t * c;
        for (let k = 0; k < n; k++) {
          const akp = a[k][p];
          const akq = a[k][q];
          a[k][p] = c * akp - s * akq;
          a[k][q] = s * akp + c * akq;
        }
        for (let k = 0; k < n; k++) {
          const apk = a[p][k];
          const aqk = a[q][k];
          a[p][k] = c * apk - s * aqk;
          a[q][k] = s * apk + c * aqk;
        }
      }
    }
  }
  return a.map((row, i) => row[i]);
}

// ─── Strike model, synthesis, spectrum ────────────────────────────────────

export interface Mode {
  n: number;
  hz: number;
  /** Linear amplitude, ≤ 1 (mode shape at strike × mallet spectrum). */
  gain: number;
  q: number;
  /** Time to −60 dB (s). */
  t60_s: number;
}

/** Half-sine force pulse spectrum magnitude, normalized to 1 at DC. */
export function malletSpectrum(hz: number, contactMs: number): number {
  const tc = contactMs / 1000;
  const u = 2 * hz * tc;
  // |F̂(f)| / |F̂(0)| for a half-sine of duration tc.
  const denom = Math.abs(1 - u * u);
  if (denom < 1e-9) return Math.PI / 4; // removable singularity at u = 1
  return Math.abs(Math.cos(Math.PI * hz * tc)) / denom;
}

/** Build the strike-excited modal set (gains normalized to the loudest). */
export function strikeModes(
  bar: BarSpec,
  femFreqs: number[],
  strikeFrac: number,
  contactMs: number,
  materialQ = 2500,
  suspensionQ0 = 150,
): Mode[] {
  const betas = freeFreeBetaL(femFreqs.length);
  const modes: Mode[] = femFreqs.map((hz, i) => {
    const phiStrike = modeShape(betas[i], strikeFrac);
    const gain = Math.abs(phiStrike) * malletSpectrum(hz, contactMs);
    // Suspension damping: cord at the holes bleeds energy ∝ φₙ² there.
    // Mode 1 has φ ≈ 0 at the nodal holes — that's why they're there.
    let phiSq = 0;
    for (const h of bar.holes_mm) {
      const phi = modeShape(betas[i], h / bar.length_mm);
      phiSq += phi * phi;
    }
    const qSusp = suspensionQ0 / Math.max(phiSq, 1e-9);
    const q = 1 / (1 / materialQ + 1 / qSusp);
    return {
      n: i + 1,
      hz,
      gain,
      q,
      t60_s: (Math.log(1000) * q) / (Math.PI * hz),
    };
  });
  const peak = Math.max(...modes.map((m) => m.gain), 1e-12);
  for (const m of modes) m.gain /= peak;
  return modes;
}

/** Sum of exponentially decaying sinusoids, peak-normalized to −1 dBFS. */
export function synthesize(
  modes: Mode[],
  durationS: number,
  sampleRate: number,
): Float64Array {
  const n = Math.floor(durationS * sampleRate);
  const out = new Float64Array(n);
  for (const m of modes) {
    if (m.hz >= 0.45 * sampleRate) continue;
    const w = (2 * Math.PI * m.hz) / sampleRate;
    const tau = m.q / (Math.PI * m.hz); // amplitude time constant (s)
    const decay = Math.exp(-1 / (tau * sampleRate));
    // Recursive oscillator: amp·decayᵏ·sin(w·k).
    let amp = m.gain;
    for (let k = 0; k < n; k++) {
      out[k] += amp * Math.sin(w * k);
      amp *= decay;
    }
  }
  let peak = 1e-12;
  for (let k = 0; k < n; k++) peak = Math.max(peak, Math.abs(out[k]));
  const norm = 0.891 / peak; // −1 dBFS
  for (let k = 0; k < n; k++) out[k] *= norm;
  return out;
}

/** Encode mono float samples as a 16-bit PCM WAV. */
export function encodeWav(samples: Float64Array, sampleRate: number): Uint8Array {
  const n = samples.length;
  const buf = new ArrayBuffer(44 + n * 2);
  const v = new DataView(buf);
  const str = (off: number, s: string) => {
    for (let i = 0; i < s.length; i++) v.setUint8(off + i, s.charCodeAt(i));
  };
  str(0, "RIFF");
  v.setUint32(4, 36 + n * 2, true);
  str(8, "WAVE");
  str(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true); // PCM
  v.setUint16(22, 1, true); // mono
  v.setUint32(24, sampleRate, true);
  v.setUint32(28, sampleRate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  str(36, "data");
  v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) {
    const s = Math.max(-1, Math.min(1, samples[i]));
    v.setInt16(44 + i * 2, Math.round(s * 32767), true);
  }
  return new Uint8Array(buf);
}

/** In-place radix-2 FFT (re, im length must be a power of two). */
function fft(re: Float64Array, im: Float64Array): void {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      [re[i], re[j]] = [re[j], re[i]];
      [im[i], im[j]] = [im[j], im[i]];
    }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const ang = (-2 * Math.PI) / len;
    const wr = Math.cos(ang);
    const wi = Math.sin(ang);
    for (let i = 0; i < n; i += len) {
      let cr = 1;
      let ci = 0;
      for (let k = 0; k < len / 2; k++) {
        const ur = re[i + k];
        const ui = im[i + k];
        const vr = re[i + k + len / 2] * cr - im[i + k + len / 2] * ci;
        const vi = re[i + k + len / 2] * ci + im[i + k + len / 2] * cr;
        re[i + k] = ur + vr;
        im[i + k] = ui + vi;
        re[i + k + len / 2] = ur - vr;
        im[i + k + len / 2] = ui - vi;
        const ncr = cr * wr - ci * wi;
        ci = cr * wi + ci * wr;
        cr = ncr;
      }
    }
  }
}

export interface SpectralPeak {
  hz: number;
  db: number;
}

/**
 * Hann-windowed FFT of the first 2¹⁶ samples, top peaks by local maxima
 * with parabolic interpolation on log magnitude (sub-bin accuracy).
 */
export function spectrumPeaks(
  samples: Float64Array,
  sampleRate: number,
  maxPeaks = 8,
): SpectralPeak[] {
  const n = Math.min(65536, 1 << Math.floor(Math.log2(samples.length)));
  const re = new Float64Array(n);
  const im = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    const w = 0.5 * (1 - Math.cos((2 * Math.PI * i) / (n - 1)));
    re[i] = samples[i] * w;
  }
  fft(re, im);
  const half = n / 2;
  const mag = new Float64Array(half);
  let maxMag = 1e-30;
  for (let i = 0; i < half; i++) {
    mag[i] = Math.hypot(re[i], im[i]);
    maxMag = Math.max(maxMag, mag[i]);
  }
  const peaks: SpectralPeak[] = [];
  const floorDb = -60;
  for (let i = 2; i < half - 2; i++) {
    if (mag[i] <= mag[i - 1] || mag[i] < mag[i + 1]) continue;
    const db = 20 * Math.log10(mag[i] / maxMag);
    if (db < floorDb) continue;
    // Parabolic interpolation on log magnitude.
    const a = Math.log(mag[i - 1] + 1e-30);
    const b = Math.log(mag[i] + 1e-30);
    const c = Math.log(mag[i + 1] + 1e-30);
    const delta = (0.5 * (a - c)) / (a - 2 * b + c || 1e-30);
    peaks.push({ hz: ((i + delta) * sampleRate) / n, db });
  }
  peaks.sort((a, b) => b.db - a.db);
  // Suppress shoulders: keep peaks at least 3% apart in frequency.
  const kept: SpectralPeak[] = [];
  for (const p of peaks) {
    if (kept.every((k) => Math.abs(p.hz - k.hz) / k.hz > 0.03)) kept.push(p);
    if (kept.length >= maxPeaks) break;
  }
  return kept.sort((a, b) => a.hz - b.hz);
}

export const cents = (a: number, b: number) => 1200 * Math.log2(a / b);

/** "C6", "F#4", "Bb3" → Hz (equal temperament, A4 = 440). */
export function noteToHz(note: string): number {
  const m = /^([A-Ga-g])([#b]?)(-?\d)$/.exec(note.trim());
  if (!m) throw new Error(`unparseable note ${JSON.stringify(note)} — use e.g. "C6", "F#4"`);
  const base: Record<string, number> = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };
  let semis = base[m[1].toUpperCase()];
  if (m[2] === "#") semis += 1;
  if (m[2] === "b") semis -= 1;
  const midi = (Number(m[3]) + 1) * 12 + semis;
  return 440 * 2 ** ((midi - 69) / 12);
}

// ─── The MCP tool ─────────────────────────────────────────────────────────

/** Aliases into the kernel registry, mirroring materials.rs `lookup`. */
const MATERIAL_ALIASES: Record<string, string> = {
  "al-6061": "al-hard", "6061": "al-hard", "6061-t6": "al-hard",
  aluminum: "al-soft", aluminium: "al-soft", al: "al-soft",
  "al-1100": "al-soft", "al-3003": "al-soft", "1100": "al-soft", "3003": "al-soft",
  steel: "steel-mild", "mild-steel": "steel-mild", a36: "steel-mild",
  crs: "steel-mild", "1018": "steel-mild",
  stainless: "ss-304", ss: "ss-304", "304": "ss-304", ss304: "ss-304",
  cu: "copper",
};

function lookupMaterial(engine: Engine, name: string) {
  const key = name.trim().toLowerCase().replace(/[_ ]/g, "-");
  const registry = engine.getSheetMetalMaterials() as Array<{
    name: string;
    display_name: string;
    modulus_gpa: number;
    density_kg_m3: number;
  }>;
  const resolved = MATERIAL_ALIASES[key] ?? key;
  const hit = registry.find((m) => m.name.toLowerCase() === resolved);
  if (!hit) {
    throw new Error(
      `material ${JSON.stringify(name)} not in the kernel registry — known: ${registry.map((m) => m.name).join(", ")}`,
    );
  }
  return hit;
}

/** Pull bar geometry out of a flat single-panel sheet-metal session doc. */
function barFromDocument(engine: Engine, documentId: string): {
  bar: Omit<BarSpec, "modulus_gpa" | "density_kg_m3">;
  material: string;
} {
  const doc = getSession(documentId);
  const nodes = Object.values(doc.nodes ?? {}) as Array<{
    op: Record<string, unknown> & { type: string };
  }>;
  if (nodes.some((n) => ["SheetMetalEdgeFlange", "SheetMetalHem", "SheetMetalJog"].includes(n.op.type))) {
    throw new Error(
      "strike simulation models flat free-free bars — this document has bends. Simulate the bars, not the stand.",
    );
  }
  const base = nodes.find((n) =>
    ["SheetMetalBaseFlangePolygon", "SheetMetalBaseFlangeRect"].includes(n.op.type),
  );
  if (!base) throw new Error("document has no sheet-metal base flange");
  const op = base.op as {
    type: string;
    outline?: { x: number; y: number }[];
    holes?: { x: number; y: number }[][];
    width?: number;
    depth?: number;
    thickness: number;
    material?: string;
  };
  let lengthMm: number;
  let widthMm: number;
  let axis: "x" | "y";
  if (op.type === "SheetMetalBaseFlangeRect") {
    lengthMm = Math.max(op.width ?? 0, op.depth ?? 0);
    widthMm = Math.min(op.width ?? 0, op.depth ?? 0);
    axis = (op.width ?? 0) >= (op.depth ?? 0) ? "x" : "y";
  } else {
    const xs = (op.outline ?? []).map((p) => p.x);
    const ys = (op.outline ?? []).map((p) => p.y);
    const ex = Math.max(...xs) - Math.min(...xs);
    const ey = Math.max(...ys) - Math.min(...ys);
    axis = ex >= ey ? "x" : "y";
    lengthMm = Math.max(ex, ey);
    widthMm = Math.min(ex, ey);
  }
  const holeLoops = op.holes ?? [];
  const holesMm: number[] = [];
  let holeDia = 0;
  for (const loop of holeLoops) {
    const cs = loop.map((p) => (axis === "x" ? p.x : p.y));
    const os = loop.map((p) => (axis === "x" ? p.y : p.x));
    holesMm.push((Math.min(...cs) + Math.max(...cs)) / 2);
    holeDia = Math.max(holeDia, Math.max(...cs) - Math.min(...cs));
    void os;
  }
  return {
    bar: {
      length_mm: lengthMm,
      width_mm: widthMm,
      thickness_mm: op.thickness,
      holes_mm: holesMm,
      hole_dia_mm: holeDia,
    },
    material: op.material ?? "al-soft",
  };
}

export const simulateStrikeSchema = {
  type: "object" as const,
  properties: {
    document_id: {
      type: "string" as const,
      description:
        "Session id of a FLAT sheet-metal part (from sheet_metal_create) — the bar's length, width, thickness, material, and hole positions are read from it. Mutually exclusive with `bar`.",
    },
    bar: {
      type: "object" as const,
      description:
        "Explicit bar spec instead of a document: {length_mm, width_mm, thickness_mm, material?, holes_mm?, hole_dia_mm?}.",
    },
    note: {
      type: "string" as const,
      description:
        'Target pitch to verify against, e.g. "C6", "F#4". Alternative to expect_hz.',
    },
    expect_hz: {
      type: "number" as const,
      description: "Target fundamental (Hz) to verify against.",
    },
    tolerance_cents: {
      type: "number" as const,
      description: "Pass/fail tolerance for the verdict (cents). Default 10.",
    },
    strike_position: {
      type: "number" as const,
      description:
        "Strike point as a fraction of bar length, 0..1. Default 0.5 (center — antinode of mode 1, node of mode 2, like a real player).",
    },
    mallet_contact_ms: {
      type: "number" as const,
      description:
        "Mallet contact time (ms). Hard glockenspiel mallet ≈ 0.5 (default); soft ≈ 2 — longer contact low-passes the upper partials.",
    },
    duration_s: {
      type: "number" as const,
      description: "Length of the synthesized strike (s). Default 2.5.",
    },
    wav_filename: {
      type: "string" as const,
      description:
        "Write the synthesized strike as a 16-bit/44.1kHz WAV with this name (local servers write to the export dir; hosted returns base64). Omit to skip audio and return the modal/verdict data only.",
    },
  },
};

export function simulateStrike(
  input: unknown,
  engine: Engine,
): { content: Array<{ type: "text"; text: string }> } {
  const a = (input ?? {}) as Record<string, unknown>;
  let barDims: Omit<BarSpec, "modulus_gpa" | "density_kg_m3">;
  let materialName: string;
  if (typeof a.document_id === "string" && a.document_id.length > 0) {
    const r = barFromDocument(engine, a.document_id);
    barDims = r.bar;
    materialName = r.material;
  } else if (a.bar && typeof a.bar === "object") {
    const b = a.bar as Record<string, unknown>;
    for (const f of ["length_mm", "width_mm", "thickness_mm"]) {
      if (typeof b[f] !== "number") throw new Error(`bar.${f} (number) is required`);
    }
    barDims = {
      length_mm: Number(b.length_mm),
      width_mm: Number(b.width_mm),
      thickness_mm: Number(b.thickness_mm),
      holes_mm: Array.isArray(b.holes_mm) ? (b.holes_mm as number[]).map(Number) : [],
      hole_dia_mm: Number(b.hole_dia_mm ?? 0),
    };
    materialName = typeof b.material === "string" ? b.material : "6061";
  } else {
    throw new Error("supply document_id or bar {length_mm, width_mm, thickness_mm}");
  }

  const mat = lookupMaterial(engine, materialName);
  const bar: BarSpec = {
    ...barDims,
    modulus_gpa: mat.modulus_gpa,
    density_kg_m3: mat.density_kg_m3,
  };

  const sampleRate = 44100;
  const durationS = Math.min(Math.max(Number(a.duration_s ?? 2.5), 0.5), 10);
  const strikeFrac = Math.min(Math.max(Number(a.strike_position ?? 0.5), 0.02), 0.98);
  const contactMs = Math.min(Math.max(Number(a.mallet_contact_ms ?? 0.5), 0.05), 10);

  const nModes = 6;
  const closed = closedFormHz(bar, nModes);
  const fem = femHz(bar, nModes);
  const modes = strikeModes(bar, fem, strikeFrac, contactMs);
  const audible = modes.filter((m) => m.hz < 0.45 * sampleRate && m.gain > 1e-4);

  const samples = synthesize(audible, durationS, sampleRate);
  const peaks = spectrumPeaks(samples, sampleRate);

  // Verdict: the dominant spectral peak vs the expected fundamental.
  let expected: number | undefined;
  if (typeof a.expect_hz === "number") expected = a.expect_hz;
  else if (typeof a.note === "string") expected = noteToHz(a.note);
  const toleranceCents = Number(a.tolerance_cents ?? 10);
  const dominant = peaks.reduce(
    (best, p) => (p.db > best.db ? p : best),
    { hz: 0, db: -Infinity } as SpectralPeak,
  );
  const verdict =
    expected !== undefined
      ? {
          expected_hz: expected,
          measured_hz: dominant.hz,
          cents_error: cents(dominant.hz, expected),
          tolerance_cents: toleranceCents,
          pass: Math.abs(cents(dominant.hz, expected)) <= toleranceCents,
        }
      : undefined;

  const payload: Record<string, unknown> = {
    bar: { ...bar, material: mat.name, material_display: mat.display_name },
    physics: {
      model: "free-free Euler–Bernoulli bar",
      f1_closed_form_hz: closed[0],
      f1_fem_with_holes_hz: fem[0],
      hole_shift_cents: cents(fem[0], closed[0]),
      overtone_ratios: fem.map((f) => f / fem[0]),
      note_overtones:
        "free-free partials are non-harmonic (≈2.76, 5.40, 8.93 × f₁) — that inharmonicity IS the glockenspiel timbre",
    },
    strike: {
      position_frac: strikeFrac,
      mallet_contact_ms: contactMs,
      duration_s: durationS,
      sample_rate: sampleRate,
    },
    modes: audible.map((m) => ({
      n: m.n,
      hz: round2(m.hz),
      gain: round4(m.gain),
      q: Math.round(m.q),
      t60_s: round2(m.t60_s),
    })),
    spectrum_peaks: peaks.map((p) => ({ hz: round2(p.hz), db: round2(p.db) })),
    ...(verdict ? { verdict } : {}),
    limits:
      "1-D transverse bending only (no torsional/lateral modes); decay Q is a heuristic (material + cord-at-holes), frequencies are not",
  };

  if (typeof a.wav_filename === "string" && a.wav_filename.length > 0) {
    if (!/\.wav$/i.test(a.wav_filename)) {
      throw new Error("wav_filename must end in .wav");
    }
    const bytes = encodeWav(samples, sampleRate);
    if (isRemoteDeployment()) {
      const cap = maxInlineExportBytes();
      if (bytes.length > cap) {
        throw new Error(
          `WAV is ${bytes.length} bytes — over the ${cap} byte inline cap; lower duration_s`,
        );
      }
      payload.wav = {
        filename: a.wav_filename,
        bytes: bytes.length,
        data_base64: Buffer.from(bytes).toString("base64"),
      };
    } else {
      const path = resolveWithinRoot(
        a.wav_filename,
        process.env.VCAD_MCP_EXPORT_DIR ?? process.cwd(),
      );
      writeFileSync(path, bytes);
      payload.wav = { path, bytes: bytes.length };
    }
  }

  return { content: [{ type: "text", text: JSON.stringify(payload, null, 2) }] };
}

const round2 = (x: number) => Math.round(x * 100) / 100;
const round4 = (x: number) => Math.round(x * 10000) / 10000;

// ─── ToolDef registry entry ───────────────────────────────────────────────

import { behavior, type ToolDef } from "./tool-def.js";

/** Tool table contributed to the server assembler. */
export const toolDefs: ToolDef[] = [
  {
    name: "simulate_strike",
    pack: "sheet_metal",
    description:
      "Hear a part before it's cut: simulate a mallet strike on a flat free-free bar (glockenspiel/vibraphone bar) and verify its pitch. Modal frequencies from BOTH the closed-form Euler–Bernoulli model and a hole-aware 1-D FEM (cord holes change A(x), I(x)); strike-excited modal synthesis (mode shape at strike point × half-sine mallet spectrum, Q-based decay) → optional 44.1 kHz WAV → FFT peak extraction → cents-error verdict vs `note`/`expect_hz`. Takes a flat sheet_metal_create `document_id` (dims, material, holes read from the session) or an explicit `bar` spec. E/ρ come from the kernel material registry. The `modes` table {hz, gain, q} is compact enough to drive client-side Web Audio synthesis.",
    inputSchema: simulateStrikeSchema,
    handler: (a, c) => simulateStrike(a, c.engine),
    behavior: behavior({}),
  },
];
