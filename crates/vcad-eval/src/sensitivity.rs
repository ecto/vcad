//! Document-parameter sensitivities: the layer that turns
//! `d(quantity)/d(named parameter)` into something a person can read and
//! an optimizer can trust.
//!
//! [`crate::diff`] already computes the derivatives. What it does not do
//! is say what they *mean*: which route produced them, what units they
//! carry, and — the part that matters most in a CAD system — **where
//! they stop being true**.
//!
//! # The trust radius, found rather than assumed
//!
//! SU2 differentiates a mesh. Its parameterization (free-form deformation
//! boxes) is smooth by construction, so the question "over what range is
//! this derivative meaningful?" barely arises. vcad differentiates the
//! *feature tree*, which is strictly more useful and not smooth at all: a
//! fillet whose radius exceeds its edge stops existing, a boolean can
//! change face count, a pattern can gain an instance. Across any of those
//! the derivative is not inaccurate, it is describing a different solid.
//!
//! [`topology_trust_radius`] does not assume a radius or ask the author
//! for one. It **searches** for the boundary: bisecting outward from θ₀
//! until the evaluated document's topology signature — part count, and
//! per-part face/edge/vertex counts — changes. What comes back is the
//! interval over which the parameterization still describes the same
//! solid, tagged [`TrustLimit::TopologyStable`].
//!
//! That number is the difference between a gradient and a gradient a user
//! can act on. "d(mass)/d(wall) = 2.4 g/mm" invites a 5 mm move; "…, valid
//! for wall ∈ [1.2, 3.1]" does not.
//!
//! # Routes
//!
//! | quantity | route | why |
//! |---|---|---|
//! | volume, mass, centroid | [`Route::Dual`] | exact seam derivatives — dual numbers through the mass-property integrals |
//! | bounding-box extent | [`Route::FiniteDifference`] | an extent is a max over vertices; non-smooth, so the seam cannot price it |
//!
//! **Honesty:** every derivative here is a derivative of the *frozen
//! tessellation* of the document, which is what the seam is built on. The
//! trust radius covers topology changes in the BRep; it does not cover a
//! tessellation that re-triangulates without changing BRep topology (the
//! seam's frozen plan pins that, and reports its own error if the plan
//! stops applying). Parameters that are structurally discrete — an
//! instance count, a hole count — have no derivative at all on this path
//! and are reported as such rather than given a fabricated zero.

use std::collections::BTreeMap;

use vcad_ir::{Document, Expr};
use vcad_kernel_adjoint::{Route, Sensitivity, SensitivityTable, TrustLimit, TrustRadius};
use vcad_kernel_tessellate::TessellationParams;

use crate::diff::{document_parameter_gradient, DocDiffError};
use crate::{evaluate_document, EvalOptions};

/// A quantity of interest computable from a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qoi {
    /// Signed volume, mm³.
    Volume,
    /// Mass, in whatever unit the supplied density implies.
    Mass,
    /// One axis of the centroid, mm.
    CentroidAxis(usize),
    /// One axis of the axis-aligned bounding-box extent, mm.
    BboxExtent(usize),
}

impl Qoi {
    /// Unit of the quantity itself.
    pub fn unit(&self) -> &'static str {
        match self {
            Qoi::Volume => "mm^3",
            Qoi::Mass => "g",
            Qoi::CentroidAxis(_) | Qoi::BboxExtent(_) => "mm",
        }
    }

    /// Whether the exact seam can price this quantity.
    fn is_exact(&self) -> bool {
        !matches!(self, Qoi::BboxExtent(_))
    }

    fn axis_label(a: usize) -> &'static str {
        ["x", "y", "z"][a.min(2)]
    }

    /// Stable name, scoped to a part or the whole document.
    pub fn name(&self, part: Option<usize>) -> String {
        let base = match self {
            Qoi::Volume => "volume".to_string(),
            Qoi::Mass => "mass".to_string(),
            Qoi::CentroidAxis(a) => format!("centroid_{}", Self::axis_label(*a)),
            Qoi::BboxExtent(a) => format!("bbox_{}", Self::axis_label(*a)),
        };
        match part {
            Some(i) => format!("part{i}.{base}"),
            None => base,
        }
    }
}

