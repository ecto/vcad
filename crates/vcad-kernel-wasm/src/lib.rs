//! WASM bindings for the vcad B-rep kernel.
//!
//! Exposes the [`Solid`] type for use in JavaScript/TypeScript via wasm-bindgen.
//!
//! ## TypeScript Type Generation
//!
//! When compiled with the `ts-rs` feature, this crate exports TypeScript type definitions
//! for all serializable types. Run `cargo test --features ts-rs` to generate types.

pub mod document_engine;
pub mod keybindings;
pub mod sketch_session;

use serde::{Deserialize, Serialize};
use vcad_kernel::vcad_kernel_math::{Point2, Point3, Vec3};
use vcad_kernel::vcad_kernel_sketch::{SketchProfile, SketchSegment};
use wasm_bindgen::prelude::*;
use wasmosis::module;

#[cfg(feature = "ts-rs")]
use ts_rs::TS;

/// Version string for verifying correct WASM build is loaded in browser.
const KERNEL_VERSION: &str = "2026-04-23-sphere-vertex-blend";

/// Get the kernel version string.
/// Use this in browser console to verify the correct WASM build is loaded:
/// `kernelWasm.get_kernel_version()` should return "2025-02-21-step-facebound-fix"
#[wasm_bindgen]
pub fn get_kernel_version() -> String {
    KERNEL_VERSION.to_string()
}

/// Get tool schema definitions for all CsgOp variants.
/// Returns JSON array of ToolSchemaEntry objects.
#[wasm_bindgen]
pub fn get_tool_schemas() -> String {
    serde_json::to_string(&vcad_ir::CsgOp::tool_schemas()).unwrap()
}

/// Get the five Anthropic CRUD tool definitions
/// (`create` / `read` / `update` / `delete` / `set_material`) as a JSON
/// array, with the `create` tool's `type` enum pre-populated from the
/// kernel's tool schema list. Consumers on the web (TypeScript
/// `CommandRegistry.toAnthropicTools`) and in the TUI (`vcad_chat::
/// anthropic_tools`) render byte-identical payloads — single source of
/// truth lives in `vcad-chat::tools`.
#[wasm_bindgen]
pub fn get_anthropic_tools_json() -> String {
    serde_json::to_string(&vcad_chat::anthropic_tools()).unwrap()
}

/// Plan a chat tool call against the current document snapshot.
///
/// This is the web-side entry point for the Rust chat executor: the TS
/// web app serializes its current `Document`, hands it plus the tool
/// name and args to this function, and gets back a JSON
/// `PlannedResponse` that describes the mutation to perform. The TS
/// caller then dispatches the outcome through the CRDT engine's
/// existing methods (`add_feature` / `setFeatureParam` / `removePart` /
/// `setPartMaterial`) — which keeps CRDT op logs in sync and preserves
/// undo, while sharing the validation + argument parsing logic with
/// the TUI via `vcad_chat::plan_crud`.
///
/// `doc_json` must deserialize into `vcad_ir::Document`; a parse
/// failure treats the doc as empty (an empty Document never validates
/// any id lookups, so planners that need to check part_id existence
/// will return a clean error).
/// Cap on how large the caller-supplied JSON strings can be. The host JS
/// always originates these, but guarding the boundary here keeps a
/// renderer bug from pushing hundreds of MB of JSON through serde on every
/// chat tool invocation.
const MAX_PLAN_CHAT_JSON_BYTES: usize = 32 * 1024 * 1024;

#[wasm_bindgen]
pub fn plan_chat_tool(tool: &str, args_json: &str, doc_json: &str) -> String {
    if args_json.len() > MAX_PLAN_CHAT_JSON_BYTES || doc_json.len() > MAX_PLAN_CHAT_JSON_BYTES {
        return r#"{"error":"plan_chat_tool: input exceeds size limit"}"#.to_string();
    }
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    let doc: vcad_ir::Document = serde_json::from_str(doc_json).unwrap_or_default();
    let response = vcad_chat::plan_crud(tool, &args, &doc);
    serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string())
}

/// Build the system prompt sent with every `/api/chat` request.
///
/// `parts_json` must deserialize into `Vec<vcad_chat::PartInfo>` (the TS
/// web caller already walks its own document store to build this shape,
/// so we accept it pre-built rather than reserializing the full Document
/// through the wasm boundary on every request). `selection_json` must
/// deserialize into `Vec<vcad_chat::SelectionInfo>`. Either defaults to
/// an empty array on parse failure.
///
/// Returns the rendered prompt string — byte-identical to what the TUI
/// produces via `vcad_chat::build_system_prompt` for the same inputs.
#[wasm_bindgen]
pub fn build_chat_system_prompt(parts_json: &str, selection_json: &str) -> String {
    let parts: Vec<vcad_chat::PartInfo> = serde_json::from_str(parts_json).unwrap_or_default();
    let selection: Vec<vcad_chat::SelectionInfo> =
        serde_json::from_str(selection_json).unwrap_or_default();
    vcad_chat::build_system_prompt(&parts, &selection)
}

// =============================================================================
// DFM (Design for Manufacturing)
// =============================================================================

/// Return the bundled default rule pack (TOML) for a process name.
///
/// Process names: `"cnc_3axis"`, `"fdm"`, `"sla"`, `"injection"`,
/// `"sheet_metal"`, `"casting_sand"`, `"casting_investment"`.
#[wasm_bindgen]
pub fn get_default_dfm_pack(process: &str) -> Result<String, JsError> {
    let p = vcad_kernel::vcad_kernel_dfm::Process::from_str(process)
        .ok_or_else(|| JsError::new(&format!("unknown process: {}", process)))?;
    Ok(vcad_kernel::vcad_kernel_dfm::DefaultPacks::source(p).to_string())
}

/// Estimate manufacturing cost for the supplied process + material.
///
/// `part_volume_mm3` is the exact part volume the caller has already
/// computed; `stock_volume_mm3` is only used for CNC (defaults to
/// `part_volume_mm3 * 2` if non-positive). `qty` matters for
/// mold/casting amortization; `feature_count` matters for CNC time.
/// Material names match the catalog in `vcad_kernel::vcad_kernel_cost::Material`.
#[wasm_bindgen]
pub fn estimate_cost_for_process(
    process: &str,
    material_name: &str,
    part_volume_mm3: f64,
    stock_volume_mm3: f64,
    qty: u32,
    feature_count: u32,
) -> Result<JsValue, JsError> {
    use vcad_kernel::vcad_kernel_cost::Material;
    let p = vcad_kernel::vcad_kernel_dfm::Process::from_str(process)
        .ok_or_else(|| JsError::new(&format!("unknown process: {}", process)))?;
    let mat = Material::catalog()
        .into_iter()
        .find(|m| m.name.eq_ignore_ascii_case(material_name))
        .unwrap_or_else(Material::pla);
    let stock_v = if stock_volume_mm3 > 0.0 {
        stock_volume_mm3
    } else {
        part_volume_mm3 * 2.0
    };
    let estimate = match p {
        vcad_kernel::vcad_kernel_cost::Process::Fdm
        | vcad_kernel::vcad_kernel_cost::Process::Sla => {
            vcad_kernel::vcad_kernel_cost::estimate_fdm_from_volume(
                part_volume_mm3,
                0.20,
                3,
                0.45,
                &mat,
            )
        }
        vcad_kernel::vcad_kernel_cost::Process::Cnc3Axis => {
            vcad_kernel::vcad_kernel_cost::estimate_cnc_from_removed_volume(
                stock_v,
                part_volume_mm3,
                feature_count,
                &mat,
            )
        }
        vcad_kernel::vcad_kernel_cost::Process::Injection => {
            let q = if qty == 0 { 1000 } else { qty };
            vcad_kernel::vcad_kernel_cost::estimate_injection(part_volume_mm3, q, &mat)
        }
        vcad_kernel::vcad_kernel_cost::Process::SheetMetal => {
            // Caller supplies the bounding-box style approximation; v1
            // treats stock_volume_mm3 as area * thickness via
            // part_volume_mm3 + thickness fallback.
            let thickness = (part_volume_mm3 / stock_v.max(1.0)).max(0.5);
            let area = stock_v.max(1.0);
            vcad_kernel::vcad_kernel_cost::estimate_sheet_metal(area, thickness, 0, &mat)
        }
        vcad_kernel::vcad_kernel_cost::Process::CastingSand
        | vcad_kernel::vcad_kernel_cost::Process::CastingInvestment => {
            vcad_kernel::vcad_kernel_cost::estimate_casting(p, part_volume_mm3, qty, 0, &mat)
        }
    };
    serde_wasm_bindgen::to_value(&estimate).map_err(|e| JsError::new(&e.to_string()))
}

/// Initialize the WASM module (sets up panic hook for better error messages).
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
    // Version marker to verify correct WASM is loaded
    web_sys::console::log_1(&format!("[WASM] vcad-kernel-wasm {} loaded", KERNEL_VERSION).into());
}

/// Triangle mesh output for rendering.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "generated/"))]
pub struct WasmMesh {
    /// Flat array of vertex positions: [x0, y0, z0, x1, y1, z1, ...]
    pub positions: Vec<f32>,
    /// Flat array of triangle indices: [i0, i1, i2, ...]
    pub indices: Vec<u32>,
    /// Flat array of vertex normals: [nx0, ny0, nz0, ...] (same length as positions).
    /// When present, these are analytical surface normals for moiré-free rendering.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normals: Option<Vec<f32>>,
    /// Optional per-triangle face-kind tag (same length as `indices / 3`).
    /// Values: 0 = Unknown, 1 = Plane, 2 = Cylinder, 3 = Sphere,
    /// 4 = Cone, 5 = Bilinear, 6 = Torus, 7 = BSpline, 8 = FanFill.
    /// Used by the viewport's click-to-inspect debugger.
    #[serde(rename = "faceKinds", skip_serializing_if = "Option::is_none")]
    pub face_kinds: Option<Vec<u8>>,
}

/// A 2D sketch segment (line or arc) for WASM input.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "generated/"))]
pub enum WasmSketchSegment {
    Line {
        start: [f64; 2],
        end: [f64; 2],
    },
    Arc {
        start: [f64; 2],
        end: [f64; 2],
        center: [f64; 2],
        ccw: bool,
    },
}

/// Input for creating a sketch profile from JS.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "generated/"))]
pub struct WasmSketchProfile {
    /// Origin point of the sketch plane [x, y, z].
    pub origin: [f64; 3],
    /// X direction vector [x, y, z].
    pub x_dir: [f64; 3],
    /// Y direction vector [x, y, z].
    pub y_dir: [f64; 3],
    /// Segments forming the closed profile.
    pub segments: Vec<WasmSketchSegment>,
}

impl WasmSketchProfile {
    fn to_kernel_profile(&self) -> Result<SketchProfile, String> {
        let segments: Vec<SketchSegment> = self
            .segments
            .iter()
            .map(|s| match s {
                WasmSketchSegment::Line { start, end } => SketchSegment::Line {
                    start: Point2::new(start[0], start[1]),
                    end: Point2::new(end[0], end[1]),
                },
                WasmSketchSegment::Arc {
                    start,
                    end,
                    center,
                    ccw,
                } => SketchSegment::Arc {
                    start: Point2::new(start[0], start[1]),
                    end: Point2::new(end[0], end[1]),
                    center: Point2::new(center[0], center[1]),
                    ccw: *ccw,
                },
            })
            .collect();

        SketchProfile::new(
            Point3::new(self.origin[0], self.origin[1], self.origin[2]),
            Vec3::new(self.x_dir[0], self.x_dir[1], self.x_dir[2]),
            Vec3::new(self.y_dir[0], self.y_dir[1], self.y_dir[2]),
            segments,
        )
        .map_err(|e| e.to_string())
    }

    /// Convert to kernel profile with coordinates centered around (0, 0).
    /// This is useful for sweep operations where the profile should be
    /// centered on the path.
    fn to_kernel_profile_centered(&self) -> Result<SketchProfile, String> {
        // Filter out degenerate (zero-length) segments first
        let valid_segments: Vec<_> = self
            .segments
            .iter()
            .filter(|seg| {
                let (start, end) = match seg {
                    WasmSketchSegment::Line { start, end } => (start, end),
                    WasmSketchSegment::Arc { start, end, .. } => (start, end),
                };
                let dx = end[0] - start[0];
                let dy = end[1] - start[1];
                (dx * dx + dy * dy).sqrt() > 1e-9
            })
            .collect();

        if valid_segments.is_empty() {
            return Err("No valid (non-degenerate) segments in profile".into());
        }

        // Compute centroid of valid segment start points only
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut count = 0;

        for seg in &valid_segments {
            let (sx, sy) = match seg {
                WasmSketchSegment::Line { start, .. } => (start[0], start[1]),
                WasmSketchSegment::Arc { start, .. } => (start[0], start[1]),
            };
            sum_x += sx;
            sum_y += sy;
            count += 1;
        }

        let (cx, cy) = if count > 0 {
            (sum_x / count as f64, sum_y / count as f64)
        } else {
            (0.0, 0.0)
        };

        // Create centered segments from valid segments only
        let segments: Vec<SketchSegment> = valid_segments
            .iter()
            .map(|s| match s {
                WasmSketchSegment::Line { start, end } => SketchSegment::Line {
                    start: Point2::new(start[0] - cx, start[1] - cy),
                    end: Point2::new(end[0] - cx, end[1] - cy),
                },
                WasmSketchSegment::Arc {
                    start,
                    end,
                    center,
                    ccw,
                } => SketchSegment::Arc {
                    start: Point2::new(start[0] - cx, start[1] - cy),
                    end: Point2::new(end[0] - cx, end[1] - cy),
                    center: Point2::new(center[0] - cx, center[1] - cy),
                    ccw: *ccw,
                },
            })
            .collect();

        SketchProfile::new(
            Point3::new(self.origin[0], self.origin[1], self.origin[2]),
            Vec3::new(self.x_dir[0], self.x_dir[1], self.x_dir[2]),
            Vec3::new(self.y_dir[0], self.y_dir[1], self.y_dir[2]),
            segments,
        )
        .map_err(|e| e.to_string())
    }
}

/// A 3D solid geometry object.
///
/// Create solids from primitives, combine with boolean operations,
/// transform, and extract triangle meshes for rendering.
#[wasm_bindgen]
pub struct Solid {
    inner: vcad_kernel::Solid,
}

#[wasm_bindgen]
impl Solid {
    // =========================================================================
    // Constructors
    // =========================================================================

    /// Create an empty solid.
    #[wasm_bindgen(js_name = empty)]
    pub fn empty() -> Solid {
        Solid {
            inner: vcad_kernel::Solid::empty(),
        }
    }

    /// Create a box with corner at origin and dimensions (sx, sy, sz).
    #[wasm_bindgen(js_name = cube)]
    pub fn cube(sx: f64, sy: f64, sz: f64) -> Solid {
        let solid = Solid {
            inner: vcad_kernel::Solid::cube(sx, sy, sz),
        };
        let (min, max) = solid.inner.bounding_box();
        web_sys::console::log_1(
            &format!(
                "[WASM] Created cube({},{},{}): bbox=[{:.2},{:.2},{:.2}]->[{:.2},{:.2},{:.2}]",
                sx, sy, sz, min[0], min[1], min[2], max[0], max[1], max[2]
            )
            .into(),
        );
        solid
    }

    /// Create a cylinder along Z axis with given radius and height.
    #[wasm_bindgen(js_name = cylinder)]
    pub fn cylinder(radius: f64, height: f64, segments: Option<u32>) -> Solid {
        let segs = segments.unwrap_or(32);
        let solid = Solid {
            inner: vcad_kernel::Solid::cylinder(radius, height, segs),
        };
        let (min, max) = solid.inner.bounding_box();
        web_sys::console::log_1(&format!(
            "[WASM] Created cylinder(r={}, h={}, segs={}): bbox=[{:.2},{:.2},{:.2}]->[{:.2},{:.2},{:.2}]",
            radius, height, segs, min[0], min[1], min[2], max[0], max[1], max[2]
        ).into());
        solid
    }

    /// Create a sphere centered at origin with given radius.
    #[wasm_bindgen(js_name = sphere)]
    pub fn sphere(radius: f64, segments: Option<u32>) -> Solid {
        Solid {
            inner: vcad_kernel::Solid::sphere(radius, segments.unwrap_or(32)),
        }
    }

    /// Create a cone/frustum along Z axis.
    #[wasm_bindgen(js_name = cone)]
    pub fn cone(radius_bottom: f64, radius_top: f64, height: f64, segments: Option<u32>) -> Solid {
        Solid {
            inner: vcad_kernel::Solid::cone(
                radius_bottom,
                radius_top,
                height,
                segments.unwrap_or(32),
            ),
        }
    }

