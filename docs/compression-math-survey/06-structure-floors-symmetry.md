# 06 — Structure, floors, and symmetry: power laws, attractors, Lyapunov rates, random-matrix cuts, orbits

Chapter 03 showed that a *shift-invariant, finite-memory* source compresses as a low-rank Hankel matrix, i.e. a fixed-bond MPS/QTT. This chapter is about what goes beyond that picture. Each section either gives a **floor** (a quantity no codec can go below), a **cut rule** (a principled way to decide what is signal and what is noise), or a **saving** (bits we never have to send because the structure fixes them). Unchecked claims are tagged [unverified].

---

## 1. Power-law correlations and hierarchical tensor networks

**Core statement.** Lin & Tegmark (arXiv:1606.06737) measure the mutual information between two symbols as a function of their distance in text, melodies and genomes, and find it decays roughly as a power law,

$$ I(X_t; X_{t+d}) \sim C\, d^{-\alpha}. $$

They prove that any probabilistic regular grammar, which includes Markov chains and HMMs with finitely many states, gives *exponential* decay,

$$ I(X_t; X_{t+d}) \le C\, \lambda_2^{\,d}, \qquad |\lambda_2| < 1, $$

where $\lambda_2$ is the second-largest eigenvalue modulus of the transfer matrix. A probabilistic context-free grammar, which generates text by recursive branching, can give a power law. A fixed-bond MPS is a finite-state transfer-matrix model, so it has a *correlation length* $\xi = -1/\ln|\lambda_2|$. It can only fake a power law over a window that grows with bond dimension $D$.

**Hierarchical networks.** A tree tensor network (TTN) is the tensor-network form of a branching grammar. Two leaves at distance $d$ connect through about $\log_2 d$ layers. If each layer shrinks the correlation by a constant factor $c$, then $I \sim c^{\log_2 d} = d^{\log_2 c}$, a power law. MERA (Vidal, quant-ph/0610099) adds *disentanglers* between neighboring branches. These remove the block-boundary artifacts of a plain tree, where two adjacent symbols that sit under different subtrees look far apart. MERA was built to describe critical (scale-invariant) quantum states, which have power-law correlations. Each MERA layer is one renormalization-group (RG) step: coarse-grain by 2, keep the $D$ most relevant degrees of freedom, discard the rest. **RG is compression.** The discarded subspace at each scale is exactly the information that is irrelevant at the next scale up.

**Language models built this way.** Pestun & Vlassopoulos (arXiv:1710.10248) propose MERA-like networks as language models. Their stated reason is that 1D chain models are "mass-gapped" (they have finite $\xi$) and cannot reproduce power-law MI. Gallego & Orús (arXiv:1708.01525) model language design itself as information renormalization with MERA-like structure. The sobering result is Lu et al. (arXiv:2103.06872): for natural text, the MI between a block and its complement scales as a power law in block size with exponent about 0.82, which is close to a volume law. They argue that MPS and TTN may not scale efficiently to long texts. A tree gives power-law *two-point* correlations. It does not give near-volume-law *block* entanglement unless the bond dimension grows with depth.

**What attention does.** Self-attention links any two positions in one hop, regardless of distance, so it has no built-in correlation length. Its cost is a KV cache that grows with context length $L$: memory is $O(L)$, not the $O(D^2)$ of an MPS. In tensor-network terms, a transformer is a model whose "bond dimension" (the cached state) grows linearly with the context. That is the resource a near-volume-law source needs. Recurrent and state-space models with a fixed state run into the same exponential-decay limit as an MPS [the formal statement for SSMs: unverified].

**Why it is elegant.** One scalar, the decay law of $I(d)$, tells you which model class you need. Exponential decay calls for an MPS or Markov model, a power law calls for a hierarchy, and volume-law block MI calls for state that grows with context.

**Open problems.** Is there a TTN or MERA with adaptive bond dimension that matches transformer bits-per-character on enwik8? No such benchmark has been found. Separately, what is the exact relation between block-entropy exponents and the achievable compression rate at a given model size?

