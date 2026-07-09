// engine-card — live kernel renderings anywhere on the site.
// Drop <canvas class="engine-card" data-part="chamfer"></canvas> on any page:
// when it approaches the viewport the shared kernel loads, the part builds
// once, a frame draws immediately (hidden-tab safe), and one rAF loop turns
// every card slowly. Reduced motion gets the still frame.

import { loadKernel, extractEdges, meshBounds, drawWireframe, drawMolecule, TILT } from "./wireframe.js";

const PARTS = {
  // /design — one card per domain, each a real kernel program
  mechanical(w) {
    const { Solid } = w;
    const block = Solid.cube(110, 80, 45);
    const ch = block.chamfer(6);
    const bore = Solid.cylinder(14, 60, 24).translate(55, 40, -5);
    const part = ch.difference(bore);
    block.free(); ch.free(); bore.free();
    return [{ solid: part }];
  },
  sheetmetal(w) {
    const chain = [
      { type: "BaseFlangeRect", width: 100, depth: 70, thickness: 2, material: "al-soft" },
      { type: "EdgeFlange", panelId: 0, edgeIndex: 0, length: 28, angle: 1.31, direction: "Up" },
      { type: "EdgeFlange", panelId: 0, edgeIndex: 2, length: 28, angle: 1.31, direction: "Up" },
    ];
    const res = JSON.parse(w.evaluateSheetMetalChain(JSON.stringify(chain)));
    if (res.error) throw new Error(res.error);
    return [{ mesh: res.mesh }];
  },
  electronics(w) {
    const { Solid } = w;
    const board = Solid.cube(100, 70, 6);
    const via = Solid.cylinder(2.2, 20, 10).translate(14, 13, -5);
    const grid = via.linearPattern(1, 0, 0, 4, 24).linearPattern(0, 1, 0, 3, 22);
    const part = board.difference(grid);
    board.free(); via.free(); grid.free();
    return [{ solid: part }];
  },
  simulation(w) {
    const { Solid } = w;
    const base = Solid.cube(84, 26, 14);
    const f1 = Solid.cube(11, 44, 12).rotate(0, 0, 14).translate(16, 24, 2);
    const f2 = Solid.cube(11, 44, 12).rotate(0, 0, -14).translate(60, 26, 2);
    return [{ solid: base }, { solid: f1 }, { solid: f2 }];
  },
  drafting(w) {
    const { Solid } = w;
    const block = Solid.cube(110, 45, 80); // stood up: front elevation
    const ch = block.chamfer(6);
    const bore = Solid.cylinder(14, 60, 24).rotate(90, 0, 0).translate(55, 50, 40);
    const part = ch.difference(bore);
    block.free(); ch.free(); bore.free();
    return [{ solid: part }];
  },
  atoms(w) {
    const lines = [];
    const S = 26;
    for (let i = 0; i < 6; i++) {
      const a = (i * Math.PI * 2) / 6;
      lines.push(`C ${(1.396 * Math.cos(a)).toFixed(3)} ${(1.396 * Math.sin(a)).toFixed(3)} 0.000`);
      lines.push(`H ${(2.48 * Math.cos(a)).toFixed(3)} ${(2.48 * Math.sin(a)).toFixed(3)} 0.000`);
    }
    const sys = JSON.parse(w.atoms_parse_xyz(`${lines.length}\nbenzene\n${lines.join("\n")}\n`));
    const pts = sys.positions.map((p) => [p[0] * S, p[1] * S, p[2] * S]);
    return [{
      mol: {
        pts,
        bonds: (sys.bonds || []).map((b) => [b.a, b.b]),
        elems: sys.speciesIdx.map((i) => (sys.species[i].element || "C")[0]),
      },
    }];
  },
};

// drafting cards render flat (front elevation) with an orange dimension — a drawing
const FLAT = new Set(["drafting"]);

const cards = [];
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
let looping = false;

