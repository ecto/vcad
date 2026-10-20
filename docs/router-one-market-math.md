# One-market router: mathematical foundations

> Research synthesis. [V] = citation verified by web search; [M] = from memory, unverified; [U] = unconfirmed. Verify [M]/[U] before publishing.

## "One market" router: research synthesis (vcad, CM5 10-layer HDI)

**How well this is verified.** Effort was limited. I checked 11 citations by web search; they are marked [V]. Citations marked [M] come from my memory and I'm confident in them but did not re-check them this session. One item is marked [U]: I believe it exists but could not confirm it. No code was touched.

---

### (a) Top results, ranked by elegance × leverage

**1. Gross substitutes ⇔ M♮-concavity, so Walrasian prices exist. This is the "one market" theorem.**
- **Statement (Fujishige–Yang 2003 [V]).** A valuation on {0,1}^n satisfies the Kelso–Crawford gross substitutes condition if and only if it is M♮-concave.
- **Existence (Kelso–Crawford 1982 [M]; Gul–Stacchetti 1999 [M]).** If every agent's valuation is gross substitutes, a Walrasian equilibrium exists. The equilibrium prices form a lattice, and an ascending auction reaches the minimal price vector.
- **Network-induced valuations are M♮-concave (Murota, *Discrete Convex Analysis*, SIAM 2003 [M]).** If an agent's value is a max-weight flow or matching from its demands into the resources it holds, that value is M♮-concave. Assignment (OXS) valuations are the canonical example.
- **Bundle-LP view (Bikhchandani–Mamer 1997 [M]).** A Walrasian equilibrium exists exactly when the bundle-assignment LP has an integral optimum. The equilibrium prices are that LP's dual variables.
- **Where it plugs in.** It makes the auction and min-cost-flow layers (via sites, plane access, BGA escape) exact markets, with zero duality gap at equilibrium. Details are in (b).

**2. Lagrangian bound with an integral sub-polytope (Geoffrion 1974 [M]) plus a certified Sinkhorn dual.**
- **Statement.** Take min{cx : Ax ≤ b (coupling), x ∈ X}. The Lagrangian dual equals min{cx : Ax ≤ b, x ∈ conv(X)}. If each block of X is integral, the Lagrangian bound equals the LP bound.
- **Why it applies.**
  - The Birkhoff polytope is integral (Birkhoff–von Neumann).
  - Single-commodity flow polytopes are integral.
  - So dualizing only the shared capacities, with λ, gives each solver an integral best-response problem.
- **Answer to Problem 1 (joint certificate).** Yes, a combined dual exists, and the construction is simple.
  - Variables: P ∈ Birkhoff for each byte lane, DQ group or polarity choice, plus the flows f.
  - Coupling: capacity rows, plus linking rows that say "lane π(i) uses the terminals of slot j", f_{k,j} ≤ P_{ij}.
  - Dualize capacities with λ ≥ 0 and the linking rows with μ.
  - Then L(λ, μ) = Σ_k min-cost-flow_k(c + λ) + min_{P∈Birkhoff} ⟨C(μ), P⟩ − λᵀb.
  - Every term is an exactly solvable integral oracle, so L is a valid lower bound for any (λ, μ) by weak duality.
- **Catch: Sinkhorn solves the entropic problem, not the assignment.** Its potentials (f, g) are not dual-feasible as they come out. Repair them with the c-transform g'_j = min_i (C_ij − f_i). That gives an exactly feasible assignment dual whose value is at most the optimum. So the receipt can carry a certified bound, not ε-fuzzy.
- **Why quadratic assignment doesn't arise.** Crosstalk between swapped lanes would make the cost quadratic in P. Keep that out of the market: price it in the SI descent stage, then pass it back as a linear μ.
- **Where it plugs in.** The receipt's duality gap is UB_oracle − max over iterates of L(λ, μ). It holds at every step, with no convergence requirement.

