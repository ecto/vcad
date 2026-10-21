# 03 — Structured low-rank representations: the shift operator, Hankel matrices, and tensor networks

> Verification status: the arXiv IDs 2606.17064 and 2609.36506 were checked against the arXiv API on 2026-10-18 (titles and abstracts summarized below). Every other citation is a well-known paper cited from memory. The IDs are given where I am confident of them, and **no benchmark number appears here unless it is marked as verified or unverified.** Before any number below goes into a design decision, re-check it against the paper.

## 0. The one idea

Take a signal `x[n]` and the shift operator `S: x[n] -> x[n+1]`. If the shifted copies of the signal span a space of small dimension `r`, the signal obeys a linear recurrence of order `r`. Its Hankel matrix `H_ij = x[i+j]` then has rank `r`. A finite recurrence is the same thing as a small weighted automaton, and a small automaton is the same thing as a low-rank tensor train in the quantized (binary-digit) index.

So "low-rank Hankel" can serve as one spine for three settings:

| Setting | Index | Shift acts on | Low-rank object |
|---|---|---|---|
| Signals (Prony/FRI) | time `n` | `n -> n+1` | Hankel `H[x]` |
| Sequences/languages (spectral WFA) | strings `u` | `u -> ua` | Hankel `H(u,v)=f(uv)` |
| Grids/fields (QTT) | binary digits `(i_1..i_d)` | digit-level splits | unfoldings `X^{<k>}` |

In all three, the compressed artifact is a small linear dynamical system `(alpha, {A_a}, beta)`, and decoding means multiplying small matrices. That is a good fit for tang. Decoding is pure Add/Mul, so it is differentiable end to end and code-generates to every backend.

---

## 1. Kronecker's theorem and the sum-of-exponentials model

**Statement.** An infinite Hankel matrix `H = [h_{i+j}]` has finite rank `r` if and only if its generating function `sum_n h_n z^{-n}` is rational of degree `r`. Equivalently, apart from confluent (repeated-root) cases,

`h_n = sum_{k=1}^r c_k z_k^n`, with `z_k = exp(s_k)`.

With repeated roots, the coefficients `c_k` become polynomials in `n`. The finite-window version: if `x` is an `r`-term sum of exponentials, every `L x (N-L+1)` Hankel matrix with `L, N-L+1 >= r` has rank exactly `r`.

