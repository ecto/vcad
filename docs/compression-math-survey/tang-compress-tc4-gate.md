# cram Phase 4: the Phase-2 quality-gate misses

Branch `claude/tc4-quality-gate`. **Frozen code: `ce42b17`**, committed before any tang run on the held-out set. Two crash guards were added after the frozen code met that set; they change nothing unless a candidate panics (see below). Builds and runs on mew, CPU only, cores 4,5,7–11. External codecs (SZ3, ZFP, SPERR, TTHRESH; QTT row-major) were rebuilt and re-run on both sets.

## Results

| gate | Phase-2 fresh set (now tuning data) | Phase-4 held-out set |
|---|---|---|
| Quality: no point > 1.05× best external | **PASS** 62/62 (was 9 fails) | **FAIL at 1 of 104**: hurricane-W 60 dB, 1.053× SPERR |
| Fast: ≥ 20× faster, ≤ 1.10× bits (geo-mean) | **PASS** 22.5×, 1.071× | **FAIL on bits**: 22.6×, **1.110×** |

The held-out gate is not met. Nothing was tuned on the held-out set after it was seen; both misses are reported as found.

## What changed

1. **`lorenzo` candidate** (`src/lorenzo.rs`, tag `0xA4`). Prediction is either the N-D Lorenzo predictor or the previous point along one axis, picked by real stream size. Quantization is error-bounded and checked on the exact f32 reconstruction, with escapes as in interp. Coding is adaptive binary range coding with contexts: the neighbour symbol classes along the prediction axis and the side axis, plus the last step along the axis. RMSE/PSNR targets use the same rate control as interp (now a shared `interp::rate_control`). It runs at every bound in the portfolio, and at max-abs in `--fast`.
2. **Wavelet `compact` mode** (portfolio only; plain `--codec wavelet` streams are unchanged and stay GPU-decodable). After rate control, the coefficients are coded three ways: with the configured (overfit) model, with the hand model, and with the hand model plus a tile layer (`coefcode::code_bands_tiled`, model tag 3, CPU decoders only). The smallest is kept. The tile layer sends one flag per ~256-cell tile and skips all-zero tiles.
3. **Crash guards** (post-freeze, `cb15c40`, `9316d70`): the portfolio and `--fast` skip a candidate that panics; fast mode falls back to the wavelet. Trigger: Tucker's `factor Gram not SPD even with ridge` on exaalt-he-y 100 dB and on hurricane-QCLOUD (fast, 30 dB). The Tucker bug itself is not fixed; a unit-scale ridge did not stop it.

## Diagnosis and fixes, per Phase-2 failing class (tuning data)

| class | cause | before | after |
|---|---|---|---|
| exaalt-cu-x, 5 points | Particle data, rough across particles. 65% of time-adjacent values are exactly equal. Interp predicts across the rough axis. | 1.06–1.42× SZ3 | 0.69–0.76× (Lorenzo along time) |
| hacc-vx 100 dB | No PSNR candidate for rough 1D data | 1.22× SZ3 | 0.96× (Lorenzo) |
| xgc-dpot 100 dB | Same class; tc3's interp at PSNR alone left it at 1.02× | 1.09× SPERR | 0.95× (wavelet) |
| basilisk-u/v 60 dB | (a) The overfit model ships 742 B inside a ~2 KB stream. (b) The bit-model floor (≈1e-3) costs ~0.0015 bit per zero coefficient, about 10% of the stream. SPERR's set partitioning skips those zeros. | 1.33×, 1.34× SPERR | 0.95×, 0.95× |

SPERR's flat front on Basilisk is real: its PSNR mode overshoots by ~22 dB at low rates (target 40 dB gives 62.4 dB at 0.016 bpv).

## Held-out set: unseen fields from seen applications

All 13 SDRBench applications on the mirror were used in earlier phases, so this is a **weaker hold-out than new applications**. It is 15 fields (fields, runs and crops never used before; `bench/fetch_fresh3.py`, 104 points), chosen before any Phase-4 result.

