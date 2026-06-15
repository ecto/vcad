//! R-tree spatial index for PCB copper elements.
//!
//! Wraps the [`rstar`] crate to provide efficient spatial queries over
//! copper elements (traces, pads, vias) on a PCB. Used by the DRC engine
//! and copper pour algorithm for proximity checks.

use rstar::{RTree, RTreeObject, AABB};

use vcad_ir::ecad::{Pad, PadShape, Pcb, PcbLayer};
use vcad_ir::Vec2;

/// True (non-bbox) copper geometry for a [`CopperElement`].
///
/// The R-tree query uses the element bounding box as a broadphase candidate
/// filter; this payload lets DRC compute the exact copper-to-copper distance
/// in the narrowphase, so diagonal traces and rotated pads are no longer
/// over-reported.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CopperGeom {
    /// A trace: a capsule (segment swept by `half_w`).
    Segment {
        /// Segment start (mm).
        a: Vec2,
        /// Segment end (mm).
        b: Vec2,
        /// Half the trace width (the capsule radius), in mm.
        half_w: f64,
    },
    /// A round disc (via, or circular/oval pad approximation).
    Disc {
        /// Disc center (mm).
        center: Vec2,
        /// Disc radius (mm).
        r: f64,
    },
    /// A (possibly rotated) rectangle (rect / roundrect / oval pad).
    Rect {
        /// Rectangle center (mm).
        center: Vec2,
        /// Half-width along the local X axis (mm).
        half_w: f64,
        /// Half-height along the local Y axis (mm).
        half_h: f64,
        /// Rotation of the rectangle in radians (counter-clockwise).
        rot: f64,
    },
}

/// A copper element stored in the spatial index.
#[derive(Debug, Clone)]
pub struct CopperElement {
    /// Minimum corner of the bounding box `[x, y]` in mm.
    pub min: [f64; 2],
    /// Maximum corner of the bounding box `[x, y]` in mm.
    pub max: [f64; 2],
    /// Net this element belongs to.
    pub net: String,
    /// Layer this element is on.
    pub layer: PcbLayer,
    /// True copper geometry for narrowphase distance computation.
    pub geom: CopperGeom,
}

impl CopperGeom {
    /// True minimum copper-to-copper distance between two geometries (mm).
    ///
    /// Returns `0.0` when the copper bodies touch or overlap. Trace and disc
    /// radii (half-widths) are accounted for, so this is edge-to-edge, not
    /// centerline-to-centerline.
    pub fn distance_to(&self, other: &CopperGeom) -> f64 {
        match (self, other) {
            (
                CopperGeom::Segment { a, b, half_w },
                CopperGeom::Segment {
                    a: c,
                    b: d,
                    half_w: hw2,
                },
            ) => (segment_segment_distance(*a, *b, *c, *d) - half_w - hw2).max(0.0),
            (CopperGeom::Segment { a, b, half_w }, CopperGeom::Disc { center, r })
            | (CopperGeom::Disc { center, r }, CopperGeom::Segment { a, b, half_w }) => {
                (point_segment_distance(*center, *a, *b) - half_w - r).max(0.0)
            }
            (CopperGeom::Disc { center: c1, r: r1 }, CopperGeom::Disc { center: c2, r: r2 }) => {
                (vec_dist(*c1, *c2) - r1 - r2).max(0.0)
            }
            (CopperGeom::Rect { .. }, CopperGeom::Rect { .. }) => {
                let pa = self.rect_corners();
                let pb = other.rect_corners();
                convex_poly_distance(&pa, &pb)
            }
            (rect @ CopperGeom::Rect { .. }, CopperGeom::Disc { center, r })
            | (CopperGeom::Disc { center, r }, rect @ CopperGeom::Rect { .. }) => {
                (rect.point_distance(*center) - r).max(0.0)
            }
            (rect @ CopperGeom::Rect { .. }, CopperGeom::Segment { a, b, half_w })
            | (CopperGeom::Segment { a, b, half_w }, rect @ CopperGeom::Rect { .. }) => {
                let corners = rect.rect_corners();
                let mut min_d = f64::MAX;
                let n = corners.len();
                for i in 0..n {
                    let e0 = corners[i];
                    let e1 = corners[(i + 1) % n];
                    let d = segment_segment_distance(e0, e1, *a, *b);
                    if d < min_d {
                        min_d = d;
                    }
                }
                // If the segment passes through the rect interior the
                // edge-distance is 0; guard with containment of an endpoint.
                if rect.contains_point(*a) || rect.contains_point(*b) {
                    min_d = 0.0;
                }
                (min_d - half_w).max(0.0)
            }
        }
    }

