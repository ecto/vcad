//! Printability lint for an **exported** mesh in a chosen print orientation.
//!
//! FDM printability is a property of the shipped file, not of the model that
//! produced it. A rana shell variant passed its author's analytic profile
//! verification while the discretised STL carried 0.05 mm cracks across 85% of
//! its circumference; only raycasts on the export caught it. So every check
//! here reads triangles.
//!
//! What it reports:
//!
//! 1. floating regions / mid-air islands (per-column support raycasts)
//! 2. interior cracks — material gaps below a threshold (default 0.15 mm)
//! 3. overhang census against a max angle, with the staircase-vs-support verdict
//! 4. bridge spans, with lengths
//! 5. min wall / min feature against the nozzle width
//! 6. manifold + closed-sections summary
//!
//! The verdict is a clean/dirty boolean so it can gate CI.
//!
//! ```no_run
//! use std::path::Path;
//! use vcad_printcheck::{check_file, Options};
//!
//! let report = check_file(Path::new("part.stl"), &Options::default()).unwrap();
//! assert!(report.ok);
//! ```

pub mod checks;
pub mod mesh;

use std::path::Path;

pub use mesh::{Mesh, MeshError, Orientation};

/// Knobs. Defaults are the rana field conventions: 0.4 mm nozzle, 4 mm bridge
/// ceiling, 0.15 mm crack threshold, 45° self-support limit.
#[derive(Debug, Clone)]
pub struct Options {
    pub orientation: Orientation,
    /// Nozzle width; anything thinner than this cannot be extruded.
    pub nozzle: f64,
    /// Longest unsupported span accepted without support material.
    pub max_bridge: f64,
    /// Material gaps below this are cracks, not channels. Never whitelistable.
    pub crack_threshold: f64,
    /// Self-support limit, degrees from vertical.
    pub max_overhang: f64,
    /// Downward faces within this many degrees of horizontal are roofs /
    /// bridges and are judged by the bridge span rule, not the overhang rule.
    pub roof_deg: f64,
    /// Turn the overhang census into a failure rather than a warning.
    pub strict_overhangs: bool,
    /// Distance between raycast columns, mm. Defaults to the nozzle width:
    /// the checks look for nozzle-scale defects, so they have to sample at
    /// nozzle scale. Finer costs time; coarser starts missing walls.
    pub pitch: f64,
    /// Hard cap on columns across the longer axis, so a large part cannot
    /// blow up the grid however fine the pitch.
    pub max_columns: usize,
    /// Minimum |cos| between a ray and the faces it enters and leaves through
    /// before its span counts as a wall thickness. 0.5 = within 60° of
    /// head-on. Below this the ray is grazing a curved surface and its chord
    /// measures the silhouette, not the wall.
    pub wall_align: f64,
    /// Section sampling pitch, mm.
    pub section_step: f64,
    /// Height above the lowest point of the mesh that still counts as sitting
    /// on the build plate — the first layer. A chamfered or filleted rim
    /// contacts the plate along a ring narrower than any sampling pitch, so
    /// without this every such part reports its own bottom edge as floating.
    pub bed_tol: f64,
    /// Height ranges where an unsupported span is a known, accepted bridge.
    /// Cracks are never exempted.
    pub allow_bridges: Vec<(f64, f64)>,
    /// Interior thin samples needed before a min-wall failure is raised.
    pub min_thin_columns: usize,
    /// Cap on findings of one kind printed before they are summarised.
    pub max_reported: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            orientation: Orientation::ZUp,
            nozzle: 0.4,
            max_bridge: 4.0,
            crack_threshold: 0.15,
            max_overhang: 45.0,
            roof_deg: 10.0,
            strict_overhangs: false,
            pitch: 0.4,
            max_columns: 512,
            wall_align: 0.5,
            section_step: 0.4,
            bed_tol: 0.4,
            allow_bridges: Vec::new(),
            min_thin_columns: 3,
            max_reported: 12,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    NonManifold,
    EmptyLayer,
    OpenSection,
    Crack,
    FloatingRegion,
    OverlongBridge,
    Overhang,
    ThinWall,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub severity: Severity,
    pub message: String,
    /// A representative point, in the print frame.
    pub location: Option<[f64; 3]>,
    pub value_mm: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    pub file: String,
    pub orientation: String,
    pub manifold: checks::ManifoldSummary,
    pub sections: checks::SectionSummary,
    pub columns: checks::ColumnSummary,
    pub overhangs: checks::OverhangSummary,
    pub walls: checks::WallSummary,
    pub findings: Vec<Finding>,
    /// True when nothing failed. Warnings do not dirty the verdict.
    pub ok: bool,
}

