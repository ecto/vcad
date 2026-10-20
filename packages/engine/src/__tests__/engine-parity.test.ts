/**
 * WASM leg of the engine parity harness (crates/vcad-app/tests/parity).
 * The same fixtures replay natively (`cargo test -p vcad-app --test parity`)
 * and through the C ABI (`vcad-ffi`); all three must produce identical
 * per-step outcomes and final state hashes.
 */
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import { getKernelWasm } from "../wasm-singleton.js";

interface Fixture {
  replica: number;
  steps: Array<{ command: unknown } | { tool: string; args: unknown }>;
  expect: { outcomes: string[]; hash: string };
}

interface ParityEngine {
  apply(json: string): string;
  apply_tool(tool: string, json: string): string;
  stateHash(): string;
  free(): void;
}

type EngineClass = { withReplica?(id: number): ParityEngine };

const dir = join(dirname(fileURLToPath(import.meta.url)), "../../../../crates/vcad-app/tests/parity");
const fixtures = readdirSync(dir)
  .filter((f) => f.endsWith(".json"))
  .sort()
  .map((f) => [f, JSON.parse(readFileSync(join(dir, f), "utf8")) as Fixture] as const);

function outcome(envelopeJson: string): string {
  const v = JSON.parse(envelopeJson) as {
    ok: boolean;
    result?: { createdFeatureId?: string };
    error?: { kind?: string };
  };
  return v.ok ? `ok:${v.result?.createdFeatureId ?? "-"}` : `err:${v.error?.kind ?? "?"}`;
}

let Engine: EngineClass | undefined;
beforeAll(async () => {
  const wasm = (await getKernelWasm()) as unknown as { WasmDocumentEngine?: EngineClass };
  Engine = wasm.WasmDocumentEngine;
});

describe("engine parity (WASM)", () => {
  it.each(fixtures)("%s matches the native golden", (_name, fixture) => {
    if (typeof Engine?.withReplica !== "function") {
      // Checked-in artifacts can predate the parity API; CI builds the
      // kernel from source and must never skip.
      if (process.env.CI) throw new Error("kernel WASM lacks WasmDocumentEngine.withReplica");
      return;
    }
    const engine = Engine.withReplica(fixture.replica);
    try {
      const outcomes = fixture.steps.map((step) =>
        outcome(
          "tool" in step
            ? engine.apply_tool(step.tool, JSON.stringify(step.args))
            : engine.apply(JSON.stringify(step.command)),
        ),
      );
      expect({ outcomes, hash: engine.stateHash() }).toEqual(fixture.expect);
    } finally {
      engine.free();
    }
  });
});
