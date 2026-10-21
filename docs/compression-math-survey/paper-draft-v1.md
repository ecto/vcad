# tang-compress: error-bounded scientific-data compression with in-band prediction, learned integer context models, and a bit-exact GPU decoder

*Preprint draft, phase 1 (2026-10-20). Not peer-reviewed. Code is on branch `claude/tang-compress-qtt` of the tang repository (worktree `/Users/cam/Developer/tang-compress`); it has not been pushed.*

> **Provenance rule.** Every number in this draft is copied from one of four reports in this folder, or from the CSV/JSONL files they cite. Each table and each claim names its source. Short names used below:
>
> | tag | file |
> |---|---|
> | [M0] | `tang-compress-qtt-m0.md` (QTT codec; §8 adds SPERR/TTHRESH) |
> | [M1] | `tang-compress-m1.md` (wavelet + Tucker + learned coder; §8 held-out) |
> | [S] | `tang-compress-m1-summary.md` (full frontier tables, held-out per-field tables) |
> | [M1d] | `tang-compress-m1d.md` (GPU formats TCG1/TCG3, learned GPU coder) |
> | [CSV-M0] | `tang-compress-qtt-m0/tang-compress-qtt-m0-results.csv` |
> | [B] | `/Users/cam/Developer/tang-compress/bench/out/` (`heldout_*.csv`, `gpu*_*.jsonl`) |
>
> Nothing here was re-measured for the paper. Where a report gives a range, the range is reproduced, not a point.

---

## Abstract

We describe tang-compress, an error-bounded lossy compressor for floating-point scientific fields, built on a Rust numerical stack. The CPU codec is a CDF 9/7 wavelet transform followed by per-band integer linear prediction, a deadzone quantizer with transmitted centroid reconstruction, a context-modelled binary range coder (hand-designed or a small learned integer MLP), and a max-abs correction stream; an encoder-side chooser picks between this and a Tucker/HOSVD codec. Every byte of every stream is counted, error is measured on the decoder output, and all probability-to-bit arithmetic is integer.

We tuned on five SDRBench fields, froze the codec, and evaluated on 14 held-out fields, 11 of them from four applications never seen during development. On the held-out set the frozen chooser (tc-auto) uses a geometric mean of 0.67–0.72× SPERR's bits at matched PSNR of 60–100 dB, and 0.74–0.81× the bits of the best external codec at each point (SPERR, TTHRESH, SZ3, ZFP or our own QTT baseline). It wins 54 of 55 matched-PSNR points and 34 of 42 max-abs points against that per-point best [M1 §8; S]. A frozen pretrained learned context model transfers to the new applications (0.83–0.89× SPERR, against 0.87–0.97× for the hand model) [M1 §8].

We also give a GPU-decodable variant whose decoder is integer-only and byte-identical between CPU and GPU on every one of several hundred tested cases. With static context priors it decodes at 1.3–7.3 GB/s (kernel time) in the bricked format; with a learned 16-hidden-unit context model compiled into the shader as literals it reaches 1.10 GB/s kernel and 0.91 GB/s end-to-end on a 537 MB field [M1d §4, §11]. The GPU formats cost 11–15% in rate against our serial coder at 80 dB, which leaves them roughly at parity with SPERR [M1d §9–10]. The CPU encoder is about 40× slower than SPERR [M1 §8].

---

## 1. Introduction

Simulation and instrument output now routinely outgrows storage and I/O bandwidth, and lossless coders achieve only modest ratios on floating-point data. Error-bounded lossy compressors trade a user-specified error (a pointwise bound, or an RMSE/PSNR target) for much higher ratios. Four families dominate current practice:

- **Prediction + quantization** — SZ and its modular successor SZ3 [Liang22; Di16] predict each value from decoded neighbours (Lorenzo, interpolation, regression), quantize the residual against the bound, and entropy-code it.
- **Block transforms** — ZFP [Lindstrom14] applies a fixed decorrelating transform to 4^d blocks and codes bit planes; fast and random-access, but rate-inefficient at a given accuracy.
- **Wavelets with set-partitioning** — SPERR [Li23] couples a CDF 9/7 wavelet with SPECK coding [Pearlman04] and an outlier stream that enforces a pointwise bound.
- **Tensor decompositions** — TTHRESH [BallesterRipoll20] codes an HOSVD/Tucker core by bit planes; it is strong at high PSNR on low-rank fields and has no pointwise mode.

Benchmarks of these tools are usually reported on SDRBench fields [Zhao20].

This paper asks a narrow question: **how much rate is still left on the table by the entropy-coding stage of a wavelet compressor, and can a coder that recovers it stay deterministic enough to decode bit-exactly on a GPU?** SPECK-style coders code significance and magnitude, but leave intra-band correlation of the coefficients themselves unused [M1 §1]. We add per-band integer linear prediction, rich causal contexts, and optionally a learned context model, and we keep every step from probability to bit in integer arithmetic.

