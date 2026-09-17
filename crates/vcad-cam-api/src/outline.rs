//! Contours out of a solid, and the check that the contour is still the part.
//!
//! Friction-log items 16 and 37: the outline was a separate DXF nothing
//! compared against the loaded solid, and there was no way to get an outline
//! *out* of the solid at all — the one that was lost had to be regenerated
//! outside vcad with shapely.
//!
//! # Which mesh gets sectioned, and why it matters
//!
//! A part in an evaluated document carries two meshes' worth of geometry: the
//! mesh the app renders, which came from `vcad_kernel::Solid::to_mesh` — that
//! is `tessellate_brep` followed by `repair_export_mesh`, the *export*
//! boundary — and, where the root is still a B-rep, the solid itself.
//!
//! Measured on the stator (2026-09-17): the export mesh sections with tears up
//! to **0.4 mm** on the faces where tangent fillets meet, while the raw
//! tessellation of the same B-rep sections cleanly to **0.005 mm**. The repair
//! pass closes the boundary for printing and ray-tracing by moving vertices
//! onto their analytic carriers; a plane through the moved region then cuts a
//! slightly different shape. A CAM contour is a wall the cutter follows, so
//! 0.4 mm is not a rounding difference — it is a quarter of a slot mouth.
//!
//! So: **section the raw tessellation whenever there is a B-rep behind the
//! part**, and say which was used in `mesh_source`. A part with no B-rep — an
//! imported mesh, or a **root-mesh cache hit**, which lands in the scene as a
//! mesh with `solid: None` because the cache stores triangles and not
//! topology — can only be sectioned as it stands. That is not wrong, only
//! less exact, and the response says so rather than letting the caller assume
//! the better path was taken.
//!
//! Which mesh to hand over is the *caller's* decision, because only the caller
//! holds the solid: the C ABI reaches a scene part, the WASM kernel reaches a
//! solid handle. Both tessellate the B-rep themselves and call
//! [`section_mesh`] with the `mesh_source` they used, which is the same code
//! path [`from_mesh_request`] runs.

use serde::Deserialize;
use serde_json::{json, Value};

use vcad_kernel_cam::outline::{
    compare_outlines, is_prismatic, read_dxf, section_at_z, CompareOptions, Loop, Outline,
    OutlineError, SectionOptions,
};
use vcad_kernel_cam::Point2D;

use crate::types::{loop_points, positive};

// ---------------------------------------------------------------------------
// Options shared by both sectioning entry points
// ---------------------------------------------------------------------------

/// `{ "z": 3.05, "auto_z": true, "weld_tolerance": 1e-6, "heal_tolerance": 1e-3,
///    "prismatic": true, "prismatic_tolerance": 0.01, "circle_tolerance": 1e-3,
///    "segments": 64 }`
#[derive(Debug, Clone, Default, Deserialize)]
struct SectionReq {
    #[serde(default)]
    z: Option<f64>,
    #[serde(default)]
    auto_z: Option<bool>,
    #[serde(default)]
    weld_tolerance: Option<f64>,
    /// Stays at the kernel's 1e-3 by default. A section that does not close is
    /// the signal from friction-log item 30 — a degraded solid — and healing
    /// it away would hide exactly the thing worth seeing.
    #[serde(default)]
    heal_tolerance: Option<f64>,
    #[serde(default)]
    prismatic: Option<bool>,
    #[serde(default)]
    prismatic_tolerance: Option<f64>,
    #[serde(default)]
    circle_tolerance: Option<f64>,
    /// Curve resolution when the raw tessellation has to be rebuilt.
    #[serde(default)]
    segments: Option<u32>,
}

impl SectionReq {
    fn options(&self) -> Result<SectionOptions, String> {
        let mut opts = SectionOptions::default();
        if let Some(w) = self.weld_tolerance {
            opts.weld_tolerance = positive("weld_tolerance", w)?;
        }
        if let Some(h) = self.heal_tolerance {
            opts.heal_tolerance = positive("heal_tolerance", h)?;
        }
        Ok(opts)
    }
}

