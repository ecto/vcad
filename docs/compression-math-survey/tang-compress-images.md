# tang-compress, Phase 2 Track D: a GPU-decoded overfitted image codec

**Status: partial.** Paused for the night. The Kodak encode campaign is complete: 120 of 120 encodes,
Cool-chic 5.0.1 HOP, 10k iterations, RTX 3090. The evaluation below covers the first **10 of the 24
images** (kodim01–05 and kodim13–17). The batch run listed only the encodes that already existed when it
started.

- Code: tang branch `claude/tc2-images-901f3d`. The main files are `crates/tang-compress/src/img.rs`,
  `src/imggpu.rs`, `examples/imgcodec.rs` and `tools/coolchic/*`.
- Data: on mew in `~/tc2-images` (runs/, ours.tsv, baselines*.tsv).

## Design

**Encoder.** The encoder is the Cool-chic 5.0.1 reference in PyTorch (HOP config, about 2 kMAC/px),
overfitted per image. I used it instead of tang-ad/tang-optim for two reasons:
- the gate compares against Cool-chic itself;
- its encoder recipe (SOAP, warm-up, soft rounding, RDOQ) is the hard part to reproduce.

**Export.** `export_cc.py` runs the reference decoder once and captures:
- the integer latents (10 grids, of which 3 are hyperlatent grids);
- the ARM, the 3 IFCEs, the upsampling filters and the synthesis networks.

Gotcha: the reference decoder applies the ARM's −4 log-scale shift **in place** on the last bias, so the
captured bias already includes it.

**TCI1 stream (`img.rs`).**
- **Integer arithmetic.** Every network runs in exact i32 fixed point: the ARM and IFCEs in Q8, the
  upsampling and synthesis in Q12. The encoder picks each layer's weight precision so that no accumulation
  exceeds 2^30 on this image. The CPU decoder re-checks this, so the GPU's wrapping i32 is bit-exact.
- **Network coding.** Networks are sent as quantization exponent plus integers, using per-tensor
  Exp-Golomb codes. Symmetric filters send only half their taps. Grid sizes are sent as downscale
  exponents and the context as Cool-chic's canonical priority prefix. The model takes 1886 B against
  Cool-chic's 1832 B (kodim01).
- **Latent coding.**
  - Probabilities come from quantized Laplace tables: μ fraction in steps of 1/32, ln b in steps of 1/32
    over [−5, 5]. The alphabet is 63 symbols plus 2 escapes.
  - The coder is 16-bit-word rANS.
  - Decode order is a **wavefront**, t = x + s·y, with s = 3 for the HOP context. Cool-chic itself uses
    a slope of 10.
  - Rows are spread over P lanes. Lane states continue across grids, from coarse to fine.
  - Optional 2D tiles cut the spatial context at tile borders. The IFCE context is never cut.

**GPU decoder (`imggpu.rs`).**
- One WGSL module is generated per stream. The ARM, IFCE and synthesis segments are `tang_expr::int`
  graphs lowered to straight-line code with the weights as literals. The plumbing is generated text.
- Kernels:
  - `ifce{g}`: one per grid that has an IFCE.
  - `lat{g}`: one workgroup per tile. Each wavefront step runs the ARM for every active row in parallel,
    then each lane decodes its rows, with a shared-memory fast path for r ∈ [−3, 3].
  - `uh`/`uv`/`ph`/`pv`: one set per pyramid level of the upsampling.
  - `seg{i}`: the synthesis segments.

## Correctness

- GPU and CPU decodes are **byte-identical** on every stream tested:
  - unit tests on a synthetic model, with several lane and tile layouts;
  - kodim01 on M4 Max (Metal) and RTX 3090 (Vulkan);
  - every 3090 evaluation run reported "identical: true".
- Against Cool-chic's own float decoder, ours differs by 62–64 dB PSNR (the two images are nearly
  identical). PSNR versus the original is within 0.002 dB of Cool-chic's.

## Rate–distortion (Kodak; 10 images so far; BD-rate vs published Cool-chic 5.0, v5.0 tsv)