**Why it is elegant.** Rank is a property of the shift operator. The annihilating polynomial of `S` on the signal (Prony's polynomial) has the `z_k` as its roots. The model class closes under addition (ranks add), multiplication (ranks multiply) and convolution. Polynomials, sinusoids, damped modes and decays all belong to it.

**Compression meaning.** `N` samples become `2r` complex numbers. For truly exponential content, this is the most parsimonious description available.

**Open problems.** Real data is only approximately low rank. Choosing the rank under noise, and the bad conditioning when nodes `z_k` sit close together (the super-resolution limit), are both still active research. Approximation of general signals by short exponential sums is AAK/Adamjan–Arov–Krein theory: the singular values of the Hankel operator give the best achievable error. That theory is elegant but hard to compute with at scale.

**cram mapping.** A "Prony atom" is one codec primitive: parameters `(c_k, s_k)`, decoder `Re sum c_k exp2(s_k n / ln2)`. It uses only Exp2, Mul, Add, Sin and Atan2 for phases, all of which tang has. Gradients with respect to `s_k` come for free, so variable projection or L-BFGS refinement after an ESPRIT initialization is easy.

## 2. Prony, matrix pencil, ESPRIT

- **Prony (1795):** solve the linear-prediction system `H a = 0` for the recurrence coefficients, take roots, then fit amplitudes by least squares. Noise-fragile.
- **Matrix pencil (Hua & Sarkar, 1990):** take an SVD of `H`, truncate to rank `r`, and form `H_1 = U_r` with the last row dropped and `H_2 = U_r` with the first row dropped. The `z_k` are the eigenvalues of `H_1^+ H_2`.
- **ESPRIT (Roy & Kailath, 1989):** the same shift-invariance of the signal subspace, `U_↑ Φ = U_↓`, with `eig(Φ) = {z_k}`. It is the subspace-method counterpart of Prony.

**Elegance.** The shift operator, restricted to the signal subspace, is the matrix `Φ`. Estimation reduces to one SVD plus a small eigenproblem.

**Known results.** ESPRIT and the matrix pencil approach the Cramér–Rao bound at moderate SNR. Theory on the conditioning of Vandermonde systems (Moitra 2015, arXiv:1408.1681; later Li, Liao and others) shows a phase transition in recoverability at node separation around `1/N`. I have not re-verified the exact constants.

**tang mapping.** The SVD and eigenproblems run outside the expression graph, in tang-la. The graph holds only the decoder. That split (classical estimator for initialization, AD refinement in the graph) recurs throughout this chapter.

## 3. Finite rate of innovation and Cadzow denoising

**Statement (Vetterli, Marziliano, Blu 2002, IEEE TSP).** A signal with `ρ` degrees of freedom per unit time, such as a stream of `K` Diracs `x(t) = sum a_k δ(t-t_k)`, can be recovered exactly from about `2K` samples taken through suitable kernels (sinc, Gaussian, or exponential-reproducing kernels per Dragotti–Vetterli–Blu 2007). The kernel samples give Fourier coefficients `X[m] = sum a_k u_k^m`, which is exactly Kronecker's model, so an annihilating filter or ESPRIT recovers the `t_k`.

**Cadzow (1988), the "lift–project" denoiser.** Alternate two projections: onto rank-`r` matrices (truncated SVD), then back onto the Hankel subspace (average the anti-diagonals). Each step is a projection, but the iteration is a heuristic and is not guaranteed to converge to the nearest structured low-rank matrix. Condat & Hirabayashi (2015, "Cadzow denoising upgraded", Sampling Theory in Signal & Image Proc.) give a Douglas–Rachford variant that does converge.

**Elegance.** Sampling theory without bandlimitedness: the sample count scales with innovation, not bandwidth.

**Compression meaning.** FRI is a sparse-in-continuous-parameter codec. It suits spikes, piecewise polynomials (through the derivative), and edges.

**Open problems.** FRI is fragile under model mismatch, and it needs a choice of `K`. Nobody has a standard rate–distortion theory for FRI parameters, meaning how to quantize the `t_k` against the `a_k`. A Fisher-information bit allocation looks natural, but I have not seen it published as a codec.

**tang mapping.** Cadzow fits a projection-based iterative refinement step in tang-la. A differentiable relaxation (unrolled Cadzow with a learned rank weighting) is an option, but the truncated SVD inside it has a non-smooth gradient at degenerate singular values.

## 4. Spectral compressed sensing and low-rank Hankel recovery

**Core program.** Recover a spectrally sparse signal from `m << N` random samples by solving

`min ||H(x)||_* s.t. x_Ω = y_Ω`

This is EMaC, from Chen & Chi, "Robust spectral compressed sensing via structured matrix completion" (arXiv:1304.8126). It needs `m = O(r log^4 N)` samples under incoherence. Atomic-norm / total-variation super-resolution (Candès & Fernandez-Granda, arXiv:1203.5871; Tang, Bhaskar, Shah & Recht, arXiv:1207.6053) is the gridless counterpart, with a separation condition `Δ >= ~2/N` in that analysis.

Non-convex variants scale better: projected gradient descent over the Burer–Monteiro factors `H(x) ≈ L R^*` (Cai, Wang, Wei and others, e.g. "Fast and provable algorithms for spectrally sparse signal reconstruction via low-rank Hankel matrix completion", arXiv:1606.01567). They run at `O(r N log N)` per iteration using FFT-based Hankel–vector products.

**2024–2026 work.** Work after 2023 extends this in three directions: robust and corrupted Hankel completion, multidimensional and multi-level Hankel structure, and learned or unrolled solvers. I was not able to verify specific 2024–2026 arXiv IDs in this pass, so I cite none rather than invent any. The closest verified recent paper is the QTT compressed-sensing work in §6.

**cram mapping.** The Hankel–vector product is a correlation, so it can be computed matrix-free with an FFT. The factor parameterization `x = H^†(L R^*)` is differentiable, which makes "Hankel rank as a regularizer" a loss term rather than a hard constraint. For lossy coding: transmit `L` and `R`, or better, transmit the poles alone, as in §1.

## 5. Quantized tensor trains (QTT)

**Construction (Oseledets 2010, "Approximation of 2^d×2^d matrices using tensor decomposition", SIMAX; Khoromskij 2011, "O(d log N)-quantics approximation of N-d tensors", Constr. Approx.).** Write the index `n = sum_k i_k 2^{d-k}` with `i_k ∈ {0,1}`, and reshape `x ∈ R^{2^d}` into a `d`-way `2×2×…×2` tensor. Then factor it as a tensor train:

`x(i_1,…,i_d) = G_1[i_1] G_2[i_2] … G_d[i_d]`, with `G_k[i] ∈ R^{r_{k-1}×r_k}`.

Storage is `O(d r^2)` = `O(log N · r^2)`.

**Exact ranks, the "Kronecker for QTT" facts.**
- `exp(αn)`: rank 1, because `exp(α sum i_k 2^{d-k}) = prod_k exp(α i_k 2^{d-k})`.
- `sin(ωn + φ)`: rank 2. It is the real part of two exponentials, or equivalently obeys the angle-addition recursion.
- Polynomial of degree `p`: rank `p+1`.
- Sum of `r` exponentials: rank `<= r`.
- Generally, QTT rank `<=` the dimension of the space spanned by shifts and dilations at each scale. That makes QTT a *multiscale* Kronecker theorem.

Piecewise-smooth functions with finitely many singularities get ranks that grow only polylogarithmically in `1/ε` (Grasedyck; Kazeev and Schwab for PDE solutions with corner singularities, where QTT reaches exponential convergence `ε ~ exp(-b N_dof^{1/κ})` in some analyses).

**Ali & Nouy approximation theory** (arXiv:2007.00118 and 2007.00128, "Approximation with tensor networks", Parts I–III). Tensorized functions in tree tensor network or TT format achieve near-optimal rates on Besov spaces and approximate as well as wavelets / adaptive splines, *without* choosing the basis in advance. The approximation classes are not embedded in classical smoothness spaces, so TT can also exploit structure that is not smoothness.

**Elegance.** Logarithmic storage for structured signals, and arithmetic (add, Hadamard product, convolution, FFT as QTT operators per Dolgov–Khoromskij–Savostyanov 2012) runs directly in the compressed format.

**Open problems.** TT ranks can blow up on discontinuities that are not dyadically aligned, and on noise. Which ordering of digits to use for multi-d data (interleaved vs. sequential) depends on the data. Rounding (TT-SVD) is sequential in `d`.

**tang mapping.** The decoder for one point is a product of `d` small matrices, which is Mul/Add only. A whole-field decode is a sequence of batched GEMMs, which code-generates well to WGSL/CUDA. The cores are the learnable parameters. Reverse-mode AD gives gradients for "Riemannian-free" fine-tuning, but manifold retraction (TT rounding) has to run outside the graph.

## 6. Tensor cross interpolation (TCI)

**Statement.** TT-cross (Oseledets & Tyrtyshnikov 2010, Lin. Alg. Appl.) builds a TT from `O(d r^2)` *function evaluations*, using skeleton/CUR decompositions on index pivots selected by maxvol. Recent tooling, the xfac / TensorCrossInterpolation.jl library (Fernández, Núñez Fernández, Waintal and coauthors, arXiv:2407.02454, "Learning tensor networks with tensor cross interpolation"), makes QTT+TCI practical for functions given as black boxes.

**Compression meaning.** This gives encoder-side sampling. You never materialize the full field, which matters for data that is too big to load.

**tang mapping.** A tang expression graph is exactly a black-box `f(i_1..i_d)` that can be JIT-compiled. TCI over a code-generated kernel is a natural "compress the function, not the samples" mode.

## 7. QTT for scientific/CFD data: verified recent work

- **arXiv:2606.17064**, "Tensor network compression using fluid dynamics as a testbed: Analytical foundations in one dimension" (physics.comp-ph). Applies MPS/TT to 1D fields. Random Fourier series are compressed *losslessly* once the bond dimension is high enough, and memory scales linearly in bond dimension. At lower bond dimension the paper tests the lossy regime on Burgers' equation evolution. It also shows a TT periodic convolution that the authors say can be "orders of magnitude faster" than FFT-based convolution. **The abstract reports no comparison against SZ/ZFP and no bit-level accounting.**
- **arXiv:2609.36506**, "Compressed Sensing with Quantized Tensor Trains (QTTs)" (math.NA). Its model class combines bounded TT rank with sparse low-order interactions, i.e. a Walsh/ANOVA structure, measured with random Walsh–Hadamard samples. It proves a uniform RIP and a uniqueness condition, and gives a structure-aware initialization for ALS (project the backprojection onto the low-order interaction space, then TT-SVD). The number of measurements grows polynomially in `d`, not in `2^d`. This paper directly joins QTT with Walsh/ANOVA bases (§8).
- Older work: Ballard, Klinvex and others built TuckerMPI for HPC compression. Tucker/HOSVD codecs such as TTHRESH (Ballester-Ripoll, Lindstrom & Pajarola, arXiv:1806.05952) *do* bit-account, and they compare favorably to ZFP/SZ on smooth volumes at low bitrates. That is Tucker, not TT, but it is the closest real codec in this family (see §11).

## 8. Walsh / ANOVA bases

A binary-digit tensor `x(i_1..i_d)` has an exact expansion in Walsh functions `w_S(i) = (-1)^{sum_{k∈S} i_k}`, indexed by subsets `S ⊆ {1..d}`. The ANOVA decomposition groups terms by `|S|` (the interaction order). Smooth functions concentrate on low-order and "local" interactions, which is why QTT ranks stay small. Under the Walsh–Hadamard transform, a function whose interactions are all of order `<= q` has `O(d^q)` nonzeros.

**tang mapping.** A Walsh–Hadamard transform is pure Add/Neg butterflies. Combining "QTT cores + a sparse Walsh residual" gives a two-layer codec that is easy to analyze and is cheap on a GPU.

## 9. Spectral learning of weighted automata and HMMs

**Hankel for languages.** For `f: Σ* -> R`, set `H_f(u,v) = f(uv)`. The Fliess/Carlyle–Paz theorem says `rank H_f = r` exactly when `f` is computed by a minimal weighted finite automaton (WFA) with `r` states: `f(a_1…a_n) = α^T A_{a_1}…A_{a_n} β`. This is Kronecker's theorem with strings in place of integers.

**Spectral algorithm.**
- Hsu, Kakade & Zhang 2009 (arXiv:0811.4413): learn HMMs from low-order moments.
- Balle, Carreras, Luque & Quattoni 2014 (Machine Learning); Balle & Mohri survey: take an empirical sub-block `H_{P,S}` and shifted blocks `H_a`, set `H ≈ U D V^T`, and recover `A_a = (H V)^+ H_a V`. The method is consistent, has PAC bounds, needs no EM and has no local optima.

**Equivalence.** A WFA on strings of fixed length `d` over alphabet `{0,1}` *is* a TT/MPS, with `G_k[i] = A_i`. This is the "uniform" (site-independent) MPS. QTT with site-dependent cores is a *non-stationary* automaton. Rabusseau, Li & Precup (arXiv:1807.01406, "Connecting weighted automata and recurrent neural networks through spectral learning") make the WFA ↔ linear 2-RNN link precise. Glasser, Sweke, Pancotti, Eisert & Cirac (arXiv:1907.03741) relate TNs, HMMs, and Born machines.

**Compression meaning.** For text and symbol streams, a low-rank WFA gives an arithmetic coder its next-symbol probabilities: `p(a|u) ∝ α^T A_u A_a β_∞`. The model *is* the codec state.

**Open problems.** Negative probabilities in unconstrained WFAs. HMMs and Born machines guarantee nonnegativity; WFAs do not. The choice of basis for prefixes and suffixes. Sample complexity at large alphabets.

## 10. Tensor-network / Born-machine language models

A Born machine models `p(x) = |Ψ(x)|^2 / Z`, with `Ψ` an MPS (Han, Wang, Fan, Wang & Zhang, arXiv:1709.01662). It gives exact sampling, exact normalization and exact marginals, all by contraction. Language modeling with MPS has been explored, for example Miller, Rabusseau & Terilla, "Tensor networks for probabilistic sequence modeling" (arXiv:2003.01039, the u-MPS). As far as I know these models do not reach transformer-level perplexity at scale. I did not verify any perplexity numbers.

**Compression relevance.** Exact likelihoods mean exact code lengths: `-log2 p(x)` bits under arithmetic coding. That makes them clean "bits-back-free" baselines.

**tang mapping.** `|Ψ|^2`, normalization and per-symbol conditionals are all contractions of small matrices. The training loss is NLL in bits, which tang differentiates directly. Log2 is one of tang's 11 ops, which conveniently gives bits natively.

## 11. Entropy-coding TT/MPS cores, and whether anyone built a real codec

**The pieces a real codec needs:**
1. **Gauge fixing.** A TT is invariant under `G_k -> G_k M`, `G_{k+1} -> M^{-1} G_{k+1}`. Put the network in canonical form (left-orthonormal cores, with all energy in one core, or Vidal form with singular values `Λ_k` on the bonds). Then quantization error in an orthonormal core propagates with norm 1, and the singular values say exactly how important each bond is.
2. **Quantization.** Quantize the orthonormal cores uniformly, with step sizes scaled by the bond singular values. Alternatively, use reverse water-filling across bonds: the squared truncated singular values `sum σ_j^2` are the exact squared error of truncation (the TT-SVD bound `||x - x̃||^2 <= sum_k ε_k^2`).
3. **Rank selection by rate–distortion.** Choose ranks `r_k` to minimize `D + λ R`, where `R ≈ sum_k r_{k-1} r_k n_k · b_k` bits. This is a greedy discrete optimization, and a natural Lagrangian sweep.
4. **Entropy coding.** Core entries after an orthogonal gauge are roughly Laplacian, so a context-adaptive arithmetic coder is the obvious choice.

**Who has built a real bit-accounted codec?**
- **Tucker:** yes. TTHRESH (arXiv:1806.05952) bit-plane-codes HOSVD core coefficients with RLE plus arithmetic coding, and reports better rate–distortion than ZFP, SZ and SQ on several smooth scientific volumes at low-to-medium bitrates. Its advantage shrinks or reverses at high fidelity or on turbulent data (as I recall; re-verify the specific datasets and PSNR margins).
- **TT/QTT:** *no codec has been verified as rigorous and bit-accounted*. Several papers report "compression ratio" as parameter count × 32 or 64 bits. I did not confirm any QTT paper that entropy-codes quantized cores and benchmarks against SZ3/ZFP/MGARD at matched PSNR. **This is a real gap and an opportunity for cram.** If such a paper exists from 2025–2026, I did not find it in this pass.
- **Wavelets:** wavelet-based compressors (e.g. SPERR, from Li, Lindstrom & Clyne, arXiv ID unverified) are the most relevant baseline. Ali–Nouy theory says QTT should *match* wavelet rates on Besov classes, and should beat them only when the data has non-smooth but low-rank structure, such as periodicity, repeated patterns, or separable components.

**Prediction to test:** QTT+entropy coding will win on data with long-range self-similarity, oscillation or separability, roughly tie with wavelets on generic smooth fields, and lose on noisy or turbulent small scales. There, every compressor degrades toward sample-level coding, and TT ranks saturate.

---

## 12. Design implications for cram

1. **A single IR for decoders:** `(α, {G_k[i]}, β)`, i.e. products of small matrices. Prony atoms (diagonal `G`), WFAs (stationary `G`), QTT (site-dependent `G`) and Tucker-like trees all lower to it. Codegen emits batched small GEMMs.
2. **Encoder = classical init + AD refine.** Initialize with ESPRIT, TT-SVD, TCI or spectral WFA (tang-la), then refine the quantized-aware objective `D(θ) + λ R(θ)` with reverse-mode AD, using a straight-through or soft quantizer and a rate proxy from Laplace/logistic entropy (Exp2/Log2).
3. **Gauge is free compression:** canonicalize before quantizing, and spend bits on bonds in proportion to `log σ`.
4. **Rank is the rate knob:** the TT-SVD truncation error gives an analytic distortion; the rank budget gives an analytic rate. You get a closed-form R–D curve before any entropy coding.
5. **Benchmark gate:** before claiming anything, run SZ3, ZFP, SPERR and TTHRESH at matched PSNR/max-error on SDRBench fields, counting all bits (header, ranks, quantizer steps).

## Unverified / to-verify list
- Exact sample-complexity exponents for EMaC and the PGD variants.
- TTHRESH vs. ZFP/SZ margins per dataset.
- Any 2024–2026 spectral-CS / Hankel-completion arXiv IDs (none cited here on purpose).
- The existence of a bit-accounted QTT codec paper (none found).