/// `{ "positions": [[x,y,z], …], "indices": [0,1,2, …], … section options }`
///
/// `positions` also reads a flat `[x,y,z,x,y,z, …]` array, which is what the
/// TypeScript and Swift sides already hold.
#[derive(Debug, Clone, Deserialize)]
struct MeshRequest {
    positions: Positions,
    indices: Vec<u32>,
    #[serde(flatten)]
    section: SectionReq,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Positions {
    Triples(Vec<[f64; 3]>),
    Flat(Vec<f64>),
}

impl Positions {
    fn triples(self) -> Result<Vec<[f64; 3]>, String> {
        let out = match self {
            Positions::Triples(v) => v,
            Positions::Flat(v) => {
                if v.len() % 3 != 0 {
                    return Err(format!(
                        "positions has {} numbers, which is not a whole number of [x, y, z] triples.",
                        v.len()
                    ));
                }
                v.as_chunks::<3>().0.to_vec()
            }
        };
        for (i, p) in out.iter().enumerate() {
            if !p[0].is_finite() || !p[1].is_finite() || !p[2].is_finite() {
                return Err(format!(
                    "positions[{i}] is ({}, {}, {}); every coordinate has to be a finite number.",
                    p[0], p[1], p[2]
                ));
            }
        }
        Ok(out)
    }
}

/// Section a mesh handed over inline.
pub fn from_mesh_request(input: &str) -> Result<Value, String> {
    let req: MeshRequest = serde_json::from_str(input).map_err(|e| {
        format!("the section request could not be read: {e}. Expected positions and indices.")
    })?;
    let positions = req.positions.clone().triples()?;
    section(&positions, &req.indices, &req.section, "inline")
}

/// Section a mesh the caller produced itself, saying where it came from.
///
/// This is the seam the C ABI's scene and the WASM kernel's solid handle both
/// come through: they hold the topology, so they choose the mesh (see the
/// module notes — the raw tessellation, never the export mesh, whenever there
/// is a B-rep) and this does the rest.
///
/// `z` is used when `auto_z` is false; when it is true the mid-height of the
/// mesh's own Z range is used and `z` is ignored. `options` is the same
/// document [`from_mesh_request`] reads, minus `positions` and `indices`, and
/// may be `"{}"`.
///
/// `curve_segments` reads back the curve resolution the caller tessellated at,
/// so the answer records it; pass `None` when it is not known.
pub fn section_mesh(
    positions: &[[f64; 3]],
    indices: &[u32],
    z: f64,
    auto_z: bool,
    options: &str,
    mesh_source: &str,
) -> Result<Value, String> {
    let text = if options.trim().is_empty() {
        "{}"
    } else {
        options
    };
    let mut req: SectionReq = serde_json::from_str(text)
        .map_err(|e| format!("the section options could not be read: {e}."))?;
    if auto_z {
        req.auto_z = Some(true);
    } else {
        req.z = Some(z);
        req.auto_z = Some(false);
    }
    section(positions, indices, &req, mesh_source)
}

/// The curve resolution a caller should tessellate a B-rep at before calling
/// [`section_mesh`], read off the same `segments` option the inline request
/// carries. Defaults to 64 — fine enough that a Ø2.5 bore sections to within a
/// few microns of round.
pub fn section_segments(options: &str) -> Result<u32, String> {
    let text = if options.trim().is_empty() {
        "{}"
    } else {
        options
    };
    let req: SectionReq = serde_json::from_str(text)
        .map_err(|e| format!("the section options could not be read: {e}."))?;
    match req.segments {
        Some(0) => Err("segments is 0: a curve needs at least one segment.".into()),
        Some(n) => Ok(n),
        None => Ok(64),
    }
}

// ---------------------------------------------------------------------------
// The section itself
// ---------------------------------------------------------------------------

fn section(
    positions: &[[f64; 3]],
    indices: &[u32],
    req: &SectionReq,
    mesh_source: &str,
) -> Result<Value, String> {
    if positions.is_empty() || indices.is_empty() {
        return Err(format!(
            "there is nothing to section: {} vertices and {} indices.",
            positions.len(),
            indices.len()
        ));
    }
    let (z_min, z_max) = positions.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
        (lo.min(p[2]), hi.max(p[2]))
    });
    let auto = req.auto_z.unwrap_or(req.z.is_none());
    let z = if auto {
        (z_min + z_max) / 2.0
    } else {
        crate::types::finite(
            "z",
            req.z
                .ok_or("no z was given and auto_z is off: say where to section.")?,
        )?
    };
    let opts = req.options()?;

    let outline = match section_at_z(positions, indices, z, &opts) {
        Ok(o) => o,
        // An open section is the signal, not a failure to smooth over: it
        // means the solid is torn there. Hand the gaps back so the caller can
        // show where, rather than a sentence that loses them.
        Err(OutlineError::OpenSection { z, gaps }) => {
            return Ok(json!({
                "error": format!(
                    "the section at z {z:.4} does not close: {} gap(s), the widest {:.4} mm. This part is torn there — heal the solid rather than the outline.",
                    gaps.len(),
                    gaps.iter().map(|g| g.distance).fold(0.0f64, f64::max)
                ),
                "mesh_source": mesh_source,
                "z": z,
                "gaps": gaps,
            }))
        }
        Err(e) => return Err(format!("the part could not be sectioned: {e}")),
    };

    let circle_tolerance = match req.circle_tolerance {
        Some(t) => positive("circle_tolerance", t)?,
        None => 1e-3,
    };
    let circles: Vec<Value> = outline
        .circular_holes(circle_tolerance)
        .iter()
        .map(|h| {
            json!({
                "region": h.region,
                "hole": h.hole,
                "center": [h.fit.center.x, h.fit.center.y],
                "diameter": h.fit.diameter(),
                "rms_error": h.fit.rms_error,
                "max_error": h.fit.max_error,
            })
        })
        .collect();

    let prismatic = if req.prismatic.unwrap_or(true) {
        let tol = match req.prismatic_tolerance {
            Some(t) => positive("prismatic_tolerance", t)?,
            None => 0.01,
        };
        match is_prismatic(positions, indices, z_min, z_max, tol) {
            Ok(r) => serde_json::to_value(r).unwrap_or(Value::Null),
            // A refusal here is information, not a failure of the section:
            // "this part is not constant-section" is exactly what a contour
            // job needs to hear.
            Err(e) => json!({ "refused": e.to_string() }),
        }
    } else {
        Value::Null
    };

    let regions: Vec<Value> = outline
        .regions
        .iter()
        .map(|r| {
            json!({
                "outer": points(&r.outer),
                "holes": r.holes.iter().map(points).collect::<Vec<_>>(),
                "area": r.area(),
            })
        })
        .collect();

    Ok(json!({
        "mesh_source": mesh_source,
        "z": outline.z,
        "auto_z": auto,
        "z_range": [z_min, z_max],
        "suggested_stock_thickness": z_max - z_min,
        "plane_nudge": outline.plane_nudge,
        "healed": outline.healed,
        "discarded_slivers": outline.discarded_slivers,
        "regions": regions,
        "circles": circles,
        "bounds": outline.bounds(),
        "area": outline.area(),
        "prismatic": prismatic,
        "outline": outline,
    }))
}

