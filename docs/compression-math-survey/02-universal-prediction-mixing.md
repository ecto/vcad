# 02 — Universal prediction, mixing, and statistical language models

> Scope: the theory behind "predict well under log loss, then arithmetic-code".
> Every compressor in this survey reduces to a sequential probability assignment
> q(x_t | x_<t); code length is -log2 q(x_1..n) to within 2 bits (arithmetic coding).
> The questions are: *which class of sources can we track, at what regret, and how
> do we combine predictors?*
>
> Verification note: citations below are canonical and widely referenced; numbers
> marked **[unverified]** were not re-checked against a primary source for this draft.

## 0. The one identity everything rests on

For any predictor q and source sequence x^n:

    L_q(x^n) = -log2 q(x^n) = sum_t -log2 q(x_t | x^{<t})

Regret against a class P = {p_θ}:

    R_n(q, x^n) = -log2 q(x^n) - min_θ [-log2 p_θ(x^n)]

Compression = cumulative log loss. Prediction, coding, and Bayesian/MDL model
selection are the same problem. This is why a differentiable stack (tang) is a
natural host: the objective is a sum of scalar `Log2`s of differentiable outputs.

## 1. Universal coding and redundancy bounds

**Core statements.**
- *Bayes mixture*: q(x^n) = ∫ p_θ(x^n) w(θ) dθ has regret ≤ -log2 w(θ̂-neighbourhood).
- *Krichevsky–Trofimov (1981)* for memoryless binary sources: Dirichlet(1/2,1/2) prior gives the sequential rule

      q(x_{t+1}=1 | x^t) = (n_1 + 1/2) / (t + 1)

  with worst-case regret ≤ (1/2) log2 n + 1 bits.
- *Rissanen (1984/86)* lower bound: for a k-parameter smooth family, almost every θ has redundancy ≥ (k/2) log2 n (1-o(1)).
- *Minimax regret (Shtarkov 1987)*: the normalized maximum likelihood
  q_NML(x^n) = p_θ̂(x^n)(x^n) / Σ_y p_θ̂(y)(y^n) achieves
  R* = log2 Σ_y max_θ p_θ(y^n) = (k/2) log2 (n/2π) + log2 ∫ sqrt(det I(θ)) dθ + o(1)
  (Rissanen 1996). Jeffreys prior Bayes mixtures are asymptotically minimax (Xie & Barron 1997/2000).

**Why elegant.** The (k/2) log n "price per parameter" ties coding to Fisher geometry; Jeffreys = uniform volume on the statistical manifold.

**Empirical.** KT is the leaf estimator in CTW and many PAQ bit counters' conceptual ancestor; in practice PAQ uses *non-stationary* counters (state machines + adaptive rates), because real text is not memoryless-stationary.

**Open problems.** Tight regret for non-stationary / large-alphabet classes (Orlitsky–Santhanam pattern coding: alphabet-size-free bounds). Regret for neural model classes (k ~ 10^6–10^9) where (k/2)log n is vacuous — the bound is useless for transformers; actual cost is governed by online learning dynamics, not parameter count.

