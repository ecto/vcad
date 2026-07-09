//! Variable edge blend: a chamfer↔fillet **loft** along a single edge.
//!
//! The cross-section of an edge blend at any point along the edge is a
//! curve between the two tangency points on the adjacent faces: a straight
//! line for a chamfer, a circular arc for a fillet. Because both profiles
//! share the same tangency endpoints, any convex combination of the two is
//! itself a valid profile — so a blend whose *shape* morphs from chamfer
//! (0.0) to fillet (1.0) along the edge, and whose *size* (tangent setback
//! = chamfer leg = fillet radius) varies linearly, is a loft of these
//! interpolated sections.
//!
//! Construction mirrors [`crate::fillet_subset`]: with a linearly varying
//! size, the side-face insets remain straight lines, cap faces get the end
//! profile spliced into their corner, and the blend strip itself is emitted
//! as a grid of planar triangular faces whose vertices are *exactly* the
//! shared section samples — welding via the quantized vertex cache makes
//! the result watertight by construction.
//!
//! Domain: one **plane-plane** manifold edge whose endpoints are each
//! touched only by this blend (cap ends, no miter chains yet).

use std::collections::HashMap;
use vcad_kernel_geom::{GeometryStore, Plane, SurfaceKind};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId, ShellType, Topology, VertexId};

use crate::fillet_subset::{add_face, orient_cap_tangents};
use crate::topology::{extract_edges, extract_faces, pair_twin_half_edges};

/// One end of a lofted edge blend.
#[derive(Debug, Clone, Copy)]
pub struct BlendSection {
    /// Tangent setback along each adjacent face — the chamfer leg length,
    /// or the fillet radius. Must be positive.
    pub size: f64,
    /// Profile shape: `0.0` = flat chamfer, `1.0` = circular fillet.
    /// Values in between blend the two; clamped to `[0, 1]`.
    pub shape: f64,
}

/// Number of samples across the blend profile (tangent-A → tangent-B).
const PROFILE_SEGMENTS: usize = 32;
/// Number of slabs along the edge axis.
const AXIAL_SEGMENTS: usize = 24;

