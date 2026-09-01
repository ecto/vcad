//! Synthetic STL fixtures — known-good and known-bad.
//!
//! The issue this crate answers records an acceptance protocol learned the
//! hard way: a checker whose sign was inverted passed *vacuously* for a whole
//! revision because nothing ever proved it could fail. So every check ships a
//! mesh it must reject, and the tests assert the specific diagnosis, not just
//! a non-zero exit.

#![allow(dead_code)]

use std::io::Write;

/// Axis-aligned box as 12 triangles, wound outward (CCW seen from outside).
pub fn cuboid(min: [f64; 3], max: [f64; 3]) -> Vec<[[f64; 3]; 3]> {
    let (x0, y0, z0) = (min[0], min[1], min[2]);
    let (x1, y1, z1) = (max[0], max[1], max[2]);
    let v = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ];
    let quads = [
        [0, 3, 2, 1], // bottom (-Z)
        [4, 5, 6, 7], // top (+Z)
        [0, 1, 5, 4], // -Y
        [1, 2, 6, 5], // +X
        [2, 3, 7, 6], // +Y
        [3, 0, 4, 7], // -X
    ];
    let mut tris = Vec::with_capacity(12);
    for q in quads {
        tris.push([v[q[0]], v[q[1]], v[q[2]]]);
        tris.push([v[q[0]], v[q[2]], v[q[3]]]);
    }
    tris
}

/// Extrude a simple polygon in the XZ plane along Y into a closed,
/// manifold solid. Fixtures that need internal features (a portal, a T) are
/// built this way rather than by stacking boxes: two boxes sharing a face are
/// non-manifold and the checker would — correctly — reject them for the wrong
/// reason.
///
/// `profile` is counter-clockwise in (x, z).
pub fn prism(profile: &[[f64; 2]], y0: f64, y1: f64) -> Vec<[[f64; 3]; 3]> {
    let cap = triangulate(profile);
    let mut tris = Vec::new();
    // -Y cap, wound so its normal points at -Y
    for [a, b, c] in &cap {
        tris.push([
            [profile[*a][0], y0, profile[*a][1]],
            [profile[*c][0], y0, profile[*c][1]],
            [profile[*b][0], y0, profile[*b][1]],
        ]);
    }
    for [a, b, c] in &cap {
        tris.push([
            [profile[*a][0], y1, profile[*a][1]],
            [profile[*b][0], y1, profile[*b][1]],
            [profile[*c][0], y1, profile[*c][1]],
        ]);
    }
    let n = profile.len();
    for i in 0..n {
        let p = profile[i];
        let q = profile[(i + 1) % n];
        let (a, b) = ([p[0], y0, p[1]], [q[0], y0, q[1]]);
        let (c, d) = ([q[0], y1, q[1]], [p[0], y1, p[1]]);
        tris.push([a, b, c]);
        tris.push([a, c, d]);
    }
    tris
}

/// Ear clipping for a simple CCW polygon.
fn triangulate(poly: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..poly.len()).collect();
    let mut out = Vec::new();
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross(a, b, c) <= 1e-12 {
                continue; // reflex
            }
            let contains = idx.iter().any(|&k| {
                if k == ia || k == ib || k == ic {
                    return false;
                }
                let p = poly[k];
                cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
            });
            if contains {
                continue;
            }
            out.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}

/// Write a binary STL. Facet normals are recomputed from the winding, which is
/// what any real exporter does.
pub fn write_stl(path: &std::path::Path, tris: &[[[f64; 3]; 3]]) {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&[0u8; 80]);
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in tris {
        let u = [
            t[1][0] - t[0][0],
            t[1][1] - t[0][1],
            t[1][2] - t[0][2],
        ];
        let v = [
            t[2][0] - t[0][0],
            t[2][1] - t[0][1],
            t[2][2] - t[0][2],
        ];
        let mut n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n = [n[0] / len, n[1] / len, n[2] / len];
        }
        for c in n {
            out.extend_from_slice(&(c as f32).to_le_bytes());
        }
        for vert in t {
            for c in vert {
                out.extend_from_slice(&(*c as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&[0u8; 2]);
    }
    // Tests run in parallel and several share a fixture; write to a private
    // temp file and rename, so no reader ever sees a half-written STL.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{seq}.part", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp).expect("create fixture");
        f.write_all(&out).expect("write fixture");
    }
    std::fs::rename(&tmp, path).expect("publish fixture");
}

