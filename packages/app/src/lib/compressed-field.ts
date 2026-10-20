/**
 * Compressed scalar fields (tang-compress TCG1/TCG3 streams) — load, decode
 * in the browser (WebGPU, WASM-CPU fallback), and verify against the
 * native decode hash. See docs/compressed-fields.md.
 *
 * The decoder is the `tang-compress-wasm` wasm-bindgen package, built into
 * `public/tc-wasm/` by `scripts/build-tc-wasm.sh` (gitignored, not
 * bundled: it is loaded on first use only).
 */

export interface FieldBound {
  kind: "max_abs";
  value: number;
  units: string;
}

/** Sidecar metadata for a stream (`<name>.json` next to `<name>.tc`). */
export interface FieldMeta {
  field: string;
  source?: string;
  dims_zyx: [number, number, number];
  origin_mm: [number, number, number];
  voxel_mm: [number, number, number];
  min: number;
  max: number;
  bound: FieldBound;
  /** FNV-1a 64 of the native CPU decode's f32 bit patterns. */
  native_fnv1a?: string;
  format?: string;
  units?: string;
  /** 2D field: the world plane its image lies in (columns → first axis, rows → second). */
  plane?: "xy" | "yz" | "xz";
}

export interface DecodedField {
  name: string;
  meta: FieldMeta;
  data: Float32Array;
  backend: "webgpu" | "cpu";
  streamBytes: number;
  hash: string;
  /** null when the meta carries no native hash. */
  hashMatch: boolean | null;
  timings: { fetchMs: number; initMs: number; decodeMs: number; totalMs: number; gpu?: Record<string, number> };
  /** Measured max |decoded - reference| when a reference grid was supplied. */
  measuredMaxAbs?: number;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type TcModule = any;
let modPromise: Promise<TcModule> | null = null;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
let gpuPromise: Promise<any | null> | null = null;

function loadModule(): Promise<TcModule> {
  if (!modPromise) {
    // Vite refuses `import()` of files under public/, so import the glue
    // through a blob URL and hand init the wasm URL explicitly.
    const dir = new URL(`${import.meta.env.BASE_URL}tc-wasm/`, window.location.href);
    modPromise = fetch(new URL("tang_compress_wasm.js", dir))
      .then((r) => {
        if (!r.ok) throw new Error(`tc-wasm missing (run scripts/build-tc-wasm.sh): ${r.status}`);
        return r.text();
      })
      .then(async (src) => {
        const blob = URL.createObjectURL(new Blob([src], { type: "text/javascript" }));
        const m = await import(/* @vite-ignore */ blob);
        URL.revokeObjectURL(blob);
        await m.default({ module_or_path: new URL("tang_compress_wasm_bg.wasm", dir) });
        return m;
      });
  }
  return modPromise;
}

function getGpu(mod: TcModule) {
  if (!gpuPromise) {
    gpuPromise = ("gpu" in navigator ? mod.TcGpu.create() : Promise.reject(new Error("no navigator.gpu"))).catch(
      (e: unknown) => {
        console.warn("[compressed-field] WebGPU unavailable, using WASM CPU decode:", e);
        return null;
      },
    );
  }
  return gpuPromise;
}

/**
 * Fetch `<base>.tc` + `<base>.json`, decode, and hash-check. Pass
 * `forceCpu` to skip WebGPU; pass `referenceUrl` (raw little-endian f32)
 * to measure the actual max error against the uncompressed field.
 */
export async function loadCompressedField(
  base: string,
  opts: { forceCpu?: boolean; forceGpu?: boolean; referenceUrl?: string } = {},
): Promise<DecodedField> {
  const t0 = performance.now();
  const [bytes, meta] = await Promise.all([
    fetch(`${base}.tc`).then((r) => {
      if (!r.ok) throw new Error(`fetch ${base}.tc: ${r.status}`);
      return r.arrayBuffer();
    }),
    fetch(`${base}.json`).then((r) => {
      if (!r.ok) throw new Error(`fetch ${base}.json: ${r.status}`);
      return r.json() as Promise<FieldMeta>;
    }),
  ]);
  const t1 = performance.now();
  const mod = await loadModule();
  // Below ~4M cells WebGPU loses to the WASM CPU decoder on fixed
  // per-dispatch costs (measured: 262k-cell TCG3s, GPU ~55 ms vs CPU ~15 ms),
  // so default to CPU there; both are byte-identical to native.
  const cells = meta.dims_zyx.reduce((a, b) => a * b, 1);
  const wantGpu = opts.forceGpu || (!opts.forceCpu && cells >= 1 << 22);
  const gpu = wantGpu ? await getGpu(mod) : null;
  const t2 = performance.now();
  const stream = new Uint8Array(bytes);
  let data: Float32Array;
  let gpuTimes: Record<string, number> | undefined;
  if (gpu) {
    data = await gpu.decode(stream);
    gpuTimes = JSON.parse(gpu.lastTimes());
  } else {
    data = mod.decodeCpu(stream);
  }
  const t3 = performance.now();
  const [nz, ny, nx] = meta.dims_zyx;
  if (data.length !== nz * ny * nx) throw new Error(`decoded ${data.length} cells, expected ${nz * ny * nx}`);
  const hash: string = mod.fnv1a(data);
  const out: DecodedField = {
    name: base.split("/").pop() ?? base,
    meta,
    data,
    backend: gpu ? "webgpu" : "cpu",
    streamBytes: stream.byteLength,
    hash,
    hashMatch: meta.native_fnv1a ? hash === meta.native_fnv1a : null,
    timings: { fetchMs: t1 - t0, initMs: t2 - t1, decodeMs: t3 - t2, totalMs: t3 - t0, gpu: gpuTimes },
  };
  if (opts.referenceUrl) {
    const r = await fetch(opts.referenceUrl);
    if (r.ok) {
      const ref = new Float32Array(await r.arrayBuffer());
      let m = 0;
      for (let i = 0; i < ref.length; i++) m = Math.max(m, Math.abs(ref[i]! - data[i]!));
      out.measuredMaxAbs = m;
    }
  }
  return out;
}

/** Turbo-ish colormap (Google's polynomial approximation), t in [0,1]. */
export function turbo(t: number): [number, number, number] {
  const x = Math.min(1, Math.max(0, t));
  const r = 0.1357 + x * (4.5974 - x * (42.3277 - x * (130.5887 - x * (150.5666 - x * 58.1375))));
  const g = 0.0914 + x * (2.1856 + x * (4.8052 - x * (14.0195 - x * (4.2109 + x * 2.7747))));
  const b = 0.1067 + x * (12.5925 - x * (60.1097 - x * (109.0745 - x * (88.5066 - x * 26.8183))));
  const c = (v: number) => Math.round(255 * Math.min(1, Math.max(0, v)));
  return [c(r), c(g), c(b)];
}

// ---------------------------------------------------------------------------
// Multiphysics stacks: several co-registered fields on one part
// (`<dir>/stack.json`, see docs/compressed-fields.md).

export interface FieldStack {
  name: string;
  title: string;
  notes?: string;
  geometry?: { boxes: { min: [number, number, number]; max: [number, number, number] }[] };
  layers: { id: string; label: string; file: string }[];
}

export interface StackLayer {
  id: string;
  label: string;
  field: DecodedField;
  /** Colormap range: over solid cells for a volume with geometry, else the field's. */
  range: [number, number];
}

/** Fetch `stack.json` and decode every layer (sequentially: one GPU queue). */
export async function loadFieldStack(
  dir: string,
  opts: { forceCpu?: boolean; forceGpu?: boolean } = {},
): Promise<{ stack: FieldStack; layers: StackLayer[]; totalMs: number }> {
  const t0 = performance.now();
  const r = await fetch(`${dir}/stack.json`);
  if (!r.ok) throw new Error(`fetch ${dir}/stack.json: ${r.status}`);
  const stack = (await r.json()) as FieldStack;
  const layers: StackLayer[] = [];
  for (const l of stack.layers) {
    const base = `${dir}/${l.file}`;
    const field = await loadCompressedField(base, { ...opts, referenceUrl: `${base}.ref.f32` });
    let range: [number, number] = [field.meta.min, field.meta.max];
    if (stack.geometry && !field.meta.plane) {
      const mask = solidMask(field.meta, stack.geometry.boxes);
      let [lo, hi] = [Infinity, -Infinity];
      for (let i = 0; i < mask.length; i++) {
        if (!mask[i]) continue;
        lo = Math.min(lo, field.data[i]!);
        hi = Math.max(hi, field.data[i]!);
      }
      if (lo < hi) range = [lo, hi];
    }
    layers.push({ id: l.id, label: l.label, field, range });
  }
  return { stack, layers, totalMs: performance.now() - t0 };
}

/** Whether a field is a 2D plane (`meta.plane`) rather than a volume. */
export const isPlane = (m: FieldMeta) => !!m.plane;

/** Solid mask of a volume field's grid: cell centers inside any geometry box. */
export function solidMask(meta: FieldMeta, boxes: NonNullable<FieldStack["geometry"]>["boxes"]): Uint8Array {
  const [nz, ny, nx] = meta.dims_zyx;
  const [ox, oy, oz] = meta.origin_mm;
  const [vx, vy, vz] = meta.voxel_mm;
  const mask = new Uint8Array(nx * ny * nz);
  for (let k = 0; k < nz; k++)
    for (let j = 0; j < ny; j++)
      for (let i = 0; i < nx; i++) {
        const p = [ox + (i + 0.5) * vx, oy + (j + 0.5) * vy, oz + (k + 0.5) * vz];
        if (boxes.some((b) => p.every((c, a) => c >= b.min[a]! && c <= b.max[a]!))) mask[(k * ny + j) * nx + i] = 1;
      }
  return mask;
}

/**
 * Exposed voxel faces of the solid as a quad soup (two triangles per face),
 * in kernel mm. `cell[q]` is the grid cell of quad q, for coloring and
 * probing.
 */
export function buildSkin(meta: FieldMeta, mask: Uint8Array): { positions: Float32Array; index: Uint32Array; cell: Uint32Array } {
  const [nz, ny, nx] = meta.dims_zyx;
  const [ox, oy, oz] = meta.origin_mm;
  const [vx, vy, vz] = meta.voxel_mm;
  const solid = (i: number, j: number, k: number) =>
    i >= 0 && j >= 0 && k >= 0 && i < nx && j < ny && k < nz && mask[(k * ny + j) * nx + i] === 1;
  const pos: number[] = [];
  const cells: number[] = [];
  // For each axis/side: neighbor offset and the 4 corners (unit cube) CCW from outside.
  const faces: [number[], number[][]][] = [
    [[-1, 0, 0], [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]]],
    [[1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]],
    [[0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]],
    [[0, 1, 0], [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]]],
    [[0, 0, -1], [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]],
    [[0, 0, 1], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]],
  ];
  for (let k = 0; k < nz; k++)
    for (let j = 0; j < ny; j++)
      for (let i = 0; i < nx; i++) {
        if (!solid(i, j, k)) continue;
        for (const [d, corners] of faces) {
          if (solid(i + d[0]!, j + d[1]!, k + d[2]!)) continue;
          for (const c of corners) pos.push(ox + (i + c[0]!) * vx, oy + (j + c[1]!) * vy, oz + (k + c[2]!) * vz);
          cells.push((k * ny + j) * nx + i);
        }
      }
  const nq = cells.length;
  const index = new Uint32Array(nq * 6);
  for (let q = 0; q < nq; q++) index.set([4 * q, 4 * q + 1, 4 * q + 2, 4 * q, 4 * q + 2, 4 * q + 3], 6 * q);
  return { positions: new Float32Array(pos), index, cell: new Uint32Array(cells) };
}
