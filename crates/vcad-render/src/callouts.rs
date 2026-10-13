//! Hole and bolt-circle callouts (`--callouts`).
//!
//! A drawing handed to a fabricator needs its hole pattern called out, not
//! just the overall envelope `--dims` gives. This module finds cylindrical
//! holes and bosses whose axis is parallel to the view direction straight
//! from the evaluated BRep (the same face walk `--exact-edges` does), tells
//! hole from boss by the mesh's outward normals, groups equal holes that sit
//! on a common circle at equal pitch into one "n× Ød on Øpcd PCD" callout,
//! and plans drafting-style leaders for each callout outside the part's
//! silhouette. Mesh-only parts have no BRep and simply contribute nothing.
//!
//! Everything here is geometry: detection works in 3D, planning in a 2D
//! screen space the caller supplies through a projection closure, and the
//! SVG and raster paths each draw the resulting [`Plan`] in their own
//! primitives.

use std::collections::HashMap;

use vcad_kernel::vcad_kernel_geom::CylinderSurface;
use vcad_kernel::vcad_kernel_topo::{LoopId, Topology};
use vcad_kernel::Solid;

use crate::{dot, format_mm, normalize, sub, SolidArtifacts};

/// `cos(1°)`: a cylinder axis counts as parallel to the view direction when
/// the absolute dot product of the two unit vectors exceeds this.
const AXIS_PARALLEL_COS: f64 = 0.99985;
/// Two features are the same hole (stacked through coaxial parts) when
/// their axes and radii agree to this many mm.
const DEDUP_MM: f64 = 0.02;
/// Holes count as equal-diameter for grouping when their radii agree to
/// this many mm — and all members of a pitch circle must sit on it to the
/// same tolerance.
const PCD_RADIUS_TOL_MM: f64 = 0.1;
/// Equal angular pitch tolerance, degrees.
const PCD_PITCH_TOL_DEG: f64 = 0.5;
/// A first-hole angle below this (degrees) is reported as zero and omitted.
const PHASE_ZERO_DEG: f64 = 0.25;

/// Whether a cylindrical feature removes material (hole) or adds it (boss).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Hole,
    Boss,
}

/// One cylindrical feature whose axis is parallel to the view direction.
#[derive(Debug, Clone)]
pub(crate) struct CylFeature {
    pub kind: Kind,
    /// Axis point at the camera-near end of the feature.
    pub near: [f64; 3],
    pub radius: f64,
}

/// Collect the 3D positions of every vertex on a face loop (bounded by the
/// half-edge count so corrupt topology can't spin forever).
fn loop_points(topo: &Topology, lp: LoopId, out: &mut Vec<[f64; 3]>) {
    let Some(l) = topo.loops.get(lp) else { return };
    let start = l.half_edge;
    let mut he = start;
    for _ in 0..topo.half_edges.len() {
        let Some(h) = topo.half_edges.get(he) else {
            return;
        };
        if let Some(vx) = topo.vertices.get(h.origin) {
            let p = vx.point;
            out.push([p.x, p.y, p.z]);
        }
        match h.next {
            Some(n) if n != start => he = n,
            _ => return,
        }
    }
}

