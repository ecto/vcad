// vcad landing — behaviors per docs/brand-spec.md
// Motion law: G0 (90ms, chrome) / G1 (240ms, geometry). Nothing else.

const $ = (id) => document.getElementById(id);

/* ---------- 01 · boot — triangle touch-off, once per visit ---------- */

const boot = $("boot");
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
if (boot) {
  if (reduced || sessionStorage.getItem("vcad-booted")) {
    boot.classList.add("off");
  } else {
    sessionStorage.setItem("vcad-booted", "1");
    requestAnimationFrame(() => boot.classList.add("landed"));
    setTimeout(() => boot.classList.add("done"), 340);
    setTimeout(() => boot.classList.add("off"), 460);
  }
}

/* ---------- measured headline — the dimension is real ---------- */

const headline = $("heroHeadline");
const measure = $("heroMeasure");
const measureValue = $("heroMeasureValue");
if (headline && measure && measureValue && "ResizeObserver" in window) {
  const update = () => {
    const w = headline.getBoundingClientRect().width;
    measure.style.width = `${w}px`;
    measureValue.textContent = `${w.toFixed(1)} px`;
    measure.classList.add("on");
  };
  new ResizeObserver(update).observe(headline);
  update();
}

/* ---------- agent CTA — dropdown + copy, no incantations ---------- */

const toast = $("copyToast");
let toastTimer;
function copied(text) {
  navigator.clipboard?.writeText(text).catch(() => {});
  if (!toast) return;
  toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toast.hidden = true), 1600);
}

const menu = $("agentMenu");
const chev = $("agentChev");
const main = $("agentMain");
function toggleMenu(open) {
  if (!menu) return;
  const willOpen = open ?? menu.hidden;
  menu.hidden = !willOpen;
  main?.setAttribute("aria-expanded", String(willOpen));
}
chev?.addEventListener("click", (e) => { e.stopPropagation(); toggleMenu(); });
main?.addEventListener("click", (e) => {
  e.stopPropagation();
  copied("claude mcp add --transport http vcad https://mcp.vcad.io/mcp");
  toggleMenu(false);
});
$("agentMain2")?.addEventListener("click", () => {
  copied("claude mcp add --transport http vcad https://mcp.vcad.io/mcp");
});
menu?.querySelectorAll("[data-copy]").forEach((b) =>
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    copied(b.dataset.copy);
    toggleMenu(false);
  })
);
document.addEventListener("click", () => toggleMenu(false));
document.addEventListener("keydown", (e) => { if (e.key === "Escape") toggleMenu(false); });

/* ---------- reveal on scroll (G0) ---------- */

const revealer = new IntersectionObserver(
  (entries) => entries.forEach((en, i) => {
    if (en.isIntersecting) {
      setTimeout(() => en.target.classList.add("in"), (i % 3) * 90);
      revealer.unobserve(en.target);
    }
  }),
  { threshold: 0.25 }
);
document.querySelectorAll(".reveal").forEach((el) => revealer.observe(el));

/* ---------- 04 · demo — the transcript types itself, skippable ---------- */

const demoLines = [
  { el: "demoPrompt", text: '"a bracket for this motor. it has to hold 40 newtons."', type: true },
  { el: "demoL1", html: 'designed · dfm 14 rules <b class="green">✓</b> · fea 40 N → margin <b class="green">2.1×</b>' },
  { el: "demoL2", html: 'quote <b style="color:#F5F5F6">$23.40</b> · ships thursday' },
  { el: "demoL3", html: "make it? ⏎" },
];
let demoDone = false;
function runDemo(skip = false) {
  if (demoDone) return;
  demoDone = true;
  let delay = 0;
  demoLines.forEach((line) => {
    const el = $(line.el);
    if (!el) return;
    if (skip || reduced) {
      if (line.type) el.textContent = line.text;
      else el.innerHTML = line.html;
      return;
    }
    if (line.type) {
      const chars = [...line.text];
      el.classList.add("caret");
      chars.forEach((ch, i) => setTimeout(() => {
        el.textContent += ch;
        if (i === chars.length - 1) el.classList.remove("caret");
      }, delay + i * 28));
      delay += chars.length * 28 + 500;
    } else {
      setTimeout(() => { el.innerHTML = line.html; }, delay);
      delay += 650;
    }
  });
}
const terminal = $("demoTerminal");
if (terminal) {
  new IntersectionObserver((entries, obs) => {
    if (entries.some((e) => e.isIntersecting)) { runDemo(); obs.disconnect(); }
  }, { threshold: 0.5 }).observe(terminal);
  terminal.addEventListener("click", () => {
    demoLines.forEach((l) => { const el = $(l.el); if (el) { el.textContent = ""; el.classList.remove("caret"); } });
    demoDone = false;
    runDemo(true);
  });
}

/* ---------- 06 · receipt — the ring closes as you scroll ---------- */

const CIRC = 182.2;
const ring = $("sealRing");
const count = $("sealCount");
const verdict = $("receiptVerdict");
const checks = [...document.querySelectorAll("#receiptCard [data-check]")];
const receiptSection = $("receiptSection");

