# cram portfolio codec (Track E, phase 1)

Branch `claude/tc-portfolio-716321`. **Frozen code: `6b00aff`** (committed before any held-out run). Harness: `bench/run_portfolio.py`, `bench/summarize_portfolio.py`. External-codec rows are reused unchanged from the M1 held-out run (`bench/out/heldout_*.csv` from the `claude/cram-qtt` worktree, copied to `bench/out/ext/`; QTT there is row-major, the same codec the portfolio calls).

## What changed

`encode_auto_f32` / `tc --codec auto` is now a portfolio (`encode_portfolio_f32`). Same 1-byte tag, new values: `0xA0` wavelet, `0xA1` Tucker, `0xA2` QTT, `0xA3` interp. `decode_any` dispatches on the tag + magic.

| candidate | when | notes |
|---|---|---|
| wavelet (configured model; overfit in these runs) | always | unchanged |
| Tucker | PSNR/RMSE, ≥2D | unchanged |
| **interp** (new, `src/interp.rs`) | max-abs | SZ3-style multilevel interpolation: per-level linear/cubic choice by trial cost, level-wise bound `b/min(α^(ℓ−1), β)` with 4 `(α, β)` presets picked by real stream size, error-bounded quantization checked on the exact f32 reconstruction (escapes stored raw), adaptive range coding with (level, log₂ neighbour-difference, previous-symbol) contexts. Median encode 0.13 s per 1M values. |
| **QTT** row-major | any bound, **only if the pre-screen passes** | `codec::encode_f32` with `DigitOrder::RowMajor` |

**Pre-screen** (no encodes): `peakiness = range / (q99 − q01)` on a ≤65k strided sample; QTT runs only when peakiness ≥ 8. Validated on the 5 original fields only (`bench/out/portfolio_dev.csv`):

- peakiness 1.0–1.7 (Miranda, Hurricane), 4.2 (NYX temperature);
- QTT never wins on any original point (it is 1.1–15× larger than the winner) and costs 0.1–58 s per encode (58 s on NYX at 1e-2), so skipping it there is free in rate;
- the threshold 8 was set at ~2× the most heavy-tailed original field.

Dev check on the originals, max-abs, interp vs SZ3: interp is smaller at every point (e.g. Hurricane TC 1e-2: 0.0112 vs 0.0195 bpv; Miranda density 1e-4: 2.87 vs 3.40). It wins outright in the portfolio only at Hurricane TC 1e-2.

## Held-out gate (14 fields, frozen `6b00aff`)

Decoder-side measured error, real file sizes (tag byte included). PSNR points are interpolated on each codec's Pareto front, as in M1 §8.

**Gate: FAIL at 1 of 97 points.** The failing point is qmcpack-orb0 at max 1e-4: 0.3346 bpv vs QTT's 0.2909 (1.150×). The pre-screen skipped QTT there (peakiness 1.4). Post-hoc diagnostic, not part of the gate: with the screen off (`portfolio-all`), that point picks QTT at 0.2909 (1.000×), so the screen is the only reason for the miss.

The peaky-data hypothesis was half right:

- EXAFEL (peakiness 48) was caught, and both of its QTT-won points went to QTT;
- QTT also wins on QMCPACK orbital 0, which is smooth and low-TT-rank rather than peaky.

Peakiness is the wrong signal for that second case.

| point | geo-mean tc-portfolio / best external | M1 tc-auto |
|---|---|---|
| 40 dB | 0.429 (n=13) | 0.429 |
| 60 dB | 0.775 | 0.775 |
| 80 dB | 0.737 | 0.737 |
| 100 dB | 0.808 | 0.808 |
| max 1e-2 | **0.677** | 0.810 |
| max 1e-3 | **0.811** | 0.843 |
| max 1e-4 | **0.871** | 0.876 |

Win/loss/tie (±1%) vs best external:

- PSNR: 54–1–0, unchanged; the loss is Hurricane-V 60 dB at 1.014×, which is inside the gate.
- max-abs: **38–1–3**, against M1's 34–8.

The 8 former max-abs losses:

| field / point | M1 tc-auto | portfolio | pick |
|---|---|---|---|
| scale-PRES max 1e-2 | 1.489× | **0.749×** | interp |
| qmcpack-orb100 max 1e-2 | 1.211× | **0.770×** | interp |
| qmcpack-orb0 max 1e-2 | 1.113× | **0.654×** | interp |
| scale-T max 1e-2 | 1.016× | **0.949×** | interp |
| exafel-ev0 max 1e-3 | 1.313× | **1.000×** | QTT |
| exafel-ev0 max 1e-4 | 1.082× | **1.000×** | QTT |
| qmcpack-orb0 max 1e-3 | 1.077× | **0.998×** | interp |
| qmcpack-orb0 max 1e-4 | 1.150× | 1.150× **FAIL** | wavelet (QTT screened out) |

