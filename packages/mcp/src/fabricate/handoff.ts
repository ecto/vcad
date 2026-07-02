/**
 * Fab-ready handoff — the interim ordering rail for processes where no fab
 * partner is signed yet (today: sheet metal).
 *
 * Until a partner adapter can flip `orderable: true`, the most useful thing a
 * quote can do is hand the agent (and its human) everything needed to finish
 * the order on a fab's own instant-quote site in one pass: which shops fit,
 * which file each shop needs, the exact vcad tool calls that produce those
 * files, and what to type into the shop's UI. The struct is data, not prose,
 * so agents can act on it; `summary` is the human-readable line.
 *
 * This is deliberately NOT browser automation of a fab's checkout — the human
 * places the order in their own account on the fab's site. vcad's job is that
 * the files arrive pre-validated against the shop's published tooling
 * (shop_profile catalogs in vcad-kernel-sheet), so the upload quotes clean on
 * the first try.
 */

import type { Process } from "./types.js";

/** One fab a handoff can target. `upload_url` is the shop's public entry
 *  point (never a deep link into their app — those churn). */
export interface HandoffShop {
  id: string;
  label: string;
  region: "US";
  upload_url: string;
  /** File kinds this shop's instant-quote flow accepts for this process. */
  formats: Array<"dxf" | "step">;
  /** vcad shop-profile catalog id when the kernel encodes this shop's
   *  tooling (fixed bend radii, K-factors, reliefs) — quote-clean uploads. */
  shop_profile: string | null;
  notes: string;
}

export interface FabHandoff {
  process: Process;
  /** False until a partner adapter can place this order via place_order. */
  orderable_via_vcad: false;
  shops: HandoffShop[];
  /** Ordered tool-call recipe that produces the upload-ready files. */
  file_recipe: string[];
  /** What the shop's UI will ask for that the files don't carry. */
  at_upload: string[];
  summary: string;
}

const SHEET_METAL_SHOPS: HandoffShop[] = [
  {
    id: "sendcutsend",
    label: "SendCutSend",
    region: "US",
    upload_url: "https://sendcutsend.com",
    formats: ["dxf", "step"],
    shop_profile: "sendcutsend",
    notes:
      "vcad encodes SendCutSend's published bend catalog — create the part with " +
      'shop_profile: "sendcutsend" and the flat pattern matches their tooling ' +
      "exactly. STEP uploads auto-detect bends (zero data entry). Note 6061 is " +
      "not bendable at SCS; flat 6061 parts are fine.",
  },
  {
    id: "oshcut",
    label: "OSH Cut",
    region: "US",
    upload_url: "https://www.oshcut.com",
    formats: ["dxf", "step"],
    shop_profile: null,
    notes:
      "Instant quote with strong in-browser DFM feedback; typically fast " +
      "turnaround. No vcad shop profile yet — verify bend radii against their " +
      "quoter after upload.",
  },
  {
    id: "fabworks",
    label: "Fabworks",
    region: "US",
    upload_url: "https://www.fabworks.com",
    formats: ["dxf", "step"],
    shop_profile: null,
    notes:
      "Fastest typical ship times (1-2 business days) and often the lowest " +
      "price. No vcad shop profile yet — verify bend radii against their " +
      "quoter after upload.",
  },
];

/**
 * Build the handoff block for a quote, or null when the process either has a
 * real ordering path or no curated shop list yet.
 */
export function buildFabHandoff(
  process: Process,
  opts: { hasArtifact: boolean },
): FabHandoff | null {
  if (process !== "sheet_metal") return null;

  const file_recipe = opts.hasArtifact
    ? [
        "Fab files are already bound to this order (fab_artifact_id) — fetch them from the artifact store and upload to the chosen shop.",
      ]
    : [
        'For a bent part: sheet_metal_unfold(document_id) returns the fab-ready DXF (merged silhouette + DASHED bend lines), or export_cad with a ".step" filename for the folded body (bends auto-detected by the shop, zero data entry — requires the part was created with the shop\'s shop_profile so radii match their tooling).',
        "Re-run quote_manufacturing with the export's fab_artifact_id to bind the exact bytes to this order for traceability.",
      ];

  return {
    process,
    orderable_via_vcad: false,
    shops: SHEET_METAL_SHOPS,
    file_recipe,
    at_upload: [
      "Material + thickness (must match the sheet_metal_create material — the DXF does not carry it).",
      "Bend angles when uploading DXF (the DXF marks bend lines, not angles). STEP uploads skip this.",
      "Quantity and finish (deburring/powder/anodize) in the shop's UI.",
    ],
    summary:
      "Sheet metal isn't agent-orderable through vcad yet — a fab partner rail is in progress. " +
      "This handoff has everything needed to finish the order on a fab's instant-quote site: " +
      "upload the recipe's files, enter material/thickness, and check out. " +
      "Files produced with a matching shop_profile quote clean on the first try.",
  };
}
