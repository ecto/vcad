# 05 — Algorithmic Information Theory and Program-Based Compression

> Scope: the "compression = shortest program" view, from the uncomputable ideal
> (Kolmogorov, Solomonoff) through resource-bounded variants (Levin, speed prior)
> down to things we can actually build (MDL, grammar compression, symbolic
> regression, library learning, CAD program inference, deterministic replay).
>
> Citation note: arXiv IDs below are from the author's knowledge and were not all
> re-fetched for this draft. Items marked **[unverified]** should be checked
> before the survey is published. No benchmark numbers were invented; where a
> number is quoted it is flagged with its source or as unverified.

---

## 1. Kolmogorov complexity and the invariance theorem

**Core statement.** Fix a universal prefix-free machine U. The (prefix) complexity
of a string x is

    K_U(x) = min { |p| : U(p) = x }.

*Invariance theorem:* for any other universal machine V there is a constant c_{UV}
(the length of a V-interpreter written for U) such that

    |K_U(x) − K_V(x)| ≤ c_{UV}   for all x.

Key facts: K(x) ≤ |x| + O(log |x|); most strings are incompressible (fewer than
2^{n−k} programs shorter than n−k); K is uncomputable (Berry paradox) but
upper-semicomputable (run all programs in parallel, keep the shortest that halts
with x). Coding theorem (Levin): −log m(x) = K(x) + O(1), where m is the universal
discrete semimeasure m(x) = Σ_{p: U(p)=x} 2^{−|p|}. Expected K under a computable
distribution P equals Shannon entropy H(P) up to K(P) + O(1).

**Why elegant.** One definition of "information content of an individual object",
machine-independent up to an additive constant, which recovers Shannon entropy
on average. Compression ratio of any real compressor is an upper bound on K.

**Open problems / caveats.** The constant c_{UV} is not small in practice (it is
the size of an interpreter); for finite data like enwik9 the choice of reference
machine is *the* modeling decision. Hutter Prize rules make this concrete: the
decompressor's size is counted, so the "reference machine" is x86 + the binary.

**Mapping.** tang-compress should report every result as a *two-part* length:
`|decoder| + |payload|`. The tang expression graph (11 scalar ops) is a natural
small reference machine: c_{UV} for tang-expr is the size of its evaluator. For
vcad, `|loon source|` (or compressed `.vcad` IR) is a literal upper bound on
K(mesh | kernel).

## 2. Solomonoff induction and its optimality

**Core statement.** The universal a-priori semimeasure on sequences with a
monotone universal machine U:

    M(x) = Σ_{p : U(p) = x*} 2^{−|p|}

(sum over minimal programs whose output starts with x). Prediction:
M(x_{t+1} | x_{1:t}) = M(x_{1:t} x_{t+1}) / M(x_{1:t}).

*Solomonoff's bound:* for any computable measure μ generating the data,

    Σ_t E_μ[ (M(0|x_{<t}) − μ(0|x_{<t}))² ] ≤ (ln 2 / 2) · K(μ)

(binary case), i.e. total expected squared prediction error is finite and bounded
by the complexity of the true source. Equivalently, M's cumulative log-loss
exceeds μ's by at most K(μ) ln 2 — a code built from M is within K(μ) bits of the
optimal code for *any* computable source.

**Why elegant.** Combines Occam (2^{−|p|} prior), Epicurus (keep all consistent
hypotheses), and Bayes; dominance M ≥ 2^{−K(μ)} μ gives a regret bound that is a
single constant. Arithmetic-coding with M is the ideal compressor.

**Open problems.** Incomputable; even approximations need a time bound (sections
3–5). Choice of U again matters for finite data.

**Mapping.** Every context-mixing compressor (PAQ/cmix) is a crude finite
Bayesian mixture approximating M: model weights ≈ 2^{−|model|} adjusted by
past log-loss. tang-compress' mixer layer should be designed explicitly as a
Bayes/Solomonoff mixture (log-loss weights, priors from description length of
each submodel) rather than an ad hoc gated linear mix — then the regret bound
"≤ prior-cost of the best expert" is a provable property we can test.

## 3. Levin's Kt complexity and Levin search

**Core statement.**

    Kt(x) = min_p { |p| + log₂ t(p, x) }

