# Hutter M2: rate-aware QAT (status 2026-10-19 13:10Z: sweep done, ½λ* real scoring running)

Work lives in `~/hutter/m2` on mew. Code:
- `~/hutter/m1/repo/training_recipes/m2_rate.py`
- `~/hutter/m2/{m2_point.sh, sweep.sh, project.py, real.sh}`
- `~/hutter/m1/rate_check.py`

Same machine rules as M1: CPU on cores 8–19 (never 6), GPU capped at 13 GB, m1+m2 disk 35 GB.

## 1. What the codec actually charges (pysrc/weights_compress.py, FX2TFWC2)
- Every 15-level weight tensor is coded with **one** adaptive 4-bit binary tree (symbol = level+7).
- That tree is shared across all 111 tensors and uses LZMA-style 11-bit probabilities with a
  shift-5 update, so it adapts fast (a memory of ~32 symbols per node).
- Scales (bf16) and fp32 tensors go through separate byte models, about 125 KB in total.
- No static histogram is stored.

**Validation of the rate estimate** (`rate_check.py`). It replays the codec's exact adaptive cost
and compares it with cheap entropy estimates:

| blob | file | exact int4 cost | rest of file | Σ n·H(per-tensor hist) | ratio | Σ per-row H | ratio |
|---|---|---|---|---|---|---|---|
| M1 baseline | 2,625,235 | 2,500,382 (3.408 b/w) | 124,853 | 2,476,115 | 0.9903 | 2,429,195 | 0.9715 |
| record | 2,930,652 | 2,803,286 (3.821 b/w) | 127,366 | 2,773,771 | 0.9895 | 2,734,466 | 0.9755 |

**Estimate used:** `blob ≈ 1.0099 · Σ_tensors n·H(level histogram)/8 + ~125 KB`. It matches both
blobs to about 0.1%. An out-of-sample check on the λ* run: the final hard estimate
1.0099·2,277,817 + 125 K = 2,425 KB, against a real exported blob of 2,428,403 B (0.15%).

**Differentiable version:**
- u = w / scale(bf16), clamped to [−7.5, 7.5].
- Soft assignment softmax_k(−(u−k)²/τ) with τ = 0.1, giving a soft per-tensor histogram h.
- R = Σ n·H(h) bits.
- The entropy of the empirical histogram is the optimal per-tensor prior, so no learned logits are
  needed.
- Soft and hard values agree to within 0.06% during training (both are logged in quantize.jsonl).

## 2. Objective and λ
- `step objective = CE_sum_step + λ · R_bits`, both in bits.
- In code, CE is in nats and divided by `accum`, so the term added is λ·ln2·R/accum. It is added
  once per optimizer step, during QAT (2 epochs) and cooldown (1 epoch).
- Every run starts from the M1 baseline's pretrained `runs/base/6m.tch`, so each point costs about
  1 h of GPU (QAT 1,770 s + cooldown 1,770 s on the lam1 run) instead of 2.5 h.

**Projected enwik9 score:**

`S_proj = K·D_proxy + 2·B + 791,906`, with K = 1e9/99,969,681 = 10.003.

- D_proxy = data bytes on the proxy.
- B = blob bytes.

**Natural λ.** One optimizer step sees T = 2^18 of the proxy's N_tok = 63,805,285 tokens. So
S_proj (in bits) ≈ K·(N_tok/T)·CE_step + 2R. Dividing by K·N_tok/T gives:

**λ\* = 2T/(K·N_tok) = 2·2^18·99,969,681/(1e9·63,805,285) = 8.2145e-4** data-bits per
weight-bit per step.

Per token this is the coordinator's figure: one weight bit is worth 2·N_raw/1e9 ≈ 0.2 proxy
data-bits, spread over the N_tok/T ≈ 243 steps of an epoch. (An earlier note of mine said 0.82;
that was an arithmetic slip, and nothing was run with it.)

**Scale.**
- 1% of transformer CE moves S_proj by ~1.47 MB.
- 1% of blob moves it by ~52 KB.
- So on enwik9's scale, weight bits are cheap relative to data bits.

## 3. Sweep (sequential on the GPU, `sweep.sh`)
| run | λ | blob B | bits/weight (all-in) | surrogate CE b/token | data B (proxy) | S_sur (proxy) | S_proj (enwik9) |
|---|---|---|---|---|---|---|---|
| M1 base (reference) | — | 2,625,235 | 3.5785 | 1.84047 | 14,674,539 | 20,716,915 | 152,832,269 |
| lam0 (control, same loop) | 0 | 2,625,248 | 3.5785 | 1.84043 | 14,674,232 | 20,716,634 | 152,829,227 |
| lam1 | λ\* | 2,428,403 (−7.5%) | 3.3102 | 1.89338 (+2.9%) | 15,096,456 | 20,745,168 | 156,659,054 (+3.83 MB) |
| lam4 | 4λ\* | 2,427,798 | 3.3094 | 1.89338 | — | — | 156,657,418 |
| lam2 | 2λ\* | 2,428,039 | 3.3097 | 1.89438 | — | — | 156,738,228 |
| lamhalf | ½λ\* | 2,429,427 | 3.3116 | 1.89295 | 15,093,042 | 20,742,834 | 156,625,987 |

All four λ > 0 points land in the same place: blob 2.428 MB ±0.1%, CE 1.893–1.894. The
outcome barely depends on λ over a 8× range. My reading is that the rate term
tips the model into a different discrete state early in QAT (once weights cross bins) and the
size of λ hardly matters after that. It is not a smooth trade-off. Note that S_proj in this table
is the **surrogate** projection, which the real runs below contradict on the proxy.

