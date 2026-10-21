# Hutter Prize plan

Target: beat fx2-cmix-transformer (Ivanov, 2026), 100,424,672 B on enwik9, by at least 1%
(≥ 1,004,247 B). Rules: 1 CPU core, ~50 h, ~10 GB RAM, no GPU at run time. Offline training
is unrestricted; whatever the decompressor needs counts toward the total.

## What we measured

| Item | Value | Source |
|---|---|---|
| Transformer weights | 5,868,864 at 15 levels (−7..7), ~3.78 bits/weight entropy | hutter-repro.md |
| Stored blob | 2,930,652 B per copy, two copies = 5.84% of the total | hutter-repro.md |
| Lossless re-coding of the blob | ~12 KB total | hutter-repro.md |
| Exact permutation symmetries | 11,872 B per copy, 23,744 B both | symmetry-bits-back.md |
| Rotation bits-back (2410.01309) | not applicable: needs float weights | symmetry-bits-back.md |
| Relative entropy coding | applies only to a retrained variational model | symmetry-bits-back.md |

Conclusion: squeezing the existing weights is worth ~35 KB, about 3.5% of what we need. The
win has to come from a better model per byte, or from not paying for the weights twice.

## Levers, ranked

| # | Lever | Plausible saving | Cost | Risk |
|---|---|---|---|---|
| A | ~~Weights counted once instead of twice~~ | none: ruled out | — | rules count the compressor and the decompressor separately (see M0 note) |
| B | Rate-aware retraining: train on data bits + weight bits (free energy) | 0.5–1.5 MB (estimate) | GPU training runs | medium |
| C | Train the transformer to complement the context-model mixture | unknown; likely the largest model-quality lever | GPU runs + mixer simulator | medium-high |
| D | Better mixer: second-order (ONS) updates, fixed-share switching over articles | 0.1–0.5% (estimate) | CPU only | low |
| E | Faster CPU inference → bigger model or more online adaptation in 50 h | depends on profile | kernel work | medium |
| F | Symmetry coding + re-coding of the weights | ~35 KB | small | low |

All "estimate" figures are guesses to rank work, not results.

Arithmetic for B: the weights cost ~2 × 2.78 MB. Cutting bits/weight from 3.78 to ~3.1 at equal
prediction quality saves ~1 MB with both copies counted, ~0.5 MB with one. Equivalently, the
same bytes can buy a larger model; the free-energy objective finds the best point on that curve.

## Milestones and gates

**M0 rules note (checked 2026-10-18, hrules.htm).** Main rule: `S := length(comp9.exe/zip) +
length(archive9.exe)`, where archive9.exe is self-extracting and so contains the decompressor.
Relaxation: `S := length(comp9a.exe/zip) + 2×length(decomp9.exe/zip) + length(archive9.bhm)`,
and "if comp9a.exe = decomp9.exe, the 2× can be reduced to 1×", which still counts the one
program twice. Every form pays for the weights twice, so lever A is dead. The record uses the
main rule (self-extracting archive9).

**M0 — Ground truth (days).**
- Quote the rules on what counts (lever A). If the compressor's copy doesn't count, A alone
  nearly clears 1% — then confirm with the prize committee before relying on it.
- Reproduce the record on native x86 (cloud VM; Rosetta is too slow for a 50 h run).
- Profile where the 50 h goes: transformer vs context models vs mixer.
- Gate: reproduced size within 0.1% of 100,424,672 B.

**M1 — A fast proxy (1–2 weeks).**
- Rebuild the record's training pipeline from the repo. Train small versions on enwik8.
- Confirm that enwik8 total bytes rank changes the same way enwik9 does.
- Gate: proxy ranking agrees with full runs on at least two variants.

**M2 — Rate-aware training (2–4 weeks).**
- Objective: cross-entropy + λ·(weight bits under a learned prior over the 15 levels),
  with weight bits counted the way the submission stores them.
- Sweep λ and model size on the proxy; plot total bytes.
- Gate: ≥ 15% fewer weight bits at equal loss, or equal bits with lower loss worth ≥ 0.5 MB.

**M3 — Mixture-aware training (3–6 weeks).**
- Train the transformer through a simulated mixer so it predicts what the context models miss.
- Gate: total-byte gain on the proxy beyond M2.

**M4 — CPU side (parallel with M2–M3).**
- Mixer upgrades (D) and symmetry/weight coding (F).
- Kernel speedups (E) only if M0's profile shows the transformer dominates the time budget.

