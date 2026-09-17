/**
 * The `cam` tool pack, driven through the real server dispatch.
 *
 * The rule these exist to defend is the one the whole CAM roadmap serves: the
 * system has to be able to say no by itself. So the load-bearing test here is
 * not that a job comes back — it is that a job the oracle refuses comes back
 * with **no G-code anywhere in the result**, which cannot be checked at the
 * handler level because result slimming, structuredContent and the preview
 * handle all run afterwards and all carry text.
 *
 * Everything else asserts geometry: measured diameters, a measured
 * disagreement between sections, which check objected and why.
 *
 * These need a kernel WASM that exports the CAM bindings. The checked-in
 * artifact refreshes on `main`; CI's TypeScript job builds it from source. On
 * a stale local artifact the suite skips rather than failing, the same
 * convention `packages/engine`'s wasm tests follow.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { createServer } from "../server.js";
import { documents, getSession } from "../tools/session.js";

interface ToolCallResult {
  content: Array<{ type: string; text: string }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
}

type Json = Record<string, unknown>;

// Top-level await, not `beforeAll`: `describe.skipIf` is evaluated while the
// file is being collected, which is before any hook has run. A gate read in
// `beforeAll` is always still `false` when it is asked, and the whole suite
// skips silently — which is worse than failing.
const engine = await Engine.init();
const hasCam =
  typeof (engine as unknown as { kernel: Json }).kernel.camJob === "function";
if (!hasCam) {
  console.warn(
    "[cam.test] the loaded kernel WASM predates the CAM bindings — skipping. Rebuild with `node packages/kernel-wasm/scripts/build.mjs`.",
  );
}

async function connect() {
  const server = await createServer(engine, { user: null });
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "test", version: "0.0.0" }, { capabilities: {} });
  await Promise.all([client.connect(clientT), server.connect(serverT)]);
  return { client, server };
}

/** The JSON object a text-only host would parse out of the result. */
function bodyOf(result: ToolCallResult): Json {
  const merged: Json = {};
  for (const block of result.content) {
    if (block.type !== "text") continue;
    try {
      const parsed = JSON.parse(block.text) as unknown;
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        Object.assign(merged, parsed);
      }
    } catch {
      // prose block
    }
  }
  return merged;
}

/**
 * An 80 × 50 × 6 plate with a Ø16 bore in the middle and two Ø5 holes.
 * Booleans, so the export-mesh repair has something to do — which is the
 * reason `cam_outline` must not be sectioning the export mesh.
 *
 * The three tools are unioned and cut in **one** Difference rather than
 * chained one after another, and that is not stylistic: a chain of Difference
 * nodes reaches `vcad-eval`'s boolean batching, which budgets itself with
 * `std::time::Instant::now` — not implemented on wasm32, so it panics and
 * traps the module. `evaluateDocument` hides that by falling back to
 * evaluating in TypeScript; sectioning cannot, because the B-rep it needs
 * only exists on the kernel side. See the trap guard in tools/cam.ts.
 */
const PLATE = `
[let plate [cube 80 50 6]]
[let bore [translate 40 25 -1 [cylinder-n 8 8 96]]]
[let hole-a [translate 12 12 -1 [cylinder-n 2.5 8 64]]]
[let hole-b [translate 68 38 -1 [cylinder-n 2.5 8 64]]]
[difference [union hole-b [union hole-a bore]] plate]
`;

/** A cone: every Z plane cuts a different circle, so it is not prismatic. */
const CONE = "[cone-n 15 0 20 96]";

async function makeDoc(client: Client, source: string): Promise<string> {
  const created = (await client.callTool({
    name: "create_cad_loon",
    arguments: { source },
  })) as ToolCallResult;
  expect(created.isError, JSON.stringify(created.content)).toBeFalsy();
  const id = bodyOf(created).document_id as string;
  expect(typeof id).toBe("string");
  return id;
}

/** The Ø3 two-flute the plate's job is cut with. */
const D3 = {
  number: 1,
  kind: "flat_end_mill",
  diameter: 3.0,
  flutes: 2,
  flute_length: 12.0,
  centre_cutting: true,
};

/**
 * The plate job: bore and both small holes from inside, profile with three
 * tabs last. `thickness` and `spoilboard` are the dials the blocked case
 * turns.
 */
