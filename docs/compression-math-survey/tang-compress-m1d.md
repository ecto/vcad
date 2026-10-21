# cram M1d: tc-gpu, a GPU-decodable random-access wavelet codec

Repo: `/Users/cam/Developer/cram`, branch `claude/cram-qtt` (not pushed).
Machine: Apple M4 Max (Metal through wgpu 25). The machine was **heavily loaded** for the whole session: load average 10–70, from other sessions sharing the CPU and probably the GPU. **All timings are noisy, ±2× between repeats.** Rates (bpv) are exact.

## Headline

| gate | result |
|---|---|
| (a) CPU and GPU decode bit-identical on every test | **MET.** 190/190 sweep cases: 19 fields × 5 targets × 2 brick sizes, 189M cells compared, 0 differing. Plus the unit tests and every `speed` run (134M-cell NYX 512³). |
| (b) GPU decode ≥ 1 GB/s | **MET on kernels**: 1.3–7.3 GB/s across all configurations, NYX 512³. **Mostly met including transfers**: 0.43–2.1 GB/s. 32³ bricks were ≥ 0.94 GB/s in every run and 2.1 GB/s on the least-loaded run. 64³ bricks fall below 1 GB/s in some runs. |
| (c) ratio cost ≤ 5% vs serial tc-wavelet (geomean, 80 dB) | **FAILED.** Cost is +48% at 32³, +25% at 64³, and +15% at 128³/whole-field. |

## 1. Design

**Stream `TCG1`** (`src/gpuwav.rs`) is a variant of the M1 wavelet codec. Choices first, with alternatives and reasons:

1. **Independent bricks.** Each brick (3D B³, 2D B² tiles, 1D B²-sample runs) has its own CDF 9/7 pyramid with the same rules as M1, and its own lanes.
   - The header holds a varint brick-size index, so random access decodes only the bricks a box touches.
   - Global data is shared by all bricks: grid exponent, mean, step, 30 centroids, predictors per (level, orientation, pass), and per-context priors.
   - *Alternative not built:* a JPEG2000-style global transform with code-blocks, i.e. bricking in the coefficient domain. It avoids the brick-boundary cost (see §3), but random access then needs halo coefficients at every level and a windowed inverse.
