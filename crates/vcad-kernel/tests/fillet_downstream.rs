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
    let result = Solid::cube(40.0, 40.0, 40.0)
        .shell(2.0)
        .expect("2mm shell fits");
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
    let filleted = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
    assert_eq!(open_edges(&filleted), 0);
    assert!(filleted.volume() < 1e6);
}

/// Case 3: shelling a filleted box. The cavity is the outer surface
/// offset inward, i.e. the same box 2 mm smaller with a 2 mm smaller
/// fillet — so the wall volume must equal the difference of the two
/// filleted solids, and the result must be watertight.
#[test]
fn shell_of_filleted_box_is_watertight_and_hollow() {
    let outer = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
    let inner = Solid::cube(96.0, 96.0, 96.0)
        .fillet(10.0)
        .expect("r=10 fits a 96mm cube");
    let shelled = outer.shell(2.0).expect("2mm shell fits");

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
    let filleted = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
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
/// The duplicate-patch half is fixed: `split_spherical_face_by_circle`
/// used to replace a patch's loop with the new circle unconditionally,
/// so three cutting planes on one corner left four identical,
/// overlapping copies of it — the reported 5.7×-too-large volume. The
/// clip now walks the patch as a spherical polygon, and a circle that
/// misses the patch reports a clean no-op instead of falling through to
/// the whole-sphere path that minted the copies.
///
/// Volume went 476028 → 105056 against a Monte-Carlo truth of 86618.
/// The remainder was a family of multi-contact trims — a circle
/// crossing a planar face's boundary more than twice, a circle
/// inscribed tangentially in a grid cell, and a circle crossing a
/// spherical patch's boundary more than twice — all of which now trace
/// their regions along the true arc (see `split_planar_face_by_multi_arc`,
/// `split_planar_face_inscribed_circle`, and `clip_spherical_face_multi`
/// in vcad-kernel-booleans). The watertight + volume end state is
/// asserted by `difference_with_filleted_subject_is_watertight` below.
#[test]
fn difference_with_filleted_subject_has_no_duplicate_patches() {
    let filleted = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    let result = filleted.difference(&inner);

    let brep = result.as_brep().expect("brep result");
    let topo = &brep.topology;
    let shell = &topo.shells[topo.solids[brep.solid_id].outer_shell];
    let mut boundaries: Vec<String> = Vec::new();
    for &fid in &shell.faces {
        let face = &topo.faces[fid];
        if format!(
            "{:?}",
            brep.geometry.surfaces[face.surface_index].surface_type()
        ) != "Sphere"
        {
            continue;
        }
        let key: String = topo
            .loop_vertices(face.outer_loop)
            .iter()
            .map(|v| {
                let p = topo.vertices[*v].point;
                format!("{:.3},{:.3},{:.3};", p.x, p.y, p.z)
            })
            .collect();
        assert!(
            !boundaries.contains(&key),
            "two sphere faces share a boundary loop — a re-split \
             discarded an earlier trim"
        );
        boundaries.push(key);
    }

    let volume = result.volume();
    assert!(volume.is_finite() && volume > 0.0, "volume {volume}");
    // Was 5.5× the Monte-Carlo truth when the corners were duplicated.
    assert!(
        volume < 86_618.0 * 1.5,
        "{volume} is far above the Monte-Carlo truth 86618 — corner \
         patches are being counted more than once again"
    );
}

/// Case 4's end state: every fillet seam of the cut is trimmed along the
/// true arc on both sides, so the shell closes and the volume lands on
/// the truth — 86618 mm³ is a 20M-sample Monte-Carlo integration of the
/// rounded-box SDF minus the inner cube.
#[test]
fn difference_with_filleted_subject_is_watertight() {
    let filleted = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    let result = filleted.difference(&inner);

    assert_eq!(open_edges(&result), 0);
    assert!(
        (result.volume() - 86_618.0).abs() < 86_618.0 * 0.05,
        "{} vs Monte-Carlo truth 86618",
        result.volume()
    );
}

/// Booleans against a filleted subject must come out with OUTWARD
/// windings. `Solid::volume()` takes the absolute value of the signed
/// divergence-theorem sum, so a globally inverted mesh — the reported
/// 2026-07-25 symptom, filleted parts turning inside-out under
/// difference — still reports a plausible positive volume. Computing
/// the signed sum directly pins the orientation.
#[test]
fn boolean_against_filleted_subject_keeps_outward_winding() {
    let filleted = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    for result in [filleted.difference(&inner), filleted.union(&inner)] {
        let mesh = result.to_mesh(SEGMENTS);
        let (verts, indices) = (&mesh.vertices, &mesh.indices);
        let mut signed = 0.0;
        for tri in indices.chunks(3) {
            let p = |i: u32| {
                let i = i as usize * 3;
                [verts[i] as f64, verts[i + 1] as f64, verts[i + 2] as f64]
            };
            let (v0, v1, v2) = (p(tri[0]), p(tri[1]), p(tri[2]));
            signed += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        signed /= 6.0;
        assert!(
            signed > 0.0,
            "boolean on a filleted solid produced an inward-facing mesh \
             (signed volume {signed})"
        );
        // `result.volume()` tessellates at the solid's own segment count,
        // ours at SEGMENTS — only chord error apart. A subset of inverted
        // faces would subtract their volume twice and blow way past 1%.
        assert!(
            (signed - result.volume()).abs() < result.volume() * 0.01,
            "signed volume {signed} disagrees with |volume| {} — some \
             faces wind inward",
            result.volume()
        );
    }
}

/// The same operations against a CHAMFERED box are exact and watertight
/// — chamfer emits planar faces only, so it never hit any of this.
#[test]
fn chamfered_box_survives_shell_and_difference() {
    let chamfered = Solid::cube(100.0, 100.0, 100.0)
        .chamfer(12.0)
        .expect("12mm chamfer fits a 100mm cube");
    assert_eq!(open_edges(&chamfered), 0);
    assert_eq!(
        open_edges(&chamfered.shell(2.0).expect("2mm shell fits")),
        0
    );
    let inner = Solid::cube(96.0, 96.0, 96.0).translate(2.0, 2.0, 2.0);
    assert_eq!(open_edges(&chamfered.difference(&inner)), 0);
}

fn fillet_boolean_signature() -> String {
    let filleted = Solid::cube(100.0, 100.0, 100.0)
        .fillet(12.0)
        .expect("r=12 fits a 100mm cube");
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
