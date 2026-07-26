//! Recover a [`SheetMetalModel`] from an already-modelled solid.
//!
//! The rest of this crate authors sheet metal: you start from a base flange
//! and add flanges, and the flat pattern falls out of the panel/bend graph.
//! This module runs the arrow the other way — it takes the **triangle mesh
//! of a solid that was modelled some other way** (extruded sketch, boolean,
//! imported STEP) and reconstructs the panel/bend graph, so the existing
//! [`unfold`](crate::unfold) → [`silhouette`](crate::silhouette) →
//! [`dxf`](crate::dxf) pipeline can emit a fab-ready flat pattern for it.
//!
//! This is the mechanical counterpart of `board_from_solid`: solid in,
//! manufacturable 2D out, no re-authoring in a second representation.
//!
//! # How recognition works
//!
//! 1. **Planar clustering.** Triangles are grouped into planar, edge-connected
//!    clusters (normal within `plane_angle_tol`, offset within
//!    `plane_offset_tol`).
//! 2. **Panel pairing.** A panel is a pair of antiparallel clusters that
//!    overlap in projection and are separated by the *same* distance as every
//!    other pair — that common separation is the material thickness. Picking
//!    the **smallest** consistent separation is what stops a 100×50×5 plate
//!    from being read as a 50 mm-thick part standing on edge.
//! 3. **Bend recognition.** For each pair of non-parallel panels, the bend
//!    axis is `nP × nC`; the connecting band is the edge-connected run of
//!    unassigned triangles whose normals are perpendicular to that axis
//!    (which excludes the flat side wall of an L-bracket — its normal is
//!    *along* the axis). The band's inner and outer cylindrical surfaces are
//!    separate components; `area = θ·R·W` recovers each radius.
//! 4. **Graph assembly.** Panels + bends become a [`SheetMetalModel`] whose
//!    frames are rebuilt with the same [`crate::unfold::refold`] math used by
//!    authored parts, so unfold/refold stay exact inverses.
//! 5. **Volume verification.** `Σ panel_area·t + Σ bend_volume` is compared
//!    against the mesh's own signed volume. A part that isn't really constant
//!    thickness fails here rather than silently emitting a wrong outline.
//!
//! Every step fails loudly with a typed [`FlattenError`]; nothing is guessed.

use crate::bend_table::{BendTable, KFactorSource};
use crate::model::{Bend, BendDirection, Frame, Panel, SheetMetalModel};
use crate::poly2d::signed_area_f;
use crate::unfold::{unfold, UnfoldError};
use std::collections::HashMap;
use vcad_kernel_math::{Point2, Point3, Vec3};

/// Borrowed triangle mesh: `positions` is `[x, y, z, ...]`, `indices` is
/// `[i0, i1, i2, ...]` with counter-clockwise winding seen from outside.
#[derive(Debug, Clone, Copy)]
pub struct MeshView<'a> {
    /// Flat `xyz` vertex coordinates (mm).
    pub positions: &'a [f64],
    /// Triangle vertex indices into `positions / 3`.
    pub indices: &'a [u32],
}

/// Tuning for [`flatten_solid`]. [`Default`] is tuned for kernel-tessellated
/// millimetre parts.
#[derive(Debug, Clone)]
pub struct FlattenOptions {
    /// Max angle between two triangle normals in the same planar cluster (deg).
    pub plane_angle_tol_deg: f64,
    /// Max plane-offset difference within a planar cluster (mm).
    pub plane_offset_tol: f64,
    /// Relative tolerance on "every face pair has the same separation".
    pub thickness_tol_frac: f64,
    /// Max acceptable relative error between recovered and mesh volume.
    pub volume_tol_frac: f64,
    /// Collinear-vertex removal tolerance for outlines (mm). Tessellated arcs
    /// survive this; only genuinely straight runs collapse.
    pub simplify_tol: f64,
    /// Material key for bend-table K-factor lookup (e.g. `"al-soft"`).
    pub material: String,
    /// Manual K-factor override; skips the table.
    pub manual_k: Option<f64>,
}

impl Default for FlattenOptions {
    fn default() -> Self {
        Self {
            plane_angle_tol_deg: 1.0,
            plane_offset_tol: 0.01,
            thickness_tol_frac: 0.05,
            volume_tol_frac: 0.02,
            simplify_tol: 1.0e-4,
            material: "al-soft".to_string(),
            manual_k: None,
        }
    }
}

/// Why a solid could not be read as sheet metal.
#[derive(Debug, Clone, PartialEq)]
pub enum FlattenError {
    /// The mesh has no triangles, or fewer than 4 vertices.
    EmptyMesh,
    /// No pair of antiparallel, overlapping planar faces — the part has no
    /// constant-thickness wall anywhere.
    NoFacePairs,
    /// More than one panel was found but they are not all connected by bends.
    DisconnectedPanels {
        /// Number of panels recovered.
        panels: usize,
        /// Number of bends recovered.
        bends: usize,
    },
    /// The panel/bend graph has a cycle (closed tube — not unfoldable here).
    CyclicPanels,
    /// Recovered geometry does not account for the solid's volume: the part
    /// is not really prismatic / constant thickness.
    VolumeMismatch {
        /// Volume of the input mesh (mm³).
        mesh: f64,
        /// Volume implied by the recovered panels + bends (mm³).
        recovered: f64,
        /// Relative error, `|recovered - mesh| / mesh`.
        error_frac: f64,
    },
    /// A bend was detected between two panels but its geometry is degenerate.
    BadBend(String),
    /// Unfold of the recovered model failed.
    Unfold(UnfoldError),
}

impl std::fmt::Display for FlattenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlattenError::EmptyMesh => write!(f, "mesh is empty"),
            FlattenError::NoFacePairs => write!(
                f,
                "no constant-thickness wall found: the part has no pair of parallel, \
                 overlapping faces, so it is not sheet metal"
            ),
            FlattenError::DisconnectedPanels { panels, bends } => write!(
                f,
                "{panels} panels but only {bends} bends connecting them — the part is \
                 not a single unfoldable sheet"
            ),
            FlattenError::CyclicPanels => {
                write!(
                    f,
                    "panel graph has a cycle (closed section); not unfoldable"
                )
            }
            FlattenError::VolumeMismatch {
                mesh,
                recovered,
                error_frac,
            } => write!(
                f,
                "recovered flat pattern accounts for {recovered:.1}mm³ of the solid's \
                 {mesh:.1}mm³ ({:.1}% off) — the part is not constant-thickness sheet",
                error_frac * 100.0
            ),
            FlattenError::BadBend(why) => write!(f, "bend recognition failed: {why}"),
            FlattenError::Unfold(e) => write!(f, "unfold failed: {e}"),
        }
    }
}

impl std::error::Error for FlattenError {}

/// Per-panel summary of what was recovered.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PanelReport {
    /// Panel id inside the recovered model.
    pub panel: usize,
    /// Face area (mm²), holes excluded.
    pub area_mm2: f64,
    /// Number of interior hole loops.
    pub holes: usize,
    /// Outward normal of the panel in the original solid's frame.
    pub normal: [f64; 3],
}

/// Per-bend summary of what was recovered.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BendReport {
    /// Bend id inside the recovered model.
    pub bend: usize,
    /// Parent panel id.
    pub parent: usize,
    /// Child panel id.
    pub child: usize,
    /// Bend angle (degrees).
    pub angle_deg: f64,
    /// Inside bend radius (mm).
    pub radius: f64,
    /// Bend length along the hinge (mm).
    pub length: f64,
    /// `"up"` or `"down"`.
    pub direction: &'static str,
    /// K-factor applied.
    pub k_factor: f64,
}

