// Generate higher-fidelity reference .vcad files for the matrix's
// "expected" column. One function per task — each authored from the
// task spec's prompt to capture the actual geometry, not just a bbox
// silhouette. The matrix renders these via vcad-render.
//
// Coverage: ALL public tasks (A1–A6, plus the Fit / Visual / Mech suites).
// Without an authored reference a task's "expected" cell falls back to a
// model's passing run — and the harder tasks (the ones nobody passes) end
// up blank. Authoring a ref per task guarantees every row renders.
//
//   - A1–A6: the single solid the prompt describes.
//   - F (Fit): a TWO-ROOT composite — the (ghosted, cool-navy) host plus
//     the ideal accessory (warm brass) mated in the host's frame, so the
//     cell shows the part doing its job, not a context-free stub. Hosts
//     are authored from the prompts' stated dimensions; see the shared
//     `*Host` builders below.
//   - D1 (Visual): the target shape itself.
//   - C (Mech): a static posed reacher arm (display only, not graded).
//
// NOTE: the kernel Cylinder is analytic and renders ROUND regardless of
// `segments`; use the `prism()` helper (true Prism primitive) for any
// polygonal cross-section (hex nut, octagon flange, pentagon, hex bolt).
//
// Run: `node mecheval/leaderboard/scripts/gen-task-refs.mjs`
// Then rebuild the SVG cache (`npm run build -w @mecheval/leaderboard`
// with vcad-render built) and commit the cache deltas — Vercel builds
// from the committed cache and has no Rust toolchain.

import { writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const TASKS_DIR = resolve(SCRIPT_DIR, "../../tasks");

// ─── builder ──────────────────────────────────────────────────────────
// A tiny DSL for authoring .vcad operation graphs.

class B {
  constructor() {
    this.nodes = {};
    this.next = 1;
  }
  add(name, op) {
    const id = this.next++;
    this.nodes[String(id)] = { id, name, op };
    return id;
  }
  cube(name, sx, sy, sz) { return this.add(name, { type: "Cube", size: { x: sx, y: sy, z: sz } }); }
  cyl(name, r, h, segments = 64) { return this.add(name, { type: "Cylinder", radius: r, height: h, segments } ); }
  sphere(name, r, segments = 64) { return this.add(name, { type: "Sphere", radius: r, segments }); }
  cone(name, rb, rt, h, segments = 64) { return this.add(name, { type: "Cone", radius_bottom: rb, radius_top: rt, height: h, segments }); }
  tr(name, child, x, y, z) { return this.add(name, { type: "Translate", child, offset: { x, y, z } }); }
  rotZ(name, child, deg) { return this.add(name, { type: "Rotate", child, angles: { x: 0, y: 0, z: deg } }); }
  rotY(name, child, deg) { return this.add(name, { type: "Rotate", child, angles: { x: 0, y: deg, z: 0 } }); }
  rotX(name, child, deg) { return this.add(name, { type: "Rotate", child, angles: { x: deg, y: 0, z: 0 } }); }
  union(name, left, right) { return this.add(name, { type: "Union", left, right }); }
  diff(name, left, right) { return this.add(name, { type: "Difference", left, right }); }
  intersect(name, left, right) { return this.add(name, { type: "Intersection", left, right }); }
  // Corner-origin cube centered in X/Y about (0,0), spanning z in [0, sz].
  centeredBox(name, sx, sy, sz) { return this.box(name, -sx / 2, -sy / 2, 0, sx, sy, sz); }
  // Build a corner-origin cube spanning [minX..minX+sx] etc.
  box(name, minX, minY, minZ, sx, sy, sz) {
    const c = this.cube(name + "_raw", sx, sy, sz);
    return this.tr(name, c, minX, minY, minZ);
  }
  // Center-axis cylinder positioned at (cx, cy, baseZ).
  cylAt(name, cx, cy, baseZ, r, h, segments = 64) {
    const c = this.cyl(name + "_raw", r, h, segments);
    return this.tr(name, c, cx, cy, baseZ);
  }
  // Union a list of ids, left-folded.
  unionAll(name, ids) {
    let acc = ids[0];
    for (let i = 1; i < ids.length; i++) {
      acc = this.union(`${name}_${i}`, acc, ids[i]);
    }
    return acc;
  }
  // Bolt circle: N cylinders evenly spaced on a radius, first hole on +X.
  boltCircle(name, cx, cy, baseZ, holeR, holeH, count, circleR, segments = 32) {
    const ids = [];
    for (let i = 0; i < count; i++) {
      const theta = (i * 2 * Math.PI) / count;
      const x = cx + circleR * Math.cos(theta);
      const y = cy + circleR * Math.sin(theta);
      ids.push(this.cylAt(`${name}_${i}`, x, y, baseZ, holeR, holeH, segments));
    }
    return this.unionAll(name, ids);
  }
  // Regular N-gon prism via the kernel's true polygonal Prism primitive
  // (extruded along +Z from z=0). `circumR` is the circumradius; the
  // cylinder primitive is analytic and would render round, so we must use
  // Prism for real facets. `firstVertexDeg` rotates the first vertex.
  prism(name, cx, cy, baseZ, circumR, h, sides, firstVertexDeg = 0) {
    const raw = this.add(name + "_raw", { type: "Prism", sides, radius: circumR, height: h });
    const rot = this.rotZ(name + "_rot", raw, firstVertexDeg);
    return this.tr(name, rot, cx, cy, baseZ);
  }
  serialize(rootId) {
    return JSON.stringify({
      version: "0.1",
      nodes: this.nodes,
      materials: {},
      part_materials: {},
      roots: [{ root: rootId, material: "default" }],
    }, null, 2) + "\n";
  }
  // Multi-root composite serialization — used for the Fit suite so the
  // matrix shows the accessory mated with its (ghosted) host, not a
  // context-free stub floating in space. `roots` is an array of
  // { root, material } and `materials` is the named-material map.
  serializeMulti(roots, materials) {
    return JSON.stringify({
      version: "0.1",
      nodes: this.nodes,
      materials,
      part_materials: {},
      roots,
    }, null, 2) + "\n";
  }
}

// Material palette for Fit composites: the host recedes (achromatic →
// the renderer keeps it in the cool navy ramp), the accessory pops with a
// warm brass hue so the eye reads "this is the part the model made".
const HOST_MAT = { name: "host", color: [0.58, 0.61, 0.66], metallic: 0.2, roughness: 0.7, density: 1000.0, friction: 0.6 };
const ACC_MAT = { name: "accessory", color: [0.82, 0.55, 0.22], metallic: 0.8, roughness: 0.4, density: 1000.0, friction: 0.6 };
const FIT_MATERIALS = { host: HOST_MAT, accessory: ACC_MAT };

// Build a Fit composite: `hostFn(b)` returns the host root id, `accFn(b)`
// returns the accessory root id; both author into the same builder. Emits
// a two-root document (ghost host + brass accessory).
function fitComposite(hostFn, accFn) {
  const b = new B();
  const host = hostFn(b);
  const acc = accFn(b);
  return [b, [
    { root: host, material: "host" },
    { root: acc, material: "accessory" },
  ], FIT_MATERIALS];
}

// ─── shared Fit hosts (authored from the task prompts' stated dims) ─────

// Stepped shaft: two OD40 flanges with an OD20 waist between them.
// Bottom flange z[0,10], waist z[10,40], top flange z[40,50].
function steppedShaftHost(b) {
  const botFlange = b.cylAt("host_bot_flange", 0, 0, 0, 20, 10);
  const waist = b.cylAt("host_waist", 0, 0, 10, 10, 30);
  const topFlange = b.cylAt("host_top_flange", 0, 0, 40, 20, 10);
  return b.unionAll("host_shaft", [botFlange, waist, topFlange]);
}

// Open thin-walled tube: OD30 (r15), bore OD24 (r12), height 30.
function capTubeHost(b) {
  const outer = b.cylAt("host_tube_outer", 0, 0, 0, 15, 30);
  const bore = b.cylAt("host_tube_bore", 0, 0, -1, 12, 32);
  return b.diff("host_tube", outer, bore);
}

// Block 50×50×15 with a 16mm-dia, 10mm-deep recess centered at (25,25).
function plugPortHost(b) {
  const block = b.box("host_block", 0, 0, 0, 50, 50, 15);
  const recess = b.cylAt("host_recess", 25, 25, 5, 8, 11);
  return b.diff("host_port", block, recess);
}

// C-channel: bottom plate z[0,6], back wall y[0,8] z[6,14], top plate z[14,20].
function shimGapHost(b) {
  const bottom = b.box("host_bottom", 0, 0, 0, 50, 40, 6);
  const back = b.box("host_back", 0, 0, 6, 50, 8, 8);
  const top = b.box("host_top", 0, 0, 14, 50, 40, 6);
  return b.unionAll("host_channel", [bottom, back, top]);
}

// Bottle neck: lower neck OD10 z[0,20], bead OD12 z[20,23], stub OD10 z[23,27].
function bottleNeckHost(b) {
  const lower = b.cylAt("host_neck_lower", 0, 0, 0, 5, 20);
  const bead = b.cylAt("host_neck_bead", 0, 0, 20, 6, 3);
  const stub = b.cylAt("host_neck_stub", 0, 0, 23, 5, 4);
  return b.unionAll("host_neck", [lower, bead, stub]);
}

// Vertical pipe OD16 (r8), 30mm tall.
function pipeHost(b) {
  return b.cylAt("host_pipe", 0, 0, 0, 8, 30);
}

// ─── per-task authoring ───────────────────────────────────────────────

const TASKS = {
  // ─── A1: primitives ──────────────────────────────────────────────────
  "a1-block-01": () => {
    const b = new B();
    return [b, b.centeredBox("part", 60, 40, 20)];
  },
  "a1-cone-01": () => {
    const b = new B();
    return [b, b.cone("part", 20, 10, 30)];
  },
  "a1-cube-01": () => {
    const b = new B();
    return [b, b.box("part", -12.5, -12.5, -12.5, 25, 25, 25)];
  },
  "a1-pipe-01": () => {
    const b = new B();
    const outer = b.cylAt("outer", 0, 0, 0, 15, 40);
    const bore = b.cylAt("bore", 0, 0, -1, 10, 42);
    return [b, b.diff("part", outer, bore)];
  },
  "a1-plate-01": () => {
    const b = new B();
    const plate = b.centeredBox("plate", 50, 30, 10);
    const corners = [[20, 10], [-20, 10], [-20, -10], [20, -10]];
    const holes = corners.map(([x, y], i) => b.cylAt(`hole_${i}`, x, y, -1, 1.5, 12));
    return [b, b.diff("part", plate, b.unionAll("holes", holes))];
  },
  "a1-sphere-01": () => {
    const b = new B();
    return [b, b.sphere("part", 15)];
  },
  "a1-stepped-shaft-01": () => {
    const b = new B();
    const base = b.cylAt("base", 0, 0, 0, 20, 10);
    const top = b.cylAt("top", 0, 0, 10, 10, 15);
    return [b, b.union("part", base, top)];
  },

  // ─── A2: blocks, holes, plates ───────────────────────────────────────
  "a2-bolt-circle-block-01": () => {
    const b = new B();
    const block = b.centeredBox("block", 60, 60, 10);
    const bolts = b.boltCircle("bolts", 0, 0, -1, 2.5, 12, 8, 20);
    return [b, b.diff("part", block, bolts)];
  },
  "a2-channel-bracket-01": () => {
    const b = new B();
    const bottom = b.box("bottom", 0, 0, 0, 30, 40, 4);
    const left = b.box("left", 0, 0, 4, 4, 40, 26);
    const right = b.box("right", 26, 0, 4, 4, 40, 26);
    return [b, b.unionAll("part", [bottom, left, right])];
  },
  "a2-cube-with-pocket-01": () => {
    const b = new B();
    const cube = b.box("cube", -20, -20, 0, 40, 40, 40);
    const pocket = b.box("pocket", -10, -10, 30, 20, 20, 11);
    return [b, b.diff("part", cube, pocket)];
  },
  "a2-cubemark-01": () => {
    const b = new B();
    const cube = b.centeredBox("cube", 30, 30, 30);
    const pts = [];
    for (const gy of [-8, 0, 8]) for (const gx of [-8, 0, 8]) pts.push([gx, gy]);
    const holes = pts.map(([x, y], i) => b.cylAt(`h_${i}`, x, y, -1, 1.5, 32));
    return [b, b.diff("part", cube, b.unionAll("holes", holes))];
  },
  "a2-finned-block-01": () => {
    const b = new B();
    const base = b.centeredBox("base", 50, 30, 5);
    const fins = [-10, 0, 10].map((cy, i) => b.box(`fin_${i}`, -25, cy - 1.5, 5, 50, 3, 15));
    return [b, b.unionAll("part", [base, ...fins])];
  },
  "a2-flanged-cap-01": () => {
    const b = new B();
    const outer = b.cylAt("outer", 0, 0, 0, 30, 8);
    const bore = b.cylAt("bore", 0, 0, -1, 20, 10);
    const bolts = b.boltCircle("bolts", 0, 0, -1, 2.5, 10, 6, 25);
    return [b, b.diff("part", outer, b.union("cuts", bore, bolts))];
  },
  "a2-l-bracket-01": () => {
    const b = new B();
    const base = b.box("base", 0, 0, 0, 50, 30, 4);
    const wall = b.box("wall", 46, 0, 4, 4, 30, 26);
    const body = b.union("body", base, wall);
    const h1 = b.cylAt("h1", 10, 15, -1, 2.5, 6);
    const h2 = b.cylAt("h2", 35, 15, -1, 2.5, 6);
    return [b, b.diff("part", body, b.union("holes", h1, h2))];
  },
  "a2-mounting-rail-01": () => {
    const b = new B();
    const rail = b.centeredBox("rail", 100, 20, 8);
    const holes = [-40, 0, 40].map((x, i) => b.cylAt(`h_${i}`, x, 0, -1, 3, 10));
    return [b, b.diff("part", rail, b.unionAll("holes", holes))];
  },
  "a2-square-flange-01": () => {
    const b = new B();
    const plate = b.centeredBox("plate", 80, 80, 6);
    const center = b.cylAt("center", 0, 0, -1, 6, 8);
    const corners = [[30, 30], [-30, 30], [-30, -30], [30, -30]];
    const ch = corners.map(([x, y], i) => b.cylAt(`c_${i}`, x, y, -1, 3, 8));
    return [b, b.diff("part", plate, b.unionAll("cuts", [center, ...ch]))];
  },
  "a2-stepped-block-01": () => {
    const b = new B();
    const block = b.box("block", -25, -15, 0, 50, 30, 20);
    const cut = b.box("cut", 0, -15, 10, 25, 30, 11);
    return [b, b.diff("part", block, cut)];
  },
  "a2-stepped-pyramid-01": () => {
    const b = new B();
    const bot = b.centeredBox("bot", 40, 40, 10);
    const mid = b.box("mid", -14, -14, 10, 28, 28, 10);
    const top = b.box("top", -8, -8, 20, 16, 16, 10);
    return [b, b.unionAll("part", [bot, mid, top])];
  },
  "a2-tee-bracket-01": () => {
    const b = new B();
    const bar = b.box("bar", -30, 0, 0, 60, 20, 5);
    const stem = b.box("stem", -10, 20, 0, 20, 30, 5);
    const body = b.union("body", bar, stem);
    const holes = [[0, 45], [-25, 10], [25, 10]].map(([x, y], i) => b.cylAt(`h_${i}`, x, y, -1, 2.5, 7));
    return [b, b.diff("part", body, b.unionAll("holes", holes))];
  },
  "a2-washer-01": () => {
    const b = new B();
    const outer = b.cylAt("outer", 0, 0, 0, 25, 5);
    const bore = b.cylAt("bore", 0, 0, -1, 12.5, 7);
    return [b, b.diff("part", outer, bore)];
  },

  // ─── A3: rotations, n-gons, tangencies ───────────────────────────────
  "a3-cross-shaft-01": () => {
    const b = new B();
    const vert = b.cylAt("vert", 0, 0, 0, 10, 40);
    const horizRaw = b.cyl("horiz_raw", 10, 40, 64);
    const horizZ = b.tr("horiz_z", horizRaw, 0, 0, -20);
    const horizRot = b.rotY("horiz_rot", horizZ, 90);
    const horiz = b.tr("horiz", horizRot, 0, 0, 20);
    return [b, b.union("part", vert, horiz)];
  },
  "a3-hex-bolt-pattern-01": () => {
    const b = new B();
    const plate = b.centeredBox("plate", 60, 60, 8);
    const center = b.cylAt("center", 0, 0, -1, 2.5, 10);
    const ring = b.boltCircle("ring", 0, 0, -1, 2.5, 10, 6, 15);
    return [b, b.diff("part", plate, b.union("holes", center, ring))];
  },
  "a3-hex-nut-01": () => {
    const b = new B();
    // Hex, flat-to-flat 19 → circumradius 19/(2cos30°)=10.97, vertex on +X.
    const hex = b.prism("hex", 0, 0, 0, 10.97, 8, 6, 0);
    const bore = b.cylAt("bore", 0, 0, -1, 5, 10);
    return [b, b.diff("part", hex, bore)];
  },
  "a3-octagonal-flange-01": () => {
    const b = new B();
    // Octagon, apothem 20 → circumradius 21.65; rotate 22.5° for a flat top.
    const oct = b.prism("oct", 0, 0, 0, 21.65, 8, 8, 22.5);
    const bolts = b.boltCircle("bolts", 0, 0, -1, 2.5, 10, 8, 15);
    return [b, b.diff("part", oct, bolts)];
  },
  "a3-pentagonal-prism-01": () => {
    const b = new B();
    // Pentagon, circumradius 15, vertex pointing +Y → first vertex at 90°.
    const pent = b.prism("pent", 0, 0, 0, 15, 25, 5, 90);
    const bore = b.cylAt("bore", 5, 0, -1, 2.5, 27);
    return [b, b.diff("part", pent, bore)];
  },
  "a3-rotated-block-01": () => {
    const b = new B();
    const raw = b.box("raw", -20, -15, -10, 40, 30, 20);
    const rz = b.rotZ("rz", raw, 30);
    return [b, b.rotX("part", rz, 15)];
  },
  "a3-spherical-dome-block-01": () => {
    const b = new B();
    const cube = b.box("cube", -20, -20, 0, 40, 40, 40);
    const ball = b.sphere("ball_raw", 15);
    const dome = b.tr("dome", ball, 0, 0, 40);
    return [b, b.union("part", cube, dome)];
  },
  "a3-tangent-cylinders-01": () => {
    const b = new B();
    const big = b.cylAt("big", 0, 0, 0, 25, 40);
    const small = b.cylAt("small", 30, 0, 0, 5, 40);
    return [b, b.union("part", big, small)];
  },
  "a3-three-tangent-cylinders-01": () => {
    const b = new B();
    // Axes 20mm apart (r10 each), centroid at origin, one axis on +Y.
    const R = 20 / Math.sqrt(3); // circumradius of axis triangle
    const cyls = [90, 210, 330].map((deg, i) => {
      const t = (deg * Math.PI) / 180;
      return b.cylAt(`c_${i}`, R * Math.cos(t), R * Math.sin(t), 0, 10, 30);
    });
    return [b, b.unionAll("part", cyls)];
  },

  // ─── F: Fit suite — accessory mated with ghosted host ────────────────
  "f1-cap-tube-01": () => fitComposite(capTubeHost, (b) => {
    const register = b.cylAt("acc_register", 0, 0, 25, 11.85, 5);
    const flange = b.cylAt("acc_flange", 0, 0, 30, 17, 2);
    return b.union("acc_cap", register, flange);
  }),
  "f1-plug-port-01": () => fitComposite(plugPortHost, (b) =>
    b.cylAt("acc_plug", 25, 25, 5, 7.9, 10)),
  "f1-shim-gap-01": () => fitComposite(shimGapHost, (b) =>
    b.box("acc_shim", 1, 8, 6.1, 48, 24, 7.8)),
  "f1-spacer-shaft-01": () => fitComposite(steppedShaftHost, (b) => {
    const outer = b.cylAt("acc_outer", 0, 0, 10, 16, 30);
    const bore = b.cylAt("acc_bore", 0, 0, 9, 10.2, 32);
    return b.diff("acc_spacer", outer, bore);
  }),
  "f2-collar-shaft-axial-01": () => fitComposite(steppedShaftHost, (b) => {
    const outer = b.cylAt("acc_outer", 0, 0, 10, 16, 28);
    const bore = b.cylAt("acc_bore", 0, 0, 9, 10.2, 30);
    return b.diff("acc_collar", outer, bore);
  }),
  "f2-plug-port-tilted-01": () => fitComposite(plugPortHost, (b) =>
    b.cylAt("acc_plug", 25, 25, 5, 7.9, 10)),
  "f2-shim-gap-tilted-01": () => fitComposite(shimGapHost, (b) =>
    b.box("acc_shim", 1, 8, 6.1, 48, 24, 7.8)),
  "f2-spacer-shaft-sideways-01": () => fitComposite(steppedShaftHost, (b) => {
    const outer = b.cylAt("acc_outer", 0, 0, 10, 16, 30);
    const bore = b.cylAt("acc_bore", 0, 0, 9, 10.2, 32);
    return b.diff("acc_spacer", outer, bore);
  }),
  "f3-cap-snap-bottle-01": () => fitComposite(bottleNeckHost, (b) => {
    const body = b.cylAt("acc_body", 0, 0, 19, 14, 12);
    const mainBore = b.cylAt("acc_main_bore", 0, 0, 20.4, 12.25, 11);
    const lipBore = b.cylAt("acc_lip_bore", 0, 0, 18.9, 11.75, 1.6);
    return b.diff("acc_cap", body, b.union("acc_cavity", mainBore, lipBore));
  }),
  "f3-clip-pipe-01": () => fitComposite(pipeHost, (b) => {
    const outer = b.cylAt("acc_outer", 0, 0, 10, 13, 10);
    const bore = b.cylAt("acc_bore", 0, 0, 9, 8.3, 12);
    const ring = b.diff("acc_ring", outer, bore);
    const slot = b.box("acc_slot", 0, -2, 9, 14, 4, 12);
    return b.diff("acc_clip", ring, slot);
  }),
  "f4-collar-loaded-01": () => fitComposite(steppedShaftHost, (b) => {
    const outer = b.cylAt("acc_outer", 0, 0, 10, 16, 28);
    const bore = b.cylAt("acc_bore", 0, 0, 9, 10.2, 30);
    return b.diff("acc_collar", outer, bore);
  }),

  // ─── D: Visual — render the target shape itself ──────────────────────
  "d1-sphere-01": () => {
    const b = new B();
    return [b, b.sphere("part", 15)];
  },

  // ─── C: Mech — a static posed reacher arm (display only) ─────────────
  "c-reacher-01": () => {
    // Two-link arm posed reaching toward +X. Units are mm here purely for
    // the isometric thumbnail; this is a display reference, not graded.
    const b = new B();
    const base = b.cylAt("base", 0, 0, 0, 18, 20);
    const shoulder = b.cylAt("shoulder", 0, 0, 20, 10, 12);
    // link 1: rises and leans +X
    const l1raw = b.box("l1_raw", -6, -6, 0, 12, 12, 110);
    const l1rot = b.rotY("l1_rot", l1raw, 35);
    const l1 = b.tr("l1", l1rot, 0, 0, 26);
    // link 2: from the elbow out toward the target
    const l2raw = b.box("l2_raw", -5, -5, 0, 10, 10, 110);
    const l2rot = b.rotY("l2_rot", l2raw, 80);
    const l2 = b.tr("l2", l2rot, 63, 0, 116);
    // l2's far end lands at ~(171, 0, 135); seat the tip there so it reads
    // as one connected arm rather than a floating puck.
    const tip = b.cylAt("tip", 166, 0, 130, 6, 8);
    return [b, b.unionAll("part", [base, shoulder, l1, l2, tip])];
  },

  "a4-bolt-circle-flange-with-bore-01": () => {
    const b = new B();
    const flange = b.cylAt("flange", 0, 0, 0, 50, 12);
    const bore = b.cylAt("bore", 0, 0, 0, 20, 12);
    const bolts = b.boltCircle("bolts", 0, 0, 0, 4, 12, 6, 37.5);
    const cuts = b.union("cuts", bore, bolts);
    return [b, b.diff("part", flange, cuts)];
  },

  "a4-counterbore-plate-01": () => {
    const b = new B();
    const plate = b.box("plate", -30, -30, 0, 60, 60, 12);
    const corners = [[20, 20], [-20, 20], [-20, -20], [20, -20]];
    const cuts = corners.map(([x, y], i) => {
      const through = b.cylAt(`th_${i}`, x, y, 0, 2, 12);
      const cbore = b.cylAt(`cb_${i}`, x, y, 8, 4, 4);
      return b.union(`cut_${i}`, through, cbore);
    });
    return [b, b.diff("part", plate, b.unionAll("all_cuts", cuts))];
  },

  "a4-flanged-shaft-01": () => {
    const b = new B();
    const flange = b.cylAt("flange", 0, 0, 0, 40, 10);
    const shaft = b.cylAt("shaft", 0, 0, 10, 15, 50);
    const body = b.union("body", flange, shaft);
    const bore = b.cylAt("bore", 0, 0, 0, 7.5, 60);
    const bolts = b.boltCircle("bolts", 0, 0, 0, 3, 10, 4, 30);
    const cuts = b.union("cuts", bore, bolts);
    return [b, b.diff("part", body, cuts)];
  },

  "a4-rectangular-tube-01": () => {
    const b = new B();
    const outer = b.box("outer", -20, -20, 0, 40, 40, 80);
    const inner = b.box("inner", -14, -14, 0, 28, 28, 80);
    return [b, b.diff("part", outer, inner)];
  },

  "a4-rounded-bar-01": () => {
    const b = new B();
    const rect = b.box("rect", -30, -10, 0, 60, 20, 10);
    const left = b.cylAt("cap_l", -30, 0, 0, 10, 10);
    const right = b.cylAt("cap_r", 30, 0, 0, 10, 10);
    const half = b.union("rect_l", rect, left);
    return [b, b.union("part", half, right)];
  },

  "a4-slotted-bracket-01": () => {
    const b = new B();
    const plate = b.box("plate", -40, -20, 0, 80, 40, 8);
    const slotRect = b.box("slot_rect", -20, -5, 0, 40, 10, 8);
    const slotL = b.cylAt("slot_l", -20, 0, 0, 5, 8);
    const slotR = b.cylAt("slot_r", 20, 0, 0, 5, 8);
    const slot = b.union("slot", b.union("slot_lr", slotRect, slotL), slotR);
    return [b, b.diff("part", plate, slot)];
  },

  "a4-stepped-pyramid-with-holes-01": () => {
    const b = new B();
    const base = b.box("base", -40, -40, 0, 80, 80, 10);
    const mid = b.box("mid", -30, -30, 10, 60, 60, 10);
    const top = b.box("top", -20, -20, 20, 40, 40, 10);
    const stack = b.union("stack", b.union("base_mid", base, mid), top);
    const corners = [[30, 30], [-30, 30], [-30, -30], [30, -30]];
    const holes = corners.map(([x, y], i) => b.cylAt(`hole_${i}`, x, y, 0, 3, 10));
    return [b, b.diff("part", stack, b.unionAll("holes", holes))];
  },

  "a4-x-frame-01": () => {
    const b = new B();
    const a = b.box("a", -50, -10, 0, 100, 20, 10);
    const c = b.box("b", -10, -50, 0, 20, 100, 10);
    return [b, b.union("part", a, c)];
  },

  "a5-disc-hub-01": () => {
    const b = new B();
    const flange = b.cylAt("flange", 0, 0, 0, 50, 10);
    const hub = b.cylAt("hub", 0, 0, 10, 20, 20);
    const body = b.union("body", flange, hub);
    const bore = b.cylAt("bore", 0, 0, 0, 9, 30);
    const bolts = b.boltCircle("bolts", 0, 0, 0, 4, 10, 6, 37.5);
    const cuts = b.union("cuts", bore, bolts);
    return [b, b.diff("part", body, cuts)];
  },

  "a5-double-d-shaft-01": () => {
    const b = new B();
    const cyl = b.cylAt("cyl", 0, 0, 0, 20, 60);
    // Subtract the two flat slabs.
    const left = b.box("left", -25, -25, 0, 11, 50, 60);   // x: -25 to -14
    const right = b.box("right", 14, -25, 0, 11, 50, 60);  // x: 14 to 25
    const cuts = b.union("cuts", left, right);
    return [b, b.diff("part", cyl, cuts)];
  },

  "a5-hex-bolt-blank-01": () => {
    const b = new B();
    // Hex head: flat-to-flat 24mm → circumradius = 12 / cos(30°) ≈ 13.856.
    // The kernel Cylinder is analytic (renders round), so use the true
    // Prism primitive. Rotate 30° to put two flats parallel to X.
    const head = b.prism("head", 0, 0, 0, 12 / Math.cos(Math.PI / 6), 14, 6, 30);
    const shank = b.cylAt("shank", 0, 0, 14, 7, 46);
    return [b, b.union("part", head, shank)];
  },

  "a5-hollow-cap-01": () => {
    const b = new B();
    const outer = b.cylAt("outer", 0, 0, 0, 30, 50);
    const inner = b.cylAt("inner", 0, 0, 8, 25, 42);
    return [b, b.diff("part", outer, inner)];
  },

  "a5-lightened-disc-01": () => {
    const b = new B();
    const disc = b.cylAt("disc", 0, 0, 0, 60, 15);
    const bore = b.cylAt("bore", 0, 0, 0, 11, 15);
    const lightening = b.boltCircle("light", 0, 0, 0, 8, 15, 6, 38);
    const cuts = b.union("cuts", bore, lightening);
    return [b, b.diff("part", disc, cuts)];
  },

  "a5-ribbed-plate-01": () => {
    const b = new B();
    const plate = b.box("plate", -60, -40, 10, 120, 80, 10);
    const ribA = b.box("ribA", -60, -28, 0, 120, 8, 10);
    const ribB = b.box("ribB", -60, -4, 0, 120, 8, 10);
    const ribC = b.box("ribC", -60, 20, 0, 120, 8, 10);
    const body = b.unionAll("body", [plate, ribA, ribB, ribC]);
    const corners = [[45, 30], [-45, 30], [-45, -30], [45, -30]];
    const holes = corners.map(([x, y], i) => b.cylAt(`hole_${i}`, x, y, 0, 4, 20));
    return [b, b.diff("part", body, b.unionAll("holes", holes))];
  },

  "a5-stepped-boss-plate-01": () => {
    const b = new B();
    const plate = b.box("plate", -40, -30, 0, 80, 60, 10);
    const boss = b.box("boss", -20, -15, 10, 40, 30, 8);
    const body = b.union("body", plate, boss);
    const bore = b.cylAt("bore", 0, 0, 0, 9, 18);
    const corners = [[28, 22], [-28, 22], [-28, -22], [28, -22]];
    const holes = corners.map(([x, y], i) => b.cylAt(`hole_${i}`, x, y, 0, 3, 10));
    const cuts = b.union("cuts", bore, b.unionAll("bolts", holes));
    return [b, b.diff("part", body, cuts)];
  },

  "a5-u-bracket-01": () => {
    const b = new B();
    const base = b.box("base", -50, -15, 0, 100, 30, 8);
    const legL = b.box("legL", -50, -15, 8, 8, 30, 35);
    const legR = b.box("legR", 42, -15, 8, 8, 30, 35);
    const body = b.unionAll("body", [base, legL, legR]);
    const h1 = b.cylAt("h1", -25, 0, 0, 3, 8);
    const h2 = b.cylAt("h2", 25, 0, 0, 3, 8);
    return [b, b.diff("part", body, b.union("holes", h1, h2))];
  },

  "a6-compound-bore-ring-01": () => {
    const b = new B();
    const outer = b.cylAt("outer", 0, 0, 0, 40, 50);
    const lower = b.cylAt("lower", 0, 0, 0, 15, 25);
    const upper = b.cylAt("upper", 0, 0, 25, 25, 25);
    const bores = b.union("bores", lower, upper);
    return [b, b.diff("part", outer, bores)];
  },

  "a6-compound-boss-01": () => {
    const b = new B();
    const plate = b.box("plate", -60, -40, 0, 120, 80, 15);
    const bossL = b.cylAt("bossL", -30, 0, 15, 15, 20);
    const bossR = b.cylAt("bossR", 30, 0, 15, 15, 20);
    const body = b.unionAll("body", [plate, bossL, bossR]);
    const boreL = b.cylAt("boreL", -30, 0, 0, 8, 35);
    const boreR = b.cylAt("boreR", 30, 0, 0, 8, 35);
    const corners = [[50, 30], [-50, 30], [-50, -30], [50, -30]];
    const corner = corners.map(([x, y], i) => b.cylAt(`c_${i}`, x, y, 0, 4, 15));
    const cuts = b.unionAll("cuts", [boreL, boreR, ...corner]);
    return [b, b.diff("part", body, cuts)];
  },

  "a6-motor-flange-01": () => {
    const b = new B();
    const flange = b.cylAt("flange", 0, 0, 0, 60, 12);
    const spigot = b.cylAt("spigot", 0, 0, 12, 36, 6);
    const body = b.union("body", flange, spigot);
    const bore = b.cylAt("bore", 0, 0, 0, 16, 18);
    const bolts = b.boltCircle("bolts", 0, 0, 0, 4.5, 12, 4, 48);
    const cuts = b.union("cuts", bore, bolts);
    return [b, b.diff("part", body, cuts)];
  },

  "a6-pulley-01": () => {
    const b = new B();
    const bot = b.cylAt("bot", 0, 0, 0, 40, 8);
    const groove = b.cylAt("groove", 0, 0, 8, 25, 24);
    const top = b.cylAt("top", 0, 0, 32, 40, 8);
    const body = b.unionAll("body", [bot, groove, top]);
    const bore = b.cylAt("bore", 0, 0, 0, 8, 40);
    return [b, b.diff("part", body, bore)];
  },

  "a6-sprocket-blank-01": () => {
    const b = new B();
    const disc = b.cylAt("disc", 0, 0, 0, 50, 20);
    const hub = b.cylAt("hub", 0, 0, 20, 20, 20);
    const body = b.union("body", disc, hub);
    const bore = b.cylAt("bore", 0, 0, 0, 10, 40);
    // Keyway: rectangular slot from y=10 outward. 6mm wide, 6mm deep.
    const keyway = b.box("keyway", -3, 10, 0, 6, 6, 40);
    const corners = [[35, 0], [0, 35], [-35, 0], [0, -35]];
    const holes = corners.map(([x, y], i) => b.cylAt(`bolt_${i}`, x, y, 0, 4, 20));
    const cuts = b.unionAll("cuts", [bore, keyway, ...holes]);
    return [b, b.diff("part", body, cuts)];
  },

  "a6-yoke-block-01": () => {
    const b = new B();
    const base = b.box("base", -40, -20, 0, 80, 40, 12);
    const tineL = b.box("tineL", -40, -20, 12, 14, 40, 45);
    const tineR = b.box("tineR", 26, -20, 12, 14, 40, 45);
    const body = b.unionAll("body", [base, tineL, tineR]);
    // Cross-bore: cylinder with axis along X. Build cylinder along Z then rotate.
    const cb = b.cyl("cb_raw", 9, 80, 64);          // diameter 18mm, length 80mm
    const cbAtZ0 = b.tr("cb_z0", cb, 0, 0, -40);     // place along Z, centered
    const cbRot = b.rotY("cb_rot", cbAtZ0, 90);     // rotate to lie along X
    const cbPos = b.tr("cross_bore", cbRot, 0, 0, 44.5);
    return [b, b.diff("part", body, cbPos)];
  },
};

// ─── main ─────────────────────────────────────────────────────────────

let written = 0;
for (const [id, fn] of Object.entries(TASKS)) {
  const [b, rootOrRoots, materials] = fn();
  // Single-root tasks return a numeric root id; Fit composites return an
  // array of { root, material } plus a materials map.
  const out = Array.isArray(rootOrRoots)
    ? b.serializeMulti(rootOrRoots, materials)
    : b.serialize(rootOrRoots);
  writeFileSync(resolve(TASKS_DIR, `${id}.vcad`), out, "utf8");
  console.log(`wrote ${id}.vcad`);
  written++;
}
console.log(`done: ${written} task refs`);
