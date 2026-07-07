#!/usr/bin/env node

/**
 * Cross-platform build for @vcad/kernel-wasm.
 *
 * Replaces the previous bash-only inline script so `npm run build` works on
 * Windows runners (cmd.exe can't parse `if [ -n "$VCAD_WASM_SKIP" ]; then ...`
 * or expand `cp pkg/vcad_kernel_wasm*`).
 *
 * Honors VCAD_WASM_SKIP=1 to short-circuit the wasm-pack rebuild when the
 * checked-in artifacts in packages/kernel-wasm/ are already current. Otherwise
 * runs wasm-pack against crates/vcad-kernel-wasm and copies the generated
 * vcad_kernel_wasm* files into the package root so the package.json `files`
 * array picks them up.
 */

import { spawnSync } from 'child_process';
import {
  appendFileSync,
  copyFileSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

/**
 * Append the trap-recovery hook to the wasm-bindgen glue if it's missing.
 *
 * wasm-pack regenerates the glue from scratch, so this re-applies the
 * `__vcad_reset_wasm` export the wasm-singleton needs to re-instantiate the
 * kernel after a panic trap (instead of poisoning the whole process). Keep
 * this in sync with the committed glue and packages/engine/src/wasm-singleton.ts.
 */
function ensureResetHook(gluePath) {
  const src = readFileSync(gluePath, 'utf8');
  if (src.includes('__vcad_reset_wasm')) return;
  appendFileSync(
    gluePath,
    [
      '',
      '// vcad: trap-recovery hook (appended by packages/kernel-wasm build).',
      '// Dropping the cached `wasm`/`wasmModule` bindings lets a subsequent',
      '// initSync()/default() re-instantiate a fresh instance in place, so the',
      '// wasm-singleton can recover from a panic trap instead of poisoning the',
      '// process. See packages/engine/src/wasm-singleton.ts (resetKernelWasm).',
      'export function __vcad_reset_wasm() {',
      '    wasm = undefined;',
      '    wasmModule = undefined;',
      '}',
      '',
    ].join('\n'),
  );
  console.log('[kernel-wasm] appended __vcad_reset_wasm trap-recovery hook');
}

/**
 * Rewire aliased export calls in the wasm-bindgen glue.
 *
 * When two `#[wasm_bindgen]` shims compile to byte-identical bodies (two
 * `-> bool { true }` availability probes; two f64 getters reading the same
 * struct offset), some host toolchains let the linker merge them and
 * wasm-bindgen then points BOTH JS wrappers at ONE of the surviving export
 * names — e.g. `isEcadAvailable()` calling `wasm.isCamAvailable()`, or
 * `SliceResult.filamentGrams` calling `wasm.circuitsim_dt`. The module still
 * exports every name (as aliases of the merged function), so calling the
 * wrapper's own same-named export is behaviorally identical — and keeps the
 * glue deterministic across toolchains. packages/engine's kernel-wasm-glue
 * drift test fails the build otherwise.
 *
 * A wrapper is only rewritten when its expected export actually exists in
 * the built wasm, so legitimate cross-class delegations (e.g. `Solid` ->
 * `raytracer_canRaytrace`, which has no `solid_*` export) are untouched.
 */
function normalizeGlue(gluePath, wasmPath) {
  const exports = new Set(
    WebAssembly.Module.exports(
      new WebAssembly.Module(readFileSync(wasmPath)),
    ).map((e) => e.name),
  );
  const lines = readFileSync(gluePath, 'utf8').split('\n');
  const classNames = new Set(
    lines
      .map((l) => l.match(/^export class (\w+)/)?.[1]?.toLowerCase())
      .filter(Boolean),
  );

  const fixes = [];
  let cls = null;
  let method = null;
  let freeFn = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const classMatch = line.match(/^export class (\w+)/);
    if (classMatch) {
      cls = classMatch[1];
      continue;
    }
    if (cls && /^}/.test(line)) {
      cls = null;
      continue;
    }
    if (cls) {
      // Track the enclosing method so the expected export name can be
      // derived: wasm-bindgen exports `classname_method` (getters keep the
      // property name, setters get a `set_` infix).
      const sig = line.match(/^ {4}(static\s+)?(get\s+|set\s+)?(\w+)\s*\(.*\)\s*{/);
      if (sig) method = (sig[2]?.trim() === 'set' ? 'set_' : '') + sig[3];
      if (!method) continue;
      for (const call of line.matchAll(/wasm\.([A-Za-z0-9_]+)\(/g)) {
        const name = call[1];
        if (name.startsWith('__')) continue;
        const prefix = name.match(/^([a-z0-9]+)_/)?.[1];
        if (!prefix || !classNames.has(prefix)) continue;
        if (prefix === cls.toLowerCase()) continue;
        const expected = `${cls.toLowerCase()}_${method}`;
        if (expected !== name && exports.has(expected)) {
          lines[i] = line.replace(`wasm.${name}(`, `wasm.${expected}(`);
          fixes.push(`${cls}.${method}: ${name} -> ${expected}`);
        }
      }
      continue;
    }
    const fnMatch = line.match(/^export function (\w+)\s*\(/);
    if (fnMatch) {
      freeFn = { name: fnMatch[1], calls: [] };
      continue;
    }
    if (!freeFn) continue;
    for (const call of line.matchAll(/wasm\.([A-Za-z0-9_]+)\(/g)) {
      if (!call[1].startsWith('__')) freeFn.calls.push({ line: i, name: call[1] });
    }
    if (/^}/.test(line)) {
      // Only unambiguous single-call pass-through wrappers are rewritten —
      // the same scope the engine drift guard checks.
      const distinct = [...new Set(freeFn.calls.map((c) => c.name))];
      if (
        distinct.length === 1 &&
        distinct[0] !== freeFn.name &&
        exports.has(freeFn.name)
      ) {
        for (const c of freeFn.calls) {
          lines[c.line] = lines[c.line].replace(
            `wasm.${c.name}(`,
            `wasm.${freeFn.name}(`,
          );
        }
        fixes.push(`${freeFn.name}: ${distinct[0]} -> ${freeFn.name}`);
      }
      freeFn = null;
    }
  }

  if (fixes.length > 0) {
    writeFileSync(gluePath, lines.join('\n'));
    console.log(
      `[kernel-wasm] rewired ${fixes.length} aliased export call(s):\n  ` +
        fixes.join('\n  '),
    );
  }
}

const here = dirname(fileURLToPath(import.meta.url));
const pkgRoot = join(here, '..');
const cratePath = join(pkgRoot, '..', '..', 'crates', 'vcad-kernel-wasm');
const outDir = join(pkgRoot, 'pkg');

// `--postprocess-only`: skip the wasm-pack build and re-apply the glue
// post-processing (reset hook + aliased-export rewiring) to the artifacts
// already sitting in the package root. For flows that run wasm-pack
// directly (the CI recipe in .github/workflows/ci.yml) instead of this
// script.
if (process.argv.includes('--postprocess-only')) {
  ensureResetHook(join(pkgRoot, 'vcad_kernel_wasm.js'));
  normalizeGlue(
    join(pkgRoot, 'vcad_kernel_wasm.js'),
    join(pkgRoot, 'vcad_kernel_wasm_bg.wasm'),
  );
  console.log('[kernel-wasm] postprocess complete');
  process.exit(0);
}

if (process.env.VCAD_WASM_SKIP) {
  console.log(
    '[kernel-wasm] VCAD_WASM_SKIP set — using checked-in artifacts',
  );
  process.exit(0);
}

const result = spawnSync(
  'wasm-pack',
  ['build', cratePath, '--target', 'web', '--out-dir', outDir],
  { stdio: 'inherit', shell: process.platform === 'win32' },
);

if (result.error || result.status !== 0) {
  console.error(
    `[kernel-wasm] wasm-pack failed (${result.error ?? `exit ${result.status}`})`,
  );
  process.exit(result.status ?? 1);
}

for (const file of readdirSync(outDir)) {
  if (file.startsWith('vcad_kernel_wasm')) {
    copyFileSync(join(outDir, file), join(pkgRoot, file));
  }
}
ensureResetHook(join(pkgRoot, 'vcad_kernel_wasm.js'));
normalizeGlue(
  join(pkgRoot, 'vcad_kernel_wasm.js'),
  join(pkgRoot, 'vcad_kernel_wasm_bg.wasm'),
);
console.log('[kernel-wasm] build complete');
