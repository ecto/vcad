//! WASM bindings for sheet-metal evaluation.
//!
//! The web app never builds a [`SheetMetalModel`] itself. Instead it
//! describes the desired ops as a base-to-tip chain, hands the chain JSON to
//! [`evaluate_sheet_metal_chain`], and receives a bundle containing:
//!
//! - A triangle mesh suitable for the existing scene renderer.
//! - The projected flat pattern (panels + holes + creases) in global flat 2D.
//! - A compact snapshot of every panel and bend (thickness, K-factor and
//!   provenance, allowance, etc.) so the property panel and DFM tools can
//!   render the manufacturing-side view without a second WASM call.
//!
//! Single JSON-over-the-boundary entry point keeps the bindings narrow and
//! lets us extend ops without touching `wasm-bindgen` signatures.
//!
//! The chain JSON shape mirrors the matching IR variants:
//! ```text
//! [
//!   {"type":"BaseFlangeRect","width":100,"depth":50,"thickness":1.0,"material":"Al-soft"},
//!   {"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":25,"angle":1.5707963,"radius":1.0,"direction":"Up"}
//! ]
//! ```
//!
//! See `docs/design/sheet-metal.md` for the strategic vision; this module is
//! the foundation tier exposed to the web app.

use serde::{Deserialize, Serialize};
use vcad_kernel_math::Point2;
use vcad_kernel_sheet::{
    add_edge_flange, add_hem, add_jog, apply_bend_relief, base_flange_polygon_with_holes,
    base_flange_rect, bend_sequence,
    bend_table::{self, BendTable},
    check_manufacturability,
    cost::{estimate_cost, CostBreakdown, CostRates},
    edge_flange::EdgeFlangeParams,
    flat_pattern_to_dxf,
    hem::{HemKind, HemParams},
    jog::JogParams,
    nest_rectangles, nested_dxf,
    nesting::{NestingParams, NestingResult, PartFootprint},
    sequence::BendStep,
    shop_catalog, BendDirection, FlangePosition, FlatPattern, NestedPlacement, ReliefParams,
    SheetMetalModel, ShopCatalog, ShopProfile, Violation,
};
use wasm_bindgen::prelude::*;

/// One op in a sheet-metal evaluation chain. Tag uses the same wire spelling
/// the IR variants ship under, minus the `SheetMetal` prefix.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
enum ChainOp {
    /// Initialise the model from an axis-aligned rectangle in the XY plane.
    BaseFlangeRect {
        width: f64,
        depth: f64,
        thickness: f64,
        /// Material name for K-factor lookup (e.g. `"Al-soft"`).
        material: String,
        /// Optional built-in shop catalog id (e.g. `"sendcutsend"`). When
        /// set, every bend's radius and K-factor resolve through the shop's
        /// published table — custom radii are rejected.
        #[serde(default)]
        shop_profile: Option<String>,
    },
    /// Initialise the model from an arbitrary CCW polygon (with optional
    /// CW hole loops) in the XY plane.
    BaseFlangePolygon {
        /// CCW outline points as `[x, y]` pairs (mm).
        outline: Vec<[f64; 2]>,
        /// Optional CW hole loops.
        #[serde(default)]
        holes: Vec<Vec<[f64; 2]>>,
        thickness: f64,
        material: String,
        /// Optional built-in shop catalog id (see `BaseFlangeRect`).
        #[serde(default)]
        shop_profile: Option<String>,
    },
    /// Add a flange off `edge_index` of `panel_id`.
    EdgeFlange {
        panel_id: usize,
        edge_index: usize,
        length: f64,
        angle: f64,
        /// Inside bend radius (mm). Omitted → thickness, or the shop's
        /// fixed radius when a shop profile is active.
        #[serde(default)]
        radius: Option<f64>,
        direction: BendDirection,
        /// Optional manual K-factor override (skips bend-table lookup).
        manual_k: Option<f64>,
    },
    /// Cut bend-relief notches at every bend end whose parent material
    /// sits in the deformation zone. Applied after all other ops; sizing
    /// defaults follow the active shop profile.
    BendRelief {
        /// Notch width (mm). Default `max(1.5 t, 1.0)`.
        #[serde(default)]
        width: Option<f64>,
        /// Notch depth from the bend line (mm). Default `R + t` or the
        /// shop's published per-thickness relief depth.
        #[serde(default)]
        depth: Option<f64>,
    },
    /// Add a hem (180° fold) off `edge_index` of `panel_id`.
    Hem {
        panel_id: usize,
        edge_index: usize,
        kind: HemKind,
        length: f64,
        #[serde(default)]
        gap: f64,
        direction: BendDirection,
    },
    /// Add a jog (Z-shaped offset) off `edge_index` of `panel_id`.
    Jog {
        panel_id: usize,
        edge_index: usize,
        offset: f64,
        length: f64,
        /// Inside bend radius (mm). Omitted → thickness, or the shop's
        /// fixed radius when a shop profile is active.
        #[serde(default)]
        radius: Option<f64>,
        direction: BendDirection,
    },
}

/// What the binding returns to the web app. `mesh` is empty on error;
/// `error` is non-empty when something went wrong inside the kernel.
#[derive(Debug, Clone, Serialize, Default)]
struct SheetMetalEvalResult {
    mesh: MeshDto,
    flat_pattern: FlatPatternDto,
    model: ModelSummaryDto,
    /// Layered DXF (CUT / BEND_UP / BEND_DOWN) of the flat pattern, ready to
    /// hand to a laser bureau. Empty string on error.
    dxf: String,
    /// Manufacturability findings against the generic shop profile. Empty
    /// when the part is shop-ready.
    violations: Vec<ViolationDto>,
    error: Option<String>,
}

