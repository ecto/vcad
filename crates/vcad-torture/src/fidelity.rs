//! Boolean representation-fidelity matrix.
//!
//! A factual map of where the kernel stands today: for each
//! (operation × operand surface types × configuration), does the result
//! come back as a true analytic B-rep, as triangle soup from the mesh-CSG
//! fallback, or not at all?
//!
//! This is deliberately *not* a pass/fail test of geometry. The torture
//! track already grades geometric correctness. This grades the
//! **representation**, which is the thing that decides whether a part can
//! be exported as meaningful STEP for fabrication, ray-traced, filleted or
//! drafted — and which, until now, nothing reported until export time.
//!
//! Run `vcad-torture fidelity --md docs/boolean-fidelity-matrix.md
//! --json crates/vcad-torture/fidelity-baseline.json` to regenerate, or
//! `VCAD_FIDELITY_BLESS=1 cargo test -p vcad-torture` to re-bless the
//! checked-in baseline after an intentional change.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use vcad_kernel::{BooleanOp, Solid, SolidFidelity};

/// Which boolean is under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Op {
    /// A ∪ B
    Union,
    /// A − B
    Difference,
    /// A ∩ B
    Intersection,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Union => "union",
            Op::Difference => "difference",
            Op::Intersection => "intersection",
        }
    }

    fn kernel(self) -> BooleanOp {
        match self {
            Op::Union => BooleanOp::Union,
            Op::Difference => BooleanOp::Difference,
            Op::Intersection => BooleanOp::Intersection,
        }
    }

    /// All three, in a stable order.
    pub fn all() -> [Op; 3] {
        [Op::Union, Op::Difference, Op::Intersection]
    }
}

/// The dominant surface type an operand contributes to the arrangement.
///
/// This is the axis the task asks about: the splitters are written
/// per-surface-kind, so the surface pair is what predicts whether the B-rep
/// pipeline can represent the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Surf {
    /// Planar faces only (cube, wedge, prism).
    Plane,
    /// Cylindrical lateral face.
    Cylinder,
    /// Conical lateral face.
    Cone,
    /// Spherical face.
    Sphere,
    /// Toroidal face.
    Torus,
}

impl Surf {
    fn name(self) -> &'static str {
        match self {
            Surf::Plane => "plane",
            Surf::Cylinder => "cylinder",
            Surf::Cone => "cone",
            Surf::Sphere => "sphere",
            Surf::Torus => "torus",
        }
    }
}

/// The arrangement of the two operands — the second axis the task asks
/// about, and the one that actually separates "works" from "degrades".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Config {
    /// Plain partial overlap, no incidences. The easy case.
    Generic,
    /// Surfaces touch at a single point or line without crossing.
    Tangent,
    /// A face of A lies exactly in the plane of a face of B.
    CoincidentFace,
    /// The tool passes fully through the target and exits through a
    /// *curved* face — the break-out case recent commits target.
    ThroughCutBreakout,
    /// A blind bore: the tool stops inside the target, leaving a floor.
    Bore,
    /// Operands meet only along an edge or at a corner — the result is
    /// non-manifold or degenerate at the contact.
    NonManifoldContact,
}

impl Config {
    fn name(self) -> &'static str {
        match self {
            Config::Generic => "generic-overlap",
            Config::Tangent => "tangent",
            Config::CoincidentFace => "coincident-face",
            Config::ThroughCutBreakout => "through-cut-breakout",
            Config::Bore => "bore",
            Config::NonManifoldContact => "non-manifold-contact",
        }
    }

    /// All six, in a stable order.
    pub fn all() -> [Config; 6] {
        [
            Config::Generic,
            Config::Tangent,
            Config::CoincidentFace,
            Config::ThroughCutBreakout,
            Config::Bore,
            Config::NonManifoldContact,
        ]
    }
}

/// One cell of the matrix: an arrangement, before any operation is applied.
pub struct Arrangement {
    /// Stable id, `<config>/<surf-a>-<surf-b>`.
    pub id: String,
    /// Configuration this arrangement realises.
    pub config: Config,
    /// Surface type of the target.
    pub a: Surf,
    /// Surface type of the tool.
    pub b: Surf,
    /// What makes this arrangement the thing it claims to be. Recorded so
    /// a reader can check the fixture actually exercises the case.
    pub note: &'static str,
    /// Build the target.
    pub build_a: fn() -> Solid,
    /// Build the tool.
    pub build_b: fn() -> Solid,
}

