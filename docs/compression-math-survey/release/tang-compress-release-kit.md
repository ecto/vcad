# cram launch kit (target Tue Nov 3)

Status as of 2026-10-21. Every number traces to a report in `docs/compression-math-survey/`.

## Headline claims (measured)

**1. Scientific fields, error-bounded: 0.67–0.72× SPERR's bits.**
- Data: SDRBench. Tuned on 5 fields (Miranda, Hurricane, NYX). Frozen codec evaluated on **14 held-out fields from 4 applications never used in tuning** (SCALE-LETKF, CESM-ATM, EXAFEL, QMCPACK), plus 3 same-application controls.
- Matched PSNR, geometric-mean ratio of tc-auto to SPERR: 0.719 (60 dB), 0.670 (80 dB), 0.693 (100 dB). Against the best external codec at each point (SPERR, TTHRESH, SZ3, ZFP): 0.74–0.81×.
- Wins 54 of 55 PSNR points and 34 of 42 max-abs points. Error is measured at the decoder from real file sizes, with every byte counted.
- Source: `tang-compress-m1.md` §8.
- Caveat: a second fresh set (9 fields, 6 apps) passed 53 of 62 points. The fix is in its gate run now; see "Gates".

**2. KV cache (storage, offload, transfer): 24–28% fewer bits than the best KV quantizer at matched perplexity.**
- Models: SmolLM2-135M, Qwen2.5-1.5B, Qwen2.5-7B. WikiText-2, whole cache replaced, Δppl ≤ 0.1. Bits per value: 3.19 / 3.25 / 3.29, against the best baseline at 4.35 / 4.54 / 4.31.
- Baselines: KIVI (fp16 R=128 window charged), KVQuant-style, QuaRot-style Hadamard, TurboQuant-mse. With KIVI's window treated as free: 18–19% fewer bits on Qwen.
- Needle-in-a-haystack (Qwen2.5-1.5B, 4K–16K): 15/15 at 2.75–3.3 bpv. KIVI-2 at a similar rate gets 9/15.
- Source: `tang-compress-tensors-bench.md` Phase 3.
- **Scope:** storage, offload and transfer only. Decoding the cache every generation step is 25–50× slower than bf16, so there is no in-generation claim. That line of work was cut.

**3. Byte-identical decode across CPU, Metal, Vulkan and WebGPU/WASM.**
- One integer-only decode path. Verified CPU == GPU on 19 fields × 5 targets (byte compare) on Metal and Vulkan, and 285/285 in Chrome on WebGPU and WASM-CPU.
- Streams can ship their own decoder (TCS1): a verified tang-expr integer program, compiled per stream.
- Sources: `tang-compress-m1d.md`, PR notes.
- Caveats:
  - The latest kernel change (#94, occupancy) was verified on Vulkan only; Metal and WebGPU timings are pending.
  - GPU speed: static streams 1.6–1.9 GB/s, learned 1.06–1.10 GB/s, measured on a shared RTX 3090. Say "≈1–2 GB/s on a consumer GPU", not a peak number.

## Gates

| # | Gate | Status |
|---|---|---|
| 1–15 | (see earlier reports) | passed |
| 16 | Portfolio quality on a third fresh set (unseen fields from seen applications; no unused applications remain on the SDRBench mirror) | **running** on mew |
| 17 | Images vs Cool-chic (Kodak, 120 encodes done) | **evaluation running** |
| 18 | Learned GPU decode ≥ 1 GB/s | **passed** (#94 merged) |
| — | KV in-generation speed | **cut** (out of scope for the launch) |

If 16 or 17 fails by Oct 28, the headline drops it: images is left out entirely, and the fresh-set claim is scoped to the sets that pass.

## Deliverables and dates

| Item | Due | Status | Risk |
|---|---|---|---|
| Gates 16–17 | Wed Oct 28 | running | low–medium |
| Paper (arXiv) | Thu Oct 29 | 14-page draft exists (`paper-draft.pdf`); needs Phase 2/3 numbers, the TCS1/IR section, KV section and limitations | **medium**: ~2–3 days of revision, which has to start by Oct 25 |
| Browser demo with URL | Fri Oct 30 | decoder done (`crates/cram-codec-wasm`); vcad viewer merged (#937) but inactive until the WASM package ships with the app | **medium–high**: hosting location undecided (brand); needs the WASM package published or bundled, plus a demo page |
| Crates | tang-compile Oct 26, cram Nov 2 | prep PR **ecto/tang#99 open, not merged** (awaiting Cam's OK) | **blocks Oct 26:** tang-compile depends on **tang-expr 0.2.0**, which must publish first, and #99 must merge before either |

## Crate publish order (from #99)

1. `tang-expr` 0.2.0 (breaking: `CompiledMany` requires Send + Sync; abs/min/max/tanh trace differently)
2. `tang-compile` 0.1.0
3. `cram` 0.1.0

`tang`, `tang-la`, `tang-ad` and `tang-optim` are already published and unchanged. Commands are in `docs/publishing.md` / `scripts/publish.sh` on the #99 branch. **The Oct 26 tang-compile date requires #99 merged and tang-expr 0.2.0 published on or before Oct 26.** tang-llm and tang-compute (zest) inherit `tang-expr = "0.2"`.