**M5 — Full run and submission.**
- Train the final model on enwik9, run the 50 h CPU verification, publish source, submit.
- Gate: ≥ 1% under the record with margin, reproducible bit-for-bit.

## Pivot (2026-10-18): bigger model, faster kernels, mixture-aware training

### Why
- **Rate-aware training is the wrong direction.** At the natural λ, weights shrank 7.5% but the
  transformer's data cost rose 2.9%, a projected +3.8 MB on enwik9 (hutter-m2-rate.md). At the
  margin one stored weight bit is worth ~10 data bits, so the model is too small, not too big.
  Caveat: measured on an under-trained proxy (1/43 of the record's compute); the full-compute
  regime may differ, and 3 sweep points are still pending.
- **The transformer is the whole game.** It takes ~60% of runtime (hutter-native-run.md), and
  the full mixture removes only ~2.4% of its bytes (record weights on the proxy: archive =
  0.9764 × transformer-only bytes).
- **Time, not bytes, is the binding constraint.** The rule is runtime < 70,000/T hours, T = the
  machine's single-core Geekbench 5 score (test machines: i7-1165G7 T≈1427 → ~49 h; Ryzen 7
  T=1310 → ~53 h). mew's i9-12900K is faster (T ≈ 1,900–2,000, unverified), so its limit is
  ~35–37 h, and the record's compress pass is projecting ~33.5 h there. Headroom is likely
  under 10%. A bigger model only fits if the kernels get faster first.

### Tracks
**P1 — Time budget truth (CPU, now).**
- Measure mew's Geekbench 5 single-core score; compute its allowed hours.
- When the enwik9 run ends, record actual compress and decompress wall time.
- Output: headroom % = 1 − measured / allowed. This sets how much bigger a model can be at
  today's kernel speed.

**P2 — Faster transformer kernels (CPU, now).**
- Targets from the profile: batch-1 int8 GEMV and the KDA linear-attention sweep, AVX2 only.
- Levers: weight layout and blocking for cache, fusing gate/projection GEMVs, prefetch, fewer
  passes over activations. No new instructions (no VNNI or AVX-512 on the test machines).
- Hard constraint: outputs bit-identical to today's kernels on Intel and AMD (the writeup
  documents arithmetic-coder desync from vendor float differences). Integer paths only.
- Harness: replay a fixed token stream through old and new kernels; compare every logit bit.
- Gate: ≥1.5× transformer throughput at identical output.

**P3 — Configurable architecture, then a size sweep (C++ + GPU).**
- The C++ inference is hard-wired to D=192, 12 layers, a fixed layer pattern and vocab 205.
  Generalize it (shapes from the weight-file header) and verify the record's model is unchanged
  bit-for-bit.
- On the proxy, train 6M, ~9M and ~12M models at matched tokens; plot projected enwik9 total
  against inference cost per token. Pick the largest size that fits the P1 budget after P2.
- Compute: full-size training of a bigger model is ~1.5–2× the record's ~208 GPU-hours
  (estimate). That needs rented GPUs; mew's single shared 3090 is for proxy work only.

**P4 — Mixture-aware training (GPU).**
- The transformer should spend capacity where the context models are weak.
- Cheap first version: dump per-byte loss of cmix *without* the transformer on the proxy, then
  weight the training loss toward bytes cmix predicts badly.
- Full version: train through a small differentiable model of the final mixer, fed saved
  predictions from PPMD and the strongest context models.
- Gate: real cmix total on the proxy beats the same-size plain-trained model.

### Order
P1 and P2 start now on mew's spare cores (never core 6). P3's C++ generalization runs alongside
P2. P4 starts once the proxy calibration (3 real totals) confirms the surrogate ranks models
correctly. Full-size GPU training waits until P2's gate and P3's size choice are in.

## Where tang fits
Training starts in the record's own stack (PyTorch) to move fast. tang comes in where it is
strongest: the deterministic integer inference path and mixer as generated C (lever E, D), and
the free-energy trainer once it is proven on the proxy. That keeps the attempt from blocking on
tang features while still building tang-compress pieces.

## Budget and payoff
- The record's model took ~8 GPUs × 26 h ≈ 208 GPU-hours per training run. A campaign of
  5–10 full runs plus proxy sweeps is roughly 1,000–2,500 GPU-hours (estimate).
- Prize: ~€5,000 per 1%. Expected payout €5–15k, likely below compute cost.
- The real return is the result itself and the reusable compression stack.

## Open questions
1. Exact rules wording on compressor vs decompressor size (lever A).
2. GPU source: cloud H100s vs owned hardware.
3. Whether the record's training code and data order are fully released (needed for M1).
