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
import { copyFileSync, readdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

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
console.log('[kernel-wasm] build complete');
