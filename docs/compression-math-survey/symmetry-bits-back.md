# Symmetry bits-back on the fx2-cmix-transformer weights

Target: Hutter Prize enwik9. Record is fx2-cmix-transformer at 100,424,672 B, so the 1% threshold is about 1,004,247 B.
Model: `models/6m-q4-fp32.tfwc2` (2,930,652 B). The question is how much of that file is redundant because the network has exact symmetries.

Scripts: `/Users/cam/Developer/hutter/work/symmetry/sym.py`. A stub `fla` package in `work/symmetry/stub/` lets `read_tensor_file` import. Run from `/Users/cam/Developer/hutter` with
`PYTHONPATH=work/symmetry/stub:src venv/bin/python work/symmetry/sym.py`.

## 1. Papers

### (a) He, Flamich, Hernández-Lobato, "Getting Free Bits Back from Rotational Symmetries in LLMs" (arXiv 2410.01309)

- **Method.** The paper does bits-back coding over the O(d) rotation gauge, a symmetry that only exists *after SliceGPT*. SliceGPT folds the norm gains into the adjacent weights and adds rotation matrices on the skip connections, so RMSNorm commutes with rotations. A plain transformer does not have this symmetry, which the paper states explicitly.
- **Encoding.** Rotate each block to a canonical direction, chosen by eigendecomposition so that a chosen weight product is diagonal. Decode a random rotation from the bitstream: decode a symmetric matrix as d(d+1)/2 floats, eigendecompose it, and re-encode the eigenvalues, so the net cost is d(d−1)/2 floats. Apply the rotation, encode the rotated weights, and store d sign bits so the eigenvectors can be recovered.
- **Coding details.** It uses a *fixed-length* float16 code rather than ANS, so a numerical error stays local instead of desynchronising the stream. A correction code sends (position, true value) pairs for reconstructed weights whose error exceeds a threshold: 0.01 for OPT, 0.005 for Llama.
- **Savings.** 3–5% of total bits on top of SliceGPT, for OPT-1.3B through 13B and Llama-2-7B. Example: OPT-13B at 30% slicing goes from −21.29% to −24.97%. PPL and accuracy are unchanged within noise (Llama-2-7B 30% slicing: PPL 8.63→8.69). The theoretical estimate is larger, but the head and embedding layers dilute it.
- **Cost.** Training-free, runs on a consumer CPU (Ryzen 7950X). Costs are eigendecompositions per block plus the correction code.
- **Limitations.** It assumes **float** weights. The rotated weights are not exactly representable, which is why the correction code exists. The authors say outright that making it work for extremely quantized networks is an open problem. The symmetry only appears with SliceGPT-style rotation-equivariant norms.
- **Relevance here.** None directly. Our weights sit on a 15-level integer grid with per-row bf16 scales, and a generic rotation leaves that grid. Rotating to floats would cost far more than log|O(d)|.

### (b) Flamich, Gündüz et al., "Data Compression with Stochastic Codes" (arXiv 2602.07635, REC survey)

- **Method.** A survey of stochastic codes and relative entropy coding (REC): encoder and decoder share randomness, and the encoder transmits an exact *sample* from q(w|x) at a cost of about KL(q‖p) bits. Algorithms covered:
  - rejection sampling
  - the Poisson functional representation (A* coding)
  - greedy rejection samplers
  - branch-and-bound A*
  - dithered and layered quantisation
  - approximate REC
- **Encode-time cost.** This is the central limitation. Any general selection sampler has expected runtime of at least exp(D∞[q‖p]), which grows exponentially in the information content. Entropy coding grows linearly. For 1-D unimodal targets, branch-and-bound A* reaches runtime roughly linear in D∞. Dithered and layered quantisers are fast for uniform/additive channels. High-dimensional problems are split into many small blocks, as in MIRACLE/RECOMBINER and the DiffC per-step chain. One work cut image encode time from hours to seconds with a custom CUDA kernel.
- **Note on extraction.** The survey's equations were not recoverable from the HTML (math was stripped), so the exact codelength-overhead constants are not reproduced here.
- **Neural-weight results cited.**
  - Bayesian INRs (COMBINER/RECOMBINER): weights are drawn from a learned posterior and REC-coded.
  - FedPM federated mask training: per-round cost drops from about 1 bit/parameter to KL(local‖global). This gives up to 82× bitrate reduction versus sending the mask directly, and over 2,600× overall.
  - The survey gives no bits/weight result for a fixed, pretrained, quantized network.
