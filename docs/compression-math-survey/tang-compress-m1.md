# tang-compress M1: wavelet + Tucker codecs and a learned context coder

Repo: `/Users/cam/Developer/tang-compress`, branch `claude/tang-compress-qtt` (not pushed).
The full frontier table, covering every codec, dataset and target, is in `tang-compress-m1-summary.md`.

**Data and protocol** are the same as in M0 §8:
- Crops are 64×128×128 f32. The harness is `bench/run_bench.py --tang`.
- Every codec is decoded by its own decoder, then measured by the harness: PSNR on range, and max-abs.
- Rate is the size of the stream file.
- All numbers are matched-PSNR bpv, log-linearly interpolated on each codec's measured Pareto front.
- Single thread, Apple M4 Max. The machine was **heavily loaded** during the runs (load average 20–35), so absolute times are noisy. Ratios between codecs are reliable.

## 1. Design

The crate is now organized as **transform → quantizer → entropy model → binary range coder**, with one bitstream and accounting layer (`bitio::ByteWriter`). Every byte goes through `ByteWriter`, so the bit account has no side channel.

**Modules**

| module | role |
|---|---|
| `abac` | Integer, carry-propagating 32-bit range coder (the LZMA construction) with 16-bit probabilities. Two-rate adaptive `BitModel`, and an adaptive Elias-gamma `UintModel`. |
| `wavelet` | Two lifting transforms, both with whole-sample symmetric extension and any size ≥ 1 (odd lengths supported): CDF 9/7 in f64, and integer LeGall 5/3, which is lossless. Pyramid rules below. |
| `coefcode` | Coefficient coder (details below). |
| `wavcodec` | The wavelet codec: lossy, max-abs and lossless modes. |
| `tucker` | The HOSVD codec. |
| `learned` | The learned models. |
| `codec` | The QTT codec from M0, unchanged. |

`decode_any` dispatches on the stream magic.

**Pyramid.** It is a dyadic multi-level pyramid on the LL region. At each level, every axis whose current length is ≥ 8 gets split, so bands become anisotropic once an axis stops (64×128×128 gives 4 levels on the short axis and 5 on the others). Coefficients are scaled by the synthesis-basis norm of their band, so one uniform step is MSE-matched across bands.

**Coefficient coder (`coefcode`).**
- **Binarization:** significance, then unary bins on `(|r|−1) >> k`, then near-raw low bits, then sign.
  - `k` is an adaptive-Golomb parameter. It comes from the magnitudes of the causal neighbours, the parent, and the prediction.
- **Contexts:** subband class (3 level groups × 3 orientation counts, plus LL), causal activity in the same band (W, N, U, NW, NE, UW, UN, WW), the parent and its 6 face neighbours one level up, and the signs of W, N and U.
- **In-band linear prediction (new, and the biggest single gain).**
  - Highpass bands of smooth fields are themselves smooth along their lowpass axes: neighbour correlation is about 0.9 there, and about −0.5 along the highpass axis.
  - SPECK and SPERR code significance and magnitude only, so they leave this structure unused.
  - Each band gets a 7-tap causal predictor: W, N, U, NW, UW, UN and the parent. Weights are least-squares Q12 integers, fitted on the quantized values the decoder will see. A band keeps its predictor only if a cost proxy drops by more than 2%.
  - The coder codes `q − pred`; contexts are computed on the residuals.
  - Effect: Hurricane-U at 80 dB went from 1.94 to 1.69 bpv (−13%).

**Quantizer.** Deadzone uniform, `q = sgn·⌊|c|/Δ + 0.45⌋`. The reconstruction point is the centroid for each (class, |q| ∈ {1, 2, ≥3}) bucket: 30 offsets of 1/512 each, transmitted in the stream. The mean is removed before the transform.

**Rate control.** For PSNR or RMSE targets, Δ is bisected on the RMSE measured from the real decoder output.

