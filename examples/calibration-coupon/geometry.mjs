/**
 * The vcad calibration coupon — a single printable plate that spans the FDM
 * failure modes worth measuring: XY spans, Z heights at several altitudes,
 * hole undersize across sizes, boss OD, thin-wall flow, and mass.
 *
 * This file is the single source of truth: the SAME `PARAMS` build both the
 * document geometry and the declared measurables, so the prediction and the
 * part cannot drift apart (the f405-enclosure pattern).
 *
 * Geometry rules (kernel-friendliness): rectilinear unions with every added
 * body sunk `PARAMS.sink` into the base (no coincident-face union seams),
 * plain cylinder differences through a flat plate — the best-tested boolean
 * path in the kernel. No fillets, no text.
 */

export const PARAMS = {
  base: { x: 80, y: 32, z: 4 },
  /** How deep added bodies embed into the base (avoids coincident faces). */
  sink: 1,
  /** Step heights above the base top; tops land at base.z + h. */
  stepHeights: [2, 4, 6, 8],
  step: { width: 12, depth: 12, y: 18, x0: 4, pitch: 16 },
  holes: [
    { d: 3, x: 10, y: 8 },
    { d: 5, x: 24, y: 8 },
    { d: 8, x: 40, y: 8 },
  ],
  boss: { d: 6, height: 6, x: 72, y: 24 },
  fins: [
    { t: 0.8, x: 56 },
    { t: 1.2, x: 64 },
    { t: 2.0, x: 72 },
  ],
  fin: { length: 10, height: 8, y: 3 },
  segments: 64,
  /** PLA. Print the coupon at 100% infill or ignore the mass row. */
  density_kg_m3: 1240,
};

const fmt = (n) => String(n).replace(".", "_");

/** Build the coupon as a vcad IR document (one solid part). */
export function couponDocument() {
  const p = PARAMS;
  const nodes = {};
  let nextId = 1;
  const add = (name, op) => {
    const id = nextId++;
    nodes[String(id)] = { id, name, op };
    return id;
  };

  let solid = add("Base plate", {
    type: "Cube",
    size: { x: p.base.x, y: p.base.y, z: p.base.z },
  });
  const union = (id, name) =>
    (solid = add(name, { type: "Union", left: solid, right: id }));

  // Staircase along the back row — Z accuracy at several heights.
  p.stepHeights.forEach((h, i) => {
    const cube = add(`Step ${i + 1} body`, {
      type: "Cube",
      size: { x: p.step.width, y: p.step.depth, z: h + p.sink },
    });
    const placed = add(`Step ${i + 1}`, {
      type: "Translate",
      child: cube,
      offset: { x: p.step.x0 + i * p.step.pitch, y: p.step.y, z: p.base.z - p.sink },
    });
    union(placed, `+ step ${i + 1}`);
  });

  // Boss — outer diameter, the mirror image of the holes.
  const bossBody = add("Boss body", {
    type: "Cylinder",
    radius: p.boss.d / 2,
    height: p.boss.height + p.sink,
    segments: p.segments,
  });
  const boss = add("Boss", {
    type: "Translate",
    child: bossBody,
    offset: { x: p.boss.x, y: p.boss.y, z: p.base.z - p.sink },
  });
  union(boss, "+ boss");

  // Thin fins — wall accuracy / flow.
  p.fins.forEach((f, i) => {
    const wall = add(`Fin ${f.t}mm body`, {
      type: "Cube",
      size: { x: f.t, y: p.fin.length, z: p.fin.height + p.sink },
    });
    const placed = add(`Fin ${f.t}mm`, {
      type: "Translate",
      child: wall,
      offset: { x: f.x - f.t / 2, y: p.fin.y, z: p.base.z - p.sink },
    });
    union(placed, `+ fin ${i + 1}`);
  });

  // Through-holes — drilled last, through the finished union.
  for (const h of p.holes) {
    const drill = add(`Hole Ø${h.d} body`, {
      type: "Cylinder",
      radius: h.d / 2,
      height: p.base.z + 2 * p.sink,
      segments: p.segments,
    });
    const placed = add(`Hole Ø${h.d}`, {
      type: "Translate",
      child: drill,
      offset: { x: h.x, y: h.y, z: -p.sink },
    });
    solid = add(`− hole Ø${h.d}`, { type: "Difference", left: solid, right: placed });
  }

  nodes[String(solid)].name = "Calibration coupon";

  return {
    version: "0.1",
    nodes,
    materials: {
      pla: {
        name: "pla",
        color: [0.9, 0.55, 0.15],
        metallic: 0.0,
        roughness: 0.6,
        density: p.density_kg_m3,
      },
    },
    part_materials: {},
    roots: [{ root: solid, material: "pla" }],
  };
}

/** The declared measurables — derived from the same PARAMS as the geometry.
 *  (bbox_x/y/z and mass are added automatically by predict_print.) */
export function measurables() {
  const p = PARAMS;
  return [
    ...p.stepHeights.map((h, i) => ({
      id: `step_z_${p.base.z + h}`,
      label: `Step ${i + 1} top height off the bed (caliper as depth/height gauge)`,
      kind: "dimension",
      axis: "Z",
      feature: "step",
      predicted: p.base.z + h,
    })),
    ...p.holes.map((h) => ({
      id: `hole_${fmt(h.d)}mm`,
      label: `Ø${h.d} through-hole diameter (caliper inside jaws, front row)`,
      kind: "diameter",
      axis: "XY",
      feature: "hole",
      predicted: h.d,
    })),
    {
      id: `boss_${fmt(PARAMS.boss.d)}mm`,
      label: `Ø${p.boss.d} boss outer diameter (caliper outside jaws, back-right)`,
      kind: "diameter",
      axis: "XY",
      feature: "boss",
      predicted: p.boss.d,
    },
    ...p.fins.map((f) => ({
      id: `fin_${fmt(f.t)}`,
      label: `${f.t}mm fin wall thickness (caliper outside jaws, right edge)`,
      kind: "dimension",
      axis: "X",
      feature: "wall",
      predicted: f.t,
    })),
  ];
}