/// The outcome of one (arrangement, op) pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cell {
    /// `<arrangement-id>/<op>`.
    pub id: String,
    /// Configuration name.
    pub config: String,
    /// Operand surface pair, `"cylinder × plane"`.
    pub operands: String,
    /// Arrangement id this cell belongs to. Distinct arrangements can
    /// share an operand pair (a cross-drill and a Steinmetz cross are both
    /// cylinder × cylinder), so this — not `operands` — identifies the row.
    pub arrangement: String,
    /// Human description of the arrangement.
    pub note: String,
    /// Operation name.
    pub op: String,
    /// `analytic` | `triangle-soup` | `mesh-only` | `empty` | `error`.
    pub fidelity: String,
    /// The `DegradeReason`/`LossKind` that fired, when one did.
    pub reason: Option<String>,
    /// Set when the operation returned `Err` outright.
    pub error: Option<String>,
    /// Did the operation produce the *wrong solid*, not merely a coarse
    /// one? (a skipped cut, or a mesh concatenation standing in for a
    /// boolean)
    pub wrong_geometry: bool,
}

impl Cell {
    /// Is this cell a representation loss?
    pub fn degraded(&self) -> bool {
        self.fidelity != "analytic" && self.fidelity != "empty"
    }
}

/// The full matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FidelityMatrix {
    /// Every cell, in corpus order.
    pub cells: Vec<Cell>,
}

impl FidelityMatrix {
    /// Run every (arrangement × op) and record what came back.
    pub fn run() -> Self {
        let mut cells = Vec::new();
        for arr in corpus() {
            let a = (arr.build_a)();
            let b = (arr.build_b)();
            for op in Op::all() {
                cells.push(run_cell(&arr, &a, &b, op));
            }
        }
        Self { cells }
    }

    /// Cells that lost the analytic representation.
    pub fn degraded(&self) -> impl Iterator<Item = &Cell> {
        self.cells.iter().filter(|c| c.degraded())
    }

    /// Just the `id -> fidelity` map. This is what the drift test
    /// compares: volumes and face counts vary across platforms (the
    /// torture baseline already has five such cases), but the fidelity
    /// class is a coarse, structural signal.
    pub fn classes(&self) -> BTreeMap<String, String> {
        self.cells
            .iter()
            .map(|c| (c.id.clone(), c.fidelity.clone()))
            .collect()
    }

    /// Render the matrix as markdown: a per-configuration table plus a
    /// summary of which degradations occur and how often.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Boolean representation-fidelity matrix\n\n");
        out.push_str(
            "Generated by `cargo run -p vcad-torture -- fidelity --md \
             docs/boolean-fidelity-matrix.md`. Do not edit by hand.\n\n\
             Each cell reports what the *representation* of the result is, \
             not whether the geometry is right — the torture track grades \
             geometry. `analytic` means the result kept real \
             Plane/Cylinder/Cone/Sphere/Torus faces. `triangle-soup` means \
             the mesh-CSG fallback fired and the result is a B-rep made of \
             one-triangle planar faces: `can_export_step()` still returns \
             `true`, and the STEP is facets. `mesh-only` means no B-rep at \
             all. `error` means the boolean returned `Err`.\n\n\
             **Coverage gap: NURBS operands are not exercised.** No modeling \
             operation in the kernel produces a B-spline face — `loft` emits \
             ruled planar faces (`LoftMode::Smooth` is unimplemented), and \
             the only practical source of `SurfaceKind::BSpline` is STEP \
             import. Booleans against imported NURBS geometry are therefore \
             uncharacterised here.\n\n",
        );

        let total = self.cells.len();
        let degraded = self.degraded().count();
        let wrong = self.cells.iter().filter(|c| c.wrong_geometry).count();
        out.push_str(&format!(
            "**{degraded} of {total} cells degrade**; {wrong} produce the \
             wrong solid rather than a coarse one.\n\n",
        ));