Picks: max-abs: interp 14, QTT 2, wavelet 26. PSNR: Tucker 62, wavelet 48, QTT 2 (EXAFEL 30 dB).

## Encode-time cost

Per encode, held-out, loaded machine.

| | median | mean |
|---|---|---|
| M1 tc-auto, max-abs | 1.55 s | 1.58 s |
| portfolio, max-abs | 1.73 s | 3.83 s |
| M1 tc-auto, PSNR | 2.02 s | 4.29 s |
| portfolio, PSNR | 2.12 s | 5.23 s |

Per candidate: wavelet median 1.42 s, Tucker 0.59 s, interp 0.13 s (max 0.67 s), QTT 5.6 s median and 11.2 s mean (max 48 s, 11 encodes, all on EXAFEL). The interp candidate adds about 8% to a max-abs encode. The mean increase is almost entirely QTT on EXAFEL. Without the screen, QTT would run on every field, at up to 58 s per encode on NYX-like data.

## Follow-ups

- Replace peakiness with a screen that predicts QTT's rate. Options are a TT-rank probe (one Gram eigen-decomposition of the first row-major unfolding, or QTT on a 2× decimated copy), or simply "always try QTT when n·(expected rank) is small". On qmcpack-orb0 QTT costs only 0.35 s, so a cost-based rule ("run QTT if its predicted time < wavelet's") would have caught the miss. Re-validate on the originals, then re-freeze. The held-out set is now seen for this decision, so a fresh held-out set is needed for an honest re-gate.
- interp at PSNR targets. SZ3 does not win PSNR points, so this is low priority.

## Full per-point table

