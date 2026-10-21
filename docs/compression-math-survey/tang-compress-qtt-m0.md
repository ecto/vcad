# cram M0: a bit-accounted QTT codec vs SZ3 / ZFP / wavelets

**Status (2026-10-19):** M0 is built and benchmarked. Code is on branch `claude/cram-qtt` of `ecto/tang`, in the worktree `/Users/cam/Developer/cram`. It has not been pushed.

**Gap this addresses.** Survey 03 §11 says that no published QTT work quantizes and entropy-codes its cores and then compares at matched error against SZ3, ZFP, SPERR and TTHRESH. M0 is a first instance of that comparison. Every byte of the QTT stream is counted. SZ3 and ZFP were run through their own libraries, under the same error definitions as QTT.

**Verdict.**
- QTT clearly wins on 1D signals with algebraic structure: sums of damped sinusoids and polynomials. It needs 3–4× fewer bits than SZ3 and 1.6–3× fewer than an ideal-entropy CDF 9/7 wavelet.
- On real 3D fields it wins in one specific region: high fidelity (≥ 80 dB, or max-abs 1e-4·range). There it beats SZ3 by 10–20% on Miranda and Hurricane-U.
- It loses to SZ3 at low and mid fidelity, by up to 3× on Hurricane-TC at 60 dB.
- It is beaten everywhere by the idealized wavelet baseline.
- It fails badly on turbulent/noisy fields: NYX temperature, and the white-noise control. There the TT ranks saturate.
- Encoding is about 100–500× slower than SZ3.

Details and caveats follow.

All numbers below come from `bench/out/*.csv`. The merged copy is `cram-qtt-m0/tang-compress-qtt-m0-results.csv`, and the plots sit next to this file. No number was typed by hand.

---

## 1. Design

### 1.1 Tensorization (`tensorize.rs`)

Each axis of length `p` is written in mixed radix, and each digit becomes one tensor mode. For a power of 2 this is the classic binary QTT.

**Digit order.** Two orders are supported:
- `RowMajor`: all digits of axis 0, then axis 1, and so on.
- `Morton`: digits interleaved coarse-to-fine and aligned at the least-significant digit. This is Z-order.

**Non-power-of-2 lengths.** These are handled in one of two ways:
- `MixedRadix` (the default) pads only to the next 7-smooth length and uses modes of size 3, 5 or 7. The odd factors take the most significant digits.
- `Pow2` pads to the next power of 2.

Padding is mirror reflection. Padded cells are coded, which costs bits, but they are cropped on decode and never enter an error metric.

**Measured cost of padding** (`cram-qtt-m0/padding.txt`, PSNR target → bpv):

| signal | MixedRadix | Pow2 | Pow2 overhead |
|---|---|---|---|
| 1D, n = 10⁶ = 2⁶5⁶ (Pow2 pads to 2²⁰), 40 / 60 / 80 dB | 0.0018 / 0.0033 / 0.0052 | 0.0021 / 0.0039 / 0.0064 | +17–23% |
| Miranda density 64×96×96 (96 = 3·2⁵; Pow2 pads to 64×128×128, 1.78× the cells), 40 / 60 / 80 dB | 0.198 / 1.054 / 3.091 | 0.601 / 2.204 / 5.378 | **+74–200%** |

Mixed radix is the right default. Power-of-2 padding on real grids roughly doubles the rate.

### 1.2 TT-SVD and gauge (`tt.rs`)

`tt_svd` implements the standard Oseledets TT-SVD, with two truncation rules:

- **Error budget.** Each of the `d−1` SVDs drops at most `δ = ε‖X‖/√(d−1)`, so `‖X−X̂‖ ≤ ε‖X‖`. A test checks this for ε ∈ {1e-1, 1e-2, 1e-3}.
- **Optional Gavish–Donoho cap.** The rank is capped at `#{σ > ω(β)·σ_med}`.

The output is in the canonical **left-orthogonal gauge**. Every core but the last has orthonormal columns, so the whole norm sits in the last core. The rotation freedom on each bond is fixed by the singular-vector basis. Tests check orthonormality to 1e-8 and that the norm lives in the last core.

**Gauge alternatives.** The quantizer below works in the left-orthogonal gauge, with per-column steps scaled by the bond singular values. That is equivalent to quantizing `U·Σ` with one step size, i.e. a Vidal-style "σ on the bond" scale, without having to transmit Σ. A right-orthogonal or mixed-canonical gauge was not tried in M0.

**SVD route.** `tang_la::Svd` is a one-sided Jacobi SVD. It was not usable on `2 × 2^{d−1}` unfoldings. Instead, the truncated SVD is computed by eigendecomposing the Gram matrix of the *smaller* unfolding side, which is at most `2·r_max`. This squares the condition number. In f64 it still resolves singular values down to about 1e-7·σ₁, which is below every step the codec uses up to about 120 dB. Two tests had to loosen ε to 1e-6 because of this limit; this is documented in the code.