**Mapping.** *cram:* the first diagnostic for any new data type should be a plot of estimated $I(d)$ on log-log axes. A straight line on semilog axes means the QTT/MPS machinery of chapter 03 is enough. A straight line on log-log axes means a hierarchical (TTN) or attention context model is needed. *Hutter:* enwik9 is a power-law source, which is why cmix-style mixers with long-range models, and now transformers, win. A TTN context model is a cheap additional mixer input that captures structure across scales. *vcad:* feature trees are literally trees, so a TTN over the parametric DAG is the natural factorization.

---

## 2. String attractors and the repetitiveness hierarchy

**Core statement.** A *string attractor* for $S[1..n]$ is a set of positions $\Gamma$ such that every distinct substring of $S$ has at least one occurrence that crosses some position in $\Gamma$. Write $\gamma$ for the size of the smallest attractor. Kempa & Prezza (arXiv:1710.10964) show that every dictionary compressor induces an attractor of its own size: LZ77 phrases ($z$), runs in the BWT ($r$), grammar size ($g$), and macro schemes ($b$). So all these compressors are approximation algorithms for one NP-hard problem. Computing $\gamma$ is NP-complete for $k$-attractors with $k \ge 3$. Kempa, Policriti, Prezza & Rotenberg (arXiv:1803.01695) cover verification and optimization of attractors.

Kociumaka, Navarro & Prezza (arXiv:1910.02151) define the **substring-complexity measure**

$$ \delta = \max_{k\ge 1} \frac{d_k(S)}{k}, $$

where $d_k$ is the number of distinct length-$k$ substrings. $\delta$ can be computed in linear time and is monotone, and $\delta \le \gamma$. They show that $z = O(\delta \log(n/\delta))$, so any string can be stored in $O(\delta\log(n/\delta))$ words. They also show the bound is tight in the sense that there are strings with $\gamma = \Omega(\delta \log(n/\delta))$. The known ordering, surveyed by Navarro (arXiv:2004.02781), is

$$ \delta \le \gamma \le b \le z \le g = O\!\big(z\log(n/z)\big), \qquad r = O(\delta \log\delta \cdot \log(n/\delta))\ \text{[exact form: unverified]}. $$

Kempa & Kociumaka (arXiv:1910.10631) settled the BWT conjecture: $r = O(z\log^2 n)$, which was the first nontrivial upper bound on $r$. They also give an $O(z\,\mathrm{polylog}\,n)$-time conversion from LZ77 to the run-length BWT.

**Why it is elegant.** It is a single combinatorial object, a set of positions that "hit" every substring, that lower-bounds LZ, grammars, BWT runs and macro schemes all at once. $\delta$ is also computable, unlike Kolmogorov complexity (chapter 05). It works as a *practical* surrogate for $K(x)$ on repetitive data.

**Open problems.** Can every string be stored in $O(\gamma)$ space, or in $o(\gamma\log n)$? Are the remaining polylog gaps between $r$, $z$ and $g$ tight? How should edit locality be handled? Theory measures repetitiveness globally, but a versioned history wants *incremental* attractors.

**Mapping.** *vcad:* CAD histories, `.vcad` version chains and undo stacks are extremely repetitive. Each revision differs from the last by a few parameter edits. Here $z$ and $r$ scale with the number of *edits*, not with total history length. A run-length BWT or LZ-End index over `document_versions` gives compressed storage plus fast "find when this feature changed" queries. *cram:* ship a `delta(S)` estimator, which is one linear pass over a suffix array or an approximate $d_k$ sketch, as the go/no-go test for using a dictionary back end versus a statistical model. *Hutter:* enwik9 is *not* highly repetitive in this sense. Wikipedia text has $\delta$ near $n/\text{polylog}$, which is why pure LZ loses there and context mixing wins. Repetitive substructures such as templates and markup are still worth a match model, which cmix already has.

---

## 3. Pesin's formula and the Lyapunov rate floor

**Core statement.** For a smooth ergodic system with an SRB/absolutely continuous invariant measure, **Pesin's identity** says

$$ h_{KS}(\mu) = \sum_{i:\ \lambda_i > 0} \lambda_i , $$

