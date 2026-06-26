/**
 * Geometry for the F405 flight-controller co-design showcase.
 *
 * One source of truth for the 3D-printable case, shared by the showcase script
 * (`build.mjs`) and the cross-domain verification test
 * (`packages/mcp/src/__tests__/enclosure-fit.test.ts`). Plain ESM data — no
 * dependencies — so both a Node runner and vitest can import it.
 *
 * Frame: Z-up, mm. The case is an open-top tray: 42×42 outer, 2 mm walls and
 * floor, four standoffs on the iconic 30.5 mm M3 pattern (post tops at z=5,
 * where the board lands), and a USB-C cutout in the +X wall. A 36×36 board
 * centered in the 38×38 cavity has 1 mm clearance and its holes drop onto the
 * standoffs.
 */

/** Tunable case + board parameters (mm). */
export const PARAMS = {
  outer: { x: 42, y: 42, z: 16 },
  wall: 2,
  floor: 2,
  /** Board edge length (square). */
  board: 36,
  boardThickness: 1.6,
  /** M3 mounting-hole pattern pitch (the FC-standard 30.5 mm). */
  holePitch: 30.5,
  standoff: { radius: 2.5, height: 3, drill: 1.6 },
  /** USB-C cutout in the +X wall. */
  usb: { width: 12, height: 6 },
};

/** Center of the cavity in world XY (and of the hole pattern). */
export function caseCenter() {
  return { x: PARAMS.outer.x / 2, y: PARAMS.outer.y / 2 };
}

/** World standoff centers — the 30.5 mm pattern about the cavity center. */
export function standoffCenters() {
  const c = caseCenter();
  const h = PARAMS.holePitch / 2;
  return [
    { x: c.x - h, y: c.y - h },
    { x: c.x + h, y: c.y - h },
    { x: c.x - h, y: c.y + h },
    { x: c.x + h, y: c.y + h },
  ];
}

/** Board-local mounting-hole centers (board origin-corner, 0..board). */
export function boardHoleCenters() {
  const m = (PARAMS.board - PARAMS.holePitch) / 2; // margin from edge
  const a = m;
  const b = PARAMS.board - m;
  return [
    { x: a, y: a },
    { x: b, y: a },
    { x: a, y: b },
    { x: b, y: b },
  ];
}

/** Board-local position of the USB-C connector (centered on the +X edge). */
export function usbConnectorLocal() {
  return { x: PARAMS.board, y: PARAMS.board / 2 };
}

/**
 * Build the enclosure as a vcad IR Document.
 *
 * Standoffs are modeled SUBTRACTIVELY: the four post columns are carved out of
 * the interior pocket *before* the pocket is subtracted from the outer box, so
 * each post is left behind as part of the same continuous solid as the floor —
 * one Difference, no Union seam. (A union of touching posts onto the floor
 * leaves a degenerate coincident face the mesh boolean can't resolve, which
 * breaks any downstream inside/outside test.) Net shape:
 *
 *   case = (outer − (pocket − post₀ − post₁ − post₂ − post₃)) − usb_cutout
 */
export function f405CaseDocument() {
  const { outer, wall, floor, standoff, usb } = PARAMS;
  const innerW = outer.x - 2 * wall;
  const innerD = outer.y - 2 * wall;
  const c = caseCenter();

  const nodes = {};
  let id = 0;
  const add = (name, op) => {
    id += 1;
    nodes[String(id)] = { id, name, op };
    return id;
  };

  const outerBox = add("outer", { type: "Cube", size: { x: outer.x, y: outer.y, z: outer.z } });

  // Interior void (open top: rises past the rim).
  const pocketBox = add("pocket-solid", {
    type: "Cube",
    size: { x: innerW, y: innerD, z: outer.z },
  });
  let pocket = add("pocket", {
    type: "Translate",
    child: pocketBox,
    offset: { x: wall, y: wall, z: floor },
  });

  // Carve the post columns out of the pocket — what remains as solid is a boss
  // rising from the floor to (floor + height). One Cylinder node, reused.
  const postCyl = add("post", {
    type: "Cylinder",
    radius: standoff.radius,
    height: standoff.height,
    segments: 32,
  });
  for (const [i, s] of standoffCenters().entries()) {
    const p = add(`post-${i}`, {
      type: "Translate",
      child: postCyl,
      offset: { x: s.x, y: s.y, z: floor },
    });
    pocket = add(`pocket-${i}`, { type: "Difference", left: pocket, right: p });
  }

  let body = add("tray", { type: "Difference", left: outerBox, right: pocket });

  // USB-C cutout through the +X wall, centered on Y.
  const usbBox = add("usb-solid", {
    type: "Cube",
    size: { x: wall + 2, y: usb.width, z: usb.height },
  });
  const usbT = add("usb-cutout", {
    type: "Translate",
    child: usbBox,
    offset: { x: outer.x - wall - 1, y: c.y - usb.width / 2, z: floor + 1 },
  });
  body = add("case", { type: "Difference", left: body, right: usbT });

  return {
    version: "0.1",
    nodes,
    materials: {},
    part_materials: {},
    roots: [{ root: body, material: "abs-black" }],
  };
}
