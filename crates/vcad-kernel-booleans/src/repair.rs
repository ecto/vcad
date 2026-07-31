//! Post-boolean topology repair utilities.
//!
//! Repairs are conservative and operate on topology only. The goal is to
//! heal small gaps and inconsistencies caused by tolerance issues:
//! - collapse zero-length half-edges
//! - remove local A-B-A spikes in loops
//! - pair orphan half-edges into edges when endpoints match

use std::collections::HashMap;

use vcad_kernel_math::Point3;
use vcad_kernel_topo::{HalfEdgeId, Topology};

/// Repair common topology issues in-place.
pub fn repair_topology(topo: &mut Topology, tolerance: f64) {
    collapse_degenerate_half_edges(topo, tolerance);
    cleanup_loop_spikes(topo, tolerance);
    collapse_degenerate_half_edges(topo, tolerance);
    split_edges_at_interior_vertices(topo, tolerance);
    pair_half_edges(topo, tolerance);
    // Coarse split-only round: split-curve discretization can leave one
    // face's seam vertex up to chord-sag off its neighbor's edge (a
    // line×polyline crossing on the cap vs the line×circle point on the
    // wall). Snapping the edge THROUGH the existing vertex (splitting at
    // it) makes both loops share the vertex, so the pieces pair by id.
    // Only still-unpaired half-edges are touched; the distortion is
    // bounded by the tolerance, i.e. tessellation-error scale.
    const SEAM_SNAP_TOL: f64 = 8e-3;
    if tolerance < SEAM_SNAP_TOL && std::env::var("VCAD_NO_WELD").is_err() {
        split_edges_at_interior_vertices(topo, SEAM_SNAP_TOL);
        pair_half_edges(topo, tolerance);
        // Endpoint-vs-endpoint mismatches (e.g. the wall splitter's exact
        // line×circle corner vs the cap splitter's line×chord crossing)
        // can't be fixed by splitting — weld nearby vertices, but ONLY
        // those on still-unpaired (boundary) half-edges, so interior
        // geometry is never disturbed.
        weld_boundary_vertices(topo, SEAM_SNAP_TOL);
        collapse_degenerate_half_edges(topo, tolerance);
        cleanup_loop_spikes(topo, tolerance);
        split_edges_at_interior_vertices(topo, tolerance);
        pair_half_edges(topo, tolerance);
    }
}

/// Weld vertices of unpaired half-edges that lie within `tolerance` of each
/// other: all references are rewritten to one survivor per cluster.
fn weld_boundary_vertices(topo: &mut Topology, tolerance: f64) {
    use vcad_kernel_topo::VertexId;
    // Vertices on unpaired loop half-edges (either endpoint).
    let mut boundary: Vec<(VertexId, Point3)> = Vec::new();
    {
        let mut seen = std::collections::HashSet::new();
        for (he_id, he) in &topo.half_edges {
            if he.twin.is_some() || he.loop_id.is_none() {
                continue;
            }
            let dest = topo.half_edge_dest(he_id);
            for v in [he.origin, dest] {
                if seen.insert(v) {
                    boundary.push((v, topo.vertices[v].point));
                }
            }
        }
    }
    let mut merge_map: HashMap<VertexId, VertexId> = HashMap::new();
    for i in 0..boundary.len() {
        if merge_map.contains_key(&boundary[i].0) {
            continue;
        }
        for j in (i + 1)..boundary.len() {
            if merge_map.contains_key(&boundary[j].0) {
                continue;
            }
            if are_close(&boundary[i].1, &boundary[j].1, tolerance) {
                merge_map.insert(boundary[j].0, boundary[i].0);
            }
        }
    }
    if merge_map.is_empty() {
        return;
    }
    let he_ids: Vec<_> = topo.half_edges.keys().collect();
    for he_id in he_ids {
        let origin = topo.half_edges[he_id].origin;
        if let Some(&target) = merge_map.get(&origin) {
            topo.half_edges[he_id].origin = target;
        }
    }
    for v_id in merge_map.keys() {
        topo.vertices.remove(*v_id);
    }
}