    /// The four corners of a rectangle geometry (panics if not a rect).
    fn rect_corners(&self) -> [Vec2; 4] {
        match self {
            CopperGeom::Rect {
                center,
                half_w,
                half_h,
                rot,
            } => {
                let (s, c) = rot.sin_cos();
                let local = [
                    (-half_w, -half_h),
                    (*half_w, -half_h),
                    (*half_w, *half_h),
                    (-half_w, *half_h),
                ];
                let mut out = [Vec2::new(0.0, 0.0); 4];
                for (i, (lx, ly)) in local.iter().enumerate() {
                    out[i] = Vec2::new(center.x + lx * c - ly * s, center.y + lx * s + ly * c);
                }
                out
            }
            _ => [Vec2::new(0.0, 0.0); 4],
        }
    }

    /// Distance from a point to this rectangle (0 inside).
    fn point_distance(&self, p: Vec2) -> f64 {
        match self {
            CopperGeom::Rect {
                center,
                half_w,
                half_h,
                rot,
            } => {
                // Transform point into the rectangle's local frame.
                let (s, c) = rot.sin_cos();
                let dx = p.x - center.x;
                let dy = p.y - center.y;
                let lx = dx * c + dy * s;
                let ly = -dx * s + dy * c;
                let qx = lx.abs() - half_w;
                let qy = ly.abs() - half_h;
                let ax = qx.max(0.0);
                let ay = qy.max(0.0);
                (ax * ax + ay * ay).sqrt()
            }
            _ => f64::MAX,
        }
    }

    /// True if a point lies inside this rectangle.
    fn contains_point(&self, p: Vec2) -> bool {
        self.point_distance(p) <= 1e-12
    }
}

impl RTreeObject for CopperElement {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.min, self.max)
    }
}

// ============================================================================
// Geometric primitives for narrowphase distance
// ============================================================================

#[inline]
fn vec_dist(a: Vec2, b: Vec2) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

/// Distance from point `p` to segment `a`-`b`.
fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-18 {
        return vec_dist(p, a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    let px = a.x + t * dx;
    let py = a.y + t * dy;
    vec_dist(p, Vec2::new(px, py))
}

/// Minimum distance between two line segments (centerlines), 0 if they cross.
fn segment_segment_distance(p1: Vec2, p2: Vec2, p3: Vec2, p4: Vec2) -> f64 {
    if segments_intersect(p1, p2, p3, p4) {
        return 0.0;
    }
    let mut d = point_segment_distance(p1, p3, p4);
    d = d.min(point_segment_distance(p2, p3, p4));
    d = d.min(point_segment_distance(p3, p1, p2));
    d = d.min(point_segment_distance(p4, p1, p2));
    d
}

#[inline]
fn cross(o: Vec2, a: Vec2, b: Vec2) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// True if segment `p1`-`p2` intersects segment `p3`-`p4` (including touching).
fn segments_intersect(p1: Vec2, p2: Vec2, p3: Vec2, p4: Vec2) -> bool {
    let d1 = cross(p3, p4, p1);
    let d2 = cross(p3, p4, p2);
    let d3 = cross(p1, p2, p3);
    let d4 = cross(p1, p2, p4);

    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }

    let on_seg = |a: Vec2, b: Vec2, c: Vec2| -> bool {
        c.x <= a.x.max(b.x) + 1e-12
            && c.x >= a.x.min(b.x) - 1e-12
            && c.y <= a.y.max(b.y) + 1e-12
            && c.y >= a.y.min(b.y) - 1e-12
    };

    (d1.abs() < 1e-12 && on_seg(p3, p4, p1))
        || (d2.abs() < 1e-12 && on_seg(p3, p4, p2))
        || (d3.abs() < 1e-12 && on_seg(p1, p2, p3))
        || (d4.abs() < 1e-12 && on_seg(p1, p2, p4))
}