| curve | BD-rate vs published CC 5.0 |
|---|---|
| Cool-chic 5.0.1, our 10k-iteration encodes (file sizes) | +5.0 % |
| ours, serial (1 lane) | +6.3 % |
| ours, 64 lanes, whole grid | +8.0 % |
| ours, 64 lanes, row bands of 128 | +8.7 % |
| ours, 128 lanes, 128×128 tiles | +11.6 % |
| AVIF (libavif 1.4.2 / aom, 4:4:4, speed 4, tune=psnr), 24 images | +31.0 % |
| AVIF (default tune), 24 images | +45.5 % |
| JPEG-XL (cjxl 0.12, e7, default perceptual tuning), 24 images | +101.9 % |

**Ratio cost of parallel-friendly decoding, measured against our own Cool-chic encodes.**
- serial: +1.3 % (net coding plus the quantized probability tables);
- 64 lanes: +3 %;
- row bands of 128: +3.7 %;
- 128×128 tiles: +6.6 %.

The wavefront order itself costs nothing. The lane cost is about 4.8 B per lane for flushing its rANS
state, which matters most at the lowest rates (about 4 KB per image at λ = 0.02). On kodim01 at
0.85 bpp it is 0.4 %, 0.8 % and 1.6 % for 32, 64 and 128 lanes.

**Encoder reproduction.** Our encodes land +5 % above Cool-chic's published curve. The loss gap is
1–2 % per point. Likely causes are the encode budget and a different GPU or torch version (torch 2.10 on
mew).

## Decode speed

All numbers are for kodim01. Cool-chic's reference decoder (`cc_decode.py`, CPU) takes:
- 0.95 s on an i9-12900K (mew);
- 0.49 s on an M4 Max.

Our GPU decode on the RTX 3090 (Vulkan), shared with tang-serve, broca and ffmpeg (about 1 % GPU
utilization when idle):

| layout | 3090 decode | speedup vs CC CPU on mew |
|---|---|---|
| 64 lanes, whole grid | 21–25 ms | about 40× |
| row bands of 128 | 12–14 ms | about 70× |
| 128×128 tiles | 8–9 ms | about 110× |

M4 Max: 21–40 ms, unreliable because other GPU jobs were running.

Our CPU decoder (single thread, i64 checked arithmetic) takes 0.8–1.0 s.

**Profile.**
- Latent decode is about 90 % of GPU time. The ARM is about 70 % of that. Without tiles, each grid runs
  in one workgroup, so the ARM's ~470 M integer MACs per image are bound by one SM's throughput.
- Upsampling plus synthesis takes about 1.5 ms.
- More lanes beyond 64 barely help. Tiles help because they spread the ARM over many SMs.

## Gate verdict (provisional, 10 of 24 images)

- **GPU decode ≥ 10× faster than Cool-chic's CPU decode: PASS.** 40× whole-grid, 70–110× tiled.
- **Byte-identical CPU and GPU decodes: PASS.**
- **Within about 5 % BD-rate of published Cool-chic: borderline FAIL as measured.**
  - The 64-lane stream is +8.0 % against the published curve, but only +3 % against the Cool-chic
    encodes it was built from.
  - Our serial stream is +6.3 % against the published curve.
  - Most of the gap is our reproduction of the encoder (+5 %), not the decoder format.
  - Against the reference implementation run locally, the format passes: +1.3 % serial, +3 % with
    64 lanes.
- **JPEG-XL and AVIF:** at matched PSNR, AVIF needs 31–46 % more rate and JPEG-XL about 2× the rate.

## Left to do

- Evaluate the remaining 14 images (`eval_runs.sh ~/tc2-images 10000 IMGCODEC ours.tsv` skips the ones
  already done).
- Rerun `rd.py` and `speed.sh` (multi-image timing plus contention snapshot) on mew.
- Reduce the per-lane flush cost: share one interleaved word stream across lanes, and pick the lane
  count from the payload size.
- Cross-workgroup wavefront: spread one grid over several SMs without cutting its context. The open
  question is WGSL's memory model.
- Optionally, more encode iterations to close the 5 % reproduction gap.