| field | point | best external | bpv | tc-auto (M1) | ratio | tc-portfolio | ratio | pick |
|---|---|---|---|---|---|---|---|---|
| cesm-CLDTOT | 40dB | sz3 | 0.1017 | 0.04556 | 0.448 | 0.04556 | 0.448 |  |
| cesm-CLDTOT | 60dB | sperr | 1.408 | 1.19 | 0.845 | 1.19 | 0.845 |  |
| cesm-CLDTOT | 80dB | sperr | 4.576 | 4.064 | 0.888 | 4.064 | 0.888 |  |
| cesm-CLDTOT | 100dB | sperr | 7.903 | 7.656 | 0.969 | 7.656 | 0.969 |  |
| cesm-CLDTOT | max0.01 | sz3 | 0.4818 | 0.3809 | 0.791 | 0.3782 | 0.785 | interp |
| cesm-CLDTOT | max0.001 | sz3 | 2.45 | 2.263 | 0.924 | 2.24 | 0.914 | interp |
| cesm-CLDTOT | max0.0001 | sz3 | 5.754 | 5.262 | 0.915 | 5.262 | 0.915 | wavelet |
| cesm-FLDS | 40dB | sz3 | 0.01215 | 0.008722 | 0.718 | 0.008722 | 0.718 |  |
| cesm-FLDS | 60dB | sperr | 0.09943 | 0.09055 | 0.911 | 0.09055 | 0.911 |  |
| cesm-FLDS | 80dB | sperr | 1.572 | 1.319 | 0.839 | 1.319 | 0.839 |  |
| cesm-FLDS | 100dB | sperr | 4.73 | 4.27 | 0.903 | 4.27 | 0.903 |  |
| cesm-FLDS | max0.01 | sz3 | 0.05589 | 0.04033 | 0.722 | 0.03299 | 0.590 | interp |
| cesm-FLDS | max0.001 | sz3 | 0.5677 | 0.4379 | 0.771 | 0.4379 | 0.771 | wavelet |
| cesm-FLDS | max0.0001 | sz3 | 2.662 | 2.435 | 0.915 | 2.422 | 0.910 | interp |
| cesm-LHFLX | 40dB | sz3 | 0.06862 | 0.01863 | 0.271 | 0.01863 | 0.271 |  |
| cesm-LHFLX | 60dB | sperr | 0.2076 | 0.1784 | 0.859 | 0.1784 | 0.859 |  |
| cesm-LHFLX | 80dB | sperr | 1.547 | 1.278 | 0.826 | 1.278 | 0.826 |  |
| cesm-LHFLX | 100dB | sperr | 4.499 | 4.06 | 0.903 | 4.06 | 0.903 |  |
| cesm-LHFLX | max0.01 | sz3 | 0.1329 | 0.1117 | 0.840 | 0.09692 | 0.729 | interp |
| cesm-LHFLX | max0.001 | sperr | 0.7516 | 0.5493 | 0.731 | 0.5493 | 0.731 | wavelet |
| cesm-LHFLX | max0.0001 | sperr | 2.787 | 2.329 | 0.836 | 2.329 | 0.836 | wavelet |
| cesm-TS | 40dB | sz3 | 0.01073 | 0.008514 | 0.794 | 0.008514 | 0.794 |  |
| cesm-TS | 60dB | sperr | 0.04098 | 0.03946 | 0.963 | 0.03946 | 0.963 |  |
| cesm-TS | 80dB | sperr | 0.3342 | 0.277 | 0.829 | 0.277 | 0.829 |  |
| cesm-TS | 100dB | sperr | 1.592 | 1.292 | 0.812 | 1.292 | 0.812 |  |
| cesm-TS | max0.01 | sz3 | 0.05013 | 0.04461 | 0.890 | 0.03007 | 0.600 | interp |
| cesm-TS | max0.001 | sz3 | 0.2293 | 0.2012 | 0.878 | 0.1797 | 0.784 | interp |
| cesm-TS | max0.0001 | sz3 | 0.8866 | 0.6843 | 0.772 | 0.6843 | 0.772 | wavelet |
| exafel-ev0 | 60dB | sz3 | 2.606 | 2.28 | 0.875 | 2.28 | 0.875 |  |
| exafel-ev0 | 80dB | sz3 | 5.824 | 5.627 | 0.966 | 5.627 | 0.966 |  |
| exafel-ev0 | 100dB | sz3 | 9.196 | 8.97 | 0.975 | 8.97 | 0.975 |  |
| exafel-ev0 | max0.01 | qtt | 0.4859 | 0.2083 | 0.429 | 0.2083 | 0.429 | wavelet |
| exafel-ev0 | max0.001 | qtt | 2.624 | 3.446 | 1.313 | 2.624 | 1.000 | qtt |
| exafel-ev0 | max0.0001 | qtt | 6.273 | 6.79 | 1.082 | 6.273 | 1.000 | qtt |
| qmcpack-orb0 | 40dB | qtt | 0.008399 | 0.004281 | 0.510 | 0.004281 | 0.510 |  |
| qmcpack-orb0 | 60dB | qtt | 0.03877 | 0.01553 | 0.401 | 0.01553 | 0.401 |  |
| qmcpack-orb0 | 80dB | tthresh | 0.1102 | 0.04545 | 0.413 | 0.04545 | 0.413 |  |
| qmcpack-orb0 | 100dB | tthresh | 0.2286 | 0.1218 | 0.533 | 0.1218 | 0.533 |  |
| qmcpack-orb0 | max0.01 | sz3 | 0.0556 | 0.06188 | 1.113 | 0.03634 | 0.654 | interp |
| qmcpack-orb0 | max0.001 | qtt | 0.1302 | 0.1402 | 1.077 | 0.1299 | 0.998 | interp |
| qmcpack-orb0 | max0.0001 | qtt | 0.2909 | 0.3346 | 1.150 | 0.3346 | 1.150 **FAIL** | wavelet |
| qmcpack-orb100 | 40dB | sz3 | 0.02619 | 0.0134 | 0.512 | 0.0134 | 0.512 |  |
| qmcpack-orb100 | 60dB | sperr | 0.0835 | 0.0648 | 0.776 | 0.0648 | 0.776 |  |
| qmcpack-orb100 | 80dB | tthresh | 0.304 | 0.1896 | 0.624 | 0.1896 | 0.624 |  |
| qmcpack-orb100 | 100dB | tthresh | 0.6353 | 0.4855 | 0.764 | 0.4855 | 0.764 |  |
| qmcpack-orb100 | max0.01 | sz3 | 0.05058 | 0.06128 | 1.211 | 0.03895 | 0.770 | interp |
| qmcpack-orb100 | max0.001 | sz3 | 0.2444 | 0.1954 | 0.800 | 0.1838 | 0.752 | interp |
| qmcpack-orb100 | max0.0001 | qtt | 0.8216 | 0.5178 | 0.630 | 0.5178 | 0.630 | wavelet |
| scale-PRES | 40dB | qtt | 0.001362 | 0.0005641 | 0.414 | 0.0005641 | 0.414 |  |
| scale-PRES | 60dB | qtt | 0.004391 | 0.003034 | 0.691 | 0.003034 | 0.691 |  |
| scale-PRES | 80dB | qtt | 0.06823 | 0.03979 | 0.583 | 0.03979 | 0.583 |  |
| scale-PRES | 100dB | tthresh | 0.3145 | 0.1983 | 0.630 | 0.1983 | 0.630 |  |
| scale-PRES | max0.01 | sz3 | 0.008682 | 0.01292 | 1.489 | 0.0065 | 0.749 | interp |
| scale-PRES | max0.001 | qtt | 0.05598 | 0.03733 | 0.667 | 0.03733 | 0.667 | wavelet |
| scale-PRES | max0.0001 | qtt | 0.2024 | 0.1812 | 0.895 | 0.1812 | 0.895 | wavelet |
| scale-QV | 40dB | qtt | 0.01676 | 0.006808 | 0.406 | 0.006808 | 0.406 |  |
| scale-QV | 60dB | sperr | 0.1588 | 0.1157 | 0.729 | 0.1157 | 0.729 |  |
| scale-QV | 80dB | sperr | 1.398 | 1.207 | 0.864 | 1.207 | 0.864 |  |
| scale-QV | 100dB | sperr | 3.684 | 3.317 | 0.900 | 3.317 | 0.900 |  |
| scale-QV | max0.01 | sperr | 0.1222 | 0.08893 | 0.728 | 0.08893 | 0.728 | wavelet |
| scale-QV | max0.001 | sperr | 0.7633 | 0.6475 | 0.848 | 0.6475 | 0.848 | wavelet |
| scale-QV | max0.0001 | sperr | 2.511 | 2.156 | 0.858 | 2.156 | 0.858 | wavelet |
| scale-T | 40dB | qtt | 0.001936 | 0.0006773 | 0.350 | 0.0006773 | 0.350 |  |
| scale-T | 60dB | sperr | 0.02316 | 0.02149 | 0.928 | 0.02149 | 0.928 |  |
| scale-T | 80dB | tthresh | 0.4548 | 0.3285 | 0.722 | 0.3285 | 0.722 |  |
| scale-T | 100dB | tthresh | 2.574 | 2.209 | 0.858 | 2.209 | 0.858 |  |
| scale-T | max0.01 | qtt | 0.02405 | 0.02444 | 1.016 | 0.02282 | 0.949 | interp |
| scale-T | max0.001 | sperr | 0.2468 | 0.1876 | 0.760 | 0.1876 | 0.760 | wavelet |
| scale-T | max0.0001 | sperr | 1.417 | 1.245 | 0.879 | 1.245 | 0.879 | wavelet |
| scale-U | 40dB | qtt | 0.0294 | 0.01149 | 0.391 | 0.01149 | 0.391 |  |
| scale-U | 60dB | tthresh | 0.2482 | 0.1499 | 0.604 | 0.1499 | 0.604 |  |
| scale-U | 80dB | tthresh | 1.616 | 1.292 | 0.799 | 1.292 | 0.799 |  |
| scale-U | 100dB | tthresh | 4.539 | 3.92 | 0.864 | 3.92 | 0.864 |  |
| scale-U | max0.01 | sperr | 0.1636 | 0.122 | 0.746 | 0.122 | 0.746 | wavelet |
| scale-U | max0.001 | sperr | 0.952 | 0.8242 | 0.866 | 0.8242 | 0.866 | wavelet |
| scale-U | max0.0001 | sperr | 3.158 | 2.716 | 0.860 | 2.716 | 0.860 | wavelet |
| hurricane-V | 40dB | sz3 | 0.03155 | 0.008008 | 0.254 | 0.008008 | 0.254 |  |
| hurricane-V | 60dB | sperr | 0.2593 | 0.2628 | 1.014 | 0.2628 | 1.014 |  |
| hurricane-V | 80dB | tthresh | 1.397 | 1.135 | 0.813 | 1.135 | 0.813 |  |
| hurricane-V | 100dB | tthresh | 3.133 | 2.757 | 0.880 | 2.757 | 0.880 |  |
| hurricane-V | max0.01 | sz3 | 0.143 | 0.09356 | 0.654 | 0.09356 | 0.654 | wavelet |
| hurricane-V | max0.001 | sperr | 0.8708 | 0.7919 | 0.909 | 0.7919 | 0.909 | wavelet |
| hurricane-V | max0.0001 | sperr | 2.637 | 2.423 | 0.919 | 2.423 | 0.919 | wavelet |
| miranda-pressure | 40dB | tthresh | 0.0635 | 0.02818 | 0.444 | 0.02818 | 0.444 |  |
| miranda-pressure | 60dB | sperr | 0.2322 | 0.1772 | 0.763 | 0.1772 | 0.763 |  |
| miranda-pressure | 80dB | sperr | 1.036 | 0.5731 | 0.553 | 0.5731 | 0.553 |  |
| miranda-pressure | 100dB | sperr | 2.648 | 1.553 | 0.587 | 1.553 | 0.587 |  |
| miranda-pressure | max0.01 | sperr | 0.1931 | 0.1142 | 0.592 | 0.1142 | 0.592 | wavelet |
| miranda-pressure | max0.001 | sperr | 0.6916 | 0.4852 | 0.702 | 0.4852 | 0.702 | wavelet |
| miranda-pressure | max0.0001 | sperr | 1.829 | 1.455 | 0.796 | 1.455 | 0.796 | wavelet |
| nyx-velocity_x | 40dB | sz3 | 0.01771 | 0.006378 | 0.360 | 0.006378 | 0.360 |  |
| nyx-velocity_x | 60dB | sperr | 0.2585 | 0.1934 | 0.748 | 0.1934 | 0.748 |  |
| nyx-velocity_x | 80dB | sperr | 2.603 | 2.179 | 0.837 | 2.179 | 0.837 |  |
| nyx-velocity_x | 100dB | sperr | 5.978 | 5.489 | 0.918 | 5.489 | 0.918 |  |
| nyx-velocity_x | max0.01 | sz3 | 0.1264 | 0.08398 | 0.664 | 0.08152 | 0.645 | interp |
| nyx-velocity_x | max0.001 | sz3 | 1.101 | 0.8186 | 0.743 | 0.8186 | 0.743 | wavelet |
| nyx-velocity_x | max0.0001 | sz3 | 3.878 | 3.405 | 0.878 | 3.405 | 0.878 | wavelet |