**Contributions.**
1. A codec stack (transform → predictor → quantizer → integer context model → binary range coder) with complete bit accounting, a max-abs mode, and an encoder-side wavelet/Tucker chooser (§3).
2. A held-out evaluation protocol: tune on 5 fields, freeze code and model, evaluate on 14 unseen fields across 4 new applications, measure all codecs on their decoded output (§4–5).
3. A GPU stream design (bricks, and code-blocks over a global transform) whose integer decoder is byte-identical on CPU and GPU, including a learned MLP context model specialised into the shader (§3.8, §5.5).
4. A frank accounting of where the method loses: encode speed, loose pointwise bounds on very smooth data, sparse peaky images, and the GPU ratio gap (§6).

A separate QTT (quantized tensor-train) codec built first [M0] is reported as a baseline: it wins on 1D signals with algebraic structure but not on 3D fields (§5.1).

---

## 2. Background and notation

- **Rate** is bits per value (bpv): full compressed file bytes × 8 / number of values [M0 §2].
- **PSNR** = 20·log₁₀(range / RMSE), range = max − min of the original [M0 §2].
- **Max-abs bound** b = rel · range, with rel ∈ {1e-2, 1e-3, 1e-4} [M0 §2].
- **Matched-PSNR rate**: each codec is swept over its own knobs; its measured (PSNR, bpv) points form a Pareto front; the rate at a target PSNR is a log-linear interpolation on that front [M0 §2; M1 intro].
- **Geomean ratio**: the geometric mean over fields of bpv(codec) / bpv(reference) at the same point, over fields where both are defined [S].

---

## 3. Method

### 3.1 Transform

- CDF 9/7 lifting [Cohen92; Sweldens96] in IEEE f64, with whole-sample symmetric extension; any length ≥ 1, odd lengths included. An integer LeGall 5/3 path gives a lossless mode [M1 §1].
- Dyadic pyramid on the LL region: at each level, every axis whose current length is ≥ 8 is split, so bands become anisotropic once an axis stops (64×128×128 → 4 levels on the short axis, 5 on the others) [M1 §1].
- Coefficients are scaled by their band's synthesis-basis norm, so a single uniform step is MSE-matched across bands. The mean is removed before the transform [M1 §1].

### 3.2 Per-band integer linear prediction

Highpass bands of smooth fields are themselves smooth along their lowpass axes (neighbour correlation ≈ 0.9) and anti-correlated along the highpass axis (≈ −0.5) [M1 §1]. Each band gets a 7-tap causal predictor over W, N, U, NW, UW, UN and the parent. The weights are least-squares Q12 integers fitted on the *quantized* values the decoder will see; a band keeps its predictor only if a cost proxy falls by more than 2%. The coder codes q − pred, and contexts are computed on residuals [M1 §1].

This was the largest single gain in development: Hurricane-U at 80 dB went from 1.94 to 1.69 bpv (−13%) [M1 §1].

### 3.3 Quantizer

Deadzone uniform quantization, q = sgn(c)·⌊|c|/Δ + 0.45⌋. Reconstruction uses a transmitted centroid for each (band class, |q| ∈ {1, 2, ≥3}) bucket: 30 offsets at 1/512 resolution [M1 §1]. For PSNR/RMSE targets Δ is bisected on the RMSE of the *actual decoder output* [M1 §1].

### 3.4 Hand context model and range coder

- **Coder:** an integer, carry-propagating 32-bit binary range coder (the LZMA construction [Pavlov, uncited — see Refs]) with 16-bit probabilities and two-rate adaptive bit models [M1 §1].
- **Binarization:** significance; unary bins on (|r|−1) ≫ k with k an adaptive-Golomb parameter from causal neighbour, parent and prediction magnitudes; near-raw low bits; sign [M1 §1].
- **Contexts:** subband class (3 level groups × 3 orientation counts, plus LL); causal activity in the band (W, N, U, NW, NE, UW, UN, WW); the parent and its 6 face neighbours; signs of W, N, U [M1 §1].

### 3.5 Learned integer context model

- **Inputs:** 37 integer Q8 features — signed log-magnitudes of 17 causal or coarser values, the signed in-band prediction, activity summaries, and one-hot band class/orientation [M1 §3].
- **Network:** an MLP with Q8 integer weights and i64 accumulators, outputting a 2-component discretized logistic mixture over the residual (the output family of [Salimans17]) [M1 §3].
- **Coding:** the residual is binarized (zero, sign, exp-Golomb bucket, mantissa); each binary probability is an integer ratio of CDF masses from Q24 logistic and Q16 2^x tables built from IEEE + − × / √ only (no libm). An optional integer adaptive probability map (secondary estimation, 33 buckets) refines probabilities [M1 §3].
- **Training:** cross-entropy of the actual quantized residuals, in bits; Adam with hand-written backprop checked against finite differences; deterministic per seed [M1 §3].
- **Modes:** *pretrained* (37→64→32→5, 4,677 parameters, 8-byte model fingerprint in the stream) and *overfit-per-file* (37→16→5, 693 parameters, trained at encode time, weights entropy-coded into the stream and counted in the rate: about 750 bytes, 0.0057 bpv on a 1M-cell field) [M1 §3].

