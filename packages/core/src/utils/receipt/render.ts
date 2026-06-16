/** Receipt renderers — a terminal/markdown view and a standalone HTML artifact.
 *  Both consume the pure `Receipt` object from the engine; no recompute here. */

import type { Receipt, ReceiptEntry, ViolationGroup, Verdict } from "./types.js";

const VERDICT_LABEL: Record<Verdict, string> = {
  "no-op": "NO-OP",
  clean: "CLEAN",
  improved: "IMPROVED",
  "improved-with-regressions": "IMPROVED — WITH REGRESSIONS",
  regression: "REGRESSION",
};

const CAUSE_LABEL: Record<string, string> = {
  footprint: "footprint (pad pitch)",
  placement: "placement (pad↔pad)",
  routing: "routing",
  via: "via / hole-to-hole",
  connectivity: "unrouted net",
  unknown: "unknown",
};

const fp = (s: string) => `${s.slice(0, 12)}…`;
const signed = (n: number) => (n > 0 ? `+${n}` : `${n}`);

function ruleRows(entry: ReceiptEntry): Array<{ rule: string; b: number; a: number; d: number }> {
  const rules = new Set([...Object.keys(entry.before.byRule), ...Object.keys(entry.after.byRule)]);
  return [...rules]
    .map((rule) => ({
      rule,
      b: entry.before.byRule[rule] ?? 0,
      a: entry.after.byRule[rule] ?? 0,
      d: (entry.after.byRule[rule] ?? 0) - (entry.before.byRule[rule] ?? 0),
    }))
    .sort((x, y) => Math.abs(y.d) - Math.abs(x.d) || y.a - x.a);
}

/** Roll the engine's per-position groups up to one chip per (rule, cause) for
 *  display — the engine keeps them fine-grained; the summary shouldn't. */
function rollup(groups: ViolationGroup[]): ViolationGroup[] {
  const map = new Map<string, ViolationGroup>();
  for (const g of groups) {
    const key = `${g.rule}|${g.cause}|${g.blame}`;
    const ex = map.get(key);
    if (ex) ex.count += g.count;
    else map.set(key, { ...g });
  }
  return [...map.values()].sort(
    (x, y) =>
      (/^\s*Short\b/i.test(x.message) ? 0 : 1) - (/^\s*Short\b/i.test(y.message) ? 0 : 1) ||
      y.count - x.count,
  );
}

function groupLine(g: ViolationGroup): string {
  const cause = CAUSE_LABEL[g.cause] ?? g.cause;
  return `${g.count}× ${g.rule} — ${cause}`;
}

// ---------------------------------------------------------------------------
// Terminal / markdown
// ---------------------------------------------------------------------------

export function renderReceiptText(receipt: Receipt): string {
  const L: string[] = [];
  const title = receipt.board.title ?? "PCB";
  L.push(`RECEIPT — ${title}`);
  L.push(
    `  ${receipt.board.components ?? "?"} components · ${receipt.board.nets?.length ?? "?"} nets · build ${receipt.build.version} (${receipt.build.sha})`,
  );
  if (receipt.preflight?.unconnectedPins?.length) {
    L.push(`  pre-flight: ${receipt.preflight.unconnectedPins.length} pin(s) in no net — ${receipt.preflight.unconnectedPins.join(", ")}`);
  }
  L.push("");

  for (const e of receipt.entries) {
    L.push(`#${e.index + 1}  ${e.tool}(${shortArgs(e.args)})   →   ${VERDICT_LABEL[e.verdict]}`);
    L.push(`     DRC ${e.before.errors} → ${e.after.errors} errors  (naive Δ ${signed(e.deltaTotal)})`);
    // per-rule delta — the hero table
    for (const r of ruleRows(e)) {
      const mark = r.d < 0 ? "✓" : r.d > 0 ? "✗" : " ";
      L.push(`       ${mark} ${r.rule.padEnd(16)} ${String(r.b).padStart(3)} → ${String(r.a).padStart(3)}   ${signed(r.d)}`);
    }
    // attributed story
    if (e.fixed.length) {
      L.push(`     ✓ fixed (credit): ${rollup(e.fixed).map(groupLine).join("; ")}`);
    }
    if (e.introduced.length) {
      L.push(`     ✗ introduced (this mutation's fault): ${rollup(e.introduced).map(groupLine).join("; ")}`);
      if (e.tally.shortsIntroduced) {
        L.push(`       ‼ ${e.tally.shortsIntroduced} hard SHORT(s) — board is electrically broken`);
      }
    }
    if (e.tally.preExisting) {
      const pe = rollup(e.persisted.filter((g) => g.blame === "pre-existing"));
      L.push(`     · pre-existing, NOT the agent's fault: ${pe.map(groupLine).join("; ")}`);
    }
    L.push(`     fingerprint ${fp(e.fingerprint)}  · re-run: ${receipt.reverification}  · coverage: ${e.coverage}`);
    L.push("");
  }
  return L.join("\n");
}