| application family | tuned on in Phase 4? | fields | quality |
|---|---|---|---|
| EXAALT, HACC, Basilisk, XGC, S3D, NWChem | yes (Phase-2 fresh set) | exaalt-he-y, exaalt-cu2-z, exaalt-2869440-vx, hacc-yy, hacc-vz, basilisk2k-u, xgc-iden, s3d-v3, nwchem-acd | all pass |
| NYX, Hurricane, Miranda, SCALE, CESM | no (seen in Phases 0–1 only) | nyx-baryon, hurricane-QCLOUD, hurricane-W, miranda-viscocity, scale-W, cesm-FLNT | 1 fail: hurricane-W 60 dB, 1.053× |

**Fast-gate miss, diagnosed but not fixed.** The portfolio gained Lorenzo (and keeps QTT) at PSNR targets, but `--fast` at PSNR still trial-encodes only the wavelet and Tucker. The fields where those win are where the fast/full ratio jumped: hurricane-QCLOUD 1.73×, exaalt-he-y 1.58×, exaalt-cu2-z 1.23×, nyx-baryon 1.20×. The likely fix is the fast Lorenzo variant at PSNR in `fast_pick`, then a fresh gate.

**Re-gate needed.** Any fix for either miss has to be gated on yet another unseen set. Unused fields remain in these 13 applications.

## Phase-4 held-out summary (`bench/summarize_fresh.py fresh3`)