/// UI-facing projection of a [`Violation`]: pre-rendered severity + message
/// plus the structured detail (kind-tagged) for camera-fly / fix actions.
#[derive(Debug, Clone, Serialize)]
struct ViolationDto {
    rule: &'static str,
    severity: String,
    message: String,
    detail: Violation,
}

#[derive(Debug, Clone, Serialize, Default)]
struct MeshDto {
    positions: Vec<f32>,
    indices: Vec<u32>,
    normals: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct FlatPatternDto {
    thickness: f64,
    panel_outlines_2d: Vec<Vec<[f64; 2]>>,
    panel_holes_2d: Vec<Vec<Vec<[f64; 2]>>>,
    /// Merged cut profile (panels ∪ allowance strips) — the closed
    /// silhouette a laser bureau cuts; what the DXF CUT layer carries.
    /// First ring is the CCW exterior, the rest are CW holes. Empty when
    /// the flat pattern is empty or disconnected (the `error` field
    /// explains the latter when the DXF fails).
    silhouette_2d: Vec<Vec<[f64; 2]>>,
    creases: Vec<FlatCreaseDto>,
    area_mm2: f64,
    /// `[min_x, min_y, max_x, max_y]`.
    bbox: [f64; 4],
}

#[derive(Debug, Clone, Serialize)]
struct FlatCreaseDto {
    line: [[f64; 2]; 2],
    angle: f64,
    radius: f64,
    k_factor: f64,
    k_factor_source: Option<String>,
    direction: BendDirection,
    bend_id: usize,
}

/// Per-panel + per-bend summary surfaced to the property panel. The full
/// `SheetMetalModel` lives inside the WASM heap; this is the user-facing
/// projection (no frames, no internal IDs that wouldn't survive across
/// rebuilds).
#[derive(Debug, Clone, Serialize, Default)]
struct ModelSummaryDto {
    thickness: f64,
    /// Material key from the model (e.g. `"al-soft"`). Empty when the
    /// chain didn't specify one.
    material: String,
    panel_count: usize,
    bend_count: usize,
    bends: Vec<BendSummaryDto>,
}

#[derive(Debug, Clone, Serialize)]
struct BendSummaryDto {
    parent: usize,
    child: usize,
    angle_rad: f64,
    radius: f64,
    direction: BendDirection,
    k_factor: f64,
    k_factor_source: Option<String>,
    /// `θ · (R + K · t)`.
    allowance_mm: f64,
    /// Estimated springback (radians). Computed from the part's material
    /// via `material.springback_per_radian * angle`; zero when the
    /// material is unknown.
    springback_rad: f64,
    /// The angle to actually form on the brake to hit the modelled
    /// (target) angle once springback releases: `angle + springback`.
    compensated_angle_rad: f64,
}

/// Evaluate a chain of sheet-metal ops and return `(mesh, flat-pattern,
/// model-summary)` as a JSON string. Caller is responsible for parsing.
///
/// On error, returns a JSON object with a non-null `error` field; the other
/// fields are zeroed. Never panics — every fallible kernel call is mapped
/// to an error string.
#[wasm_bindgen(js_name = evaluateSheetMetalChain)]
pub fn evaluate_sheet_metal_chain(chain_json: &str) -> String {
    let result = match build_result(chain_json) {
        Ok(r) => r,
        Err(msg) => SheetMetalEvalResult {
            error: Some(msg),
            ..Default::default()
        },
    };
    serde_json::to_string(&result).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

fn build_result(chain_json: &str) -> Result<SheetMetalEvalResult, String> {
    let chain: Vec<ChainOp> =
        serde_json::from_str(chain_json).map_err(|e| format!("chain JSON: {e}"))?;
    if chain.is_empty() {
        return Err("empty op chain".to_string());
    }
    let table = BendTable::builtin();
    let mut model = build_model(&chain, &table)?;
    // Recompute flat frames before projecting — the kernel keeps them in
    // sync after every `add_edge_flange`, but `unfold` is the source of
    // truth and is a cheap call on a tree of this size.
    vcad_kernel_sheet::unfold(&mut model).map_err(|e| format!("unfold: {e:?}"))?;

    let mesh = tessellate_model(&model);
    let flat = FlatPattern::from_model(&model);
    // Merged-silhouette export fails loudly on disconnected flat patterns —
    // surface that as the result error rather than emitting a DXF the fab
    // would reject anyway.
    let dxf = flat_pattern_to_dxf(&flat).map_err(|e| format!("dxf: {e}"))?;
    // Check against the chain's shop profile when one is named, else generic.
    let shop_profile = match chain_shop(&chain)? {
        Some(cat) => cat.shop_profile_for(&model.material, model.thickness),
        None => ShopProfile::generic(),
    };
    let violations = check_manufacturability(&model, &shop_profile)
        .into_iter()
        .map(|v| ViolationDto {
            rule: v.rule(),
            severity: format!("{:?}", v.severity()),
            message: v.message(),
            detail: v,
        })
        .collect();
    let flat_dto = flat_pattern_to_dto(flat);
    let summary = summarise_model(&model);
    Ok(SheetMetalEvalResult {
        mesh,
        flat_pattern: flat_dto,
        model: summary,
        dxf,
        violations,
        error: None,
    })
}

/// Result of [`check_sheet_metal`].
#[derive(Debug, Clone, Serialize, Default)]
struct CheckResult {
    /// Findings against the supplied shop profile. Empty = shop-ready.
    violations: Vec<ViolationDto>,
    /// The profile actually used (after field-tolerant merge onto the
    /// generic defaults) — so the UI can show what it checked against.
    shop: Option<ShopProfile>,
    error: Option<String>,
}

/// Re-run manufacturability against a *caller-supplied* shop profile.
///
/// Separate from [`evaluate_sheet_metal_chain`] on purpose: the spec treats
/// manufacturability as a **typed query against the model**, not a
/// by-product of mesh evaluation. The app's DFM inspector and the
/// `sheet_metal.check` MCP tool both call this so a shop's real
/// capabilities — not the generic defaults — drive the result.
///
/// `shop_json` is field-tolerant (see [`ShopProfile`]); pass `""` for the
/// generic shop. On any error the `error` field is set and `violations` is
/// empty.
#[wasm_bindgen(js_name = checkSheetMetal)]
pub fn check_sheet_metal(chain_json: &str, shop_json: &str) -> String {
    let result = match check_impl(chain_json, shop_json) {
        Ok(r) => r,
        Err(msg) => CheckResult {
            error: Some(msg),
            ..Default::default()
        },
    };
    serde_json::to_string(&result).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

fn check_impl(chain_json: &str, shop_json: &str) -> Result<CheckResult, String> {
    let chain: Vec<ChainOp> =
        serde_json::from_str(chain_json).map_err(|e| format!("chain JSON: {e}"))?;
    if chain.is_empty() {
        return Err("empty op chain".to_string());
    }
    let table = BendTable::builtin();
    let model = build_model(&chain, &table)?;
    let shop = resolve_shop_arg(shop_json, &chain, &model)?;
    let violations = check_manufacturability(&model, &shop)
        .into_iter()
        .map(|v| ViolationDto {
            rule: v.rule(),
            severity: format!("{:?}", v.severity()),
            message: v.message(),
            detail: v,
        })
        .collect();
    Ok(CheckResult {
        violations,
        shop: Some(shop),
        error: None,
    })
}

/// Interpret the `shop_json` argument of [`check_sheet_metal`].
///
/// Accepts: empty (→ the chain's shop profile, else the generic shop), a
/// built-in catalog id (bare or JSON-quoted, e.g. `"sendcutsend"`), or a
/// full [`ShopProfile`] object (field-tolerant).
fn resolve_shop_arg(
    shop_json: &str,
    chain: &[ChainOp],
    model: &SheetMetalModel,
) -> Result<ShopProfile, String> {
    let trimmed = shop_json.trim();
    if trimmed.is_empty() {
        return Ok(match chain_shop(chain)? {
            Some(cat) => cat.shop_profile_for(&model.material, model.thickness),
            None => ShopProfile::generic(),
        });
    }
    if trimmed.starts_with('{') {
        return serde_json::from_str::<ShopProfile>(trimmed).map_err(|e| format!("shop JSON: {e}"));
    }
    // A catalog id — bare ("sendcutsend") or JSON string ("\"sendcutsend\"").
    let id = serde_json::from_str::<String>(trimmed).unwrap_or_else(|_| trimmed.to_string());
    let cat = shop_catalog(&id).map_err(|e| e.to_string())?;
    Ok(cat.shop_profile_for(&model.material, model.thickness))
}

/// Result of [`cost_sheet_metal`].
#[derive(Debug, Clone, Serialize, Default)]
struct CostResult {
    breakdown: Option<CostBreakdown>,
    rates: Option<CostRates>,
    error: Option<String>,
}

/// Estimate the manufacturing cost of a sheet-metal chain.
///
/// `rates_json` is field-tolerant (omit keys to use the generic shop
/// rates); pass `""` for full defaults. `quantity` is clamped to `>= 1`.
#[wasm_bindgen(js_name = costSheetMetal)]
pub fn cost_sheet_metal(chain_json: &str, rates_json: &str, quantity: u32) -> String {
    let result = match cost_impl(chain_json, rates_json, quantity) {
        Ok(r) => r,
        Err(msg) => CostResult {
            error: Some(msg),
            ..Default::default()
        },
    };
    serde_json::to_string(&result).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

fn cost_impl(chain_json: &str, rates_json: &str, quantity: u32) -> Result<CostResult, String> {
    let chain: Vec<ChainOp> =
        serde_json::from_str(chain_json).map_err(|e| format!("chain JSON: {e}"))?;
    if chain.is_empty() {
        return Err("empty op chain".to_string());
    }
    let rates = if rates_json.trim().is_empty() {
        CostRates::generic()
    } else {
        serde_json::from_str::<CostRates>(rates_json).map_err(|e| format!("rates JSON: {e}"))?
    };
    let table = BendTable::builtin();
    let mut model = build_model(&chain, &table)?;
    vcad_kernel_sheet::unfold(&mut model).map_err(|e| format!("unfold: {e:?}"))?;
    let flat = FlatPattern::from_model(&model);
    let breakdown = estimate_cost(&model, &flat, quantity, &rates);
    Ok(CostResult {
        breakdown: Some(breakdown),
        rates: Some(rates),
        error: None,
    })
}

/// Result of [`sheet_metal_sequence`].
#[derive(Debug, Clone, Serialize, Default)]
struct SequenceResult {
    steps: Vec<BendStep>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct NestingDto {
    result: Option<NestingResult>,
    error: Option<String>,
}

/// Rectangular nesting of multiple parts on stock sheets.
///
/// `parts_json` is a JSON array of `PartFootprint` objects (each with
/// `name`, `width_mm`, `height_mm`, `quantity`); `params_json` is a
/// `NestingParams` object (pass `""` for the generic 4'×8' default).
#[wasm_bindgen(js_name = nestSheetMetalParts)]
pub fn nest_sheet_metal_parts(parts_json: &str, params_json: &str) -> String {
    let dto = match nest_impl(parts_json, params_json) {
        Ok(r) => NestingDto {
            result: Some(r),
            error: None,
        },
        Err(msg) => NestingDto {
            result: None,
            error: Some(msg),
        },
    };
    serde_json::to_string(&dto).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

/// Placement spec for [`nested_sheet_metal_dxf`]: each entry pairs a
/// sheet-metal op chain with the (sheet, dx, dy, rotated) location it
/// occupies on a stock sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NestedPlacementDto {
    chain: Vec<ChainOp>,
    sheet: usize,
    #[serde(default)]
    dx_mm: f64,
    #[serde(default)]
    dy_mm: f64,
    #[serde(default)]
    rotated: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
struct NestedDxfResult {
    /// One DXF string per sheet (index 0 = sheet 0).
    sheets: Vec<String>,
    error: Option<String>,
}

/// Produce one layered DXF per stock sheet for a set of nested parts.
///
/// `placements_json` is an array of [`NestedPlacementDto`]; each chain
/// is independently evaluated into a flat pattern, then translated /
/// rotated according to its placement before being written to the
/// sheet's DXF. Layers are the same `CUT` / `BEND_UP` / `BEND_DOWN`
/// triple a shop's post-processor already knows.
#[wasm_bindgen(js_name = nestedSheetMetalDxf)]
pub fn nested_sheet_metal_dxf(placements_json: &str) -> String {
    let result = match nested_dxf_impl(placements_json) {
        Ok(sheets) => NestedDxfResult {
            sheets,
            error: None,
        },
        Err(msg) => NestedDxfResult {
            sheets: Vec::new(),
            error: Some(msg),
        },
    };
    serde_json::to_string(&result).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

fn nested_dxf_impl(placements_json: &str) -> Result<Vec<String>, String> {
    let placements: Vec<NestedPlacementDto> =
        serde_json::from_str(placements_json).map_err(|e| format!("placements JSON: {e}"))?;
    let table = BendTable::builtin();
    // Build a flat pattern per placement (own it so the slice of refs is
    // stable across the call).
    let mut flats: Vec<FlatPattern> = Vec::with_capacity(placements.len());
    for (i, p) in placements.iter().enumerate() {
        let mut model =
            build_model(&p.chain, &table).map_err(|e| format!("placement #{i}: {e}"))?;
        vcad_kernel_sheet::unfold(&mut model)
            .map_err(|e| format!("placement #{i} unfold: {e:?}"))?;
        flats.push(FlatPattern::from_model(&model));
    }
    let placements_ref: Vec<NestedPlacement<'_>> = placements
        .iter()
        .zip(flats.iter())
        .map(|(p, f)| NestedPlacement {
            flat: f,
            sheet: p.sheet,
            dx_mm: p.dx_mm,
            dy_mm: p.dy_mm,
            rotated: p.rotated,
        })
        .collect();
    nested_dxf(&placements_ref).map_err(|e| format!("nested dxf: {e}"))
}

fn nest_impl(parts_json: &str, params_json: &str) -> Result<NestingResult, String> {
    let parts: Vec<PartFootprint> =
        serde_json::from_str(parts_json).map_err(|e| format!("parts JSON: {e}"))?;
    let params = if params_json.trim().is_empty() {
        NestingParams::generic()
    } else {
        serde_json::from_str::<NestingParams>(params_json)
            .map_err(|e| format!("nesting params JSON: {e}"))?
    };
    Ok(nest_rectangles(&parts, &params))
}

/// Return a feasible bend sequence for the chain. Outermost-first
/// heuristic; pure query, no mesh evaluation.
#[wasm_bindgen(js_name = sheetMetalSequence)]
pub fn sheet_metal_sequence(chain_json: &str) -> String {
    let result = match sequence_impl(chain_json) {
        Ok(r) => r,
        Err(msg) => SequenceResult {
            error: Some(msg),
            ..Default::default()
        },
    };
    serde_json::to_string(&result).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string())
}

fn sequence_impl(chain_json: &str) -> Result<SequenceResult, String> {
    let chain: Vec<ChainOp> =
        serde_json::from_str(chain_json).map_err(|e| format!("chain JSON: {e}"))?;
    if chain.is_empty() {
        return Err("empty op chain".to_string());
    }
    let table = BendTable::builtin();
    let model = build_model(&chain, &table)?;
    Ok(SequenceResult {
        steps: bend_sequence(&model),
        error: None,
    })
}

/// Return the built-in sheet-metal materials registry as JSON.
///
/// Lets the UI populate a material picker and the MCP tools advertise
/// what alloys are available — without each consumer hard-coding the list.
#[wasm_bindgen(js_name = getSheetMetalMaterials)]
pub fn get_sheet_metal_materials() -> String {
    let mats = vcad_kernel_sheet::builtin_materials();
    serde_json::to_string(&mats).unwrap_or_else(|_| "[]".to_string())
}

/// Return a built-in shop bending catalog (per-material fixed radius,
/// K-factor, die width, relief depth, flange minimums, max bend length) as
/// JSON. Pass `"sendcutsend"`; unknown ids return `{"error": ...}` listing
/// the available catalogs.
#[wasm_bindgen(js_name = getSheetMetalShopCatalog)]
pub fn get_sheet_metal_shop_catalog(shop_id: &str) -> String {
    match shop_catalog(shop_id) {
        Ok(cat) => serde_json::to_string(cat).unwrap_or_else(|_| "{}".to_string()),
        Err(e) => serde_json::to_string(&serde_json::json!({ "error": e.to_string() }))
            .unwrap_or_else(|_| "{}".to_string()),
    }
}

/// Return the built-in bend-table rows as JSON.
///
/// Exposes the curated `(material, t, R) → K` lookup so a shop / agent can
/// audit what K-factor an upcoming bend will use without having to model
/// the part first.
#[wasm_bindgen(js_name = getSheetMetalBendTable)]
pub fn get_sheet_metal_bend_table() -> String {
    let table = BendTable::builtin();
    let rows: Vec<_> = table
        .rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "material": r.material,
                "thickness_mm": r.thickness,
                "radius_mm": r.radius,
                "k_factor": r.k_factor,
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "id": table.id,
        "rows": rows,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

/// The shop catalog named by the chain's base op, if any.
fn chain_shop(chain: &[ChainOp]) -> Result<Option<&'static ShopCatalog>, String> {
    let shop_id = match chain.first() {
        Some(ChainOp::BaseFlangeRect { shop_profile, .. })
        | Some(ChainOp::BaseFlangePolygon { shop_profile, .. }) => shop_profile.clone(),
        _ => None,
    };
    match shop_id {
        None => Ok(None),
        Some(id) => shop_catalog(&id).map(Some).map_err(|e| e.to_string()),
    }
}

/// Resolve a bend radius (+ shop-pinned K) for one chain op.
///
/// No shop: omitted radius defaults to the material thickness. With a shop:
/// the shop's fixed radius and K apply, and a conflicting explicit radius is
/// rejected with the nearest valid radius named.
fn resolve_radius(
    shop: Option<&'static ShopCatalog>,
    material: &str,
    thickness: f64,
    requested: Option<f64>,
) -> Result<(f64, Option<(f64, String)>), String> {
    match shop {
        None => Ok((requested.unwrap_or(thickness), None)),
        Some(cat) => {
            let (r, k, label) = cat
                .resolve_bend(material, thickness, requested)
                .map_err(|e| e.to_string())?;
            Ok((r, Some((k, label))))
        }
    }
}

/// Effective relief sizing for a chain: explicit op values beat the shop's
/// published numbers beat the formula defaults.
fn chain_relief_params(
    shop: Option<&'static ShopCatalog>,
    model: &SheetMetalModel,
    width: Option<f64>,
    depth: Option<f64>,
) -> ReliefParams {
    let shop_params = shop
        .map(|cat| {
            cat.shop_profile_for(&model.material, model.thickness)
                .relief_params()
        })
        .unwrap_or_default();
    ReliefParams {
        width_mm: width.or(shop_params.width_mm),
        depth_mm: depth.or(shop_params.depth_mm),
        die_width_mm: shop_params.die_width_mm,
    }
}

fn build_model(chain: &[ChainOp], table: &BendTable) -> Result<SheetMetalModel, String> {
    let shop = chain_shop(chain)?;
    let mut iter = chain.iter();
    let base = iter
        .next()
        .ok_or_else(|| "chain has no base flange".to_string())?;
    let mut model = match base {
        ChainOp::BaseFlangeRect {
            width,
            depth,
            thickness,
            material,
            ..
        } => {
            let mut m = base_flange_rect(*width, *depth, *thickness)
                .map_err(|e| format!("base flange: {e}"))?;
            m.material = material.clone();
            m
        }
        ChainOp::BaseFlangePolygon {
            outline,
            holes,
            thickness,
            material,
            ..
        } => {
            let to_pts = |loop_pts: &[[f64; 2]]| -> Vec<Point2> {
                loop_pts.iter().map(|p| Point2::new(p[0], p[1])).collect()
            };
            let outline_pts = to_pts(outline);
            let hole_loops: Vec<Vec<Point2>> = holes.iter().map(|h| to_pts(h)).collect();
            let mut m = base_flange_polygon_with_holes(outline_pts, hole_loops, *thickness)
                .map_err(|e| format!("base flange (polygon): {e}"))?;
            m.material = material.clone();
            m
        }
        _ => {
            return Err("first chain op must be a base flange".to_string());
        }
    };
    let material = model.material.clone();
    let mut relief: Option<(Option<f64>, Option<f64>)> = None;
    for (i, op) in iter.enumerate() {
        match op {
            ChainOp::BaseFlangeRect { .. } | ChainOp::BaseFlangePolygon { .. } => {
                return Err(format!(
                    "chain op #{} is a base flange (only one allowed)",
                    i + 1
                ));
            }
            ChainOp::EdgeFlange {
                panel_id,
                edge_index,
                length,
                angle,
                radius,
                direction,
                manual_k,
            } => {
                let (radius, shop_k) = resolve_radius(shop, &material, model.thickness, *radius)
                    .map_err(|e| format!("edge flange #{}: {e}", i + 1))?;
                let params = EdgeFlangeParams {
                    panel: *panel_id,
                    edge_index: *edge_index,
                    length: *length,
                    angle: *angle,
                    radius,
                    direction: *direction,
                    position: FlangePosition::MaterialInside,
                    material: material.clone(),
                    manual_k: manual_k.or(shop_k.as_ref().map(|(k, _)| *k)),
                };
                let (_, bend_id) = add_edge_flange(&mut model, table, params)
                    .map_err(|e| format!("edge flange #{}: {e}", i + 1))?;
                // Shop-resolved K carries shop provenance, not "manual".
                if manual_k.is_none() {
                    if let Some((_, label)) = shop_k {
                        model.bends[bend_id].k_factor_source = Some(label);
                    }
                }
            }
            ChainOp::Hem {
                panel_id,
                edge_index,
                kind,
                length,
                gap,
                direction,
            } => {
                let params = HemParams {
                    panel: *panel_id,
                    edge_index: *edge_index,
                    kind: *kind,
                    length: *length,
                    gap: *gap,
                    direction: *direction,
                };
                add_hem(&mut model, table, params).map_err(|e| format!("hem #{}: {e}", i + 1))?;
            }
            ChainOp::Jog {
                panel_id,
                edge_index,
                offset,
                length,
                radius,
                direction,
            } => {
                let (radius, _) = resolve_radius(shop, &material, model.thickness, *radius)
                    .map_err(|e| format!("jog #{}: {e}", i + 1))?;
                let params = JogParams {
                    panel: *panel_id,
                    edge_index: *edge_index,
                    offset: *offset,
                    length: *length,
                    bend_radius: radius,
                    direction: *direction,
                };
                add_jog(&mut model, table, params).map_err(|e| format!("jog #{}: {e}", i + 1))?;
            }
            ChainOp::BendRelief { width, depth } => {
                relief = Some((*width, *depth));
            }
        }
    }
    // Relief is a model post-pass: it must see every bend in the chain.
    if let Some((width, depth)) = relief {
        let params = chain_relief_params(shop, &model, width, depth);
        apply_bend_relief(&mut model, &params).map_err(|e| format!("bend relief: {e}"))?;
    }
    Ok(model)
}

/// Foundation-tier tessellation: each panel becomes a thickness-`t` slab
/// (top + bottom + side quads). Bend regions are not yet rendered as
/// cylinders — adjacent panels meet at the hinge. The bend cylinders land
/// alongside the springback / lofted-flange tier; this is sufficient for
/// the property panel + flat pattern to be visibly meaningful.
fn tessellate_model(model: &SheetMetalModel) -> MeshDto {
    let mut positions: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let half_t = model.thickness * 0.5;
    let signed_area2 = |ring: &[Point2]| -> f64 {
        let mut a = 0.0;
        for i in 0..ring.len() {
            let p = ring[i];
            let q = ring[(i + 1) % ring.len()];
            a += p.x * q.y - q.x * p.y;
        }
        a
    };
    for panel in &model.panels {
        let frame = panel.frame_bent;
        let n = frame.normal();
        if panel.outline.len() < 3 {
            continue;
        }
        // Normalize ring windings locally: outline CCW, holes CW. That's the
        // documented convention, but construction doesn't enforce it and the
        // cap + wall windings below depend on it.
        let mut outline = panel.outline.clone();
        if signed_area2(&outline) < 0.0 {
            outline.reverse();
        }
        let mut holes: Vec<Vec<Point2>> = panel
            .holes
            .iter()
            .filter(|h| h.len() >= 3)
            .cloned()
            .collect();
        for h in &mut holes {
            if signed_area2(h) > 0.0 {
                h.reverse();
            }
        }
        let rings: Vec<&[Point2]> = std::iter::once(outline.as_slice())
            .chain(holes.iter().map(|h| h.as_slice()))
            .collect();

        // Top (+n) and bottom (−n) cap vertices: every ring, outline first —
        // the cap triangulation below indexes into this combined order.
        let base_top = (positions.len() / 3) as u32;
        for ring in &rings {
            for p in *ring {
                let w = frame.to_world(*p);
                positions.push((w.x + n.x * half_t) as f32);
                positions.push((w.y + n.y * half_t) as f32);
                positions.push((w.z + n.z * half_t) as f32);
                normals.push(n.x as f32);
                normals.push(n.y as f32);
                normals.push(n.z as f32);
            }
        }
        let base_bot = (positions.len() / 3) as u32;
        for ring in &rings {
            for p in *ring {
                let w = frame.to_world(*p);
                positions.push((w.x - n.x * half_t) as f32);
                positions.push((w.y - n.y * half_t) as f32);
                positions.push((w.z - n.z * half_t) as f32);
                normals.push(-n.x as f32);
                normals.push(-n.y as f32);
                normals.push(-n.z as f32);
            }
        }
        // Cap triangulation honouring hole loops (earcut; also correct for
        // concave outlines, unlike a fan). CCW triples in the panel frame
        // face +n on the top cap; the bottom cap mirrors them.
        let outer_2d: Vec<(f64, f64)> = outline.iter().map(|p| (p.x, p.y)).collect();
        let holes_2d: Vec<Vec<(f64, f64)>> = holes
            .iter()
            .map(|h| h.iter().map(|p| (p.x, p.y)).collect())
            .collect();
        if let Some(tris) = vcad_kernel_tessellate::triangulate_polygon_2d(&outer_2d, &holes_2d) {
            for t in &tris {
                indices.push(base_top + t[0]);
                indices.push(base_top + t[1]);
                indices.push(base_top + t[2]);
            }
            for t in &tris {
                indices.push(base_bot + t[0]);
                indices.push(base_bot + t[2]);
                indices.push(base_bot + t[1]);
            }
        }
        // Lateral walls: outline perimeter + one bore per hole. With the
        // outline CCW and holes CW, `edge × n` is the outward
        // (away-from-material) normal for BOTH ring kinds, and one quad
        // winding faces outward for both.
        for ring in &rings {
            for i in 0..ring.len() {
                let a = ring[i];
                let b = ring[(i + 1) % ring.len()];
                let a_world = frame.to_world(a);
                let b_world = frame.to_world(b);
                let edge_x = b_world.x - a_world.x;
                let edge_y = b_world.y - a_world.y;
                let edge_z = b_world.z - a_world.z;
                // Side normal = edge × n, normalised.
                let mut snx = edge_y * n.z - edge_z * n.y;
                let mut sny = edge_z * n.x - edge_x * n.z;
                let mut snz = edge_x * n.y - edge_y * n.x;
                let m = (snx * snx + sny * sny + snz * snz).sqrt();
                if m > 1e-12 {
                    snx /= m;
                    sny /= m;
                    snz /= m;
                } else {
                    snx = 0.0;
                    sny = 0.0;
                    snz = 1.0;
                }
                let base = (positions.len() / 3) as u32;
                // a_top, b_top, b_bot, a_bot.
                let push = |positions: &mut Vec<f32>, normals: &mut Vec<f32>, x, y, z| {
                    positions.push(x);
                    positions.push(y);
                    positions.push(z);
                    normals.push(snx as f32);
                    normals.push(sny as f32);
                    normals.push(snz as f32);
                };
                push(
                    &mut positions,
                    &mut normals,
                    (a_world.x + n.x * half_t) as f32,
                    (a_world.y + n.y * half_t) as f32,
                    (a_world.z + n.z * half_t) as f32,
                );
                push(
                    &mut positions,
                    &mut normals,
                    (b_world.x + n.x * half_t) as f32,
                    (b_world.y + n.y * half_t) as f32,
                    (b_world.z + n.z * half_t) as f32,
                );
                push(
                    &mut positions,
                    &mut normals,
                    (b_world.x - n.x * half_t) as f32,
                    (b_world.y - n.y * half_t) as f32,
                    (b_world.z - n.z * half_t) as f32,
                );
                push(
                    &mut positions,
                    &mut normals,
                    (a_world.x - n.x * half_t) as f32,
                    (a_world.y - n.y * half_t) as f32,
                    (a_world.z - n.z * half_t) as f32,
                );
                // Wound so the face agrees with `edge × n` (outward). The
                // previous fan-era winding faced inward, which broke the
                // directed-edge pairing and the signed-volume integral.
                indices.push(base);
                indices.push(base + 2);
                indices.push(base + 1);
                indices.push(base);
                indices.push(base + 3);
                indices.push(base + 2);
            }
        }
    }
    MeshDto {
        positions,
        indices,
        normals,
    }
}

fn flat_pattern_to_dto(flat: FlatPattern) -> FlatPatternDto {
    let bbox = flat.bbox();
    let panel_outlines_2d = flat
        .panel_outlines_2d
        .iter()
        .map(|o| o.iter().map(|p| [p.x, p.y]).collect::<Vec<[f64; 2]>>())
        .collect();
    let panel_holes_2d = flat
        .panel_holes_2d
        .iter()
        .map(|panel_holes| {
            panel_holes
                .iter()
                .map(|hole| hole.iter().map(|p| [p.x, p.y]).collect())
                .collect()
        })
        .collect();
    // Merged silhouette: exterior first, then holes. Best-effort — a
    // disconnected pattern already fails the DXF with a diagnostic, so the
    // DTO just carries an empty list here.
    let silhouette_2d = match vcad_kernel_sheet::silhouette(&flat) {
        Ok(s) => {
            let ring = |r: &[Point2]| r.iter().map(|p| [p.x, p.y]).collect::<Vec<[f64; 2]>>();
            std::iter::once(ring(&s.exterior))
                .chain(s.holes.iter().map(|h| ring(h)))
                .collect()
        }
        Err(_) => Vec::new(),
    };
    let creases = flat
        .creases
        .iter()
        .map(|c| FlatCreaseDto {
            line: [[c.line.0.x, c.line.0.y], [c.line.1.x, c.line.1.y]],
            angle: c.angle,
            radius: c.radius,
            k_factor: c.k_factor,
            k_factor_source: c.k_factor_source.clone(),
            direction: c.direction,
            bend_id: c.bend_id,
        })
        .collect();
    FlatPatternDto {
        thickness: flat.thickness,
        panel_outlines_2d,
        panel_holes_2d,
        silhouette_2d,
        creases,
        area_mm2: flat.area_mm2,
        bbox: [bbox.0 .0, bbox.0 .1, bbox.1 .0, bbox.1 .1],
    }
}

fn summarise_model(model: &SheetMetalModel) -> ModelSummaryDto {
    // Material-driven; zero for an unspecified material so the
    // compensated angle equals the design angle.
    let springback_factor = model.springback_per_radian();
    let bends = model
        .bends
        .iter()
        .map(|b| {
            let springback_rad = springback_factor * b.angle;
            BendSummaryDto {
                parent: b.parent,
                child: b.child,
                angle_rad: b.angle,
                radius: b.radius,
                direction: b.direction,
                k_factor: b.k_factor,
                k_factor_source: b.k_factor_source.clone(),
                allowance_mm: bend_table::bend_allowance(
                    b.angle,
                    b.radius,
                    b.k_factor,
                    model.thickness,
                ),
                springback_rad,
                compensated_angle_rad: b.angle + springback_rad,
            }
        })
        .collect();
    ModelSummaryDto {
        thickness: model.thickness,
        material: model.material.clone(),
        panel_count: model.panels.len(),
        bend_count: model.bends.len(),
        bends,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_u_channel() {
        let chain = r#"[
            {"type":"BaseFlangeRect","width":100,"depth":50,"thickness":1.0,"material":"Al-soft"},
            {"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":25,"angle":1.5707963267948966,"radius":1.0,"direction":"Up"},
            {"type":"EdgeFlange","panelId":0,"edgeIndex":2,"length":25,"angle":1.5707963267948966,"radius":1.0,"direction":"Up"}
        ]"#;
        let out = evaluate_sheet_metal_chain(chain);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed["error"].is_null(), "got error: {parsed}");
        assert_eq!(parsed["model"]["panel_count"], 3);
        assert_eq!(parsed["model"]["bend_count"], 2);
        assert!(!parsed["mesh"]["positions"].as_array().unwrap().is_empty());
        assert_eq!(
            parsed["flat_pattern"]["creases"].as_array().unwrap().len(),
            2
        );
        let dxf = parsed["dxf"].as_str().unwrap();
        assert!(dxf.contains("0\nLAYER\n2\nCUT\n"));
        assert!(dxf.contains("0\nLINE\n8\nBEND_UP"));
        assert!(dxf.trim_end().ends_with("0\nEOF"));
        // 100x50 base with two 25 mm flanges is shop-ready.
        assert_eq!(
            parsed["violations"].as_array().unwrap().len(),
            0,
            "expected shop-ready, got {}",
            parsed["violations"]
        );
    }

    #[test]
    fn surfaces_manufacturability_violations() {
        // 2 mm flange off a 1 mm sheet is below the 5 mm minimum.
        let chain = r#"[
            {"type":"BaseFlangeRect","width":100,"depth":50,"thickness":1.0,"material":"Al-soft"},
            {"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":2,"angle":1.5707963267948966,"radius":1.0,"direction":"Up","manualK":0.42}
        ]"#;
        let parsed: serde_json::Value =
            serde_json::from_str(&evaluate_sheet_metal_chain(chain)).unwrap();
        let viols = parsed["violations"].as_array().unwrap();
        assert!(!viols.is_empty());
        assert_eq!(viols[0]["severity"], "Error");
        assert_eq!(viols[0]["rule"], "sheet.flange_height");
        assert_eq!(viols[0]["detail"]["kind"], "FlangeBelowMinHeight");
    }

    #[test]
    fn error_on_empty_chain() {
        let parsed: serde_json::Value =
            serde_json::from_str(&evaluate_sheet_metal_chain("[]")).unwrap();
        assert!(parsed["error"].as_str().unwrap().contains("empty"));
    }

    #[test]
    fn error_when_first_is_edge_flange() {
        let chain = r#"[{"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":25,"angle":1.57,"radius":1,"direction":"Up"}]"#;
        let parsed: serde_json::Value =
            serde_json::from_str(&evaluate_sheet_metal_chain(chain)).unwrap();
        assert!(parsed["error"].as_str().unwrap().contains("base flange"));
    }

    const CLEAN_CHAIN: &str = r#"[
        {"type":"BaseFlangeRect","width":100,"depth":50,"thickness":1.0,"material":"Al-soft"},
        {"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":25,"angle":1.5707963267948966,"radius":1.0,"direction":"Up","manualK":0.42}
    ]"#;

    #[test]
    fn check_sheet_metal_generic_shop_is_clean() {
        let parsed: serde_json::Value =
            serde_json::from_str(&check_sheet_metal(CLEAN_CHAIN, "")).unwrap();
        assert!(parsed["error"].is_null(), "got {parsed}");
        assert_eq!(parsed["violations"].as_array().unwrap().len(), 0);
        assert_eq!(parsed["shop"]["name"], "Generic shop");
    }

    #[test]
    fn check_sheet_metal_custom_shop_flags_radius() {
        // Stricter shop: R/t ≥ 4 → the 1 mm radius on 1 mm stock fails.
        let shop = r#"{"name":"Strict Inc","min_bend_radius_ratio":4.0}"#;
        let parsed: serde_json::Value =
            serde_json::from_str(&check_sheet_metal(CLEAN_CHAIN, shop)).unwrap();
        assert!(parsed["error"].is_null(), "got {parsed}");
        let viols = parsed["violations"].as_array().unwrap();
        assert!(viols
            .iter()
            .any(|v| v["detail"]["kind"] == "BendRadiusBelowMinimum"));
        // Field-tolerant merge kept the generic brake length.
        assert_eq!(parsed["shop"]["name"], "Strict Inc");
        assert_eq!(parsed["shop"]["max_bend_length_mm"], 3000.0);
    }

    #[test]
    fn check_sheet_metal_reports_bad_shop_json() {
        let parsed: serde_json::Value =
            serde_json::from_str(&check_sheet_metal(CLEAN_CHAIN, "{not json")).unwrap();
        assert!(parsed["error"].as_str().unwrap().contains("shop JSON"));
    }
}

/// Result of [`sheet_metal_folded_step`]: the full ASCII STEP file text,
/// or an error message. Exactly one of the two is meaningful.
#[derive(Debug, Clone, Serialize, Default)]
struct FoldedStepResult {
    /// Full STEP AP214 file contents. Empty string on error.
    step: String,
    /// Error message; `null` on success.
    error: Option<String>,
}

/// Export the **folded** sheet-metal solid as a STEP AP214 file.
///
/// Builds the model from the same chain JSON that
/// [`evaluate_sheet_metal_chain`] accepts, constructs the folded B-rep via
/// `vcad_kernel::folded_sheet_solid` (panel slabs + true cylindrical bend
/// sectors, unioned into one body), and serialises it to STEP. The
/// cylindrical bend faces let downstream fab pipelines (e.g. SendCutSend)
/// auto-detect bend radii, angles, and directions.
///
/// Returns JSON: `{"step": "<full ASCII STEP file>", "error": null}` on
/// success or `{"step": "", "error": "..."}` on failure. Never panics.
#[wasm_bindgen(js_name = sheetMetalFoldedStep)]
pub fn sheet_metal_folded_step(chain_json: &str) -> String {
    let result = match folded_step_impl(chain_json) {
        Ok(step) => FoldedStepResult { step, error: None },
        Err(msg) => FoldedStepResult {
            step: String::new(),
            error: Some(msg),
        },
    };
    serde_json::to_string(&result)
        .unwrap_or_else(|_| r#"{"step":"","error":"serialize failed"}"#.to_string())
}

fn folded_step_impl(chain_json: &str) -> Result<String, String> {
    let chain: Vec<ChainOp> =
        serde_json::from_str(chain_json).map_err(|e| format!("chain JSON: {e}"))?;
    if chain.is_empty() {
        return Err("empty op chain".to_string());
    }
    let table = BendTable::builtin();
    let model = build_model(&chain, &table)?;
    let solid = vcad_kernel::folded_sheet_solid(&model, 32)?;
    let buf = solid
        .to_step_buffer()
        .map_err(|e| format!("STEP export: {e}"))?;
    String::from_utf8(buf).map_err(|e| format!("STEP is not valid UTF-8: {e}"))
}

#[cfg(test)]
mod folded_step_tests {
    use super::*;

    #[test]
    fn folded_step_exports_cylindrical_bend_faces() {
        let chain = r#"[
            {"type":"BaseFlangeRect","width":60,"depth":40,"thickness":2,"material":"Al-soft"},
            {"type":"EdgeFlange","panelId":0,"edgeIndex":0,"length":30,"angle":1.5707963267948966,"radius":2.0,"direction":"Up","manualK":0.44}
        ]"#;
        let out = sheet_metal_folded_step(chain);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["error"].is_null(), "unexpected error: {}", v["error"]);
        let step = v["step"].as_str().unwrap();
        assert!(step.starts_with("ISO-10303-21"), "not a STEP file");
        assert!(step.contains("CYLINDRICAL_SURFACE"));
    }

    #[test]
    fn folded_step_reports_errors_as_json() {
        let out = sheet_metal_folded_step("not json");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["step"], "");
        assert!(v["error"].as_str().unwrap().contains("chain JSON"));
    }
}
