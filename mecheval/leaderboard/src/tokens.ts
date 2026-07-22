// Design tokens shared across every page in the leaderboard.
//
// A small, deliberate system: one sans for structure and data (Inter),
// one serif for editorial warmth (Newsreader), one mono for ids and
// source (JetBrains Mono). Colour is defined as semantic roles with a
// light and a dark value each — the build emits both and lets the OS
// choose via prefers-color-scheme. Renders are framed like prints on a
// constant light mat, so they read as the only colour on the page in
// either theme.

/** Semantic colour roles. `light` is the default; `dark` is emitted
 *  under a prefers-color-scheme media query. Renders sit on `mat`,
 *  which stays light in both themes (a gallery passe-partout). */
export const theme = {
  light: {
    ground: "#fbfbfc",
    surface: "#ffffff",
    sunken: "#f5f6f8",
    ink: "#16181d",
    inkSoft: "#5b626d",
    inkFaint: "#9aa1ad",
    rule: "#e9ebef",
    ruleStrong: "#dcdfe4",
    soft: "#eef0f3",
    hover: "#f5f6f8",
    accent: "#2563eb",
    accentDeep: "#1d4ed8",
    accentSoft: "#eaf0fe",
    pass: "#1a7f4b",
    fail: "#d23f3f",
    pending: "#9aa1ad",
    mat: "#f4f3f1",
    matRule: "#e6e4df",
    shadow: "none",
    shadowLift: "0 8px 28px rgba(16,24,40,0.07)",
  },
  dark: {
    ground: "#0b0c0e",
    surface: "#141619",
    sunken: "#0f1113",
    ink: "#e9eaec",
    inkSoft: "#9aa1ad",
    inkFaint: "#5f6672",
    rule: "#23262b",
    ruleStrong: "#30343b",
    soft: "#1c1f23",
    hover: "#1a1d21",
    accent: "#5b8bff",
    accentDeep: "#84a6ff",
    accentSoft: "#15233f",
    pass: "#43c182",
    fail: "#ff6b6b",
    pending: "#6b7280",
    mat: "#f4f3f1",
    matRule: "#d8d6d1",
    shadow: "none",
    shadowLift: "0 10px 32px rgba(0,0,0,0.5)",
  },
} as const;

/** Back-compat flat palette (light values) for code that bakes a literal.
 *  Prefer the CSS variables (var(--accent) …) inside emitted SVG/HTML. */
export const colors = {
  ink: theme.light.ink,
  inkSoft: theme.light.inkSoft,
  ground: theme.light.ground,
  surface: theme.light.surface,
  rule: theme.light.rule,
  soft: theme.light.soft,
  hover: theme.light.hover,
  pass: theme.light.pass,
  fail: theme.light.fail,
  pending: theme.light.pending,
  accent: theme.light.accent,
  accentDeep: theme.light.accentDeep,
} as const;

export const fonts = {
  /** Headings, UI, and all data. IBM Plex Sans — an engineered, lightly
   *  academic grotesque, with a clean system fallback. */
  display:
    '"IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
  body: '"IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
  /** Monospace for ids, hashes, and source. IBM Plex Mono pairs with the
   *  sans for a cohesive Plex family. */
  mono: '"IBM Plex Mono", ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace',
} as const;

/** IBM Plex Sans + IBM Plex Mono via Google Fonts. */
export const fontsHref =
  "https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:wght@400;500;600;700&family=IBM+Plex+Mono:wght@400;500&display=swap";

/** The umbrella brand: eval.vcad.io. Chapters (mecheval, pcbeval, …)
 *  live under their own path prefix; the branded .com domains 301 here. */
export const family = {
  brand: "vcad evals",
  tagline: "Evals for atoms, not tokens. Graded by kernels, not vibes.",
  subtagline:
    "AI models graded by a real CAD kernel, real DRC, and real physics — every check deterministic, every number auditable.",
  canonicalHost: "eval.vcad.io",
} as const;

/** One entry per eval chapter, in scale-ladder order (atoms → machines).
 *  `pathPrefix` is where the chapter's pages are emitted inside dist/;
 *  `comDomain` 301s to that prefix. Only `live` chapters get built. */
export interface DomainSpec {
  slug: string;
  pathPrefix: string;
  name: string;
  scale: string;
  tagline: string;
  comDomain: string;
  live: boolean;
}

export const domains: readonly DomainSpec[] = [
  {
    slug: "atom",
    pathPrefix: "atom",
    name: "atomeval",
    scale: "atoms → materials",
    tagline:
      "Matter design graded by molecular dynamics, energy minimization, and atoms-to-continuum homogenization.",
    comDomain: "atomeval.com",
    live: false,
  },
  {
    slug: "mech",
    pathPrefix: "mech",
    name: "mecheval",
    scale: "parts → machines",
    tagline:
      "The mechanical, physical, and CAD evaluation suite. The AI designs the robot and drives it.",
    comDomain: "mecheval.com",
    live: true,
  },
  {
    slug: "pcb",
    pathPrefix: "pcb",
    name: "pcbeval",
    scale: "boards → circuits",
    tagline:
      "Schematic and layout graded by real DRC, ERC, and circuit simulation. Passing boards are orderable.",
    comDomain: "pcbeval.com",
    live: false,
  },
  {
    slug: "sim",
    pathPrefix: "sim",
    name: "simeval",
    scale: "fields → systems",
    tagline:
      "Devices graded by field behavior: thermal, EM, photonics, antennas, acoustics.",
    comDomain: "simeval.com",
    live: false,
  },
] as const;

export const copy = {
  brand: "mecheval",
  tagline: "The mechanical, physical, and CAD evaluation suite for AI models.",
  subtagline: "Every check is something the CAD kernel can compute exactly.",
  footerOwner: "Municipal Robotics",
  footerOwnerUrl: "https://muni.works",
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
  /** Page-specific label for the bottom-right cell (e.g. "leaderboard",
   *  task id, model id, run id). Defaults to "—" if omitted. */
  project?: string;
}
