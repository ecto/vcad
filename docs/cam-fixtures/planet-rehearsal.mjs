#!/usr/bin/env node
// The first milestone — one brass planet — rehearsed end to end, headless.
//
//   node docs/cam-fixtures/planet-rehearsal.mjs [out-dir]
//
// Every number this prints comes out of `vcad cam`; nothing is computed here
// except the arithmetic a machinist would do on paper (where to put the blank,
// how much to leave for the reamer, which tooth space is which). The chain is:
//
//   gear      the geometry, graded against a Ø1 cutter, over pins and span
//   recommend the feeds, for C360 brass on a hobby gantry with a dial router
//   job       the 23 cuts, posted and verified — G-code only if it passes
//   verify    the posted file, replayed again from disk
//   fit       does a Ø1 cutter fit a tooth space?
//   outline   the design intent sectioned back out of a solid
//   compare   that section against the gear's own profile
//
// It fails loudly: any step whose verdict is not the expected one stops the
// run with a non-zero exit. That is the point — a rehearsal that "mostly
// worked" is how the wrong file gets sent to the machine. Four of the steps
// EXPECT a refusal: see docs/planet-milestone.md for what each one found.
//
// The ring and the sun are rehearsed as reports only (no job): the ring is not
// cut from plate with this setup, and the sun's spaces are blind pockets on a
// different blank.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..", "..");
const VCAD = process.env.VCAD_BIN ?? join(REPO, "target", "debug", "vcad");
const OUT = resolve(process.argv[2] ?? join(REPO, "target", "planet-rehearsal"));
mkdirSync(OUT, { recursive: true });

// --- the part, as the drawing states it ------------------------------------
// rana-60-cnc planet: C360 brass, 20T module 1.0, x +0.00, face 5.0 mm, tip
// radius 10.89 (shortened from the nominal 11.00 so the tip clears the ring
// root the same Ø1 cutter leaves). Bore Ø4 H7, reamed.
const CUTTER = 1.0; // mm, 2-flute carbide end mill
const PIN = 1.4; // mm, the pins the shop has
const FACE = 5.0; // mm, = the plate thickness
const TIP_R = 10.89;
const BORE_FINISHED = 4.0;
// 0.2 mm on the diameter left for the reamer: 0.1 per side is the usual
// allowance for a Ø4 machine reamer, and it is the number the job cuts to.
const REAM_ALLOWANCE = 0.2;
const BORE_MILLED = BORE_FINISHED - REAM_ALLOWANCE; // Ø3.8
// Cut 0.2 mm past the underside so the spaces and the profile actually part.
// Anything past the stock needs a declared sacrificial board under it.
const BREAK_THROUGH = 0.2;
const SPOILBOARD = 6.0; // mm MDF
const BLANK = 30.0; // mm square
const CENTRE = BLANK / 2;

const failures = [];
function expect(ok, what) {
  console.log(`   ${ok ? "ok  " : "FAIL"}  ${what}`);
  if (!ok) failures.push(what);
}
function banner(title) {
  console.log(`\n${"=".repeat(72)}\n${title}\n${"=".repeat(72)}`);
}