        // Summary by reason, worst first.
        let mut by_reason: BTreeMap<&str, usize> = BTreeMap::new();
        for c in self.degraded() {
            let key = c.reason.as_deref().unwrap_or("(unattributed)");
            *by_reason.entry(key).or_default() += 1;
        }
        if !by_reason.is_empty() {
            out.push_str("## Degradations by cause\n\n| cause | cells |\n|---|---|\n");
            let mut rows: Vec<_> = by_reason.into_iter().collect();
            rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            for (reason, n) in rows {
                out.push_str(&format!("| `{reason}` | {n} |\n"));
            }
            out.push('\n');
        }

        for config in Config::all() {
            let rows: Vec<&Cell> = self
                .cells
                .iter()
                .filter(|c| c.config == config.name())
                .collect();
            if rows.is_empty() {
                continue;
            }
            out.push_str(&format!("## {}\n\n", config.name()));
            out.push_str("| operands | arrangement | union | difference | intersection |\n");
            out.push_str("|---|---|---|---|---|\n");
            let mut seen: Vec<&str> = Vec::new();
            for c in &rows {
                if !seen.contains(&c.arrangement.as_str()) {
                    seen.push(&c.arrangement);
                }
            }
            for arrangement in seen {
                let cell_for = |op: Op| {
                    rows.iter()
                        .find(|c| c.arrangement == arrangement && c.op == op.name())
                        .map(|c| {
                            let mark = match c.fidelity.as_str() {
                                "analytic" => "✅ analytic",
                                "triangle-soup" => "⚠️ soup",
                                "mesh-only" => "❌ mesh",
                                "empty" => "· empty",
                                other => other,
                            };
                            match &c.reason {
                                Some(r) => format!("{mark}<br>`{r}`"),
                                None => mark.to_string(),
                            }
                        })
                        .unwrap_or_else(|| "—".into())
                };
                let head = rows
                    .iter()
                    .find(|c| c.arrangement == arrangement)
                    .expect("arrangement came from these rows");
                out.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    head.operands,
                    head.note,
                    cell_for(Op::Union),
                    cell_for(Op::Difference),
                    cell_for(Op::Intersection),
                ));
            }
            out.push('\n');
        }
        out
    }
}

fn run_cell(arr: &Arrangement, a: &Solid, b: &Solid, op: Op) -> Cell {
    let id = format!("{}/{}", arr.id, op.name());
    let operands = format!("{} × {}", arr.a.name(), arr.b.name());
    let arrangement = arr.id.clone();
    let note = arr.note.to_string();
    match a.try_boolean_reported(b, op.kernel()) {
        Ok((solid, event)) => Cell {
            id,
            config: arr.config.name().to_string(),
            operands,
            arrangement,
            note,
            op: op.name().to_string(),
            fidelity: solid.fidelity().as_str().to_string(),
            reason: event
                .as_ref()
                .map(|e| e.kind.as_str().to_string())
                .or_else(|| {
                    // Structurally soup with nothing recorded: the loss came in
                    // through an operand rather than this operation.
                    (solid.fidelity() == SolidFidelity::TriangleSoup)
                        .then(|| "inherited-or-unrecorded".to_string())
                }),
            error: None,
            wrong_geometry: event.is_some_and(|e| e.kind.is_wrong_geometry()),
        },
        Err(e) => Cell {
            id,
            config: arr.config.name().to_string(),
            operands,
            arrangement,
            note,
            op: op.name().to_string(),
            fidelity: "error".to_string(),
            reason: None,
            error: Some(e.to_string()),
            // An `Err` produces no solid at all, so the caller cannot ship
            // a wrong one. That is the honest, fail-closed outcome.
            wrong_geometry: false,
        },
    }
}

// =============================================================================
// The corpus
// =============================================================================

const SEG: u32 = 32;

// --- operand builders. Free functions so `Arrangement` stays plain data. ---

fn cube_a() -> Solid {
    Solid::cube(20.0, 20.0, 20.0)
}
fn cyl_a() -> Solid {
    Solid::cylinder(8.0, 24.0, SEG)
}
fn cone_a() -> Solid {
    Solid::cone(9.0, 3.0, 20.0, SEG)
}
fn sphere_a() -> Solid {
    Solid::sphere(10.0, SEG)
}
fn torus_a() -> Solid {
    Solid::torus(10.0, 3.0, SEG)
}