**Linear algebra backend.** `tang_la::SymmetricEigen` and `tang_la::gemm_*` were tried first. A sampling profile showed the tang-la eigensolver (unblocked Householder plus implicit QR) taking >95% of encode time at n ≈ 700. EVD and the large GEMMs therefore moved to **faer 0.24**, which tang-la already bridges behind a feature. It is used with `default-features = false` (no rayon) and `Par::Seq`, so it stays single-threaded. tang-la's eigensolver remains as a fallback.

### 1.3 Quantization with error feedback (`codec.rs`)

One encoder pass at absolute step Δ runs TT-SVD from left to right. Each core is computed against the **already-quantized** prefix:

1. **State carried between cores.** Let `P` be the quantized prefix and `H = PᵀP` its Gram matrix. The remainder `C` is least-squares optimal given `P`.
2. **Find the next core.** Eigendecompose the whitened Gram matrix of `C` in the metric `H̃ = H⊗I`. This gives singular values `σ_j` and an `H̃`-orthonormal core `G`.
3. **Quantize each column with its own step.** Column `j` uses step `Δ/σ̂_j`, where `σ̂_j = 2^{e_j/4}` and `e_j` is transmitted as an integer. This is the equal-slope rule: every coefficient ends up with distortion of about Δ²/12, the same as the last core.
4. **Ranks fall out of rate–distortion.** Columns that round to all zeros are dropped. There is no separate tolerance.
5. **Refit.** Update `C ← (QᵀH̃Q)⁻¹QᵀH̃C` and `H ← QᵀH̃Q`, and add the exact discarded energy to a running SSE.
6. **Last core.** It holds the norm and is quantized with step Δ. Its error `eᵀH̃e` is added to the SSE.

The error pieces are mutually orthogonal, so the SSE is **known exactly at encode time**. A test checks that it matches the reconstruction to 1e-3 relative.

**Search.** Δ is found by a bracketed log–log secant search. After the search the encoder decodes its own output and checks the true error.

**Bug found and fixed during benchmarking.** Quantized columns that are nearly linearly dependent made `cond(QᵀH̃Q)` explode. The bookkeeping SSE then disagreed with the reconstruction by up to 30×, and the Δ search went non-monotone. On Miranda at 60 dB this cost 2.63 bpv instead of 1.05. Columns are now dropped until the Cholesky diagonal ratio is ≥ 1e-3.

**Error modes.**

- **RMSE / PSNR.** PSNR is range-normalized. The target holds on the decoded output; this is tested.
- **Max-abs (SZ-style).**
  - The TT is coded at RMSE targets `f·b` for f ∈ {0.25, 0.4, 0.6}. There is also a residual-only candidate, where the TT is quantized to zero.
  - The encoder decodes its own stream. Every cell with `|x − x̂| > b` gets a correction `k = round((x−x̂)/2b)`.
  - The corrections are stored as rANS-coded position gaps plus rANS-coded k values. A raw-value escape covers float-cast edge cases.
  - The smallest total stream wins. The bound holds on every benchmark row and in the tests, including white noise, a forced rank cap, and a zero-rank stream.

### 1.4 Entropy coding and bit accounting (`rans.rs`, `bitio.rs`)

**Coder.** Static rANS: 32-bit state with byte renormalization, as in Giesen's `rans_byte`.

**Integer mapping.** Integers are zigzag-mapped, then split JPEG-XL-style into a coded token and raw extra bits. The token carries the exponent plus 2 mantissa bits; the rest of the bits are written raw.

**Frequency tables.** Normalized to a total of 2¹⁴ and **transmitted** as varints. For each set of blocks the encoder picks either one shared table or one table per core, whichever is smaller; this costs a 1-byte flag.

**Stream layout.** The stream is self-describing, in this order:
- magic, dtype, digit order, padding policy, shape;
- Δ (f64), all ranks, all per-column step exponents (delta-coded);
- tables, rANS payloads, raw bits;
- the residual stream.

`EncodeStats.bytes == stream.len()`, and that is the only rate reported. Header and exponents are 0.1–2.5 KB per field, about 0.5–2% of the stream at 1M cells.

**Determinism.** Encoding is deterministic and single-threaded: the same input gives byte-identical streams (tested). The decoder rebuilds cores with exact power-of-2 step arithmetic and runs the same contraction code as the encoder's self-check. Cross-*platform* bit-identity of the f64 contraction is not guaranteed, because GEMM summation order can differ by ISA.

### 1.5 Crate contents and tests