/// Find every cylindrical face across `solids` whose axis is parallel to
/// `cam`, classify it as hole or boss from the tessellation's outward
/// normals, and de-duplicate coaxial equal-radius features (a bolt hole
/// running through a plate and the disc under it is one callout). The
/// `right`/`down` vectors are the view's screen basis (unit length).
pub(crate) fn detect(
    solids: &[Solid],
    arts: &[SolidArtifacts],
    cam: [f64; 3],
    right: [f64; 3],
    down: [f64; 3],
    segments: u32,
) -> Vec<CylFeature> {
    let seg = segments.max(3) as f64;
    let sag = |r: f64| r * (1.0 - (std::f64::consts::PI / seg).cos());

    let mut feats: Vec<CylFeature> = Vec::new();
    // Dedup key: quantized screen-plane axis position, radius, kind. Keeps
    // the camera-nearest end so the leader lands on the visible rim.
    let mut seen: HashMap<(i64, i64, i64, Kind), usize> = HashMap::new();
    let q = |x: f64| (x / DEDUP_MM).round() as i64;

    for art in arts {
        let Some(solid) = solids.get(art.src) else {
            continue;
        };
        let Some(brep) = solid.as_brep() else {
            continue;
        };
        let topo = &brep.topology;
        let geom = &brep.geometry;
        for (_fid, face) in &topo.faces {
            let Some(surface) = geom.surfaces.get(face.surface_index) else {
                continue;
            };
            let Some(cyl) = surface.as_any().downcast_ref::<CylinderSurface>() else {
                continue;
            };
            let n = normalize([
                cyl.axis.as_ref().x,
                cyl.axis.as_ref().y,
                cyl.axis.as_ref().z,
            ]);
            if n == [0.0; 3] || dot(n, cam).abs() < AXIS_PARALLEL_COS {
                continue;
            }
            let r = cyl.radius;
            if !(r.is_finite() && r > 1e-6) {
                continue;
            }
            let c0 = [cyl.center.x, cyl.center.y, cyl.center.z];
            let mut pts: Vec<[f64; 3]> = Vec::new();
            loop_points(topo, face.outer_loop, &mut pts);
            for &il in &face.inner_loops {
                loop_points(topo, il, &mut pts);
            }
            if pts.is_empty() {
                continue;
            }
            let ts: Vec<f64> = pts.iter().map(|p| dot(sub(*p, c0), n)).collect();
            let tmin = ts.iter().cloned().fold(f64::INFINITY, f64::min);
            let tmax = ts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            if tmax - tmin <= 1e-6 {
                continue;
            }

            // Hole or boss: sum the outward mesh normals' radial component
            // over the triangles that lie on this cylinder. Inward-facing
            // (negative) means the surface bounds a void — a hole.
            let tol = (4.0 * sag(r)).max(0.02);
            let on_surface = |p: [f64; 3]| -> Option<[f64; 3]> {
                let d = sub(p, c0);
                let axial = dot(d, n);
                if axial < tmin - tol || axial > tmax + tol {
                    return None;
                }
                let radial = [
                    d[0] - n[0] * axial,
                    d[1] - n[1] * axial,
                    d[2] - n[2] * axial,
                ];
                let len = dot(radial, radial).sqrt();
                if (len - r).abs() > tol {
                    return None;
                }
                Some([radial[0] / len, radial[1] / len, radial[2] / len])
            };
            let mut sign = 0.0;
            let mut count = 0usize;
            for (ti, t) in art.tris.iter().enumerate() {
                let (a, b, c) = (art.verts[t[0]], art.verts[t[1]], art.verts[t[2]]);
                if on_surface(a).is_none() || on_surface(b).is_none() || on_surface(c).is_none() {
                    continue;
                }
                let centroid = [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                ];
                let Some(radial) = on_surface(centroid) else {
                    continue;
                };
                sign += dot(art.normals[ti], radial);
                count += 1;
            }
            if count == 0 {
                continue;
            }
            let kind = if sign < 0.0 { Kind::Hole } else { Kind::Boss };
            let t_near = if dot(n, cam) > 0.0 { tmax } else { tmin };
            let near = [
                c0[0] + n[0] * t_near,
                c0[1] + n[1] * t_near,
                c0[2] + n[2] * t_near,
            ];
            let key = (q(dot(near, right)), q(dot(near, down)), q(r), kind);
            match seen.get(&key) {
                Some(&i) => {
                    if dot(near, cam) > dot(feats[i].near, cam) {
                        feats[i].near = near;
                    }
                }
                None => {
                    seen.insert(key, feats.len());
                    feats.push(CylFeature {
                        kind,
                        near,
                        radius: r,
                    });
                }
            }
        }
    }
    // Canonical order so output is deterministic regardless of face order.
    feats.sort_by(|a, b| {
        a.radius
            .total_cmp(&b.radius)
            .then(dot(a.near, right).total_cmp(&dot(b.near, right)))
            .then(dot(a.near, down).total_cmp(&dot(b.near, down)))
    });
    feats
}

