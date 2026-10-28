//! Sphere ∩/∪/− cube seam quality, swept over the sphere's segment count.
//!
//! The vcad.io landing page renders these three booleans and found the seam
//! quality depended NON-MONOTONICALLY on the segment count (2026-10-27):
//! seg 32 grew zero-area ribbons and seam triangles tilted 45–70° off the
//! sphere's radial; seg 64 bridged sphere vertices to a cube corner (a
//! "curved" vertex at r 33.9 on an r 15 sphere); 96 was clean; and the
//! intersection silently lost ~34% of its volume (576 open edges) at seg ≥ 80.
//!
//! Every count must give a closed, faithful mesh: volume within 0.5% of the
//! closed form, zero open edges, every curved-face vertex on the sphere, and
//! every curved-face triangle within 25° of the surface normal.

use std::collections::HashMap;
use std::f64::consts::PI;

use vcad_kernel::Solid;
use vcad_kernel_tessellate::TriangleMesh;

const SEGMENTS: [u32; 10] = [24, 32, 40, 48, 56, 64, 80, 96, 112, 128];

struct Case {
    name: &'static str,
    build: fn(u32) -> Solid,
    center: [f64; 3],
    r: f64,
    /// Planes of the cube: (axis, value).
    planes: [(usize, f64); 6],
    exact: f64,
}

fn cube_planes(lo: f64, hi: f64) -> [(usize, f64); 6] {
    [(0, lo), (0, hi), (1, lo), (1, hi), (2, lo), (2, hi)]
}

fn sphere_vol(r: f64) -> f64 {
    4.0 / 3.0 * PI * r.powi(3)
}

fn cases() -> Vec<Case> {
    let octant = sphere_vol(15.0) / 8.0;
    let cap = |r: f64, h: f64| PI * h * h * (3.0 * r - h) / 3.0;
    vec![
        Case {
            name: "union",
            build: |seg| {
                Solid::cube(24.0, 24.0, 24.0)
                    .union(&Solid::sphere(15.0, seg).translate(24.0, 24.0, 24.0))
            },
            center: [24.0; 3],
            r: 15.0,
            planes: cube_planes(0.0, 24.0),
            exact: 24f64.powi(3) + sphere_vol(15.0) - octant,
        },
        Case {
            name: "difference",
            build: |seg| {
                Solid::cube(24.0, 24.0, 24.0)
                    .difference(&Solid::sphere(15.0, seg).translate(24.0, 24.0, 24.0))
            },
            center: [24.0; 3],
            r: 15.0,
            planes: cube_planes(0.0, 24.0),
            exact: 24f64.powi(3) - octant,
        },
        Case {
            name: "intersection",
            build: |seg| {
                Solid::cube(24.0, 24.0, 24.0)
                    .translate(-12.0, -12.0, -12.0)
                    .intersection(&Solid::sphere(16.0, seg))
            },
            center: [0.0; 3],
            r: 16.0,
            planes: cube_planes(-12.0, 12.0),
            exact: sphere_vol(16.0) - 6.0 * cap(16.0, 4.0),
        },
    ]
}

