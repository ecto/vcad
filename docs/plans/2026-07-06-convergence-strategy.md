# vcad — the loop is the product

*Convergence audit and highest-leverage strategy. 2026-07-06.*

## What the tree already says

Nobody wrote this roadmap down, but the crate list did. Seven convergences,
each with receipts in the repo:

1. **MEMS / semiconductor devices** — `vcad-gdsii` + `vcad-process` (implant is
   already a recipe step; that's TCAD, not photonics) + phyz + the
   differentiable seam. Photonics and MEMS are siblings of the same layer-stack
   substrate.
2. **A robot factory, not a robot CAD** — `vcad-sim` (GPU batch), `batch_step`
   MCP tools, `packages/training` with `modal/` and `lambda/` cloud-GPU dirs,
   M11 gradients through contact dynamics. Design → train → deploy, morphology
   and policy co-optimized.
3. **Training a CAD model** — mecheval + leaderboard (frontier labs already
   show up on it), `vcad-eval` as a canonical evaluator (an RL reward
   function), a multimodal training-data pipeline with a self-grading oracle.
4. **Designed materials** — atoms → homogenize → continuum → rollout, with
   dJ/d(lattice constant) proven against finite differences. "Solve for the
   material" joins "solve for the geometry."
5. **Every machine that makes things gets a backend** — FDM slicer, CAM +
   stock sim, sheet metal, wafers, three embroidery crates. One document, any
   machine.
6. **Agents that buy atoms** — quote/order/wallet, a shared cost model, DFM
   packs per fab, `validate_for_fab` gates, an ACP-CM protocol draft.
7. **vcad everywhere, agents as peers** — CRDT concurrent editing, the AI
   contract in Rust, termview, spatial, wasmosis.

The temptation is to rank these and pick a vertical. That's the wrong axis.

## The leverage analysis

Sort the assets by what *compounds* versus what merely *adds*:

**Linear:** each new domain. Photonics, MEMS, embroidery — every one adds
surface area, costs engineering, and alone competes as a niche tool against an
entrenched incumbent (gdsfactory, Coventor, Wilcom). Domains are multipliers
only if something else compounds them.

**Compounding:**

- **The single DAG.** Cross-domain value lives in the couplings, not the
  domains — n domains, n² couplings. This is already the stated moat
  ([cross-domain co-design](2026-06-23-cross-domain-codesign-vision.md)).
- **Differentiability.** Gradients compose across domains (atoms → continuum →
  rollout is proven). Every domain added makes intent-as-interface stronger.
- **Verification.** Every domain feeds the same trust primitive — the Receipt.
  Trust is what unlocks both commerce and training reward, so its value grows
  superlinearly with domain count.
- **The commerce rail.** Protocols compound by network effect. Whoever defines
  how agents purchase fabrication takes a rate on the agent economy's physical
  output.
- **The data flywheel.** Every verified session, every eval run, every order
  emits (design, spec, outcome) triples. Data moats outlive feature moats.

These five stack into one loop:

> **intent → design → proof → purchase → physical outcome → data → better
> models → better designs.**

The domains are not the product. **The loop is the product.** The highest
leverage move is always the one that strengthens the loop, never the one that
adds a domain for its own sake.

## The point of view

> A design isn't real because it renders. It's real because it ships with a
> machine-checkable proof, survives a purchase order, and comes back from the
> fab measuring what the proof said it would.

## The highest-leverage bet: proof-carrying design

Proof-carrying code (Necula, 1997) let untrusted code run because the code
carried its own machine-checkable certificate. vcad's version: **a `.vcad`
file travels with a machine-verifiable Receipt** — DRC clean under this fab
pack, mass 41.3 g ± tolerance, resonance at 1550.0 nm, min-wall held, quote
locked, every claim traceable to the oracle that checked it.

This is the answer to the defining problem of the agent era: **slop**.
Generative output becomes engineering exactly when it carries proof. The
Receipt is:

- what makes a fab accept an agent's purchase order,
- what makes a human trust an agent's bracket,
- what makes an eval a reward function,
- the one primitive every current and future domain plugs into.

The pieces exist but are siloed: `build_receipt`/`verify_receipt` (PCB),
`verify_part` (mech), `validate_for_fab`, sheet-metal checks, DFM packs.
The bet is to unify them: **one signed, versioned, fail-closed receipt schema
across every domain**, with the `unverifiable ≠ clean` discipline the ecad
stack already enforces.

## The insight that makes it legendary: the order flow is the instrument

Receipts today prove *simulated* properties. The legendary version closes
sim2real — and the commerce rail is secretly the mechanism.

Every agent-placed order returns a physical object that can be measured
against its receipt. That makes each transaction a **paid experiment**: the
customer funds the fabrication, the delivered part is ground truth, and the
receipt-vs-caliper delta is calibration data for the kernel, the process
models, and the trained models. Stripe never got physical feedback from a
payment. vcad's take-rate transactions each return a measurable object.

No lab can buy physical ground truth at this scale. The order flow collects
it as a side effect of revenue. That's the flywheel's last gear, and nobody
else is positioned to build it.

## The three ownership moves (all open)

Legendary infrastructure companies own a benchmark, a protocol, or an
environment. vcad can own all three:

1. **Own the benchmark** (the ImageNet move). mecheval → a multi-domain
   physeval family (mech, pcb, photonics, materials). Frontier labs already
   submit runs. When labs optimize against your benchmark, your representation
   becomes the field's coordinate system.
2. **Own the gym** (the Gym move). The verified environment — kernel + oracles
   + render — is where models learn physical design. Publish it. Every lab
   that trains in your world makes `.vcad` the native tongue of machine
   engineering.
3. **Own the protocol** (the Stripe move). ACP-CM becomes the rail agents use
   to buy atoms, with the Receipt as its trust layer. Take rate on the
   physical output of the agent economy.

## The signature moment

A prompt goes in: *"a mounting bracket for this board, under 50 g, ships this
week."* No human opens a CAD window. The agent designs, the receipt certifies
mass 41.3 g and min-bend legality, the order places against the receipt, the
bracket arrives. On camera, a scale reads **41.3 g**. The caliper matches the
drawing. The measurement posts back into the flywheel as ground truth.

Then the second act: a specialist model, trained in the gym on flywheel data,
tops the public leaderboard — above the frontier labs — at designing parts
that survive this exact test. The loop, made visible.

## Sequencing

1. **Unify the Receipt.** One schema, signed, versioned, fail-closed, across
   mech / pcb / sheet metal. Mostly consolidation of existing oracles; the
   highest ratio of leverage to effort in the repo.
2. **Close one physical loop publicly.** PCB (JLCPCB) or sheet metal
   (SendCutSend) — both have APIs and the order tools exist. Publish the
   receipt next to the caliper photo.
3. **Generalize the eval.** mecheval patterns → per-domain leaderboards, each
   graded by the same oracles that sign receipts.
4. **Ship the flywheel.** Opt-in verified sessions → training data → specialist
   model → beat frontier models on the public benchmark → publish everything.
5. **Admit domains through the gate.** A new domain enters only if it plugs
   into all five compounding layers: representable in the DAG, verifiable,
   differentiable, purchasable, evaluable. Photonics passes all five and
   should be next. Anything that passes fewer than four is a distraction,
   however fun.

## What kills it

- **A wrong receipt.** A signed proof on top of a flaky boolean is worse than
  no proof — it's a false instrument. Receipt integrity must stay fail-closed
  (#343's `unverifiable ≠ clean` is the house rule; extend it everywhere), and
  kernel robustness debt is receipt debt.
- **Scope honesty.** A receipt proves manufacturability and simulated
  properties — not fitness for purpose. Overclaiming is the liability that
  ends the commerce rail. Say exactly what was checked, by which oracle, at
  which version.
- **Breadth without the gate.** Embroidery-style excursions that don't plug
  into the loop burn the scarce resource — kernel-team attention.
- **Losing the environment race.** Frontier labs will build in-house design
  environments. The counter is to be open, be first, and be the benchmark
  they report numbers on. A closed vcad loses this race; an open one referees
  it.

## One line

**vcad is the compiler from intent to matter — and every build ships with its
proof.**