/** Run `vcad cam <sub>`; returns { code, answer } with the JSON answer. */
function cam(sub, request, { args = [], name = sub, echo = true, allowFailure = false } = {}) {
  const requestPath = join(OUT, `${name}-request.json`);
  const outPath = join(OUT, `${name}-answer.json`);
  if (request !== null) writeFileSync(requestPath, JSON.stringify(request, null, 1));
  const argv = ["cam", sub, ...args, "--out", outPath];
  if (request !== null) argv.push("--request", requestPath);
  let code = 0;
  let stdout = "";
  let stderr = "";
  try {
    stdout = execFileSync(VCAD, argv, {
      encoding: "utf8",
      maxBuffer: 1 << 30,
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (e) {
    code = e.status ?? 1;
    stdout = e.stdout ?? "";
    stderr = e.stderr ?? "";
    if (echo) process.stderr.write(stderr);
    // Exit 1 is "the request could not be run" and there is no answer document
    // to read back, so only a caller expecting that refusal may ask for it.
    if (code !== 2 && !allowFailure) {
      throw new Error(`vcad cam ${sub} could not run (exit ${code}): ${stderr.trim()}`);
    }
  }
  if (echo) process.stdout.write(stdout);
  const answer = code === 1 ? null : JSON.parse(readFileSync(outPath, "utf8"));
  return { code, answer, stderr };
}

const rotate = (loop, radians) =>
  loop.map(([x, y]) => [
    x * Math.cos(radians) - y * Math.sin(radians),
    x * Math.sin(radians) + y * Math.cos(radians),
  ]);
const circle = (r, n) =>
  Array.from({ length: n }, (_, i) => {
    const a = (2 * Math.PI * i) / n;
    return [r * Math.cos(a), r * Math.sin(a)];
  });

// ===========================================================================
banner("1 · gear — the geometry the milestone is measured against");
// ===========================================================================
const gearRequest = JSON.parse(readFileSync(join(HERE, "planet-gear.json"), "utf8"));
const { answer: gear } = cam("gear", gearRequest, { name: "planet-gear" });
const planet = gear.report;
const [sunReport, planetReport, ringReport] = gear.planetary.reports;

expect(
  Math.abs(planet.over_pins.dimension - 21.0738) < 1e-3,
  `over Ø${PIN} pins M = ${planet.over_pins.dimension.toFixed(4)} mm (expected 21.0738)`,
);
expect(
  Math.abs(planet.span.length - 7.6322) < 1e-3,
  `span over ${planet.span.teeth_spanned} teeth W = ${planet.span.length.toFixed(4)} mm (expected 7.6322)`,
);
expect(
  planetReport.reachability.ok && planetReport.flank_deviation_at_contact_limit <= 0.005,
  `planet reachable at Ø${CUTTER}: margin ${planetReport.reachability.margin.toFixed(6)} mm, ` +
    `flank deviation ${planetReport.flank_deviation_at_contact_limit.toFixed(6)} mm`,
);
expect(
  sunReport.reachability.ok,
  `sun reachable at Ø${CUTTER}: margin ${sunReport.reachability.margin.toFixed(6)} mm`,
);
expect(
  ringReport.reachability.ok,
  `ring reachable at Ø${CUTTER}: margin ${ringReport.reachability.margin.toFixed(6)} mm, ` +
    `flank deviation ${ringReport.flank_deviation_at_contact_limit.toFixed(6)} mm, ` +
    `${ringReport.reachability.encroachment}`,
);

const space0 = gear.contours.tooth_space;
const profile = gear.contours.full_profile;
const toolPath0 = gear.contours.tool_centre_path;
console.log(
  `\n   tooth space 0: ${space0.length} points, tool centre path ${toolPath0.length} points`,
);
console.log(`   full profile: ${profile.length} points`);

// The narrowest place in a tooth space is its root. A Ø1 cutter fits it if,
// and only if, that width is wider than the cutter; otherwise the only honest
// strategy is the thin-slot centre line, which cuts past the wall.
const root = planet.space_width_at_root;
console.log(
  `\n   space width at the root ${root.toFixed(4)} mm vs a Ø${CUTTER} cutter ` +
    `→ ${((root - CUTTER) / 2).toFixed(4)} mm of clearance per side`,
);
expect(
  root > CUTTER,
  `the Ø${CUTTER} cutter fits the root of the space, so the spaces are cut as ` +
    `ordinary inside contours and the wall error is zero (no centre-line fallback)`,
);

// Every space is space 0 turned by one pitch. Proved, not assumed: a gear
// whose spaces were not congruent would be cut wrong 19 times over.
const pitch = (2 * Math.PI) / planet.teeth;
const { answer: gear7 } = cam(
  "gear",
  { ...gearRequest, tooth: 7, planetary: undefined, span: false },
  { name: "planet-gear-tooth7", echo: false },
);
const rotated7 = rotate(space0, 7 * pitch);
const worstRotation = Math.max(
  ...gear7.contours.tooth_space.map((p, i) => Math.hypot(p[0] - rotated7[i][0], p[1] - rotated7[i][1])),
);
expect(
  worstRotation < 1e-9,
  `space 7 is space 0 turned ${((7 * pitch * 180) / Math.PI).toFixed(1)}° ` +
    `(worst disagreement ${worstRotation.toExponential(2)} mm)`,
);

// ===========================================================================
banner("2 · recommend — feeds for C360 brass, Ø1 2-flute, hobby, dial spindle");
// ===========================================================================
const tool = { diameter: CUTTER, flutes: 2, kind: "flat_end_mill", flute_length: 6.0 };
const machine = {
  name: "Anolex Ultra 2",
  class: "hobby",
  spindle: "dial",
  max_feed: 4000,
  max_accel: 400,
  travel: { min: [0, 0, -80], max: [400, 300, 0] },
  work_offset: [40, 40, -25],
};
const { answer: slotFeeds } = cam(
  "recommend",
  { material: "brass-c360", op: "slot", tool, machine },
  { name: "planet-feeds-slot" },
);
const { answer: profileFeeds } = cam(
  "recommend",
  { material: "brass-c360", op: "profile", tool, machine },
  { name: "planet-feeds-profile", echo: false },
);
const slot = slotFeeds.recommendation;
const prof = profileFeeds.recommendation;
console.log(
  `\n   profile (one side open): ${prof.feed_mm_min.toFixed(0)} mm/min, ` +
    `stepdown ${prof.stepdown_mm.toFixed(3)} mm, plunge ${prof.plunge_mm_min.toFixed(0)} mm/min`,
);
expect(
  slotFeeds.dial_spindle && slot.dial !== null,
  `the S word does nothing on this spindle — set the dial to ${slot.dial} by hand`,
);
expect(
  Math.abs(slot.chipload_mm / 0.0146 - 0.6) < 0.02,
  `the hobby class derates the table chipload by 60 % (${slot.chipload_mm.toFixed(4)} mm/tooth)`,
);
const passes = Math.ceil((FACE + BREAK_THROUGH) / slot.stepdown_mm);
console.log(
  `\n   ${(FACE + BREAK_THROUGH).toFixed(2)} mm of cut at ${slot.stepdown_mm.toFixed(3)} mm ` +
    `per pass = ${passes} passes per space (the drawing note's "3 passes" is ` +
    `${((FACE + BREAK_THROUGH) / 3 / slot.stepdown_mm).toFixed(1)}× this stepdown)`,
);

// ===========================================================================
banner("3 · job — 20 tooth spaces, the bore, the profile: posted and verified");
// ===========================================================================
const placement = { dx: CENTRE, dy: CENTRE };

// A tooth space as `gear` hands it back is CLOSED across its mouth by an arc
// of the tip circle. Cut as a closed inside contour it leaves the top ~0.5 mm
// of both flanks standing, because the cutter centre can never come within its
// own radius of the mouth — the first run of this rehearsal was refused for
// exactly that: 40 patches of 0.147 mm², up to 0.578 mm proud.
//
// So open the mouth. The two mouth points are pushed radially out to one
// cutter diameter past the tip circle and the mouth arc is dropped, which
// turns each space into a channel the cutter can run into from outside the
// gear. Radial rays keep the angles the mouth points already had, so the tip
// land between two spaces is untouched, and everything past r 10.89 in a
// space's own sector is waste the profile pass takes anyway.
//
// This is what "mill the 20 spaces through from +z" means once the blank is
// round: the cutter does not start inside the space, it drives into it.
const MOUTH_R = TIP_R + CUTTER;
const openMouth = (loop) => {
  const push = (p) => {
    const r = Math.hypot(p[0], p[1]);
    return [(p[0] * MOUTH_R) / r, (p[1] * MOUTH_R) / r];
  };
  const below = loop.map((p) => Math.hypot(p[0], p[1]) < TIP_R - 1e-9);
  const first = below.indexOf(true);
  const last = below.lastIndexOf(true);
  if (first < 1 || last < first) throw new Error("the tooth space does not open onto the tip circle");
  // `first - 1` and `last + 1` are the two mouth points, the ends of the arc
  // that closed the space. Keep the flanks and the root, drop the arc, and
  // carry the mouth points outward along their own radii.
  return [push(loop[first - 1]), ...loop.slice(first, last + 1), push(loop[last + 1])];
};
const space0Open = openMouth(space0);

const spaceOps = Array.from({ length: planet.teeth }, (_, n) => ({
  name: `tooth space ${n}`,
  tool: 1,
  kind: "contour_inside",
  contour: rotate(space0Open, n * pitch),
  depth: FACE,
  bottom_allowance: -BREAK_THROUGH,
  stepdown: slot.stepdown_mm,
  feed: slot.feed_mm_min,
  plunge: slot.plunge_mm_min,
  rpm: slot.rpm,
  direction: "climb",
  entry: "ramp",
  ramp_angle: slot.ramp_angle_deg,
  // A lead-in arc needs room beside the wall, and a tooth space is 0.07 mm
  // wider than the cutter: there is none. Ramp down on the path itself.
  lead_in: false,
  phase: 1,
}));

// The bore. A helical bore is the obvious way to open Ø3.8 and it is REFUSED
// here — a helix sweeps an annulus, so a Ø1 cutter only opens a hole under
// Ø2.0 and anything wider leaves a core standing in the middle. The refusal is
// rehearsed below and then obeyed: the same opening, cleared as a pocket.
const boreShared = {
  name: `bore Ø${BORE_MILLED} (Ø${BORE_FINISHED} H7 after reaming)`,
  tool: 1,
  depth: FACE,
  bottom_allowance: -BREAK_THROUGH,
  stepdown: slot.stepdown_mm,
  feed: slot.feed_mm_min,
  plunge: slot.plunge_mm_min,
  rpm: slot.rpm,
  phase: 0,
};
const boreHelical = () => ({
  ...boreShared,
  kind: "helical_bore",
  x: 0,
  y: 0,
  diameter: BORE_MILLED,
  through: true,
  pitch: slot.stepdown_mm,
});
const borePocket = () => ({
  ...boreShared,
  kind: "pocket",
  contour: circle(BORE_MILLED / 2, 96),
  stepover: slot.stepover_mm * 0.4,
});
// And the pocket is refused too — not by the kernel but by the verification
// oracle, on a phantom tab: every pocket pass ends with a 0.002 mm lift at the
// pocket centre, the tab audit records each one as a tab with a metal width of
// -0.998 mm (a lifted run narrower than the cutter), and then treats every
// deeper pass as cutting through it. See docs/planet-milestone.md.
//
// A pilot bore under twice the cutter diameter, opened out to size by a
// contour, clears the same Ø3.8 opening and is what the job actually cuts.
const BORE_PILOT = 1.9; // mm — under 2 x Ø1, so the helix leaves no core
const boreOps = () => [
  {
    ...boreShared,
    name: `${boreShared.name} — pilot Ø${BORE_PILOT}`,
    kind: "helical_bore",
    x: 0,
    y: 0,
    diameter: BORE_PILOT,
    through: true,
    pitch: slot.stepdown_mm,
  },
  {
    ...boreShared,
    name: `${boreShared.name} — out to size`,
    kind: "contour_inside",
    contour: circle(BORE_MILLED / 2, 96),
    lead_in: false,
    entry: "ramp",
    ramp_angle: slot.ramp_angle_deg,
  },
];

// The outside profile. Phase 2: the bore is opened first (phase 0) so a
// Ø4 screw through it can hold the blank down, then the spaces (phase 1), then
// this. `extra` carries whatever tab request is being rehearsed.
const profileOp = (extra) => ({
  name: "outside profile (tip circle r 10.89)",
  tool: 1,
  kind: "contour_outside",
  contour: circle(TIP_R, 180),
  depth: FACE,
  bottom_allowance: -BREAK_THROUGH,
  stepdown: prof.stepdown_mm,
  feed: prof.feed_mm_min,
  plunge: prof.plunge_mm_min,
  rpm: prof.rpm,
  direction: "climb",
  entry: "ramp",
  ramp_angle: prof.ramp_angle_deg,
  phase: 2,
  ...extra,
});

const job = {
  name: "rana-60-cnc planet (C360, 20T m1.0)",
  stock: { thickness: FACE, bbox: [0, 0, BLANK, BLANK], spoilboard: SPOILBOARD },
  machine,
  tools: [
    {
      number: 1,
      name: "Ø1.0 2-flute carbide, brass",
      kind: "flat_end_mill",
      diameter: CUTTER,
      flutes: 2,
      flute_length: 6.0,
      stickout: 12.0,
      shank_diameter: 3.175,
      centre_cutting: true,
      holder: { diameter: 30.0, length: 40.0 },
    },
  ],
  operations: [...boreOps(), ...spaceOps, profileOp({ tabs: 3, tab_width: 2.0, tab_height: 0.5 })],
  options: {
    placement,
    safe_z: 5.0,
    park_z: 5.0,
    spin_up_seconds: 3.0,
    wcs: "G54",
    end: "m2",
    post: "grbl",
    arc_fit: { tolerance: 0.01 },
    verify: true,
    tool_change: { type: "manual_pause_reprobe" },
    // The part this job is meant to make, in the part's own frame: the toothed
    // outline, with the bore AS MILLED (Ø3.8) — the 0.1 mm per side left for
    // the reamer is metal on purpose, not metal the job failed to reach.
    part: { outer: profile, holes: [circle(BORE_MILLED / 2, 96)] },
  },
};

// FINDINGS, rehearsed rather than quietly avoided. Both bore strategies the
// task called for are refused, and the refusals are recorded here so the
// milestone doc quotes the machine rather than a memory of it.
// Reproduced on the bore and the profile alone — the same request minus the
// twenty spaces, so the refusal is legible and the run is seconds rather than
// minutes.
const withBore = (ops) => ({
  ...job,
  operations: [...ops, job.operations[job.operations.length - 1]],
});
const helical = cam("job", withBore([boreHelical()]), {
  name: "planet-job-helical-bore",
  echo: false,
  allowFailure: true,
});
expect(
  helical.code === 2 && /helix only opens a hole under twice the cutter diameter/.test(helical.stderr),
  `a helical Ø${BORE_MILLED} bore with a Ø${CUTTER} cutter is REFUSED by the kernel — ` +
    helical.stderr.trim().replace(/^REFUSED: /, "").replace(/\s+/g, " "),
);
const pocketed = cam("job", withBore([borePocket()]), {
  name: "planet-job-pocket-bore",
  echo: false,
  allowFailure: true,
});
const pocketStub = (pocketed.answer?.verification?.tabs?.observations ?? []).find(
  (o) => o.metal_width < 0,
);
expect(
  pocketed.code === 2 &&
    pocketed.answer.policy.blocked_by.includes("tabs") &&
    pocketStub !== undefined,
  `the same bore as a POCKET is blocked on a phantom tab: a ` +
    `${pocketStub ? pocketStub.lifted_run.toFixed(5) : "?"} mm lift at the pocket centre recorded ` +
    `as a tab with ${pocketStub ? pocketStub.metal_width.toFixed(3) : "?"} mm of metal under it — ` +
    `a lifted run narrower than the cutter, which is not a tab. ` +
    `${pocketed.answer.verification.tabs.check.violation_count} violations, one per pass.`,
);
console.log(
  `   → the job below bores with a Ø${BORE_PILOT} helical pilot opened out to Ø${BORE_MILLED} ` +
    `by a contour, which clears the same hole with no tab observation at all.\n`,
);

// FINDING 3 — the ramp entry. This is the job AS DESIGNED: a ramped entry on
// every space, which is what a Ø1 carbide cutter wants in brass. It is
// BLOCKED, and not by the part. The ramp lays down a level stub at the
// previous pass's floor on the way in; the tab audit records that stub as a
// tab, with a NEGATIVE metal width — a lifted run narrower than the cutter is
// wide, which cannot be a tab — and then reads every deeper pass as cutting
// through it.
const designed = cam("job", job, { name: "planet-job-as-designed", echo: false });
const stubs = (designed.answer.verification?.tabs?.observations ?? []).filter(
  (o) => o.metal_width < 0,
);
const stubPlaces = new Set(stubs.map((o) => o.xy.map((v) => v.toFixed(3)).join()));
expect(
  designed.code === 2 &&
    designed.answer.blocked === true &&
    designed.answer.policy.blocked_by.join() === "tabs" &&
    stubPlaces.size === planet.teeth,
  `the job AS DESIGNED is BLOCKED on ${stubPlaces.size} phantom tabs, one per space — ` +
    `${stubs.length ? stubs[0].lifted_run.toFixed(5) : "?"} mm ramp stubs with ` +
    `${stubs.length ? stubs[0].metal_width.toFixed(3) : "?"} mm of metal under each, ` +
    `${designed.answer.verification.tabs.check.violation_count} violations. Every other check ` +
    `passes: ${designed.answer.verification.material_left.check.violation_count} material-left, ` +
    `${designed.answer.verification.gouge.violation_count} gouge, ` +
    `${designed.answer.verification.depth.check.violation_count} depth.`,
);
expect(
  !existsSync(join(OUT, "planet-as-designed.nc")),
  "a blocked job writes no G-code file — there is nothing to send by accident",
);

// FINDING 4 — the tabs. Plunge the spaces instead of ramping and the phantoms
// vanish, which leaves the tab audit free to answer the real question: can
// this part hang on tabs at its tip circle? It cannot, and the reason is the
// gear's own geometry.
const plunged = (o) =>
  o.name.startsWith("tooth space") ? { ...o, entry: "plunge", ramp_angle: undefined } : o;
const tabbed = cam(
  "job",
  { ...job, operations: job.operations.map(plunged) },
  { name: "planet-job-tabbed", echo: false },
);
const tabCheck = tabbed.answer.verification.tabs;
expect(
  tabbed.code === 2 &&
    tabbed.answer.policy.blocked_by.join() === "tabs" &&
    tabCheck.observations.every((o) => o.metal_width > 1.5),
  `with the spaces plunged the audit finds exactly the ${tabCheck.tab_count} tabs asked for ` +
    `(${tabCheck.observations[0].metal_width.toFixed(3)} mm of metal each) and still BLOCKS: ` +
    `${tabCheck.check.violation_count} passes cut straight through them, because a tooth space ` +
    `runs out past the tip circle at the same angle.`,
);
expect(
  planet.tip_land < 1.5,
  `and no tab can be moved out of the way: a tab wants at least 1.50 mm of metal under it, and ` +
    `this gear's tooth tip land is ${planet.tip_land.toFixed(4)} mm. There is nowhere on a ` +
    `Ø${(2 * TIP_R).toFixed(2)} 20T gear to hang one. The part is held by a screw through its own ` +
    `bore instead — which is why the bore is phase 0.`,
);

// FINDING 5 — the workholding. With no tabs the profile frees the part, and
// `loose_pieces` is an ERROR when what comes free is the part rather than
// waste: 294.42 mm² at the centre, "no tab, no skin". That is the right answer
// to the question it can ask. What it cannot be told is that the part is
// SCREWED DOWN — there is no way to declare workholding to the oracle, so the
// only way to say "yes, I know, it is bolted through its own bore" is
// `verify_policy`. That override IS the workholding claim, and it is the one
// line of this job a reviewer should look at hardest.
//
// The alternative that needs no override is an onion skin: a positive
// `bottom_allowance` on every operation leaves the part hanging on 0.15 mm of
// brass to be sanded off afterwards. On a 20-tooth gear that is 20 slots to
// clean up by hand, which is why the screw wins here.
const gcodePath = join(OUT, "planet.nc");
const posted = {
  ...job,
  name: `${job.name} — screwed through the bore, no tabs`,
  verify_policy: { loose_pieces: "warning" },
  operations: job.operations
    .map(plunged)
    .map((o) => (o.kind === "contour_outside" ? profileOp({}) : o)),
};
const { code: jobCode, answer: jobAnswer } = cam("job", posted, {
  name: "planet-job",
  args: ["--gcode", gcodePath],
});
expect(jobCode === 0 && jobAnswer.blocked === false, "the posted job is NOT blocked");
expect(
  typeof jobAnswer.gcode === "string" && existsSync(gcodePath),
  `the posted job carries G-code (${jobAnswer.gcode?.split("\n").length} lines, on disk)`,
);
expect(
  jobAnswer.verification.tabs.tab_count === 0 &&
    jobAnswer.policy.warnings.join() === "loose_pieces",
  `no tabs are cut, and the ONLY check demoted is loose_pieces — ` +
    `${jobAnswer.verification.loose.check.worst.toFixed(2)} mm² of part comes free, which the ` +
    `Ø${BORE_FINISHED} screw through the bore is holding. Everything else passes on its own merits.`,
);

// ===========================================================================
banner("4 · verify — the posted file, replayed from disk");
// ===========================================================================
const { code: verifyCode, answer: verifyAnswer } = cam(
  "verify",
  {
    part: job.options.part,
    stock: job.stock,
    machine,
    tool_diameter: CUTTER,
    placement,
    bottom_allowance: -BREAK_THROUGH,
    centre_cutting: true,
    // The same workholding claim the job was posted under. Without it the
    // replay of a job's own output disagrees with the job.
    verify_policy: posted.verify_policy,
  },
  { name: "planet-verify", args: ["--gcode", gcodePath] },
);
expect(
  verifyCode === 0 && verifyAnswer.blocked === false,
  `the posted program, read back off disk and replayed against the part, is accepted under the ` +
    `same policy it was posted under (${verifyAnswer.verification.moves} moves replayed; the only ` +
    `demoted check is ${verifyAnswer.policy.warnings.join(", ") || "none"})`,
);

// ===========================================================================
banner("5 · fit — does a Ø1 cutter fit a tooth space?");
// ===========================================================================
const { code: fitCode, answer: fitAnswer } = cam(
  "fit",
  { contour: space0, tool_diameter: CUTTER, side: "inside" },
  { name: "planet-fit" },
);
expect(
  fitCode === 0 && fitAnswer.fits === true,
  `a Ø${CUTTER} cutter fits the planet's tooth space ` +
    `(the largest that does is Ø${fitAnswer.largest_tool_diameter.toFixed(4)})`,
);
// The sun's spaces are the tightest of the three: report them too, no job.
const { answer: sunGear } = cam(
  "gear",
  {
    gear: { module: 1.0, teeth: 10, profile_shift: 0.47, backlash_thinning: 0.03, tip_radius: 6.45, face_width: 5.0 },
    cutter_diameter: CUTTER,
    contours: true,
    tooth: 0,
  },
  { name: "planet-sun-gear", echo: false },
);
const { code: sunFitCode, answer: sunFit } = cam(
  "fit",
  { contour: sunGear.contours.tooth_space, tool_diameter: CUTTER, side: "inside" },
  { name: "planet-sun-fit", echo: false },
);
console.log(
  `   sun (10 blind pockets): root width ${sunGear.report.space_width_at_root.toFixed(4)} mm, ` +
    `fit ${sunFit.fits ? "fits" : "DOES NOT FIT"}, largest tool ` +
    `Ø${sunFit.largest_tool_diameter.toFixed(4)} (exit ${sunFitCode})`,
);
const { answer: ringGear } = cam(
  "gear",
  {
    gear: { module: 1.0, teeth: 50, internal: true, profile_shift: 0.47, backlash_thinning: 0.03, tip_radius: 24.47, face_width: 6.0 },
    cutter_diameter: CUTTER,
    contours: true,
    tooth: 0,
  },
  { name: "planet-ring-gear", echo: false },
);
const { code: ringFitCode, answer: ringFit } = cam(
  "fit",
  { contour: ringGear.contours.tooth_space, tool_diameter: CUTTER, side: "inside" },
  { name: "planet-ring-fit", echo: false },
);
console.log(
  `   ring (50T internal, Ø59 blank, 6 mm): root width ` +
    `${ringGear.report.space_width_at_root.toFixed(4)} mm, fit ` +
    `${ringFit.fits ? "fits" : "DOES NOT FIT"}, largest tool ` +
    `Ø${ringFit.largest_tool_diameter.toFixed(4)} (exit ${ringFitCode})`,
);

// ===========================================================================
banner("6 · outline + compare — the design intent, sectioned and checked");
// ===========================================================================
// The design intent as a solid: the gear's own profile, extruded the face
// width. Written as an STL and sectioned straight back out, so the comparison
// runs through the real section code rather than comparing a list against
// itself.
const stlPath = join(OUT, "planet-design-intent.stl");
writeFileSync(stlPath, prismStl(profile, 0, FACE));
const { code: outlineCode, answer: outlineAnswer } = cam("outline", null, {
  name: "planet-outline",
  args: [stlPath],
});
expect(outlineCode === 0, "the design-intent solid sections cleanly (no tear)");
const { code: compareCode, answer: compareAnswer } = cam(
  "compare",
  { a: { loops: [profile] }, b: { outline: outlineAnswer.outline }, tolerance: 0.02 },
  { name: "planet-compare" },
);
expect(
  compareCode === 0 && compareAnswer.agrees === true,
  `the section agrees with the gear's own profile to ` +
    `${compareAnswer.diff.max_boundary_distance.toFixed(5)} mm (tolerance 0.02)`,
);

// ===========================================================================
banner("the shop sheet");
// ===========================================================================
const summary = {
  part: "rana-60-cnc planet, C360 brass, 20T module 1.0, x +0.00",
  blank: `${BLANK} × ${BLANK} × ${FACE} mm C360 plate over a ${SPOILBOARD} mm sacrificial board`,
  zero: `top of the stock, part centre at X${CENTRE} Y${CENTRE}`,
  cutter: `Ø${CUTTER} 2-flute carbide, ${slot.dial === null ? "" : `router dial ${slot.dial} ≈ `}${slot.rpm} rpm`,
  feeds: {
    slot: { feed: slot.feed_mm_min, plunge: slot.plunge_mm_min, stepdown: slot.stepdown_mm, chipload: slot.chipload_mm },
    profile: { feed: prof.feed_mm_min, plunge: prof.plunge_mm_min, stepdown: prof.stepdown_mm },
  },
  passes_per_space: passes,
  bore: `milled Ø${BORE_MILLED}, reamed to Ø${BORE_FINISHED} H7 (${REAM_ALLOWANCE} mm on the diameter)`,
  break_through: BREAK_THROUGH,
  measure_after_the_cut: {
    over_pins: { pin_diameter: PIN, nominal: planet.over_pins.dimension, sensitivity: planet.over_pins.sensitivity },
    span: { teeth: planet.span.teeth_spanned, nominal: planet.span.length },
    tip_diameter: 2 * TIP_R,
    bore: BORE_FINISHED,
  },
  cutting_time_s: jobAnswer.duration.accel_aware_s,
  moves: jobAnswer.moves.length,
  gcode_lines: jobAnswer.gcode.split("\n").length,
  reachability: {
    sun: sunReport.reachability,
    planet: planetReport.reachability,
    ring: ringReport.reachability,
  },
};
writeFileSync(join(OUT, "planet-shop-sheet.json"), JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));

// What a measured reading turns into, worked twice so the sign is unmistakable.
banner("7 · compensation — turning a measured M into the next cut's offset");
for (const measured of [planet.over_pins.dimension + 0.02, planet.over_pins.dimension - 0.02]) {
  const { answer } = cam(
    "gear",
    { ...gearRequest, contours: false, planetary: undefined, measured: Number(measured.toFixed(4)) },
    { name: `planet-compensation-${measured > planet.over_pins.dimension ? "over" : "under"}` },
  );
  expect(
    Math.sign(answer.compensation.detail.tool_normal_offset) ===
      -Math.sign(measured - planet.over_pins.dimension),
    `M ${measured.toFixed(4)} (${(measured - planet.over_pins.dimension > 0 ? "+" : "") + (measured - planet.over_pins.dimension).toFixed(4)}) ` +
      `→ move the cutter ${answer.compensation.detail.tool_normal_offset.toFixed(5)} mm along the flank normal`,
  );
}

console.log(`\nartifacts in ${OUT}`);
if (failures.length > 0) {
  console.error(`\n${failures.length} step(s) did not come out as expected:`);
  for (const f of failures) console.error(`  · ${f}`);
  process.exit(1);
}
console.log("\nevery step came out as expected.");

// ---------------------------------------------------------------------------

/**
 * A binary STL of `loop` extruded from `z0` to `z1`. The caps are fanned from
 * the centre, which is valid here and only here: a gear profile is star-shaped
 * about its own axis, so every triangle (centre, p[i], p[i+1]) lies inside it.
 */
function prismStl(loop, z0, z1) {
  const tris = [];
  const c = [0, 0];
  for (let i = 0; i < loop.length; i += 1) {
    const a = loop[i];
    const b = loop[(i + 1) % loop.length];
    tris.push([[a[0], a[1], z0], [b[0], b[1], z0], [b[0], b[1], z1]]);
    tris.push([[a[0], a[1], z0], [b[0], b[1], z1], [a[0], a[1], z1]]);
    tris.push([[c[0], c[1], z1], [a[0], a[1], z1], [b[0], b[1], z1]]);
    tris.push([[c[0], c[1], z0], [b[0], b[1], z0], [a[0], a[1], z0]]);
  }
  const buffer = Buffer.alloc(84 + tris.length * 50);
  buffer.write("vcad planet design intent", 0, "ascii");
  buffer.writeUInt32LE(tris.length, 80);
  let at = 84;
  for (const t of tris) {
    at += 12; // normal: zero, which every reader recomputes
    for (const v of t) {
      buffer.writeFloatLE(v[0], at);
      buffer.writeFloatLE(v[1], at + 4);
      buffer.writeFloatLE(v[2], at + 8);
      at += 12;
    }
    at += 2;
  }
  return buffer;
}