/// Make loops conforming: split unpaired half-edges at existing vertices
/// that lie in their interior.
///
/// Trimming splits one operand's face boundary at intersection points, but
/// the neighboring face that shares the untrimmed edge keeps it whole — a
/// T-junction. The whole edge can never pair with the two halves, so the
/// sewn shell reports open edges, tessellation inherits seam cracks, and
/// downstream booleans on the result degrade or blow up. Splitting the
/// whole edge at the interior vertex (reusing the existing `VertexId`, so
/// `pair_half_edges` can pair the pieces by id) restores a conforming,
/// closed shell.
fn split_edges_at_interior_vertices(topo: &mut Topology, tolerance: f64) {
    // Snapshot loop vertices (id + position). Only vertices that appear in
    // loops matter; isolated vertices can't be a neighbor's endpoint.
    let mut verts: Vec<(vcad_kernel_topo::VertexId, Point3)> = Vec::new();
    {
        let mut seen = std::collections::HashSet::new();
        for (_, he) in &topo.half_edges {
            if he.loop_id.is_none() {
                continue;
            }
            if seen.insert(he.origin) {
                verts.push((he.origin, topo.vertices[he.origin].point));
            }
        }
    }

    let he_ids: Vec<_> = topo.half_edges.keys().collect();
    for he_id in he_ids {
        // Only unpaired half-edges: a paired edge already conforms with its
        // twin, and splitting it would need a synchronized twin split.
        if topo.half_edges[he_id].twin.is_some() || topo.half_edges[he_id].loop_id.is_none() {
            continue;
        }
        let next = match topo.half_edges[he_id].next {
            Some(n) => n,
            None => continue,
        };
        if next == he_id {
            continue; // single-edge loop (closed curve)
        }
        let v0 = topo.half_edges[he_id].origin;
        let v1 = topo.half_edges[next].origin;
        let a = topo.vertices[v0].point;
        let b = topo.vertices[v1].point;
        let ab = b - a;
        let len2 = ab.dot(ab);
        if len2 <= tolerance * tolerance {
            continue;
        }
        let len = len2.sqrt();

        // Interior vertices within `tolerance` of segment ab, ordered by t.
        let mut hits: Vec<(f64, vcad_kernel_topo::VertexId)> = Vec::new();
        for &(vid, p) in &verts {
            if vid == v0 || vid == v1 {
                continue;
            }
            let t = (p - a).dot(ab) / len2;
            // Strict interior: at least `tolerance` away from both endpoints.
            let margin = tolerance / len;
            if t <= margin || t >= 1.0 - margin {
                continue;
            }
            let foot = a + ab * t;
            if are_close(&p, &foot, tolerance) {
                hits.push((t, vid));
            }
        }
        if hits.is_empty() {
            continue;
        }
        hits.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        hits.dedup_by_key(|h| h.1);

        // Rechain: he keeps the first sub-segment (origin v0); each hit
        // starts a new half-edge, the last one ending at the original dest.
        let loop_id = topo.half_edges[he_id].loop_id;
        let mut prev = he_id;
        for &(_, vid) in &hits {
            let he_new = topo.add_half_edge(vid);
            topo.half_edges[he_new].loop_id = loop_id;
            topo.half_edges[he_new].prev = Some(prev);
            topo.half_edges[he_new].next = Some(next);
            topo.half_edges[prev].next = Some(he_new);
            topo.half_edges[next].prev = Some(he_new);
            prev = he_new;
        }
    }
}

fn collapse_degenerate_half_edges(topo: &mut Topology, tolerance: f64) {
    let he_ids: Vec<_> = topo.half_edges.keys().collect();
    for he_id in he_ids {
        let loop_id = topo.half_edges[he_id].loop_id;
        if loop_id.is_none() {
            continue;
        }
        let next = match topo.half_edges[he_id].next {
            Some(next) => next,
            None => continue,
        };

        // Don't collapse half-edges that form a single-edge loop (valid closed curves like circles)
        if next == he_id {
            continue;
        }

        // Don't collapse half-edges that have a twin - they're part of a valid edge pair
        // (even if origin == dest, they might represent closed curves like circles)
        if topo.half_edges[he_id].twin.is_some() {
            continue;
        }

        let origin = topo.half_edges[he_id].origin;
        let dest = topo.half_edges[next].origin;
        let origin_point = topo.vertices[origin].point;
        let dest_point = topo.vertices[dest].point;
        if are_close(&origin_point, &dest_point, tolerance) {
            unlink_half_edge(topo, he_id);
        }
    }
}