**Geo-mean tc-portfolio / best external:** 40dB 0.429 (n=13), 60dB 0.775 (n=14), 80dB 0.737 (n=14), 100dB 0.808 (n=14), max0.01 0.677 (n=14), max0.001 0.811 (n=14), max0.0001 0.871 (n=14)

**Win/loss/tie (±1%) vs best external:** PSNR 54–1–0; max-abs 38–1–3

**Gate (no point > 1.05× best external):** FAIL at 1 points
- qmcpack-orb0 max0.0001: tc-portfolio 0.3346 vs qtt 0.2909 (1.150×), pick=wavelet

**Encode time per encode (s), median / mean:** tc-auto maxabs: 1.55 / 1.58 (n=42); tc-auto psnr: 2.02 / 4.29 (n=112); tc-portfolio maxabs: 1.73 / 3.83 (n=42); tc-portfolio psnr: 2.12 / 5.23 (n=112)

**Picks:** maxabs/interp=14, maxabs/qtt=2, maxabs/wavelet=26, psnr/qtt=2, psnr/tucker=62, psnr/wavelet=48

**QTT tried (pre-screen passed):** 11 of 154 encodes; peakiness per field: cesm-CLDTOT 1.3, cesm-FLDS 1.3, cesm-LHFLX 1.3, cesm-TS 1.8, exafel-ev0 48.5, qmcpack-orb0 1.4, qmcpack-orb100 1.6, scale-PRES 1.0, scale-QV 1.1, scale-T 1.0, scale-U 1.3, hurricane-V 1.2, miranda-pressure 2.5, nyx-velocity_x 2.7