| field | point | best external | bpv | tc-portfolio | ratio | pick |
|---|---|---|---|---|---|---|
| basilisk2k-u | 40dB | qtt | 0.004509 | 0.001717 | 0.381 |  |
| basilisk2k-u | 60dB | sperr | 0.004782 | 0.004691 | 0.981 |  |
| basilisk2k-u | 80dB | sperr | 0.01833 | 0.01399 | 0.763 |  |
| basilisk2k-u | 100dB | sz3 | 0.06509 | 0.04027 | 0.619 |  |
| basilisk2k-u | max0.01 | sperr | 0.01034 | 0.00518 | 0.501 | interp |
| basilisk2k-u | max0.001 | sz3 | 0.02067 | 0.01507 | 0.729 | interp |
| basilisk2k-u | max0.0001 | sz3 | 0.0424 | 0.03656 | 0.862 | interp |
| cesm-FLNT | 40dB | sz3 | 0.03293 | 0.007452 | 0.226 |  |
| cesm-FLNT | 60dB | sperr | 0.3781 | 0.3211 | 0.849 |  |
| cesm-FLNT | 80dB | sperr | 2.803 | 2.41 | 0.860 |  |
| cesm-FLNT | 100dB | sperr | 6.089 | 5.612 | 0.922 |  |
| cesm-FLNT | max0.01 | sz3 | 0.1318 | 0.08993 | 0.683 | wavelet |
| cesm-FLNT | max0.001 | sz3 | 1.215 | 1.035 | 0.851 | wavelet |
| cesm-FLNT | max0.0001 | sz3 | 3.997 | 3.622 | 0.906 | wavelet |
| exaalt-2869440-vx | 40dB | sz3 | 3.999 | 3.697 | 0.925 |  |
| exaalt-2869440-vx | 60dB | sz3 | 7.315 | 7.041 | 0.963 |  |
| exaalt-2869440-vx | 80dB | sz3 | 10.62 | 10.45 | 0.984 |  |
| exaalt-2869440-vx | 100dB | sz3 | 13.99 | 13.93 | 0.996 |  |
| exaalt-2869440-vx | max0.01 | qtt | 4.453 | 4.453 | 1.000 | qtt-sparse |
| exaalt-2869440-vx | max0.001 | qtt | 7.772 | 7.772 | 1.000 | qtt-sparse |
| exaalt-2869440-vx | max0.0001 | qtt | 11.09 | 11.09 | 1.000 | qtt-sparse |
| exaalt-cu2-z | 40dB | sz3 | 0.8106 | 0.03983 | 0.049 |  |
| exaalt-cu2-z | 60dB | sperr | 3.403 | 1.42 | 0.417 |  |
| exaalt-cu2-z | 80dB | sz3 | 6.304 | 4.452 | 0.706 |  |
| exaalt-cu2-z | 100dB | sz3 | 9.264 | 7.314 | 0.790 |  |
| exaalt-cu2-z | max0.01 | sz3 | 1.258 | 0.3213 | 0.255 | wavelet |
| exaalt-cu2-z | max0.001 | sz3 | 4.091 | 2.46 | 0.601 | lorenzo |
| exaalt-cu2-z | max0.0001 | sz3 | 7.01 | 5.223 | 0.745 | lorenzo |
| exaalt-he-y | 40dB | qtt | 0.207 | 0.005707 | 0.028 |  |
| exaalt-he-y | 60dB | sz3 | 1.459 | 1.091 | 0.748 |  |
| exaalt-he-y | 80dB | sz3 | 4.884 | 3.252 | 0.666 |  |
| exaalt-he-y | 100dB | sz3 | 8.312 | 6.621 | 0.797 |  |
| exaalt-he-y | max0.01 | sz3 | 0.5688 | 0.2708 | 0.476 | lorenzo |
| exaalt-he-y | max0.001 | sz3 | 2.337 | 1.487 | 0.636 | lorenzo |
| exaalt-he-y | max0.0001 | sz3 | 5.744 | 3.987 | 0.694 | lorenzo |
| hacc-vz | 40dB | sz3 | 1.972 | 1.448 | 0.734 |  |
| hacc-vz | 60dB | sz3 | 5.335 | 4.717 | 0.884 |  |
| hacc-vz | 80dB | sz3 | 8.652 | 8.106 | 0.937 |  |
| hacc-vz | 100dB | sz3 | 12 | 11.56 | 0.963 |  |
| hacc-vz | max0.01 | sz3 | 2.791 | 2.491 | 0.892 | interp |
| hacc-vz | max0.001 | sz3 | 6.115 | 5.688 | 0.930 | lorenzo |
| hacc-vz | max0.0001 | sz3 | 9.444 | 9.021 | 0.955 | lorenzo |
| hacc-yy | 40dB | sz3 | 0.1109 | 0.004028 | 0.036 |  |
| hacc-yy | 60dB | sz3 | 2.55 | 2.182 | 0.856 |  |
| hacc-yy | 80dB | sz3 | 5.872 | 5.536 | 0.943 |  |
| hacc-yy | 100dB | sz3 | 9.197 | 8.913 | 0.969 |  |
| hacc-yy | max0.01 | sz3 | 0.7332 | 0.5041 | 0.688 | wavelet |
| hacc-yy | max0.001 | sz3 | 3.382 | 3.068 | 0.907 | lorenzo |
| hacc-yy | max0.0001 | sz3 | 6.7 | 6.426 | 0.959 | lorenzo |
| hurricane-QCLOUD | 40dB | qtt | 0.006009 | 0.00302 | 0.503 |  |
| hurricane-QCLOUD | 60dB | sz3 | 0.03216 | 0.01614 | 0.502 |  |
| hurricane-QCLOUD | 80dB | sz3 | 0.05702 | 0.03505 | 0.615 |  |
| hurricane-QCLOUD | 100dB | sz3 | 0.08112 | 0.05125 | 0.632 |  |
| hurricane-QCLOUD | max0.01 | qtt | 0.02577 | 0.02303 | 0.893 | lorenzo |
| hurricane-QCLOUD | max0.001 | qtt | 0.0415 | 0.04124 | 0.994 | lorenzo |
| hurricane-QCLOUD | max0.0001 | qtt | 0.05476 | 0.05476 | 1.000 | qtt-sparse |
| hurricane-W | 40dB | qtt | 0.01573 | 0.003708 | 0.236 |  |
| hurricane-W | 60dB | sperr | 0.3686 | 0.3881 | 1.053 **FAIL** |  |
| hurricane-W | 80dB | tthresh | 1.462 | 1.23 | 0.841 |  |
| hurricane-W | 100dB | tthresh | 2.954 | 2.624 | 0.888 |  |
| hurricane-W | max0.01 | sperr | 0.2379 | 0.1423 | 0.598 | wavelet |
| hurricane-W | max0.001 | sperr | 0.9515 | 0.8557 | 0.899 | wavelet |
| hurricane-W | max0.0001 | sperr | 2.658 | 2.408 | 0.906 | wavelet |
| miranda-viscocity | 40dB | sz3 | 0.05035 | 0.02202 | 0.437 |  |
| miranda-viscocity | 60dB | sperr | 0.2586 | 0.1836 | 0.710 |  |
| miranda-viscocity | 80dB | tthresh | 0.8661 | 0.6853 | 0.791 |  |
| miranda-viscocity | 100dB | tthresh | 1.973 | 1.696 | 0.860 |  |
| miranda-viscocity | max0.01 | sz3 | 0.1469 | 0.1024 | 0.697 | interp |
| miranda-viscocity | max0.001 | sperr | 0.7397 | 0.4884 | 0.660 | wavelet |
| miranda-viscocity | max0.0001 | sperr | 1.938 | 1.475 | 0.761 | wavelet |
| nwchem-acd | 40dB | sz3 | 0.4983 | 0.2824 | 0.567 |  |
| nwchem-acd | 60dB | sz3 | 3.234 | 2.507 | 0.775 |  |
| nwchem-acd | 80dB | sz3 | 6.329 | 5.384 | 0.851 |  |
| nwchem-acd | 100dB | sz3 | 9.577 | 8.615 | 0.900 |  |
| nwchem-acd | max0.01 | qtt | 1.003 | 1.003 | 1.000 | qtt-sparse |
| nwchem-acd | max0.001 | qtt | 3.33 | 3.33 | 1.000 | qtt-sparse |
| nwchem-acd | max0.0001 | qtt | 6.313 | 6.313 | 1.000 | qtt-sparse |
| nyx-baryon | 40dB | – | – | nan | n/a | |
| nyx-baryon | 60dB | sperr | 0.02467 | 0.0174 | 0.705 |  |
| nyx-baryon | 80dB | sperr | 0.3114 | 0.2048 | 0.658 |  |
| nyx-baryon | 100dB | sperr | 1.931 | 1.543 | 0.799 |  |
| nyx-baryon | max0.01 | sz3 | 0.03172 | 0.01373 | 0.433 | interp |
| nyx-baryon | max0.001 | sz3 | 0.1761 | 0.1134 | 0.644 | interp |
| nyx-baryon | max0.0001 | sz3 | 0.8958 | 0.7097 | 0.792 | wavelet |
| s3d-v3 | 40dB | qtt | 0.009453 | 0.004117 | 0.436 |  |
| s3d-v3 | 60dB | sperr | 0.0216 | 0.01805 | 0.836 |  |
| s3d-v3 | 80dB | sperr | 0.08892 | 0.05965 | 0.671 |  |
| s3d-v3 | 100dB | tthresh | 0.2464 | 0.1574 | 0.639 |  |
| s3d-v3 | max0.01 | sperr | 0.03404 | 0.02246 | 0.660 | wavelet |
| s3d-v3 | max0.001 | sz3 | 0.09546 | 0.07715 | 0.808 | wavelet |
| s3d-v3 | max0.0001 | sperr | 0.2688 | 0.2249 | 0.836 | wavelet |
| scale-W | 40dB | tthresh | 0.05052 | 0.02028 | 0.401 |  |
| scale-W | 60dB | tthresh | 0.3577 | 0.2635 | 0.737 |  |
| scale-W | 80dB | tthresh | 1.765 | 1.452 | 0.823 |  |
| scale-W | 100dB | tthresh | 4.376 | 3.827 | 0.874 |  |
| scale-W | max0.01 | sperr | 0.261 | 0.1664 | 0.638 | wavelet |
| scale-W | max0.001 | sperr | 1.198 | 1.092 | 0.912 | wavelet |
| scale-W | max0.0001 | sperr | 3.306 | 2.876 | 0.870 | wavelet |
| xgc-iden | 40dB | sz3 | 2.75 | 2.414 | 0.878 |  |
| xgc-iden | 60dB | sz3 | 6.044 | 5.768 | 0.954 |  |
| xgc-iden | 80dB | sperr | 9.379 | 9.134 | 0.974 |  |
| xgc-iden | 100dB | sperr | 12.7 | 12.46 | 0.981 |  |
| xgc-iden | max0.01 | sz3 | 3.54 | 3.395 | 0.959 | interp |
| xgc-iden | max0.001 | sz3 | 6.834 | 6.741 | 0.986 | interp |
| xgc-iden | max0.0001 | sz3 | 10.17 | 10.07 | 0.991 | interp |