/// A sheet-metal model recovered from a solid, plus the evidence for it.
#[derive(Debug, Clone)]
pub struct RecoveredSheet {
    /// The reconstructed panel/bend graph, already unfolded.
    pub model: SheetMetalModel,
    /// Detected material thickness (mm).
    pub thickness: f64,
    /// Per-panel evidence.
    pub panels: Vec<PanelReport>,
    /// Per-bend evidence.
    pub bends: Vec<BendReport>,
    /// Signed volume of the input mesh (mm³).
    pub mesh_volume: f64,
    /// Volume implied by the recovered panels + bends (mm³).
    pub recovered_volume: f64,
    /// Non-fatal observations (chamfered edges, dropped slivers, …).
    pub warnings: Vec<String>,
}

impl RecoveredSheet {
    /// Relative volume error of the round-trip check.
    pub fn volume_error_frac(&self) -> f64 {
        if self.mesh_volume.abs() < 1e-12 {
            return 0.0;
        }
        (self.recovered_volume - self.mesh_volume).abs() / self.mesh_volume.abs()
    }
}

// ───────────────────────── internal geometry ──────────────────────────

/// Vertex-weld cell size (mm). Coarse enough to absorb f32 round-tripping of
/// kernel meshes, far finer than any manufacturing tolerance.
const WELD_GRID: f64 = 1.0e-4;

/// K-factor used when the bend table has no row for the recovered
/// (material, thickness, radius). Mid-range for sheet alloys; always labelled
/// as a fallback in the bend's provenance so a shop can override it.
const FALLBACK_K: f64 = 0.44;

struct Tri {
    v: [usize; 3],
    n: Vec3,
    area: f64,
    d: f64,
}

struct Cluster {
    n: Vec3,
    d: f64,
    area: f64,
    tris: Vec<usize>,
}

/// A panel candidate: a pair of antiparallel clusters.
struct FacePair {
    outer: usize,
    inner: usize,
    sep: f64,
}

fn weld(mesh: MeshView<'_>) -> (Vec<Point3>, Vec<[usize; 3]>) {
    let mut map: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut verts: Vec<Point3> = Vec::new();
    let mut remap = vec![usize::MAX; mesh.positions.len() / 3];
    for (i, slot) in remap.iter_mut().enumerate() {
        let p = Point3::new(
            mesh.positions[i * 3],
            mesh.positions[i * 3 + 1],
            mesh.positions[i * 3 + 2],
        );
        let key = (
            (p.x / WELD_GRID).round() as i64,
            (p.y / WELD_GRID).round() as i64,
            (p.z / WELD_GRID).round() as i64,
        );
        // Probe the neighbouring cells too. Meshes reach us as f32, so the
        // same corner emitted by two faces can differ in the last bit and
        // land either side of a cell boundary; an unwelded corner breaks the
        // boundary-loop walk that every outline depends on.
        let mut hit = None;
        'probe: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(&v) = map.get(&(key.0 + dx, key.1 + dy, key.2 + dz)) {
                        if (verts[v] - p).norm() <= WELD_GRID {
                            hit = Some(v);
                            break 'probe;
                        }
                    }
                }
            }
        }
        let id = hit.unwrap_or_else(|| {
            verts.push(p);
            verts.len() - 1
        });
        map.entry(key).or_insert(id);
        *slot = id;
    }
    let mut tris = Vec::with_capacity(mesh.indices.len() / 3);
    for t in mesh.indices.as_chunks::<3>().0 {
        let a = remap[t[0] as usize];
        let b = remap[t[1] as usize];
        let c = remap[t[2] as usize];
        if a != b && b != c && a != c {
            tris.push([a, b, c]);
        }
    }
    (verts, tris)
}

fn mesh_volume(verts: &[Point3], tris: &[[usize; 3]]) -> f64 {
    let mut v = 0.0;
    for t in tris {
        let a = verts[t[0]];
        let b = verts[t[1]];
        let c = verts[t[2]];
        v += (a.x * (b.y * c.z - c.y * b.z) - a.y * (b.x * c.z - c.x * b.z)
            + a.z * (b.x * c.y - c.x * b.y))
            / 6.0;
    }
    v
}

/// Undirected-edge → incident triangle ids.
fn edge_map(tris: &[[usize; 3]]) -> HashMap<(usize, usize), Vec<usize>> {
    let mut m: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            m.entry(if a < b { (a, b) } else { (b, a) })
                .or_default()
                .push(ti);
        }
    }
    m
}

fn build_tris(verts: &[Point3], tris: &[[usize; 3]]) -> Vec<Tri> {
    tris.iter()
        .map(|t| {
            let a = verts[t[0]];
            let b = verts[t[1]];
            let c = verts[t[2]];
            let cross = (b - a).cross(c - a);
            let len = cross.norm();
            let n = if len > 1e-18 {
                cross / len
            } else {
                Vec3::new(0.0, 0.0, 1.0)
            };
            Tri {
                v: *t,
                n,
                area: len * 0.5,
                d: n.dot(a - Point3::origin()),
            }
        })
        .collect()
}

/// Edge-connected planar clusters. Two triangles join only if they share an
/// edge *and* agree on the plane, so coplanar-but-disjoint faces stay apart.
fn cluster_planes(
    tri: &[Tri],
    edges: &HashMap<(usize, usize), Vec<usize>>,
    opts: &FlattenOptions,
) -> (Vec<Cluster>, Vec<usize>) {
    let cos_tol = opts.plane_angle_tol_deg.to_radians().cos();
    let mut owner = vec![usize::MAX; tri.len()];
    let mut clusters: Vec<Cluster> = Vec::new();
    // Seed from the largest triangles so cluster planes are well conditioned.
    let mut order: Vec<usize> = (0..tri.len()).collect();
    order.sort_by(|&a, &b| tri[b].area.total_cmp(&tri[a].area));
    for &seed in &order {
        if owner[seed] != usize::MAX {
            continue;
        }
        let cid = clusters.len();
        let mut cl = Cluster {
            n: tri[seed].n,
            d: tri[seed].d,
            area: 0.0,
            tris: Vec::new(),
        };
        let mut stack = vec![seed];
        owner[seed] = cid;
        while let Some(t) = stack.pop() {
            cl.area += tri[t].area;
            cl.tris.push(t);
            for k in 0..3 {
                let (a, b) = (tri[t].v[k], tri[t].v[(k + 1) % 3]);
                let key = if a < b { (a, b) } else { (b, a) };
                for &nb in edges.get(&key).into_iter().flatten() {
                    if owner[nb] != usize::MAX {
                        continue;
                    }
                    if tri[nb].n.dot(cl.n) < cos_tol {
                        continue;
                    }
                    if (tri[nb].d - cl.d).abs() > opts.plane_offset_tol {
                        continue;
                    }
                    owner[nb] = cid;
                    stack.push(nb);
                }
            }
        }
        clusters.push(cl);
    }
    (clusters, owner)
}

/// Boundary loops of a triangle set, as vertex-index rings. A directed edge
/// with no opposite twin inside the set is on the boundary.
fn boundary_loops(tri: &[Tri], set: &[usize]) -> Vec<Vec<usize>> {
    let mut dir: HashMap<(usize, usize), usize> = HashMap::new();
    for &t in set {
        for k in 0..3 {
            *dir.entry((tri[t].v[k], tri[t].v[(k + 1) % 3])).or_insert(0) += 1;
        }
    }
    let mut next: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(a, b) in dir.keys() {
        if !dir.contains_key(&(b, a)) {
            next.entry(a).or_default().push(b);
        }
    }
    let mut loops = Vec::new();
    while let Some((&start, _)) = next.iter().next() {
        let mut ring = vec![start];
        let mut cur = start;
        while let Some(outs) = next.get_mut(&cur) {
            let Some(nxt) = outs.pop() else { break };
            if outs.is_empty() {
                next.remove(&cur);
            }
            if nxt == start {
                break;
            }
            ring.push(nxt);
            cur = nxt;
            if ring.len() > 4_000_000 {
                break;
            }
        }
        if ring.len() >= 3 {
            loops.push(ring);
        }
    }
    loops
}