/// A pitch circle through ≥3 equal features at equal angular pitch.
#[derive(Debug, Clone)]
pub(crate) struct PitchCircle {
    /// Member indices into the feature list, sorted by angle from the
    /// first hole.
    pub members: Vec<usize>,
    /// Circle centre, 3D.
    pub center: [f64; 3],
    /// Pitch-circle radius (mm).
    pub radius: f64,
    /// Angle of the first hole (degrees, CCW as seen from the camera,
    /// measured from the view's `right` axis); `0.0` when below the
    /// reporting threshold.
    pub phase_deg: f64,
}

/// One callout to draw: a single feature or a grouped pitch circle.
#[derive(Debug, Clone)]
pub(crate) enum Callout {
    Single(usize),
    Pcd(PitchCircle),
}

/// Least-squares (Kåsa) circle fit through 2D points. `None` when the
/// points are (near-)collinear.
fn fit_circle(pts: &[(f64, f64)]) -> Option<((f64, f64), f64)> {
    // Normal equations for x² + y² + a·x + b·y + c = 0.
    let mut m = [[0.0f64; 3]; 3];
    let mut v = [0.0f64; 3];
    for &(x, y) in pts {
        let z = -(x * x + y * y);
        let row = [x, y, 1.0];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += row[i] * row[j];
            }
            v[i] += row[i] * z;
        }
    }
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    if d.abs() < 1e-9 {
        return None;
    }
    let mut sol = [0.0; 3];
    for (k, s) in sol.iter_mut().enumerate() {
        let mut mk = m;
        for row in 0..3 {
            mk[row][k] = v[row];
        }
        *s = det(&mk) / d;
    }
    let (cx, cy) = (-sol[0] / 2.0, -sol[1] / 2.0);
    let r2 = cx * cx + cy * cy - sol[2];
    if r2.is_nan() || r2 <= 1e-12 {
        return None;
    }
    Some(((cx, cy), r2.sqrt()))
}

/// Angle of `(x, y)` about `c`, degrees in `[0, 360)`, counter-clockwise
/// as seen from the camera (screen `y` points down, so it is negated).
fn angle_deg(c: (f64, f64), p: (f64, f64)) -> f64 {
    let a = (-(p.1 - c.1)).atan2(p.0 - c.0).to_degrees();
    a.rem_euclid(360.0)
}

/// A recognised pitch circle: members sorted by angle, centre, radius,
/// first-hole angle (degrees).
type PitchFit = (Vec<usize>, (f64, f64), f64, f64);

