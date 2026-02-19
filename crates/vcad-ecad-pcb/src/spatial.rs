//! R-tree spatial index for PCB copper elements.
//!
//! Wraps the [`rstar`] crate to provide efficient spatial queries over
//! copper elements (traces, pads, vias) on a PCB. Used by the DRC engine
//! and copper pour algorithm for proximity checks.

use rstar::{RTree, RTreeObject, AABB};

use vcad_ir::ecad::{PadShape, Pcb, PcbLayer};

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
}

impl RTreeObject for CopperElement {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.min, self.max)
    }
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
                });
            }
        }

        // Index footprint pads
        for footprint in &pcb.footprints {
            for pad in &footprint.pads {
                let (hw, hh) = pad_half_extents(pad);
                let abs_x = footprint.position.x + pad.position.x;
                let abs_y = footprint.position.y + pad.position.y;
                let net = pad.net.clone().unwrap_or_default();

                for &layer in &pad.layers {
                    if layer.is_copper() {
                        elements.push(CopperElement {
                            min: [abs_x - hw, abs_y - hh],
                            max: [abs_x + hw, abs_y + hh],
                            net: net.clone(),
                            layer,
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

/// Get the half-width and half-height of a pad for bounding box computation.
fn pad_half_extents(pad: &vcad_ir::ecad::Pad) -> (f64, f64) {
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
        });
        index.insert(CopperElement {
            min: [20.0, 20.0],
            max: [25.0, 25.0],
            net: "GND".to_string(),
            layer: PcbLayer::FCu,
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
        });
        index.insert(CopperElement {
            min: [50.0, 50.0],
            max: [52.0, 52.0],
            net: "B".to_string(),
            layer: PcbLayer::FCu,
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
}
