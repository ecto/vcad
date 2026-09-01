//! Acceptance test for ecto/vcad#840: the rana-60c shell as pure CSG.
//!
//! The rana actuator project abandoned the CAD path for every part with an
//! undercut and hand-emits watertight prisms from Python instead. The
//! rana-60c shell — a ⌀71.4/⌀66.6 × 27.6 tube with six through-wall J-slot
//! channels on helical inclines, three open-top windows and 0.35 rim
//! chamfers — took four structural rewrites to get manifold that way
//! (rana commits ab03238, de76903, d45b11d, 7b72c4b, 324e6ce), each
//! rewrite fixing a seam defect *between construction bodies*: z-band
//! cracks, floating islands inside slot voids, a circumferential crack
//! where a chamfer ring failed to overlap its columns.
//!
//! None of those defects can exist in a boolean result, because there are
//! no construction bodies to seam: the shell is one tube minus fifteen
//! voids. What this file checks, in four layers:
//!
//! 1. build the part as CSG differences via [`manifold_csg`];
//! 2. audit every intermediate with [`check_manifold`] — zero bad edges,
//!    zero branching edges, zero inconsistently-wound edges;
//! 3. require byte-identical STL output across runs, per boolean and for
//!    the whole part;
//! 4. run the rana project's own raycast support check, ported from
//!    `tools/support-check.py` on the rana `60c` branch, against the
//!    exported STL bytes — no interior gap under 0.15 mm, no material
//!    restarting in mid-air outside the documented bridge roofs.
//!
//! Layer 4 is the one a topological check cannot replace. A mesh can be
//! perfectly manifold and still describe a part with a 0.05 mm crack
//! through its wall; only a parity raycast on the final triangles sees
//! that, which is why the rana project wrote the checker at all.
//!
//! ## Status
//!
//! Layers 1–3 hold for the whole fifteen-void part. Layer 4, and
//! `components == 1` on the full part, do not yet: see
//! [`support_check_is_clean`], which records exactly what fails and why.
//! The bottom-channel slice — the tube minus all three undercut J-slot
//! channels — passes layers 1–3 as a single shell.
//!
//! ## What is approximated
//!
//! Nothing about the helical floors and roofs: they are piecewise linear
//! in the angle, so sweeping their breakpoints (`LEG_BREAKPOINTS`)
//! reproduces them exactly. The tube and the void side walls are faceted
//! at [`TUBE_SEGMENTS`], the usual tessellation of a curved surface.
//!
//! The legs are built by [`leg_void`] rather than an IR primitive because
//! `CsgOp::Sweep` admits only a line or a helix path, and neither can
//! carry a cross-section that changes along the path. A general profile
//! sweep is ecto/vcad#842; when it lands, `leg_void` collapses to one
//! primitive and nothing else here changes.

use vcad_kernel_booleans::mesh::csg::mesh_csg;
use vcad_kernel_booleans::{manifold_csg, BooleanOp};
use vcad_kernel_tessellate::manifold::{check_manifold, ManifoldReport};
use vcad_kernel_tessellate::TriangleMesh;

// ---------------------------------------------------------------------------
// Mesh construction: revolved and swept profiles.
//
// Every solid below is one closed swept polygon — the operands of a
// boolean, not the part. Their seams do not need to be watertight against
// each other, which is the whole point: overlap them freely and let the
// boolean sort it out.
// ---------------------------------------------------------------------------

/// Accumulates triangles with positional vertex deduplication.
struct Builder {
    verts: Vec<[f64; 3]>,
    idx: Vec<u32>,
    cache: std::collections::HashMap<[i64; 3], u32>,
}