**Correction:** FIXED is 2·194,836 + 2·100,096 + 201,042 + 32 = **790,938 B**. Earlier M1/M2 text
said 791,906, an arithmetic slip. score.py always used the right value, and project.py is now fixed.
Surrogate S_proj values in the table above therefore read 968 B high.

**Readings so far:**
- The λ=0 control reproduces the M1 baseline (blob within 13 B, CE within 0.002%). So the M2
  training loop is a faithful copy of the record's, and differences come from λ alone.
- At λ\*, the rate term buys 7.5% fewer weight bytes (−197 KB) but costs 2.9% more data bits
  (+422 KB on the proxy, +4.2 MB projected).
- That is roughly 10× worse than the exchange rate S_proj prices it at. Even the raw proxy total,
  which over-weights the weights, gets worse (+28.5 KB).
- The data loss is much larger than a first-order balance at λ\* predicts. With Muon's normalized
  updates and the QAT LR peaking at 2.5e-5, the rate gradient visibly steers the weights. The
  cooldown loss jumps at the end of the lam1 run (last-step CE 2.015 b/t).
- Higher λ will save more bits at a larger data cost. ½λ\* is the most likely to be non-negative.

## 4. Real cmix totals (full mixture on the proxy, one core each)
| model | blob | transformer-only data B | real archive | archive / tf bytes | real proxy total S | surrogate S | extrapolated enwik9 from REAL archive |
|---|---|---|---|---|---|---|---|
| λ=0 (M1 baseline; lam0 reproduces it) | 2,625,235 | 14,674,539 | 13,596,451 | 0.92653 | **19,637,859** (decompress VERIFIED) | 20,715,947 | 142,047,154 |
| ½λ\* (lamhalf) | 2,429,427 | 15,093,042 | pending (core 8, started 13:04:50Z, ~4–5 h + verify) | | | 20,741,866 | |
| λ\* (lam1) | 2,428,403 | 15,096,456 | 13,729,880 | 0.90948 | **19,377,624** | 20,744,200 | 142,988,184 |
| record weights | 2,930,652 | 12,143,584 | 11,857,526 | 0.97644 | 18,509,768 | 18,795,826 | 125,263,464 |

The extrapolation is `archive·1e9/99,969,681 + 2·blob + 790,938`. **It is only an
extrapolation:** it assumes enwik9 compresses at the proxy's bytes per input byte. The record's
real enwik9 total is 100.42 MB against 125.26 MB extrapolated, because more data means a better
online model. Use it to rank models, not as an absolute number.

**What the real runs show**
- **The surrogate mis-ranks.** The mixture absorbs most of the transformer's extra bytes. The
  baseline-to-λ\* change is +421,917 B transformer-only but only +133,429 B real archive (68%
  absorbed). On the proxy, λ\* is therefore really 260,235 B *better*: −393,664 B of weights,
  +133,429 B of archive. The surrogate said 28 KB worse.
- **On the projection, λ\* is still worse.** Scaling the archive term by 10.003 turns +133 KB of
  archive into +1.33 MB, against −0.39 MB of weights: **+0.94 MB worse** extrapolated to enwik9.
  The proxy's own total over-weights the weights by about 10× (as expected), and that is what
  makes λ\* look good on the proxy.
- **The mixture's gain is not a constant.** The archive/tf ratio is 0.977 on the record model,
  0.927 on the baseline and 0.909 on λ\*. The weaker the transformer, the more the other models
  make up. A surrogate that ignores this is unreliable for ranking. Use real cmix totals for any
  decision. A better surrogate would replay the mixer on saved transformer probabilities
  (`--load-transformer-probs` exists for this and skips the transformer, roughly 40% of the run
  time, by the M0 profile).

## 5. Verdict vs the gate
- **Gate not met.** Gate: ≥ 15% fewer weight bits at equal data cost, or an equal-bits model that
  is ≥ 0.5 MB better projected.
- Every λ > 0 gives ~7.5% fewer weight bits (2.625 → 2.428 MB).
  - On the real proxy total, λ\* is 260 KB (1.3%) better.
  - Extrapolated to enwik9 from the real archive, it is ~0.94 MB worse.
- ½λ\* lands in the same state, blob 2,429,427 B and CE 1.8930. Its real archive will most likely
  match λ\*'s; the run is still in progress to confirm.
- The plain entropy penalty in a 2-epoch QAT does not give a tunable trade-off. It jumps to one
  state no matter λ.
- **Next:**
  - Use real totals, or a mixer-replay surrogate, for any ranking.
  - Try λ ≪ λ\* (e.g. λ\*/16) or ramp the rate term in late, to see whether a smooth regime exists.
  - Bank the free codec gain from per-row level models (~70 KB per copy).

## 6. Commands and how to collect
```bash
ssh mew; cd ~/hutter/m2
cat sweep.log sweep.err 2>/dev/null; cat runs/*/score.json        # sweep points as they land
RUN=lamX LAMBDA=<x> [TAU=0.1 LR_MULT=1] ./m2_point.sh               # one point (~1 h GPU)
RUN=<point> CORE=<free core, not 6> nohup ./real.sh > runs/<point>/real.out 2>&1 &   # real cmix score (~5 h)
cat runs/lamhalf/real_score.json runs/lamhalf/verify.txt            # ½λ* real total + decompress check (pending)
cat runs/lam1/real_score.json                                       # λ* real total
~/hutter/m1/collect.sh                                              # M1 baseline real total + verify
cd ~/hutter/m1/repo && .venv/bin/python ../rate_check.py <blob.tfwc2>   # rate-estimate check
```