**Max-abs mode (SPERR-style outliers).**
- The wavelet layer is coded at Δ = f·b, for eight values of f between 1 and 5.
- The encoder decodes its own output. Every cell with |x − x̂| > b gets k = round((x − x̂)/2b), which guarantees |x − x̂ − 2bk| ≤ b.
- These corrections are coded as an adaptive significance map with 8 neighbour contexts, plus |k|.
- Cells that the f32 cast still pushes over b are stored raw.
- The value of f with the smallest total stream wins. The bound is re-verified on the exact decoder output, and the harness verified it again: every max-abs row has max error ≤ b.

**Lossless mode.** Integer 5/3 transform plus the same coder, on `i32` input. Covered by a round-trip test.

**Tucker codec (`tucker`, TTHRESH-style).**
1. HOSVD from the mode Gram matrices. Factors come from the faer EVD; the core is computed by GEMM.
2. Ranks are chosen by rate–distortion: the last slice in each mode that still has a nonzero quantized core coefficient.
3. Factors are quantized at equal slope: column j gets step Δ/‖core slice j‖, stored as a quarter-octave exponent. Each column is coded as DPCM (order 0, 1 or 2, chosen per column) with a Rice split.
4. **Error feedback:** the core is recomputed by least squares against the *quantized* factors, then quantized.
5. The core is coded with the wavelet coefficient coder over a dyadic pseudo-pyramid in core index space.
6. Δ is bisected on the measured RMSE.

There is no max-abs mode in the Tucker codec; TTHRESH has none either.

**Determinism.**
- Every step from probability to bits is integer arithmetic.
- The wavelet decoder uses plain IEEE f64 lifting with no fused operations, so it is deterministic on any CPU.
- The Tucker reconstruction uses faer GEMM. It is bit-identical on a given CPU, but across ISAs the float output may differ by an ulp. Its bitstream is identical everywhere.
- Tests check that encoding the same input twice gives the same bytes, and that decoder output matches the encoder's self-check exactly.

## 2. M1a: frontier parity (classical)

Lower bpv is better.

| field | target | SPERR | TTHRESH | SZ3 | QTT (M0) | **tc-wavelet** | **tc-tucker** |
|---|---|---|---|---|---|---|---|
| Miranda density | 60 dB | 0.307 | 0.314 | 0.580 | 0.775 | 0.246 (−20%) | **0.218 (−29%)** |
| Miranda density | 80 dB | 1.196 | 0.773 | 2.58 | 2.16 | 1.064 (−11%) | **0.620** |
| Miranda density | 100 dB | 2.843 | 1.590 | 5.56 | 4.60 | 2.504 | **1.413 (−11% vs TTHRESH)** |
| Miranda density | max-abs 1e-4·R | 2.005 | n/a | 3.40 | 3.12 | **1.694 (−16%)** | n/a |
| Miranda velocityx | 60 dB | 0.191 | 0.222 | 0.352 | 0.517 | 0.172 (−10%) | **0.144 (−25%)** |
| Miranda velocityx | 80 dB | 0.876 | 0.618 | 1.98 | 1.78 | 0.816 (−7%) | **0.493** |
| Miranda velocityx | 100 dB | 2.347 | 1.534 | 4.87 | 4.36 | 2.163 | **1.299 (−15% vs TTHRESH)** |
| Miranda velocityx | max-abs 1e-4·R | 1.617 | n/a | 2.60 | 2.76 | **1.403 (−13%)** | n/a |
| Hurricane U | 60 dB | 0.323 | 0.397 | 0.809 | 0.874 | 0.338 (+4.7%) | **0.284 (−12%)** |
| Hurricane U | 80 dB | 1.711 | 1.421 | 3.67 | 3.03 | 1.680 (−2%) | **1.167** |
| Hurricane U | max-abs 1e-4·R | 2.810 | n/a | 4.45 | 4.26 | **2.592 (−8%)** | n/a |

Percentages compare against SPERR unless the cell says otherwise.