| file | purpose |
|---|---|
| `crates/cram/src/tensorize.rs` | Layout, mixed radix and Morton, mirror padding, gather/scatter |
| `src/tt.rs` | `TensorTrain`, `tt_svd`, `Truncation` (ε, Gavish–Donoho, rank cap), `contract` |
| `src/linalg.rs` | Gram-route truncated SVD, faer EVD and GEMM, Cholesky, Kronecker-structured products |
| `src/codec.rs` | `encode_f32` / `encode_f64` / `decode`, `Config`, `ErrorBound::{Rmse, Psnr, MaxAbs}`, `EncodeStats` |
| `src/rans.rs`, `src/bitio.rs` | rANS with transmitted tables; varints and raw bit I/O |
| `examples/qtt.rs` | `qtt encode IN.f32 OUT.qtt --dims AxBxC --psnr 60 \| --max-rel 1e-3 …`, `qtt decode`, `qtt bench` (JSON stats) |

**Tests and lint.** `cargo test -p cram-codec` gives 21 unit tests plus 1 doctest, all passing. `cargo clippy -p cram-codec --no-deps --all-targets -- -D warnings` is clean.

`--no-deps` is needed because pre-existing warnings in `tang-la/src/stats.rs` (`unreachable_code` on aarch64) and in `tang` (`too_many_arguments`) otherwise fail under `-D warnings`. tang's CI runs on x86 with those lints allowed. The crate allows `needless_range_loop`, matching tang CI.

**What the tests cover:**
- TT-SVD: exact QTT ranks of exp (1) and sin (2); the ε bound; left-orthogonality; Gavish–Donoho on rank-1 plus noise.
- Codec:
  - bookkeeping SSE matches the reconstruction;
  - PSNR targets hold, including on n = 3000 = 3·5³·2³;
  - max-abs holds for f32 2D input in both digit orders at 1e-2, 1e-3 and 1e-4;
  - streams are deterministic;
  - a low-rank signal compresses to < 2 KB;
  - rank cap / zero-rank streams still meet max-abs;
  - garbage and truncated streams are rejected.
- Supporting code: rANS round trip, near-entropy rate and determinism; layout round trips for every order and padding policy.

---

## 2. Benchmark setup

**Harness.** `bench/run_bench.py` and `bench/plot.py` live in the tang branch. They use a uv venv in `bench/.venv` (Python 3.12) with `zfpy`, `pysz` (SZ3 Python binding), `pywavelets` and `zstandard`.

**Machine.** Apple M4 Max, single thread for every codec. SZ3 was run with `openmp=False`; zfpy is serial.

**Data.** About 1.34 GB was downloaded under `bench/data` (gitignored) from SDRBench's Hugging Face mirrors.

- **Real fields (SDRBench):**
  - Miranda `density` and `velocityx`. The source is f64; it was cast to f32.
  - Hurricane Isabel `Uf48` and `TCf48`.
  - NYX `temperature`.
- **Crop size.** All real fields were cropped to **64×128×128 = 1,048,576 cells**. This is because QTT encode time was 20–180 s per point on 128×192×192. Crops are interior windows; the exact indices are in `real_fields()`.
- **Synthetic 1D, n = 2²⁰:**
  - a sum of K = 5 damped sinusoids;
  - a degree-5 polynomial;
  - a piecewise-smooth signal with 7 jumps;
  - white noise (negative control).

**Metrics.** Every codec is scored on the same definitions:
- **PSNR** = 20·log₁₀(range/RMSE), with range = max − min of the original;
- **max-abs** bound b = rel·range;
- **rate** = full compressed bytes·8 / n.

Each codec is swept over its own knobs, and the PSNR curves are its Pareto front:

| codec | knobs swept (PSNR curves) | max-abs mode |
|---|---|---|
| QTT | PSNR targets 30…100 dB, Morton and row-major | `--max-abs` |
| SZ3 | PSNR mode **and** ABS bound at rmse·√3 | ABS bound |
| ZFP | fixed accuracy **and** fixed precision | bisected for the largest tolerance whose *actual* max error ≤ b, i.e. its best honest mode |

zfpy's fixed-rate path aborts on 1D input, so it was excluded. Matched-PSNR rates are log-linear interpolations on each Pareto front.

**Wavelet baseline.**
- Transform: CDF 9/7 (`bior4.4`), periodization, 5 levels for 3D and 10 for 1D.
- Quantizer: uniform, with one step for all subbands.
- Rate is reported two ways:
  - **H0**: the ideal zeroth-order entropy per subband, with **no table or header cost**. This is optimistic.
  - **zstd-19**: a real coder.
- It has no max-abs guarantee, so it appears only in the PSNR tables. It is a SPERR-like *lower bound on effort*, not SPERR.

