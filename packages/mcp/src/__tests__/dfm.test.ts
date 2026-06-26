import { describe, it, expect, beforeAll, beforeEach } from "vitest";
import { Engine } from "@vcad/engine";
import { createSchematic, placeComponents, addTrace } from "../tools/ecad.js";
import { dfmCheck } from "../tools/dfm.js";
import { documents } from "../tools/session.js";

let engine: Engine;

beforeAll(async () => {
  engine = await Engine.init();
});

beforeEach(() => {
  documents.clear();
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
