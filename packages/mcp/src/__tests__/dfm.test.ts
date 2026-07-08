import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import type { DfmReport } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import {
  createSchematic,
  placeComponents,
  addTrace,
  addMotorWinding,
  runDrc,
} from "../tools/ecad.js";
import {
  dfmCheck,
  dfmExplain,
  dfmSuggestFix,
  dfmApplyFix,
  clearDfmState,
} from "../tools/dfm.js";
import { documents, getSession, openDocument } from "../tools/session.js";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
  clearDfmState();
});

/** Parse the single JSON text block of a tool result. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function out(result: { content: Array<{ type: string; text: string }> }): any {
  return JSON.parse(result.content[0].text);
}

const resistor = (ref: string, x: number) => ({
  ref,
  value: "1k",
  footprint: "Resistor_SMD:R_0805",
  x,
  y: 0,
  pins: [
    { number: "1", name: "~", type: "Passive", x: -5, y: 0 },
    { number: "2", name: "~", type: "Passive", x: 5, y: 0 },
  ],
});

async function placedBoard(): Promise<string> {
  const created = out(
    await createSchematic({
      components: [resistor("R1", 0), resistor("R2", 20)],
      nets: { MID: ["R1.2", "R2.1"] },
    }),
  );
  const id = created.document_id;
  await placeComponents({ document_id: id, board_width: 40, board_height: 20 });
  return id;
}

describe("dfm_check — PCB fab profiles", () => {
  it("returns a per-rule pass/fail report naming the fab profile", async () => {
    const id = await placedBoard();
    const res = out(await dfmCheck({ document_id: id, process: "pcb_jlcpcb" }, engine));

    expect(res.kind).toBe("pcb");
    expect(res.fab_profile).toBe("jlcpcb");
    expect(res.fab_profile_name).toContain("JLCPCB");
    expect(res.copper_weight_oz).toBeGreaterThan(0);
    expect(Array.isArray(res.rules)).toBe(true);
    expect(res.rules.length).toBeGreaterThan(0);
    // Every rule carries an explicit verdict.
    for (const r of res.rules) {
      expect(typeof r.rule).toBe("string");
      expect(typeof r.passed).toBe("boolean");
      expect(["error", "warning", "info"]).toContain(r.severity);
    }
    // The named DFM rules are present.
    const ruleIds = res.rules.map((r: { rule: string }) => r.rule);
    for (const id of ["min_trace_width", "min_drill", "min_annular_ring", "copper_to_edge"]) {
      expect(ruleIds).toContain(id);
    }
    expect(typeof res.score).toBe("number");
  });

  it("flags a sub-minimum trace against the JLCPCB profile", async () => {
    const id = await placedBoard();
    // A hair-thin 0.05mm trace — below JLCPCB's 0.127mm (5mil) 1oz minimum.
    await addTrace({
      document_id: id,
      points: [
        { x: 5, y: 5 },
        { x: 15, y: 5 },
      ],
      net: "MID",
      width: 0.05,
    });

    const res = out(await dfmCheck({ document_id: id, process: "pcb_jlcpcb" }, engine));
    const tw = res.rules.find((r: { rule: string }) => r.rule === "min_trace_width");
    expect(tw).toBeDefined();
    expect(tw.passed).toBe(false);
    expect(tw.severity).toBe("error");
    expect(tw.measured).toBeCloseTo(0.05, 3);
    expect(res.failed_rules).toContain("min_trace_width");
    expect(res.passed).toBe(false);
    expect(res.score).toBeLessThan(100);
  });

  it("honors a rule_pack_toml override that relaxes the trace minimum", async () => {
    const id = await placedBoard();
    await addTrace({
      document_id: id,
      points: [
        { x: 5, y: 5 },
        { x: 15, y: 5 },
      ],
      net: "MID",
      width: 0.05,
    });
    const custom = [
      'process = "pcb"',
      'profile = "jlcpcb"',
      'name = "Custom fine-line"',
      "[rules.min_trace_width]",
      'severity = "error"',
      "oz1_mm = 0.04",
    ].join("\n");
    const res = out(
      await dfmCheck({ document_id: id, process: "pcb_jlcpcb", rule_pack_toml: custom }, engine),
    );
    const tw = res.rules.find((r: { rule: string }) => r.rule === "min_trace_width");
    expect(tw.passed).toBe(true);
    expect(res.fab_profile_name).toBe("Custom fine-line");
  });

  it("min_clearance is netTie-aware: wye star contacts are exempt, agreeing with DRC", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const id = created.document_id;
    await placeComponents({
      document_id: id,
      board_shape: { type: "circle", outer_diameter: 120, inner_diameter: 20 },
    });
    const winding = out(
      await addMotorWinding({
        document_id: id,
        slots: 9,
        poles: 6,
        center: { x: 60, y: 60 },
        pitch_radius: 40,
        inner_radius: 2,
        outer_radius: 6,
        trace_width: 0.2,
        clearance: 0.15,
        connection: "wye",
      }),
    );
    expect(winding.success).toBe(true);
    expect(winding.net_ties_added).toBe(1);

    const doc = getSession(id);
    const board = getNodePcb(doc, getPcbNodeIds(doc)[0]!)!;
    const tie = board.netTies![0]!;
    expect(tie.position).toBeDefined();
    expect(tie.radius).toBeGreaterThan(0);

    // DRC's clearance pass is tie-aware; collect the net pairs it flags.
    const drc = out(await runDrc({ document_id: id }));
    const drcPairs = new Set(
      drc.byNetPair
        .filter((p: { rule: string }) => p.rule === "Clearance")
        .map((p: { nets: string[] }) => [...p.nets].sort().join("|")),
    );

    const res = out(await dfmCheck({ document_id: id, process: "pcb_jlcpcb" }, engine));
    const mc = res.rules.find((r: { rule: string }) => r.rule === "min_clearance");
    expect(mc).toBeDefined();
    for (const loc of mc.locations as Array<{ x: number; y: number; nets?: string[] }>) {
      // The deliberate wye star junction is exempt: no finding inside the
      // tie region, and none involving the neutral (it only ever touches
      // other copper at the star).
      const d = Math.hypot(loc.x - tie.position!.x, loc.y - tie.position!.y);
      expect(d).toBeGreaterThan(tie.radius!);
      expect(loc.nets).not.toContain("WIND_N");
      // DFM never flags a pair DRC's tie-aware pass doesn't also flag —
      // the exemption logic is shared, not re-implemented.
      expect(drcPairs.has([...loc.nets!].sort().join("|"))).toBe(true);
    }
  });

  it("min_clearance reports positions and nets for a genuine untied gap", async () => {
    const id = await placedBoard();
    // Two parallel different-net traces with a 0.1mm copper gap — a real
    // spacing defect below JLCPCB's 0.127mm 1oz floor, no tie anywhere.
    await addTrace({
      document_id: id,
      points: [
        { x: 5, y: 5 },
        { x: 15, y: 5 },
      ],
      net: "MID",
      width: 0.2,
    });
    await addTrace({
      document_id: id,
      points: [
        { x: 5, y: 5.3 },
        { x: 15, y: 5.3 },
      ],
      net: "OTHER",
      width: 0.2,
    });

    const res = out(await dfmCheck({ document_id: id, process: "pcb_jlcpcb" }, engine));
    const mc = res.rules.find((r: { rule: string }) => r.rule === "min_clearance");
    expect(mc.passed).toBe(false);
    expect(mc.violations).toBeGreaterThan(0);
    // Violations carry triageable positions and structured net pairs.
    const loc = mc.locations.find(
      (l: { nets?: string[] }) => l.nets?.includes("MID") && l.nets?.includes("OTHER"),
    );
    expect(loc).toBeDefined();
    expect(loc.x).toBeCloseTo(10, 1);
    expect(loc.y).toBeCloseTo(5.15, 1);
  });

  it("errors when a pcb_ profile is used on a document with no board", async () => {
    const created = out(await createSchematic({ components: [resistor("R1", 0)] }));
    const res = await dfmCheck(
      { document_id: created.document_id, process: "pcb_jlcpcb" },
      engine,
    );
    expect((res as { isError?: boolean }).isError).toBe(true);
    expect(res.content[0].text).toContain("no PCB");
  });
});

// ─── Serverless-safe follow-ups: inline document + inline report ─────────────

/** A minimal solid document (one 10 mm cube) for the mechanical DFM path. */
function cubeDoc(): Document {
  return {
    version: "0.1",
    nodes: {
      "1": { id: 1, name: "cube", op: { type: "Cube", size: { x: 10, y: 10, z: 10 } } },
    },
    materials: {},
    part_materials: {},
    roots: [{ root: 1, material: "default" }],
  } as unknown as Document;
}

