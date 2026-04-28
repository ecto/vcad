// Design tokens shared across every page in the leaderboard.
//
// Identity: drafting / blueprint, but distinct from vcad's Berkeley-Mono /
// Borland aesthetic. mecheval uses a contrasting condensed sans for the
// wordmark + headlines, blueprint-cyan ink (not pure black), and graph-
// paper underlay. Cheeky copy is voiced by OPERATOR, the project mascot.

export const colors = {
  /** Deep blueprint-cyan. Replaces near-black for body and rules. */
  ink: "#0e3960",
  /** Muted cyan for secondary text and metadata. */
  inkSoft: "#5b7892",
  /** Bone-white drafting ground, kept from earlier iteration. */
  ground: "#fbf6ee",
  /** Heavy rule color (= ink). */
  rule: "#0e3960",
  /** Dotted-rule color. */
  soft: "#cfc6b4",
  /** Pass / fail / pending status colors. */
  pass: "#27ae60",
  fail: "#c0392b",
  pending: "#8a8576",
  /** OPERATOR's hot-orange accent. Used sparingly. */
  accent: "#d68910",
} as const;

export const fonts = {
  /** Wordmark + headings: condensed sans for a drafting-engineering feel. */
  display: '"Space Grotesk", "Inter", system-ui, sans-serif',
  /** Body, tables, code, data: monospace. */
  body: '"JetBrains Mono", "Berkeley Mono", ui-monospace, SFMono-Regular, Menlo, monospace',
} as const;

/** Hosted at fonts.googleapis.com — embedded as a single <link> on every page. */
export const fontsHref =
  "https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600&family=Space+Grotesk:wght@500;700&display=swap";

export const copy = {
  brand: "mecheval.",
  tagline: "AI builds the mech. We measure how badly it fits.",
  subtagline:
    "mechanical, physical, and CAD evaluation suite for AI models",
  footerOwner: "Municipal Robotics",
  footerOwnerUrl: "https://municipalrobotics.com",
  siblingProjectName: "vcad.",
  siblingProjectUrl: "https://vcad.io",
  repoUrl: "https://github.com/ecto/vcad",
} as const;

/** Title-block fields rendered in the upper-right corner of every page. */
export interface TitleBlock {
  drawing: string;
  sheet: string;
  /** Optional dimension callout shown in the corner — e.g. "PASS^5". */
  scale?: string;
}
