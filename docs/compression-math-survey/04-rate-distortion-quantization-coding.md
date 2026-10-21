# 04 — Rate–distortion theory, transforms, quantization, entropy coding

> Scope: the classical lossy-coding stack (transform → quantize → entropy code) as mathematics, plus the 2016–2026 learned-codec and LLM-weight-quantization results that reuse it. Every section ends with a **cram** note.
>
> **Citation status.** I wrote this from memory of the primary literature, with no live search in this pass. The arXiv IDs below are ones I'm confident in. Anything marked **[unverified]** must be checked before anyone quotes it.

---

## 1. Shannon rate–distortion

**Statement.** For a source X and distortion measure d,

```
R(D) = min_{p(x̂|x) : E d(X,X̂) ≤ D}  I(X; X̂)
```

This is the operational minimum bits/sample at average distortion D. The coding theorem (Shannon 1959) says rates above R(D) are achievable and rates below it are not. R(D) is convex and non-increasing. The Blahut–Arimoto algorithm (1972) computes it for discrete sources by alternating minimization.

**Gaussian, MSE.** For a scalar source, R(D) = ½ log₂(σ²/D), i.e. 6.02 dB per bit. Shannon's lower bound for any source with differential entropy h(X) is R(D) ≥ h(X) − ½ log₂(2πe D). It is tight at high rate.

**Why it's elegant.** The bound is a single variational problem, and its dual (a Lagrangian in λ, R + λD) is exactly the loss that learned codecs minimize.

**Open.** Computing R(D) for real high-dimensional sources such as natural images is still open; see §6.

**cram.** `R + λD` is the universal objective, and λ should be a first-class parameter of every codec graph. Blahut–Arimoto is a fixed-point iteration over Exp2/Log2/Mul/Recip, so it can be expressed directly in tang-expr. That makes it a useful reference oracle for small discrete sources and for checking learned rate estimates in tests.

## 2. Reverse water-filling and KLT optimality

**Statement.** For independent Gaussian components with variances σᵢ²:

```
Dᵢ = min(θ, σᵢ²),   R = Σᵢ ½ log₂(σᵢ² / Dᵢ),   θ chosen so Σ Dᵢ = D
```

Components with variance below the "water level" θ get zero bits. For a correlated Gaussian vector, the Karhunen–Loève transform (eigenbasis of the covariance) decorrelates it, and water-filling over the eigenvalues achieves R(D). The KLT is therefore optimal for Gaussian sources under MSE. Under high-resolution assumptions it also maximizes coding gain, defined as the arithmetic mean over the geometric mean of the eigenvalues (Huang–Schultz 1963; Goyal, "Theoretical foundations of transform coding", IEEE SP Mag. 2001).

**Elegance.** Bit allocation reduces to one scalar (θ). The "log of the geometric mean" form shows why decorrelation is worth bits.

**Empirical.** The DCT approximates the KLT for first-order Markov sources with ρ→1, which is why JPEG, H.26x and AV1 use DCT-family transforms.

**Open.** The KLT is *not* optimal for non-Gaussian sources (Effros, Feng & Zeger 2004 gave counterexamples). This is the gap that nonlinear transforms fill (§6).

**cram.** Water-filling is a 1-D root find on θ. It is differentiable almost everywhere, since the `Select` op handles min(θ, σ²). Use it as the analytic baseline that every learned transform must beat on a Gaussian test source. Eigen-decomposition belongs in tang-la. Ship KLT/PCA bases as *data* in the bitstream header, never recomputed on the decoder, to keep decoding deterministic.

## 3. High-resolution quantization (Bennett, Gersho, Zador)

**Statement.** As the cell size goes to 0, the MSE of a k-dim quantizer with point density λ(x) is

```
D ≈ G(Λ) · ∫ p(x) λ(x)^{-2/k} dx          (Bennett/Gersho)
```

Zador's theorem (1982, IEEE T-IT) gives the optimal entropy-constrained distortion as

```
D ≈ G_k · 2^{(2/k)(h(X) − R)}
```

Here G_k is the best normalized second moment (NSM) of any k-dim cell. Gersho's 1979 conjecture is that optimal cells are congruent tessellating polytopes. It is proven for k=2 (hexagons, Fejes Tóth / Newman) and open in general.