function plateJob(
  documentId: string,
  thickness: number,
  spoilboard: number | null,
): Json {
  const stock: Json = { thickness, margin: 3.0 };
  if (spoilboard !== null) stock.spoilboard = spoilboard;
  return {
    document_id: documentId,
    name: "plate",
    stock,
    machine: { name: "Anolex Ultra 2", spindle: "dial" },
    tools: [D3],
    operations: [
      {
        // A pocket, not a helix: a Ø3 cutter helixing a Ø16 hole would leave
        // a Ø10 core standing in the middle, and the kernel says so rather
        // than cutting a ring. The contour is named, not spelled out.
        name: "bore",
        tool: 1,
        kind: "contour_inside",
        contour: "hole:0",
        depth: 6.0,
        stepdown: 0.4,
        feed: 500,
        plunge: 80,
        rpm: 12000,
        bottom_allowance: 0.3,
      },
      {
        name: "hole a",
        tool: 1,
        kind: "helical_bore",
        x: 12,
        y: 12,
        diameter: 5,
        pitch: 0.4,
        depth: 6.0,
        stepdown: 0.4,
        feed: 500,
        plunge: 80,
        rpm: 12000,
        bottom_allowance: 0.3,
      },
      {
        name: "hole b",
        tool: 1,
        kind: "helical_bore",
        x: 68,
        y: 38,
        diameter: 5,
        pitch: 0.4,
        depth: 6.0,
        stepdown: 0.4,
        feed: 500,
        plunge: 80,
        rpm: 12000,
        bottom_allowance: 0.3,
      },
      {
        name: "profile",
        tool: 1,
        kind: "contour_outside",
        contour: "outer",
        depth: 6.0,
        stepdown: 0.4,
        feed: 500,
        plunge: 80,
        rpm: 12000,
        tabs: 3,
        tab_width: 5.0,
        tab_height: 1.0,
        bottom_allowance: 0.3,
        lead_in: false,
      },
    ],
  };
}

beforeEach(() => {
  documents.clear();
});