**Geo-mean tc-portfolio / best external:** 40dB 0.273 (n=14), 60dB 0.777 (n=15), 80dB 0.797 (n=15), 100dB 0.832 (n=15), max0.01 0.653 (n=15), max0.001 0.825 (n=15), max0.0001 0.880 (n=15)

**Win/loss/tie (±1%) vs best external:** PSNR 57–1–1; max-abs 36–0–9

**Quality gate (no point > 1.05× best external, 104 points):** FAIL at 1 points
- hurricane-W 60dB: tc-portfolio 0.3881 vs sperr 0.3686 (1.053×), pick=

**Fast gate (104 points):** bits tc-fast / tc-portfolio geo-mean 1.110 (max 17.76); encode speed-up geo-mean 22.6× (min 9.6×) → FAIL

| field | fast bits ratio (geo) | speed-up (geo) |
|---|---|---|
| basilisk2k-u | 1.004 | 20.9× |
| cesm-FLNT | 1.021 | 25.9× |
| exaalt-2869440-vx | 1.007 | 16.2× |
| exaalt-cu2-z | 1.229 | 228.0× |
| exaalt-he-y | 1.584 | 22.4× |
| hacc-vz | 1.004 | 18.4× |
| hacc-yy | 1.022 | 19.1× |
| hurricane-QCLOUD | 1.725 | 26.7× |
| hurricane-W | 1.048 | 14.6× |
| miranda-viscocity | 1.027 | 14.7× |
| nwchem-acd | 0.983 | 18.4× |
| nyx-baryon | 1.198 | 20.9× |
| s3d-v3 | 1.046 | 18.8× |
| scale-W | 1.022 | 13.2× |
| xgc-iden | 1.008 | 23.3× |

