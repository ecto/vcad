# 01 — Minimum description length and the cost of models

> Survey chapter for tang-compress. Equations are plain LaTeX. Numbers are only
> quoted where I am confident of the source; anything else is flagged
> **[unverified]**. arXiv IDs are given so each claim can be checked.

The Hutter Prize scores `|compressed data| + |decompressor incl. weights|`. That
is literally a two-part MDL code length, which makes this chapter the
accounting rulebook for every other chapter: it says what a model *costs*, and
which ways of paying for it are cheapest.

---

## 1. Two-part codes vs prequential (online) codes

**Core statement.** A two-part code sends a model, then the data given the model:

$$L_{2p}(x) = \min_\theta \; L(\theta) + \big(-\log_2 p_\theta(x)\big).$$

A prequential (predictive-sequential, Dawid 1984) code sends nothing up front;
encoder and decoder run the *same deterministic learning algorithm* and each
symbol is coded under the model trained on the prefix:

$$L_{preq}(x_{1:n}) = \sum_{t=1}^{n} -\log_2 p_{\hat\theta(x_{<t})}(x_t).$$

The weights are never transmitted: their cost is paid implicitly as the excess
loss incurred while the model is still bad ("regret").

**Why elegant.** The decoder is the training loop. Model cost is measured in the
same unit as data cost, with no need to choose a weight precision, and the
code is valid for any learner, including SGD with arbitrary hyperparameters,
as long as it is bit-exactly reproducible.

**Results.** Blier & Ollivier, *The Description Length of Deep Learning Models*
(NeurIPS 2018, arXiv:1802.07044) showed that variational/two-part codes for deep
nets give description lengths often *worse* than the uniform code on MNIST/CIFAR,
while prequential codes (implemented as block-wise retraining on growing
prefixes) give genuinely short codes, finally making "deep nets generalise
because they compress" quantitatively true. (Exact table values not reproduced
here — see paper.) Voita & Titov (arXiv:2003.12298) made "online code" MDL a
standard probing metric. Delétang et al., *Language Modeling Is Compression*
(arXiv:2309.10668) reframed large LMs as compressors and showed that once model
size is counted (two-part view), larger pretrained models stop paying off on
enwik-scale data — the scaling curve has an optimum.

Practical prequential compressors already dominate: Bellard's NNCP (LSTM, then
Transformer, trained online on enwik) and cmix are prequential codes with an
ensemble/mixer in front. Their decompressors are small because weights are
*re-derived*, not stored.

**Recent.** *Requential coding* (2026, arXiv:2607.11883; NeurIPS 2026 poster)
argues prequential code length is dominated by the data's own entropy and
proposes instead coding *teacher samples* relative to the student via relative
entropy coding, so code length ≈ cumulative teacher→student KL along training,
claimed to be independent of parameter count and often orders of magnitude
shorter **[claim from abstract; not reproduced]**. "Compressibility Measures
Complexity" (arXiv:2510.12077) connects prequential MDL to singular learning
theory (the learning coefficient λ replaces the classical d/2). "From Entropy
to Epiplexity" (arXiv:2601.03220) addresses information for computationally
bounded observers — relevant because Hutter's 50h/1-CPU budget is exactly such a
bound **[read abstract only]**.

**Open problems.** (i) Prequential code with *optimal* learning-rate/size
schedule — the regret term is schedule-sensitive and nobody has a closed form.
(ii) Combining a stored prior (pretrained weights) with online adaptation
optimally (section 7). (iii) Bit-exact determinism of floating-point training
across machines.

**tang-compress / Hutter.** Prequential is the default mode. tang's reverse-mode
AD + optimizers *are* the decoder; the hard engineering requirement is
**bit-exact replay**: fixed op order, no nondeterministic GPU reductions,
deterministic codegen to C for the decompressor. tang's small 11-op IR is an
asset here — one can prove the encoder and decoder execute identical graphs.
Note: Hutter's decompressor must train on 1 CPU in ≤50h, which caps online
model size (NNCP-class models are near that edge).

---

## 2. MML and Fisher-information precision (Wallace)

**Core statement.** Minimum Message Length (Wallace & Boulton 1968; Wallace &
Freeman 1987) asks how precisely to state θ. Quantising each parameter to a
cell of volume $V$ costs $-\log(\pi(\theta)V)$ bits and adds expected loss
from the rounding. Optimising $V$ under a quadratic approximation gives