    /// Create a solid by extruding a 2D sketch profile.
    ///
    /// Takes a sketch profile and extrusion direction as JS objects.
    #[wasm_bindgen(js_name = extrude)]
    pub fn extrude(profile_json: String, direction: Vec<f64>) -> Result<Solid, JsError> {
        let profile: WasmSketchProfile = serde_json::from_str(&profile_json)
            .map_err(|e| JsError::new(&format!("Invalid profile: {}", e)))?;

        if direction.len() != 3 {
            return Err(JsError::new("Direction must have 3 components"));
        }

        let kernel_profile = profile.to_kernel_profile().map_err(|e| JsError::new(&e))?;

        let dir = Vec3::new(direction[0], direction[1], direction[2]);

        vcad_kernel::Solid::extrude(kernel_profile, dir)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Create a solid by extruding a 2D sketch profile with twist and/or scale.
    ///
    /// Takes a sketch profile, extrusion direction, twist angle (radians),
    /// and scale factor at the end (1.0 = no taper).
    #[wasm_bindgen(js_name = extrudeWithOptions)]
    pub fn extrude_with_options(
        profile_json: String,
        direction: Vec<f64>,
        twist_angle: f64,
        scale_end: f64,
    ) -> Result<Solid, JsError> {
        let profile: WasmSketchProfile = serde_json::from_str(&profile_json)
            .map_err(|e| JsError::new(&format!("Invalid profile: {}", e)))?;

        if direction.len() != 3 {
            return Err(JsError::new("Direction must have 3 components"));
        }

        let kernel_profile = profile.to_kernel_profile().map_err(|e| JsError::new(&e))?;

        let dir = Vec3::new(direction[0], direction[1], direction[2]);

        vcad_kernel::Solid::extrude_with_options(kernel_profile, dir, twist_angle, scale_end)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Create a solid by revolving a 2D sketch profile around an axis.
    ///
    /// Takes a sketch profile, axis origin, axis direction, and angle in degrees.
    #[wasm_bindgen(js_name = revolve)]
    pub fn revolve(
        profile_json: String,
        axis_origin: Vec<f64>,
        axis_dir: Vec<f64>,
        angle_deg: f64,
    ) -> Result<Solid, JsError> {
        let profile: WasmSketchProfile = serde_json::from_str(&profile_json)
            .map_err(|e| JsError::new(&format!("Invalid profile: {}", e)))?;

        if axis_origin.len() != 3 || axis_dir.len() != 3 {
            return Err(JsError::new(
                "Axis origin and direction must have 3 components",
            ));
        }

        let kernel_profile = profile.to_kernel_profile().map_err(|e| JsError::new(&e))?;

        let origin = Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]);
        let dir = Vec3::new(axis_dir[0], axis_dir[1], axis_dir[2]);

        vcad_kernel::Solid::revolve(kernel_profile, origin, dir, angle_deg)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Create a solid by sweeping a profile along a line path.
    ///
    /// Takes a sketch profile and path endpoints.
    #[wasm_bindgen(js_name = sweepLine)]
    pub fn sweep_line(
        profile_json: String,
        start: Vec<f64>,
        end: Vec<f64>,
        twist_angle: Option<f64>,
        scale_start: Option<f64>,
        scale_end: Option<f64>,
        orientation: Option<f64>,
    ) -> Result<Solid, JsError> {
        use vcad_kernel::vcad_kernel_geom::Line3d;
        use vcad_kernel::vcad_kernel_sweep::SweepOptions;

        let profile: WasmSketchProfile = serde_json::from_str(&profile_json)
            .map_err(|e| JsError::new(&format!("Invalid profile: {}", e)))?;

        if start.len() != 3 || end.len() != 3 {
            return Err(JsError::new("Start and end must have 3 components"));
        }

        // Use centered profile so it wraps around the path properly
        let kernel_profile = profile
            .to_kernel_profile_centered()
            .map_err(|e| JsError::new(&e))?;

        let path = Line3d::from_points(
            Point3::new(start[0], start[1], start[2]),
            Point3::new(end[0], end[1], end[2]),
        );

        let options = SweepOptions {
            twist_angle: twist_angle.unwrap_or(0.0),
            scale_start: scale_start.unwrap_or(1.0),
            scale_end: scale_end.unwrap_or(1.0),
            orientation_angle: orientation.unwrap_or(0.0),
            ..Default::default()
        };

        vcad_kernel::Solid::sweep(kernel_profile, &path, options)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Create a solid by sweeping a profile along a helix path.
    ///
    /// Takes a sketch profile and helix parameters.
    #[wasm_bindgen(js_name = sweepHelix)]
    #[allow(clippy::too_many_arguments)]
    pub fn sweep_helix(
        profile_json: String,
        radius: f64,
        pitch: f64,
        height: f64,
        turns: f64,
        twist_angle: Option<f64>,
        scale_start: Option<f64>,
        scale_end: Option<f64>,
        path_segments: Option<u32>,
        arc_segments: Option<u32>,
        orientation: Option<f64>,
    ) -> Result<Solid, JsError> {
        use vcad_kernel::vcad_kernel_sweep::{Helix, SweepOptions};

        let profile: WasmSketchProfile = serde_json::from_str(&profile_json)
            .map_err(|e| JsError::new(&format!("Invalid profile: {}", e)))?;

        // Use centered profile so it wraps around the helix path properly
        let kernel_profile = profile
            .to_kernel_profile_centered()
            .map_err(|e| JsError::new(&e))?;

        let path = Helix::new(radius, pitch, height, turns);

        let options = SweepOptions {
            twist_angle: twist_angle.unwrap_or(0.0),
            scale_start: scale_start.unwrap_or(1.0),
            scale_end: scale_end.unwrap_or(1.0),
            path_segments: path_segments.unwrap_or(0),
            arc_segments: arc_segments.unwrap_or(8),
            orientation_angle: orientation.unwrap_or(0.0),
        };

        vcad_kernel::Solid::sweep(kernel_profile, &path, options)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Create a solid by lofting between multiple profiles.
    ///
    /// Takes an array of sketch profiles (minimum 2).
    #[wasm_bindgen(js_name = loft)]
    pub fn loft(profiles_json: String, closed: Option<bool>) -> Result<Solid, JsError> {
        use vcad_kernel::vcad_kernel_sweep::{LoftMode, LoftOptions};

        let profiles: Vec<WasmSketchProfile> = serde_json::from_str(&profiles_json)
            .map_err(|e| JsError::new(&format!("Invalid profiles: {}", e)))?;

        if profiles.len() < 2 {
            return Err(JsError::new("Loft requires at least 2 profiles"));
        }

        let kernel_profiles: Result<Vec<_>, _> =
            profiles.iter().map(|p| p.to_kernel_profile()).collect();
        let kernel_profiles = kernel_profiles.map_err(|e| JsError::new(&e))?;

        let options = LoftOptions {
            mode: LoftMode::Ruled,
            closed: closed.unwrap_or(false),
        };

        vcad_kernel::Solid::loft(&kernel_profiles, options)
            .map(|inner| Solid { inner })
            .map_err(|e| JsError::new(&e.to_string()))
    }

    // =========================================================================
    // Boolean operations
    // =========================================================================

    /// Boolean union (self ∪ other).
    #[wasm_bindgen(js_name = union)]
    pub fn union(&self, other: &Solid) -> Solid {
        Solid {
            inner: self.inner.union(&other.inner),
        }
    }

    /// Boolean difference (self − other).
    #[wasm_bindgen(js_name = difference)]
    pub fn difference(&self, other: &Solid) -> Solid {
        // Log input solid info with more detail
        let self_tris = self.inner.num_triangles();
        let other_tris = other.inner.num_triangles();

        // Get detailed info about inputs
        let (self_min, self_max) = self.inner.bounding_box();
        let (other_min, other_max) = other.inner.bounding_box();

        web_sys::console::log_1(&format!(
            "[WASM] Boolean difference inputs:\n  self: {} tris, bbox=[{:.2},{:.2},{:.2}]->[{:.2},{:.2},{:.2}]\n  other: {} tris, bbox=[{:.2},{:.2},{:.2}]->[{:.2},{:.2},{:.2}]",
            self_tris, self_min[0], self_min[1], self_min[2], self_max[0], self_max[1], self_max[2],
            other_tris, other_min[0], other_min[1], other_min[2], other_max[0], other_max[1], other_max[2]
        ).into());

        let result = Solid {
            inner: self.inner.difference(&other.inner),
        };

        let result_tris_before_mesh = result.inner.num_triangles();
        let (result_min, result_max) = result.inner.bounding_box();
        web_sys::console::log_1(
            &format!(
                "[WASM] Difference result: {} tris, bbox=[{:.2},{:.2},{:.2}]->[{:.2},{:.2},{:.2}]",
                result_tris_before_mesh,
                result_min[0],
                result_min[1],
                result_min[2],
                result_max[0],
                result_max[1],
                result_max[2]
            )
            .into(),
        );

        let mesh = result.inner.to_mesh(32);
        let tris = mesh.indices.len() / 3;
        let verts = mesh.vertices.len() / 3;
        web_sys::console::log_1(
            &format!(
                "[WASM] Difference mesh (32 segs): {} triangles, {} vertices",
                tris, verts
            )
            .into(),
        );

        // Analyze the mesh to find any problematic triangles
        // Check for triangles with NEGATIVE x or y coordinates (the "ears")
        let mut negative_x_tris = Vec::new();
        let mut negative_y_tris = Vec::new();
        // Also check triangles on z=0 plane (bottom cap)
        let mut z0_cap_tris = Vec::new();

        for i in (0..mesh.indices.len()).step_by(3) {
            let i0 = mesh.indices[i] as usize * 3;
            let i1 = mesh.indices[i + 1] as usize * 3;
            let i2 = mesh.indices[i + 2] as usize * 3;
            let v0 = [
                mesh.vertices[i0],
                mesh.vertices[i0 + 1],
                mesh.vertices[i0 + 2],
            ];
            let v1 = [
                mesh.vertices[i1],
                mesh.vertices[i1 + 1],
                mesh.vertices[i1 + 2],
            ];
            let v2 = [
                mesh.vertices[i2],
                mesh.vertices[i2 + 1],
                mesh.vertices[i2 + 2],
            ];

            // Check for any vertex with negative x
            if v0[0] < -0.01 || v1[0] < -0.01 || v2[0] < -0.01 {
                negative_x_tris.push(format!(
                    "({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2})",
                    v0[0], v0[1], v0[2], v1[0], v1[1], v1[2], v2[0], v2[1], v2[2]
                ));
            }

            // Check for any vertex with negative y
            if v0[1] < -0.01 || v1[1] < -0.01 || v2[1] < -0.01 {
                negative_y_tris.push(format!(
                    "({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2})",
                    v0[0], v0[1], v0[2], v1[0], v1[1], v1[2], v2[0], v2[1], v2[2]
                ));
            }

            // Check triangles on z=0 plane (the bottom cap where ears appear)
            if v0[2].abs() < 0.1 && v1[2].abs() < 0.1 && v2[2].abs() < 0.1 {
                z0_cap_tris.push(format!(
                    "({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2})-({:.2},{:.2},{:.2})",
                    v0[0], v0[1], v0[2], v1[0], v1[1], v1[2], v2[0], v2[1], v2[2]
                ));
            }
        }

        web_sys::console::log_1(
            &format!(
                "[WASM] Triangles with NEGATIVE x: {}",
                negative_x_tris.len()
            )
            .into(),
        );
        for (i, tri) in negative_x_tris.iter().take(10).enumerate() {
            web_sys::console::log_1(&format!("[WASM]   neg_x tri {}: {}", i, tri).into());
        }

        web_sys::console::log_1(
            &format!(
                "[WASM] Triangles with NEGATIVE y: {}",
                negative_y_tris.len()
            )
            .into(),
        );
        for (i, tri) in negative_y_tris.iter().take(10).enumerate() {
            web_sys::console::log_1(&format!("[WASM]   neg_y tri {}: {}", i, tri).into());
        }

        web_sys::console::log_1(
            &format!("[WASM] Triangles on z=0 cap: {}", z0_cap_tris.len()).into(),
        );
        for (i, tri) in z0_cap_tris.iter().enumerate() {
            web_sys::console::log_1(&format!("[WASM]   z0_cap tri {}: {}", i, tri).into());
        }

        // Compute actual bounding box from mesh
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut min_z = f32::INFINITY;
        let mut max_z = f32::NEG_INFINITY;
        for i in (0..mesh.vertices.len()).step_by(3) {
            let x = mesh.vertices[i];
            let y = mesh.vertices[i + 1];
            let z = mesh.vertices[i + 2];
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            min_z = min_z.min(z);
            max_z = max_z.max(z);
        }
        web_sys::console::log_1(
            &format!(
                "[WASM] Mesh BBox: [{:.2},{:.2},{:.2}] -> [{:.2},{:.2},{:.2}]",
                min_x, min_y, min_z, max_x, max_y, max_z
            )
            .into(),
        );

        result
    }

    /// Boolean intersection (self ∩ other).
    #[wasm_bindgen(js_name = intersection)]
    pub fn intersection(&self, other: &Solid) -> Solid {
        Solid {
            inner: self.inner.intersection(&other.inner),
        }
    }

    // =========================================================================
    // Transforms
    // =========================================================================

    /// Translate the solid by (x, y, z).
    #[wasm_bindgen(js_name = translate)]
    pub fn translate(&self, x: f64, y: f64, z: f64) -> Solid {
        Solid {
            inner: self.inner.translate(x, y, z),
        }
    }

    /// Rotate the solid by angles in degrees around X, Y, Z axes.
    #[wasm_bindgen(js_name = rotate)]
    pub fn rotate(&self, x_deg: f64, y_deg: f64, z_deg: f64) -> Solid {
        Solid {
            inner: self.inner.rotate(x_deg, y_deg, z_deg),
        }
    }

    /// Scale the solid by (x, y, z).
    #[wasm_bindgen(js_name = scale)]
    pub fn scale(&self, x: f64, y: f64, z: f64) -> Solid {
        Solid {
            inner: self.inner.scale(x, y, z),
        }
    }

    // =========================================================================
    // Fillet & Chamfer
    // =========================================================================

    /// Chamfer all edges of the solid by the given distance.
    #[wasm_bindgen(js_name = chamfer)]
    pub fn chamfer(&self, distance: f64) -> Solid {
        Solid {
            inner: self.inner.chamfer(distance),
        }
    }

    /// Fillet all edges of the solid with the given radius.
    #[wasm_bindgen(js_name = fillet)]
    pub fn fillet(&self, radius: f64) -> Solid {
        Solid {
            inner: self.inner.fillet(radius),
        }
    }

    /// Shell (hollow) the solid by offsetting all faces inward.
    #[wasm_bindgen(js_name = shell)]
    pub fn shell(&self, thickness: f64) -> Solid {
        Solid {
            inner: self.inner.shell(thickness),
        }
    }

    // =========================================================================
    // Pattern operations
    // =========================================================================

    /// Create a linear pattern of the solid along a direction.
    ///
    /// # Arguments
    ///
    /// * `dir_x`, `dir_y`, `dir_z` - Direction vector
    /// * `count` - Number of copies (including original)
    /// * `spacing` - Distance between copies
    #[wasm_bindgen(js_name = linearPattern)]
    pub fn linear_pattern(
        &self,
        dir_x: f64,
        dir_y: f64,
        dir_z: f64,
        count: u32,
        spacing: f64,
    ) -> Solid {
        use vcad_kernel::vcad_kernel_math::Vec3;
        Solid {
            inner: self
                .inner
                .linear_pattern(Vec3::new(dir_x, dir_y, dir_z), count, spacing),
        }
    }

    /// Create a circular pattern of the solid around an axis.
    ///
    /// # Arguments
    ///
    /// * `axis_origin_x/y/z` - A point on the rotation axis
    /// * `axis_dir_x/y/z` - Direction of the rotation axis
    /// * `count` - Number of copies (including original)
    /// * `angle_deg` - Total angle span in degrees
    #[wasm_bindgen(js_name = circularPattern)]
    #[allow(clippy::too_many_arguments)]
    pub fn circular_pattern(
        &self,
        axis_origin_x: f64,
        axis_origin_y: f64,
        axis_origin_z: f64,
        axis_dir_x: f64,
        axis_dir_y: f64,
        axis_dir_z: f64,
        count: u32,
        angle_deg: f64,
    ) -> Solid {
        use vcad_kernel::vcad_kernel_math::{Point3, Vec3};
        Solid {
            inner: self.inner.circular_pattern(
                Point3::new(axis_origin_x, axis_origin_y, axis_origin_z),
                Vec3::new(axis_dir_x, axis_dir_y, axis_dir_z),
                count,
                angle_deg,
            ),
        }
    }

    // =========================================================================
    // Queries
    // =========================================================================

    /// Check if the solid is empty (has no geometry).
    #[wasm_bindgen(js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Get the triangle mesh representation.
    ///
    /// Returns a JS object with `positions` (Float32Array) and `indices` (Uint32Array).
    ///
    /// Runs the tessellator output through
    /// [`vcad_kernel_tessellate::render_bake`] so the emitted mesh carries
    /// angle-based creased vertex normals. Every downstream renderer —
    /// three.js today, wgpu / STL / GLB / ray tracer later — consumes this
    /// same attribute layout without recomputing anything.
    #[wasm_bindgen(js_name = getMesh)]
    pub fn get_mesh(&self, segments: Option<u32>) -> JsValue {
        let mut mesh = self.inner.to_mesh(segments.unwrap_or(32));
        vcad_kernel_tessellate::render_bake_default(&mut mesh);
        let num_verts = mesh.vertices.len() / 3;

        // Validate indices - check for out-of-bounds references
        let mut max_index = 0u32;
        let mut invalid_count = 0usize;
        for &idx in &mesh.indices {
            if idx as usize >= num_verts {
                invalid_count += 1;
            }
            if idx > max_index {
                max_index = idx;
            }
        }

        if invalid_count > 0 {
            web_sys::console::error_1(
                &format!(
                    "[WASM] getMesh: {} invalid indices (max index {} but only {} vertices)",
                    invalid_count, max_index, num_verts
                )
                .into(),
            );
        }

        let normals = if mesh.normals.len() == mesh.vertices.len() {
            Some(mesh.normals)
        } else {
            None
        };
        let face_kinds = if mesh.face_kinds.len() == mesh.indices.len() / 3 {
            Some(mesh.face_kinds)
        } else {
            None
        };
        let wasm_mesh = WasmMesh {
            positions: mesh.vertices,
            indices: mesh.indices,
            normals,
            face_kinds,
        };
        serde_wasm_bindgen::to_value(&wasm_mesh).unwrap_or(JsValue::NULL)
    }

    /// Compute the volume of the solid.
    #[wasm_bindgen(js_name = volume)]
    pub fn volume(&self) -> f64 {
        self.inner.volume()
    }

    /// Compute the surface area of the solid.
    #[wasm_bindgen(js_name = surfaceArea)]
    pub fn surface_area(&self) -> f64 {
        self.inner.surface_area()
    }

    /// Get the bounding box as [minX, minY, minZ, maxX, maxY, maxZ].
    #[wasm_bindgen(js_name = boundingBox)]
    pub fn bounding_box(&self) -> Vec<f64> {
        let (min, max) = self.inner.bounding_box();
        vec![min[0], min[1], min[2], max[0], max[1], max[2]]
    }

    /// Run DFM directly on this solid's BRep.
    ///
    /// Returns the report JSON; if the solid is mesh-only (e.g. after
    /// a boolean — see issue #186), the report has an empty `issues`
    /// array and a note in `rule_pack_name`.
    ///
    /// `root_node_id` (when > 0) attributes every face in the BRep to
    /// that IR node — the v1 coarse provenance heuristic. Pass 0 to
    /// skip provenance entirely; emitted issues will then carry
    /// `origin_op: null` and `dfm_apply_fix` will only be able to act
    /// on rules whose fix kind is `manual`.
    #[wasm_bindgen(js_name = runDfm)]
    pub fn run_dfm(
        &self,
        process: &str,
        rule_pack_toml: &str,
        root_node_id: u64,
    ) -> Result<String, JsError> {
        let p = vcad_kernel::vcad_kernel_dfm::Process::from_str(process)
            .ok_or_else(|| JsError::new(&format!("unknown process: {}", process)))?;
        let pack = if rule_pack_toml.trim().is_empty() {
            vcad_kernel::vcad_kernel_dfm::RulePack::default_for(p)
        } else {
            vcad_kernel::vcad_kernel_dfm::RulePack::from_toml(rule_pack_toml)
                .map_err(|e| JsError::new(&format!("rule pack parse: {}", e)))?
        };
        let Some(brep) = self.inner.as_brep() else {
            return Ok(format!(
                r#"{{"process":"{}","rule_pack_name":"(mesh-only solid; DFM skipped)","rule_pack_version":"1","issues":[],"cost_estimate":null}}"#,
                p.as_str()
            ));
        };
        let provenance = if root_node_id > 0 {
            Some(
                vcad_kernel::vcad_kernel_dfm::geom::provenance::ProvenanceMap::single_root(
                    brep,
                    root_node_id,
                ),
            )
        } else {
            None
        };
        let report = vcad_kernel::vcad_kernel_dfm::run_dfm(brep, provenance.as_ref(), p, &pack);
        serde_json::to_string(&report).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Get the center of mass as [x, y, z].
    #[wasm_bindgen(js_name = centerOfMass)]
    pub fn center_of_mass(&self) -> Vec<f64> {
        let com = self.inner.center_of_mass();
        vec![com[0], com[1], com[2]]
    }

    /// Get the number of triangles in the tessellated mesh.
    #[wasm_bindgen(js_name = numTriangles)]
    pub fn num_triangles(&self) -> usize {
        self.inner.num_triangles()
    }

    /// Return mesh boundary edges as a flat float array
    /// `[x0, y0, z0, x1, y1, z1, ...]` with each pair of 3-component
    /// positions defining one edge segment. Used by the viewport's
    /// "show boundary edges" overlay to surface tessellation holes.
    ///
    /// Closed, manifold meshes return an empty array; each entry means
    /// there's a hole in the mesh.
    #[wasm_bindgen(js_name = boundaryEdges)]
    pub fn boundary_edges(&self, segments: Option<u32>) -> Vec<f32> {
        let mesh = self.inner.to_mesh(segments.unwrap_or(32));
        let positions = mesh.boundary_edge_positions();
        let mut out = Vec::with_capacity(positions.len() * 6);
        for [a, b] in positions {
            out.extend_from_slice(&a);
            out.extend_from_slice(&b);
        }
        out
    }

    /// Generate a section view by cutting the solid with a plane.
    ///
    /// # Arguments
    /// * `plane_json` - JSON string with plane definition: `{"origin": [x,y,z], "normal": [x,y,z], "up": [x,y,z]}`
    /// * `hatch_json` - Optional JSON string with hatch pattern: `{"spacing": f64, "angle": f64}`
    /// * `segments` - Number of segments for tessellation (optional, default 32)
    ///
    /// # Returns
    /// A JS object containing the section view with curves, hatch lines, and bounds.
    #[wasm_bindgen(js_name = sectionView)]
    pub fn section_view(
        &self,
        plane_json: &str,
        hatch_json: Option<String>,
        segments: Option<u32>,
    ) -> JsValue {
        use vcad_kernel_drafting::{section_mesh, HatchPattern, SectionPlane};

        // Parse plane
        let plane: SectionPlane = match serde_json::from_str(plane_json) {
            Ok(p) => p,
            Err(_) => return JsValue::NULL,
        };

        // Parse optional hatch pattern
        let hatch: Option<HatchPattern> = hatch_json.and_then(|h| serde_json::from_str(&h).ok());

        // Get mesh
        let mesh = self.inner.to_mesh(segments.unwrap_or(32));

        // Generate section view
        let view = section_mesh(&mesh, &plane, hatch.as_ref());

        serde_wasm_bindgen::to_value(&view).unwrap_or(JsValue::NULL)
    }

    /// Generate a horizontal section view at a given Z height.
    ///
    /// Convenience method that creates a horizontal section plane.
    #[wasm_bindgen(js_name = horizontalSection)]
    pub fn horizontal_section(
        &self,
        z: f64,
        hatch_spacing: Option<f64>,
        hatch_angle: Option<f64>,
        segments: Option<u32>,
    ) -> JsValue {
        use vcad_kernel_drafting::{section_mesh, HatchPattern, SectionPlane};

        let plane = SectionPlane::horizontal(z);

        let hatch = hatch_spacing.map(|spacing| {
            HatchPattern::new(spacing, hatch_angle.unwrap_or(std::f64::consts::FRAC_PI_4))
        });

        let mesh = self.inner.to_mesh(segments.unwrap_or(32));
        let view = section_mesh(&mesh, &plane, hatch.as_ref());

        serde_wasm_bindgen::to_value(&view).unwrap_or(JsValue::NULL)
    }

    /// Project the solid to a 2D view for technical drawing.
    ///
    /// # Arguments
    /// * `view_direction` - View direction: "front", "back", "top", "bottom", "left", "right", or "isometric"
    /// * `segments` - Number of segments for tessellation (optional, default 32)
    ///
    /// # Returns
    /// A JS object containing the projected view with edges and bounds.
    #[wasm_bindgen(js_name = projectView)]
    pub fn project_view(&self, view_direction: &str, segments: Option<u32>) -> JsValue {
        use vcad_kernel_drafting::{project_mesh, ViewDirection};

        let mesh = self.inner.to_mesh(segments.unwrap_or(32));

        let view_dir = match view_direction.to_lowercase().as_str() {
            "front" => ViewDirection::Front,
            "back" => ViewDirection::Back,
            "top" => ViewDirection::Top,
            "bottom" => ViewDirection::Bottom,
            "left" => ViewDirection::Left,
            "right" => ViewDirection::Right,
            "isometric" => ViewDirection::ISOMETRIC_STANDARD,
            _ => ViewDirection::Front,
        };

        let view = project_mesh(&mesh, view_dir);
        serde_wasm_bindgen::to_value(&view).unwrap_or(JsValue::NULL)
    }

    /// Export the solid to STEP format.
    ///
    /// # Returns
    /// A byte buffer containing the STEP file data.
    ///
    /// # Errors
    /// Returns an error if the solid has no B-rep data (e.g., mesh-only after certain operations).
    #[wasm_bindgen(js_name = toStepBuffer)]
    pub fn to_step_buffer(&self) -> Result<Vec<u8>, JsError> {
        self.inner
            .to_step_buffer()
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Check if the solid can be exported to STEP format.
    ///
    /// Returns `true` if the solid has B-rep data available for STEP export.
    /// Returns `false` for mesh-only or empty solids.
    #[wasm_bindgen(js_name = canExportStep)]
    pub fn can_export_step(&self) -> bool {
        self.inner.can_export_step()
    }

    // =========================================================================
    // Text operations
    // =========================================================================

    /// Create a solid by extruding text as 2D profiles.
    ///
    /// Converts text to sketch profiles and extrudes them. Each character glyph
    /// becomes a separate profile, and holes (like in 'O') are subtracted.
    ///
    /// # Arguments
    ///
    /// * `text` - The text string to convert
    /// * `origin` - Origin point [x, y, z]
    /// * `x_dir` - X direction vector [x, y, z]
    /// * `y_dir` - Y direction vector [x, y, z]
    /// * `direction` - Extrusion direction [x, y, z] (magnitude = extrusion depth)
    /// * `height` - Text height in mm
    /// * `font` - Font name (currently only "sans-serif" supported)
    /// * `alignment` - Text alignment: "left", "center", or "right"
    /// * `letter_spacing` - Letter spacing multiplier (1.0 = normal)
    /// * `line_spacing` - Line spacing multiplier (1.0 = normal)
    #[wasm_bindgen(js_name = textExtrude)]
    #[allow(clippy::too_many_arguments)]
    pub fn text_extrude(
        text: &str,
        origin: Vec<f64>,
        x_dir: Vec<f64>,
        y_dir: Vec<f64>,
        direction: Vec<f64>,
        height: f64,
        font: Option<String>,
        alignment: Option<String>,
        letter_spacing: Option<f64>,
        line_spacing: Option<f64>,
    ) -> Result<Solid, JsError> {
        use vcad_kernel::vcad_kernel_text::{FontRegistry, TextAlignment};

        if origin.len() != 3 || x_dir.len() != 3 || y_dir.len() != 3 || direction.len() != 3 {
            return Err(JsError::new(
                "origin, x_dir, y_dir, and direction must have 3 components",
            ));
        }

        // Parse alignment
        let align = match alignment.as_deref() {
            Some("center") => TextAlignment::Center,
            Some("right") => TextAlignment::Right,
            _ => TextAlignment::Left,
        };

        // Get font (only builtin sans-serif for now)
        let font_ref = match font.as_deref() {
            Some("sans-serif") | None => FontRegistry::builtin_sans(),
            Some(name) => {
                return Err(JsError::new(&format!(
                    "Unknown font: {}. Use 'sans-serif' or omit for default.",
                    name
                )));
            }
        };

        let letter_sp = letter_spacing.unwrap_or(1.0);
        let line_sp = line_spacing.unwrap_or(1.0);

        // Convert text to profiles
        let profiles = vcad_kernel::vcad_kernel_text::text_to_profiles(
            text, font_ref, height, letter_sp, line_sp, align,
        );

        if profiles.is_empty() {
            return Ok(Solid {
                inner: vcad_kernel::Solid::empty(),
            });
        }

        // Separate profiles into outer contours and holes based on winding order
        let dir = Vec3::new(direction[0], direction[1], direction[2]);
        let origin_pt = Point3::new(origin[0], origin[1], origin[2]);
        let x_vec = Vec3::new(x_dir[0], x_dir[1], x_dir[2]);
        let y_vec = Vec3::new(y_dir[0], y_dir[1], y_dir[2]);

        // Determine holes by geometric containment
        // A profile is a hole if it's contained inside another profile
        let n = profiles.len();
        let mut is_hole = vec![false; n];

        for i in 0..n {
            for j in 0..n {
                if i != j && profiles[i].is_contained_in(&profiles[j]) {
                    is_hole[i] = true;
                    break;
                }
            }
        }

        let mut outer_profiles = Vec::new();
        let mut hole_profiles = Vec::new();

        for (i, profile) in profiles.into_iter().enumerate() {
            if is_hole[i] {
                hole_profiles.push(profile);
            } else {
                outer_profiles.push(profile);
            }
        }

        // Merge outer profile meshes (bypass boolean union)
        let mut all_vertices: Vec<f32> = Vec::new();
        let mut all_normals: Vec<f32> = Vec::new();
        let mut all_indices: Vec<u32> = Vec::new();

        for profile in &outer_profiles {
            let world_profile = profile.transform(origin_pt, x_vec, y_vec);

            if let Ok(solid) = vcad_kernel::Solid::extrude(world_profile, dir) {
                let mesh = solid.to_mesh(32);
                let vertex_offset = (all_vertices.len() / 3) as u32;
                all_vertices.extend_from_slice(&mesh.vertices);
                all_normals.extend_from_slice(&mesh.normals);
                for idx in mesh.indices {
                    all_indices.push(idx + vertex_offset);
                }
            }
        }

        // Create solid from merged outer meshes
        let mut result = if !all_vertices.is_empty() {
            let merged_mesh = vcad_kernel_tessellate::TriangleMesh {
                vertices: all_vertices,
                indices: all_indices,
                normals: all_normals,
                face_kinds: Vec::new(),
            };
            Some(vcad_kernel::Solid::from_mesh(merged_mesh))
        } else {
            None
        };

        // Subtract holes using boolean difference
        if let Some(solid) = result.take() {
            let mut current = solid;
            let hole_dir = dir * 1.1;
            let hole_offset = dir * -0.05;

            for profile in &hole_profiles {
                let offset_origin = origin_pt + hole_offset;
                let world_profile = profile.transform(offset_origin, x_vec, y_vec);

                if let Ok(hole_solid) = vcad_kernel::Solid::extrude(world_profile, hole_dir) {
                    current = current.difference(&hole_solid);
                }
            }
            result = Some(current);
        }

        Ok(Solid {
            inner: result.unwrap_or_else(vcad_kernel::Solid::empty),
        })
    }
}

// =========================================================================
// Standalone advanced operations (lazy-loaded module)
// =========================================================================

/// Fillet all edges of a solid with the given radius.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("advanced")]
#[wasm_bindgen]
pub fn op_fillet(solid: &Solid, radius: f64) -> Solid {
    solid.fillet(radius)
}

/// Chamfer all edges of a solid by the given distance.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("advanced")]
#[wasm_bindgen]
pub fn op_chamfer(solid: &Solid, distance: f64) -> Solid {
    solid.chamfer(distance)
}

/// Shell (hollow) a solid by offsetting all faces inward.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("advanced")]
#[wasm_bindgen]
pub fn op_shell(solid: &Solid, thickness: f64) -> Solid {
    solid.shell(thickness)
}

// =========================================================================
// Text utilities
// =========================================================================

/// Text bounds result containing width and height.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "generated/"))]
pub struct TextBoundsResult {
    /// Width of the rendered text in mm.
    pub width: f64,
    /// Height of the rendered text in mm.
    pub height: f64,
}

/// Get the bounding box of rendered text.
///
/// Returns the width and height of the text in mm without creating geometry.
/// Useful for layout calculations before extruding text.
///
/// # Arguments
///
/// * `text` - The text string to measure
/// * `height` - Text height in mm
/// * `font` - Font name (currently only "sans-serif" supported)
/// * `letter_spacing` - Letter spacing multiplier (1.0 = normal)
/// * `line_spacing` - Line spacing multiplier (1.0 = normal)
#[wasm_bindgen(js_name = textBounds)]
pub fn text_bounds(
    text: &str,
    height: f64,
    font: Option<String>,
    letter_spacing: Option<f64>,
    line_spacing: Option<f64>,
) -> Result<JsValue, JsError> {
    use vcad_kernel::vcad_kernel_text::FontRegistry;

    // Get font (only builtin sans-serif for now)
    let font_ref = match font.as_deref() {
        Some("sans-serif") | None => FontRegistry::builtin_sans(),
        Some(name) => {
            return Err(JsError::new(&format!(
                "Unknown font: {}. Use 'sans-serif' or omit for default.",
                name
            )));
        }
    };

    let letter_sp = letter_spacing.unwrap_or(1.0);
    let line_sp = line_spacing.unwrap_or(1.0);

    let (width, text_height) =
        vcad_kernel::vcad_kernel_text::text_bounds(text, font_ref, height, letter_sp, line_sp);

    let result = TextBoundsResult {
        width,
        height: text_height,
    };

    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
}

// =========================================================================
// Standalone sweep operations (lazy-loaded module)
// =========================================================================

/// Create a solid by revolving a 2D sketch profile around an axis.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("sweep")]
#[wasm_bindgen]
pub fn op_revolve(
    profile_json: String,
    axis_origin: Vec<f64>,
    axis_dir: Vec<f64>,
    angle_deg: f64,
) -> Result<Solid, JsError> {
    Solid::revolve(profile_json, axis_origin, axis_dir, angle_deg)
}

/// Create a solid by sweeping a profile along a line path.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("sweep")]
#[wasm_bindgen]
pub fn op_sweep_line(
    profile_json: String,
    start: Vec<f64>,
    end: Vec<f64>,
    twist_angle: Option<f64>,
    scale_start: Option<f64>,
    scale_end: Option<f64>,
    orientation: Option<f64>,
) -> Result<Solid, JsError> {
    Solid::sweep_line(
        profile_json,
        start,
        end,
        twist_angle,
        scale_start,
        scale_end,
        orientation,
    )
}

/// Create a solid by sweeping a profile along a helix path.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("sweep")]
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn op_sweep_helix(
    profile_json: String,
    radius: f64,
    pitch: f64,
    height: f64,
    turns: f64,
    twist_angle: Option<f64>,
    scale_start: Option<f64>,
    scale_end: Option<f64>,
    path_segments: Option<u32>,
    arc_segments: Option<u32>,
    orientation: Option<f64>,
) -> Result<Solid, JsError> {
    Solid::sweep_helix(
        profile_json,
        radius,
        pitch,
        height,
        turns,
        twist_angle,
        scale_start,
        scale_end,
        path_segments,
        arc_segments,
        orientation,
    )
}

/// Create a solid by lofting between multiple profiles.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("sweep")]
#[wasm_bindgen]
pub fn op_loft(profiles_json: String, closed: Option<bool>) -> Result<Solid, JsError> {
    Solid::loft(profiles_json, closed)
}

// =========================================================================
// Standalone pattern operations (lazy-loaded module)
// =========================================================================

/// Create a linear pattern of a solid along a direction.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("patterns")]
#[wasm_bindgen]
pub fn op_linear_pattern(
    solid: &Solid,
    dir_x: f64,
    dir_y: f64,
    dir_z: f64,
    count: u32,
    spacing: f64,
) -> Solid {
    solid.linear_pattern(dir_x, dir_y, dir_z, count, spacing)
}

/// Create a circular pattern of a solid around an axis.
///
/// This is a standalone wrapper for lazy loading via wasmosis.
#[module("patterns")]
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn op_circular_pattern(
    solid: &Solid,
    axis_origin_x: f64,
    axis_origin_y: f64,
    axis_origin_z: f64,
    axis_dir_x: f64,
    axis_dir_y: f64,
    axis_dir_z: f64,
    count: u32,
    angle_deg: f64,
) -> Solid {
    solid.circular_pattern(
        axis_origin_x,
        axis_origin_y,
        axis_origin_z,
        axis_dir_x,
        axis_dir_y,
        axis_dir_z,
        count,
        angle_deg,
    )
}

// =========================================================================
// Standalone drafting functions
// =========================================================================

/// Generate a section view from a triangle mesh.
///
/// # Arguments
/// * `mesh_js` - Mesh data as JS object with `positions` (Float32Array) and `indices` (Uint32Array)
/// * `plane_json` - JSON string with plane definition: `{"origin": [x,y,z], "normal": [x,y,z], "up": [x,y,z]}`
/// * `hatch_json` - Optional JSON string with hatch pattern: `{"spacing": f64, "angle": f64}`
///
/// # Returns
/// A JS object containing the section view with curves, hatch lines, and bounds.
#[module("drafting")]
#[wasm_bindgen(js_name = sectionMesh)]
pub fn section_mesh_wasm(
    mesh_js: JsValue,
    plane_json: &str,
    hatch_json: Option<String>,
) -> JsValue {
    use vcad_kernel_drafting::{section_mesh, HatchPattern, SectionPlane};
    use vcad_kernel_tessellate::TriangleMesh;

    // Parse mesh from JS
    let mesh_data: WasmMesh = match serde_wasm_bindgen::from_value(mesh_js) {
        Ok(m) => m,
        Err(_) => return JsValue::NULL,
    };

    let mesh = TriangleMesh {
        vertices: mesh_data.positions,
        indices: mesh_data.indices,
        normals: Vec::new(),
        face_kinds: Vec::new(),
    };

    // Parse plane
    let plane: SectionPlane = match serde_json::from_str(plane_json) {
        Ok(p) => p,
        Err(_) => return JsValue::NULL,
    };

    // Parse optional hatch pattern
    let hatch: Option<HatchPattern> = hatch_json.and_then(|h| serde_json::from_str(&h).ok());

    // Generate section view
    let view = section_mesh(&mesh, &plane, hatch.as_ref());

    serde_wasm_bindgen::to_value(&view).unwrap_or(JsValue::NULL)
}

/// Project a triangle mesh to a 2D view.
///
/// # Arguments
/// * `mesh_js` - Mesh data as JS object with `positions` (Float32Array) and `indices` (Uint32Array)
/// * `view_direction` - View direction: "front", "back", "top", "bottom", "left", "right", or "isometric"
///
/// # Returns
/// A JS object containing the projected view with edges and bounds.
#[module("drafting")]
#[wasm_bindgen(js_name = projectMesh)]
pub fn project_mesh_wasm(mesh_js: JsValue, view_direction: &str) -> JsValue {
    use vcad_kernel_drafting::{project_mesh, ViewDirection};
    use vcad_kernel_tessellate::TriangleMesh;

    // Parse mesh from JS
    let mesh_data: WasmMesh = match serde_wasm_bindgen::from_value(mesh_js) {
        Ok(m) => m,
        Err(_) => return JsValue::NULL,
    };

    let mesh = TriangleMesh {
        vertices: mesh_data.positions,
        indices: mesh_data.indices,
        normals: Vec::new(),
        face_kinds: Vec::new(),
    };

    let view_dir = match view_direction.to_lowercase().as_str() {
        "front" => ViewDirection::Front,
        "back" => ViewDirection::Back,
        "top" => ViewDirection::Top,
        "bottom" => ViewDirection::Bottom,
        "left" => ViewDirection::Left,
        "right" => ViewDirection::Right,
        "isometric" => ViewDirection::ISOMETRIC_STANDARD,
        _ => ViewDirection::Front,
    };

    let view = project_mesh(&mesh, view_dir);
    serde_wasm_bindgen::to_value(&view).unwrap_or(JsValue::NULL)
}

// =========================================================================
// Dimension annotation bindings
// =========================================================================

/// Annotation layer for dimension annotations.
///
/// This class provides methods for creating and rendering dimension annotations
/// on 2D projected views.
#[wasm_bindgen]
pub struct WasmAnnotationLayer {
    inner: vcad_kernel_drafting::AnnotationLayer,
}

#[wasm_bindgen]
impl WasmAnnotationLayer {
    /// Create a new empty annotation layer.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: vcad_kernel_drafting::AnnotationLayer::new(),
        }
    }

    /// Add a horizontal dimension between two points.
    ///
    /// # Arguments
    /// * `x1`, `y1` - First point coordinates
    /// * `x2`, `y2` - Second point coordinates
    /// * `offset` - Distance from points to dimension line (positive = above)
    #[wasm_bindgen(js_name = addHorizontalDimension)]
    pub fn add_horizontal_dimension(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, offset: f64) {
        use vcad_kernel_drafting::Point2D;
        self.inner
            .add_horizontal_dimension(Point2D::new(x1, y1), Point2D::new(x2, y2), offset);
    }

    /// Add a vertical dimension between two points.
    ///
    /// # Arguments
    /// * `x1`, `y1` - First point coordinates
    /// * `x2`, `y2` - Second point coordinates
    /// * `offset` - Distance from points to dimension line (positive = right)
    #[wasm_bindgen(js_name = addVerticalDimension)]
    pub fn add_vertical_dimension(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, offset: f64) {
        use vcad_kernel_drafting::Point2D;
        self.inner
            .add_vertical_dimension(Point2D::new(x1, y1), Point2D::new(x2, y2), offset);
    }

    /// Add an aligned dimension between two points.
    ///
    /// The dimension line is parallel to the line connecting the two points.
    ///
    /// # Arguments
    /// * `x1`, `y1` - First point coordinates
    /// * `x2`, `y2` - Second point coordinates
    /// * `offset` - Distance from points to dimension line
    #[wasm_bindgen(js_name = addAlignedDimension)]
    pub fn add_aligned_dimension(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, offset: f64) {
        use vcad_kernel_drafting::Point2D;
        self.inner
            .add_aligned_dimension(Point2D::new(x1, y1), Point2D::new(x2, y2), offset);
    }

    /// Add a diameter dimension for a circle.
    ///
    /// # Arguments
    /// * `cx`, `cy` - Center of the circle
    /// * `radius` - Radius of the circle
    /// * `leader_angle` - Angle in radians for the leader line direction
    #[wasm_bindgen(js_name = addDiameterDimension)]
    pub fn add_diameter_dimension(&mut self, cx: f64, cy: f64, radius: f64, leader_angle: f64) {
        use vcad_kernel_drafting::GeometryRef;
        self.inner.add_diameter_dimension(
            GeometryRef::Circle {
                center: vcad_kernel_drafting::Point2D::new(cx, cy),
                radius,
            },
            leader_angle,
        );
    }

    /// Add a radius dimension for a circle.
    ///
    /// # Arguments
    /// * `cx`, `cy` - Center of the circle
    /// * `radius` - Radius of the circle
    /// * `leader_angle` - Angle in radians for the leader line direction
    #[wasm_bindgen(js_name = addRadiusDimension)]
    pub fn add_radius_dimension(&mut self, cx: f64, cy: f64, radius: f64, leader_angle: f64) {
        use vcad_kernel_drafting::GeometryRef;
        self.inner.add_radius_dimension(
            GeometryRef::Circle {
                center: vcad_kernel_drafting::Point2D::new(cx, cy),
                radius,
            },
            leader_angle,
        );
    }

    /// Add an angular dimension between three points.
    ///
    /// The angle is measured at the vertex (middle point).
    ///
    /// # Arguments
    /// * `x1`, `y1` - First point on one leg
    /// * `vx`, `vy` - Vertex point (angle measured here)
    /// * `x2`, `y2` - Second point on other leg
    /// * `arc_radius` - Radius of the arc showing the angle
    #[wasm_bindgen(js_name = addAngleDimension)]
    #[allow(clippy::too_many_arguments)]
    pub fn add_angle_dimension(
        &mut self,
        x1: f64,
        y1: f64,
        vx: f64,
        vy: f64,
        x2: f64,
        y2: f64,
        arc_radius: f64,
    ) {
        use vcad_kernel_drafting::Point2D;
        self.inner.add_angle_dimension(
            Point2D::new(x1, y1),
            Point2D::new(vx, vy),
            Point2D::new(x2, y2),
            arc_radius,
        );
    }

    /// Get the number of annotations in the layer.
    #[wasm_bindgen(js_name = annotationCount)]
    pub fn annotation_count(&self) -> usize {
        self.inner.annotation_count()
    }

    /// Check if the layer has any annotations.
    #[wasm_bindgen(js_name = isEmpty)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clear all annotations from the layer.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Render all dimensions and return as JSON.
    ///
    /// Returns an array of rendered dimensions, each containing:
    /// - `lines`: Array of line segments [[x1, y1], [x2, y2]]
    /// - `arcs`: Array of arc definitions
    /// - `arrows`: Array of arrow definitions
    /// - `texts`: Array of text labels
    ///
    /// # Arguments
    /// * `view_json` - Optional JSON string of a ProjectedView for geometry resolution
    #[wasm_bindgen(js_name = renderAll)]
    pub fn render_all(&self, view_json: Option<String>) -> JsValue {
        use vcad_kernel_drafting::ProjectedView;

        // Parse optional view for geometry resolution
        let view: Option<ProjectedView> = view_json.and_then(|v| serde_json::from_str(&v).ok());

        let rendered = self.inner.render_all(view.as_ref());
        serde_wasm_bindgen::to_value(&rendered).unwrap_or(JsValue::NULL)
    }
}

impl Default for WasmAnnotationLayer {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// DXF Export
// =========================================================================

/// Export a projected view to DXF format.
///
/// Returns the DXF content as bytes.
///
/// # Arguments
/// * `view_json` - JSON string of a ProjectedView
///
/// # Returns
/// A byte array containing the DXF file content.
#[module("drafting")]
#[wasm_bindgen(js_name = exportProjectedViewToDxf)]
pub fn export_projected_view_to_dxf(view_json: &str) -> Result<Vec<u8>, JsError> {
    use std::io::Write;
    use vcad_kernel_drafting::{ProjectedView, Visibility};

    let view: ProjectedView =
        serde_json::from_str(view_json).map_err(|e| JsError::new(&e.to_string()))?;

    // Build DXF content
    let mut buffer = Vec::new();

    // Header
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "SECTION").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "HEADER").unwrap();
    writeln!(buffer, "9").unwrap();
    writeln!(buffer, "$ACADVER").unwrap();
    writeln!(buffer, "1").unwrap();
    writeln!(buffer, "AC1009").unwrap();
    writeln!(buffer, "9").unwrap();
    writeln!(buffer, "$INSUNITS").unwrap();
    writeln!(buffer, "70").unwrap();
    writeln!(buffer, "4").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "ENDSEC").unwrap();

    // Tables
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "SECTION").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "TABLES").unwrap();

    // Linetypes
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "TABLE").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "LTYPE").unwrap();
    writeln!(buffer, "70").unwrap();
    writeln!(buffer, "2").unwrap();

    // Continuous
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "LTYPE").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "CONTINUOUS").unwrap();
    writeln!(buffer, "70").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "3").unwrap();
    writeln!(buffer, "Solid line").unwrap();
    writeln!(buffer, "72").unwrap();
    writeln!(buffer, "65").unwrap();
    writeln!(buffer, "73").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "40").unwrap();
    writeln!(buffer, "0.0").unwrap();

    // Hidden
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "LTYPE").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "HIDDEN").unwrap();
    writeln!(buffer, "70").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "3").unwrap();
    writeln!(buffer, "Hidden line").unwrap();
    writeln!(buffer, "72").unwrap();
    writeln!(buffer, "65").unwrap();
    writeln!(buffer, "73").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "40").unwrap();
    writeln!(buffer, "9.525").unwrap();
    writeln!(buffer, "49").unwrap();
    writeln!(buffer, "6.35").unwrap();
    writeln!(buffer, "49").unwrap();
    writeln!(buffer, "-3.175").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "ENDTAB").unwrap();

    // Layers
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "TABLE").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "LAYER").unwrap();
    writeln!(buffer, "70").unwrap();
    writeln!(buffer, "2").unwrap();

    // VISIBLE layer
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "LAYER").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "VISIBLE").unwrap();
    writeln!(buffer, "70").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "62").unwrap();
    writeln!(buffer, "7").unwrap();
    writeln!(buffer, "6").unwrap();
    writeln!(buffer, "CONTINUOUS").unwrap();

    // HIDDEN layer
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "LAYER").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "HIDDEN").unwrap();
    writeln!(buffer, "70").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "62").unwrap();
    writeln!(buffer, "8").unwrap();
    writeln!(buffer, "6").unwrap();
    writeln!(buffer, "HIDDEN").unwrap();
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "ENDTAB").unwrap();

    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "ENDSEC").unwrap();

    // Entities
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "SECTION").unwrap();
    writeln!(buffer, "2").unwrap();
    writeln!(buffer, "ENTITIES").unwrap();

    for edge in &view.edges {
        let (layer, linetype) = match edge.visibility {
            Visibility::Visible => ("VISIBLE", "CONTINUOUS"),
            Visibility::Hidden => ("HIDDEN", "HIDDEN"),
        };

        writeln!(buffer, "0").unwrap();
        writeln!(buffer, "LINE").unwrap();
        writeln!(buffer, "8").unwrap();
        writeln!(buffer, "{}", layer).unwrap();
        writeln!(buffer, "6").unwrap();
        writeln!(buffer, "{}", linetype).unwrap();
        writeln!(buffer, "10").unwrap();
        writeln!(buffer, "{:.6}", edge.start.x).unwrap();
        writeln!(buffer, "20").unwrap();
        writeln!(buffer, "{:.6}", edge.start.y).unwrap();
        writeln!(buffer, "11").unwrap();
        writeln!(buffer, "{:.6}", edge.end.x).unwrap();
        writeln!(buffer, "21").unwrap();
        writeln!(buffer, "{:.6}", edge.end.y).unwrap();
    }

    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "ENDSEC").unwrap();

    // EOF
    writeln!(buffer, "0").unwrap();
    writeln!(buffer, "EOF").unwrap();

    Ok(buffer)
}