fn vert(m: &TriangleMesh, i: u32) -> [f64; 3] {
    let k = i as usize * 3;
    [
        m.vertices[k] as f64,
        m.vertices[k + 1] as f64,
        m.vertices[k + 2] as f64,
    ]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

#[derive(Debug, Default)]
struct Report {
    volume: f64,
    vol_err: f64,
    open_edges: usize,
    worst_radius_err: f64,
    worst_tilt_deg: f64,
    sag_tol: f64,
}

fn measure(case: &Case, seg: u32) -> Report {
    let solid = (case.build)(seg);
    let mesh = solid.to_mesh(seg);
    let tris: Vec<[u32; 3]> = mesh
        .indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();

    let mut volume = 0.0;
    for t in &tris {
        let (a, b, c) = (vert(&mesh, t[0]), vert(&mesh, t[1]), vert(&mesh, t[2]));
        volume += dot(a, cross(b, c)) / 6.0;
    }

    // Open edges, keyed by quantized position so per-face vertex
    // duplication doesn't read as a boundary. Zero-area triangles are kept:
    // a sliver still has to pair its edges.
    let q = |p: [f64; 3]| {
        [
            (p[0] * 1e4).round() as i64,
            (p[1] * 1e4).round() as i64,
            (p[2] * 1e4).round() as i64,
        ]
    };
    let mut edges: HashMap<([i64; 3], [i64; 3]), i32> = HashMap::new();
    for t in &tris {
        for k in 0..3 {
            let (a, b) = (q(vert(&mesh, t[k])), q(vert(&mesh, t[(k + 1) % 3])));
            if a == b {
                continue;
            }
            // +1 for a→b, −1 for b→a: a closed oriented surface sums to 0.
            if a < b {
                *edges.entry((a, b)).or_default() += 1;
            } else {
                *edges.entry((b, a)).or_default() -= 1;
            }
        }
    }
    let open_edges = edges.values().filter(|&&n| n != 0).count();

    // Curved triangles: not lying in one cube plane.
    let on_plane = |p: [f64; 3], (ax, v): (usize, f64)| (p[ax] - v).abs() < 1e-3;
    let r = case.r;
    let sag_tol = r * (1.0 - (PI / seg as f64).cos()) + 1e-3 * r;
    let mut worst_radius_err = 0.0f64;
    let mut worst_tilt_deg = 0.0f64;
    for t in &tris {
        let p = [vert(&mesh, t[0]), vert(&mesh, t[1]), vert(&mesh, t[2])];
        if case
            .planes
            .iter()
            .any(|&pl| p.iter().all(|&v| on_plane(v, pl)))
        {
            continue;
        }
        for v in p {
            worst_radius_err = worst_radius_err.max((norm(sub(v, case.center)) - r).abs());
        }
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let area2 = norm(n);
        let centroid = [
            (p[0][0] + p[1][0] + p[2][0]) / 3.0,
            (p[0][1] + p[1][1] + p[2][1]) / 3.0,
            (p[0][2] + p[1][2] + p[2][2]) / 3.0,
        ];
        let radial = sub(centroid, case.center);
        // A zero-area sliver has no normal; it reads as maximally tilted.
        let tilt = if area2 < 1e-9 {
            90.0
        } else {
            let mut c = dot(n, radial) / (area2 * norm(radial));
            if solid_is_inward(case) {
                c = -c;
            }
            c.clamp(-1.0, 1.0).acos().to_degrees()
        };
        worst_tilt_deg = worst_tilt_deg.max(tilt);
    }

    Report {
        volume,
        vol_err: (volume - case.exact).abs() / case.exact,
        open_edges,
        worst_radius_err,
        worst_tilt_deg,
        sag_tol,
    }
}

/// The difference's sphere face is a cavity wall: its outward normal points
/// toward the sphere centre.
fn solid_is_inward(case: &Case) -> bool {
    case.name == "difference"
}

#[test]
fn sphere_cube_seam_sweep() {
    let mut failures = Vec::new();
    for case in cases() {
        for seg in SEGMENTS {
            let r = measure(&case, seg);
            eprintln!(
                "{:<12} seg {:>3}: vol {:>9.1} (exact {:>9.1}, err {:.3}%) open {:>3} \
                 radius err {:.4} (tol {:.4}) tilt {:.1}°",
                case.name,
                seg,
                r.volume,
                case.exact,
                r.vol_err * 100.0,
                r.open_edges,
                r.worst_radius_err,
                r.sag_tol,
                r.worst_tilt_deg,
            );
            if r.vol_err > 0.005
                || r.open_edges != 0
                || r.worst_radius_err > r.sag_tol
                || r.worst_tilt_deg > 25.0
            {
                failures.push(format!("{} seg {}: {:?}", case.name, seg, r));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
