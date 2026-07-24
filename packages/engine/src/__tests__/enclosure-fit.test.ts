import { describe, it, expect, beforeAll } from "vitest";
import type { BoardOutline, Pcb } from "@vcad/ir";
import { getKernelWasm } from "../wasm-singleton.js";
import {
  checkEnclosureFit,
  deriveBoardFromCavity,
  mountingHolesFromPcb,
  connectorsFromPcb,
  componentExtentsFromMeshes,
  type EnclosureCavity,
  type Standoff,
  type WallOpening,
  type ComponentExtent,
} from "../enclosure-fit.js";
import { extractEnclosureFeatures } from "../enclosure-mesh.js";

// The verification core lives in Rust (crates/vcad-kernel-enclosure); these
// tests exercise it through the WASM bridge, so the module must be up first.
beforeAll(async () => {
  await getKernelWasm();
});

// ---------------------------------------------------------------------------
// Synthetic mesh helpers — union of disjoint axis-aligned boxes (coincident
// faces are fine; the extractor collapses them).
// ---------------------------------------------------------------------------

function box(min: [number, number, number], max: [number, number, number]) {
  const [x0, y0, z0] = min;
  const [x1, y1, z1] = max;
  const v = [
    [x0, y0, z0],
    [x1, y0, z0],
    [x1, y1, z0],
    [x0, y1, z0],
    [x0, y0, z1],
    [x1, y0, z1],
    [x1, y1, z1],
    [x0, y1, z1],
  ];
  const faces = [
    [0, 1, 2],
    [0, 2, 3], // bottom
    [4, 6, 5],
    [4, 7, 6], // top
    [0, 4, 5],
    [0, 5, 1], // -Y
    [3, 2, 6],
    [3, 6, 7], // +Y
    [0, 3, 7],
    [0, 7, 4], // -X
    [1, 5, 6],
    [1, 6, 2], // +X
  ];
  return { positions: v.flat(), indices: faces.flat() };
}

function merge(boxes: Array<{ positions: number[]; indices: number[] }>) {
  const positions: number[] = [];
  const indices: number[] = [];
  let off = 0;
  for (const b of boxes) {
    positions.push(...b.positions);
    for (const i of b.indices) indices.push(i + off);
    off += b.positions.length / 3;
  }
  return { positions, indices };
}

/**
 * An open-top tray: 40×40×12 outer, 2mm walls, 2mm floor, four M3 standoffs on
 * a 30.5mm pattern (posts top at z=5), and a 10mm full-height USB cutout in the
 * +X wall centered at y=20.
 */
function trayMesh() {
  const W = 40,
    D = 40,
    H = 12,
    t = 2,
    fz = 2;
  const post = (cx: number, cy: number) => box([cx - 1.5, cy - 1.5, fz], [cx + 1.5, cy + 1.5, 5]);
  const c = 20;
  const half = 30.5 / 2; // 15.25
  return merge([
    box([0, 0, 0], [W, D, fz]), // floor
    box([0, 0, fz], [t, D, H]), // -X wall
    box([W - t, 0, fz], [W, 15, H]), // +X wall, lower
    box([W - t, 25, fz], [W, D, H]), // +X wall, upper (cutout y 15..25)
    box([t, 0, fz], [W - t, t, H]), // -Y wall
    box([t, D - t, fz], [W - t, D, H]), // +Y wall
    post(c - half, c - half),
    post(c + half, c - half),
    post(c - half, c + half),
    post(c + half, c + half),
  ]);
}

// ---------------------------------------------------------------------------
// Mesh extraction
// ---------------------------------------------------------------------------

