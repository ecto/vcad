# K1 ball cuff — tool coupling for the Booster K1 humanoid

Real hardware designed in vcad/loon: a tool-side coupling that grips the K1's
Ø50.000 terminal hand ball so the robot can hold a shovel (or any handled tool).

| file | what |
|---|---|
| `k1-ball-cuff.loon` | rev D — two-piece clamshell, exact spherical socket, pillow-block face relief, bump-stop journal |
| `k1-ball-cuff-mono.loon` | rev E — single-piece slide-on pinch collar (the ball is the terminal link, so the cuff enters axially and one M4 across a slit captures it) |
| `tool-coupling-first-principles.md` | why the design looks like this: the 14 N·m arm budget, the over-constraint story, the forearm-axis torque problem |
| `k1-cuff-functional-sim.md` | seating-funnel and load-capacity verification methodology |
| `vcad-boolean-bug-handoff.md` | the kernel bugs this part flushed out, with repros and post-fix evaluations (defects A–C fixed; **defect D — sphere tessellation vertices ±0.6 mm off-surface — still open and blocks printing these**) |

This part is a natural kernel torture test: sphere∕cylinder∕box booleans in
every combination, thin slits through curved bodies, and printed-fit tolerances
(0.05–0.4 mm) that fail loudly when tessellation fidelity slips. Every claim in
the handoff doc is volume-verified against an independent Monte-Carlo model —
never render-verified.

Export: `vcad export k1-ball-cuff-mono.loon mono.stl`
