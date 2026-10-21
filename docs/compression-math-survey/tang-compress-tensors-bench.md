# cram on ML tensors — Phase 1 benchmark (Track C)

Question: does cram (error-bounded float codec: wavelet / Tucker / QTT
transforms + learned integer context coding, deterministic GPU decode) carry
over from scientific fields to ML tensors? This is the baseline benchmark only —
cram is run **as-is**, no tensor-specific transforms.

Harness: `tang` repo, branch `claude/tc-tensor-bench-4a82a9`, `bench/tensors_*.py`
(data under gitignored `bench/tensors/`, ~500 MB). cram at `22304c6`.

## Dataset (exact sources)

| source | revision | what we use |
|---|---|---|
| `HuggingFaceTB/SmolLM2-135M` (bf16 safetensors, Llama arch, 30 L, d=576, GQA 9/3) | `93efa2f0` | weights: q/k/v/o/gate/up/down of L0/L15/L29 + first 1792 embedding rows (22 tensors) |
| `timm/resnet18.a1_in1k` (f32) | `491b427b` | conv1, layer2.0.conv1, layer3.1.conv2, layer4.1.conv2, fc |
| `timm/vit_tiny_patch16_224.augreg_in21k_ft_in1k` (f32) | `7d3afdd0` | patch_embed, blocks.5 qkv, blocks.5 fc1, blocks.11 fc2 |
| `Salesforce/wikitext` wikitext-2-raw-v1 test parquet | `b08601e0` | first 600 rows → 45,960 SmolLM2 tokens |

Derived dumps (own ~100-line torch Llama forward on raw safetensors, fp32, MPS; ref ppl 20.26 on tokens [0,8192)):

* **KV cache**: post-RoPE K and V `[3 kv-heads, T, 64]` at L0/7/15/22/29 for a 1024-token wiki window, plus L0/15/29 for a 512-token greedy continuation of a fixed prompt (16 tensors benchmarked).
* **Activations**: residual stream `[1024, 576]` and SwiGLU MLP activations `[T, 1536]` at L7/15/22/29 (8).
* **Gradients**: 3 AdamW steps (lr 1e-5, batch 4×256) — step-0 and step-2 grads of q/k/down at L0/15/29 (18), and Adam `exp_avg`/`exp_avg_sq` after step 3 for 7 matrices (14).

Tensors capped at 1 M values (row crop). "Native" dtype for lossless: bf16 for all SmolLM2 tensors incl. KV/acts (deployment dtype), f32 for timm weights, grads, Adam state.

## Metrics