### 3.6 Max-abs correction stream

For a pointwise bound b, the wavelet layer is coded at Δ = f·b for eight values of f in [1, 5]. The encoder decodes its own output; every cell with |x − x̂| > b gets k = round((x − x̂)/2b), which guarantees |x − x̂ − 2bk| ≤ b. Corrections are coded as an adaptive significance map (8 neighbour contexts) plus |k|; cells the f32 cast still pushes past b are stored raw. The smallest total stream over f wins, and the bound is re-verified on the exact decoder output [M1 §1] — the same outlier idea SPERR uses [Li23].

### 3.7 Tucker codec and the auto chooser

- **Tucker (TTHRESH-style)** [BallesterRipoll20; DeLathauwer00]: HOSVD from mode Gram matrices; rate–distortion rank truncation; factors quantized at equal slope with per-column DPCM; core recomputed by least squares against the quantized factors (error feedback), then coded with the wavelet coefficient coder; Δ bisected on measured RMSE. No max-abs mode [M1 §1]. A ridge fallback handles dependent quantized factors [M1 §8].
- **tc-auto:** encode with wavelet (overfit learned model in the held-out runs) and with Tucker (PSNR/RMSE targets, 2D/3D only); keep the smaller stream; a 1-byte tag is counted in the rate. No pre-screen [M1 §8].

### 3.8 Determinism

All probability-to-bit steps are integer. The CPU wavelet decoder uses plain IEEE f64 lifting with no fused operations, so it is deterministic across CPUs; the Tucker reconstruction uses a GEMM that is bit-identical on a given CPU but may differ by an ulp across ISAs (its bitstream is identical everywhere) [M1 §1].

### 3.9 GPU stream formats

**TCG1 (bricks)** [M1d §1]:
- Independent B³ bricks (B²-tiles in 2D), each with its own pyramid, for random access.
- A parallel coding order: within each band, lines are split into hierarchical parity passes (7 in 3D, 4 in 2D) and dealt round-robin to L interleaved rANS lanes [Duda13] (default 32; 32-bit state, 16-bit words).
- **Static context priors** (a 64-level logit grid sent in the header) instead of adaptive models: the adaptive per-lane version was memory-bound, about 30× slower on GPU, while static priors cost +1–4% bpv.
- **Integer-only decode:** a fixed-point grid x ≈ v·2^e chosen so values fit i32 with headroom; dequantization with a 30-bit step mantissa per band; inverse CDF 9/7 lifting with Q24 constants through a 32×32→64 multiply (emulated with 16-bit limbs in WGSL); max-abs corrections in i32; the f32 output is exact because |v| < 2^24 and the scale is a normal power of two. The encoder rate-controls on this integer decoder, so fixed-point loss is already inside every reported number.

**TCG3 (code-blocks over one global transform)** [M1d §9]: one global fixed-point pyramid; each subband is tiled into 16³/32³ blocks (64²/128² in 2D), in the spirit of JPEG 2000 code-blocks [Taubman00]. A block reads any coarser-level coefficient and only its own region at its level, so all blocks of a level are independent. Residual-based contexts and a cost-gated prior header (a prior is sent only if it saves more than 12 bits) are added. Blocks use 1 lane (raster order) or 8 lanes (parity passes). Random access maps a box to coefficient ranges per level with a conservative ±3 margin, then selects blocks and their parent blocks transitively (CPU only).

**Learned GPU coder on TCG3** [M1d §10–11]: 28 integer Q8 features restricted to what TCG3's dependency rule allows; the same MLP form running entirely in i32, with clamps (weights ±2047, hidden [0, 2^14], outputs ±2^14, biases ±2^24) that rule out overflow; a Q16 CDF; probabilities as u32 integer ratios of masses. Pretrained sizes 37→32→16→5 (1,829 parameters) and 37→16→5 (693 parameters). The speedups that mattered were (i) a straight-line MLP with the stream's weights as WGSL literals, compiled once per network (5.0× / 3.2×), (ii) an exact 32-bit form of the CDF scale multiply replacing an emulated 64-bit one (+10% / +14%), and (iii) packing 4 code-blocks per workgroup to fill the 32-wide SIMD group (+55% / +31%). Putting weights in workgroup memory was slower; `dot4I8Packed` was unavailable in the wgpu/naga version used and would change bitstreams.

