//! Wire geometry: grids, segments, and the triangular current bases.
//!
//! A [`WireGrid`] is authored in **millimeters** (vcad convention) as
//! straight wires, open paths, and closed loops; endpoints within 1 µm are
//! merged, so a dipole fed at its center is one wire and a two-element
//! array is two `add_wire` calls. [`Mesh::build`] compiles the grid to SI
//! units and attaches one **triangular (rooftop) basis function per
//! interior node**: the basis peaks at its node, falls linearly to zero at
//! the two neighboring nodes, and represents current flowing *through* the
//! node in a fixed reference direction. Free wire ends get no basis — the
//! current physically vanishes there — and nodes joining three or more
//! segments are rejected fail-closed until the junction milestone (M1).
//!
//! Each basis half stores an explicit sign relating the basis reference
//! direction to the segment's own orientation, so meshes built from wires
//! authored in any direction assemble correctly (and M1 junctions reuse
//! the same machinery: a junction basis is just more halves).

use crate::error::AntennaError;

/// Node-merge tolerance when authoring geometry, mm (1 µm).
pub const NODE_TOL_MM: f64 = 1e-3;

/// Wire geometry under construction, in millimeters.
#[derive(Debug, Clone, Default)]
pub struct WireGrid {
    nodes_mm: Vec<[f64; 3]>,
    /// (node0, node1, radius_mm) per segment.
    segments: Vec<(usize, usize, f64)>,
    ground_plane: bool,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

impl WireGrid {
    /// Empty grid.
    pub fn new() -> Self {
        Self::default()
    }

    fn node_at(&mut self, p_mm: [f64; 3]) -> usize {
        for (i, n) in self.nodes_mm.iter().enumerate() {
            if norm(sub(*n, p_mm)) <= NODE_TOL_MM {
                return i;
            }
        }
        self.nodes_mm.push(p_mm);
        self.nodes_mm.len() - 1
    }

    fn add_leg(
        &mut self,
        a_mm: [f64; 3],
        b_mm: [f64; 3],
        radius_mm: f64,
        segments: usize,
    ) -> Result<(), AntennaError> {
        if radius_mm <= 0.0 || !radius_mm.is_finite() {
            return Err(AntennaError::InvalidRadius { radius_mm });
        }
        if segments == 0 {
            return Err(AntennaError::InvalidSegmentCount);
        }
        let len = norm(sub(b_mm, a_mm));
        if len <= NODE_TOL_MM * segments as f64 {
            return Err(AntennaError::DegenerateWire { length_mm: len });
        }
        let mut prev = self.node_at(a_mm);
        for i in 1..=segments {
            let t = i as f64 / segments as f64;
            let next = if i == segments {
                self.node_at(b_mm)
            } else {
                // Interior subdivision points are freshly authored; they are
                // deduplicated too, so coincident wires still share nodes.
                self.node_at(lerp(a_mm, b_mm, t))
            };
            self.segments.push((prev, next, radius_mm));
            prev = next;
        }
        Ok(())
    }

    /// Add a straight wire from `a_mm` to `b_mm`, split into `segments`
    /// equal segments of wire radius `radius_mm`.
    pub fn add_wire(
        &mut self,
        a_mm: [f64; 3],
        b_mm: [f64; 3],
        radius_mm: f64,
        segments: usize,
    ) -> Result<(), AntennaError> {
        self.add_leg(a_mm, b_mm, radius_mm, segments)
    }

    /// Add an open polyline through `points_mm`; leg `i` (from point `i` to
    /// `i+1`) is split into `segments_per_leg[i]` segments.
    pub fn add_path(
        &mut self,
        points_mm: &[[f64; 3]],
        radius_mm: f64,
        segments_per_leg: &[usize],
    ) -> Result<(), AntennaError> {
        if points_mm.len() < 2 {
            return Err(AntennaError::PathTooShort);
        }
        if segments_per_leg.len() != points_mm.len() - 1 {
            return Err(AntennaError::LegCountMismatch {
                legs: points_mm.len() - 1,
                counts: segments_per_leg.len(),
            });
        }
        for (i, w) in points_mm.windows(2).enumerate() {
            self.add_leg(w[0], w[1], radius_mm, segments_per_leg[i])?;
        }
        Ok(())
    }