Other fields (full table in the summary file):
- **NYX temperature:** tc-wavelet 0.445 / 2.75 / 6.09 bpv at 60 / 80 / 100 dB, against SPERR 0.575 / 3.13 / 6.50. Tucker loses here (1.18 / 4.40 / 7.95), because NYX is not low-rank.
- **Hurricane TC:** wavelet and Tucker sit around SPERR at 60 dB (0.033 vs 0.029), and Tucker wins at 100 dB (1.90 vs TTHRESH 2.17).
- **1D signals:** QTT still wins on damped sines and polynomials. tc-wavelet wins on the piecewise signal at ≥ 60 dB and on white noise.

**Gate M1a: MET.**
- SPERR parity (within 10%) at 60 and 80 dB and at max-abs 1e-4: tc-wavelet alone meets it on all three fields. The worst case is Hurricane-U at 60 dB, +4.7%. The best tang codec beats SPERR everywhere.
- TTHRESH at 100 dB on Miranda: Tucker is −11% (density) and −15% (velocityx).

## 3. M1b: learned context entropy coding

**Model.**
- **Inputs:** 37 integer Q8 features.
  - Signed log-magnitudes of 17 causal or coarser values: the 8 in-band neighbours, NN, UU, UE, the parent and the parent's 6 face neighbours.
  - The signed in-band prediction.
  - Activity summaries.
  - One-hot band class and orientation.
- **Network:** an MLP with integer Q8 weights and i64 accumulators. It outputs a **2-component discretized logistic mixture** over the residual.
- **Coding:** the residual is binarized (zero, sign, exp-Golomb bucket, mantissa). Each binary probability is an integer ratio of CDF masses taken from Q24 logistic and Q16 2^x tables. Those tables are built only from IEEE + − × / √, with no libm.
- **APM:** an optional integer adaptive probability map (secondary estimation, 33 buckets) refines the probabilities.
- **Training:** minimizes the cross-entropy of the actual quantized residuals, in bits. It uses `tang_optim::Adam` with hand-written backprop, checked against finite differences.
  - `tang-ad` was not used. Its scalar tape is one node per scalar operation, which is impractical at about 10⁸ samples × 5k ops.
  - Training is deterministic for a given seed.

**Modes.**
- **(a) Pretrained.** Shape 37→64→32→5 (4,677 parameters). Trained on 12.6M samples drawn from 4 crops × 7 PSNR targets per training field. The stream carries an 8-byte fingerprint of the model.
  - `pre_A` is trained on Hurricane U, Hurricane TC and NYX. It is used **only for the Miranda tests**.
  - `pre_B` is trained on Miranda density, Miranda velocityx and NYX. It is used **only for the Hurricane-U test**.
  - Neither model sees its test field or any field from the same simulation.
  - Training time: about 90 s per model.
- **(b) Overfit per file.** Shape 37→16→5 (693 parameters), trained at encode time. Its weights are entropy-coded and **shipped in the stream, counted in the rate**: about 750 bytes, i.e. 0.0057 bpv.

| field | target | SPERR | tc-wavelet hand | **pretrained** | **overfit (weights counted)** | learned vs SPERR |
|---|---|---|---|---|---|---|
| Miranda density | 60 dB | 0.307 | 0.246 | **0.231** | 0.236 | **−25%** |
| Miranda density | 80 dB | 1.196 | 1.064 | 1.008 | **0.994** | **−17%** |
| Miranda density | max-abs 1e-4 | 2.005 | 1.694 | 1.619 | **1.605** | **−20%** |
| Miranda velocityx | 60 dB | 0.191 | 0.172 | **0.159** | 0.165 | **−17%** |
| Miranda velocityx | 80 dB | 0.876 | 0.816 | 0.770 | **0.769** | **−12%** |
| Miranda velocityx | max-abs 1e-4 | 1.617 | 1.403 | 1.339 | **1.330** | **−18%** |
| Hurricane U | 60 dB | 0.323 | 0.338 | **0.328** | 0.331 | +1.5% |
| Hurricane U | 80 dB | 1.711 | 1.680 | 1.667 | **1.629** | −5% |
| Hurricane U | max-abs 1e-4 | 2.810 | 2.592 | 2.577 | **2.535** | **−10%** |