// =========================================================================
// Detail Views
// =========================================================================

/// Create a detail view from a projected view.
///
/// A detail view is a magnified region of a parent view, useful for showing
/// fine features that would be too small in the main view.
///
/// # Arguments
/// * `parent_json` - JSON string of the parent ProjectedView
/// * `center_x` - X coordinate of the region center
/// * `center_y` - Y coordinate of the region center
/// * `scale` - Magnification factor (e.g., 2.0 = 2x)
/// * `width` - Width of the region to capture
/// * `height` - Height of the region to capture
/// * `label` - Label for the detail view (e.g., "A")
///
/// # Returns
/// A JS object containing the detail view with edges and bounds.
#[module("drafting")]
#[wasm_bindgen(js_name = createDetailView)]
#[allow(clippy::too_many_arguments)]
pub fn create_detail_view(
    parent_json: &str,
    center_x: f64,
    center_y: f64,
    scale: f64,
    width: f64,
    height: f64,
    label: &str,
) -> Result<JsValue, JsError> {
    use vcad_kernel_drafting::{
        create_detail_view as create_detail, DetailViewParams, Point2D, ProjectedView,
    };

    let parent: ProjectedView =
        serde_json::from_str(parent_json).map_err(|e| JsError::new(&e.to_string()))?;

    let params = DetailViewParams::new(
        Point2D::new(center_x, center_y),
        scale,
        width,
        height,
        label,
    );

    let detail = create_detail(&parent, &params);

    serde_wasm_bindgen::to_value(&detail).map_err(|e| JsError::new(&e.to_string()))
}

// =========================================================================
// STEP Import
// =========================================================================

/// Import solids from STEP file bytes.
///
/// Returns a JS array of mesh data for each imported body.
/// Each mesh contains `positions` (Float32Array) and `indices` (Uint32Array).
///
/// # Arguments
/// * `data` - Raw STEP file contents as bytes
///
/// # Returns
/// A JS array of mesh objects for rendering the imported geometry.
#[module("step")]
#[wasm_bindgen(js_name = importStepBuffer)]
pub fn import_step_buffer(data: &[u8]) -> Result<JsValue, JsError> {
    let solids =
        vcad_kernel::Solid::from_step_buffer_all(data).map_err(|e| JsError::new(&e.to_string()))?;

    // Convert each solid to a mesh (use fewer segments for imported files)
    let meshes: Vec<WasmMesh> = solids
        .iter()
        .map(|s| {
            let mesh = s.to_mesh(16); // Lower resolution for faster rendering
            let normals = if mesh.normals.len() == mesh.vertices.len() {
                Some(mesh.normals)
            } else {
                None
            };
            WasmMesh {
                positions: mesh.vertices,
                indices: mesh.indices,
                normals,
                face_kinds: None,
            }
        })
        .collect();

    serde_wasm_bindgen::to_value(&meshes).map_err(|e| JsError::new(&e.to_string()))
}

/// Import a URDF (Unified Robot Description Format) file and return a
/// serialised vcad [`Document`].
///
/// Browsers cannot resolve `package://` URIs or relative mesh paths
/// against the user's filesystem, so any `<mesh>` reference in the URDF
/// falls back to a 1cm placeholder cube — the kinematic + inertial tree
/// is still imported correctly. Loading STL/DAE meshes in the browser
/// would require either uploading them alongside or vendoring them.
///
/// # Arguments
///
/// * `data` - Raw URDF XML bytes (UTF-8).
///
/// # Returns
///
/// JSON-encoded `Document` string. The web app parses it via
/// `Document.fromJson` (TS) or `vcad_ir::Document::from_json` (Rust).
#[module("urdf")]
#[wasm_bindgen(js_name = importUrdfBuffer)]
pub fn import_urdf_buffer(data: &[u8]) -> Result<String, JsError> {
    let xml = std::str::from_utf8(data)
        .map_err(|e| JsError::new(&format!("URDF must be valid UTF-8: {e}")))?;
    let doc = vcad_kernel_urdf::read_urdf_from_str(xml)
        .map_err(|e| JsError::new(&format!("URDF parse error: {e}")))?;
    doc.to_json()
        .map_err(|e| JsError::new(&format!("Document serialise error: {e}")))
}

// =========================================================================
// GPU-Accelerated Geometry Processing
// =========================================================================

/// GPU geometry processing result.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "generated/"))]
pub struct GpuGeometryResult {
    /// Vertex positions (flat array: x, y, z, ...).
    pub positions: Vec<f32>,
    /// Triangle indices.
    pub indices: Vec<u32>,
    /// Vertex normals (flat array: nx, ny, nz, ...).
    pub normals: Vec<f32>,
}

/// Initialize the GPU context for accelerated geometry processing.
///
/// Returns `true` if WebGPU is available and initialized, `false` otherwise.
/// This should be called once at application startup.
// Module inferred from #[cfg(feature = "gpu")]
#[cfg(feature = "gpu")]
#[wasm_bindgen(js_name = initGpu)]
pub async fn init_gpu() -> Result<bool, JsError> {
    match vcad_kernel_gpu::GpuContext::init().await {
        Ok(_) => {
            web_sys::console::log_1(&"[WASM] GPU context initialized successfully".into());
            Ok(true)
        }
        Err(e) => {
            web_sys::console::warn_1(&format!("[WASM] GPU init failed: {}", e).into());
            Ok(false)
        }
    }
}

/// Initialize the GPU context (stub when GPU feature is disabled).
#[cfg(not(feature = "gpu"))]
#[module("gpu")]
#[wasm_bindgen(js_name = initGpu)]
pub async fn init_gpu() -> Result<bool, JsError> {
    web_sys::console::log_1(&"[WASM] GPU feature not enabled".into());
    Ok(false)
}

/// Check if GPU processing is available.
#[module("gpu")]
#[wasm_bindgen(js_name = isGpuAvailable)]
pub fn is_gpu_available() -> bool {
    #[cfg(feature = "gpu")]
    {
        vcad_kernel_gpu::GpuContext::get().is_some()
    }
    #[cfg(not(feature = "gpu"))]
    {
        false
    }
}