**3. Exact feasibility for homotopic routing: Schrijver's cut condition.**
- **Statement (Schrijver, "Disjoint homotopic paths and trees in a planar graph", DCG 6 (1991) 527–574 [V]).** In a planar graph with holes, take prescribed homotopy classes C_1..C_k. Pairwise vertex-disjoint paths P_i ∈ C_i exist if and only if the instance passes a cross-free test and a cut condition. The cut condition requires, for every curve D (the dual of a cut), that |D ∩ G| ≥ Σ_i mincr(C_i, D). Here mincr is the minimum number of crossings of D with any path in class C_i. The certificate is polynomial and constructive.
- **Edge-disjoint companion (Schrijver, SIAM J. Discrete Math 1991 [V]).** It extends Kaufmann–Mehlhorn results on grid graphs.
- **Okamura–Seymour (JCTB 31 (1981) 75–81 [M]).** In a planar Eulerian graph with all terminals on the outer face, the cut condition is sufficient for edge-disjoint paths.
- **Continuous version.**
  - Leiserson–Maley (STOC 1985, 69–78 [V]) and Cole–Siegel ("River routing every which way, but loose", FOCS 1984 [U]): a homotopic one-layer route with width and spacing exists if and only if every cut between obstacle features has capacity ≥ its flow. Here flow is the number of wires whose class crosses the cut, and only polynomially many critical cuts between point pairs need checking.
  - Gao–Jerrum–Kaufmann–Mehlhorn–Rülling (SoCG 1988, 392–402 [V]; one version also lists Storb as an author) give O(n³ log n) using sweeps in the universal cover.
- **Cleanest exact certificate for single-layer homotopic routing.** Feasible if and only if, for every segment between two obstacle features, capacity ≥ Σ (clearance-weighted wire widths) × mincr. Infeasibility is witnessed by one violated segment.
- **This is exactly vcad's per-region × layer channel resource with the right definition of "region".** The critical cut segments are the resources that should carry prices λ.
- **Multi-layer with vias.** I know of no exact theorem. See Conjecture C1.

**4. Shortest homotopic realization (the funnel).**
- **Hershberger–Snoeyink (CGTA 4(2) (1994) 63–97 [V]).** Funnel in the universal cover, linear in the sleeve size. It also covers fixed-orientation metrics, which gives octilinear (45°) for free.
- **Efrat–Kobourov–Lubiw (CGTA 35(3) (2006) 162–172 [V]).** Homotopic shortest paths for a set of disjoint input paths among point obstacles, in O(k_out + k_in log n + n√n).
- **Bespamyatnikh (J. Algorithms 49(2) (2003) 284–303 [V]).** O(n log^{1+ε} n + k_in log n + k_out) for simple paths.
- **Thick paths (Polishchuk–Mitchell, "Thick non-crossing paths and minimum-cost flows in polygonal domains", SoCG 2007 [U]).** Thick non-crossing paths reduce to a min-cost flow in a "corridor" sense: inflate obstacles by the width and run a funnel per thread. The coupling between threads is again a cut condition.
- **Where it plugs in.** The geometry stage is Hershberger–Snoeyink in the octilinear metric on clearance-inflated obstacles. Its output is a canonical starting point for SI descent.

**5. Ordered escape and layer assignment.**
- **Ozdal & Wong.** "Algorithms for simultaneous escape routing and layer assignment of dense PCBs", IEEE TCAD 25(8) (2006) 1510–1522; ICCAD 2004 version 822–829 [V; author list from memory].
  - Modeled as a longest path with forbidden pairs.
  - Exact polynomial algorithm for the maximal planar routing on one layer, plus a randomized algorithm for scale.
- **Ozdal & Wong, "Algorithmic study of single-layer bus routing for high-speed boards", TCAD 25(3) 2006 [V].** O(A) / O(A log A) optimal length matching.
- **Unordered single-layer escape.** It is a unit-capacity max flow (node splitting for vias and pin capacity), so it is totally unimodular and integral [M; this is standard].
- **Ordered escape.** Luo–Wong (ASP-DAC 2009 [V]) say SAT was the only known method. I found no clean NP-hardness proof for ordered BGA escape.
- **Rectangle Escape Problem.** NP-complete (HKBU paper [V; authors unconfirmed]).
- **Practical consequence.** Integrality is lost the moment ordering is imposed. Keep ordering as a homotopy word (the plan already does this) and use flow only for unordered capacity checks.

**6. Return currents and planes: cohomology.**
- **Gross–Kotiuga, *Electromagnetic Theory and Computation: A Topological Approach*, Cambridge UP / MSRI Publ. 48 (2004) [M].** It gives algorithms for cohomology generators ("cuts") in eddy-current and low-frequency EM.
- **Direct use (a computable test for slots under traces).** A trace's return path is the plane-layer 1-chain dual to it.
  - Compute H₁ of (plane copper minus antipads and slots) relative to the via stitching.
  - A trace crossing a slot is detected when the loop formed by the trace plus its ideal return image has a nonzero pairing with an H¹ generator of the plane.
  - In practice: union-find on the plane mesh, and check whether the trace's projection crosses a boundary component that separates its endpoints' reference vias.