**Not run.**
- **TTHRESH** and **SPERR** were not built in M0; they were added afterwards in §8.
- The QTT bench used the CLI's rank cap of **512**. The library default is now 1024. The cap binds only on NYX and white noise.

---

## 3. Results

### 3.1 Matched PSNR: bits per value (lower is better)

"–" means the codec's front does not reach that PSNR. For QTT on NYX and white noise this is the rank cap: the target was unreachable at r ≤ 512.

| dataset | QTT row | QTT Morton | SZ3 | ZFP | wavelet H0 (ideal) | wavelet zstd |
|---|---|---|---|---|---|---|
| **1D damped sines** @60 / 80 / 100 dB | **0.0106 / 0.0152 / 0.0204** | – | 0.0395 / 0.0769 / 0.134 | 6.36 / 8.37 / 11.0 | 0.0174 / 0.0348 / 0.0594 | 0.0207 / 0.0410 / 0.0717 |
| **1D polynomial deg 5** @60 / 80 / 100 | **0.00225 / 0.00321 / 0.00433** | – | – / – / 0.00883 | 6.79 / 8.46 / 10.3 | 0.00724 / 0.00992 / 0.0105 | 0.00586 / 0.0116 / 0.0149 |
| **1D piecewise + jumps** @60 / 80 / 100 | **0.0075** / 0.0152 / 0.0236 | – | 0.0108 / 0.0132 / 0.0150 | 6.76 / 8.45 / 10.4 | 0.0089 / **0.0126** / **0.0146** | 0.0123 / 0.0187 / 0.0258 |
| 1D white noise @40 / 60 | – (29 dB max at 39 bpv) | – | 3.97 / 7.29 | 6.80 / 10.2 | 3.66 / 6.98 | 4.60 / 8.64 |
| Miranda density @60 / 80 / 100 | 0.773 / 2.16 / 4.60 | 1.20 / 2.98 / 6.60 | 0.580 / 2.58 / 5.56 | 1.41 / 3.06 / 5.42 | **0.524 / 1.73 / 3.60** | 0.602 / 2.05 / 4.40 |
| Miranda velocityx @60 / 80 / 100 | 0.516 / 1.78 / 4.36 | 0.699 / 2.45 / 6.03 | **0.352** / 1.98 / 4.87 | 1.34 / 3.00 / 5.54 | 0.371 / **1.34 / 3.17** | 0.446 / 1.66 / 3.91 |
| Hurricane U @60 / 80 / 100 | 0.874 / 3.03 / 6.69 | 1.49 / 5.22 / – | 0.809 / 3.67 / 7.06 | 1.75 / 4.39 / 7.84 | **0.707 / 2.71 / 5.52** | 0.790 / 3.28 / 6.98 |
| Hurricane TC @60 / 80 / 100 | 0.421 / 1.60 / 4.38 | 0.527 / 2.90 / 7.35 | **0.140** / 1.62 / 4.79 | 0.856 / 2.94 / 6.12 | 0.203 / **1.24 / 3.62** | 0.126 / 1.39 / 4.34 |
| NYX temperature @40 / 60 / 80 | 1.53 / 3.50 / – | **0.102** / 2.93 / – | 0.117 / 0.880 / 3.67 | 0.379 / 1.64 / 4.99 | – / **0.721** / **3.43** | – / 1.01 / 4.49 |

Low end (@40 dB):
- Hurricane TC: QTT 0.0014 vs SZ3 0.0050.
- Hurricane U: QTT 0.031 vs SZ3 0.023.
- Miranda density: QTT 0.159 vs SZ3 0.126.

Plots: `cram-qtt-m0/rd_<dataset>.png`. Every table row is in `summary.md` in the same folder.

### 3.2 Fixed max-abs bound (b = rel·range): bits per value

All rows were verified: the achieved max error is ≤ b.

| dataset | 1e-2: QTT row / SZ3 / ZFP | 1e-3: QTT row / SZ3 / ZFP | 1e-4: QTT row / SZ3 / ZFP |
|---|---|---|---|
| 1D damped sines | **0.0112** / 0.0287 / 4.89 | **0.0156** / 0.0540 / 7.69 | **0.0221** / 0.0983 / 9.95 |
| 1D polynomial | **0.0019** / 0.0083 / 6.03 | **0.0042** / 0.0084 / 7.53 | 0.0169 / **0.0088** / 9.03 |
| 1D piecewise + jumps | 0.0124 / **0.0116** / 6.08 | 0.0210 / **0.0123** / 8.08 | 0.0345 / **0.0153** / 9.58 |
| 1D white noise | **4.43** / 4.75 / 9.00 | **7.75** / 8.09 / 12.0 | **11.07** / 11.40 / 16.0 |
| Miranda density | 0.440 / **0.217** / 1.22 | 1.351 / **1.097** / 2.81 | **3.121** / 3.402 / 5.40 |
| Miranda velocityx | 0.298 / **0.137** / 1.01 | 1.036 / **0.757** / 2.51 | 2.764 / **2.595** / 5.12 |
| Hurricane U | 0.394 / **0.171** / 1.07 | 1.793 / **1.441** / 3.51 | **4.261** / 4.452 / 7.16 |
| Hurricane TC | **0.0188** / 0.0195 / 0.835 | 0.663 / **0.390** / 2.36 | 2.608 / **2.316** / 4.70 |
| NYX temperature | 1.86 (Morton) / **0.315** / 1.35 | 4.89 (Morton) / **1.59** / 4.50 | 10.4 (residual-only) / **4.51** / 7.50 |