---

# Phase 2 (Track E): rate-predicting screen, fresh held-out gate, `--fast`

Branch `claude/tc2-portfolio-7c5fe2`. **Frozen code: `838bc22`**, committed before any fresh-set run. The binary was clean-rebuilt in this worktree's own `target/` (not the shared `CARGO_TARGET_DIR`), and its streams were byte-identical to the tuning binary on two spot checks. Harness: `bench/fetch_fresh.py`, `bench/run_fresh.py`, `bench/summarize_fresh.py`. Outputs (untracked) are in `bench/out/{seen,fresh,fresh_ext}_*.csv` and `summary_{seen,fresh}.md`.

## Results

| gate | result |
|---|---|
| **Quality**: no fresh point worse than the best external codec by more than 5% | **FAIL at 9 of 62 points** |
| **Fast**: ≥20× faster encode than tc-auto, with ≤10% more bits (geo-mean, fresh set) | **PASS**: 23.7× faster, 1.013× bits |

## Screen v2 (replaces peakiness)

The Phase-1 report data was lost when that worktree was cleaned up. So per-candidate sizes and times were re-measured on the 19 seen fields: the 5 originals plus the 14 Phase-1 held-out fields, 11 targets each, with every candidate (`portfolio-all`). Only these seen fields were used for tuning.

1. **The QTT wins on EXAFEL are its residual-only stream.** At max 1e-3 and 1e-4 the TT core is 0 bytes and all the bytes are the sparse residual. That stream is now its own candidate, `qtt-sparse`. It uses the same QTT tag and format, with the new `codec::Config::residual_only` flag. It always runs at max-abs targets, costs 0.02–0.1 s, and reproduces the old QTT bytes exactly (for example 343,943 B at 1e-3, which took 15 s through full QTT).
2. **Full QTT is gated by a TT-rank probe.** The probe runs row-major QTT on a 2×-decimated copy (every other sample per axis) at the same absolute bound, with ranks capped at 64. Mean cost is 0.12 s, max 0.78 s. Full QTT runs only when the probe's max rank stays below 64, and also above 1 at max-abs targets: a probe rank ≤ 1 means the residual-only stream won, which `qtt-sparse` already covers. The probe's byte count turned out to predict full-QTT size poorly (ratios 0.5–260×); the rank is the useful signal.
3. **Seen-set check.** Over 209 points, the screened portfolio matched the all-candidate oracle at every point (worst 1.000003×). Full QTT ran on 60 of 209 encodes and cost 45 s in total, against 1,443 s unscreened. It catches qmcpack-orb0 at max 1e-4, the Phase-1 failure, at probe rank 48.

## Fresh held-out set

Nine fields from six SDRBench applications never used before. They were fetched by HTTP range request; 36 MB on disk.

