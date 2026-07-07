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

const here = dirname(fileURLToPath(import.meta.url));
const pkgRoot = join(here, '..');
const cratePath = join(pkgRoot, '..', '..', 'crates', 'vcad-kernel-wasm');
const outDir = join(pkgRoot, 'pkg');

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

/**
 * Rewrite glue call sites that wasm-bindgen canonicalized onto a *different*
 * member's export name.
 *
 * LLVM's identical-code-folding merges Rust functions that compile to the same
 * body (e.g. two f64 field getters at the same struct offset). All export
 * names survive in the .wasm as aliases of the merged function, but the glue
 * generator picks ONE alias for every call site — which rustc version decides,
 * so a toolchain bump can leave `SliceResult.filamentGrams` calling
 * `wasm.circuitsim_dt`. Runtime-identical, but it is exactly the shape the
 * glue-drift gate (packages/engine kernel-wasm-glue.test.ts) forbids, because
 * a REAL mis-wiring looks the same. Restore the invariant: every member calls
 * its own same-named export whenever that export exists in the wasm.
 */
function canonicalizeAliasedExportCalls(gluePath, dtsPath) {
  const exports = new Set(
    [...readFileSync(dtsPath, 'utf8').matchAll(/^export const (\w+):/gm)].map(
      (m) => m[1],
    ),
  );
  const lines = readFileSync(gluePath, 'utf8').split('\n');
  let cls = null;
  let member = null;
  let freeFn = null;
  let rewrites = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const clsM = line.match(/^export class (\w+)/);
    if (clsM) {
      cls = clsM[1];
      member = null;
      continue;
    }
    if (cls && /^}/.test(line)) {
      cls = null;
      continue;
    }
    // The export name a call inside the current member SHOULD use.
    let expected = null;
    if (cls) {
      // Method definitions sit at 4-space indent; bodies at 8+. Setters
      // export as `classname_set_member`, getters as plain `classname_member`.
      const mm = line.match(/^ {4}(?:static\s+)?(get\s+|set\s+)?(\w+)\s*\(/);
      if (mm) member = mm[1]?.trim() === 'set' ? `set_${mm[2]}` : mm[2];
      if (member) {
        expected =
          member === 'constructor'
            ? `${cls.toLowerCase()}_new`
            : `${cls.toLowerCase()}_${member}`;
      }
    } else {
      const fm = line.match(/^export function (\w+)\s*\(/);
      if (fm) freeFn = fm[1];
      if (/^}/.test(line)) freeFn = null;
      expected = freeFn;
    }
    if (!expected || !exports.has(expected)) continue;
    lines[i] = line.replace(/wasm\.(\w+)\(/g, (whole, name) => {
      if (name.startsWith('__') || name === expected) return whole;
      rewrites++;
      return `wasm.${expected}(`;
    });
  }
  if (rewrites > 0) {
    writeFileSync(gluePath, lines.join('\n'));
    console.log(
      `[kernel-wasm] re-canonicalized ${rewrites} aliased export call(s)`,
    );
  }
}

for (const file of readdirSync(outDir)) {
  if (file.startsWith('vcad_kernel_wasm')) {
    copyFileSync(join(outDir, file), join(pkgRoot, file));
  }
}
canonicalizeAliasedExportCalls(
  join(pkgRoot, 'vcad_kernel_wasm.js'),
  join(pkgRoot, 'vcad_kernel_wasm_bg.wasm.d.ts'),
);
ensureResetHook(join(pkgRoot, 'vcad_kernel_wasm.js'));
console.log('[kernel-wasm] build complete');