fn cleanup_loop_spikes(topo: &mut Topology, tolerance: f64) {
    let loop_ids: Vec<_> = topo.loops.keys().collect();
    for loop_id in loop_ids {
        let mut changed = true;
        while changed {
            changed = false;
            let hes: Vec<_> = topo.loop_half_edges(loop_id).collect();
            if hes.len() < 3 {
                break;
            }
            for i in 0..hes.len() {
                let he_prev = hes[(i + hes.len() - 1) % hes.len()];
                let he_mid = hes[i];
                let he_next = hes[(i + 1) % hes.len()];

                // Don't remove half-edges that have a twin - they're part of a valid edge pair
                if topo.half_edges[he_mid].twin.is_some() {
                    continue;
                }

                let v_prev = topo.half_edges[he_prev].origin;
                let v_next = topo.half_edges[he_next].origin;
                let p_prev = topo.vertices[v_prev].point;
                let p_next = topo.vertices[v_next].point;
                if are_close(&p_prev, &p_next, tolerance) {
                    topo.half_edges[he_next].origin = v_prev;
                    unlink_half_edge(topo, he_mid);
                    changed = true;
                    break;
                }
            }
        }
    }
}

fn pair_half_edges(topo: &mut Topology, tolerance: f64) {
    // Use vertex IDs directly for matching (after vertex merging, IDs are canonical)
    // Also use position-based fallback with coarser tolerance for robustness
    use vcad_kernel_topo::VertexId;

    // First pass: match by vertex IDs (fast and exact)
    let mut id_candidates: HashMap<(VertexId, VertexId), HalfEdgeId> = HashMap::new();
    let he_ids: Vec<_> = topo.half_edges.keys().collect();

    for he_id in &he_ids {
        if topo.half_edges[*he_id].twin.is_some() {
            continue;
        }
        if topo.half_edges[*he_id].loop_id.is_none() {
            continue;
        }
        let next = match topo.half_edges[*he_id].next {
            Some(next) => next,
            None => continue,
        };
        let origin = topo.half_edges[*he_id].origin;
        let dest = topo.half_edges[next].origin;

        // Look for twin going dest -> origin
        if let Some(&twin_he) = id_candidates.get(&(dest, origin)) {
            if topo.half_edges[twin_he].twin.is_none() {
                topo.add_edge(*he_id, twin_he);
                id_candidates.remove(&(dest, origin));
                continue;
            }
        }
        // Store for later matching
        id_candidates.insert((origin, dest), *he_id);
    }

    // Second pass: match remaining by position (for cases where vertex IDs differ but positions match)
    let mut pos_candidates: HashMap<EdgeKey, HalfEdgeInfo> = HashMap::new();

    for he_id in he_ids {
        if topo.half_edges[he_id].twin.is_some() {
            continue;
        }
        if topo.half_edges[he_id].loop_id.is_none() {
            continue;
        }
        let next = match topo.half_edges[he_id].next {
            Some(next) => next,
            None => continue,
        };
        let origin = topo.half_edges[he_id].origin;
        let dest = topo.half_edges[next].origin;
        // Use coarser tolerance (2x) for position matching to handle floating point issues
        let origin_key = VertexKey::from_point(&topo.vertices[origin].point, tolerance * 2.0);
        let dest_key = VertexKey::from_point(&topo.vertices[dest].point, tolerance * 2.0);
        let edge_key = EdgeKey::new(origin_key, dest_key);

        if let Some(existing) = pos_candidates.remove(&edge_key) {
            let opposite = existing.origin_key == dest_key && existing.dest_key == origin_key;
            if opposite
                && topo.half_edges[existing.half_edge].twin.is_none()
                && topo.half_edges[he_id].twin.is_none()
            {
                topo.add_edge(existing.half_edge, he_id);
            } else {
                pos_candidates.insert(edge_key, existing);
            }
        } else {
            pos_candidates.insert(
                edge_key,
                HalfEdgeInfo {
                    half_edge: he_id,
                    origin_key,
                    dest_key,
                },
            );
        }
    }
}

