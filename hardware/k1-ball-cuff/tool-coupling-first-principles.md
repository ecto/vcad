# K1 tool coupling — first-principles reconsideration

Written after rev C printed and didn't seat cleanly. This revisits the premises
rather than the dimensions, because the dimensions turn out to be right.

## What is not wrong

The CAD geometry is exact, so nothing here is a measurement error:

- hand ball: best-fit centre y = 203.000 mm, R = 25.000, **sphericity spread
  0.002 mm** over 1621 vertices. A true Ø50.000 sphere.
- forearm rod: Ø34.00, concentric with the ball axis to 0.06 mm.

Both halves also verified against an independent volume model (+0.5%/+0.6%) and
the seating funnel is ±34 mm. The part that got printed is the part that was
designed. So the problem is in the design premises.

## The premise that was wrong

**I optimised for strength. Strength was never the binding constraint.**

Every arm joint on the K1 is limited to **14 N·m** (URDF `effort`, all four:
shoulder pitch, shoulder roll, elbow pitch, elbow yaw). With the ball 369 mm
from shoulder roll, that is:

| quantity | value |
|---|---|
| static payload at the hand | **3.9 kg** |
| straight pull the arm can exert at the hand | **38 N** |
| largest moment the arm can apply about any axis | **14 N·m** |

Against that, rev C at its *specified* 0.8 N·m bolt torque delivers 6 kN of
clamp and 75 N·m of rod-axis friction capacity — **5× more than the arm can
ever demand**, and the caged directions are 30× over. I built a coupling for a
machine an order of magnitude stronger than the one we own.

Working backwards from 14 N·m instead: friction capacity needs
`N = M/(4 μ r) = 14/(4·0.25·0.01768)` = 792 N per contact, ≈ 1.1 kN of clamp,
which is **0.15 N·m per bolt** — a fifth of spec, comfortably inside PLA. And a
retention feature only has to beat a 38 N pull, so ~100 N of snap holds the tool
against anything the robot can do to it.

So the real objective function was never strength. It is, in order:

1. **mass at the hand** — 161 g is 4% of a 3.9 kg payload budget, spent at the
   worst possible moment arm
2. **tolerance robustness** — it has to seat on a printed part every time
3. **tool-change** — the original goal was tool-*using* behaviour

Rev C scores badly on all three and superbly on the one that didn't matter.

## The fit failure is over-constraint, and it was predictable

Count constraints on the tool relative to the hand:

- V-seat, 4 point contacts on the sphere → locates 3 translations (1 redundant,
  benign, standard for a clamped V-block)
- neck journal, a Ø34.4 bore on a Ø34.0 rod → locates 2 more translations *and*
  2 rotations

That is **8 constraints for 5 DOF** (the 6th, spin about the rod axis, is
deliberately free). Three are redundant. With perfect parts they agree; with a
printed part the V-seat says "ball centre here" and the journal says "rod axis
there," they disagree by the print error, and the assembly rocks or refuses to
close. Kinematic design rule I violated: **constrain each DOF exactly once.**

Compounding it, the journal's 0.2 mm diametral clearance is below what FDM
holds — printed holes typically come out 0.1–0.4 mm *under* nominal from flow
and elephant's foot. The journal is very likely interfering before the V-seat
ever touches, which would present exactly as "close, but doesn't sit right."

## The constraint that cannot be designed away

The ball and the rod are both **surfaces of revolution about the forearm axis**.
No amount of cleverness extracts torque about that axis from them — only
friction, which needs clamp force, which needs bolts and mass. This is a
property of the robot, not of the cuff.

Three ways out, and they are genuinely different products:

**A. Pay for friction (rev C's answer, now correctly sized).** 1.1 kN of clamp
covers the arm's full 14 N·m. Bolted, human-serviced. Right when the tool must
resist twist about the forearm — a shovel held crosswise.

**B. Move the interface to the forearm.** `Left_Arm_3` is measurably non-round —
6 to 18 mm of shape off-round across its length. That is a real key: full torque
transmission with no friction, no preload, no creep. The cost is that
`Left_Arm_3` sits *before* elbow yaw, so the tool loses the one rotational DOF
the arm has, and the hand ball becomes a protrusion to design around. Right for
tools that don't need blade rotation.

**C. Don't need the torque.** Two hands on one bar leaves exactly one free DOF
(spin about the line joining the two ball centres), killed by a third passive
bearing point against a forearm or the torso — no clamping anywhere. Also: any
tool whose load line passes near the forearm axis (a poker, a hook, a coaxial
gripper) never generates the troublesome moment at all.

## What I would build now

**A light single-locator cuff.** Keep the V-seat as the *only* locator. Open the
journal to ~1 mm diametral clearance and demote it from bearing to **bump
stop** — it carries nothing until ~1.5° of rotation, then takes the moment. That
removes all three redundant constraints and the un-printable fit in one change,
and 1.5° of blade slop is irrelevant for digging.

Then re-size everything for 0.15 N·m bolts instead of 0.8: fewer bolts, thinner
sections, and the block can lose most of its 161 g.

Separately, and more interesting for the actual goal: **the docking question is
the real product.** Because the arm can only pull 38 N, a snap that holds 100 N
can never be removed by the robot — but *can* be released by pressing the tool
into a fixed dock, which is a robot-executable motion. That is the difference
between a prosthetic and a tool-using hand, and it costs nothing in strength
because there is no strength to spend.

## Measurements needed before cutting metal (or plastic) again

The CAD is exact but the robot may not be:

1. **Caliper the actual ball** in two or three axes. If it is moulded or
   rubber-skinned it may not be Ø50.000, and a compliant skin changes the seat
   entirely.
2. **Caliper the rod** near y = -33 from the ball centre.
3. **Caliper the printed journal bore** and the V-seat opening on the part in
   hand — that separates "design over-constraint" from "printer tolerance."
4. With the lower half alone, check whether the ball rocks in the V before the
   journal is involved. If it seats cleanly alone and fights once both halves
   close, over-constraint is confirmed.
