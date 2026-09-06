//! Hobby 3-axis mill ruleset (`ruleset = "hobby_3axis_mill"`).
//!
//! Seven rules for a small bench mill — a 2 mm minimum end mill, a
//! 3/4/5/6 reamer set, metric tap drills, plate stock, and a
//! 300 × 200 × 80 envelope — reported pass/fail per rule with located
//! examples and suggested affordances.
//!
//! Unlike the per-process modules this ruleset runs in the *mesh*
//! domain: a BRep is tessellated first, and a bare triangle soup (an STL
//! straight off disk) is accepted as-is. Every real part that reaches a
//! mill has been through a boolean or an export, and the geometry the
//! machinist sees is the triangle soup, so that is what gets judged.
//!
//! The rules read three views of the mesh:
//!
//! * **Horizontal cross-sections** (R1, R3, R5, R7) — loops walked with
//!   material on the left, so a right turn is an internal corner.
//! * **Vertical rays** through an XY grid (R2, R5) — reachability along
//!   ±Z and floor thickness.
//! * **The bounding box** (R4, R6).
//!
//! Exported meshes are often a *loose union* — every additive sub-body
//! dumped as its own closed shell (the rana 60c rotor is 51 shells, the
//! cup 745). Faces buried inside another shell are not part of the part,
//! so shells are detected up front, buried triangles are dropped from the
//! ray checks, and section loops are unioned in 2D before any corner,
//! bore or wall is measured.

use std::collections::HashMap;
use std::f64::consts::PI;

use vcad_kernel_cost::Process;
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;

use crate::issue::{DfmFix, DfmIssue, DfmSeverity, RuleResult};
use crate::rules::{Rule, RulePack};

/// `RulePack::ruleset` value that selects this module.
pub const RULESET: &str = "hobby_3axis_mill";

/// Segments per full circle when tessellating a BRep for analysis.
const TESS_SEGMENTS: u32 = 48;
/// Cap on located examples emitted per rule (counts are still exact).
const MAX_EXAMPLES: usize = 12;
/// Minimum number of uniform slicing levels over the part height (on top
/// of one level per horizontal-face band), their target spacing, and a cap.
const UNIFORM_LEVELS: usize = 32;
const UNIFORM_SPACING: f64 = 0.4;
const MAX_UNIFORM_LEVELS: usize = 160;
/// A concave run must turn at least this much to count as a corner.
const MIN_CORNER_TURN: f64 = 30.0 * PI / 180.0;
/// A single vertex turning this much is a sharp corner, not a fillet chord
/// (after simplification a R1 arc still turns ≤ ~17° per vertex).
const SHARP_TURN: f64 = 40.0 * PI / 180.0;
/// Shells closer than this are one body: sector-column exports leave
/// ~20 µm slits between neighbours, and nothing that small is a feature.
const MERGE_TOL: f64 = 0.05;
/// Section loops are simplified to this chord deviation — it erases the
/// 2 µm step risers a helix-as-staircase leaves, keeps a R1 fillet's chords.
const SIMPLIFY_TOL: f64 = 0.01;
/// Loops below this area are debris, not features.
const MIN_LOOP_AREA: f64 = 0.1;

// ─── entry points ─────────────────────────────────────────────────────

/// Run the ruleset over a BRep (tessellated at [`TESS_SEGMENTS`]).
pub fn run(
    brep: &BRepSolid,
    pack: &RulePack,
    issues: &mut Vec<DfmIssue>,
    results: &mut Vec<RuleResult>,
) {
    let mesh = vcad_kernel_tessellate::tessellate_brep(brep, TESS_SEGMENTS);
    let tris = tris_from_indexed(&mesh.vertices, &mesh.indices);
    run_mesh(&tris, pack, issues, results);
}

/// Run the ruleset over a triangle soup in millimetres.
pub fn run_mesh(
    tris: &[[[f64; 3]; 3]],
    pack: &RulePack,
    issues: &mut Vec<DfmIssue>,
    results: &mut Vec<RuleResult>,
) {
    let mut mesh = Mesh::new(tris);
    if mesh.tris.is_empty() {
        return;
    }
    let grid = Grid::new(&mesh);
    mesh.classify_exterior(&grid);
    let sections = mesh.sections(&grid);
    if std::env::var_os("VCAD_DFM_DEBUG").is_some() {
        eprintln!(
            "[hobby_mill] {} triangles, {} shells, {} buried, {} section levels ({} loops)",
            mesh.tris.len(),
            mesh.n_shells,
            mesh.tris.iter().filter(|t| !t.exterior).count(),
            sections.len(),
            sections.iter().map(|s| s.loops.len()).sum::<usize>()
        );
    }

    let mut ctx = Ctx {
        process: Process::Cnc3Axis,
        issues,
        results,
    };
    if let Some(rule) = pack.rule("r1_internal_corner_radius") {
        r1_corner_radius(&mut ctx, rule, &sections);
    }
    if let Some(rule) = pack.rule("r2_reachability") {
        r2_reachability(&mut ctx, rule, &mesh, &grid);
    }
    if let Some(rule) = pack.rule("r3_hole_diameters") {
        r3_hole_diameters(&mut ctx, rule, &sections);
    }
    if let Some(rule) = pack.rule("r4_plate_stock") {
        r4_plate_stock(&mut ctx, rule, &mesh);
    }
    if let Some(rule) = pack.rule("r5_min_wall") {
        r5_min_wall(&mut ctx, rule, &mesh, &grid, &sections);
    }
    if let Some(rule) = pack.rule("r6_envelope") {
        r6_envelope(&mut ctx, rule, &mesh);
    }
    if let Some(rule) = pack.rule("r7_threads_and_gears") {
        r7_threads_and_gears(&mut ctx, rule, &sections);
    }
}

/// Convert an indexed f32 mesh into a triangle soup.
pub fn tris_from_indexed(vertices: &[f32], indices: &[u32]) -> Vec<[[f64; 3]; 3]> {
    let v = |i: u32| {
        let i = i as usize * 3;
        [
            vertices[i] as f64,
            vertices[i + 1] as f64,
            vertices[i + 2] as f64,
        ]
    };
    indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| [v(t[0]), v(t[1]), v(t[2])])
        .collect()
}

struct Ctx<'a> {
    process: Process,
    issues: &'a mut Vec<DfmIssue>,
    results: &'a mut Vec<RuleResult>,
}

impl Ctx<'_> {
    #[allow(clippy::too_many_arguments)]
    fn issue(
        &mut self,
        rule_id: &str,
        severity: DfmSeverity,
        message: String,
        anchor: [f64; 3],
        measured: f64,
        limit: f64,
        units: &str,
        explanation: &str,
        fix: String,
    ) {
        let issue = DfmIssue::new(
            rule_id,
            severity,
            self.process,
            message,
            Point3::new(anchor[0], anchor[1], anchor[2]),
            measured,
            limit,
            units,
        )
        .with_explanation(explanation)
        .with_fix(DfmFix::Manual { description: fix });
        self.issues.push(issue);
    }

    fn verdict(
        &mut self,
        rule_id: &str,
        label: &str,
        violations: usize,
        summary: String,
        affordances: Vec<String>,
    ) {
        self.results.push(RuleResult {
            rule: rule_id.to_string(),
            label: label.to_string(),
            passed: violations == 0,
            violation_count: violations,
            summary,
            affordances,
        });
    }
}

// ─── mesh ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Tri {
    p: [[f64; 3]; 3],
    n: [f64; 3],
    area: f64,
    centroid: [f64; 3],
    /// Connected-shell id (vertex-welded).
    shell: u32,
    /// False when the triangle is buried inside another shell.
    exterior: bool,
}

struct Mesh {
    tris: Vec<Tri>,
    n_shells: usize,
    lo: [f64; 3],
    hi: [f64; 3],
}

/// Quantised vertex key for welding.
fn vkey(p: [f64; 3]) -> (i64, i64, i64) {
    (
        (p[0] * 1e4).round() as i64,
        (p[1] * 1e4).round() as i64,
        (p[2] * 1e4).round() as i64,
    )
}

