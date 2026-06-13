/**
 * Loader for the differentiable-design Rust engine compiled to WASM
 * (`crates/vcad-ecad-diff-wasm`). Lazily `require`s the nodejs-target pkg and
 * exposes the kernel solvers — PDN sizing with the implicit-function adjoint,
 * and the differentiable plant/controller co-design — to the MCP server. Returns
 * null when the artifact is absent, so callers fall back to the TS path.
 */
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

interface EcadDiffWasm {
  size_pdn: (json: string) => string;
  codesign_motor_json: (json: string) => string;
}

let cached: EcadDiffWasm | null | undefined;

function load(): EcadDiffWasm | null {
  if (cached !== undefined) return cached;
  try {
    const here = dirname(fileURLToPath(import.meta.url));
    const pkg = resolve(here, "../../../../crates/vcad-ecad-diff-wasm/pkg/vcad_ecad_diff_wasm.js");
    const req = createRequire(import.meta.url);
    cached = req(pkg) as EcadDiffWasm;
  } catch {
    cached = null;
  }
  return cached;
}

/** True when the Rust engine wasm is loadable. */
export function ecadDiffEngineAvailable(): boolean {
  return load() !== null;
}

/** Size a PDN mesh with the Rust analytic-adjoint solver. Null if unavailable. */
export function sizePdnExact(spec: Record<string, unknown>): Record<string, unknown> | null {
  const m = load();
  if (!m) return null;
  return JSON.parse(m.size_pdn(JSON.stringify(spec))) as Record<string, unknown>;
}

/** Run the differentiable plant/controller co-design in Rust. Null if unavailable. */
export function codesignMotorExact(spec: Record<string, unknown>): Record<string, unknown> | null {
  const m = load();
  if (!m) return null;
  return JSON.parse(m.codesign_motor_json(JSON.stringify(spec))) as Record<string, unknown>;
}