/// Process geometry with GPU acceleration.
///
/// Computes creased normals and optionally generates LOD meshes.
///
/// # Arguments
/// * `positions` - Flat array of vertex positions (x, y, z, ...)
/// * `indices` - Triangle indices
/// * `crease_angle` - Angle in radians for creased normal computation
/// * `generate_lod` - If true, returns multiple LOD levels
///
/// # Returns
/// A JS array of geometry results. If `generate_lod` is true, returns
/// [full, 50%, 25%] detail levels. Otherwise returns a single mesh.
// Module inferred from #[cfg(feature = "gpu")]
#[cfg(feature = "gpu")]
#[wasm_bindgen(js_name = processGeometryGpu)]
pub async fn process_geometry_gpu(
    positions: Vec<f32>,
    indices: Vec<u32>,
    crease_angle: f32,
    generate_lod: bool,
) -> Result<JsValue, JsError> {
    use vcad_kernel_gpu::{compute_creased_normals, decimate_mesh};

    // Compute normals for full-resolution mesh
    let normals = compute_creased_normals(&positions, &indices, crease_angle)
        .await
        .map_err(|e| JsError::new(&format!("Normal computation failed: {}", e)))?;

    let mut results = vec![GpuGeometryResult {
        positions: positions.clone(),
        indices: indices.clone(),
        normals,
    }];

    if generate_lod {
        // Generate 50% LOD
        let lod1 = decimate_mesh(&positions, &indices, 0.5)
            .await
            .map_err(|e| JsError::new(&format!("Decimation (50%) failed: {}", e)))?;
        results.push(GpuGeometryResult {
            positions: lod1.positions,
            indices: lod1.indices,
            normals: lod1.normals,
        });

        // Generate 25% LOD
        let lod2 = decimate_mesh(&positions, &indices, 0.25)
            .await
            .map_err(|e| JsError::new(&format!("Decimation (25%) failed: {}", e)))?;
        results.push(GpuGeometryResult {
            positions: lod2.positions,
            indices: lod2.indices,
            normals: lod2.normals,
        });
    }

    serde_wasm_bindgen::to_value(&results).map_err(|e| JsError::new(&e.to_string()))
}

/// Process geometry (CPU fallback when GPU feature is disabled).
#[cfg(not(feature = "gpu"))]
#[module("gpu")]
#[wasm_bindgen(js_name = processGeometryGpu)]
pub async fn process_geometry_gpu(
    _positions: Vec<f32>,
    _indices: Vec<u32>,
    _crease_angle: f32,
    _generate_lod: bool,
) -> Result<JsValue, JsError> {
    Err(JsError::new("GPU feature not enabled"))
}

/// Compute creased normals using GPU acceleration.
///
/// # Arguments
/// * `positions` - Flat array of vertex positions (x, y, z, ...)
/// * `indices` - Triangle indices
/// * `crease_angle` - Angle in radians; faces meeting at sharper angles get hard edges
///
/// # Returns
/// Flat array of normals (nx, ny, nz, ...), same length as positions.
// Module inferred from #[cfg(feature = "gpu")]
#[cfg(feature = "gpu")]
#[wasm_bindgen(js_name = computeCreasedNormalsGpu)]
pub async fn compute_creased_normals_gpu(
    positions: Vec<f32>,
    indices: Vec<u32>,
    crease_angle: f32,
) -> Result<Vec<f32>, JsError> {
    vcad_kernel_gpu::compute_creased_normals(&positions, &indices, crease_angle)
        .await
        .map_err(|e| JsError::new(&format!("Normal computation failed: {}", e)))
}

/// Compute creased normals (CPU fallback when GPU feature is disabled).
#[cfg(not(feature = "gpu"))]
#[module("gpu")]
#[wasm_bindgen(js_name = computeCreasedNormalsGpu)]
pub async fn compute_creased_normals_gpu(
    _positions: Vec<f32>,
    _indices: Vec<u32>,
    _crease_angle: f32,
) -> Result<Vec<f32>, JsError> {
    Err(JsError::new("GPU feature not enabled"))
}

/// Decimate a mesh to reduce triangle count.
///
/// # Arguments
/// * `positions` - Flat array of vertex positions
/// * `indices` - Triangle indices
/// * `target_ratio` - Target ratio of triangles to keep (0.5 = 50%)
///
/// # Returns
/// A JS object with decimated positions, indices, and normals.
// Module inferred from #[cfg(feature = "gpu")]
#[cfg(feature = "gpu")]
#[wasm_bindgen(js_name = decimateMeshGpu)]
pub async fn decimate_mesh_gpu(
    positions: Vec<f32>,
    indices: Vec<u32>,
    target_ratio: f32,
) -> Result<JsValue, JsError> {
    let result = vcad_kernel_gpu::decimate_mesh(&positions, &indices, target_ratio)
        .await
        .map_err(|e| JsError::new(&format!("Decimation failed: {}", e)))?;

    let gpu_result = GpuGeometryResult {
        positions: result.positions,
        indices: result.indices,
        normals: result.normals,
    };

    serde_wasm_bindgen::to_value(&gpu_result).map_err(|e| JsError::new(&e.to_string()))
}

/// Decimate a mesh (CPU fallback when GPU feature is disabled).
#[cfg(not(feature = "gpu"))]
#[module("gpu")]
#[wasm_bindgen(js_name = decimateMeshGpu)]
pub async fn decimate_mesh_gpu(
    _positions: Vec<f32>,
    _indices: Vec<u32>,
    _target_ratio: f32,
) -> Result<JsValue, JsError> {
    Err(JsError::new("GPU feature not enabled"))
}

// =========================================================================
// GPU Ray Tracing (Direct BRep Rendering)
// =========================================================================

/// GPU-accelerated ray tracer for direct BRep rendering.
///
/// This ray tracer renders BRep surfaces directly without tessellation,
/// achieving pixel-perfect silhouettes at any zoom level.
#[cfg(feature = "raytrace")]
#[wasm_bindgen]
pub struct RayTracer {
    pipeline: vcad_kernel_raytrace::gpu::RayTracePipeline,
    scene: Option<vcad_kernel_raytrace::gpu::GpuScene>,
    /// Current frame index for progressive rendering (1-based).
    frame_index: u32,
    /// Accumulation buffer for progressive anti-aliasing.
    accum_buffer: Option<wgpu::Buffer>,
    /// AO buffer for progressive SSAO accumulation.
    ao_buffer: Option<wgpu::Buffer>,
    /// Last camera state for detecting camera changes.
    last_camera_hash: u64,
    /// Last render dimensions.
    last_width: u32,
    last_height: u32,
    /// Debug render mode: 0=normal, 1=show normals, 2=show face_id, 3=show n_dot_l,
    /// 4=orientation, 5=sample-count heatmap.
    debug_mode: u32,
    /// Enable edge detection overlay (master switch).
    enable_edges: bool,
    /// Edge depth threshold.
    edge_depth_threshold: f32,
    /// Edge normal threshold (degrees).
    edge_normal_threshold: f32,
    /// Theme: 0 = dark, 1 = light. Drives the visible background palette.
    theme: u32,
    /// SSAO world-space sample radius.
    ao_radius: f32,
    /// SSAO intensity (0 = disabled, 1 = default).
    ao_intensity: f32,
    /// SSAO depth bias.
    ao_bias: f32,
    /// SSAO sample count per frame.
    ao_sample_count: u32,
    /// Additional rays per edge pixel for adaptive refinement (0 = disabled).
    refine_sample_count: u32,
    // Per-type edge style
    enable_silhouette: bool,
    enable_crease: bool,
    enable_boundary: bool,
    silhouette_color: [f32; 4],
    crease_color: [f32; 4],
    boundary_color: [f32; 4],
    silhouette_width: f32,
    crease_width: f32,
    boundary_width: f32,
    edge_softness: f32,
}

#[cfg(feature = "raytrace")]
#[wasm_bindgen]
impl RayTracer {
    /// Create a new ray tracer.
    ///
    /// Requires WebGPU to be available and initialized.
    /// Call `initGpu()` before calling this method.
    #[wasm_bindgen(js_name = create)]
    pub fn create() -> Result<RayTracer, JsError> {
        // Ensure GPU context is initialized
        let ctx = vcad_kernel_gpu::GpuContext::get()
            .ok_or_else(|| JsError::new("GPU not initialized. Call initGpu() first."))?;

        let pipeline = vcad_kernel_raytrace::gpu::RayTracePipeline::new(ctx)
            .map_err(|e| JsError::new(&format!("Failed to create ray trace pipeline: {}", e)))?;

        web_sys::console::log_1(&"[WASM] RayTracer created".into());

        Ok(RayTracer {
            pipeline,
            scene: None,
            frame_index: 0,
            accum_buffer: None,
            ao_buffer: None,
            last_camera_hash: 0,
            last_width: 0,
            last_height: 0,
            debug_mode: 0,
            enable_edges: true,
            edge_depth_threshold: 0.1,
            edge_normal_threshold: 30.0,
            theme: 0,
            ao_radius: 0.3,
            ao_intensity: 1.0,
            ao_bias: 0.001,
            ao_sample_count: 16,
            refine_sample_count: 0,
            enable_silhouette: true,
            enable_crease: true,
            enable_boundary: true,
            silhouette_color: [0.08, 0.08, 0.10, 1.0],
            crease_color: [0.12, 0.12, 0.14, 1.0],
            boundary_color: [0.06, 0.06, 0.08, 1.0],
            silhouette_width: 1.0,
            crease_width: 0.75,
            boundary_width: 1.25,
            edge_softness: 1.5,
        })
    }

    /// Set the visible-background theme. 0 = dark (default), 1 = light.
    /// IBL panels and direct lighting stay constant across themes — this
    /// only swaps the atmospheric backdrop and ground tint.
    #[wasm_bindgen(js_name = setTheme)]
    pub fn set_theme(&mut self, theme: u32) {
        self.theme = theme;
        self.frame_index = 0;
        self.accum_buffer = None;
        self.ao_buffer = None;
    }

    /// Set the adaptive refinement sample count.
    ///
    /// Edge pixels on silhouettes receive additional stratified rays for sub-pixel
    /// anti-aliasing. Set to 0 to disable (default), or 4/9/16 for typical quality.
    /// Mode 5 in setDebugMode shows a heatmap of rays per pixel for tuning.
    #[wasm_bindgen(js_name = setRefineSamples)]
    pub fn set_refine_samples(&mut self, count: u32) {
        self.refine_sample_count = count;
        self.frame_index = 0;
        self.accum_buffer = None;
    }

    /// Get the current refinement sample count.
    #[wasm_bindgen(js_name = getRefineSamples)]
    pub fn get_refine_samples(&self) -> u32 {
        self.refine_sample_count
    }

    /// Reset the progressive accumulation (call when camera moves).
    #[wasm_bindgen(js_name = resetAccumulation)]
    pub fn reset_accumulation(&mut self) {
        self.frame_index = 0;
        self.accum_buffer = None;
        self.ao_buffer = None;
    }

    /// Get the current frame index for progressive rendering.
    #[wasm_bindgen(js_name = getFrameIndex)]
    pub fn get_frame_index(&self) -> u32 {
        self.frame_index
    }

    /// Set the debug render mode.
    ///
    /// # Arguments
    /// * `mode` - Debug mode: 0=normal, 1=normals as RGB, 2=face_id colors, 3=N·L grayscale, 4=orientation
    ///
    /// Call resetAccumulation() after changing mode to see immediate effect.
    #[wasm_bindgen(js_name = setDebugMode)]
    pub fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode;
        // Reset accumulation when debug mode changes
        self.frame_index = 0;
        self.accum_buffer = None;
        self.ao_buffer = None;
        web_sys::console::log_1(&format!("[WASM] Debug mode set to {}", mode).into());
    }

    /// Get the current debug render mode.
    #[wasm_bindgen(js_name = getDebugMode)]
    pub fn get_debug_mode(&self) -> u32 {
        self.debug_mode
    }

    /// Set edge detection settings.
    ///
    /// # Arguments
    /// * `enabled` - Whether to show edge detection overlay
    /// * `depth_threshold` - Depth discontinuity threshold (default: 0.1)
    /// * `normal_threshold` - Normal angle threshold in degrees (default: 30.0)
    #[wasm_bindgen(js_name = setEdgeDetection)]
    pub fn set_edge_detection(
        &mut self,
        enabled: bool,
        depth_threshold: f32,
        normal_threshold: f32,
    ) {
        self.enable_edges = enabled;
        self.edge_depth_threshold = depth_threshold;
        self.edge_normal_threshold = normal_threshold;
        // Reset accumulation when edge settings change
        self.frame_index = 0;
        self.accum_buffer = None;
        self.ao_buffer = None;
        web_sys::console::log_1(
            &format!(
                "[WASM] Edge detection: enabled={}, depth={:.2}, normal={:.1}°",
                enabled, depth_threshold, normal_threshold
            )
            .into(),
        );
    }

    /// Get whether edge detection is enabled.
    #[wasm_bindgen(js_name = getEdgeDetectionEnabled)]
    pub fn get_edge_detection_enabled(&self) -> bool {
        self.enable_edges
    }

    /// Set per-type edge style (colors, widths, softness, and individual toggles).
    ///
    /// Colors are RGBA in linear space (0–1). Width 1.0 = one pixel; softness controls
    /// the sub-pixel anti-aliasing transition width.
    #[wasm_bindgen(js_name = setEdgeStyle)]
    #[allow(clippy::too_many_arguments)]
    pub fn set_edge_style(
        &mut self,
        enable_silhouette: bool,
        enable_crease: bool,
        enable_boundary: bool,
        silhouette_r: f32,
        silhouette_g: f32,
        silhouette_b: f32,
        silhouette_a: f32,
        crease_r: f32,
        crease_g: f32,
        crease_b: f32,
        crease_a: f32,
        boundary_r: f32,
        boundary_g: f32,
        boundary_b: f32,
        boundary_a: f32,
        silhouette_width: f32,
        crease_width: f32,
        boundary_width: f32,
        edge_softness: f32,
    ) {
        self.enable_silhouette = enable_silhouette;
        self.enable_crease = enable_crease;
        self.enable_boundary = enable_boundary;
        self.silhouette_color = [silhouette_r, silhouette_g, silhouette_b, silhouette_a];
        self.crease_color = [crease_r, crease_g, crease_b, crease_a];
        self.boundary_color = [boundary_r, boundary_g, boundary_b, boundary_a];
        self.silhouette_width = silhouette_width;
        self.crease_width = crease_width;
        self.boundary_width = boundary_width;
        self.edge_softness = edge_softness;
        self.frame_index = 0;
        self.accum_buffer = None;
    }

    /// Clear all uploaded geometry. Call before re-uploading a fresh
    /// scene; subsequent `upload_solid` calls will accumulate into a
    /// new merged scene.
    #[wasm_bindgen(js_name = clearScene)]
    pub fn clear_scene(&mut self) {
        self.scene = None;
        self.frame_index = 0;
        self.accum_buffer = None;
        self.ao_buffer = None;
    }

    /// Set SSAO (screen-space ambient occlusion) parameters.
    ///
    /// # Arguments
    /// * `radius` - World-space hemisphere sample radius (default 0.3)
    /// * `intensity` - Occlusion strength: 0 = disabled, 1 = default (>1 stylized)
    /// * `bias` - Depth bias to prevent self-occlusion (default 0.001)
    /// * `sample_count` - Hemisphere samples per frame: 8, 16, or 32 (default 16)
    #[wasm_bindgen(js_name = setAO)]
    pub fn set_ao(&mut self, radius: f32, intensity: f32, bias: f32, sample_count: u32) {
        self.ao_radius = radius;
        self.ao_intensity = intensity;
        self.ao_bias = bias;
        self.ao_sample_count = sample_count.clamp(1, 64);
        self.frame_index = 0;
        self.accum_buffer = None;
        self.ao_buffer = None;
        web_sys::console::log_1(
            &format!(
                "[WASM] SSAO: radius={:.3}, intensity={:.2}, bias={:.4}, samples={}",
                radius, intensity, bias, sample_count
            )
            .into(),
        );
    }

    /// Upload a solid's BRep representation for ray tracing.
    ///
    /// First call after clearScene seeds the GPU scene. Subsequent calls
    /// merge into the existing scene — surfaces/faces/BVH from each new
    /// solid are unified under a fresh root, so multi-part scenes render
    /// in a single ray-trace pass.
    #[wasm_bindgen(js_name = uploadSolid)]
    pub fn upload_solid(&mut self, solid: &Solid) -> Result<(), JsError> {
        use vcad_kernel_raytrace::gpu::GpuScene;

        // Get the BRep from the solid
        let brep = solid
            .inner
            .brep()
            .ok_or_else(|| JsError::new("Solid has no BRep representation (mesh-only)"))?;

        // Build GPU scene from this BRep, then merge into the existing
        // scene (or seed if this is the first upload).
        let new_scene = GpuScene::from_brep(brep)
            .map_err(|e| JsError::new(&format!("Failed to build GPU scene: {}", e)))?;

        let scene = match self.scene.take() {
            Some(existing) => existing.merge(new_scene),
            None => new_scene,
        };

        let num_faces = scene.faces.len();
        let num_surfaces = scene.surfaces.len();
        let num_bvh_nodes = scene.bvh_nodes.len();

        // Debug: print face AABBs, inner loop data, and UV bounds from trim vertices
        for (i, face) in scene.faces.iter().enumerate() {
            // Compute UV bounds from trim vertices for this face
            let trim_start = face.trim_start as usize;
            let trim_count = face.trim_count as usize;
            let (uv_min_x, uv_max_x, uv_min_y, uv_max_y) = if trim_count > 0 {
                let mut min_x = f32::MAX;
                let mut max_x = f32::MIN;
                let mut min_y = f32::MAX;
                let mut max_y = f32::MIN;
                for j in 0..trim_count {
                    let uv = &scene.trim_verts[trim_start + j];
                    min_x = min_x.min(uv.x);
                    max_x = max_x.max(uv.x);
                    min_y = min_y.min(uv.y);
                    max_y = max_y.max(uv.y);
                }
                (min_x, max_x, min_y, max_y)
            } else {
                (0.0, 0.0, 0.0, 0.0)
            };

            web_sys::console::log_1(&format!(
                "[WASM] Face {}: surface={}, trim={}/{}@{}, UV_bounds=[{:.2},{:.2}]->[{:.2},{:.2}], inner={}/{}@{} (desc@{}), AABB=[{:.2},{:.2},{:.2}]->[{:.2},{:.2},{:.2}]",
                i, face.surface_idx,
                face.trim_count, face.trim_start, face.trim_start,
                uv_min_x, uv_min_y, uv_max_x, uv_max_y,
                face.inner_loop_count, face.inner_count, face.inner_start, face.inner_desc_start,
                face.aabb_min[0], face.aabb_min[1], face.aabb_min[2],
                face.aabb_max[0], face.aabb_max[1], face.aabb_max[2]
            ).into());
        }

        // Log inner_loop_descs buffer size
        web_sys::console::log_1(
            &format!(
                "[WASM] inner_loop_descs buffer: {} entries, trim_verts: {} entries",
                scene.inner_loop_descs.len(),
                scene.trim_verts.len()
            )
            .into(),
        );

        self.scene = Some(scene);

        web_sys::console::log_1(
            &format!(
                "[WASM] Uploaded solid: {} faces, {} surfaces, {} BVH nodes",
                num_faces, num_surfaces, num_bvh_nodes
            )
            .into(),
        );

        Ok(())
    }

    /// Set the material for all faces in the scene.
    ///
    /// # Arguments
    /// * `r`, `g`, `b` - RGB color components (0-1 range, linear)
    /// * `metallic` - Metallic factor (0 = dielectric, 1 = metal)
    /// * `roughness` - Roughness factor (0 = smooth/mirror, 1 = rough/diffuse)
    #[wasm_bindgen(js_name = setMaterial)]
    pub fn set_material(
        &mut self,
        r: f32,
        g: f32,
        b: f32,
        metallic: f32,
        roughness: f32,
    ) -> Result<(), JsError> {
        let scene = self
            .scene
            .as_mut()
            .ok_or_else(|| JsError::new("No solid uploaded. Call uploadSolid() first."))?;

        scene.set_material(r, g, b, metallic, roughness);

        // Reset accumulation since material changed
        self.frame_index = 0;
        self.accum_buffer = None;

        web_sys::console::log_1(
            &format!(
                "[WASM] Set material: rgb=({:.2}, {:.2}, {:.2}), metallic={:.2}, roughness={:.2}",
                r, g, b, metallic, roughness
            )
            .into(),
        );

        Ok(())
    }

    /// Render the scene to an RGBA image with progressive anti-aliasing.
    ///
    /// Each call accumulates another sample. Call `resetAccumulation()` when the
    /// camera moves to restart the accumulation.
    ///
    /// # Arguments
    /// * `camera` - Camera position [x, y, z]
    /// * `target` - Look-at target [x, y, z]
    /// * `up` - Up vector [x, y, z]
    /// * `width` - Image width in pixels
    /// * `height` - Image height in pixels
    /// * `fov` - Field of view in radians
    ///
    /// # Returns
    /// RGBA pixel data as a byte array (width * height * 4 bytes).
    ///
    /// # Note
    /// This function is async to support WASM's single-threaded environment.
    /// In JavaScript, it returns a Promise<Uint8Array>.
    pub async fn render(
        &mut self,
        camera: Vec<f64>,
        target: Vec<f64>,
        up: Vec<f64>,
        width: u32,
        height: u32,
        fov: f32,
    ) -> Result<Vec<u8>, JsError> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use vcad_kernel_raytrace::gpu::GpuCamera;

        if camera.len() != 3 || target.len() != 3 || up.len() != 3 {
            return Err(JsError::new(
                "camera, target, and up must each have 3 components",
            ));
        }

        let scene = self
            .scene
            .as_ref()
            .ok_or_else(|| JsError::new("No solid uploaded. Call uploadSolid() first."))?;

        // Compute camera hash to detect changes
        // Round to 2 decimal places (~1cm) to avoid floating-point precision issues
        // (e.g., 29.659999999 vs 29.660000001 should hash the same)
        // The React side handles settling detection to avoid spurious renders during damping
        let mut hasher = DefaultHasher::new();
        for v in &camera {
            ((v * 100.0).round() as i64).hash(&mut hasher);
        }
        for v in &target {
            ((v * 100.0).round() as i64).hash(&mut hasher);
        }
        ((fov * 100.0).round() as i32).hash(&mut hasher);
        let camera_hash = hasher.finish();

        // Reset accumulation if camera changed or dimensions changed
        if camera_hash != self.last_camera_hash
            || width != self.last_width
            || height != self.last_height
        {
            self.frame_index = 0;
            self.accum_buffer = None;
            self.ao_buffer = None;
            self.last_camera_hash = camera_hash;
            self.last_width = width;
            self.last_height = height;
        }

        // Increment frame index (capped at 256 for convergence)
        self.frame_index = (self.frame_index + 1).min(256);

        // Log progress occasionally
        if self.frame_index == 1 || self.frame_index.is_multiple_of(16) {
            web_sys::console::log_1(
                &format!(
                "[WASM] render() frame={} camera=[{:.2},{:.2},{:.2}] target=[{:.2},{:.2},{:.2}]",
                self.frame_index,
                camera[0], camera[1], camera[2],
                target[0], target[1], target[2],
            )
                .into(),
            );
        }

        let gpu_camera = GpuCamera::new(
            [camera[0] as f32, camera[1] as f32, camera[2] as f32],
            [target[0] as f32, target[1] as f32, target[2] as f32],
            [up[0] as f32, up[1] as f32, up[2] as f32],
            fov,
            width,
            height,
        );

        let ctx =
            vcad_kernel_gpu::GpuContext::get().ok_or_else(|| JsError::new("GPU context lost"))?;

        // Build per-type enable flags (bit 0 = silhouette, bit 1 = crease, bit 2 = boundary).
        let render_state = {
            let (s, c, b) = if self.enable_edges {
                (
                    self.enable_silhouette,
                    self.enable_crease,
                    self.enable_boundary,
                )
            } else {
                (false, false, false)
            };
            let mut rs = vcad_kernel_raytrace::gpu::GpuRenderState::new_styled(
                self.frame_index,
                self.debug_mode,
                s,
                c,
                b,
                self.edge_depth_threshold,
                self.edge_normal_threshold,
                self.theme,
                self.silhouette_color,
                self.crease_color,
                self.boundary_color,
                self.silhouette_width,
                self.crease_width,
                self.boundary_width,
                self.edge_softness,
            );
            rs.ao_radius = self.ao_radius;
            rs.ao_intensity = self.ao_intensity;
            rs.ao_bias = self.ao_bias;
            rs.ao_sample_count = self.ao_sample_count;
            rs.refine_sample_count = self.refine_sample_count;
            rs
        };

        let (pixels, new_accum, new_ao) = self
            .pipeline
            .render_with_render_state(
                ctx,
                scene,
                &gpu_camera,
                width,
                height,
                self.accum_buffer.take(),
                self.ao_buffer.take(),
                render_state,
            )
            .await
            .map_err(|e| JsError::new(&format!("Render failed: {}", e)))?;

        // Store accumulation and AO buffers for next frame
        self.accum_buffer = Some(new_accum);
        self.ao_buffer = Some(new_ao);

        Ok(pixels)
    }

    /// Pick a face at the given pixel coordinates.
    ///
    /// # Arguments
    /// * `camera`, `target`, `up` - Camera parameters
    /// * `width`, `height`, `fov` - View parameters
    /// * `pixel_x`, `pixel_y` - Pixel coordinates to pick
    ///
    /// # Returns
    /// Face index if a face was hit, or -1 if background was hit.
    #[allow(clippy::too_many_arguments)]
    pub fn pick(
        &self,
        camera: Vec<f64>,
        target: Vec<f64>,
        up: Vec<f64>,
        width: u32,
        height: u32,
        fov: f32,
        pixel_x: u32,
        pixel_y: u32,
    ) -> Result<i32, JsError> {
        use vcad_kernel_math::{Point3, Vec3};
        use vcad_kernel_raytrace::Ray;

        if camera.len() != 3 || target.len() != 3 || up.len() != 3 {
            return Err(JsError::new(
                "camera, target, and up must each have 3 components",
            ));
        }

        let scene = self
            .scene
            .as_ref()
            .ok_or_else(|| JsError::new("No solid uploaded. Call uploadSolid() first."))?;

        // Compute ray from camera through pixel
        let cam_pos = Point3::new(camera[0], camera[1], camera[2]);
        let tgt = Point3::new(target[0], target[1], target[2]);
        let up_vec = Vec3::new(up[0], up[1], up[2]);

        let forward = (tgt - cam_pos).normalize();
        let right = forward.cross(up_vec).normalize();
        let up_normalized = right.cross(forward);

        let aspect = width as f64 / height as f64;
        let fov_tan = (fov as f64 * 0.5).tan();

        // NDC for pixel center
        let ndc_x = (pixel_x as f64 + 0.5) / width as f64 * 2.0 - 1.0;
        let ndc_y = 1.0 - (pixel_y as f64 + 0.5) / height as f64 * 2.0;

        let ray_dir =
            (forward + right * ndc_x * fov_tan * aspect + up_normalized * ndc_y * fov_tan)
                .normalize();

        let ray = Ray::new(cam_pos, ray_dir);

        // Use CPU BVH for picking (more accurate than GPU render)
        // For now, return -1 as we don't have a CPU trace path in GpuScene
        // The full implementation would trace against the BRep directly

        // TODO: Implement CPU picking path
        // For now, this is a stub that always returns -1
        let _ = (ray, scene);
        Ok(-1)
    }

    /// Check if a solid can be ray traced.
    ///
    /// Returns true if the solid has a BRep representation.
    #[wasm_bindgen(js_name = canRaytrace)]
    pub fn can_raytrace(solid: &Solid) -> bool {
        solid.inner.brep().is_some()
    }

    /// Check if the ray tracer has a scene loaded.
    #[wasm_bindgen(js_name = hasScene)]
    pub fn has_scene(&self) -> bool {
        self.scene.is_some()
    }
}