where $h_{KS}$ is the Kolmogorov–Sinai entropy (bits per unit time, when logarithms are base 2) and $\lambda_i$ are the Lyapunov exponents. In general only the Ruelle inequality $h_{KS} \le \sum \lambda_i^+$ holds. **Brudno's theorem** says that for $\mu$-almost every initial point, the algorithmic complexity rate of the symbolic trajectory, under a generating partition, equals $h_{KS}$:

$$ \lim_{T\to\infty} \frac{K(x_{0:T})}{T} = h_{KS}\quad \mu\text{-a.e.} $$

Boffetta et al. (nlin/0101029) put the two together: a chaotic system *generates* information at rate $\sum\lambda^+$. To reproduce a trajectory to precision $\epsilon$ for time $T$, you must transmit about $T\sum\lambda^+$ bits plus $O(\log 1/\epsilon)$ bits for the initial condition. No predictor, however clever, can do better on a typical orbit.

**Why it is elegant.** A dynamical quantity (how fast nearby states separate) equals an information quantity (how fast new bits appear) equals an algorithmic quantity (the incompressible description rate). It is the continuous-time analogue of Shannon's entropy rate.

**Two regimes for simulation replay.**
- *Stable or regular dynamics* (resting contact, damped joints, periodic gaits): all $\lambda_i \le 0$, so $h_{KS}=0$. A replay compresses to *initial state + controls + the deterministic integrator*, i.e. $O(\log T)$ bits beyond the inputs. Bit-exact replay comes from the code, not the data.
- *Chaotic dynamics* (stacking and tumbling contact, granular flow, turbulence): the rate floor is $\sum\lambda^+$ bits per second **if you want the actual trajectory**. A store-the-seed approach also fails in practice, because floating-point non-associativity across GPUs means the "same" program diverges after about $\log(1/\epsilon_{mach})/\lambda_{max}$ seconds. For turbulence the number of positive exponents grows with Reynolds number, so the floor grows extensively with resolution [scaling exponent: unverified].

The way out is the distortion axis of chapter 04. If only *statistics* matter (an ensemble, a plausible trajectory), the rate can drop far below $h_{KS}$, because you code the invariant measure, not the orbit.

**Open problems.** There are no practical, tight estimators of $h_{KS}$ for high-dimensional contact systems with non-smooth events, where Pesin's smoothness assumptions fail. What rate–distortion function $R(D)$ applies under a shadowing distortion (does some true orbit stay within $D$)?

**Mapping.** *phyz/cram:* phyz already has the tangent dynamics through tang's reverse-mode AD and the `phyz::diff` trajectory adjoint. A QR-based Lyapunov spectrum (Benettin's method) is a few hundred lines on top of that and gives a *per-episode bit budget*. Log a replay as seed + actions when $\sum\lambda^+ \cdot T \lesssim$ a few bits. Otherwise, insert checkpoint keyframes every $\sim 1/\lambda_{max}$ seconds and entropy-code the residuals. *vcad:* K1-style RL rollouts on GPU contact are mostly near-stable, so they should compress to the action stream. The tumbling or falling segments are where the bits go.

---

## 4. Random-matrix truncation: where noise begins

**Core statement.** Let $Y = X + \sigma Z$, where $X$ is low rank and $Z$ is an $m\times n$ matrix of iid noise with aspect ratio $\beta=m/n\le 1$. The singular values of the noise follow the **Marchenko–Pastur** law. Its bulk edge is at $\sigma\sqrt{n}\,(1+\sqrt\beta)$. Signal singular values below a detection threshold are swallowed by the bulk and *cannot* be recovered by any method. Gavish & Donoho (arXiv:1305.5870) find the AMSE-optimal *hard* threshold. For a square matrix with known $\sigma$ it is

$$ \tau^* = \frac{4}{\sqrt3}\sqrt{n}\,\sigma \approx 2.309\sqrt n\,\sigma . $$

