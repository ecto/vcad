// Generate higher-fidelity reference .vcad files for the matrix's
// "expected" column. One function per task — each authored from the
// task spec's prompt to capture the actual geometry, not just a bbox
// silhouette. The matrix renders these via vcad-render.
//
// Run: `node mecheval/leaderboard/scripts/gen-task-refs.mjs`

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
  tr(name, child, x, y, z) { return this.add(name, { type: "Translate", child, offset: { x, y, z } }); }
  rotZ(name, child, deg) { return this.add(name, { type: "Rotate", child, angles: { x: 0, y: 0, z: deg } }); }
  rotY(name, child, deg) { return this.add(name, { type: "Rotate", child, angles: { x: 0, y: deg, z: 0 } }); }
  rotX(name, child, deg) { return this.add(name, { type: "Rotate", child, angles: { x: deg, y: 0, z: 0 } }); }
  union(name, left, right) { return this.add(name, { type: "Union", left, right }); }
  diff(name, left, right) { return this.add(name, { type: "Difference", left, right }); }
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
  serialize(rootId) {
    return JSON.stringify({
      version: "0.1",
      nodes: this.nodes,
      materials: {},
      part_materials: {},
      roots: [{ root: rootId, material: "default" }],
    }, null, 2) + "\n";
  }
}

// ─── per-task authoring ───────────────────────────────────────────────

const TASKS = {
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
    // Hex prism: cylinder with 6 segments, flat-to-flat 24mm → circumradius = 12 / cos(30°) ≈ 13.856.
    // vcad Cylinder is faceted, so segments=6 produces a hex. Rotate so two flats are parallel to X.
    const hexRaw = b.add("hex_raw", { type: "Cylinder", radius: 12 / Math.cos(Math.PI / 6), height: 14, segments: 6 });
    const hexRot = b.rotZ("hex_rot", hexRaw, 30); // align flats with X axis
    const head = b.tr("head", hexRot, 0, 0, 0);
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
  const [b, root] = fn();
  const out = b.serialize(root);
  writeFileSync(resolve(TASKS_DIR, `${id}.vcad`), out, "utf8");
  console.log(`wrote ${id}.vcad`);
  written++;
}
console.log(`done: ${written} task refs`);
