/**
 * Single, reviewable source of the MCP presentation + behavior metadata that
 * every tool advertises but that the kernel- and module-level `ToolDef`s don't
 * carry themselves: a human-readable `title`, an MCP `annotations` object
 * (readOnlyHint / destructiveHint / openWorldHint), and — for the tools that
 * return `structuredContent` on every success — an `outputSchema`.
 *
 * `server.ts` merges an entry from here onto each `ToolDef` as it assembles the
 * dispatch map (see `withToolMetadata`), and asserts at boot that EVERY
 * advertised tool (static module defs, the inline server_info / pack meta-tools,
 * and the registry-tier kernel tools) has an entry — so a new tool can't ship
 * without a truthful title + annotations. Keeping this in one table (rather than
 * scattered across ~35 tool modules) makes the annotation policy auditable in a
 * single diff.
 *
 * ── Annotation policy (conservative + truthful) ─────────────────────────────
 *  - `readOnlyHint: true` ONLY for genuinely pure tools — they read/inspect or
 *    compute and never mutate a session document, write a file, or open a
 *    session (read, inspect_*, describe_*, render_*, calc_*, size_*, list_*,
 *    search_*, get_*, and pure analyses like run_drc, run_erc, verify_part,
 *    dfm_check).
 *  - `destructiveHint: true` for tools that delete/free/commit irreversibly:
 *    delete_*, close_document, gym_close, place_order.
 *  - `openWorldHint: true` for tools that reach an external service/catalog:
 *    the parts catalogs (search_/resolve_/find_/verify_substitution), the
 *    Fabricate order flow, and continue_document.
 *  - Every other (mutating, local) tool gets `readOnlyHint: false` — a
 *    non-empty, truthful annotations object rather than `{}`.
 *
 * ── outputSchema policy ─────────────────────────────────────────────────────
 *  Per the MCP spec, a tool that declares `outputSchema` MUST return
 *  `structuredContent` on every success. We declare it only on the tools whose
 *  handler UNCONDITIONALLY sets `structuredContent` on success. The schemas are
 *  deliberately permissive (no `required`, `additionalProperties` left open):
 *  the server pipeline also stamps `next_actions` into the structuredContent of
 *  ERROR results (next-actions.ts), and the MCP SDK client validates
 *  structuredContent on error results too — a success-only `required` list would
 *  make an SDK client throw a schema error on every tool error, masking the real
 *  message. Declaring the success-shape properties (typed) without `required`
 *  documents the payload while staying compatible with the error envelope.
 */

/** Presentation + behavior metadata merged onto a ToolDef for ListTools. */
export interface ToolMetadata {
  /** Human-readable display name (MCP `title`). */
  title: string;
  /** MCP tool annotations, advertised verbatim. */
  annotations: Record<string, unknown>;
  /** JSON Schema for the structured result — only on unconditional emitters. */
  outputSchema?: Record<string, unknown>;
}

// Annotation shorthands — one object per behavior class, reused below.
/** Pure read/compute: no mutation, no file write, no session open. */
const RO = { readOnlyHint: true } as const;
/** Mutates local state (session document, saved file, connection config). */
const RW = { readOnlyHint: false } as const;
/** Deletes / frees / commits irreversibly. */
const DESTRUCTIVE = { readOnlyHint: false, destructiveHint: true } as const;
/** Pure read that reaches an external service/catalog. */
const RO_NET = { readOnlyHint: true, openWorldHint: true } as const;
/** Mutating call that reaches an external service/catalog. */
const RW_NET = { readOnlyHint: false, openWorldHint: true } as const;

/** Permissive object schema: declares the success-shape properties (so the
 *  payload is documented) but omits `required` so the server's error envelope
 *  ({error, next_actions}) still validates. See the outputSchema policy above. */
const objectOut = (
  properties: Record<string, unknown>,
): Record<string, unknown> => ({
  type: "object",
  properties,
});

