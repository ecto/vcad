//! Real-data demo: three rana-60c parts posed from one assembly document.
//!
//! The transforms below are the ones the rana project maintains by hand in
//! `tools/probe-60c.py`'s `A` table — the dict that a probe script, a WebGL
//! viewer pack and a 3MF kit layout each keep their own copy of. Written as an
//! assembly document they exist once, and the mates that were previously prose
//! in `DESIGN-60c.md` ("flipped, clock 180", "rear rotor carrier at z5.1")
//! become checks.
//!
//! Run it against the exported STLs:
//!
//! ```text
//! cargo run -p vcad-kernel-assembly --example rana_60c -- ~/Developer/rana/exports/parts-60c
//! ```
//!
//! An optional second argument overrides the interference tolerance:
//!
//! ```text
//! cargo run -p vcad-kernel-assembly --example rana_60c -- <dir> 0.0
//! ```
//!
//! Note the `import-mesh-scaled 0.001` — `import-mesh` treats STL units as
//! metres (a URDF convention) and multiplies by 1000, so a millimetre STL has
//! to be scaled back down.

use vcad_kernel_assembly::{check_interference, check_mates, pose_document, InterferenceOptions};

/// rana models parts with deliberate overlaps up to this depth so unions print
/// without a hairline seam. Anything deeper is a real interference.
const MODELLING_OVERLAP_MM: f64 = 0.05;

fn document_source(dir: &str) -> String {
    let stl = |name: &str| {
        format!("[import-mesh-scaled 0.001 0.001 0.001 \"{dir}/rana-60c-{name}.stl\"]")
    };
    format!(
        r#"; rana-60c — mini assembly document (3 of 10 parts).
; Transforms are probe-60c.py's A table, which this file replaces as the
; source of truth.
[assembly
  #[[part "backplate" {backplate} "aluminum"]
    [part "rotor"     {rotor}     "abs_black"]]
  #[; datum A = backplate front face
    [instance "backplate" "backplate" 0.0 0.0 0.0]
    ; rear rotor carrier seats at z5.1
    [instance-exploded "rotor-rear"  "rotor" 0.0 0.0  5.1  0.0 0.0   0.0   0.0 0.0 -18.0]
    ; front rotor: FLIPPED about X and CLOCKED 180 — the pole-alignment fix.
    ; (100b's "clock 60" habit misaligns a 10-pole array by 12 degrees.)
    [instance-exploded "rotor-front" "rotor" 0.0 0.0 23.5  180.0 0.0 180.0  0.0 0.0 18.0]]
  #[]
  "backplate"]

; Both rotors turn on the sun axis.
[mate-coaxial "rotors-coaxial" "rotor-rear" "rotor-front" 0.0 0.0 1.0 0.001]
; The z-chain from DESIGN-60c.md, checked instead of read.
[mate-planar-offset "rotor-rear-seat" "backplate" "rotor-rear"  0.0 0.0 1.0  5.1 0.001]
[mate-planar-offset "rotor-span"      "rotor-rear" "rotor-front" 0.0 0.0 1.0 18.4 0.001]
; THE check: 10 poles per disc, 36 degree pitch, flipped and clocked 180.
[mate-pattern-phase "pole-phase" "rotor-rear" "rotor-front" 10.0 0.0 0.0 1.0 0.0 0.0 0.5]
"#,
        backplate = stl("backplate"),
        rotor = stl("rotor"),
    )
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: rana_60c <parts-60c directory>");
        std::process::exit(2);
    });
    let dir = dir.trim_end_matches('/');

    let source = document_source(dir);
    println!("=== assembly document ===\n{source}");

    let doc = match vcad_loon::eval_vcad(&source, None) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("loon evaluation failed: {e}");
            std::process::exit(1);
        }
    };
    let posed = match pose_document(&doc) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("posing failed: {e}");
            std::process::exit(1);
        }
    };

    println!("=== posed parts ===");
    for part in &posed.parts {
        let tris = part.mesh.num_triangles();
        match part.bounds() {
            Some((min, max)) => println!(
                "  {:<12} {:>6} tris   z {:>7.2} .. {:>7.2}   r <= {:.2}",
                part.instance_id,
                tris,
                min[2],
                max[2],
                max[0]
                    .abs()
                    .max(max[1].abs())
                    .max(min[0].abs())
                    .max(min[1].abs())
            ),
            None => println!("  {:<12} EMPTY", part.instance_id),
        }
    }

    println!("=== mates ===");
    let mut all_pass = true;
    match check_mates(&posed, &doc.mates) {
        Ok(checks) => {
            for c in &checks {
                all_pass &= c.pass;
                println!("  {}", c.summary());
            }
        }
        Err(e) => {
            eprintln!("  mate check failed: {e}");
            all_pass = false;
        }
    }

    let tolerance = std::env::args()
        .nth(2)
        .and_then(|a| a.parse().ok())
        .unwrap_or(MODELLING_OVERLAP_MM);
    let report = check_interference(&posed, &InterferenceOptions::with_tolerance(tolerance));
    println!("=== interference (tolerance {tolerance} mm) ===");
    for line in report.summary().lines() {
        println!("  {line}");
    }

    if !all_pass || !report.is_clean() {
        std::process::exit(1);
    }
}