describe.skipIf(!hasCam)("cam_job", () => {
  it("posts a verified job and hands back the G-code as an artifact", async () => {
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);

    const result = (await client.callTool({
      name: "cam_job",
      // 6 mm of stock, a 0.3 mm onion skin: nothing reaches the bed.
      arguments: plateJob(docId, 6.0, null),
    })) as ToolCallResult;
    expect(result.isError, JSON.stringify(result.content)).toBeFalsy();
    const body = bodyOf(result);

    expect(body.blocked, JSON.stringify(body.checks)).toBe(false);
    expect(body.verified).toBe(true);
    expect(body.blocked_by).toEqual([]);

    // Every check the oracle runs, and every one of them passed.
    const checks = body.checks as Json;
    for (const name of [
      "gouge",
      "rapids",
      "plunges",
      "material_left",
      "depth",
      "tabs",
      "envelope",
      "loose",
    ]) {
      expect(checks[name], `${name}: ${JSON.stringify(checks[name])}`).toBe("pass");
    }

    // The deliverable: a real program, at a URL, of a plausible size.
    const gcode = body.gcode as Json;
    expect(typeof gcode.artifact_url).toBe("string");
    expect(gcode.filename).toBe("plate.nc");
    expect(gcode.bytes as number).toBeGreaterThan(5000);
    expect(gcode.lines as number).toBeGreaterThan(200);
    expect(typeof gcode.sha256).toBe("string");
    // Compact by default: the program text is behind `detail: "full"`.
    expect(gcode.text).toBeUndefined();

    // Geometry, not existence. The onion skin is really left: 6 mm of stock
    // less a 0.3 mm skin is a floor at -5.7, and nothing goes below it.
    expect(body.deepest_z as number).toBeCloseTo(-5.7, 3);

    // The sweep: an 80 × 50 plate profiled with a Ø3 cutter runs one radius
    // outside the wall and sweeps another, so it spans 80 + 6 by 50 + 6.
    const envelope = body.envelope as Json;
    const min = envelope.work_min as number[];
    const max = envelope.work_max as number[];
    expect(max[0] - min[0]).toBeCloseTo(86, 1);
    expect(max[1] - min[1]).toBeCloseTo(56, 1);

    // Four operations, one tool, inside features before the profile that
    // frees the part — whatever order they were typed in.
    const ops = body.operations as Json[];
    expect(ops.map((o) => o.name)).toEqual(["bore", "hole a", "hole b", "profile"]);
    expect(body.tool_sequence).toEqual([1]);
    expect((body.duration as Json).accel_aware_s as number).toBeGreaterThan(0);

    // Three tabs asked for, three standing, each leaving about 5 mm of metal.
    const placement = (body.tab_placement as Json[])[0];
    expect(placement.requested).toBe(3);
    expect(placement.found).toBe(3);
    for (const tab of placement.tabs as Json[]) {
      expect(tab.metal_width as number).toBeCloseTo(5.0, 1);
    }

    await client.close();
    await server.close();
  });

  it("refuses a cut deeper than the stock over a bare bed, and returns no G-code anywhere", async () => {
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);

    const result = (await client.callTool({
      // The same 6 mm cut in 4 mm stock, with nothing sacrificial beneath it.
      name: "cam_job",
      arguments: plateJob(docId, 4.0, null),
    })) as ToolCallResult;
    const body = bodyOf(result);

    expect(body.blocked, JSON.stringify(body)).toBe(true);
    expect(body.ok).toBe(false);
    expect(body.blocked_by).toContain("depth");
    expect(String(body.why)).toMatch(/depth/);

    // The rule, checked the only way that means anything: no G-code in the
    // structured result, and none in the serialized bytes a host would read
    // either. A `blocked` flag a caller could ignore beside a program it
    // could still run is not failing closed.
    expect(body.gcode).toBeNull();
    const wire = JSON.stringify(result);
    expect(wire).not.toMatch(/\bG0\s*[XYZ]/);
    expect(wire).not.toMatch(/\bG1\s*[XYZ]/);
    expect(wire).not.toMatch(/\bM3\b/);
    expect(wire).not.toMatch(/artifact_url/);

    // It still says what it found — a refusal with no findings is unusable.
    const checks = body.checks as Json;
    expect(checks.depth).not.toBe("pass");
    expect((checks.depth as Json).failed).toBe(true);

    await client.close();
    await server.close();
  });

  it("allows the same over-depth cut once a spoilboard is declared", async () => {
    // The control for the test above: the refusal is about what is under the
    // stock, not about the depth number being large.
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);
    const job = plateJob(docId, 4.0, 6.0);
    // A break-through is deliberate and stated: 0.2 mm past the underside.
    for (const op of job.operations as Json[]) {
      op.depth = 4.0;
      op.bottom_allowance = -0.2;
      op.through = true;
    }
    const result = (await client.callTool({
      name: "cam_job",
      arguments: job,
    })) as ToolCallResult;
    const body = bodyOf(result);
    expect(body.blocked, JSON.stringify(body.checks ?? body)).toBe(false);
    expect((body.gcode as Json).bytes as number).toBeGreaterThan(1000);
    // It really goes past the back of the stock, by the 0.2 mm declared.
    expect(body.deepest_z as number).toBeCloseTo(-4.2, 3);
    await client.close();
    await server.close();
  });
});