---

## 4. Experimental protocol

### 4.1 Data

- **Development set (5 fields)** [M0 §2]: Miranda density and velocityx (f64 cast to f32), Hurricane Isabel U and TC, NYX temperature, all cropped to interior 64×128×128 windows (1,048,576 cells) from SDRBench's Hugging Face mirrors. Plus four synthetic 1D signals of 2²⁰ samples (damped sinusoids, degree-5 polynomial, piecewise with 7 jumps, white noise).
- **Held-out set (14 fields)** [M1 §8]: fetched only after the codec was frozen, via HTTP range requests (~52 MB).

| group | application | fields | crop |
|---|---|---|---|
| new-app | SCALE-LETKF | T, U, QV, PRES | 64×128×128 |
| new-app | CESM-ATM (2D) | CLDTOT, FLDS, LHFLX, TS | 1024×1024 |
| new-app | EXAFEL (2D) | event 0 | 1024×1024 |
| new-app | QMCPACK | orbitals 0, 100 | 115×69×69 |
| same-app | Miranda | pressure | 64×128×128 |
| same-app | Hurricane | V | 64×128×128 |
| same-app | NYX | velocity_x | 64×128×128 |

*Source: [M1 §8].*

### 4.2 Freeze

The frozen codec is commit `4e27445`; the frozen pretrained model `bench/models/pre_ALL.tcm` (37→64→32→5) was trained only on crops of the 5 development fields. One encoder-only crash fix (`009bec6`, Tucker ridge fallback, triggered by scale-PRES) landed during the run; development-field Tucker streams were re-verified byte-identical afterwards [M1 §8].

For the earlier development-set learned-coder gate, pretrained models were trained with field-family exclusion: `pre_A` (Hurricane U, Hurricane TC, NYX) was tested only on Miranda, `pre_B` (Miranda ×2, NYX) only on Hurricane U [M1 §3].

### 4.3 Baselines

- **SPERR** commit `b801258…`, PSNR and pointwise-error modes, `--omp 1`; **TTHRESH** commit `2dbad4e…`, PSNR mode only (no max-abs mode) [M0 §8.1–8.2].
- **SZ3** (PSNR mode and ABS bound) and **ZFP** (fixed accuracy and fixed precision; for max-abs, bisected for the largest tolerance whose *actual* max error ≤ b) via their Python bindings [M0 §2].
- Our **QTT** codec [M0], row-major order on held-out data [M1 §8].
- TTHRESH accepts only ≥3D input, so it has no results on the five 2D fields; SPERR's lowest point is above 40 dB, so 40 dB comparisons against SPERR are blank [M1 §8].

### 4.4 Measurement

- All tools' own error reports are ignored. Every row is decoded by the codec's own decoder, then PSNR and max-abs are measured by the harness on the output. Every max-abs row was verified ≤ b [M0 §8.2; M1 intro].
- Rate is the size of the stream file, so headers, side information, model fingerprints, shipped weights and the chooser tag are all included [M1 §1, §3, §8].
- CPU timings are single-thread on an Apple M4 Max; GPU is the same machine's GPU via Metal/wgpu. **The machine was heavily loaded** throughout (load average 20–35 in M1, 5–70 in M1d), so timings are noisy (±2× between repeats in M1d). Rates are exact [M1 intro; M1d intro].

---

## 5. Results

### 5.1 Baseline context: QTT and the real frontier (development set)

On the 3D development fields, at every fidelity ≥ 60 dB the best external codec is SPERR or TTHRESH, and our QTT codec is 2–3× worse at matched PSNR on Miranda and Hurricane and ≥ 6× on NYX. QTT keeps the 1D oscillatory/algebraic regime: on damped sinusoids it needs 0.0060 / 0.0106 / 0.0152 / 0.0204 bpv at 40/60/80/100 dB, 2–6× fewer bits than the next codec [M0 §8.3, §8.5]. (SPERR and TTHRESH see the 1D signals reshaped to 2D/3D, which handicaps them [M0 §8.2].) This motivated the wavelet/Tucker codecs below.

### 5.2 Development set: classical stack (tc-wavelet, tc-tucker)

bpv, lower is better. *Source: [M1 §2]; full table [S].*