/// Minimum edge-to-edge distance between two convex polygons (0 if overlapping).
fn convex_poly_distance(a: &[Vec2], b: &[Vec2]) -> f64 {
    // If any vertex of one is inside the other, they overlap.
    if a.iter().any(|p| point_in_convex(*p, b)) || b.iter().any(|p| point_in_convex(*p, a)) {
        return 0.0;
    }
    let mut min_d = f64::MAX;
    let na = a.len();
    let nb = b.len();
    for i in 0..na {
        let a0 = a[i];
        let a1 = a[(i + 1) % na];
        for j in 0..nb {
            let b0 = b[j];
            let b1 = b[(j + 1) % nb];
            let d = segment_segment_distance(a0, a1, b0, b1);
            if d < min_d {
                min_d = d;
            }
        }
    }
    min_d
}

/// Point-in-polygon test for an arbitrary (possibly concave) simple polygon,
/// using the ray-casting / even-odd rule. Inclusive-ish near the boundary.
pub(crate) fn point_in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let pi = poly[i];
        let pj = poly[j];
        let intersect = ((pi.y > p.y) != (pj.y > p.y))
            && (p.x < (pj.x - pi.x) * (p.y - pi.y) / (pj.y - pi.y) + pi.x);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// True if segment `a`-`b` intersects (or lies inside) a closed polygon.
pub(crate) fn segment_polygon_intersects(a: Vec2, b: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    // Endpoint inside the polygon ⇒ intersects.
    if point_in_polygon(a, poly) || point_in_polygon(b, poly) {
        return true;
    }
    // Otherwise check crossing against each polygon edge.
    for i in 0..n {
        let e0 = poly[i];
        let e1 = poly[(i + 1) % n];
        if segments_intersect(a, b, e0, e1) {
            return true;
        }
    }
    false
}

/// Point-in-convex-polygon test (CCW or CW), inclusive of the boundary.
fn point_in_convex(p: Vec2, poly: &[Vec2]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut has_pos = false;
    let mut has_neg = false;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let c = cross(a, b, p);
        if c > 1e-12 {
            has_pos = true;
        } else if c < -1e-12 {
            has_neg = true;
        }
    }
    !(has_pos && has_neg)
}

impl rstar::PointDistance for CopperElement {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let dx = if point[0] < self.min[0] {
            self.min[0] - point[0]
        } else if point[0] > self.max[0] {
            point[0] - self.max[0]
        } else {
            0.0
        };
        let dy = if point[1] < self.min[1] {
            self.min[1] - point[1]
        } else if point[1] > self.max[1] {
            point[1] - self.max[1]
        } else {
            0.0
        };
        dx * dx + dy * dy
    }
}

/// Spatial index for PCB copper elements.
///
/// Provides efficient region queries over all copper elements on a board
/// using an R-tree data structure.
pub struct SpatialIndex {
    tree: RTree<CopperElement>,
}

impl SpatialIndex {
    /// Create an empty spatial index.
    pub fn new() -> Self {
        Self { tree: RTree::new() }
    }