**Key numbers (Conway & Sloane, *SPLAG*):**

| quantizer | NSM G | gap to sphere bound |
|---|---|---|
| Z (scalar) | 1/12 ≈ 0.08333 | 1.53 dB |
| A₂ hexagonal | ≈ 0.08019 | 1.37 dB |
| E8 | ≈ 0.07168 | 0.86 dB |
| Leech Λ24 | ≈ 0.06577 | 0.50 dB |
| sphere bound (k→∞) | 1/(2πe) ≈ 0.05855 | 0 |

The 1.53 dB figure corresponds to ≈0.254 bit/sample. It is the full "space-filling loss" of uniform scalar quantization plus ideal entropy coding at high rate, relative to R(D). (The table values are as I remember them from SPLAG. Recheck the third decimal.)

**Elegance.** Distortion factors into a *geometric* constant (G) times a *source* constant (h(X)). Shaping and granular gain separate cleanly.

**cram.** This explains why "uniform scalar quantizer + good entropy coder" is within 0.25 bit/sample of optimal at high rate. That pipeline should be the default, because it is also the easiest to make bit-exact. Vector quantization is only worth the complexity at low rate or when that 0.25 bit matters (LLM weights at 2 bits/param, §5).

## 4. Lattice quantization and sphere packing

**Statement.** A lattice quantizer Q_Λ(x) = argmin_{λ∈Λ} ‖x−λ‖ has Voronoi cells that are all congruent. Fast nearest-point algorithms exist for Dₙ, E8 (as D8 ∪ (D8+½)) and Leech (Conway & Sloane 1982; SPLAG ch. 20). Quantization (minimize NSM) and packing (maximize density) are different problems, but the same lattices win both in dims 8 and 24.

**Context.** Viazovska proved E8 is the densest sphere packing in ℝ⁸ (arXiv:1603.04246). Cohn, Kumar, Miller, Radchenko and Viazovska proved the same for Leech in ℝ²⁴ (arXiv:1603.06518). Both proofs use Cohn–Elkies linear-programming bounds with modular-form "magic functions". Viazovska received the 2022 Fields Medal. *Optimality of E8/Leech as quantizers (minimal NSM) is still unproven.*

**Open.** Optimal-NSM lattices in most dimensions. Learned "lattice transform coding" (Lei, Hassani, Saeedi Bidokhti, arXiv:2403.07320 **[unverified ID]**) puts a lattice quantizer in the latent space of an NTC.

**cram.** E8 nearest-point decoding needs only round, compare and select, so it is exactly reproducible in integer/fixed-point arithmetic. Recommendation: implement `Z^n`, `D_n`, `E8` quantizers in integer arithmetic on the half-integer grid (scale by 2) so CPU/GPU agree bit for bit. Treat Leech as optional.

## 5. Trellis-coded quantization and LLM weights (QuIP#, QTIP, AQLM)

**TCQ.** TCQ (Marcellin & Fischer, IEEE T-Comm 1990) borrows Ungerboeck's set partitioning. It uses an expanded scalar codebook split into 4 subsets, an N-state trellis restricts which subsets are allowed, and Viterbi finds the best path. Asymptotically it recovers most of the 1.53 dB granular gap at near-scalar decode cost. I recall ~1 dB for 8 states and ~1.3 dB for 256 states (Eyuboglu–Forney's granular-gain analysis) **[unverified figures]**. Decoding is a shift register plus table lookup, so it is cheap and sequential-friendly. Encoding is Viterbi.

**LLM weight quantization (2023–2025):**
- **QuIP** (Chee et al., arXiv:2307.13304). Introduces "incoherence processing": random orthogonal rotations make weight and Hessian entries roughly Gaussian and spread out, after which simple rounding works well at ~2 bits.
- **QuIP#** (Tseng et al., arXiv:2402.04396). Replaces the random rotations with randomized Hadamard transforms (fast and structured) and quantizes 8-dim weight blocks with the **E8P** codebook, an E8-lattice-based code, at 2 bits/weight plus fine-tuning. This is a direct application of §4: incoherence makes weights ≈ i.i.d. Gaussian, and E8 is a near-ideal ball-shaped codebook for that.
- **QTIP** (Tseng et al., arXiv:2406.11235). Replaces the VQ with high-dimensional **TCQ** using a "bitshift trellis" and *computed* codes: a hash-like function of the trellis state yields pseudo-Gaussian reconstruction values, so no large codebook has to sit in cache. It reports better quality than QuIP# at 2–4 bits with fast GPU decoding. Exact perplexity numbers are in the paper and not reproduced here.
- **AQLM** (Egiazarian et al., arXiv:2401.06118). Additive/multi-codebook quantization: each weight block is the sum of M learned codewords, learned with beam search plus fine-tuning. It is competitive at ~2 bits.