/// Try to read `idx` (indices into `pts`) as one pitch circle: all points
/// on a common circle within [`PCD_RADIUS_TOL_MM`] and at equal angular
/// pitch within [`PCD_PITCH_TOL_DEG`] (a full ring or a partial arc).
/// Returns the members sorted by angle, the centre, the radius, and the
/// first-hole angle.
fn pitch_circle(pts: &[(f64, f64)], idx: &[usize]) -> Option<PitchFit> {
    if idx.len() < 3 {
        return None;
    }
    let sel: Vec<(f64, f64)> = idx.iter().map(|&i| pts[i]).collect();
    let (c, r) = fit_circle(&sel)?;
    if r < 1e-3 {
        return None;
    }
    for p in &sel {
        let d = ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt();
        if (d - r).abs() > PCD_RADIUS_TOL_MM {
            return None;
        }
    }
    // A hole nominally at 0° can land at 359.999° through rounding; fold
    // anything within the zero threshold of a full turn back below zero so
    // it sorts first and the pitch check sees 0 → 60, not 60 → 359.999.
    let mut ordered: Vec<(f64, usize)> = idx
        .iter()
        .map(|&i| {
            let a = angle_deg(c, pts[i]);
            (
                if a >= 360.0 - PHASE_ZERO_DEG {
                    a - 360.0
                } else {
                    a
                },
                i,
            )
        })
        .collect();
    ordered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let angles: Vec<f64> = ordered.iter().map(|o| o.0).collect();
    let n = angles.len();
    let mut diffs: Vec<f64> = (0..n - 1).map(|k| angles[k + 1] - angles[k]).collect();
    let wrap = angles[0] + 360.0 - angles[n - 1];
    let spread = |d: &[f64]| {
        let lo = d.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = d.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        hi - lo
    };
    diffs.push(wrap);
    let full = spread(&diffs) <= PCD_PITCH_TOL_DEG;
    diffs.pop();
    if !full && spread(&diffs) > PCD_PITCH_TOL_DEG {
        return None;
    }
    let members: Vec<usize> = ordered.iter().map(|o| o.1).collect();
    let phase = if angles[0].abs() < PHASE_ZERO_DEG {
        0.0
    } else {
        angles[0]
    };
    Some((members, c, r, phase))
}

/// Group features into callouts: equal-diameter holes (or bosses) that
/// share a pitch circle become one [`Callout::Pcd`]; everything else is a
/// [`Callout::Single`]. `right`/`down` are the unit screen basis and `cam`
/// the unit view direction.
pub(crate) fn group(
    feats: &[CylFeature],
    cam: [f64; 3],
    right: [f64; 3],
    down: [f64; 3],
) -> Vec<Callout> {
    // Screen-plane mm coordinates and view-depth of each feature.
    let pts: Vec<(f64, f64)> = feats
        .iter()
        .map(|f| (dot(f.near, right), dot(f.near, down)))
        .collect();
    let depth: Vec<f64> = feats.iter().map(|f| dot(f.near, cam)).collect();

    let mut taken = vec![false; feats.len()];
    let mut out: Vec<Callout> = Vec::new();

    // Clusters of equal radius + kind (features arrive sorted by radius).
    let mut i = 0;
    while i < feats.len() {
        let mut cluster = vec![i];
        let mut j = i + 1;
        while j < feats.len()
            && feats[j].kind == feats[i].kind
            && (feats[j].radius - feats[i].radius).abs() <= PCD_RADIUS_TOL_MM
        {
            cluster.push(j);
            j += 1;
        }
        i = j;
        if cluster.len() < 3 {
            continue;
        }
        let mut candidates: Vec<Vec<usize>> = vec![cluster.clone()];
        // Fallback partition: two concentric patterns of the same hole
        // size (or a pattern plus strays) split by distance from the
        // cluster centroid, binned at twice the radius tolerance.
        let cx = cluster.iter().map(|&k| pts[k].0).sum::<f64>() / cluster.len() as f64;
        let cy = cluster.iter().map(|&k| pts[k].1).sum::<f64>() / cluster.len() as f64;
        let mut bins: HashMap<i64, Vec<usize>> = HashMap::new();
        for &k in &cluster {
            let d = ((pts[k].0 - cx).powi(2) + (pts[k].1 - cy).powi(2)).sqrt();
            bins.entry((d / (2.0 * PCD_RADIUS_TOL_MM)).round() as i64)
                .or_default()
                .push(k);
        }
        let mut binned: Vec<Vec<usize>> = bins.into_values().filter(|b| b.len() >= 3).collect();
        binned.sort();
        candidates.extend(binned);
        for cand in candidates {
            if cand.iter().any(|&k| taken[k]) {
                continue;
            }
            if let Some((members, c, r, phase)) = pitch_circle(&pts, &cand) {
                for &k in &members {
                    taken[k] = true;
                }
                let z = members.iter().map(|&k| depth[k]).sum::<f64>() / members.len() as f64;
                let center = [
                    right[0] * c.0 + down[0] * c.1 + cam[0] * z,
                    right[1] * c.0 + down[1] * c.1 + cam[1] * z,
                    right[2] * c.0 + down[2] * c.1 + cam[2] * z,
                ];
                out.push(Callout::Pcd(PitchCircle {
                    members,
                    center,
                    radius: r,
                    phase_deg: phase,
                }));
            }
        }
    }
    for (k, t) in taken.iter().enumerate() {
        if !t {
            out.push(Callout::Single(k));
        }
    }
    out
}