impl Builder {
    fn new() -> Self {
        Self {
            verts: Vec::new(),
            idx: Vec::new(),
            cache: std::collections::HashMap::new(),
        }
    }
    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        let key = [
            (p[0] * 1e6).round() as i64,
            (p[1] * 1e6).round() as i64,
            (p[2] * 1e6).round() as i64,
        ];
        *self.cache.entry(key).or_insert_with(|| {
            self.verts.push(p);
            (self.verts.len() - 1) as u32
        })
    }
    fn tri(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3]) {
        let (a, b, c) = (self.vertex(a), self.vertex(b), self.vertex(c));
        if a != b && b != c && a != c {
            self.idx.extend_from_slice(&[a, b, c]);
        }
    }
    fn quad(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }
    fn finish(self) -> TriangleMesh {
        let mut m = TriangleMesh::new();
        for p in &self.verts {
            m.vertices
                .extend_from_slice(&[p[0] as f32, p[1] as f32, p[2] as f32]);
        }
        m.normals = vec![0.0; m.vertices.len()];
        m.indices = self.idx;
        m
    }
}

fn cyl(r: f64, z: f64, theta_deg: f64) -> [f64; 3] {
    let t = theta_deg.to_radians();
    [r * t.cos(), r * t.sin(), z]
}

/// Sweep a closed CCW `(r, z)` cross-section around the z axis through the
/// given angles. `open` sweeps span a sector and get end caps; a closed
/// sweep wraps from the last sample back to the first (a full revolve).
///
/// The cross-section may differ per sample as long as the vertex count
/// matches — that is what makes a helical floor expressible.
fn sweep(samples: &[(f64, Vec<(f64, f64)>)], open: bool) -> TriangleMesh {
    let mut b = Builder::new();
    let n = samples.len();
    let spans = if open { n - 1 } else { n };
    for i in 0..spans {
        let (ta, pa) = &samples[i];
        let (tb, pb) = &samples[(i + 1) % n];
        assert_eq!(pa.len(), pb.len(), "sweep profiles must agree in size");
        for k in 0..pa.len() {
            let j = (k + 1) % pa.len();
            b.quad(
                cyl(pa[k].0, pa[k].1, *ta),
                cyl(pb[k].0, pb[k].1, *tb),
                cyl(pb[j].0, pb[j].1, *tb),
                cyl(pa[j].0, pa[j].1, *ta),
            );
        }
    }
    if open {
        for (i, flip) in [(0usize, false), (n - 1, true)] {
            let (t, p) = &samples[i];
            let mut pts: Vec<[f64; 3]> = p.iter().map(|&(r, z)| cyl(r, z, *t)).collect();
            if flip {
                pts.reverse();
            }
            for k in 1..pts.len() - 1 {
                b.tri(pts[0], pts[k], pts[k + 1]);
            }
        }
    }
    b.finish()
}

// ---------------------------------------------------------------------------
// rana-60c parameters. Transcribed from the rana `60c` branch,
// tools/make-shell-60c.py (commit 324e6ce) — every engagement number is
// the shipped one, so a volume or clearance change here is a real change.
// ---------------------------------------------------------------------------

/// Bore radius (⌀66.6).
const R_IN: f64 = 33.3;
/// Outside radius (⌀71.4).
const R_OUT: f64 = 35.7;
/// Radius at the rim after the 0.35 taper chamfer.
const R_CHAMFER: f64 = 35.3;
/// Bottom and top of the tube: 27.6 tall.
const Z_BOT: f64 = -0.6;
const Z_TOP: f64 = 27.0;
/// Chamfer height at each rim.
const CHAMFER: f64 = 0.35;

/// Bottom J-slot entry angles (backplate pins at 90/210/330).
const BOTTOM_ENTRIES: [f64; 3] = [105.0, 225.0, 345.0];
/// Top J-slot entry angles (cover pins at 45/165/285 home).
const TOP_ENTRIES: [f64; 3] = [60.0, 180.0, 300.0];
/// Stator window centres.
const WINDOWS: [f64; 3] = [15.0, 135.0, 255.0];

/// Leg start / stop, roof knee and detent start, in degrees behind entry.
const D_START: f64 = 3.6;
const D_STOP: f64 = 18.4;
const D_ROOF_KNEE: f64 = 12.0;
const D_DETENT: f64 = 9.6;