**Elegance.** A random rotation turns arbitrary data into an approximately Gaussian source, so you can then apply known-optimal Gaussian quantizers. That is a "universalization by transform" trick.

**Open.** Rate allocation across layers, and joint quantization with the Hessian metric (all of these use GPTQ/LDLQ-style Hessian-aware rounding).

**cram.** This is the strongest single recipe to adopt for tensor and weight compression:
1. Randomized Hadamard with a seeded sign vector. It is integer-friendly, and the seed goes in the header.
2. Hessian-aware (LDLQ) rounding, where tang AD supplies the proxy Hessian.
3. A bitshift-trellis TCQ whose decoder is a pure integer function of the state.

The computed-code idea matches tang's codegen well, because the decoder is a small expression graph emitted to WGSL/MSL/CUDA. Keep it in integer ops to stay bit-exact.

## 6. Dithered and universal quantization ↔ additive-noise training proxies

**Statement.** With subtractive dither U ~ Unif[−Δ/2, Δ/2] shared by encoder and decoder,

```
Q(x + U) − U  =_d  x + U'      (U' uniform, independent of x)
```

This holds exactly, for any input distribution (Schuchman 1964; Ziv 1985; Zamir & Feder 1992). Ziv showed that entropy-coded dithered lattice quantization is within a constant (≈0.754 bit/sample for scalar) of R(D) for *any* source. Zamir & Feder showed the gap shrinks toward ½ log₂(2πe G) for good lattices.

**Link to learned codecs.** Ballé et al. replace rounding with additive uniform noise during training (arXiv:1611.01704; hyperprior arXiv:1802.01436). That substitution is exactly the dithered-quantization identity, used as a training proxy. Agustsson & Theis (arXiv:2006.09952) made it operational: they train *and test* with universal (dithered) quantization, so there is no train/test mismatch. Without dither, the mismatch is the source of the "straight-through vs noise" debates.

**Open.** The best surrogate gradient for hard quantization. Shared-randomness costs in practice.

**cram.** Dither must come from a counter-based PRNG (e.g. Philox or a SplitMix64 hash keyed by (seed, index)) implemented in integer ops, so the encoder and decoder regenerate identical U on any backend. Training: implement both the noise proxy and the straight-through estimator as interchangeable `Quantize` graph nodes with custom VJPs.

## 7. Nonlinear transform coding (NTC) and estimating R(D) of real sources

**NTC (Ballé, Chou, Minnen, Singh, Johnston, Agustsson, Hwang, Toderici, arXiv:2007.03034).** The review frames learned codecs as an analysis transform g_a, a quantizer, a learned entropy model p(ŷ) (factorized or hyperprior or autoregressive), and a synthesis transform g_s. They are trained on

```
L = E[−log₂ p(⌊g_a(x)⌉)] + λ E[d(x, g_s(⌊g_a(x)⌉))]
```

That is a variational autoencoder whose ELBO equals R + λD under the uniform-noise posterior. The review also shows that on simple 2-D sources NTC learns quantization cells that track the source manifold, which linear transform coding cannot do.

**Estimating R(D) of real data.** Yang & Mandt, "Towards empirical sandwich bounds on the rate–distortion function" (ICLR 2022, arXiv:2111.12166), give learned upper bounds (β-VAE style) and a dual lower bound, applied to GAN-generated images and natural images. Their reported conclusion, which I recall but have **not verified** in figures, is that state-of-the-art neural codecs on natural images may be ≳1 dB PSNR from R(D) at some rates. Follow-ups estimate R(D) with neural optimal transport and Wasserstein-gradient-flow variants of Blahut–Arimoto (Yang, Eckstein, Nutz, Mandt, "Estimating the rate-distortion function by Wasserstein gradient descent", NeurIPS 2023 **[arXiv ID unverified]**).