describe.skipIf(!hasCam)("cam_outline", () => {
  it("sections the plate from its raw tessellation and measures the holes", async () => {
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);

    const result = (await client.callTool({
      name: "cam_outline",
      arguments: { document_id: docId },
    })) as ToolCallResult;
    expect(result.isError, JSON.stringify(result.content)).toBeFalsy();
    const body = bodyOf(result);

    // The B-rep's own tessellation, never the repaired export mesh — that is
    // what keeps a tangent-fillet tear out of the wall the cutter follows.
    expect(body.mesh_source).toBe("raw_tessellation");
    // Auto z lands mid-height, and the part's Z range is the stock to buy.
    expect(body.z as number).toBeCloseTo(3.0, 6);
    expect(body.suggested_stock_thickness_mm as number).toBeCloseTo(6.0, 6);

    const bounds = body.bounds as number[];
    expect(bounds[2] - bounds[0]).toBeCloseTo(80, 2);
    expect(bounds[3] - bounds[1]).toBeCloseTo(50, 2);

    const regions = body.regions as Json[];
    expect(regions).toHaveLength(1);
    expect(regions[0].holes).toBe(3);

    // Diameters, not "three circles were found": the bore is Ø16 and the two
    // pilots are Ø5, and the holes are ordered largest-first so `hole:0` is
    // stably the bore.
    const circles = body.circles as Json[];
    const diameters = circles.map((c) => c.diameter as number).sort((a, b) => b - a);
    expect(diameters[0]).toBeCloseTo(16, 1);
    expect(diameters[1]).toBeCloseTo(5, 1);
    expect(diameters[2]).toBeCloseTo(5, 1);
    expect(circles.find((c) => c.name === "hole:0")?.diameter as number).toBeCloseTo(16, 1);

    // A constant-section part is what a contour job assumes.
    expect((body.prismatic as Json).prismatic).toBe(true);

    // Compact by default: the names travel, the four hundred points do not.
    expect(regions[0].outer).toBeUndefined();
    expect(body.contour_names).toEqual(["outer", "hole:0", "hole:1", "hole:2"]);

    await client.close();
    await server.close();
  });

  it("agrees with the export mesh on a part simple enough for them to agree", async () => {
    // Read this one for what it does NOT prove.
    //
    // `cam_outline` sections the B-rep's raw tessellation and says so in
    // `mesh_source`. Asserting that string only checks the label — a
    // deliberate mutation that sectioned `to_mesh()` while still reporting
    // "raw_tessellation" passed every test in this file, because on a plate
    // with three round holes the repair pass has nothing to move and the two
    // meshes section to the same contour: identical area to five decimals,
    // identical fitted diameters, identical fit errors.
    //
    // The 0.4 mm divergence that motivates the raw path needs a part where
    // repair really does move vertices — tangent fillets meeting, as on the
    // stator, which is a ~20 s kernel solve and lives in the Rust fixtures.
    //
    // What this test IS: the regression guard that they agree here. A simple
    // prismatic boolean sectioning differently through the two paths would
    // mean one of them has broken, and that is worth catching cheaply.
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);

    const raw = bodyOf(
      (await client.callTool({
        name: "cam_outline",
        arguments: { document_id: docId },
      })) as ToolCallResult,
    );
    expect(raw.mesh_source).toBe("raw_tessellation");

    // The export mesh, as the evaluated scene hands it to JavaScript.
    const mesh = engine.evaluate(getSession(docId)).parts[0].mesh!;
    const exported = engine.camOutlineFromMesh<Json>({
      positions: Array.from(mesh.positions),
      indices: Array.from(mesh.indices),
      auto_z: true,
    });
    expect(exported.error).toBeUndefined();
    expect(exported.mesh_source).toBe("inline");

    expect(exported.area as number).toBeCloseTo(raw.area_mm2 as number, 4);
    const diameters = (o: Json[]) =>
      o.map((c) => Number((c.diameter as number).toFixed(4))).sort((a, b) => b - a);
    expect(diameters(exported.circles as Json[])).toEqual(
      diameters(raw.circles as Json[]),
    );

    await client.close();
    await server.close();
  });

  it("reports a cone as not prismatic, with the disagreement in millimetres", async () => {
    const { client, server } = await connect();
    const docId = await makeDoc(client, CONE);

    const result = (await client.callTool({
      name: "cam_outline",
      arguments: { document_id: docId },
    })) as ToolCallResult;
    expect(result.isError, JSON.stringify(result.content)).toBeFalsy();
    const body = bodyOf(result);

    const prismatic = body.prismatic as Json;
    expect(prismatic.prismatic).toBe(false);
    // Not merely "false": the measured gap between the reference section and
    // the worst one. A Ø30 × 20 cone is R7.5 at mid-height and R14.25 at the
    // lowest plane sampled, so the two disagree by about 6.75 mm of wall —
    // whole millimetres, which is what makes this a refusal to machine rather
    // than a tolerance note.
    expect(prismatic.max_disagreement_mm as number).toBeGreaterThan(5);
    expect(prismatic.max_disagreement_mm as number).toBeLessThan(8);
    expect(String(prismatic.note)).toMatch(/disagree/);

    await client.close();
    await server.close();
  });

  it("names the contours cam_fit then accepts by name", async () => {
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);

    // A Ø3 cutter fits the Ø16 bore with room to spare; a Ø6 one cannot get
    // into the Ø5 pilots at all, and the report has to say which.
    const roomy = bodyOf(
      (await client.callTool({
        name: "cam_fit",
        arguments: { document_id: docId, contour: "hole:0", tool_diameter: 3.0 },
      })) as ToolCallResult,
    );
    expect(roomy.fits).toBe(true);
    // Fits means fits: nothing of the wall is left standing anywhere.
    expect(roomy.unreachable_corners).toBe(0);
    expect(roomy.metal_left_mm2 as number).toBeLessThan(0.01);

    const tight = bodyOf(
      (await client.callTool({
        name: "cam_fit",
        arguments: { document_id: docId, contour: "hole:1", tool_diameter: 6.0 },
      })) as ToolCallResult,
    );
    // A Ø6 cutter cannot get inside a Ø5 hole at all — which is a different
    // answer from "it misses some corners": there is no reachable interior to
    // have corners in, so the count is zero and the verdict is still no.
    expect(tight.fits).toBe(false);
    expect(String(tight.note)).toMatch(/cannot reach|standing/);

    // `largest_tool_that_fits_mm` is the kernel's own neck measurement, and
    // the claims worth pinning are the ones that follow from geometry rather
    // than from its sampling grid: a Ø16 bore admits a bigger cutter than a
    // Ø5 hole, neither admits one wider than itself, and the answer belongs to
    // the contour rather than to the tool that was asked about.
    const small = bodyOf(
      (await client.callTool({
        name: "cam_fit",
        arguments: { document_id: docId, contour: "hole:1", tool_diameter: 3.0 },
      })) as ToolCallResult,
    );
    expect(small.fits).toBe(true);
    const smallest = small.largest_tool_that_fits_mm as number;
    const biggest = roomy.largest_tool_that_fits_mm as number;
    expect(smallest).toBeGreaterThan(0);
    expect(smallest).toBeLessThanOrEqual(5);
    expect(biggest).toBeLessThanOrEqual(16);
    expect(biggest).toBeGreaterThan(smallest);
    expect(tight.largest_tool_that_fits_mm).toBe(smallest);

    await client.close();
    await server.close();
  });
});