With unknown $\sigma$ it is about $2.858\cdot y_{\mathrm{med}}$, where $y_{\mathrm{med}}$ is the median empirical singular value. For general $\beta$ the threshold is $\lambda(\beta)\sqrt n\sigma$ with

$$ \lambda(\beta) = \sqrt{2(\beta+1) + \frac{8\beta}{(\beta+1)+\sqrt{\beta^2+14\beta+1}}}, $$

and the unknown-$\sigma$ coefficient $\omega(\beta)$ is tabulated, roughly $0.56\beta^3-0.95\beta^2+1.82\beta+1.43$ [polynomial fit: unverified]. Guaranteed AMSE for a rank-$r$ square matrix: hard threshold at $\tau^*$ gives $3nr\sigma^2$, truncated SVD at the "right" rank $5nr\sigma^2$, tuned soft thresholding $6nr\sigma^2$, and the best possible shrinker $2nr\sigma^2$. The **optimal shrinkage** paper (arXiv:1405.7511) derives, for each of Frobenius, nuclear and operator loss, a unique admissible nonlinearity $\eta(y)$. For Frobenius loss in the square case it is $\eta(y)=\sqrt{(y^2-\beta-1)^2-4\beta}\,/y$ above the bulk edge and 0 below [form given from memory: unverified]. ScreeNOT (arXiv:2009.12297) extends optimal thresholding to correlated noise, with no assumption of white noise.

**Why it is elegant.** "How many singular values should I keep?" stops being a hyperparameter. It is a closed-form function of the shape and the noise, derived from the limiting spectral law of the noise.

**Neural weight spectra.** Martin & Mahoney (arXiv:1810.01075, short version arXiv:1901.08276) fit the empirical spectral density of trained weight matrices. Random initializations follow Marchenko–Pastur. Well-trained state-of-the-art networks show *heavy-tailed* ESDs, $\rho(\lambda)\sim\lambda^{-\alpha}$, with smaller $\alpha$ correlating with better generalization. They call this heavy-tailed self-regularization. For compression, a heavy-tailed spectrum does *not* have a clean "signal spikes + MP bulk" split. Gavish–Donoho assumes the signal is a few outliers, which is true for initial-like layers and false for heavy-tailed ones. There, the tail *is* the signal, and truncating to the MP edge throws away learned structure [practical consequence: inferred, unverified].

**Open problems.** We lack optimal thresholds for *sequential* truncation (TT-SVD applies a cut per unfolding and errors compound), and thresholds for heavy-tailed noise or signal. The joint optimum of rank truncation and quantization of the kept factors (chapter 04) is also open.

**Mapping.** *cram:* replace the $\epsilon/\sqrt{d-1}$ per-unfolding rule in TT-SVD/QTT (chapter 03) with a Gavish–Donoho cut per unfolding whenever the data has a measurement-noise floor, such as sensor logs, scanned meshes or simulation output with discretization noise. Each unfolding has its own $\beta$, so use $\lambda(\beta)$ or $\omega(\beta)$ per core. Do not compress below the noise: bits spent coding the MP bulk are bits spent coding the noise. Use ScreeNOT when the noise is colored. *Hutter / model compression:* when compressing the weights of a learned model (a transformer mixer in the decompressor counts toward the Hutter size), fit the ESD tail exponent per layer first. Layers that look MP-like are candidates for aggressive low rank. Heavy-tailed layers should be quantized, not truncated.

---

## 5. Symmetry savings: code the orbit, not the representative

**Core statement.** If a group $G$ acts on the data, and the decoder only needs the data up to $G$, then sending an arbitrary element of an orbit wastes up to $\log_2|G|$ bits:

$$ H(X) = H([X]_G) + H(g \mid [X]_G) \;\Rightarrow\; \text{saving} \le \log_2 |G| \text{ (equality on free orbits, uniform } g). $$

The standard tool is a **canonical form**: a function $c$ with $c(gx)=c(x)$ that picks one representative per orbit. Encode $c(x)$, and send $g$ only if it matters. In practice that means a sort order for sets, canonical graph labelings (nauty-style) for graphs, and a principal-axes frame for rigid poses.