fn uf_find(parent: &mut [u32], mut a: u32) -> u32 {
    while parent[a as usize] != a {
        let g = parent[parent[a as usize] as usize];
        parent[a as usize] = g;
        a = g;
    }
    a
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
fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

impl Mesh {
    fn new(tris: &[[[f64; 3]; 3]]) -> Self {
        let mut out = Vec::with_capacity(tris.len());
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for t in tris {
            let c = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            let len = norm(c);
            if len < 1e-12 {
                continue;
            }
            for v in t {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            out.push(Tri {
                p: *t,
                n: [c[0] / len, c[1] / len, c[2] / len],
                area: len * 0.5,
                centroid: [
                    (t[0][0] + t[1][0] + t[2][0]) / 3.0,
                    (t[0][1] + t[1][1] + t[2][1]) / 3.0,
                    (t[0][2] + t[1][2] + t[2][2]) / 3.0,
                ],
                shell: 0,
                exterior: true,
            });
        }
        // Connected shells by vertex welding (union-find over vertex ids).
        let mut ids: HashMap<(i64, i64, i64), u32> = HashMap::new();
        let mut parent: Vec<u32> = Vec::new();
        let mut tri_vert: Vec<[u32; 3]> = Vec::with_capacity(out.len());
        for t in &out {
            let mut v = [0u32; 3];
            for (k, p) in t.p.iter().enumerate() {
                let next = parent.len() as u32;
                let id = *ids.entry(vkey(*p)).or_insert(next);
                if id == next {
                    parent.push(next);
                }
                v[k] = id;
            }
            tri_vert.push(v);
        }
        for v in &tri_vert {
            let r = uf_find(&mut parent, v[0]);
            for &o in &v[1..] {
                let ro = uf_find(&mut parent, o);
                parent[ro as usize] = r;
            }
        }
        let mut shell_ids: HashMap<u32, u32> = HashMap::new();
        for (t, v) in out.iter_mut().zip(&tri_vert) {
            let root = uf_find(&mut parent, v[0]);
            let n = shell_ids.len() as u32;
            t.shell = *shell_ids.entry(root).or_insert(n);
        }
        Mesh {
            tris: out,
            n_shells: shell_ids.len(),
            lo,
            hi,
        }
    }

    /// Mark triangles buried inside another shell (their nudged centroid
    /// has odd crossing parity against some other shell along +Z).
    fn classify_exterior(&mut self, grid: &Grid) {
        if self.n_shells < 2 {
            return;
        }
        let eps = MERGE_TOL;
        let mut buried = Vec::new();
        for (i, t) in self.tris.iter().enumerate() {
            let q = [
                t.centroid[0] + t.n[0] * eps,
                t.centroid[1] + t.n[1] * eps,
                t.centroid[2] + t.n[2] * eps,
            ];
            if grid.inside_other_shell(self, q, t.shell) {
                buried.push(i);
            }
        }
        for i in buried {
            self.tris[i].exterior = false;
        }
    }

    fn extent(&self) -> [f64; 3] {
        sub(self.hi, self.lo)
    }

    /// Z levels to slice at: the midpoint of every band between
    /// horizontal faces, plus a uniform sweep so helical features that
    /// have no horizontal faces still get sampled.
    fn levels(&self) -> Vec<f64> {
        let (z0, z1) = (self.lo[2], self.hi[2]);
        let h = z1 - z0;
        if h <= 1e-6 {
            return Vec::new();
        }
        let mut flats: Vec<f64> = self
            .tris
            .iter()
            .filter(|t| t.n[2].abs() > 0.999)
            .map(|t| (t.centroid[2] * 100.0).round() / 100.0)
            .collect();
        flats.push(z0);
        flats.push(z1);
        flats.sort_by(|a, b| a.partial_cmp(b).unwrap());
        flats.dedup_by(|a, b| (*a - *b).abs() < 0.02);
        let mut levels: Vec<f64> = flats.windows(2).map(|w| (w[0] + w[1]) * 0.5).collect();
        // Fine enough to resolve a 2 mm thread pitch, capped for tall parts.
        let uniform =
            ((h / UNIFORM_SPACING).ceil() as usize).clamp(UNIFORM_LEVELS, MAX_UNIFORM_LEVELS);
        for i in 0..uniform {
            levels.push(z0 + h * (i as f64 + 0.5) / uniform as f64);
        }
        // Nudge off any vertex z so no slice runs through an edge.
        let mut vz: Vec<f64> = self
            .tris
            .iter()
            .flat_map(|t| t.p.iter().map(|p| p[2]))
            .collect();
        vz.sort_by(|a, b| a.partial_cmp(b).unwrap());
        vz.dedup();
        let on_vertex = |z: f64| {
            let i = vz.partition_point(|v| *v < z - 1e-6);
            i < vz.len() && (vz[i] - z).abs() < 1e-6
        };
        for z in levels.iter_mut() {
            let mut guard = 0;
            while guard < 8 && on_vertex(*z) {
                *z += 1.3e-4;
                guard += 1;
            }
        }
        levels.sort_by(|a, b| a.partial_cmp(b).unwrap());
        levels.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
        levels
    }

    fn sections(&self, grid: &Grid) -> Vec<Section> {
        self.levels()
            .into_iter()
            .filter_map(|z| {
                let loops = self.slice(z, grid);
                (!loops.is_empty()).then_some(Section { z, loops })
            })
            .collect()
    }

    /// Slice at `z`: oriented segments (material on the left) from every
    /// shell, unioned in 2D, linked into closed loops.
    fn slice(&self, z: f64, grid: &Grid) -> Vec<Loop> {
        let mut segs: Vec<([f64; 2], [f64; 2], u32)> = Vec::new();
        for t in &self.tris {
            let zs = [t.p[0][2], t.p[1][2], t.p[2][2]];
            let (zmin, zmax) = (zs[0].min(zs[1]).min(zs[2]), zs[0].max(zs[1]).max(zs[2]));
            if !(zmin < z && z < zmax) {
                continue;
            }
            let nh = (t.n[0] * t.n[0] + t.n[1] * t.n[1]).sqrt();
            if nh < 1e-9 {
                continue;
            }
            let mut pts: Vec<[f64; 2]> = Vec::with_capacity(2);
            for i in 0..3 {
                let mut a = t.p[i];
                let mut b = t.p[(i + 1) % 3];
                if (a[2] < z) != (b[2] < z) {
                    // Canonical endpoint order so the two triangles sharing
                    // this edge compute a bit-identical point.
                    if b < a {
                        std::mem::swap(&mut a, &mut b);
                    }
                    let s = (z - a[2]) / (b[2] - a[2]);
                    pts.push([a[0] + s * (b[0] - a[0]), a[1] + s * (b[1] - a[1])]);
                }
            }
            if pts.len() != 2 {
                continue;
            }
            // Travel direction d = ẑ × n keeps material on the left.
            let d = [-t.n[1] / nh, t.n[0] / nh];
            let along = (pts[1][0] - pts[0][0]) * d[0] + (pts[1][1] - pts[0][1]) * d[1];
            let (a, b) = if along >= 0.0 {
                (pts[0], pts[1])
            } else {
                (pts[1], pts[0])
            };
            if ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() > 1e-6 {
                segs.push((a, b, t.shell));
            }
        }
        if self.n_shells > 1 {
            segs = union_segments(segs, z, self, grid);
        }
        link_loops(&segs)
    }
}

/// 2D union of a section: split segments where shells cross, then drop
/// every piece that lies inside another shell's material.
fn union_segments(
    segs: Vec<([f64; 2], [f64; 2], u32)>,
    z: f64,
    mesh: &Mesh,
    grid: &Grid,
) -> Vec<([f64; 2], [f64; 2], u32)> {
    let mut shells_here: Vec<u32> = segs.iter().map(|s| s.2).collect();
    shells_here.sort_unstable();
    shells_here.dedup();
    if shells_here.len() < 2 {
        return segs;
    }
    // Bin segments into a 2D grid to find cross-shell intersections.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for (a, b, _) in &segs {
        for p in [a, b] {
            lo[0] = lo[0].min(p[0]);
            lo[1] = lo[1].min(p[1]);
            hi[0] = hi[0].max(p[0]);
            hi[1] = hi[1].max(p[1]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-3);
    let cell = (span / 64.0).max(0.25);
    let nx = ((hi[0] - lo[0]) / cell).ceil() as usize + 1;
    let ny = ((hi[1] - lo[1]) / cell).ceil() as usize + 1;
    let mut cells: Vec<Vec<u32>> = vec![Vec::new(); nx * ny];
    let idx = |v: f64, l: f64| ((v - l) / cell).floor() as usize;
    for (i, (a, b, _)) in segs.iter().enumerate() {
        let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
        let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
        for iy in idx(y0, lo[1])..=idx(y1, lo[1]).min(ny - 1) {
            for ix in idx(x0, lo[0])..=idx(x1, lo[0]).min(nx - 1) {
                cells[iy * nx + ix].push(i as u32);
            }
        }
    }
    let mut splits: Vec<Vec<f64>> = vec![Vec::new(); segs.len()];
    for c in &cells {
        for (p, &i) in c.iter().enumerate() {
            for &j in &c[p + 1..] {
                let (i, j) = (i as usize, j as usize);
                if segs[i].2 == segs[j].2 {
                    continue;
                }
                if let Some((t, u)) = seg_seg_2d(segs[i].0, segs[i].1, segs[j].0, segs[j].1) {
                    splits[i].push(t);
                    splits[j].push(u);
                }
                // T-junctions and collinear overlaps (adjacent sector columns
                // sharing a radial face): split where the other segment's
                // endpoints land on this one.
                for (target, other) in [(i, j), (j, i)] {
                    for p in [segs[other].0, segs[other].1] {
                        if let Some(t) = point_on_segment_2d(p, segs[target].0, segs[target].1) {
                            splits[target].push(t);
                        }
                    }
                }
            }
        }
    }
    let mut pieces = Vec::with_capacity(segs.len());
    for (i, (a, b, shell)) in segs.iter().enumerate() {
        let ts = &mut splits[i];
        ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-9);
        let mut prev = 0.0;
        let at = |t: f64| [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
        for &t in ts.iter().chain(std::iter::once(&1.0)) {
            if t - prev > 1e-9 {
                pieces.push((at(prev), at(t), *shell));
            }
            prev = t;
        }
    }
    // Keep a piece only if a point just outside our own material is not
    // inside some other shell. (Just outside, so two shells sharing a
    // coincident wall both keep it — duplicates are deduped downstream.)
    pieces.retain(|(a, b, shell)| {
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len < 1e-9 {
            return false;
        }
        let out = [d[1] / len, -d[0] / len];
        let q = [
            (a[0] + b[0]) * 0.5 + out[0] * MERGE_TOL,
            (a[1] + b[1]) * 0.5 + out[1] * MERGE_TOL,
            z,
        ];
        !grid.inside_other_shell(mesh, q, *shell)
    });
    pieces
}

/// Parameter of `p` along `a→b` if it lies on the segment's interior
/// (within 1 µm), else `None`.
fn point_on_segment_2d(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    if len2 < 1e-18 {
        return None;
    }
    let t = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2;
    if !(1e-7..(1.0 - 1e-7)).contains(&t) {
        return None;
    }
    let q = [a[0] + t * d[0], a[1] + t * d[1]];
    let off = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt();
    (off < 1e-6).then_some(t)
}

/// Proper crossing of segments `a→b` and `c→d`: `(t, u)` parameters, both
/// strictly inside (0, 1).
fn seg_seg_2d(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> Option<(f64, f64)> {
    let r = [b[0] - a[0], b[1] - a[1]];
    let s = [d[0] - c[0], d[1] - c[1]];
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom.abs() < 1e-14 {
        return None;
    }
    let ac = [c[0] - a[0], c[1] - a[1]];
    let t = (ac[0] * s[1] - ac[1] * s[0]) / denom;
    let u = (ac[0] * r[1] - ac[1] * r[0]) / denom;
    let open = 1e-7..(1.0 - 1e-7);
    (open.contains(&t) && open.contains(&u)).then_some((t, u))
}

/// Link oriented segments end-to-start into closed loops. Endpoint
/// matching is exact to 0.1 µm first; failing that, the nearest free
/// start within [`MERGE_TOL`] is taken (bridging the slit between two
/// sector columns), and the loop closes the same way.
fn link_loops(segs: &[([f64; 2], [f64; 2], u32)]) -> Vec<Loop> {
    let key = |p: [f64; 2]| ((p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64);
    let ckey = |p: [f64; 2]| {
        (
            (p[0] / MERGE_TOL).floor() as i64,
            (p[1] / MERGE_TOL).floor() as i64,
        )
    };
    let mut by_start: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut by_cell: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        by_start.entry(key(s.0)).or_default().push(i);
        by_cell.entry(ckey(s.0)).or_default().push(i);
    }
    let dist = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    let mut used = vec![false; segs.len()];
    let find_next = |cur: usize, used: &[bool]| -> Option<usize> {
        let end = segs[cur].1;
        let dir = {
            let d = [end[0] - segs[cur].0[0], end[1] - segs[cur].0[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-12);
            [d[0] / l, d[1] / l]
        };
        let k = key(end);
        for dx in [0i64, -1, 1] {
            for dy in [0i64, -1, 1] {
                if let Some(c) = by_start.get(&(k.0 + dx, k.1 + dy)) {
                    if let Some(&j) = c.iter().find(|&&j| !used[j]) {
                        return Some(j);
                    }
                }
            }
        }
        // Gap bridge: a free start within MERGE_TOL, preferring the one
        // that continues our direction (debris pieces branch off sideways).
        let ck = ckey(end);
        let mut best: Option<(f64, usize)> = None;
        for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                if let Some(c) = by_cell.get(&(ck.0 + dx, ck.1 + dy)) {
                    for &j in c {
                        if used[j] {
                            continue;
                        }
                        let d = dist(end, segs[j].0);
                        if d > MERGE_TOL {
                            continue;
                        }
                        let nd = [segs[j].1[0] - segs[j].0[0], segs[j].1[1] - segs[j].0[1]];
                        let nl = (nd[0] * nd[0] + nd[1] * nd[1]).sqrt().max(1e-12);
                        let smooth = (dir[0] * nd[0] + dir[1] * nd[1]) / nl; // 1 = straight on
                        let score = d - 0.5 * MERGE_TOL * smooth;
                        if best.is_none_or(|b| score < b.0) {
                            best = Some((score, j));
                        }
                    }
                }
            }
        }
        best.map(|b| b.1)
    };
    let same = |a: [f64; 2], b: [f64; 2]| dist(a, b) <= MERGE_TOL;
    let mut loops = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        let mut pts = vec![segs[start].0];
        let mut cur = start;
        used[cur] = true;
        let mut closed = false;
        for _ in 0..segs.len() {
            let end = segs[cur].1;
            if same(end, segs[start].0) {
                closed = true;
                break;
            }
            let Some(next) = find_next(cur, &used) else {
                break;
            };
            pts.push(end);
            used[next] = true;
            cur = next;
        }
        if closed && pts.len() >= 3 {
            if let Some(l) = Loop::new(pts) {
                loops.push(l);
            }
        }
    }
    loops
}

// ─── cross-section loops ──────────────────────────────────────────────

struct Section {
    z: f64,
    loops: Vec<Loop>,
}

struct Loop {
    /// Vertices with collinear / duplicate points removed.
    pts: Vec<[f64; 2]>,
    /// Signed turn at each vertex (+ left/convex, − right/internal corner).
    turns: Vec<f64>,
    /// Segment length `pts[i] → pts[i+1]`.
    lens: Vec<f64>,
    /// Signed area: + = outer boundary, − = hole / pocket.
    area: f64,
    centroid: [f64; 2],
}

impl Loop {
    fn new(raw: Vec<[f64; 2]>) -> Option<Self> {
        // Drop near-duplicate points, then simplify: remove any vertex
        // that sits within SIMPLIFY_TOL of the chord between its
        // neighbours. Flat walls arrive as many collinear pieces and a
        // helix-as-staircase as 2 µm risers; both fold away, while a
        // R≥0.5 fillet keeps chords turning ≤ ~30°.
        let mut pts: Vec<[f64; 2]> = Vec::with_capacity(raw.len());
        for p in raw {
            if let Some(last) = pts.last() {
                if ((p[0] - last[0]).powi(2) + (p[1] - last[1]).powi(2)).sqrt() < 1e-5 {
                    continue;
                }
            }
            pts.push(p);
        }
        // Passes until stable: (1) collapse spikes — a vertex the loop
        // reaches and immediately backs away from (|turn| > 170°) is a
        // zero-width slit in the source profile, not a wall; (2) chord
        // simplification; (3) re-dedupe what the collapse left touching.
        let mut changed = true;
        let mut guard = 0;
        while changed && pts.len() >= 3 && guard < 64 {
            changed = false;
            guard += 1;
            let n = pts.len();
            let mut keep = vec![true; n];
            for i in 0..n {
                if !keep[(i + n - 1) % n] {
                    continue; // don't remove two neighbours in one pass
                }
                let a = pts[(i + n - 1) % n];
                let b = pts[i];
                let c = pts[(i + 1) % n];
                let spike = turn(a, b, c).abs() > 170.0 * PI / 180.0;
                if spike || chord_deviation(a, b, c) < SIMPLIFY_TOL {
                    keep[i] = false;
                    changed = true;
                }
            }
            if changed {
                pts = pts
                    .into_iter()
                    .zip(keep)
                    .filter_map(|(p, k)| k.then_some(p))
                    .collect();
                let mut dd: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
                for p in pts {
                    if let Some(last) = dd.last() {
                        if ((p[0] - last[0]).powi(2) + (p[1] - last[1]).powi(2)).sqrt() < 1e-5 {
                            continue;
                        }
                    }
                    dd.push(p);
                }
                if dd.len() >= 2 {
                    let (f, l) = (dd[0], dd[dd.len() - 1]);
                    if ((f[0] - l[0]).powi(2) + (f[1] - l[1]).powi(2)).sqrt() < 1e-5 {
                        dd.pop();
                    }
                }
                pts = dd;
            }
        }
        let n = pts.len();
        if n < 3 {
            return None;
        }
        let turns: Vec<f64> = (0..n)
            .map(|i| turn(pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]))
            .collect();
        let lens: Vec<f64> = (0..n)
            .map(|i| {
                let a = pts[i];
                let b = pts[(i + 1) % n];
                ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
            })
            .collect();
        // Shoelace area and area centroid (robust to perimeter debris).
        let (mut area, mut cx, mut cy) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let w = a[0] * b[1] - b[0] * a[1];
            area += w;
            cx += (a[0] + b[0]) * w;
            cy += (a[1] + b[1]) * w;
        }
        area *= 0.5;
        if area.abs() < MIN_LOOP_AREA {
            return None;
        }
        let centroid = [cx / (6.0 * area), cy / (6.0 * area)];
        Some(Loop {
            pts,
            turns,
            lens,
            area,
            centroid,
        })
    }

    fn radii(&self) -> Vec<f64> {
        self.pts
            .iter()
            .map(|p| ((p[0] - self.centroid[0]).powi(2) + (p[1] - self.centroid[1]).powi(2)).sqrt())
            .collect()
    }

    /// If this loop is a circle (all radii within 3 % of the mean),
    /// return its diameter.
    fn circle_diameter(&self) -> Option<f64> {
        let r = self.radii();
        let mean = r.iter().sum::<f64>() / r.len() as f64;
        if mean < 1e-6 {
            return None;
        }
        let lo = r.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = r.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        ((hi - lo) / mean < 0.03 && self.pts.len() >= 8).then_some(2.0 * mean)
    }

    /// Radial profile `(θ, r)` about the centroid.
    fn polar(&self) -> Vec<(f64, f64)> {
        self.pts
            .iter()
            .map(|p| {
                let dx = p[0] - self.centroid[0];
                let dy = p[1] - self.centroid[1];
                (dy.atan2(dx), (dx * dx + dy * dy).sqrt())
            })
            .collect()
    }

    /// Amplitude and phase of the `k`-th angular harmonic of `r(θ)`.
    fn harmonic(&self, k: usize) -> (f64, f64) {
        let polar = self.polar();
        let mean = polar.iter().map(|p| p.1).sum::<f64>() / polar.len() as f64;
        let (mut c, mut s) = (0.0, 0.0);
        for (th, r) in &polar {
            c += (r - mean) * (k as f64 * th).cos();
            s += (r - mean) * (k as f64 * th).sin();
        }
        let n = polar.len() as f64;
        let amp = 2.0 * (c * c + s * s).sqrt() / n;
        (amp, s.atan2(c))
    }
}