/// Stub RayTracer when raytrace feature is not enabled.
#[cfg(not(feature = "raytrace"))]
#[wasm_bindgen]
pub struct RayTracer;

#[cfg(not(feature = "raytrace"))]
#[wasm_bindgen]
impl RayTracer {
    /// Returns an error when raytrace feature is not enabled.
    #[wasm_bindgen(js_name = create)]
    pub fn create() -> Result<RayTracer, JsError> {
        Err(JsError::new(
            "Ray tracing feature not enabled. Compile with --features raytrace",
        ))
    }
}

// =========================================================================
// VCode (for cad0 model integration)
// =========================================================================

/// Parse VCode text format into a vcad IR Document (JSON).
///
/// The VCode format is a token-efficient text representation designed
/// for ML model training and inference. See `vcad_ir::vcode` for format details.
///
/// # Arguments
/// * `vcode` - The VCode text to parse
///
/// # Returns
/// A JSON string representing the parsed vcad IR Document.
///
/// # Example
/// ```javascript
/// const ir = "C 50 30 5\nY 5 10\nT 1 25 15 0\nD 0 2";
/// const doc = parseVCode(ir);
/// console.log(doc); // JSON document
/// ```
#[module("ml")]
#[wasm_bindgen(js_name = parseVCode)]
pub fn parse_vcode(vcode: &str) -> Result<String, JsError> {
    let doc = vcad_ir::vcode::from_vcode(vcode)
        .map_err(|e| JsError::new(&format!("Parse error: {}", e)))?;

    doc.to_json()
        .map_err(|e| JsError::new(&format!("JSON serialization failed: {}", e)))
}

/// Convert a vcad IR Document (JSON) to VCode text format.
///
/// # Arguments
/// * `doc_json` - JSON string representing a vcad IR Document
///
/// # Returns
/// The VCode text representation.
///
/// # Example
/// ```javascript
/// const compact = toVCode(docJson);
/// console.log(compact); // "C 50 30 5\nY 5 10\n..."
/// ```
#[module("ml")]
#[wasm_bindgen(js_name = toVCode)]
pub fn to_vcode(doc_json: &str) -> Result<String, JsError> {
    let doc = vcad_ir::Document::from_json(doc_json)
        .map_err(|e| JsError::new(&format!("Invalid JSON: {}", e)))?;

    vcad_ir::vcode::to_vcode(&doc).map_err(|e| JsError::new(&format!("Conversion error: {}", e)))
}

/// Evaluate VCode and return a Solid for rendering.
///
/// This is a convenience function that parses VCode and evaluates
/// the geometry in a single step.
///
/// # Arguments
/// * `vcode` - The VCode text to evaluate
///
/// # Returns
/// A Solid object that can be rendered or queried.
#[module("ml")]
#[wasm_bindgen(js_name = evaluateVCode)]
pub fn evaluate_vcode(vcode: &str) -> Result<Solid, JsError> {
    let doc = vcad_ir::vcode::from_vcode(vcode)
        .map_err(|e| JsError::new(&format!("Parse error: {}", e)))?;

    // Find the root node
    let root_id = doc
        .roots
        .first()
        .ok_or_else(|| JsError::new("Document has no root nodes"))?
        .root;

    // Evaluate the DAG to produce a solid
    evaluate_node(&doc, root_id)
}

// =========================================================================
// Physics Simulation (Rapier-based gym environment)
// =========================================================================

/// Physics simulation environment for robotics and RL.
///
/// This provides a gym-style interface for simulating robot assemblies
/// with physics, joints, and collision detection.
#[cfg(feature = "physics")]
#[wasm_bindgen]
pub struct PhysicsSim {
    env: vcad_kernel_physics::RobotEnv,
}

#[cfg(feature = "physics")]
#[wasm_bindgen]
impl PhysicsSim {
    /// Create a new physics simulation from a vcad document JSON.
    ///
    /// # Arguments
    /// * `doc_json` - JSON string representing a vcad IR Document
    /// * `end_effector_ids` - Array of instance IDs to track as end effectors
    /// * `dt` - Simulation timestep in seconds (default: 1/240)
    /// * `substeps` - Number of physics substeps per step (default: 4)
    #[wasm_bindgen(constructor)]
    pub fn new(
        doc_json: &str,
        end_effector_ids: Vec<String>,
        dt: Option<f32>,
        substeps: Option<u32>,
    ) -> Result<PhysicsSim, JsError> {
        let doc = vcad_ir::Document::from_json(doc_json)
            .map_err(|e| JsError::new(&format!("Invalid document JSON: {}", e)))?;

        let env = vcad_kernel_physics::RobotEnv::new(doc, end_effector_ids, dt, substeps)
            .map_err(|e| JsError::new(&format!("Failed to create physics env: {}", e)))?;

        web_sys::console::log_1(
            &format!("[WASM] PhysicsSim created with {} joints", env.num_joints()).into(),
        );

        Ok(PhysicsSim { env })
    }

    /// Reset the environment to initial state.
    ///
    /// Returns the initial observation as JSON.
    #[wasm_bindgen(js_name = reset)]
    pub fn reset(&mut self) -> JsValue {
        let obs = self.env.reset();
        serde_wasm_bindgen::to_value(&obs).unwrap_or(JsValue::NULL)
    }

    /// Step the simulation with a torque action.
    ///
    /// # Arguments
    /// * `torques` - Array of torques/forces for each joint (Nm or N)
    ///
    /// # Returns
    /// Object with { observation, reward, done }
    #[wasm_bindgen(js_name = stepTorque)]
    pub fn step_torque(&mut self, torques: Vec<f64>) -> JsValue {
        let action = vcad_kernel_physics::Action::Torque(torques);
        let (obs, reward, done) = self.env.step(action);

        let result = serde_json::json!({
            "observation": obs,
            "reward": reward,
            "done": done
        });

        serde_wasm_bindgen::to_value(&result).unwrap_or(JsValue::NULL)
    }

    /// Step the simulation with position targets.
    ///
    /// # Arguments
    /// * `targets` - Array of position targets for each joint (degrees or mm)
    ///
    /// # Returns
    /// Object with { observation, reward, done }
    #[wasm_bindgen(js_name = stepPosition)]
    pub fn step_position(&mut self, targets: Vec<f64>) -> JsValue {
        let action = vcad_kernel_physics::Action::PositionTarget(targets);
        let (obs, reward, done) = self.env.step(action);

        let result = serde_json::json!({
            "observation": obs,
            "reward": reward,
            "done": done
        });

        serde_wasm_bindgen::to_value(&result).unwrap_or(JsValue::NULL)
    }

    /// Step the simulation with velocity targets.
    ///
    /// # Arguments
    /// * `targets` - Array of velocity targets for each joint (deg/s or mm/s)
    ///
    /// # Returns
    /// Object with { observation, reward, done }
    #[wasm_bindgen(js_name = stepVelocity)]
    pub fn step_velocity(&mut self, targets: Vec<f64>) -> JsValue {
        let action = vcad_kernel_physics::Action::VelocityTarget(targets);
        let (obs, reward, done) = self.env.step(action);

        let result = serde_json::json!({
            "observation": obs,
            "reward": reward,
            "done": done
        });

        serde_wasm_bindgen::to_value(&result).unwrap_or(JsValue::NULL)
    }

    /// Get current observation without stepping.
    ///
    /// Returns observation as JSON.
    #[wasm_bindgen(js_name = observe)]
    pub fn observe(&self) -> JsValue {
        let obs = self.env.observe();
        serde_wasm_bindgen::to_value(&obs).unwrap_or(JsValue::NULL)
    }

    /// Get the number of joints in the environment.
    #[wasm_bindgen(js_name = numJoints)]
    pub fn num_joints(&self) -> usize {
        self.env.num_joints()
    }

    /// Get the observation dimension.
    #[wasm_bindgen(js_name = observationDim)]
    pub fn observation_dim(&self) -> usize {
        self.env.observation_dim()
    }

    /// Get the action dimension.
    #[wasm_bindgen(js_name = actionDim)]
    pub fn action_dim(&self) -> usize {
        self.env.action_dim()
    }

    /// Set the maximum episode length.
    #[wasm_bindgen(js_name = setMaxSteps)]
    pub fn set_max_steps(&mut self, max_steps: u32) {
        self.env.set_max_steps(max_steps);
    }

    /// Set the random seed.
    #[wasm_bindgen(js_name = setSeed)]
    pub fn set_seed(&mut self, seed: u64) {
        self.env.seed(seed);
    }
}

/// Stub PhysicsSim when physics feature is not enabled.
#[cfg(not(feature = "physics"))]
#[wasm_bindgen]
pub struct PhysicsSim;

#[cfg(not(feature = "physics"))]
#[wasm_bindgen]
impl PhysicsSim {
    /// Returns an error when physics feature is not enabled.
    #[wasm_bindgen(constructor)]
    pub fn new(
        _doc_json: &str,
        _end_effector_ids: Vec<String>,
        _dt: Option<f32>,
        _substeps: Option<u32>,
    ) -> Result<PhysicsSim, JsError> {
        Err(JsError::new(
            "Physics feature not enabled. Compile with --features physics",
        ))
    }
}

/// Check if physics simulation is available.
#[module("physics")]
#[wasm_bindgen(js_name = isPhysicsAvailable)]
pub fn is_physics_available() -> bool {
    cfg!(feature = "physics")
}

// =========================================================================
// Internal evaluation helpers
// =========================================================================

/// Recursively evaluate a node in the IR DAG.
fn evaluate_node(doc: &vcad_ir::Document, node_id: vcad_ir::NodeId) -> Result<Solid, JsError> {
    let node = doc
        .nodes
        .get(&node_id)
        .ok_or_else(|| JsError::new(&format!("Node {} not found", node_id)))?;

    match &node.op {
        vcad_ir::CsgOp::Cube { size } => Ok(Solid::cube(size.x, size.y, size.z)),

        vcad_ir::CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => {
            let segs = if *segments == 0 {
                None
            } else {
                Some(*segments)
            };
            Ok(Solid::cylinder(*radius, *height, segs))
        }

        vcad_ir::CsgOp::Sphere { radius, segments } => {
            let segs = if *segments == 0 {
                None
            } else {
                Some(*segments)
            };
            Ok(Solid::sphere(*radius, segs))
        }

        vcad_ir::CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => {
            let segs = if *segments == 0 {
                None
            } else {
                Some(*segments)
            };
            Ok(Solid::cone(*radius_bottom, *radius_top, *height, segs))
        }

        vcad_ir::CsgOp::Empty => Ok(Solid::empty()),

        vcad_ir::CsgOp::Union { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            Ok(l.union(&r))
        }

        vcad_ir::CsgOp::Difference { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            Ok(l.difference(&r))
        }

        vcad_ir::CsgOp::Intersection { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            Ok(l.intersection(&r))
        }

        vcad_ir::CsgOp::Translate { child, offset } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.translate(offset.x, offset.y, offset.z))
        }

        vcad_ir::CsgOp::Rotate { child, angles } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.rotate(angles.x, angles.y, angles.z))
        }

        vcad_ir::CsgOp::Scale { child, factor } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.scale(factor.x, factor.y, factor.z))
        }

        vcad_ir::CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.linear_pattern(direction.x, direction.y, direction.z, *count, *spacing))
        }

        vcad_ir::CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.circular_pattern(
                axis_origin.x,
                axis_origin.y,
                axis_origin.z,
                axis_dir.x,
                axis_dir.y,
                axis_dir.z,
                *count,
                *angle_deg,
            ))
        }

        vcad_ir::CsgOp::Shell { child, thickness } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.shell(*thickness))
        }

        vcad_ir::CsgOp::Fillet { child, radius } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.fillet(*radius))
        }

        vcad_ir::CsgOp::Chamfer { child, distance } => {
            let c = evaluate_node(doc, *child)?;
            Ok(c.chamfer(*distance))
        }

        vcad_ir::CsgOp::Sketch2D { .. } => {
            // Sketch2D nodes cannot be evaluated directly - they must be used with Extrude/Revolve
            Err(JsError::new(
                "Sketch2D cannot be evaluated directly - use Extrude or Revolve",
            ))
        }

        vcad_ir::CsgOp::Extrude {
            sketch,
            direction,
            twist_angle,
            scale_end,
        } => {
            // Get the sketch node
            let sketch_node = doc
                .nodes
                .get(sketch)
                .ok_or_else(|| JsError::new(&format!("Sketch node {} not found", sketch)))?;

            match &sketch_node.op {
                vcad_ir::CsgOp::Sketch2D {
                    origin,
                    x_dir,
                    y_dir,
                    segments,
                } => {
                    let wasm_segments: Vec<WasmSketchSegment> = segments
                        .iter()
                        .map(|seg| match seg {
                            vcad_ir::SketchSegment2D::Line { start, end } => {
                                WasmSketchSegment::Line {
                                    start: [start.x, start.y],
                                    end: [end.x, end.y],
                                }
                            }
                            vcad_ir::SketchSegment2D::Arc {
                                start,
                                end,
                                center,
                                ccw,
                            } => WasmSketchSegment::Arc {
                                start: [start.x, start.y],
                                end: [end.x, end.y],
                                center: [center.x, center.y],
                                ccw: *ccw,
                            },
                        })
                        .collect();

                    let profile = WasmSketchProfile {
                        origin: [origin.x, origin.y, origin.z],
                        x_dir: [x_dir.x, x_dir.y, x_dir.z],
                        y_dir: [y_dir.x, y_dir.y, y_dir.z],
                        segments: wasm_segments,
                    };

                    let profile_json = serde_json::to_string(&profile).map_err(|e| {
                        JsError::new(&format!("Profile serialization failed: {}", e))
                    })?;

                    // Use extrudeWithOptions if twist or scale is specified
                    let has_twist = twist_angle.is_some_and(|t| t.abs() > 1e-12);
                    let has_scale = scale_end.is_some_and(|s| (s - 1.0).abs() > 1e-12);
                    if has_twist || has_scale {
                        Solid::extrude_with_options(
                            profile_json,
                            vec![direction.x, direction.y, direction.z],
                            twist_angle.unwrap_or(0.0),
                            scale_end.unwrap_or(1.0),
                        )
                    } else {
                        Solid::extrude(profile_json, vec![direction.x, direction.y, direction.z])
                    }
                }
                _ => Err(JsError::new("Extrude requires a Sketch2D node")),
            }
        }

        vcad_ir::CsgOp::Revolve {
            sketch,
            axis_origin,
            axis_dir,
            angle_deg,
        } => {
            let sketch_node = doc
                .nodes
                .get(sketch)
                .ok_or_else(|| JsError::new(&format!("Sketch node {} not found", sketch)))?;

            match &sketch_node.op {
                vcad_ir::CsgOp::Sketch2D {
                    origin,
                    x_dir,
                    y_dir,
                    segments,
                } => {
                    let wasm_segments: Vec<WasmSketchSegment> = segments
                        .iter()
                        .map(|seg| match seg {
                            vcad_ir::SketchSegment2D::Line { start, end } => {
                                WasmSketchSegment::Line {
                                    start: [start.x, start.y],
                                    end: [end.x, end.y],
                                }
                            }
                            vcad_ir::SketchSegment2D::Arc {
                                start,
                                end,
                                center,
                                ccw,
                            } => WasmSketchSegment::Arc {
                                start: [start.x, start.y],
                                end: [end.x, end.y],
                                center: [center.x, center.y],
                                ccw: *ccw,
                            },
                        })
                        .collect();

                    let profile = WasmSketchProfile {
                        origin: [origin.x, origin.y, origin.z],
                        x_dir: [x_dir.x, x_dir.y, x_dir.z],
                        y_dir: [y_dir.x, y_dir.y, y_dir.z],
                        segments: wasm_segments,
                    };

                    let profile_json = serde_json::to_string(&profile).map_err(|e| {
                        JsError::new(&format!("Profile serialization failed: {}", e))
                    })?;

                    Solid::revolve(
                        profile_json,
                        vec![axis_origin.x, axis_origin.y, axis_origin.z],
                        vec![axis_dir.x, axis_dir.y, axis_dir.z],
                        *angle_deg,
                    )
                }
                _ => Err(JsError::new("Revolve requires a Sketch2D node")),
            }
        }

        vcad_ir::CsgOp::StepImport { .. } => Err(JsError::new(
            "STEP import not supported in VCode evaluation",
        )),

        vcad_ir::CsgOp::MeshImport { .. } => Err(JsError::new(
            "Mesh import not supported in VCode evaluation",
        )),

        vcad_ir::CsgOp::Text2D { .. } => {
            // Text2D doesn't produce geometry by itself - it needs to be extruded.
            // This case handles direct evaluation of Text2D nodes (should be rare).
            // Typically Text2D nodes are used as children of Extrude operations.

            // For now, return an error - the proper way to use Text2D is:
            // 1. Create a Text2D node
            // 2. Use it as the sketch input to an Extrude operation
            // The TypeScript evaluate.ts handles converting Text2D inside Extrude
            Err(JsError::new(
                "Text2D cannot be evaluated directly - use Extrude to convert to solid",
            ))
        }

        vcad_ir::CsgOp::Sweep { .. } => Err(JsError::new(
            "Sweep not supported in VCode evaluation - use evaluateDocument",
        )),

        vcad_ir::CsgOp::Loft { .. } => Err(JsError::new(
            "Loft not supported in VCode evaluation - use evaluateDocument",
        )),

        vcad_ir::CsgOp::ImportedMesh { .. } => Err(JsError::new(
            "ImportedMesh not supported in VCode evaluation - use evaluateDocument",
        )),

        vcad_ir::CsgOp::PcbBoard { .. } => Err(JsError::new(
            "PcbBoard not supported in VCode evaluation - use evaluateDocument",
        )),

        vcad_ir::CsgOp::EmbroideryPattern { .. } => Err(JsError::new(
            "EmbroideryPattern not supported in VCode evaluation - use evaluateDocument",
        )),

        vcad_ir::CsgOp::PartInstance { .. } => Err(JsError::new(
            "PartInstance must be expanded by the engine before VCode evaluation",
        )),
    }
}

// =========================================================================
// Slicer module (feature-gated)
// =========================================================================

#[cfg(feature = "slicer")]
mod slicer_wasm {
    use super::*;
    use vcad_kernel_tessellate::TriangleMesh;
    use vcad_slicer::{InfillPattern, SliceSettings};
    use vcad_slicer_gcode::{GcodeSettings, PrinterProfile};

    /// Slicer settings for WASM.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[wasm_bindgen]
    pub struct SlicerSettings {
        /// Layer height (mm).
        pub layer_height: f64,
        /// First layer height (mm).
        pub first_layer_height: f64,
        /// Nozzle diameter (mm).
        pub nozzle_diameter: f64,
        /// Line width (mm).
        pub line_width: f64,
        /// Wall count.
        pub wall_count: u32,
        /// Infill density (0-1).
        pub infill_density: f64,
        /// Infill pattern (0=Grid, 1=Lines, 2=Triangles, 3=Honeycomb, 4=Gyroid).
        pub infill_pattern: u32,
        /// Enable support.
        pub support_enabled: bool,
        /// Support angle threshold.
        pub support_angle: f64,
    }