- **Limitations (survey §IV).** Speed: known practical REC algorithms are too slow or too limited. Synchronisation: the shared PRNG state must match exactly, or decoding fails catastrophically.
- **Relevance here.** REC is a *lossy* channel: it transmits a random sample, not the given weights. Using it for Hutter would mean retraining so that "any sample from q" compresses enwik9 as well, a MIRACLE-style variational objective. The saving would be the posterior entropy that bits-back recovers, and it does not apply to the existing record weights. It is not a drop-in recoding.

## 2. Architecture (from `training_recipes/quantize.py`, `pysrc/model.py`, tensor list of `work/weights.bin`)

- Byte-level vocabulary of 205; d_model = 192; 12 layers. `kimi_linear=[T,T,T,F]*3`, so 9 KDA (Kimi Delta Attention) layers and 3 softmax layers (3, 7, 11).
- Softmax attention: 3 heads × d_head 64, n_kv_heads 3, sliding window 1024. RoPE base 1e4 with 32 frequencies per head. QK-norm is `rms_norm` **without gain**. Per-head int8 quantizers on Q/K/V (`quantize_queries.scale` (3,)).
- KDA layers:
  - q/k/v are 192×192, each followed by a depthwise conv of width 4 with fused SiLU (`(192,4)` float32).
  - Forget gate and output gate are low-rank: 192→64→192, with no nonlinearity between the two factors.
  - `beta_projection` (3,192) is float. `A_log` (3,), `dt_bias` (192,).
  - `use_qk_l2norm_in_kernel=True`.
  - `output_fused_norm_gate.weight` (64,) is shared across the 3 heads.
- MLP: 192→768→192 with ReLU². Every block uses pre-norm `rms_norm(x)` **without gain**.
- Other components:
  - Embedding norm, plus a prior (PPMD) embedding 205→192.
  - U-net skip connections with 6 scalar weights. Each block also has scalar residual and token-embedding coefficients.
  - Untied unembedding 205×192, with logit softcap 15.
- Quantization: weights use 15 levels in −7..7, round-to-nearest. Each matrix has one bf16 scale **per output row**; the scale shape equals the row count, e.g. `mlp.up` (768,), `mlp.down` (192,). Activations are 8-bit, `buckets=256`, so qmin −128 and qmax +127, an **asymmetric** clamp. Each activation scale is a single scalar per tensor (`quantize_activation.scale` (1,)), so the whole input vector shares one scale.

## 3. Which symmetries hold exactly for the stored quantized model

General rule: a transformation T is a symmetry only if it maps every stored integer to an integer in −7..7, maps every bf16 scale to a stored bf16 scale, and leaves the network function unchanged.
- **Permutations** of a channel that every tensor indexes consistently pass this test. Rows move together with their per-row scale, and per-tensor activation scales do not care about order.
- **Sign flips** keep the weight grid, because it is symmetric (−7..7) and round-half-even is odd. They break at the activation quantizer's −128/+127 clamp whenever an activation saturates. They are also not symmetries across any non-odd nonlinearity (ReLU², SiLU, softmax inputs to ReLU…). They are listed as *conditional*.
- **Continuous gauges are broken by the integer grid.** This covers GL/O(d) on Q/K, V/O, the residual stream, and the low-rank pairs: a generic rotation produces non-integer weights. Only the finite subgroup that preserves the lattice survives, which is the signed permutations. Positive rescaling (ReLU² is degree-2 homogeneous) would need a per-column scale on `mlp.down`, and none exists, so it is broken. Rescaling between bf16 scale vectors and float conv weights is not exact in finite precision; it is tiny anyway and not counted.