where t(p,x) is the running time of p to produce x. *Levin universal search*
allocates time 2^{−|p|} · T to each program p in phase T = 1,2,4,…; any inversion
problem solvable by program p in time t is solved in time O(2^{|p|} · t), i.e.
within a constant factor (enormous, 2^{|p|}) of optimal.

**Why elegant.** Turns "shortest program" into "shortest program that is also
fast", and yields an asymptotically optimal search algorithm. Kt is computable.

**Empirical.** Direct Levin search is only practical for tiny programs; it
underlies OOPS (Schmidhuber's Optimal Ordered Problem Solver, 2002–2004,
arXiv cs/0207097 **[unverified ID]**) which reuses earlier found code.

**Mapping.** The Hutter Prize is literally a Kt-style objective with a hard time
cap: minimise |decompressor + archive| subject to t ≤ ~50 h. For tang's
Sheffer-operator search crate, Levin's time-allocation schedule (budget ∝ 2^{−len})
is the principled enumeration order — expression length in bits, not node count.

## 4. Schmidhuber's speed prior

**Core statement.** Schmidhuber, "The Speed Prior: a new simplicity measure
yielding near-optimal computable predictions", COLT 2002. The prior S weights
program p producing x by roughly 2^{−|p|} / t(p) — derived from the FAST
algorithm that in phase i runs all programs of length ≤ i for 2^{i−|p|} steps.
Mass of x is dominated by programs minimising |p| + log t, i.e. it is the
probabilistic counterpart of Kt.

**Why elegant.** Asserts a testable cosmological/statistical claim: data is
generated by *fast* processes, so beliefs should down-weight slow explanations.
Gives computable predictions with Solomonoff-style loss bounds relative to
S-computable sources.

**Mapping.** For replay compression (section 12), the speed prior is the right
prior: a phyz simulation that reproduces a trajectory in 10 s is better than one
needing 10 h even at equal code length, because the decoder must re-run it.

## 5. Resource-bounded Kolmogorov complexity

**Core statement.** K^t(x) = min{|p| : U(p) = x within t(|x|) steps}. Variants:
distinguishing complexity CD^t, probabilistic pK^t, and Kt. Modern
"meta-complexity" results connect these to cryptography: e.g. Liu–Pass (FOCS 2020,
arXiv 2009.11514 **[unverified ID]**) show one-way functions exist iff K^t is
mildly hard on average for a polynomial t. Hirahara and others link MKtP/MCSP
hardness to NP-hardness questions.

**Why elegant.** Bridges compression and complexity theory: "compressing
time-bounded strings well on average is exactly as hard as breaking crypto."

**Open problems.** Is computing K^t (or MCSP) NP-hard? Exact relationships
between pK^t, Kt and average-case hardness remain active.

**Mapping.** A theoretical ceiling: there is no general efficient optimal
compressor for time-bounded sources unless OWFs fail. Expect gains from *domain
structure* (CAD, physics, text grammar), not generic search.

## 6. AIXI and the rationale of the Hutter Prize

**Core statement.** Hutter's AIXI (arXiv cs/0004001, "A Theory of Universal
Artificial Intelligence based on Algorithmic Complexity", 2000; book *Universal
Artificial Intelligence*, 2005) chooses actions by expectimax over a Solomonoff
mixture of environments:

    a_t = argmax_{a_t} Σ_{o_t r_t} … max_{a_m} Σ_{o_m r_m} [r_t + … + r_m]
          · Σ_{q : U(q, a_{1:m}) = o_{1:m} r_{1:m}} 2^{−|q|}

AIXItl is a time/length-bounded (still impractical) version.

**Hutter Prize rationale** (prize.hutter1.net, explicit on the site): since
optimal prediction ≡ optimal compression (via arithmetic coding and the Solomonoff
bound), and intelligence ≈ prediction in AIXI, compressing human knowledge
(Wikipedia) is a proxy benchmark for AI. Lossless is required because lossy
metrics are not objective; the decompressor is counted to forbid hiding the data;
resource limits (single core, ~50 h, ~10 GB RAM, no GPU in the current enwik9
rules — **verify current rules**) prevent brute-force model sizes.

**Empirical.** The record line is held by cmix/PAQ-derived context-mixing
programs (fx-cmix / fx2-cmix by Kaido Orav and Byron Knoll in 2024, ~110–112 MB;
current record fx2-cmix-transformer by Vladimir Ivanov, 2026, 100,424,672 bytes,
which adds a ~6M-param 4-bit transformer pretrained offline, per prize.hutter1.net). LLM-based
compressors (e.g. "Language Modeling Is Compression", Delétang et al., arXiv
2309.10668) achieve much better raw ratios but violate the size/compute
constraints once the model weights are counted.

