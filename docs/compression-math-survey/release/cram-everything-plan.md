# cram: "compress everything" plan (2026-10-21, draft for Cam)

**Idea.** One container. The portfolio tries several methods on each input and keeps the smallest stream, with a one-byte method tag. It can never be worse than the best method it includes, plus that tag. That makes "compress everything" honest, with the right framing: cram is the best of what it contains on every input, and genuinely ahead only where its own methods win.

## Where each data class stands today (measured unless marked)

| Class | cram today | Verdict | Evidence |
|---|---|---|---|
| Scientific float fields, lossy | wavelet/Tucker/interp/QTT portfolio | **WIN**: 0.67–0.72× SPERR's bits, 54/55 points. Loses some fields (hurricane-TC: SPERR −6%) | m1 §8; examples.json |
| KV cache, lossy | kvcodec | **WIN**: −24–28% vs the best KV quantizer at matched ppl (storage/offload only) | tensors-bench P3 |
| LLM weights, lossy | wavelet codec | **MIXED**: 5.43 bpv at ppl 20.98 vs int8 8.02 at 20.39; SZ3 is smaller at 4.69 but worse ppl (22.78). No clear win | tensors-bench |
| Weights/activations/optimizer state, lossless | none (byte-group + zstd measured as a baseline) | **ABSENT**: best known is ~10.7 bpv bf16 (ZipNN/exponent-ANS); cram has no arm | tensors-bench lossless |
| Images, lossy | overfitted (Cool-chic-style) codec, GPU decode | **WIN vs codecs, LOSE vs its reference**: AVIF needs +24–38% rate and JPEG-XL +91% at matched PSNR (Kodak). But +6.9–8.7% vs published Cool-chic (gate 17 fail). PSNR-tuned only; encode takes minutes per image on a GPU | images.md |
| Generic bytes: text, logs, JSON, binaries | none | **ABSENT** | — |
| Tables (parquet/CSV), numpy arrays, lossless | none | **ABSENT** (float columns would reuse the lossless-float arm) | — |
| Audio | none | **ABSENT / UNKNOWN** | — |

## Plan (launch is Tue Nov 3; everything gets a measured gate, and losses are published)

**By Oct 24: container + dispatch (M-A).**
- Add a `cram::compress(bytes, hint)` front door that sniffs the type (magic numbers, dtype headers, UTF-8 ratio) and runs the portfolio, plus a method tag.
- Add zstd-19, xz-9 and brotli-11 as baseline arms, so every class has a floor.
- Gate: on every class, cram ≤ min(arms) + 1 B.

**By Oct 28: lossless float/tensor arm (M-B).**
- Byte-group split (exponent / mantissa planes), then context-modelled rANS on the exponent plane, reusing coefcode/abac.
- Target: ≤ ZipNN on safetensors, KV and optimizer state, and beat zstd on numpy and parquet float columns.
- Gate: SmolLM2 + timm tensors, plus 3 public parquet files. Measured against zstd, ZipNN and parquet's own zstd.

**By Nov 2: context-mixing byte arm (M-C).**
- Order-1–4 context mixing with the existing learned/APM mixer, aimed at text, logs and JSON.
- Gate: enwik8 slice, Silesia, and a JSON/log corpus, vs zstd-19, xz-9 and brotli-11, reporting ratio and MB/s.
- Honest expectation: better ratio than xz on text, but 10–100× slower. It ships as an opt-in "max" level, with zstd as the default arm.

**Images: present as-is.**
- The page states the gate-17 result and the encode cost.
- Integrating it as an arm in the container is post-launch (it needs a PyTorch encoder).

**Audio: post-launch (M-D).**
- Lossless LPC + rANS (FLAC-class), gated vs FLAC -8.
- Lossy (vs Opus) is out of scope.

**Site.**
- Hero: "compress everything — cram picks the best method per input."
- A gallery card per class with measured numbers, showing win, tie or loss in plain words. Classes:
  - sci fields (existing)
  - KV
  - weights
  - images (Kodak crops: ours vs AVIF vs JPEG-XL at matched PSNR)
  - text/JSON/logs (after M-C)
  - tensors lossless (after M-B)
- A class with no gate result yet appears as "coming", never as a claim.

## Risks
- M-C can easily miss its date; context mixing is the slowest arm to get right. Fallback: ship the dispatch with zstd/xz/brotli arms and say "ties the best standard codec" for text.
- "Compress everything" invites tests on arbitrary data. The floor guarantee (≤ best arm + 1 B) is the defensible claim, and it must hold.
- No jobs on the Mac: every gate runs on mew.