fn points(l: &Loop) -> Vec<[f64; 2]> {
    l.points.iter().map(|p| [p.x, p.y]).collect()
}

// ---------------------------------------------------------------------------
// vcad_cam_compare_outline
// ---------------------------------------------------------------------------

/// One side of the comparison: a DXF's text, explicit loops, or an `outline`
/// document straight back from `vcad_cam_outline_from_*`.
#[derive(Debug, Clone, Default, Deserialize)]
struct SourceReq {
    #[serde(default)]
    dxf: Option<String>,
    #[serde(default)]
    loops: Option<Vec<Vec<[f64; 2]>>>,
    #[serde(default)]
    outline: Option<Outline>,
}

impl SourceReq {
    fn is_empty(&self) -> bool {
        self.dxf.is_none() && self.loops.is_none() && self.outline.is_none()
    }

    fn build(&self, what: &str) -> Result<Outline, String> {
        if let Some(text) = &self.dxf {
            return read_dxf(text).map_err(|e| format!("{what}: the DXF could not be read: {e}"));
        }
        if let Some(loops) = &self.loops {
            if loops.is_empty() {
                return Err(format!("{what}.loops is empty."));
            }
            let mut built = Vec::with_capacity(loops.len());
            for (i, l) in loops.iter().enumerate() {
                let pts = loop_points(&format!("{what}.loops[{i}]"), l)?;
                built.push(Loop::new(
                    pts.iter().map(|p| Point2D::new(p[0], p[1])).collect(),
                ));
            }
            return Ok(Outline::from_loops(built));
        }
        match &self.outline {
            Some(o) => Ok(o.clone()),
            None => Err(format!(
                "{what} is empty: give a \"dxf\", \"loops\" or an \"outline\"."
            )),
        }
    }
}