/// A callout's label text.
pub(crate) fn label(feats: &[CylFeature], c: &Callout) -> String {
    match c {
        Callout::Single(i) => format!("Ø{}", format_mm(2.0 * feats[*i].radius)),
        Callout::Pcd(p) => {
            let d = format_mm(2.0 * feats[p.members[0]].radius);
            let mut s = format!(
                "{}× Ø{} on Ø{} PCD",
                p.members.len(),
                d,
                format_mm(2.0 * p.radius)
            );
            if p.phase_deg != 0.0 {
                s.push_str(&format!(" @ {}°", format_mm(p.phase_deg)));
            }
            s
        }
    }
}

/// Axis-aligned 2D rectangle in screen units (`x0 < x1`, `y0 < y1`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    fn overlap(&self, o: &Rect) -> f64 {
        let w = (self.x1.min(o.x1) - self.x0.max(o.x0)).max(0.0);
        let h = (self.y1.min(o.y1) - self.y0.max(o.y0)).max(0.0);
        w * h
    }
}

/// Screen-space sizing for leader planning, in the caller's units (SVG
/// user units or raster pixels).
pub(crate) struct Metrics {
    /// Advance width of one glyph.
    pub char_w: f64,
    /// Height of a line of text.
    pub text_h: f64,
    /// Clear distance between the silhouette and the leader elbow.
    pub clearance: f64,
    /// Gap between the elbow and the text.
    pub gap: f64,
    /// Canvas bounds; a label falling outside costs as much as one
    /// overlapping another label.
    pub bounds: Option<Rect>,
}

/// A planned label: top-left corner of its text box and the text.
#[derive(Debug, Clone)]
pub(crate) struct Text {
    pub pos: (f64, f64),
    pub text: String,
}

/// Drawing primitives for the callouts of one render.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Leader lines, rim anchor → elbow.
    pub leaders: Vec<((f64, f64), (f64, f64))>,
    /// Anchor dots on the rims.
    pub dots: Vec<(f64, f64)>,
    /// Dashed construction circles (centre, radius) for pitch circles.
    pub dashed_circles: Vec<((f64, f64), f64)>,
    /// Label texts.
    pub texts: Vec<Text>,
}

/// A candidate leader placement: cost, rim anchor, elbow, text box.
type Placement = (f64, (f64, f64), (f64, f64), Rect);