describe.skipIf(!hasCam)("cam_verify_gcode", () => {
  /**
   * A program that cuts the plate's outside profile on the WRONG side: the
   * cutter centre follows a path one radius *inside* the wall instead of
   * outside it, so a Ø3 cutter eats 3 mm into the part all the way round.
   * This is friction-log item 32's mistake, as a file.
   */
  function insideOutProfile(): string {
    const lines = ["G21", "G90", "G54", "M3 S12000", "G0 Z5"];
    // The part is 0..80 by 0..50 with a 3 mm stock margin, so the profile
    // wall is the plate's own boundary. One radius inside it is 1.5 mm in.
    const r = 1.5;
    const corners: Array<[number, number]> = [
      [r, r],
      [80 - r, r],
      [80 - r, 50 - r],
      [r, 50 - r],
    ];
    lines.push(`G0 X${corners[0][0]} Y${corners[0][1]}`);
    for (let z = -1; z >= -6; z -= 1) {
      lines.push(`G1 Z${z} F80`);
      for (const [x, y] of [...corners.slice(1), corners[0]]) {
        lines.push(`G1 X${x} Y${y} F500`);
      }
    }
    lines.push("G0 Z5", "M5", "M2", "");
    return lines.join("\n");
  }

  it("rejects a profile cut on the wrong side of the wall", async () => {
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);

    const result = (await client.callTool({
      name: "cam_verify_gcode",
      arguments: {
        document_id: docId,
        gcode: insideOutProfile(),
        stock: { thickness: 6.0, margin: 3.0 },
        tool_diameter: 3.0,
      },
    })) as ToolCallResult;
    expect(result.isError, JSON.stringify(result.content)).toBeFalsy();
    const body = bodyOf(result);

    expect(body.pass, JSON.stringify(body.checks)).toBe(false);
    expect(body.blocked).toBe(true);
    expect(body.blocked_by).toContain("gouge");
    expect(String(body.verdict)).toMatch(/does not make this part/);

    // How badly: a Ø3 cutter running one radius the wrong way takes about a
    // tool diameter out of the wall, and the report gives the number.
    const gouge = (body.checks as Json).gouge as Json;
    expect(gouge.failed).toBe(true);
    expect(gouge.worst as number).toBeGreaterThan(1.0);
    expect(gouge.worst as number).toBeLessThan(5.0);

    await client.close();
    await server.close();
  });

  it("accepts the program cam_job just produced for the same part", async () => {
    // The round trip: what the oracle passes on the way out, it passes on the
    // way back in. Without this the rejection above could be a verifier that
    // rejects everything.
    const { client, server } = await connect();
    const docId = await makeDoc(client, PLATE);

    const job = bodyOf(
      (await client.callTool({
        name: "cam_job",
        arguments: { ...plateJob(docId, 6.0, null), detail: "full" },
      })) as ToolCallResult,
    );
    expect(job.blocked).toBe(false);
    const gcode = (job.gcode as Json).text as string;
    expect(typeof gcode).toBe("string");

    const check = bodyOf(
      (await client.callTool({
        name: "cam_verify_gcode",
        arguments: {
          document_id: docId,
          gcode,
          stock: { thickness: 6.0, margin: 3.0 },
          tool_diameter: 3.0,
          bottom_allowance: 0.3,
          tabs: [
            { width: 5.0, height: 1.0 },
            { width: 5.0, height: 1.0 },
            { width: 5.0, height: 1.0 },
          ],
        },
      })) as ToolCallResult,
    );
    expect(check.pass, JSON.stringify(check.checks)).toBe(true);
    expect(check.blocked).toBe(false);
    expect(check.moves_replayed as number).toBeGreaterThan(100);

    await client.close();
    await server.close();
  });
});