* rate: bits per value (bpv), everything the decoder needs (scales included; zstd/zipnn frames included).
* distortion: relative L2 error `||x − x̂|| / ||x||`. Every lossy codec is swept; table reports bpv at relL2 = 1 % and 10 % (log-linear interpolation on each codec's Pareto front, median over tensors; `(k/n)` = only k tensors reached that error inside the sweep).
* downstream: SmolLM2 WikiText-2 perplexity with all 210 block linear weights compressed (8192 tokens), or with the whole KV cache replaced by its reconstruction (4096 tokens, KIVI-style simulated prefill).

## Baseline table

Lossless (mean bpv; native = 16 for bf16 rows, 32 for f32 rows):

| type | native | zstd-19 | byte-group + zstd-19 | ZipNN 0.5.4 | ANS on exponent (DietGPU-style, ideal H0) |
|---|---|---|---|---|---|
| LLM weights (bf16) | 16 | 12.50 | 10.81 | 10.71 | **10.67** |
| CNN/ViT weights (f32) | 32 | 29.77 | 26.84 | 26.83 | **26.77** |
| KV cache (bf16) | 16 | 11.25 | **9.46** | 10.76 | 10.72 |
| activations (bf16) | 16 | 12.76 | 11.06 | 11.01 | **10.96** |
| gradients (f32) | 32 | 29.88 | 27.04 | 26.97 | **26.94** |
| Adam m / v (f32) | 32 | 29.50 | 26.61 | **26.57** | 27.04 |

Lossless is the known story: only the exponent byte compresses (~33 % for bf16, ~16 % for f32). DietGPU/nvCOMP don't run on macOS; the "ANS on exponent" column is the ideal order-0 bound of what their float mode does.

Lossy, bpv at matched relative L2 error (lower is better; **bold** = best real codec, `ecsq` is an ideal reference):

| type | int per-row (8/6/4/3/2) | int g128 | int g128 + H0 | KIVI g32 | SZ3 abs | ZFP acc | **ecsq (ideal)** | tang wavelet | tang Tucker | tang QTT |
|---|---|---|---|---|---|---|---|---|---|---|
| weights @1 % | 7.79 (21/31) | 7.63 (29/31) | 6.79 | — | 7.23 | 8.63 | 6.86 | **7.04** | 9.52 | 17.1 |
| weights @10 % | 4.70 | 4.47 | 3.65 | — | 3.91 | 5.29 | 3.56 | **3.64** | — | — |
| KV @1 % | 7.73 (8/16) | 7.60 (8/16) | — | 7.57 | 7.00 | 11.09 | 6.71 | **6.48** | 6.85 | 15.1 |
| KV @10 % | 4.99 | 4.87 | 2.52* | 4.37 | 3.71 | 6.90 | 3.41 | **3.08** | — | — |
| activations @1 % | — | — | — | — | 6.45 | 7.82 | 6.16 | **6.25** | 7.46 | — |
| activations @10 % | 6.01 | 5.31 | 3.39 | — | 3.13 | 4.50 | 2.86 | **2.87** | — | — |
| gradients @1 % | — | — | — | — | 6.24 | 7.51 | 5.73 | 5.94 | **5.78** | 14.5 |
| gradients @10 % | 6.47 | 5.55 | 3.86 | — | 2.95 | 4.16 | 2.42 | **2.58** | — | — |
| Adam m/v @1 % | — | — | — | — | 4.40 | 5.65 | 3.67 | **4.31** | 5.30 | 11.4 |
| Adam m/v @10 % | 6.52 (7/14) | 5.56 (9/14) | 3.32 | — | 1.91 | 3.66 | 1.62 (9/14) | **1.05** | — | — |

`—` = error level never reached in the sweep (e.g. int8 per-row on activations/grads is already > 1 % because of outlier rows). `*` median over the 8/16 tensors that reached it.
`ecsq` = one global uniform step + ideal order-0 entropy: it isolates what a codec gains beyond plain entropy-coded quantization.

Downstream (SmolLM2-135M WikiText-2 perplexity):

| target | codec | bpv | relL2 | ppl |
|---|---|---|---|---|
| weights | bf16 reference | 16 | 0 | 20.26 |
| weights | int8 per-row | 8.02 | 0.009 | 20.39 |
| weights | ZFP tol 0.3 rms | 8.01 | 0.016 | 20.48 |
| weights | **tang wavelet rmse 0.03 rms** | **5.43** | 0.030 | **20.98** |
| weights | SZ3 eb 0.1 rms | 4.69 | 0.058 | 22.78 |
| weights | int4 g128 (RTN) | 4.12 | 0.124 | 31.49 |
| weights | int4 per-row | 4.02 | 0.155 | 49.2 |
| weights | **tang wavelet rmse 0.1 rms** | **3.64** | 0.100 | **31.18** |
| weights | SZ3 eb 0.3 rms | 3.15 | 0.173 | 105 |
| weights | int3 g128 | 3.12 | 0.288 | 1403 |
| KV | fp32 reference (4096-token set) | 32 | 0 | 16.53 |
| KV | int8 per-token | 8.25 | 0.010 | 16.54 |
| KV | KIVI-4 (g32) | 5.00 | 0.049 | 16.75 |
| KV | int4 per-token | 4.25 | 0.187 | 17.89 |
| KV | SZ3 eb 0.3 rms | 3.06 | 0.167 | 17.83 |
| KV | **tang wavelet rmse 0.1 rms** | **3.03** | 0.100 | **17.38** |
| KV | KIVI-2 (g32) | 3.00 | 0.248 | 19.72 |
| KV | int2 per-token | 2.25 | 0.739 | 1363 |
| KV | SZ3 eb 1.0 rms | 1.48 | 0.533 | 55.6 |
| KV | **tang wavelet rmse 0.3 rms** | **1.44** | 0.300 | **26.74** |

(KIVI here is the quantizer only — no full-precision recent-token window, no outlier channels.)

## Reading

1. **Current cram wavelet is already at or near the front everywhere**, mostly because it entropy-codes (with a context model) instead of storing fixed-width codes: it is within ~0.1–0.2 bpv of ideal ECSQ on weights, activations and gradients — i.e. on those the 2-D/3-D wavelet transform buys essentially nothing (no spatial smoothness in a weight matrix or a residual-stream matrix), as expected.
2. **It beats ideal ECSQ where the tensor has real structure along an axis**:
   * **KV cache**: −0.2 bpv @1 %, −0.33 bpv @10 %, and the best downstream point at ~3 bpv (ppl +0.85 vs KIVI-2's +3.2 and SZ3's +1.3 at the same rate). K has strong per-channel scale structure (RoPE pairs, outlier channels) and smooth-ish variation along tokens.
   * **Adam second/first moments**: 1.05 bpv vs 1.62 (ECSQ) / 1.91 (SZ3) @10 % — `exp_avg_sq` is positive, heavy-tailed and row/column-scaled, which the wavelet low-pass + context model exploits.
3. **Tucker** is competitive only on gradients @1 % (gradients of a linear layer are a sum of B·T rank-1 outer products — low-rank by construction) and on small KV; **QTT** is unusable as-is on these shapes (fails the bound or 2–5× the rate).
4. Max-abs codecs (SZ3, ZFP, int RTN) are judged here on L2, which flatters RMSE-targeted tang a little; ECSQ is the fair L2 reference and tang ≈ it.
5. Per-row int with fixed-width codes wastes 1–3 bpv vs the same quantizer entropy-coded (`+H0` rows) — the cheapest win for any ML deployment and exactly what a deterministic GPU entropy decoder would deliver.

## Most promising for Phase 2 tensor transforms

1. **KV cache** (best): already ahead of KIVI/SZ3 at matched rate and perplexity. Phase-2 transforms: per-channel scale normalization of K (pre-RoPE or RoPE-pair aware), token-axis prediction/wavelet only (not across head-dim), per-head context models; streaming/append-friendly blocks for decode-time caches.
2. **Optimizer state (Adam m/v) and gradients**: low-rank (Tucker/PowerSGD-like) + residual coding, log-domain coding of `exp_avg_sq`, temporal delta across steps; checkpoints are dominated by these 2× f32 buffers.
3. **Weights**: transforms won't help (≈ ECSQ); the lever is rate allocation (Hessian/activation-aware error budget per channel, GPTQ-style) inside the entropy coder, plus deterministic GPU decode — that's a different project than tensor transforms.
4. Activations: lowest priority (≈ ECSQ; only useful for activation checkpointing/offload).

## Reproduce

```
UVW="uv run --no-project --python 3.12 --with zipnn --with zfpy --with pysz --with zstandard --with numpy"
hf download HuggingFaceTB/SmolLM2-135M model.safetensors config.json tokenizer.json --local-dir bench/tensors/hf/SmolLM2-135M   # + the two timm repos, wikitext parquet
uv run --no-project --python 3.12 --with tokenizers --with numpy --with pyarrow python bench/tensors_prep.py
$UVW python bench/tensors_dump.py      # torch comes with zipnn
TC_BIN=target/release/examples/tc QTT_BIN=target/release/examples/qtt $UVW python bench/tensors_bench.py   # ~5 min, 4 procs
TC_BIN=target/release/examples/tc $UVW python bench/tensors_ppl.py      # ~10 min
```

## Phase 2 — Track C: KV-cache codec (`kvcodec`)

Branch `claude/tc2-kv-cache-333ccb` (tang), commit `8ec4cd1`: `crates/cram/src/kvcodec.rs`, CLI `examples/kvc.rs`, harness `bench/kv_ppl.py` / `kv_bench.py` / `kv_codecs.py` / `adam_look.py`.

### Design (and what each piece bought)

Input is one layer's cache `[kv-heads, T, 64]`. The final design came out of a numpy prototype with ideal per-context entropy, then a Rust port that counts real bytes (real bits are 3–12 % above the ideal estimate).

* **Per-(head, channel) mean + closed-loop AR(1) DPCM along tokens only**, with one uniform step. Pre-RoPE, 68–79 % of K's energy is a per-channel constant (post-RoPE: 45–60 %). Lag-1 correlation of the residual is about 0.4 pre-RoPE. Closed loop gives an exact bound: |error| ≤ step/2 per value in the coded domain.
* **K coded pre-RoPE** (undo with `theta` and absolute position, code, redo on decode). Every per-channel statistic is shared by the RoPE pair (c, c+32). Pre-RoPE is 0.08–0.12 bpv cheaper than post-RoPE at matched relL2. At matched rate the two give the same ppl within noise.
* **Attention sinks.** The first 4 absolute positions use a 16× finer step. Token 0 has tiny ‖V‖ (≈0.3× the median) and takes most of the attention mass. With a uniform step its error dominates perplexity: Δppl +0.033 → +0.004 at 3 % relL2, for +0.02 bpv. KIVI gets this for free from per-token/per-group min/max. A plain error-bounded codec doesn't.
* **K/V error split.** K errors cost about 2× more ppl than V errors at the same relL2. Coding K at 0.5× the relative error of V halves Δppl at equal bpv: at 3.39 bpv, Δ +0.154 → +0.069. A split of 0.35 is worse.
* **Per-channel step weighting** (step ∝ channel sd^α, α = 0.5 or 1) was worse than a uniform step. Per-token V step scaling (KIVI-like) bought nothing beyond the sink handling. Both were dropped.
* **Entropy coder.** Adaptive binary arithmetic coding (`abac`). The context is (head, channel spread class, |previous residual of that channel| ∈ {0, 1, 2+}). Models start from a discrete-Laplace prior for the class, so a fresh block doesn't start from flat models. The header holds 3 B per (head, channel): mean on the step grid, AR coefficient, class.
* **Append-friendly blocks.** Blocks of 128 tokens are independent (prediction and models reset). New tokens never re-encode the past; only the < 128-token tail is coded at serialization. Any token range decodes from its covering blocks. Stream statistics come from the prefill. Block size barely matters: 32 / 64 / 128 / 256 tokens → 3.890 / 3.880 / 3.874 / 3.870 bpv. Prefilling half the tokens and appending the rest one by one costs ≤ 0.01 bpv over one-shot.
* **Modes:** `--rmse` (step = rmse·√12), `--maxabs` (exact bound; ×√2 for pre-RoPE K to cover the rotation back), and `--bpv` (rate-targeted, bisection on the step).

### Gate: SmolLM2-135M WikiText-2 ppl, whole KV cache replaced (CPU, 4096 tokens, ref ppl 16.544)

Rates are real bytes for tang and SZ3. KIVI is the quantizer only, swept over bits 2–8 and groups 32/64/128 (no fp16 recent-token window). Each cell is the cheapest measured config that meets the bound.

| | Δppl ≤ 0.1 | Δppl ≤ 0.5 |
|---|---|---|
| **tang kvcodec** (K at 0.5× V's relL2) | **3.39 bpv** (Δ +0.069) | **2.64 bpv** (Δ +0.411) |
| KIVI-2 (g32/64/128) | not reached (best Δ +3.17 at 3.00) | not reached |
| KIVI-4 (g32/64/128) | not reached (best Δ +0.209 at 5.00) | 4.25 (g128, Δ +0.344) |
| KIVI, any bits/group | 5.25 (5-bit g128, Δ +0.050) | 4.00 (3-bit g32, Δ +0.480) |
| SZ3 abs | 5.11 (eb 0.07 rms, Δ +0.065) | 4.02 (eb 0.15 rms, Δ +0.251) |
| tang wavelet (Phase 1, as-is) | — | — (3.03 bpv → Δ +0.85) |

**Gate: PASS.** −35 % bpv vs the best KIVI config and −34 % vs SZ3 at Δ ≤ 0.1. −34 % vs both at Δ ≤ 0.5. Log-linear interpolation between sweep points puts the bound-hitting rates at ≈ 3.31 and ≈ 2.48 bpv. Run-to-run Δppl noise is about ±0.02.

Full tang curve (bpv → Δppl): 3.87 → +0.036, 3.39 → +0.069, 3.12 → +0.180, 2.89 → +0.273, 2.64 → +0.411, 2.33 → +0.582, 1.96 → +0.977.

### Bits/value vs relative L2 (32 KV dumps, median of per-tensor Pareto fronts)

| codec | bpv @2 % | @5 % | @10 % |
|---|---|---|---|
| **tang kvcodec, pre-RoPE** | **5.07** | **3.75** | **2.78** |
| tang kvcodec, post-RoPE | 5.18 | 3.87 | 2.88 |
| tang wavelet (Phase 1) | 5.46 | 4.10 | 3.08 |
| SZ3 abs | 6.00 | 4.69 | 3.70 |
| KIVI g128 | 6.23 | 4.95 | 4.01 |
| KIVI g32 | 6.58 | 5.31 | 4.39 |

Per-tensor points are in `bench/out/kv_curves.csv`; ppl sweep rows are in `bench/out/kv_ppl.csv`. Both are gitignored, so rerun to regenerate.

### Cost (CPU, M4, single thread)

| | ns / value | note |
|---|---|---|
| kvcodec encode | 36–44 | one-shot, incl. header fit |
| kvcodec decode | 27–29 | full stream; the latest token's block (128 tok × 192 ch) takes 0.35–0.49 ms |
| kvcodec append | ~7 µs / token | amortized; a block is coded when it seals |
| SZ3 decode (pysz) | 5–6 | |

The decoder is bit-serial binary arithmetic coding and about 5× slower than SZ3's. Speeding it up (rANS with per-context tables, or many independent per-head/per-block lanes on GPU) is the obvious next step and was not attempted. For a decode-time cache, attention reads would decode whole 128-token blocks, or keep a dequantized hot window.

### Quick look: Adam state as the next tensor type (`bench/adam_look.py`, ideal entropy, 7 matrices, 512-row crops)

The metric is relL2 of the Adam update `m/√v` with one buffer compressed. Phase-1 relL2 on raw v understates how bad linear-domain coding of v is.

| codec | bpv @ update relL2 1 % | @5 % | @10 % |
|---|---|---|---|
| v: uniform step, linear | — | 10.85 (3/7) | 10.11 (3/7) |
| v: uniform step on log v | 6.42 | 4.10 | 3.12 |
| **v: log v = aᵢ + bⱼ (Adafactor rank-1, f16) + quantized residual** | **5.72** | **3.51** | **2.57** |
| m: uniform step, linear | 8.37 (4/7) | 6.02 | 5.00 |
| **m: quantized in the update domain (step ∝ √v̂)** | **6.59** | **4.28** | **3.35** |
| m: rank-4 / rank-16 SVD + residual | 8.34 (4/7) / 8.61 (5/7) | 5.94 / 6.25 | 4.93 / 5.24 |

Reading: log-domain coding plus an Adafactor-style rank-1 log offset is the right start for `exp_avg_sq`, about 4× cheaper than linear at matched update error. For `exp_avg`, the win is error weighting by 1/√v, not low rank: after 3 steps m is not low rank, and rank-k factors cost more than they remove. Next steps: a temporal delta across checkpoints and a real coder (the kvcodec context machinery carries over).

### Reproduce

```
ln -s <data>/bench/tensors bench/tensors   # SmolLM2 weights + tokens.npz + kv/ dumps
cargo build --release -p cram-codec --example kvc --example tc
UVW="uv run --no-project --python 3.12 --with numpy --with pysz --with zstandard --with torch"
$UVW python bench/kv_ppl.py tkvk kivi sz3   # ppl sweep, ~1 min/config on CPU
$UVW --with zfpy python bench/kv_bench.py   # curves + cost
$UVW python bench/adam_look.py
```

## Phase 3 — KV codec: fair baselines, real model sizes, in-loop cost

Branch `claude/tc3-kv-phase-730c4c` (tang, from `claude/cram-qtt`; not pushed). Harness: `bench/kv_hf.py` (HF transformers, model-generic), baselines `bench/kv_base.py`, summary `bench/kv_summary.py`. GPU: `cram_codec::kvgpu`, `tang-compute` `tkv_decode` / `kivi_dequant` (MSL, `kernels/kvdec_msl.rs`), `tang-llm` `kvcomp` + `examples/kvcomp_bench.rs`.

### Harness

* Clean pass per window captures every layer's **pre-RoPE K** (the model's `apply_rotary_pos_emb` is patched to record its input), post-RoPE K and V. Override pass: every layer's K/V for the compressed span is replaced by the codec's reconstruction. Pre-RoPE codecs are re-rotated with the model's own cos/sin. Attention runs in fp32.
* **KIVI's fp16 window is applied per query**: query t attends to the exact key/value s when t − s < R (R = 128, the paper default), else to the reconstruction. Its 16-bit cost is charged at the eval length (1024 or 2048 tokens). That costs KIVI ~1–2 bpv at these lengths, so the tables also give KIVI's quantizer-only rate ("window free"), the long-context limit.
* Reproduces Phase 2 exactly on SmolLM2-135M: ref 16.5442, tkv-0.1 2.887 bpv / Δ +0.273, KIVI-4 g32 no window Δ +0.209. `identity` config Δ = 0.000 (fp32) and −0.004 (7B, bf16 weights).
* **Step rule fix.** Qwen2.5 K has a huge per-channel constant (layer 0: rms 69, centered rms 1.0). A step set from the raw rms wrecks ppl (Δ +300 at 3 bpv). The codec's step is now `rel × centered rms` (per-(head,channel) mean removed). A K channel constant is invisible to softmax and the codec codes the mean anyway. With this rule the K/V split moves: K at the same rel as V (`tkvk1.0`) is best on all three models. "tang (any split)" below is the front over splits K/V ∈ {0.5, 0.7, 1.0, 1.4} and both layouts. "tang kx=1.0" is one fixed rule.

### Baselines (numpy re-implementations from the papers; all side info counted)

* **KIVI** (Liu et al. 2024, arXiv 2402.02750): K per-channel / V per-token asym groups g ∈ {32, 128}, 2–8 bits, fp16 scale+zero, R ∈ {0, 128}.
* **KVQuant-style** (Hooper et al. 2024, arXiv 2401.18079): pre-RoPE K with offline per-channel ranges (calibration window disjoint from eval), V per-token online. Per-layer non-uniform datatype by 1-D k-means on calibration data (Fisher weighting omitted). 1 % dense-and-sparse outliers in fp16 + 16-bit index. Sink token fp16.
* **QuaRot-style Hadamard** (Ashkboos et al. 2024, arXiv 2404.00456): post-RoPE Walsh–Hadamard over head_dim, per-(head,token) asym, group hd or 32, clip 1.0/0.9.
* **TurboQuant-mse** (Zandieh et al. 2025, arXiv 2504.19874): fp16 norm, random rotation, Lloyd–Max for N(0, 1/hd). No QJL residual, no outlier-channel split.

### Gate 1 — SmolLM2-135M, WikiText-2, 4 × 1024 tokens, whole cache replaced (ref ppl 16.544)

Cheapest bpv at Δppl ≤ 0.1, log-linear interpolation on each family's Pareto front (measured point in brackets).

| codec | bpv @ Δ ≤ 0.1 | bpv @ Δ ≤ 0.5 |
|---|---|---|
| **tang kvcodec (any split)** | **3.19** (3.21, Δ +0.095) | **2.73** |
| tang, fixed rule kx = 1.0 | 3.19 | 2.90 (meas.) |
| QuaRot-style Hadamard, g = hd | 4.35 (4.50, Δ +0.066) | 3.78 |
| KVQuant-style (NUQ + 1 % outliers) | 4.85 (5.61, Δ +0.025) | 3.59 |
| TurboQuant-mse | 5.35 (6.25) | 3.92 |
| KIVI, no window (best g) | 5.13 (5.25) | 3.98 |
| KIVI, R = 128 fp16 window, bits charged at T = 1024 | 6.17 (6.38, g32 b4, Δ +0.075) | 5.25 |
| KIVI R = 128, window free (quantizer bits only) | 5.00 (g32 b4) | 4.00 |

**Gate 1: PASS.** −26.6 % bpv vs the best new baseline (Hadamard) and −36 % vs window-free KIVI at Δ ≤ 0.1 (gate ≥ 15 %).

### Gate 2 — real sizes (WikiText-2, 4 × 2048 tokens)

Qwen2.5-1.5B (fp32, Metal) and Qwen2.5-7B (bf16 weights streamed layer by layer to the 3090: ~2.7 GB GPU; the box had ~4 GB free).

| | SmolLM2-135M | Qwen2.5-1.5B (ref 9.978) | Qwen2.5-7B (ref 5.983) |
|---|---|---|---|
| **tang (any split)** | **3.19** | **3.25** | **3.29** |
| tang, fixed kx = 1.0 | 3.19 | 3.38 | 3.43 |
| KVQuant-style | 4.85 | 4.54 | 4.31 |
| KIVI R = 128 (window charged at T) | 6.17 | 4.56 | 4.51 |
| KIVI R = 128 window free | 5.00 | 4.00 (g32 b3, Δ +0.068) | 4.00 (g32 b3, Δ +0.070) |
| Hadamard (QuaRot-style) | 4.35 | not reached (Δ +3.5 at 6.25) | not reached (Δ +0.81 at 6.25) |
| TurboQuant-mse | 5.35 | not reached (Δ +15 at 6.1) | not reached (Δ +6.8 at 6.1) |
| **fewer bits vs best baseline** | **26.6 %** | **28.4 %** | **23.7 %** |
| fewer bits vs window-free KIVI | 36 % | 19 % | 18 % |

At Δ ≤ 0.5: 1.5B 2.48 vs 3.25 (KVQuant, −24 %); 7B 2.57 vs 3.14 (−18 %).

**Gate 2 (bits): PASS.** The advantage vs the best charged baseline is 23.7–28.4 %, within 5 points of the 135M 26.6 %. Caveat: against KIVI with its window treated as free (the very-long-context limit), the margin on Qwen shrinks to 18–19 %. Rotation methods without outlier handling collapse on Qwen2.5: its K bias dominates every per-token range.

**Long context: needle in a haystack, Qwen2.5-1.5B (bf16, 3090).** The haystack is WikiText. The needle is "The special magic number for <city> is <7 digits>." at depths 10/30/50/70/90 %, with lengths 4K/8K/16K, 15 cases per config. The **whole context cache is compressed**: it is captured exactly, compressed, and the question + answer then attend to the reconstruction. A case is correct when the teacher-forced argmax matches every answer token (equivalent to an exact greedy match); answer NLL is the mean over cases. bpv is the mean over cases, with KIVI's window charged at the case length.

| config | bpv | 4K | 8K | 16K | answer NLL (4K / 8K / 16K) |
|---|---|---|---|---|---|
| exact bf16 | 16 | 5/5 | 5/5 | 5/5 | 0.088 / 0.065 / 0.045 |
| **tang kx=1.0, rel 0.085** (ppl Δ +0.044) | **3.55** | 5/5 | 5/5 | 5/5 | 0.083 / 0.061 / 0.042 |
| **tang GPU layout, K 0.7** (ppl Δ +0.090) | **3.31** | 5/5 | 5/5 | 5/5 | 0.133 / 0.062 / 0.046 |
| **tang kx=1.0, rel 0.15** (ppl Δ +0.17) | **2.75** | 5/5 | 5/5 | 5/5 | 0.102 / 0.062 / 0.048 |
| KIVI-3 g32 R128 (ppl Δ +0.068) | 4.22 | 5/5 | 5/5 | 5/5 | 0.074 / 0.067 / 0.046 |
| KIVI-2 g32 R128 (ppl Δ +0.51) | 3.24 | 2/5 | 3/5 | 4/5 | 1.42 / 0.95 / 1.09 |
| KVQuant-style 4-bit (ppl Δ +0.12) | 4.43 | 5/5 | 5/5 | 5/5 | 0.144 / 0.079 / 0.057 |
| KVQuant-style 5-bit | 5.43 | 5/5 | 5/5 | 5/5 | 0.145 / 0.066 / 0.047 |

The task is easy for this model: everything at ≥ 3.3 bpv retrieves perfectly. The informative points are the low-rate ones. At ~3.2–3.3 bpv, tang is perfect with exact-level NLL, while KIVI-2 at the same rate fails 6 of 15 cases. tang at 2.75 bpv is still 15/15. So the long-context result agrees with the ppl gate. (A first KIVI run scored 1/15 because of a harness bug: the per-query fp16 window read keys the NIAH pass does not compute. It was fixed and rerun; ppl runs were unaffected because they compute every row.)

### Gate 3 — in-loop cost (tang-llm, Metal, M4 Max)

**GPU layout.** A header flag (`gpu`) switches kvcodec to integer reconstruction: values are i32 multiples of `u = step/sink_div`, prediction `mu + (rho·(prev−mu)) >> 4`, and only the final `float(q)·u` is float. Every 128-token block is coded as one adaptive-arithmetic substream per head. One GPU thread decodes one (stream, block, head) lane, with its context models in a device scratch. The MSL kernel's output is **bit-identical** to `KvStream::decode_range`, checked on every run of `tang-compute/examples/kvdec_bench.rs` (SmolLM2 K and V, 1–1024 copies, blocks of 32 and 128). Rate is the same as the CPU layout within noise ("tkvgpu" rows; 7B 3.36 bpv at Δ +0.091).

**Decode throughput** (one dispatch, SmolLM2 L15 V at 3.47 bpv):

| lanes | 24 | 384 | 1 536 | 6 144 | 24 576 |
|---|---|---|---|---|---|
| Gvalues/s | 0.017 | 0.26 | 0.83 | 2.30 | 2.78 |

That is ~100× one CPU core (25–30 ns/value) once there are ≥ 20 k lanes. One GPU thread manages only ~0.7 Mvalues/s, because the bit-serial decoder is latency bound.

**In the inference path.** `Model::compress_kv` compresses a cache's whole 128-token blocks, keeping K pre-RoPE: it rotates K back with the model's own tables, and the kernel rotates it forward with the same tables. Each decode step, right before a layer's attention, the layer's rows are decoded back into the paged pool. The tail and new tokens stay bf16. Qwen3-0.6B-Base, bf16 weights, teacher-forced WikiText continuation, 16 steps (NLL over 16 tokens is noisy; quality comes from the gates above):

| ctx | mode | bpv | ms/step | tok/s | vs bf16 |
|---|---|---|---|---|---|
| 2048 | bf16 | 16 | 6.6 | 151.6 | — |
| 2048 | KIVI-4 g32 dequant/step | 5.0 | 9.6 | 103.9 | −31 % |
| 2048 | KIVI-2 g32 dequant/step | 3.0 | 9.8 | 101.7 | −33 % |
| 2048 | tang, 32-token lanes | 3.57 | 569 | 1.76 | −99 % |
| 2048 | tang, 16-token lanes | 3.59 | 349 | 2.86 | −98 % |
| 8192 | bf16 | 16 | 19.7 | 50.8 | — |
| 8192 | KIVI-4 | 5.0 | 22.6 | 44.2 | −13 % |
| 8192 | KIVI-2 | 3.0 | 29.2 | 34.3 | −33 % |
| 8192 | tang, 32-token lanes | 3.56 | 657 | 1.52 | −97 % |
| 8192 | tang, 16-token lanes | 3.58 | 493 | 2.03 | −96 % |

(A first 2048 run of the same bench: bf16 90.7, KIVI-4 92.8, tang 128-token lanes 0.54 tok/s. M4 times drift ±2×. The KIVI numbers are dequant-then-attend, not a fused kernel.)

**Gate 3: FAIL on throughput (≤ 20 % loss not met); tradeoff reported.** Decoding the whole compressed cache every step costs 25–50× the bf16 step. Per-layer dispatches have only (blocks × heads × 2) lanes, so the GPU idles. Memory vs speed:

* **bf16 resident:** 16 bpv, full speed.
* **KIVI int dequant per step:** 3–5 bpv charged (plus the window at short contexts), −13 to −33 %. Its quality at 3 bpv is far worse than tang at 3.3 (gates 1–2).
* **tang decode per step:** 3.2–3.6 bpv, ~2 tok/s. Not usable for interactive decode in this form.
* **tang as cold/parked storage** (prefix cache, kvstore spill, session swap): decode once on resume at ~2.8 Gvalues/s. That is ~0.17 s for an 8K-token Qwen3-0.6B cache (470 M values), vs re-prefill, at 4.5–5× less memory than bf16. This is where the codec pays today.

What would move it: decode only once per sealed block into a bf16 working set (memory then = working set + compressed cold), rANS-style interleaved lanes (as `gpuwav`) instead of bit-serial abac, and context models in threadgroup memory. A fused int4 attention is the fair speed baseline for KIVI.

Note: the Mac crashed while `kvdec_bench --copies 2389` (≈ 57 k lanes, ~170 MB scratch) was running. That run's result was never recorded, and it may have caused the crash. All later runs are on mew. The ≥ 20 k-lane numbers above come from the earlier, completed runs.

## Phase 4: KV codec decoded every generation step (cut)

Branch `claude/kv-decode-speed` in ecto/tang, commit `a1c9e7a` (PR #103). Full write-up: `docs/compression-math-survey/tang-compress-kv-decode-speed.md` on that branch.

All runs were on the M4 Max with Metal, before the no-Mac rule arrived. Nothing ran on mew.

Rows are Qwen2.5-1.5B, step in ms, 2K / 8K context. "Resident" is the memory the cache occupies during generation, in bits per value. Quality is matched: tkv's NLL is within ±0.005 of bf16 at 8K.

| idea | bpv | ms/step 2K / 8K | resident bpv | notes |
|---|---|---|---|---|
| bf16 | 16 | 9.6 / 12.6 | 16 | |
| KIVI-4 g32 | 5.00 | 10.4 / 15.0 | 5 | NLL +0.003 at 8K |
| KIVI-2 g32 | 3.00 | 10.4 / 14.9 | 3 | NLL +0.05 to +0.25 across both models and contexts |
| tkv PR #73 layout (128-token × whole-head lanes) | 3.83–3.85 | 1577 / 1570 | 3.8 | lane-starved: 48 threads per dispatch at 2K (on SmolLM2) |
| (1) decoded-tile cache | 3.83–3.85 | 10.0 / 12.3 | 16 + 3.8 | runs at bf16 speed but saves no memory: storage use case only |
| (2) 32×4 lanes | 4.15–4.18 | 25.5 / 38.6 | 4.2 | +8% bits; 15–17% fewer bits than KIVI-4 |
| (2) 16×2 lanes | 5.01–5.04 | 17.5 / 41.7 | 5.0 | +31% bits; no saving vs KIVI-4 |
| (3) fused decode + attention (bound: stores removed) | 5.04 | 17.4 / 41.4 | 5.0 | fusion can save at most 1–3% |
| (4) 1024-token raw window + 16:2 | 5.01–5.08 | 15.6 / 39.8 | ~5 | saves only the window's share of the work |

Needle retrieval: 5/5 at 2K and 8K for every mode, KIVI-2 included, so the test doesn't separate them.

**Verdict:** there is a documented limit. The adaptive binary arithmetic decoder tops out at about 4 Gvalue/s in the model, the same as the kernel alone. KIVI-level overhead at Qwen2.5 8K needs about 50 Gvalue/s.

**Not reached:** the parallel-decodable variants (static-table or interleaved rANS, DPCM as a parallel scan) and any CUDA/mew measurement.

The release claim stays "KV codec for storage, offload and transfer."