$$L_{MML} \approx -\log \pi(\hat\theta) + \tfrac12\log\det I(\hat\theta) - \log p(x\mid\hat\theta) + \tfrac d2\big(1+\log\kappa_d\big),$$

with $I$ the (total) Fisher information and $\kappa_d$ the optimal lattice
quantising constant ($\kappa_1=1/12$). Optimal precision per parameter scales as
$\delta_i \propto 1/\sqrt{I_{ii}}$ (axis-aligned approximation).

**Why elegant.** It turns "how many bits per weight?" into a geometric
statement: flat directions (small Fisher) are cheap, sharp ones are expensive.
It is the same $\tfrac d2\log n$ asymptotics as BIC/NML, derived constructively.

**Results / modern echo.** Fisher-weighted quantisation underlies practical
post-training quantisers (OBS/OBC, GPTQ-style Hessian-aware rounding), and
Hessian-aware mixed precision (e.g. HAWQ). These are not usually framed as MML
but optimise essentially the same second-order objective.

**Open problems.** Singular models (neural nets) have degenerate Fisher, so
$\log\det I$ is $-\infty$; the right replacement is the SLT learning coefficient
(see arXiv:2510.12077). Practical MML for 10⁶–10⁸ parameters remains heuristic.

**tang-compress / Hutter.** tang can compute exact diagonal Fisher / Gauss-Newton
per weight symbolically. Use it to pick per-tensor (or per-group) bit widths for
the *stored* pretrained component: cost model
$\sum_i \log_2(\text{range}_i/\delta_i)$ + $\tfrac12\sum_i I_{ii}\delta_i^2/12 \cdot \log_2 e$.
For fx2-style 4-bit weights this is the principled way to decide which tensors
deserve 3, 4, or 6 bits.

---

## 3. Bits-back coding