/// `{ "a": {dxf|loops|outline}, "b": {…}, "tolerance": 0.02,
///    "hole_match_tolerance": 0.5 }`
///
/// `dxf` and `outline` may also sit at the top level, which reads as *this
/// DXF* against *this sectioned solid* — item 16's question, spelled the way
/// it is asked.
#[derive(Debug, Clone, Deserialize)]
struct CompareRequest {
    #[serde(default)]
    a: SourceReq,
    #[serde(default)]
    b: SourceReq,
    #[serde(default)]
    dxf: Option<String>,
    #[serde(default)]
    loops: Option<Vec<Vec<[f64; 2]>>>,
    #[serde(default)]
    outline: Option<Outline>,
    #[serde(default)]
    tolerance: Option<f64>,
    #[serde(default)]
    hole_match_tolerance: Option<f64>,
}

/// Compare two outlines and say whether they agree.
pub fn compare_request(input: &str) -> Result<Value, String> {
    let req: CompareRequest = serde_json::from_str(input)
        .map_err(|e| format!("the comparison request could not be read: {e}."))?;
    let mut a = req.a.clone();
    if a.is_empty() {
        a.dxf = req.dxf.clone();
        a.loops = req.loops.clone();
    }
    let mut b = req.b.clone();
    if b.is_empty() {
        b.outline = req.outline.clone();
    }
    let a = a.build("a")?;
    let b = b.build("b")?;

    let mut opts = CompareOptions::default();
    if let Some(t) = req.hole_match_tolerance {
        opts.hole_match_tolerance = positive("hole_match_tolerance", t)?;
    }
    let tolerance = match req.tolerance {
        Some(t) => positive("tolerance", t)?,
        None => 0.02,
    };
    let diff = compare_outlines(&a, &b, &opts);
    let agrees = diff.agrees_within(tolerance);
    Ok(json!({
        "agrees": agrees,
        "tolerance": tolerance,
        "diff": diff,
        "note": if agrees {
            format!(
                "the two outlines agree: nothing further apart than {:.4} mm and every hole matched.",
                diff.max_boundary_distance
            )
        } else {
            format!(
                "these are not the same outline: boundaries up to {:.4} mm apart (tolerance {tolerance:.4}), {} hole(s) on one side only. Machining one as if it were the other cuts the wrong part.",
                diff.max_boundary_distance,
                diff.unmatched_a.len() + diff.unmatched_b.len()
            )
        },
    }))
}
