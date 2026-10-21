# cram, Phase 2 Track D: a GPU-decoded overfitted image codec

**Question.** Do "compiled per-stream decoders" generalize from scientific fields to images? Here, a
Cool-chic-style overfitted image codec whose per-image networks are compiled to straight-line WGSL with
the weights as literals, decoded on the GPU.

**Answer.**
- **Speed:** yes. GPU decode is 40–110× faster than the reference CPU decoder.
- **Exactness:** yes. GPU and CPU decodes are byte-identical, on Metal and Vulkan.
- **Rate:** the 5 %-of-published-Cool-chic gate is a **FAIL** for the parallel layouts, measured on all
  24 Kodak images.
  - The 64-lane stream is +8.7 % BD-rate against the published curve. The serial stream is +6.9 %.
  - Most of that is our reproduction of the Cool-chic encoder (+5.6 %).
  - Against the reference encodes our streams are built from, the format costs +1.3 % serial and
    +3.0 % with 64 lanes. That is inside the 5 % budget.

## Where the code and data are

- Code: tang branch `claude/tc2-images-901f3d`.
  - `crates/cram/src/img.rs`: stream format, encoder from the export, CPU reference decoder.
  - `src/imggpu.rs`: per-stream WGSL generation and the wgpu decoder.
  - `examples/imgcodec.rs`: encode, decode and compare.
  - `tools/coolchic/`:
    - `export_cc.py`, `run_kodak.sh`, `eval_runs.sh`, `speed.sh`: the encode and evaluation pipeline.
    - `baselines.py`, `rd.py`: the JPEG-XL/AVIF baselines and BD-rates.
    - `results/*.tsv`: the raw results.
- Encodes: mew, `~/tc2-images/runs/n10000`. 120 encodes: 24 images × λ ∈ {1e-4, 4e-4, 1e-3, 4e-3, 2e-2},
  10k iterations on an RTX 3090, about 8.5 min each.

## Design

**Encoder.** The encoder is the Cool-chic 5.0.1 reference (PyTorch, HOP config, about 2 kMAC/px,
Ladune et al., arXiv 2605.02726), overfitted per image. I used it instead of tang-ad/tang-optim for two
reasons:
- the gate compares against Cool-chic itself;
- its encoder recipe (SOAP, warm-up, soft rounding, RDOQ) is the expensive part to reproduce.

**Export.** `export_cc.py` runs the reference decoder once and captures:
- the integer latents: 10 grids, of which 3 are hyperlatent grids;
- every network: the ARM, the 3 IFCEs, the 12 upsampling filters, and the synthesis with its stabiliser
  and output transform.

Gotcha: the reference decoder applies the ARM's −4 log-scale shift **in place** on the last bias, so the
captured bias already includes it.

**TCI1 stream (`img.rs`).**
- **Integer arithmetic.** Every network runs in exact i32 fixed point: the ARM and IFCEs in Q8, the
  upsampling and synthesis in Q12. The encoder picks each layer's weight precision so that no accumulation
  exceeds 2^30 on that image, and the CPU decoder re-checks this. As a result the GPU's wrapping i32
  arithmetic matches the CPU's checked i64 arithmetic bit for bit.
- **Network coding.** Networks are sent as quantization exponent plus integers, using per-tensor
  Exp-Golomb codes. Symmetric filters send half their taps. Grid sizes are sent as downscale exponents
  and the context as Cool-chic's canonical priority prefix. The model is 1886 B against Cool-chic's
  1832 B, header included (kodim01).
- **Latent coding.**
  - Probabilities come from quantized Laplace tables: μ fraction in steps of 1/32, ln b in steps of 1/32
    over [−5, 5]. The alphabet is 63 symbols plus 2 escapes with 16-bit raw tails. The tables are
    built on the host with plain f64 and uploaded.
  - The coder is rANS with 16-bit words.
- **Parallel-friendly decode order.**
  - The order is a **wavefront**, t = x + s·y. The slope s is the smallest value with every context
    pixel strictly earlier: s = 3 for the HOP template. Cool-chic itself uses s = 10.
  - Rows are spread over P rANS lanes. Lane states continue across grids, from coarse to fine.
  - Optional 2D tiles cut the spatial context at tile borders. The IFCE context comes from coarser grids
    and is never cut.