/// Distance from `b` to the chord `a–c` (how much removing `b` moves the
/// boundary).
fn chord_deviation(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    let d = [c[0] - a[0], c[1] - a[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-12 {
        return ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    }
    ((b[0] - a[0]) * d[1] - (b[1] - a[1]) * d[0]).abs() / len
}

/// Signed turn angle at `b` going `a → b → c` (+ = left).
fn turn(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    let d0 = [b[0] - a[0], b[1] - a[1]];
    let d1 = [c[0] - b[0], c[1] - b[1]];
    let cr = d0[0] * d1[1] - d0[1] * d1[0];
    let dot = d0[0] * d1[0] + d0[1] * d1[1];
    cr.atan2(dot)
}

// ─── vertical-ray grid ────────────────────────────────────────────────

struct Grid {
    lo: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(mesh: &Mesh) -> Self {
        let ext = mesh.extent();
        let span = ext[0].max(ext[1]).max(1e-3);
        let target = ((mesh.tris.len() as f64).sqrt() * 0.5).clamp(8.0, 256.0);
        let cell = (span / target).max(0.25);
        let nx = ((ext[0] / cell).ceil() as usize).max(1);
        let ny = ((ext[1] / cell).ceil() as usize).max(1);
        let mut cells = vec![Vec::new(); nx * ny];
        for (i, t) in mesh.tris.iter().enumerate() {
            let (x0, x1, y0, y1) = t.p.iter().fold(
                (
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                ),
                |acc, p| {
                    (
                        acc.0.min(p[0]),
                        acc.1.max(p[0]),
                        acc.2.min(p[1]),
                        acc.3.max(p[1]),
                    )
                },
            );
            let ix0 = (((x0 - mesh.lo[0]) / cell).floor() as isize).clamp(0, nx as isize - 1);
            let ix1 = (((x1 - mesh.lo[0]) / cell).floor() as isize).clamp(0, nx as isize - 1);
            let iy0 = (((y0 - mesh.lo[1]) / cell).floor() as isize).clamp(0, ny as isize - 1);
            let iy1 = (((y1 - mesh.lo[1]) / cell).floor() as isize).clamp(0, ny as isize - 1);
            for iy in iy0..=iy1 {
                for ix in ix0..=ix1 {
                    cells[iy as usize * nx + ix as usize].push(i as u32);
                }
            }
        }
        Grid {
            lo: [mesh.lo[0], mesh.lo[1]],
            cell,
            nx,
            ny,
            cells,
        }
    }

    fn cell_at(&self, x: f64, y: f64) -> Option<&[u32]> {
        let ix = ((x - self.lo[0]) / self.cell).floor() as isize;
        let iy = ((y - self.lo[1]) / self.cell).floor() as isize;
        if ix < 0 || iy < 0 || ix >= self.nx as isize || iy >= self.ny as isize {
            return None;
        }
        Some(&self.cells[iy as usize * self.nx + ix as usize])
    }

    /// Is `p` inside the material of a shell other than `shell`? Crossing
    /// parity along +Z per shell; the ray is jittered off any shared edge.
    fn inside_other_shell(&self, mesh: &Mesh, p: [f64; 3], shell: u32) -> bool {
        let (x, y) = (p[0] + 1.7e-5, p[1] + 2.3e-5);
        let Some(cell) = self.cell_at(x, y) else {
            return false;
        };
        let mut hits: Vec<u32> = Vec::new();
        for &ti in cell {
            let t = &mesh.tris[ti as usize];
            if t.shell == shell || t.n[2].abs() < 1e-9 {
                continue;
            }
            if let Some(z) = plane_z_at(t, x, y) {
                if z > p[2] {
                    hits.push(t.shell);
                }
            }
        }
        hits.sort_unstable();
        let mut i = 0;
        while i < hits.len() {
            let mut j = i;
            while j < hits.len() && hits[j] == hits[i] {
                j += 1;
            }
            if (j - i) % 2 == 1 {
                return true;
            }
            i = j;
        }
        false
    }

    /// Nearest exterior hit along +Z (`up = true`) or −Z from `origin`,
    /// excluding triangle `skip`. Returns `(distance, triangle index)`.
    fn cast(&self, mesh: &Mesh, origin: [f64; 3], up: bool, skip: usize) -> Option<(f64, usize)> {
        self.cast_filtered(mesh, origin, up, skip, false)
    }

    /// [`Grid::cast`], optionally counting only *exit* crossings — faces
    /// whose normal points back along the ray, i.e. the far side of the
    /// material the ray is travelling through. Entry faces (another
    /// shell's near-coincident wall) are skipped.
    fn cast_filtered(
        &self,
        mesh: &Mesh,
        origin: [f64; 3],
        up: bool,
        skip: usize,
        exit_only: bool,
    ) -> Option<(f64, usize)> {
        // Jitter off exact edges so a ray grazing a cap boundary (a wall
        // meeting a flush cap) reads as clear rather than occluded.
        let origin = [origin[0] + 1.7e-5, origin[1] + 2.3e-5, origin[2]];
        let cell = self.cell_at(origin[0], origin[1])?;
        let mut best: Option<(f64, usize)> = None;
        for &ti in cell {
            let ti = ti as usize;
            if ti == skip {
                continue;
            }
            let t = &mesh.tris[ti];
            if !t.exterior {
                continue;
            }
            if t.n[2].abs() < 1e-9 {
                continue; // vertical: a vertical ray can't hit it
            }
            if exit_only && ((up && t.n[2] < 0.5) || (!up && t.n[2] > -0.5)) {
                continue;
            }
            let Some(z) = plane_z_at(t, origin[0], origin[1]) else {
                continue;
            };
            let d = if up { z - origin[2] } else { origin[2] - z };
            if d > 1e-6 && best.is_none_or(|b| d < b.0) {
                best = Some((d, ti));
            }
        }
        best
    }
}

/// Z of the triangle's plane at `(x, y)` if that point lies inside the
/// triangle's XY projection.
fn plane_z_at(t: &Tri, x: f64, y: f64) -> Option<f64> {
    let [a, b, c] = t.p;
    let d1 = (x - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (y - b[1]);
    let d2 = (x - c[0]) * (b[1] - c[1]) - (b[0] - c[0]) * (y - c[1]);
    let d3 = (x - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (y - a[1]);
    let has_neg = d1 < -1e-9 || d2 < -1e-9 || d3 < -1e-9;
    let has_pos = d1 > 1e-9 || d2 > 1e-9 || d3 > 1e-9;
    if has_neg && has_pos {
        return None;
    }
    let z = a[2] - (t.n[0] * (x - a[0]) + t.n[1] * (y - a[1])) / t.n[2];
    Some(z)
}

// ─── R1 ───────────────────────────────────────────────────────────────

/// Quantised XY key.
type Key2 = (i64, i64);
/// `(radius, z_min, z_max, loop centroid)` of one located corner.
type Corner = (f64, f64, f64, [f64; 2]);

fn r1_corner_radius(ctx: &mut Ctx, rule: &Rule, sections: &[Section]) {
    const ID: &str = "hobby_mill.r1_internal_corner_radius";
    let tool_dia = rule.num("min_tool_dia_mm", 2.0);
    let r_tool = tool_dia * 0.5;
    let relief = tool_dia * rule.num("relief_factor", 1.1);

    // key: quantised xy → (radius, z range, pocket centroid)
    let mut corners: HashMap<Key2, Corner> = HashMap::new();
    for sec in sections {
        for lp in &sec.loops {
            if lp.area < 0.0 && lp.circle_diameter().is_some() {
                continue; // a round hole — R3's business
            }
            if lp.lens.iter().sum::<f64>() < 1.0 {
                continue; // tessellation sliver, not a feature
            }
            for (pos, r) in concave_runs(lp) {
                if r >= r_tool - 0.01 {
                    continue;
                }
                let k = (
                    (pos[0] * 10.0).round() as i64,
                    (pos[1] * 10.0).round() as i64,
                );
                let e = corners.entry(k).or_insert((r, sec.z, sec.z, lp.centroid));
                e.0 = e.0.min(r);
                e.1 = e.1.min(sec.z);
                e.2 = e.2.max(sec.z);
            }
        }
    }
    let mut found: Vec<(Key2, Corner)> = corners.into_iter().collect();
    found.sort_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap().then(a.0.cmp(&b.0)));
    let count = found.len();

    // Group by the loop centroid so the affordance can say "pocket at (x, y)".
    let mut by_pocket: HashMap<(i64, i64), (usize, [f64; 2])> = HashMap::new();
    for (_, (_, _, _, c)) in &found {
        let k = ((c[0] * 2.0).round() as i64, (c[1] * 2.0).round() as i64);
        let e = by_pocket.entry(k).or_insert((0, *c));
        e.0 += 1;
    }
    let mut affordances: Vec<String> = by_pocket
        .values()
        .map(|(n, c)| {
            format!(
                "add corner relief Ø{:.1} (mouse ears) at {} corner{} of the pocket/profile centred ({:.1}, {:.1}), or fillet them R≥{:.1}",
                relief,
                n,
                if *n == 1 { "" } else { "s" },
                c[0],
                c[1],
                r_tool
            )
        })
        .collect();
    affordances.sort();

    for (k, (r, z0, z1, _)) in found.iter().take(MAX_EXAMPLES) {
        let (x, y) = (k.0 as f64 / 10.0, k.1 as f64 / 10.0);
        ctx.issue(
            ID,
            rule.severity_enum(),
            format!(
                "Internal corner R{:.2} at ({:.1}, {:.1}) z {:.1}..{:.1} — below cutter R{:.2}",
                r, x, y, z0, z1, r_tool
            ),
            [x, y, (z0 + z1) * 0.5],
            *r,
            r_tool,
            "mm",
            "A rotating end mill leaves its own radius in every internal corner; a sharper \
             corner can't be cut. Round the corner to the cutter radius or add a corner \
             relief (mouse ear) so the mating part still seats.",
            format!(
                "Fillet to R≥{:.2} or add a Ø{:.1} corner relief at ({:.1}, {:.1}).",
                r_tool, relief, x, y
            ),
        );
    }
    ctx.verdict(
        ID,
        "R1 internal corner radius",
        count,
        if count == 0 {
            format!(
                "all internal corners ≥ R{:.2} (Ø{:.1} end mill)",
                r_tool, tool_dia
            )
        } else {
            format!(
                "{} internal corner{} below R{:.2} (smallest R{:.2})",
                count,
                if count == 1 { "" } else { "s" },
                r_tool,
                found[0].1 .0
            )
        },
        affordances,
    );
}

/// Concave (right-turn) corners of a loop, each as `(position, fitted
/// radius)`. A vertex turning at least [`SHARP_TURN`] is a corner on its
/// own (radius 0); a run of gentler consecutive right turns is a
/// tessellated fillet, whose radius is recovered from its chords.
fn concave_runs(lp: &Loop) -> Vec<([f64; 2], f64)> {
    let n = lp.pts.len();
    let gentle: Vec<bool> = lp
        .turns
        .iter()
        .map(|t| *t < -1e-3 && -*t < SHARP_TURN)
        .collect();
    let mut out = Vec::new();
    // Sharp vertices stand alone — if both flanks are real walls, not a
    // bridged slit between two shells.
    for i in 0..n {
        if lp.turns[i] <= -SHARP_TURN && lp.lens[(i + n - 1) % n].min(lp.lens[i]) >= 2.0 * MERGE_TOL
        {
            out.push((lp.pts[i], 0.0));
        }
    }
    // Gentle runs: start at a non-gentle vertex so runs don't wrap; if
    // every vertex is gentle (an oval), take the whole loop as one run.
    let start = (0..n).find(|&i| !gentle[i]).unwrap_or(0);
    let mut i = 0;
    while i < n {
        let idx = (start + i) % n;
        if !gentle[idx] {
            i += 1;
            continue;
        }
        let mut run = vec![idx];
        let mut j = i + 1;
        while j < n && gentle[(start + j) % n] {
            run.push((start + j) % n);
            j += 1;
        }
        i = j;
        let theta: f64 = run.iter().map(|&k| -lp.turns[k]).sum();
        if theta < MIN_CORNER_TURN {
            continue;
        }
        let m = run.len();
        let inner_len: f64 = run[..m - 1].iter().map(|&k| lp.lens[k]).sum();
        let r = if m < 2 {
            0.0
        } else {
            inner_len * m as f64 / ((m - 1) as f64 * theta)
        };
        out.push((lp.pts[run[m / 2]], r));
    }
    out
}

// ─── R2 ───────────────────────────────────────────────────────────────

fn r2_reachability(ctx: &mut Ctx, rule: &Rule, mesh: &Mesh, grid: &Grid) {
    const ID: &str = "hobby_mill.r2_reachability";
    let two_sided = rule.flag("two_sided", true);
    let min_area = rule.num("min_area_mm2", 1.0);
    let eps = 1e-3;

    let mut flip: Vec<(f64, [f64; 3])> = Vec::new();
    let mut undercut: Vec<(f64, [f64; 3])> = Vec::new();
    let (mut flip_area, mut undercut_area, mut top_area) = (0.0, 0.0, 0.0);
    for (i, t) in mesh.tris.iter().enumerate() {
        if t.area < 1e-6 || !t.exterior {
            continue;
        }
        let o = [
            t.centroid[0] + t.n[0] * eps,
            t.centroid[1] + t.n[1] * eps,
            t.centroid[2] + t.n[2] * eps,
        ];
        let up_ok = t.n[2] > -1e-6 && grid.cast(mesh, o, true, i).is_none();
        let down_ok = t.n[2] < 1e-6 && grid.cast(mesh, o, false, i).is_none();
        if up_ok {
            top_area += t.area;
        } else if down_ok {
            flip_area += t.area;
            flip.push((t.area, t.centroid));
        } else {
            undercut_area += t.area;
            undercut.push((t.area, t.centroid));
        }
    }
    let total = top_area + flip_area + undercut_area;
    let sort = |v: &mut Vec<(f64, [f64; 3])>| {
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    };
    sort(&mut flip);
    sort(&mut undercut);

    let mut violations = 0;
    let mut affordances = Vec::new();
    if undercut_area >= min_area {
        violations += undercut.len();
        for (area, c) in undercut.iter().take(MAX_EXAMPLES) {
            ctx.issue(
                ID,
                rule.severity_enum(),
                format!(
                    "Undercut: face at ({:.1}, {:.1}, {:.1}) ({:.1} mm²) is reachable from neither +Z nor −Z",
                    c[0], c[1], c[2], area
                ),
                *c,
                *area,
                0.0,
                "mm2",
                "A 3-axis spindle only reaches what it can see straight down; a two-sided \
                 job adds the view straight up. Anything hidden from both is an undercut — \
                 a T-slot, a dovetail, a side hole, or the flank of a thread.",
                "Remove the undercut, split the part so each piece is two-sided, or move the \
                 feature to a side that can be presented to the spindle."
                    .to_string(),
            );
        }
        affordances.push(format!(
            "{:.0} mm² of undercut faces: redesign as a two-piece assembly, or take the feature to a 4th axis / EDM",
            undercut_area
        ));
    }
    if flip_area >= min_area {
        if two_sided {
            affordances.push(format!(
                "second setup: flip the part — {:.0} mm² ({:.0} %) of faces are only reachable from −Z (e.g. at {:.1}, {:.1}, {:.1})",
                flip_area,
                100.0 * flip_area / total.max(1e-9),
                flip[0].1[0],
                flip[0].1[1],
                flip[0].1[2]
            ));
        } else {
            violations += flip.len();
            for (area, c) in flip.iter().take(MAX_EXAMPLES) {
                ctx.issue(
                    ID,
                    rule.severity_enum(),
                    format!(
                        "Face at ({:.1}, {:.1}, {:.1}) ({:.1} mm²) is reachable only from −Z",
                        c[0], c[1], c[2], area
                    ),
                    *c,
                    *area,
                    0.0,
                    "mm2",
                    "This ruleset is configured single-sided; the face needs a flip.",
                    "Enable two-sided machining or move the feature to the top.".to_string(),
                );
            }
        }
    }
    ctx.verdict(
        ID,
        "R2 reachability (±Z)",
        violations,
        format!(
            "+Z {:.0} mm², −Z-only {:.0} mm², undercut {:.0} mm²",
            top_area, flip_area, undercut_area
        ),
        affordances,
    );
}

// ─── R3 ───────────────────────────────────────────────────────────────

/// `(diameter, cx, cy, z_min, z_max)` of one located bore.
type Hole = (f64, f64, f64, f64, f64);

fn r3_hole_diameters(ctx: &mut Ctx, rule: &Rule, sections: &[Section]) {
    const ID: &str = "hobby_mill.r3_hole_diameters";
    let reamers = rule.nums("reamers_mm");
    let taps = rule.nums("tap_drills_mm");
    let tap_labels = rule.strings("tap_labels");
    let tol = rule.num("tolerance_mm", 0.02);
    let max_reamed = rule.num(
        "max_reamed_dia_mm",
        reamers.iter().cloned().fold(0.0, f64::max),
    );

    // key: (cx, cy, dia) quantised → (dia, cx, cy, z0, z1)
    let mut holes: HashMap<(i64, i64, i64), Hole> = HashMap::new();
    for sec in sections {
        for lp in &sec.loops {
            if lp.area >= 0.0 {
                continue;
            }
            let Some(d) = lp.circle_diameter() else {
                continue;
            };
            let k = (
                (lp.centroid[0] * 10.0).round() as i64,
                (lp.centroid[1] * 10.0).round() as i64,
                (d * 20.0).round() as i64,
            );
            let e = holes
                .entry(k)
                .or_insert((d, lp.centroid[0], lp.centroid[1], sec.z, sec.z));
            e.3 = e.3.min(sec.z);
            e.4 = e.4.max(sec.z);
        }
    }
    let mut all: Vec<Hole> = holes.into_values().collect();
    all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut bad = Vec::new();
    let mut interpolated = 0usize;
    let mut ok = 0usize;
    for h in &all {
        let d = h.0;
        if d > max_reamed + tol {
            interpolated += 1;
            continue;
        }
        let on = |set: &[f64]| set.iter().any(|s| (s - d).abs() <= tol);
        if on(&reamers) || on(&taps) {
            ok += 1;
        } else {
            bad.push(*h);
        }
    }
    let nearest = |set: &[f64], d: f64| -> Option<(usize, f64)> {
        set.iter()
            .enumerate()
            .map(|(i, s)| (i, *s))
            .min_by(|a, b| (a.1 - d).abs().partial_cmp(&(b.1 - d).abs()).unwrap())
    };
    let mut affordances = Vec::new();
    for (d, cx, cy, z0, z1) in bad.iter().take(MAX_EXAMPLES) {
        let mut options = Vec::new();
        if let Some((_, r)) = nearest(&reamers, *d) {
            options.push(format!(
                "ream Ø{:.0} H7 ({}{:.2})",
                r,
                if r >= *d { "open " } else { "shrink " },
                (r - d).abs()
            ));
        }
        if let Some((i, t)) = nearest(&taps, *d) {
            let label = tap_labels.get(i).cloned().unwrap_or_default();
            options.push(format!(
                "drill Ø{:.2} and tap {}",
                t,
                if label.is_empty() {
                    "-".to_string()
                } else {
                    label
                }
            ));
        }
        let suggestion = format!("bore Ø{:.2} → {}", d, options.join(" or "));
        affordances.push(suggestion.clone());
        ctx.issue(
            ID,
            rule.severity_enum(),
            format!(
                "Bore Ø{:.2} at ({:.1}, {:.1}) z {:.1}..{:.1} matches no reamer / tap drill (±{:.2})",
                d, cx, cy, z0, z1, tol
            ),
            [*cx, *cy, (z0 + z1) * 0.5],
            *d,
            nearest(&reamers, *d).map(|r| r.1).unwrap_or(0.0),
            "mm",
            "A hobby mill holds size in a bore with a reamer or a tap, not by interpolating \
             a small end mill. Pick a diameter the tool drawer has.",
            suggestion,
        );
    }
    ctx.verdict(
        ID,
        "R3 hole diameters",
        bad.len(),
        format!(
            "{} bore{} on a reamer/tap size, {} off-size, {} > Ø{:.0} (interpolated)",
            ok,
            if ok == 1 { "" } else { "s" },
            bad.len(),
            interpolated,
            max_reamed
        ),
        affordances,
    );
}

// ─── R4 ───────────────────────────────────────────────────────────────

fn r4_plate_stock(ctx: &mut Ctx, rule: &Rule, mesh: &Mesh) {
    const ID: &str = "hobby_mill.r4_plate_stock";
    let stock = rule.nums("stock_thicknesses_mm");
    let tol = rule.num("tolerance_mm", 0.05);
    let documented = rule.flag("multi_op_documented", false);
    let ext = mesh.extent();
    let dz = ext[2];
    let hit = stock.iter().any(|s| (s - dz).abs() <= tol);
    let nearest_above = stock
        .iter()
        .cloned()
        .filter(|s| *s > dz)
        .fold(f64::INFINITY, f64::min);
    let nearest_below = stock
        .iter()
        .cloned()
        .filter(|s| *s < dz)
        .fold(f64::NEG_INFINITY, f64::max);
    let other_axis = [("X", ext[0]), ("Y", ext[1])]
        .into_iter()
        .find(|(_, e)| stock.iter().any(|s| (s - e).abs() <= tol));

    let mut affordances = Vec::new();
    let violations = if hit || documented { 0 } else { 1 };
    if !hit {
        if nearest_above.is_finite() {
            affordances.push(format!(
                "face Ø{:.1} plate down to {:.2} as a documented extra op (set multi_op_documented = true)",
                nearest_above, dz
            ));
        }
        if nearest_below.is_finite() {
            affordances.push(format!(
                "redesign to {:.1} thick to use stock as-is",
                nearest_below
            ));
        }
        if let Some((ax, e)) = other_axis {
            affordances.push(format!(
                "or re-orient: the {} extent ({:.2}) is already a stock thickness",
                ax, e
            ));
        }
        let msg = format!(
            "Part is {:.2} thick in Z — not a stock thickness (±{:.2}); nearest {}{}",
            dz,
            tol,
            if nearest_below.is_finite() {
                format!("{:.1}", nearest_below)
            } else {
                "—".into()
            },
            if nearest_above.is_finite() {
                format!(" / {:.1}", nearest_above)
            } else {
                String::new()
            }
        );
        ctx.issue(
            ID,
            if documented {
                DfmSeverity::Info
            } else {
                rule.severity_enum()
            },
            msg,
            [
                (mesh.lo[0] + mesh.hi[0]) * 0.5,
                (mesh.lo[1] + mesh.hi[1]) * 0.5,
                mesh.hi[2],
            ],
            dz,
            if nearest_above.is_finite() {
                nearest_above
            } else {
                nearest_below
            },
            "mm",
            "Plate parts come out of the stock at stock thickness; any other height means \
             a facing op on both sides, a second setup, and a tolerance you now own.",
            affordances.first().cloned().unwrap_or_default(),
        );
    }
    ctx.verdict(
        ID,
        "R4 plate stock",
        violations,
        if hit {
            format!("Z extent {:.2} matches stock (±{:.2})", dz, tol)
        } else if documented {
            format!("Z extent {:.2} is a documented multi-op height", dz)
        } else {
            format!("Z extent {:.2} matches no stock thickness", dz)
        },
        affordances,
    );
}

// ─── R5 ───────────────────────────────────────────────────────────────

fn r5_min_wall(ctx: &mut Ctx, rule: &Rule, mesh: &Mesh, grid: &Grid, sections: &[Section]) {
    const ID: &str = "hobby_mill.r5_min_wall";
    let min_wall = rule.num("min_wall_mm", 1.0);
    // Below this the gap is a tessellation seam, not a wall.
    let floor = 0.02;
    let eps = 1e-3;

    // (thickness, anchor, kind)
    let mut thin: Vec<(f64, [f64; 3], &str)> = Vec::new();
    let mut min_seen = f64::INFINITY;

    // Floors: from every (near-)flat down-facing triangle, the material
    // column above. Slanted faces — thread flanks, chamfers, run-out
    // feather edges — are not floors, and the thread is R7's finding.
    for (i, t) in mesh.tris.iter().enumerate() {
        if t.n[2] > -0.85 || t.area < 1e-4 || !t.exterior {
            continue;
        }
        let o = [t.centroid[0], t.centroid[1], t.centroid[2] - t.n[2] * eps];
        if let Some((d, _)) = grid.cast_filtered(mesh, o, true, i, true) {
            let d = d - eps;
            if d > floor {
                min_seen = min_seen.min(d);
                if d < min_wall {
                    thin.push((
                        d,
                        [t.centroid[0], t.centroid[1], t.centroid[2] + d * 0.5],
                        "floor",
                    ));
                }
            }
        }
    }

    // Walls: in every section, from each segment midpoint into material.
    for sec in sections {
        let segs: Vec<([f64; 2], [f64; 2])> = sec
            .loops
            .iter()
            .flat_map(|lp| {
                let n = lp.pts.len();
                (0..n).map(move |i| (lp.pts[i], lp.pts[(i + 1) % n]))
            })
            .collect();
        let stride = (segs.len() / 4000).max(1);
        for (si, (a, b)) in segs.iter().enumerate().step_by(stride) {
            let d = [b[0] - a[0], b[1] - a[1]];
            let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if len < 1e-6 {
                continue;
            }
            let left = [-d[1] / len, d[0] / len];
            let m = [
                (a[0] + b[0]) * 0.5 + left[0] * eps,
                (a[1] + b[1]) * 0.5 + left[1] * eps,
            ];
            let mut best = f64::INFINITY;
            for (sj, (c, e)) in segs.iter().enumerate() {
                if sj == si {
                    continue;
                }
                // Only exits: the hit segment's material side must face us.
                let hd = [e[0] - c[0], e[1] - c[1]];
                let hleft = [-hd[1], hd[0]];
                let hlen = (hleft[0] * hleft[0] + hleft[1] * hleft[1]).sqrt();
                if hlen < 1e-9 || (left[0] * hleft[0] + left[1] * hleft[1]) / hlen > -0.5 {
                    continue;
                }
                if let Some(t) = ray_segment_2d(m, left, *c, *e) {
                    if t > 1e-6 {
                        best = best.min(t);
                    }
                }
            }
            if best.is_finite() && best > floor {
                min_seen = min_seen.min(best);
                if best < min_wall {
                    thin.push((
                        best,
                        [
                            m[0] + left[0] * best * 0.5,
                            m[1] + left[1] * best * 0.5,
                            sec.z,
                        ],
                        "wall",
                    ));
                }
            }
        }
    }

    thin.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    // Dedupe by quantised anchor.
    let mut seen = std::collections::HashSet::new();
    thin.retain(|(_, a, _)| {
        seen.insert((
            (a[0] * 2.0).round() as i64,
            (a[1] * 2.0).round() as i64,
            (a[2] * 2.0).round() as i64,
        ))
    });
    let count = thin.len();
    for (d, a, kind) in thin.iter().take(MAX_EXAMPLES) {
        ctx.issue(
            ID,
            rule.severity_enum(),
            format!(
                "{} {:.2} mm at ({:.1}, {:.1}, {:.1}) — below minimum {:.2}",
                if *kind == "floor" { "Floor" } else { "Wall" },
                d,
                a[0],
                a[1],
                a[2],
                min_wall
            ),
            *a,
            *d,
            min_wall,
            "mm",
            "Thin walls and floors chatter, deflect away from the cutter and tear out at \
             the last pass; on a light hobby mill the limit is about a millimetre.",
            format!("Thicken to ≥ {:.2} mm.", min_wall),
        );
    }
    ctx.verdict(
        ID,
        "R5 minimum wall",
        count,
        if min_seen.is_finite() {
            format!("thinnest wall/floor {:.2} mm (limit {:.2})", min_seen, min_wall)
        } else {
            "no opposing faces found".to_string()
        },
        if count > 0 {
            vec![format!(
                "thicken the {} thin region{} to ≥ {:.1} mm (thinnest {:.2} at {:.1}, {:.1}, {:.1})",
                count,
                if count == 1 { "" } else { "s" },
                min_wall,
                thin[0].0,
                thin[0].1[0],
                thin[0].1[1],
                thin[0].1[2]
            )]
        } else {
            Vec::new()
        },
    );
}

/// Distance along the ray `o + t·d` to segment `a→b`, if it hits.
fn ray_segment_2d(o: [f64; 2], d: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let e = [b[0] - a[0], b[1] - a[1]];
    let denom = d[0] * e[1] - d[1] * e[0];
    if denom.abs() < 1e-12 {
        return None;
    }
    let ao = [a[0] - o[0], a[1] - o[1]];
    let t = (ao[0] * e[1] - ao[1] * e[0]) / denom;
    let u = (ao[0] * d[1] - ao[1] * d[0]) / denom;
    (t >= 0.0 && (-1e-9..=1.0 + 1e-9).contains(&u)).then_some(t)
}

// ─── R6 ───────────────────────────────────────────────────────────────

fn r6_envelope(ctx: &mut Ctx, rule: &Rule, mesh: &Mesh) {
    const ID: &str = "hobby_mill.r6_envelope";
    let mut env = rule.nums("envelope_mm");
    if env.len() != 3 {
        env = vec![300.0, 200.0, 80.0];
    }
    let ext = mesh.extent();
    let mut sorted_ext = ext;
    sorted_ext.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let mut sorted_env = [env[0], env[1], env[2]];
    sorted_env.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let fits = sorted_ext.iter().zip(&sorted_env).all(|(e, m)| e <= m);
    let as_is = ext.iter().zip(&env).all(|(e, m)| e <= m);
    let mut affordances = Vec::new();
    if !fits {
        let worst = sorted_ext
            .iter()
            .zip(&sorted_env)
            .map(|(e, m)| e - m)
            .fold(f64::NEG_INFINITY, f64::max);
        ctx.issue(
            ID,
            rule.severity_enum(),
            format!(
                "Part {:.0} × {:.0} × {:.0} exceeds the {:.0} × {:.0} × {:.0} envelope by {:.0} mm",
                ext[0], ext[1], ext[2], env[0], env[1], env[2], worst
            ),
            [
                (mesh.lo[0] + mesh.hi[0]) * 0.5,
                (mesh.lo[1] + mesh.hi[1]) * 0.5,
                (mesh.lo[2] + mesh.hi[2]) * 0.5,
            ],
            sorted_ext[0],
            sorted_env[0],
            "mm",
            "The table travel is the hard limit; nothing larger can be presented to the spindle in one setup.",
            "Split the part, or move it to a larger machine.".to_string(),
        );
        affordances.push("split the part along its long axis with a dowelled joint".to_string());
    } else if !as_is {
        affordances.push(format!(
            "rotate the part on the table: {:.0} × {:.0} × {:.0} fits the envelope only when re-oriented",
            ext[0], ext[1], ext[2]
        ));
    }
    ctx.verdict(
        ID,
        "R6 work envelope",
        usize::from(!fits),
        format!(
            "part {:.1} × {:.1} × {:.1} vs envelope {:.0} × {:.0} × {:.0}",
            ext[0], ext[1], ext[2], env[0], env[1], env[2]
        ),
        affordances,
    );
}

// ─── R7 ───────────────────────────────────────────────────────────────

fn r7_threads_and_gears(ctx: &mut Ctx, rule: &Rule, sections: &[Section]) {
    const ID: &str = "hobby_mill.r7_threads_and_gears";
    let tool_dia = rule.num("min_tool_dia_mm", 2.0);
    // Centroid eccentricity below this is tessellation noise.
    let min_amp = 0.04;

    // Track every round-ish loop by (centroid, sign) across levels.
    #[derive(Clone)]
    struct Track {
        internal: bool,
        cx: f64,
        cy: f64,
        r_mean: f64,
        // (z, mean radius, centroid x, centroid y, best_n, amp_n)
        samples: Vec<(f64, f64, f64, f64, usize, f64)>,
    }
    // Loops are clustered by proximity rather than a quantised key: a
    // thread's section centroid wanders with the helix phase.
    let mut tracks: Vec<Track> = Vec::new();
    for sec in sections {
        for lp in &sec.loops {
            let radii = lp.radii();
            let mean = radii.iter().sum::<f64>() / radii.len() as f64;
            let lo = radii.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = radii.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            if mean < 0.5 || lp.pts.len() < 12 || (hi - lo) / mean > 0.5 {
                continue; // not a round-ish feature
            }
            let max_n = (lp.pts.len() / 3).clamp(6, 240);
            let (mut best_n, mut best_amp) = (0usize, 0.0);
            for k in 6..=max_n {
                let (a, _) = lp.harmonic(k);
                if a > best_amp {
                    best_amp = a;
                    best_n = k;
                }
            }
            let internal = lp.area < 0.0;
            let near = tracks.iter_mut().find(|t| {
                t.internal == internal
                    && ((t.cx - lp.centroid[0]).powi(2) + (t.cy - lp.centroid[1]).powi(2)).sqrt()
                        < (0.15 * mean).max(0.5)
                    && (t.r_mean - mean).abs() < 0.12 * t.r_mean + 0.3
            });
            let t = match near {
                Some(t) => t,
                None => {
                    tracks.push(Track {
                        internal,
                        cx: lp.centroid[0],
                        cy: lp.centroid[1],
                        r_mean: mean,
                        samples: Vec::new(),
                    });
                    tracks.last_mut().unwrap()
                }
            };
            let n = t.samples.len() as f64;
            t.r_mean = (t.r_mean * n + mean) / (n + 1.0);
            t.samples.push((
                sec.z,
                mean,
                lp.centroid[0],
                lp.centroid[1],
                best_n,
                best_amp,
            ));
        }
    }

    let mut findings: Vec<(String, [f64; 3], f64, String)> = Vec::new();
    for t in &tracks {
        let mut s = t.samples.clone();
        s.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        // Thread: a single-start thread's section is an off-centre circle,
        // and the offset (the loop centroid's eccentricity about the
        // track's axis) rotates steadily with z.
        let n = s.len() as f64;
        let ax = s.iter().map(|x| x.2).sum::<f64>() / n;
        let ay = s.iter().map(|x| x.3).sum::<f64>() / n;
        let ecc: Vec<(f64, f64, f64, f64)> = s
            .iter()
            .map(|x| {
                let (ex, ey) = (x.2 - ax, x.3 - ay);
                (x.0, x.1, (ex * ex + ey * ey).sqrt(), ey.atan2(ex))
            })
            .collect();
        let mut helical: Vec<(f64, f64, f64)> = Vec::new(); // (z, dphi/dz, radius)
        for w in ecc.windows(2) {
            let (z0, r0, a0, p0) = w[0];
            let (z1, r1, a1, p1) = w[1];
            if a0 < min_amp || a1 < min_amp || (r1 - r0).abs() > 0.3 * r0 {
                continue;
            }
            let dz = z1 - z0;
            if !(1e-4..=1.5).contains(&dz) {
                continue;
            }
            let mut dphi = p1 - p0;
            while dphi > PI {
                dphi -= 2.0 * PI;
            }
            while dphi < -PI {
                dphi += 2.0 * PI;
            }
            if dphi.abs() > 0.15 && dphi.abs() < 2.9 {
                helical.push((z0, dphi / dz, (r0 + r1) * 0.5));
            }
        }
        let consistent = helical.len() >= 3
            && helical
                .windows(2)
                .filter(|w| (w[0].1 * w[1].1) > 0.0)
                .count()
                + 1
                >= helical.len().saturating_sub(1);
        if consistent {
            let z0 = helical.first().map(|h| h.0).unwrap_or(0.0);
            let z1 = helical.last().map(|h| h.0).unwrap_or(0.0);
            let rate = helical.iter().map(|h| h.1.abs()).sum::<f64>() / helical.len() as f64;
            let pitch = 2.0 * PI / rate;
            let r = helical.iter().map(|h| h.2).sum::<f64>() / helical.len() as f64;
            let kind = if t.internal { "internal" } else { "external" };
            let fix = if t.internal {
                format!(
                    "Ø{:.1} × {:.2} pitch internal thread: tap it if a standard size, else buy a threaded insert or EDM/single-point it — not millable with a Ø{:.1} end mill",
                    2.0 * r, pitch, tool_dia
                )
            } else {
                format!(
                    "Ø{:.1} × {:.2} pitch external thread: buy it (threaded stud / die) or single-point on a lathe — not a mill feature",
                    2.0 * r, pitch
                )
            };
            findings.push((
                format!(
                    "Helical {} thread Ø{:.1}, pitch ≈ {:.2}, z {:.1}..{:.1} at ({:.1}, {:.1})",
                    kind,
                    2.0 * r,
                    pitch,
                    z0,
                    z1,
                    t.cx,
                    t.cy
                ),
                [t.cx, t.cy, (z0 + z1) * 0.5],
                pitch,
                fix,
            ));
            continue;
        }

        // Gear: a dominant N-fold ripple with a tooth space narrower than the cutter.
        let gearish: Vec<&(f64, f64, f64, f64, usize, f64)> =
            s.iter().filter(|x| x.5 > 0.1 && x.4 >= 6).collect();
        if gearish.len() >= 2 {
            let n = gearish[gearish.len() / 2].4;
            if gearish.iter().filter(|g| g.4 == n).count() * 2 < gearish.len() {
                continue;
            }
            let r = gearish.iter().map(|g| g.1).sum::<f64>() / gearish.len() as f64;
            let space = PI * r / n as f64;
            if space < tool_dia {
                let z0 = gearish.first().unwrap().0;
                let z1 = gearish.last().unwrap().0;
                findings.push((
                    format!(
                        "{} gear-like teeth on Ø{:.1} (space ≈ {:.2} < Ø{:.1} cutter), z {:.1}..{:.1} at ({:.1}, {:.1})",
                        n, 2.0 * r, space, tool_dia, z0, z1, t.cx, t.cy
                    ),
                    [t.cx, t.cy, (z0 + z1) * 0.5],
                    space,
                    format!(
                        "{}-tooth gear finer than the cutter: buy a stock gear, hob it, or wire-EDM the profile",
                        n
                    ),
                ));
            }
        }
    }
    findings.sort_by(|a, b| a.1[2].partial_cmp(&b.1[2]).unwrap());
    let count = findings.len();
    let mut affordances = Vec::new();
    for (msg, anchor, measured, fix) in findings.iter().take(MAX_EXAMPLES) {
        affordances.push(format!("buy or EDM: {}", fix));
        ctx.issue(
            ID,
            rule.severity_enum(),
            msg.clone(),
            *anchor,
            *measured,
            tool_dia,
            "mm",
            "Threads are helical and gear flanks are involute: neither is a 3-axis end-mill \
             shape at hobby scale. Both are commodity parts — buy them, or send the feature to EDM.",
            fix.clone(),
        );
    }
    ctx.verdict(
        ID,
        "R7 threads / gear teeth",
        count,
        if count == 0 {
            "no helical or gear-tooth features".to_string()
        } else {
            format!(
                "{} thread / gear feature{}",
                count,
                if count == 1 { "" } else { "s" }
            )
        },
        affordances,
    );
}

// ─── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_tessellate::triangulate_polygon_2d;

    fn pack() -> RulePack {
        RulePack::named("hobby-3axis-mill").unwrap()
    }

    fn report(tris: &[[[f64; 3]; 3]]) -> crate::DfmReport {
        crate::run_dfm_mesh(tris, &pack())
    }

    fn verdict<'a>(r: &'a crate::DfmReport, rule: &str) -> &'a RuleResult {
        r.rule_results
            .iter()
            .find(|x| x.rule.ends_with(rule))
            .unwrap_or_else(|| panic!("no verdict for {rule}"))
    }

    /// Axis mapping for `extrude`: (u, v, w) → world.
    type Map = fn(f64, f64, f64) -> [f64; 3];
    fn xyz(u: f64, v: f64, w: f64) -> [f64; 3] {
        [u, v, w]
    }
    /// Profile in the XZ plane, extruded along +Y.
    fn xzy(u: f64, v: f64, w: f64) -> [f64; 3] {
        [u, w, v]
    }

    /// Extrude a CCW outer polygon with CW holes from w0 to w1, outward
    /// normals. `map` chooses which world axes (u, v, w) land on.
    fn extrude(
        outer: &[(f64, f64)],
        holes: &[Vec<(f64, f64)>],
        w0: f64,
        w1: f64,
        map: Map,
    ) -> Vec<[[f64; 3]; 3]> {
        let mut tris = Vec::new();
        let mut all: Vec<(f64, f64)> = outer.to_vec();
        for h in holes {
            all.extend(h.iter().cloned());
        }
        let cap = triangulate_polygon_2d(outer, holes).expect("triangulate");
        // Whether `map` flips handedness decides which cap faces +w.
        let flip = {
            let e0 = sub(map(1.0, 0.0, 0.0), map(0.0, 0.0, 0.0));
            let e1 = sub(map(0.0, 1.0, 0.0), map(0.0, 0.0, 0.0));
            let e2 = sub(map(0.0, 0.0, 1.0), map(0.0, 0.0, 0.0));
            let c = cross(e0, e1);
            c[0] * e2[0] + c[1] * e2[1] + c[2] * e2[2] < 0.0
        };
        for t in &cap {
            let p = |i: u32, w: f64| {
                let (u, v) = all[i as usize];
                map(u, v, w)
            };
            let (top, bot) = (
                [p(t[0], w1), p(t[1], w1), p(t[2], w1)],
                [p(t[0], w0), p(t[2], w0), p(t[1], w0)],
            );
            if flip {
                tris.push([top[0], top[2], top[1]]);
                tris.push([bot[0], bot[2], bot[1]]);
            } else {
                tris.push(top);
                tris.push(bot);
            }
        }
        let mut wall = |ring: &[(f64, f64)]| {
            let n = ring.len();
            for i in 0..n {
                let (a, b) = (ring[i], ring[(i + 1) % n]);
                let (a0, a1) = (map(a.0, a.1, w0), map(a.0, a.1, w1));
                let (b0, b1) = (map(b.0, b.1, w0), map(b.0, b.1, w1));
                if flip {
                    tris.push([a0, a1, b0]);
                    tris.push([b0, a1, b1]);
                } else {
                    tris.push([a0, b0, a1]);
                    tris.push([b0, b1, a1]);
                }
            }
        };
        wall(outer);
        for h in holes {
            wall(h);
        }
        tris
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }
    fn rev(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        v.reverse();
        v
    }
    fn circle(cx: f64, cy: f64, r: f64, n: usize) -> Vec<(f64, f64)> {
        (0..n)
            .map(|i| {
                let a = 2.0 * PI * i as f64 / n as f64;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    }

    /// A 40 × 30 plate of thickness `t` with a blind pocket `pocket`
    /// (CCW in plate coordinates) of depth `depth` from the top.
    fn plate_with_pocket(t: f64, pocket: Vec<(f64, f64)>, depth: f64) -> Vec<[[f64; 3]; 3]> {
        let outer = rect(0.0, 0.0, 40.0, 30.0);
        let floor_z = t - depth;
        // Top slab: outer minus pocket, from floor to top.
        let mut tris = extrude(&outer, &[rev(pocket.clone())], floor_z, t, xyz);
        // Remove the bottom cap of the slab and top cap of the base: build
        // the base as a full plate below the floor and stitch by simply
        // emitting both (interior coincident caps cancel for our
        // slicing/ray purposes only if removed) — so drop them explicitly.
        tris.retain(|tr| !tr.iter().all(|p| (p[2] - floor_z).abs() < 1e-9));
        let mut base = extrude(&outer, &[], 0.0, floor_z, xyz);
        base.retain(|tr| !tr.iter().all(|p| (p[2] - floor_z).abs() < 1e-9));
        tris.extend(base);
        // Pocket floor: the pocket polygon at floor_z facing up.
        let cap = triangulate_polygon_2d(&pocket, &[]).unwrap();
        for c in cap {
            tris.push([
                [pocket[c[0] as usize].0, pocket[c[0] as usize].1, floor_z],
                [pocket[c[1] as usize].0, pocket[c[1] as usize].1, floor_z],
                [pocket[c[2] as usize].0, pocket[c[2] as usize].1, floor_z],
            ]);
        }
        tris
    }

    /// A rectangle with "mouse ear" corner reliefs of radius `r_ear`.
    fn pocket_with_mouse_ears(x0: f64, y0: f64, x1: f64, y1: f64, r_ear: f64) -> Vec<(f64, f64)> {
        let mut pts = Vec::new();
        // Walk CCW; at each corner insert an ear arc bulging outward
        // (into material), centred on the corner.
        let corners = [
            ((x0, y0), (-1.0, -1.0)),
            ((x1, y0), (1.0, -1.0)),
            ((x1, y1), (1.0, 1.0)),
            ((x0, y1), (-1.0, 1.0)),
        ];
        let n = 16;
        for (i, ((cx, cy), _)) in corners.iter().enumerate() {
            // Incoming edge direction at this corner (CCW order).
            let base = match i {
                0 => PI * 0.5, // arriving from (x0,y1) going down → ear opens toward -x/-y
                1 => PI,       // arriving from (x0,y0) going +x
                2 => PI * 1.5, // arriving going +y
                _ => 0.0,      // arriving going -x
            };
            // Ear arc: 270° sweep bulging away from the pocket.
            for k in 0..=n {
                let a = base + 1.5 * PI * k as f64 / n as f64;
                // a 270° arc from the incoming wall around the outside to the outgoing wall
                pts.push((cx + r_ear * a.cos(), cy + r_ear * a.sin()));
            }
        }
        pts
    }

    #[test]
    fn sharp_pocket_fails_r1_and_mouse_ears_pass() {
        let sharp = plate_with_pocket(6.0, rect(10.0, 8.0, 30.0, 22.0), 3.0);
        let r = report(&sharp);
        let v = verdict(&r, "r1_internal_corner_radius");
        assert!(!v.passed, "sharp pocket must fail R1: {}", v.summary);
        assert_eq!(v.violation_count, 4, "{}", v.summary);
        assert!(
            v.affordances[0].contains("corner relief Ø2.2"),
            "{:?}",
            v.affordances
        );
        assert!(verdict(&r, "r4_plate_stock").passed);
        assert!(verdict(&r, "r2_reachability").passed);

        let eared = plate_with_pocket(6.0, pocket_with_mouse_ears(10.0, 8.0, 30.0, 22.0, 1.1), 3.0);
        let r = report(&eared);
        let v = verdict(&r, "r1_internal_corner_radius");
        assert!(
            v.passed,
            "mouse-eared pocket must pass R1: {} / {:?}",
            v.summary,
            r.issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn undercut_fails_r2() {
        // T-slot profile in XZ (CCW), extruded 30 along Y: a 4-wide neck
        // opening into a 12-wide chamber under the top face.
        let profile = vec![
            (0.0, 0.0),
            (40.0, 0.0),
            (40.0, 10.0),
            (22.0, 10.0),
            (22.0, 7.0),
            (26.0, 7.0),
            (26.0, 3.0),
            (14.0, 3.0),
            (14.0, 7.0),
            (18.0, 7.0),
            (18.0, 10.0),
            (0.0, 10.0),
        ];
        let tris = extrude(&profile, &[], 0.0, 30.0, xzy);
        let r = report(&tris);
        let v = verdict(&r, "r2_reachability");
        assert!(!v.passed, "T-slot must fail R2: {}", v.summary);
        assert!(r
            .issues
            .iter()
            .any(|i| i.rule.ends_with("r2_reachability") && i.message.contains("Undercut")));

        // A plain step needs no flip and has no undercut.
        let step = vec![
            (0.0, 0.0),
            (40.0, 0.0),
            (40.0, 6.0),
            (20.0, 6.0),
            (20.0, 10.0),
            (0.0, 10.0),
        ];
        let r = report(&extrude(&step, &[], 0.0, 30.0, xzy));
        assert!(verdict(&r, "r2_reachability").passed);
    }

    #[test]
    fn off_size_bore_fails_r3_with_ream_suggestion() {
        let outer = rect(0.0, 0.0, 40.0, 30.0);
        let hole = rev(circle(20.0, 15.0, 1.9, 64));
        let tris = extrude(&outer, &[hole], 0.0, 6.0, xyz);
        let r = report(&tris);
        let v = verdict(&r, "r3_hole_diameters");
        assert!(!v.passed, "{}", v.summary);
        assert!(
            v.affordances[0].starts_with("bore Ø3.80 → ream Ø4 H7"),
            "{:?}",
            v.affordances
        );
        assert!(verdict(&r, "r1_internal_corner_radius").passed);

        let good = extrude(&outer, &[rev(circle(20.0, 15.0, 2.0, 64))], 0.0, 6.0, xyz);
        assert!(verdict(&report(&good), "r3_hole_diameters").passed);
    }

    #[test]
    fn off_stock_plate_fails_r4() {
        let tris = extrude(&rect(0.0, 0.0, 40.0, 30.0), &[], 0.0, 4.2, xyz);
        let r = report(&tris);
        let v = verdict(&r, "r4_plate_stock");
        assert!(!v.passed, "{}", v.summary);
        assert!(
            v.affordances
                .iter()
                .any(|a| a.contains("face Ø5.0 plate down to 4.20")),
            "{:?}",
            v.affordances
        );
        assert!(verdict(&r, "r6_envelope").passed);
        assert!(verdict(&r, "r5_min_wall").passed);
    }

    #[test]
    fn thin_floor_fails_r5_and_oversize_fails_r6() {
        let tris = plate_with_pocket(6.0, rect(10.0, 8.0, 30.0, 22.0), 5.5);
        let rep = report(&tris);
        let v = verdict(&rep, "r5_min_wall");
        assert!(!v.passed, "{}", v.summary);
        assert!(v.summary.contains("0.50"), "{}", v.summary);

        let big = extrude(&rect(0.0, 0.0, 350.0, 30.0), &[], 0.0, 6.0, xyz);
        assert!(!verdict(&report(&big), "r6_envelope").passed);
        // Fits when rotated: 250 × 90 × 6 → sorted extents inside 300/200/80.
        let rot = extrude(&rect(0.0, 0.0, 90.0, 250.0), &[], 0.0, 6.0, xyz);
        assert!(verdict(&report(&rot), "r6_envelope").passed);
    }

    #[test]
    fn internal_thread_fails_r7() {
        // A 20-tall block with a Ø10 bore carrying a helical 45° V-thread,
        // pitch 2, built as stacked rings whose radius varies with angle
        // and advances with z.
        let (cx, cy) = (20.0, 15.0);
        let outer = rect(0.0, 0.0, 40.0, 30.0);
        let (r_minor, depth, pitch) = (5.0, 0.6, 2.0);
        let n_ang = 96;
        let n_z = 100;
        let h = 20.0;
        let ring = |k: usize| -> Vec<[f64; 3]> {
            let z = h * k as f64 / n_z as f64;
            (0..n_ang)
                .map(|i| {
                    let a = 2.0 * PI * i as f64 / n_ang as f64;
                    // sawtooth-ish thread profile: r = minor + depth * tri(phase)
                    let ph = ((a / (2.0 * PI)) - z / pitch).rem_euclid(1.0);
                    let tri = 1.0 - (2.0 * ph - 1.0).abs();
                    let r = r_minor + depth * tri;
                    [cx + r * a.cos(), cy + r * a.sin(), z]
                })
                .collect()
        };
        let mut tris = Vec::new();
        let rings: Vec<Vec<[f64; 3]>> = (0..=n_z).map(ring).collect();
        for k in 0..n_z {
            for i in 0..n_ang {
                let j = (i + 1) % n_ang;
                let (a, b, c, d) = (rings[k][i], rings[k][j], rings[k + 1][i], rings[k + 1][j]);
                // inward-facing bore wall: wind so the normal points to the axis
                tris.push([a, c, b]);
                tris.push([b, c, d]);
            }
        }
        // Caps: outer rect minus the ring polygon at each end.
        for (z, top) in [(0.0, false), (h, true)] {
            let k = if top { n_z } else { 0 };
            let hole: Vec<(f64, f64)> = rev(rings[k].iter().map(|p| (p[0], p[1])).collect());
            let mut all = outer.clone();
            all.extend(hole.iter().cloned());
            for t in triangulate_polygon_2d(&outer, std::slice::from_ref(&hole)).unwrap() {
                let p = |i: u32| [all[i as usize].0, all[i as usize].1, z];
                if top {
                    tris.push([p(t[0]), p(t[1]), p(t[2])]);
                } else {
                    tris.push([p(t[0]), p(t[2]), p(t[1])]);
                }
            }
        }
        // Outer walls.
        for i in 0..4 {
            let (a, b) = (outer[i], outer[(i + 1) % 4]);
            tris.push([[a.0, a.1, 0.0], [b.0, b.1, 0.0], [a.0, a.1, h]]);
            tris.push([[b.0, b.1, 0.0], [b.0, b.1, h], [a.0, a.1, h]]);
        }
        let r = report(&tris);
        let v = verdict(&r, "r7_threads_and_gears");
        assert!(!v.passed, "{}", v.summary);
        let msg = &r
            .issues
            .iter()
            .find(|i| i.rule.ends_with("r7_threads_and_gears"))
            .unwrap()
            .message;
        assert!(msg.contains("internal thread"), "{msg}");
        assert!(msg.contains("pitch ≈ 2.0"), "{msg}");
        assert!(
            v.affordances[0].starts_with("buy or EDM"),
            "{:?}",
            v.affordances
        );
    }

    /// Two overlapping boxes exported as separate shells must be judged
    /// as their union: no buried "walls", no false undercuts.
    #[test]
    fn loose_union_of_shells_is_unioned() {
        let mut tris = extrude(&rect(0.0, 0.0, 30.0, 30.0), &[], 0.0, 6.0, xyz);
        tris.extend(extrude(&rect(20.0, 10.0, 50.0, 40.0), &[], 0.0, 6.0, xyz));
        let r = report(&tris);
        assert!(
            verdict(&r, "r5_min_wall").passed,
            "{}",
            verdict(&r, "r5_min_wall").summary
        );
        assert!(
            verdict(&r, "r2_reachability").passed,
            "{}",
            verdict(&r, "r2_reachability").summary
        );
        // The L-shaped union has exactly one internal corner, at (20, 30)
        // and (30, 10) — two, in fact — both sharp.
        let v = verdict(&r, "r1_internal_corner_radius");
        assert_eq!(v.violation_count, 2, "{}", v.summary);
        assert!(verdict(&r, "r4_plate_stock").passed);
    }

    #[test]
    fn brep_entry_point_tessellates() {
        let brep = vcad_kernel_primitives::make_cube(40.0, 30.0, 6.0);
        let r = crate::run_dfm(&brep, None, Process::Cnc3Axis, &pack());
        assert_eq!(r.rule_results.len(), 7);
        assert!(r.rule_results.iter().all(|v| v.passed), "{:?}", r.issues);
    }
}