**Gate M1b: MET on 2 fields** (Miranda density and Miranda velocityx), at matched PSNR (60 and 80 dB) and at max-abs, with all model bits counted. Hurricane U does not reach −10% at matched PSNR.

How much of the gain is the learned model itself:
- Learned versus the hand context model alone, on the same transform and predictor: **−3% to −7%** (overfit, 80 dB: −6.6% on Miranda density, −5.8% on velocityx, −3.0% on Hurricane U).
- The rest of the margin over SPERR comes from the M1a hand stack, mainly in-band prediction and adaptive-Golomb contexts.
- Pretrained wins at low rate, where the shipped weights are a large share of the stream. Overfit wins at higher rate.
- A larger overfit network trained longer reached 0.944 bpv on Miranda density at 80 dB, after paying for 1.4 KB of weights. That is −11% versus hand, but the encode took 14 s.

The learned coder is used only on the wavelet path. The Tucker core still uses the hand model, so applying the learned coder to it is still to do.

## 4. Speed

Miranda density, 1M cells, single thread, loaded machine. Wall times are noisy.

| codec | encode | decode |
|---|---|---|
| tc-wavelet hand, 60 dB | 0.38 s | 0.05 s (≈80 MB/s) |
| tc-wavelet hand, max-abs 1e-4 | 0.80 s | 0.06 s |
| tc-tucker, 60 dB | 0.30 s | 0.01 s |
| tc-tucker, 100 dB | ~1.4 s | 0.06 s |
| tc-wavelet pretrained, 60 dB | 0.96 s | **0.70 s** |
| tc-wavelet overfit, 60 dB | 1.2 s (includes about 1 s of training) | 0.23 s |
| SPERR (M0) | 0.06–0.10 s | 0.05–0.07 s |

**Honest note on the learned decoder:** it is 5–14× slower than the hand decoder. The cost is integer MLP inference (4.7k MAC per non-quiet coefficient for the pretrained net) plus CDF bisection for the mantissa. It is unvectorized scalar code, and the inference is embarrassingly parallel per band, so this is the obvious target for the GPU decode work.

The Hurricane-U pretrained decode measured 1.4–1.7 s under load.

## 5. Caveats

- **Only five fields are available locally, so the training pool is small.** Pretrained models use 2–3 fields. No new data was downloaded, because downloading needs user approval.
- The test windows are the M0 crops. Training crops are disjoint from them, but the gate claims rest on the field-family exclusion described in §3.
- **Tucker is not universally better.** It loses badly on NYX, because NYX is not low-rank. An automatic chooser between wavelet and Tucker is not implemented. The table reports both codecs.
- **The QTT codec is kept unchanged.** It still wins on oscillatory and algebraic 1D signals.

## 6. Commits (tang branch `claude/tang-compress-qtt`)

- `e422677` wavelet codec: lifting, deadzone quantizer, context-modelled binary range coding, in-band prediction, max-abs outliers, 5/3 lossless
- `ec0346c` Tucker/HOSVD codec
- `e2f4cc7` bench: tc codecs in the harness, plus the M1 summary script
- `a0cfc9c` learned context model: integer MLP → logistic mixture, APM, overfit and pretrained modes, pretrain example
- `8a27b23` mean removal, `tc decode --model`, inference buffers

## 7. Reproduce

```
cargo build --release -p tang-compress --examples
cd bench
.venv/bin/python run_bench.py --tang --tag=tang
./../target/release/examples/pretrain out/models/pre_A.tcm --hidden 64,32 --per 150000 --epochs 8 --per-epoch 3000000 --psnr 40,50,60,70,80,90,100 out/train/{hurricane-U,hurricane-TC,nyx-temperature}-*.f32
.venv/bin/python run_bench.py --tang --tag=tang_learnA miranda --models=overfit,pretrained:$PWD/out/models/pre_A.tcm
.venv/bin/python summarize_m1.py out/tang-compress-qtt-m0-results.csv out/results_tang*.csv
```