**Determinism.** A learned entropy model must give *bit-identical* CDFs on encoder and decoder, or arithmetic decoding desynchronizes catastrophically. Ballé, Johnston & Minnen, "Integer networks for data compression" (arXiv ID unverified) is the canonical fix: the entropy-model network runs in integer arithmetic.

**cram.** This section is the core design constraint. Train in float with AD. For deployment, *lower the entropy-model subgraph to integer ops* (quantized weights, integer accumulation, fixed-point division via `Recip` lookup) and emit it through codegen as integer-only kernels. The analysis/synthesis transforms can stay float, since a decoder-side mismatch there only perturbs reconstruction. Anything that produces probabilities must be integer.

## 8. Lifting and integer-to-integer transforms

**Statement (Sweldens 1996; Daubechies & Sweldens, J. Fourier Anal. Appl. 1998).** Every FIR biorthogonal wavelet (polyphase matrix with det 1) factors via the Euclidean algorithm into alternating predict/update steps:

```
d ← d − P(s);   s ← s + U(d)
```

Each step is trivially invertible, even after rounding: s ← s + ⌊U(d)⌉ is undone by s ← s − ⌊U(d)⌉. That gives exact integer-to-integer wavelets (Calderbank, Daubechies, Sweldens & Yeo 1998). JPEG 2000 lossless uses the reversible LeGall 5/3 this way. Any orthogonal or unimodular matrix factors into lifting/shear steps, which also yields integer DCTs and rotations (three-shear rotation).

**Elegance.** Perfect reconstruction is structural rather than numerical: it doesn't depend on floating-point accuracy at all.

**Learned variants.** iWave and iWave++ (Ma et al.) and the integer discrete flows of Hoogeboom et al. (arXiv:1905.07376) make the P and U operators neural nets.

**cram.** This is the most important primitive for bit-exactness. A `Lift{predict: Expr, update: Expr}` node whose inverse is auto-generated by negation guarantees lossless invertibility no matter how P and U are computed, *provided P and U themselves are deterministic integer functions*. Build lossless modes and integer Hadamard/DCT stages from lifting. For training, the rounding inside a lift uses a straight-through gradient.

## 9. Sparse approximation and compressed sensing

**Matching pursuit (Mallat & Zhang, IEEE TSP 1993).** Greedily pick the atom with the largest |⟨r, gᵢ⟩| and subtract it. OMP re-orthogonalizes after each pick.

**Compressed sensing.** If A satisfies the RIP of order 2s with δ₂ₛ < √2 − 1, then ℓ1 minimization (basis pursuit) recovers every s-sparse x exactly, and recovers approximately sparse x stably (Candès, Romberg & Tao, arXiv:math/0409186; Candès & Tao, arXiv:math/0502327; Donoho 2006). Random Gaussian A satisfies RIP with m = O(s log(n/s)) measurements. Later work improved the constant to δ₂ₛ < 1/√2 (Cai & Zhang 2014).

**Relation to compression.** CS is a poor *rate*-efficient compressor. The quantized measurements of a CS encoder fall well short of transform coding plus entropy coding (Goyal, Fletcher & Rangan, "Compressive sampling and lossy compression", IEEE SP Mag. 2008). Its value is a cheap encoder: the decoder does the heavy work.

**cram.** Use sparse coding (OMP or ISTA unrolled as a tang graph) for dictionary-based residual coding. Don't build a CS codec. ISTA/FISTA unrolled through tang AD gives a LISTA-style learnable sparse encoder.

## 10. Entropy coding: arithmetic coding, ANS, parallelism, determinism

**Bound.** E[length] ≥ H(X). Arithmetic coding (Rissanen 1976; Witten, Neal & Cleary 1987) gets within ~2 bits per *message*, plus precision loss from finite-precision interval arithmetic.

**ANS (Duda, arXiv:0902.0271; arXiv:1311.2540).** The coder state x ∈ [L, bL) encodes symbol s with quantized frequency f_s, cumulative C_s, total M = 2^n:

```
encode:  x' = ⌊x / f_s⌋·M + C_s + (x mod f_s)
decode:  s = sym(x mod M);  x = f_s·⌊x / M⌋ + (x mod M) − C_s
```