describe.skipIf(!hasCam)("cam_recommend_feeds and cam_gear", () => {
  it("reproduces the copper anchor, dial position included", async () => {
    const { client, server } = await connect();
    const body = bodyOf(
      (await client.callTool({
        name: "cam_recommend_feeds",
        arguments: {
          material: "copper-c110",
          op: "slot",
          tool: { diameter: 2.0, flutes: 2, kind: "flat_end_mill", flute_length: 6.0 },
          machine: { class: "hobby", spindle: "dial" },
        },
      })) as ToolCallResult,
    );
    // The cut that worked on the day: 1 mm copper, Ø2 two-flute, dial 2.
    expect(body.dial).toBe("2");
    expect(body.dial_spindle).toBe(true);
    expect(body.feed_mm_min as number).toBeGreaterThan(180);
    expect(body.feed_mm_min as number).toBeLessThan(350);
    expect((body.warnings as string[]).join(" ")).toMatch(/weld/);
    await client.close();
    await server.close();
  });

  it("measures the milestone planet over pins", async () => {
    const { client, server } = await connect();
    const body = bodyOf(
      (await client.callTool({
        name: "cam_gear",
        arguments: {
          gear: { module: 1.0, teeth: 20, backlash_thinning: 0.03 },
          cutter_diameter: 1.0,
          pin_diameter: 1.4,
        },
      })) as ToolCallResult,
    );
    // One 20 T module-1.0 planet over Ø1.4 pins: M = 21.0738 mm.
    expect(body.over_pins_mm as number).toBeCloseTo(21.0738, 3);
    expect(body.recommended_pin_diameter as number).toBeCloseTo(1.3711, 3);
    // Compact by default: no contour point lists unless asked.
    expect(body.contours).toBeUndefined();
    await client.close();
    await server.close();
  });
});

describe.skipIf(!hasCam)("the pack", () => {
  it("is a gated pack of six tools, off when cam is disabled", async () => {
    const { client, server } = await connect();
    const packs = bodyOf(
      (await client.callTool({ name: "list_tool_packs", arguments: {} })) as ToolCallResult,
    );
    const cam = (packs.packs as Json[]).find((p) => p.name === "cam");
    expect(cam, JSON.stringify(packs.packs)).toBeDefined();
    expect(cam!.tool_count).toBe(6);

    await client.callTool({ name: "set_tool_packs", arguments: { disable: ["cam"] } });
    const listed = (await client.listTools()).tools.map((t) => t.name);
    expect(listed).not.toContain("cam_job");
    const refused = (await client.callTool({
      name: "cam_job",
      arguments: {},
    })) as ToolCallResult;
    expect(refused.isError).toBe(true);

    await client.callTool({ name: "set_tool_packs", arguments: { enable: ["cam"] } });
    expect((await client.listTools()).tools.map((t) => t.name)).toContain("cam_job");

    await client.close();
    await server.close();
  });
});