/// A quantity plus the scope it is measured over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QoiRequest {
    /// The quantity.
    pub qoi: Qoi,
    /// Part index, or `None` for the whole document (volume and mass sum;
    /// the centroid is mass-weighted; the bounding box is the union).
    pub part: Option<usize>,
}

impl QoiRequest {
    /// A document-scoped request.
    pub fn document(qoi: Qoi) -> Self {
        QoiRequest { qoi, part: None }
    }

    /// A part-scoped request.
    pub fn part(qoi: Qoi, index: usize) -> Self {
        QoiRequest {
            qoi,
            part: Some(index),
        }
    }

    /// Parse a quantity name: `volume`, `mass`, `centroid_x|y|z`,
    /// `bbox_x|y|z`. Case-insensitive.
    pub fn parse(name: &str, part: Option<usize>) -> Option<Self> {
        let n = name.trim().to_ascii_lowercase();
        let axis = |s: &str| match s {
            "x" => Some(0),
            "y" => Some(1),
            "z" => Some(2),
            _ => None,
        };
        let qoi = match n.as_str() {
            "volume" => Qoi::Volume,
            "mass" => Qoi::Mass,
            other => match other.split_once('_') {
                Some(("centroid", a)) => Qoi::CentroidAxis(axis(a)?),
                Some(("bbox", a)) => Qoi::BboxExtent(axis(a)?),
                _ => return None,
            },
        };
        Some(QoiRequest { qoi, part })
    }

    /// Every quantity name [`Self::parse`] accepts.
    pub const NAMES: [&'static str; 8] = [
        "volume",
        "mass",
        "centroid_x",
        "centroid_y",
        "centroid_z",
        "bbox_x",
        "bbox_y",
        "bbox_z",
    ];
}

/// Options for a sensitivity sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensitivityOptions {
    /// Density fed to the mass-property integrals. Geometry is in mm, so
    /// pass `g/cm³ · 1e-3` for grams.
    pub density: f64,
    /// Probe step for seeding synthesis (matching surfaces between θ and
    /// θ ± h). The returned mass-property derivatives are analytic; this
    /// only pairs surfaces up.
    pub probe_step: f64,
    /// How far out, relative to |θ| (floored at 1 mm), to search for the
    /// topology boundary before declaring the neighbourhood stable.
    pub topology_reach: f64,
    /// Bisection refinements when a topology boundary is found inside the
    /// reach. Each costs one document evaluation per side.
    pub topology_refinements: usize,
    /// Whether to search for the topology boundary at all. Off, every row
    /// falls back to the author's declared scrub bounds.
    pub find_topology_radius: bool,
}

impl Default for SensitivityOptions {
    fn default() -> Self {
        SensitivityOptions {
            density: 1.0,
            probe_step: 1e-4,
            topology_reach: 0.5,
            topology_refinements: 6,
            find_topology_radius: true,
        }
    }
}

/// Why a sensitivity sweep failed.
#[derive(Debug)]
pub enum SensitivityError {
    /// Underlying differentiation failed.
    Diff(DocDiffError),
    /// The parameter is not in the document.
    UnknownParameter(String),
    /// A requested part index does not exist.
    UnknownPart {
        /// The index asked for.
        index: usize,
        /// How many solid parts the document has.
        available: usize,
    },
    /// Parameter resolution failed.
    Resolve(String),
    /// A requested quantity name is not one this layer knows.
    UnknownQuantity {
        /// The name asked for.
        name: String,
        /// The names that would work.
        known: String,
    },
}

impl std::fmt::Display for SensitivityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SensitivityError::Diff(e) => write!(f, "{e}"),
            SensitivityError::UnknownParameter(p) => write!(f, "unknown parameter {p:?}"),
            SensitivityError::UnknownPart { index, available } => write!(
                f,
                "part {index} does not exist ({available} solid part(s) in the document)"
            ),
            SensitivityError::Resolve(e) => write!(f, "parameter resolution: {e}"),
            SensitivityError::UnknownQuantity { name, known } => {
                write!(f, "unknown quantity {name:?}; known quantities are {known}")
            }
        }
    }
}

impl std::error::Error for SensitivityError {}

impl From<DocDiffError> for SensitivityError {
    fn from(e: DocDiffError) -> Self {
        SensitivityError::Diff(e)
    }
}

