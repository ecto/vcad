# Hutter M1: training pipeline rebuild and enwik8-scale proxy (status 2026-10-18 ~17:50Z)

Everything lives in `~/hutter/m1` on mew. `PIPELINE.md` there has the detailed pipeline map.
Repo: fresh clone of fx2-cmix-transformer-v1 @ 83f2603 in `~/hutter/m1/repo`, with two small
patches to src/runner.cpp (`m1-runner.patch`) plus new files: `training_recipes/m1_proxy.py`,
`../eval_ce.py`, `../export.py`, `../pipeline.sh`, `../score.py` and `../collect.sh`. I did not
touch the M0 run (container `hutter-compress`, core 6, ~/hutter/src, ~/hutter/logs). My CPU work
ran on cores 8–19 and my GPU memory was capped at 13 GB.

## 1. Pipeline map (short; full version in ~/hutter/m1/PIPELINE.md)
- **Data.** `cmix -e enwik9 --ppmd-only --save-ppmd-{bytes,probs} --save-article-boundaries`.
  The `-e` path runs the enwik9-specific front end first: decode the dictionary and the article
  order, split off intro/coda, reorder articles, phda9. That gives `.ready4cmix`, 934,220,400 B.
  It then runs the generic RunCompression path, the same one `-c` uses: WRT dictionary transform →
  vocab extraction → PPMD. Measured on enwik9: 586,459,020 tokens, 243,428 boundaries. The probs
  file is 586M × 205 × fp16 = 240 GB.
- **Model.** Byte vocab 205, d_model 192, 12 layers in the pattern (KDA, KDA, KDA, sliding-window
  attention with window 1024) ×3, MLP 768 relu², 5.90 M params. The PPMD distribution enters as a
  prior embedding.
- **Schedule.**
  - train: 320 epochs, bf16, LR 5e-4 Muon + AdamW, 2^20 tokens per step.
  - QAT: 128 epochs in quantize.py; writeup.md says 256. Weights at 15 levels with per-input-channel
    bf16 scales, activations at 8 bits.
  - cooldown: 16 epochs, fp32.
  - Total ≈ 2.7e11 token passes. 8× RTX 5090, 26 h.
- **Export.**
  - Path: `.tch` → `export_weights` (FX2TFW01, 39.6 MB) → `weights_compress compress` (FX2TFWC2
    range coder) → appended as-is to `cmix_orig + comp_dict + comp_order + blob + header`.
  - The repo's own `export_weights.make_reference_config()` is broken: it lacks
    `query_key_norm_gain`. `export.py` uses the cooldown config instead.
  - **Verified:** exporting the shipped `models/6m-q4-fp32.tch` reproduces the shipped `.tfwc2`
    byte-for-byte (2,930,652 B).
- **Hard constraints.**
  - The C++ inference is shape-specialized (`arena_build.h`: V=205, D=192, NL=12, DH=64, NH=3,
    DMLP=768, fixed layer pattern, window 1024). **Changing model size needs C++ work.**
  - The predictor refuses the transformer unless the vocabulary has exactly 205 bytes.
- **Bug in the shipped recipe.** quantize.py's schedule divides by zero on its final batch, because
  warmup == epochs. The M1 recipe clamps this.

## 2. Proxy design
**Data: a prefix of the record's own preprocessed enwik9 stream, not enwik8.**

*Why:* the record's front end (article reorder + phda9) and its 205-byte vocabulary are
enwik9-specific. Plain enwik8 through `-c` would skip the reorder and phda9 and would have a
different vocabulary. So it would measure a different pipeline, and the record's transformer could
not even run on it. A prefix of `.ready4cmix` exercises the exact same code path as the record
(`-e` calls the same RunCompression as `-c dict in out --transformer w`). It also lets the record's
own weights be scored on the proxy as a reference point.

- `data/proxy.raw`: the first 99,969,681 B of `.ready4cmix` (19,097 articles). The cut sits just
  before an article separator, at an offset whose 4 header length bytes are all in enwik9's vocab.
- **Vocab trap.** The vocabulary is built from every byte in the stream, header included. enwik9's
  own length bytes put 0x12 into it, while a naive 10^8 cut lacks bytes 3 and 18 and gains 0x13.
- **Patches.**
  - `FX2_FORCE_VOCAB=data/enwik9-vocab.mask` pins enwik9's 205-byte vocabulary. The vocabulary is
    written to the archive header, so decompression needs nothing extra.
  - `--save-article-boundaries` is allowed with `-c`.
- **Checked:** the proxy token stream (63,805,285 tokens) equals enwik9's first 63,805,285 tokens
  except the 4 length bytes. Its boundaries equal enwik9's first 19,098. So the proxy is exactly
  the first 10.9% of what the record's transformer sees.
- **PPMD data generation:** 7.5 min on one core, 26.2 GB of probs. PPMD alone gives a
  19,422,117 B archive.

**Model: the record's architecture unchanged (5.90 M params).** The C++ is hard-wired to it, so a
smaller model would mean kernel work before any score could be measured. Only the training budget
shrinks.

**Recipe:** the same phases, optimizers, LR, schedule shapes (warmup *fraction* kept), QAT config,
fp32 cooldown and data loader. Deliberate deviations:
- Epochs are 12 / 2 / 1 instead of 320 / 128 / 16. This is set by the time budget: QAT runs about
  4× slower per token than bf16 training, and the fp32 cooldown about 5×.