/// Orthonormal in-plane axes for a normal.
fn plane_axes(n: Vec3) -> (Vec3, Vec3) {
    let helper = if n.x.abs() < 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let x = n.cross(helper);
    let x = x / x.norm();
    let y = n.cross(x);
    (x, y)
}

fn drop_collinear(ring: &[Point2], tol: f64) -> Vec<Point2> {
    if ring.len() < 3 {
        return ring.to_vec();
    }
    let mut out: Vec<Point2> = Vec::with_capacity(ring.len());
    for i in 0..ring.len() {
        let prev = *out
            .last()
            .unwrap_or(&ring[(i + ring.len() - 1) % ring.len()]);
        let cur = ring[i];
        let nxt = ring[(i + 1) % ring.len()];
        let a = cur - prev;
        let b = nxt - cur;
        let cross = a.x * b.y - a.y * b.x;
        let base = b.norm().max(1e-12);
        if (cross / base).abs() > tol {
            out.push(cur);
        }
    }
    if out.len() < 3 {
        ring.to_vec()
    } else {
        out
    }
}

// ───────────────────────────── main entry ─────────────────────────────

/// Recover a sheet-metal model (panels + bends + flat pattern) from a solid's
/// triangle mesh.
///
/// Returns [`FlattenError`] rather than a best guess whenever the solid isn't
/// constant-thickness sheet — see the module docs for the recognition steps.
pub fn flatten_solid(
    mesh: MeshView<'_>,
    table: &BendTable,
    opts: &FlattenOptions,
) -> Result<RecoveredSheet, FlattenError> {
    let (verts, faces) = weld(mesh);
    if faces.is_empty() || verts.len() < 4 {
        return Err(FlattenError::EmptyMesh);
    }
    let volume = mesh_volume(&verts, &faces).abs();
    let tri = build_tris(&verts, &faces);
    let edges = edge_map(&faces);
    let (clusters, owner) = cluster_planes(&tri, &edges, opts);

    let total_area: f64 = tri.iter().map(|t| t.area).sum();
    let min_face_area = total_area * 1.0e-4;

    // ── panel pairing ────────────────────────────────────────────────
    // A tessellated bend is a fan of narrow planar facets, and the facet at
    // angle θ on the outside pairs with the facet at θ on the inside across
    // exactly the material thickness — indistinguishable from a panel pair by
    // separation alone. What tells them apart is that a facet always has a
    // *smoothly adjacent* neighbour at least as large as itself, while a real
    // panel is the local area maximum of its smooth run. That test is
    // scale-free: it holds for a 24-facet fillet and an 8-facet one alike.
    let smooth = smooth_neighbours(&clusters, &owner, &edges);
    let biggest = clusters.iter().map(|c| c.area).fold(0.0, f64::max);
    let big: Vec<usize> = (0..clusters.len())
        .filter(|&c| clusters[c].area >= min_face_area)
        .filter(|&c| clusters[c].area >= biggest * 0.02)
        .filter(|&c| {
            smooth[c]
                .iter()
                .all(|&nb| clusters[nb].area < clusters[c].area)
        })
        .collect();
    let mut pairs: Vec<FacePair> = Vec::new();
    let cos_tol = opts.plane_angle_tol_deg.to_radians().cos();
    for (ai, &a) in big.iter().enumerate() {
        for &b in &big[ai + 1..] {
            let (ca, cb) = (&clusters[a], &clusters[b]);
            if ca.n.dot(cb.n) > -cos_tol {
                continue;
            }
            let sep = ca.d + cb.d;
            if sep <= 0.0 {
                continue;
            }
            // Overlap test: the smaller face's centroid must project inside
            // the larger one's bbox in the shared plane frame.
            if !faces_overlap(&tri, &verts, ca, cb) {
                continue;
            }
            let (outer, inner) = if ca.area >= cb.area { (a, b) } else { (b, a) };
            pairs.push(FacePair { outer, inner, sep });
        }
    }
    if pairs.is_empty() {
        return Err(FlattenError::NoFacePairs);
    }
    // The material thickness is the *smallest* wall separation: a plate's
    // length and width also pair up their side walls, and those pairings must
    // lose.
    let thickness = pairs
        .iter()
        .map(|p| p.sep)
        .fold(f64::INFINITY, |m, s| m.min(s));
    let tol = (thickness * opts.thickness_tol_frac).max(opts.plane_offset_tol * 2.0);
    // Largest faces first, so which pair wins when a cluster could pair two
    // ways is a property of the geometry, not of cluster iteration order.
    pairs.sort_by(|a, b| {
        clusters[b.outer]
            .area
            .total_cmp(&clusters[a.outer].area)
            .then(a.outer.cmp(&b.outer))
    });
    let mut kept: Vec<FacePair> = Vec::new();
    let mut used: Vec<usize> = Vec::new();
    for p in pairs {
        if (p.sep - thickness).abs() > tol {
            continue;
        }
        if used.contains(&p.outer) || used.contains(&p.inner) {
            continue;
        }
        used.push(p.outer);
        used.push(p.inner);
        kept.push(p);
    }
    if kept.is_empty() {
        return Err(FlattenError::NoFacePairs);
    }
    // A part with two different wall thicknesses is NOT rejected here: on a
    // plain plate the side walls also pair up (at the part's own length and
    // width), so "some other separation exists" says nothing. Walls at the
    // wrong thickness simply fail to become panels, and the volume
    // round-trip below is what turns that into a loud failure.

    // cluster id → panel index
    let mut panel_of_cluster: HashMap<usize, usize> = HashMap::new();
    for (i, p) in kept.iter().enumerate() {
        panel_of_cluster.insert(p.outer, i);
        panel_of_cluster.insert(p.inner, i);
    }

    // ── panel geometry (world-space, on the mid-plane) ───────────────
    let mut warnings: Vec<String> = Vec::new();
    let mut geo: Vec<PanelGeo> = Vec::new();
    for p in &kept {
        let cl = &clusters[p.outer];
        let g = panel_geometry(&tri, &verts, cl, thickness, opts);
        let other = &clusters[p.inner];
        if (cl.area - other.area).abs() > 0.05 * cl.area {
            warnings.push(format!(
                "panel {} faces differ in area by {:.1}% (chamfer or draft?) — the larger \
                 face was used as the cut profile",
                geo.len(),
                100.0 * (cl.area - other.area).abs() / cl.area
            ));
        }
        geo.push(g);
    }

    // ── bend recognition ─────────────────────────────────────────────
    let mut found: Vec<RawBend> = Vec::new();
    for i in 0..geo.len() {
        for j in i + 1..geo.len() {
            if let Some(b) = detect_bend(
                &tri,
                &verts,
                &edges,
                &clusters,
                &owner,
                &panel_of_cluster,
                &kept,
                &geo,
                i,
                j,
                thickness,
            )? {
                found.push(b);
            }
        }
    }
    if geo.len() > 1 && found.len() + 1 != geo.len() {
        if found.len() >= geo.len() {
            return Err(FlattenError::CyclicPanels);
        }
        return Err(FlattenError::DisconnectedPanels {
            panels: geo.len(),
            bends: found.len(),
        });
    }

    // ── assemble the model ───────────────────────────────────────────
    let root = (0..geo.len())
        .max_by(|&a, &b| geo[a].area.total_cmp(&geo[b].area))
        .unwrap_or(0);
    let model = assemble(&geo, &found, &kept, thickness, root, table, opts)?;

    // ── round-trip: does the recovered sheet account for the solid? ──
    let mut recovered: f64 = geo.iter().map(|g| g.area * thickness).sum();
    for b in &found {
        let ro = b.radius + thickness;
        recovered += 0.5 * b.angle * (ro * ro - b.radius * b.radius) * b.length;
    }
    let err = if volume.abs() < 1e-12 {
        0.0
    } else {
        (recovered - volume).abs() / volume
    };
    if err > opts.volume_tol_frac {
        return Err(FlattenError::VolumeMismatch {
            mesh: volume,
            recovered,
            error_frac: err,
        });
    }

    let panels = geo
        .iter()
        .enumerate()
        .map(|(i, g)| PanelReport {
            panel: i,
            area_mm2: g.area,
            holes: g.holes.len(),
            normal: [g.normal.x, g.normal.y, g.normal.z],
        })
        .collect();
    let bends = model
        .bends
        .iter()
        .enumerate()
        .map(|(i, b)| BendReport {
            bend: i,
            parent: b.parent,
            child: b.child,
            angle_deg: b.angle.to_degrees(),
            radius: b.radius,
            length: (b.edge_parent.1 - b.edge_parent.0).norm(),
            direction: match b.direction {
                BendDirection::Up => "up",
                BendDirection::Down => "down",
            },
            k_factor: b.k_factor,
        })
        .collect();

    Ok(RecoveredSheet {
        model,
        thickness,
        panels,
        bends,
        mesh_volume: volume,
        recovered_volume: recovered,
        warnings,
    })
}