function shortArgs(args: Record<string, unknown>): string {
  const a = { ...args };
  delete (a as Record<string, unknown>).document_id;
  delete (a as Record<string, unknown>).document;
  const s = JSON.stringify(a);
  return s === "{}" ? "all" : s;
}

// ---------------------------------------------------------------------------
// Standalone HTML
// ---------------------------------------------------------------------------

const esc = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function verdictColor(v: Verdict): string {
  if (v === "regression") return "#ff5c5c";
  if (v === "improved-with-regressions") return "#f5a623";
  if (v === "improved" || v === "clean") return "#3ddc84";
  return "#8a8f98";
}

function entryHtml(e: ReceiptEntry, rev: string): string {
  const rows = ruleRows(e)
    .map((r) => {
      const color = r.d < 0 ? "#3ddc84" : r.d > 0 ? "#ff5c5c" : "#8a8f98";
      return `<tr><td>${esc(r.rule)}</td><td class="num">${r.b}</td><td class="num">${r.a}</td><td class="num" style="color:${color};font-weight:600">${signed(r.d)}</td></tr>`;
    })
    .join("");

  const chips = (gs: ViolationGroup[], cls: string) =>
    gs
      .map(
        (g) =>
          `<span class="chip ${cls}">${g.count}× ${esc(g.rule)} <em>${esc(CAUSE_LABEL[g.cause] ?? g.cause)}</em></span>`,
      )
      .join(" ");

  const preExisting = rollup(e.persisted.filter((g) => g.blame === "pre-existing"));
  const shortBanner = e.tally.shortsIntroduced
    ? `<div class="banner">‼ ${e.tally.shortsIntroduced} hard short${e.tally.shortsIntroduced > 1 ? "s" : ""} introduced — the board is electrically broken, and <code>route_nets</code> reported only <code>{document_id}</code>.</div>`
    : "";

  return `
  <div class="entry">
    <div class="entry-head">
      <div class="step">#${e.index + 1} &nbsp;<code>${esc(e.tool)}(${esc(shortArgs(e.args))})</code></div>
      <div class="verdict" style="background:${verdictColor(e.verdict)}">${VERDICT_LABEL[e.verdict]}</div>
    </div>
    <div class="counts">DRC <b>${e.before.errors}</b> → <b>${e.after.errors}</b> errors <span class="muted">(naive Δ ${signed(e.deltaTotal)} — the headline that hides the truth)</span></div>
    ${shortBanner}
    <table class="delta">
      <thead><tr><th>rule</th><th class="num">before</th><th class="num">after</th><th class="num">Δ</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
    <div class="attrib">
      ${e.fixed.length ? `<div class="row"><span class="tag credit">FIXED · credit</span>${chips(rollup(e.fixed), "credit")}</div>` : ""}
      ${e.introduced.length ? `<div class="row"><span class="tag blame">INTRODUCED · this mutation</span>${chips(rollup(e.introduced), "blame")}</div>` : ""}
      ${preExisting.length ? `<div class="row"><span class="tag neutral">PRE-EXISTING · not the agent</span>${chips(preExisting, "neutral")}</div>` : ""}
    </div>
    <div class="foot">fingerprint <code>${fp(e.fingerprint)}</code> · re-run: ${esc(rev)} · coverage: ${e.coverage}</div>
  </div>`;
}