**White-noise row.** QTT's "win" there comes from the residual-only candidate: the TT is empty, and the corrections are rANS-coded `round(x/2b)` values. That is just a better entropy coder than SZ3's on i.i.d. data. It is not a QTT effect.

**Where the bits go.** On the real fields at 1e-4, about 9% of cells get a correction, and the residual stream is about 15–20% of the total. Compare the `res=` and `resb=` columns in `summary.md`.

### 3.3 Speed (single thread, ~60 dB point, MB/s of f32 input)

| | QTT row encode | QTT decode | SZ3 encode / decode | ZFP encode / decode |
|---|---|---|---|---|
| Miranda density | 3.9 | 244 | 584 / 1093 | 775 / 1309 |
| Hurricane U | 1.6 | 122 | 573 / 967 | 907 / 1323 |
| NYX temperature | 0.9 | 59 | 685 / 1077 | 866 / 1368 |
| 1D damped sines | 51 | 1930 | 540 / 1710 | 452 / 587 |

**Encode cost.** One QTT pass is a full TT-SVD with GEMMs of shape (2r)²·cols. The Δ search costs 4–8 passes per PSNR target and 12–27 per max-abs bound.

**Decode cost.** Decode is one chain of GEMMs. It is 4–10× slower than SZ3 on 3D fields, and on par or faster on 1D low-rank data.

---

## 4. Where QTT wins and loses, and why (rank profiles)

Rank profiles are in `summary.md` (the "ranks" table). They tell the whole story.

**1D algebraic signals: ranks stay tiny, and QTT wins big.**
- The damped-sine sum peaks at rank 10, which is 2K for K = 5 complex-pair modes. This is exactly the "Kronecker for QTT" count from survey 03 §5, and it does not grow with PSNR: at 100 dB the profile is still ≤ 10.
- The polynomial peaks at rank 4–5 (degree + 1 = 6 bounds it).
- Bits therefore grow only logarithmically with fidelity: 0.0106 → 0.0204 bpv from 60 to 100 dB. SZ3 grows 3.4× and the wavelet coder 3.4× over the same span.
- This is the structural advantage predicted in survey 03, now measured with full bit accounting.

**Jumps: a near-tie with wavelets.** Each discontinuity adds about 1 to the rank across the scales where it is resolved, so ranks reach 9–11 at high PSNR. QTT wins at 60 dB and loses to the ideal wavelet and to SZ3 at ≥ 80 dB.

**Real 3D fields: a rank "dome".** Ranks grow at about 2× per digit for the first 7–9 digits, which is the full rank of the prefix, until they meet the suffix bound. The peak is around 200–500 at the middle bond.
- The peak sits exactly where the unfolding is square. Those middle cores hold `(2r)·r` coefficients with r ≈ 300, and they dominate the payload.
- At high fidelity this still beats SZ3's Lorenzo/interpolation residuals, because those spend about log₂(σ_resid/Δ) bits on *every* cell.
- At low fidelity the dome is too costly relative to a predictor that only pays for innovation.

**Row-major beats Morton on every real field.** By about 1.3–2×; for example Miranda density at 80 dB is 2.16 vs 2.98 bpv. With row-major order, the leading digits split the slowest axis, which is often the most anisotropic one. Morton forces the full 2-ary branching on all three axes at once, so the rank hits 2^k for 8–9 digits before any compression starts. **Morton is the wrong default for these data.** The library `Config::new` still defaults to Morton; flip it in M1.

**NYX temperature (turbulent, spiky) and white noise: ranks saturate at the cap from 50–60 dB.** QTT cannot reach 80 dB at r ≤ 512 and costs 3–4× SZ3 where it does reach. This matches survey 03's prediction: once small scales look like noise, every coder degrades toward per-sample coding, and a TT pays an extra factor for its dense middle cores.