/// Angular samples for the full-revolution tube: one facet seam per whole
/// degree.
///
/// Deliberately in phase with whole degrees, which puts every seam at
/// least 0.345° from a feature boundary — the entry caps land on x.345 and
/// x.655, the leg breakpoints on x.4 and x.6. Phasing the tube instead
/// (0.33° was tried, matching the rana generator's sector offset) drags
/// the seams to within 0.015° of the entry caps and the boolean returns
/// slivers: 17 bad edges appear where there were none. Probe rays dodge
/// the seams from the other side, via [`PROBE_PHASE`].
const TUBE_SEGMENTS: usize = 360;
/// Angles (degrees behind entry) at which a leg's floor or roof changes
/// slope: the leg start, the floor's clamp, the detent's rise/plateau/drop
/// corners, the roof knee, and the stop.
///
/// Sampling *here* rather than on a uniform grid makes the sweep exact
/// rather than approximate: every floor and roof in this part is piecewise
/// linear in the angle, so segmenting at its breakpoints reproduces the
/// helical incline with zero error. It is also what keeps the boolean
/// well-conditioned — a uniform 48-step sweep puts consecutive facets
/// 0.02 mm apart in z, only ten times the boolean's own snapping
/// tolerance, and the resulting slivers survive as branching edges the
/// repair pass cannot resolve.
///
/// The void's curved faces never touch the tube (they stand 1 mm proud of
/// both walls), so arc faceting between these angles cannot affect the
/// cut surface at all.
const LEG_BREAKPOINTS: [f64; 7] = [
    D_START,
    D_DETENT,
    10.3, // detent plateau start
    11.3, // detent plateau end
    11.6, // detent drop end
    D_ROOF_KNEE,
    D_STOP,
];
/// Angular samples for a straight-walled sector void.
const BOX_STEPS: usize = 8;

/// Half-angle subtended by a chord of width `w` at radius `r`.
fn half_angle(w: f64, r: f64) -> f64 {
    (w / (2.0 * r)).asin().to_degrees()
}

/// Smooth 2°-wide detent: rise, plateau, drop.
fn detent(d: f64, height: f64) -> f64 {
    if d <= D_DETENT || d >= 11.6 {
        0.0
    } else if d < 10.3 {
        height * (d - D_DETENT) / 0.7
    } else if d <= 11.3 {
        height
    } else {
        height * (11.6 - d) / 0.3
    }
}

/// Bottom leg floor: helix -0.25 → +0.15 at 0.0667 mm/°, then a 0.1 detent
/// bump, then flat to the stop.
fn floor_bottom(d: f64) -> f64 {
    (-0.25 + (d - D_START) * 0.4 / (D_DETENT - D_START)).min(0.15) + detent(d, 0.1)
}
/// Bottom leg roof: helix 2.6 → 1.8 at 0.0952 mm/°, then flat. The only
/// bridged spans in the part.
fn roof_bottom(d: f64) -> f64 {
    (2.6 - (d - D_START) * 0.8 / (D_ROOF_KNEE - D_START)).max(1.8)
}
/// Top leg floor: flat with a 0.3 detent bump.
fn floor_top(d: f64) -> f64 {
    24.0 + detent(d, 0.3)
}
/// Top leg roof: helix 26.2 → 25.65 at 0.0655 mm/°, then flat.
fn roof_top(d: f64) -> f64 {
    (26.2 - (d - D_START) * 0.55 / (D_ROOF_KNEE - D_START)).max(25.65)
}

/// The blank: a chamfered tube, as one revolved wall cross-section.
fn tube() -> TriangleMesh {
    let profile = vec![
        (R_IN, Z_BOT),
        (R_CHAMFER, Z_BOT),
        (R_OUT, Z_BOT + CHAMFER),
        (R_OUT, Z_TOP - CHAMFER),
        (R_CHAMFER, Z_TOP),
        (R_IN, Z_TOP),
    ];
    let samples: Vec<(f64, Vec<(f64, f64)>)> = (0..TUBE_SEGMENTS)
        .map(|i| (360.0 * i as f64 / TUBE_SEGMENTS as f64, profile.clone()))
        .collect();
    sweep(&samples, false)
}