/// Apply a chamfer↔fillet loft to a single plane-plane edge of `brep`.
///
/// `start` applies at the edge's `v_start` endpoint, `end` at `v_end`;
/// size and shape interpolate linearly between them. Errors (with a
/// human-readable reason) when the edge is unsupported, leaving the caller
/// to fall back or surface the message.
pub fn loft_blend_edge(
    brep: &BRepSolid,
    edge_id: EdgeId,
    start: BlendSection,
    end: BlendSection,
) -> Result<BRepSolid, String> {
    let start = BlendSection {
        size: start.size,
        shape: start.shape.clamp(0.0, 1.0),
    };
    let end = BlendSection {
        size: end.size,
        shape: end.shape.clamp(0.0, 1.0),
    };
    if start.size <= 1e-9 || end.size <= 1e-9 {
        return Err("blend size must be positive at both ends".into());
    }

    let edges = extract_edges(brep);
    let faces = extract_faces(brep);
    let topo = &brep.topology;

    let e = edges
        .iter()
        .find(|e| e.edge_id == edge_id)
        .ok_or_else(|| "edge not found or non-manifold".to_string())?;

    for f in [e.face_a, e.face_b] {
        let kind = brep.geometry.surfaces[topo.faces[f].surface_index].surface_type();
        if kind != SurfaceKind::Plane {
            return Err("edge blend loft requires a plane-plane edge".into());
        }
    }

    let face_normal: HashMap<FaceId, Vec3> = faces.iter().map(|f| (f.face_id, f.normal)).collect();
    let n_a = face_normal[&e.face_a];
    let n_b = face_normal[&e.face_b];
    let sin2_half = (1.0 + n_a.dot(n_b)) * 0.5;
    if sin2_half < 1e-9 {
        return Err("degenerate (tangent or reflex) edge".into());
    }

    let p_start = topo.vertices[e.v_start].point;
    let p_end = topo.vertices[e.v_end].point;
    let edge_vec = p_end - p_start;
    let edge_len = edge_vec.norm();
    if edge_len < 1e-12 {
        return Err("zero-length edge".into());
    }
    let axis = edge_vec / edge_len;

    // Per-unit-size offset from the edge line to the section center
    // (the fillet arc's center), and the in-section arc frame.
    let offset_unit = -(n_a + n_b) / (2.0 * sin2_half);
    let u = n_a; // center → tangent-A direction (unit)
    let mut w = axis.cross(u);
    let wn = w.norm();
    if wn < 1e-12 {
        return Err("degenerate section frame".into());
    }
    w = w / wn;
    // Signed arc sweep from tangent-A to tangent-B about the center.
    let sweep = n_b.dot(w).atan2(n_b.dot(u));

    // Sample one profile ring at edge parameter t: PROFILE_SEGMENTS + 1
    // points from the tangency on face A to the tangency on face B.
    let section = |t: f64| -> Vec<Point3> {
        let size = start.size + (end.size - start.size) * t;
        let shape = start.shape + (end.shape - start.shape) * t;
        let center = p_start + edge_vec * t + offset_unit * size;
        let tan_a = center + u * size;
        let tan_b = center + (u * sweep.cos() + w * sweep.sin()) * size;
        (0..=PROFILE_SEGMENTS)
            .map(|i| {
                let s = i as f64 / PROFILE_SEGMENTS as f64;
                let arc_pt = center + (u * (sweep * s).cos() + w * (sweep * s).sin()) * size;
                let line_pt = tan_a + (tan_b - tan_a) * s;
                line_pt + (arc_pt - line_pt) * shape
            })
            .collect()
    };

    // All rings computed once and reused verbatim by side faces, cap faces
    // and the strip, so shared points weld exactly.
    let rings: Vec<Vec<Point3>> = (0..=AXIAL_SEGMENTS)
        .map(|j| section(j as f64 / AXIAL_SEGMENTS as f64))
        .collect();
    let ring_start = &rings[0];
    let ring_end = &rings[AXIAL_SEGMENTS];

    // Both endpoints must be cap ends: check that the blend's end sections
    // stay within the edge (sizes can't consume more than the edge length).
    if start.size + end.size >= 2.0 * edge_len {
        return Err("blend size too large for edge length".into());
    }

    // Tangent line endpoints on each side face.
    let ring_at_vertex = |v: VertexId| -> &Vec<Point3> {
        if v == e.v_start {
            ring_start
        } else {
            ring_end
        }
    };

    let mut new_topo = Topology::new();
    let mut new_geom = GeometryStore::new();
    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();
    let mut all_faces: Vec<FaceId> = Vec::new();

    // 1. Rebuild every original face.
    for face in &faces {
        let n = face.vertex_ids.len();
        let mut loop_positions: Vec<Point3> = Vec::with_capacity(n);

        for i in 0..n {
            let v = face.vertex_ids[i];
            let pos = face.positions[i];
            if v != e.v_start && v != e.v_end {
                loop_positions.push(pos);
                continue;
            }

            let is_side = face.face_id == e.face_a || face.face_id == e.face_b;
            if is_side {
                // Side face: corner moves to its tangency point; when the
                // next loop vertex is the edge's other endpoint we also
                // splice the interior axial samples so the strip's
                // triangles share every vertex on this line (no
                // T-junctions in the tessellation).
                let profile_idx = if face.face_id == e.face_a {
                    0
                } else {
                    PROFILE_SEGMENTS
                };
                loop_positions.push(ring_at_vertex(v)[profile_idx]);
                let next_v = face.vertex_ids[(i + 1) % n];
                if (v == e.v_start && next_v == e.v_end)
                    || (v == e.v_end && next_v == e.v_start)
                {
                    let interior: Box<dyn Iterator<Item = usize>> = if v == e.v_start {
                        Box::new(1..AXIAL_SEGMENTS)
                    } else {
                        Box::new((1..AXIAL_SEGMENTS).rev())
                    };
                    for j in interior {
                        loop_positions.push(rings[j][profile_idx]);
                    }
                }
            } else {
                // Cap face: splice the end profile into this corner,
                // oriented to keep the loop direction.
                let ring = ring_at_vertex(v);
                let prev = face.positions[(i + n - 1) % n];
                let next = face.positions[(i + 1) % n];
                let ta = ring[0];
                let tb = ring[PROFILE_SEGMENTS];
                let (first, _) = orient_cap_tangents(pos, prev, next, ta, tb);
                if (first - ta).norm() < 1e-12 {
                    loop_positions.extend(ring.iter().copied());
                } else {
                    loop_positions.extend(ring.iter().rev().copied());
                }
            }
        }

        if loop_positions.len() < 3 {
            continue;
        }
        let surf_idx =
            new_geom.add_surface(Box::new(Plane::from_normal(loop_positions[0], face.normal)));
        let orientation = topo.faces[face.face_id].orientation;
        add_face(
            &loop_positions,
            surf_idx,
            orientation,
            &mut vertex_cache,
            &mut new_topo,
            &mut all_faces,
        );
    }

    // 2. Emit the blend strip as planar triangles over the sample grid,
    //    wound so the geometric normal points out of the solid (along
    //    n_a + n_b for a convex edge).
    let outward = n_a + n_b;
    for j in 0..AXIAL_SEGMENTS {
        for i in 0..PROFILE_SEGMENTS {
            let quad = [
                rings[j][i],
                rings[j][i + 1],
                rings[j + 1][i + 1],
                rings[j + 1][i],
            ];
            for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                let mut normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
                let area2 = normal.norm();
                if area2 < 1e-14 {
                    continue; // degenerate sliver (e.g. flat-chamfer seam)
                }
                let mut tri = tri;
                if normal.dot(outward) < 0.0 {
                    tri.swap(1, 2);
                    normal = -normal;
                }
                let surf_idx = new_geom
                    .add_surface(Box::new(Plane::from_normal(tri[0], normal / area2)));
                add_face(
                    &tri,
                    surf_idx,
                    vcad_kernel_topo::Orientation::Forward,
                    &mut vertex_cache,
                    &mut new_topo,
                    &mut all_faces,
                );
            }
        }
    }

    pair_twin_half_edges(&mut new_topo);
    let shell = new_topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = new_topo.add_solid(shell);

    Ok(BRepSolid {
        topology: new_topo,
        geometry: new_geom,
        solid_id,
    })
}