**Mapping.** Under the 1-CPU constraint, a tang-compress Hutter attempt should
target *small online-trained* models (context mixing + small LSTM/transformer
trained during decompression), exploiting tang's codegen for fast CPU kernels and
reverse-mode AD for online gradient steps. Determinism (section 12) is
mandatory: encoder and decoder must compute bit-identical probabilities, so all
floating-point in the model path must be reproducible (fixed op order, no
FMA-contraction drift, or fixed-point arithmetic).

## 7. MDL as a computable proxy

**Core statement.** Rissanen's Minimum Description Length. Crude two-part form:

    H* = argmin_H [ L(H) + L(D | H) ].

Refined MDL uses universal codes for a model class: normalized maximum likelihood
(NML) P_NML(D) = P(D | θ̂(D)) / Σ_{D'} P(D' | θ̂(D')), whose log-normalizer is the
parametric complexity COMP(M) ≈ (k/2) log(n/2π) + log ∫ √det I(θ) dθ. Also
prequential (predictive) MDL: L(D) = Σ_t −log P(x_t | x_{<t}) — exactly what an
adaptive compressor pays. Reference: Grünwald, *The Minimum Description Length
Principle* (MIT Press, 2007).

**Why elegant.** Replaces "shortest program over all programs" by "shortest code
within a model class", which is computable and still penalises overfitting
automatically. Prequential MDL means the compressor *is* the model selector.

**Mapping.** tang can compute Fisher information symbolically, so the
(k/2) log n + Fisher-volume term is exactly computable for tang-expr model
families. For vcad: choose between "store mesh" vs "store parametric feature +
residual" by a two-part code length comparison.

## 8. Logical depth and sophistication

**Bennett's logical depth.** depth_s(x) = min{ t(p) : U(p) = x, |p| ≤ K(x) + s }:
the running time of near-shortest programs. Random strings and trivial strings are
both shallow; deep objects (genomes, solved chess endgames, simulation outputs)
require long computation from their shortest description.

**Sophistication (Koppel; Gács–Tromp–Vitányi "algorithmic statistics").** Split
x's shortest description into a model set S ∋ x and an index:
K(x) ≈ K(S) + log|S|. Sophistication = K(S) for the minimal sufficient statistic;
the "structure function" h_x(α) = min{log|S| : K(S) ≤ α} traces the trade-off
(Vereshchagin–Vitányi, 2004).

**Why elegant.** Distinguishes *meaningful* structure from randomness and from
mere regularity; formalises "model vs noise" as a split of bits.

**Mapping.** Replay-based compression targets deep data: a CAD mesh or a physics
trajectory has small K but large depth — the decoder must spend time. The
structure function is the right picture for lossy-with-residual coding in vcad:
parametric model = K(S), residual = log|S|.

## 9. Practical program-synthesis compressors

### 9.1 Symbolic regression with MDL
- **AI Feynman** (Udrescu & Tegmark, arXiv 1905.11481; AI Feynman 2.0, arXiv
  2006.10782): recursive simplification via dimensional analysis, symmetry,
  separability detected with NN fits; 2.0 uses a Pareto front of description
  length vs. error, explicitly MDL-scored. Reported solving all 100 equations of
  their Feynman benchmark (v1) — see paper for exact figures.
- **PySR** (Cranmer, arXiv 2305.01582): multi-population evolutionary search
  in Julia (SymbolicRegression.jl) with a complexity–loss Pareto front;
  complexity is node-count weighted, not bits.
- **Bayesian Symbolic Regression** (Jin et al., arXiv 1910.08892): MCMC over
  expression trees with priors on structure. Related: Guimerà et al. "A Bayesian
  machine scientist" (Science Advances 2020) uses a description-length posterior
  directly.
- Recent: SRBench (La Cava et al., arXiv 2107.14351) is the comparison suite.

**Mapping.** tang-expr + symbolic diff is a ready substrate: enumerate/evolve
expressions, fit constants by reverse-mode AD, score in *bits*
(L(expr) + L(constants at precision δ) + L(residuals)). Optimal constant
precision δ ≈ 1/√(n·Fisher) falls out of tang's symbolic Hessian.

### 9.2 DreamCoder / library learning
Ellis et al., "DreamCoder" (arXiv 2006.08381, PLDI 2021): wake–sleep loop:
wake = search programs for tasks under a learned prior; sleep-abstraction =
refactor solutions to extract reusable library functions (via version spaces /
e-graph-like refactoring) minimising total description length of library +
programs; sleep-dream = train a neural recognition model on fantasies. Follow-ups:
Stitch (Bowers et al., arXiv 2211.16605, top-down corpus-guided abstraction,
much faster), babble (Cao et al., e-graph anti-unification, arXiv 2212.04596
**[unverified ID]**).

**Mapping.** The library is a learned dictionary in program space — the code
analogue of LZ's dictionary. For vcad, run Stitch-style abstraction over a corpus
of loon programs to grow a CAD stdlib (lib/src/lib.loon) that minimises
corpus code length; that library becomes the shared prior/decoder.

### 9.3 Grammar-based compression and SLPs
A straight-line program (SLP) is a CFG generating exactly one string. Smallest
grammar problem is NP-hard and APX-hard; Charikar et al. (IEEE TIT 2005) give
O(log(n/g*)) approximations and show LZ78 / Sequitur / RePair have poly-log or
worse approximation ratios. Key elegance: many algorithms (pattern matching,
equality, random access in O(log n)) run directly on SLPs without decompression
(Lohrey's survey, "Algorithmics on SLP-compressed strings", 2012).

**Mapping.** tang expression DAGs *are* SLPs over a richer alphabet (arithmetic
ops instead of concatenation). CSE/hash-consing in tang-expr is grammar
compression. A "SLP over scalar ops" is a natural codec for numeric arrays with
algebraic structure.

### 9.4 Lempel–Ziv as crude program search
LZ77 output is a program in a two-instruction language (`literal c`,
`copy(dist,len)`). It is universal: for stationary ergodic sources, the LZ rate
converges to the entropy rate (Ziv–Lempel 1977/78), and LZ78 complexity bounds
K from above. It finds *only* copy-structure, which is why it is weak on data
with arithmetic regularity (e.g. a quadratic sequence costs ~linear bits).

**Mapping.** Treat LZ as the baseline "instruction set" and add instructions
(e.g. `eval tang-expr`, `replay sim`, `call CAD feature`) — each extension is a
richer reference machine with potential exponential savings on in-domain data.

## 10. LLMs, in-context learning and Solomonoff induction (2023–2026)

- **Delétang et al., "Language Modeling Is Compression"** (arXiv 2309.10668,
  ICLR 2024): arithmetic coding with Chinchilla 70B compresses ImageNet patches
  and LibriSpeech below PNG/FLAC in raw rate; once model size is counted the
  advantage disappears for small datasets — a direct illustration of the two-part
  code.
- **Grau-Moya et al., "Learning Universal Predictors"** (DeepMind, arXiv
  2401.14953, ICML 2024): meta-train transformers/LSTMs on data from Universal
  Turing Machine programs (sampled with a length prior) so the network
  approximates a Solomonoff mixture; shows transfer to other algorithmic tasks and
  that larger transformers approach Bayes-optimal on variable-order Markov
  sources.
- In-context learning as implicit Bayesian inference: Xie et al. (arXiv
  2111.02080). Huang et al., "Compression Represents Intelligence Linearly"
  (arXiv 2404.09937): benchmark scores of LLMs correlate ~linearly with
  compression rate on held-out text — **[verify exact claims]**.
- 2025–2026: continued work on "prequential code length" of in-context learners
  (e.g. Elmoznino et al., "In-context learning and Occam's razor",
  arXiv 2410.14086 **[unverified ID]**) arguing next-token training minimises
  prequential code length = data cost + implicit model cost.

**Mapping.** For Hutter (no model weights for free), the actionable idea is the
*prequential* view: train online during decompression so model cost is paid in
compute, not bytes. For tang-compress generally: offer a "meta-trained prior"
mode (ship weights once, amortised over many files), the regime where LLM
compression wins.

## 11. Inverse procedural modeling / CAD program inference as compression

- **CSGNet** (Sharma et al., arXiv 1712.08290, CVPR 2018): RNN emits CSG
  programs (primitives + boolean ops) from 2D/3D shapes, trained supervised then
  with REINFORCE on reconstruction reward.
- **ShapeAssembly** (Jones et al., arXiv 2009.08026, SIGGRAPH Asia 2020):
  domain-specific language of cuboids + attachments; hierarchical program
  generation of PartNet shapes. Follow-up ShapeMOD (arXiv 2104.06392) does
  library/macro discovery — DreamCoder for CAD.
- **CAD-sequence datasets/models**: DeepCAD (Wu et al., arXiv 2105.09492,
  sketch-and-extrude sequences); Fusion 360 Gallery (Willis et al.,
  arXiv 2010.02392).
- **CAD-code LLMs (2024–2026)**: e.g. CAD-Recode (point cloud → CadQuery Python,
  arXiv 2412.14042 **[unverified ID]**), Text2CAD, CAD-Coder variants. These are
  inverse procedural modelers whose output is a short program.

**Compression framing.** A mesh with N triangles costs O(N) bytes; its generating
CAD program costs O(#features). Code length ratio can be 10³–10⁶ for designed
parts. The residual (mesh minus kernel replay) must be coded too for lossless.

**Mapping.** vcad is the decoder. A codec "vcad-mesh" = `loon program +
tessellation params + residual`. Inference can use vcad-kernel-diff gradients
(dx/dθ) to fit continuous parameters after a discrete program search proposes
structure — exactly the AI-Feynman pattern (discrete search + gradient fit + MDL
score) transplanted to geometry.

## 12. Simulation / replay-based compression and bit-exact determinism

**Core statement.** If data D = Sim(θ, seed, T) for a known simulator, then
K(D) ≤ |Sim| + |θ| + |seed| + O(log T) — the initial conditions plus program.
Lossless requires the decoder to reproduce D *bit-exactly*, or to code the
residual D − Sim(·) which must be small and itself predictable.

**Determinism requirements.**
- IEEE-754 is deterministic per operation, but results depend on op order,
  FMA contraction, denormal flags, transcendental library implementations,
  parallel reduction order, and GPU vendor. Cross-platform bit-exactness needs
  fixed evaluation order, either strict-IEEE with own libm (e.g. correctly-rounded
  CORE-MATH style) or fixed-point/integer arithmetic.
- Chaotic systems amplify any 1-ulp difference exponentially — Lyapunov time
  bounds how far replay can run before mismatch; checkpoint/residual keyframes
  every ~Lyapunov horizon.
- Game-engine lockstep netcode (e.g. RTS replays storing only inputs) is the
  industrial precedent: replay files are tiny precisely because of enforced
  determinism.

**Why elegant.** It is literally K(x) via the shortest *known* generating program;
logical depth (section 8) moves from storage to decoder compute.

**Mapping.** tang codegen should provide a "deterministic mode": a single
canonical lowering (no reassociation, no fast-math, deterministic reductions,
own transcendental impls built from the 11 scalar ops) shared by encoder and
decoder on CPU and GPU. phyz trajectories, vcad tessellation and the Hutter
model's probability computation all require this. Test: hash of output across
x86_64/aarch64/GPU must match.

---

## Summary of strongest ideas

1. Every compressor is an upper bound on K(x); report results as two-part code |decoder| + |payload|.
2. Context-mixing ≈ finite Solomonoff mixture; design tang-compress' mixer as a true Bayes mixture with provable regret ≤ expert prior cost.
3. Hutter Prize = Kt with a hard time cap; prequential (online-trained) models pay model cost in compute, not bytes.
4. Levin-search time allocation (budget ∝ 2^{−len}) is the principled enumeration order for tang's Sheffer/expression search.
5. Speed prior is the right prior for replay compression: prefer fast generators.
6. MDL with Fisher-volume parametric complexity is exactly computable with tang's symbolic diff.
7. Symbolic regression = discrete search + AD constant fitting + bits-scored Pareto (AI Feynman 2.0 pattern).
8. DreamCoder/Stitch library learning over loon corpora grows a CAD stdlib that is a learned decoder prior.
9. CAD program inference (CSGNet, ShapeAssembly, CAD-code LLMs) + vcad-kernel-diff fits = mesh codec with orders-of-magnitude gains on designed parts.
10. Replay compression needs bit-exact determinism: a canonical deterministic lowering in tang codegen is a prerequisite for phyz/vcad/Hutter codecs.