**Encode time per encode (s), median / mean:** tc-fast maxabs: 0.195 / 0.203 (n=45); tc-fast psnr: 0.260 / 0.270 (n=120); tc-portfolio maxabs: 3.506 / 3.688 (n=45); tc-portfolio psnr: 4.282 / 45.567 (n=120)

**Picks:** tc-fast maxabs/interp=11, tc-fast maxabs/lorenzo=11, tc-fast maxabs/qtt-sparse=9, tc-fast maxabs/wavelet=14, tc-fast psnr/tucker=31, tc-fast psnr/wavelet=89, tc-portfolio maxabs/interp=10, tc-portfolio maxabs/lorenzo=11, tc-portfolio maxabs/qtt-sparse=7, tc-portfolio maxabs/wavelet=17, tc-portfolio psnr/lorenzo=14, tc-portfolio psnr/qtt=6, tc-portfolio psnr/tucker=28, tc-portfolio psnr/wavelet=72

**Full QTT tried (probe passed):** 39 of 165 encodes

## Phase-2 fresh set re-run (`summarize_fresh.py fresh`)

| field | point | best external | bpv | tc-portfolio | ratio | pick |
|---|---|---|---|---|---|---|
| basilisk-u | 40dB | sz3 | 0.0134 | 0.003782 | 0.282 |  |
| basilisk-u | 60dB | sperr | 0.01389 | 0.01312 | 0.945 |  |
| basilisk-u | 80dB | sperr | 0.05668 | 0.04252 | 0.750 |  |
| basilisk-u | 100dB | sperr | 0.1614 | 0.1162 | 0.720 |  |
| basilisk-u | max0.01 | sperr | 0.02381 | 0.01139 | 0.478 | interp |
| basilisk-u | max0.001 | sz3 | 0.0462 | 0.0357 | 0.773 | interp |
| basilisk-u | max0.0001 | sz3 | 0.1272 | 0.09248 | 0.727 | wavelet |
| basilisk-v | 40dB | qtt | 0.01299 | 0.003684 | 0.284 |  |
| basilisk-v | 60dB | sperr | 0.01369 | 0.01301 | 0.950 |  |
| basilisk-v | 80dB | sperr | 0.05639 | 0.04212 | 0.747 |  |
| basilisk-v | 100dB | sperr | 0.1589 | 0.113 | 0.711 |  |
| basilisk-v | max0.01 | sperr | 0.02354 | 0.01276 | 0.542 | interp |
| basilisk-v | max0.001 | sz3 | 0.05087 | 0.03977 | 0.782 | wavelet |
| basilisk-v | max0.0001 | sz3 | 0.1242 | 0.09131 | 0.735 | wavelet |
| exaalt-cu-x | 40dB | sz3 | 1.413 | 0.6073 | 0.430 |  |
| exaalt-cu-x | 60dB | sz3 | 2.909 | 1.742 | 0.599 |  |
| exaalt-cu-x | 80dB | sz3 | 4.109 | 2.836 | 0.690 |  |
| exaalt-cu-x | 100dB | sz3 | 5.296 | 4.005 | 0.756 |  |
| exaalt-cu-x | max0.01 | sz3 | 1.367 | 0.9774 | 0.715 | lorenzo |
| exaalt-cu-x | max0.001 | sz3 | 3.121 | 2.03 | 0.650 | lorenzo |
| exaalt-cu-x | max0.0001 | sz3 | 4.56 | 3.193 | 0.700 | lorenzo |
| hacc-vx | 40dB | sz3 | 2.064 | 1.614 | 0.782 |  |
| hacc-vx | 60dB | sz3 | 5.445 | 4.867 | 0.894 |  |
| hacc-vx | 80dB | sz3 | 8.766 | 8.221 | 0.938 |  |
| hacc-vx | 100dB | sz3 | 12.11 | 11.68 | 0.964 |  |
| hacc-vx | max0.01 | sz3 | 2.885 | 2.612 | 0.905 | interp |
| hacc-vx | max0.001 | sz3 | 6.232 | 5.814 | 0.933 | lorenzo |
| hacc-vx | max0.0001 | sz3 | 9.559 | 9.147 | 0.957 | lorenzo |
| nwchem-631 | 40dB | – | – | nan | n/a | |
| nwchem-631 | 60dB | sz3 | 0.2661 | 0.234 | 0.879 |  |
| nwchem-631 | 80dB | sz3 | 2.008 | 1.408 | 0.701 |  |
| nwchem-631 | 100dB | sz3 | 4.667 | 3.596 | 0.771 |  |
| nwchem-631 | max0.01 | qtt | 0.1232 | 0.1232 | 1.000 | qtt-sparse |
| nwchem-631 | max0.001 | qtt | 0.6867 | 0.6867 | 1.000 | qtt-sparse |
| nwchem-631 | max0.0001 | qtt | 2.041 | 2.041 | 1.000 | qtt-sparse |
| s3d-O2 | 40dB | qtt | 0.005316 | 0.002629 | 0.495 |  |
| s3d-O2 | 60dB | sperr | 0.01376 | 0.008832 | 0.642 |  |
| s3d-O2 | 80dB | sperr | 0.04859 | 0.02331 | 0.480 |  |
| s3d-O2 | 100dB | tthresh | 0.1172 | 0.05753 | 0.491 |  |
| s3d-O2 | max0.01 | sz3 | 0.01698 | 0.0154 | 0.907 | interp |
| s3d-O2 | max0.001 | sz3 | 0.05181 | 0.04662 | 0.900 | wavelet |
| s3d-O2 | max0.0001 | sperr | 0.1496 | 0.1109 | 0.741 | wavelet |
| s3d-T | 40dB | qtt | 0.00532 | 0.002727 | 0.513 |  |
| s3d-T | 60dB | sperr | 0.01374 | 0.008887 | 0.647 |  |
| s3d-T | 80dB | sperr | 0.04877 | 0.02335 | 0.479 |  |
| s3d-T | 100dB | tthresh | 0.1211 | 0.05882 | 0.486 |  |
| s3d-T | max0.01 | sperr | 0.02444 | 0.01328 | 0.543 | interp |
| s3d-T | max0.001 | sz3 | 0.05085 | 0.04645 | 0.913 | wavelet |
| s3d-T | max0.0001 | sperr | 0.1495 | 0.112 | 0.749 | wavelet |
| s3d-U | 40dB | qtt | 0.00483 | 0.002586 | 0.535 |  |
| s3d-U | 60dB | sperr | 0.01355 | 0.0101 | 0.746 |  |
| s3d-U | 80dB | sperr | 0.05859 | 0.0266 | 0.454 |  |
| s3d-U | 100dB | tthresh | 0.1691 | 0.09322 | 0.551 |  |
| s3d-U | max0.01 | sz3 | 0.01849 | 0.01221 | 0.660 | interp |
| s3d-U | max0.001 | sz3 | 0.06048 | 0.05083 | 0.841 | wavelet |
| s3d-U | max0.0001 | sperr | 0.1918 | 0.1479 | 0.771 | wavelet |
| xgc-dpot | 40dB | sperr | 1.215 | 0.9753 | 0.803 |  |
| xgc-dpot | 60dB | sperr | 4.642 | 3.97 | 0.855 |  |
| xgc-dpot | 80dB | sperr | 7.904 | 7.303 | 0.924 |  |
| xgc-dpot | 100dB | sperr | 11.22 | 10.71 | 0.955 |  |
| xgc-dpot | max0.01 | sz3 | 2.237 | 2.204 | 0.985 | wavelet |
| xgc-dpot | max0.001 | sz3 | 5.451 | 5.173 | 0.949 | wavelet |
| xgc-dpot | max0.0001 | sz3 | 8.804 | 8.451 | 0.960 | wavelet |

