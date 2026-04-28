// OPERATOR — the project mascot.
//
// A small bipedal mech rendered in line-art SVG. Strokes inherit from
// `currentColor`, so a parent element's `color` controls the ink. Default
// usage is blueprint cyan (var(--ink)) with a hot-orange accent for the
// status light.
//
// Multiple poses keep the brand alive without dropping into corporate
// stock-illustration territory: the same OPERATOR is on the hero, on
// failure callouts, and on empty states.

export type MascotPose = "salute" | "peer" | "snooze" | "stand";

interface MascotOpts {
  /** Approx height in CSS pixels. Width scales with the viewBox. */
  height?: number;
  /** Optional class name for layout / positioning. */
  className?: string;
  /** Line weight relative to the default 1.5. */
  weight?: number;
  /** Color for the status-light accent. Defaults to var(--accent). */
  accent?: string;
}

/** Render OPERATOR in the requested pose. */
export function operator(pose: MascotPose, opts: MascotOpts = {}): string {
  const h = opts.height ?? 96;
  const w = (h * 80) / 100;
  const cls = opts.className ? ` class="${opts.className}"` : "";
  const sw = opts.weight ?? 1.5;
  const accent = opts.accent ?? "var(--accent)";
  const inner = renderPose(pose, accent, sw);
  return `<svg viewBox="0 0 80 100" width="${w}" height="${h}"${cls} stroke="currentColor" stroke-width="${sw}" fill="none" stroke-linecap="round" stroke-linejoin="round" aria-label="OPERATOR (${pose})" role="img">${inner}</svg>`;
}

function renderPose(pose: MascotPose, accent: string, sw: number): string {
  // Common body parts shared across poses, parameterized by pose-specific
  // tweaks (arm angles, antenna, etc).
  const head = `
    <rect x="20" y="14" width="40" height="28" rx="2"/>
    <rect x="24" y="22" width="32" height="10" fill="currentColor" stroke="none"/>
    <rect x="30" y="25" width="3" height="4" fill="${accent}" stroke="none"/>
    <rect x="47" y="25" width="3" height="4" fill="${accent}" stroke="none"/>
  `;
  const torso = `
    <path d="M 22 44 L 22 72 Q 22 74 24 74 L 56 74 Q 58 74 58 72 L 58 44 Z"/>
    <line x1="32" y1="50" x2="48" y2="50"/>
    <circle cx="40" cy="58" r="3"/>
    <line x1="40" y1="61" x2="40" y2="68"/>
  `;
  const legs = `
    <line x1="30" y1="74" x2="30" y2="92"/>
    <line x1="50" y1="74" x2="50" y2="92"/>
    <rect x="24" y="90" width="14" height="6"/>
    <rect x="42" y="90" width="14" height="6"/>
  `;

  switch (pose) {
    case "salute": {
      // Right arm bent up with the hand at the visor (greeting).
      const antenna = `<line x1="40" y1="14" x2="40" y2="6"/><circle cx="40" cy="4" r="2.5" fill="${accent}" stroke="none"/>`;
      const leftArm = `<line x1="22" y1="50" x2="10" y2="62"/><rect x="6" y="60" width="8" height="8"/>`;
      const rightArm = `<polyline points="58,50 70,52 64,38 56,32"/><rect x="50" y="26" width="10" height="6"/>`;
      return `${antenna}${leftArm}${rightArm}${head}${torso}${legs}`;
    }
    case "peer": {
      // Hand cupped over the visor, slight forward lean indicated by an
      // angle line under the chin.
      const antenna = `<line x1="40" y1="14" x2="40" y2="6"/><circle cx="40" cy="4" r="2.5" fill="${accent}" stroke="none"/>`;
      const leftArm = `<polyline points="22,52 14,60 22,66"/><rect x="18" y="64" width="10" height="6"/>`;
      const rightArm = `<polyline points="58,50 64,40 50,34 36,18"/>`;
      return `${antenna}${leftArm}${rightArm}${head}${torso}${legs}`;
    }
    case "snooze": {
      // Antenna drooping; a stylized "z" floats above.
      const antennaDroop = `<path d="M 40 14 Q 44 9 50 11"/>`;
      const z = `<text x="56" y="10" font-family="${"var(--display)"}" font-size="14" font-weight="700" fill="currentColor" stroke="none">z</text>`;
      const leftArm = `<line x1="22" y1="52" x2="14" y2="68"/><rect x="10" y="66" width="8" height="8"/>`;
      const rightArm = `<line x1="58" y1="52" x2="66" y2="68"/><rect x="62" y="66" width="8" height="8"/>`;
      return `${antennaDroop}${z}${leftArm}${rightArm}${head}${torso}${legs}`;
    }
    case "stand":
    default: {
      const antenna = `<line x1="40" y1="14" x2="40" y2="6"/><circle cx="40" cy="4" r="2.5" fill="${accent}" stroke="none"/>`;
      const leftArm = `<line x1="22" y1="50" x2="14" y2="68"/><rect x="10" y="66" width="8" height="8"/>`;
      const rightArm = `<line x1="58" y1="50" x2="66" y2="68"/><rect x="62" y="66" width="8" height="8"/>`;
      return `${antenna}${leftArm}${rightArm}${head}${torso}${legs}`;
    }
  }
  // Note: `sw` is currently unused in the pose paths but retained in the
  // signature so callers can size strokes via the wrapping <svg>.
  void sw;
}

/** Cheeky one-liner attributed to OPERATOR. */
export function operatorSays(quote: string): string {
  return `<div class="operator-says"><span class="op-name">OPERATOR</span> says: <span class="op-quote">${escapeHtml(quote)}</span></div>`;
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
