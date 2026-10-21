# cram-codec-wasm

Browser decoder for cram GPU streams: **TCG1** (bricks) and
**TCG3** (code-blocks, static or learned probabilities). It runs the native
WGSL kernels (`cram/src/gpuwav.wgsl`) through wgpu's WebGPU backend.
It also has a CPU fallback that runs the same integer reference decoder
compiled to WASM. Both paths produce byte-identical output to native
`gpuwav::decode_box_parsed` / `gpuwav3::decode3_box`.

## Build

```bash
cd crates/cram-codec-wasm
wasm-pack build --target web --release --out-dir demo/pkg --no-typescript
```

This produces `demo/pkg/cram_codec_wasm.js` and `cram_codec_wasm_bg.wasm`
(about 300 KB, after wasm-opt).

## JS API

```js
import init, { TcGpu, decodeCpu, fnv1a, registerModel, streamInfo } from "./pkg/cram_codec_wasm.js";
await init();
registerModel(tcmBytes);              // pretrained net for learned TCG3 (stream carries only its fingerprint)
const info = JSON.parse(streamInfo(bytes)); // {format, dims, lanes, learned, cells, bytes}
const gpu = await TcGpu.create();     // throws without navigator.gpu → use decodeCpu
const f32 = await gpu.decode(bytes);  // Float32Array, row-major dims (z, y, x)
JSON.parse(gpu.lastTimes());          // {parse_s, upload_s, kernels_s, readback_s}
const f32cpu = decodeCpu(bytes);      // same bits, synchronous, one thread (run in a Worker)
fnv1a(f32);                           // hex FNV-1a 64 over the f32 bit patterns
await gpu.warm(bytes);                // optional: build the pipelines for a stream kind ahead of time
```

`TcGpu` caches pipelines per (lanes, line length rounded up to a power of
two). For learned streams it also caches per net, because the MLP is compiled
into WGSL as literals. The first stream of each kind pays for shader
compilation.

## Demo and gate

```bash
scripts/corpus.sh /path/to/cram/bench   # 19 fields × 5 targets × {tcg1, tcg3s, tcg3l} + manifest.json
python3 -m http.server 8765 --directory demo     # open http://localhost:8765/
python3 scripts/gate.py                          # headless Chrome; exits non-zero on any hash mismatch
```

- `demo/index.html` loads a corpus stream or a local `.tc` file. It decodes on
  WebGPU, or falls back to CPU, and shows one z-slice.
- `index.html?gate` decodes every manifest stream on WebGPU and on the WASM
  CPU path. It compares the FNV-1a hash of each result with the hash of the
  native CPU decode that `corpus` wrote. Results go to the page and
  `window.__gate`.
- `scripts/gate.py` serves `demo/` and runs that page in headless Chrome
  (`--enable-unsafe-webgpu`). It collects the posted result into
  `demo/gate.json`.

The corpus uses these formats:

- `tcg1`: 64³ / 512² bricks, 32 lanes.
- `tcg3s`: 32³ / 128² blocks, 1 lane, static priors.
- `tcg3l`: 32³ / 128² blocks, 8 lanes, pretrained 32×16 net.

## WebGPU notes

- **Async only.** On wasm32, `GpuDecoder::new` is replaced by `new_async`,
  `decode`/`decode3` by `decode_async`/`decode3_async`, and readback is an
  awaited `map_async`. Native polling (`PollType::Wait`) is a no-op in the
  browser.
- **No `on_submitted_work_done` in wgpu 25's WebGPU backend.** It panics with
  "not implemented". Kernel completion is awaited by mapping a 4-byte buffer
  copied from the output instead. So the browser's `kernels_s` includes one
  map round trip.
- **Limits.** The decoder requests the adapter's `maxBufferSize`,
  `maxStorageBufferBindingSize` and `maxComputeWorkgroupsPerDimension`.
  Chrome on M4 (Metal 3) reports 4 GiB, 4 GiB and 65535. That leaves room for
  whole fields up to roughly 1G cells per buffer. The real cap is browser
  memory, because the decode holds about 3 field-sized `u32` buffers plus the
  staging copy. Everything else fits WebGPU defaults: the largest workgroup is
  256 invocations (TCG1 `decode` at 256 lanes; the corpus uses 32), no
  workgroup storage beyond one `u32`, and 6 storage/uniform bindings.
- **No subgroups.** The kernels use no subgroup builtins. The `decode3l`
  "32-wide" packing is only a workgroup size, so nothing assumes a SIMD width,
  and Chrome's Tint compiled the WGSL unchanged.
- **32-bit `usize`.** Two size guards (`n > 1 << 34`) overflowed on wasm32 and
  now compare in `u64`. Fields above 4G cells cannot be addressed on wasm32
  anyway.

## Results (M4, Chrome 155 headless, 2026-10-20)

- **Gate:** 285/285 byte-identical. That is 19 fields × p60/p80/p100/m1e-3/m1e-4
  × {tcg1, tcg3s, tcg3l}, with the WebGPU decode and the WASM CPU decode each
  hashed against the native CPU decode.
- **Throughput:** GB/s of decoded f32, total cells over total time across the
  95 streams per format. "End-to-end" is parse, upload, kernels and readback.
  The browser figures are the best of 3 runs after the pipelines were built.

| format | web GPU end-to-end | native GPU end-to-end | web CPU, 1 thread | native CPU, 1 thread |
|---|---|---|---|---|
| tcg1  | 0.114 | 0.067 | 0.067 | 0.068 |
| tcg3s | 0.021 | 0.013 | 0.060 | 0.070 |
| tcg3l | 0.030 | 0.019 | 0.0049 | 0.0065 |

- **Treat the native GPU column as a pessimistic baseline, not a fair
  comparison.** Native GPU times were taken inside `corpus` while it was
  encoding with 6 threads, so the machine was loaded (M4 timings also drift by
  up to ±2×). The browser ran on an idle machine.
- **What the numbers do show:**
  - WebGPU adds no visible overhead over native wgpu/Metal on these streams.
  - WASM CPU runs at 0.75–0.86× native single-thread speed.
- **Absolute GB/s is low because the fields are small** (1M cells, or 1M for
  1024²). Fixed per-dispatch costs dominate: about 45 lift dispatches, and
  1-lane tcg3s decodes one block per invocation. The report's 512³ numbers
  (0.6–7 GB/s) are the large-field regime.
