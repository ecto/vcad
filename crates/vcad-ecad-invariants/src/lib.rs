//! Cross-surface geometry invariants for the ECAD pipeline.
//!
//! A pad is one physical rectangle of copper. Every stage of the pipeline —
//! DRC connectivity, the router's spatial index, the Gerber writer, the SVG
//! renderer, the KiCad reader/writer — has its own opinion about where that
//! rectangle is, and until this crate existed nothing asserted they agreed.
//!
//! Two shipped bugs came out of that gap, both on 2026-07-25:
//!
//! 1. The KiCad importer stored pad angles **absolutely** while every geometry
//!    consumer composes `fp.rotation + pad.rotation`, double-counting the
//!    footprint rotation. A TQFN's 0.25 x 0.875mm pads at 0.5mm pitch came out
//!    turned 90 degrees, so neighbouring pads OVERLAPPED — 648 phantom DRC
//!    violations on the CM5 fixture.
//! 2. The Gerber exporter ignored pad rotation entirely — 828 CM5 pads emitted
//!    in the wrong orientation, on the files that actually get fabricated.
//!
//! This crate provides the [`corpus`] of small synthetic boards that exercise
//! rotation, and [`PadRect`] — the canonical world-space rectangle a pad
//! occupies, derived from [`vcad_ecad_pcb::geometry::pad_world_position`] plus
//! the composed shape rotation. The tests in `tests/` assert every surface
//! reproduces it to micrometres.

#![warn(missing_docs)]

use std::collections::HashMap;

use vcad_ir::ecad::{
    BoardOutline, DesignRules, Footprint, LayerStackup, Net, NetClassRules, Pad, PadShape, PadType,
    Pcb, PcbLayer, StackupLayer,
};
use vcad_ir::Vec2;

/// Agreement tolerance between surfaces, in mm (one micrometre).
///
/// Tight on purpose: these are all closed-form transforms of the same numbers,
/// so any real disagreement is a *different formula*, not accumulated error.
pub const TOL_MM: f64 = 1e-6;

// ---------------------------------------------------------------------------
// The canonical pad rectangle
// ---------------------------------------------------------------------------

/// The world-space oriented rectangle a pad occupies.
///
/// This is the source of truth every other surface is measured against:
/// centre from [`vcad_ecad_pcb::geometry::pad_world_position`], extents from
/// the pad shape, angle from `fp.rotation + pad.rotation`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PadRect {
    /// Board-space centre (mm).
    pub center: Vec2,
    /// Half-extent along the pad's local X axis (mm).
    pub half_w: f64,
    /// Half-extent along the pad's local Y axis (mm).
    pub half_h: f64,
    /// Total rotation, degrees CCW (`fp.rotation + pad.rotation`).
    pub rot_deg: f64,
    /// True when the shape is a disc, and therefore rotation-invariant.
    pub is_round: bool,
}

impl PadRect {
    /// The canonical rectangle for `pad` on `fp`.
    pub fn of(fp: &Footprint, pad: &Pad) -> Self {
        let center = vcad_ecad_pcb::geometry::pad_world_position(fp, pad);
        let (w, h) = vcad_ecad_pcb::geometry::pad_dimensions(&pad.shape);
        PadRect {
            center,
            half_w: w / 2.0,
            half_h: h / 2.0,
            rot_deg: fp.rotation + pad.rotation,
            is_round: matches!(pad.shape, PadShape::Circle { .. }),
        }
    }

    /// The four corners, counter-clockwise from the local `(-w, -h)` corner.
    pub fn corners(&self) -> [Vec2; 4] {
        let (s, c) = self.rot_deg.to_radians().sin_cos();
        let place = |dx: f64, dy: f64| {
            Vec2::new(
                self.center.x + dx * c - dy * s,
                self.center.y + dx * s + dy * c,
            )
        };
        [
            place(-self.half_w, -self.half_h),
            place(self.half_w, -self.half_h),
            place(self.half_w, self.half_h),
            place(-self.half_w, self.half_h),
        ]
    }

    /// Corners as an unordered, rotation-canonical set: sorted lexicographically
    /// so two surfaces that wind the rectangle differently still compare equal.
    pub fn corner_set(&self) -> Vec<Vec2> {
        let mut v = self.corners().to_vec();
        v.sort_by(|a, b| {
            a.x.partial_cmp(&b.x)
                .unwrap()
                .then(a.y.partial_cmp(&b.y).unwrap())
        });
        v
    }

    /// Largest corner-to-corner deviation from `other` (mm), comparing the
    /// rectangles as point sets. Round pads compare centre and radius instead,
    /// since their corners are meaningless.
    pub fn max_deviation(&self, other: &PadRect) -> f64 {
        if self.is_round && other.is_round {
            return (self.center.x - other.center.x)
                .abs()
                .max((self.center.y - other.center.y).abs())
                .max((self.half_w - other.half_w).abs());
        }
        let (a, b) = (self.corner_set(), other.corner_set());
        a.iter()
            .zip(b.iter())
            .map(|(p, q)| (p.x - q.x).abs().max((p.y - q.y).abs()))
            .fold(0.0f64, f64::max)
    }