/// Max angle between two clusters that still counts as the same smooth
/// (curved) surface run. Wider than any tessellation step of a real fillet,
/// narrower than any manufacturable bend.
const SMOOTH_TOL_DEG: f64 = 25.0;

/// For each cluster, the clusters it meets across an edge at less than
/// [`SMOOTH_TOL_DEG`] — i.e. its neighbours on the same curved surface.
fn smooth_neighbours(
    clusters: &[Cluster],
    owner: &[usize],
    edges: &HashMap<(usize, usize), Vec<usize>>,
) -> Vec<Vec<usize>> {
    let cos_tol = SMOOTH_TOL_DEG.to_radians().cos();
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); clusters.len()];
    for ts in edges.values() {
        for (a, b) in ts.iter().zip(ts.iter().skip(1)) {
            let (ca, cb) = (owner[*a], owner[*b]);
            if ca == cb {
                continue;
            }
            if clusters[ca].n.dot(clusters[cb].n) < cos_tol {
                continue;
            }
            if !out[ca].contains(&cb) {
                out[ca].push(cb);
            }
            if !out[cb].contains(&ca) {
                out[cb].push(ca);
            }
        }
    }
    out
}

/// Do two antiparallel clusters overlap when projected onto their shared
/// plane? Cheap bbox test in the plane frame — enough to reject the top face
/// of one boss pairing with the bottom face of another across the part.
fn faces_overlap(tri: &[Tri], verts: &[Point3], a: &Cluster, b: &Cluster) -> bool {
    let (ex, ey) = plane_axes(a.n);
    let bbox = |cl: &Cluster| {
        let mut lo = (f64::INFINITY, f64::INFINITY);
        let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &t in &cl.tris {
            for v in tri[t].v {
                let r = verts[v] - Point3::origin();
                let (u, w) = (r.dot(ex), r.dot(ey));
                lo = (lo.0.min(u), lo.1.min(w));
                hi = (hi.0.max(u), hi.1.max(w));
            }
        }
        (lo, hi)
    };
    let (alo, ahi) = bbox(a);
    let (blo, bhi) = bbox(b);
    alo.0 <= bhi.0 && blo.0 <= ahi.0 && alo.1 <= bhi.1 && blo.1 <= ahi.1
}

/// World-space panel geometry, expressed on the mid-plane.
struct PanelGeo {
    normal: Vec3,
    /// A point on the panel's mid-plane.
    origin: Point3,
    ex: Vec3,
    ey: Vec3,
    outline: Vec<Point2>,
    holes: Vec<Vec<Point2>>,
    area: f64,
    centroid: Point2,
}

impl PanelGeo {
    fn to_local(&self, p: Point3) -> Point2 {
        let r = p - self.origin;
        Point2::new(r.dot(self.ex), r.dot(self.ey))
    }
    fn to_world(&self, p: Point2) -> Point3 {
        Point3::new(
            self.origin.x + self.ex.x * p.x + self.ey.x * p.y,
            self.origin.y + self.ex.y * p.x + self.ey.y * p.y,
            self.origin.z + self.ex.z * p.x + self.ey.z * p.y,
        )
    }
}

fn panel_geometry(
    tri: &[Tri],
    verts: &[Point3],
    cl: &Cluster,
    thickness: f64,
    opts: &FlattenOptions,
) -> PanelGeo {
    let n = cl.n;
    let (ex, ey) = plane_axes(n);
    // Mid-plane origin: outer face pushed half a thickness inward.
    let origin = Point3::origin() + n * (cl.d - thickness * 0.5);
    let mut g = PanelGeo {
        normal: n,
        origin,
        ex,
        ey,
        outline: Vec::new(),
        holes: Vec::new(),
        area: 0.0,
        centroid: Point2::new(0.0, 0.0),
    };
    let mut rings: Vec<(Vec<Point2>, f64)> = boundary_loops(tri, &cl.tris)
        .into_iter()
        .map(|ring| {
            let pts: Vec<Point2> = ring.iter().map(|&v| g.to_local(verts[v])).collect();
            let pts = drop_collinear(&pts, opts.simplify_tol);
            let a = signed_area_f(&pts);
            (pts, a)
        })
        .filter(|(p, a)| p.len() >= 3 && a.abs() > 1e-9)
        .collect();
    rings.sort_by(|a, b| b.1.abs().total_cmp(&a.1.abs()));
    if let Some((outer, area)) = rings.first().cloned() {
        let outer = if area < 0.0 {
            outer.iter().rev().copied().collect()
        } else {
            outer
        };
        g.area = area.abs();
        for (ring, a) in rings.iter().skip(1) {
            // Holes wind CW.
            let hole: Vec<Point2> = if *a > 0.0 {
                ring.iter().rev().copied().collect()
            } else {
                ring.clone()
            };
            g.area -= a.abs();
            g.holes.push(hole);
        }
        let mut cx = 0.0;
        let mut cy = 0.0;
        for p in &outer {
            cx += p.x;
            cy += p.y;
        }
        g.centroid = Point2::new(cx / outer.len() as f64, cy / outer.len() as f64);
        g.outline = outer;
    }
    g
}

struct RawBend {
    parent: usize,
    child: usize,
    angle: f64,
    radius: f64,
    length: f64,
    /// Tangent (crease) line on panel `parent`, world space.
    hinge_parent: (Point3, Point3),
    /// Tangent (crease) line on panel `child`, world space.
    hinge_child: (Point3, Point3),
}