Renormalization streams bytes or words out. ANS is LIFO: you encode in reverse order. The rate cost per symbol is ≈ log₂(M/f_s) exactly, so coding cost depends only on how well f approximates p.

- **rANS** does the arithmetic above directly. It suits adaptive or learned models and needs one multiply and one shift per symbol (the division becomes a reciprocal multiply).
- **tANS/FSE** tabulates the state machine for a fixed distribution and is division-free. Zstd and LZFSE use it.
- **Bits-back / BB-ANS** (Townsend, Bird & Barber, arXiv:1901.04866) uses ANS's stack property for lossless coding with latent-variable models.

**Parallelism.** Giesen's interleaved rANS (arXiv:1402.3392) runs K independent states into one stream, which maps to SIMD lanes and GPU warps. GPU work includes Weißenberger & Schmidt, "Massively parallel ANS decoding on GPUs" (ICPP 2019) and Meta's DietGPU rANS library **[both unverified in this pass]**. Recoil (Lin et al. 2023, decoder-adaptive parallelism for rANS; **[arXiv ID unverified]**) splits one stream for an arbitrary number of decoder threads at small overhead.

**Precision and determinism.** Arithmetic coding and ANS are exact integer algorithms. They break only when encoder and decoder see *different quantized tables*. Rules:
1. Turn model probabilities into an integer frequency table with a deterministic procedure: fixed rounding, a fixed tie-break, and every f_s ≥ 1, summing exactly to 2^n.
2. Never let floating-point `exp`/`log` reach that table on the decode path unless both sides run identical integer code.
3. M = 2^12–2^16 with 32-bit state and 16-bit renorm is the common design point. Precision loss is ~O(|A|/M) bits/symbol **[order-of-magnitude, not a quoted bound]**.

**cram.** Build one rANS core over `u32` state with K-way interleaving (K = 32 for a warp). Write it once in tang's integer subset and codegen it to CPU/WGSL/MSL/CUDA. The CDF quantizer is *the* determinism boundary and should carry a test that hashes the tables across backends. Rate estimation during training uses −log₂ p in float, which never touches the coder.

## 11. The rate–distortion–perception tradeoff

**Statement.** Blau & Michaeli, "The perception–distortion tradeoff" (CVPR 2018, arXiv:1711.06077), and "Rethinking lossy compression: the rate–distortion–perception tradeoff" (ICML 2019, arXiv:1901.07821), define

```
R(D, P) = min I(X; X̂)  s.t.  E d(X,X̂) ≤ D,  δ(p_X, p_X̂) ≤ P
```

δ is a divergence between the source and reconstruction distributions. For MSE, perfect realism (P = 0) costs at most 2× the minimal MSE at the same rate. Achieving perception reduces distortion performance, and the tradeoff can be strict.

Theis & Wagner give a coding theorem for the RDP function with common randomness (arXiv:2104.13662). Zhang et al. (NeurIPS 2021, "Universal rate-distortion-perception representations" **[ID unverified]**) show a single encoder can serve multiple (D, P) points with only the decoder changing. That is the theoretical basis for "one bitstream, MSE or GAN decoder" codecs such as HiFiC (Mentzer et al., arXiv:2006.09965).

**Open.** Operational RDP coding beyond Gaussians. The cost of common randomness. Which divergence matches human perception.

**cram.** Make the decoder head swappable over a fixed latent and entropy model. Distortion-optimal and perception-optimal decoders then share one bit-exact bitstream format. Only the (float, non-critical) synthesis transform changes.

---

## Synthesis: architecture implied by the theory

1. **Objective.** R + λD (+ μP) everywhere. λ is a graph input.
2. **Transform.** Lifting-based, so it is invertible by construction. Hadamard rotations "Gaussianize" the data. Learned NTC transforms in float.
3. **Quantize.** Uniform scalar quantization with dither by default (0.25 bit from optimal at high rate). E8/TCQ for low-rate tensors (QuIP#/QTIP recipe).
4. **Entropy model.** Train in float, deploy as an integer network, with deterministic CDF quantization.
5. **Coder.** Interleaved rANS, written once and codegen'd to every backend.
6. **Bit-exact boundary.** Everything from probabilities to bits is integer-only. Floats are allowed only where an error perturbs reconstruction quality and cannot desynchronize the decoder.