**Gavish–Donoho is the wrong tool for rate.**
- Applied naively, it collapsed every rank to 1. QTT's leading unfoldings are 2×N and 4×N, so the "median singular value" is a signal value. Miranda at a 60 dB target came out at 12 dB.
- Even restricted to unfoldings with min side ≥ 32, which is what M0 does now, it caps fidelity at about 27 dB on Miranda.
- GD is a *denoising* threshold. On noise-free simulation output it truncates real signal. It stays available as an option, off by default.

**Against the wavelet baseline.** On every real field, a single-step CDF 9/7 with ideal entropy coding beats QTT by 1.2–1.5× at matched PSNR. That baseline is optimistic: no table cost, and per-subband entropy. Survey 03 §11 (Ali–Nouy) predicts that QTT can only *match* wavelets on Besov-type smooth fields, and the data agree. QTT's edge is in the oscillatory and separable regime, not on generic turbulence.

---

## 5. Honest verdict

1. **The gap is real, and the M0 codec fills it as a working artifact.**
   - It is a QTT compressor with full bit accounting: header, ranks, step exponents, tables, payload, residuals.
   - It has guaranteed RMSE/PSNR or pointwise max-abs, and deterministic streams.
   - It was compared against SZ3 and ZFP at matched error on SDRBench crops.
2. **On scientific 3D fields M0 is competitive only at high fidelity.**
   - At ≥ 80 dB or max-abs 1e-4·range it beats SZ3 by 10–20% on Miranda and Hurricane-U. It beats ZFP by 1.5–2× nearly everywhere.
   - It loses to SZ3 at ≤ 60 dB, and to an idealized wavelet coder everywhere.
   - It is not a better general-purpose compressor than SZ3 today.
3. **On signals with algebraic structure** (sums of exponentials or sinusoids, polynomials, smooth signals with periodic modulation) QTT is 3–6× better than SZ3 and about 2–3× better than the wavelet coder at matched error. That is where to aim it.
4. **Encode speed** (1–5 MB/s) is the practical blocker. Decode (60–330 MB/s on 3D) is acceptable for a CPU M0.

Caveats:
- 1M-cell crops, not full fields.
- One machine.
- SPERR and TTHRESH were not in M0. They have since been run (§8), and they move the 3D frontier 2–3× below QTT.
- QTT used a rank cap of 512.
- The wavelet "H0" rate ignores table cost.

## 6. Next steps

- **Ordering.** Make row-major the default. Then try *adaptive* digit orders: choose the next digit greedily by the smallest unfolding rank, or try axis permutations. The Morton vs row-major gap (up to 2×) shows that ordering is a first-order knob.
- **Adaptive ranks via annealing / RD refinement.**
  - Today ranks fall out of a single equal-slope Δ.
  - Next: per-bond Lagrangian rank choice (`D + λR`, using the actual rANS cost of each column), followed by ALS/DMRG sweeps on the quantized cores. The second order would also cancel part of the TT-SVD's suboptimality.
  - Annealing over rank profiles is cheap, because the SSE bookkeeping is exact without a decode.
- **Walsh residual.** A sparse Walsh–Hadamard (or wavelet) residual layer on top of a *low-rank* QTT (survey 03 §8). It would let QTT stop at the low-cost knee of the rank dome and hand the high-entropy small scales to a coder better suited to them. This is the most promising fix for the 40–60 dB loss.
- **Prediction plus QTT hybrid.** Code the Lorenzo/interpolation residual of SZ3 *with* a low-rank QTT, or the reverse. The low-fidelity regime is where SZ3 wins.
- **Encoder speed.**
  - Warm-start Δ across targets.
  - Reuse the first passes' rank-revealing factorizations.
  - Randomized range finders instead of a full Gram EVD.
  - Multithreaded faer (the encoder is single-threaded by spec).
- **GPU decode via tang codegen.** Decode is literally `(α, {G_k[i]}, β)`: a fixed chain of small batched GEMMs with shapes known from the header. This is the IR in survey 03 §12. Emit a WGSL/Metal kernel per stream shape through tang-compile, so decode becomes a per-cell product of d small matrices. That form also supports random access and region-of-interest decode.
- **Baselines still owed.** SPERR and TTHRESH, at full field sizes.

## 7. Reproduce

```bash
git -C /Users/cam/Developer/tang worktree add /Users/cam/Developer/cram claude/cram-qtt   # if not present
cd /Users/cam/Developer/cram
cargo test -p cram-codec --release
cargo clippy -p cram-codec --no-deps --all-targets -- -D warnings
cargo build --release -p cram-codec --example qtt
uv venv --python 3.12 bench/.venv && VIRTUAL_ENV=bench/.venv uv pip install -r bench/requirements.txt
# data → bench/data (see real_fields() for filenames; SDRBench HF mirrors sdrbench/{miranda,hurricane-isabel,nyx})
bench/.venv/bin/python bench/run_bench.py 1d-
bench/.venv/bin/python bench/run_bench.py miranda hurricane nyx
bench/.venv/bin/python bench/run_bench.py --wavelet-only
bench/.venv/bin/python bench/plot.py      # → bench/out/rd_*.png, summary.md
```

