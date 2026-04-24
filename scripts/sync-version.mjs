#!/usr/bin/env node

/**
 * Syncs version from CHANGELOG.json to all package.json files and Cargo.toml
 *
 * Usage:
 *   node scripts/sync-version.mjs        # Update all versions
 *   node scripts/sync-version.mjs --check  # Check if versions are in sync (for CI)
 */

import { readFileSync, writeFileSync, readdirSync, statSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, '..');

const checkOnly = process.argv.includes('--check');

// Read version from CHANGELOG.json (first entry's version)
function getVersion() {
  const changelog = JSON.parse(readFileSync(join(root, 'CHANGELOG.json'), 'utf8'));
  if (!changelog.entries || changelog.entries.length === 0) {
    console.error('Error: CHANGELOG.json has no entries');
    process.exit(1);
  }
  return changelog.entries[0].version;
}

// Find all package.json files in packages/
function getPackageJsonPaths() {
  const packagesDir = join(root, 'packages');
  const packages = [];

  for (const name of readdirSync(packagesDir)) {
    const pkgPath = join(packagesDir, name, 'package.json');
    try {
      statSync(pkgPath);
      packages.push(pkgPath);
    } catch {
      // Skip if no package.json
    }
  }

  return packages;
}

// Update a package.json file
function updatePackageJson(path, version) {
  const content = readFileSync(path, 'utf8');
  const pkg = JSON.parse(content);
  const oldVersion = pkg.version;

  if (oldVersion === version) {
    return { path, changed: false, oldVersion };
  }

  if (checkOnly) {
    return { path, changed: true, oldVersion, newVersion: version };
  }

  pkg.version = version;
  // Preserve formatting by using 2-space indent and trailing newline
  writeFileSync(path, JSON.stringify(pkg, null, 2) + '\n');
  return { path, changed: true, oldVersion, newVersion: version };
}

// Update the Tauri desktop app version. Tauri writes this into the signed
// latest.json manifest the updater consumes, so leaving it stale makes the
// updater lie about the version. Uses a targeted regex instead of
// JSON.stringify to avoid reformatting unrelated arrays.
function updateTauriConf(version) {
  const path = join(root, 'crates/vcad-desktop/tauri.conf.json');
  let content = readFileSync(path, 'utf8');

  // Match the top-level "version": "..." — anchored after the opening brace
  // and before "identifier" so we don't accidentally match a nested field.
  const versionRegex = /^(\s*"version"\s*:\s*)"([^"]+)"/m;
  const match = content.match(versionRegex);

  if (!match) {
    console.error('Error: Could not find version in tauri.conf.json');
    process.exit(1);
  }

  const oldVersion = match[2];

  if (oldVersion === version) {
    return { path, changed: false, oldVersion };
  }

  if (checkOnly) {
    return { path, changed: true, oldVersion, newVersion: version };
  }

  content = content.replace(versionRegex, `$1"${version}"`);
  writeFileSync(path, content);
  return { path, changed: true, oldVersion, newVersion: version };
}

// Update Cargo.toml workspace version
function updateCargoToml(version) {
  const cargoPath = join(root, 'Cargo.toml');
  let content = readFileSync(cargoPath, 'utf8');

  const versionRegex = /(\[workspace\.package\][\s\S]*?version\s*=\s*)"([^"]+)"/;
  const match = content.match(versionRegex);

  if (!match) {
    console.error('Error: Could not find workspace.package version in Cargo.toml');
    process.exit(1);
  }

  const oldVersion = match[2];

  if (oldVersion === version) {
    return { path: cargoPath, changed: false, oldVersion };
  }

  if (checkOnly) {
    return { path: cargoPath, changed: true, oldVersion, newVersion: version };
  }

  content = content.replace(versionRegex, `$1"${version}"`);
  writeFileSync(cargoPath, content);
  return { path: cargoPath, changed: true, oldVersion, newVersion: version };
}

// Main
const version = getVersion();
console.log(`Version from CHANGELOG.json: ${version}\n`);

const results = [];

// Update package.json files
for (const path of getPackageJsonPaths()) {
  results.push(updatePackageJson(path, version));
}

// Update Cargo.toml
results.push(updateCargoToml(version));

// Update Tauri desktop config (drives latest.json's version field)
results.push(updateTauriConf(version));

// Report results
const changed = results.filter(r => r.changed);
const unchanged = results.filter(r => !r.changed);

if (unchanged.length > 0) {
  console.log(`Already at ${version}:`);
  for (const r of unchanged) {
    console.log(`  ${r.path.replace(root + '/', '')}`);
  }
  console.log();
}

if (changed.length > 0) {
  if (checkOnly) {
    console.log('Out of sync:');
    for (const r of changed) {
      console.log(`  ${r.path.replace(root + '/', '')}: ${r.oldVersion} -> ${r.newVersion}`);
    }
    console.log('\nRun `npm run version:sync` to fix.');
    process.exit(1);
  } else {
    console.log('Updated:');
    for (const r of changed) {
      console.log(`  ${r.path.replace(root + '/', '')}: ${r.oldVersion} -> ${r.newVersion}`);
    }
  }
} else {
  console.log('All versions are in sync.');
}
