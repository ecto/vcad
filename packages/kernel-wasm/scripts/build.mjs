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
import { appendFileSync, copyFileSync, readdirSync, readFileSync } from 'fs';
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

for (const file of readdirSync(outDir)) {
  if (file.startsWith('vcad_kernel_wasm')) {
    copyFileSync(join(outDir, file), join(pkgRoot, file));
  }
}
ensureResetHook(join(pkgRoot, 'vcad_kernel_wasm.js'));
console.log('[kernel-wasm] build complete');