impl Report {
    pub fn failures(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Fail)
    }

    pub fn has(&self, kind: FindingKind) -> bool {
        self.findings
            .iter()
            .any(|f| f.kind == kind && f.severity == Severity::Fail)
    }
}

/// Load an STL and run every check.
pub fn check_file(path: &Path, opts: &Options) -> Result<Report, MeshError> {
    let mesh = Mesh::load_stl(path, opts.orientation)?;
    Ok(check_mesh(&mesh, path.display().to_string(), opts))
}

/// Run every check on an already-loaded mesh (already in the print frame).
pub fn check_mesh(mesh: &Mesh, name: String, opts: &Options) -> Report {
    let mut findings = Vec::new();
    let (manifold, f) = checks::check_manifold(mesh);
    findings.extend(f);
    let (sections, f) = checks::check_sections(mesh, opts);
    findings.extend(f);
    let (columns, f) = checks::check_columns(mesh, opts);
    findings.extend(f);
    let (overhangs, f) = checks::check_overhangs(mesh, opts);
    findings.extend(f);
    let (walls, f) = checks::check_walls(mesh, opts);
    findings.extend(f);

    let ok = !findings.iter().any(|f| f.severity == Severity::Fail);
    Report {
        file: name,
        orientation: format!("{:?}", opts.orientation),
        manifold,
        sections,
        columns,
        overhangs,
        walls,
        findings,
        ok,
    }
}

/// Human-readable report, the shape `vcad check` prints.
pub fn render_text(r: &Report) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "printability: {}  [{}]", r.file, r.orientation);
    let _ = writeln!(
        s,
        "  mesh       {} triangles, {} edges, {}",
        r.manifold.triangles,
        r.manifold.edges,
        if r.manifold.bad_edges == 0 {
            "manifold".to_string()
        } else {
            format!("{} BAD EDGES", r.manifold.bad_edges)
        }
    );
    let _ = writeln!(
        s,
        "  sections   {} sampled over z {:.2}..{:.2}, {} empty, {} open",
        r.sections.sections,
        r.sections.z_min,
        r.sections.z_max,
        r.sections.empty_layers.len(),
        r.sections.open_sections.len()
    );
    let _ = writeln!(
        s,
        "  columns    {}/{} carry material; {} crack(s), thinnest gap {}",
        r.columns.columns_with_material,
        r.columns.columns_sampled,
        r.columns.cracks,
        r.columns
            .thinnest_gap_mm
            .map(|g| format!("{g:.3} mm"))
            .unwrap_or_else(|| "n/a".into())
    );
    let _ = writeln!(
        s,
        "  floating   {} region(s) with nothing beneath or beside them",
        r.columns.floating_regions
    );
    if r.columns.bridges.is_empty() {
        let _ = writeln!(s, "  bridges    none");
    } else {
        let _ = writeln!(s, "  bridges    {} span(s), longest first:", r.columns.bridges.len());
        for b in &r.columns.bridges {
            let _ = writeln!(
                s,
                "               {:.2} mm at z={:.3} ({} columns){}",
                b.span_mm,
                b.z,
                b.columns,
                if b.whitelisted { "  [allowed]" } else { "" }
            );
        }
    }
    let _ = writeln!(s, "  overhangs  {}", r.overhangs.verdict);
    let _ = writeln!(
        s,
        "             roofs {:.1} mm², self-supporting {:.1} mm², downward {:.1} of {:.1} mm² total",
        r.overhangs.roof_area_mm2,
        r.overhangs.self_supporting_area_mm2,
        r.overhangs.downward_area_mm2,
        r.overhangs.total_area_mm2
    );
    let _ = writeln!(
        s,
        "  min wall   {} vs {:.2} mm nozzle ({} interior samples below)",
        r.walls
            .min_feature_mm
            .map(|t| format!("{t:.3} mm"))
            .unwrap_or_else(|| "n/a".into()),
        r.walls.nozzle_mm,
        r.walls.thin_columns
    );
    if r.findings.is_empty() {
        let _ = writeln!(s, "\nPRINTABILITY PASS");
    } else {
        s.push('\n');
        for f in &r.findings {
            let tag = match f.severity {
                Severity::Fail => "FAIL",
                Severity::Warn => "warn",
            };
            let loc = f
                .location
                .map(|p| format!("  @ ({:.2}, {:.2}, {:.2})", p[0], p[1], p[2]))
                .unwrap_or_default();
            let _ = writeln!(s, "  {tag}  {}{loc}", f.message);
        }
        let _ = writeln!(
            s,
            "\n{}",
            if r.ok {
                "PRINTABILITY PASS (warnings only)"
            } else {
                "PRINTABILITY FAIL"
            }
        );
    }
    s
}