function scrubRing() {
  if (!ring || !receiptSection) return;
  const r = receiptSection.getBoundingClientRect();
  const vh = innerHeight;
  // 0 when section top hits bottom of viewport, 1 when section center passes 45% of viewport
  const p = Math.min(1, Math.max(0, (vh - r.top) / (vh * 0.85)));
  ring.style.strokeDashoffset = String(CIRC * (1 - p));
  const passed = Math.round(p * checks.length);
  checks.forEach((c, i) => {
    c.classList.toggle("pass", i < passed);
    c.querySelector(".mark").textContent = i < passed ? "✓" : "·";
  });
  if (count) count.textContent = `${passed}/${checks.length}`;
  if (verdict) {
    verdict.textContent = p >= 1 ? "pass" : "running";
    verdict.className = p >= 1 ? "green" : "t-dim2";
  }
}
if (receiptSection) {
  addEventListener("scroll", scrubRing, { passive: true });
  addEventListener("resize", scrubRing, { passive: true });
  scrubRing();
  if (reduced) {
    ring.style.strokeDashoffset = "0";
    checks.forEach((c) => { c.classList.add("pass"); c.querySelector(".mark").textContent = "✓"; });
    if (count) count.textContent = `${checks.length}/${checks.length}`;
    if (verdict) { verdict.textContent = "pass"; verdict.className = "green"; }
  }
}

/* ---------- dimensioned UI — hold alt, the page measures itself ---------- */

let dimBadges = [];
function setDimensioned(on) {
  document.body.classList.toggle("dimensioned", on);
  dimBadges.forEach((b) => b.remove());
  dimBadges = [];
  if (!on) return;
  document.querySelectorAll("[data-dim]").forEach((el) => {
    const r = el.getBoundingClientRect();
    const badge = document.createElement("span");
    badge.className = "dim-badge";
    badge.textContent = `${Math.round(r.width)} × ${Math.round(r.height)}`;
    el.appendChild(badge);
    dimBadges.push(badge);
  });
}
addEventListener("keydown", (e) => { if (e.key === "Alt" && !e.repeat) setDimensioned(true); });
addEventListener("keyup", (e) => { if (e.key === "Alt") setDimensioned(false); });
addEventListener("blur", () => setDimensioned(false));

/* ---------- hero build loop — model, dimension, chamfer, verify, repeat ---------- */

// set true by the live kernel background; the SVG loop yields the stage
let engineLive = false;
document.addEventListener("vcad-engine-live", () => {
  engineLive = true;
  document.querySelector(".hero")?.classList.add("engine-live");
  const vp = $("heroViewport");
  if (vp) {
    vp.style.opacity = "0";
    vp.style.pointerEvents = "none";
    setTimeout(() => { vp.style.visibility = "hidden"; }, 300);
  }
});

const viewport = $("heroViewport");
const partReceipt = $("partReceipt");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function buildLoop() {
  if (!viewport || !partReceipt) return;
  if (reduced) {
    viewport.dataset.stage = "4";
    partReceipt.innerHTML = '<span class="green">✓ dfm · 0 violations</span> · receipt 9f3a…c2';
    return;
  }
  for (;;) {
    if (engineLive) return;
    viewport.dataset.stage = "0";
    partReceipt.innerHTML = '<span class="t-dim2">g1 · tracing…</span>';
    await wait(120);
    viewport.dataset.stage = "1";           // edges trace at feed rate
    await wait(1000);
    viewport.dataset.stage = "2";           // dimensions land
    partReceipt.innerHTML = '<span class="t-dim2">checking · 14 rules…</span>';
    await wait(900);
    viewport.dataset.stage = "3";           // chamfer proposed (orange = preview)
    await wait(1400);
    viewport.dataset.stage = "4";           // ⏎ committed, checks pass
    partReceipt.innerHTML = '<span class="green">✓ dfm · 0 violations</span> · receipt 9f3a…c2';
    await wait(3400);
    viewport.classList.add("resetting");    // sheet cleared, next part
    await wait(180);
    viewport.classList.remove("resetting");
  }
}
buildLoop();

/* ---------- hover dims — hovering a card measures it ---------- */

document.querySelectorAll(".gallery figure, .terminal").forEach((el) => {
  el.addEventListener("mouseenter", () => {
    let badge = el.querySelector(".hover-dim");
    if (!badge) {
      badge = document.createElement("span");
      badge.className = "hover-dim mono";
      el.appendChild(badge);
    }
    const r = el.getBoundingClientRect();
    badge.textContent = `${Math.round(r.width)} × ${Math.round(r.height)}`;
  });
});

/* ---------- live kernel background — lazy, gated, honest ---------- */

(() => {
  const canvas = $("heroEngine");
  if (!canvas) return;
  const conn = navigator.connection;
  const slowNet = conn?.saveData || /\b[23]g\b/.test(conn?.effectiveType || "");
  if (reduced || slowNet) return; // hard gates
  let booted = false;
  const boot = () => {
    if (booted) return;
    if (innerWidth < 480) return; // true-phone gate, re-checked on resize
    booted = true;
    import("./hero-engine.js")
      .then((m) => m.startHeroEngine(canvas, $("heroEngineCaption")))
      .catch((e) => { booted = false; console.warn("vcad engine background unavailable:", e); });
  };
  const idle = () => {
    if ("requestIdleCallback" in window) requestIdleCallback(boot, { timeout: 2000 });
    else setTimeout(boot, 800);
  };
  if (document.readyState === "complete") idle();
  else addEventListener("load", idle, { once: true });
  addEventListener("resize", () => { if (!booted) boot(); }, { passive: true });
})();

/* ---------- title block — real values or honest ones ---------- */
// Rev is wired at build time; "checked" flips to ✓ ci pass only when CI actually runs.
// TODO(landing-ci): inject rev from package.json + CI status via vite define.
