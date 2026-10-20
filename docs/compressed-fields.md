# Compressed solver fields (tang-compress) — M0 integration + schema proposal

Status: **M0 prototype** (app-side decode + viewer). The `.vcad` / MCP / receipt
parts below are a **proposal** — no kernel or IR changes yet.

## Why

vcad's solvers (thermal, EM, FEA, topopt, flow, stocksim SDF) emit smooth 2D/3D
scalar fields. Today they leave the solver as summary scalars or a downsampled
map, because a raw f32 grid is too big to keep in a document or ship through an
MCP result. tang-compress is an error-bounded codec for exactly this kind of
field. It is 0.67–0.72× SPERR's bits on held-out SDRBench data, and it has a
browser decoder (WebGPU, with a WASM-CPU fallback) that is byte-identical to the
native decoder. So a solver can return the **full field** with a **hard
pointwise error bound**, and the app can show it.

## M0: what exists

| piece | where |
|---|---|
| field producer | `crates/vcad-kernel-thermal/examples/dump_field.rs`: the `hot_chip` steady solve (2 W die on a 100×100×1.6 mm board, k = [15, 15, 0.4], h = 10) on a 128×128×16 grid. Writes raw f32 `(z, y, x)` °C plus a `.json` sidecar. Void voxels are pinned to the 25 °C ambient. |
| encode (offline) | tang-compress `corpus` example (TCG1 / TCG3-static / TCG3-learned) and the `tc` CLI (wavelet, max-abs mode) |
| browser decoder | `tang-compress-wasm` from `/Users/cam/Developer/tang-compress` (branch `claude/tang-compress-qtt`). It is built by `scripts/build-tc-wasm.sh` into `packages/app/public/tc-wasm/`. That folder is **gitignored, not vendored**: wasm-pack writes its own `*` `.gitignore` there. It is loaded lazily via dynamic import on first use, so the main bundle doesn't grow. |
| loader | `packages/app/src/lib/compressed-field.ts`: fetches `.tc` + `.json`, decodes (WebGPU → CPU fallback), computes FNV-1a over the f32 bits, and checks it against the native hash in the sidecar. If `.ref.f32` is present, it also measures the real max error. |
| viewer | `stores/field-overlay-store.ts` + `components/CompressedFieldOverlay.tsx`: a colormapped z-slice at its true mm position in the Z-up kernel group, the domain box, and a HUD with a slice slider, legend, error bound, hash verdict and timings. Open it with `?field=/fields/hot_chip` (add `&fieldcpu` to force the CPU path). |
| sample | `packages/app/public/fields/hot_chip.{tc,json}`: TCG3-static, max-abs 4.962e-3 °C (1e-4 × range). The `.ref.f32` reference is gitignored; regenerate it with `dump_field`. |

### Numbers (hot_chip, 16×128×128 = 262,144 cells, 1 MiB raw)

Bound = max-abs 1e-4 × range = **4.962e-3 °C** (range 25.00 → 74.62 °C).

| stream | bytes | ratio | native CPU decode |
|---|---|---|---|
| `tc` wavelet (max-abs) | 13,537 | 77× | n/a (not browser-decodable) |
| TCG1 | 30,904 | 34× | 10.4 ms |
| **TCG3-static (shipped)** | **23,480** | **45×** | 10.7 ms |
| TCG3-learned | 30,434 | 34× | 114 ms |
| TCG3-static @ 1e-3 × range | 9,602 | 109× | 10.1 ms |

The `tc` wavelet self-check reports a max error of 4.959e-3 against the 4.962e-3
bound. The streams the browser can decode (TCG1/TCG3) cost about 1.7× the bits
of the CPU-only wavelet codec at this size. That gap is the price of
GPU-parallel decode. The browser measurements are in the gate section at the end.

## Proposal: compressed fields in `.vcad` documents

A field is a *result*, not a modeling input. It goes in a new top-level `fields`
map, keyed by a stable id. Solver specs keep referencing parts as they do today.

```jsonc
"fields": {
  "thermal.hot_chip.T": {
    "quantity": "temperature",          // open vocabulary
    "units": "degC",
    "grid": {
      "kind": "regular",                // future: "octree" (stocksim), "mesh_vertex"
      "dims_zyx": [16, 128, 128],
      "origin_mm": [0, 0, 0],           // Z-up kernel frame, voxel-corner origin
      "voxel_mm": [0.78125, 0.78125, 0.1625],
      "mask": null                      // optional: ref to a bit-packed solid mask field
    },
    "range": [25.0, 74.62377],
    "codec": {
      "name": "tang-compress",
      "format": "tcg3s",                // tcg1 | tcg3s | tcg3l
      "model_fnv1a": null,              // tcg3l: fingerprint of the registered net
      "stream": { "encoding": "base64", "data": "…" }, // or { "blob": "sha256:…" }
      "bytes": 23480
    },
    "bound": { "kind": "max_abs", "value": 0.004962377 },
    "decoded_fnv1a": "e1a3021ce69b8d0d", // FNV-1a 64 over the native decode's f32 bits
    "provenance": {
      "oracle": { "id": "vcad-kernel-thermal/steady", "version": "0.10.0" },
      "spec_hash": "…",                 // hash of the resolved ThermalSpec + params
      "geometry_hash": "…"              // hash of the evaluated parts the solve consumed
    }
  }
}
```

