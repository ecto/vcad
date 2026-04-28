#!/usr/bin/env node

/**
 * Publish workspace crates to crates.io in dependency order.
 *
 * Walks `cargo metadata`, drops anything marked `publish = false`, sorts the
 * remainder topologically by intra-workspace deps, and runs `cargo publish`
 * on each crate that isn't already at the current version on crates.io.
 *
 * The script is idempotent: re-running after a successful (or partial) run
 * skips crates whose current version is already on the registry, so a
 * mid-run failure can be retried by just running it again. After each
 * successful publish it polls crates.io until the new version is indexed,
 * which is required so the next crate's `cargo publish` can resolve it.
 *
 * Usage:
 *   node scripts/publish-crates.mjs              # publish for real
 *   node scripts/publish-crates.mjs --dry-run    # cargo publish --dry-run, skip indexing wait
 *   node scripts/publish-crates.mjs --from <name>  # resume from a specific crate
 *   node scripts/publish-crates.mjs --only <name>  # publish a single crate, ignoring order
 *   node scripts/publish-crates.mjs --list       # print resolved order and exit
 *
 * Env:
 *   CARGO_REGISTRY_TOKEN — required for non-dry-run publishes.
 */

import { execFileSync, spawnSync } from 'child_process';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, '..');

const INDEX_POLL_TIMEOUT_MS = 90_000;
const INDEX_POLL_INTERVAL_MS = 5_000;

function parseArgs(argv) {
  const args = { dryRun: false, list: false, from: null, only: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--dry-run') args.dryRun = true;
    else if (a === '--list') args.list = true;
    else if (a === '--from') args.from = argv[++i];
    else if (a === '--only') args.only = argv[++i];
    else {
      console.error(`unknown arg: ${a}`);
      process.exit(2);
    }
  }
  return args;
}

function loadMetadata() {
  const out = execFileSync(
    'cargo',
    ['metadata', '--format-version', '1', '--no-deps'],
    { cwd: root, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 },
  );
  return JSON.parse(out);
}

// Build the publishable subgraph and a topological order over it.
// Cargo encodes `publish = false` as `publish: []` and the default (any
// registry) as `publish: null`; explicit ["crates-io"] is also publishable.
function buildPublishableOrder(metadata) {
  const wsMembers = new Set(metadata.workspace_members);
  const allByName = new Map();
  for (const pkg of metadata.packages) {
    if (wsMembers.has(pkg.id)) allByName.set(pkg.name, pkg);
  }

  const publishable = new Map();
  for (const [name, pkg] of allByName) {
    const isPub = pkg.publish === null || (Array.isArray(pkg.publish) && pkg.publish.length > 0);
    if (isPub) publishable.set(name, pkg);
  }

  // Edges: A -> B means A depends on B. Restrict to deps that are themselves
  // publishable workspace members (so that a publishable crate only waits
  // on other publishable crates). dev-dependencies are excluded since
  // cargo strips them on publish.
  const inEdges = new Map();
  const outEdges = new Map();
  for (const name of publishable.keys()) {
    inEdges.set(name, new Set());
    outEdges.set(name, new Set());
  }
  for (const [name, pkg] of publishable) {
    for (const dep of pkg.dependencies) {
      if (dep.kind === 'dev') continue;
      if (!publishable.has(dep.name)) continue;
      if (dep.name === name) continue;
      inEdges.get(name).add(dep.name);
      outEdges.get(dep.name).add(name);
    }
  }

  // Kahn's. Tie-break alphabetically for stable output.
  const ready = [];
  for (const [name, ins] of inEdges) {
    if (ins.size === 0) ready.push(name);
  }
  ready.sort();
  const order = [];
  while (ready.length) {
    const n = ready.shift();
    order.push(n);
    for (const m of outEdges.get(n)) {
      inEdges.get(m).delete(n);
      if (inEdges.get(m).size === 0) {
        ready.push(m);
        ready.sort();
      }
    }
  }
  if (order.length !== publishable.size) {
    const stuck = [...publishable.keys()].filter((n) => !order.includes(n));
    throw new Error(`dependency cycle among publishable crates: ${stuck.join(', ')}`);
  }
  return { order, publishable };
}

