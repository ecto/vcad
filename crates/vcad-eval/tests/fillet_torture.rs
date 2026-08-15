//! Fillet torture track — an adversarial corpus of fillet/blend cases with a
//! never-regress scoreboard.
//!
//! Each `.loon` file in `tests/fillet_torture/` evaluates to a two-root scene:
//! root 0 is the filleted body, root 1 the same body without the fillet.
//! Comparing the two lets the harness detect the kernel's documented
//! fail-soft path (returning the input unchanged) as a distinct `NoOp`
//! outcome instead of mistaking it for success. A handful of additional
//! kernel-level cases exercise `Solid::edge_blend` (variable radius, keyed
//! chamfer→fillet morphs, vertex-adjacent selections) that loon cannot
//! express yet.
//!
//! Outcome ranking (higher is better): Crash < BadGeometry < Refused ≈
//! NoOp < Success. The baseline in `BASELINE` records the current behavior
//! per case; the test fails if any case regresses below its recorded rank.
//! When a fix promotes a case, update its baseline entry to lock it in.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use vcad_eval::{evaluate_document, EvalOptions};
use vcad_kernel::Solid;
use vcad_kernel_fillet::{BlendKey, BlendSection, EdgeQuery};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_tessellate::tessellate_brep;

/// Classified outcome of one torture case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    /// Kernel panicked.
    Crash,
    /// Produced geometry, but it is non-watertight or its volume is insane.
    BadGeometry,
    /// Evaluation surfaced a clean error for this root.
    Refused,
    /// Fail-soft: the fillet returned its input unchanged.
    NoOp,
    /// Watertight output with sane volume, distinct from the input.
    Success,
}

impl Outcome {
    fn rank(self) -> u8 {
        match self {
            Outcome::Crash => 0,
            Outcome::BadGeometry => 1,
            Outcome::Refused => 2,
            Outcome::NoOp => 2,
            Outcome::Success => 3,
        }
    }
}

/// Baseline outcome rank per case. `Success` entries must stay successes;
/// `Refused`/`NoOp` entries may be promoted to `Success` by future fixes
/// (update the entry when they are). Cases marked infeasible are *expected*
/// to refuse forever.
const BASELINE: &[(&str, Outcome)] = &[
    // loon corpus. NoOp entries are the remaining known-failure classes
    // (see docs/fillet-torture-track.md): boolean-seam solids (concave
    // edges + coplanar split faces), the curved plane-cylinder /
    // plane-cone rim rebuild, and curved-curved intersection edges.
    ("boss-on-plate", Outcome::NoOp),
    ("cone-fillet", Outcome::NoOp),
    ("cube-baseline", Outcome::Success),
    ("cyl-cross", Outcome::NoOp),
    ("cylinder-cap", Outcome::NoOp),
    ("fillet-after-chamfer", Outcome::Success),
    ("fillet-of-fillet", Outcome::NoOp),
    ("hole-difference", Outcome::NoOp), // inner loops: documented fail-soft
    ("lens-spheres", Outcome::NoOp),
    ("notch-difference", Outcome::NoOp),
    ("pocket-in-fillet", Outcome::Success),
    ("prism3-fillet", Outcome::Success),
    ("radius-exceeds", Outcome::NoOp), // infeasible: refusal/no-op is correct
    ("radius-half", Outcome::NoOp),    // borderline: opposite insets collapse
    ("radius-near-half", Outcome::Success),
    ("refillet-larger", Outcome::Success),
    ("seam-tee", Outcome::NoOp),
    ("seam-union-cubes", Outcome::NoOp),
    ("shell-of-fillet", Outcome::Success),
    ("slot-extrude", Outcome::NoOp),
    ("sphere-box", Outcome::NoOp),
    ("thin-slab", Outcome::Success),
    ("wedge-fillet", Outcome::Success),
    // kernel-level edge_blend cases
    ("kb-var-radius-one-edge", Outcome::Success),
    ("kb-chamfer-morph-fillet", Outcome::Success),
    ("kb-vertical-edges-const", Outcome::Success),
    ("kb-keyed-all-edges", Outcome::Success),
];

struct CaseResult {
    name: String,
    outcome: Outcome,
    detail: String,
}

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fillet_torture")
}

/// Watertightness + volume sanity for a produced solid, given the
/// reference (unfilleted) volume.
fn classify_solid(filleted: &Solid, ref_volume: f64) -> (Outcome, String) {
    let vol = filleted.volume();
    if !vol.is_finite() || vol <= 0.0 {
        return (Outcome::BadGeometry, format!("volume {vol}"));
    }
    let rel = (vol - ref_volume).abs() / ref_volume.max(1e-12);
    if rel < 1e-9 {
        return (Outcome::NoOp, format!("volume unchanged ({vol:.3})"));
    }
    // Fillets/chamfers only remove material, and never more than a large
    // fraction of it at the radii this corpus uses.
    if vol > ref_volume * (1.0 + 1e-6) {
        return (
            Outcome::BadGeometry,
            format!("volume grew: {vol:.3} > ref {ref_volume:.3}"),
        );
    }
    if vol < ref_volume * 0.5 {
        return (
            Outcome::BadGeometry,
            format!("volume collapsed: {vol:.3} vs ref {ref_volume:.3}"),
        );
    }
    if let Some(brep) = filleted.as_brep() {
        let mesh = tessellate_brep(brep, 32);
        let open = mesh.boundary_edges().len();
        if open > 0 {
            return (
                Outcome::BadGeometry,
                format!("{open} open mesh edges (vol {vol:.3} / ref {ref_volume:.3})"),
            );
        }
    }
    (
        Outcome::Success,
        format!("vol {vol:.3} / ref {ref_volume:.3}"),
    )
}