- **Plane resonance.** On the CM5's PWR/PWR pair, plane resonance is the cavity Laplacian. Use DEC (Hodge star on the plane mesh) with eigenvalues ∝ (mπ/a)² + (nπ/b)². This is classical and needs no new math.

**7. Differentiable combinatorial solvers.**
- **Vlastelica et al., "Differentiation of blackbox combinatorial solvers", ICLR 2020, arXiv:1912.02175 [M].** Gradient = −(y(w + λ∇L) − y(w))/λ. It needs only one extra call to the solver, so the existing solvers can be reused as-is.
- **Berthet et al., "Learning with differentiable perturbed optimizers", NeurIPS 2020, arXiv:2002.08676 [M].** Monte Carlo over perturbed solver calls. Unbiased for the smoothed objective, but noisy.
- **Implicit differentiation of OT.** Differentiate through the Sinkhorn fixed point. This is exact and cheap, and fits a symbolic graph naturally.
- **Fit for tang-expr.**
  - Sinkhorn: use implicit differentiation.
  - Market solvers: they don't need gradients. The augmented-Lagrangian duals already *are* the sensitivities, by the envelope theorem ∂OPT/∂b = −λ.
  - Use Vlastelica only for homotopy-word changes, where no dual exists.

**8. PathFinder as a congestion game.**
- **McMurchie–Ebeling, "PathFinder: a negotiation-based performance-driven router for FPGAs", FPGA 1995 [M].**
- **Interpretation.** History cost is subgradient ascent on λ, and present-congestion cost is an augmented-Lagrangian penalty.
- **Roughgarden–Tardos, "How bad is selfish routing?", JACM 49(2) 2002 [M].** For linear latencies the price of anarchy is 4/3. That bound is non-atomic; each atomic, unsplittable net gives about 5/2 (Christodoulou–Koutsoupias 2005 [M]).
- **Use.** This bounds how far a converged PathFinder equilibrium can be from the social optimum when congestion is priced linearly. It is the right caveat for the receipt when the market is not gross substitutes.

**Less leverage.**
- Tropical geometry: shortest paths are min-plus linear algebra, and λ-parametric shortest paths are tropical polynomials, giving price breakpoints. Useful for sensitivity tables only.
- Persistent homology: track plane connectivity as clearance grows. The death time of a component is the clearance at which that plane island disconnects; this is a cheap DFM robustness score.
- Serpentines and length matching: Ozdal–Wong above is the exact combinatorial result. As SOCP, the trace length is Σ‖p_{i+1} − p_i‖₂ ≥ L_target. That constraint is non-convex (a lower bound on a norm). It becomes convex if the meander topology (number of bumps) is fixed and only the amplitudes a_k are variables, because length is then affine in a_k for rectangular meanders. I found no verified paper on optimal meander shapes or elastica.

---

### (b) Strongest route to making "one market" exact

**Theorem to rely on.** Combine Fujishige–Yang (gross substitutes ⇔ M♮-concave), Kelso–Crawford / Gul–Stacchetti (equilibrium exists and the ascending auction finds the minimal price vector), and Murota (network-induced valuations are M♮-concave).

**What would have to hold on a PCB.**
1. **Unit-demand-per-resource valuations induced by a flow.** Each net's value for a set S of via sites or plane-access slots must be a max-weight matching or flow from its needs into S.
   - True for via-site, plane-access and fan-out escape.
   - Why: a net needs "any k of these sites", and sites are interchangeable up to cost.
2. **No complementarity.** It fails for global routing channels: a path needs *all* tiles along it, which makes the valuation complementary rather than gross substitutes. In general, then, no Walrasian prices exist.
   - Known fix: Bikhchandani–Mamer. An equilibrium exists exactly when the bundle LP is integral, which holds for series-parallel or tree channel graphs.
   - So channels are exact only on trees: hierarchical, river-like regions such as BGA breakout corridors.
3. **Linear money.** Quasi-linear utility, with $ as numéraire. Skew budget is a separable concave term, which preserves M♮-concavity.
4. **Lane swaps.** Assignment valuations are gross substitutes, so S_4 and S_8 swaps with linear cost are exact.