// Generic overlap: the tool sits partly inside the target, meeting nothing
// exactly.
fn g_cube_tool() -> Solid {
    Solid::cube(12.0, 12.0, 12.0).translate(13.0, 7.0, 6.0)
}
fn g_cyl_tool() -> Solid {
    Solid::cylinder(5.0, 30.0, SEG).translate(13.0, 7.0, -3.0)
}
fn g_cone_tool() -> Solid {
    Solid::cone(6.0, 1.0, 26.0, SEG).translate(13.0, 7.0, -3.0)
}
fn g_sphere_tool() -> Solid {
    Solid::sphere(7.0, SEG).translate(17.0, 9.0, 11.0)
}
fn g_torus_tool() -> Solid {
    Solid::torus(8.0, 2.5, SEG).translate(16.0, 9.0, 11.0)
}

// Generic-overlap tools sized for the *curved* targets, which sit on the
// origin (cylinder/cone base at z=0, sphere/torus centred). The cube tools
// above are placed for the 0..20 box and would miss these entirely.
fn g_sphere_on_cyl_wall() -> Solid {
    // Centred on the r=8 wall, so it straddles it.
    Solid::sphere(7.0, SEG).translate(8.0, 0.0, 12.0)
}
fn g_cyl_through_sphere_offaxis() -> Solid {
    Solid::cylinder(5.0, 30.0, SEG).translate(8.0, 0.0, -15.0)
}
fn g_cyl_on_cone_wall() -> Solid {
    // The cone's radius at z=6 is 7.2; an axis at r=7 straddles the slant.
    Solid::cylinder(5.0, 30.0, SEG).translate(7.0, 0.0, -5.0)
}
fn g_cyl_through_torus_tube() -> Solid {
    // Axis through the tube centreline circle (radius 10).
    Solid::cylinder(5.0, 30.0, SEG).translate(10.0, 0.0, -15.0)
}
// Tangent tools for the cylinder target.
fn t_sphere_on_cyl_cap() -> Solid {
    // Resting exactly on the z=24 top cap.
    Solid::sphere(6.0, SEG).translate(0.0, 0.0, 30.0)
}
// Coincident-face tool for the cylinder target: coaxial, sharing z=0.
fn c_cyl_coaxial_flush() -> Solid {
    Solid::cylinder(5.0, 10.0, SEG).translate(0.0, 0.0, -10.0)
}

// Tangent: surfaces touch without crossing.
fn t_sphere_on_face() -> Solid {
    // Sphere resting exactly on the cube's top face (z = 20).
    Solid::sphere(6.0, SEG).translate(10.0, 10.0, 26.0)
}
fn t_cyl_side() -> Solid {
    // Cylinder whose lateral surface grazes the cube's x = 20 face.
    Solid::cylinder(6.0, 30.0, SEG).translate(26.0, 10.0, -5.0)
}
fn t_sphere_in_sphere() -> Solid {
    // Internally tangent: touches at exactly one point.
    Solid::sphere(4.0, SEG).translate(0.0, 0.0, 6.0)
}
fn t_cone_apex() -> Solid {
    // Cone apex exactly on the cube's top face.
    Solid::cone(6.0, 0.0, 6.0, SEG).translate(10.0, 10.0, 20.0)
}

// Coincident faces: a face of the tool lies exactly in a face plane of the
// target.
fn c_cube_flush() -> Solid {
    Solid::cube(10.0, 10.0, 10.0).translate(20.0, 5.0, 5.0)
}
fn c_cyl_flush() -> Solid {
    // Cylinder cap exactly on the cube's z = 0 face.
    Solid::cylinder(5.0, 10.0, SEG).translate(10.0, 10.0, -10.0)
}
fn c_cone_flush() -> Solid {
    Solid::cone(5.0, 2.0, 10.0, SEG).translate(10.0, 10.0, -10.0)
}
fn c_cube_coplanar_overlap() -> Solid {
    // Shares the z = 0 plane AND overlaps in volume.
    Solid::cube(10.0, 10.0, 10.0).translate(15.0, 15.0, 0.0)
}

