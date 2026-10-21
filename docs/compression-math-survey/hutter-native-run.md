# Hutter M0: native x86 reproduction and time profile

Machine: mew (i9-12900K, Linux 6.8.0-146, 62 GB). Everything lives in `~/hutter` on mew;
`~/hutter/NOTES.md` has the exact commands.

## Build
- Source: fx2-cmix-transformer-v1 @ `83f2603f`, cloned fresh. **No patches.** `mmap_to_disk`
  left at the shipped `true` (the record's config; PPMD state goes to a sparse 14.7 GB `ppm.temp`).
- Image: `ubuntu:20.04` + clang-17/llvm-17 from apt.llvm.org (`llvm.sh 17`), upx-ucl, make,
  binutils, time (`~/hutter/docker/Dockerfile`).
- `build_and_construct_comp.sh` ran unmodified in 2m55s. PGO, the AVX2-only check and the
  no-reciprocal-estimate check all passed.
- Native vs Rosetta, dictionary step (411,996 → 100,096 B): **41.84 s native vs 18m22s under
  Rosetta, ~26× faster.** Same output size.

## Sizes
| Item | Bytes |
|---|---|
| comp9 = `run/cmix` (self-contained compressor) | 3,426,642 |
| comp9 zipped (`zip -9`, only matters if the rules' /zip form is used) | 3,423,410 |
| inside it: `cmix_orig` (UPX) / dict / article order / weights / header | 194,836 / 100,096 / 201,042 / 2,930,652 / 16 |
| archive9 | pending (run in progress) |
| Implied archive9 if the record's total holds: 100,424,672 − 3,426,642 | 96,998,030 (inferred, not measured) |

enwik9: 1,000,000,000 B, md5 `e206c3450ac99950df65bf70ef61a12d`, sha256 `159b8535…3744bc`.
I found no checksum on hrules.htm or textdata.html to check it against.

## Run status
- Container `hutter-compress` (id `1506708f2338`), `--cpuset-cpus=6` (P-core 3, 5.2 GHz max),
  `/usr/bin/time -v ./cmix -e enwik9 enwik9.comp` → `~/hutter/logs/compress.log`.
- Started 2026-10-18T13:39:31Z. About 9 min of setup (decoding the dictionary and article
  order) came before the main pass.
- The binary's own estimate at 1.51% done: elapsed 0:30:12, **remaining 32:49:39**. That puts
  the main pass at about 33.3 h, finishing around 2026-10-19 23:00Z. It is under the 50 h limit
  on this CPU. Note that the 12900K P-core is faster than the prize's reference machines.
- RSS at 34 min: 6.03 GB (peak so far 6.04 GB). The final peak will come from `time -v`.
- `~/hutter/after_compress.sh` is running under nohup. When compression exits it writes
  `logs/result.txt` with the archive9 size and S = len(comp9) + len(archive9) vs the record.
  Then it decompresses `archive9` in `decomp/` on the same core, timed
  (`logs/decompress.log`), and runs `cmp` against enwik9.

## Profile
How it was measured: `perf record -F 99 -g -p <pid>` for 600 s each time, from a privileged
ubuntu:24.04 sidecar with the host-matching perf, pinned to E-core 20, so it took no time from
the compressor's core. The shipped binary is UPX-packed and stripped. I rebuilt a twin without
`-s`, reusing the same PGO profile. Its `.text` is **byte-identical** (`cmp`) to the unpacked
shipped binary, so leaf IPs map exactly onto its symbols. The numbers below are self time at
the leaf.

| Bucket | t=0:10–0:20 (59,360 samples) | t=0:24–0:34 (59,351) |
|---|---|---|
| Transformer inference (`fx2::opt::*`: qgemv kernels, KDA linear-attention layers, attention, softmax/norms) | **61.1%** | **59.4%** |
| `Predictor::Predict` self (LTO-inlined bit models plus the `mixer_0_`/`mixer_1_` dot products and SSE) | 13.0% | 13.5% |
| Context models (`fxcmv1` ContextMaps, E/E1 tables, match; `FXCM::Perceive`) | 12.4% | 12.7% |
| Mixers (cmix `Mixer::Perceive/GetContextData`, `fxcmv1::Mixer1`) | 8.2% | 8.7% |
| PPMD | 2.3% | 2.3% |
| Other cmix (ByteModel, contexts, coder, …) | 1.5% | 1.4% |
| libc/libm/kernel | 1.5% | 2.0% |

The arithmetic coder does not show up as its own symbol; it is negligible or inlined.
The top transformer kernels at t=0:24–0:34 were `qgemv_relu2q` 12.4%, `qgemv_f32` 10.7%,
`kda_sweep_head` 7.6%, `pv_dense_i8` 6.6%, `kda_layer_step` 5.2%, `sparse4_accum` 3.8%,
`attn_step_fixed` 3.5% and `qgemv_add` 3.2%.

Caveats:
- Both windows fall in the first ~1.5% of the input.
- The attention and KV cost depends on article length, which varies.
- The 13% `Predictor::Predict` bucket can't be split further without debug info.
- A mid-run profile (~16 h in) was not taken. Run `~/hutter/profile.sh mid 600` and then
  `perf/sym.py` / `perf/cat.py` to get one.

## What this means for lever E
The transformer takes about 60% of single-core time, so the M4 condition is met ("only if
M0's profile shows the transformer dominates"). Lever E is worth doing.
- Upper bound: if transformer inference cost nothing, the run would take about 40% of 33 h,
  roughly 13 h. A 2× kernel speedup brings the total to about 0.70 × 33 h ≈ 23 h.
- The 50 h limit is set on the prize machines, not on mew. To know how much compute a bigger
  model can actually use, measure the same binary's speed on a Tiger Lake i7-1165G7 or a Zen 2
  chip. Without that, the 33 h here overstates the headroom.
- The ~40% spent outside the transformer stays fixed. So a model that costs k× today's
  transformer fits on this machine if 0.4 + 0.6k ≤ 50/33.3, i.e. k ≲ 1.8. On a slower machine
  the headroom shrinks in proportion.
- The hot kernels are int8/u8 GEMV (`qgemv_*`, `sparse4_accum`) and the KDA recurrent sweep.
  These are batch-1, memory-bound shapes under an AVX2-only, bit-exact rule (no VNNI, no
  AVX-512). The likely wins are in blocking and weight layout, not in new instructions.
