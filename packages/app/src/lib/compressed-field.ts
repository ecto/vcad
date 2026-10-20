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
  opts: { forceCpu?: boolean; referenceUrl?: string } = {},
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
  const gpu = opts.forceCpu ? null : await getGpu(mod);
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