**Architecture conclusion.** Split the market into two parts.
- **Exact sub-market** (vias, plane access, escape, lane assignment): gross substitutes, so prices clear and the duality gap is 0. Certify by checking the dual feasibility of the network LP.
- **Lagrangian sub-market** (channels): positive gap, bounded by Theorem 2, with Roughgarden–Tardos as a quality caveat.
- One price vector λ still couples the two. The receipt reports the gap as gap_exact (0) + gap_channel.

---

### (c) Conjectures and new formulations

**C1. Multi-layer homotopic cut condition with via generators.**
- **Model.** A stack of L punctured planes with via sites as edges between layers forms a 2-complex X. Its π₁ is free, so homotopy words are reduced words in obstacle loops plus via moves.
- **Conjecture.** If every connection fixes its layer-change word (which vias, in what order), feasibility holds if and only if every layer separately passes the Leiserson–Maley / Schrijver cut condition, with via-site capacity treated as an extra cut.
- **Proof sketch.** With the word fixed, every via is pinned. Each layer becomes an independent single-layer homotopic instance, with the via pads acting as additional obstacles and terminals, and Schrijver's theorem then applies to each layer.
- **Key obstacle.** If the word is *not* fixed, choosing vias is NP-hard. Even two-layer via minimization is hard [M; classical].
- **Consequence.** The exact certificate is "word fixed ⇒ polynomial check", and the market searches over words.

**C2. The Sinkhorn c-transform bound is tight in the ε→0 limit at rate O(ε log n).**
- Repairing entropic potentials via the c-transform gives L_assign ≥ OPT − ε·log(n!)-type slack.
- Proof route: standard entropic OT error bounds, plus the c-transform being 1-Lipschitz.
- This would give the receipt an explicit ε-to-gap formula.

**C3. BGA via-site and escape markets are gross substitutes.** This is publishable as a short note.
- **Claim.** Model escape as unit-capacity flow, with net n's value v_n(S) = max-flow-cost into site set S. Then v_n is M♮-concave, and the ascending via-site auction terminates at the minimal Walrasian prices, which equal the min-cost-flow node potentials.
- **Proof.** Network induction preserves M♮-concavity (Murota). Then Gul–Stacchetti applies.
- **What would be new.** The PCB interpretation, and showing that PathFinder history costs restricted to this layer converge to exact equilibrium prices.

**C4. Channel duality gap ≤ PoA-type bound.**
- **Conjecture.** For PathFinder with present-cost multiplier p→∞ and linear history pricing on tree-like regions, the converged plan's cost is at most (1 + O(1/min capacity)) × LP.
- **Reasoning.** This is randomized-rounding / Raghavan–Thompson style, giving a concrete prediction to compare against the receipt.
- **Obstacle.** Non-tree channel graphs, where an integrality gap exists (Leighton–Rao style).

**C5. Detecting return-path slots is an H¹ pairing.**
- **Formulation.** For trace γ on layer i and reference plane j: the return is undisturbed iff [γ̂] = 0 in H₁(plane_j, via-stitch set). Here γ̂ is γ projected onto the plane and closed through its end reference vias.
- **Proof.** Immediate from the Gross–Kotiuga framework. A nonzero class means the return current must detour around a hole.
- **Result.** An exact, fail-closed oracle predicate, and the detour length gives a price.

---

### (d) Bibliography
[V] = verified by web search this session. [M] = from memory, confident. [U] = not confirmed.