/// Find the plane-plane manifold edge whose nearest endpoint is closest to
/// `near`, returning `(edge_id, flip)` — `flip` is true when the *end*
/// vertex is the closer one, i.e. the caller's "start" section should
/// apply at `v_end`.
pub fn find_edge_near(brep: &BRepSolid, near: Point3) -> Option<(EdgeId, bool)> {
    let edges = extract_edges(brep);
    let topo = &brep.topology;
    let mut best: Option<(EdgeId, bool, f64)> = None;
    for e in &edges {
        let planar = [e.face_a, e.face_b].iter().all(|&f| {
            brep.geometry.surfaces[topo.faces[f].surface_index].surface_type()
                == SurfaceKind::Plane
        });
        if !planar {
            continue;
        }
        let d_start = (topo.vertices[e.v_start].point - near).norm();
        let d_end = (topo.vertices[e.v_end].point - near).norm();
        let (d, flip) = if d_start <= d_end {
            (d_start, false)
        } else {
            (d_end, true)
        };
        let better = match best {
            None => true,
            Some((_, _, bd)) => d < bd,
        };
        if better {
            best = Some((e.edge_id, flip, d));
        }
    }
    best.map(|(id, flip, _)| (id, flip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    fn mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
        let verts = &mesh.vertices;
        let mut vol = 0.0;
        for tri in mesh.indices.chunks(3) {
            let p = |k: usize| {
                let i = tri[k] as usize * 3;
                [verts[i] as f64, verts[i + 1] as f64, verts[i + 2] as f64]
            };
            let (v0, v1, v2) = (p(0), p(1), p(2));
            vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        (vol / 6.0).abs()
    }

    fn first_edge(cube: &BRepSolid) -> EdgeId {
        extract_edges(cube)[0].edge_id
    }

    /// Constant pure-chamfer loft matches the single-edge chamfer closed
    /// form V = L³ − s²/2·L for a 90° edge, and is watertight.
    #[test]
    fn test_constant_chamfer_loft() {
        let l = 10.0;
        let s = 1.5;
        let cube = make_cube(l, l, l);
        let sect = BlendSection { size: s, shape: 0.0 };
        let out = loft_blend_edge(&cube, first_edge(&cube), sect, sect).unwrap();
        let mesh = vcad_kernel_tessellate::tessellate_brep(&out, 32);
        assert_eq!(mesh.boundary_edges().len(), 0, "must be watertight");
        let expected = l * l * l - s * s / 2.0 * l;
        let vol = mesh_volume(&mesh);
        assert!(
            (vol - expected).abs() / expected < 1e-6,
            "chamfer loft volume: expected {expected:.6}, got {vol:.6}"
        );
    }

    /// Constant pure-fillet loft matches the single-edge fillet closed
    /// form V = L³ − (1 − π/4)·r²·L within the profile-polygon sampling
    /// error, and is watertight.
    #[test]
    fn test_constant_fillet_loft() {
        let l = 10.0;
        let r = 1.5;
        let cube = make_cube(l, l, l);
        let sect = BlendSection { size: r, shape: 1.0 };
        let out = loft_blend_edge(&cube, first_edge(&cube), sect, sect).unwrap();
        let mesh = vcad_kernel_tessellate::tessellate_brep(&out, 32);
        assert_eq!(mesh.boundary_edges().len(), 0, "must be watertight");
        let pi = std::f64::consts::PI;
        let expected = l * l * l - (1.0 - pi / 4.0) * r * r * l;
        let vol = mesh_volume(&mesh);
        // The strip is a sampled polygon of the arc (32 segments), so the
        // removed sliver is slightly larger than the smooth closed form.
        assert!(
            (vol - expected).abs() / expected < 1e-4,
            "fillet loft volume: expected {expected:.6}, got {vol:.6}"
        );
    }

    /// The tweet: chamfer at one end lofting into a fillet at the other.
    /// Watertight, and volume matches a numeric integral of the exact
    /// section areas (Simpson — the section area is quadratic in t).
    #[test]
    fn test_chamfer_into_fillet_loft() {
        let l = 10.0;
        let s = 2.0;
        let cube = make_cube(l, l, l);
        let start = BlendSection { size: s, shape: 0.0 };
        let end = BlendSection { size: s, shape: 1.0 };
        let out = loft_blend_edge(&cube, first_edge(&cube), start, end).unwrap();
        let mesh = vcad_kernel_tessellate::tessellate_brep(&out, 32);
        assert_eq!(mesh.boundary_edges().len(), 0, "must be watertight");

        // Removed cross-section area at shape f (90° corner, setback s),
        // computed by shoelace over the same profile the builder samples.
        let area = |f: f64| -> f64 {
            let n = 4096;
            // 2D section: corner at the origin, faces along +x and +y,
            // tangencies at (0, s) and (s, 0), fillet center at (s, s).
            let mut pts = Vec::with_capacity(n + 2);
            pts.push([0.0, 0.0]);
            for i in 0..=n {
                let u = i as f64 / n as f64;
                let t = std::f64::consts::FRAC_PI_2 * u;
                let line = [s * u, s * (1.0 - u)];
                let arc = [s - s * t.cos(), s - s * t.sin()];
                pts.push([
                    line[0] + (arc[0] - line[0]) * f,
                    line[1] + (arc[1] - line[1]) * f,
                ]);
            }
            let m = pts.len();
            let mut a2 = 0.0;
            for i in 0..m {
                let p = pts[i];
                let q = pts[(i + 1) % m];
                a2 += p[0] * q[1] - q[0] * p[1];
            }
            (a2 / 2.0).abs()
        };
        // Simpson over t (area is quadratic in the linear shape blend).
        let removed = l / 6.0 * (area(0.0) + 4.0 * area(0.5) + area(1.0));
        let expected = l * l * l - removed;
        let vol = mesh_volume(&mesh);
        assert!(
            (vol - expected).abs() / expected < 1e-4,
            "loft volume: expected {expected:.6}, got {vol:.6}"
        );
        // Sanity: strictly between the pure-fillet and pure-chamfer volumes.
        let v_chamfer = l * l * l - s * s / 2.0 * l;
        let v_fillet = l * l * l - (1.0 - std::f64::consts::FRAC_PI_4) * s * s * l;
        assert!(vol > v_chamfer && vol < v_fillet);
    }

    /// Tapered size: fillet radius shrinking along the edge.
    #[test]
    fn test_tapered_fillet_loft_watertight() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let out = loft_blend_edge(
            &cube,
            first_edge(&cube),
            BlendSection { size: 3.0, shape: 1.0 },
            BlendSection { size: 0.5, shape: 1.0 },
        )
        .unwrap();
        let mesh = vcad_kernel_tessellate::tessellate_brep(&out, 32);
        assert_eq!(mesh.boundary_edges().len(), 0, "must be watertight");
        let vol = mesh_volume(&mesh);
        assert!(vol < 1000.0 && vol > 950.0, "vol = {vol}");
    }

    #[test]
    fn test_rejects_bad_input() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let e = first_edge(&cube);
        let ok = BlendSection { size: 1.0, shape: 0.5 };
        assert!(loft_blend_edge(&cube, e, BlendSection { size: 0.0, shape: 0.0 }, ok).is_err());
        assert!(loft_blend_edge(&cube, e, BlendSection { size: 20.0, shape: 0.0 }, ok).is_err());
    }

    #[test]
    fn test_find_edge_near_picks_closest_endpoint() {
        let cube = make_cube(10.0, 10.0, 10.0);
        let (eid, _flip) = find_edge_near(&cube, Point3::new(0.0, 0.0, 0.0)).unwrap();
        let edges = extract_edges(&cube);
        let e = edges.iter().find(|e| e.edge_id == eid).unwrap();
        let a = cube.topology.vertices[e.v_start].point;
        let b = cube.topology.vertices[e.v_end].point;
        assert!(
            a.to_vec().norm() < 1e-9 || b.to_vec().norm() < 1e-9,
            "picked edge should touch the origin corner"
        );
    }
}