| field | target | SPERR | TTHRESH | SZ3 | tc-wavelet | tc-tucker |
|---|---|---|---|---|---|---|
| Miranda density | 60 dB | 0.307 | 0.314 | 0.580 | 0.246 | **0.218** |
| Miranda density | 80 dB | 1.196 | 0.773 | 2.58 | 1.064 | **0.620** |
| Miranda density | 100 dB | 2.843 | 1.590 | 5.56 | 2.504 | **1.413** |
| Miranda density | max-abs 1e-4 | 2.005 | n/a | 3.40 | **1.694** | n/a |
| Miranda velocityx | 60 dB | 0.191 | 0.222 | 0.352 | 0.172 | **0.144** |
| Miranda velocityx | 80 dB | 0.876 | 0.618 | 1.98 | 0.816 | **0.493** |
| Miranda velocityx | 100 dB | 2.347 | 1.534 | 4.87 | 2.163 | **1.299** |
| Miranda velocityx | max-abs 1e-4 | 1.617 | n/a | 2.60 | **1.403** | n/a |
| Hurricane U | 60 dB | 0.323 | 0.397 | 0.809 | 0.338 | **0.284** |
| Hurricane U | 80 dB | 1.711 | 1.421 | 3.67 | 1.680 | **1.167** |
| Hurricane U | max-abs 1e-4 | 2.810 | n/a | 4.45 | **2.592** | n/a |

NYX temperature is not low-rank: tc-wavelet 0.445 / 2.75 / 6.09 bpv at 60/80/100 dB vs SPERR 0.575 / 3.13 / 6.50, while Tucker loses (1.18 / 4.40 / 7.95) [M1 §2].

### 5.3 Development set: learned context model

bpv; overfit column includes shipped weights. *Source: [M1 §3].*

| field | target | SPERR | hand | pretrained | overfit | best learned vs SPERR |
|---|---|---|---|---|---|---|
| Miranda density | 60 dB | 0.307 | 0.246 | **0.231** | 0.236 | −25% |
| Miranda density | 80 dB | 1.196 | 1.064 | 1.008 | **0.994** | −17% |
| Miranda density | max-abs 1e-4 | 2.005 | 1.694 | 1.619 | **1.605** | −20% |
| Miranda velocityx | 60 dB | 0.191 | 0.172 | **0.159** | 0.165 | −17% |
| Miranda velocityx | 80 dB | 0.876 | 0.816 | 0.770 | **0.769** | −12% |
| Miranda velocityx | max-abs 1e-4 | 1.617 | 1.403 | 1.339 | **1.330** | −18% |
| Hurricane U | 60 dB | 0.323 | 0.338 | **0.328** | 0.331 | +1.5% |
| Hurricane U | 80 dB | 1.711 | 1.680 | 1.667 | **1.629** | −5% |
| Hurricane U | max-abs 1e-4 | 2.810 | 2.592 | 2.577 | **2.535** | −10% |

The learned model alone (same transform and predictor) accounts for −3% to −7% over the hand context model (overfit, 80 dB: −6.6% Miranda density, −5.8% velocityx, −3.0% Hurricane U); most of the margin over SPERR comes from the hand stack — in-band prediction and adaptive-Golomb contexts [M1 §3].

### 5.4 Held-out validation (main result)

**Aggregate.** Geometric-mean bpv ratio over all 14 held-out fields (n = 13 at 40 dB). *Source: [M1 §8]; [S] "Aggregate"; per-field data [B] `heldout_*.csv`.*

| codec | reference | 40 dB | 60 dB | 80 dB | 100 dB | max 1e-2 | max 1e-3 | max 1e-4 |
|---|---|---|---|---|---|---|---|---|
| **tc-auto** | SPERR | — | **0.719** | **0.670** | **0.693** | 0.621 | 0.728 | 0.795 |
| **tc-auto** | best external | 0.429 | **0.775** | **0.737** | **0.808** | 0.810 | 0.843 | 0.876 |
| tc-wavelet (hand) | SPERR | — | 0.970 | 0.871 | 0.883 | 0.588 | 0.740 | 0.813 |
| tc-wavelet-pretrained (frozen) | SPERR | — | 0.891 | 0.833 | 0.865 | 0.543 | 0.704 | 0.788 |
| tc-wavelet-pretrained (frozen) | best external | 0.478 | 0.960 | 0.916 | 1.009 | 0.708 | 0.815 | 0.869 |
| tc-wavelet-overfit | SPERR | — | 0.995 | 0.849 | 0.872 | 0.621 | 0.728 | 0.795 |
| tc-tucker | SPERR | — | 1.481 | 1.297 | 1.227 | — | — | — |

At max-abs points tc-auto equals tc-wavelet-overfit, since Tucker has no max-abs mode.

**No new-application penalty.** tc-auto vs best external, by group [M1 §8; S]:

| group | 60 dB | 80 dB | 100 dB | max 1e-4 |
|---|---|---|---|---|
| new-app (11 fields) | 0.760 | 0.741 | 0.816 | 0.880 |
| same-app (3 fields) | 0.833 | 0.722 | 0.780 | 0.863 |

**Win/loss** of tc-auto vs the best external codec at each (field, point), ±1% counted as a tie [M1 §8; S]:

| group | PSNR points | max-abs points |
|---|---|---|
| all | 54–1 | 34–8 |
| new-app | 43–0 | 25–8 |
| same-app | 11–1 | 9–0 |