The training crops are written by the snippet in this session. They are cut from the four disjoint windows per field, and for Miranda at z = 96.

## 8. Held-out validation

The data was new, and nothing was tuned on it. The full per-field tables are under "Held-out validation" in `tang-compress-m1-summary.md`.

### Protocol

1. **Chooser.** `encode_auto_f32` and `tc --codec auto` encode with two codecs: wavelet with the configured model (the held-out runs use overfit), and Tucker (PSNR/RMSE targets, 2D/3D only). The smaller stream is kept. A 1-byte tag is written and counted in the rate.
   - There is no pre-screen.
   - On the 5 original fields the chooser costs about 2.0 s mean encode. A single hand-wavelet encode is 0.42 s and a Tucker encode 0.45 s. Most of the extra time is overfit training.
2. **Freeze.**
   - The frozen codec is **`4e27445`**.
   - The frozen pretrained model is `bench/models/pre_ALL.tcm`: 37→64→32→5, trained only on crops of the 5 original fields.
   - One crash fix landed during the run: **`009bec6`**. Tucker's least-squares core now falls back to a ridge when the quantized factors are dependent (this crashed on scale-PRES). It is encoder-only, so streams that factor cleanly are unchanged. Old-field Tucker streams were re-verified to be byte-identical at 40, 80 and 100 dB on Miranda density, Hurricane U and NYX.
   - scale-PRES was rerun after the fix.
3. **Data.** 14 held-out fields, all from SDRBench's Hugging Face mirror. Only the crop slabs were fetched, via HTTP range requests, about 52 MB in total. `bench/fetch_heldout.py` writes `data/heldout/manifest.json`.

| group | application | fields | crop |
|---|---|---|---|
| new-app | SCALE-LETKF | T, U, QV, PRES | 64×128×128 |
| new-app | CESM-ATM (2D) | CLDTOT, FLDS, LHFLX, TS | 1024×1024 |
| new-app | EXAFEL (2D) | event 0 | 1024×1024 |
| new-app | QMCPACK | orbitals 0 and 100 | 115×69×69 |
| same-app | Miranda | pressure (f64 source, cast to f32) | 64×128×128 |
| same-app | Hurricane | V | 64×128×128 |
| same-app | NYX | velocity_x | 64×128×128 |

That is 11 new-app fields across 4 new applications, and 3 same-app controls.

4. **Frontier.** Every codec was run with error measured on the decoder side and rate taken from actual file sizes.
   - QTT used row order only, for time.
   - **TTHRESH accepts only ≥3D input** (`-s` needs 3 sizes), and it also failed on 2D data padded with unit axes. So it has no results on the 5 2D fields.
   - SPERR's lowest point is above 40 dB, so the 40 dB comparisons against SPERR are blank.

### Aggregate

Geometric mean of the bpv ratio. The reference is SPERR, or the best external codec (QTT, SZ3, ZFP, SPERR, TTHRESH) at each point. All 14 fields unless n says otherwise.

| codec | ref | 40 dB | 60 dB | 80 dB | 100 dB | max 1e-2 | max 1e-3 | max 1e-4 |
|---|---|---|---|---|---|---|---|---|
| **tc-auto** | SPERR | — | **0.719** | **0.670** | **0.693** | 0.621 | 0.728 | 0.795 |
| **tc-auto** | best external | 0.429 (n=13) | **0.775** | **0.737** | **0.808** | 0.810 | 0.843 | 0.876 |
| tc-wavelet (hand) | SPERR | — | 0.970 | 0.871 | 0.883 | 0.588 | 0.740 | 0.813 |
| tc-wavelet-pretrained (frozen) | SPERR | — | 0.891 | 0.833 | 0.865 | 0.543 | 0.704 | 0.788 |
| tc-wavelet-pretrained (frozen) | best external | 0.478 | 0.960 | 0.916 | 1.009 | 0.708 | 0.815 | 0.869 |
| tc-wavelet-overfit | SPERR | — | 0.995 | 0.849 | 0.872 | 0.621 | 0.728 | 0.795 |
| tc-tucker | SPERR | — | 1.481 | 1.297 | 1.227 | — | — | — |