#[allow(clippy::too_many_arguments)]
fn detect_bend(
    tri: &[Tri],
    verts: &[Point3],
    edges: &HashMap<(usize, usize), Vec<usize>>,
    clusters: &[Cluster],
    owner: &[usize],
    panel_of_cluster: &HashMap<usize, usize>,
    kept: &[FacePair],
    geo: &[PanelGeo],
    i: usize,
    j: usize,
    thickness: f64,
) -> Result<Option<RawBend>, FlattenError> {
    let (ni, nj) = (geo[i].normal, geo[j].normal);
    let dot = ni.dot(nj).clamp(-1.0, 1.0);
    let angle = dot.acos();
    if !(0.02..=std::f64::consts::PI - 0.02).contains(&angle) {
        return Ok(None);
    }
    let axis = ni.cross(nj);
    let al = axis.norm();
    if al < 1e-9 {
        return Ok(None);
    }
    let axis = axis / al;

    // Band triangles: unassigned to any panel face, normal perpendicular to
    // the bend axis. The flat side wall of an L-bracket has its normal *along*
    // the axis and is excluded here — that is the whole point of the filter.
    let is_panel_face = |t: usize| panel_of_cluster.contains_key(&owner[t]);
    let in_band = |t: usize| !is_panel_face(t) && tri[t].n.dot(axis).abs() < 0.09;

    let clusters_of = |p: usize| (kept[p].outer, kept[p].inner);
    let (i_out, i_in) = clusters_of(i);
    let (j_out, j_in) = clusters_of(j);

    // Seed from triangles sharing an edge with panel i's faces.
    let mut seeds: Vec<usize> = Vec::new();
    for &c in &[i_out, i_in] {
        for &t in &clusters[c].tris {
            for k in 0..3 {
                let (a, b) = (tri[t].v[k], tri[t].v[(k + 1) % 3]);
                let key = if a < b { (a, b) } else { (b, a) };
                for &nb in edges.get(&key).into_iter().flatten() {
                    if in_band(nb) {
                        seeds.push(nb);
                    }
                }
            }
        }
    }
    if seeds.is_empty() {
        return Ok(None);
    }

    // Grow each seed's component; keep components that also touch panel j.
    let mut visited: HashMap<usize, usize> = HashMap::new();
    let mut comps: Vec<BandComp> = Vec::new();
    for &s in &seeds {
        if visited.contains_key(&s) {
            continue;
        }
        let cid = comps.len();
        let mut comp = BandComp::default();
        let mut stack = vec![s];
        visited.insert(s, cid);
        while let Some(t) = stack.pop() {
            comp.area += tri[t].area;
            comp.tris.push(t);
            for k in 0..3 {
                let (a, b) = (tri[t].v[k], tri[t].v[(k + 1) % 3]);
                let key = if a < b { (a, b) } else { (b, a) };
                for &nb in edges.get(&key).into_iter().flatten() {
                    if nb == t {
                        continue;
                    }
                    if in_band(nb) {
                        if let std::collections::hash_map::Entry::Vacant(e) = visited.entry(nb) {
                            e.insert(cid);
                            stack.push(nb);
                        }
                    } else {
                        let c = owner[nb];
                        if c == i_out || c == j_out {
                            comp.outer_touch = true;
                        }
                        if c == i_in || c == j_in {
                            comp.inner_touch = true;
                        }
                        if c == i_out || c == i_in {
                            comp.touch_i = true;
                            if c == i_out {
                                comp.hinge_i.push((a, b));
                            }
                        }
                        if c == j_out || c == j_in {
                            comp.touch_j = true;
                            if c == j_out {
                                comp.hinge_j.push((a, b));
                            }
                        }
                    }
                }
            }
        }
        comps.push(comp);
    }
    let bridging: Vec<&BandComp> = comps.iter().filter(|c| c.touch_i && c.touch_j).collect();
    if bridging.is_empty() {
        return Ok(None);
    }

    // Hinge: the chains of edges the band shares with each panel's outer
    // face — the two tangent lines where the flats meet the arc.
    let chain_i: Vec<(usize, usize)> = bridging
        .iter()
        .flat_map(|c| c.hinge_i.iter().copied())
        .collect();
    let chain_j: Vec<(usize, usize)> = bridging
        .iter()
        .flat_map(|c| c.hinge_j.iter().copied())
        .collect();
    if chain_i.is_empty() || chain_j.is_empty() {
        return Err(FlattenError::BadBend(format!(
            "panels {i} and {j} are joined by a curved band that does not reach both \
             outer faces"
        )));
    }
    let (h0, h1, length) = hinge_span(verts, &chain_i, axis);
    let (c0, c1, _) = hinge_span(verts, &chain_j, axis);
    if length <= 1e-6 {
        return Err(FlattenError::BadBend(format!(
            "bend between panels {i} and {j} has zero length"
        )));
    }

    // Radius from the arc's own area: A = θ·R·W for each cylindrical surface.
    // The inner surface is the one that touches both panels' inner faces.
    let inner = bridging
        .iter()
        .filter(|c| c.inner_touch && !c.outer_touch)
        .map(|c| c.area)
        .fold(0.0, f64::max);
    let outer = bridging
        .iter()
        .filter(|c| c.outer_touch && !c.inner_touch)
        .map(|c| c.area)
        .fold(0.0, f64::max);
    let radius = if inner > 0.0 {
        inner / (angle * length)
    } else if outer > 0.0 {
        (outer / (angle * length) - thickness).max(0.0)
    } else {
        // A single component touching both faces = a sharp (zero-radius) fold.
        0.0
    };
    if !radius.is_finite() || radius < 0.0 {
        return Err(FlattenError::BadBend(format!(
            "bend between panels {i} and {j} has a non-physical radius"
        )));
    }

    // Project each tangent line onto its own panel's mid-plane so it lands in
    // that panel's 2D frame exactly.
    let onto = |p: Point3, g: &PanelGeo| -> Point3 {
        let r = p - g.origin;
        p - g.normal * r.dot(g.normal)
    };
    Ok(Some(RawBend {
        parent: i,
        child: j,
        angle,
        radius,
        length,
        hinge_parent: (onto(h0, &geo[i]), onto(h1, &geo[i])),
        hinge_child: (onto(c0, &geo[j]), onto(c1, &geo[j])),
    }))
}

#[derive(Default)]
struct BandComp {
    tris: Vec<usize>,
    area: f64,
    touch_i: bool,
    touch_j: bool,
    inner_touch: bool,
    outer_touch: bool,
    hinge_i: Vec<(usize, usize)>,
    hinge_j: Vec<(usize, usize)>,
}

/// Extreme points of the hinge edge chain along the bend axis.
fn hinge_span(verts: &[Point3], edges: &[(usize, usize)], axis: Vec3) -> (Point3, Point3, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut plo = Point3::origin();
    let mut phi = Point3::origin();
    for &(a, b) in edges {
        for v in [a, b] {
            let p = verts[v];
            let s = (p - Point3::origin()).dot(axis);
            if s < lo {
                lo = s;
                plo = p;
            }
            if s > hi {
                hi = s;
                phi = p;
            }
        }
    }
    (plo, phi, hi - lo)
}