async function isVersionOnCratesIo(name, version) {
  const url = `https://crates.io/api/v1/crates/${name}/${version}`;
  try {
    const res = await fetch(url, { headers: { 'User-Agent': 'vcad-publish-crates (cam@campedersen.com)' } });
    if (res.status === 404) return false;
    if (!res.ok) {
      console.error(`  warn: crates.io returned ${res.status} for ${name}/${version}; assuming not indexed yet`);
      return false;
    }
    return true;
  } catch (err) {
    console.error(`  warn: crates.io fetch failed for ${name}: ${err.message}`);
    return false;
  }
}

async function waitForIndex(name, version) {
  const start = Date.now();
  while (Date.now() - start < INDEX_POLL_TIMEOUT_MS) {
    if (await isVersionOnCratesIo(name, version)) return true;
    await new Promise((r) => setTimeout(r, INDEX_POLL_INTERVAL_MS));
  }
  return false;
}

function runCargoPublish(name, dryRun) {
  const args = ['publish', '-p', name];
  if (dryRun) {
    // Local dirty checkouts are common during dev; CI tags are always clean.
    // Bypassing the dirty check is harmless under --dry-run since nothing
    // gets uploaded.
    args.push('--dry-run', '--allow-dirty');
  }
  const r = spawnSync('cargo', args, { cwd: root, stdio: 'inherit' });
  if (r.status !== 0) {
    throw new Error(`cargo publish -p ${name} exited with status ${r.status}`);
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));

  if (!args.dryRun && !args.list && !process.env.CARGO_REGISTRY_TOKEN) {
    console.error('error: CARGO_REGISTRY_TOKEN is not set; pass --dry-run to skip publish.');
    process.exit(1);
  }

  const metadata = loadMetadata();
  const { order, publishable } = buildPublishableOrder(metadata);

  let queue = order;
  if (args.only) {
    if (!publishable.has(args.only)) {
      console.error(`error: ${args.only} is not a publishable workspace member.`);
      process.exit(1);
    }
    queue = [args.only];
  } else if (args.from) {
    const idx = order.indexOf(args.from);
    if (idx < 0) {
      console.error(`error: --from ${args.from} not found in publish order.`);
      process.exit(1);
    }
    queue = order.slice(idx);
  }

  if (args.list) {
    for (const name of queue) {
      const v = publishable.get(name).version;
      console.log(`${name}\t${v}`);
    }
    return;
  }

  console.log(`Publishing ${queue.length} crate(s)${args.dryRun ? ' (dry run)' : ''}:`);
  for (const name of queue) console.log(`  ${name} ${publishable.get(name).version}`);
  console.log();

  const dryRunFailures = [];

  for (const name of queue) {
    const version = publishable.get(name).version;
    process.stdout.write(`[${name} ${version}] `);

    if (!args.dryRun && (await isVersionOnCratesIo(name, version))) {
      console.log('already on crates.io, skipping.');
      continue;
    }
    console.log(args.dryRun ? 'dry-running cargo publish...' : 'publishing...');

    try {
      runCargoPublish(name, args.dryRun);
    } catch (err) {
      // Under --dry-run, downstream crates can't resolve their deps until
      // the upstream is actually published. Treat failures as warnings and
      // keep walking so the user sees any standalone packaging issues
      // (missing keywords, oversized package, etc.) for later crates too.
      if (args.dryRun) {
        console.error(`  warn: ${err.message} — continuing dry-run.`);
        dryRunFailures.push(name);
        continue;
      }
      throw err;
    }

    if (!args.dryRun) {
      process.stdout.write(`[${name} ${version}] waiting for crates.io to index... `);
      const indexed = await waitForIndex(name, version);
      console.log(indexed ? 'done.' : 'timed out (continuing anyway).');
    }
  }

  if (args.dryRun && dryRunFailures.length) {
    console.log(
      `\n${dryRunFailures.length} dry-run failure(s) — typically due to ` +
        `unpublished workspace deps. Real publish will resolve these as ` +
        `each upstream crate goes live.`,
    );
    for (const name of dryRunFailures) console.log(`  - ${name}`);
  }
  console.log('\nDone.');
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