- [V] Schrijver, A. "Disjoint homotopic paths and trees in a planar graph." Discrete Comput. Geom. 6(6):527–574, 1991. https://ir.cwi.nl/pub/2352
- [V] Schrijver, A. "Edge-disjoint homotopic paths in straight-line planar graphs." SIAM J. Discrete Math., 1991. (found via the CWI repository, https://ir.cwi.nl)
- [V] Leiserson, C.E., Maley, F.M. "Algorithms for routing and testing routability of planar VLSI layouts." STOC 1985, 69–78. doi:10.1145/22145.22153
- [U] Cole, R., Siegel, A. "River routing every which way, but loose." FOCS 1984.
- [V] Gao, S., Jerrum, M., Kaufmann, M., Mehlhorn, K., Rülling, W. (one version also lists Storb). "On continuous homotopic one layer routing." SoCG 1988, 392–402. https://people.mpi-inf.mpg.de/~mehlhorn/ftp/HomotopicOneLayerRouting.pdf
- [M] Okamura, H., Seymour, P.D. "Multicommodity flows in planar graphs." JCTB 31:75–81, 1981.
- [V] Hershberger, J., Snoeyink, J. "Computing minimum length paths of a given homotopy class." CGTA 4(2):63–97, 1994. https://www.uni-trier.de/fileadmin/fb4/prof/INF/DEA/Seminar0708/Hershberger-Snoeyink3.pdf
- [V] Efrat, A., Kobourov, S., Lubiw, A. "Computing homotopic shortest paths efficiently." CGTA 35(3):162–172, 2006. https://arxiv.org/abs/cs/0204050
- [V] Bespamyatnikh, S. "Computing homotopic shortest paths in the plane." J. Algorithms 49(2):284–303, 2003. https://www.utdallas.edu/~besp/abs/homo.htm
- [U] Polishchuk, V., Mitchell, J.S.B. "Thick non-crossing paths and minimum-cost flows in polygonal domains." SoCG 2007.
- [V] "Algorithms for simultaneous escape routing and layer assignment of dense PCBs." IEEE TCAD 25(8):1510–1522, 2006 (authors Ozdal & Wong, from memory). https://scholars.hkbu.edu.hk/en/publications/algorithms-for-simultaneous-escape-routing-and-layer-assignment-o/
- [V] Ozdal, M.M., Wong, M.D.F. "Algorithmic study of single-layer bus routing for high-speed boards." IEEE TCAD 25(3), 2006. https://scholars.hkbu.edu.hk/en/publications/algorithmic-study-of-single-layer-bus-routing-for-high-speed-boar/
- [V] Luo, L., Wong, M.D.F. Ordered escape routing (ASP-DAC 2009, SAT-based). https://experts.illinois.edu/en/publications/ordered-escape-routing-based-on-boolean-satisfiability/
- [V] Fujishige, S., Yang, Z. "A note on Kelso and Crawford's gross substitutes condition." Math. Oper. Res. 28(3):463–469, 2003. doi:10.1287/moor.28.3.463.16393
- [M] Kelso, A.S., Crawford, V.P. "Job matching, coalition formation, and gross substitutes." Econometrica 50(6):1483–1504, 1982.
- [M] Gul, F., Stacchetti, E. "Walrasian equilibrium with gross substitutes." J. Econ. Theory 87:95–124, 1999.
- [M] Bikhchandani, S., Mamer, J. "Competitive equilibrium in an exchange economy with indivisibilities." J. Econ. Theory 74:385–413, 1997.
- [M] Murota, K. *Discrete Convex Analysis.* SIAM, 2003.
- [V] Murota, K. "Discrete convex analysis: a tool for economics and game theory." J. Mechanism and Institution Design 1(1), 2016. https://mechanism-design.org/arch/v001-1/v001-1-5.html
- [V] Shioura, A., Tamura, A. "Gross substitutes condition and discrete concavity for multi-unit valuations: a survey." J. ORSJ 58(1):61, 2015. https://www.jstage.jst.go.jp/article/jorsj/58/1/58_61/_article
- [M] Geoffrion, A.M. "Lagrangean relaxation for integer programming." Math. Programming Study 2:82–114, 1974.
- [M] Vlastelica, M., Paulus, A., Musil, V., Martius, G., Rolínek, M. "Differentiation of blackbox combinatorial solvers." ICLR 2020. arXiv:1912.02175
- [M] Berthet, Q., Blondel, M., Teboul, O., Cuturi, M., Vert, J.-P., Bach, F. "Learning with differentiable perturbed optimizers." NeurIPS 2020. arXiv:2002.08676
- [M] McMurchie, L., Ebeling, C. "PathFinder: a negotiation-based performance-driven router for FPGAs." FPGA 1995.
- [M] Roughgarden, T., Tardos, É. "How bad is selfish routing?" JACM 49(2):236–259, 2002.
- [M] Christodoulou, G., Koutsoupias, E. "The price of anarchy of finite congestion games." STOC 2005.
- [M] Gross, P.W., Kotiuga, P.R. *Electromagnetic Theory and Computation: A Topological Approach.* MSRI Publ. 48, Cambridge UP, 2004.

**Before anything is published:** check the [U] items (Cole–Siegel, Polishchuk–Mitchell) and the Ozdal–Wong author list for the TCAD 25(8) paper on IEEE Xplore or DBLP.