**Losses** (all nine, ratio > 1.01) [S "Losses of tc-auto"]: EXAFEL max-abs 1e-3 (1.31× vs QTT) and 1e-4 (1.08× vs QTT); QMCPACK orb0 max-abs 1e-2 (1.11× vs SZ3), 1e-3 (1.08× vs QTT), 1e-4 (1.15× vs QTT); QMCPACK orb100 max-abs 1e-2 (1.21× vs SZ3); scale-PRES max-abs 1e-2 (1.49× vs SZ3); scale-T max-abs 1e-2 (1.02× vs QTT); Hurricane-V 60 dB (1.01× vs SPERR). (The "1.4% loss" for Hurricane-V in [M1 §8] corresponds to the per-field ratio 1.014 in [S].)

**Chooser accuracy** [M1 §8; S]: against the oracle over its own two candidates, geomean 1.0004, within 1% at 95 of 97 points, worst 1.014×. Against the oracle over all four tang codecs, geomean 1.068, worst 2.76× — the gap is points where the frozen pretrained model (not a chooser candidate) beats the overfit one at low rate. Picks: wavelet 90, Tucker 64.

### 5.5 GPU formats

**Bit-exactness** [M1d §2, §9, §10, §11]:

| format | sweep | result |
|---|---|---|
| TCG1, bricks 32³/64³ (2D 256²/512²) | 19 fields × {60, 80, 100 dB, max-abs 1e-3, 1e-4} × 2 brick sizes | 190/190 byte-identical, bound re-verified, sub-box == crop |
| TCG3 static, lanes = 1, blocks 16³/32³ | same × 2 block sizes | 190/190 |
| TCG3 static, lanes = 8, 32³ | 19 × 5 | 95/95 (re-run after kernel change: 95/95) |
| TCG3 learned, pretrained 32×16, 8 lanes | 19 × 5 | 95/95 (after all speedups: 95/95) |

**Rate cost.** Geomean bpv ratio over all 19 fields (development + held-out) at 80 dB unless noted [M1d §3, §9, §10]:

| configuration | vs serial tc-wavelet | vs SPERR |
|---|---|---|
| TCG1 32³ bricks | 1.486 | 1.325 |
| TCG1 64³ bricks | 1.253 | 1.117 |
| TCG1 128³ / whole field | 1.152 | 1.027 |
| TCG3 static, 32³ blocks, 1 lane | 1.107 | 0.987 |
| TCG3 static, 32³ blocks, 8 lanes | 1.182 | 1.054 |
| TCG3 learned pretrained 32×16, 8 lanes | 1.118 | 0.997 |
| TCG3 learned pretrained 16, 8 lanes | 1.134 | 1.011 |
| serial tc-wavelet (reference) | 1 | 0.892 |

At max-abs 1e-3, TCG3 static 32³ / 1 lane is 0.816× SPERR and the learned 32×16 model 0.845× [M1d §9–10]. The learned model beats TCG3's static priors by 4–5.5% on held-out fields and 7–9% on the development fields [M1d §10]. On the development fields alone, the pretrained 32×16 learned GPU coder is +3.4% vs serial at 80 dB [M1d §10].

**Throughput.** GB/s of decoded f32, NYX temperature 512³ (537 MB), best of several runs on a loaded machine [M1d §4, §9, §11; [B] `gpu_speed.jsonl`]:

| format / model | bpv @ 80 dB | GPU kernels | GPU end-to-end | CPU 16 threads |
|---|---|---|---|---|
| TCG1 32³, 32 lanes | 0.106 | 2.65–3.35 | 1.06–2.11 | 0.28–0.40 |
| TCG1 64³, 32 lanes | 0.063 | 1.97–5.00 | 1.40–2.11 | 0.37–0.54 |
| TCG1 32³, 64 lanes | 0.144 | 7.34 | 0.94 | 0.18 |
| TCG3 static 32³, 8 lanes, final kernels | — | 1.90 | 1.41 | — |
| TCG3 learned, pretrained 16, final kernels | 0.0570 | **1.10** | **0.91** | — |
| TCG3 learned, pretrained 32×16, final kernels | 0.0564 | 0.85 | 0.74 | — |

The final-kernel rows were measured at load average 4–5; bpv for the learned rows is from [M1d §10] (same streams, unchanged by the speedups [M1d §11]); no bpv is reported for the static final-kernel row in [M1d §11]. Unmatched-rate references on the same field: SPERR 0.095 GB/s (1 thread) and 0.227 GB/s (16 threads, including file I/O); SZ3 serial 0.59 GB/s [M1d §4].

**Random access** (CPU, 64³ sub-box): TCG1 32³ bricks take 22–41 ms on 1 thread [M1d §4]; TCG3 reads 27–61% of the stream bytes for a 0.2%-of-field box and takes 0.4–1.1 s on 1 thread, because coarse levels hold much of a low-rate stream [M1d §9]. GPU random access is not implemented.