**Core statement.** Hinton & van Camp (1993, "Keeping neural networks simple by
minimising the description length of the weights") and Frey & Hinton
(1996/97): if the sender samples weights (or a latent) $w\sim q(w)$ using bits
taken *from the stream itself*, then codes $w$ under prior $p(w)$ and data under
$p(x\mid w)$, the receiver can recover the random bits, so net cost is

$$L = \mathbb{E}_q[-\log p(x\mid w)] + \mathrm{KL}(q\,\|\,p) = -\mathrm{ELBO}.$$

The variational free energy *is* a code length.

**BB-ANS.** Townsend, Bird & Barber (ICLR 2019, arXiv:1901.04866) made this
practical with ANS's stack (LIFO) semantics: pop $z$ under $q(z\mid x)$, push
$x$ under $p(x\mid z)$, push $z$ under $p(z)$. Follow-ups: HiLLoC
(arXiv:1912.09953, hierarchical latents, full-size images), Bit-Swap (Kingma et
al., arXiv:1905.06845), Monte-Carlo bits-back to close the ELBO→log-likelihood
gap (Ruan et al., arXiv:2102.11086). The "initial bits" overhead is the known
practical tax.

**Why elegant.** Randomness is free if you can give it back. It explains why
the KL term, not the entropy of $q$, is the real cost of a noisy model.

**Open problems.** Initial-bits cost for short messages; numerical precision of
discretised continuous latents; applying bits-back to *weights* of a large
model is in principle possible but nobody does it at enwik scale.

**tang-compress / Hutter.** Relevant for latent-variable models of text (e.g. a
discrete "topic/section state" latent over Wikipedia articles), and for the
weight-coding interpretation below. tang needs an ANS backend that exposes
push/pop of arbitrary discretised distributions defined by tang graphs.

---

## 4. Relative entropy coding (REC) / MIRACLE

**Core statement.** Instead of quantising, send a *sample* from $q$ using
shared randomness. With a common seed both sides draw candidates
$w_1,\dots,w_K\sim p$; the sender picks an index via importance weights
$q(w_k)/p(w_k)$. With $K\approx 2^{\mathrm{KL}(q\|p)+t}$ the selected sample is
approximately $q$-distributed. Li & El Gamal's Poisson functional
representation (arXiv:1701.02827) gives the tight bound

$$\mathbb{E}[L] \le \mathrm{KL}(q\|p) + \log_2(\mathrm{KL}(q\|p)+1) + 4.$$

**MIRACLE** (Havasi, Peharz, Hernández-Lobato, ICLR 2019, arXiv:1810.00440)
applies this to network weights: train a variational posterior with a KL budget
per block, then encode each block's sample by its index. They reported
state-of-the-art compression of LeNet-5 and VGG-16 at the time
(~hundreds of × on LeNet) **[exact ratios unverified here]**. Later REC work:
Flamich, Havasi & Hernández-Lobato (arXiv:2010.01185, images), Theis & Yosri
(arXiv:2110.12805, survey of sample-communication algorithms), Flamich's greedy
Poisson rejection sampling (arXiv:2305.15313) giving runtime that is tractable
for 1-D/unimodal cases; runtime exponential in KL in general remains the
blocker, hence block-wise coding.

**Why elegant.** It achieves the bits-back rate *without* bits-back: no stream
juggling, no initial bits, just a seed and an index. Weights become
"$\approx$KL bits of choice among random draws".

**Open problems.** Encoding time $\sim 2^{\mathrm{KL}}$ per block forces small
blocks (≈ 10–20 bits each), costing the $\log$ overhead per block; good
non-factorised $q$ is hard. Requential coding (sec. 1) applies REC to *training
data* rather than weights — possibly the more promising direction.

**tang-compress / Hutter.** Highly attractive for the stored pretrained model:
weights decode from a PRNG seed + indices, which is decompressor-side cheap
(sample $K$ Gaussians, pick index). tang's diff stack can train $q$ with an
explicit $\beta\cdot\mathrm{KL}$ term per block. Requires a portable
deterministic PRNG and Gaussian sampler shared by encoder and decoder.

---

## 5. Variational / Bayesian weight coding and rate-penalised training

**Core statement.** Train with
$\mathcal{L} = \mathbb{E}_q[-\log p(x\mid w)] + \beta\, R(w)$, where $R$ is an
estimate of the weights' code length: KL to a prior (variational), or the
entropy of quantised weights under a learned density.

**Key work.** Ullrich, Meeds & Welling, soft weight-sharing (arXiv:1702.04008:
mixture-of-Gaussians prior; LeNet compressed by a large factor
**[ratio unverified]**). Louizos et al., Bayesian compression
(arXiv:1705.08665). Wiedemann et al., entropy-constrained training
(arXiv:1812.07520). Oktay, Ballé, Singh, Shrivastava, *Scalable Model Compression
by Entropy Penalized Reparameterization* (arXiv:1906.06624) — weights live in a
latent space with a learned entropy model, trained with straight-through
quantisation exactly like Ballé-style image codecs. Practical LLM quantisation
(GPTQ, QuIP#, AQLM, trellis/QTIP) is the engineering descendant; these give
~2–4 bits/weight for multi-billion-parameter LMs with modest perplexity loss.

**Why elegant.** A single differentiable objective makes "loss + bits" the
training target, which is exactly the Hutter score.

**Open problems.** Rate estimates are surrogates; straight-through gradient
bias; joint optimisation of weight rate and downstream mixer.

**tang-compress / Hutter.** This is the direct tang play: express
$R(w)$ (entropy model over quantised weight latents) as a tang graph, so
symbolic diff gives exact gradients of total score. For fx2-cmix-style entries
(6M params at 4 bit ≈ 3 MB **[back-of-envelope, not measured]**) going to a
learned entropy model + Fisher-aware bit allocation could plausibly save a
meaningful fraction of the weight bytes; each 1 MB saved is ~1% of the 100 MB
total.

---

## 6. NML and stochastic complexity (Rissanen, Grünwald)

**Core statement.** The minimax-regret code for a model class
$\{p_\theta\}$ is normalised maximum likelihood:

$$p_{NML}(x^n) = \frac{p_{\hat\theta(x^n)}(x^n)}{\sum_{y^n} p_{\hat\theta(y^n)}(y^n)},\qquad
\mathrm{COMP}_n = \log \sum_{y^n} p_{\hat\theta(y^n)}(y^n).$$

Rissanen (1996, "Fisher information and stochastic complexity", IEEE T-IT)
gives the asymptotic

$$\mathrm{COMP}_n = \tfrac d2 \log\tfrac{n}{2\pi} + \log\!\int\!\sqrt{\det I(\theta)}\,d\theta + o(1).$$

Grünwald, *The Minimum Description Length Principle* (MIT Press 2007), unifies
two-part, Bayes (Jeffreys prior achieves the same asymptotics), prequential and
NML as "universal codes"; Grünwald & Roos, "MDL revisited" (arXiv:1908.08484) is
the modern summary.

**Why elegant.** Model complexity = log of how many distinguishable
distributions the class contains (Jeffreys volume). It is parametrisation
invariant.

**Results.** For memoryless / Markov sources NML ≈ Krichevsky–Trofimov
(KT, add-½) estimator, whose regret is within a constant of $\tfrac{d}{2}\log n$:
this is why KT counters and CTW are near-optimal context-model primitives
and why cmix/paq counter-state machines look the way they do.

**Open problems.** NML is intractable for neural classes; the sum diverges for
many classes; singular models need SLT corrections.

**tang-compress / Hutter.** Use as theory for the *small* models: per-context
counters should be KT/NML-style (add-½, or sequential NML), and the
$\tfrac12\log n$ per-parameter regret is a budget rule for how many contexts a
context model can afford. Also a principled rule for when to split a context.

---

## 7. Hybrid pretrain + online schemes

**Core statement.** Total cost =
$L(\theta_0) + \sum_t -\log p_{\theta_t}(x_t)$ with $\theta_0$ stored (two-part)
and $\theta_t$ adapted online (prequential). Pretraining pays bytes to lower
early regret; online learning pays nothing in bytes but costs CPU time.

**Results.** This is exactly fx2-cmix-transformer's design (Ivanov 2026, Hutter record 100,424,672 B; ~6M-param
4-bit transformer pretrained offline, weights appended, mixed with context
models). Delétang et al. (arXiv:2309.10668) quantifies the trade-off from the
other side: very large pretrained LMs compress enwik9 extremely well *excluding*
weights, but are hopeless once weights are counted. NNCP is the pure-online
extreme. **I could not verify published ablations giving the optimum
pretrained-size/online-size split on enwik9.**

**Why elegant.** The optimum sits where marginal bytes of stored weights equal
marginal bits of saved regret — a clean first-order condition
$\partial L(\theta_0)/\partial s = -\partial L_{preq}/\partial s$ in model size $s$.

**Open problems.** Is pretraining on enwik9 itself (then shipping weights) ever
better than on other text? (Hutter rules effectively make the data the only
legal "corpus" to amortise.) Optimal fine-tune schedule given a fixed CPU budget.

**tang-compress / Hutter.** Make the split a tunable hyperparameter and measure
the curve. Combine: REC/entropy-coded $\theta_0$ (secs. 4–5) + deterministic
online adaptation (sec. 1) + KT-style context models (sec. 6) + a mixer.

---

## 8. Summary table

| Scheme | Model cost paid as | Decoder needs | Fit for Hutter |
|---|---|---|---|
| Two-part (quantised) | $L(\theta)$ bytes | dequantiser | stored prior |
| MML | Fisher-optimal precision | same | bit allocation |
| Prequential | regret | bit-exact trainer | core |
| Bits-back | KL(q‖p) | ANS + q | latents |
| REC/MIRACLE | KL + log KL per block | PRNG + index | stored prior |
| NML/KT | $\tfrac d2\log n$ | counters | context models |

## Sources
- Blier & Ollivier 2018 — https://arxiv.org/abs/1802.07044
- Voita & Titov 2020 — https://arxiv.org/abs/2003.12298
- Delétang et al. 2023 — https://arxiv.org/abs/2309.10668
- Requential coding 2026 — https://arxiv.org/abs/2607.11883 , https://neurips.cc/virtual/2026/poster/150681
- MDL meets SLT 2025 — https://arxiv.org/abs/2510.12077
- Epiplexity 2026 — https://arxiv.org/abs/2601.03220
- Townsend et al. BB-ANS — https://arxiv.org/abs/1901.04866 ; HiLLoC https://arxiv.org/abs/1912.09953 ; Bit-Swap https://arxiv.org/abs/1905.06845 ; MC bits-back https://arxiv.org/abs/2102.11086
- MIRACLE — https://arxiv.org/abs/1810.00440 ; REC images https://arxiv.org/abs/2010.01185 ; Theis & Yosri https://arxiv.org/abs/2110.12805 ; GPRS https://arxiv.org/abs/2305.15313 ; Li & El Gamal https://arxiv.org/abs/1701.02827
- Ullrich et al. https://arxiv.org/abs/1702.04008 ; Louizos et al. https://arxiv.org/abs/1705.08665 ; Wiedemann et al. https://arxiv.org/abs/1812.07520 ; Oktay et al. https://arxiv.org/abs/1906.06624
- Grünwald & Roos, MDL revisited — https://arxiv.org/abs/1908.08484
- Non-arXiv: Hinton & van Camp (COLT 1993); Wallace & Freeman (JRSS-B 1987); Rissanen (IEEE T-IT 1996); Grünwald (MIT Press 2007); Dawid (JRSS-A 1984).