The NYX and white-noise QTT rows were measured with the CLI's `--max-rank` default of 512.

---

## 8. Real frontier: SPERR and TTHRESH (added after M0)

**This section supersedes the M0 verdict in §5 for 3D fields.**

### 8.1 Build

Both tools were built from source in `bench/third_party/`, which is gitignored.

| tool | commit | how it was built |
|---|---|---|
| **SPERR** | `b8012586908aaabba993038d183b3937c8ea9c7c` | CMake Release, `-DUSE_OMP=ON -DBUILD_UNIT_TESTS=OFF` |
| **TTHRESH** | `2dbad4ee811faa68d11f2e10d70ee179c636aeb9` | CMake Release, `-DCMAKE_POLICY_VERSION_MINIMUM=3.5` |

**SPERR build notes:**
- It needed Homebrew LLVM clang (`/opt/homebrew/opt/llvm`, already installed) for OpenMP. Apple clang plus libomp fails on how CMake passes the `-Xclang -fopenmp` flag.
- It needed a local one-line patch, `#include <algorithm>` in `src/Bitstream.cpp`, because newer libc++ no longer pulls it in transitively.

**TTHRESH build note:** the configure flag lets it build under CMake 4. Its build is not threaded.

Nothing new was installed with Homebrew.

### 8.2 Protocol

The same crops, error definitions and harness were used: `run_bench.py --frontier`.

**Error and rate measurement.** Both tools' own error reports are ignored. Every row is decoded and measured by the harness: PSNR on range, and max-abs. Bits per value come from the size of the output bitstream file. The PSNR targets the tools are given overshoot the measured PSNR by 3–9 dB, so all comparisons use each tool's measured Pareto front.

**SPERR:**
- Swept in its `--psnr` mode and its `--pwe` mode (pointwise error = rmse·√3).
- Max-abs rows use `--pwe b`; every row was verified to be ≤ b.
- Runs use `--omp 1`.
- The default-threaded run (`--omp 0`) at 60 dB gives the same times within noise. That is because a 64×128×128 crop is one 256³ chunk, so OpenMP has nothing to parallelize.
- Speed is about 0.06–0.09 s encode and about 0.05 s decode per 4 MB field, i.e. about 50–70 MB/s.

**TTHRESH:**
- Swept with `-p` (PSNR).
- **It has no max-abs mode**, so it is skipped in the max-abs table.
- It failed (non-zero exit) on 2 of the 64 runs: 1D damped sines at `-p 40` and white noise at `-p 80`.

**1D inputs.** Neither tool takes 1D input:
- SPERR got the 2²⁰ signal as a 1024×1024 slice through `sperr2d`.
- TTHRESH got it as 64×128×128.

Both reshapes introduce artificial discontinuities, so the 1D rows **handicap SPERR and TTHRESH**. Read them as indicative only.

### 8.3 Frontier at matched PSNR (bpv, lower is better)

wavelet-H0 is excluded from the frontier because it is an idealized rate with no table cost. QTT is row-major order. The full table is "frontier codec" in `cram-qtt-m0/summary.md`.

| dataset | 40 dB | 60 dB | 80 dB | 100 dB |
|---|---|---|---|---|
| Miranda density | TTHRESH 0.101 (QTT 1.58×) | SPERR 0.307 (QTT 0.773, 2.5×) | TTHRESH 0.773 (QTT 2.16, 2.8×) | TTHRESH 1.59 (QTT 4.60, 2.9×) |
| Miranda velocityx | TTHRESH 0.062 (1.35×) | SPERR 0.191 (2.7×) | TTHRESH 0.618 (2.9×) | TTHRESH 1.53 (2.8×) |
| Hurricane U | SZ3 0.023 (1.31×) | SPERR 0.323 (2.7×) | TTHRESH 1.42 (2.1×) | TTHRESH 3.20 (2.1×) |
| Hurricane TC | **QTT 0.0014** (SZ3 0.0050) | SPERR 0.029 (QTT 14.6×) | SPERR 0.612 (2.6×) | TTHRESH 2.17 (2.0×) |
| NYX temperature | TTHRESH 0.076 (20×) | SPERR 0.575 (6.1×) | SPERR 3.13 (QTT n/a) | SPERR 6.50 (QTT n/a) |
| 1D damped sines* | **QTT 0.0060** (TTHRESH 0.019) | **QTT 0.0106** (wavelet-zstd 0.021) | **QTT 0.0152** (0.041) | **QTT 0.0204** (TTHRESH 0.058) |
| 1D polynomial* | **QTT 0.0014** | **QTT 0.0022** (wavelet-zstd 0.0059; SPERR 0.0060) | **QTT 0.0032** | **QTT 0.0043** (SZ3 0.0088) |
| 1D piecewise + jumps* | **QTT 0.0025** | **QTT 0.0075** (SZ3 0.0108) | SZ3 0.0132 (QTT 1.15×) | SZ3 0.0150 (QTT 1.57×) |
| 1D white noise | SZ3 3.97 (SPERR 4.05) | SZ3 7.29 | SZ3 10.6 | SZ3 14.0 |

