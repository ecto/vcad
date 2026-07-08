/**
 * Generate the tuning-fork family DXFs via the vcad sheet-metal pipeline.
 *
 * Run from the repo root after building workspaces:
 *   VCAD_WASM_SKIP=1 npm run build --workspaces --if-present
 *   node examples/tuning-fork/generate.mjs
 *
 * Design rationale, model-error budget, and the oracle protocol live in
 * docs/plans/tuning-fork.md. Pitch model: in-plane cantilever tine,
 * f1 = (β1²/2π)·(w/L²)·√(E/12ρ), β1L = 1.8751 — for 304 stainless
 * (E = 193 GPa, ρ = 8000 kg/m³) and w = 6 mm: L = 793.4·w/f² … solved
 * per note below. Sheet thickness (6.35 mm) > w keeps the in-plane mode
 * the fundamental.
 */
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import fs from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const { Engine } = await import(join(here, "../../packages/engine/dist/index.js"));
const sm = await import(join(here, "../../packages/mcp/dist/tools/sheet-metal.js"));

const engine = await Engine.init();

// Fork profile: 6 mm tines (gap 8), 20×14 yoke, 10 mm handle ending in a
// 14 mm lollipop tab with a 5 mm hanging hole. Handle/tab geometry does not
// participate in the tine mode. All dims mm.
const TINE_W = 6;
const outline = (L) => {
  const yTop = 74 + L;
  return [
    [3, 0], [17, 0], [17, 14], [15, 14], [15, 60], [20, 60],
    [20, yTop], [14, yTop], [14, 74], [6, 74], [6, yTop], [0, yTop],
    [0, 60], [5, 60], [5, 14], [3, 14],
  ];
};

// CW circle for hole loops.
const circle = (cx, cy, r, n = 24) =>
  Array.from({ length: n }, (_, i) => {
    const t = (-2 * Math.PI * i) / n;
    return [cx + r * Math.cos(t), cy + r * Math.sin(t)];
  });

// tine length L (mm) for target f1 (Hz): L = sqrt(k·w/f), k = 0.55958·√(E/12ρ)
// 304 stainless: √(193e9/(12·8000)) = 1417.9 m/s → k·w = 793.42·0.006 = 4.76052
const FORKS = [
  { file: "tuning-fork-g4-392hz.dxf", label: "G4", L: 110.2, hz: 392.0 },
  { file: "tuning-fork-440hz.dxf",    label: "A4", L: 104.0, hz: 440.0 },
  { file: "tuning-fork-c5-523hz.dxf", label: "C5", L: 95.4,  hz: 523.25 },
];

for (const { file, label, L, hz } of FORKS) {
  const created = sm.sheetMetalCreate(
    {
      outline: outline(L),
      holes: [circle(10, 7, 2.5)],
      // Vertical label up the handle on the ENGRAVE layer (SCS laser marking).
      engravings: [{ type: "Text", text: label, x: 12.5, y: 20, height: 5, angle: Math.PI / 2 }],
      thickness: 6.35,
      material: "stainless-304",
      shop_profile: "sendcutsend",
      width: 20,
      depth: 74 + L,
    },
    engine,
  );
  const c = JSON.parse(created.content[0].text);
  if (c.violations?.length > 0) {
    throw new Error(`${file}: DFM violations: ${JSON.stringify(c.violations)}`);
  }
  const u = JSON.parse(
    sm.sheetMetalUnfold({ document_id: c.document_id, include_dxf: true }, engine).content[0].text,
  );
  fs.writeFileSync(join(here, file), u.dxf);
  console.log(`${file}: predicted ${hz} Hz, bbox ${JSON.stringify(u.flat_pattern.bbox)}`);
}
