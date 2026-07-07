# vcad domain atlas — now and future

*Companion to [the convergence strategy](2026-07-06-convergence-strategy.md).
Every domain, scored against the admission gate. 2026-07-06.*

## How to read this

**Domains vs. layers.** The Receipt, the gradients, the commerce rail, the
eval, and the DAG are *layers* — they are not domains and never compete with
one another. A domain is a kind of physical thing vcad can design. Layers
compound; domains plug in.

**The gate.** A domain is admitted when it plugs into all five layers:

1. **Representable** — lives in the parametric DAG / IR
2. **Verifiable** — has oracles that can sign receipt claims
3. **Differentiable** — parameters priced through the seam
4. **Purchasable** — a rail exists to make it real (vendor API, shuttle run,
   or the user's own machine)
5. **Evaluable** — mecheval-style tasks with a self-grading oracle

Scores below: ✓ shipped/clearly reachable, ~ partial or manual, ✗ missing.

---

## Scorecard

| domain | repr | verify | diff | purchase | eval | verdict |
|---|---|---|---|---|---|---|
| **Now** |
| mechanical BRep | ✓ | ✓ | ✓ | ~ | ✓ | the substrate |
| sheet metal | ✓ | ✓ | ✓ | ✓ SCS | ~ | first public loop |
| electronics / PCB | ✓ | ✓ | ✓ | ~ (API denied) | ~ | deepest oracle stack |
| 3D printing | ✓ | ~ | ✓ | ✓ (own Bambu) | ~ | private-loop volume |
| CNC / CAM | ~ | ~ | ~ | ~ | ✗ | stocksim needs a consumer |
| robotics / physics | ✓ | ✓ | ✓ | ~ | ~ | the wave-2 demo |
| materials / atoms | ✓ | ✓ | ✓ | ✗ | ~ | longest horizon, real gradients |
| wafer process + GDSII | ✓ | ~ | ~ | ~ | ✗ | substrate for photonics/MEMS/silicon |
| electromagnetics | ~ | ~ | ✓ | ✓ (rides PCB) | ✗ | half-shipped, unnamed |
| embroidery | ✓ | ~ | ✗ | ~ | ✗ | 2.5/5 — park it |
| **Future** |
| photonics (PIC) | ✓ | ✓ | ✓ | ✓ (ANT via SiEPIC) | ✓ | 5/5 — next admission |
| MEMS | ✓ | ~ | ✓ | ✓ (MUMPs shuttles) | ✓ | photonics' sibling |
| silicon / ASIC | ✓ | ✓ | ~ | ✓ (TinyTapeout/IHP) | ✓ | most legendary rail |
| free-space optics | ~ | ✓ | ✓ | ~ (COTS + mounts) | ✓ | enters via optomech |
| cable harness | ✓ | ✓ | ✓ | ~ | ✓ | the missing mechatronic glue |
| injection molding | ✓ | ✓ | ✓ | ✓ (Protolabs-class) | ✓ | wave-3, capital-heavy |
| textiles / soft goods | ~ | ~ | ~ | ~ | ✗ | embroidery's redemption arc |

---

## Now

### Mechanical BRep (the substrate)
Kernel, booleans, fillets, sketches/constraints, sweep/loft/shell, STEP,
drafting/GD&T. Oracles: mass properties, mecheval grading, DFM. Gradients:
the differentiable seam, complete M0–M11. Commerce: no direct CNC rail yet
(quotes exist, ordering is manual). **Gap:** kernel robustness debt *is*
receipt debt — boolean edge cases are now trust bugs, not just geometry bugs.

### Sheet metal
Flanges/bends/unfold, SCS shop profiles, cost model, nesting, sequencing,
DXF + folded STEP. The chosen first public loop
([demo spec](2026-07-06-scs-closed-loop-demo.md)). **Gap:** a sheet-metal
mecheval task family.

### Electronics / PCB
The deepest oracle stack in the repo: DRC/ERC (fail-closed), DFM fab packs,
impedance/SI/thermal/PDN, autorouter with negotiated congestion, Gerber +
native KiCad export, the Receipt already exists here. **Purchase rail
regression:** JLCPCB denied the API application. Options: PCBWay's API,
Aisler, Seeed Fusion — or keep PCB ordering human-in-the-loop and let sheet
metal carry the public loop. **Gap:** pick the replacement rail; pcbeval.

### 3D printing
Slicer + Bambu G-code. The only domain where the loop closes with **zero
vendor dependency** — the user's printer is the effector. Cheapest physical
ground truth per datum; this is the *volume* calibration loop while SCS is
the *public* one. **Gap:** print-then-measure receipt flow; dimensional
accuracy oracles per printer profile.

### CNC / CAM
Toolpaths exist, stocksim exists (octree SDF, marching cubes) with **no
consumer** — the canonical gate violation from before the gate existed.
Admit properly or leave parked: wire stocksim as the toolpath verification
oracle (receipt claim: "stock minus toolpath ⊆ target + tolerance"), then a
rail (Xometry-class API or SCS CNC).

### Robotics / physics
phyz, gym tools, GPU batch envs, URDF, cloud training dirs, contact-dynamics
gradients. Verification is unusual: the receipt claim is *behavioral* ("this
policy achieves the grasp in sim"). Purchase: parts are COTS + the other
domains' rails — a robot is a cross-domain assembly, which is the point.
**This is the wave-2 signature demo** (gripper: trained before its body
existed).

### Materials / atoms
MD, ML potentials, homogenize → MaterialCard → continuum, cross-scale
gradients proven. Purchase rail: none — you cannot order a lattice constant.
Nearest real rail: **printed lattices/metamaterials** (homogenize a printed
unit cell instead of a crystal, then the 3DP rail makes it). That converts
this domain from "research demo" to "orderable material."

### Wafer process + GDSII
`vcad-process` (deposit/etch/grow/implant/litho) + `vcad-gdsii`
(read/write/flatten/IR bridge). Not a user-facing domain — the **shared
substrate** for photonics, MEMS, and silicon. Its oracles (cross-sections,
film stacks) become receipt claims in all three.

### Electromagnetics (half-shipped, unnamed)
Already in the tree without a name: coils, motor windings, winding-factor
math, impedance, RF calculators, diff-pair routing. Naming it a domain turns
scattered tools into a receipt family (inductance, torque constant, Z₀).
Future: antennas, filter synthesis, eventually a field solver. Rides the PCB
rail — coils are copper.

### Embroidery
Three crates, machine formats (.dst/.pes). Scores 2.5/5: no gradients, no
eval, no scaled rail (home machines only). **Park it** — it re-enters later
as the output device of the textiles domain, not as its own domain.

---

## Future (in admission order)

### Photonics — next
Full case in the convergence discussion: `vcad-pic-pdk` / components /
layout / sim / verify, S-matrix spectra through tang-expr, GDS via
`vcad-gdsii`, 3D truth via `vcad-process`. Purchase rail is real: SiEPIC
EBeam PDK → Applied Nanotools fab runs. Eval: "design a ring filter at
1550 nm" self-grades from the simulated spectrum. 5/5.

### MEMS
Same substrate as photonics + one recipe step (release etch) + phyz for
resonator/accelerometer behavior (needs modal analysis — a small eigenproblem,
tang-la, same machinery as the glockenspiel stretch goal). Purchase rail:
MEMSCAP MUMPs-class multi-user shuttles (manual ordering, real). Eval:
"resonator at 32.768 kHz" — self-grading, brutal, wonderful.

### Silicon / ASIC
GDSII ✓, open-PDK DRC decks are importable rules, and the rail is the most
legendary one available: **TinyTapeout on IHP shuttles, ~hundreds of dollars,
agent-budget-sized**. "An agent taped out working silicon and the receipt
predicted its behavior" is a milestone no one else is positioned to claim.
Slow cadence (months per shuttle) — start one early so it's in flight while
faster loops iterate.

### Free-space optics + optomech
Enters through the **parts engine**, not the physics: agents design mounts,
cage plates, and baffles (SCS + 3DP rails, shipped) around COTS optics from a
catalog. Then `vcad-kernel-raytrace` grows refraction + conic/asphere
surfaces and the receipt gains spot size / focal length claims. Lens design
was historically *the* gradient-optimization field — the seam will feel at
home.

### Cable harness
The missing glue in every mechatronic assembly and the n² coupling story
made literal: connects PCB (pinouts), enclosure (routing space), and
assembly (lengths through the kinematic envelope). Oracles: continuity,
gauge-vs-current (`size_trace_for_current` generalizes), bend radius, length
through the moving mechanism. Rail: custom-harness vendors are semi-manual
today — start with the receipt, let the rail mature.

### Injection molding
DFM pack pattern fits perfectly (draft angles, wall thickness, undercuts,
sink); Protolabs-class APIs exist; wall thickness is a differentiable
parameter. Wave 3 because tooling cost raises the stakes on receipt
correctness — a wrong receipt on a $79 SCS order is a lesson; on a $8k mold
it's a lawsuit.

### Textiles / soft goods
Sewing patterns are flat-pattern unfold on developable surfaces — shared math
with `vcad-kernel-sheet`. Embroidery becomes the decoration pass. Speculative;
gate honestly before admitting.

---

## Waves

- **Wave 1 (now):** unify the Receipt; SCS public loop
  (glockenspiel); 3DP private calibration loop; name the EM domain; wire
  stocksim or park it.
- **Wave 2:** photonics admission (full crate family); gripper demo
  (robotics × sheet metal × training); harness receipts; pcbeval +
  sheet-metal eval families; pick the PCB rail replacement.
- **Wave 3:** MEMS; silicon tapeout (start the shuttle clock early);
  optomech wedge into free-space optics; injection molding.
- **Declined (examples to keep the gate honest):** architecture/AEC (rail
  and eval both fail), garment fashion at scale (no oracle for "fits"),
  food/bio (no substrate). Fun is not a gate criterion.

## The rule, restated

New domains enter through the gate, in whatever order the rails and demos
make them ready — but **no domain enters ahead of the loop**. If a quarter's
work strengthens a domain but not the Receipt, the flywheel, or a rail, it's
the wrong quarter.
