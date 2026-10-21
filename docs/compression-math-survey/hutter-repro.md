# Hutter Prize record reproduction: fx2-cmix-transformer (status 2026-10-18)

## Source
- Official page http://prize.hutter1.net/ (TLS cert expired; fetched with `curl -k`) names Vladimir Ivanov the eleventh winner. Its link goes to https://github.com/astOwOlfo/fx2-cmix-transformer-v1
- Cloned to `/Users/cam/Developer/hutter/src`, commit `83f2603f3f7751da5f429fd32669807eb8510494` (2026-09-01, astOwOlfo).
- Total 100,424,672 B is the published figure (prize page/news). I have not reproduced it.

## Hardware
Apple Silicon arm64, 16 cores, 48 GiB RAM, macOS (Darwin 27). Free disk: **28 GiB (97% full)**, below the 30 GB stop threshold.

## Rules: what counts
hrules.htm (fetched with `curl -k`) says: "Total size is measured as S := length(comp9.exe/zip)+length(archive9.exe)." So **the compressor counts as well as the self-extracting archive**. The weights are in both, which makes the double count real.

## Build (Rosetta, amd64 container) — partial
- Data: enwik8 = 100,000,000 B and enwik9 = 1,000,000,000 B are in `/Users/cam/Developer/hutter/data` (sizes verified; zips deleted).
- Image `hutter-build` (`/Users/cam/Developer/hutter/docker/Dockerfile`): ubuntu:24.04 amd64 with clang-17, llvm-17, upx-ucl, make, binutils. I used distro packages instead of apt.llvm.org; the compiler is the same major version. No source patches. I left `mmap_to_disk = true` as shipped.
- `./build_and_construct_comp.sh` ran unmodified: the PGO build, the AVX2-only guard ("AVX2-only check passed") and the no-reciprocal-estimate guard all passed, and `upx-ucl` packed the binary. **AVX2 code runs under Rosetta**, even though /proc/cpuinfo does not list avx2.
- The script's self-extractor stage compresses `dictionary/english.dic` with the full model: 411,996 B → 100,096 B in **18 min 22 s ≈ 374 B/s**. I stopped it (coordinator: the reproduction moves to a native x86 box) while it was compressing `new_article_order`, at 15%. `run/cmix` was **not** assembled, and enwik8 and enwik9 were not run.
- Rosetta ETA (estimate, extrapolated from the dictionary rate; enwik9's preprocessed stream length differs from 10^9): 10^9 B / 374 B/s ≈ 2.7 Ms ≈ **~740 h (~31 days)**. That is far over the 72 h budget, so no long run was launched locally.
- I did not check whether the binary is enwik9-only. The README calls `run/cmix` "the enwik9-specialized compressor".

## Weight blob (measured)
How it is stored: `models/6m-q4-fp32.tfwc2` (format FX2TFWC2, from `pysrc/weights_compress.py`) is appended unchanged to the compressor binary. The build script does `cat cmix_orig comp_dict comp_order comp_tfweights header.dat > cmix`. The writeup says it is in both binaries ("2.9MB each binary (or 5.9MB combined)"). I decoded it with the repo's own Python decoder (torch installed, GPU-only `fla` import stubbed) to `weights.bin`, which is 39,615,533 B.

| quantity | value |
|---|---|
| quantized weights | 5,868,864, int levels −7..7 (15 levels), one bf16 scale per input channel |
| other params | 26,083 bf16 (scales etc.) + 28,313 f32 (convs/norms) + 23 int32 |
| rope sin/cos tables | 8,388,608 f32, **not stored**; recomputed bit-exactly at load |
| raw export (weights.bin) | 39,615,533 B |
| plain 4-bit packing of quantized weights | 2,934,432 B |
| uniform log2(15) coding (v1 format) | 2,866,126 B |
| **stored blob (.tfwc2)** | **2,930,652 B** (adaptive bit-tree range coder over everything) |
| stored × 2 binaries | 5,861,304 B = **5.84% of 100,424,672** (one copy = 2.92%) |
| order-0 entropy, global | 3.7860 b/w → 2,777,440 B |
| order-0 entropy, per tensor (Σ H·n/8) | 3.7810 b/w → 2,773,771 B |
| order-1 entropy (previous weight in the row, per tensor) | 2,771,015 B |
| xz -9e / zstd -22 on the .tfwc2 | 2,930,868 / 2,930,734 (no gain) |
| xz -9e / zstd -22 on the quantized weights alone (1 B each) | 2,942,048 / 2,811,351 |

Estimate (not measured separately): the non-quantized float params are about 165 KB raw and code to roughly 150 KB inside the blob. The quantized weights code to about 2.78 MB, which is close to their order-0 entropy.

## Runs
None locally. The native x86 run moves to mew (i9-12900K).

## What this implies for "compress the weights harder"
- Lossless gains on the *existing* quantized weights are tiny. The best context model I tried (per-tensor order-0 or order-1) is about 6 KB below the current coding of the weight part, ×2 copies ≈ 12 KB. That is **about 1% of the 1,004,247 B threshold**. Generic compressors do nothing to the stored blob. The coder is already at entropy for symbol-wise models.
- The structural lever is larger. If the weights did not have to ship in **both** binaries (rules/packaging question, unverified), that would save about 2.93 MB, roughly 2.9× the 1% threshold. Lossy changes (fewer bits/weight, pruning, a smaller model) trade weight bytes against archive bytes. A 1% total win from weights alone would need ~1.0 MB less across both copies, i.e. cutting ~17% of weight bits (~3.1 b/w) **with no loss increase** — that would need retraining, not entropy coding.