2. **Parallel coding order.**
   - Coarse → fine over bands. Inside a band, the lines along the fastest axis are partitioned into hierarchical parity passes:
     - 3D, 7 passes: `(y,z) mod 4 ∈ {00, 22, 20, 02}`, then the odd lattices `{11, 10, 01}`.
     - 2D, 4 passes: `y ≡ 0 mod 8`, `4 mod 8`, `2 mod 4`, odd.
   - A line sees: earlier passes of its band (faces, diagonals, ±2/±4 lines), its own earlier samples (W, WW), and coarser bands (parent plus the parent's 6 faces).
   - The lines of one pass are dealt round-robin to L interleaved rANS lanes (default 32): 32-bit state, 16-bit words, 16-bit probabilities. Binary decisions use the hand model's binarization: significance, adaptive-Golomb unary, 2 modelled low bits, raw bits, sign, gamma escape.
   - A 2-pass checkerboard was tried first. The 7-pass hierarchy gained only 0.5% over it.
3. **Context models are static.** The per-context probabilities are the transmitted priors: 64-level logit grid, adaptive-coded presence + delta in the header.
   - Per-lane adaptive models (4.2k models of state per lane) were built first. The GPU decode was then memory-bound: 430 ms vs 11–17 ms for 256³ at 100 dB, about 30× slower.
   - Static priors with the same contexts cost **+1–4% bpv** (miranda 80 dB: 1.153 vs 1.117; hurricane-U: 2.032 vs 1.945; nyx256: 1.308 vs 1.299).
   - Static priors make lanes stateless, so lanes no longer dilute model learning. With adaptive models, 32 lanes and no priors cost +34% at 60 dB.
   - Adaptive mode is kept as a CPU-only option (`GpuConfig::adaptive`).
4. **Integer-only decode.**
   - Fixed-point grid `x ≈ v·2^e`. `e` is chosen so `|x|/2^e ≤ 2^23` and `|x−mean|/2^e ≤ 2^20` (headroom for the LL gain in `i32`).
   - Dequant: `c = ⌊|q|·m/2^s⌉ − ⌊δ·m/2^(s+9)⌉`, with a 30-bit step mantissa per band.
   - Inverse CDF 9/7 lifting with Q24 constants: `⌊(a·k + 2^23)/2^24⌋` through a 32×32→64 multiply. WGSL emulates it with 16-bit limbs.
   - Max-abs corrections `v += k·⌊2b/2^e⌋` are applied in `i32`.
   - Output `f32 = (v+mean) as f32 · 2^e`. This is exact, because `|v| < 2^24` and the scale is a normal power of two (e ≥ −126, so no denormals and no flush-to-zero divergence).
   - Cells the grid still pushes past `b` are stored as raw f32 bits.
   - We kept 9/7 rather than switching to 5/3. The encoder rate-controls on the *integer decoder's* output, so any fixed-point loss is already inside every number reported here. Every decoded max-abs case met its bound, every PSNR case met its target, and the fixed-point grid adds at most 2^-21 of the maximum deviation. A separate fixed-point-vs-float ablation was not run.
5. **CPU reference decoder:** single- and multi-threaded (bricks on scoped threads), plus a sub-box decode (`decode_box_bits`).

**GPU decoder** (`src/gpudec.rs`, `src/gpuwav.wgsl`, feature `gpu`).
- We did not use tang-gpu. Its kernels are f32 tensor programs generated from tang-expr, and the crate pulls tang-compile, safetensors and the CUDA/GLSL paths. The decoder is plain wgpu, following tang-gpu's device-init pattern (adapter limits).
- Kernels, all integer:
  1. `decode`: one workgroup per brick, one invocation per lane. Pass-major with `storageBarrier`+`workgroupBarrier` between passes; loop bounds come through `workgroupUniformLoad`. Context and prediction come from the coefficient buffer. Ends with in-place fixed-point dequant.
  2. `lift`: one dispatch per (level, axis), one invocation per line, i32 lifting in a private line buffer.
  3. `outliers`: one invocation per brick decodes the correction lane.
  4. `finalize`: one invocation per cell.
  5. `escapes`.
- Host prep: parse the header, build the brick/band/lane tables, upload the stream as u32.

## 2. Tests

- `cargo test -p cram-codec` (45) and `--features gpu` (46) pass. They cover:
  - rANS round trip
  - PSNR round trip across brick sizes, with output identical for any thread count
  - max-abs bound plus random-access crop == full-decode crop
  - 2D and 1D
  - the 64-bit `mulsh` against i128
  - the pass lattice == the parity partition
  - `gpu_matches_cpu`: 4 bounds × 3 brick sizes + 2D
- `cargo clippy -p cram-codec --all-targets` is clean. The remaining warnings are in other tang crates.
- **Sweep verification** (`gpubench verify`, results in `bench/out/gpu_verify.jsonl`):
  - All 19 local fields: the original 5 crops and the 14 held-out fields.
  - Targets: 60/80/100 dB and max-abs 1e-3/1e-4.
  - Bricks 32³ and 64³ for 3D, 256² and 512² for 2D.
  - Checks: GPU == CPU byte for byte; the error bound re-verified; a 64³ sub-box straddling bricks equals the crop of the full decode.
  - **190/190 passed.**

## 3. Ratio cost

All 19 fields; bpv from actual stream sizes.
- tc-wavelet: the serial M1 hand coder at the same target.
- SPERR: interpolated on its measured front at tc-gpu's measured PSNR (max-abs: SPERR at the same bound), from the M1/held-out CSVs.
- Classes: A = 32³ / 256², B = 64³ / 512², C = 128³ / 1024². For the 64×128×128 crops and the 1024² fields, C is the whole field.

### Geometric-mean bpv ratio

| ratio | bricks | p60 | p80 | p100 | m1e-3 | m1e-4 |
|---|---|---|---|---|---|---|
| tc-gpu / tc-wavelet (serial) | 32³ / 256² | 1.954 (n=19) | 1.486 (n=19) | 1.333 (n=19) | 1.516 (n=19) | 1.368 (n=19) |
| tc-gpu / tc-wavelet (serial) | 64³ / 512² | 1.325 (n=19) | 1.253 (n=19) | 1.209 (n=19) | 1.229 (n=19) | 1.215 (n=19) |
| tc-gpu / tc-wavelet (serial) | 128³ / 1024² | 1.131 (n=19) | 1.152 (n=19) | 1.151 (n=19) | 1.119 (n=19) | 1.147 (n=19) |
| tc-gpu / SPERR | 32³ / 256² | 1.870 (n=19) | 1.325 (n=19) | 1.193 (n=19) | 1.142 (n=19) | 1.139 (n=19) |
| tc-gpu / SPERR | 64³ / 512² | 1.269 (n=19) | 1.117 (n=19) | 1.082 (n=19) | 0.925 (n=19) | 1.011 (n=19) |
| tc-gpu / SPERR | 128³ / 1024² | 1.083 (n=19) | 1.027 (n=19) | 1.030 (n=19) | 0.843 (n=19) | 0.954 (n=19) |
| tc-wavelet (serial) / SPERR | — | 0.957 (n=19) | 0.892 (n=19) | 0.895 (n=19) | 0.753 (n=19) | 0.832 (n=19) |

### Per-field bpv

| field | target | serial | A | B | C | SPERR |
|---|---|---|---|---|---|---|
| cesm-CLDTOT | m1e-3 | 2.321 | 2.398 | 2.37 | 2.357 | 2.722 |
| cesm-CLDTOT | m1e-4 | 5.331 | 5.504 | 5.472 | 5.457 | 5.968 |
| cesm-CLDTOT | p100 | 7.439 | 7.557 | 7.526 | 7.512 | 7.903 |
| cesm-CLDTOT | p60 | 1.224 | 1.294 | 1.27 | 1.259 | 1.408 |
| cesm-CLDTOT | p80 | 4.132 | 4.244 | 4.214 | 4.202 | 4.577 |
| cesm-FLDS | m1e-3 | 0.4385 | 0.4931 | 0.4657 | 0.4566 | 0.6328 |
| cesm-FLDS | m1e-4 | 2.47 | 2.57 | 2.532 | 2.516 | 2.907 |
| cesm-FLDS | p100 | 4.247 | 4.407 | 4.37 | 4.355 | 4.73 |
| cesm-FLDS | p60 | 0.08839 | 0.1204 | 0.1014 | 0.09549 | 0.09945 |
| cesm-FLDS | p80 | 1.349 | 1.436 | 1.406 | 1.392 | 1.572 |
| cesm-LHFLX | m1e-3 | 0.5514 | 0.6273 | 0.5965 | 0.5827 | 0.7516 |
| cesm-LHFLX | m1e-4 | 2.344 | 2.505 | 2.465 | 2.444 | 2.787 |
| cesm-LHFLX | p100 | 4.007 | 4.264 | 4.221 | 4.197 | 4.499 |
| cesm-LHFLX | p60 | 0.1773 | 0.2202 | 0.1982 | 0.1893 | 0.2076 |
| cesm-LHFLX | p80 | 1.299 | 1.436 | 1.399 | 1.379 | 1.547 |
| cesm-TS | m1e-3 | 0.2005 | 0.2329 | 0.212 | 0.2079 | 0.2592 |
| cesm-TS | m1e-4 | 0.6794 | 0.7758 | 0.7352 | 0.7252 | 0.8972 |
| cesm-TS | p100 | 1.304 | 1.506 | 1.458 | 1.439 | 1.593 |
| cesm-TS | p60 | 0.03687 | 0.06091 | 0.04262 | 0.03911 | 0.04098 |
| cesm-TS | p80 | 0.2771 | 0.3326 | 0.3028 | 0.2956 | 0.3342 |
| exafel-ev0 | m1e-3 | 3.463 | 3.572 | 3.552 | 3.547 | 4.023 |
| exafel-ev0 | m1e-4 | 6.844 | 6.93 | 6.908 | 6.902 | 7.315 |
| exafel-ev0 | p100 | 9.035 | 9.062 | 9.043 | 9.038 | 9.247 |
| exafel-ev0 | p60 | 2.302 | 2.343 | 2.324 | 2.32 | 2.647 |
| exafel-ev0 | p80 | 5.69 | 5.69 | 5.671 | 5.666 | 5.935 |
| hurricane-TC | m1e-3 | 0.2465 | 0.4158 | 0.306 | 0.2875 | 0.3435 |
| hurricane-TC | m1e-4 | 1.331 | 1.933 | 1.67 | 1.628 | 1.411 |
| hurricane-TC | p100 | 2.364 | 3.27 | 2.942 | 2.89 | 2.409 |
| hurricane-TC | p60 | 0.03285 | 0.119 | 0.04623 | 0.039 | 0.02872 |
| hurricane-TC | p80 | 0.6644 | 1.055 | 0.8498 | 0.818 | 0.6118 |
| hurricane-U | m1e-3 | 0.8744 | 1.193 | 1.058 | 1.02 | 0.9621 |
| hurricane-U | m1e-4 | 2.592 | 3.353 | 3.12 | 3.052 | 2.81 |
| hurricane-U | p100 | 4.062 | 5.059 | 4.778 | 4.695 | 4.343 |
| hurricane-U | p60 | 0.3385 | 0.511 | 0.4201 | 0.397 | 0.3232 |
| hurricane-U | p80 | 1.68 | 2.288 | 2.09 | 2.033 | 1.711 |
| hurricane-V | m1e-3 | 0.8197 | 1.109 | 0.988 | 0.9534 | 0.8708 |
| hurricane-V | m1e-4 | 2.471 | 3.172 | 2.957 | 2.896 | 2.637 |
| hurricane-V | p100 | 3.956 | 4.869 | 4.614 | 4.547 | 4.166 |
| hurricane-V | p60 | 0.2756 | 0.4271 | 0.3447 | 0.3217 | 0.2593 |
| hurricane-V | p80 | 1.589 | 2.132 | 1.959 | 1.91 | 1.565 |
| miranda-density | m1e-3 | 0.6371 | 0.9538 | 0.7361 | 0.6831 | 0.8117 |
| miranda-density | m1e-4 | 1.694 | 2.294 | 1.924 | 1.827 | 2.005 |
| miranda-density | p100 | 2.504 | 3.338 | 2.883 | 2.756 | 2.843 |
| miranda-density | p60 | 0.2463 | 0.4525 | 0.3115 | 0.277 | 0.3074 |
| miranda-density | p80 | 1.064 | 1.517 | 1.226 | 1.154 | 1.196 |
| miranda-pressure | m1e-3 | 0.5205 | 0.7733 | 0.611 | 0.569 | 0.6916 |
| miranda-pressure | m1e-4 | 1.544 | 2.061 | 1.766 | 1.683 | 1.829 |
| miranda-pressure | p100 | 2.361 | 3.117 | 2.726 | 2.602 | 2.648 |
| miranda-pressure | p60 | 0.1876 | 0.3301 | 0.2332 | 0.209 | 0.2323 |
| miranda-pressure | p80 | 0.9148 | 1.293 | 1.069 | 1.006 | 1.036 |
| miranda-velocityx | m1e-3 | 0.4522 | 0.7452 | 0.5498 | 0.4973 | 0.5899 |
| miranda-velocityx | m1e-4 | 1.403 | 2.027 | 1.651 | 1.541 | 1.616 |
| miranda-velocityx | p100 | 2.163 | 3.061 | 2.56 | 2.401 | 2.347 |
| miranda-velocityx | p60 | 0.1723 | 0.3388 | 0.2218 | 0.1904 | 0.1913 |
| miranda-velocityx | p80 | 0.8161 | 1.271 | 0.9823 | 0.9017 | 0.8756 |
| nyx-temperature | m1e-3 | 1.236 | 1.42 | 1.321 | 1.293 | 1.587 |
| nyx-temperature | m1e-4 | 3.958 | 4.273 | 4.147 | 4.115 | 4.567 |
| nyx-temperature | p100 | 6.089 | 6.396 | 6.256 | 6.22 | 6.5 |
| nyx-temperature | p60 | 0.4451 | 0.574 | 0.4957 | 0.474 | 0.5746 |
| nyx-temperature | p80 | 2.752 | 3.023 | 2.895 | 2.861 | 3.131 |
| nyx-velocity_x | m1e-3 | 0.8436 | 1.026 | 0.9199 | 0.8904 | 1.137 |
| nyx-velocity_x | m1e-4 | 3.458 | 3.789 | 3.649 | 3.605 | 4.047 |
| nyx-velocity_x | p100 | 5.545 | 5.872 | 5.723 | 5.676 | 5.979 |
| nyx-velocity_x | p60 | 0.2003 | 0.3091 | 0.2362 | 0.2156 | 0.2586 |
| nyx-velocity_x | p80 | 2.242 | 2.54 | 2.395 | 2.353 | 2.603 |
| qmcpack-orb0 | m1e-3 | 0.1417 | 0.328 | 0.2376 | 0.1523 | 0.2137 |
| qmcpack-orb0 | m1e-4 | 0.3431 | 0.6821 | 0.5255 | 0.3819 | 0.4994 |
| qmcpack-orb0 | p100 | 0.4183 | 0.8725 | 0.6499 | 0.4727 | 0.5699 |
| qmcpack-orb0 | p60 | 0.0493 | 0.1712 | 0.1012 | 0.05161 | 0.059 |
| qmcpack-orb0 | p80 | 0.1464 | 0.3687 | 0.2622 | 0.163 | 0.205 |
| qmcpack-orb100 | m1e-3 | 0.2028 | 0.5327 | 0.3494 | 0.2242 | 0.3581 |
| qmcpack-orb100 | m1e-4 | 0.5421 | 1.266 | 0.868 | 0.6251 | 0.8636 |
| qmcpack-orb100 | p100 | 0.8005 | 1.809 | 1.275 | 0.9393 | 1.149 |
| qmcpack-orb100 | p60 | 0.06881 | 0.2536 | 0.1411 | 0.07665 | 0.08352 |
| qmcpack-orb100 | p80 | 0.2656 | 0.7373 | 0.4681 | 0.3001 | 0.4102 |
| scale-PRES | m1e-3 | 0.03546 | 0.1697 | 0.06923 | 0.05766 | 0.08129 |
| scale-PRES | m1e-4 | 0.1922 | 0.7298 | 0.494 | 0.4559 | 0.3148 |
| scale-PRES | p100 | 0.3822 | 1.288 | 0.981 | 0.9668 | 0.3944 |
| scale-PRES | p60 | 0.008667 | 0.08255 | 0.01816 | 0.0125 | 0.007816 |
| scale-PRES | p80 | 0.05779 | 0.3188 | 0.1485 | 0.126 | 0.07114 |
| scale-QV | m1e-3 | 0.6691 | 0.8788 | 0.7865 | 0.7554 | 0.7633 |
| scale-QV | m1e-4 | 2.185 | 2.627 | 2.497 | 2.456 | 2.511 |
| scale-QV | p100 | 3.341 | 3.927 | 3.79 | 3.744 | 3.684 |
| scale-QV | p60 | 0.1952 | 0.3158 | 0.2508 | 0.2318 | 0.1588 |
| scale-QV | p80 | 1.277 | 1.644 | 1.529 | 1.489 | 1.398 |
| scale-T | m1e-3 | 0.1897 | 0.3766 | 0.2605 | 0.2348 | 0.2468 |
| scale-T | m1e-4 | 1.275 | 1.831 | 1.604 | 1.54 | 1.417 |
| scale-T | p100 | 2.441 | 3.315 | 3.047 | 2.971 | 2.702 |
| scale-T | p60 | 0.03818 | 0.1211 | 0.05823 | 0.04898 | 0.02316 |
| scale-T | p80 | 0.5621 | 0.9319 | 0.7475 | 0.695 | 0.4886 |
| scale-U | m1e-3 | 0.845 | 1.204 | 1.057 | 0.9983 | 0.952 |
| scale-U | m1e-4 | 2.77 | 3.553 | 3.344 | 3.26 | 3.158 |
| scale-U | p100 | 4.494 | 5.461 | 5.256 | 5.16 | 4.932 |
| scale-U | p60 | 0.3176 | 0.5285 | 0.4279 | 0.3916 | 0.2574 |
| scale-U | p80 | 1.751 | 2.406 | 2.216 | 2.139 | 1.902 |

### Where the cost comes from

Measured on nyx-temperature 256³ at 80 dB, where all bricks are interior:

| configuration | bpv |
|---|---|
| serial tc-wavelet | 1.240 |
| tc-gpu, whole field | 1.308 (+5.5%) |
| 128³ bricks | 1.326 (+6.9%) |
| 64³ bricks | 1.371 (+10.6%) |
| 32³ bricks | 1.460 (+17.7%) |

So there are two parts:

1. **The coder, about +5–15% whole-field.**
   - Static vs adaptive probabilities: +1–4%.
   - Pass-restricted contexts and predictor: the rest.
   - It is worst on the anisotropic atmospheric fields: Hurricane and SCALE are +20–24% whole-field. On scale-PRES, 0.058 bpv serial, tc-gpu is 2.2× because the ~1.2 KB of priors and predictors in the header is 7% of the stream.
   - One tried change helped only a little: residual-based (instead of value-based) magnitude contexts gave −2.5% on Hurricane-U and ≈0 elsewhere. It was not adopted, because it needs a second GPU buffer.
2. **Bricking:** symmetric extension at brick faces, fewer levels, a per-lane 4-byte state plus varint size. At low rate the lane overhead dominates: NYX 512³ at 80 dB, 32³ bricks, gives 0.106 bpv with 32 lanes and 0.144 with 64 lanes.

Against SPERR, the 128³ / whole-field class is at parity: 1.03 at 80 and 100 dB, 0.84–0.95 at max-abs. The 64³ class wins at max-abs 1e-3 (0.93) and is +8–12% at matched PSNR.

## 4. Throughput

Throughput is GB/s of decoded f32. The field is full NYX temperature 512³ (537 MB, local `bench/data`), plus Hurricane-Isabel U 100×500×500. These are from `bench/out/gpu_speed.jsonl`; each number is the best of 3–5 runs, on a loaded machine.

- **GPU kernels:** excludes upload and readback. Stream and tables are already resident.
- **GPU total:** bytes in host memory → parse → tables → upload → kernels → readback.

| field / target | bricks, lanes | bpv | GPU kernels | GPU total | CPU 1 thread | CPU 16 threads |
|---|---|---|---|---|---|---|
| NYX 512³ 80 dB | 32³, 32 | 0.106 | 2.65–3.35 | 1.06–2.11 | 0.038–0.057 | 0.28–0.40 |
| NYX 512³ 80 dB | 64³, 32 | 0.063 | 1.97–5.00 | 1.40–2.11 | 0.034–0.070 | 0.37–0.54 |
| NYX 512³ 80 dB | 128³, 32 | 0.054 | 1.83 | 0.71 | 0.034 | 0.25 |
| NYX 512³ 80 dB | 32³, 64 | 0.144 | 7.34 | 0.94 | 0.052 | 0.18 |
| NYX 512³ 80 dB | 64³, 64 | 0.067 | 6.81 | 1.13 | 0.021 | 0.21 |
| NYX 512³ 60 dB | 32³, 64 | 0.079 | 3.78 | 1.08 | 0.020 | 0.08 |
| NYX 512³ 100 dB | 32³, 64 | 1.133 | 4.13 | 1.52 | 0.012 | 0.12 |
| NYX 512³ 100 dB | 64³, 64 | 1.002 | 3.69 | 0.87 | 0.023 | 0.17 |
| NYX 512³ max-abs 1e-4 | 32³, 64 | 0.474 | 3.50 | 0.67 | 0.028 | 0.12 |
| Hurricane U 80 dB | 32³, 64 | 1.897 | 3.56 | 0.85 | 0.034 | 0.16 |

- **Kernel split** at NYX 80 dB, 32³: decode ≈ 120 ms, lift ≈ 40–80 ms, finalize ≈ 6–14 ms.
- **Transfer:** readback is 0.07–0.30 s, which caps the total at ~2 GB/s here. The upload of a compressed stream is ≤ 0.03 s.
- **References on the same NYX 512³ field.** These are not rate-matched: SPERR's PSNR mode overshot to 95 dB at 1.37 bpv, and SZ3 reached 84.6 dB at 0.27 bpv.

  | codec | decode |
  |---|---|
  | SPERR, 1 thread | 0.095 GB/s |
  | SPERR, 16 OpenMP threads | 0.227 GB/s (includes file I/O) |
  | SZ3, serial | 0.59 GB/s |

  tc-gpu's GPU decode is about 10–30× faster than SPERR and 2–12× faster than SZ3. Its CPU decode is comparable to SPERR's.

**Random access** to a 64³ sub-box at the field centre, CPU, including header parse:

| bricks | 1 thread | 16 threads |
|---|---|---|
| 32³ | 22–41 ms | 5–17 ms |
| 64³ | 107–511 ms | 14–60 ms |
| 128³ | 0.9–2.6 s | 0.1–0.4 s |

The centred box is not brick-aligned, so it touches 27 bricks of 32³ or 8 of 64³. GPU random access was not implemented: the kernels take whole streams.

**Encode** is CPU only (16 threads): 16–190 s for NYX 512³, mostly the Δ bisection on the integer decoder.

## 5. What an integer dialect of tang-expr would need

Emitting these kernels from tang-expr instead of hand-written WGSL would need:
1. **Integer scalar types** (`i32` and `u32`), with *defined wrapping* `+ − ×`, arithmetic and logical shifts, bitwise ops, `bitcast`, `firstLeadingBit` and `clamp`, kept distinct from the f32 graph.
2. **A wide multiply primitive** `mulhi`/`mul64 → (lo, hi)` with a shift-round lowering: native on CUDA/MSL, 16-bit limbs on WGSL.
3. **Stateful sequential primitives:** a loop-carried rANS state with a data-dependent word fetch, and loops with data-dependent trip counts (unary codes, escapes).
4. **Workgroup structure:** workgroup-scoped barriers inside uniform loops, `workgroupUniformLoad` for brick-dependent bounds, and indexed scatter/gather into storage buffers.
5. **A bit-exactness contract:** no float reassociation or FMA contraction on the integer path, and a CPU interpreter for the same IR. That interpreter would *be* the reference decoder, so bit-identity holds by construction rather than by parallel hand-written code.

Items 1, 2 and 4 are mechanical extensions of the codegen. Item 3 is the real change, because tang-expr is a pure expression DAG today.

## 6. Commits (`claude/cram-qtt`)

- `0b706b6` tc-gpu stream (TCG1): bricks, pass-ordered lane-parallel rANS, integer decode, CPU reference decoder
- `08b708c` GPU decoder (wgpu + WGSL), bit-identical
- `c70080c` static context models (stateless lanes), direct pass-lattice enumeration, decode kernel rewrite (≈30× faster)
- `98d45e0` gpubench verify mode, clippy; bench sweep (`run_gpu.py`), summary (`summarize_gpu.py`), SPERR/SZ3 reference (`speed_ref.py`)

## 7. Reproduce

```
cargo build --release -p cram-codec --features gpu --example gpubench
cd bench && .venv/bin/python run_gpu.py 32 && .venv/bin/python summarize_gpu.py
../target/release/examples/gpubench speed data/nyx-temperature.f32 512x512x512 --bricks 32,64 --target p80 --lanes 32 --reps 5
../target/release/examples/gpubench verify <field> <dims> --bricks 32,64 --targets p60,p80,p100,m1e-3,m1e-4
.venv/bin/python speed_ref.py data/nyx-temperature.f32 512x512x512 80
```

## 8. Next steps

1. **Close the ratio gap.** The two candidates are code-blocks over a global transform, which removes the bricking cost, and residual contexts plus richer static contexts and a compact prior header, which address the coder cost.
2. **GPU random access:** decode the touched bricks only.
3. **Faster lift kernel:** shared-memory lines.
4. **Overlapping readback.**
5. **Learned integer-MLP probabilities on GPU.** These are not started. The static-prior slot is where they would plug in.

---

## 9. v3: code-blocks over one global transform (TCG3)

TCG1 is unchanged, and its streams still decode. v3 lives in `src/gpuwav3.rs`, plus `decode3`, `decode3l`, `dequant3` and `outliers3` in `gpuwav.wgsl`. Machine load during these runs was 5–40, so timings vary by up to 2×.

### Design

- **Transform.** One global fixed-point CDF 9/7 pyramid over the whole field: the same integer lifting and dequantization as TCG1, applied to a single brick the size of the field.
- **Code-blocks.** Each subband is tiled into blocks: 16³ or 32³ in 3D, 64² or 128² in 2D.
  - A *one-block-per-band* setting (4096) isolates what the block rule itself costs.
  - The header carries a per-block byte-size index. Max-abs corrections are coded per 64³ spatial tile (256² in 2D).
- **Dependency rule.** Levels are decoded coarse → fine.
  - A block may read any coefficient of a coarser level. Its context uses the parent and the parent's 6 face neighbours (values and residuals).
  - Inside its own region it may read every earlier line in raster order, the x±1 neighbours of its face lines, and its own earlier samples.
  - It may read nothing else at its own level. So all blocks of one level are independent.
- **Lanes per block.**
  - `lanes = 1` (default): one rANS lane per block, raster order. The parallelism comes from the number of blocks.
  - `lanes = 8`: TCG1's parity passes inside the block, with 8 lanes and a barrier per pass (kernel `decode3l`).
- **Residual contexts.** Magnitude and sign contexts read `q − pred` of the visible cells; the predictor still reads the values. This needs a second GPU buffer.
  - **Kept.** Over the 19 fields at 32³ / 128² blocks, the geomean is res/no-res = 0.989 at 60 dB, 0.982 at 80 dB, 0.985 at 100 dB, 0.988 at max-abs 1e-3 and 0.984 at max-abs 1e-4.
- **Prior header.** A context's prior is sent only when it saves more than 12 bits over p = ½. On scale-PRES the fixed header dropped from about 1.2 KB (TCG1) to 0.5 KB.
- **Random access.**
  - Start from the box at level 0. At each level, map the needed sample range to coefficient ranges with a margin of ±3 per half-band per level. That is conservative for the 4-step lifting, where an output sample depends on s[i−1, i+2] and d[i−2, i+2].
  - Select the blocks that intersect those ranges, then add the parent blocks their contexts read, transitively.
  - Run the inverse lifting only on the lines inside the needed region at each level, then crop.
  - CPU only; not done on GPU.

### Gate (a), re-verified: MET

- `gpubench verify3`, lanes = 1: all 19 fields × 60/80/100 dB and max-abs 1e-3/1e-4 × blocks 16³/32³ (2D: 64²/128²). **190/190 passed**: GPU == CPU byte for byte, error bound re-checked, and the random-access box equal to the crop of the full decode.
- lanes = 8 at 32³ / 128²: **95/95 passed**.
- Every NYX 512³ speed run (134M cells) was also bit-identical.
- Unit tests:
  - 4 bounds × 4 block/lane configurations
  - 4 random-access boxes, including corner and single-cell boxes
  - 2D
  - the GPU test, which covers TCG1 and v3 with lanes 1 and 8

### Ratio: geometric-mean bpv over 19 fields

v3 with lanes = 1. Results are in `bench/out/gpu3_ratio.jsonl`.

| ratio | blocks | p60 | p80 | p100 | m1e-3 | m1e-4 |
|---|---|---|---|---|---|---|
| tc-gpu3 / tc-wavelet (serial) | 16³ / 64² | 1.197 (n=19) | 1.144 (n=19) | 1.141 (n=19) | 1.127 (n=19) | 1.137 (n=19) |
| tc-gpu3 / tc-wavelet (serial) | 32³ / 128² | 1.067 (n=19) | 1.107 (n=19) | 1.123 (n=19) | 1.084 (n=19) | 1.114 (n=19) |
| tc-gpu3 / tc-wavelet (serial) | one block per band | 1.047 (n=19) | 1.099 (n=19) | 1.118 (n=19) | 1.076 (n=19) | 1.108 (n=19) |
| tc-gpu3 / TCG1 whole-field | 16³ / 64² | 1.058 (n=19) | 0.993 (n=19) | 0.992 (n=19) | 1.007 (n=19) | 0.991 (n=19) |
| tc-gpu3 / TCG1 whole-field | 32³ / 128² | 0.944 (n=19) | 0.961 (n=19) | 0.976 (n=19) | 0.968 (n=19) | 0.972 (n=19) |
| tc-gpu3 / TCG1 whole-field | one block per band | 0.926 (n=19) | 0.954 (n=19) | 0.972 (n=19) | 0.961 (n=19) | 0.966 (n=19) |
| tc-gpu3 / SPERR | 16³ / 64² | 1.146 (n=19) | 1.020 (n=19) | 1.022 (n=19) | 0.848 (n=19) | 0.946 (n=19) |
| tc-gpu3 / SPERR | 32³ / 128² | 1.022 (n=19) | 0.987 (n=19) | 1.005 (n=19) | 0.816 (n=19) | 0.928 (n=19) |
| tc-gpu3 / SPERR | one block per band | 1.003 (n=19) | 0.979 (n=19) | 1.001 (n=19) | 0.810 (n=19) | 0.922 (n=19) |

With lanes = 8 at 32³ / 128² (`gpu3_l8_ratio.jsonl`):

| ratio | 60 dB | 80 dB | 100 dB | max-abs 1e-3 | max-abs 1e-4 |
|---|---|---|---|---|---|
| vs serial tc-wavelet | 1.339 | 1.182 | 1.157 | 1.174 | 1.160 |
| vs SPERR | 1.282 | 1.054 | 1.036 | 0.884 | 0.966 |

**Readout**

- Moving to code-blocks removed most of TCG1's bricking cost. At 80 dB, v3 32³ costs +10.7% vs serial, against +25% for TCG1 with 64³ bricks and +15% for whole-field TCG1.
- The block rule itself costs little at 32³: 0.7–2% vs one block per band. It costs about 4–14% at 16³.
- **Gate (c): FAILED.** At 80 dB, v3 is +10.7% vs serial at 32³ and +14.4% at 16³. Even with one block per band it is +9.9%.
- What remains is the coder, not the blocks: static probabilities against the serial coder's adaptive ones, and its richer causal neighbourhood. Fields like scale-PRES, at 0.06 bpv, still pay 1.6×.
- Against SPERR, v3 32³ is at parity on PSNR (0.99 at 80 dB, 1.01 at 100 dB) and wins at max-abs (0.82 at 1e-3, 0.93 at 1e-4).

### Speed: NYX 512³ (537 MB of f32)

| target | blocks, lanes | bpv | GPU kernels | GPU end-to-end | CPU 1 thread | CPU 16 threads |
|---|---|---|---|---|---|---|
| 80 dB | 16³, 1 | 0.0580 | 0.64 GB/s | 0.28 GB/s | 0.035 | 0.32 |
| 80 dB | 32³, 1 | 0.0501 | 0.29 | 0.15 | 0.033 | 0.24 |
| 100 dB | 32³, 1 | 0.909 | 0.17 | 0.08 | 0.031 | 0.27 |
| max-abs 1e-4 | 32³, 1 | 0.338 | 0.17 | 0.08 | 0.038 | 0.28 |
| 80 dB | 32³, 8 | 0.0586 | **1.08** | 0.77 | 0.061 | 0.53 |
| 100 dB | 32³, 8 | 0.937 | **1.23** | 0.64 | 0.047 | 0.37 |

- **Kernel split.** With lanes = 1, `decode3` dominates: 0.64 s at 16³, 1.6–3.1 s at 32³. It is bound by serial chains, one invocation per block, with only ~3.6k blocks at the finest level for 32³. With lanes = 8 it drops to 0.22–0.25 s.
- The whole-field `lift` costs 0.09–0.34 s. It uses one long private line buffer per invocation (MAXL = 512).
- **Gate (b)** is met only on kernels with lanes = 8 (1.08–1.23 GB/s), which costs +2.6–3.5% ratio on the two fields tested and +7% geomean at 80 dB. End-to-end is 0.64–0.77 GB/s. TCG1 remains the faster GPU format (1.3–7.3 GB/s on kernels).

### Random access: 64³ box at ⅓ of NYX 512³, CPU

| configuration | stream bytes | bytes read | blocks | latency, 1 thread / 16 threads |
|---|---|---|---|---|
| 80 dB, 16³ | 973,670 | 314,974 (32%) | 638 / 32,775 | 431 / 124 ms |
| 80 dB, 32³ | 840,832 | 511,990 (61%) | 323 / 4,110 | 1,019 / 247 ms |
| 100 dB, 32³ | 15,245,150 | 4,075,794 (27%) | 323 / 4,110 | 1,084 / 206 ms |
| max-abs 1e-4, 32³ | 5,670,128 | 1,825,780 (32%) | 323 / 4,110 | 1,085 / 277 ms |

For comparison, a whole-field decode takes 1.7–4 s on 16 threads.

- The box is 0.2% of the field, yet it reads 27–61% of the bytes. The coarse levels hold much of the low-rate stream, every coarse-level block is needed, and the dilated support at deep levels is wide.
- Latency is dominated by the field-sized zero buffers and the windowed lifting.
- TCG1's 32³ bricks remain far better for random access: a 64³ box took 22–41 ms.

### Commits

- `d2dfccd` TCG3: global transform, code-blocks, residual contexts, cost-based priors, support-aware random access, and GPU kernels
- `c78eb54` multi-lane code-blocks (`decode3l`), gpubench `verify3`/`speed3`, v3 sweep and summary (`run_gpu.py --v3`, `summarize_gpu3.py`)

### Remaining ideas

- **Ratio.** What is left is the coder:
  - adaptive models with compact per-block state (a few hundred models)
  - NW/NE prediction taps
  - learned integer-MLP probabilities
- **Speed.** A shared-memory line buffer in the lift kernel; for lanes = 1, more blocks at the finest level.
- **Random access.** A scratch buffer covering only the dilated window instead of field-sized zero buffers; GPU sub-box decode.

---

## 10. Learned integer-MLP probabilities on TCG3 code-blocks

The code is in `src/lmodel3.rs`, with the `learned_res` kernel in `gpuwav.wgsl`. The static-prior mode is unchanged and is used whenever the stream's model tag is 0.

### Design

**Features.** 28 integer Q8 inputs: the 18 raw context quantities that TCG3's dependency rule allows, plus a 10-way class one-hot.
- The signed log-magnitudes of W, WW, Σfaces, Σdiagonals, Σfar lines, the parent value, and the prediction.
- The log-magnitudes of the residual-buffer context: |W|, |WW|, Σ|faces|, Σ|diagonals|, Σ|far|, |parent|, and the parent's neighbours.
- The visible-line counts and a has-parent flag.

The remaining 9 inputs of M1b's 37-input layout are zero.

**Network.** M1b's MLP (ReLU, Q8 weights, Q16 biases), retrained on these features. It runs entirely in i32:
- weights are clamped to ±2047, hidden activations to [0, 2^14], outputs to ±2^14, and biases to ±2^24;
- with those clamps no partial sum can overflow i32.

**Output.** A 2-component discretized logistic mixture, with a CDF in Q16.
- σ and 2^{f/256} come from integer tables, built on the CPU with basic IEEE arithmetic only and uploaded to the GPU as data.
- The scale is applied as `⌊d·m/2^16⌉` followed by an arithmetic or saturating shift. This uses the existing 32×32→64 `mulsh` with no i64, and supports scales down to 2^-16.
- A first version clamped the scale at 2^-2. That version was 10% *worse* than static priors on Miranda, because it could not represent sharply peaked zero bins.

**Binarization** (as in M1b, without the adaptive probability map):
- zero flag, sign, an exp-Golomb bucket chosen by unary tail ratios, and the mantissa by integer bisection;
- every `P(1)` is an integer ratio of Q16 masses, `min(n,d)·65535/d` in u32;
- the decisions feed the same 32-bit rANS lanes.

**Models.**
- **Pretrained:** two sizes, 37→32→16→5 (1,829 parameters) and 37→16→5 (693 parameters). They were trained only on the 20 training windows of the original 5 fields (4 windows each, disjoint from the test crops), at 40–100 dB. That is 13.3M samples, taking 39 s and 20 s respectively. The stream carries an 8-byte fingerprint.
- **Overfit per file:** 37→16→5, trained at encode time; its weights (about 0.7 KB) are shipped and counted in the rate.

**GPU.** The MLP runs per coefficient inside the existing 8-lane pass schedule (`decode3l`; `decode3` for 1 lane). Weights and tables are read from the storage buffer. Workgroup memory was not needed for correctness and was not tried.

### Gate (a), byte-identical: MET

- All 19 fields × 60/80/100 dB and max-abs 1e-3/1e-4, with 32³ / 128² blocks, 8 lanes and the pretrained 32×16 model. **95/95 passed**: GPU == CPU byte for byte, error bounds re-checked, random-access crops equal.
- The NYX 512³ speed runs were identical too, 134M cells for each model.
- The unit tests add overfit models with 1 and 8 lanes, PSNR and max-abs.
- One false alarm along the way: `gpubench` had not been rebuilt after the WGSL change, so the GPU was decoding with a stale shader. After the rebuild every case matched.

### Ratio

Geomean bpv over 19 fields, 32³ / 128² blocks, 8 lanes (`bench/out/gpu3_learned_ratio.jsonl`). "orig" is the 5 fields whose training windows the pretrained models saw; "held" is the 14 held-out fields.

| model | vs | fields | p60 | p80 | p100 | m1e-3 | m1e-4 |
|---|---|---|---|---|---|---|---|
| overfit16 | serial | all | 1.338 | 1.117 | 1.113 | 1.131 | 1.102 |
| overfit16 | serial | orig | 1.182 | 1.042 | 1.069 | 1.055 | 1.049 |
| overfit16 | serial | held | 1.399 | 1.145 | 1.130 | 1.160 | 1.122 |
| overfit16 | static | all | 0.999 | 0.945 | 0.962 | 0.963 | 0.950 |
| overfit16 | static | orig | 0.964 | 0.918 | 0.956 | 0.935 | 0.936 |
| overfit16 | static | held | 1.012 | 0.954 | 0.964 | 0.973 | 0.955 |
| overfit16 | sperr | all | 1.281 | 0.996 | 0.997 | 0.852 | 0.918 |
| overfit16 | sperr | orig | 1.091 | 0.991 | 0.995 | 0.832 | 0.932 |
| overfit16 | sperr | held | 1.357 | 0.998 | 0.997 | 0.859 | 0.913 |
| pre16 | serial | all | 1.287 | 1.134 | 1.135 | 1.134 | 1.123 |
| pre16 | serial | orig | 1.140 | 1.054 | 1.077 | 1.055 | 1.059 |
| pre16 | serial | held | 1.345 | 1.164 | 1.156 | 1.164 | 1.146 |
| pre16 | static | all | 0.961 | 0.959 | 0.981 | 0.966 | 0.968 |
| pre16 | static | orig | 0.930 | 0.930 | 0.963 | 0.935 | 0.945 |
| pre16 | static | held | 0.973 | 0.970 | 0.987 | 0.977 | 0.976 |
| pre16 | sperr | all | 1.233 | 1.011 | 1.016 | 0.854 | 0.934 |
| pre16 | sperr | orig | 1.052 | 1.003 | 1.002 | 0.832 | 0.941 |
| pre16 | sperr | held | 1.305 | 1.014 | 1.021 | 0.862 | 0.932 |
| pre32x16 | serial | all | 1.276 | 1.118 | 1.123 | 1.122 | 1.110 |
| pre32x16 | serial | orig | 1.125 | 1.034 | 1.065 | 1.039 | 1.043 |
| pre32x16 | serial | held | 1.335 | 1.150 | 1.144 | 1.153 | 1.134 |
| pre32x16 | static | all | 0.953 | 0.946 | 0.970 | 0.955 | 0.956 |
| pre32x16 | static | orig | 0.918 | 0.912 | 0.952 | 0.922 | 0.931 |
| pre32x16 | static | held | 0.966 | 0.958 | 0.977 | 0.968 | 0.966 |
| pre32x16 | sperr | all | 1.222 | 0.997 | 1.005 | 0.845 | 0.923 |
| pre32x16 | sperr | orig | 1.039 | 0.984 | 0.991 | 0.820 | 0.927 |
| pre32x16 | sperr | held | 1.295 | 1.002 | 1.010 | 0.854 | 0.922 |

**Readout**
- The learned model beats TCG3's static priors by 4–5.5% on the held-out fields and by 7–9% on the original 5. All three models are close: pretrained 32×16 ≥ overfit 16 ≥ pretrained 16.
- vs SPERR: at parity on PSNR (0.997 at 80 dB for pretrained 32×16), and clearly better at max-abs (0.85 at 1e-3, 0.92 at 1e-4).
- **Gate (c), ≤5% vs serial tc-wavelet at 80 dB: FAILED.** The 80 dB geomeans are 1.117 (overfit 16), 1.118 (pretrained 32×16) and 1.134 (pretrained 16).
  - On the original 5 fields pretrained 32×16 is +3.4%, which is under 5%. Miranda density reaches 1.027 against serial's 1.064.
  - The held-out fields are at +15%, and the gate is defined on all 19.
  - The remaining gap sits in the 8-lane pass ordering (~+7% over 1 lane, §9) and on the new-application fields (SCALE at +15–20%, scale-PRES at 0.06 bpv).
  - We did not measure the learned model with 1-lane raster blocks across all 19 fields; on Miranda the static coder gains 2.6% from 1 lane.

### Speed: NYX 512³, 32³ blocks, 8 lanes

| model | target | bpv | GPU kernels | GPU end-to-end | CPU 1 thread | CPU 16 threads |
|---|---|---|---|---|---|---|
| static (§9) | 80 dB | 0.0586 | 1.08 GB/s | 0.77 GB/s | 0.061 | 0.53 |
| pretrained 32×16 | 80 dB | 0.0564 | **0.095** | 0.047 | 0.0091 | 0.096 |
| pretrained 32×16 | 100 dB | 0.8891 | 0.095 | 0.046 | 0.0087 | 0.101 |
| pretrained 16 | 80 dB | 0.0570 | **0.224** | 0.109 | 0.018 | 0.21 |
| pretrained 16 | 100 dB | 0.8988 | 0.199 | 0.097 | 0.016 | 0.17 |

Machine load was 9–13 during these runs.

- **Gate (b), ≥1 GB/s on GPU kernels with the learned model: FAILED**, by 5–11×.
- `decode3l` takes 2.4–6.0 s, against 0.22 s with static priors. Each coefficient costs an MLP pass (~530–1,850 MACs, weights read from storage), plus 2 to ~3·log₂|r| CDF evaluations, each with two emulated 64-bit multiplies and two table reads.
- Size/ratio/speed tradeoff: the 16-hidden net is 2.4× faster than 32×16 and costs +1.4% bpv at 80 dB. Neither reaches 1 GB/s.

**Untried speed ideas**
- Weights in workgroup memory (shared by all 8 lanes).
- Evaluating the net once per coefficient but deriving the mantissa decisions from a per-coefficient CDF table instead of re-evaluating per bisection step.
- An 8-bit SIMD dot product (WGSL `dot4I8Packed`).
- Running the net only for the zero/sign/bucket decisions and coding the mantissa raw.
- A larger GPU-side lane count.

The CPU decode is 0.1–0.2 GB/s on 16 threads.

### Commits

- `2253526` learned integer-MLP probabilities for TCG3 (32-bit CDF path, overfit/pretrained, WGSL `learned_res`), `pretrain3`
- `5972552` learned sweep summary, pretrained TCG3 models (`bench/models/pre3_32x16.tcm`, `pre3_16.tcm`), clippy

---

## 11. Speeding up the learned TCG3 GPU decoder

**Setup**
- **Bitstreams unchanged.** The streams were encoded once, before any kernel change, and saved (`gpubench encode3`) together with a hash of the CPU decode.
- **Check after every change:** decode them on the GPU (`gpubench gpu3`, `bench/gpu3_time.sh`) and compare against the stored hash.
  - The quick subset: Miranda 80 dB and max-abs 1e-4, scale-T 100 dB, CESM-TS 2D 80 dB, QMCPACK max-abs 1e-3, plus NYX 512³ at 80 dB with each model.
  - Every step matched on every stream.
- **Timings:** decode-kernel time, best of 2–3 runs. Load average was 6–14 during the per-idea steps and 4–5 for the final row.

| step | kept? | pretrained 32×16 kernels (GB/s) | pretrained 16 kernels (GB/s) |
|---|---|---|---|
| baseline (§10) | — | 0.099 | 0.222 |
| (1) MLP weights in workgroup memory | no: slower, the 8 KB per 8-thread group cuts occupancy | 0.080 | 0.206 |
| (2) carry CDF tails across bisection / unary steps (each value computed once) | yes, but no measurable effect: at 80 dB most coefficients are zero, so the MLP dominates | 0.096 | 0.220 |
| (3) `dot4I8Packed` | not applicable: naga 25 (our wgpu) has no `dot4I8Packed` (naga 30 does), and the weights (±2047) and activations (≤2^14) are not int8, so using it would change existing bitstreams | — | — |
| (4) straight-line MLP with the stream's weights as WGSL literals (pipeline compiled once per net and cached; no weight loads, no dynamically indexed arrays) | yes | 0.492 (5.0×) | 0.719 (3.2×) |
| (5) exact 32-bit form of the CDF scale multiply instead of the emulated 64-bit `mulsh` (`dh·m + dl + ⌊(dl·(m−2^16) + 2^15)/2^16⌋`, provably equal for \|d\| ≤ 2^25) | yes | 0.542 (+10%) | 0.821 (+14%) |
| (6) 32/lanes = 4 code-blocks per workgroup, filling the 32-wide SIMD group instead of 8 of 32 lanes; pass loop runs to the group's largest pass count | yes; also speeds up static streams | 0.841 (+55%) | 1.074 (+31%) |
| **final, quieter machine (load 4–5)** | | **0.85** (end-to-end 0.74) | **1.10** (end-to-end 0.91) |
| static TCG3 with the same kernels (was 1.08 / 0.77 in §9) | | | 1.90 (end-to-end 1.41) |

Final kernel split on NYX 512³ at 80 dB:

| kernel | 16-hidden | 32×16 | static |
|---|---|---|---|
| `decode3l` | 0.48 s | 0.60 s | 0.26 s |
| lift | 0.08 s | 0.08 s | 0.08 s |
| dequant + finalize | 0.013 s | 0.013 s | 0.013 s |

### Gates

- **Byte identity: MET.**
  - The full 95-case sweep after all changes (19 fields × 60/80/100 dB and max-abs 1e-3/1e-4, pretrained 32×16, 8 lanes, `gpu3_learned_verify2.jsonl`): 95/95. That covers GPU == CPU, the error bounds, and the random-access crops.
  - The static TCG3 8-lane sweep (`gpu3_l8_verify2.jsonl`), re-run because step 6 changed `decode3l`: 95/95.
  - Streams written before the changes decode to the same hash.
- **≥1 GB/s on GPU kernels with the learned model:**
  - **MET with the 16-hidden net:** 1.10 GB/s, and 0.91 GB/s end to end.
  - **Not met with 32×16:** 0.85 GB/s.
  - The 16-hidden net costs +1.4% bpv at 80 dB over 32×16 (§10).
- **Ratio gate (§10) is unchanged by these speedups.** The learned model is still 1.13× serial tc-wavelet at 80 dB with 16 hidden, 1.12× with 32×16.

### Commits

- `930adfe` literal-weight MLP specialization, tail caching, the gpubench `encode3`/`gpu3` modes
- `459b737` exact 32-bit CDF scale multiply
- `75b9892` 4 code-blocks per workgroup in `decode3l`
- `22304c6` clippy

---

## 12. Closing the learned TCG3 ratio gap (Track A)

Commit `dcff117` on branch `claude/tc-ratio-gap-9db550` of the tang repo, based on `22304c6`. Not pushed. New streams set a header flag (bits 9 and 10 of the lanes varint, plus one byte); streams written before this change decode unchanged, and the saved §11 streams still match their hashes on the GPU.

### Where the 12% went

On the 19 fields at 80 dB, using the `sweep3` CPU sweep with pretrained 32×16, 8 lanes and the old layout:

| measure | geomean vs serial |
|---|---|
| actual stream | 1.1185 |
| without the per-lane rANS framing (state flush, length varint, pad) | 1.075 |
| payload only (sum of −log₂ p of the coded decisions) | 1.061 |

- **Lane framing.** About 4.5 bytes per non-empty lane, on 400–600 lanes per test field. Half of those lanes sit in tiny coarse-level blocks (for example 2×4×4 bands) that still used 8 lanes.
- **scale-PRES** (2.10×) alone cost about 4% of the geomean. In its level-1 z-detail band, passes 2, 4 and 6 of the 8-lane parity lattice coded *more* nonzero residuals than nonzero values. The cause: the predictor's tap sums mixed lines in the same z row with lines in other z rows. The "far" tap, for example, summed (0,±2) together with (±2,0).

### Changes (one format flag)

1. **Compact lanes.**
   - A multi-lane block of v cells uses clamp(8·v·lanes/V, 1, lanes) lanes, where V is the full block volume. In 3D a lane gets at least 512 cells (256 in 2D). Full blocks keep 8 lanes.
   - The cap is set by latency. Levels decode one after another, so kernel time is about Σ over levels of the longest lane chain. A first rule, which gave 16³ blocks 1 lane, cost −14% GPU speed. The 1/8 cap costs about −3%.
   - The last lane's length is implied by the block size. The other lane lengths are zig-zag deltas from the previous lane.
2. **Axis-grouped taps.** The three line taps sum the visible lines on the z axis, the y axis and the diagonals, instead of face, diagonal and far lines. This applies to the predictor and to the net's three line-value inputs. NTAP, NFL and MLP cost are unchanged. In WGSL the grouping is branch-free: each visible line stores a precomputed group.
3. **Compact header.** Predictor presence is a bitset instead of one byte per key. Taps are stored in Q8 units instead of Q12. Q16 and Q64 units were worse.
4. **Retrained nets** on the same 20 training windows of the original 5 fields, 40–100 dB, 16 epochs. No new data, because disk was below the floor.

### Ladder: 16-hidden net, 80 dB, all 19 fields

| step | geomean vs serial |
|---|---|
| §10 baseline (pre3_16, 8 lanes) | 1.134 |
| + compact lanes (cap v·lanes/V, old net) | 1.113 |
| + axis taps, retrained (8 epochs) | 1.074 |
| latency cap 1/8 for GPU speed | 1.082 |
| 16 epochs | 1.079 |
| delta lane lengths | 1.079 |
| Q8 taps, retrained | **1.076** |
| static priors with the new layout (§9 8-lane static: 1.182) | 1.129 |

### Frontier: 8 lanes, 32³ / 128² blocks

Geomean bpv vs serial tc-wavelet. GPU numbers are for NYX 512³ at 80 dB, decode kernels best of 5, on a quiet machine (load 4–5). In the same run the §11 pre3_16 stream measured 1.10 GB/s.

| net | params | 80 dB all | 80 dB held-14 | 60 dB | 100 dB | max-abs 1e-3 | max-abs 1e-4 | NYX bpv | kernels GB/s | end-to-end GB/s |
|---|---|---|---|---|---|---|---|---|---|---|
| §10 pre3_16 (old layout) | 693 | 1.134 | 1.164 | 1.287 | 1.135 | 1.134 | 1.123 | 0.0570 | 1.10 | 0.92 |
| §10 pre3_32x16 (old layout) | 1829 | 1.119 | 1.150 | 1.276 | 1.123 | 1.122 | 1.110 | 0.0564 | 0.85 | 0.74 |
| **pre3v4_16** | 693 | **1.076** | 1.104 | 1.185 | 1.093 | 1.077 | 1.078 | 0.0572 | **1.065** | 0.89 |
| pre3v4_16x8 | 789 | 1.076 | 1.104 | 1.187 | 1.090 | 1.078 | 1.078 | 0.0570 | 1.04 | 0.88 |
| pre3v4_24x8 | 1157 | 1.069 | 1.098 | 1.180 | 1.087 | 1.073 | 1.073 | 0.0567 | 0.96 | 0.82 |
| pre3v4_16x16 | 965 | 1.066 | 1.095 | 1.177 | 1.086 | 1.069 | 1.070 | 0.0567 | 0.99 | 0.85 |
| pre3v4_32x16 | 1829 | 1.058 | 1.088 | 1.171 | 1.080 | 1.066 | 1.063 | 0.0564 | 0.83 | 0.73 |

- Under load 17–50, pre3v4_16 measured 0.97–1.03 GB/s, against 1.03–1.08 for the old pre3_16.
- Worst fields with pre3v4_16 at 80 dB: scale-PRES 1.82 (it was 2.13), qmcpack-orb0 1.15, scale-T 1.12, cesm-TS 1.11, qmcpack-orb100 1.09.
- Original 5 fields: 1.001 (16 hidden), 0.979 (32×16).

### Gates

- **Ratio, geomean gap < 8% at 80 dB on all 19 fields: MET.** pre3v4_16 is 1.076, pre3v4_16x16 1.066 and pre3v4_32x16 1.058. On the 14 held-out fields: 1.104, 1.095 and 1.088.
- **≥ 1 GB/s on GPU kernels, NYX 512³ at 80 dB: MET with pre3v4_16** (1.065 GB/s) and pre3v4_16x8 (1.04). The larger nets miss: 16×16 at 0.99, 24×8 at 0.96, 32×16 at 0.83.
- **Byte identity: MET.**
  - Unit tests cover new and old layouts, static and overfit, 1 and 8 lanes.
  - `gpubench verify3` with pre3v4_16 over 19 fields × {60, 80, 100 dB, max-abs 1e-3, 1e-4}: 95/95 for GPU == CPU, error bound and random-access crop.
  - Every NYX 512³ stream was identical on the GPU.
  - The saved §11 streams still decode to their hashes.

### Tried and rejected

- 16 lanes per block: 1.102 at 16 hidden (+2.6%).
- A lane cap of 256 cells in 3D: 1.081.
- Axis-grouped *magnitude* inputs for the net: 1.085.
- Predictor acceptance thresholds of 0, 16, 128 or 256 bits instead of 64: 1.080–1.085.
- 300k samples per field per PSNR: 1.083 vs 1.079.
- No predictors (`TC_NOPRED`): 1.23.

### Not done / next

- **More pretraining data.** Disk was below the 15 GB floor (10–14 GB free), so the SDRBench download was skipped.
- **Mixture / CDF changes.**
- **NW/NE taps.** The axis grouping takes their place for now.
- **What remains** is about 3% of rANS lane framing (about 4.9 bytes per lane of 32-bit state flush) and the pass lattice on scale-PRES-like fields.
- **Reproduce:** `sweep3 <cram>/bench --l3 8 --targets p80 --model3 pretrained:bench/models/pre3v4_16.tcm`

## 13. Per-lane adaptive probability maps on the learned TCG3 coder (Phase 2, Track A)

Commit `587cf4f` on branch `claude/tc2-ratio-gap-c8de52` of the tang repo. Not pushed.

### What it does

The gap to the serial coder came mostly from static vs adaptive probabilities. Each rANS lane now keeps a small adaptive probability map (APM, also called secondary estimation) on top of the MLP output.

- **State per lane:** 8 decision contexts × 17 bins.
  - The contexts are: zero flag; sign; exp-Golomb bucket step for k = 0, 1, 2 and ≥ 3; first mantissa bisection bit; remaining mantissa bits.
  - The bins are spaced 1.0 apart in the logistic domain and cover ±8.
  - That is 136 words of state per lane, reset at the start of every block.
- **Coding a decision:**
  - The MLP probability p goes through `stretch` (a 4096-entry integer table, the inverse of the existing σ table).
  - The two neighbouring bins are interpolated.
  - The coded probability is (10·APM + 6·p) / 16.
- **Update:** only the nearer bin moves, by `d >> ⌊log₂(n+2)⌋`, where n is the bin's count, capped at 14. This is a count-based rate with no divide.
- **Byte identity:** all of it is integer, so CPU and GPU stay byte-identical. In WGSL the tables live in workgroup memory, and the 1-lane `decode3` kernel now uses workgroups of 32.
- **Format:** bit 11 of the lanes varint is a flag, followed by a packed varint (wa | lim<<5 | mask<<13). Older streams decode unchanged. `TC_AD=wa,lim,mask` overrides the setting for sweeps.

### Ratio: CPU `sweep3`, 8 lanes, geomean vs serial tc-wavelet

| net | adapt | 80 dB all-19 | 80 dB held-14 | 60 dB | 100 dB | max-abs 1e-3 | max-abs 1e-4 |
|---|---|---|---|---|---|---|---|
| pre3v4_16 | none | 1.076 | 1.104 | 1.185 | 1.093 | 1.077 | 1.078 |
| **pre3v4_16** | **10,14,15** | **1.043** | **1.065** | 1.154 | 1.050 | 1.050 | 1.044 |
| pre3v4_16x16 | none | 1.066 | 1.095 | 1.177 | 1.086 | 1.069 | 1.070 |
| pre3v4_16x16 | 10,14,15 | 1.035 | 1.059 | 1.149 | 1.045 | 1.045 | 1.038 |
| pre3v4_32x16 | none | 1.058 | 1.088 | 1.171 | 1.080 | 1.066 | 1.063 |
| pre3v4_32x16 | 10,14,15 | 1.028 | 1.053 | 1.144 | 1.038 | 1.040 | 1.031 |

### Ablation: pre3v4_16, 80 dB, all 19 fields

Settings are (weight in sixteenths, count cap, decision mask). The weights and caps tested were all within 0.2% of each other. Using every decision type matters.

| setting | rate rule | all-19 geomean |
|---|---|---|
| 10,14,15 | power-of-two | **1.0426** |
| 10,30,15 | power-of-two | 1.0434 |
| 10,20,15 | divide-based | 1.0430 |
| 16,60,15 | divide-based | 1.0556 |
| 8,30,7 (no mantissa) | divide-based | 1.051 |
| 8,60,5 (zero flag + bucket) | divide-based | 1.056 |
| 8,60,1 (zero flag only) | divide-based | 1.063 |

### GPU: NYX 512³ at 80 dB, pre3v4_16, 8 lanes

- **Byte identity:** every stream decoded on the GPU is identical to the CPU decode.
- **bpv:** 0.05223 without adaptation, 0.04993 with 10,14,15 (−4.4%), 0.05010 with 10,14,7.
- **Speed:** the GPU was never quiet during these runs. The image track's MPS job and then the vcad app held it at 78–99%, so absolute numbers sit around 0.2 GB/s for both streams. The relative cost, interleaved and best of 5–7 over 3 runs:

| stream | kernels relative to no adaptation |
|---|---|
| 10,14,15 | 0.92 |
| 10,14,7 | 0.93 |
| first version (private-memory table, divide-based rate) | 0.86–0.90 |

- **Projection:** applied to the quiet-machine 1.065 GB/s from §12, the default layer gives about **0.98 GB/s**. This is an estimate, not a measurement.

### Gates

- **Ratio ≤ 1.05 at 80 dB on all 19 fields: MET.** 1.043 with pre3v4_16, 1.035 with pre3v4_16x16. Held-out 14: 1.065 and 1.059.
- **≥ 1 GB/s on GPU kernels: NOT verified.** The projection is about 0.98 GB/s (−8%). It needs re-timing on a quiet GPU with `gpubench gpu3 bench/out/trackA2c/*.tcg3`.

### Frontier (projected kernel speed)

| config | 80 dB all-19 | kernels |
|---|---|---|
| pre3v4_16, no adaptation | 1.076 | 1.065 measured (§12) |
| pre3v4_16, 10,14,3 (zero flag + sign) | about 1.06 | about 1.02 projected (×0.955) |
| pre3v4_16, 10,14,7 | about 1.050 | about 0.99 projected |
| pre3v4_16, 10,14,15 | 1.043 | about 0.98 projected |

### Next

- Re-time on an idle GPU.
- If the default misses 1 GB/s:
  - Put the stretch table in workgroup memory. A packed i16 table on a p>>6 grid would be 2 KB.
  - Drop the APM on mantissa bits after the first one.