export const TOOL_METADATA: Record<string, ToolMetadata> = {
  // ── Session lifecycle ──────────────────────────────────────────────────
  open_document: { title: "Open Document", annotations: RW },
  get_document: { title: "Get Document", annotations: RO },
  close_document: { title: "Close Document", annotations: DESTRUCTIVE },
  save_document: { title: "Save Document", annotations: RW },
  load_document: { title: "Load Document", annotations: RW },
  checkpoint_document: { title: "Checkpoint Document", annotations: RW },
  branch_from: { title: "Branch From Checkpoint", annotations: RW },
  continue_document: { title: "Continue Document", annotations: RW_NET },
  server_info: { title: "Server Info", annotations: RO },
  list_tool_packs: { title: "List Tool Packs", annotations: RO },
  set_tool_packs: { title: "Set Tool Packs", annotations: RW },

  // ── vcad Fabricate (external manufacturing service) ────────────────────
  quote_manufacturing: { title: "Quote Manufacturing", annotations: RW_NET },
  get_order_status: { title: "Get Order Status", annotations: RO_NET },
  list_orders: { title: "List Orders", annotations: RO_NET },
  authorize_spend: { title: "Authorize Spend", annotations: RW_NET },
  place_order: {
    title: "Place Order",
    annotations: { readOnlyHint: false, destructiveHint: true, openWorldHint: true },
  },

  // ── Project BOM ────────────────────────────────────────────────────────
  bom_create: { title: "Create BOM", annotations: RW },
  bom_add_line: { title: "Add BOM Line", annotations: RW },
  bom_export: { title: "Export BOM", annotations: RW },
  search_mechanical_parts: { title: "Search Mechanical Parts", annotations: RO_NET },

  // ── Live review window ─────────────────────────────────────────────────
  share_session: { title: "Share Session", annotations: RW },
  unshare_session: { title: "Unshare Session", annotations: RW },

  // ── Stdlib parts library (local) ───────────────────────────────────────
  search_parts: { title: "Search Stdlib Parts", annotations: RO },
  place_part: { title: "Place Part", annotations: RW },

  // ── Registry-tier kernel tools ─────────────────────────────────────────
  create: { title: "Create Feature Node", annotations: RW },
  read: { title: "Read Nodes", annotations: RO },
  update: { title: "Update Node", annotations: RW },
  delete: { title: "Delete Node", annotations: DESTRUCTIVE },
  set_material: { title: "Set Material", annotations: RW },
  inspect_part: { title: "Inspect Part", annotations: RO },
  describe_scene: { title: "Describe Scene", annotations: RO },

  // ── MCP Apps: app-only preview fetchers ────────────────────────────────
  get_preview_glb: { title: "Get Preview GLB", annotations: RO },
  get_preview_version: { title: "Get Preview Version", annotations: RO },

  // ── Atomic multi-op editing ────────────────────────────────────────────
  apply_edits: { title: "Apply Edits", annotations: RW },

  // ── Loon one-shot + core see/measure/export ────────────────────────────
  create_cad_loon: { title: "Create CAD (Loon)", annotations: RW },
  export_cad: { title: "Export CAD", annotations: RW },
  inspect_cad: { title: "Inspect CAD", annotations: RO },
  measure: {
    title: "Measure",
    annotations: RO,
    outputSchema: objectOut({
      measure: { type: "object" },
      document_id: { type: "string" },
    }),
  },

  // ── Parametric parameters + differentiable seam ────────────────────────
  list_parameters: { title: "List Parameters", annotations: RO },
  set_parameters: {
    title: "Set Parameters",
    annotations: RW,
    outputSchema: objectOut({ changed: { type: "array" } }),
  },
  parameter_gradient: { title: "Parameter Gradient", annotations: RO },

  // ── Print-then-measure calibration loop ────────────────────────────────
  predict_print: { title: "Predict Print", annotations: RO },
  record_measurement: { title: "Record Measurement", annotations: RW },

  // ── Verify-and-iterate loop ────────────────────────────────────────────
  render_view: { title: "Render View", annotations: RO },
  verify_part: { title: "Verify Part", annotations: RO },
  list_eval_tasks: { title: "List Eval Tasks", annotations: RO },
  verify_spec: { title: "Verify Spec", annotations: RO },

  // ── DFM ────────────────────────────────────────────────────────────────
  dfm_check: { title: "DFM Check", annotations: RO },
  dfm_explain: { title: "DFM Explain", annotations: RO },
  dfm_suggest_fix: { title: "DFM Suggest Fix", annotations: RO },
  dfm_apply_fix: { title: "DFM Apply Fix", annotations: RW },

  // ── Sheet metal ────────────────────────────────────────────────────────
  sheet_metal_create: { title: "Create Sheet Metal", annotations: RW },
  sheet_metal_unfold: { title: "Unfold Sheet Metal", annotations: RO },
  sheet_metal_check: { title: "Check Sheet Metal", annotations: RO },
  sheet_metal_materials: { title: "Sheet Metal Materials", annotations: RO },
  sheet_metal_bend_table: { title: "Sheet Metal Bend Table", annotations: RO },
  sheet_metal_cost: { title: "Sheet Metal Cost", annotations: RO },
  sheet_metal_suggest_fix: { title: "Sheet Metal Suggest Fix", annotations: RO },
  sheet_metal_sequence: { title: "Sheet Metal Bend Sequence", annotations: RO },
  sheet_metal_nest: { title: "Sheet Metal Nest", annotations: RO },

  // ── Import + share ─────────────────────────────────────────────────────
  import_step: { title: "Import STEP", annotations: RW },
  import_kicad: { title: "Import KiCad", annotations: RW },
  import_eagle: { title: "Import EAGLE", annotations: RW },
  open_in_browser: { title: "Open in Browser", annotations: RW },

  // ── Physics gym ────────────────────────────────────────────────────────
  create_robot_env: { title: "Create Robot Env", annotations: RW },
  gym_step: { title: "Gym Step", annotations: RW },
  gym_reset: { title: "Gym Reset", annotations: RW },
  gym_observe: { title: "Gym Observe", annotations: RO },
  gym_close: { title: "Gym Close", annotations: DESTRUCTIVE },

  // ── Atoms ──────────────────────────────────────────────────────────────
  load_structure: { title: "Load Structure", annotations: RW },
  inspect_molecule: { title: "Inspect Molecule", annotations: RO },
  minimize_energy: { title: "Minimize Energy", annotations: RW },
  md_run: { title: "Run Molecular Dynamics", annotations: RW },
  design_material: { title: "Design Material", annotations: RW },
  homogenize_material: { title: "Homogenize Material", annotations: RO },
  render_molecule: { title: "Render Molecule", annotations: RO },
  record_simulation: { title: "Record Simulation", annotations: RW },
  batch_create_envs: { title: "Batch Create Envs", annotations: RW },
  batch_step: { title: "Batch Step", annotations: RW },
  batch_reset: { title: "Batch Reset", annotations: RW },
  get_changelog: { title: "Get Changelog", annotations: RO },

  // ── ECAD (PCB) ─────────────────────────────────────────────────────────
  create_schematic: { title: "Create Schematic", annotations: RW },
  place_components: { title: "Place Components", annotations: RW },
  route_nets: { title: "Route Nets", annotations: RW },
  add_coil: { title: "Add Coil", annotations: RW },
  add_coil_array: { title: "Add Coil Array", annotations: RW },
  winding_layout: { title: "Winding Layout", annotations: RO },
  board_from_solid: { title: "Board From Solid", annotations: RW },
  solid_from_board: {
    title: "Solid From Board",
    annotations: RW,
    outputSchema: objectOut({
      solid_from_board: { type: "object" },
      document_id: { type: "string" },
    }),
  },
  check_enclosure_fit: {
    title: "Check Enclosure Fit",
    annotations: RW,
    outputSchema: objectOut({
      enclosure_fit: { type: "object" },
      document_id: { type: "string" },
      enclosure_document_id: { type: "string" },
    }),
  },
  check_clearance: {
    title: "Check Clearance",
    annotations: RW,
    outputSchema: objectOut({
      clearance: { type: "object" },
      document_id: { type: "string" },
    }),
  },
  list_footprints: { title: "List Footprints", annotations: RO },
  search_footprints: { title: "Search Footprints", annotations: RO },
  get_pad_positions: { title: "Get Pad Positions", annotations: RO },
  get_footprint: { title: "Get Footprint", annotations: RO },
  describe_pcb: { title: "Describe PCB", annotations: RO },
  add_trace: { title: "Add Trace", annotations: RW },
  add_via: { title: "Add Via", annotations: RW },
  set_stackup: { title: "Set Stackup", annotations: RW },
  set_placement: { title: "Set Placement", annotations: RW },
  set_board_outline: { title: "Set Board Outline", annotations: RW },
  add_zone: { title: "Add Zone", annotations: RW },
  delete_zone: { title: "Delete Zone", annotations: DESTRUCTIVE },
  delete_trace: { title: "Delete Trace", annotations: DESTRUCTIVE },
  delete_via: { title: "Delete Via", annotations: DESTRUCTIVE },
  get_copper: { title: "Get Copper", annotations: RO },
  add_net_tie: { title: "Add Net Tie", annotations: RW },
  delete_net_tie: { title: "Delete Net Tie", annotations: DESTRUCTIVE },
  undo: { title: "Undo", annotations: RW },
  set_design_rules: { title: "Set Design Rules", annotations: RW },
  size_trace_for_current: { title: "Size Trace for Current", annotations: RO },
  add_via_array: { title: "Add Via Array", annotations: RW },
  add_motor_winding: { title: "Add Motor Winding", annotations: RW },
  calc_motor: { title: "Calc Motor", annotations: RO },
  check_self_start: { title: "Check Self-Start", annotations: RO },
  render_pcb: { title: "Render PCB", annotations: RO },
  render_ratsnest: { title: "Render Ratsnest", annotations: RO },
  render_stackup: { title: "Render Stackup", annotations: RO },
  run_drc: { title: "Run DRC", annotations: RO },
  search_electronic_parts: { title: "Search Electronic Parts", annotations: RO_NET },
  resolve_part: { title: "Resolve Part", annotations: RO_NET },
  find_alternatives: { title: "Find Alternatives", annotations: RO_NET },
  verify_substitution: { title: "Verify Substitution", annotations: RO_NET },
  build_receipt: {
    title: "Build Receipt",
    annotations: RO,
    outputSchema: objectOut({
      unified: { type: "object" },
      receipt: { type: "object" },
      document_id: { type: "string" },
    }),
  },
  verify_receipt: {
    title: "Verify Receipt",
    annotations: RO,
    outputSchema: objectOut({
      verify_receipt: { type: "object" },
      document_id: { type: "string" },
    }),
  },
  route_diff_pair: { title: "Route Differential Pair", annotations: RW },
  critique_route: { title: "Critique Route", annotations: RO },
  run_erc: { title: "Run ERC", annotations: RO },
  export_gerber: { title: "Export Gerber", annotations: RW },
  export_kicad: {
    title: "Export KiCad",
    annotations: RW,
    outputSchema: objectOut({ export_kicad: { type: "object" } }),
  },
  validate_for_fab: { title: "Validate for Fab", annotations: RO },
  calc_impedance: { title: "Calc Impedance", annotations: RO },
  size_impedance: { title: "Size Impedance", annotations: RO },
  size_pdn: { title: "Size PDN", annotations: RO },
  calc_coil: { title: "Calc Coil", annotations: RO },
  size_coil: { title: "Size Coil", annotations: RO },
  calc_rf: { title: "Calc RF", annotations: RO },
};