    /// Build a spatial index from all copper elements on a PCB.
    ///
    /// Indexes traces, vias, and pads from all footprints.
    pub fn from_pcb(pcb: &Pcb) -> Self {
        let mut elements = Vec::new();

        // Index traces
        for trace in &pcb.traces {
            let half_w = trace.width / 2.0;
            elements.push(CopperElement {
                min: [
                    trace.start.x.min(trace.end.x) - half_w,
                    trace.start.y.min(trace.end.y) - half_w,
                ],
                max: [
                    trace.start.x.max(trace.end.x) + half_w,
                    trace.start.y.max(trace.end.y) + half_w,
                ],
                net: trace.net.clone(),
                layer: trace.layer,
                geom: CopperGeom::Segment {
                    a: trace.start,
                    b: trace.end,
                    half_w,
                },
            });
        }

        // Index vias (on all copper layers they span)
        for via in &pcb.vias {
            let r = via.diameter / 2.0;
            // Vias appear on FCu and BCu at minimum
            for layer in [via.start_layer, via.end_layer] {
                elements.push(CopperElement {
                    min: [via.position.x - r, via.position.y - r],
                    max: [via.position.x + r, via.position.y + r],
                    net: via.net.clone(),
                    layer,
                    geom: CopperGeom::Disc {
                        center: via.position,
                        r,
                    },
                });
            }
        }

        // Index footprint pads
        for footprint in &pcb.footprints {
            for pad in &footprint.pads {
                let abs_x = footprint.position.x + pad.position.x;
                let abs_y = footprint.position.y + pad.position.y;
                let center = Vec2::new(abs_x, abs_y);
                let net = pad.net.clone().unwrap_or_default();
                // Total pad rotation = footprint rotation + pad-local rotation.
                let rot = (footprint.rotation + pad.rotation).to_radians();
                let (hw, hh) = pad_rotated_aabb_extents(pad, rot);
                let geom = pad_geom(pad, center, rot);

                for &layer in &pad.layers {
                    if layer.is_copper() {
                        elements.push(CopperElement {
                            min: [abs_x - hw, abs_y - hh],
                            max: [abs_x + hw, abs_y + hh],
                            net: net.clone(),
                            layer,
                            geom,
                        });
                    }
                }
            }
        }

        Self {
            tree: RTree::bulk_load(elements),
        }
    }

    /// Insert a copper element into the index.
    pub fn insert(&mut self, element: CopperElement) {
        self.tree.insert(element);
    }

    /// Query all elements whose bounding boxes intersect the given region.
    pub fn query_region(&self, min: [f64; 2], max: [f64; 2]) -> Vec<&CopperElement> {
        let envelope = AABB::from_corners(min, max);
        self.tree
            .locate_in_envelope_intersecting(&envelope)
            .collect()
    }

    /// Query the nearest element to a point.
    pub fn nearest(&self, point: [f64; 2]) -> Option<&CopperElement> {
        self.tree.nearest_neighbor(&point)
    }

    /// Returns the number of elements in the index.
    pub fn len(&self) -> usize {
        self.tree.size()
    }

    /// Returns true if the index is empty.
    pub fn is_empty(&self) -> bool {
        self.tree.size() == 0
    }
}

impl Default for SpatialIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the unrotated half-width and half-height of a pad for bounding box
/// computation.
pub(crate) fn pad_half_extents(pad: &vcad_ir::ecad::Pad) -> (f64, f64) {
    match &pad.shape {
        PadShape::Circle { diameter } => {
            let r = diameter / 2.0;
            (r, r)
        }
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => (width / 2.0, height / 2.0),
        PadShape::Custom { vertices } => {
            if vertices.is_empty() {
                return (0.0, 0.0);
            }
            let mut max_x: f64 = 0.0;
            let mut max_y: f64 = 0.0;
            for v in vertices {
                max_x = max_x.max(v.x.abs());
                max_y = max_y.max(v.y.abs());
            }
            (max_x, max_y)
        }
    }
}