// Through-cut break-out: the tool passes fully through and exits a curved
// face. This is the family recent commits target.
fn bo_cyl_through_cyl() -> Solid {
    // Cross-drill through the round wall of a cylinder.
    Solid::cylinder(3.0, 40.0, SEG)
        .rotate(0.0, 90.0, 0.0)
        .translate(-20.0, 0.0, 12.0)
}
fn bo_cyl_through_sphere() -> Solid {
    Solid::cylinder(3.0, 40.0, SEG)
        .rotate(0.0, 90.0, 0.0)
        .translate(-20.0, 0.0, 0.0)
}
fn bo_cyl_through_cone() -> Solid {
    Solid::cylinder(2.5, 40.0, SEG)
        .rotate(0.0, 90.0, 0.0)
        .translate(-20.0, 0.0, 8.0)
}
fn bo_cyl_through_torus() -> Solid {
    Solid::cylinder(2.0, 40.0, SEG)
        .rotate(0.0, 90.0, 0.0)
        .translate(-20.0, 0.0, 0.0)
}
fn bo_cyl_through_cube() -> Solid {
    Solid::cylinder(4.0, 40.0, SEG)
        .rotate(0.0, 90.0, 0.0)
        .translate(-10.0, 10.0, 10.0)
}
fn bo_cyl_equal_radius_cross() -> Solid {
    // The Steinmetz case: perpendicular, intersecting, equal radii.
    Solid::cylinder(8.0, 40.0, SEG)
        .rotate(0.0, 90.0, 0.0)
        .translate(-20.0, 0.0, 12.0)
}

// Bore: blind hole, tool stops inside the target.
fn b_cyl_blind() -> Solid {
    Solid::cylinder(4.0, 12.0, SEG).translate(10.0, 10.0, 12.0)
}
fn b_cone_blind() -> Solid {
    Solid::cone(4.0, 1.0, 12.0, SEG).translate(10.0, 10.0, 12.0)
}
fn b_sphere_pocket() -> Solid {
    // A hemispherical socket — the case a 2026-08-11 field report hit.
    Solid::sphere(6.0, SEG).translate(10.0, 10.0, 20.0)
}
fn b_cyl_blind_in_cyl() -> Solid {
    Solid::cylinder(3.0, 10.0, SEG).translate(0.0, 0.0, 16.0)
}

// Non-manifold contact: operands meet only along an edge or at a corner.
fn nm_cube_corner() -> Solid {
    // Corner-to-corner at (20,20,20): contact is a single point.
    Solid::cube(10.0, 10.0, 10.0).translate(20.0, 20.0, 20.0)
}
fn nm_cube_edge() -> Solid {
    // Edge-to-edge along the x = 20, z = 20 edge.
    Solid::cube(10.0, 10.0, 10.0).translate(20.0, 5.0, 20.0)
}
fn nm_cyl_edge() -> Solid {
    // Cylinder tangent to the cube's vertical edge at x = 20, y = 20.
    Solid::cylinder(5.0, 30.0, SEG).translate(23.53553, 23.53553, -5.0)
}
fn nm_sphere_corner() -> Solid {
    Solid::sphere(5.0, SEG).translate(22.88675, 22.88675, 22.88675)
}