Rules:

- **Inline only small streams.** Base64-inline up to about 256 KB per field.
  Above that, store `{ "blob": "sha256:<hex>" }` in the existing Supabase
  storage path or a side file, so documents stay diffable.
- **Stale = drop or flag, never silently show.** If `spec_hash` or
  `geometry_hash` no longer matches the document, the viewer marks the field
  *stale*, the same way `check_clearance` assertions go Stale.
- **`decoded_fnv1a` makes decode deterministic and checkable.** Any client
  (native, WASM-CPU, WebGPU) must reproduce these bits exactly. M0 shows that
  all three do.

## Proposal: MCP results

Solver tools (`solve_thermal`, `simulate_em`, `simulate_flow`,
`topology_optimize`, …) gain an opt-in `return_field: { bound?: {max_abs | max_rel}, format?: "tcg3s" }`.
The result carries a `field` object with the same shape as the document entry.
The stream goes base64 in `structuredContent` or, when large, as an artifact URL.
It is also attached to the session document under `fields`, so `save_document`
keeps it. The default bound is `max_rel 1e-4`, with the absolute value always
echoed. The inline-preview widget uses this same decoder.

## Proposal: receipt claim

The codec's guarantee is a claim like any other. It uses the open
domain vocabulary and needs no schema bump:

```jsonc
{
  "id": "field.compression.max_abs",
  "domain": "field",
  "subject": "field:thermal.hot_chip.T",
  "description": "decoded field within ±4.962e-3 degC of the solver output at every cell",
  "oracle": { "id": "tang-compress/encode-selfcheck", "version": "<crate ver>+<git sha>" },
  "basis": "verified",
  "predicted": { "value": 0.004962377, "units": "degC" },   // the bound
  "measured":  { "value": 0.0049591,   "units": "degC" },   // encoder's self-check max |Δ|
  "verdict": "pass",
  "details": "tcg3s 23480 B; decoded_fnv1a e1a3021ce69b8d0d"
}
```

- **Verified.** The encoder decodes its own stream and measures the error
  against the uncompressed solver output. That is a real oracle run, not an
  estimate. If the measured error exceeds the bound, the verdict is `fail` and
  the stream is not emitted (fail closed).
- **This claim does not grade the physics.** Compression error stacks on top of
  the solver's own claims (`vcad.thermal-claims/1`: energy residual, mesh
  convergence, predicted → Provisional). A thermal `T_max` claim read from the
  *decoded* field must widen its tolerance by the bound. `verify_receipt` can
  re-check this claim by re-decoding the stream and comparing against
  `decoded_fnv1a`. It cannot re-measure |Δ| without the raw field, so on
  re-verify the claim becomes *Holds* (hash matches) or *Violated* (bits
  changed).
- **Pair with a second claim:** `field.decode.deterministic`, verdict pass when
  the client's decode hash equals `decoded_fnv1a`. This is the M0 browser
  gate, written as a receipt line.

## M0 gate (2026-10-20, M4, Chrome in the Claude desktop browser pane)

![hot_chip z-slice at 1.54 mm in the viewport](assets/compressed-field-hot-chip.jpg)

The stream is `hot_chip.tc` (TCG3-static, 23,480 B for 262,144 cells, 45×).

| check | WebGPU | WASM CPU |
|---|---|---|
| FNV-1a of decode | `e1a3021ce69b8d0d` | `e1a3021ce69b8d0d` |
| = native CPU decode (`corpus` manifest) | ✓ | ✓ |
| measured max \|Δ\| vs raw solver output | 4.9610e-3 °C | 4.9610e-3 °C |
| claimed bound | 4.9624e-3 °C ✓ | 4.9624e-3 °C ✓ |
| cold page load: fetch + wasm init + adapter + decode | 28 + 64 + 112 = **204 ms** | — |
| warm decode (same page, best of 3, total incl. fetch) | **57 ms** (kernels ≈ 55 ms) | **17 ms** |

- **Gate: PASS.** Both browser paths are byte-identical to native. The measured
  error is inside the bound, and the slice is visibly correct: the hot spot sits
  on the 45–55 mm die footprint, falls off radially across the board, and the
  board edges are the coolest region.
- One warm WebGPU run took 8.3 s. That was likely the browser pane being
  backgrounded and throttled; it did not repeat in later runs.
- **At this field size, CPU beats GPU.** At 262k cells TCG3-static decode is
  dominated by per-dispatch overhead (the tang-compress-wasm README shows the
  same pattern). WebGPU starts to pay off at roughly 512³ and above. A sensible
  next step is a size-based backend choice.
- **Build note.** The tang-compress integration branch (`0d9a886`) does not
  currently build for wasm32: `interp.rs:322` has `n > 1 << 34`, which
  overflows on 32-bit `usize`. The pkg used here was built from a scratch
  worktree with that comparison done in `u64`. The fix belongs on
  `claude/tang-compress-qtt`.
