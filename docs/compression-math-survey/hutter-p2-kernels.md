# Hutter P2: faster transformer kernels (status 2026-10-19 ~18:30Z)

**Gate status: NOT MET.** The best kernel (v4) is bit-identical and runs **1.21× faster in the
standalone harness** (paired median, 5 × 200k tokens). The gate is 1.5×.

End to end, on a 50.5 MB proxy slice, the full cmix archive is **byte-identical** (md5
37071957…), the new archive decompresses correctly, and compression is **1.056× faster**
(7827 s → 7410 s). That is one run per binary, not interleaved.

Absolute times are inflated and noisy: the record compressor on core 6 and the M2 jobs share
memory bandwidth with these runs. Ratios from the interleaved A/B runs are what count.

Everything lives in `~/hutter/p2` on mew:
- `ref/` is a pristine worktree of fx2-cmix-transformer-v1 @ 83f2603.
- `fx2/` is the working tree.
- `harness/` holds the harness, the scripts and the dumps.
- Patch: **`~/hutter/p2/p2-kernels.patch`** (`git diff` of `fx2/` vs 83f2603).

Build: docker `hutter-fx2:20.04` (clang-17) with the record's exact transformer flags:
`-O3 -march=x86-64-v3 -mtune=generic -mrecip=none -fno-math-errno`, no fast-math.
Builds ran on cores 12–13 and benchmarks on core 10 (its SMT sibling 11 left idle).

## 1. Per-token call graph (cpp_infer/src/opt/model_opt.cpp)
The record's predictor runs `TransformerOpt(..., AttnKind::KVI8)`, with one `step()` per token.

1. **Prior:** quant + index, then sparse int4-column spmv (192×205).
2. **Embed:** combine the token embedding with the prior.
3. **Each of 12 layers:**
   - skip/residual ops.
   - **KDA layers (9):**
     - rmsnorm + quant ×5.
     - q/k/v projections, 3× qgemv 192×192.
     - Forget gate: up (64×192, quant-bias epilogue) → down (192×64).
     - Output gate: up → down, same shapes.
     - beta: 3×192 fp32 dot.
     - `kda_layer_step`: conv4+SiLU, decay (softplus/exp), l2norm, then 3 head sweeps over a 64×64 fp32 state (48 KB per layer).
     - Gated rmsnorm, then out-proj via qgemv_add.
   - **Softmax layers (3, 7, 11):**
     - rmsnorm + quant ×3.
     - q/k/v 192×192.
     - q/k rmsnorm, rope, int8 quant.
     - KV insert, then attention: int8 K·q, softmax, PV with int8 V, window 1024.
     - out-proj.
   - **MLP (all layers):** rmsnorm + quant, then mlp.up 768×192 with a fused relu² quant + nonzero index, then mlp.down as sparse int4 columns.
4. **Head:** final norm, unembed 205×192, softcap + softmax.

Budget per token:
- Dense MACs ≈ 4.0 M: KDA projections 1.77 M, softmax projections 0.44 M, mlp.up 1.77 M, unembed 0.04 M.
- Dense weight bytes: 4.21 MB unpacked int8 in the record, 2.0 MB in the packed layout.
- Sparse mlp.down: about 20% of 12 × 98 KB columns.
- KDA state: 432 KB read + written.
- KV ring: about 1 MB read at n≈860.

## 2. Harness (`harness/harness.cpp`, `build.sh`, `ab.py`)
- Input: the first 200,000 tokens of the proxy stream, `~/hutter/m1/data/proxy-bytes.uint8`, plus their PPMD priors from `proxy-probs.float16` (82 MB copy).
- Articles are split at `proxy-bounds.int32`: 199,719 steps over 281 articles.
- Uses the shipped `models/6m-q4-fp32.tfwc2`.
- Dumps every step's 205 output probabilities **and** 205 logits as raw fp32 bits (328 MB), and prints an FNV hash.
- Reference hash `8d21ba455abb509d`. RSS 172 MB.
- `ab.py` alternates binaries for R rounds on core 10 and reports medians and the paired-median ratio.

**Noise warning:** mew was heavily loaded the whole time: three cmix runs, a 26 GB eval, swap full, load 6–10. The same binary drifted 97–166 µs/token between sessions. The usable numbers are A/B ratios from interleaved runs, not absolute times.

