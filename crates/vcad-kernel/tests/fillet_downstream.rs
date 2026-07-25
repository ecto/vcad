//! Booleans and shells consuming a FILLETED solid.
//!
//! Reported 2026-07-25: `shell` and `difference` against a filleted box
//! returned wrong volumes and non-watertight meshes, while the same
//! operations on plain primitives were exact. The two controls here pin
//! the exact answers; the filleted cases pin watertightness and the
//! volume the geometry actually has.

use vcad_kernel::Solid;

/// Tessellation density used for every measurement below.
const SEGMENTS: u32 = 64;

fn open_edges(solid: &Solid) -> usize {
    solid.to_mesh(SEGMENTS).boundary_edges().len()
}

/// Control: difference of two plain boxes is exact and watertight.
#[test]
fn difference_of_plain_boxes_is_exact() {
    let cut = Solid::cube(8.0, 8.0, 8.0).translate(1.0, 1.0, 1.0);
    let result = Solid::cube(10.0, 10.0, 10.0).difference(&cut);
    assert!(
        (result.volume() - 488.0).abs() < 1e-6,
        "{}",
        result.volume()
    );
    assert_eq!(open_edges(&result), 0);
}

/// Control: shelling a plain box is exact and watertight.
#[test]
fn shell_of_plain_box_is_exact() {
    let result = Solid::cube(40.0, 40.0, 40.0).shell(2.0);
    let expected = 40.0f64.powi(3) - 36.0f64.powi(3);
    assert!(
        (result.volume() - expected).abs() < 1e-6,
        "{}",
        result.volume()
    );
    assert_eq!(open_edges(&result), 0);
}

/// A filleted box is watertight on its own — the defect was never in
/// what `fillet` emits, so this stays the baseline the cases below are
/// measured against.
#[test]
fn filleted_box_is_watertight() {
    let filleted = Solid::cube(100.0, 100.0, 100.0).fillet(12.0);
    assert_eq!(open_edges(&filleted), 0);
    assert!(filleted.volume() < 1e6);
}

/// Case 3: shelling a filleted box. The cavity is the outer surface
/// offset inward, i.e. the same box 2 mm smaller with a 2 mm smaller
/// fillet — so the wall volume must equal the difference of the two
/// filleted solids, and the result must be watertight.
#[test]
fn shell_of_filleted_box_is_watertight_and_hollow() {
    let outer = Solid::cube(100.0, 100.0, 100.0).fillet(12.0);
    let inner = Solid::cube(96.0, 96.0, 96.0).fillet(10.0);
    let shelled = outer.shell(2.0);

    assert_eq!(open_edges(&shelled), 0, "shell of a filleted box cracked");
    let expected = outer.volume() - inner.volume();
    assert!(
        (shelled.volume() - expected).abs() < expected * 1e-3,
        "wall volume {} vs expected {expected}",
        shelled.volume()
    );
    // The reported symptom: a wall volume ABOVE the unfilleted upper
    // bound (100³ − 96³), which no amount of corner rounding can reach.
    assert!(shelled.volume() < 115_264.0);
}

/// A pocket cut into a filleted box's flat face, clear of every blend.
/// The cut splits the top face and pushes its new vertices onto the
/// neighboring blends' end rulings; dropping them collapsed those
/// blends to flat quads and lost 57 mm³ of material per corner.
#[test]
fn pocket_in_filleted_box_is_exact() {
    let filleted = Solid::cube(100.0, 100.0, 100.0).fillet(12.0);
    let pocket = Solid::cube(20.0, 20.0, 20.0).translate(40.0, 40.0, 90.0);
    let result = filleted.difference(&pocket);

    assert_eq!(open_edges(&result), 0, "pocket cut cracked the blends");
    // The pocket is 20×20 and 10 deep inside the solid.
    let expected = filleted.volume() - 4000.0;
    assert!(
        (result.volume() - expected).abs() < 1e-3,
        "{} vs {expected}",
        result.volume()
    );
}