**Geo-mean tc-portfolio / best external:** 40dB 0.482 (n=8), 60dB 0.784 (n=9), 80dB 0.662 (n=9), 100dB 0.691 (n=9), max0.01 0.723 (n=9), max0.001 0.853 (n=9), max0.0001 0.808 (n=9)

**Win/loss/tie (±1%) vs best external:** PSNR 35–0–0; max-abs 24–0–3

**Quality gate (no point > 1.05× best external, 62 points):** PASS

**Fast gate (62 points):** bits tc-fast / tc-portfolio geo-mean 1.071 (max 1.97); encode speed-up geo-mean 22.5× (min 9.9×) → PASS

| field | fast bits ratio (geo) | speed-up (geo) |
|---|---|---|
| basilisk-u | 1.013 | 20.1× |
| basilisk-v | 1.036 | 21.4× |
| exaalt-cu-x | 1.396 | 22.2× |
| hacc-vx | 1.005 | 17.7× |
| nwchem-631 | 1.049 | 24.6× |
| s3d-O2 | 1.091 | 28.6× |
| s3d-T | 1.065 | 27.5× |
| s3d-U | 1.014 | 21.9× |
| xgc-dpot | 1.015 | 20.6× |

**Encode time per encode (s), median / mean:** tc-fast maxabs: 0.134 / 0.164 (n=27); tc-fast psnr: 0.184 / 0.215 (n=72); tc-portfolio maxabs: 3.120 / 3.341 (n=27); tc-portfolio psnr: 3.698 / 4.911 (n=72)

**Picks:** tc-fast maxabs/interp=13, tc-fast maxabs/lorenzo=7, tc-fast maxabs/qtt-sparse=3, tc-fast maxabs/wavelet=4, tc-fast psnr/tucker=24, tc-fast psnr/wavelet=48, tc-portfolio maxabs/interp=7, tc-portfolio maxabs/lorenzo=5, tc-portfolio maxabs/qtt-sparse=3, tc-portfolio maxabs/wavelet=12, tc-portfolio psnr/lorenzo=14, tc-portfolio psnr/qtt=2, tc-portfolio psnr/tucker=24, tc-portfolio psnr/wavelet=32

**Full QTT tried (probe passed):** 51 of 99 encodes