fn run_loon_case(path: &std::path::Path) -> CaseResult {
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> (Outcome, String) {
        let doc = match vcad_loon::eval_vcad_file(path) {
            Ok(d) => d,
            Err(e) => return (Outcome::Refused, format!("loon error: {e}")),
        };
        assert_eq!(doc.roots.len(), 2, "{name}: corpus files must have 2 roots");
        let scene = match evaluate_document(&doc, &EvalOptions::default()) {
            Ok(s) => s,
            Err(e) => return (Outcome::Refused, format!("eval error: {e}")),
        };
        // Panics inside evaluate_document are contained per-root and tagged.
        for f in &scene.failures {
            if f.scope == "root[0]" {
                return if f.error.starts_with("kernel panic") {
                    (Outcome::Crash, f.error.clone())
                } else {
                    (Outcome::Refused, f.error.clone())
                };
            }
        }
        let filleted = scene.parts[0].solid.as_ref();
        let reference = scene.parts[1].solid.as_ref();
        match (filleted, reference) {
            (Some(f), Some(r)) => classify_solid(f, r.volume()),
            _ => (Outcome::Refused, "missing solid".into()),
        }
    }));
    match outcome {
        Ok((outcome, detail)) => CaseResult {
            name,
            outcome,
            detail,
        },
        Err(_) => CaseResult {
            name,
            outcome: Outcome::Crash,
            detail: "panic escaped harness".into(),
        },
    }
}

fn run_kernel_case(name: &str, query: EdgeQuery, keys: Vec<BlendKey>) -> CaseResult {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let cube = Solid::cube(10.0, 10.0, 10.0);
        let ref_volume = cube.volume();
        match cube.edge_blend(&query, &keys) {
            Ok(blended) => classify_solid(&blended, ref_volume),
            // The kernel now names its refusals instead of silently
            // returning the input — that's `Refused`, not `NoOp`.
            Err(e) => (Outcome::Refused, e.to_string()),
        }
    }));
    match outcome {
        Ok((outcome, detail)) => CaseResult {
            name: name.into(),
            outcome,
            detail,
        },
        Err(p) => CaseResult {
            name: name.into(),
            outcome: Outcome::Crash,
            detail: format!(
                "kernel panic: {}",
                p.downcast_ref::<&str>().copied().unwrap_or("<non-str>")
            ),
        },
    }
}

fn key(t: f64, size: f64, shape: f64) -> BlendKey {
    BlendKey {
        t,
        section: BlendSection { size, shape },
    }
}

fn kernel_cases() -> Vec<CaseResult> {
    vec![
        // Variable radius along a single edge.
        run_kernel_case(
            "kb-var-radius-one-edge",
            EdgeQuery::Near {
                point: Point3::new(0.0, 0.0, 0.0),
            },
            vec![key(0.0, 1.0, 1.0), key(1.0, 3.0, 1.0)],
        ),
        // Chamfer morphing into a fillet along one edge.
        run_kernel_case(
            "kb-chamfer-morph-fillet",
            EdgeQuery::Near {
                point: Point3::new(0.0, 0.0, 0.0),
            },
            vec![key(0.0, 2.0, 0.0), key(1.0, 2.0, 1.0)],
        ),
        // Four parallel vertical edges, constant fillet.
        run_kernel_case(
            "kb-vertical-edges-const",
            EdgeQuery::Direction {
                axis: Vec3::new(0.0, 0.0, 1.0),
                tol_deg: 5.0,
            },
            vec![key(0.0, 1.5, 1.0)],
        ),
        // Keyed profile over ALL edges: forces the per-edge loft path onto
        // vertex-adjacent selections (documented skip, must not crash).
        run_kernel_case(
            "kb-keyed-all-edges",
            EdgeQuery::All,
            vec![key(0.0, 1.0, 1.0), key(1.0, 2.0, 1.0)],
        ),
    ]
}

#[test]
fn fillet_torture_scoreboard() {
    let mut results: Vec<CaseResult> = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.extension().is_some_and(|x| x == "loon")).then_some(p)
        })
        .collect();
    paths.sort();
    for p in &paths {
        results.push(run_loon_case(p));
    }
    results.extend(kernel_cases());

    // Scoreboard
    let total = results.len();
    let n = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    println!("\n=== fillet torture track ===");
    for r in &results {
        println!(
            "  {:10} {:26} {}",
            format!("{:?}", r.outcome),
            r.name,
            r.detail
        );
    }
    println!(
        "  {}/{} success, {} no-op, {} refused, {} bad-geometry, {} crash",
        n(Outcome::Success),
        total,
        n(Outcome::NoOp),
        n(Outcome::Refused),
        n(Outcome::BadGeometry),
        n(Outcome::Crash),
    );

    // Never-regress: every case must be at least as good as its baseline.
    let mut errors = Vec::new();
    for r in &results {
        let Some((_, base)) = BASELINE.iter().find(|(n, _)| *n == r.name) else {
            errors.push(format!("{}: no baseline entry — add one", r.name));
            continue;
        };
        if r.outcome.rank() < base.rank() {
            errors.push(format!(
                "{}: regressed to {:?} (baseline {:?}) — {}",
                r.name, r.outcome, base, r.detail
            ));
        }
    }
    // And the corpus must stay in sync with the baseline list.
    for (name, _) in BASELINE {
        if !results.iter().any(|r| r.name == *name) {
            errors.push(format!("baseline entry {name} has no corpus case"));
        }
    }
    // Crashes and silently-bad geometry are never acceptable, baseline or not.
    for r in &results {
        if matches!(r.outcome, Outcome::Crash | Outcome::BadGeometry) {
            errors.push(format!(
                "{}: {:?} is never acceptable — {}",
                r.name, r.outcome, r.detail
            ));
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}