/// Turn recovered panels + bends into a [`SheetMetalModel`] whose frames obey
/// the same bend math as an authored part, then unfold it.
fn assemble(
    geo: &[PanelGeo],
    raw: &[RawBend],
    _kept: &[FacePair],
    thickness: f64,
    root: usize,
    table: &BendTable,
    opts: &FlattenOptions,
) -> Result<SheetMetalModel, FlattenError> {
    let mut model = SheetMetalModel::new(thickness);
    model.material = opts.material.clone();
    model.root = root;

    // Panels go in with their measured local outlines; frames are the measured
    // world poses (already consistent with the bends by construction).
    for g in geo {
        let frame = Frame {
            origin: g.origin,
            x_dir: g.ex,
            y_dir: g.ey,
        };
        model.push_panel(Panel {
            outline: g.outline.clone(),
            holes: g.holes.clone(),
            frame_bent: frame,
            frame_flat: frame,
            incident_bends: Vec::new(),
        });
    }

    // Orient the tree away from the root so `parent` really is the upstream
    // panel, then push bends in BFS order.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); geo.len()];
    for (bi, b) in raw.iter().enumerate() {
        adj[b.parent].push(bi);
        adj[b.child].push(bi);
    }
    let mut seen = vec![false; geo.len()];
    let mut queue = vec![root];
    seen[root] = true;
    let mut ordered: Vec<(usize, usize, &RawBend)> = Vec::new();
    while let Some(p) = queue.pop() {
        for &bi in &adj[p] {
            let b = &raw[bi];
            let other = if b.parent == p { b.child } else { b.parent };
            if seen[other] {
                continue;
            }
            seen[other] = true;
            ordered.push((p, other, b));
            queue.push(other);
        }
    }
    if seen.iter().any(|s| !s) {
        return Err(FlattenError::DisconnectedPanels {
            panels: geo.len(),
            bends: raw.len(),
        });
    }

    for (parent, child, b) in ordered {
        let g = &geo[parent];
        let (world_hinge, child_hinge) = if b.parent == parent {
            (b.hinge_parent, b.hinge_child)
        } else {
            (b.hinge_child, b.hinge_parent)
        };
        let mut p0 = g.to_local(world_hinge.0);
        let mut p1 = g.to_local(world_hinge.1);
        // Right-hand rule: (dir.y, -dir.x) must point *away* from the parent's
        // interior, i.e. toward the child.
        let dir = p1 - p0;
        let outward = Point2::new(dir.y, -dir.x) - Point2::new(0.0, 0.0);
        let mid = Point2::new((p0.x + p1.x) * 0.5, (p0.y + p1.y) * 0.5);
        if outward.dot(g.centroid - mid) > 0.0 {
            std::mem::swap(&mut p0, &mut p1);
        }
        // K-factor provenance survives into the crease and the DXF, so a
        // table miss is labelled as the guess it is rather than passed off as
        // a looked-up value.
        let (k_factor, source) = match opts.manual_k {
            Some(k) => (k, KFactorSource::Manual),
            None => table
                .lookup(&opts.material, thickness, b.radius.max(1e-6))
                .unwrap_or_else(|| {
                    (
                        FALLBACK_K,
                        KFactorSource::Measured {
                            note: format!(
                                "fallback K={FALLBACK_K}: no bend-table row for {} at t={thickness:.2} R={:.2}",
                                opts.material, b.radius
                            ),
                        },
                    )
                }),
        };
        // Pick the fold direction — and, if the hinge came out reversed, the
        // hinge orientation — that lands the child's body on the far side of
        // the crease (local +Y), which is where the flange math puts it. At
        // exactly 90° both directions span the same plane and differ only in
        // which way the flange points, so the *normal* cannot decide this;
        // the child's own centroid can.
        let child_centroid = geo[child].to_world(geo[child].centroid);
        let mut best: Option<(f64, BendDirection, (Point2, Point2))> = None;
        for hinge in [(p0, p1), (p1, p0)] {
            for dirn in [BendDirection::Up, BendDirection::Down] {
                let probe = Bend {
                    parent,
                    child,
                    edge_parent: hinge,
                    radius: b.radius,
                    angle: b.angle,
                    direction: dirn,
                    k_factor,
                    k_factor_source: None,
                };
                let f =
                    crate::unfold::child_bent_frame_for(&model.panels[parent].frame_bent, &probe);
                let local = reproject(child_centroid, &f);
                if local.y <= 0.0 {
                    continue;
                }
                // Among the survivors prefer the frame whose plane actually
                // contains the child.
                let score = -(child_centroid - f.origin).dot(f.normal()).abs();
                if best.is_none_or(|(s, _, _)| score > s) {
                    best = Some((score, dirn, hinge));
                }
            }
        }
        let Some((_, direction, (p0, p1))) = best else {
            return Err(FlattenError::BadBend(format!(
                "panel {child} does not sit on either side of its crease with panel {parent}"
            )));
        };
        let bend = Bend {
            parent,
            child,
            edge_parent: (p0, p1),
            radius: b.radius,
            angle: b.angle,
            direction,
            k_factor,
            k_factor_source: Some(source.label()),
        };
        // Re-express the child in the frame the bend implies, so refold is
        // exact rather than approximately right.
        let derived = crate::unfold::child_bent_frame_for(&model.panels[parent].frame_bent, &bend);
        // The child's own tangent line sits a bend-radius away from the
        // parent's crease in the solid; in the panel/bend model the child
        // starts *at* the crease and the allowance strip covers the arc. Slide
        // the child back by exactly that offset so no material is double
        // counted (and none goes missing) in the flat pattern.
        let cg = &geo[child];
        let shift =
            0.5 * (reproject(child_hinge.0, &derived).y + reproject(child_hinge.1, &derived).y);
        let map = |p: Point2| {
            let q = reproject(cg.to_world(p), &derived);
            Point2::new(q.x, q.y - shift)
        };
        let outline: Vec<Point2> = cg.outline.iter().map(|&p| map(p)).collect();
        let holes: Vec<Vec<Point2>> = cg
            .holes
            .iter()
            .map(|h| h.iter().map(|&p| map(p)).collect())
            .collect();
        model.panels[child].outline = outline;
        model.panels[child].holes = holes;
        model.panels[child].frame_bent = derived;
        model.panels[child].frame_flat = derived;
        model.push_bend(bend);
    }

    unfold(&mut model).map_err(FlattenError::Unfold)?;
    Ok(model)
}