**tang-compress / Hutter.** Implement KT, KT with zero-redundancy (Willems' "KT with deterministic sources"), and adaptive-rate counters as primitive estimators. The (k/2)log n heuristic is the right back-of-envelope for *transmitted* parameters: a weight costs ~0.5 log2 n bits if sent, which is the formal reason Hutter entries prefer online learning over shipping large weights (except where pretrained weights compress very well).

## 2. Context tree weighting (Willems, Shtarkov, Tjalkens 1995)

**Core.** For tree sources of depth ≤ D, each node s keeps a KT estimate P_e(s) and a weighted probability

    P_w(s) = 1/2 P_e(s) + 1/2 P_w(0s) P_w(1s)    (internal node)
    P_w(s) = P_e(s)                              (depth D)

The root P_w is an exact Bayes mixture over all ~2^(2^D) tree models with prior 2^{-Γ_D(S)} (Γ = model description length ~ 2|S|-1). Regret vs the best tree S with parameters θ:

    ≤ Γ_D(S) + (|S|/2) log2(n/|S|) + |S| + 2 bits.

Cost O(D) per symbol.

**Why elegant.** Exponentially many models mixed exactly in linear time via a sum-product recursion; the prior is an MDL code for the tree.

**Extensions.** Binary decomposition for bytes; CTW with unbounded depth (Willems 1998, "context tree weighting: extensions"); *Context Tree Switching* (Veness, Ng, Hutter, Bowling 2012, arXiv:1111.3182) replaces the fixed 1/2 mixing with switching between the two hypotheses at each node (fixed-share style), which empirically beats CTW; *Partition Tree Weighting* (Veness, White, Bowling, György 2013, arXiv:1211.0587) mixes over temporal segmentations for piecewise-stationary sources. CTW was also the model in MC-AIXI-CTW (Veness et al. 2011, arXiv:0909.0801).

**Empirical.** CTW variants are competitive with PPM on Calgary-scale data but are not the state of the art on enwik8/9; PAQ-family mixing of many heterogeneous models dominates.

**Open.** Efficient CTW over non-suffix contexts (word, sparse, column contexts); exact Bayesian mixing over context *sets* rather than trees.

**tang-compress.** CTW is a clean, provable baseline and a good test of the estimator API. The CTS idea (local switching weights) is directly reusable inside a logistic mixer's per-context weight selection.

## 3. PPM and its Bayesian reading

**Core.** PPM (Cleary & Witten 1984) predicts from the longest matching context of order ≤ k, escaping to shorter contexts with escape probability (methods A/B/C/D/X; PPMD's escape = (#distinct)/(2·count)). PPM* (Cleary, Teahan 1997) uses unbounded contexts; PPMd/PPMII (Shkarin 2002) adds information inheritance and SEE (secondary escape estimation).

**Bayesian relation.** Escape-blending is a back-off mixture; with "exclusion" it approximates a hierarchical prior where each context's distribution is drawn around its parent's. This is made exact in §4: PPM-like blending with discount-based escapes ≈ posterior predictive of a hierarchical Pitman–Yor process (Teh 2006).

**Why elegant.** No parameters to learn beyond counts; the escape mechanism is a crude but effective Bayesian shrinkage.

**tang-compress.** PPMd-style models are cheap high-order predictors to feed a mixer; SEE is precisely an "APM/SSE" stage (§8).

## 4. Hierarchical Pitman–Yor, the sequence memoizer, and Kneser–Ney

**Core.** Pitman–Yor PY(d, α, G0) predictive (Chinese restaurant):

    P(w | context u) = (c_uw - d·t_uw)/(α + c_u·) + (α + d·t_u·)/(α + c_u·) · P(w | π(u))

where t are table counts and π(u) drops the oldest symbol. Teh (2006, ACL "A Hierarchical Bayesian Language Model based on Pitman-Yor Processes") showed interpolated Kneser–Ney is the special case α=0, t_uw = min(1, c_uw) — i.e. KN is an approximate inference scheme for HPYP. Goldwater, Griffiths & Johnson (2006) reached the same observation independently.

**Sequence memoizer** (Wood, Archambeau, Gasthaus, James, Teh, ICML 2009; CACM 2011): unbounded-depth HPYP with α=0, using the PY *coagulation/fragmentation* property to marginalize non-branching nodes, so the context tree is a suffix tree with O(n) nodes. Deplump (Gasthaus, Wood, Teh, DCC 2010, "Lossless compression based on the Sequence Memoizer") reported results better than PPM and CTW on Calgary corpus **[exact bpc unverified]**; Bartlett, Pfau, Wood (2010) gave a constant-space forgetting variant.

**Why elegant.** Power-law (Zipf) behaviour of language is exactly the PY discount; "smoothing heuristics" become posterior inference.

**Open.** Fast/streaming inference at enwik9 scale in 10GB; learning discounts d per depth online by gradient (they are differentiable!).

**tang-compress.** The predictive above is rational in counts and (d, α) — expressible in tang's Add/Mul/Recip and differentiable in d, α. Online gradient tuning of discounts per depth/context class is a cheap, novel knob. Store table counts with the min(1,c) KN approximation to keep memory O(contexts).

## 5. Prediction with expert advice under log loss

**Core.** With N experts p_i and the Bayes/exponential-weights mixture (learning rate η=1 for log loss):

    q(x_t) = Σ_i w_{i,t} p_i(x_t),   w_{i,t+1} ∝ w_{i,t} p_i(x_t)

regret vs best expert ≤ ln N (nats), independent of n. Log loss is *1-mixable* (Vovk's Aggregating Algorithm), so the Bayes mixture is optimal (Cesa-Bianchi & Lugosi, *Prediction, Learning, and Games*, 2006, ch. 3, 9).

**Why elegant.** Constant regret; mixing is just multiply-and-normalize.

**But.** Linear (Bayes) mixing posteriors concentrate on one expert — bad when the best expert changes. Hence §7, and hence PAQ's logistic mixing (§8), which competes with the best *combination*, not the best single model.

## 6. Universal portfolios ≡ log-loss mixing

**Core.** Cover (1991) "Universal Portfolios": wealth of the constant-rebalanced portfolio b on price relatives x_t is S_n(b)=Π_t b·x_t. The universal portfolio b̂_{t+1} = ∫ b S_t(b) dμ(b) / ∫ S_t(b) dμ(b) achieves

    log S_n* - log Ŝ_n ≤ (m-1)/2 · log(n+1) + O(1)

with Dirichlet(1/2) prior (Cover & Ordentlich 1996). Equivalence: betting on sequences with fair odds turns wealth into 2^{n - L_q}, so maximizing log wealth = minimizing code length; Cover & Thomas, *Elements of Information Theory*, ch. 6 & 16. Experts with *densities* p_i correspond to "stocks", and the CRP over them is exactly a *linear* mixture with fixed weights — so universal portfolio = Bayes mixture over fixed mixing weights, i.e. it competes with the best fixed linear combination at (N-1)/2·log n regret.

**Why elegant.** Gambling, coding, and investing are one theorem.

**Computation.** Exact Cover portfolio is exponential in m; efficient alternatives: Online Newton Step (Hazan, Agarwal, Kale 2007) with O(m log n) regret via exp-concavity of -log(b·x). This is the clean theoretical reason second-order / ONS updates are appealing for mixers.

**tang-compress.** "Linear mixing with learned weights" in the probability domain is a portfolio problem; ONS is a principled optimizer for it and fits tang's optimizer module.

## 7. Switching: fixed share, switch distribution, catch-up

**Core.** Fixed Share (Herbster & Warmuth 1998, "Tracking the best expert"):

    v_i = w_i p_i(x_t) / Σ_j w_j p_j(x_t)
    w_{i,t+1} = (1-α) v_i + α/N

Regret vs best sequence with k switches: ≤ (k+1) ln N + (n-1) H(k/(n-1)) nats with tuned α. Volf & Willems (1998) "switching method"; Koolen & de Rooij (2008/2013, "Universal codes from switching strategies", arXiv:0802.2015) unify these as HMMs over expert identities and give efficient priors with near-optimal regret for unknown k. van Erven, Grünwald, de Rooij (NIPS 2007; JRSS-B 2012, "Catching up faster by switching sooner", arXiv:0807.1005): Bayesian model averaging suffers the *catch-up phenomenon* — complex models predict better only after enough data, but BMA switches too late; the *switch distribution* fixes this and is both AIC-rate and BIC-consistent.

**Why elegant.** Non-stationarity handled exactly by a tiny forward algorithm over a hidden "which-expert" state; O(N) per step.

**Empirical relevance.** The catch-up phenomenon is exactly why compressors combine a small fast-adapting model with a slow large model, and why PAQ's learning rates decay but don't go to zero.

**tang-compress.** Put a fixed-share/switch layer over heterogeneous model families (n-gram, match model, neural) — cheap, provable, and complementary to logistic mixing. Variable-share and "mixing past posteriors" (Bousquet & Warmuth 2002) handle returning-to-old-expert patterns typical of Wikipedia articles.

## 8. Geometric / logistic mixing (PAQ) — Mattern's theory

**Core.** For binary prediction with model probabilities p_i, PAQ (Mahoney 2005+) mixes in the logit domain:

    st(p) = ln(p/(1-p)),   squash(z) = 1/(1+e^{-z})
    p_mix = squash( Σ_i w_i st(p_i) )
    w_i ← w_i + η · (y - p_mix) · st(p_i)

The update is exactly online gradient descent on log loss for this single-neuron model. Mattern (DCC 2012, "Mixing strategies in data compression"; 2013, "Linear and geometric mixtures — analysis"; arXiv:1302.?? **[ID unverified]**) shows: (a) logistic mixing is the binary case of *geometric mixing* p ∝ Π p_i^{w_i}, the minimizer of weighted KL divergence Σ w_i D(q‖p_i), whereas linear mixing minimizes Σ w_i D(p_i‖q); (b) regret bounds for OGD on these mixtures relative to the best fixed weight vector, O(√n) generally and O(log n) under exp-concavity-like conditions. Gated Linear Networks (Veness et al. 2017 arXiv:1712.01897; 2019 "Gated Linear Networks" arXiv:1910.01526) generalize PAQ mixers: layers of geometric mixers whose weight vectors are *selected by side-information contexts*; each neuron's loss is convex, so learning is local and fast, and GLNs are universal approximators.

**Why elegant.** Geometric mixing behaves like a product of experts: a confident expert can override many unconfident ones (log-odds add), which linear mixing cannot. Context-selected weight sets turn a convex learner into a powerful non-linear one with no backprop.

**Empirical.** cmix and paq8 variants stack: hundreds of models → several context-selected mixers → second-layer mixer → APM/SSE chain. This architecture holds the Hutter records.

**APM/SSE.** p' = interpolate table[ctx][bucket(st(p))]; a learned 1-D calibration curve per context — a piecewise-linear "isotonic" correction.

**Open.** Tight regret theory for multi-layer context-gated mixers; optimal learning-rate schedules (Mattern advocates decaying rates; practice tunes by hand); principled weight-set selection.

**tang-compress.** This is the sweet spot for tang: the whole mixer graph is `Exp2`/`Log2`/`Recip`/`Select` — stretch(p) = Log2(p) - Log2(1-p) (scaled), squash via Exp2 + Recip, context gating via Select. Symbolic diff gives exact per-weight gradients; codegen gives a branch-free CPU kernel (Hutter: 1 CPU). Use the symbolic graph to derive *second-order* (ONS/diagonal-Newton) updates automatically — something hand-written PAQ code rarely does.

## 9. Online convex optimization view

Each mixer step minimizes f_t(w) = -log p_mix(x_t; w). For geometric mixing f_t is convex in w (log-sum-exp structure); for binary logistic it is exp-concave on bounded domains. Hence:
- OGD: regret O(√n); with strong convexity / exp-concavity, ONS: O(d log n) (Hazan 2016, *Introduction to Online Convex Optimization*, arXiv:1909.05207).
- Adaptive methods (AdaGrad/Adam) are used in cmix's LSTM and in NNCP; Adam-style rates on mixer weights are a cheap experiment.
- Meta-view: the compressor is an online learner whose total regret IS the excess file size. Every bit saved is regret reduction; this gives a principled way to tune hyperparameters (they're "experts" too — mix over learning rates, à la MetaGrad, van Erven & Koolen 2016, arXiv:1604.08740).

**tang-compress.** MetaGrad-style mixing over learning rates is a natural use of tang's vectorized codegen: run K rate copies of the mixer in SIMD, mix them with exponential weights.

## 10. "Language modeling is compression" and neural lossless text compression

**Core claim.** Delétang et al. (ICLR 2024, arXiv:2309.10668): any predictor is a compressor via arithmetic coding; Chinchilla-70B compresses enwik9 *data alone* to ~8.3% raw and ImageNet patches to 43.4% / LibriSpeech to 16.4%, beating PNG/FLAC **[figures from abstract; verify]**. But counting model size, large LMs are terrible compressors of enwik9 — the "adjusted compression rate" makes the parameter-cost point explicit. Scaling laws plus this accounting give an optimal model size for a given dataset size: exactly the Hutter trade-off.

**Others.**
- LLMZip (Valmeekam et al. 2023, arXiv:2306.04050): LLaMA-7B + AC, reported ~0.7 bpc on a small text sample **[unverified]**.
- NNCP (Bellard; v2/v3 2021–2023, transformer trained *online* while compressing): tops Mahoney's Large Text Compression Benchmark on enwik9 with ~107MB **[unverified exact figure]**, but needs GPU and so is not Hutter-eligible.
- cmix (Byron Knoll): PAQ8 models + LSTM mixed; long-standing top CPU result on enwik8/9.
- Hutter Prize lineage: phda9 (Rhatushnyak) → starlit (Ruse, 2021) → fx-cmix (Ruse/Kaido Orav, 2023) → Orav 112,578,322 B (Feb 2024) → fx2-cmix 110,793,128 B (Sept 2024) → current record **fx2-cmix-transformer** (Vladimir Ivanov, 2026), 100,424,672 B: a ~6M-parameter 4-bit transformer pretrained offline on GPUs, weights appended to the decompressor (run time: 1 CPU, ~50h, ~10GB RAM, no GPU). 2026 awards also went to cmix-lex (Marcouch & Orav, 109,671,639 B) and cmix-obias (Freelan, 108,521,870 B). Sources: http://prize.hutter1.net/ and Mahoney's announcement http://www.mail-archive.com/agi@agi.topicbox.com/msg11585.html .
- "nacrith" and other 2025–2026 LLM-based compressors: **not verified** in this pass; Mahoney's LTCB page was updated Mar 2026 and should be consulted for current enwik8/9 ranks (mattmahoney.net/dc/text.html).

**Why elegant.** Collapses compression, generalization, and Solomonoff/AIXI induction into one quantity; MDL makes model size part of the loss.

**Open problems.** (1) Optimal *two-part* allocation: pretrained-weight bits vs online-learning bits under fixed CPU time. (2) Deterministic floating point across encode/decode (bit-exact replay on 1 CPU — tang's codegen to C with fixed op order helps). (3) Distilling a transformer into context-gated mixers (GLN-like) that run 10–100x faster at equal bpc. (4) Throughput: enwik9 = 10^9 bytes in ~50h ⇒ ≤ ~180 µs/byte total budget, ~22 µs/bit.

**tang-compress / Hutter mapping.**
1. Pretrained small transformer (≤ ~10M params, quantized, weights themselves compressed) as one expert; it pays its own weight cost in the total.
2. Online-learned sparse experts: PPMd/HPYP n-grams (§3–4), match models, word/column contexts, CTW/CTS (§2).
3. Context-gated geometric mixer layers (§8) with tang-derived gradients; fixed-share over model families (§7); MetaGrad over learning rates (§9).
4. APM/SSE calibration chain.
5. Everything compiled to deterministic scalar C/SIMD via tang codegen; encoder and decoder share the exact same generated kernel.

## Key references
- Krichevsky & Trofimov, IEEE T-IT 1981. Shtarkov, Probl. Inf. Transm. 1987. Rissanen, IEEE T-IT 1996. Xie & Barron, IEEE T-IT 2000.
- Willems, Shtarkov, Tjalkens, "The context-tree weighting method", IEEE T-IT 1995.
- Veness et al., Context Tree Switching, arXiv:1111.3182; Partition Tree Weighting, arXiv:1211.0587; GLN arXiv:1712.01897, arXiv:1910.01526.
- Cleary & Witten 1984; Shkarin, PPMII, DCC 2002.
- Teh, ACL 2006; Wood et al., ICML 2009 / CACM 2011; Gasthaus, Wood, Teh, DCC 2010.
- Cesa-Bianchi & Lugosi 2006; Vovk 1990/1998.
- Cover 1991; Cover & Ordentlich 1996; Hazan, Agarwal, Kale 2007.
- Herbster & Warmuth 1998; Bousquet & Warmuth 2002; Koolen & de Rooij arXiv:0802.2015; van Erven, Grünwald, de Rooij arXiv:0807.1005.
- Mattern, DCC 2012/2013 (ID unverified); Mahoney, *Data Compression Explained*.
- Hazan, OCO arXiv:1909.05207; van Erven & Koolen, MetaGrad arXiv:1604.08740.
- Delétang et al. arXiv:2309.10668; Valmeekam et al. arXiv:2306.04050; Bellard, NNCP (bellard.org/nncp).
- Hutter Prize: https://www.hutter1.net/prize/ ; LTCB: https://mattmahoney.net/dc/text.html ; https://github.com/spicylemonade/HutterPrize