| field | app | shape | source |
|---|---|---|---|
| s3d-O2 / s3d-T / s3d-U | S3D stat_planar t=2.995e-3 | 64×128×128 | f64, middle crop |
| basilisk-u / basilisk-v | Basilisk 2D turbulence, timestep 750 | 1024×1024 | f32 |
| xgc-dpot | XGC 08100, nodes 8000–10047 | 2048×512 | f64 |
| exaalt-cu-x | EXAALT copper dataset1 | 1024×1024 | f32 |
| hacc-vx | HACC medium vx | 2²⁰ (1D) | f32 |
| nwchem-631 | NWChem 631-tst | 2²⁰ (1D) | f32 |

The external codecs (QTT row-major, SZ3, ZFP, SPERR, TTHRESH) were run with the M1 sweep. TTHRESH fails on 2D inputs, as in Phase 1. All errors are measured on the decoder output, all rates are real file sizes, and the code is frozen.

## Quality gate: FAIL at 9 of 62 points

Win/loss/tie (±1%) vs the best external codec:

- PSNR: 28–7–0
- max-abs: 21–3–3 (the three ties are nwchem, where `qtt-sparse` equals external QTT)

Geo-mean vs best external:

| 40 dB | 60 dB | 80 dB | 100 dB | max 1e-2 | max 1e-3 | max 1e-4 |
|---|---|---|---|---|---|---|
| 0.614 | 0.881 | 0.718 | 0.778 | 0.769 | 0.936 | 0.883 |

The screen is not what failed. None of the 9 failing points has a skipped candidate that would have won. Every failure is a gap in what the candidate set can do at all:

| failing points | cause |
|---|---|
| **exaalt-cu-x**, 5 points (80/100 dB, max 1e-2/1e-3/1e-4; 1.06–1.42×) | SZ3 beats every tang candidate. Our interp is 1.25× larger than SZ3 on this field at max 1e-2. This is particle-coordinate data with little spatial smoothness. |
| **hacc-vx 100 dB** (1.22× vs SZ3), **xgc-dpot 100 dB** (1.09× vs SPERR) | High-rate PSNR points with no interp candidate, because interp is max-abs only. At max-abs, interp wins on hacc. |
| **basilisk-u/v 60 dB** (1.33×) | SPERR's front is nearly flat between 50 and 70 dB on these fields: 0.0139 bpv at 60 dB, while our wavelet needs 0.0131 at 50 dB and 0.0184 at 60 dB. Not diagnosed further. |

## Fast mode (`tc encode … --fast`, equivalently `--codec fast`)

- **Step size from a formula.** It starts at Δ = 4·rmse_target. Up to 5 log–log secant steps follow on a coefficient-domain error estimate (no inverse transform), then 1–3 real error checks. Max-abs uses Δ = 2.2·b. Interp uses one fixed (α, β) preset (index 1) instead of four trial encodes.
- **Model.** The static in-library model, never an in-stream overfit. The pretrained net (`pre_ALL.tcm`) was about 10% smaller on the seen fields but costs about 1 s per encode on the CPU, which makes 20× unreachable.
- **Single candidate.** The screen trial-encodes the fast variant of each applicable candidate on a centered eighth of the field: max-abs tries wavelet, interp and qtt-sparse; PSNR tries wavelet, plus Tucker on 3D. The smallest stream picks the one candidate used for the full encode. Full QTT is never used in fast mode.
- **Seen fields** (tuning check): 1.044× bits, 21.8× faster.
- **Fresh set: PASS** at 62 points:
  - 1.013× bits (max 1.61, exaalt);
  - 23.7× faster (min 10.0×);
  - median encode 0.086 s at max-abs and 0.117 s at PSNR, against 2.8 s and 2.2 s for tc-auto.

Caveat: the machine was shared with other sessions (load average 10–30) for all timings, but both modes ran interleaved under the same load.

## Follow-ups

- Add interp at PSNR targets and close interp's gap to SZ3 on rough data (exaalt). This accounts for 7 of the 9 failures.
- Find out why SPERR's front is flat on basilisk around 60 dB.
- A third fresh set would be needed to re-gate any change made in response to these results.

## Fresh per-point table and fast-gate detail