/// Half-extents of a pad's axis-aligned bounding box after applying a rotation
/// (radians). Used so the R-tree broadphase still covers a rotated pad.
pub(crate) fn pad_rotated_aabb_extents(pad: &vcad_ir::ecad::Pad, rot: f64) -> (f64, f64) {
    let (hw, hh) = pad_half_extents(pad);
    // A disc-like pad (circle) is rotation-invariant.
    if matches!(pad.shape, PadShape::Circle { .. }) {
        return (hw, hh);
    }
    let (s, c) = rot.sin_cos();
    let ext_x = hw * c.abs() + hh * s.abs();
    let ext_y = hw * s.abs() + hh * c.abs();
    (ext_x, ext_y)
}

/// Build the narrowphase [`CopperGeom`] for a pad at an absolute `center` with
/// the given total rotation in radians.
pub(crate) fn pad_geom(pad: &Pad, center: Vec2, rot: f64) -> CopperGeom {
    match &pad.shape {
        PadShape::Circle { diameter } => CopperGeom::Disc {
            center,
            r: diameter / 2.0,
        },
        PadShape::Rect { width, height } | PadShape::RoundRect { width, height, .. } => {
            CopperGeom::Rect {
                center,
                half_w: width / 2.0,
                half_h: height / 2.0,
                rot,
            }
        }
        // Oval: if (near) square treat as disc, otherwise approximate as a
        // capsule via the rect distance — the rounded ends make rect a safe
        // (slightly conservative) bound. We model it as a rect here.
        PadShape::Oval { width, height } => CopperGeom::Rect {
            center,
            half_w: width / 2.0,
            half_h: height / 2.0,
            rot,
        },
        PadShape::Custom { .. } => {
            // Approximate a custom polygon by its (rotated) bounding rectangle.
            let (hw, hh) = pad_half_extents(pad);
            CopperGeom::Rect {
                center,
                half_w: hw,
                half_h: hh,
                rot,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;
    use vcad_ir::Vec2;

    #[test]
    fn empty_index() {
        let index = SpatialIndex::new();
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        let results = index.query_region([0.0, 0.0], [10.0, 10.0]);
        assert!(results.is_empty());
    }

    #[test]
    fn insert_and_query() {
        let mut index = SpatialIndex::new();
        index.insert(CopperElement {
            min: [5.0, 5.0],
            max: [10.0, 10.0],
            net: "VCC".to_string(),
            layer: PcbLayer::FCu,
            geom: CopperGeom::Rect {
                center: Vec2::new(7.5, 7.5),
                half_w: 2.5,
                half_h: 2.5,
                rot: 0.0,
            },
        });
        index.insert(CopperElement {
            min: [20.0, 20.0],
            max: [25.0, 25.0],
            net: "GND".to_string(),
            layer: PcbLayer::FCu,
            geom: CopperGeom::Rect {
                center: Vec2::new(22.5, 22.5),
                half_w: 2.5,
                half_h: 2.5,
                rot: 0.0,
            },
        });

        assert_eq!(index.len(), 2);

        // Query that hits the first element
        let results = index.query_region([0.0, 0.0], [12.0, 12.0]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].net, "VCC");

        // Query that hits both
        let results = index.query_region([0.0, 0.0], [30.0, 30.0]);
        assert_eq!(results.len(), 2);

        // Query that hits nothing
        let results = index.query_region([50.0, 50.0], [60.0, 60.0]);
        assert!(results.is_empty());
    }

    #[test]
    fn nearest_query() {
        let mut index = SpatialIndex::new();
        index.insert(CopperElement {
            min: [10.0, 10.0],
            max: [12.0, 12.0],
            net: "A".to_string(),
            layer: PcbLayer::FCu,
            geom: CopperGeom::Disc {
                center: Vec2::new(11.0, 11.0),
                r: 1.0,
            },
        });
        index.insert(CopperElement {
            min: [50.0, 50.0],
            max: [52.0, 52.0],
            net: "B".to_string(),
            layer: PcbLayer::FCu,
            geom: CopperGeom::Disc {
                center: Vec2::new(51.0, 51.0),
                r: 1.0,
            },
        });

        let nearest = index.nearest([11.0, 11.0]).unwrap();
        assert_eq!(nearest.net, "A");

        let nearest = index.nearest([49.0, 49.0]).unwrap();
        assert_eq!(nearest.net, "B");
    }

    #[test]
    fn from_pcb() {
        let pcb = Pcb {
            outline: BoardOutline {
                vertices: vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(100.0, 0.0),
                    Vec2::new(100.0, 80.0),
                    Vec2::new(0.0, 80.0),
                ],
                cutouts: vec![],
                thickness: 1.6,
            },
            stackup: LayerStackup {
                layers: vec![StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                }],
            },
            nets: vec![
                Net {
                    id: "1".to_string(),
                    name: "VCC".to_string(),
                },
                Net {
                    id: "2".to_string(),
                    name: "GND".to_string(),
                },
            ],
            rules: DesignRules {
                default_rules: NetClassRules {
                    name: "Default".to_string(),
                    trace_width: 0.25,
                    clearance: 0.2,
                    via_diameter: 0.8,
                    via_drill: 0.4,
                    diff_pair_gap: None,
                    diff_pair_width: None,
                },
                class_rules: vec![],
                net_class_assignments: std::collections::HashMap::new(),
                edge_clearance: 0.5,
                hole_to_hole: 0.5,
                min_annular_ring: 0.15,
                min_drill: 0.2,
            },
            footprints: vec![Footprint {
                reference: "R1".to_string(),
                value: "10k".to_string(),
                footprint_name: "R_0805".to_string(),
                position: Vec2::new(25.0, 40.0),
                rotation: 0.0,
                front: true,
                pads: vec![
                    Pad {
                        number: "1".to_string(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(-1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("1".to_string()),
                        layers: vec![PcbLayer::FCu],
                    },
                    Pad {
                        number: "2".to_string(),
                        pad_type: PadType::SMD,
                        shape: PadShape::Rect {
                            width: 1.0,
                            height: 1.2,
                        },
                        position: Vec2::new(1.0, 0.0),
                        rotation: 0.0,
                        drill: None,
                        net: Some("2".to_string()),
                        layers: vec![PcbLayer::FCu],
                    },
                ],
                graphics: vec![],
                model_3d: None,
                properties: std::collections::HashMap::new(),
            }],
            traces: vec![Trace {
                start: Vec2::new(24.0, 40.0),
                end: Vec2::new(10.0, 40.0),
                width: 0.25,
                layer: PcbLayer::FCu,
                net: "1".to_string(),
            }],
            trace_arcs: vec![],
            vias: vec![Via {
                position: Vec2::new(10.0, 40.0),
                diameter: 0.8,
                drill: 0.4,
                start_layer: PcbLayer::FCu,
                end_layer: PcbLayer::BCu,
                net: "1".to_string(),
            }],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        };

        let index = SpatialIndex::from_pcb(&pcb);
        // 1 trace + 2 via entries (FCu + BCu) + 2 pads = 5
        assert_eq!(index.len(), 5);

        // Query around the resistor area
        let results = index.query_region([22.0, 38.0], [28.0, 42.0]);
        assert!(!results.is_empty());
    }

    #[test]
    fn default_index() {
        let index = SpatialIndex::default();
        assert!(index.is_empty());
    }

    // ------------------------------------------------------------------
    // Narrowphase geometry
    // ------------------------------------------------------------------

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn disc_disc_distance() {
        let a = CopperGeom::Disc {
            center: Vec2::new(0.0, 0.0),
            r: 1.0,
        };
        let b = CopperGeom::Disc {
            center: Vec2::new(5.0, 0.0),
            r: 1.0,
        };
        // 5 center - 1 - 1 = 3.
        assert!(approx(a.distance_to(&b), 3.0));
        // Overlapping discs → 0.
        let c = CopperGeom::Disc {
            center: Vec2::new(1.0, 0.0),
            r: 1.0,
        };
        assert!(approx(a.distance_to(&c), 0.0));
    }

    #[test]
    fn segment_segment_capsule_distance() {
        // Two parallel horizontal traces, width 0.25 (half_w 0.125), centerline
        // gap 0.35 → edge-to-edge 0.1.
        let a = CopperGeom::Segment {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(10.0, 0.0),
            half_w: 0.125,
        };
        let b = CopperGeom::Segment {
            a: Vec2::new(0.0, 0.35),
            b: Vec2::new(10.0, 0.35),
            half_w: 0.125,
        };
        assert!(approx(a.distance_to(&b), 0.1));
    }

    #[test]
    fn crossing_segments_distance_zero() {
        let a = CopperGeom::Segment {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(10.0, 10.0),
            half_w: 0.1,
        };
        let b = CopperGeom::Segment {
            a: Vec2::new(0.0, 10.0),
            b: Vec2::new(10.0, 0.0),
            half_w: 0.1,
        };
        assert!(approx(a.distance_to(&b), 0.0));
    }

    #[test]
    fn segment_disc_distance() {
        let seg = CopperGeom::Segment {
            a: Vec2::new(0.0, 0.0),
            b: Vec2::new(10.0, 0.0),
            half_w: 0.0,
        };
        let disc = CopperGeom::Disc {
            center: Vec2::new(5.0, 3.0),
            r: 1.0,
        };
        // Perpendicular distance 3, minus disc radius 1 = 2.
        assert!(approx(seg.distance_to(&disc), 2.0));
    }

    #[test]
    fn rotated_rect_distance() {
        // A 2x2 square rotated 45° centered at origin has corners at
        // (±sqrt(2), 0) and (0, ±sqrt(2)). A disc to the right at x=5 should
        // measure from the (sqrt(2),0) corner.
        let rect = CopperGeom::Rect {
            center: Vec2::new(0.0, 0.0),
            half_w: 1.0,
            half_h: 1.0,
            rot: std::f64::consts::FRAC_PI_4,
        };
        let disc = CopperGeom::Disc {
            center: Vec2::new(5.0, 0.0),
            r: 0.0,
        };
        let expected = 5.0 - std::f64::consts::SQRT_2;
        assert!(approx(rect.distance_to(&disc), expected));
    }

    #[test]
    fn point_in_polygon_basic() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        assert!(point_in_polygon(Vec2::new(5.0, 5.0), &square));
        assert!(!point_in_polygon(Vec2::new(15.0, 5.0), &square));
    }

    #[test]
    fn segment_polygon_intersect_basic() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        // Crosses through.
        assert!(segment_polygon_intersects(
            Vec2::new(-5.0, 5.0),
            Vec2::new(15.0, 5.0),
            &square
        ));
        // Entirely outside, no crossing.
        assert!(!segment_polygon_intersects(
            Vec2::new(-5.0, -5.0),
            Vec2::new(-1.0, -1.0),
            &square
        ));
    }

    #[test]
    fn pad_rotated_aabb_grows() {
        let pad = Pad {
            number: "1".to_string(),
            pad_type: vcad_ir::ecad::PadType::SMD,
            shape: PadShape::Rect {
                width: 4.0,
                height: 1.0,
            },
            position: Vec2::new(0.0, 0.0),
            rotation: 0.0,
            drill: None,
            net: None,
            layers: vec![PcbLayer::FCu],
        };
        let (hw0, hh0) = pad_rotated_aabb_extents(&pad, 0.0);
        assert!(approx(hw0, 2.0) && approx(hh0, 0.5));
        // Rotated 90°, width and height swap.
        let (hw90, hh90) = pad_rotated_aabb_extents(&pad, std::f64::consts::FRAC_PI_2);
        assert!(approx(hw90, 0.5) && approx(hh90, 2.0));
    }
}