---

## 6. Limitations

1. **Encode speed.** tc-auto encodes in a median 1.8 s per held-out field vs SPERR's 0.04 s and SZ3's 0.007 s (about 40× slower than SPERR) [M1 §8]; most of the cost is overfit training plus running both candidates. The GPU formats encode on CPU only, 16–190 s for NYX 512³, dominated by Δ bisection on the integer decoder [M1d §4].
2. **Learned CPU decode is slow.** The pretrained learned decoder is 5–14× slower than the hand decoder on CPU (0.70 s vs 0.05 s on Miranda density, 60 dB) [M1 §4]; on held-out data the median learned decode is 0.47 s vs SPERR's 0.03 s [M1 §8].
3. **Loose pointwise bounds on very smooth data.** At max-abs 1e-2 on QMCPACK, scale-PRES and scale-T, SZ3 wins by 1.1–1.5×: at ~0.01–0.06 bpv, fixed overheads (30 centroid bytes, per-band predictor bytes, the outlier significance map) dominate [M1 §8].
4. **Sparse, peaky data.** On the EXAFEL detector image and QMCPACK orb0 at tight max-abs bounds, QTT wins by 1.08–1.31×; the wavelet spreads each peak and the outlier stream grows [M1 §8].
5. **GPU ratio gap.** The ≤ 5%-vs-serial target at 80 dB was not met by any GPU format: best is +10.7% (TCG3 static, 1 lane, 32³) and +11.8% (learned 32×16, 8 lanes); the residual is the coder (static vs adaptive probabilities, pass-restricted contexts), not the blocking. Anisotropic atmospheric fields are worst (Hurricane and SCALE +20–24% whole-field in TCG1); low-rate fields pay header overhead (scale-PRES ≈1.6× in TCG3) [M1d §3, §9, §10]. The learned GPU model with 1-lane raster blocks was not measured across all 19 fields [M1d §10].
6. **Throughput targets are partial.** The learned GPU decoder reaches 1 GB/s only with the 16-hidden network; the 32×16 network reaches 0.85 GB/s kernel. TCG3 with 1 lane is far slower (0.17–0.64 GB/s kernel) [M1d §9, §11].
7. **Timing hygiene.** All timings were taken on a shared machine under heavy load (load averages up to 70), with ±2× variation between repeats; we report ranges or best-of-N, and treat timings as order-of-magnitude. Rates are exact [M1 intro; M1d intro].
8. **Scale of evaluation.** 19 fields total, cropped (64×128×128, 1024², or 115×69×69), on a single machine; the throughput study uses one large field (NYX 512³) plus Hurricane-Isabel U 100×500×500 [M1d §4]. Development-set pretraining used only 2–3 fields per model [M1 §5].
9. **Cross-ISA determinism of Tucker.** Tucker reconstruction can differ by an ulp across CPU ISAs (the bitstream does not) [M1 §1]. The bit-exact claims of §5.5 cover the wavelet GPU formats only, on one GPU (Apple M4 Max, Metal).
10. **No fixed-point vs float ablation** for the GPU integer transform was run; its effect is included in reported rates but not isolated [M1d §1].

---

## 7. Related work

- **Error-bounded compressors.** SZ [Di16] and SZ3 [Liang22]; ZFP [Lindstrom14]; SPERR [Li23], building on SPECK [Pearlman04]; TTHRESH [BallesterRipoll20]; MGARD multilevel compression [Ainsworth19 — *venue to verify*]. Benchmark data: SDRBench [Zhao20].
- **Wavelets and coding.** CDF biorthogonal wavelets [Cohen92]; lifting [Sweldens96]; EBCOT code-blocks in JPEG 2000 [Taubman00], which inspired TCG3's code-blocks; set partitioning in hierarchical trees [Said96].
- **Tensor methods.** HOSVD [DeLathauwer00]; tensor trains / TT-SVD [Oseledets11]; QTT [Khoromskij11 — *venue to verify*]; optimal SVD hard thresholding [GavishDonoho14] (used as an optional rank cap in [M0]).
- **Entropy coding.** Asymmetric numeral systems / rANS [Duda13]; range coding [Martin79]; context mixing and secondary symbol estimation (APM/SSE) in PAQ [Mahoney05 — *tech report, verify*].
- **Learned compression.** Discretized logistic mixture likelihoods [Salimans17]; end-to-end learned image compression [Balle17]; learned scientific-data compressors exist (e.g. autoencoder-based) but we have not yet surveyed them carefully — *to do before submission*.
- **Integer/deterministic inference.** Integer-only quantized network inference [Jacob18].

---

## 8. Reproducibility