    #[wasm_bindgen]
    impl SlicerSettings {
        /// Create default settings.
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                layer_height: 0.2,
                first_layer_height: 0.25,
                nozzle_diameter: 0.4,
                line_width: 0.45,
                wall_count: 3,
                infill_density: 0.15,
                infill_pattern: 0,
                support_enabled: false,
                support_angle: 45.0,
            }
        }

        /// Create from JSON.
        #[wasm_bindgen(js_name = fromJson)]
        pub fn from_json(json: &str) -> Result<SlicerSettings, JsError> {
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))
        }
    }

    impl Default for SlicerSettings {
        fn default() -> Self {
            Self::new()
        }
    }

    impl From<SlicerSettings> for SliceSettings {
        fn from(settings: SlicerSettings) -> Self {
            Self {
                layer_height: settings.layer_height,
                first_layer_height: settings.first_layer_height,
                nozzle_diameter: settings.nozzle_diameter,
                line_width: settings.line_width,
                wall_count: settings.wall_count,
                infill_density: settings.infill_density,
                infill_pattern: match settings.infill_pattern {
                    0 => InfillPattern::Grid,
                    1 => InfillPattern::Lines,
                    2 => InfillPattern::Triangles,
                    3 => InfillPattern::Honeycomb,
                    _ => InfillPattern::Gyroid,
                },
                support_enabled: settings.support_enabled,
                support_angle: settings.support_angle,
            }
        }
    }

    /// Slice result for WASM.
    #[wasm_bindgen]
    pub struct SliceResult {
        inner: vcad_slicer::SliceResult,
    }

    #[wasm_bindgen]
    impl SliceResult {
        /// Get number of layers.
        #[wasm_bindgen(getter, js_name = layerCount)]
        pub fn layer_count(&self) -> usize {
            self.inner.stats.layer_count
        }

        /// Get estimated print time in seconds.
        #[wasm_bindgen(getter, js_name = printTimeSeconds)]
        pub fn print_time_seconds(&self) -> f64 {
            self.inner.stats.print_time_seconds
        }

        /// Get filament usage in mm.
        #[wasm_bindgen(getter, js_name = filamentMm)]
        pub fn filament_mm(&self) -> f64 {
            self.inner.stats.filament_mm
        }

        /// Get filament weight in grams.
        #[wasm_bindgen(getter, js_name = filamentGrams)]
        pub fn filament_grams(&self) -> f64 {
            self.inner.stats.filament_grams
        }

        /// Get stats as JSON.
        #[wasm_bindgen(js_name = statsJson)]
        pub fn stats_json(&self) -> Result<String, JsError> {
            serde_json::to_string(&self.inner.stats).map_err(|e| JsError::new(&e.to_string()))
        }

        /// Get layer data for preview.
        #[wasm_bindgen(js_name = getLayerPreview)]
        pub fn get_layer_preview(&self, layer_index: usize) -> Result<JsValue, JsError> {
            if layer_index >= self.inner.layers.len() {
                return Err(JsError::new("layer index out of bounds"));
            }

            let layer = &self.inner.layers[layer_index];

            #[derive(Serialize)]
            struct LayerPreview {
                z: f64,
                index: usize,
                outer_perimeters: Vec<Vec<[f64; 2]>>,
                inner_perimeters: Vec<Vec<[f64; 2]>>,
                infill: Vec<Vec<[f64; 2]>>,
            }

            let preview = LayerPreview {
                z: layer.z,
                index: layer.index,
                outer_perimeters: layer
                    .outer_perimeters
                    .iter()
                    .map(|p| p.points.iter().map(|pt| [pt.x, pt.y]).collect())
                    .collect(),
                inner_perimeters: layer
                    .inner_perimeters
                    .iter()
                    .map(|p| p.points.iter().map(|pt| [pt.x, pt.y]).collect())
                    .collect(),
                infill: layer
                    .infill
                    .iter()
                    .map(|p| p.points.iter().map(|pt| [pt.x, pt.y]).collect())
                    .collect(),
            };

            serde_wasm_bindgen::to_value(&preview).map_err(|e| JsError::new(&e.to_string()))
        }
    }

    /// Slice a mesh from vertices and indices.
    #[wasm_bindgen(js_name = sliceMesh)]
    pub fn slice_mesh(
        vertices: &[f32],
        indices: &[u32],
        settings: &SlicerSettings,
    ) -> Result<SliceResult, JsError> {
        let mesh = TriangleMesh {
            vertices: vertices.to_vec(),
            indices: indices.to_vec(),
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };

        let slice_settings: SliceSettings = settings.clone().into();
        let result =
            vcad_slicer::slice(&mesh, &slice_settings).map_err(|e| JsError::new(&e.to_string()))?;

        Ok(SliceResult { inner: result })
    }

    /// Slice a mesh and report progress to a JS callback.
    ///
    /// The callback is invoked synchronously during the WASM call as
    /// `cb(stageLabel: string, current: number, total: number)`. Inside a
    /// dedicated worker, the callback can safely `postMessage` to the main
    /// thread — the worker thread is the one running the WASM, not the
    /// main thread.
    #[wasm_bindgen(js_name = sliceMeshWithProgress)]
    pub fn slice_mesh_with_progress(
        vertices: &[f32],
        indices: &[u32],
        settings: &SlicerSettings,
        progress_cb: &js_sys::Function,
    ) -> Result<SliceResult, JsError> {
        let mesh = TriangleMesh {
            vertices: vertices.to_vec(),
            indices: indices.to_vec(),
            normals: Vec::new(),
            face_kinds: Vec::new(),
        };

        let slice_settings: SliceSettings = settings.clone().into();

        let cb = progress_cb.clone();
        let progress = move |stage: vcad_slicer::SliceStage, current: usize, total: usize| {
            let _ = cb.call3(
                &JsValue::NULL,
                &JsValue::from_str(stage.label()),
                &JsValue::from_f64(current as f64),
                &JsValue::from_f64(total as f64),
            );
        };
        let result = vcad_slicer::slice_with_progress(&mesh, &slice_settings, Some(&progress))
            .map_err(|e| JsError::new(&e.to_string()))?;

        Ok(SliceResult { inner: result })
    }

    /// Slice a solid.
    #[wasm_bindgen(js_name = sliceSolid)]
    pub fn slice_solid(
        solid: &Solid,
        settings: &SlicerSettings,
        segments: Option<u32>,
    ) -> Result<SliceResult, JsError> {
        let mesh = solid.inner.to_mesh(segments.unwrap_or(32));
        let slice_settings: SliceSettings = settings.clone().into();
        let result =
            vcad_slicer::slice(&mesh, &slice_settings).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(SliceResult { inner: result })
    }

    /// Generate G-code from slice result.
    #[wasm_bindgen(js_name = generateGcode)]
    pub fn generate_gcode(
        result: &SliceResult,
        printer_profile: &str,
        print_temp: u32,
        bed_temp: u32,
    ) -> Result<String, JsError> {
        let profile = match printer_profile {
            "bambu_x1c" => PrinterProfile::bambu_x1c(),
            "bambu_p1s" => PrinterProfile::bambu_p1s(),
            "bambu_a1" => PrinterProfile::bambu_a1(),
            "bambu_a1_mini" | "bambu_lab_a1_mini" => PrinterProfile::bambu_a1_mini(),
            "ender3" => PrinterProfile::ender3(),
            "prusa_mk4" => PrinterProfile::prusa_mk4(),
            "voron_24" => PrinterProfile::voron_24(),
            _ => PrinterProfile::generic(),
        };

        let settings = GcodeSettings {
            printer: profile,
            print_temp,
            bed_temp,
            ..Default::default()
        };

        Ok(vcad_slicer_gcode::generate_gcode(&result.inner, settings))
    }

    /// Get available printer profiles.
    #[wasm_bindgen(js_name = getSlicerPrinterProfiles)]
    pub fn get_slicer_printer_profiles() -> Result<JsValue, JsError> {
        #[derive(Serialize)]
        struct ProfileInfo {
            id: String,
            name: String,
            bed_x: f64,
            bed_y: f64,
            bed_z: f64,
            nozzle_diameter: f64,
        }

        fn profile_id(name: &str) -> String {
            name.to_lowercase()
                .replace(' ', "_")
                .replace(['(', ')'], "")
        }

        let profiles: Vec<ProfileInfo> = PrinterProfile::all_profiles()
            .into_iter()
            .map(|p| ProfileInfo {
                id: profile_id(&p.name),
                name: p.name,
                bed_x: p.bed_x,
                bed_y: p.bed_y,
                bed_z: p.bed_z,
                nozzle_diameter: p.nozzle_diameter,
            })
            .collect();

        serde_wasm_bindgen::to_value(&profiles).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Check if slicer is available.
    #[wasm_bindgen(js_name = isSlicerAvailable)]
    pub fn is_slicer_available() -> bool {
        true
    }

    /// Analyze a solid for 3D printing characteristics.
    ///
    /// Returns JSON with wall thicknesses, overhang angles, hole sizes, etc.
    /// Only works on solids with BRep data (primitives, not boolean results).
    #[wasm_bindgen(js_name = analyzeForPrinting)]
    pub fn analyze_for_printing(solid: &Solid) -> Result<JsValue, JsError> {
        let brep = solid
            .inner
            .brep()
            .ok_or_else(|| JsError::new("Solid has no BRep data (mesh-only)"))?;

        let volume = solid.inner.volume();
        let surface_area = solid.inner.surface_area();

        let analysis = vcad_slicer::analyze::analyze_for_printing(brep, volume, surface_area);
        serde_wasm_bindgen::to_value(&analysis).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Recommend smart print settings from analysis results.
    ///
    /// Takes a PrintAnalysis JSON and printer profile name,
    /// returns recommended SliceSettings + explanations.
    #[wasm_bindgen(js_name = recommendPrintSettings)]
    pub fn recommend_print_settings(
        analysis_json: &str,
        printer_profile: &str,
    ) -> Result<JsValue, JsError> {
        let analysis: vcad_slicer::analyze::PrintAnalysis =
            serde_json::from_str(analysis_json).map_err(|e| JsError::new(&e.to_string()))?;

        let profile = match printer_profile {
            "bambu_x1c" => PrinterProfile::bambu_x1c(),
            "bambu_p1s" => PrinterProfile::bambu_p1s(),
            "bambu_a1" => PrinterProfile::bambu_a1(),
            "bambu_a1_mini" | "bambu_lab_a1_mini" => PrinterProfile::bambu_a1_mini(),
            "ender3" => PrinterProfile::ender3(),
            "prusa_mk4" => PrinterProfile::prusa_mk4(),
            "voron_24" => PrinterProfile::voron_24(),
            _ => PrinterProfile::generic(),
        };

        let params = vcad_slicer::smart_defaults::PrinterParams {
            nozzle_diameter: profile.nozzle_diameter,
            bed_x: profile.bed_x,
            bed_y: profile.bed_y,
            bed_z: profile.bed_z,
        };

        let defaults = vcad_slicer::smart_defaults::recommend_settings(&analysis, &params);
        serde_wasm_bindgen::to_value(&defaults).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Check a solid for DFM (Design for Manufacturing) printability issues.
    ///
    /// Returns warnings with face indices for viewport highlighting.
    #[wasm_bindgen(js_name = checkPrintability)]
    pub fn check_printability(solid: &Solid, printer_profile: &str) -> Result<JsValue, JsError> {
        let brep = solid
            .inner
            .brep()
            .ok_or_else(|| JsError::new("Solid has no BRep data (mesh-only)"))?;

        let profile = match printer_profile {
            "bambu_x1c" => PrinterProfile::bambu_x1c(),
            "bambu_p1s" => PrinterProfile::bambu_p1s(),
            "bambu_a1" => PrinterProfile::bambu_a1(),
            "bambu_a1_mini" | "bambu_lab_a1_mini" => PrinterProfile::bambu_a1_mini(),
            "ender3" => PrinterProfile::ender3(),
            "prusa_mk4" => PrinterProfile::prusa_mk4(),
            "voron_24" => PrinterProfile::voron_24(),
            _ => PrinterProfile::generic(),
        };

        let params = vcad_slicer::smart_defaults::PrinterParams {
            nozzle_diameter: profile.nozzle_diameter,
            bed_x: profile.bed_x,
            bed_y: profile.bed_y,
            bed_z: profile.bed_z,
        };

        let result = vcad_slicer::dfm::check_printability(brep, &params);
        serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Estimate print cost from volume (instant, pre-slice).
    #[wasm_bindgen(js_name = estimatePrintCost)]
    pub fn estimate_print_cost(
        volume_mm3: f64,
        infill_density: f64,
        wall_count: u32,
        line_width: f64,
        material_name: &str,
    ) -> Result<JsValue, JsError> {
        let material = match material_name {
            "PETG" | "petg" => vcad_slicer::cost::Material::petg(),
            "ABS" | "abs" => vcad_slicer::cost::Material::abs(),
            "TPU" | "tpu" => vcad_slicer::cost::Material::tpu(),
            _ => vcad_slicer::cost::Material::pla(),
        };

        let estimate = vcad_slicer::cost::estimate_cost_from_volume(
            volume_mm3,
            infill_density,
            wall_count,
            line_width,
            &material,
        );
        serde_wasm_bindgen::to_value(&estimate).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Generate a 3MF file from mesh data.
    ///
    /// Returns the 3MF file as a byte array suitable for download or upload to a printer.
    #[wasm_bindgen(js_name = generate3mf)]
    pub fn generate_3mf(
        name: &str,
        vertices: &[f32],
        indices: &[u32],
        settings_json: &str,
    ) -> Result<Vec<u8>, JsError> {
        build_threemf(name, vertices, indices, settings_json, None)
    }

    /// Generate a Bambu sliced `.gcode.3mf` containing the mesh and the
    /// pre-generated G-code, ready to send to a Bambu printer over LAN.
    #[wasm_bindgen(js_name = generate3mfWithGcode)]
    pub fn generate_3mf_with_gcode(
        name: &str,
        vertices: &[f32],
        indices: &[u32],
        gcode: &[u8],
        settings_json: &str,
    ) -> Result<Vec<u8>, JsError> {
        build_threemf(name, vertices, indices, settings_json, Some(gcode.to_vec()))
    }

    fn build_threemf(
        name: &str,
        vertices: &[f32],
        indices: &[u32],
        settings_json: &str,
        gcode: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, JsError> {
        use vcad_slicer_bambu::{PrintSettings, ThreeMfModel};

        let mut model = ThreeMfModel::new(name.to_string(), vertices.to_vec(), indices.to_vec());

        if !settings_json.is_empty() {
            #[derive(Deserialize)]
            struct ThreeMfSettings {
                layer_height: Option<f64>,
                first_layer_height: Option<f64>,
                wall_count: Option<u32>,
                infill_density: Option<f64>,
                print_temp: Option<u32>,
                bed_temp: Option<u32>,
                filament_type: Option<String>,
            }

            if let Ok(s) = serde_json::from_str::<ThreeMfSettings>(settings_json) {
                let defaults = PrintSettings::default();
                model.settings = PrintSettings {
                    layer_height: s.layer_height.unwrap_or(defaults.layer_height),
                    first_layer_height: s.first_layer_height.unwrap_or(defaults.first_layer_height),
                    wall_count: s.wall_count.unwrap_or(defaults.wall_count),
                    infill_density: s.infill_density.unwrap_or(defaults.infill_density),
                    print_temp: s.print_temp.unwrap_or(defaults.print_temp),
                    bed_temp: s.bed_temp.unwrap_or(defaults.bed_temp),
                    filament_type: s.filament_type.unwrap_or(defaults.filament_type),
                };
            }
        }

        if let Some(g) = gcode {
            model = model.with_gcode(g);
        }

        model.to_bytes().map_err(|e| JsError::new(&e.to_string()))
    }
}

// Re-export slicer types at module level when feature is enabled
#[cfg(feature = "slicer")]
pub use slicer_wasm::*;

// =========================================================================
// CAM (Computer-Aided Manufacturing) bindings
// =========================================================================

#[cfg(feature = "cam")]
mod cam_wasm {
    use super::*;
    use vcad_kernel_cam::{
        post::{GrblPost, PostProcessor},
        CamSettings, Contour, Contour2D, Face, Pocket2D, Tool, ToolLibrary, Toolpath,
    };

    /// CAM tool definition for WASM.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WasmCamTool {
        /// Tool type: "flat_endmill", "ball_endmill", "bull_endmill", "vbit", "drill", "face_mill"
        #[serde(rename = "type")]
        pub tool_type: String,
        /// Tool diameter (mm).
        pub diameter: f64,
        /// Flute length (mm, for endmills).
        pub flute_length: Option<f64>,
        /// Number of flutes/inserts.
        pub flutes: Option<u8>,
        /// V-bit angle (degrees).
        pub angle: Option<f64>,
        /// Drill point angle (degrees).
        pub point_angle: Option<f64>,
        /// Corner radius for bull endmills (mm).
        pub corner_radius: Option<f64>,
    }

    impl From<WasmCamTool> for Tool {
        fn from(t: WasmCamTool) -> Self {
            match t.tool_type.as_str() {
                "flat_endmill" => Tool::FlatEndMill {
                    diameter: t.diameter,
                    flute_length: t.flute_length.unwrap_or(20.0),
                    flutes: t.flutes.unwrap_or(2),
                },
                "ball_endmill" => Tool::BallEndMill {
                    diameter: t.diameter,
                    flute_length: t.flute_length.unwrap_or(20.0),
                    flutes: t.flutes.unwrap_or(2),
                },
                "bull_endmill" => Tool::BullEndMill {
                    diameter: t.diameter,
                    corner_radius: t.corner_radius.unwrap_or(1.0),
                    flute_length: t.flute_length.unwrap_or(20.0),
                    flutes: t.flutes.unwrap_or(2),
                },
                "vbit" => Tool::VBit {
                    diameter: t.diameter,
                    angle: t.angle.unwrap_or(90.0),
                },
                "drill" => Tool::Drill {
                    diameter: t.diameter,
                    point_angle: t.point_angle.unwrap_or(118.0),
                },
                "face_mill" => Tool::FaceMill {
                    diameter: t.diameter,
                    inserts: t.flutes.unwrap_or(4),
                },
                _ => Tool::FlatEndMill {
                    diameter: t.diameter,
                    flute_length: t.flute_length.unwrap_or(20.0),
                    flutes: t.flutes.unwrap_or(2),
                },
            }
        }
    }

    /// CAM settings for WASM.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[wasm_bindgen]
    pub struct WasmCamSettings {
        /// Stepover distance (mm).
        pub stepover: f64,
        /// Stepdown distance (mm).
        pub stepdown: f64,
        /// Feed rate (mm/min).
        pub feed_rate: f64,
        /// Plunge rate (mm/min).
        pub plunge_rate: f64,
        /// Spindle RPM.
        pub spindle_rpm: f64,
        /// Safe Z height (mm).
        pub safe_z: f64,
        /// Retract Z height (mm).
        pub retract_z: f64,
    }

    #[wasm_bindgen]
    impl WasmCamSettings {
        /// Create default CAM settings.
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self {
                stepover: 3.0,
                stepdown: 2.0,
                feed_rate: 1000.0,
                plunge_rate: 300.0,
                spindle_rpm: 12000.0,
                safe_z: 5.0,
                retract_z: 10.0,
            }
        }

        /// Create from JSON.
        #[wasm_bindgen(js_name = fromJson)]
        pub fn from_json(json: &str) -> Result<WasmCamSettings, JsError> {
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))
        }
    }

    impl Default for WasmCamSettings {
        fn default() -> Self {
            Self::new()
        }
    }

    impl From<WasmCamSettings> for CamSettings {
        fn from(s: WasmCamSettings) -> Self {
            Self {
                stepover: s.stepover,
                stepdown: s.stepdown,
                feed_rate: s.feed_rate,
                plunge_rate: s.plunge_rate,
                spindle_rpm: s.spindle_rpm,
                safe_z: s.safe_z,
                retract_z: s.retract_z,
            }
        }
    }

    /// Generate a face toolpath.
    ///
    /// # Arguments
    /// * `min_x`, `min_y`, `max_x`, `max_y` - Bounds of the area to face
    /// * `depth` - Cut depth (positive value)
    /// * `tool_json` - Tool definition as JSON
    /// * `settings` - CAM settings
    ///
    /// # Returns
    /// Toolpath as JSON string.
    #[wasm_bindgen(js_name = camGenerateFace)]
    pub fn cam_generate_face(
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
        depth: f64,
        tool_json: &str,
        settings: &WasmCamSettings,
    ) -> Result<String, JsError> {
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: Tool = tool.into();
        let settings: CamSettings = settings.clone().into();

        let face = Face::new(min_x, min_y, max_x, max_y, depth);
        let toolpath = face
            .generate(&tool, &settings)
            .map_err(|e| JsError::new(&e.to_string()))?;

        serde_json::to_string(&toolpath).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Generate a rectangular pocket toolpath.
    ///
    /// # Arguments
    /// * `x`, `y` - Top-left corner
    /// * `width`, `height` - Pocket dimensions
    /// * `depth` - Cut depth
    /// * `tool_json` - Tool definition as JSON
    /// * `settings` - CAM settings
    ///
    /// # Returns
    /// Toolpath as JSON string.
    #[wasm_bindgen(js_name = camGeneratePocket)]
    pub fn cam_generate_pocket(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        depth: f64,
        tool_json: &str,
        settings: &WasmCamSettings,
    ) -> Result<String, JsError> {
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: Tool = tool.into();
        let settings: CamSettings = settings.clone().into();

        let pocket = Pocket2D::rectangle(x, y, width, height, depth);
        let toolpath = pocket
            .generate(&tool, &settings)
            .map_err(|e| JsError::new(&e.to_string()))?;

        serde_json::to_string(&toolpath).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Generate a circular pocket toolpath.
    ///
    /// # Arguments
    /// * `cx`, `cy` - Center point
    /// * `radius` - Pocket radius
    /// * `depth` - Cut depth
    /// * `tool_json` - Tool definition as JSON
    /// * `settings` - CAM settings
    ///
    /// # Returns
    /// Toolpath as JSON string.
    #[wasm_bindgen(js_name = camGenerateCircularPocket)]
    pub fn cam_generate_circular_pocket(
        cx: f64,
        cy: f64,
        radius: f64,
        depth: f64,
        tool_json: &str,
        settings: &WasmCamSettings,
    ) -> Result<String, JsError> {
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: Tool = tool.into();
        let settings: CamSettings = settings.clone().into();

        let pocket = Pocket2D::circle(cx, cy, radius, depth);
        let toolpath = pocket
            .generate(&tool, &settings)
            .map_err(|e| JsError::new(&e.to_string()))?;

        serde_json::to_string(&toolpath).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Generate a rectangular contour toolpath.
    ///
    /// # Arguments
    /// * `x`, `y` - Top-left corner
    /// * `width`, `height` - Rectangle dimensions
    /// * `depth` - Cut depth
    /// * `offset` - Offset from contour (positive = outside)
    /// * `tab_count` - Number of tabs (0 for none)
    /// * `tab_width` - Tab width in mm
    /// * `tab_height` - Tab height in mm
    /// * `tool_json` - Tool definition as JSON
    /// * `settings` - CAM settings
    ///
    /// # Returns
    /// Toolpath as JSON string.
    #[wasm_bindgen(js_name = camGenerateContour)]
    #[allow(clippy::too_many_arguments)]
    pub fn cam_generate_contour(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        depth: f64,
        offset: f64,
        tab_count: u32,
        tab_width: f64,
        tab_height: f64,
        tool_json: &str,
        settings: &WasmCamSettings,
    ) -> Result<String, JsError> {
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: Tool = tool.into();
        let settings: CamSettings = settings.clone().into();

        let contour = Contour::rectangle(x, y, width, height);
        let mut op = Contour2D::new(contour, depth).with_offset(offset);

        if tab_count > 0 {
            op = op.with_tabs(tab_count as usize, tab_width, tab_height);
        }

        let toolpath = op
            .generate(&tool, &settings)
            .map_err(|e| JsError::new(&e.to_string()))?;

        serde_json::to_string(&toolpath).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Export toolpath to GRBL G-code.
    ///
    /// # Arguments
    /// * `toolpath_json` - Toolpath as JSON string
    /// * `job_name` - Name for the G-code file header
    /// * `tool_json` - Tool definition as JSON
    /// * `settings` - CAM settings
    ///
    /// # Returns
    /// G-code as string.
    #[wasm_bindgen(js_name = camExportGcode)]
    pub fn cam_export_gcode(
        toolpath_json: &str,
        job_name: &str,
        tool_json: &str,
        settings: &WasmCamSettings,
    ) -> Result<String, JsError> {
        let toolpath: Toolpath =
            serde_json::from_str(toolpath_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: Tool = tool.into();
        let settings: CamSettings = settings.clone().into();

        let post = GrblPost::default();
        Ok(post.generate(job_name, &tool, &toolpath, &settings))
    }

    /// Get toolpath statistics.
    ///
    /// # Arguments
    /// * `toolpath_json` - Toolpath as JSON string
    ///
    /// # Returns
    /// JSON object with statistics: { cutting_length, estimated_time, bounding_box }
    #[wasm_bindgen(js_name = camToolpathStats)]
    pub fn cam_toolpath_stats(toolpath_json: &str) -> Result<JsValue, JsError> {
        let toolpath: Toolpath =
            serde_json::from_str(toolpath_json).map_err(|e| JsError::new(&e.to_string()))?;

        #[derive(Serialize)]
        struct Stats {
            cutting_length: f64,
            estimated_time: f64,
            segment_count: usize,
            bounding_box: Option<[[f64; 3]; 2]>,
        }

        let bbox = toolpath.bounding_box().map(|(min, max)| [min, max]);

        let stats = Stats {
            cutting_length: toolpath.cutting_length(),
            estimated_time: toolpath.estimated_time(),
            segment_count: toolpath.len(),
            bounding_box: bbox,
        };

        serde_wasm_bindgen::to_value(&stats).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Get default tool library.
    ///
    /// # Returns
    /// Tool library as JSON array.
    #[wasm_bindgen(js_name = camGetDefaultTools)]
    pub fn cam_get_default_tools() -> Result<String, JsError> {
        let lib = ToolLibrary::default_library();

        #[derive(Serialize)]
        struct ToolInfo {
            number: u32,
            name: String,
            tool_type: String,
            diameter: f64,
            default_rpm: f64,
            default_feed: f64,
        }

        let tools: Vec<ToolInfo> = lib
            .tools
            .iter()
            .map(|entry| {
                let tool_type = match &entry.tool {
                    Tool::FlatEndMill { .. } => "flat_endmill",
                    Tool::BallEndMill { .. } => "ball_endmill",
                    Tool::BullEndMill { .. } => "bull_endmill",
                    Tool::VBit { .. } => "vbit",
                    Tool::Drill { .. } => "drill",
                    Tool::FaceMill { .. } => "face_mill",
                };

                ToolInfo {
                    number: entry.number,
                    name: entry.name.clone(),
                    tool_type: tool_type.to_string(),
                    diameter: entry.tool.diameter(),
                    default_rpm: entry.default_rpm,
                    default_feed: entry.default_feed,
                }
            })
            .collect();

        serde_json::to_string(&tools).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Check if CAM is available.
    #[wasm_bindgen(js_name = isCamAvailable)]
    pub fn is_cam_available() -> bool {
        true
    }

    // =========================================================================
    // Phase 2: 3D Roughing
    // =========================================================================

    /// Generate a height field from mesh using drop-cutter algorithm.
    ///
    /// # Arguments
    /// * `vertices_json` - Vertex array as JSON [[x,y,z], ...]
    /// * `indices_json` - Triangle indices as JSON [i0, i1, i2, ...]
    /// * `tool_json` - Tool definition as JSON
    /// * `bounds_json` - Bounds [min_x, min_y, max_x, max_y] as JSON
    /// * `resolution` - Sample spacing in mm
    ///
    /// # Returns
    /// Height field as JSON with { nx, ny, bounds, heights }
    #[wasm_bindgen(js_name = camDropCutter)]
    pub fn cam_drop_cutter(
        vertices_json: &str,
        indices_json: &str,
        tool_json: &str,
        bounds_json: &str,
        resolution: f64,
    ) -> Result<String, JsError> {
        use vcad_kernel_cam::dropcutter::{generate_height_field, MeshAccel};

        let vertices: Vec<[f64; 3]> =
            serde_json::from_str(vertices_json).map_err(|e| JsError::new(&e.to_string()))?;
        let indices: Vec<u32> =
            serde_json::from_str(indices_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let bounds: [f64; 4] =
            serde_json::from_str(bounds_json).map_err(|e| JsError::new(&e.to_string()))?;

        let tool: Tool = tool.into();
        let cell_size = resolution.max(1.0);

        let accel = MeshAccel::new(&vertices, &indices, cell_size);
        let height_field = generate_height_field(&accel, &tool, bounds, resolution);

        serde_json::to_string(&height_field).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Generate 3D roughing toolpath from a height field.
    ///
    /// # Arguments
    /// * `height_field_json` - Height field from cam_drop_cutter
    /// * `tool_json` - Tool definition as JSON
    /// * `settings` - CAM settings
    /// * `target_z` - Target bottom Z depth
    /// * `top_z` - Top Z (stock surface)
    /// * `stock_margin` - Extra material to leave (mm)
    /// * `direction` - Raster direction in degrees (0=X, 90=Y)
    ///
    /// # Returns
    /// Toolpath as JSON string.
    #[wasm_bindgen(js_name = camGenerateRoughing3d)]
    #[allow(clippy::too_many_arguments)]
    pub fn cam_generate_roughing3d(
        height_field_json: &str,
        tool_json: &str,
        settings: &WasmCamSettings,
        target_z: f64,
        top_z: f64,
        stock_margin: f64,
        direction: f64,
    ) -> Result<String, JsError> {
        use vcad_kernel_cam::dropcutter::HeightField;
        use vcad_kernel_cam::Roughing3D;

        let height_field: HeightField =
            serde_json::from_str(height_field_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: Tool = tool.into();
        let settings: CamSettings = settings.clone().into();

        let op = Roughing3D::new(target_z, top_z)
            .with_margin(stock_margin)
            .with_direction(direction);

        let toolpath = op
            .generate(&height_field, &tool, &settings)
            .map_err(|e| JsError::new(&e.to_string()))?;

        serde_json::to_string(&toolpath).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Export toolpath to LinuxCNC G-code.
    ///
    /// # Arguments
    /// * `toolpath_json` - Toolpath as JSON string
    /// * `job_name` - Name for the G-code file header
    /// * `tool_json` - Tool definition as JSON
    /// * `settings` - CAM settings
    /// * `program_number` - O-word program number
    ///
    /// # Returns
    /// G-code as string.
    #[wasm_bindgen(js_name = camExportLinuxCnc)]
    pub fn cam_export_linuxcnc(
        toolpath_json: &str,
        job_name: &str,
        tool_json: &str,
        settings: &WasmCamSettings,
        program_number: u32,
    ) -> Result<String, JsError> {
        use vcad_kernel_cam::post::{LinuxCncPost, PostProcessor};

        let toolpath: Toolpath =
            serde_json::from_str(toolpath_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: WasmCamTool =
            serde_json::from_str(tool_json).map_err(|e| JsError::new(&e.to_string()))?;
        let tool: Tool = tool.into();
        let settings: CamSettings = settings.clone().into();

        let post = LinuxCncPost::default().with_program_number(program_number);
        Ok(post.generate(job_name, &tool, &toolpath, &settings))
    }
}

// Re-export CAM types at module level when feature is enabled
#[cfg(feature = "cam")]
pub use cam_wasm::*;

// =============================================================================
// ECAD (Electronics) bindings
// =============================================================================

#[cfg(feature = "ecad")]
mod ecad_wasm {
    use vcad_ir::ecad::{Pcb, SchematicSheet};
    use wasm_bindgen::prelude::*;

    /// Check if ECAD features are available in this build.
    #[wasm_bindgen(js_name = isEcadAvailable)]
    pub fn is_ecad_available() -> bool {
        true
    }

    /// Run Design Rule Check on a PCB layout.
    ///
    /// # Arguments
    /// * `pcb_json` - JSON-serialized `Pcb` struct
    ///
    /// # Returns
    /// Array of DRC violations as JsValue.
    #[wasm_bindgen(js_name = ecadCheckDrc)]
    pub fn ecad_check_drc(pcb_json: &str) -> Result<JsValue, JsError> {
        let pcb: Pcb = serde_json::from_str(pcb_json).map_err(|e| JsError::new(&e.to_string()))?;
        let violations = vcad_ecad_pcb::drc::check_drc(&pcb);
        serde_wasm_bindgen::to_value(&violations).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Run Electrical Rule Check on a schematic sheet.
    ///
    /// # Arguments
    /// * `sch_json` - JSON-serialized `SchematicSheet` struct
    ///
    /// # Returns
    /// Array of ERC violations as JsValue.
    #[wasm_bindgen(js_name = ecadCheckErc)]
    pub fn ecad_check_erc(sch_json: &str) -> Result<JsValue, JsError> {
        let sheet: SchematicSheet =
            serde_json::from_str(sch_json).map_err(|e| JsError::new(&e.to_string()))?;
        let violations = vcad_ecad_schematic::erc::check_erc(&sheet);
        serde_wasm_bindgen::to_value(&violations).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Generate a netlist from a schematic sheet.
    ///
    /// # Arguments
    /// * `sch_json` - JSON-serialized `SchematicSheet` struct
    ///
    /// # Returns
    /// Netlist as JsValue.
    #[wasm_bindgen(js_name = ecadGenerateNetlist)]
    pub fn ecad_generate_netlist(sch_json: &str) -> Result<JsValue, JsError> {
        let sheet: SchematicSheet =
            serde_json::from_str(sch_json).map_err(|e| JsError::new(&e.to_string()))?;
        let netlist = vcad_ecad_schematic::generate_netlist(&sheet);
        serde_wasm_bindgen::to_value(&netlist).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Route a net between two points on the PCB using the grid router.
    ///
    /// # Arguments
    /// * `pcb_json` - JSON-serialized `Pcb` struct
    /// * `net` - Net name to route
    /// * `start_x`, `start_y` - Start coordinates (mm)
    /// * `end_x`, `end_y` - End coordinates (mm)
    /// * `width` - Trace width (mm)
    ///
    /// # Returns
    /// Route result with segments and vias.
    #[wasm_bindgen(js_name = ecadRouteNet)]
    pub fn ecad_route_net(
        pcb_json: &str,
        net: &str,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        width: f64,
    ) -> Result<JsValue, JsError> {
        let pcb: Pcb = serde_json::from_str(pcb_json).map_err(|e| JsError::new(&e.to_string()))?;

        // Determine board extents from outline
        let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
        let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
        for v in &pcb.outline.vertices {
            min_x = min_x.min(v.x);
            min_y = min_y.min(v.y);
            max_x = max_x.max(v.x);
            max_y = max_y.max(v.y);
        }
        let board_w = max_x - min_x;
        let board_h = max_y - min_y;

        // Resolution based on trace width (half width for decent grid)
        let resolution = (width * 0.5).max(0.1);
        let mut router = vcad_ecad_pcb::router::grid::GridRouter::new(board_w, board_h, resolution);

        // Add existing traces as obstacles
        for trace in &pcb.traces {
            if trace.net != net {
                let hw = trace.width * 0.5 + pcb.rules.default_rules.clearance;
                let tx_min = trace.start.x.min(trace.end.x) - hw - min_x;
                let ty_min = trace.start.y.min(trace.end.y) - hw - min_y;
                let tx_max = trace.start.x.max(trace.end.x) + hw - min_x;
                let ty_max = trace.start.y.max(trace.end.y) + hw - min_y;
                router.add_obstacle(
                    vcad_ir::Vec2 {
                        x: tx_min,
                        y: ty_min,
                    },
                    vcad_ir::Vec2 {
                        x: tx_max,
                        y: ty_max,
                    },
                );
            }
        }

        let start = vcad_ir::Vec2 {
            x: start_x - min_x,
            y: start_y - min_y,
        };
        let end = vcad_ir::Vec2 {
            x: end_x - min_x,
            y: end_y - min_y,
        };
        let result = router.route_net(net, start, end);
        serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Fill copper pour zones on the PCB.
    ///
    /// # Arguments
    /// * `pcb_json` - JSON-serialized `Pcb` struct
    ///
    /// # Returns
    /// Array of filled zone polygons.
    #[wasm_bindgen(js_name = ecadFillZones)]
    pub fn ecad_fill_zones(pcb_json: &str) -> Result<JsValue, JsError> {
        let pcb: Pcb = serde_json::from_str(pcb_json).map_err(|e| JsError::new(&e.to_string()))?;
        let filled = vcad_ecad_pcb::copper_pour::fill_zones(&pcb);
        serde_wasm_bindgen::to_value(&filled).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Parse a KiCad `.kicad_pcb` file content into a JSON-serialized `Pcb`.
    ///
    /// # Arguments
    /// * `content` - The `.kicad_pcb` file content as a string
    ///
    /// # Returns
    /// JSON-serialized `Pcb` struct as JsValue, or error.
    #[wasm_bindgen(js_name = parseKicadPcb)]
    pub fn parse_kicad_pcb(content: &str) -> Result<JsValue, JsError> {
        let pcb = vcad_ecad_symbols::parse_kicad_pcb(content)
            .map_err(|e| JsError::new(&e.to_string()))?;
        serde_wasm_bindgen::to_value(&pcb).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Return all builtin symbol definitions.
    ///
    /// # Returns
    /// Array of `SymbolDef` as JsValue.
    #[wasm_bindgen(js_name = ecadBuiltinSymbols)]
    pub fn ecad_builtin_symbols() -> Result<JsValue, JsError> {
        let symbols = vcad_ecad_symbols::builtin::builtin_symbols();
        serde_wasm_bindgen::to_value(&symbols).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Look up a single builtin symbol by ID.
    ///
    /// # Arguments
    /// * `id` - Symbol identifier (e.g. "resistor", "capacitor", "npn")
    ///
    /// # Returns
    /// `SymbolDef` as JsValue, or null if not found.
    #[wasm_bindgen(js_name = ecadGetSymbol)]
    pub fn ecad_get_symbol(id: &str) -> Result<JsValue, JsError> {
        let symbol = vcad_ecad_symbols::builtin::get_symbol(id);
        serde_wasm_bindgen::to_value(&symbol).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Compute ratsnest lines for unrouted net connections.
    ///
    /// # Arguments
    /// * `pcb_json` - JSON-serialized `Pcb` struct
    /// * `netlist_json` - JSON-serialized netlist
    ///
    /// # Returns
    /// Array of ratsnest lines as JsValue.
    #[wasm_bindgen(js_name = ecadComputeRatsnest)]
    pub fn ecad_compute_ratsnest(pcb_json: &str, netlist_json: &str) -> Result<JsValue, JsError> {
        let pcb: vcad_ir::ecad::Pcb =
            serde_json::from_str(pcb_json).map_err(|e| JsError::new(&e.to_string()))?;
        let netlist: vcad_ecad_pcb::ratsnest::Netlist =
            serde_json::from_str(netlist_json).map_err(|e| JsError::new(&e.to_string()))?;
        let lines = vcad_ecad_pcb::ratsnest::compute_ratsnest(&pcb, &netlist);
        serde_wasm_bindgen::to_value(&lines).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Compute Z offset for a PCB layer.
    ///
    /// # Arguments
    /// * `layer` - Layer name (e.g. "FCu", "BCu")
    /// * `thickness` - Board thickness in mm
    /// * `explosion` - Explosion factor (0 = normal, >0 = exploded)
    #[wasm_bindgen(js_name = ecadLayerZ)]
    pub fn ecad_layer_z(layer: &str, thickness: f64, explosion: f64) -> f64 {
        let pcb_layer: vcad_ir::ecad::PcbLayer =
            serde_json::from_str(&format!("\"{layer}\"")).unwrap_or(vcad_ir::ecad::PcbLayer::FCu);
        vcad_ecad_pcb::geometry::layer_z(pcb_layer, thickness, explosion)
    }

    /// Generate 3D component body meshes for all footprints on a PCB.
    ///
    /// # Arguments
    /// * `pcb_json` - JSON-serialized `Pcb` struct
    ///
    /// # Returns
    /// Array of component meshes as JsValue.
    #[wasm_bindgen(js_name = ecadComponentMeshes)]
    pub fn ecad_component_meshes(pcb_json: &str) -> Result<JsValue, JsError> {
        let pcb: vcad_ir::ecad::Pcb =
            serde_json::from_str(pcb_json).map_err(|e| JsError::new(&e.to_string()))?;
        let meshes = vcad_ecad_pcb::component_mesh::generate_component_meshes(&pcb);
        serde_wasm_bindgen::to_value(&meshes).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Snap a position to the nearest component pin or grid point.
    ///
    /// # Arguments
    /// * `x`, `y` - Cursor position
    /// * `components_json` - JSON-serialized `SchematicComponent[]`
    /// * `grid` - Grid spacing
    /// * `threshold` - Max distance to snap to a pin
    ///
    /// # Returns
    /// `{ position: { x, y }, is_pin: bool }` as JsValue.
    #[wasm_bindgen(js_name = ecadSnapToGridOrPin)]
    pub fn ecad_snap_to_grid_or_pin(
        x: f64,
        y: f64,
        components_json: &str,
        grid: f64,
        threshold: f64,
    ) -> Result<JsValue, JsError> {
        let components: Vec<vcad_ir::ecad::SchematicComponent> =
            serde_json::from_str(components_json).map_err(|e| JsError::new(&e.to_string()))?;
        let pos = vcad_ir::Vec2::new(x, y);
        let result =
            vcad_ecad_schematic::geometry::snap_to_grid_or_pin(pos, &components, grid, threshold);
        serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Get the net for a wire based on endpoint proximity to component pins.
    ///
    /// # Arguments
    /// * `wire_json` - JSON-serialized `SchematicWire`
    /// * `netlist_json` - JSON-serialized `Netlist`
    /// * `components_json` - JSON-serialized `SchematicComponent[]`
    ///
    /// # Returns
    /// Net name as string, or null.
    #[wasm_bindgen(js_name = ecadNetForWire)]
    pub fn ecad_net_for_wire(
        wire_json: &str,
        netlist_json: &str,
        components_json: &str,
    ) -> Result<JsValue, JsError> {
        let wire: vcad_ir::ecad::SchematicWire =
            serde_json::from_str(wire_json).map_err(|e| JsError::new(&e.to_string()))?;
        let netlist: vcad_ecad_schematic::Netlist =
            serde_json::from_str(netlist_json).map_err(|e| JsError::new(&e.to_string()))?;
        let components: Vec<vcad_ir::ecad::SchematicComponent> =
            serde_json::from_str(components_json).map_err(|e| JsError::new(&e.to_string()))?;
        let result = vcad_ecad_schematic::geometry::net_for_wire(&wire, &netlist, &components);
        serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
    }
}

#[cfg(feature = "ecad")]
pub use ecad_wasm::*;

// =============================================================================
// Full document evaluation
// =============================================================================

// =============================================================================
// WASM Clock for timing instrumentation
// =============================================================================

#[wasm_bindgen]
extern "C" {
    /// Binding to `performance.now()` — works in both main thread and web workers.
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;
}

/// Clock implementation backed by `performance.now()`.
struct WasmClock;

impl vcad_eval::Clock for WasmClock {
    fn now_ms(&self) -> f64 {
        performance_now()
    }
}

/// Returns the `WebAssembly.Module` instance backing this kernel-wasm
/// import. Workers can pass this to `wasm.default({ module_or_path })`
/// to skip the multi-second recompile of a fresh fetch — see
/// `packages/engine/src/eval-worker.ts` for the consumer.
#[wasm_bindgen(js_name = getCompiledModule)]
pub fn get_compiled_module() -> JsValue {
    wasm_bindgen::module()
}

/// Evaluate a full vcad document JSON into a serialized EvaluatedScene.
///
/// This is the canonical Rust-side evaluator that handles all CsgOp variants
/// including Sketch2D, Extrude, Revolve, Sweep, Loft, Text2D, ImportedMesh,
/// assembly with forward kinematics, and clash detection.
///
/// # Arguments
///
/// * `doc_json` - A JSON string representing a vcad Document
/// * `skip_clash_detection` - If true, skip O(n²) clash detection
///
/// # Returns
///
/// A JsValue containing the serialized EvaluatedScene.
#[wasm_bindgen(js_name = evaluateDocument)]
pub fn evaluate_document(doc_json: &str, skip_clash_detection: bool) -> Result<JsValue, JsError> {
    let t_parse = performance_now();
    let doc: vcad_ir::Document = serde_json::from_str(doc_json)
        .map_err(|e| JsError::new(&format!("Failed to parse document: {}", e)))?;
    let parse_ms = performance_now() - t_parse;

    let options = vcad_eval::EvalOptions {
        skip_clash_detection,
        clock: Some(Box::new(WasmClock)),
    };

    let mut scene = vcad_eval::evaluate_document(&doc, &options)
        .map_err(|e| JsError::new(&format!("Evaluation error: {}", e)))?;

    // Inject parse_ms into timing
    if let Some(ref mut timing) = scene.timing {
        timing.parse_ms = Some(parse_ms);
    }

    // Serialize the scene to a JS-friendly format using typed arrays (not serde_wasm_bindgen)
    // serde_wasm_bindgen converts Vec<f32> element-by-element → individual JS Numbers,
    // which is ~300ms for large meshes. Direct typed array copy is ~1ms.
    let t_ser = performance_now();
    let js_val = scene_to_js(&scene);
    let serialize_ms = performance_now() - t_ser;

    // Inject serialize_ms into timing
    if let Ok(timing_val) = js_sys::Reflect::get(&js_val, &"timing".into()) {
        if !timing_val.is_undefined() && !timing_val.is_null() {
            let _ = js_sys::Reflect::set(
                &timing_val,
                &"serialize_ms".into(),
                &JsValue::from_f64(serialize_ms),
            );
        }
    }

    Ok(js_val)
}

/// Solve forward kinematics for an assembly document.
///
/// # Arguments
///
/// * `doc_json` - A JSON string representing a vcad Document
///
/// # Returns
///
/// A JsValue containing a Map of instance_id -> Transform3D.
#[wasm_bindgen(js_name = solveForwardKinematics)]
pub fn solve_forward_kinematics(doc_json: &str) -> Result<JsValue, JsError> {
    let doc: vcad_ir::Document = serde_json::from_str(doc_json)
        .map_err(|e| JsError::new(&format!("Failed to parse document: {}", e)))?;
    let transforms = vcad_eval::kinematics::solve_forward_kinematics(&doc);
    serde_wasm_bindgen::to_value(&transforms).map_err(|e| JsError::new(&e.to_string()))
}

/// Convert an EvaluatedScene to JsValue using typed arrays for mesh data.
///
/// This replaces `serde_wasm_bindgen::to_value` which is ~300ms because it converts
/// each f32/u32 element individually. Using `js_sys::Float32Array::from` does a single
/// memcpy, bringing serialization to ~1ms.
fn scene_to_js(scene: &vcad_eval::EvaluatedScene) -> JsValue {
    let obj = js_sys::Object::new();

    // Parts
    let parts_arr = js_sys::Array::new_with_length(scene.parts.len() as u32);
    for (i, part) in scene.parts.iter().enumerate() {
        let part_obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&part_obj, &"mesh".into(), &mesh_to_js(&part.mesh));
        let _ = js_sys::Reflect::set(&part_obj, &"material".into(), &part.material.clone().into());
        parts_arr.set(i as u32, part_obj.into());
    }
    let _ = js_sys::Reflect::set(&obj, &"parts".into(), &parts_arr.into());

    // Part defs
    if let Some(ref part_defs) = scene.part_defs {
        let defs_arr = js_sys::Array::new_with_length(part_defs.len() as u32);
        for (i, pd) in part_defs.iter().enumerate() {
            let pd_obj = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&pd_obj, &"id".into(), &pd.id.clone().into());
            let _ = js_sys::Reflect::set(&pd_obj, &"mesh".into(), &mesh_to_js(&pd.mesh));
            defs_arr.set(i as u32, pd_obj.into());
        }
        let _ = js_sys::Reflect::set(&obj, &"partDefs".into(), &defs_arr.into());
    }

    // Instances
    if let Some(ref instances) = scene.instances {
        let inst_arr = js_sys::Array::new_with_length(instances.len() as u32);
        for (i, inst) in instances.iter().enumerate() {
            let inst_obj = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &inst_obj,
                &"instance_id".into(),
                &inst.instance_id.clone().into(),
            );
            let _ = js_sys::Reflect::set(
                &inst_obj,
                &"part_def_id".into(),
                &inst.part_def_id.clone().into(),
            );
            if let Some(ref name) = inst.name {
                let _ = js_sys::Reflect::set(&inst_obj, &"name".into(), &name.clone().into());
            }
            let _ = js_sys::Reflect::set(&inst_obj, &"mesh".into(), &mesh_to_js(&inst.mesh));
            let _ =
                js_sys::Reflect::set(&inst_obj, &"material".into(), &inst.material.clone().into());
            if let Some(ref transform) = inst.transform {
                // Serialize transform via serde (small object, fast)
                if let Ok(t) = serde_wasm_bindgen::to_value(transform) {
                    let _ = js_sys::Reflect::set(&inst_obj, &"transform".into(), &t);
                }
            }
            inst_arr.set(i as u32, inst_obj.into());
        }
        let _ = js_sys::Reflect::set(&obj, &"instances".into(), &inst_arr.into());
    }

    // Clashes
    let clashes_arr = js_sys::Array::new_with_length(scene.clashes.len() as u32);
    for (i, clash) in scene.clashes.iter().enumerate() {
        clashes_arr.set(i as u32, mesh_to_js(clash));
    }
    let _ = js_sys::Reflect::set(&obj, &"clashes".into(), &clashes_arr.into());

    // Failures (per-root evaluation errors). Omit when empty so JS consumers
    // can treat `undefined` and `[]` interchangeably.
    if !scene.failures.is_empty() {
        if let Ok(f) = serde_wasm_bindgen::to_value(&scene.failures) {
            let _ = js_sys::Reflect::set(&obj, &"failures".into(), &f);
        }
    }

    // Timing
    if let Some(ref timing) = scene.timing {
        if let Ok(t) = serde_wasm_bindgen::to_value(timing) {
            let _ = js_sys::Reflect::set(&obj, &"timing".into(), &t);
        }
    }

    obj.into()
}

/// Convert an EvaluatedMesh to JsValue using typed arrays (single memcpy each).
fn mesh_to_js(mesh: &vcad_eval::EvaluatedMesh) -> JsValue {
    let obj = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &obj,
        &"positions".into(),
        &js_sys::Float32Array::from(mesh.positions.as_slice()).into(),
    );
    let _ = js_sys::Reflect::set(
        &obj,
        &"indices".into(),
        &js_sys::Uint32Array::from(mesh.indices.as_slice()).into(),
    );
    if let Some(ref normals) = mesh.normals {
        let _ = js_sys::Reflect::set(
            &obj,
            &"normals".into(),
            &js_sys::Float32Array::from(normals.as_slice()).into(),
        );
    }
    if let Some(ref face_kinds) = mesh.face_kinds {
        let _ = js_sys::Reflect::set(
            &obj,
            &"faceKinds".into(),
            &js_sys::Uint8Array::from(face_kinds.as_slice()).into(),
        );
    }
    obj.into()
}

/// Run the render-bake pipeline on a raw triangle mesh.
///
/// Used by the imported-mesh path (STL / STEP drops) so meshes that arrive
/// from outside the kernel get the same post-processing as kernel-emitted
/// meshes: angle-based creased vertex normals today, tangent generation and
/// LOD baking later. Positions and indices may be duplicated (the mesh
/// becomes unindexed) so downstream consumers just upload the returned
/// arrays.
///
/// Input is `{ positions: Float32Array, indices: Uint32Array, crease_angle_rad?: f64 }`
/// encoded as JSON. Returns `{ positions, indices, normals }` with the same
/// encoding.
#[wasm_bindgen(js_name = renderBakeMesh)]
pub fn render_bake_mesh_wasm(input_json: &str) -> Result<String, JsError> {
    #[derive(serde::Deserialize)]
    struct Input {
        positions: Vec<f32>,
        indices: Vec<u32>,
        #[serde(default)]
        crease_angle_rad: Option<f64>,
    }
    #[derive(serde::Serialize)]
    struct Output {
        positions: Vec<f32>,
        indices: Vec<u32>,
        normals: Vec<f32>,
    }
    let input: Input = serde_json::from_str(input_json)
        .map_err(|e| JsError::new(&format!("invalid input JSON: {e}")))?;
    let mut mesh = vcad_kernel_tessellate::TriangleMesh {
        vertices: input.positions,
        indices: input.indices,
        normals: Vec::new(),
        face_kinds: Vec::new(),
    };
    let opts = vcad_kernel_tessellate::RenderBakeOptions {
        crease_angle_rad: input
            .crease_angle_rad
            .unwrap_or(vcad_kernel_tessellate::DEFAULT_CREASE_ANGLE_RAD),
    };
    vcad_kernel_tessellate::render_bake(&mut mesh, opts);
    let out = Output {
        positions: mesh.vertices,
        indices: mesh.indices,
        normals: mesh.normals,
    };
    serde_json::to_string(&out).map_err(|e| JsError::new(&format!("serialize failed: {e}")))
}

// ============================================================================
// Parts library (stdlib)
// ============================================================================

/// Return the full parts manifest JSON for the built-in stdlib.
///
/// The app consumes this on boot to populate the palette's Parts tab and
/// the Cmd+K search index.
#[wasm_bindgen(js_name = getPartsManifest)]
pub fn get_parts_manifest() -> String {
    vcad_parts::manifest_json()
}

/// Build a built-in part's sub-document given its path and params JSON.
///
/// `path` is either a bare id (`"fastener.bolt.socket-head"`) or prefixed
/// with `std:`. `params_json` is a JSON object whose keys are parameter
/// names. Returns a JSON-serialized [`vcad_ir::Document`] that the engine
/// can splice into the parent document.
#[wasm_bindgen(js_name = buildPart)]
pub fn build_part(path: &str, params_json: &str) -> Result<String, JsError> {
    let params: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(params_json)
            .map_err(|e| JsError::new(&format!("invalid params JSON: {e}")))?;
    let doc = vcad_parts::build_part(path, &params).map_err(|e| JsError::new(&e))?;
    serde_json::to_string(&doc).map_err(|e| JsError::new(&format!("serialize failed: {e}")))
}

/// Evaluate a loon source string and return a JSON-serialized vcad Document.
///
/// The vcad library (types, constructors) is automatically prepended.
/// Module resolution (`[use ...]`) is not available in WASM — all code
/// must be self-contained or use the bundled vcad library.
#[wasm_bindgen(js_name = evalVcadSource)]
pub fn eval_vcad_source(source: &str) -> Result<JsValue, JsError> {
    let doc = vcad_loon::eval_vcad(source, None).map_err(|e| JsError::new(&e))?;
    let json = serde_json::to_string(&doc)
        .map_err(|e| JsError::new(&format!("Serialization error: {}", e)))?;
    Ok(JsValue::from_str(&json))
}

/// Convert a Document (as JSON) back to loon source code.
#[wasm_bindgen(js_name = documentToLoon)]
pub fn document_to_loon(doc_json: &str) -> Result<String, JsError> {
    let doc: vcad_ir::Document = serde_json::from_str(doc_json)
        .map_err(|e| JsError::new(&format!("Failed to parse document: {}", e)))?;
    Ok(vcad_ir::to_loon::document_to_loon(&doc))
}

/// Convert a Document (as JSON) to loon, also returning unsupported variant names.
///
/// Returns a JS object `{ source: string, unsupported: string[] }`.
/// When `unsupported` is non-empty, the output contains comment placeholders for
/// those nodes and callers should warn the user that data will be lost.
#[wasm_bindgen(js_name = documentToLoonChecked)]
pub fn document_to_loon_checked(doc_json: &str) -> Result<JsValue, JsError> {
    let doc: vcad_ir::Document = serde_json::from_str(doc_json)
        .map_err(|e| JsError::new(&format!("Failed to parse document: {}", e)))?;
    let (source, unsupported) = vcad_ir::to_loon::document_to_loon_checked(&doc);
    let result = serde_json::json!({ "source": source, "unsupported": unsupported });
    serde_wasm_bindgen::to_value(&result).map_err(|e| JsError::new(&e.to_string()))
}

/// Parse a .vcad file (JSON v0.1, VCode v0.2, or loon v0.3).
///
/// Returns a JSON-serialized VcadFile with document, parts, and metadata.
#[wasm_bindgen(js_name = parseVcadFile)]
pub fn parse_vcad_file(content: &str) -> Result<JsValue, JsError> {
    let eval_loon =
        |source: &str| -> Result<vcad_ir::Document, String> { vcad_loon::eval_vcad(source, None) };
    let vcad_file = vcad_ir::file_io::parse_vcad_file_with_loon(content, Some(&eval_loon))
        .map_err(|e| JsError::new(&e))?;
    serde_wasm_bindgen::to_value(&vcad_file).map_err(|e| JsError::new(&e.to_string()))
}

/// Derive parts from a Document (as JSON).
///
/// Returns a JSON-serialized Vec<PartInfo>.
#[wasm_bindgen(js_name = deriveParts)]
pub fn derive_parts(doc_json: &str) -> Result<JsValue, JsError> {
    let doc: vcad_ir::Document = serde_json::from_str(doc_json)
        .map_err(|e| JsError::new(&format!("Failed to parse document: {}", e)))?;
    let parts = vcad_ir::file_io::derive_parts(&doc);
    serde_wasm_bindgen::to_value(&parts).map_err(|e| JsError::new(&e.to_string()))
}

/// Compute volume of a closed triangle mesh using the divergence theorem.
///
/// Positions are `[x, y, z, ...]` (flat f32), indices are `[i0, i1, i2, ...]`.
/// Returns volume in mm³ (same units as positions).
#[wasm_bindgen(js_name = computeMeshVolume)]
pub fn compute_mesh_volume(positions: &[f32], indices: &[u32]) -> f64 {
    let mut vol = 0.0_f64;
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            break;
        }
        let (i0, i1, i2) = (
            tri[0] as usize * 3,
            tri[1] as usize * 3,
            tri[2] as usize * 3,
        );
        if i2 + 2 >= positions.len() {
            continue;
        }
        let v0 = [
            positions[i0] as f64,
            positions[i0 + 1] as f64,
            positions[i0 + 2] as f64,
        ];
        let v1 = [
            positions[i1] as f64,
            positions[i1 + 1] as f64,
            positions[i1 + 2] as f64,
        ];
        let v2 = [
            positions[i2] as f64,
            positions[i2 + 1] as f64,
            positions[i2 + 2] as f64,
        ];
        vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2]) - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
            + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
    }
    (vol / 6.0).abs()
}

// =============================================================================
// Embroidery module (feature-gated)
// =============================================================================

#[cfg(feature = "embroidery")]
mod embroidery_wasm {
    use serde::{Deserialize, Serialize};
    use vcad_embroidery::{
        fill_stitch, fill_stitch_multi, running_stitch, satin_stitch, EmbPattern, FillParams,
        Path2D, PatternMetadata, RunningStitchParams, SatinParams, StitchCommand, StitchGroup,
        Thread,
    };
    use wasm_bindgen::prelude::*;

    /// Check if embroidery support is available.
    #[wasm_bindgen(js_name = isEmbroideryAvailable)]
    pub fn is_embroidery_available() -> bool {
        true
    }

    /// Read a PES file and return embroidery data as JSON.
    ///
    /// Returns `{ threads, stitchPaths, stats }` as a JSON string.
    #[wasm_bindgen(js_name = readEmbroideryPes)]
    pub fn read_embroidery_pes(data: &[u8]) -> Result<String, JsError> {
        let pattern =
            vcad_embroidery_pes::read_pes(data).map_err(|e| JsError::new(&e.to_string()))?;
        serialize_pattern(&pattern).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Read a DST file and return embroidery data as JSON.
    #[wasm_bindgen(js_name = readEmbroideryDst)]
    pub fn read_embroidery_dst(data: &[u8]) -> Result<String, JsError> {
        let pattern =
            vcad_embroidery_dst::read_dst(data).map_err(|e| JsError::new(&e.to_string()))?;
        serialize_pattern(&pattern).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Write a PES file from an embroidery pattern JSON string.
    #[wasm_bindgen(js_name = writeEmbroideryPes)]
    pub fn write_embroidery_pes(json: &str) -> Result<Vec<u8>, JsError> {
        let pattern: EmbPattern =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        vcad_embroidery_pes::write_pes(&pattern).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Write a DST file from an embroidery pattern JSON string.
    #[wasm_bindgen(js_name = writeEmbroideryDst)]
    pub fn write_embroidery_dst(json: &str) -> Result<Vec<u8>, JsError> {
        let pattern: EmbPattern =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        vcad_embroidery_dst::write_dst(&pattern).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Options for text digitization.
    #[derive(Deserialize)]
    struct DigitizeTextOptions {
        #[serde(default = "default_stitch_type")]
        stitch_type: String,
        #[serde(default = "default_thread_color")]
        color: [u8; 3],
        #[serde(default = "default_stitch_length")]
        stitch_length: f64,
        #[serde(default = "default_density")]
        density: f64,
        #[serde(default = "default_satin_width")]
        satin_width: f64,
        #[serde(default)]
        fill_angle: f64,
        #[serde(default = "default_letter_spacing")]
        letter_spacing: f64,
        #[serde(default = "default_line_spacing")]
        line_spacing: f64,
        #[serde(default = "default_alignment")]
        alignment: String,
    }

    fn default_stitch_type() -> String {
        "running".into()
    }
    fn default_thread_color() -> [u8; 3] {
        [255, 255, 255]
    }
    fn default_stitch_length() -> f64 {
        2.5
    }
    fn default_density() -> f64 {
        4.0
    }
    fn default_satin_width() -> f64 {
        3.0
    }
    fn default_letter_spacing() -> f64 {
        1.0
    }
    fn default_line_spacing() -> f64 {
        1.2
    }
    fn default_alignment() -> String {
        "left".into()
    }

    /// Convert a `SketchProfile` from text_to_profiles into a `Path2D`.
    ///
    /// Line segments contribute their start point; arcs are discretized into
    /// small line segments. The path is always marked as closed since glyph
    /// contours are closed loops.
    fn sketch_profile_to_path2d(
        profile: &vcad_kernel::vcad_kernel_sketch::SketchProfile,
    ) -> Path2D {
        let mut points: Vec<(f64, f64)> = Vec::new();
        for seg in &profile.segments {
            match seg {
                vcad_kernel::vcad_kernel_sketch::SketchSegment::Line { start, end } => {
                    if points.is_empty()
                        || (points.last().unwrap().0 - start.x).abs() > 1e-9
                        || (points.last().unwrap().1 - start.y).abs() > 1e-9
                    {
                        points.push((start.x, start.y));
                    }
                    points.push((end.x, end.y));
                }
                vcad_kernel::vcad_kernel_sketch::SketchSegment::Arc {
                    start,
                    end,
                    center,
                    ccw,
                } => {
                    // Discretize arc into line segments
                    if points.is_empty()
                        || (points.last().unwrap().0 - start.x).abs() > 1e-9
                        || (points.last().unwrap().1 - start.y).abs() > 1e-9
                    {
                        points.push((start.x, start.y));
                    }
                    let radius =
                        ((start.x - center.x).powi(2) + (start.y - center.y).powi(2)).sqrt();
                    let start_angle = (start.y - center.y).atan2(start.x - center.x);
                    let end_angle = (end.y - center.y).atan2(end.x - center.x);
                    let mut sweep = end_angle - start_angle;
                    if *ccw && sweep < 0.0 {
                        sweep += 2.0 * std::f64::consts::PI;
                    } else if !ccw && sweep > 0.0 {
                        sweep -= 2.0 * std::f64::consts::PI;
                    }
                    // ~1 segment per 10 degrees
                    let n_segs = ((sweep.abs() / (10.0_f64.to_radians())).ceil() as usize).max(2);
                    for i in 1..=n_segs {
                        let t = i as f64 / n_segs as f64;
                        let angle = start_angle + sweep * t;
                        points.push((
                            center.x + radius * angle.cos(),
                            center.y + radius * angle.sin(),
                        ));
                    }
                }
            }
        }
        Path2D {
            points,
            closed: true,
        }
    }

    /// Digitize text into embroidery stitches.
    ///
    /// Converts a text string into glyph outlines, then applies the specified
    /// stitch algorithm (running, satin, or fill) to produce an `EmbPattern`.
    /// Returns the same JSON shape as `readEmbroideryPes`.
    #[wasm_bindgen(js_name = digitizeText)]
    pub fn digitize_text(text: &str, height: f64, options_json: &str) -> Result<String, JsError> {
        use vcad_kernel::vcad_kernel_text::{FontRegistry, TextAlignment};

        let opts: DigitizeTextOptions =
            serde_json::from_str(options_json).map_err(|e| JsError::new(&e.to_string()))?;

        let align = match opts.alignment.as_str() {
            "center" => TextAlignment::Center,
            "right" => TextAlignment::Right,
            _ => TextAlignment::Left,
        };

        let font = FontRegistry::builtin_sans();
        let profiles = vcad_kernel::vcad_kernel_text::text_to_profiles(
            text,
            font,
            height,
            opts.letter_spacing,
            opts.line_spacing,
            align,
        );

        if profiles.is_empty() {
            return Err(JsError::new("Text produced no glyph outlines"));
        }

        let color = opts.color;
        let thread = Thread::new(color, "Thread 1");

        let mut all_commands: Vec<StitchCommand> = Vec::new();

        // Convert all profiles to paths up front.
        let paths: Vec<Path2D> = profiles
            .iter()
            .map(sketch_profile_to_path2d)
            .filter(|p| p.points.len() >= 2)
            .collect();

        if opts.stitch_type == "fill" {
            // Fill uses all contours together so even-odd rule subtracts holes.
            let cmds = fill_stitch_multi(
                &paths,
                &FillParams {
                    angle: opts.fill_angle,
                    row_spacing: 1.0 / opts.density.max(0.1),
                    stitch_length: opts.stitch_length,
                    stagger: 0.25,
                },
            );
            all_commands.extend(cmds);
        } else {
            // Running/satin: process each contour independently.
            for path in &paths {
                let cmds = match opts.stitch_type.as_str() {
                    "satin" => satin_stitch(
                        path,
                        &SatinParams {
                            width: opts.satin_width,
                            density: opts.density,
                            pull_compensation: 0.0,
                        },
                    ),
                    _ => running_stitch(
                        path,
                        &RunningStitchParams {
                            stitch_length: opts.stitch_length,
                        },
                    ),
                };

                if !cmds.is_empty() {
                    if !all_commands.is_empty() {
                        all_commands.push(StitchCommand::Trim);
                    }
                    all_commands.extend(cmds);
                }
            }
        }

        if all_commands.is_empty() {
            return Err(JsError::new("No stitches generated from text"));
        }

        // Flip Y: font coordinates are Y-up, embroidery renderer expects Y-down
        for cmd in &mut all_commands {
            match cmd {
                StitchCommand::MoveTo { y, .. }
                | StitchCommand::StitchTo { y, .. }
                | StitchCommand::Jump { y, .. } => {
                    *y = -*y;
                }
                _ => {}
            }
        }

        all_commands.push(StitchCommand::End);

        let pattern = EmbPattern {
            threads: vec![thread],
            stitch_groups: vec![StitchGroup {
                thread_index: 0,
                commands: all_commands,
            }],
            metadata: PatternMetadata {
                name: text.chars().take(50).collect(),
                author: String::new(),
                category: Some("Text".into()),
            },
        };

        serialize_pattern(&pattern).map_err(|e| JsError::new(&e))
    }

    /// Options for sketch digitization (subset of text options, no text-specific fields).
    #[derive(Deserialize)]
    struct DigitizeSketchOptions {
        #[serde(default = "default_stitch_type")]
        stitch_type: String,
        #[serde(default = "default_thread_color")]
        color: [u8; 3],
        #[serde(default = "default_stitch_length")]
        stitch_length: f64,
        #[serde(default = "default_density")]
        density: f64,
        #[serde(default = "default_satin_width")]
        satin_width: f64,
        #[serde(default)]
        fill_angle: f64,
    }

    /// Convert IR `SketchSegment2D` segments into a `Path2D`.
    fn sketch_segments_to_path2d(segments: &[vcad_ir::SketchSegment2D]) -> Path2D {
        let mut points: Vec<(f64, f64)> = Vec::new();
        for seg in segments {
            match seg {
                vcad_ir::SketchSegment2D::Line { start, end } => {
                    if points.is_empty()
                        || (points.last().unwrap().0 - start.x).abs() > 1e-9
                        || (points.last().unwrap().1 - start.y).abs() > 1e-9
                    {
                        points.push((start.x, start.y));
                    }
                    points.push((end.x, end.y));
                }
                vcad_ir::SketchSegment2D::Arc {
                    start,
                    end,
                    center,
                    ccw,
                } => {
                    if points.is_empty()
                        || (points.last().unwrap().0 - start.x).abs() > 1e-9
                        || (points.last().unwrap().1 - start.y).abs() > 1e-9
                    {
                        points.push((start.x, start.y));
                    }
                    let radius =
                        ((start.x - center.x).powi(2) + (start.y - center.y).powi(2)).sqrt();
                    let start_angle = (start.y - center.y).atan2(start.x - center.x);
                    let end_angle = (end.y - center.y).atan2(end.x - center.x);
                    let mut sweep = end_angle - start_angle;
                    if *ccw && sweep < 0.0 {
                        sweep += 2.0 * std::f64::consts::PI;
                    } else if !ccw && sweep > 0.0 {
                        sweep -= 2.0 * std::f64::consts::PI;
                    }
                    let n_segs = ((sweep.abs() / (10.0_f64.to_radians())).ceil() as usize).max(2);
                    for i in 1..=n_segs {
                        let t = i as f64 / n_segs as f64;
                        let angle = start_angle + sweep * t;
                        points.push((
                            center.x + radius * angle.cos(),
                            center.y + radius * angle.sin(),
                        ));
                    }
                }
            }
        }
        Path2D {
            points,
            closed: true,
        }
    }

    /// Digitize sketch segments into embroidery stitches.
    ///
    /// Takes a JSON array of `SketchSegment2D` (from a Sketch2D node) plus
    /// stitch options, and returns an `EmbPattern` JSON string.
    #[wasm_bindgen(js_name = digitizeSketch)]
    pub fn digitize_sketch(segments_json: &str, options_json: &str) -> Result<String, JsError> {
        let segments: Vec<vcad_ir::SketchSegment2D> =
            serde_json::from_str(segments_json).map_err(|e| JsError::new(&e.to_string()))?;

        if segments.is_empty() {
            return Err(JsError::new("No sketch segments provided"));
        }

        let opts: DigitizeSketchOptions =
            serde_json::from_str(options_json).map_err(|e| JsError::new(&e.to_string()))?;

        let path = sketch_segments_to_path2d(&segments);
        if path.points.len() < 2 {
            return Err(JsError::new("Sketch produced too few points"));
        }

        let color = opts.color;
        let thread = Thread::new(color, "Thread 1");

        let cmds = match opts.stitch_type.as_str() {
            "satin" => satin_stitch(
                &path,
                &SatinParams {
                    width: opts.satin_width,
                    density: opts.density,
                    pull_compensation: 0.0,
                },
            ),
            "fill" => fill_stitch(
                &path,
                &FillParams {
                    angle: opts.fill_angle,
                    row_spacing: 1.0 / opts.density.max(0.1),
                    stitch_length: opts.stitch_length,
                    stagger: 0.25,
                },
            ),
            _ => running_stitch(
                &path,
                &RunningStitchParams {
                    stitch_length: opts.stitch_length,
                },
            ),
        };

        if cmds.is_empty() {
            return Err(JsError::new("No stitches generated from sketch"));
        }

        let mut all_commands = cmds;
        all_commands.push(StitchCommand::End);

        let pattern = EmbPattern {
            threads: vec![thread],
            stitch_groups: vec![StitchGroup {
                thread_index: 0,
                commands: all_commands,
            }],
            metadata: PatternMetadata {
                name: "Sketch".into(),
                author: String::new(),
                category: Some("Sketch".into()),
            },
        };

        serialize_pattern(&pattern).map_err(|e| JsError::new(&e))
    }

    #[derive(Serialize)]
    struct EmbroideryResult {
        threads: Vec<ThreadInfo>,
        #[serde(rename = "stitchPaths")]
        stitch_paths: Vec<StitchPathInfo>,
        stats: StatsInfo,
        /// Serialized pattern JSON for round-trip export
        #[serde(rename = "patternJson")]
        pattern_json: String,
    }

    #[derive(Serialize)]
    struct ThreadInfo {
        color: [u8; 3],
        name: String,
    }

    #[derive(Serialize)]
    struct StitchPathInfo {
        #[serde(rename = "threadIndex")]
        thread_index: usize,
        color: [u8; 3],
        points: Vec<[f64; 2]>,
    }

    #[derive(Serialize)]
    struct StatsInfo {
        #[serde(rename = "stitchCount")]
        stitch_count: usize,
        #[serde(rename = "colorCount")]
        color_count: usize,
        width: f64,
        height: f64,
        #[serde(rename = "threadLength")]
        thread_length: f64,
        #[serde(rename = "estimatedTimeSeconds")]
        estimated_time_seconds: f64,
    }

    fn serialize_pattern(pattern: &EmbPattern) -> Result<String, String> {
        let stats = pattern.stats();

        let threads: Vec<ThreadInfo> = pattern
            .threads
            .iter()
            .map(|t| ThreadInfo {
                color: t.color,
                name: t.name.clone(),
            })
            .collect();

        let mut paths: Vec<StitchPathInfo> = Vec::new();
        for group in &pattern.stitch_groups {
            let thread = pattern
                .threads
                .get(group.thread_index)
                .cloned()
                .unwrap_or_else(|| Thread::new([128, 128, 128], "Unknown"));

            let mut points: Vec<[f64; 2]> = Vec::new();
            for cmd in &group.commands {
                match cmd {
                    StitchCommand::MoveTo { x, y } | StitchCommand::StitchTo { x, y } => {
                        points.push([*x, *y]);
                    }
                    StitchCommand::Jump { x, y } => {
                        if !points.is_empty() {
                            paths.push(StitchPathInfo {
                                thread_index: group.thread_index,
                                color: thread.color,
                                points: std::mem::take(&mut points),
                            });
                        }
                        points.push([*x, *y]);
                    }
                    StitchCommand::Trim | StitchCommand::End if !points.is_empty() => {
                        paths.push(StitchPathInfo {
                            thread_index: group.thread_index,
                            color: thread.color,
                            points: std::mem::take(&mut points),
                        });
                    }
                    _ => {}
                }
            }
            if !points.is_empty() {
                paths.push(StitchPathInfo {
                    thread_index: group.thread_index,
                    color: thread.color,
                    points,
                });
            }
        }

        let pattern_json = serde_json::to_string(pattern).map_err(|e| e.to_string())?;

        let result = EmbroideryResult {
            threads,
            stitch_paths: paths,
            stats: StatsInfo {
                stitch_count: stats.stitch_count,
                color_count: stats.color_count,
                width: stats.width,
                height: stats.height,
                thread_length: stats.thread_length,
                estimated_time_seconds: stats.estimated_time_seconds,
            },
            pattern_json,
        };

        serde_json::to_string(&result).map_err(|e| e.to_string())
    }
}

// =============================================================================
// TypeScript type generation (ts-rs)
// =============================================================================

#[cfg(all(test, feature = "ts-rs"))]
mod ts_tests {
    use super::*;

    /// Generate TypeScript type definitions.
    ///
    /// Run with: `cargo test --features ts-rs export_bindings -- --ignored`
    #[test]
    #[ignore = "requires --features ts-rs; produces bindings/ output, opt-in only"]
    fn export_bindings() {
        // Types are auto-exported via #[ts(export)] attribute
        // This test ensures all types compile correctly with ts-rs
        WasmMesh::export_all().expect("WasmMesh export failed");
        WasmSketchSegment::export_all().expect("WasmSketchSegment export failed");
        WasmSketchProfile::export_all().expect("WasmSketchProfile export failed");
        GpuGeometryResult::export_all().expect("GpuGeometryResult export failed");
        TextBoundsResult::export_all().expect("TextBoundsResult export failed");
    }
}