| field | point | best external | bpv | tc-portfolio | ratio | pick |
|---|---|---|---|---|---|---|
| basilisk-u | 40dB | sz3 | 0.01341 | 0.009787 | 0.730 |  |
| basilisk-u | 60dB | sperr | 0.01389 | 0.01844 | 1.327 **FAIL** |  |
| basilisk-u | 80dB | sperr | 0.05668 | 0.04766 | 0.841 |  |
| basilisk-u | 100dB | sperr | 0.1614 | 0.1248 | 0.773 |  |
| basilisk-u | max0.01 | sperr | 0.02381 | 0.01139 | 0.478 | interp |
| basilisk-u | max0.001 | sz3 | 0.0462 | 0.0357 | 0.773 | interp |
| basilisk-u | max0.0001 | sz3 | 0.1272 | 0.09269 | 0.729 | interp |
| basilisk-v | 40dB | qtt | 0.01299 | 0.009695 | 0.746 |  |
| basilisk-v | 60dB | sperr | 0.01369 | 0.0184 | 1.344 **FAIL** |  |
| basilisk-v | 80dB | sperr | 0.05639 | 0.04697 | 0.833 |  |
| basilisk-v | 100dB | sperr | 0.1589 | 0.12 | 0.755 |  |
| basilisk-v | max0.01 | sperr | 0.02354 | 0.01276 | 0.542 | interp |
| basilisk-v | max0.001 | sz3 | 0.05087 | 0.04108 | 0.808 | interp |
| basilisk-v | max0.0001 | sz3 | 0.1242 | 0.09622 | 0.775 | wavelet |
| exaalt-cu-x | 40dB | sz3 | 1.413 | 0.606 | 0.429 |  |
| exaalt-cu-x | 60dB | sz3 | 2.909 | 2.143 | 0.737 |  |
| exaalt-cu-x | 80dB | sz3 | 4.109 | 4.36 | 1.061 **FAIL** |  |
| exaalt-cu-x | 100dB | sz3 | 5.296 | 6.695 | 1.264 **FAIL** |  |
| exaalt-cu-x | max0.01 | sz3 | 1.367 | 1.705 | 1.247 **FAIL** | wavelet |
| exaalt-cu-x | max0.001 | sz3 | 3.121 | 3.824 | 1.225 **FAIL** | wavelet |
| exaalt-cu-x | max0.0001 | sz3 | 4.56 | 6.454 | 1.415 **FAIL** | wavelet |
| hacc-vx | 40dB | sz3 | 2.064 | 1.614 | 0.782 |  |
| hacc-vx | 60dB | sz3 | 5.445 | 4.867 | 0.894 |  |
| hacc-vx | 80dB | sz3 | 8.766 | 8.221 | 0.938 |  |
| hacc-vx | 100dB | sz3 | 12.11 | 14.76 | 1.218 **FAIL** |  |
| hacc-vx | max0.01 | sz3 | 2.885 | 2.612 | 0.905 | interp |
| hacc-vx | max0.001 | sz3 | 6.232 | 5.865 | 0.941 | interp |
| hacc-vx | max0.0001 | sz3 | 9.559 | 9.221 | 0.965 | interp |
| nwchem-631 | 40dB | – | – | nan | n/a | |
| nwchem-631 | 60dB | sz3 | 0.2661 | 0.2725 | 1.024 |  |
| nwchem-631 | 80dB | sz3 | 2.008 | 1.507 | 0.751 |  |
| nwchem-631 | 100dB | sz3 | 4.667 | 3.771 | 0.808 |  |
| nwchem-631 | max0.01 | qtt | 0.1232 | 0.1232 | 1.000 | qtt-sparse |
| nwchem-631 | max0.001 | qtt | 0.6867 | 0.6867 | 1.000 | qtt-sparse |
| nwchem-631 | max0.0001 | qtt | 2.041 | 2.041 | 1.000 | qtt-sparse |
| s3d-O2 | 40dB | qtt | 0.005316 | 0.002629 | 0.495 |  |
| s3d-O2 | 60dB | sperr | 0.01376 | 0.008832 | 0.642 |  |
| s3d-O2 | 80dB | sperr | 0.04859 | 0.02331 | 0.480 |  |
| s3d-O2 | 100dB | tthresh | 0.1172 | 0.05753 | 0.491 |  |
| s3d-O2 | max0.01 | sz3 | 0.01698 | 0.0154 | 0.907 | interp |
| s3d-O2 | max0.001 | sz3 | 0.05181 | 0.04981 | 0.961 | wavelet |
| s3d-O2 | max0.0001 | sperr | 0.1496 | 0.1124 | 0.751 | wavelet |
| s3d-T | 40dB | qtt | 0.00532 | 0.002727 | 0.513 |  |
| s3d-T | 60dB | sperr | 0.01374 | 0.008887 | 0.647 |  |
| s3d-T | 80dB | sperr | 0.04877 | 0.02335 | 0.479 |  |
| s3d-T | 100dB | tthresh | 0.1211 | 0.05882 | 0.486 |  |
| s3d-T | max0.01 | sperr | 0.02444 | 0.01328 | 0.543 | interp |
| s3d-T | max0.001 | sz3 | 0.05085 | 0.04675 | 0.919 | interp |
| s3d-T | max0.0001 | sperr | 0.1495 | 0.1131 | 0.757 | wavelet |
| s3d-U | 40dB | qtt | 0.004858 | 0.00264 | 0.543 |  |
| s3d-U | 60dB | sperr | 0.01355 | 0.01011 | 0.746 |  |
| s3d-U | 80dB | sperr | 0.05859 | 0.0266 | 0.454 |  |
| s3d-U | 100dB | tthresh | 0.1691 | 0.09322 | 0.551 |  |
| s3d-U | max0.01 | sz3 | 0.01849 | 0.01221 | 0.660 | interp |
| s3d-U | max0.001 | sz3 | 0.06048 | 0.05521 | 0.913 | wavelet |
| s3d-U | max0.0001 | sperr | 0.1918 | 0.1496 | 0.780 | wavelet |
| xgc-dpot | 40dB | sperr | 1.215 | 0.9753 | 0.803 |  |
| xgc-dpot | 60dB | sperr | 4.642 | 3.97 | 0.855 |  |
| xgc-dpot | 80dB | sperr | 7.904 | 7.307 | 0.925 |  |
| xgc-dpot | 100dB | sperr | 11.22 | 12.2 | 1.088 **FAIL** |  |
| xgc-dpot | max0.01 | sz3 | 2.237 | 2.204 | 0.985 | wavelet |
| xgc-dpot | max0.001 | sz3 | 5.451 | 5.173 | 0.949 | wavelet |
| xgc-dpot | max0.0001 | sz3 | 8.804 | 8.451 | 0.960 | wavelet |