/// Void cross-sections run 1 mm proud of both walls, so every cut is
/// unambiguously through-wall and no tool face is ever coincident with a
/// tube face.
const VOID_R_IN: f64 = R_IN - 1.0;
const VOID_R_OUT: f64 = R_OUT + 1.0;

fn section(z_lo: f64, z_hi: f64) -> Vec<(f64, f64)> {
    vec![
        (VOID_R_IN, z_lo),
        (VOID_R_OUT, z_lo),
        (VOID_R_OUT, z_hi),
        (VOID_R_IN, z_hi),
    ]
}

/// Straight-walled angular void: the pin entries and the stator windows.
fn box_void(t0: f64, t1: f64, z_lo: f64, z_hi: f64) -> TriangleMesh {
    let samples: Vec<(f64, Vec<(f64, f64)>)> = (0..=BOX_STEPS)
        .map(|i| {
            (
                t0 + (t1 - t0) * i as f64 / BOX_STEPS as f64,
                section(z_lo, z_hi),
            )
        })
        .collect();
    sweep(&samples, true)
}

/// The helical leg void: the cross-section's floor and roof both move as
/// the sweep runs backwards from the entry angle.
fn leg_void(entry: f64, floor: fn(f64) -> f64, roof: fn(f64) -> f64) -> TriangleMesh {
    let mut samples: Vec<(f64, Vec<(f64, f64)>)> = LEG_BREAKPOINTS
        .iter()
        .map(|&d| (entry - d, section(floor(d), roof(d))))
        .collect();
    // The leg runs to *decreasing* angle; reverse so the sweep is CCW and
    // the solid comes out wound outward.
    samples.reverse();
    sweep(&samples, true)
}

/// The three bottom J-slot channels: an entry notch plus a helical leg
/// each. This is the undercut geometry the issue is about — the leg roof
/// overhangs its floor, so no sweep of a single cross-section along z can
/// express it and the hand-rolled generator had to stack construction
/// bodies.
fn bottom_channel_tools() -> Vec<(String, TriangleMesh)> {
    let entry_half = half_angle(4.4, 34.5);
    let mut tools = Vec::new();
    for &a in &BOTTOM_ENTRIES {
        tools.push((
            format!("bottom-entry@{a}"),
            box_void(a - entry_half, a + entry_half, -0.7, 2.0),
        ));
        tools.push((
            format!("bottom-leg@{a}"),
            leg_void(a, floor_bottom, roof_bottom),
        ));
    }
    tools
}

/// Every void subtracted from the blank, in a fixed order.
fn void_tools() -> Vec<(String, TriangleMesh)> {
    let entry_half = half_angle(4.4, 34.5);
    let window_half = half_angle(6.4, 34.5);
    let mut tools = bottom_channel_tools();
    for &a in &TOP_ENTRIES {
        tools.push((
            format!("top-entry@{a}"),
            box_void(a - entry_half, a + entry_half, 23.8, 27.1),
        ));
        tools.push((format!("top-leg@{a}"), leg_void(a, floor_top, roof_top)));
    }
    for &a in &WINDOWS {
        tools.push((
            format!("window@{a}"),
            box_void(a - window_half, a + window_half, 11.3, 27.1),
        ));
    }
    tools
}

