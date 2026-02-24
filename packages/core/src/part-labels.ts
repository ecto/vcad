import type { PartInfo } from "./types.js";

/** Unicode glyphs for every part kind (terminal-safe). */
export const PART_GLYPHS: Record<PartInfo["kind"], string> = {
  cube: "\u25A0",            // ■
  cylinder: "\u25CB",        // ○
  sphere: "\u25CF",          // ●
  boolean: "\u2295",         // ⊕
  extrude: "\u2191",         // ↑
  revolve: "\u21BB",         // ↻
  sweep: "~",
  loft: "\u2261",            // ≡
  "imported-mesh": "\u25B3", // △
  fillet: "\u25E0",          // ◠
  chamfer: "\u2312",         // ⌒
  shell: "\u25A1",           // □
  "linear-pattern": "\u2237",// ∷
  "circular-pattern": "\u25C9", // ◉
  mirror: "\u21C4",          // ⇄
  text: "A",
  "pcb-board": "\u2339",     // ⌹
  "embroidery-pattern": "\u2702", // ✂
  stitch: "\u2702",              // ✂
};

/** Look up the glyph for a part kind. */
export function getPartGlyph(kind: PartInfo["kind"]): string {
  return PART_GLYPHS[kind] ?? "?";
}