**GPU decoder (`imggpu.rs`).**
- One WGSL module is generated per stream. The ARM, IFCE and synthesis segments are
  `tang_expr::int::IntGraph`s lowered with `to_wgsl_body`, so they are straight-line code with the
  weights as literals. Loops, barriers and buffers are generated text.
- Dispatch plan:
  - per grid: `ifce{g}`, then `lat{g}`. `lat{g}` runs one workgroup per tile. In each wavefront step it
    runs the ARM for every active row in parallel, then each lane decodes its rows. There is a
    shared-memory CDF window for r ∈ [−3, 3], and a slot-table fallback for other symbols.
  - the upsampling pyramid: `uh`, `uv`, `ph`, `pv` per level;
  - `seg{i}`: the synthesis segments.
- The GPU output is RGBA8.

## Correctness

- GPU output is **byte-identical** to the CPU decoder on all of the following:
  - unit tests on a synthetic Cool-chic-shaped model, across several lane and tile layouts;
  - kodim01 on M4 Max (Metal);
  - 18 of 18 multi-image runs on the RTX 3090 (Vulkan).
- Against Cool-chic's float decoder, our output differs by 62–64 dB PSNR (nearly identical). PSNR
  versus the original is within 0.002 dB of Cool-chic's own decode.

## Rate–distortion: Kodak, all 24 images

PSNR is RGB, from the MSE over all samples. Curves are the 24-image means per λ or quality setting.
BD-rates use a Bjontegaard cubic fit in log-rate.

| curve | BD vs published CC 5.0 | BD vs our CC 5.0.1 encodes |
|---|---|---|
| Cool-chic 5.0.1, our 10k-iteration encodes (file sizes) | **+5.6 %** | 0 |
| ours, serial (1 lane) | +6.9 % | **+1.3 %** |
| ours, 64 lanes, whole grid | +8.7 % | **+3.0 %** |
| ours, 64 lanes, row bands of 128 | +9.7 % | +3.9 % |
| ours, 128 lanes, 128×128 tiles | +12.8 % | +6.9 % |
| AVIF (libavif 1.4.2 / aom 3.15, 4:4:4, speed 4, `tune=psnr`) | +31.0 % | +24.2 % |
| AVIF (same, default tune) | +45.5 % | +37.9 % |
| JPEG-XL (cjxl 0.12, e7, default perceptual tuning) | +101.9 % | +91.3 % |

The "published CC 5.0" curve is `results/v5.0/image-kodak.tsv` from the Cool-chic repository.

Mean rate points:

| λ | published CC 5.0 | our CC encodes | ours, 64 lanes |
|---|---|---|---|
| 0.02 | 0.0839 bpp / 27.44 dB | 0.0867 / 27.37 | 0.0962 / 27.37 |
| 0.004 | 0.2186 / 30.86 | 0.2258 / 30.75 | 0.2349 / 30.75 |
| 0.001 | 0.4819 / 34.52 | 0.4999 / 34.45 | 0.5090 / 34.45 |
| 0.0004 | 0.7594 / 37.25 | 0.7963 / 37.18 | 0.8056 / 37.18 |
| 0.0001 | 1.3571 / 41.17 | 1.4179 / 41.12 | 1.4274 / 41.12 |

### Ratio cost of parallel-friendly entropy decoding

- **Wavefront order: free.** It uses the same causal context as Cool-chic's serial raster order.
- **Lanes: about 4.8 B per lane** for the rANS state flush and the lane length. That is about 0.1 % at
  1.4 bpp and about 10 % at 0.09 bpp with 64 lanes, which is why BD-rate goes from +1.3 % serial to
  +3.0 % with 64 lanes.
- **Tiles: cost compression through the cut context.** On kodim01 at 0.85 bpp, in ideal latent bits:
  - row bands of 256 rows: +0.1 %;
  - row bands of 128: +0.55 %;
  - row bands of 64: +1.6 %;
  - 128×128 tiles: +1.5 %.