fn reproject(p: Point3, frame: &Frame) -> Point2 {
    let r = p - frame.origin;
    Point2::new(r.dot(frame.x_dir), r.dot(frame.y_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bend_table::BendTable;

    /// Axis-aligned box mesh, corner at `origin`, size `s`.
    fn box_mesh(o: [f64; 3], s: [f64; 3]) -> (Vec<f64>, Vec<u32>) {
        let (x0, y0, z0) = (o[0], o[1], o[2]);
        let (x1, y1, z1) = (o[0] + s[0], o[1] + s[1], o[2] + s[2]);
        let p = vec![
            x0, y0, z0, x1, y0, z0, x1, y1, z0, x0, y1, z0, // 0..3 bottom
            x0, y0, z1, x1, y0, z1, x1, y1, z1, x0, y1, z1, // 4..7 top
        ];
        let idx = vec![
            // bottom (-Z)
            0, 2, 1, 0, 3, 2, // top (+Z)
            4, 5, 6, 4, 6, 7, // -Y
            0, 1, 5, 0, 5, 4, // +X
            1, 2, 6, 1, 6, 5, // +Y
            2, 3, 7, 2, 7, 6, // -X
            3, 0, 4, 3, 4, 7,
        ];
        (p, idx)
    }

    #[test]
    fn plate_recovers_profile_and_thickness() {
        let (pos, idx) = box_mesh([0.0, 0.0, 0.0], [100.0, 50.0, 5.0]);
        let r = flatten_solid(
            MeshView {
                positions: &pos,
                indices: &idx,
            },
            &BendTable::builtin(),
            &FlattenOptions::default(),
        )
        .expect("plate should flatten");
        assert!((r.thickness - 5.0).abs() < 1e-6, "t = {}", r.thickness);
        assert_eq!(r.model.panels.len(), 1);
        assert_eq!(r.model.bends.len(), 0);
        assert!((r.panels[0].area_mm2 - 5000.0).abs() < 1e-6);
        assert!(r.volume_error_frac() < 1e-9);
    }

    /// Ear-clip a simple CCW polygon into triangles.
    fn ear_clip(poly: &[[f64; 2]]) -> Vec<[usize; 3]> {
        let mut idx: Vec<usize> = (0..poly.len()).collect();
        let mut out = Vec::new();
        let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
        };
        let mut guard = 0;
        while idx.len() > 3 && guard < 10_000 {
            guard += 1;
            let n = idx.len();
            for k in 0..n {
                let (ia, ib, ic) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
                let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
                if cross(a, b, c) <= 1e-12 {
                    continue;
                }
                let contains = idx.iter().any(|&p| {
                    p != ia
                        && p != ib
                        && p != ic
                        && cross(a, b, poly[p]) >= 0.0
                        && cross(b, c, poly[p]) >= 0.0
                        && cross(c, a, poly[p]) >= 0.0
                });
                if contains {
                    continue;
                }
                out.push([ia, ib, ic]);
                idx.remove(k);
                break;
            }
        }
        if idx.len() == 3 {
            out.push([idx[0], idx[1], idx[2]]);
        }
        out
    }

    /// Extrude a CCW cross-section in the YZ plane along +X into a watertight
    /// mesh (side walls + ear-clipped end caps).
    fn extrude_yz(section: &[[f64; 2]], width: f64) -> (Vec<f64>, Vec<u32>) {
        let n = section.len();
        let mut pos = Vec::with_capacity(n * 6);
        for x in [0.0, width] {
            for p in section {
                pos.extend([x, p[0], p[1]]);
            }
        }
        let mut idx: Vec<u32> = Vec::new();
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b, c, d) = (i as u32, j as u32, (j + n) as u32, (i + n) as u32);
            idx.extend([a, b, c, a, c, d]);
        }
        for t in ear_clip(section) {
            idx.extend([t[0] as u32, t[2] as u32, t[1] as u32]);
            idx.extend([(t[0] + n) as u32, (t[1] + n) as u32, (t[2] + n) as u32]);
        }
        // Orient outward.
        let verts: Vec<Point3> = pos
            .chunks_exact(3)
            .map(|c| Point3::new(c[0], c[1], c[2]))
            .collect();
        let tris: Vec<[usize; 3]> = idx
            .chunks_exact(3)
            .map(|c| [c[0] as usize, c[1] as usize, c[2] as usize])
            .collect();
        if mesh_volume(&verts, &tris) < 0.0 {
            for t in idx.chunks_exact_mut(3) {
                t.swap(1, 2);
            }
        }
        (pos, idx)
    }

    /// L-bracket cross-section: parent leg along +Y (length `a`), child leg
    /// along +Z (length `b`), material `t`, inside radius `r`, arcs faceted.
    fn l_section(a: f64, b: f64, t: f64, r: f64, facets: usize) -> Vec<[f64; 2]> {
        let cy = a - r - t;
        let cz = r + t;
        let mut p = vec![[0.0, 0.0], [cy, 0.0]];
        for k in 1..facets {
            let ang = -std::f64::consts::FRAC_PI_2
                + std::f64::consts::FRAC_PI_2 * (k as f64 / facets as f64);
            p.push([cy + (r + t) * ang.cos(), cz + (r + t) * ang.sin()]);
        }
        p.push([a, cz]);
        p.push([a, t + b]);
        p.push([a - t, t + b]);
        p.push([a - t, cz]);
        for k in 1..facets {
            let ang = -std::f64::consts::FRAC_PI_2 * (k as f64 / facets as f64);
            p.push([cy + r * ang.cos(), cz + r * ang.sin()]);
        }
        p.push([cy, t]);
        p.push([0.0, t]);
        p
    }

    #[test]
    fn l_bracket_recovers_one_bend() {
        let (a, b, t, r, w) = (30.0, 20.0, 2.0, 2.0, 40.0);
        let (pos, idx) = extrude_yz(&l_section(a, b, t, r, 24), w);
        let rec = flatten_solid(
            MeshView {
                positions: &pos,
                indices: &idx,
            },
            &BendTable::builtin(),
            &FlattenOptions::default(),
        )
        .expect("L-bracket should flatten");
        assert!((rec.thickness - t).abs() < 1e-6, "t = {}", rec.thickness);
        assert_eq!(rec.model.panels.len(), 2);
        assert_eq!(rec.model.bends.len(), 1);
        let bend = &rec.bends[0];
        assert!(
            (bend.angle_deg - 90.0).abs() < 0.5,
            "angle = {}",
            bend.angle_deg
        );
        // Faceted arcs read a hair under the true radius.
        assert!((bend.radius - r).abs() < 0.05, "radius = {}", bend.radius);
        assert!((bend.length - w).abs() < 1e-6, "length = {}", bend.length);
        assert!(
            rec.volume_error_frac() < 0.005,
            "{:?}",
            rec.volume_error_frac()
        );

        // Flat length = both leg lengths + the bend allowance.
        let flat = crate::unfold::FlatPattern::from_model(&rec.model);
        let sil = crate::silhouette::silhouette(&flat).expect("single silhouette");
        let (lo, hi) = sil.exterior.iter().fold(
            ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN)),
            |(lo, hi), p| {
                (
                    (lo.0.min(p.x), lo.1.min(p.y)),
                    (hi.0.max(p.x), hi.1.max(p.y)),
                )
            },
        );
        let ba = bend.angle_deg.to_radians() * (bend.radius + bend.k_factor * t);
        let expected = (a - r - t) + (b - r) + ba;
        let long = (hi.0 - lo.0).max(hi.1 - lo.1);
        let short = (hi.0 - lo.0).min(hi.1 - lo.1);
        assert!((short - w).abs() < 0.05, "blank width = {short}");
        assert!(
            (long - expected).abs() < 0.1,
            "blank length {long} vs expected {expected}"
        );
    }

    /// Cross-section of a constant-thickness strip following `path`
    /// (centerline, in the YZ plane) with inside radius `r` at every corner.
    /// Returns a CCW closed polygon.
    fn thick_path(path: &[[f64; 2]], t: f64, r: f64, facets: usize) -> Vec<[f64; 2]> {
        let rho = r + t * 0.5;
        // One offset side, walked forward; the other, walked backward.
        let side = |s: f64| -> Vec<[f64; 2]> {
            let mut out: Vec<[f64; 2]> = Vec::new();
            let off = |p: [f64; 2], d: [f64; 2]| {
                let n = [-d[1], d[0]]; // left normal
                [p[0] + n[0] * s * t * 0.5, p[1] + n[1] * s * t * 0.5]
            };
            let dir = |a: [f64; 2], b: [f64; 2]| {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l = (dx * dx + dy * dy).sqrt();
                [dx / l, dy / l]
            };
            out.push(off(path[0], dir(path[0], path[1])));
            for k in 1..path.len() - 1 {
                let (d0, d1) = (dir(path[k - 1], path[k]), dir(path[k], path[k + 1]));
                let turn = (d0[0] * d1[1] - d0[1] * d1[0]).atan2(d0[0] * d1[0] + d0[1] * d1[1]);
                let half = turn.abs() * 0.5;
                let tan_len = rho * half.tan();
                // Arc center: offset from the vertex along the inner normal.
                let inner = turn.signum();
                let n0 = [-d0[1] * inner, d0[0] * inner];
                let c = [
                    path[k][0] - n0[0] * 0.0 + (d0[0] * -tan_len) + n0[0] * rho,
                    path[k][1] - n0[1] * 0.0 + (d0[1] * -tan_len) + n0[1] * rho,
                ];
                let radius = rho - s * inner * t * 0.5;
                let start = (
                    off(
                        [path[k][0] - d0[0] * tan_len, path[k][1] - d0[1] * tan_len],
                        d0,
                    ),
                    off(
                        [path[k][0] + d1[0] * tan_len, path[k][1] + d1[1] * tan_len],
                        d1,
                    ),
                );
                let a0 = (start.0[1] - c[1]).atan2(start.0[0] - c[0]);
                let a1 = (start.1[1] - c[1]).atan2(start.1[0] - c[0]);
                let mut sweep = a1 - a0;
                while sweep > std::f64::consts::PI {
                    sweep -= std::f64::consts::TAU;
                }
                while sweep < -std::f64::consts::PI {
                    sweep += std::f64::consts::TAU;
                }
                for f in 0..=facets {
                    let a = a0 + sweep * (f as f64 / facets as f64);
                    out.push([c[0] + radius * a.cos(), c[1] + radius * a.sin()]);
                }
            }
            let n = path.len();
            out.push(off(path[n - 1], dir(path[n - 2], path[n - 1])));
            out
        };
        let mut poly = side(-1.0);
        let mut back = side(1.0);
        back.reverse();
        poly.extend(back);
        poly
    }

    #[test]
    fn u_channel_recovers_two_bends() {
        let (t, r, w) = (1.5, 1.5, 25.0);
        let path = [[0.0, 18.0], [0.0, 0.0], [40.0, 0.0], [40.0, 18.0]];
        let section = thick_path(&path, t, r, 16);
        let (pos, idx) = extrude_yz(&section, w);
        let rec = flatten_solid(
            MeshView {
                positions: &pos,
                indices: &idx,
            },
            &BendTable::builtin(),
            &FlattenOptions::default(),
        )
        .expect("U-channel should flatten");
        assert!((rec.thickness - t).abs() < 1e-6, "t = {}", rec.thickness);
        assert_eq!(rec.model.panels.len(), 3, "base + two upstands");
        assert_eq!(rec.model.bends.len(), 2);
        for b in &rec.bends {
            assert!((b.angle_deg - 90.0).abs() < 0.5, "angle = {}", b.angle_deg);
            assert!((b.radius - r).abs() < 0.05, "radius = {}", b.radius);
            assert!((b.length - w).abs() < 1e-6);
        }
        assert!(
            rec.volume_error_frac() < 0.01,
            "{}",
            rec.volume_error_frac()
        );
        // Both flanges fold the same way, and the blank is one closed piece.
        assert_eq!(rec.bends[0].direction, rec.bends[1].direction);
        let flat = crate::unfold::FlatPattern::from_model(&rec.model);
        crate::silhouette::silhouette(&flat).expect("one silhouette");
    }

    /// Rectangular plate `w × d × t` with a concentric rectangular hole.
    fn plate_with_hole(w: f64, d: f64, t: f64, hw: f64, hd: f64) -> (Vec<f64>, Vec<u32>) {
        let outer = [[0.0, 0.0], [w, 0.0], [w, d], [0.0, d]];
        let (ix, iy) = ((w - hw) * 0.5, (d - hd) * 0.5);
        let inner = [[ix, iy], [ix + hw, iy], [ix + hw, iy + hd], [ix, iy + hd]];
        let mut pos = Vec::new();
        // 0..3 outer bottom, 4..7 outer top, 8..11 inner bottom, 12..15 inner top
        for z in [0.0, t] {
            for p in outer {
                pos.extend([p[0], p[1], z]);
            }
        }
        for z in [0.0, t] {
            for p in inner {
                pos.extend([p[0], p[1], z]);
            }
        }
        let (ob, ot, ib, it) = (0u32, 4u32, 8u32, 12u32);
        let mut idx: Vec<u32> = Vec::new();
        for k in 0..4u32 {
            let n = (k + 1) % 4;
            // top face ring (+Z), bottom face ring (-Z)
            idx.extend([ot + k, ot + n, it + n, ot + k, it + n, it + k]);
            idx.extend([ob + k, ib + n, ob + n, ob + k, ib + k, ib + n]);
            // outer wall, inner wall
            idx.extend([ob + k, ob + n, ot + n, ob + k, ot + n, ot + k]);
            idx.extend([ib + k, it + n, ib + n, ib + k, it + k, it + n]);
        }
        (pos, idx)
    }

    #[test]
    fn plate_hole_becomes_an_interior_loop() {
        let (pos, idx) = plate_with_hole(60.0, 40.0, 3.0, 10.0, 8.0);
        let r = flatten_solid(
            MeshView {
                positions: &pos,
                indices: &idx,
            },
            &BendTable::builtin(),
            &FlattenOptions::default(),
        )
        .expect("plate with a bore should flatten");
        assert_eq!(r.model.panels.len(), 1);
        assert_eq!(
            r.panels[0].holes, 1,
            "the bore should survive as a hole loop"
        );
        assert!((r.panels[0].area_mm2 - (2400.0 - 80.0)).abs() < 1e-6);
        assert!(r.volume_error_frac() < 1e-9);
        let flat = crate::unfold::FlatPattern::from_model(&r.model);
        let dxf = crate::dxf::flat_pattern_to_dxf(&flat).expect("dxf");
        assert!(dxf.contains("CUT"));
        // Outer ring + hole ring.
        assert_eq!(
            dxf.matches("LWPOLYLINE")
                .count()
                .max(dxf.matches("POLYLINE").count()),
            2
        );
    }

    #[test]
    fn faceted_washer_keeps_its_arcs() {
        // The realistic plate: both rings are tessellated arcs, which must
        // survive collinear-removal as arcs rather than collapsing.
        let (ro, ri, t, n) = (30.0, 8.0, 4.0, 48usize);
        let ring = |r: f64, z: f64| -> Vec<f64> {
            (0..n)
                .flat_map(|k| {
                    let a = std::f64::consts::TAU * (k as f64 / n as f64);
                    [r * a.cos(), r * a.sin(), z]
                })
                .collect()
        };
        let mut pos = Vec::new();
        for (r, z) in [(ro, 0.0), (ro, t), (ri, 0.0), (ri, t)] {
            pos.extend(ring(r, z));
        }
        let (ob, ot, ib, it) = (0, n as u32, 2 * n as u32, 3 * n as u32);
        let mut idx: Vec<u32> = Vec::new();
        for k in 0..n as u32 {
            let m = (k + 1) % n as u32;
            idx.extend([ot + k, ot + m, it + m, ot + k, it + m, it + k]);
            idx.extend([ob + k, ib + m, ob + m, ob + k, ib + k, ib + m]);
            idx.extend([ob + k, ob + m, ot + m, ob + k, ot + m, ot + k]);
            idx.extend([ib + k, it + m, ib + m, ib + k, it + k, it + m]);
        }
        let r = flatten_solid(
            MeshView {
                positions: &pos,
                indices: &idx,
            },
            &BendTable::builtin(),
            &FlattenOptions::default(),
        )
        .expect("washer should flatten");
        assert!((r.thickness - t).abs() < 1e-9);
        assert_eq!(r.panels[0].holes, 1);
        // Faceted polygon area, not the analytic circle area.
        let poly =
            |rad: f64| 0.5 * (n as f64) * rad * rad * (std::f64::consts::TAU / n as f64).sin();
        assert!((r.panels[0].area_mm2 - (poly(ro) - poly(ri))).abs() < 1e-6);
        assert!(r.volume_error_frac() < 1e-9);
        // Every facet vertex survives — an arc flattened to a chord would be
        // a part the shop cuts wrong.
        assert_eq!(r.model.panels[0].outline.len(), n);
        assert_eq!(r.model.panels[0].holes[0].len(), n);
    }

    #[test]
    fn drafted_wall_fails_the_volume_check() {
        // Truncated pyramid: top and bottom are parallel and 2mm apart, so
        // pairing "succeeds", but the draft means the profile does not
        // account for the solid — the round-trip check is what catches it.
        let pos = vec![
            0.0, 0.0, 0.0, 50.0, 0.0, 0.0, 50.0, 30.0, 0.0, 0.0, 30.0, 0.0, // bottom 50×30
            5.0, 5.0, 2.0, 45.0, 5.0, 2.0, 45.0, 25.0, 2.0, 5.0, 25.0, 2.0, // top 40×20
        ];
        let idx = vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7,
            6, 3, 0, 4, 3, 4, 7,
        ];
        let e = flatten_solid(
            MeshView {
                positions: &pos,
                indices: &idx,
            },
            &BendTable::builtin(),
            &FlattenOptions::default(),
        );
        assert!(
            matches!(e, Err(FlattenError::VolumeMismatch { .. })),
            "a drafted block is not sheet metal: {e:?}"
        );
    }

    #[test]
    fn non_sheet_solid_is_rejected() {
        // A cube: every pairing has the same separation, so thickness would be
        // 10 and the "flat pattern" would be a lie. The volume check catches
        // it — one 10×10 panel × 10 thick is the whole cube, so this one is
        // actually consistent; use a stepped block instead.
        let (mut pos, mut idx) = box_mesh([0.0, 0.0, 0.0], [50.0, 50.0, 3.0]);
        let (p2, i2) = box_mesh([10.0, 10.0, 3.0], [10.0, 10.0, 20.0]);
        let base = (pos.len() / 3) as u32;
        pos.extend(p2);
        idx.extend(i2.iter().map(|i| i + base));
        let e = flatten_solid(
            MeshView {
                positions: &pos,
                indices: &idx,
            },
            &BendTable::builtin(),
            &FlattenOptions::default(),
        );
        assert!(
            matches!(e, Err(FlattenError::VolumeMismatch { .. })),
            "{e:?}"
        );
    }
}