**Permutations.** An unordered set of $n$ distinct points, sent as a list, costs $\log_2 n! \approx n\log_2 n - 1.44n$ extra bits. For a million-point cloud that is about $18.5$ Mbit, roughly $2.3$ MB, of pure ordering information (the arithmetic from Stirling's formula). Mesh coders have long exploited this. Isenburg, Lindstrom & Snoeyink's streaming-mesh work notes that standard formats specify a *permutation* of vertices and triangles as well as connectivity, and that connectivity coders save by bringing elements into a canonical order (traversal order). Chiang et al. show that choosing the vertex order to minimize geometry entropy is a minimum-entropy Hamiltonian path problem and is NP-hard. Octree and sorted-coordinate point-cloud codecs (MPEG G-PCC-style) get the $\log n!$ for free, because the octree *is* a canonical order. Learned codecs inherit permutation invariance through PointNet-style symmetric pooling (e.g. arXiv:2308.05959, learned point-cloud compression for classification).

**Bits-back for sets.** When no cheap canonical form exists, bits-back coding achieves the $\log n!$ saving by *decoding* a permutation from the ANS stack before encoding (shuffle-coding, chapter 02's ANS). The reference to the specific paper is [unverified].

**Equivariant models.** A $G$-equivariant predictor assigns equal probability to every element of an orbit, so its code length is already orbit-optimal up to the canonicalization cost. It also uses $|G|$ times fewer effective parameters for the same data, which is a smaller MDL model term (chapter 01).

**Why it is elegant.** The saving is exact, combinatorial and data-independent: $\log_2|G|$ bits, known before you see the data.

**Open problems.** Canonical forms for approximate symmetry, such as near-mirrors and patterns with one perturbed instance, where you need a residual against the symmetric prediction. Cheap canonical labeling of large meshes and B-rep graphs. Symmetry that is only partial or local.

**Mapping.** *vcad:* parametric CAD is the best case. A circular pattern of $k$ instances is the orbit of $C_k$: send one instance plus $(k,\text{axis},\text{angle})$ instead of $k$ copies, which is exactly what the `.vcad` DAG already does. The work is in **recovering** symmetry from dumb geometry (STEP import, scanned meshes). Detect mirror/pattern groups, canonicalize, code the representative, and code the residual against the symmetric prediction. For exported meshes, canonical vertex order (sorted octree or traversal) removes the $n\log n$ term outright. *cram:* make "canonicalize under declared group" a first-class pass before entropy coding, with the group declared by the caller: permutations for sets, $SE(3)$ for rigid bodies, $C_k/D_k$ for patterns. *Hutter:* little rigid symmetry exists in text. The analogue is order-invariance inside Wikipedia lists and tables, which is a small win [unverified magnitude].

---

## Synthesis: how chapter 6 refines chapters 2–3

- **Model class by correlation law.** Chapter 03's MPS/QTT is the right tool for *exponentially-mixing* or exactly low-rank shift-invariant signals: sensor streams, smooth fields, simulation states. Section 1 says text and other hierarchical sources need *hierarchical* (TTN/MERA) or *growing-state* (attention) models. The decision is made by measuring $I(d)$, not by taste.
- **Mixing needs a floor.** Chapter 02's universal mixers converge to the entropy rate. Sections 2 and 3 give computable stand-ins for that rate: $\delta$ for repetitive data and $\sum\lambda^+$ for dynamics. They tell a mixer when it is done and when no more model capacity will help.
- **Rank needs a noise model.** Section 4 turns chapter 03's rank truncation from an $\epsilon$ knob into a statistical decision. It also warns that heavy-tailed spectra, as in neural weights, break the spike-plus-bulk assumption.
- **Symmetry comes first.** Section 5 is a preprocessing law. Quotient by the known group, then model. Every bit of $\log|G|$ removed up front is a bit no mixer has to learn.

**Practical order for cram:** canonicalize by symmetry, measure $I(d)$, $\delta$ and $\sum\lambda^+$, choose between dictionary, MPS/QTT, TTN and attention, apply Gavish–Donoho cuts, then quantize and entropy-code (chapter 04).