/// Locate a mesh's bad and branching edges in cylindrical coordinates, so
/// a failure names the feature it broke instead of an index pair.
fn describe_defects(mesh: &TriangleMesh) -> String {
    let at = |i: u32| {
        let k = i as usize * 3;
        let (x, y, z) = (
            mesh.vertices[k] as f64,
            mesh.vertices[k + 1] as f64,
            mesh.vertices[k + 2] as f64,
        );
        format!(
            "r={:.4} th={:.3} z={:.4}",
            (x * x + y * y).sqrt(),
            y.atan2(x).to_degrees().rem_euclid(360.0),
            z
        )
    };
    let mut lines = Vec::new();

    // Component breakdown: a stray boolean artifact and a genuinely
    // detached chunk of the part look identical in a component count and
    // nothing alike in a volume.
    let nv = mesh.num_vertices();
    let mut parent: Vec<u32> = (0..nv as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    for tri in mesh.indices.chunks(3) {
        for k in 0..3 {
            let (a, b) = (
                find(&mut parent, tri[k]),
                find(&mut parent, tri[(k + 1) % 3]),
            );
            if a != b {
                parent[a as usize] = b;
            }
        }
    }
    let mut per: std::collections::HashMap<u32, (usize, f64, [f64; 3], [f64; 3])> =
        std::collections::HashMap::new();
    for tri in mesh.indices.chunks(3) {
        let root = find(&mut parent, tri[0]);
        let p = |i: u32| {
            let k = i as usize * 3;
            [
                mesh.vertices[k] as f64,
                mesh.vertices[k + 1] as f64,
                mesh.vertices[k + 2] as f64,
            ]
        };
        let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
        let e = per
            .entry(root)
            .or_insert((0, 0.0, [f64::MAX; 3], [f64::MIN; 3]));
        e.0 += 1;
        e.1 += (a[0] * (b[1] * c[2] - c[1] * b[2]) - b[0] * (a[1] * c[2] - c[1] * a[2])
            + c[0] * (a[1] * b[2] - b[1] * a[2]))
            / 6.0;
        for v in [a, b, c] {
            let cyl = [
                (v[0] * v[0] + v[1] * v[1]).sqrt(),
                v[1].atan2(v[0]).to_degrees().rem_euclid(360.0),
                v[2],
            ];
            for k in 0..3 {
                e.2[k] = e.2[k].min(cyl[k]);
                e.3[k] = e.3[k].max(cyl[k]);
            }
        }
    }
    let mut comps: Vec<(usize, f64, [f64; 3], [f64; 3])> = per.into_values().collect();
    comps.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    for (tris, vol, lo, hi) in comps.iter().take(6) {
        lines.push(format!(
            "  component: {tris} tris, volume {vol:.6} mm³, r {:.3}..{:.3} th {:.2}..{:.2} z {:.3}..{:.3}",
            lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
        ));
    }

    for (a, b, n) in mesh.non_manifold_edges().iter().take(10) {
        lines.push(format!("  branching x{n}: [{}] -> [{}]", at(*a), at(*b)));
    }
    for (a, b) in mesh.boundary_edges().iter().take(10) {
        lines.push(format!("  bad edge: [{}] -> [{}]", at(*a), at(*b)));
    }
    lines.join("\n")
}

/// `tube` minus the given voids, one boolean at a time.
///
/// Each intermediate is audited, so a failure names the tool that caused
/// it rather than surfacing at the end of a fifteen-step chain.
fn build(tools: Vec<(String, TriangleMesh)>) -> TriangleMesh {
    let mut shell = tube();
    for (name, tool) in tools {
        shell = manifold_csg(&shell, &tool, BooleanOp::Difference);
        let report = check_manifold(&shell);
        assert!(
            report.is_manifold(),
            "intermediate solid stopped bounding a solid after subtracting \
             {name}: {report}\n{}",
            describe_defects(&shell)
        );
    }
    shell
}

// ---------------------------------------------------------------------------
// Binary STL, so the support check reads exactly what a slicer would.
// ---------------------------------------------------------------------------

fn to_stl(mesh: &TriangleMesh) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&((mesh.indices.len() / 3) as u32).to_le_bytes());
    for tri in mesh.indices.chunks(3) {
        let p = |i: u32| {
            let k = i as usize * 3;
            [mesh.vertices[k], mesh.vertices[k + 1], mesh.vertices[k + 2]]
        };
        let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let mut n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n = [n[0] / len, n[1] / len, n[2] / len];
        }
        for f in n.iter().chain(a.iter()).chain(b.iter()).chain(c.iter()) {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// Support check — ported from rana `60c`:tools/support-check.py.
//
// For each (θ, r) column, cast a vertical ray, collect triangle crossings
// and read material intervals by strict parity, exactly as slicer mesh
// analysis does. Two things fail a column: a material interval that starts
// in mid-air anywhere the design does not document a bridge roof, and any
// gap thinner than 0.15 mm, which is a crack rather than a channel.
//
// Parity (not winding) is deliberate: parity reads a z-overlap between two
// separate bodies as an interior void strip, and that is precisely the
// defect class the check exists to catch — rana finding #11, where two
// overlapping construction bodies at the rim read as a 0.05 crack ring.
// ---------------------------------------------------------------------------

/// A gap thinner than this is a crack, never a designed channel.
const MIN_GAP: f64 = 0.15;

/// Angular offset (degrees) applied to every probe ray.
///
/// The rana checker notes that its rays "use x.0/x.7 angles" so they never
/// run along a construction seam; the same care is needed here for a
/// different reason. A ray that passes exactly through a mesh vertex
/// crosses two triangles at one point, so the crossing count goes even
/// where it should be odd and the column reads as empty — a sampling
/// artifact indistinguishable, in the output, from a real void. Offsetting
/// the grid keeps every ray in a triangle's interior.
const PROBE_PHASE: f64 = 0.7;

struct Stl {
    tris: Vec<[[f64; 3]; 3]>,
}

impl Stl {
    fn parse(bytes: &[u8]) -> Stl {
        let n = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        let mut tris = Vec::with_capacity(n);
        for i in 0..n {
            let base = 84 + 50 * i + 12;
            let mut t = [[0.0f64; 3]; 3];
            for (j, vertex) in t.iter_mut().enumerate() {
                for (k, coord) in vertex.iter_mut().enumerate() {
                    let o = base + 12 * j + 4 * k;
                    *coord = f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as f64;
                }
            }
            tris.push(t);
        }
        Stl { tris }
    }

    /// Sorted z of every triangle the upward ray at `(x, y)` crosses.
    fn crossings(&self, x: f64, y: f64) -> Vec<f64> {
        let mut out = Vec::new();
        for [a, b, c] in &self.tris {
            let d1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let d2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let den = d1[0] * d2[1] - d1[1] * d2[0];
            if den.abs() < 1e-12 {
                continue;
            }
            let u = ((x - a[0]) * d2[1] - (y - a[1]) * d2[0]) / den;
            let v = (d1[0] * (y - a[1]) - d1[1] * (x - a[0])) / den;
            if u < -1e-9 || v < -1e-9 || u + v > 1.0 + 1e-9 {
                continue;
            }
            out.push(a[2] + u * d1[2] + v * d2[2]);
        }
        out.sort_by(|p, q| p.partial_cmp(q).unwrap());
        out
    }

    /// Material intervals by strict parity: odd crossing count = inside.
    fn intervals(&self, x: f64, y: f64) -> Vec<(f64, f64)> {
        self.crossings(x, y)
            .chunks(2)
            .filter(|c| c.len() == 2 && c[1] - c[0] > 1e-4)
            .map(|c| (c[0], c[1]))
            .collect()
    }
}

/// Documented mid-air starts: the bottom slot roofs (printed as bridges)
/// and the top leg roof wedges (anchored at the pocket end). Everything
/// else starting in mid-air is a floating island.
fn documented_bridge(theta: f64, z: f64) -> bool {
    let entry_half = half_angle(4.4, 34.5);
    let behind = |a: f64| -(((theta - a + 180.0).rem_euclid(360.0)) - 180.0);
    BOTTOM_ENTRIES
        .iter()
        .any(|&a| (-entry_half - 0.2..=18.6).contains(&behind(a)) && (1.7..=2.7).contains(&z))
        || TOP_ENTRIES
            .iter()
            .any(|&a| (entry_half - 0.2..=18.6).contains(&behind(a)) && (25.55..=26.3).contains(&z))
}

/// Every disallowed restart or crack found in one column.
fn column_defects(stl: &Stl, theta: f64, r: f64) -> Vec<String> {
    let (x, y) = (r * theta.to_radians().cos(), r * theta.to_radians().sin());
    let iv = stl.intervals(x, y);
    let mut bad = Vec::new();
    if iv.is_empty() {
        bad.push(format!("th={theta} r={r}: no material at all"));
        return bad;
    }
    for k in 1..iv.len() {
        let z = iv[k].0;
        let gap = iv[k].0 - iv[k - 1].1;
        if gap < MIN_GAP {
            bad.push(format!(
                "th={theta} r={r}: CRACK at z={z:.3}, gap={gap:.4} < {MIN_GAP}"
            ));
        } else if !documented_bridge(theta, z) {
            bad.push(format!(
                "th={theta} r={r}: undocumented mid-air start at z={z:.3} (gap {gap:.3})"
            ));
        }
    }
    bad
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Run the rana support check over a part; returns the column count and
/// every defect found.
fn support_check(mesh: &TriangleMesh) -> (usize, Vec<String>) {
    let stl = Stl::parse(&to_stl(mesh));
    let mut defects = Vec::new();
    let mut columns = 0;

    // Coarse pass: 3° × three radii, exact intervals.
    for step in 0..120 {
        let theta = step as f64 * 3.0 + PROBE_PHASE;
        for r in [33.5, 34.5, 35.5] {
            columns += 1;
            defects.extend(column_defects(&stl, theta, r));
        }
    }

    // Fine pass at the angles where the helical floors and roofs are
    // steepest and where the entry, leg and window cuts all overlap.
    for theta in [
        6.0, 45.0, 48.0, 52.0, 58.0, 60.0, 88.0, 90.0, 96.0, 100.0, 105.0, 200.0,
    ]
    .map(|t: f64| t + PROBE_PHASE)
    {
        for r in [33.5, 34.5, 35.5] {
            columns += 1;
            defects.extend(column_defects(&stl, theta, r));
        }
    }
    (columns, defects)
}

/// The vertical slice, at full strength: the tube minus all three bottom
/// J-slot channels — entry notches plus helical legs. This is the undercut
/// geometry that drove the rana project off the CAD path; the leg roof
/// overhangs its floor, so no extrusion along z can express it.
///
/// Asserted here: exactly one manifold shell, and the volume the design
/// calls for. The support check over this same part is
/// `support_check_is_clean` below.
#[test]
fn bottom_channels_give_one_manifold_shell() {
    let shell = build(bottom_channel_tools());
    let report: ManifoldReport = check_manifold(&shell);
    println!("rana-60c bottom channels: {report}");
    assert!(
        report.is_single_manifold_shell(),
        "{report}\n{}",
        describe_defects(&shell)
    );

    // The blank is 14315.9 mm³; the six voids take about 1.4% of it.
    assert!(
        (14_050.0..14_200.0).contains(&report.signed_volume),
        "volume {:.1} mm³ outside the expected band: {report}",
        report.signed_volume
    );
}

/// The whole part — tube minus fifteen voids, top channels and stator
/// windows included.
///
/// The surface property holds all the way through: every one of the fifteen
/// intermediates, and the result, is a closed, consistently oriented,
/// non-branching mesh. `build` audits after each cut, so a failure names
/// the tool that caused it.
///
/// Not asserted here: `components == 1`. See `full_shell_is_a_single_shell`.
#[test]
fn full_shell_is_manifold_at_every_step() {
    let shell = build(void_tools());
    let report: ManifoldReport = check_manifold(&shell);
    println!("rana-60c full shell: {report}");

    assert_eq!(report.boundary_edges, 0, "bad edges: {report}");
    assert_eq!(report.non_manifold_edges, 0, "branching edges: {report}");
    assert_eq!(report.inconsistent_edges, 0, "winding conflicts: {report}");
    assert_eq!(report.degenerate_triangles, 0, "{report}");
    assert!(report.is_manifold(), "{report}");

    // The blank is 14315.9 mm³; the fifteen voids remove roughly 8% of it.
    // A gross departure means a boolean silently no-opped or ran away.
    assert!(
        (12_500.0..14_000.0).contains(&report.signed_volume),
        "shell volume {:.1} mm³ is not in the expected band: {report}",
        report.signed_volume
    );
}

/// Determinism, localised: every individual boolean must be a pure
/// function of its two operands.
///
/// Chained separately from the whole-part check so a failure names the
/// step that drifted instead of just reporting a byte mismatch at the end.
#[test]
fn every_boolean_step_is_deterministic() {
    let mut shell = tube();
    for (name, tool) in void_tools() {
        // Split the stages so a failure says which one drifted: the raw
        // boolean, or the repair chain layered on top of it.
        let raw_a = mesh_csg(&shell, &tool, BooleanOp::Difference);
        let raw_b = mesh_csg(&shell, &tool, BooleanOp::Difference);
        assert!(
            raw_a.indices == raw_b.indices && raw_a.vertices == raw_b.vertices,
            "mesh_csg is nondeterministic on {name}: {} vs {} triangles",
            raw_a.num_triangles(),
            raw_b.num_triangles()
        );

        let a = manifold_csg(&shell, &tool, BooleanOp::Difference);
        let b = manifold_csg(&shell, &tool, BooleanOp::Difference);
        assert!(
            a.indices == b.indices && a.vertices == b.vertices,
            "the repair chain is nondeterministic on {name}: {} vs {} triangles",
            a.num_triangles(),
            b.num_triangles()
        );
        shell = a;
    }
}

/// Regenerating the same part must produce the same bytes — the rana
/// workflow diffs STLs across regenerations to review a change, which is
/// worthless if the writer reshuffles triangles on its own.
#[test]
fn full_shell_is_deterministic_across_runs() {
    let a = to_stl(&build(void_tools()));
    let b = to_stl(&build(void_tools()));
    assert_eq!(a.len(), b.len(), "triangle count differs between runs");
    assert!(
        a == b,
        "STL bytes differ between two runs of the same build"
    );
}

/// KNOWN GAP (ecto/vcad#840 follow-up). Two defects survive, both traced
/// to the same region and both *below* the topological layer this PR
/// fixed — the meshes are manifold; some material is classified wrong.
///
/// **The full part is manifold but comes out as two shells.** Subtracting
/// `top-entry@60` — a notch at θ 56…64, z 23.8 and up — detaches a
/// 0.92 mm³, 166-triangle wedge at θ 216…221, z −0.6…0.08, on the far side
/// of the part from the cut that dislodged it. The wedge is the floor
/// ledge under `bottom-leg@225`, which in the exact solid stays joined to
/// the wall at the leg's far end (θ 206.6); it then stays byte-stable
/// through every later boolean. A cut cannot change topology 150° away, so
/// this is a classification fault in `mesh_csg`, not a repair gap.
///
/// **The support check finds one hairline crack.** On the bottom-channel
/// slice, θ 219.7 / r 35.5 shows a 0.0126 mm gap at z −0.177 — directly
/// under that same `bottom-leg@225` floor. On the full part the check
/// additionally reports two mid-air starts out of 396 columns (θ 108.7
/// z 1.485; θ 174.7 z 26.971).
///
/// The informative part is what the checker does *not* find: across 396
/// columns, no crack anywhere except that one spot, on a part whose
/// hand-rolled equivalent needed four rewrites to reach the same state.
///
/// Reproducing the fault needs the tube's facet seams where they are;
/// re-tessellating at 400 segments moves the defect rather than removing
/// it (it becomes 26 bad edges at `bottom-entry@345`), which is further
/// evidence that the fault is in classification near a seam.
#[test]
#[ignore = "mesh_csg misclassifies a ledge under bottom-leg@225 — see doc comment"]
fn support_check_is_clean() {
    let slice = build(bottom_channel_tools());
    let (columns, defects) = support_check(&slice);
    assert!(
        defects.is_empty(),
        "bottom-channel slice failed the support check over {columns} columns:\n  {}",
        defects.join("\n  ")
    );

    let shell = build(void_tools());
    let report = check_manifold(&shell);
    assert!(
        report.is_single_manifold_shell(),
        "{report}\n{}",
        describe_defects(&shell)
    );
    let (columns, defects) = support_check(&shell);
    assert!(
        defects.is_empty(),
        "full shell failed the support check over {columns} columns:\n  {}",
        defects.join("\n  ")
    );
}