/// The arrangement corpus.
///
/// Curated rather than a full cross-product: a "coincident face" between a
/// sphere and a torus is not a thing, and filling the matrix with
/// nonsensical cells would make it look more complete than it is. Every
/// entry below is an arrangement that a real model actually contains.
pub fn corpus() -> Vec<Arrangement> {
    let mut v = Vec::new();
    let mut push = |config: Config,
                    a: Surf,
                    b: Surf,
                    note: &'static str,
                    build_a: fn() -> Solid,
                    build_b: fn() -> Solid| {
        v.push(Arrangement {
            id: format!("{}/{}-{}", config.name(), a.name(), b.name()),
            config,
            a,
            b,
            note,
            build_a,
            build_b,
        });
    };

    // --- Generic overlap: every surface kind against a plane, plus a few
    // curved × curved pairs. The control group.
    push(
        Config::Generic,
        Surf::Plane,
        Surf::Plane,
        "two boxes overlapping in a corner region",
        cube_a,
        g_cube_tool,
    );
    push(
        Config::Generic,
        Surf::Plane,
        Surf::Cylinder,
        "cylinder straddling a box face, no incidence",
        cube_a,
        g_cyl_tool,
    );
    push(
        Config::Generic,
        Surf::Plane,
        Surf::Cone,
        "cone straddling a box face",
        cube_a,
        g_cone_tool,
    );
    push(
        Config::Generic,
        Surf::Plane,
        Surf::Sphere,
        "sphere overlapping a box corner",
        cube_a,
        g_sphere_tool,
    );
    push(
        Config::Generic,
        Surf::Plane,
        Surf::Torus,
        "torus overlapping a box corner",
        cube_a,
        g_torus_tool,
    );
    push(
        Config::Generic,
        Surf::Cylinder,
        Surf::Sphere,
        "sphere straddling a cylinder's round wall",
        cyl_a,
        g_sphere_on_cyl_wall,
    );
    push(
        Config::Generic,
        Surf::Sphere,
        Surf::Cylinder,
        "cylinder through a sphere, off-axis",
        sphere_a,
        g_cyl_through_sphere_offaxis,
    );
    push(
        Config::Generic,
        Surf::Cone,
        Surf::Cylinder,
        "cylinder straddling a cone's slant wall",
        cone_a,
        g_cyl_on_cone_wall,
    );
    push(
        Config::Generic,
        Surf::Torus,
        Surf::Cylinder,
        "cylinder through a torus tube",
        torus_a,
        g_cyl_through_torus_tube,
    );

    // --- Tangent
    push(
        Config::Tangent,
        Surf::Plane,
        Surf::Sphere,
        "sphere resting exactly on the box's top face",
        cube_a,
        t_sphere_on_face,
    );
    push(
        Config::Tangent,
        Surf::Plane,
        Surf::Cylinder,
        "cylinder wall grazing the box's x=20 face",
        cube_a,
        t_cyl_side,
    );
    push(
        Config::Tangent,
        Surf::Plane,
        Surf::Cone,
        "cone apex exactly on the box's top face",
        cube_a,
        t_cone_apex,
    );
    push(
        Config::Tangent,
        Surf::Sphere,
        Surf::Sphere,
        "internally tangent spheres, single contact point",
        sphere_a,
        t_sphere_in_sphere,
    );
    push(
        Config::Tangent,
        Surf::Cylinder,
        Surf::Sphere,
        "sphere resting on a cylinder's top cap",
        cyl_a,
        t_sphere_on_cyl_cap,
    );

    // --- Coincident faces
    push(
        Config::CoincidentFace,
        Surf::Plane,
        Surf::Plane,
        "box faces flush at x=20, no volume overlap",
        cube_a,
        c_cube_flush,
    );
    push(
        Config::CoincidentFace,
        Surf::Plane,
        Surf::Cylinder,
        "cylinder cap coplanar with the box's z=0 face",
        cube_a,
        c_cyl_flush,
    );
    push(
        Config::CoincidentFace,
        Surf::Plane,
        Surf::Cone,
        "cone cap coplanar with the box's z=0 face",
        cube_a,
        c_cone_flush,
    );
    push(
        Config::CoincidentFace,
        Surf::Cylinder,
        Surf::Cylinder,
        "coaxial cylinders sharing the z=0 cap plane",
        cyl_a,
        c_cyl_coaxial_flush,
    );
    push(
        Config::CoincidentFace,
        Surf::Plane,
        Surf::Plane,
        "boxes sharing the z=0 plane and overlapping in volume",
        cube_a,
        c_cube_coplanar_overlap,
    );

    // --- Through-cut break-out (the family recent commits target)
    push(
        Config::ThroughCutBreakout,
        Surf::Cylinder,
        Surf::Cylinder,
        "cross-drill exiting a cylinder's round wall",
        cyl_a,
        bo_cyl_through_cyl,
    );
    push(
        Config::ThroughCutBreakout,
        Surf::Sphere,
        Surf::Cylinder,
        "drill straight through a sphere",
        sphere_a,
        bo_cyl_through_sphere,
    );
    push(
        Config::ThroughCutBreakout,
        Surf::Cone,
        Surf::Cylinder,
        "cross-drill exiting a cone's slant wall",
        cone_a,
        bo_cyl_through_cone,
    );
    push(
        Config::ThroughCutBreakout,
        Surf::Torus,
        Surf::Cylinder,
        "drill through a torus tube",
        torus_a,
        bo_cyl_through_torus,
    );
    push(
        Config::ThroughCutBreakout,
        Surf::Plane,
        Surf::Cylinder,
        "through-hole exiting two planar box faces (the control)",
        cube_a,
        bo_cyl_through_cube,
    );
    push(
        Config::ThroughCutBreakout,
        Surf::Cylinder,
        Surf::Cylinder,
        "Steinmetz: perpendicular equal-radius cylinders",
        cyl_a,
        bo_cyl_equal_radius_cross,
    );

    // --- Bores
    push(
        Config::Bore,
        Surf::Plane,
        Surf::Cylinder,
        "blind cylindrical bore into a box",
        cube_a,
        b_cyl_blind,
    );
    push(
        Config::Bore,
        Surf::Plane,
        Surf::Cone,
        "blind tapered bore into a box",
        cube_a,
        b_cone_blind,
    );
    push(
        Config::Bore,
        Surf::Plane,
        Surf::Sphere,
        "hemispherical socket in a box's top face",
        cube_a,
        b_sphere_pocket,
    );
    push(
        Config::Bore,
        Surf::Cylinder,
        Surf::Cylinder,
        "coaxial blind bore into a cylinder",
        cyl_a,
        b_cyl_blind_in_cyl,
    );
    push(
        Config::Bore,
        Surf::Cone,
        Surf::Cylinder,
        "blind bore into a cone's small end",
        cone_a,
        b_cyl_blind_in_cyl,
    );

    // --- Non-manifold contact
    push(
        Config::NonManifoldContact,
        Surf::Plane,
        Surf::Plane,
        "boxes touching at a single corner point",
        cube_a,
        nm_cube_corner,
    );
    push(
        Config::NonManifoldContact,
        Surf::Plane,
        Surf::Plane,
        "boxes touching along one edge only",
        cube_a,
        nm_cube_edge,
    );
    push(
        Config::NonManifoldContact,
        Surf::Plane,
        Surf::Cylinder,
        "cylinder tangent to a box's vertical edge",
        cube_a,
        nm_cyl_edge,
    );
    push(
        Config::NonManifoldContact,
        Surf::Plane,
        Surf::Sphere,
        "sphere tangent to a box's corner",
        cube_a,
        nm_sphere_corner,
    );

    // Ids must be unique — two entries sharing a config and surface pair
    // would silently collide in the baseline map.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for arr in &mut v {
        let n = counts.entry(arr.id.clone()).or_default();
        *n += 1;
        if *n > 1 {
            arr.id = format!("{}#{}", arr.id, *n);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell reporting `analytic` proves nothing if the two operands never
    /// actually met — an untouched solid trivially keeps its B-rep. For
    /// every configuration where the tool is supposed to remove material,
    /// assert the difference really removed some.
    ///
    /// Without this, a fixture that drifts out of contact (a translate
    /// tweaked, a radius changed) would silently turn the matrix green.
    #[test]
    fn overlap_fixtures_actually_intersect() {
        for arr in corpus() {
            if !matches!(
                arr.config,
                Config::Generic | Config::ThroughCutBreakout | Config::Bore
            ) {
                // Tangent, coincident-face and non-manifold contacts are
                // *defined* by removing (near-)zero volume. Requiring a cut
                // here would be requiring the wrong answer.
                continue;
            }
            let a = (arr.build_a)();
            let b = (arr.build_b)();
            let before = a.volume();
            let (cut, _) = a
                .try_boolean_reported(&b, BooleanOp::Difference)
                .unwrap_or_else(|e| panic!("{}: difference failed: {e}", arr.id));
            let removed = before - cut.volume();
            assert!(
                removed > 1e-6,
                "{} ({}): difference removed {removed:.6} mm³ — the fixture \
                 does not actually intersect, so its matrix row is vacuous",
                arr.id,
                arr.note,
            );
        }
    }

    /// Every arrangement id must be unique; a collision would silently
    /// overwrite a row in the baseline map.
    #[test]
    fn arrangement_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for arr in corpus() {
            assert!(seen.insert(arr.id.clone()), "duplicate id {}", arr.id);
        }
    }

    /// The matrix must cover every operation for every arrangement.
    #[test]
    fn matrix_is_complete() {
        let m = FidelityMatrix::run();
        assert_eq!(m.cells.len(), corpus().len() * 3);
        assert_eq!(m.classes().len(), m.cells.len(), "cell ids collided");
    }
}