export function renderReceiptHtml(receipt: Receipt): string {
  const title = esc(receipt.board.title ?? "PCB");
  const entries = receipt.entries.map((e) => entryHtml(e, receipt.reverification)).join("\n");
  const pre = receipt.preflight?.unconnectedPins?.length
    ? `<div class="preflight">pre-flight: ${receipt.preflight.unconnectedPins.length} pin(s) in no net (${esc(receipt.preflight.unconnectedPins.join(", "))})</div>`
    : "";
  return `<!doctype html><html><head><meta charset="utf-8"><title>Receipt — ${title}</title>
<style>
  :root { color-scheme: dark; }
  body { margin:0; background:#0e0f13; color:#e6e7ea; font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace; padding:28px; }
  .wrap { max-width: 760px; margin: 0 auto; }
  h1 { font-size:18px; margin:0 0 2px; letter-spacing:.02em; }
  .sub { color:#8a8f98; font-size:12px; margin-bottom:18px; }
  .preflight { color:#c9a227; font-size:12px; margin-bottom:14px; }
  .entry { border:1px solid #23252c; border-radius:12px; padding:16px 18px; margin-bottom:16px; background:#15171c; }
  .entry-head { display:flex; align-items:center; justify-content:space-between; gap:12px; }
  .step code { color:#e6e7ea; font-size:13px; }
  .verdict { color:#0e0f13; font-weight:700; font-size:11px; padding:3px 9px; border-radius:999px; letter-spacing:.04em; }
  .counts { margin:10px 0 6px; }
  .muted, .foot { color:#8a8f98; }
  .banner { background:#2a1416; border:1px solid #ff5c5c55; color:#ff8b8b; padding:8px 10px; border-radius:8px; margin:8px 0; font-size:12.5px; }
  table.delta { width:100%; border-collapse:collapse; margin:8px 0 12px; }
  table.delta th { text-align:left; color:#8a8f98; font-weight:500; font-size:11px; border-bottom:1px solid #23252c; padding:4px 6px; }
  table.delta td { padding:4px 6px; border-bottom:1px solid #1b1d23; }
  .num { text-align:right; font-variant-numeric: tabular-nums; }
  .attrib .row { display:flex; flex-wrap:wrap; align-items:center; gap:6px; margin:6px 0; }
  .tag { font-size:10.5px; font-weight:700; padding:2px 7px; border-radius:6px; letter-spacing:.03em; }
  .tag.credit { background:#10331f; color:#3ddc84; }
  .tag.blame { background:#33141a; color:#ff7a7a; }
  .tag.neutral { background:#222; color:#9aa0a8; }
  .chip { font-size:12px; background:#1d2027; border:1px solid #2a2d35; border-radius:6px; padding:2px 8px; }
  .chip em { color:#8a8f98; font-style:normal; }
  .chip.credit { border-color:#21603a; } .chip.blame { border-color:#5a2730; }
  .foot { font-size:11px; margin-top:8px; }
  code { background:#1d2027; padding:1px 5px; border-radius:4px; }
</style></head>
<body><div class="wrap">
  <h1>RECEIPT — ${title}</h1>
  <div class="sub">${receipt.board.components ?? "?"} components · ${receipt.board.nets?.length ?? "?"} nets · build ${esc(receipt.build.version)} (${esc(receipt.build.sha)}) · fingerprint ${receipt.fingerprintAlgo}</div>
  ${pre}
  ${entries}
  <div class="sub">Each entry wraps one agent mutation in a deterministic <code>run_drc</code> snapshot before &amp; after. The mutators returned only <code>{document_id}</code> — every fact above is recovered from the oracle diff, with cause attributed from the DRC message so footprint faults are never blamed on the router. Re-running <code>run_drc</code> reproduces each fingerprint byte-for-byte.</div>
</div></body></html>`;
}