/// Unique scratch path for a fixture, cleaned up by the OS temp dir.
pub fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("vcad-printcheck-fixtures");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir.join(format!("{name}.stl"))
}

pub fn build(name: &str, tris: Vec<[[f64; 3]; 3]>) -> std::path::PathBuf {
    let p = scratch(name);
    write_stl(&p, &tris);
    p
}

// --- known-good --------------------------------------------------------

/// A plain 10 mm cube sitting on the bed. Nothing to complain about.
pub fn good_cube() -> std::path::PathBuf {
    build("good_cube", cuboid([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]))
}

/// Two 2 mm pillars carrying a roof with a 3 mm clear span — inside the 4 mm
/// bridge convention, so this must PASS while reporting the bridge.
pub fn good_bridge() -> std::path::PathBuf {
    build("good_bridge", portal(3.0))
}

/// A portal: two 2 mm legs 6 mm tall carrying a 2 mm roof, with a clear span
/// of `gap` mm between them. One closed solid, no coincident faces.
fn portal(gap: f64) -> Vec<[[f64; 3]; 3]> {
    let w = 2.0 + gap + 2.0;
    let profile = [
        [0.0, 0.0],
        [2.0, 0.0],
        [2.0, 6.0],
        [2.0 + gap, 6.0],
        [2.0 + gap, 0.0],
        [w, 0.0],
        [w, 8.0],
        [0.0, 8.0],
    ];
    prism(&profile, 0.0, 6.0)
}

// --- known-bad ---------------------------------------------------------

/// Two stacked slabs with a 0.05 mm horizontal gap: the exact rana finding
/// #10 defect — a crack a slicer reads as a floating layer, far too thin to
/// be a deliberate channel.
pub fn crack_005() -> std::path::PathBuf {
    let mut t = cuboid([0.0, 0.0, 0.0], [10.0, 10.0, 5.0]);
    t.extend(cuboid([0.0, 0.0, 5.05], [10.0, 10.0, 10.0]));
    build("crack_005", t)
}

/// A 4 mm cube hanging 3 mm above a base plate, touching nothing.
pub fn floating_island() -> std::path::PathBuf {
    let mut t = cuboid([0.0, 0.0, 0.0], [20.0, 20.0, 2.0]);
    t.extend(cuboid([8.0, 8.0, 5.0], [12.0, 12.0, 9.0]));
    build("floating_island", t)
}

/// A 0.2 mm wall — half the 0.4 mm nozzle. Invisible to any vertical
/// analysis; this is the comb wall that shipped once and only failed on the
/// printer.
pub fn thin_wall_02() -> std::path::PathBuf {
    // T profile: a 10x1 base with a 0.2 mm fin standing 9 mm off it.
    let profile = [
        [0.0, 0.0],
        [10.0, 0.0],
        [10.0, 1.0],
        [5.1, 1.0],
        [5.1, 10.0],
        [4.9, 10.0],
        [4.9, 1.0],
        [0.0, 1.0],
    ];
    build("thin_wall_02", prism(&profile, 0.0, 10.0))
}

/// Two pillars 12 mm apart carrying a roof: an unsupported span three times
/// the 4 mm bridge convention.
pub fn overlong_bridge() -> std::path::PathBuf {
    build("overlong_bridge", portal(12.0))
}

/// A cube with one facet deleted — watertight nowhere near the hole.
pub fn holed_cube() -> std::path::PathBuf {
    let mut t = cuboid([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
    t.remove(3);
    build("holed_cube", t)
}