- **Serial overhead (+1.3 %):** about +54 B of network and header coding, and about +0.25 % from
  quantizing the probability tables.

## Decode speed

**Setup.** The GPU is an RTX 3090 (Vulkan) on mew. It was shared at the time with tang-serve
(tang-llm, 9.1 GB), broca (1.8 GB), 5× ffmpeg and sunshine: 2 % utilization and 13.2 GB used before the
run. The snapshot is in `results/speed.tsv`.

Cool-chic's CPU number is the reference `cc_decode.py` on mew's i9-12900K, best of 3, with torch
threads at their default. On the M4 Max, kodim01 took 0.49 s.

| image, λ | CC 5.0.1 CPU | ours, 64 lanes | ours, row bands of 128 | ours, 128×128 tiles |
|---|---|---|---|---|
| kodim01, 1e-3 | 0.98 s | 21.9 ms | 13.1 ms | 8.5 ms |
| kodim08, 1e-3 | 0.98 s | 23.2 ms | 13.6 ms | 9.2 ms |
| kodim14, 1e-3 | 0.96 s | 26.5 ms | 14.4 ms | 9.2 ms |
| kodim23, 1e-3 | 0.95 s | 20.4 ms | 12.3 ms | 8.4 ms |
| kodim01, 2e-2 | 2.28 s* | 19.9 ms | 12.1 ms | 8.3 ms |
| kodim20, 1e-4 | 2.16 s* | 23.2 ms | 14.2 ms | 9.2 ms |

\* These two runs were slower, probably from CPU contention on mew; the typical value is about 0.95 s.

- **Speedup:** about **40–48×** with 64 lanes, about 70× with row bands of 128, and about 110× with
  128×128 tiles, all against about 0.95 s.
- **Our CPU decoder** (single thread, checked i64 arithmetic) takes about 1.05 s, roughly the same as
  Cool-chic's CPU decoder.
- **Profile.**
  - Latent decode is about 90 % of GPU time, and the ARM is about 70 % of that. Without tiles, each grid
    runs in one workgroup, so the ARM's ~470 M integer MACs per image are bound by one SM's throughput.
  - Upsampling plus synthesis takes about 1.5 ms.
  - More than 64 lanes barely helps. Tiles help because they put the ARM on more SMs.
- **M4 Max:** 21–40 ms, unreliable because other GPU jobs were running at the same time.

## Gate verdict (all 24 Kodak images)

| criterion | result |
|---|---|
| GPU decode ≥ 10× Cool-chic's measured CPU decode | **PASS**: 40–48× (64 lanes), about 110× (128×128 tiles) |
| Byte-identical CPU and GPU decode | **PASS**: unit tests plus 18/18 multi-image runs, Metal and Vulkan |
| Within about 5 % BD-rate of published Cool-chic 5.0 | **FAIL**: +8.7 % (64 lanes), +6.9 % (serial) |
| Within about 5 % BD-rate of the reference implementation run locally | **PASS**: +3.0 % (64 lanes), +1.3 % (serial); tiled 128×128: +6.9 % (fail) |
| JPEG-XL and AVIF at matched PSNR | AVIF needs +24 % to +38 % more rate than our CC encodes, JPEG-XL +91 % |

**Main cause of the published-curve fail.** Our 10k-iteration Cool-chic 5.0.1 encodes are already
+5.6 % against the published curve. Possible causes are torch 2.10 against the required ≥2.11, the GPU,
or seeds; per-point loss gaps are 1–2 %. Our format adds +1.3 % to +3 % on top of that.

## Next steps

- **Per-lane overhead.** Use one interleaved word stream shared by all lanes, and choose the lane count
  from the payload size. This would recover most of the +1.7 % between serial and 64 lanes at low rates.
- **Cross-workgroup wavefront.** Split one grid's rows over several SMs without cutting its context.
  This would give tile-like speed at zero context cost. The open question is WGSL's memory model: it has
  only relaxed atomics, so cross-workgroup message passing is not formally guaranteed.
- **Encoder reproduction.** Match the published setup (torch ≥2.11, more iterations) to close the
  +5.6 % reproduction gap.