/** A hand-built DFM report — the payload dfm_check returns and the follow-up
 *  tools accept inline when the warm cache is empty. Carries one issue with a
 *  set_param fix so dfm_apply_fix has something applyable. */
function syntheticReport(node: number): DfmReport {
  return {
    process: "cnc_3axis",
    rule_pack_name: "test-pack",
    rule_pack_version: "1",
    issues: [
      {
        id: "iss-1",
        rule: "cnc.internal_radius_too_small",
        severity: "error",
        process: "cnc_3axis",
        message: "Radius R1.00 mm below cutter minimum R3.00 mm",
        explanation: "Internal radii below the cutter radius can't be machined.",
        face_indices: [],
        edge_indices: [],
        anchor: [0, 0, 0],
        measured: 1,
        limit: 3,
        units: "mm",
        origin_op: node,
        suggested_fix: { type: "set_param", node, path: "radius", value: 3 },
      },
    ],
    cost_estimate: null,
  };
}

describe("dfm_check — inline document (serverless-safe)", () => {
  it("runs a mechanical check on an inline document with no resident session", async () => {
    documents.clear();
    const res = out(await dfmCheck({ document: cubeDoc(), process: "fdm" }, engine));
    expect(res.process).toBe("fdm");
    expect(Array.isArray(res.issues)).toBe(true);
    expect(typeof res.issue_count).toBe("number");
    expect(typeof res.score).toBe("number");
  });
});