/// Plan leaders and labels for `callouts`. `to2d` projects a world point
/// to screen space; `silhouette` is every projected model vertex, so
/// elbows land clear of the part; `occupied` holds the text boxes already
/// placed by other overlays (dimension values, part labels) and receives
/// the callouts' own boxes. Each callout tries eight leader directions
/// and keeps the cheapest: a label overlapping another (or leaving the
/// canvas) costs its overlap area, a leader crossing another hole costs
/// far more, and a longer leader costs a little — so a hole near an edge
/// leads straight out rather than across the part.
pub(crate) fn plan(
    feats: &[CylFeature],
    callouts: &[Callout],
    right: [f64; 3],
    to2d: &dyn Fn([f64; 3]) -> (f64, f64),
    silhouette: &[(f64, f64)],
    occupied: &mut Vec<Rect>,
    m: &Metrics,
) -> Plan {
    let mut plan = Plan::default();
    // Screen radius of a world circle of radius `r` about `c`.
    let radius_px = |c: [f64; 3], r: f64| -> f64 {
        let p0 = to2d(c);
        let p1 = to2d([
            c[0] + right[0] * r,
            c[1] + right[1] * r,
            c[2] + right[2] * r,
        ]);
        ((p1.0 - p0.0).powi(2) + (p1.1 - p0.1).powi(2)).sqrt()
    };
    // Every feature's screen disc, so leaders can be charged for crossing
    // a hole they don't belong to.
    let discs: Vec<((f64, f64), f64)> = feats
        .iter()
        .map(|f| (to2d(f.near), radius_px(f.near, f.radius)))
        .collect();
    let crosses = |a: (f64, f64), b: (f64, f64), skip: usize| -> usize {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        discs
            .iter()
            .enumerate()
            .filter(|&(i, (c, r))| {
                if i == skip || len2 < 1e-12 {
                    return false;
                }
                let t = (((c.0 - a.0) * dx + (c.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
                let (px, py) = (a.0 + dx * t - c.0, a.1 + dy * t - c.1);
                (px * px + py * py).sqrt() < r + m.gap
            })
            .count()
    };
    // Cost of a label box: overlap with placed boxes plus area outside
    // the canvas.
    let box_cost = |occupied: &[Rect], rect: &Rect| -> f64 {
        let mut cost: f64 = occupied.iter().map(|o| o.overlap(rect)).sum();
        if let Some(b) = &m.bounds {
            let area = (rect.x1 - rect.x0) * (rect.y1 - rect.y0);
            cost += area - rect.overlap(b);
        }
        cost
    };
    // Lexicographic in practice: a hole crossing outweighs any overlap,
    // any overlap outweighs any leader length.
    let cross_penalty = 1.0e9 * m.text_h * m.text_h;
    let overlap_weight = 1.0e3;
    const DIRS: [(f64, f64); 8] = [
        (1.0, -1.0),
        (-1.0, -1.0),
        (1.0, 1.0),
        (-1.0, 1.0),
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, -1.0),
        (0.0, 1.0),
    ];
    for c in callouts {
        let text = label(feats, c);
        let (anchor_idx, pcd) = match c {
            Callout::Single(i) => (*i, None),
            Callout::Pcd(p) => (p.members[0], Some(p)),
        };
        let anchor_feat = &feats[anchor_idx];
        if let Some(p) = pcd {
            plan.dashed_circles
                .push((to2d(p.center), radius_px(p.center, p.radius)));
        }
        let centre = to2d(anchor_feat.near);
        let rim = radius_px(anchor_feat.near, anchor_feat.radius);
        let tw = text.chars().count() as f64 * m.char_w;

        let mut best: Option<Placement> = None;
        for (dx, dy) in DIRS.iter() {
            let len = (dx * dx + dy * dy).sqrt();
            let d = (dx / len, dy / len);
            let start = (centre.0 + d.0 * rim, centre.1 + d.1 * rim);
            let reach = silhouette
                .iter()
                .map(|p| (p.0 - start.0) * d.0 + (p.1 - start.1) * d.1)
                .fold(0.0, f64::max);
            let elbow = (
                start.0 + d.0 * (reach + m.clearance),
                start.1 + d.1 * (reach + m.clearance),
            );
            let x0 = if d.0 > 0.0 {
                elbow.0 + m.gap
            } else if d.0 < 0.0 {
                elbow.0 - m.gap - tw
            } else {
                elbow.0 - tw / 2.0
            };
            let y0 = if d.1 == 0.0 {
                elbow.1 - m.text_h / 2.0
            } else if d.1 < 0.0 {
                elbow.1 - m.text_h
            } else {
                elbow.1
            };
            let rect = Rect {
                x0,
                y0,
                x1: x0 + tw,
                y1: y0 + m.text_h,
            };
            let leader_len = reach + m.clearance;
            let cost = box_cost(occupied, &rect) * overlap_weight
                + crosses(start, elbow, anchor_idx) as f64 * cross_penalty
                + leader_len * m.text_h * 0.25;
            if best.as_ref().is_none_or(|b| cost < b.0) {
                best = Some((cost, start, elbow, rect));
            }
        }
        let Some((_, start, elbow, rect)) = best else {
            continue;
        };
        occupied.push(rect);
        plan.leaders.push((start, elbow));
        plan.dots.push(start);
        plan.texts.push(Text {
            pos: (rect.x0, rect.y0),
            text,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_circle_recovers_exact_ring() {
        let pts: Vec<(f64, f64)> = (0..6)
            .map(|k| {
                let a = (k as f64) * std::f64::consts::TAU / 6.0;
                (5.0 + 13.5 * a.cos(), -2.0 + 13.5 * a.sin())
            })
            .collect();
        let ((cx, cy), r) = fit_circle(&pts).unwrap();
        assert!(
            (cx - 5.0).abs() < 1e-9 && (cy + 2.0).abs() < 1e-9,
            "{cx} {cy}"
        );
        assert!((r - 13.5).abs() < 1e-9, "{r}");
    }

    #[test]
    fn fit_circle_rejects_collinear() {
        assert!(fit_circle(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]).is_none());
    }

    #[test]
    fn pitch_circle_full_ring_phase_and_order() {
        // Six holes starting at 30°, screen y down (so CCW-from-camera
        // angles negate y).
        let pts: Vec<(f64, f64)> = (0..6)
            .map(|k| {
                let a = (30.0 + 60.0 * k as f64).to_radians();
                (13.5 * a.cos(), -13.5 * a.sin())
            })
            .collect();
        let idx: Vec<usize> = (0..6).collect();
        let (members, _c, r, phase) = pitch_circle(&pts, &idx).unwrap();
        assert_eq!(members, idx);
        assert!((r - 13.5).abs() < 1e-9);
        assert!((phase - 30.0).abs() < 1e-9, "{phase}");
    }

    #[test]
    fn pitch_circle_unequal_pitch_rejected() {
        let angles = [0.0f64, 60.0, 130.0, 180.0, 240.0, 300.0];
        let pts: Vec<(f64, f64)> = angles
            .iter()
            .map(|a| (10.0 * a.to_radians().cos(), -10.0 * a.to_radians().sin()))
            .collect();
        let idx: Vec<usize> = (0..6).collect();
        assert!(pitch_circle(&pts, &idx).is_none());
    }

    #[test]
    fn pitch_circle_partial_arc_accepted() {
        let angles = [0.0f64, 30.0, 60.0];
        let pts: Vec<(f64, f64)> = angles
            .iter()
            .map(|a| (10.0 * a.to_radians().cos(), -10.0 * a.to_radians().sin()))
            .collect();
        let idx: Vec<usize> = (0..3).collect();
        let (_, _, r, phase) = pitch_circle(&pts, &idx).unwrap();
        assert!((r - 10.0).abs() < 1e-9);
        assert_eq!(phase, 0.0);
    }

    #[test]
    fn labels_format_like_drafting() {
        let feats = vec![
            CylFeature {
                kind: Kind::Hole,
                near: [0.0; 3],
                radius: 1.7,
            },
            CylFeature {
                kind: Kind::Hole,
                near: [0.0; 3],
                radius: 3.0,
            },
        ];
        assert_eq!(label(&feats, &Callout::Single(0)), "Ø3.4");
        assert_eq!(label(&feats, &Callout::Single(1)), "Ø6");
        let pcd = Callout::Pcd(PitchCircle {
            members: vec![0, 0, 0, 0, 0, 0],
            center: [0.0; 3],
            radius: 13.5,
            phase_deg: 0.0,
        });
        assert_eq!(label(&feats, &pcd), "6× Ø3.4 on Ø27 PCD");
        let phased = Callout::Pcd(PitchCircle {
            members: vec![0, 0, 0],
            center: [0.0; 3],
            radius: 13.5,
            phase_deg: 30.0,
        });
        assert_eq!(label(&feats, &phased), "3× Ø3.4 on Ø27 PCD @ 30°");
    }
}