function buildCard(canvas, wasm) {
  const key = canvas.dataset.part;
  const maker = PARTS[key];
  if (!maker) return;
  try {
    const bodies = maker(wasm).map((b) => {
      if (b.mol) return { kind: "mol", ...b.mol, ...meshBounds(b.mol.pts.flat()) };
      const m = b.solid ? b.solid.getMesh(20) : null;
      const positions = b.solid ? m.positions : Float32Array.from(b.mesh.positions);
      const indices = b.solid ? m.indices : Uint32Array.from(b.mesh.indices);
      const out = { kind: "mesh", positions, edges: extractEdges(positions, indices), ...meshBounds(positions) };
      b.solid?.free();
      return out;
    });
    // frame the union of all bodies
    const extent = Math.max(...bodies.map((b) => b.extent)) * (bodies.length > 1 ? 1.7 : 1.25);
    const card = { canvas, bodies, extent, flat: FLAT.has(key), spin: 0.55, seed: Math.abs(hash(key)) % 100 };
    cards.push(card);
    render(card);
    startLoop();
  } catch (e) {
    console.warn(`engine-card "${key}" failed:`, e);
  }
}

function hash(s) { let h = 0; for (const c of s) h = (h * 31 + c.charCodeAt(0)) | 0; return h; }

function render(card, t = 0) {
  const { canvas } = card;
  const dpr = Math.min(devicePixelRatio || 1, 2);
  canvas.width = canvas.clientWidth * dpr;
  canvas.height = canvas.clientHeight * dpr;
  const ctx = canvas.getContext("2d");
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  const spin = card.flat ? 0 : card.spin + card.seed + t * 0.00013;
  const tilt = card.flat ? 0 : TILT;
  const n = card.bodies.length;
  card.bodies.forEach((body, i) => {
    const ax = n === 1 ? canvas.width / 2 : canvas.width * (0.5 + (i - (n - 1) / 2) * 0.02);
    const opts = {
      spin, tilt,
      ax,
      ay: canvas.height * 0.52,
      s: Math.min(canvas.width, canvas.height) / card.extent,
      alpha: 0.55, dpr,
    };
    if (body.kind === "mol") drawMolecule(ctx, body, opts);
    else drawWireframe(ctx, body, opts);
  });
  if (card.flat) {
    // the drawing gets a dimension — orange, like every annotation
    const y = canvas.height * 0.86;
    const x1 = canvas.width * 0.22, x2 = canvas.width * 0.78;
    ctx.strokeStyle = "rgba(242, 92, 31, 0.7)";
    ctx.lineWidth = 1 * dpr;
    ctx.beginPath();
    ctx.moveTo(x1, y); ctx.lineTo(x2, y);
    ctx.moveTo(x1, y - 4 * dpr); ctx.lineTo(x1, y + 4 * dpr);
    ctx.moveTo(x2, y - 4 * dpr); ctx.lineTo(x2, y + 4 * dpr);
    ctx.stroke();
    ctx.fillStyle = "rgba(242, 92, 31, 0.8)";
    ctx.font = `${9 * dpr}px "JetBrains Mono", monospace`;
    ctx.fillText("110.0", (x1 + x2) / 2 - 12 * dpr, y - 4 * dpr);
  }
}

function startLoop() {
  if (looping || reduced) return;
  looping = true;
  const tick = (t) => {
    if (!document.hidden) for (const c of cards) render(c, t);
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

/* boot: lazy, shared, gated like the hero engine */
const canvases = [...document.querySelectorAll("canvas.engine-card")];
if (canvases.length && !navigator.connection?.saveData) {
  let booted = false;
  const boot = () => {
    if (booted) return;
    booted = true;
    loadKernel()
      .then((wasm) => canvases.forEach((c) => buildCard(c, wasm)))
      .catch((e) => console.warn("engine cards unavailable:", e));
  };
  const io = new IntersectionObserver((entries) => {
    if (entries.some((e) => e.isIntersecting)) { io.disconnect(); boot(); }
  }, { rootMargin: "500px" });
  canvases.forEach((c) => io.observe(c));
  // IO never fires in hidden documents — boot on a timer as the fallback
  setTimeout(boot, 4000);
}