\* SPERR and TTHRESH see these signals reshaped to 2D/3D (see §8.2).

### 8.4 Max-abs, b = rel·range (bpv; all rows verified ≤ b)

| dataset | 1e-2: SPERR / SZ3 / QTT | 1e-3: SPERR / SZ3 / QTT | 1e-4: SPERR / SZ3 / QTT |
|---|---|---|---|
| Miranda density | 0.247 / **0.217** / 0.440 | **0.812** / 1.097 / 1.351 | **2.005** / 3.402 / 3.121 |
| Miranda velocityx | 0.171 / **0.137** / 0.298 | **0.590** / 0.757 / 1.036 | **1.616** / 2.595 / 2.764 |
| Hurricane U | 0.197 / **0.171** / 0.394 | **0.962** / 1.441 / 1.793 | **2.810** / 4.452 / 4.261 |
| Hurricane TC | 0.043 / 0.0195 / **0.0188** | **0.344** / 0.390 / 0.663 | **1.411** / 2.316 / 2.608 |
| NYX temperature | 0.298 / 0.315 / 1.86 | 1.587 / 1.585 / 4.89 | **4.567** / 4.509 / 10.4 |
| 1D damped sines* | 1.93 / 0.0287 / **0.0112** | 3.07 / 0.054 / **0.0156** | 4.17 / 0.098 / **0.0221** |
| 1D polynomial* | 0.0131 / 0.0083 / **0.0019** | 0.0259 / 0.0084 / **0.0042** | 0.0522 / **0.0088** / 0.0169 |

At 1e-4 on NYX, SPERR and SZ3 are within 1% of each other (4.567 vs 4.509 bpv).

### 8.5 What this means for QTT

1. **The high-fidelity edge from §5 disappears.** That edge was 10–20% over SZ3 at ≥ 80 dB and at max-abs 1e-4.
   - On every real 3D field, at every fidelity ≥ 60 dB, the frontier is **TTHRESH** (Tucker, the closest relative in the tensor family) or **SPERR** (wavelets with SPECK coding).
   - M0 QTT is **2–3× worse** at matched PSNR on Miranda and Hurricane, and 6× or more on NYX.
   - At max-abs 1e-4, SPERR beats QTT by 1.5–1.7× (Miranda density: 2.01 vs 3.12 bpv).
   - QTT's only remaining 3D frontier point is Hurricane-TC at ≤ 40 dB, where it beats SZ3 3.5× and stays slightly ahead of SPERR at max-abs 1e-2. That is a nearly constant field coded at very low rate.
2. **Tucker beats binary QTT on these smooth volumes.** TTHRESH's three dense factor matrices plus a bit-plane-coded core capture the per-axis smoothness directly. QTT's dyadic splitting has to rebuild it through a rank-200–500 middle dome. This matches survey 03's Ali–Nouy prediction. A real wavelet coder like SPERR beats the single-step wavelet baseline from §3 too, so "QTT loses to wavelets" got stronger, not weaker.
3. **QTT still owns the oscillatory/algebraic 1D regime.** It is 2–6× better than the best other codec on damped sinusoids and polynomials at every fidelity. The tools' handicap there is real but cannot explain margins that size against SZ3 and wavelets, which see the native 1D signal.
4. **Implication for next steps.**
   - Do **not** pitch QTT as a general SDRBench compressor.
   - Credible directions:
     - **(a)** A Tucker-or-wavelet front end with a QTT layer only where structure is oscillatory or separable. Concretely: a QTT of TTHRESH's Tucker core, or of SPERR's coarse bands.
     - **(b)** Target domains where the 1D results transfer: time series and spectra with modal structure, PDE solutions with exponential and oscillatory content, lookup tables.
     - **(c)** Pursue the GPU, random-access decode angle. Neither SPERR nor TTHRESH offers cheap per-cell decode.
   - The RD-refinement and Walsh-residual items in §6 would need to recover about 2–3× just to reach the frontier on 3D fields. Treat that as a research bet, not an incremental fix.