    /// Whether this rectangle overlaps `other`, by the separating-axis test on
    /// the two rectangles' four edge normals.
    pub fn overlaps(&self, other: &PadRect) -> bool {
        let (a, b) = (self.corners(), other.corners());
        for quad in [&a, &b] {
            // Two distinct edge directions per rectangle.
            for i in 0..2 {
                let e = Vec2::new(quad[i + 1].x - quad[i].x, quad[i + 1].y - quad[i].y);
                let axis = Vec2::new(-e.y, e.x);
                let proj = |q: &[Vec2; 4]| {
                    let mut lo = f64::INFINITY;
                    let mut hi = f64::NEG_INFINITY;
                    for p in q {
                        let d = p.x * axis.x + p.y * axis.y;
                        lo = lo.min(d);
                        hi = hi.max(d);
                    }
                    (lo, hi)
                };
                let (alo, ahi) = proj(&a);
                let (blo, bhi) = proj(&b);
                // A shared edge is not an overlap; require real interior
                // interpenetration.
                if ahi <= blo + TOL_MM || bhi <= alo + TOL_MM {
                    return false;
                }
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// A named board in the rotation corpus.
pub struct CorpusBoard {
    /// Human-readable case name, used in assertion messages.
    pub name: String,
    /// The board.
    pub pcb: Pcb,
}

/// Rotations every corpus part is placed at.
pub const ROTATIONS: [f64; 6] = [0.0, 45.0, 90.0, 135.0, 180.0, 270.0];

/// The rotated-footprint corpus: small synthetic boards with non-square pads
/// (rect, roundrect, oval) on footprints at every angle in [`ROTATIONS`], on
/// both board sides, plus a fine-pitch part whose pads overlap if the rotation
/// is wrong.
pub fn corpus() -> Vec<CorpusBoard> {
    let mut out = Vec::new();

    for &rot in &ROTATIONS {
        for &front in &[true, false] {
            let side = if front { "front" } else { "back" };
            out.push(CorpusBoard {
                name: format!("shapes@{rot}deg/{side}"),
                pcb: board(vec![shapes_footprint(rot, front)]),
            });
            out.push(CorpusBoard {
                name: format!("tqfn@{rot}deg/{side}"),
                pcb: board(vec![fine_pitch_tqfn(rot, front)]),
            });
        }
        // A part whose pads carry their own local rotation on top of the
        // footprint's, so a surface that reads only one of the two angles is
        // caught even when the other is zero.
        out.push(CorpusBoard {
            name: format!("padlocal@{rot}deg"),
            pcb: board(vec![pad_local_rotation_footprint(rot)]),
        });
    }

    out
}

/// Every `(footprint, pad)` pair in a board, as `(fp_index, pad_index)`.
pub fn pad_indices(pcb: &Pcb) -> Vec<(usize, usize)> {
    pcb.footprints
        .iter()
        .enumerate()
        .flat_map(|(i, fp)| (0..fp.pads.len()).map(move |j| (i, j)))
        .collect()
}

/// A part carrying one of each non-square shape, deliberately elongated so a
/// wrong angle is a visibly different rectangle.
fn shapes_footprint(rot: f64, front: bool) -> Footprint {
    let layer = if front { PcbLayer::FCu } else { PcbLayer::BCu };
    Footprint {
        reference: "U1".to_string(),
        value: "SHAPES".to_string(),
        footprint_name: "ROT_SHAPES".to_string(),
        position: Vec2::new(20.0, 15.0),
        rotation: rot,
        front,
        pads: vec![
            pad("1", PadShape::Rect { width: 2.4, height: 0.6 }, Vec2::new(-3.0, 0.0), 0.0, layer, "N1"),
            pad(
                "2",
                PadShape::RoundRect { width: 2.4, height: 0.6, corner_ratio: 0.25 },
                Vec2::new(0.0, 0.0),
                0.0,
                layer,
                "N2",
            ),
            pad("3", PadShape::Oval { width: 2.4, height: 0.6 }, Vec2::new(3.0, 0.0), 0.0, layer, "N3"),
            // A disc, which must stay rotation-invariant.
            pad("4", PadShape::Circle { diameter: 1.0 }, Vec2::new(0.0, 2.5), 0.0, layer, "N4"),
        ],
        graphics: vec![],
        model_3d: None,
        properties: HashMap::new(),
    }
}

/// A part where the pads carry a local rotation of their own, on top of the
/// footprint's. Catches a surface that reads `fp.rotation` or `pad.rotation`
/// alone rather than composing both.
fn pad_local_rotation_footprint(rot: f64) -> Footprint {
    Footprint {
        reference: "U2".to_string(),
        value: "PADROT".to_string(),
        footprint_name: "ROT_PADLOCAL".to_string(),
        position: Vec2::new(8.0, 30.0),
        rotation: rot,
        front: true,
        pads: vec![
            pad("1", PadShape::Rect { width: 2.0, height: 0.5 }, Vec2::new(-2.0, 0.0), 30.0, PcbLayer::FCu, "P1"),
            pad("2", PadShape::Oval { width: 2.0, height: 0.5 }, Vec2::new(2.0, 0.0), -60.0, PcbLayer::FCu, "P2"),
        ],
        graphics: vec![],
        model_3d: None,
        properties: HashMap::new(),
    }
}

/// The case with teeth: a 0.5mm-pitch TQFN edge with 0.25 x 0.875mm pads, the
/// exact geometry the KiCad double-count bug turned 90 degrees.
///
/// Correctly oriented, the pads' long axis runs *across* the pitch and
/// neighbours clear each other by 0.25mm. Turn them 90 degrees and the
/// 0.875mm long axis lies *along* a 0.5mm pitch — neighbours overlap by
/// 0.375mm. A wrong answer here is a DRC violation, not a different number.
pub fn fine_pitch_tqfn(rot: f64, front: bool) -> Footprint {
    let layer = if front { PcbLayer::FCu } else { PcbLayer::BCu };
    let pitch = 0.5;
    let pads = (0..6)
        .map(|i| {
            let y = (i as f64 - 2.5) * pitch;
            pad(
                &format!("{}", i + 1),
                PadShape::Rect { width: 0.875, height: 0.25 },
                Vec2::new(-2.0, y),
                0.0,
                layer,
                &format!("Q{}", i + 1),
            )
        })
        .collect();
    Footprint {
        reference: "U3".to_string(),
        value: "TQFN".to_string(),
        footprint_name: "TQFN-24_0.5mm".to_string(),
        position: Vec2::new(30.0, 30.0),
        rotation: rot,
        front,
        pads,
        graphics: vec![],
        model_3d: None,
        properties: HashMap::new(),
    }
}

fn pad(
    number: &str,
    shape: PadShape,
    position: Vec2,
    rotation: f64,
    layer: PcbLayer,
    net: &str,
) -> Pad {
    Pad {
        number: number.to_string(),
        pad_type: PadType::SMD,
        shape,
        position,
        rotation,
        drill: None,
        net: Some(net.to_string()),
        layers: vec![layer],
    }
}

/// A minimal 2-layer board around the given footprints.
pub fn board(footprints: Vec<Footprint>) -> Pcb {
    let nets: Vec<Net> = footprints
        .iter()
        .flat_map(|f| f.pads.iter())
        .filter_map(|p| p.net.clone())
        .map(|n| Net {
            id: n.clone(),
            name: n,
        })
        .collect();
    Pcb {
        outline: BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(50.0, 0.0),
                Vec2::new(50.0, 50.0),
                Vec2::new(0.0, 50.0),
            ],
            cutouts: vec![],
            thickness: 1.6,
        },
        stackup: LayerStackup {
            layers: vec![
                StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.53),
                    dielectric_er: Some(4.5),
                    material: None,
                },
                StackupLayer {
                    layer: PcbLayer::BCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: None,
                    dielectric_er: None,
                    material: None,
                },
            ],
        },
        nets,
        rules: DesignRules {
            default_rules: NetClassRules {
                name: "Default".to_string(),
                trace_width: 0.2,
                clearance: 0.2,
                via_diameter: 0.6,
                via_drill: 0.3,
                diff_pair_gap: None,
                diff_pair_width: None,
            },
            class_rules: vec![],
            net_class_assignments: HashMap::new(),
            edge_clearance: 0.5,
            hole_to_hole: 0.5,
            min_annular_ring: 0.13,
            min_drill: 0.2,
        },
        footprints,
        traces: vec![],
        trace_arcs: vec![],
        vias: vec![],
        zones: vec![],
        keepouts: vec![],
        net_ties: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corpus is only worth anything if the fine-pitch case really does
    /// overlap when turned. Pin both directions so the fixture can't silently
    /// lose its teeth.
    #[test]
    fn fine_pitch_pads_clear_correctly_and_overlap_when_turned() {
        let fp = fine_pitch_tqfn(0.0, true);
        let rects: Vec<PadRect> = fp.pads.iter().map(|p| PadRect::of(&fp, p)).collect();
        for w in rects.windows(2) {
            assert!(
                !w[0].overlaps(&w[1]),
                "correctly oriented TQFN pads must clear each other"
            );
        }

        // Now the bug: every pad turned by the footprint angle a second time.
        let mut turned = fine_pitch_tqfn(90.0, true);
        for p in &mut turned.pads {
            p.rotation += 90.0;
        }
        let bad: Vec<PadRect> = turned.pads.iter().map(|p| PadRect::of(&turned, p)).collect();
        assert!(
            bad.windows(2).any(|w| w[0].overlaps(&w[1])),
            "double-counted rotation must make neighbouring TQFN pads overlap — \
             otherwise this corpus case has no teeth"
        );
    }

    #[test]
    fn corpus_covers_every_rotation_and_both_sides() {
        let c = corpus();
        for &r in &ROTATIONS {
            assert!(c.iter().any(|b| b.name.contains(&format!("@{r}deg"))));
        }
        assert!(c.iter().any(|b| b.name.ends_with("/back")));
        assert!(!c.is_empty());
    }
}
