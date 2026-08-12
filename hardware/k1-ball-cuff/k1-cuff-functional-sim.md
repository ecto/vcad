# K1 ball cuff — functional verification (quasi-static, mesh-grounded)

Method: the printable STLs (exported by vcad from k1-ball-cuff.loon), trimesh
signed-distance settle sweeps, and direct force balance. Scripts: settle_mesh.py /
loads.py alongside; rerunnable against any revision.

Note: vcad's STL export still emits some T-junction edges (61 upper / 16 lower,
tracked as defect C in vcad-boolean-bug-handoff.md). Volumes are verified correct
(+0.5%/+0.6% vs Monte-Carlo, pure tessellation). Slicers handle T-junctions;
some renderers show them as spurious backfaces. Re-export once defect C lands.

## Seating (the robot-facing half of "attach")

Sphere-drop settle against the actual lower-half mesh: every start inside the
V mouth — ±34 mm across the groove — converges to the exact seat (x=0,
z=+0.42 mm) with zero snags from bolt slots or the neck relief. Along the
groove the cradle is deliberately prismatic; Y is set by pressing the rod
against the journal end, then the upper half's crossed groove locks it.
Yaw tolerance before the rod fouls the block edge: ~41°.

±34 mm / ±41° is an enormous funnel by robot standards — the arm can be
centimeters off and gravity finishes the job.

## Attach/detach as designed — honest status

Rev C is a bolted clamshell: DETACH IS A HUMAN WITH A DRIVER, not a robot
action. The robot's part is only: lay the ball into the open cradle (verified
above), hold still while a human closes six M4s, and later pull straight out
once opened. Robot-self-service don/doff needs the over-center latch variant
in place of the bolt rows — same socket geometry, latch replaces fasteners.

## Load capacity (does it shovel?)

Preload path: 6× M4 at 0.8 N·m → ~6 kN clamp, all of it through the ball
(the 0.42 mm designed gap guarantees this) → ~4.2 kN per V contact, which
flattens to a ~7 mm-radius patch at PETG's long-term 30 MPa. Fine.

- **Caged directions** (all translations, both bending moments): geometric.
  Prying at 27.5 N·m (4 kg on a 0.7 m arm) puts 0.86 MPa on the journal —
  30x margin. A 200 N yank adds 33 N per bolt against ~4 kN proof. Not the
  limit in any realistic dig.
- **Rod-axis torque is friction-only** — sphere and cylinder are both
  surfaces of revolution about the forearm axis, so nothing geometric resists
  rotation about it. Capacity ~75 N·m at μ=0.25 with fresh preload. That is
  11x the elbow-yaw actuator's ~7 N·m, so the actuator stalls before the cuff
  slips — and a slip under rock-strike overload acts as a mechanical fuse.
  The real risk is PETG creep quietly eating the preload: retorque after the
  first day, and treat audible slip as the retorque alarm. If it ever needs
  to be geometric, that's a keyway rev — but there is no flat on the forearm
  to key against, so it would mean bonding a collar to the rod.

## What quasi-statics cannot answer

Impact/vibration (blade striking rock), preload decay rate, and the actual
digging cycle under the balance policy. Next step there: convex-decompose the
halves (coacd), weld into K1_22dof.xml as a MuJoCo body or into phyz via the
URDF, and replay a dig trajectory. The mass properties to use: assembled cuff
137 g, COM (0, −10.4, +7.5) mm from ball center (at 4-wall/40%-infill effective density).


## Correction, 2026-08-12 — the lower journal did not exist

Found while bisecting a T-junction count: the rev-C lower V-groove ran the full
block length in Y, and at x = 0 that trough is void past the block's bottom
face — so it deleted every scrap of material under the forearm rod. Probing at
the journal station y = -33 found air below, beside and above the rod in the
lower half. The journal was upper-shell-only: the ball-seat/journal couple
carried moment in one direction and nothing in the other, which is most of the
reason the cuff exists.

Fixed by stopping the lower trough at y = -19 — exactly where the robot stops
being a ball and starts being a rod (ball radius there is 16.25 mm, already
inside the 17.2 mm bore, so the bore takes over clearance duty). Groove and
bore now hand off at the same plane. Re-probed: lower half SOLID below and
beside the rod, upper half SOLID above it — a full journal. Interference back
to 26.8 mm³, i.e. the four designed preload lenses and nothing else.

Why the earlier verification missed it: the Monte-Carlo model was written from
the same CSG as the loon source, so it proved the mesh matched the code — never
that the code expressed the design. Geometric-intent probes (is there material
where the journal is supposed to be?) are a different check from volume
agreement, and only the probe could have caught this.

Mass rose 127 g -> 137 g; COM (0, -10.4, +7.5) mm from the ball center.