/// Case 4: a cut whose planes slice every blend and corner at once.
///
/// KNOWN BROKEN — not fixed here, and deliberately not papered over.
///
/// Diagnosis: `split_spherical_face_by_circle` replaces the face's loop
/// with the new circle unconditionally, discarding whatever trim the
/// face already carried. Three cutting planes on one corner patch
/// therefore leave FOUR identical, fully-overlapping copies of it —
/// which is where the reported 5.7×-too-large volume comes from. A
/// clipping implementation (walk the patch as a spherical polygon, solve
/// the crossings on the great arcs, close both sides with one shared arc
/// of the cutting circle) removes the duplicates, but on its own it only
/// takes this case from 24 duplicate patches to 20 and leaves it just as
/// cracked, because a second, independent defect sits underneath: the
/// boundary sampling of a trimmed curved face does not match its
/// neighbors'. That one is not fillet-specific — a bore through a PLAIN
/// box cracks the same way. Both want fixing together.
///
/// The `#[ignore]`d test below is the target state; this one pins what
/// holds today so the case cannot silently get worse.
#[test]
fn difference_with_filleted_subject_still_cuts() {
    let filleted = Solid::cube(100.0, 100.0, 100.0).fillet(12.0);
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    let result = filleted.difference(&inner);

    let volume = result.volume();
    assert!(volume.is_finite() && volume > 0.0, "volume {volume}");
    // The reported symptom included INVERTED winding (a net negative
    // volume) on the humanoid assembly; positive-and-shrinking is the
    // floor this case must keep.
    assert!(
        volume < filleted.volume(),
        "cut added material: {volume} vs {}",
        filleted.volume()
    );
}

/// Case 4's target state. Un-`ignore` this when the spherical-patch clip
/// and the curved-trim boundary sampling land — 86618 mm³ is a 20M-sample
/// Monte-Carlo integration of the rounded-box SDF minus the inner cube.
#[test]
#[ignore = "known broken: spherical re-split discards prior trims, and \
            trimmed curved faces don't share boundary samples with their \
            neighbors (a bore through a plain box cracks identically)"]
fn difference_with_filleted_subject_is_watertight() {
    let filleted = Solid::cube(100.0, 100.0, 100.0).fillet(12.0);
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    let result = filleted.difference(&inner);

    assert_eq!(open_edges(&result), 0);
    assert!(
        (result.volume() - 86_618.0).abs() < 86_618.0 * 0.05,
        "{} vs Monte-Carlo truth 86618",
        result.volume()
    );
}

/// The same operations against a CHAMFERED box are exact and watertight
/// — chamfer emits planar faces only, so it never hit any of this.
#[test]
fn chamfered_box_survives_shell_and_difference() {
    let chamfered = Solid::cube(100.0, 100.0, 100.0).chamfer(12.0);
    assert_eq!(open_edges(&chamfered), 0);
    assert_eq!(open_edges(&chamfered.shell(2.0)), 0);
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    assert_eq!(open_edges(&chamfered.difference(&inner)), 0);
}

fn fillet_boolean_signature() -> String {
    let filleted = Solid::cube(100.0, 100.0, 100.0).fillet(12.0);
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    let result = filleted.difference(&inner);
    let brep = result.as_brep().expect("brep result");
    let topo = &brep.topology;
    let shell = &topo.shells[topo.solids[brep.solid_id].outer_shell];
    let mut sig = String::new();
    for &fid in &shell.faces {
        let face = &topo.faces[fid];
        sig.push_str(&format!(
            "{:?}:",
            brep.geometry.surfaces[face.surface_index].surface_type()
        ));
        for v in topo.loop_vertices(face.outer_loop) {
            let p = topo.vertices[v].point;
            sig.push_str(&format!("{:.6},{:.6},{:.6};", p.x, p.y, p.z));
        }
    }
    sig
}

/// Child half of `fillet_then_boolean_is_deterministic` — prints the
/// signature when the parent re-runs this binary, and is inert
/// otherwise.
#[test]
fn print_fillet_boolean_signature() {
    if std::env::var("VCAD_DETERMINISM_CHILD").is_ok() {
        println!("SIG {}", fillet_boolean_signature());
    }
}

/// The pipeline must be reproducible ACROSS PROCESSES: face ordering
/// used to come out of HashMap walks (the primitives' twin pairing, the
/// fillet's corner blends, the boolean's split order), and since Rust
/// re-seeds its hasher per process, the same fillet-then-difference
/// returned a differently-ordered solid — with a different volume and a
/// different crack pattern — on every run. An in-process comparison
/// cannot see this: both halves would share one seed.
#[test]
fn fillet_then_boolean_is_deterministic() {
    let exe = std::env::current_exe().expect("test binary path");
    let run = || {
        let out = std::process::Command::new(&exe)
            .args(["print_fillet_boolean_signature", "--exact", "--nocapture"])
            .env("VCAD_DETERMINISM_CHILD", "1")
            .output()
            .expect("re-run test binary");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find(|l| l.starts_with("SIG "))
            .expect("child printed a signature")
            .to_string()
    };
    let (first, second) = (run(), run());
    assert_eq!(first, second, "fillet → boolean is not reproducible");
}