/// A document's topology signature: part count plus per-part
/// (faces, edges).
///
/// Coarse on purpose. It has to be cheap enough to evaluate a dozen times
/// during a bisection, and it has to catch the changes that actually
/// invalidate a parameterization — a face appearing, an edge collapsing,
/// a part splitting. It does not catch a boolean that reshuffles the same
/// counts; the frozen plan's own structural check is the backstop there.
///
/// **Vertices are deliberately excluded.** vcad's boolean rims are
/// sag-adaptive: a bigger hole gets a denser rim, so the vertex count of
/// a plate with a through-hole climbs smoothly with the radius (252 at
/// r = 3, 452 at r = 9.9) while nothing topological happens at all.
/// Including vertices would make every continuous parameter look like it
/// crosses a topology boundary immediately, and the trust radius would
/// collapse to the search resolution. Faces and edges hold flat across
/// that same sweep and then jump — 7 faces and 13 edges throughout,
/// 20 and 20 once the hole breaks through the wall — which is the
/// signal we want.
pub type TopologySignature = Vec<(usize, usize)>;

/// Evaluate the document with one parameter overridden and read its
/// topology signature.
pub fn topology_signature(
    doc: &Document,
    parameter: &str,
    value: f64,
) -> Result<TopologySignature, SensitivityError> {
    let mut d = doc.clone();
    match d.parameters.get_mut(parameter) {
        Some(p) => p.value = Expr::Number(value),
        None => return Err(SensitivityError::UnknownParameter(parameter.to_string())),
    }
    let options = EvalOptions {
        skip_clash_detection: true,
        clock: None,
    };
    let scene = evaluate_document(&d, &options).map_err(DocDiffError::from)?;
    Ok(scene
        .parts
        .iter()
        .filter_map(|p| p.solid.as_ref().and_then(|s| s.as_brep()))
        .map(|b| (b.topology.faces.len(), b.topology.edges.len()))
        .collect())
}