    /// Add a closed loop through `points_mm` (last point connects back to
    /// the first); `segments_per_leg` has one entry per leg including the
    /// closing one.
    pub fn add_loop(
        &mut self,
        points_mm: &[[f64; 3]],
        radius_mm: f64,
        segments_per_leg: &[usize],
    ) -> Result<(), AntennaError> {
        if points_mm.len() < 3 {
            return Err(AntennaError::PathTooShort);
        }
        if segments_per_leg.len() != points_mm.len() {
            return Err(AntennaError::LegCountMismatch {
                legs: points_mm.len(),
                counts: segments_per_leg.len(),
            });
        }
        for i in 0..points_mm.len() {
            let a = points_mm[i];
            let b = points_mm[(i + 1) % points_mm.len()];
            self.add_leg(a, b, radius_mm, segments_per_leg[i])?;
        }
        Ok(())
    }

    /// Number of segments authored so far.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Model a perfect electric conductor at z = 0 via image theory.
    ///
    /// All geometry must then satisfy z ≥ 0. A wire *endpoint* at z = 0 is
    /// electrically connected to the plane (monopole base); wires lying in
    /// the plane or passing junctions through it fail closed.
    pub fn set_ground_plane(&mut self, on: bool) {
        self.ground_plane = on;
    }
}

/// One straight wire segment, SI units (meters).
#[derive(Debug, Clone, Copy)]
pub struct Segment {
    /// Start node index.
    pub n0: usize,
    /// End node index.
    pub n1: usize,
    /// Start point, m.
    pub p0: [f64; 3],
    /// Unit tangent from `n0` to `n1`.
    pub tangent: [f64; 3],
    /// Length, m.
    pub len: f64,
    /// Wire radius, m.
    pub radius: f64,
}

/// One half of a triangular basis: which segment it lives on, which end of
/// that segment is the basis node, and the sign relating basis reference
/// direction to the segment's orientation.
#[derive(Debug, Clone, Copy)]
pub struct BasisHalf {
    /// Segment index.
    pub seg: usize,
    /// Endpoint of `seg` that is the basis node: 0 → `n0`, 1 → `n1`.
    pub end: u8,
    /// +1 when current in the basis reference direction flows along the
    /// segment's own orientation, −1 when against it.
    pub sign: f64,
}

/// A triangular current basis centered on a node.
///
/// A degree-2 (interior) node carries one basis with two halves. A
/// degree-`d` junction carries `d − 1` bases — each routes current from a
/// common reference branch into one other branch, which spans exactly the
/// KCL-constrained current space at the junction. A wire endpoint on the
/// ground plane carries a one-half basis: its other half is the image
/// current below the plane, supplied by the image-source fill.
#[derive(Debug, Clone)]
pub struct Basis {
    /// The node the basis peaks at.
    pub node: usize,
    /// The segment halves it spans (2 normally, 1 for a grounded end).
    pub halves: Vec<BasisHalf>,
}

/// Compiled wire mesh in SI units, ready for the MoM fill.
#[derive(Debug, Clone)]
pub struct Mesh {
    /// Node positions, m.
    pub nodes: Vec<[f64; 3]>,
    /// Straight segments.
    pub segments: Vec<Segment>,
    /// Triangular bases (interior nodes, junctions, grounded ends).
    pub bases: Vec<Basis>,
    /// Perfect electric conductor at z = 0, modeled by image theory.
    pub ground_plane: bool,
}

fn toward(seg: usize, end: u8) -> BasisHalf {
    BasisHalf {
        seg,
        end,
        sign: if end == 1 { 1.0 } else { -1.0 },
    }
}

fn away(seg: usize, end: u8) -> BasisHalf {
    BasisHalf {
        seg,
        end,
        sign: if end == 0 { 1.0 } else { -1.0 },
    }
}

impl Mesh {
    /// Compile a wire grid: convert to meters, derive per-node degrees, and
    /// build the triangular bases (interior nodes, KCL junction bases, and
    /// — with a ground plane — grounded-end bases).
    pub fn build(grid: &WireGrid) -> Result<Mesh, AntennaError> {
        let ground = grid.ground_plane;
        let mut nodes: Vec<[f64; 3]> = grid
            .nodes_mm
            .iter()
            .map(|p| [p[0] * 1e-3, p[1] * 1e-3, p[2] * 1e-3])
            .collect();
        let tol = NODE_TOL_MM * 1e-3;
        let mut grounded = vec![false; nodes.len()];
        if ground {
            for (i, n) in nodes.iter_mut().enumerate() {
                if n[2] < -tol {
                    return Err(AntennaError::BelowGroundPlane {
                        node: i,
                        z_mm: n[2] * 1e3,
                    });
                }
                if n[2].abs() <= tol {
                    n[2] = 0.0; // snap for a clean mirror
                    grounded[i] = true;
                }
            }
        }
        let mut segments = Vec::with_capacity(grid.segments.len());
        for (si, &(n0, n1, r_mm)) in grid.segments.iter().enumerate() {
            if ground && grounded[n0] && grounded[n1] {
                return Err(AntennaError::SegmentOnGroundPlane { segment: si });
            }
            let p0 = nodes[n0];
            let d = sub(nodes[n1], p0);
            let len = norm(d);
            segments.push(Segment {
                n0,
                n1,
                p0,
                tangent: [d[0] / len, d[1] / len, d[2] / len],
                len,
                radius: r_mm * 1e-3,
            });
        }

        // Incidence: (segment, end-at-this-node) per node.
        let mut incident: Vec<Vec<(usize, u8)>> = vec![Vec::new(); nodes.len()];
        for (si, s) in segments.iter().enumerate() {
            incident[s.n0].push((si, 0));
            incident[s.n1].push((si, 1));
        }

        let mut bases = Vec::new();
        for (node, inc) in incident.iter().enumerate() {
            if grounded[node] {
                // A wire end on the ground plane: current continues into
                // the image. One real half; the mirror is supplied by the
                // image-source fill. Interior/junction ground contacts are
                // out of scope (the plane would need its own port model).
                match inc.len() {
                    0 => {}
                    1 => {
                        let (s, e) = inc[0];
                        bases.push(Basis {
                            node,
                            halves: vec![away(s, e)],
                        });
                    }
                    degree => {
                        return Err(AntennaError::GroundContactUnsupported { node, degree });
                    }
                }
                continue;
            }
            match inc.len() {
                0 | 1 => {} // isolated (unreachable) or free end: current = 0
                2 => {
                    // Reference direction: through the node from inc[0]'s
                    // side into inc[1]'s side.
                    let (s_a, e_a) = inc[0];
                    let (s_b, e_b) = inc[1];
                    bases.push(Basis {
                        node,
                        halves: vec![toward(s_a, e_a), away(s_b, e_b)],
                    });
                }
                d => {
                    // Junction: d − 1 bases, each carrying current from the
                    // reference branch inc[0] into branch i. Any KCL-legal
                    // current split is a combination of these; continuity
                    // per basis makes charge bookkeeping automatic.
                    let (s_ref, e_ref) = inc[0];
                    for &(s_i, e_i) in &inc[1..d] {
                        bases.push(Basis {
                            node,
                            halves: vec![toward(s_ref, e_ref), away(s_i, e_i)],
                        });
                    }
                }
            }
        }

        if bases.is_empty() {
            return Err(AntennaError::NoBases);
        }
        Ok(Mesh {
            nodes,
            segments,
            bases,
            ground_plane: ground,
        })
    }

