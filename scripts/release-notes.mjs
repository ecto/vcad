#!/usr/bin/env node

/**
 * Generate Markdown release notes for a vcad version.
 *
 * Pulls entries from CHANGELOG.json matching the target version, pairs them
 * with recent commit subjects for color, and emits Markdown on stdout.
 *
 * When ANTHROPIC_API_KEY is set (and --raw isn't passed), the notes are
 * rewritten by Claude into a friendly narrative. Falls back to a deterministic
 * template on API failure or missing key — CI never blocks on a missing secret.
 *
 * Usage:
 *   node scripts/release-notes.mjs              # current (first) version
 *   node scripts/release-notes.mjs 0.9.1        # explicit version
 *   node scripts/release-notes.mjs --raw        # skip humanization
 *   node scripts/release-notes.mjs --json       # emit { version, notes } JSON
 */

import { execSync } from 'child_process';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { loadEntries } from './build-changelog.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, '..');

const CATEGORY_LABELS = {
  feat: 'New',
  fix: 'Fixed',
  breaking: 'Breaking changes',
  perf: 'Performance',
  docs: 'Docs',
};
const CATEGORY_ORDER = ['breaking', 'feat', 'fix', 'perf', 'docs'];

// Until the macOS build is notarized with an Apple Developer ID, the
// downloaded .app is unsigned and Gatekeeper blocks first launch with
// "Apple could not verify 'vcad.app' is free of malware". Append the
// bypass to every release page so users hitting that dialog don't have
// to dig through the README. Drop this once Apple signing is wired up.
const MACOS_FIRST_LAUNCH_FOOTER = `### Installing on macOS

This build is not yet Apple-notarized, so first launch shows
*"Apple could not verify 'vcad.app' is free of malware"*. To open it:

- **macOS 15+** — System Settings → Privacy & Security → "Open Anyway".
- **Earlier macOS** — right-click \`vcad.app\` in /Applications → Open.
- **Terminal** — \`xattr -d com.apple.quarantine /Applications/vcad.app\`.

You only need to do this once per install.`;

function parseArgs(argv) {
  const args = { version: null, json: false, raw: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--json') args.json = true;
    else if (a === '--raw') args.raw = true;
    else if (a === '--version') args.version = argv[++i];
    else if (!a.startsWith('-')) args.version = a;
  }
  return args;
}

function entriesForVersion(entries, version) {
  return entries.filter((e) => e.version === version);
}

function previousVersion(entries, version) {
  for (const e of entries) {
    if (e.version !== version) return e.version;
  }
  return null;
}

function recentCommitSubjects(count = 50) {
  try {
    const out = execSync(`git log -n ${count} --no-merges --format=%s`, {
      cwd: root,
      encoding: 'utf8',
    });
    return out.trim().split('\n').filter(Boolean);
  } catch {
    return [];
  }
}

function renderDeterministic(entries, version) {
  const headline = entries.find((e) => e.title === `v${version}`);
  const body = entries.filter((e) => e !== headline);
  const buckets = {};
  for (const e of body) {
    (buckets[e.category] ??= []).push(e);
  }

  const out = [];
  if (headline?.summary) {
    out.push(headline.summary, '');
  }
  for (const cat of CATEGORY_ORDER) {
    const list = buckets[cat];
    if (!list?.length) continue;
    out.push(`### ${CATEGORY_LABELS[cat]}`, '');
    for (const e of list) {
      out.push(`- **${e.title}** — ${e.summary}`);
      if (e.breaking?.migration) {
        out.push(`  - Migration: ${e.breaking.migration}`);
      }
      if (e.mcpTools?.length) {
        out.push(
          `  - MCP tools: ${e.mcpTools.map((t) => '`' + t + '`').join(', ')}`,
        );
      }
    }
    out.push('');
  }
  return out.join('\n').trimEnd() + '\n';
}

async function humanize({ version, entries, commits, prevVersion }) {
  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) return null;

  let Anthropic;
  try {
    ({ default: Anthropic } = await import('@anthropic-ai/sdk'));
  } catch {
    console.error(
      '[release-notes] @anthropic-ai/sdk not installed; run `npm ci` at the repo root',
    );
    return null;
  }

  const client = new Anthropic({ apiKey });

  const system =
    'You write release notes for vcad, an open-source parametric CAD app. ' +
    'Tone is friendly and concise — the voice of a thoughtful project changelog, not a marketing blurb. ' +
    'Open with a short paragraph (one or two sentences) capturing the gist of the release. ' +
    'Then group items under Markdown H3 headings in this order: "Breaking changes", "New", "Fixed", "Performance", "Docs". ' +
    'Omit any heading with no items. Within each group, use a list. ' +
    'Bold the feature name, then an em-dash, then a plain-English sentence. ' +
    'Preserve migration notes verbatim, wrap MCP tool names in backticks, and keep concrete numbers from the source. ' +
    'Never invent features, fixes, or numbers that are not in the provided input. ' +
    'Output Markdown only — no preamble, no sign-off, no "Release notes for..." header.';

  const userLines = [
    `Release: vcad v${version}`,
    prevVersion ? `Previous release: v${prevVersion}` : '',
    '',
    'Changelog entries for this version (JSON):',
    JSON.stringify(entries, null, 2),
  ];
  if (commits.length) {
    userLines.push(
      '',
      'Recent commit subjects, for flavor only — do not cite directly:',
      ...commits.slice(0, 30).map((c) => `- ${c}`),
    );
  }

  const stream = client.messages.stream({
    model: 'claude-opus-4-7',
    max_tokens: 4096,
    system,
    messages: [{ role: 'user', content: userLines.join('\n') }],
  });

  const message = await stream.finalMessage();
  const textBlock = message.content.find((b) => b.type === 'text');
  const text = textBlock?.text ?? '';
  if (!text.trim()) return null;
  return text.trim() + '\n';
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const allEntries = loadEntries();
  const version = args.version ?? allEntries[0]?.version;
  if (!version) {
    console.error('Could not determine version; changelog/entries/ has no entries.');
    process.exit(1);
  }
  const entries = entriesForVersion(allEntries, version);
  if (!entries.length) {
    console.error(`No changelog entries for v${version}.`);
    process.exit(1);
  }

  const prev = previousVersion(allEntries, version);
  const commits = recentCommitSubjects(50);

  let notes = null;
  if (!args.raw) {
    try {
      notes = await humanize({ version, entries, commits, prevVersion: prev });
    } catch (err) {
      console.error(`[release-notes] humanize failed: ${err.message}`);
      console.error('[release-notes] falling back to deterministic template');
      notes = null;
    }
  }
  if (!notes) notes = renderDeterministic(entries, version);

  notes = notes.trimEnd() + '\n\n' + MACOS_FIRST_LAUNCH_FOOTER + '\n';

  if (args.json) {
    process.stdout.write(JSON.stringify({ version, notes }) + '\n');
  } else {
    process.stdout.write(notes);
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