/// Find the interval around θ₀ over which the document's topology is
/// unchanged.
///
/// Searches outward to `reach · max(|θ₀|, 1)` on each side; if the
/// signature still matches there, returns `None` — no topology limit was
/// found *within the reach*, which is a statement about the search, not a
/// promise about infinity. Otherwise bisects `refinements` times per side
/// and returns the largest verified-stable interval.
///
/// An evaluation that *errors* counts as a boundary. A parameter value
/// that makes the document fail to build is exactly as far as the
/// derivative can be trusted.
pub fn topology_trust_radius(
    doc: &Document,
    parameter: &str,
    theta0: f64,
    reach: f64,
    refinements: usize,
) -> Option<TrustRadius> {
    let base = topology_signature(doc, parameter, theta0).ok()?;
    let span = reach * theta0.abs().max(1.0);
    if span <= 0.0 {
        return None;
    }

    // Largest stable offset in one direction, by bisection.
    let edge = |sign: f64| -> Option<f64> {
        let stable = |d: f64| {
            topology_signature(doc, parameter, theta0 + sign * d)
                .map(|s| s == base)
                .unwrap_or(false)
        };
        if stable(span) {
            return None; // no boundary within reach
        }
        let (mut lo, mut hi) = (0.0_f64, span);
        for _ in 0..refinements {
            let mid = 0.5 * (lo + hi);
            if stable(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(lo)
    };

    let up = edge(1.0);
    let down = edge(-1.0);
    if up.is_none() && down.is_none() {
        return None;
    }
    TrustRadius::new(
        theta0 - down.unwrap_or(span),
        theta0 + up.unwrap_or(span),
        TrustLimit::TopologyStable,
    )
}

/// Axis-aligned bounding box of an evaluated scene's solid parts, and of
/// each part individually.
fn bboxes(
    doc: &Document,
    parameter: &str,
    value: f64,
) -> Result<(Vec<[f64; 3]>, [f64; 3]), SensitivityError> {
    let mut d = doc.clone();
    match d.parameters.get_mut(parameter) {
        Some(p) => p.value = Expr::Number(value),
        None => return Err(SensitivityError::UnknownParameter(parameter.to_string())),
    }
    let options = EvalOptions {
        skip_clash_detection: true,
        clock: None,
    };
    let scene = evaluate_document(&d, &options).map_err(DocDiffError::from)?;

    let mut per_part = Vec::new();
    let mut umin = [f64::INFINITY; 3];
    let mut umax = [f64::NEG_INFINITY; 3];
    for p in scene
        .parts
        .iter()
        .filter(|p| p.solid.as_ref().and_then(|s| s.as_brep()).is_some())
    {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for v in p.mesh.positions.as_chunks::<3>().0 {
            for a in 0..3 {
                let c = v[a] as f64;
                if c < min[a] {
                    min[a] = c;
                }
                if c > max[a] {
                    max[a] = c;
                }
                if c < umin[a] {
                    umin[a] = c;
                }
                if c > umax[a] {
                    umax[a] = c;
                }
            }
        }
        per_part.push([
            (max[0] - min[0]).max(0.0),
            (max[1] - min[1]).max(0.0),
            (max[2] - min[2]).max(0.0),
        ]);
    }
    let union = [
        (umax[0] - umin[0]).max(0.0),
        (umax[1] - umin[1]).max(0.0),
        (umax[2] - umin[2]).max(0.0),
    ];
    Ok((per_part, union))
}

/// Aggregate mass properties across parts: `(value, derivative)` for
/// volume, mass, and the mass-weighted centroid.
struct Aggregate {
    volume: (f64, f64),
    mass: (f64, f64),
    centroid: ([f64; 3], [f64; 3]),
}

fn aggregate(grads: &[crate::diff::DocumentPartGradient]) -> Aggregate {
    let mut v = (0.0, 0.0);
    let mut m = (0.0, 0.0);
    let mut mc = ([0.0; 3], [0.0; 3]);
    for g in grads {
        v.0 += g.properties.volume;
        v.1 += g.derivative.volume;
        m.0 += g.properties.mass;
        m.1 += g.derivative.mass;
        let c = [
            g.properties.centroid.x,
            g.properties.centroid.y,
            g.properties.centroid.z,
        ];
        let dc = [
            g.derivative.centroid.x,
            g.derivative.centroid.y,
            g.derivative.centroid.z,
        ];
        for a in 0..3 {
            mc.0[a] += g.properties.mass * c[a];
            // d(m·c) = dm·c + m·dc
            mc.1[a] += g.derivative.mass * c[a] + g.properties.mass * dc[a];
        }
    }
    // Quotient rule for the mass-weighted centroid.
    let mut centroid = ([0.0; 3], [0.0; 3]);
    if m.0.abs() > 0.0 {
        for a in 0..3 {
            centroid.0[a] = mc.0[a] / m.0;
            centroid.1[a] = (mc.1[a] * m.0 - mc.0[a] * m.1) / (m.0 * m.0);
        }
    }
    Aggregate {
        volume: v,
        mass: m,
        centroid,
    }
}

/// Differentiate a set of quantities with respect to a set of named
/// document parameters, producing a ranked, trust-bounded table.
///
/// One call to the seam per parameter prices every exact quantity at
/// once — that is the adjoint-shaped part of the cost. Bounding-box
/// extents add two document evaluations per parameter, and the topology
/// search adds a bounded handful more.
pub fn document_sensitivities(
    doc: &Document,
    parameters: &[String],
    qois: &[QoiRequest],
    tess: &TessellationParams,
    opts: &SensitivityOptions,
) -> Result<SensitivityTable, SensitivityError> {
    let env = vcad_ir::resolve_parameters(&doc.parameters)
        .map_err(|e| SensitivityError::Resolve(e.to_string()))?;

    let mut table = SensitivityTable::new();
    for name in parameters {
        let theta0 = *env
            .get(name.as_str())
            .ok_or_else(|| SensitivityError::UnknownParameter(name.clone()))?;
        let meta = doc
            .parameters
            .get(name.as_str())
            .ok_or_else(|| SensitivityError::UnknownParameter(name.clone()))?;
        let param_unit = meta.unit.clone().unwrap_or_else(|| "1".to_string());

        // Trust radius: the tighter of what the author declared and what
        // the geometry actually supports.
        let bounds = match (meta.min, meta.max) {
            (Some(lo), Some(hi)) => TrustRadius::from_bounds(lo, hi),
            _ => None,
        };
        let topo = if opts.find_topology_radius {
            topology_trust_radius(
                doc,
                name,
                theta0,
                opts.topology_reach,
                opts.topology_refinements,
            )
        } else {
            None
        };
        let trust = TrustRadius::tighter(bounds, topo);

        // One seam pass: every exact quantity for this parameter.
        let grads = document_parameter_gradient(doc, name, opts.density, tess, opts.probe_step)?;
        let agg = aggregate(&grads);

        // Bounding boxes, if anything asked for them.
        let want_bbox = qois.iter().any(|q| matches!(q.qoi, Qoi::BboxExtent(_)));
        let bbox_step = opts.probe_step.max(theta0.abs() * 1e-3).max(1e-3);
        let bbox = if want_bbox {
            let base = bboxes(doc, name, theta0)?;
            let plus = bboxes(doc, name, theta0 + bbox_step)?;
            let minus = bboxes(doc, name, theta0 - bbox_step)?;
            Some((base, plus, minus))
        } else {
            None
        };

        for req in qois {
            if let Some(i) = req.part {
                if i >= grads.len() {
                    return Err(SensitivityError::UnknownPart {
                        index: i,
                        available: grads.len(),
                    });
                }
            }
            let (value, route) = match (req.qoi, req.part) {
                (Qoi::Volume, None) => (agg.volume.1, Route::Dual),
                (Qoi::Volume, Some(i)) => (grads[i].derivative.volume, Route::Dual),
                (Qoi::Mass, None) => (agg.mass.1, Route::Dual),
                (Qoi::Mass, Some(i)) => (grads[i].derivative.mass, Route::Dual),
                (Qoi::CentroidAxis(a), None) => (agg.centroid.1[a.min(2)], Route::Dual),
                (Qoi::CentroidAxis(a), Some(i)) => {
                    let d = grads[i].derivative.centroid;
                    ([d.x, d.y, d.z][a.min(2)], Route::Dual)
                }
                (Qoi::BboxExtent(a), scope) => {
                    let (_, plus, minus) = bbox.as_ref().expect("bbox requested");
                    let a = a.min(2);
                    let (p, m) = match scope {
                        None => (plus.1[a], minus.1[a]),
                        Some(i) => (
                            *plus.0.get(i).map(|e| &e[a]).unwrap_or(&f64::NAN),
                            *minus.0.get(i).map(|e| &e[a]).unwrap_or(&f64::NAN),
                        ),
                    };
                    (
                        (p - m) / (2.0 * bbox_step),
                        Route::FiniteDifference { step: bbox_step },
                    )
                }
            };

            let mut row = Sensitivity::new(
                name.clone(),
                req.qoi.name(req.part),
                value,
                format!("{}/{}", req.qoi.unit(), param_unit),
                theta0,
                route,
            )
            .with_trust(trust);
            if !req.qoi.is_exact() {
                row = row.with_note(
                    "bounding-box extents are a max over vertices — non-smooth, so this row is a \
                     central finite difference rather than a seam derivative",
                );
            }
            if trust.map(|t| t.limited_by) == Some(TrustLimit::TopologyStable) {
                row = row.with_note(
                    "trust radius found by search: the document's topology signature changes \
                     outside this interval",
                );
            }
            table.push(row);
        }
    }
    Ok(table)
}

/// Rank every document parameter by how much it commands one quantity.
///
/// The ordering behind a feature tree sorted by influence. Parameters
/// whose trust radius could not be established sort last: an influence
/// computed over an unstated range is not comparable to one computed over
/// a stated range.
pub fn rank_parameters(
    doc: &Document,
    qoi: QoiRequest,
    tess: &TessellationParams,
    opts: &SensitivityOptions,
) -> Result<Vec<(String, f64, Option<f64>)>, SensitivityError> {
    let names: Vec<String> = doc.parameters.keys().cloned().collect();
    let table = document_sensitivities(doc, &names, &[qoi], tess, opts)?;
    let objective = qoi.qoi.name(qoi.part);
    Ok(table
        .ranked_for(&objective)
        .into_iter()
        .map(|r| (r.parameter.clone(), r.value, r.influence()))
        .collect())
}

/// Group a table by objective for display: objective → rows, ranked.
pub fn by_objective(table: &SensitivityTable) -> BTreeMap<String, Vec<&Sensitivity>> {
    let mut out: BTreeMap<String, Vec<&Sensitivity>> = BTreeMap::new();
    for obj in table.objectives() {
        out.insert(obj.to_string(), table.ranked_for(obj));
    }
    out
}

/// A whole sensitivity request, deserializable straight off the WASM / MCP
/// boundary.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SensitivityRequest {
    /// Parameters to differentiate. Empty or absent means every named
    /// parameter the document declares.
    pub parameters: Option<Vec<String>>,
    /// Quantity names ([`QoiRequest::NAMES`]). Absent means volume + mass.
    pub quantities: Option<Vec<String>>,
    /// Part index, or absent for the whole document.
    pub part: Option<usize>,
    /// Density for the mass integrals.
    pub density: Option<f64>,
    /// Seeding-synthesis probe step.
    pub probe_step: Option<f64>,
    /// Whether to search for the topology trust radius.
    pub find_trust_radius: Option<bool>,
    /// How far the topology search reaches, relative to |θ|.
    pub topology_reach: Option<f64>,
}

/// The answer to a sensitivity request: the table, a rendered view of it,
/// the ranking, and receipt claims.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SensitivityReport {
    /// Every row.
    pub table: SensitivityTable,
    /// Fixed-width rendering — what an agent should read first.
    pub rendered: String,
    /// Objective → parameter names, most influential first.
    pub ranked: BTreeMap<String, Vec<String>>,
    /// Rows that may not steer an optimizer, with the reason.
    pub unusable: Vec<String>,
    /// Whether every row is safe to act on.
    pub all_usable: bool,
    /// One receipt claim per row, ready for `build_receipt`. Rows whose
    /// derivative is not established come through as `Unverifiable` rather
    /// than being dropped.
    pub claims: Vec<vcad_kernel_adjoint::ReceiptClaim>,
}