describe("extractEnclosureFeatures", () => {
  it("finds the cavity, four standoffs, and the wall cutout of a tray", () => {
    const mesh = trayMesh();
    const f = extractEnclosureFeatures(mesh.positions, mesh.indices);

    expect(f.outer.maxZ).toBeCloseTo(12, 1);
    expect(f.cavity).not.toBeNull();
    const cav = f.cavity!;
    // Within ~1 grid cell of the true [2,38] pocket (GWN samples cell centers).
    const near = (got: number, want: number) => Math.abs(got - want) <= 1.2;
    expect(near(cav.minX, 2)).toBe(true);
    expect(near(cav.maxX, 38)).toBe(true);
    expect(near(cav.minY, 2)).toBe(true);
    expect(near(cav.maxY, 38)).toBe(true);
    expect(cav.floorZ).toBeCloseTo(2, 0);
    expect(cav.ceilZ).toBeCloseTo(12, 0); // open top → rim height
    expect(cav.hasLid).toBe(false);

    expect(f.standoffs.length).toBe(4);
    for (const s of f.standoffs) expect(s.topZ).toBeCloseTo(5, 0);
    // The 30.5mm pattern centers (±15.25 about 20).
    const xs = f.standoffs.map((s) => Math.round(s.x)).sort((a, b) => a - b);
    expect(xs[0]).toBeGreaterThanOrEqual(4);
    expect(xs[0]).toBeLessThanOrEqual(6);

    expect(f.openings.length).toBe(1);
    expect(f.openings[0].edge).toBe("maxX");
    expect(f.openings[0].center.y).toBeCloseTo(20, 0);
    expect(f.openings[0].width).toBeGreaterThan(8);
    expect(f.openings[0].width).toBeLessThan(12);
  });

  it("returns a null cavity for a solid block", () => {
    const mesh = box([0, 0, 0], [20, 20, 10]);
    const f = extractEnclosureFeatures(mesh.positions, mesh.indices);
    expect(f.cavity).toBeNull();
    expect(f.standoffs).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Pure verification core
// ---------------------------------------------------------------------------

const cavity: EnclosureCavity = {
  minX: 1,
  maxX: 39,
  minY: 1,
  maxY: 39,
  floorZ: 2,
  ceilZ: 12,
  hasLid: false,
};
const standoffs: Standoff[] = [
  { x: 4.75, y: 4.75, topZ: 5, radius: 1.6 },
  { x: 35.25, y: 4.75, topZ: 5, radius: 1.6 },
  { x: 4.75, y: 35.25, topZ: 5, radius: 1.6 },
  { x: 35.25, y: 35.25, topZ: 5, radius: 1.6 },
];
const openings: WallOpening[] = [
  { edge: "maxX", center: { x: 38, y: 20 }, width: 10, zMin: 2, zMax: 12 },
];

// 36×36 board (cavity 36×36 inset 0 — board fills the floor on the standoffs).
const outline: BoardOutline = {
  vertices: [
    { x: 0, y: 0 },
    { x: 36, y: 0 },
    { x: 36, y: 36 },
    { x: 0, y: 36 },
  ],
  thickness: 1.6,
};
// Holes at the 30.5 pattern, board-local (board origin lands at cavity.min+offset).
const mountingHoles = [
  { x: 2.75, y: 2.75, diameter: 3.2 },
  { x: 33.25, y: 2.75, diameter: 3.2 },
  { x: 2.75, y: 33.25, diameter: 3.2 },
  { x: 33.25, y: 33.25, diameter: 3.2 },
];
// Board rests on the standoffs (z=5). offset.x/y = 1 so cavity.min(2)+? — board
// origin at cavity.minX + (36-36)/2 - 0 = 2. Holes at 2.75 → world 4.75 ✓.
const placement = { offset: { x: 2, y: 2, z: 5 }, rotationDeg: 0 };

const componentExtents: ComponentExtent[] = [
  { ref: "U1", front: true, topZ: 1.6 + 1.0, bottomZ: 1.6 }, // QFN, 1mm tall
  { ref: "J1", front: true, topZ: 1.6 + 3.0, bottomZ: 1.6 }, // USB, 3mm tall
];
const connectors = [{ ref: "J1", x: 36, y: 18, edge: "maxX" as const, height: 3 }];

describe("checkEnclosureFit", () => {
  it("passes a board that fits, clears the lid, and lands on standoffs", () => {
    const r = checkEnclosureFit({
      outline,
      cavity,
      standoffs,
      openings,
      mountingHoles,
      connectors,
      componentExtents,
      placement,
      clearance: 0.5,
    });
    expect(r.ok).toBe(true);
    expect(r.verified).toBe(true);
    expect(r.summary).toMatch(/PASS/);
    const byId = Object.fromEntries(r.checks.map((c) => [c.id, c]));
    expect(byId.board_fit.status).toBe("pass");
    expect(byId.lid_clearance.status).toBe("pass");
    expect(byId.mounting_holes.status).toBe("pass");
    expect(byId.connector_cutouts.status).toBe("pass");
    expect(byId.mounting_holes.measurements!.holes_matched).toBe(4);
  });

  it("fails when the board overhangs the cavity", () => {
    const big: BoardOutline = { ...outline, vertices: outline.vertices.map((v) => ({ x: v.x * 1.2, y: v.y * 1.2 })) };
    const r = checkEnclosureFit({ outline: big, cavity, placement, clearance: 0.5 });
    const fit = r.checks.find((c) => c.id === "board_fit")!;
    expect(fit.status).toBe("fail");
    expect(r.ok).toBe(false);
    expect(fit.detail).toMatch(/overhangs/);
  });

  it("fails when a tall component punches through the lid", () => {
    const tall: ComponentExtent[] = [{ ref: "C1", front: true, topZ: 1.6 + 12, bottomZ: 1.6 }];
    const r = checkEnclosureFit({
      outline,
      cavity,
      placement,
      componentExtents: tall,
      clearance: 0.5,
    });
    const lid = r.checks.find((c) => c.id === "lid_clearance")!;
    expect(lid.status).toBe("fail");
    expect(lid.detail).toMatch(/C1/);
  });

  it("fails when a mounting hole misses every standoff", () => {
    const offHoles = [{ x: 10, y: 10, diameter: 3.2, ref: "H1" }];
    const r = checkEnclosureFit({ outline, cavity, standoffs, mountingHoles: offHoles, placement });
    const mh = r.checks.find((c) => c.id === "mounting_holes")!;
    expect(mh.status).toBe("fail");
    expect(mh.measurements!.holes_matched).toBe(0);
  });

  it("warns when a connector has no wall cutout at all", () => {
    const r = checkEnclosureFit({ outline, cavity, connectors, openings: [], placement });
    const cc = r.checks.find((c) => c.id === "connector_cutouts")!;
    expect(cc.status).toBe("warn");
    expect(r.verified).toBe(false);
    expect(r.ok).toBe(true); // a warning is not a hard failure
  });

  it("skips checks whose inputs are absent", () => {
    const r = checkEnclosureFit({ outline, cavity, placement });
    expect(r.checks.find((c) => c.id === "lid_clearance")!.status).toBe("skip");
    expect(r.checks.find((c) => c.id === "mounting_holes")!.status).toBe("skip");
    expect(r.checks.find((c) => c.id === "connector_cutouts")!.status).toBe("skip");
  });

  it("auto-fits the board centered on the standoffs when no placement is given", () => {
    const r = checkEnclosureFit({ outline, cavity, standoffs, mountingHoles });
    expect(r.placement.offset.z).toBeCloseTo(5, 1); // on the posts
    expect(r.checks.find((c) => c.id === "board_fit")!.status).toBe("pass");
  });
});

// ---------------------------------------------------------------------------
// Derive + board feature extraction
// ---------------------------------------------------------------------------

describe("deriveBoardFromCavity", () => {
  it("insets the outline and places a hole on every standoff", () => {
    const d = deriveBoardFromCavity(cavity, standoffs, { clearance: 0.5, holeDiameter: 3.2 });
    expect(d.outline.vertices[2].x).toBeCloseTo(37, 1); // 38 - 2*0.5
    expect(d.mountingHoles.length).toBe(4);
    // Round-trips: the derived board verifies against the same cavity.
    const r = checkEnclosureFit({
      outline: d.outline,
      cavity,
      standoffs,
      mountingHoles: d.mountingHoles,
      placement: d.placement,
      clearance: 0.5,
    });
    expect(r.checks.find((c) => c.id === "board_fit")!.status).toBe("pass");
    expect(r.checks.find((c) => c.id === "mounting_holes")!.status).toBe("pass");
  });
});

describe("board feature extraction", () => {
  const pcb: Pcb = {
    outline,
    stackup: { layers: [] } as unknown as Pcb["stackup"],
    nets: [],
    rules: {} as unknown as Pcb["rules"],
    footprints: [
      {
        ref: "H1",
        value: "M3",
        footprintName: "MountingHole_3.2mm_M3",
        position: { x: 2.75, y: 2.75 },
        pads: [
          {
            number: "1",
            padType: "NPTH",
            shape: { type: "Circle", diameter: 3.2 },
            position: { x: 0, y: 0 },
            layers: [],
            drill: { diameter: 3.2 } as unknown as never,
          },
        ],
      },
      {
        ref: "J1",
        value: "USB-C",
        footprintName: "USB_C_Receptacle",
        position: { x: 36, y: 18 },
        pads: [],
      },
      {
        ref: "U1",
        value: "STM32F405",
        footprintName: "QFN-48",
        position: { x: 18, y: 18 },
        pads: [],
      },
    ] as unknown as Pcb["footprints"],
    traces: [],
    vias: [],
    zones: [],
  };

  it("pulls mounting holes from MountingHole footprints / NPTH pads", () => {
    const holes = mountingHolesFromPcb(pcb);
    expect(holes.length).toBe(1);
    expect(holes[0].ref).toBe("H1");
    expect(holes[0].diameter).toBeCloseTo(3.2, 1);
  });

  it("identifies connectors but not the MCU", () => {
    const conns = connectorsFromPcb(pcb, outline);
    expect(conns.map((c) => c.ref)).toEqual(["J1"]);
    expect(conns[0].edge).toBe("maxX");
  });

  it("derives component Z extents from kernel meshes", () => {
    const meshes = [{ footprint_ref: "U1", positions: [0, 0, 1.6, 1, 1, 2.6, 2, 2, 1.6] }];
    const ext = componentExtentsFromMeshes(meshes, pcb);
    expect(ext[0].topZ).toBeCloseTo(2.6, 1);
    expect(ext[0].bottomZ).toBeCloseTo(1.6, 1);
    expect(ext[0].front).toBe(true);
  });
});
