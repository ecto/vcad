/**
 * The TS delta fold against Rust-produced envelopes: the Swift codegen
 * fixture records each command as both a full envelope and a delta (twin
 * engines, same replica). Folding delta i into state i-1 must give state i.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { applyEngineDelta, EngineRefusal, parseDeltaEnvelope, stateFromEnvelope } from "../delta.js";

const fixture = JSON.parse(
  readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "../../../../apple/VcadApp/Tests/Fixtures/engine-types.json"),
    "utf8",
  ),
) as { envelopes: unknown[]; deltas: unknown[] };

describe("engine deltas", () => {
  it("folding each delta reproduces the full envelope", () => {
    let state = stateFromEnvelope(fixture.envelopes[0] as never);
    let folded = 0;
    for (let i = 1; i < fixture.deltas.length; i++) {
      const env = fixture.envelopes[i] as { ok: boolean };
      if (!env.ok) {
        expect(() => parseDeltaEnvelope(fixture.deltas[i] as never)).toThrow(EngineRefusal);
        continue;
      }
      const { state: next, changes } = applyEngineDelta(state, parseDeltaEnvelope(fixture.deltas[i] as never));
      const full = stateFromEnvelope(env as never);
      expect(next.document).toEqual(full.document);
      expect(next.parts).toEqual(full.parts);
      expect(changes.rebuild.length).toBeGreaterThan(0);
      state = next;
      folded++;
    }
    expect(folded).toBeGreaterThanOrEqual(3);
  });
});