    /// Fail-closed physical-validity gates at a given frequency.
    ///
    /// Checks, per segment: length ≥ 4 × radius (thin-wire kernel),
    /// length ≤ λ/8 (current sampling), and k·a ≤ 0.1 (radius ≪ λ).
    pub fn validate_for(&self, freq_hz: f64) -> Result<(), AntennaError> {
        if freq_hz <= 0.0 || !freq_hz.is_finite() {
            return Err(AntennaError::InvalidFrequency { freq_hz });
        }
        let lambda = crate::constants::C0 / freq_hz;
        let k = 2.0 * std::f64::consts::PI / lambda;
        for (i, s) in self.segments.iter().enumerate() {
            if s.len < 4.0 * s.radius {
                return Err(AntennaError::ThinWireViolation {
                    segment: i,
                    length_mm: s.len * 1e3,
                    radius_mm: s.radius * 1e3,
                });
            }
            if s.len > lambda / 8.0 {
                return Err(AntennaError::SegmentTooLong {
                    segment: i,
                    length_mm: s.len * 1e3,
                    max_mm: lambda / 8.0 * 1e3,
                });
            }
            if k * s.radius > 0.1 {
                return Err(AntennaError::RadiusTooThick {
                    radius_mm: s.radius * 1e3,
                    max_mm: 0.1 / k * 1e3,
                });
            }
        }
        Ok(())
    }

