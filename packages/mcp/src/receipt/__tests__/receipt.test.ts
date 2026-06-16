import { describe, it, expect } from "vitest";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { buildReceipt, renderReceiptHtml, renderReceiptText } from "../index.js";
import { S0, S1, S2 } from "./fixtures.js";

describe("Receipt — diffs before/after DRC and attributes blame", () => {
  it("builds a ledger where a clean route then an accidental re-route reads correctly", () => {
    const receipt = buildReceipt({
      board: { title: "Receipt demo board", components: 4, nets: ["VCC", "GND", "SIG1", "SIG2", "SIG3"] },
      preflight: { unconnectedPins: ["U1.5", "U1.6", "U1.7"] },
      build: { version: "0.9.4", sha: "fixture" },
      steps: [
        { tool: "route_nets", args: { nets: [] }, before: S0, after: S1 },
        { tool: "route_nets", args: { nets: [] }, before: S1, after: S2 },
      ],
    });
    const [e1, e2] = receipt.entries;

    // ---- Entry 1: the route does real work, with a small via cost. ----
    expect(e1!.verdict).toBe("improved-with-regressions");
    expect(e1!.tally.credited).toBe(5); // closed the 5 unrouted nets
    expect(e1!.fixed.some((g) => g.rule === "UnconnectedNet")).toBe(true);
    expect(e1!.introduced.some((g) => g.rule === "HoleToHole")).toBe(true);

    // ---- Entry 2: the re-route is a silent catastrophe. ----
    expect(e2!.verdict).toBe("regression");
    expect(e2!.tally.shortsIntroduced).toBe(10);
    expect(e2!.deltaByRule.Short).toBe(10);
    expect(e2!.introduced.some((g) => /^Short\b/.test(g.message))).toBe(true);

    // ---- Attribution invariant: a footprint fault is NEVER blamed on the router. ----
    for (const e of receipt.entries) {
      for (const g of e!.introduced) {
        expect(g.cause).not.toBe("footprint");
        expect(g.cause).not.toBe("placement");
      }
      for (const g of e!.persisted.filter((g) => g.cause === "footprint")) {
        expect(g.blame).toBe("pre-existing");
      }
    }
    expect(e2!.persisted.some((g) => g.cause === "footprint")).toBe(true);

    // ---- Coverage + deterministic fingerprint. ----
    expect(e2!.coverage).toBe("full");
    const again = buildReceipt({
      board: {},
      build: { version: "0.9.4", sha: "fixture" },
      steps: [{ tool: "route_nets", args: { nets: [] }, before: S1, after: S2 }],
    });
    expect(again.entries[0]!.fingerprint).toBe(e2!.fingerprint); // recomputes identically

    // ---- Emit the rendered artifacts. ----
    const html = renderReceiptHtml(receipt);
    expect(html).toContain("hard short");
    expect(html).toContain("PRE-EXISTING");
    writeFileSync(fileURLToPath(new URL("../../../receipt-demo.html", import.meta.url)), html);
    writeFileSync(fileURLToPath(new URL("../../../receipt-demo.txt", import.meta.url)), renderReceiptText(receipt));
  });
});