**Geo-mean tc-portfolio / best external:** 40dB 0.614 (n=8), 60dB 0.881 (n=9), 80dB 0.718 (n=9), 100dB 0.778 (n=9), max0.01 0.769 (n=9), max0.001 0.936 (n=9), max0.0001 0.883 (n=9)

**Win/loss/tie (±1%) vs best external:** PSNR 28–7–0; max-abs 21–3–3

**Quality gate (no point > 1.05× best external, 62 points):** FAIL at 9 points
- basilisk-u 60dB: tc-portfolio 0.01844 vs sperr 0.01389 (1.327×), pick=
- basilisk-v 60dB: tc-portfolio 0.0184 vs sperr 0.01369 (1.344×), pick=
- exaalt-cu-x 80dB: tc-portfolio 4.36 vs sz3 4.109 (1.061×), pick=
- exaalt-cu-x 100dB: tc-portfolio 6.695 vs sz3 5.296 (1.264×), pick=
- exaalt-cu-x max0.01: tc-portfolio 1.705 vs sz3 1.367 (1.247×), pick=wavelet
- exaalt-cu-x max0.001: tc-portfolio 3.824 vs sz3 3.121 (1.225×), pick=wavelet
- exaalt-cu-x max0.0001: tc-portfolio 6.454 vs sz3 4.56 (1.415×), pick=wavelet
- hacc-vx 100dB: tc-portfolio 14.76 vs sz3 12.11 (1.218×), pick=
- xgc-dpot 100dB: tc-portfolio 12.2 vs sperr 11.22 (1.088×), pick=

**Fast gate (62 points):** bits tc-fast / tc-portfolio geo-mean 1.013 (max 1.61); encode speed-up geo-mean 23.7× (min 10.0×) → PASS

| field | fast bits ratio (geo) | speed-up (geo) |
|---|---|---|
| basilisk-u | 0.892 | 24.2× |
| basilisk-v | 0.903 | 23.8× |
| exaalt-cu-x | 1.203 | 18.8× |
| hacc-vx | 0.990 | 19.7× |
| nwchem-631 | 1.003 | 36.9× |
| s3d-O2 | 1.078 | 22.8× |
| s3d-T | 1.062 | 22.6× |
| s3d-U | 1.022 | 21.8× |
| xgc-dpot | 0.995 | 28.1× |

**Encode time per encode (s), median / mean:** tc-fast maxabs: 0.086 / 0.108 (n=27); tc-fast psnr: 0.117 / 0.134 (n=72); tc-portfolio maxabs: 2.805 / 2.847 (n=27); tc-portfolio psnr: 2.234 / 3.116 (n=72)

**Picks:** tc-fast maxabs/interp=17, tc-fast maxabs/qtt-sparse=3, tc-fast maxabs/wavelet=7, tc-fast psnr/tucker=24, tc-fast psnr/wavelet=48, tc-portfolio maxabs/interp=12, tc-portfolio maxabs/qtt-sparse=3, tc-portfolio maxabs/wavelet=12, tc-portfolio psnr/qtt=2, tc-portfolio psnr/tucker=34, tc-portfolio psnr/wavelet=36

**Full QTT tried (probe passed):** 51 of 99 encodes

## Phase 3 note: interp at PSNR targets

Interp now runs at RMSE/PSNR targets through a rate-controlled bound and is a portfolio candidate at every bound. On the 19 seen fields it wins 5 of 146 PSNR points, all at 30–50 dB (portfolio geomean 0.998×). As a diagnostic only, it takes hacc-vx 100 dB to 11.76 bpv (0.97× SZ3) and xgc-dpot 100 dB to 11.42 (1.02× SPERR). Details: [tang-compress-tc3-cleanup.md](tang-compress-tc3-cleanup.md).