/// Run a whole sensitivity request and package the answer.
///
/// The single entry point for the WASM export and the MCP tool: parses
/// quantity names, defaults the parameter list to everything the document
/// declares, and attaches receipt claims so a sensitivity can be carried
/// into a receipt instead of being re-derived.
pub fn document_sensitivity_report(
    doc: &Document,
    req: &SensitivityRequest,
    tess: &TessellationParams,
) -> Result<SensitivityReport, SensitivityError> {
    let parameters: Vec<String> = match &req.parameters {
        Some(p) if !p.is_empty() => p.clone(),
        _ => {
            let mut names: Vec<String> = doc.parameters.keys().cloned().collect();
            names.sort();
            names
        }
    };
    let qois: Vec<QoiRequest> = match &req.quantities {
        Some(q) if !q.is_empty() => q
            .iter()
            .map(|n| {
                QoiRequest::parse(n, req.part).ok_or_else(|| SensitivityError::UnknownQuantity {
                    name: n.clone(),
                    known: QoiRequest::NAMES.join(", "),
                })
            })
            .collect::<Result<_, _>>()?,
        _ => vec![
            QoiRequest {
                qoi: Qoi::Volume,
                part: req.part,
            },
            QoiRequest {
                qoi: Qoi::Mass,
                part: req.part,
            },
        ],
    };

    let defaults = SensitivityOptions::default();
    let opts = SensitivityOptions {
        density: req.density.unwrap_or(defaults.density),
        probe_step: req.probe_step.unwrap_or(defaults.probe_step),
        topology_reach: req.topology_reach.unwrap_or(defaults.topology_reach),
        topology_refinements: defaults.topology_refinements,
        find_topology_radius: req.find_trust_radius.unwrap_or(true),
    };

    let table = document_sensitivities(doc, &parameters, &qois, tess, &opts)?;
    let oracle =
        vcad_kernel_adjoint::OracleRef::new("vcad-eval/sensitivity", env!("CARGO_PKG_VERSION"));
    let claims = table
        .rows
        .iter()
        .map(|r| r.to_claim(oracle.clone()))
        .collect();
    let ranked = table
        .objectives()
        .into_iter()
        .map(|o| {
            (
                o.to_string(),
                table
                    .ranked_for(o)
                    .into_iter()
                    .map(|r| r.parameter.clone())
                    .collect(),
            )
        })
        .collect();
    let unusable = table
        .unusable()
        .into_iter()
        .map(|(r, why)| format!("{}/{}: {why}", r.objective, r.parameter))
        .collect();

    Ok(SensitivityReport {
        rendered: table.render(),
        all_usable: table.all_usable(),
        ranked,
        unusable,
        claims,
        table,
    })
}
