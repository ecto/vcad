import { describe, it, expect } from "vitest";
import { ReceiptSession, type DrcSnapshot } from "../index.js";
import { S0, S1, S2, S2_drifted } from "./fixtures.js";

const noop = async () => ({});

describe("ReceiptSession — hands a verdict, reverify catches drift", () => {
  it("accumulates entries, returns a verdict per mutation, and detects an off-the-books change", async () => {
    // Scripted oracle: record() reads drc twice (before/after), reverify() once.
    const seq: DrcSnapshot[] = [S0, S1, /*reverify*/ S1, /*rec2 before*/ S1, /*rec2 after*/ S2, /*reverify*/ S2_drifted];
    let i = 0;
    const drc = async () => seq[i++]!;
    const s = new ReceiptSession(
      "doc_test",
      { title: "t", components: 4, nets: ["VCC", "GND", "SIG1", "SIG2", "SIG3"] },
      { drc, build: { version: "0.9.4", sha: "test" } },
    );

    const r1 = await s.record("route_nets", { document_id: "doc_test" }, noop);
    expect(r1.view.document_id).toBe("doc_test"); // a verdict, not a bare id
    expect(r1.view.verdict).toBe("improved-with-regressions");
    expect(r1.view.credited).toBe(5);

    const v1 = await s.reverify(); // oracle returns S1 == r1.after → matches
    expect(v1.ok).toBe(true);
    expect(v1.stored).toBe(v1.recomputed);

    const r2 = await s.record("route_nets", { document_id: "doc_test" }, noop);
    expect(r2.view.verdict).toBe("regression");
    expect(r2.view.shortsIntroduced).toBe(10);
    expect(r2.view.headline).toMatch(/REGRESSION/);
    expect(r2.view.headline).toMatch(/untouched/); // pre-existing faults not blamed

    const drift = await s.reverify(); // oracle returns S2_drifted != r2.after → mismatch
    expect(drift.ok).toBe(false);
    expect(drift.stored).not.toBe(drift.recomputed);

    expect(s.receipt().entries.length).toBe(2);
  });

  it("goes dirty if an after-DRC fails, and refuses to continue until resync()", async () => {
    let calls = 0;
    const drc = async () => {
      calls++;
      if (calls === 2) throw new Error("oracle down"); // the after-DRC of the first record
      return S0;
    };
    const s = new ReceiptSession("d", {}, { drc });

    await expect(s.record("route_nets", {}, noop)).rejects.toThrow(/after-DRC failed/);
    await expect(s.record("route_nets", {}, noop)).rejects.toThrow(/unverified/); // won't trust a dirty baseline
    s.resync();
    const r = await s.record("route_nets", {}, noop); // S0 -> S0, a clean no-op
    expect(r.entry.verdict).toBe("no-op");
  });
});