    /// Index of the basis whose node is closest to `p_mm` (millimeters).
    pub fn nearest_basis(&self, p_mm: [f64; 3]) -> Result<usize, AntennaError> {
        let p = [p_mm[0] * 1e-3, p_mm[1] * 1e-3, p_mm[2] * 1e-3];
        self.bases
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = norm(sub(self.nodes[a.node], p));
                let db = norm(sub(self.nodes[b.node], p));
                da.partial_cmp(&db).unwrap()
            })
            .map(|(i, _)| i)
            .ok_or(AntennaError::NoBases)
    }

    /// Endpoint current coefficients `(c0, c1)` per segment for a solved
    /// current vector: the current on segment `s` at parameter `t ∈ [0, L]`
    /// is `c0·(1 − t/L) + c1·(t/L)` along the segment tangent.
    pub fn segment_endpoint_currents(
        &self,
        currents: &[crate::complex::Complex],
    ) -> Vec<(crate::complex::Complex, crate::complex::Complex)> {
        let mut out = vec![
            (crate::complex::Complex::ZERO, crate::complex::Complex::ZERO);
            self.segments.len()
        ];
        for (bi, b) in self.bases.iter().enumerate() {
            for h in &b.halves {
                let contrib = currents[bi].scale(h.sign);
                if h.end == 0 {
                    out[h.seg].0 += contrib;
                } else {
                    out[h.seg].1 += contrib;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dipole_mesh_has_interior_bases_only() {
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 10)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        assert_eq!(m.segments.len(), 10);
        assert_eq!(m.bases.len(), 9); // free ends carry no unknown
                                      // Consistently-oriented chain: all halves get sign +1.
        for b in &m.bases {
            assert!(b.halves.iter().all(|h| h.sign == 1.0));
        }
    }

    #[test]
    fn two_wires_sharing_an_endpoint_merge_nodes() {
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, 0.0], [100.0, 0.0, 0.0], 1.0, 4)
            .unwrap();
        g.add_wire([100.0, 0.0, 0.0], [100.0, 100.0, 0.0], 1.0, 4)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        // 9 distinct nodes (shared corner), 8 segments, 7 bases.
        assert_eq!(m.nodes.len(), 9);
        assert_eq!(m.segments.len(), 8);
        assert_eq!(m.bases.len(), 7);
    }

    #[test]
    fn opposed_orientation_gets_signed_basis() {
        // Both wires oriented INTO the shared node: the basis must flip one
        // side so the element still represents continuous through-current.
        let mut g = WireGrid::new();
        g.add_wire([-100.0, 0.0, 0.0], [0.0, 0.0, 0.0], 1.0, 1)
            .unwrap();
        g.add_wire([100.0, 0.0, 0.0], [0.0, 0.0, 0.0], 1.0, 1)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        assert_eq!(m.bases.len(), 1);
        let signs: Vec<f64> = m.bases[0].halves.iter().map(|h| h.sign).collect();
        assert_eq!(signs.iter().product::<f64>(), -1.0, "one half must flip");
        // Endpoint currents: unit coefficient must give equal-magnitude
        // endpoint currents on both segments at the shared node.
        let cur = vec![crate::complex::Complex::ONE];
        let ends = m.segment_endpoint_currents(&cur);
        assert!((ends[0].1.re.abs() - 1.0).abs() < 1e-14);
        assert!((ends[1].1.re.abs() - 1.0).abs() < 1e-14);
    }

    #[test]
    fn closed_loop_has_one_basis_per_segment() {
        let mut g = WireGrid::new();
        let pts: Vec<[f64; 3]> = (0..8)
            .map(|i| {
                let th = std::f64::consts::TAU * i as f64 / 8.0;
                [100.0 * th.cos(), 100.0 * th.sin(), 0.0]
            })
            .collect();
        g.add_loop(&pts, 1.0, &[1; 8]).unwrap();
        let m = Mesh::build(&g).unwrap();
        assert_eq!(m.segments.len(), 8);
        assert_eq!(m.bases.len(), 8); // every node interior on a loop
    }

    #[test]
    fn junction_gets_kcl_spanning_bases() {
        // Three wires meeting at the origin: degree 3 → 2 junction bases,
        // each pairing the reference branch with one other branch.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, 0.0], [100.0, 0.0, 0.0], 1.0, 2)
            .unwrap();
        g.add_wire([0.0, 0.0, 0.0], [0.0, 100.0, 0.0], 1.0, 2)
            .unwrap();
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 100.0], 1.0, 2)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        // 6 segments; 3 interior mid-wire nodes + 2 junction bases.
        assert_eq!(m.segments.len(), 6);
        assert_eq!(m.bases.len(), 5);
        let at_origin = m.bases.iter().filter(|b| b.node == 0).count();
        assert_eq!(at_origin, 2);
        // KCL: with any coefficients, the signed endpoint currents at the
        // junction node must sum to zero (inflow = outflow).
        let coeffs: Vec<crate::complex::Complex> = (0..m.bases.len())
            .map(|i| crate::complex::Complex::new(1.0 + i as f64, 0.5 * i as f64))
            .collect();
        let ends = m.segment_endpoint_currents(&coeffs);
        let mut net = crate::complex::Complex::ZERO;
        for (si, s) in m.segments.iter().enumerate() {
            // Current flowing INTO the junction node along each segment.
            if s.n0 == 0 {
                net -= ends[si].0; // tangent points away from the node
            }
            if s.n1 == 0 {
                net += ends[si].1; // tangent points into the node
            }
        }
        assert!(net.abs() < 1e-12, "KCL violated at junction: {net:?}");
    }

    #[test]
    fn ground_plane_gates_and_grounded_base() {
        // Below-plane geometry fails closed.
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        g.add_wire([0.0, 0.0, -50.0], [0.0, 0.0, 500.0], 1.0, 4)
            .unwrap();
        assert!(matches!(
            Mesh::build(&g),
            Err(AntennaError::BelowGroundPlane { .. })
        ));

        // A wire lying in the plane is shorted by its own image.
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        g.add_wire([0.0, 0.0, 0.0], [100.0, 0.0, 0.0], 1.0, 2)
            .unwrap();
        assert!(matches!(
            Mesh::build(&g),
            Err(AntennaError::SegmentOnGroundPlane { .. })
        ));

        // A monopole: base node gets a one-half basis, so the base carries
        // current (feed point), and the count is nseg (not nseg − 1).
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 500.0], 1.0, 10)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        assert!(m.ground_plane);
        assert_eq!(m.bases.len(), 10);
        let base = m.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        assert_eq!(m.bases[base].halves.len(), 1);

        // Without the plane, the same wire has free ends: nseg − 1 bases.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 500.0], 1.0, 10)
            .unwrap();
        assert_eq!(Mesh::build(&g).unwrap().bases.len(), 9);
    }

    #[test]
    fn validity_gates_fire() {
        // 100 mm wire, 10 segments → 10 mm segments; radius 3 mm → 4a = 12 mm.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 100.0], 3.0, 10)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        match m.validate_for(100e6) {
            Err(AntennaError::ThinWireViolation { .. }) => {}
            other => panic!("expected thin-wire violation, got {other:?}"),
        }

        // 1 mm radius passes the thin-wire gate but 10 mm segments fail
        // λ/8 = 4.6 mm at 8 GHz.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 100.0], 1.0, 10)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        match m.validate_for(8e9) {
            Err(AntennaError::SegmentTooLong { .. }) => {}
            other => panic!("expected segment-too-long, got {other:?}"),
        }

        // Fat wire vs wavelength: k·a > 0.1 at 6 GHz for a = 1 mm
        // (k·a = 2π·6e9/3e8 · 1e-3 ≈ 0.126) with segments kept legal.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, 0.0], [0.0, 0.0, 25.0], 1.0, 5)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        match m.validate_for(6e9) {
            Err(AntennaError::RadiusTooThick { .. }) => {}
            other => panic!("expected radius-too-thick, got {other:?}"),
        }

        // And a legal mesh validates.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 20)
            .unwrap();
        let m = Mesh::build(&g).unwrap();
        m.validate_for(150e6).unwrap();
    }
}