## 3. Baseline profile (rdtsc ticks/token, TSC ≈3.19 GHz, `-DFX2_PROF`, 200k tokens, ref)
| site | ticks/token | % |
|---|---|---|
| mlp.up + relu²q (12) | 113,112 | 25.6 |
| KDA q/k/v proj (27) | 60,259 | 13.7 |
| KDA fused step (9) | 81,996 | 18.6 |
| attention qk+softmax+pv (3) | 48,737 | 11.1 |
| mlp.down sparse (12) | 42,451 | 9.6 |
| KDA out proj (9) | 22,141 | 5.0 |
| softmax-layer q/k/v (9) | 20,285 | 4.6 |
| gates up/down (36) | 26,094 | 5.9 |
| other | ~26,000 | 6 |
| **total** | **441,023** (≈138 µs) | |

Microbenchmark on the same core (`mb.cpp`, `mb3.cpp`):
- The record's unpacked int8 kernel does **157 MAC/ns hot, but 58 GB/s ≈ 76 MAC/ns when it has to stream** 768×192 arenas.
- On this Alder Lake P-core, at this load, the dense GEMVs are **L3-bandwidth-bound**.

## 4. Optimizations
| # | change | result | bit-exact |
|---|---|---|---|
| 1 | **p4: packed int4 dense weights** for every dense GEMV (d_in 64/192/768). Layout: 4-row groups, nibbles stored as w+7. The kernel turns the biased-u8 activation into signed int8 once per call (xor 0x80) and subtracts the exact `7·Σqa`. The int32 dots are identical, so all fp32 epilogues are unchanged. Dense weight stream drops 4.2 → 2.0 MB per token. | profile: matmul group 294K → 177K ticks; total 441K → 310K. **A/B: 1.274× (median 130.9 vs 102.8 µs, 7 rounds × 60k tokens; paired median 1.278×)**. Other sessions: 1.185–1.226× (3 rounds × 40k) | **0 differing bits** over 199,719 × 410 floats (`cmp` of dumps, same hash) |
| 2 | KV cache f32-V instead of int8-V (outputs identical by design) | 7–8% slower | identical (`cmp`) |
| 3 | KDA prefetch modes 0/4/6 vs the default 5 | within noise | — |
| 4 | burst-prefetch the KDA state at layer entry (T0/T1) | −2…−3% | — |
| 5 | NTA hint / prefetch distance 1–4 KB on the p4 stream | NTA −13…−80%; distance neutral | — |
| 6 | prefetch the mlp.down columns from the mlp.up epilogue (rows with y>0) | down spmv 31.8K → 15.5K ticks, but the scalar epilogue cost +29K: −5% net | identical (`cmp`) |
| 7 | side-stream prefetch of the whole mlp.down arena during mlp.up | −9…−12% (it competes for bandwidth) | identical (`cmp`) |

The patch keeps only #1 active. #6 and #7 are opt-in (`-DFX2_DOWNPF`); #4 is opt-in (`-DFX2_EXP_KSTPF`).

### Where the remaining time is (p4, ticks/token)
| site | ticks/token |
|---|---|
| dense matmuls | ≈145K |
| KDA step | ≈80K (conv 25K, gates 12K, sweep 43K, norm 3K) |
| attention | ≈41K |
| mlp.down | ≈32K |
| other | ≈13K |

Two measurements explain what is left:
- **The p4 kernel is now compute-bound.** It runs at 100 MAC/ns, both hot and streaming.
- **The KDA step has a hot-state floor of ≈60K ticks** (measured in `mbk.cpp`). Its sweep has an irreducible 4 FP ops per state element (mul, fma, fma, fma; the order is fixed for exactness).