fn unlink_half_edge(topo: &mut Topology, he_id: HalfEdgeId) {
    let loop_id = match topo.half_edges[he_id].loop_id {
        Some(loop_id) => loop_id,
        None => return,
    };
    let prev = match topo.half_edges[he_id].prev {
        Some(prev) => prev,
        None => return,
    };
    let next = match topo.half_edges[he_id].next {
        Some(next) => next,
        None => return,
    };
    if prev == he_id || next == he_id {
        return;
    }

    topo.half_edges[prev].next = Some(next);
    topo.half_edges[next].prev = Some(prev);
    if topo.loops[loop_id].half_edge == he_id {
        topo.loops[loop_id].half_edge = next;
    }

    let origin = topo.half_edges[he_id].origin;
    if topo.vertices[origin].half_edge == Some(he_id) {
        topo.vertices[origin].half_edge = Some(next);
    }

    if let Some(twin) = topo.half_edges[he_id].twin {
        topo.half_edges[twin].twin = None;
        topo.half_edges[twin].edge = None;
    }
    if let Some(edge_id) = topo.half_edges[he_id].edge {
        topo.edges.remove(edge_id);
    }

    topo.half_edges[he_id].twin = None;
    topo.half_edges[he_id].edge = None;
    topo.half_edges[he_id].loop_id = None;
    topo.half_edges[he_id].next = None;
    topo.half_edges[he_id].prev = None;
}

fn are_close(a: &Point3, b: &Point3, tolerance: f64) -> bool {
    (a - b).norm_squared() <= tolerance * tolerance
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct VertexKey {
    x: i64,
    y: i64,
    z: i64,
}

impl VertexKey {
    fn from_point(p: &Point3, tolerance: f64) -> Self {
        let scale = if tolerance > 0.0 {
            1.0 / tolerance
        } else {
            1.0e6
        };
        Self {
            x: (p.x * scale).round() as i64,
            y: (p.y * scale).round() as i64,
            z: (p.z * scale).round() as i64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EdgeKey {
    a: VertexKey,
    b: VertexKey,
}

impl EdgeKey {
    fn new(a: VertexKey, b: VertexKey) -> Self {
        if vertex_key_less(a, b) {
            Self { a, b }
        } else {
            Self { a: b, b: a }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HalfEdgeInfo {
    half_edge: HalfEdgeId,
    origin_key: VertexKey,
    dest_key: VertexKey,
}

fn vertex_key_less(a: VertexKey, b: VertexKey) -> bool {
    (a.x, a.y, a.z) < (b.x, b.y, b.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collapse_degenerate_half_edge() {
        let mut topo = Topology::new();
        let v0 = topo.add_vertex(Point3::new(0.0, 0.0, 0.0));
        let v1 = topo.add_vertex(Point3::new(1.0, 0.0, 0.0));

        let he0 = topo.add_half_edge(v0);
        let he1 = topo.add_half_edge(v1);
        let he2 = topo.add_half_edge(v1);

        let loop_id = topo.add_loop(&[he0, he1, he2]);
        topo.add_face(loop_id, 0, vcad_kernel_topo::Orientation::Forward);

        repair_topology(&mut topo, 1e-6);

        assert!(topo.half_edges[he1].loop_id.is_none());
        assert_eq!(topo.half_edges[he0].next, Some(he2));
        assert_eq!(topo.half_edges[he2].prev, Some(he0));
    }

    #[test]
    fn test_pair_half_edges() {
        let mut topo = Topology::new();
        let v0 = topo.add_vertex(Point3::new(0.0, 0.0, 0.0));
        let v1 = topo.add_vertex(Point3::new(1.0, 0.0, 0.0));

        let he0 = topo.add_half_edge(v0);
        let he1 = topo.add_half_edge(v1);
        let loop_a = topo.add_loop(&[he0, he1]);
        topo.add_face(loop_a, 0, vcad_kernel_topo::Orientation::Forward);

        let he2 = topo.add_half_edge(v1);
        let he3 = topo.add_half_edge(v0);
        let loop_b = topo.add_loop(&[he2, he3]);
        topo.add_face(loop_b, 1, vcad_kernel_topo::Orientation::Forward);

        repair_topology(&mut topo, 1e-6);

        let twin = topo.half_edges[he0].twin.expect("expected paired twin");
        let dest = topo.half_edge_dest(he0);
        let twin_dest = topo.half_edge_dest(twin);
        assert_eq!(topo.half_edges[twin].origin, dest);
        assert_eq!(twin_dest, topo.half_edges[he0].origin);
    }
}