Split by group, tc-auto against the best external codec:

| group | 60 dB | 80 dB | 100 dB | max 1e-4 |
|---|---|---|---|---|
| new-app | 0.760 | 0.741 | 0.816 | 0.880 |
| same-app | 0.833 | 0.722 | 0.780 | 0.863 |

There is no new-app penalty.

### Win/loss: tc-auto vs the best external codec (±1% counts as a tie)

| group | PSNR points | max-abs points |
|---|---|---|
| all | **54–1** | **34–8** |
| new-app | 43–0 | 25–8 |
| same-app | 11–1 | 9–0 |

### Where tc-auto loses, and why

- **Max-abs at loose bounds on very smooth fields** (QMCPACK orbitals, scale-PRES, scale-T at 1e-2): SZ3 wins by 1.1–1.5×. At about 0.01–0.06 bpv the fixed overheads dominate: 30 centroid bytes, per-band predictor bytes, and the outlier significance map over every cell. SZ3's Lorenzo predictor plus Huffman coding is leaner there.
- **EXAFEL max-abs 1e-3 and 1e-4, and QMCPACK orb0 max-abs:** QTT wins by 1.08–1.31×. EXAFEL is a detector image with sharp, sparse peaks. The wavelet layer spreads each peak, so the outlier stream grows.
- **Hurricane-V at 60 dB:** a 1.4% loss to SPERR. This is the same pattern as Hurricane-U in M1.

### Chooser accuracy

- Against the oracle over its own two candidates (wavelet-overfit vs Tucker), the geometric-mean ratio is 1.0004. tc-auto is within 1% at 95 of 97 points, worst 1.014×. The small misses are interpolation between the two Pareto fronts.
- Against the oracle over **all** tang codecs the ratio is 1.068, worst 2.76×. The gap comes from points where the frozen pretrained wavelet beats the overfit one, mostly at low rate, where about 750 bytes of shipped weights dominate the stream. tc-auto does not consider the pretrained model. Adding it as a third candidate would cost encode time only.
- Picks: wavelet 90, Tucker 64. Tucker alone is poor on the new applications (1.6× SPERR at 60 dB). That gap is exactly why the chooser matters.

### Speed on held-out data

Median per encode, loaded machine.

| codec | encode | decode |
|---|---|---|
| tc-auto | 1.8 s | 0.07 s |
| tc-wavelet hand | 0.32 s | 0.04 s |
| tc-wavelet pretrained | 0.83 s | 0.47 s |
| tc-tucker | 0.56 s | 0.015 s |
| SPERR | 0.04 s | 0.03 s |
| SZ3 | 0.007 s | 0.004 s |
| TTHRESH | 0.10 s | 0.04 s |
| QTT | 3.0 s | 0.02 s |

### Verdict

**The rate advantage generalizes.** On 14 held-out fields, 11 of them from 4 applications never seen during tuning, the frozen tc-auto uses 0.67–0.72× SPERR's bits at matched PSNR, and 0.74–0.81× the best external codec. It wins 54 of 55 PSNR points.

The frozen pretrained learned coder also transfers: 0.83–0.89× SPERR, compared with 0.87–0.97× for the hand model.

**Weak spots:**
- Max-abs at loose bounds on very smooth data loses to SZ3.
- Max-abs on sparse, peaky images loses to QTT.
- Encode and decode speed: tc-auto encodes about 40× slower than SPERR, and the learned decode is 15× slower.

Harness commits: `4e27445` (chooser, held-out mode, fetcher, frozen model) and `009bec6` (ridge fix). The summary script is committed with this report. No data was committed.