## 5. v4 vertical int4 kernel (`qmat_v4.inc`, `-DFX2_V4`, now enabled in fx2/makefile)
- **Layout:** a 16-bit lane is (row, half), and nibbles cover 8 columns per 32 B vector. The kernel uses `maddubs(nibbles, bcast32(act[4]))` and folds the halves of each row with `madd(ones)`.
- **Why it should be faster:** 8 consecutive rows land directly in one int32 vector, with no `hadd` reductions. That cuts about 12% of uops versus p4 and removes the p5-heavy hadds. Expected 100 → ~120 MAC/ns, roughly −25K ticks.
- **Bit-exactness:** the dump `cmp` against ref.dump shows 0 differing bits (same hash 8d21ba455abb509d).
- **Interleaved A/B**, core 12, 5 rounds × 200k tokens:

  | binary | median µs/token | paired-median speedup vs ref |
  |---|---|---|
  | ref | 107.4 | 1.000× |
  | p4 | 92.0 | 1.157× |
  | v4 | 88.6 | **1.211×** |

  An earlier 7 × 60k session gave ref 118.5, p4 1.078×, v4 1.148×.
- **The packing gain shrinks as the machine gets quieter.** It is mostly a bandwidth win. Under the heavy load of 2026-10-18, p4 measured 1.27×.
- **In-model, the v4 kernel is not compute-faster than p4.** The mlp.up site stays at ≈64K ticks/token; the gain shows up elsewhere in the matmul group (161K vs ≈177K ticks/token for the whole group).

## 6. End-to-end (full cmix, PGO build, transformer v4)
**Build.** `make prof_use` in `ref/` and `fx2/`, reusing the record's pgo_data. Both trees carry
the same `m1-runner.patch` (FX2_FORCE_VOCAB).

**Why the slice is 50.5 MB and not 10 MB.** `-c` mode writes the input length as 4 bytes into the
stream, and all 4 bytes must be in the 205-byte vocabulary. Any length below 2^24 has a top byte of
0x00, which is not in the vocabulary. The smallest valid cut at an article separator is 50,536,647 B.

Runs, one cmix at a time, docker `--cpuset-cpus=10`, RSS 6.5 GB:

| run | wall | result |
|---|---|---|
| ref compress | 7827 s | 5,710,529 B |
| new compress | 7410 s | 5,710,529 B, **cmp identical** (md5 37071957748adc59a4e86b6d6c4e1fef) |
| new decompress | 7108 s | **cmp == slice.raw** |

**Speedup.** End to end it is 1.056× (single run, not interleaved). The transformer is ≈60% of
runtime, so the harness's 1.21× projects to 1/(0.4 + 0.6/1.21) = **1.12×**. The measured figure is
about half of that. Two possible reasons, neither verified:
- load drift between the two sequential runs;
- in-situ cache behaviour that differs from the harness.

Interleaved repeats would settle it; each costs about 2 h per binary. Applied to the record's
≈33.5 h enwik9 run, 1.056× would save ≈1.8 h, and 1.12× would save ≈3.6 h.

Artifacts: `~/hutter/p2/e2e/` (run.sh, times.txt, logs, archives). Patch:
`~/hutter/p2/p2-kernels.patch`, 581 lines, which includes the makefile `-DFX2_V4`.

## 7. Not done / next
- **Step 3, interleaving the KDA sweep across heads:** not attempted. A hot microbenchmark puts
  the ceiling at roughly 43K → 28K ticks/token on the sweep, i.e. ≤5% of the transformer.

- (End to end is done; see section 6.) The integration is drop-in: the top-level makefile already compiles `cpp_infer/src/opt/*.cpp`, and only those files change.
  - In-situ, cmix's GB-scale tables evict the transformer's working set from L3. A 2.2 MB smaller weight stream should help more there than standalone, but that is **unmeasured**.
  - A crude pollution emulation (random line touches over 512 MB between tokens) gave 1.19–1.23×. That run was too noisy to conclude from.
- **Zen 2 risk:** Zen 2 has a single maddubs pipe, and the original authors measured packed int4 *slower* there (front-end bound). p4/v4 must be timed on a Zen 2 or Zen 3 box before adoption. Picking the kernel per CPU at runtime would be safe: the int32 results are identical either way.
- Further ideas:
  - Interleave pass 1 of head h+1 with pass 2 of head h in the KDA sweep, using column-half splits. This keeps the per-element op order but is register-tight.
  - Restage the KDA conv/exp passes.
  - Batch the attention K/V reads across heads.
  - Fuse the q/k/v projections into one 576-row pass to amortize the activation prep.
  - None of these alone obviously closes the gap to 1.5× at fixed numerics on this box.