- Batch is 2^18 tokens per step instead of 2^20, to get more optimizer steps (3,021 in train) from
  a small budget. The micro batch is 2^17 with gradient accumulation.
- Gradient checkpointing is on in train too, to fit under 13 GB. This has no math effect.
- The quantize schedule clamp described above.
- The proxy uses 6.2e9 token passes, about 1/43 of the record's.

**Score (prize sense):**

`S = len(comp9) + len(archive9) = 2·blob + cmix_output + FIXED`

where FIXED = 2·194,836 (unpatched UPX cmix_orig) + 2·100,096 (comp_dict) + 201,042 (comp_order)
+ 32 (headers) = **791,906 B**. These are the record's unpatched code sizes; the patched binary is
252 B larger and is not counted.

**Surrogate:**

`S_sur = CE_bits(transformer on proxy, file order, 2^17-piece contexts)/8 + 2·blob + FIXED`

- It is computed on the GPU in about 3 min with `eval_ce.py`.
- It ignores the mixing gain over the other models. The PPMD-only rows for the 19k article-start
  tokens are missing; that effect is small.
- It is not held-out data. The record trains on the data it compresses, and the proxy does the same.
- Calibration plan: put each proxy run's real S next to its S_sur. With the baseline and the record
  weights as the first two points, we get the offset and slope. Fit after ≥ 4 points.

## 3. Baseline results (run `runs/base`)
| item | value |
|---|---|
| train (bf16, 12 ep) | 4,824 s, final epoch loss 1.2339 nats/token |
| quantize (QAT, 2 ep + 256 scale-init batches) | 2,764 s, final epoch loss 1.2782 nats/token (+3.6%) |
| cooldown (fp32, 1 ep) | 1,365 s, final epoch loss 1.2758 nats/token |
| GPU wall total | 8,953 s = 2.49 h (3090 shared with another process at ~100% util) |
| eval CE on proxy (surrogate, quantized model) | **1.8405 bits/token**, 14,674,539 B over 63,786,186 predicted tokens (PPMD prior alone: 2.4336 b/t) |
| weight blob (.tfwc2) | **2,625,235 B** (record: 2,930,652 B). Same 5,868,864 quantized weights, so ≈3.6 bits/weight all-in; this less-trained model's weights are more compressible |
| binary (cmix with blob) | 3,121,481 B (patched cmix_orig) |
| S_sur | 14,674,539 + 2·2,625,235 + 791,906 = **20,716,915 B** |
| archive (full cmix on proxy) | **pending**: compression started 2026-10-18T17:39:48Z on core 8 |
| total S | pending (= archive + 6,042,376) |
| decompress + cmp | runs automatically after compression (`runs/base/verify.txt`) |

Reference: the record's own weights on the proxy (they were trained on all of enwik9, this prefix
included, with about 43× more compute):
- transformer CE 1.5230 bits/token = 12,143,584 B
- S_sur = 12,143,584 + 2·2,930,652 + 791,906 = **18,796,794 B**
- Full cmix run `runs/record-on-proxy` on core 9: 68.6% done at 2:51 elapsed, ETA ~19:00Z.

## 4. Cost per proxy run
- GPU: ~2.5 h on the shared 3090.
- CPU, one core: about 4.1 h to compress (projected from the reference run's progress; not yet
  measured) and the same again to decompress-verify.
- Disk: 26 GB of shared data plus ~0.1 GB per run, plus a 14.7 GB sparse ppm.temp while cmix runs.
- The surrogate costs 3 GPU-min on top of training, so a sweep can rank variants on S_sur and spend
  CPU only on the finalists.

## 5. Commands
```bash
ssh mew; cd ~/hutter/m1
# one-time data (already done): full-stream tokens/boundaries + proxy cut + PPMD probs
docker run --rm --cpuset-cpus=8 -u $(id -u):$(id -g) -v ~/hutter/m1:/m -w /m/full hutter-fx2:20.04 \
  ./cmix -e enwik9 x.comp --bytes-only --save-ppmd-bytes /m/data/full-bytes.uint8 --save-article-boundaries /m/data/full-bounds.int32
python3 cut.py        # -> data/proxy.raw (needs data/enwik9-vocab.mask, built from cmix -s output)
docker run --rm --cpuset-cpus=8 -e FX2_FORCE_VOCAB=/m/data/enwik9-vocab.mask -u $(id -u):$(id -g) -v ~/hutter/m1:/m -w /m/gen \
  hutter-fx2:20.04 ./cmix -c dict /m/data/proxy.raw x.comp --ppmd-only --save-ppmd-bytes /m/data/proxy-bytes.uint8 \
  --save-ppmd-probs /m/data/proxy-probs.float16 --save-article-boundaries /m/data/proxy-bounds.int32
# a proxy run (train -> QAT -> cooldown -> surrogate -> export -> binary -> compress -> decompress+cmp)
RUN=base TRAIN_EP=12 QUANT_EP=2 COOL_EP=1 BATCH=18 CORE=8 VERIFY=1 nohup ./pipeline.sh > runs/base.pipeline.log 2>&1 &
# collect results (archive size, S, timing, verify)
./collect.sh            # runs/base/score.json has total_bytes once compression ends
```
Set `NOCOMPRESS=1` to stop after the binary is built; add `VERIFY=0` to skip decompression.