| Symmetry | Why it is exact (or not) | Multiplicity | bits | bytes |
|---|---|---|---|---|
| Residual-stream permutation (192!) | No norm gains, no biases. Embedding/unembedding/prior columns, every input projection's columns, and every output projection's rows+scales are permuted together. Activation scale is per tensor. | 1 | 1,184 | 148 |
| MLP hidden-unit permutation (768!) | ReLU² is elementwise. `up` rows+scales and `down` columns move together. `down` input scale is a scalar. 0 duplicate units found, so the action is free. | 12 | 75,113 | 9,389 |
| KDA head permutation (3!) | Per-head A_log, beta rows, dt_bias blocks; the norm-gate weight is shared. | 9 | 23 | 3 |
| KDA key-dim permutation within head ((64!)^3) | q, k, g share the key index: q/k projection rows+scales, conv rows, forget-gate `down` rows, dt_bias. The L2 norm and the diagonal per-channel decay are permutation-equivariant. | 9 | 7,992 | 999 |
| KDA value-dim permutation (64!, one joint σ for all heads) | v rows, v conv, output-gate rows, out-proj columns. The shared (64,) norm-gate weight is permuted along, which forces the same σ for every head. | 9 | 2,664 | 333 |
| KDA low-rank bottlenecks, forget + output gate ((64!)^2) | `up` rows+scales and `down` columns. Linear in between, and the activation scale is a scalar. | 9 | 5,328 | 666 |
| Softmax head permutation (3!) | Per-head Q/K/V quantizer scales move along. | 3 | 8 | 1 |
| Softmax value-dim permutation within head ((64!)^3) | V rows+scales and O columns; per-head value scale. | 3 | 2,664 | 333 |
| **Exact total, one copy** | | | **94,976** | **11,872** |
| *Conditional:* residual sign flips (2^192) | RMSNorm is odd-invariant, but the asymmetric −128/127 activation clamp can break it. | 1 | 192 | 24 |
| *Conditional:* KDA bottleneck sign flips (2^128 per layer) | Linear bottleneck; breaks only at clamp saturation. | 9 | 1,152 | 144 |
| *Conditional:* softmax V/O sign flips (2^192 per layer) | Same clamp caveat. | 3 | 576 | 72 |
| *Conditional:* RoPE-pair 90° rotations (C4 per pair, applied to q and k together) | RoPE commutes with SO(2) per pair, and 90° maps the grid to itself. Reflections are excluded because they reverse rotation direction. Same clamp caveat. | 3 | 576 | 72 |
| **Conditional total** | | | 2,496 | 312 |

Not symmetries:
- **Q/K channel permutations in softmax layers**: RoPE frequencies differ across pairs, so only the C4 above survives.
- **Sign flips through SiLU (KDA q/k/v) or ReLU² (MLP)**: these activations are not odd.
- **Vocabulary rows**: byte identity is fixed.
- **Layer order.**

Calculations: log2(768!) = 6,259.4, log2(192!) = 1,184.4, log2(64!) = 296.0, log2(3!) = 2.585, computed with lgamma.
- MLP: 12 × 6,259.4 = 75,113.
- KDA per layer: 2.585 + 3·296 + 296 + 2·296 = 1,778.6; × 9 = 16,007.
- Softmax per layer: 2.585 + 3·296 = 890.6; × 3 = 2,672.
- Residual: 1,184.
- Sum: 94,976 bits = **11,872 B per copy**, or **23,744 B for two copies** (the compressor's copy only matters if it counts toward the score; the Hutter score is archive + decompressor). Adding the conditional symmetries gives at most 12,184 B per copy.

## 4. Verdict vs the 1% threshold

The exact symmetry saving is 11.9 KB per copy (23.7 KB for both). That is **1.2% (2.4%) of the 1,004,247 B threshold**, about 0.4% of the weights file. It is roughly the same size as the ~12 KB already measured for lossless recoding. **It is far from sufficient on its own.** Even in the best case it covers only about 2–3% of the gap.

## 5. Practicality

- **No bits-back is needed in this setting.** The encoder and decoder ship together, so the compressor can simply *use* the canonicalized model: sort MLP units lexicographically, and so on. Permutations of integer matmuls do not change the int32 sums, so the compressed output is bit-identical. Float KDA paths may round differently, but both sides run the same canonical model, so this is harmless. The model then needs to be coded as a *multiset* of rows rather than a sequence. That ideally saves log2(n!) per group.
- **Shuffle coding** (Kunze et al., arXiv 2408.08837, "Practical Shuffle Coding"; cited by 2410.01309 via Kunze et al. ICLR 2024 for unordered structures) gets the same saving with an ANS bits-back step: decode a coset representative from the stream, then re-encode it. The overhead is the stabilizer, which is trivial here (0 duplicate units), plus the initial-bits cost. Either route works. Canonical sort plus a sorted-sequence (multiset) coder is simpler and deterministic.
- **Experiment.** Sorting the 768 MLP units per layer lexicographically (concatenated `up` row and `down` column) and measuring entropy with a column-wise order-1 context (previous row, same column):

  | | order-1 entropy |
  |---|---|
  | Original order | 1,676,116 B |
  | Sorted | 1,675,992 B |
  | Gain | **124 B** |

  Order-0 entropy is unchanged by construction (1,676,361 B for these tensors). A naive context model therefore does not capture the log2(768!) ≈ 782 B per layer (9,389 B total). Realising that saving needs a coder that actually exploits the sort constraint, for example coding the leading column as a non-decreasing sequence via run lengths or differences and then refining within ties, or ANS-based shuffle coding.