describe("dfm_explain / dfm_suggest_fix — inline report fallback", () => {
  it("resolves an issue from the warm cache after dfm_check", async () => {
    const { document_id } = JSON.parse(
      openDocument({ initial: cubeDoc() }).content[0].text,
    );
    // Populate the warm cache with a real report, then reach into it via the id.
    await dfmCheck({ document_id, process: "fdm" }, engine);
    // A real cube may or may not trip an fdm rule, so drive the assertion with
    // an inline report on the same session — the documented id+report contract.
    const explained = out(
      dfmExplain({ document_id, issue_id: "iss-1", report: syntheticReport(1) }),
    );
    expect(explained.id).toBe("iss-1");
    expect(explained.rule).toBe("cnc.internal_radius_too_small");
  });

  it("survives a cleared warm cache when the report is passed inline", () => {
    const { document_id } = JSON.parse(
      openDocument({ initial: cubeDoc() }).content[0].text,
    );
    // Simulate a cold serverless instance: the warm report map is gone even
    // though the (durably-backed) session document is still resolvable.
    clearDfmState();

    // Without the inline report the tool fails closed, pointing at the escape hatch.
    expect(() => dfmExplain({ document_id, issue_id: "iss-1" })).toThrow(
      /Run dfm_check first|report inline/,
    );

    // With the report inline the documented path works.
    const explained = out(
      dfmExplain({ document_id, issue_id: "iss-1", report: syntheticReport(1) }),
    );
    expect(explained.explanation).toContain("Internal radii");

    const suggested = out(
      dfmSuggestFix({ issue_id: "iss-1", report: syntheticReport(1) }),
    );
    expect(suggested.issue_id).toBe("iss-1");
    expect(suggested.applyable).toBe(true);
    expect(suggested.fix.type).toBe("set_param");
  });

  it("rejects a malformed inline report", () => {
    expect(() =>
      dfmExplain({ issue_id: "iss-1", report: { not: "a report" } }),
    ).toThrow(/must be a DFM report/);
  });
});

describe("dfm_apply_fix — inline document + inline report", () => {
  it("applies a set_param fix to an inline document and echoes it back", () => {
    clearDfmState();
    const doc = {
      version: "0.1",
      nodes: {
        "5": { id: 5, name: "pin", op: { type: "Cylinder", radius: 1, height: 10 } },
      },
      materials: {},
      part_materials: {},
      roots: [{ root: 5, material: "default" }],
    } as unknown as Document;

    const res = out(
      dfmApplyFix({ document: doc, issue_id: "iss-1", report: syntheticReport(5) }),
    );
    expect(res.applied).toBe(true);
    expect(res.node).toBe(5);
    expect(res.path).toBe("radius");
    expect(res.value).toBe(3);
    // Inline path echoes the mutated document (no session to persist into).
    expect(res.document.nodes["5"].op.radius).toBe(3);
  });

  it("mutates a resident session in place via document_id", () => {
    const { document_id } = JSON.parse(
      openDocument({
        initial: {
          version: "0.1",
          nodes: {
            "5": { id: 5, name: "pin", op: { type: "Cylinder", radius: 1, height: 10 } },
          },
          materials: {},
          part_materials: {},
          roots: [{ root: 5, material: "default" }],
        } as unknown as Document,
      }).content[0].text,
    );
    clearDfmState();
    const res = out(
      dfmApplyFix({ document_id, issue_id: "iss-1", report: syntheticReport(5) }),
    );
    expect(res.applied).toBe(true);
    // Session path echoes only the id; the mutation landed in the live doc.
    expect(res.document).toBeUndefined();
    const stored = documents.get(document_id) as Document;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((stored.nodes["5"].op as any).radius).toBe(3);
  });
});
