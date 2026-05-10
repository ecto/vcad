// Design tokens shared across every page in the leaderboard.
//
// Plain-HTML aesthetic — Times New Roman, white background, default
// blue underlined links. Tokens drive the build's <style> block.

export const colors = {
  /** Pure black for body and rules. */
  ink: "#000000",
  /** Soft gray for metadata. */
  inkSoft: "#666666",
  /** Plain white background. */
  ground: "#ffffff",
  /** Rule color. */
  rule: "#000000",
  /** Light gray for inner rules. */
  soft: "#cccccc",
  /** Pass / fail / pending. Kept readable on white. */
  pass: "#0a7a2f",
  fail: "#b00020",
  pending: "#888888",
  /** Hyperlink blue, used for all links. */
  accent: "#0000ee",
} as const;

export const fonts = {
  /** Body and headings: Times New Roman serif. */
  display: '"Times New Roman", Times, serif',
  /** Body: same Times serif for the plain-html look. */
  body: '"Times New Roman", Times, serif',
} as const;

/** No web fonts — system Times New Roman is enough. */
export const fontsHref = "";

export const copy = {
  brand: "mecheval",
  tagline: "The mechanical, physical, and CAD evaluation suite for AI models.",
  subtagline:
    "Every check is something the CAD kernel can compute exactly.",
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
