//! The invariant on the real board: after import, no two pads of *different*
//! nets may overlap.
//!
//! This is the assertion that fails loudly on the original bug. The KiCad
//! importer stored pad angles absolutely while every geometry consumer composes
//! `fp.rotation + pad.rotation`, so every rotated footprint had its rotation
//! counted twice; on the CM5 fixture that turned fine-pitch pads into
//! overlapping copper and produced 648 phantom DRC violations.
//!
//! The fixture is an 11MB reverse-engineered third-party board and is not
//! committed (`.scratch/` is gitignored), and the run takes far longer than the
//! unit suite, so this test is `#[ignore]`d. Run it with:
//!
//! ```text
//! cargo test -p vcad-ecad-invariants --test cm5_no_overlap -- --ignored
//! ```
//!
//! Point it at a copy elsewhere with `VCAD_CM5_PCB=/path/to/CM5RevEng.kicad_pcb`.

use std::path::PathBuf;

use vcad_ecad_invariants::PadRect;

/// Pad pairs that overlap in the *source* board, not because of anything vcad
/// does to them. CM5RevEng is reverse-engineered, and these nine pairs are
/// components placed on top of each other in the file itself — absolute zero is
/// not achievable on an imported fixture, so the invariant is "no overlap
/// beyond this pinned baseline".
///
/// The evidence that they are not orientation bugs: their centre separations
/// are 0.01mm to 0.74mm against pads 0.25mm to 1.3mm wide — the copper simply
/// coincides — and `R53.2/C174.1` is a pair where *both* footprints sit at 0
/// degrees, which no rotation handling, right or wrong, can explain.
///
/// Shrinking this list is good news. Growing it is the regression this test
/// exists to catch: adding an entry needs the same kind of evidence.
const KNOWN_SOURCE_OVERLAPS: &[(&str, &str)] = &[
    ("C12.2", "C13.1"),
    ("C138.1", "C139.2"),
    ("C139.1", "C68.2"),
    ("C136.2", "C69.1"),
    ("C138.2", "C76.2"),
    ("C174.1", "R53.2"),
    ("C3.1", "U6.5"),
    ("C3.1", "U6.6"),
    ("C216.1", "Y2.1"),
];

/// Order-independent key for a pad pair.
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn fixture_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("VCAD_CM5_PCB") {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    // CARGO_MANIFEST_DIR is <repo>/crates/vcad-ecad-invariants.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .to_path_buf();
    let p = root.join(".scratch/CM5RevEng.kicad_pcb");
    p.exists().then_some(p)
}

#[test]
#[ignore = "needs the uncommitted 11MB CM5 fixture; run with --ignored"]
fn cm5_pads_of_different_nets_do_not_overlap() {
    let Some(path) = fixture_path() else {
        panic!(
            "CM5 fixture not found. Place it at .scratch/CM5RevEng.kicad_pcb \
             or set VCAD_CM5_PCB."
        );
    };
    let text = std::fs::read_to_string(&path).expect("read fixture");
    let pcb = vcad_ecad_symbols::parse_kicad_pcb(&text).expect("parse fixture");

    // Flatten to (net, label, rect), keeping only netted pads — an unnetted pad
    // cannot short anything.
    struct P {
        net: String,
        label: String,
        rect: PadRect,
        /// Copper layers this pad sits on. Two pads on opposite sides may
        /// legitimately occupy the same XY.
        layers: Vec<vcad_ir::ecad::PcbLayer>,
    }
    let pads: Vec<P> = pcb
        .footprints
        .iter()
        .flat_map(|fp| {
            fp.pads.iter().filter_map(move |pad| {
                let net = pad.net.clone()?;
                if net.is_empty() {
                    return None;
                }
                let layers: Vec<_> = pad
                    .layers
                    .iter()
                    .copied()
                    .filter(|l| l.is_copper())
                    .collect();
                if layers.is_empty() {
                    return None;
                }
                Some(P {
                    net,
                    label: format!("{}.{}", fp.reference, pad.number),
                    rect: PadRect::of(fp, pad),
                    layers,
                })
            })
        })
        .collect();
    assert!(pads.len() > 1000, "fixture looks wrong: {} pads", pads.len());

    // Bucket by a grid cell so this is a sweep, not an O(n^2) scan over ~9k
    // pads. Cell size covers the largest pad, so any overlapping pair shares
    // or neighbours a cell.
    let reach = pads
        .iter()
        .map(|p| p.rect.half_w.hypot(p.rect.half_h))
        .fold(0.0f64, f64::max);
    let cell = (reach * 2.0).max(1.0);
    let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);

    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = Default::default();
    for (i, p) in pads.iter().enumerate() {
        grid.entry(key(p.rect.center.x, p.rect.center.y))
            .or_default()
            .push(i);
    }

    let mut overlaps: Vec<String> = Vec::new();
    for (&(cx, cy), bucket) in &grid {
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(other) = grid.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &i in bucket {
                    for &j in other {
                        if j <= i {
                            continue;
                        }
                        let (a, b) = (&pads[i], &pads[j]);
                        if a.net == b.net {
                            continue;
                        }
                        if !a.layers.iter().any(|l| b.layers.contains(l)) {
                            continue;
                        }
                        if a.rect.overlaps(&b.rect) {
                            let key = pair_key(&a.label, &b.label);
                            if KNOWN_SOURCE_OVERLAPS
                                .iter()
                                .any(|&(x, y)| pair_key(x, y) == key)
                            {
                                continue;
                            }
                            overlaps.push(format!(
                                "{} ({}) overlaps {} ({})",
                                a.label, a.net, b.label, b.net
                            ));
                        }
                    }
                }
            }
        }
    }
    overlaps.sort();
    overlaps.dedup();

    assert!(
        overlaps.is_empty(),
        "{} different-net pad pairs overlap after import, beyond the {} pinned \
         source-board pairs — pad orientation is wrong somewhere in the import \
         path. First 20:\n{}",
        overlaps.len(),
        KNOWN_SOURCE_OVERLAPS.len(),
        overlaps
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