Commands for each stage are in [M0 §7], [M1 §7] and [M1d §7, §9, §11]. Relevant commits on `claude/tang-compress-qtt`: `e422677`, `ec0346c`, `a0cfc9c` (codecs, learned model) [M1 §6]; `4e27445`, `009bec6` (frozen held-out codec, ridge fix) [M1 §8]; `0b706b6`, `08b708c`, `c70080c` (TCG1, GPU decoder) [M1d §6]; `d2dfccd`, `c78eb54` (TCG3); `2253526`, `5972552` (learned TCG3); `930adfe`, `459b737`, `75b9892` (GPU speedups) [M1d §9–11]. No data was committed.

---

## References

Entries marked **[verify]** have uncertain venue/year/details and must be checked before circulation. Unmarked entries are, to the drafter's knowledge, correct, but were not re-fetched for this draft.

- [Ainsworth19] M. Ainsworth, O. Tugluk, B. Whitney, S. Klasky. Multilevel techniques for compression and reduction of scientific data — the univariate case. *Computing and Visualization in Science*, 2018/2019. **[verify]**
- [Balle17] J. Ballé, V. Laparra, E. P. Simoncelli. End-to-end optimized image compression. ICLR 2017.
- [BallesterRipoll20] R. Ballester-Ripoll, P. Lindstrom, R. Pajarola. TTHRESH: Tensor compression for multidimensional visual data. *IEEE TVCG* 26(9), 2020.
- [Cohen92] A. Cohen, I. Daubechies, J.-C. Feauveau. Biorthogonal bases of compactly supported wavelets. *Comm. Pure Appl. Math.* 45(5), 1992.
- [DeLathauwer00] L. De Lathauwer, B. De Moor, J. Vandewalle. A multilinear singular value decomposition. *SIAM J. Matrix Anal. Appl.* 21(4), 2000.
- [Di16] S. Di, F. Cappello. Fast error-bounded lossy HPC data compression with SZ. IPDPS 2016.
- [Duda13] J. Duda. Asymmetric numeral systems: entropy coding combining speed of Huffman coding with compression rate of arithmetic coding. arXiv:1311.2540, 2013.
- [GavishDonoho14] M. Gavish, D. L. Donoho. The optimal hard threshold for singular values is 4/√3. *IEEE Trans. Inf. Theory* 60(8), 2014.
- [Jacob18] B. Jacob et al. Quantization and training of neural networks for efficient integer-arithmetic-only inference. CVPR 2018.
- [Khoromskij11] B. N. Khoromskij. O(d log N)-quantics approximation of N-d tensors in high-dimensional numerical modeling. *Constructive Approximation* 34, 2011. **[verify]**
- [Li23] S. Li, P. Lindstrom, J. Clyne. Lossy scientific data compression with SPERR. IPDPS 2023.
- [Liang22] X. Liang, K. Zhao, S. Di, et al. SZ3: A modular framework for composing prediction-based error-bounded lossy compressors. *IEEE Trans. Big Data*, 2022/2023. **[verify year]**
- [Lindstrom14] P. Lindstrom. Fixed-rate compressed floating-point arrays. *IEEE TVCG* 20(12), 2014.
- [Mahoney05] M. Mahoney. Adaptive weighing of context models for lossless data compression. Florida Tech. Tech. Report CS-2005-16, 2005. **[verify]**
- [Martin79] G. N. N. Martin. Range encoding: an algorithm for removing redundancy from a digitised message. Video & Data Recording Conference, 1979.
- [Oseledets11] I. V. Oseledets. Tensor-train decomposition. *SIAM J. Sci. Comput.* 33(5), 2011.
- [Pavlov] I. Pavlov. LZMA SDK (range coder used in LZMA). Software, no paper; cite as software or drop. **[verify]**
- [Pearlman04] W. A. Pearlman, A. Islam, N. Nagaraj, A. Said. Efficient, low-complexity image coding with a set-partitioning embedded block coder. *IEEE TCSVT* 14(11), 2004.
- [Said96] A. Said, W. A. Pearlman. A new, fast, and efficient image codec based on set partitioning in hierarchical trees. *IEEE TCSVT* 6(3), 1996.
- [Salimans17] T. Salimans, A. Karpathy, X. Chen, D. P. Kingma. PixelCNN++: improving the PixelCNN with discretized logistic mixture likelihood and other modifications. ICLR 2017.
- [Sweldens96] W. Sweldens. The lifting scheme: a custom-design construction of biorthogonal wavelets. *Appl. Comput. Harmon. Anal.* 3(2), 1996.
- [Taubman00] D. Taubman. High performance scalable image compression with EBCOT. *IEEE Trans. Image Process.* 9(7), 2000.
- [Zhao20] K. Zhao, S. Di, X. Liang, et al. SDRBench: Scientific data reduction benchmark for lossy compressors. IEEE BigData 2020 (workshop). **[verify venue]**
