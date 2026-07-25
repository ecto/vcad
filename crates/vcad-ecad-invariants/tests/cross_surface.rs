//! Every surface must see the same physical pad.
//!
//! For each pad in the rotated-footprint corpus, the world-space rectangle is
//! compared between:
//!
//! - `geometry::pad_world_position` + the composed shape rotation (truth),
//! - the DRC's connectivity nodes,
//! - the router's spatial index,
//! - the Gerber aperture and flash,
//! - the SVG renderer.
//!
//! Tolerance is one micrometre. These are all closed-form transforms of the
//! same numbers — a disagreement is a different formula, which is exactly what
//! shipped twice on 2026-07-25.

mod common;

use common::gerber::parse_flashes;

use vcad_ecad_invariants::{corpus, pad_indices, PadRect, TOL_MM};
use vcad_ecad_pcb::spatial::{CopperGeom, SpatialIndex};
use vcad_ir::ecad::PcbLayer;
use vcad_ir::Vec2;

/// Convert a narrowphase copper geometry back into a pad rectangle.
fn rect_of_geom(g: &CopperGeom) -> PadRect {
    match *g {
        CopperGeom::Rect {
            center,
            half_w,
            half_h,
            rot,
        } => PadRect {
            center,
            half_w,
            half_h,
            rot_deg: rot.to_degrees(),
            is_round: false,
        },
        CopperGeom::Disc { center, r } => PadRect {
            center,
            half_w: r,
            half_h: r,
            rot_deg: 0.0,
            is_round: true,
        },
        CopperGeom::Segment { .. } => panic!("a pad must not be a segment"),
    }
}

fn assert_agrees(case: &str, surface: &str, key: &str, truth: &PadRect, got: &PadRect) {
    let dev = truth.max_deviation(got);
    assert!(
        dev <= TOL_MM,
        "{case}: {surface} disagrees with pad_world_position for pad {key} by {dev:.6}mm\n\
         truth: {truth:?}\n  {surface}: {got:?}"
    );
}

#[test]
fn drc_connectivity_nodes_agree() {
    for b in corpus() {
        let nodes = vcad_ecad_pcb::drc::connectivity_pad_geoms(&b.pcb);
        for (i, j) in pad_indices(&b.pcb) {
            let fp = &b.pcb.footprints[i];
            let pad = &fp.pads[j];
            let key = (fp.reference.clone(), pad.number.clone());
            let (_, geom) = nodes
                .iter()
                .find(|(k, _)| *k == key)
                .unwrap_or_else(|| panic!("{}: DRC lost pad {key:?}", b.name));
            assert_agrees(
                &b.name,
                "DRC connectivity",
                &pad.number,
                &PadRect::of(fp, pad),
                &rect_of_geom(geom),
            );
        }
    }
}

#[test]
fn router_spatial_index_agrees() {
    for b in corpus() {
        let index = SpatialIndex::from_pcb(&b.pcb);
        let elements = index.query_region([-1e3, -1e3], [1e3, 1e3]);
        for (i, j) in pad_indices(&b.pcb) {
            let fp = &b.pcb.footprints[i];
            let pad = &fp.pads[j];
            // Corpus nets are unique per pad, so the net names the element.
            let net = pad.net.as_deref().unwrap();
            let el = elements
                .iter()
                .find(|e| e.net == net)
                .unwrap_or_else(|| panic!("{}: spatial index lost pad {net}", b.name));
            let truth = PadRect::of(fp, pad);
            assert_agrees(
                &b.name,
                "spatial index",
                &pad.number,
                &truth,
                &rect_of_geom(&el.geom),
            );
            // The broadphase AABB must still contain the true copper, or the
            // R-tree filters out pairs the narrowphase would have caught.
            let hull: Vec<Vec2> = if truth.is_round {
                vec![
                    Vec2::new(truth.center.x - truth.half_w, truth.center.y - truth.half_w),
                    Vec2::new(truth.center.x + truth.half_w, truth.center.y + truth.half_w),
                ]
            } else {
                truth.corners().to_vec()
            };
            for c in hull {
                assert!(
                    c.x >= el.min[0] - TOL_MM
                        && c.x <= el.max[0] + TOL_MM
                        && c.y >= el.min[1] - TOL_MM
                        && c.y <= el.max[1] + TOL_MM,
                    "{}: broadphase AABB does not cover rotated pad {net}",
                    b.name
                );
            }
        }
    }
}

#[test]
fn gerber_apertures_agree() {
    for b in corpus() {
        let files = vcad_ecad_export::gerber::generate_gerbers(&b.pcb).expect("gerbers");
        for (i, j) in pad_indices(&b.pcb) {
            let fp = &b.pcb.footprints[i];
            let pad = &fp.pads[j];
            let layer = pad.layers[0];
            let name = match layer {
                PcbLayer::FCu => "F_Cu.gbr",
                PcbLayer::BCu => "B_Cu.gbr",
                other => panic!("{}: corpus pad on unexpected layer {other:?}", b.name),
            }
            .to_string();
            assert!(
                files.contains_key(&name),
                "{}: no copper gerber {name}",
                b.name
            );

            let truth = PadRect::of(fp, pad);
            let flashes = parse_flashes(&files[&name]);
            // Find the flash at this pad's centre; its aperture must describe
            // the same rectangle.
            let hit = flashes
                .iter()
                .map(|f| f.rect())
                .find(|r| {
                    (r.center.x - truth.center.x).abs() < 1e-4
                        && (r.center.y - truth.center.y).abs() < 1e-4
                })
                .unwrap_or_else(|| {
                    panic!(
                        "{}: no Gerber flash at pad {} centre {:?}",
                        b.name, pad.number, truth.center
                    )
                });
            // Gerber coordinates are quantised to nanometres, so allow that on
            // top of the agreement tolerance.
            let dev = truth.max_deviation(&hit);
            assert!(
                dev <= 1e-5,
                "{}: Gerber aperture disagrees for pad {} by {dev:.7}mm\n  truth: {truth:?}\n  gerber: {hit:?}",
                b.name,
                pad.number
            );
        }
    }
}

#[test]
fn svg_renderer_agrees() {
    for b in corpus() {
        for (i, j) in pad_indices(&b.pcb) {
            let fp = &b.pcb.footprints[i];
            let pad = &fp.pads[j];
            let truth = PadRect::of(fp, pad);
            let Some(quad) = vcad_render::pcb::pad_world_quad(fp, pad) else {
                // Circles: only the centre is meaningful.
                let (origin, _) = vcad_render::pcb::pad_placement(fp, pad);
                assert!(
                    (origin.x - truth.center.x).abs() <= TOL_MM
                        && (origin.y - truth.center.y).abs() <= TOL_MM,
                    "{}: renderer centre disagrees for round pad {}",
                    b.name,
                    pad.number
                );
                continue;
            };
            let mut got = quad.to_vec();
            let mut want = truth.corners().to_vec();
            let by_xy = |a: &Vec2, b: &Vec2| {
                a.x.partial_cmp(&b.x)
                    .unwrap()
                    .then(a.y.partial_cmp(&b.y).unwrap())
            };
            got.sort_by(by_xy);
            want.sort_by(by_xy);
            for (g, w) in got.iter().zip(want.iter()) {
                assert!(
                    (g.x - w.x).abs() <= TOL_MM && (g.y - w.y).abs() <= TOL_MM,
                    "{}: SVG renderer corner disagrees for pad {}: {g:?} vs {w:?}",
                    b.name,
                    pad.number
                );
            }
        }
    }
}

/// The renderer's public quad is only worth asserting on if the SVG really is
/// drawn from it. Pin the emitted geometry itself.
///
/// The SVG transform is a uniform scale, a Y flip and a translation, so the
/// polygon's *shape* survives it exactly: side lengths keep their ratio, and
/// the long axis lands at `-rot` in screen space. Both are transform-free, so
/// this asserts on the real output without reconstructing the viewport.
#[test]
fn svg_output_carries_the_pad_rotation() {
    for b in corpus() {
        let svg = vcad_render::pcb::render_pcb_svg(&b.pcb, &[PcbLayer::FCu, PcbLayer::BCu], 20.0);
        let polys: Vec<Vec<Vec2>> = svg
            .split("points=\"")
            .skip(1)
            .filter_map(|s| s.split_once('"').map(|(p, _)| p))
            .map(|p| {
                p.split_whitespace()
                    .filter_map(|v| v.split_once(','))
                    .map(|(x, y)| Vec2::new(x.parse().unwrap(), y.parse().unwrap()))
                    .collect()
            })
            .filter(|v: &Vec<Vec2>| v.len() == 4)
            .collect();

        for (i, j) in pad_indices(&b.pcb) {
            let fp = &b.pcb.footprints[i];
            let pad = &fp.pads[j];
            let truth = PadRect::of(fp, pad);
            if truth.is_round || (truth.half_w - truth.half_h).abs() < 1e-9 {
                continue; // no distinguishable long axis
            }
            // Oval pads are drawn as a round-capped stroke, not a polygon.
            if matches!(pad.shape, vcad_ir::ecad::PadShape::Oval { .. }) {
                continue;
            }
            let want_ratio = truth.half_w / truth.half_h;
            // Screen space flips Y, so a board-space angle t appears at -t.
            let want_deg = norm180(-truth.rot_deg);

            let found = polys.iter().any(|p| {
                let e0 = (p[1].x - p[0].x, p[1].y - p[0].y);
                let e1 = (p[2].x - p[1].x, p[2].y - p[1].y);
                let l0 = (e0.0 * e0.0 + e0.1 * e0.1).sqrt();
                let l1 = (e1.0 * e1.0 + e1.1 * e1.1).sqrt();
                if l0 <= 0.0 || l1 <= 0.0 {
                    return false;
                }
                let ratio = l0 / l1;
                if (ratio - want_ratio).abs() > 1e-3 * want_ratio.max(1.0) {
                    return false;
                }
                let deg = norm180(e0.1.atan2(e0.0).to_degrees());
                angle_close(deg, want_deg)
            });
            assert!(
                found,
                "{}: no SVG polygon carries pad {}'s {:.1}deg orientation and {want_ratio:.3} aspect",
                b.name, pad.number, truth.rot_deg
            );
        }
    }
}

fn norm180(d: f64) -> f64 {
    let r = d % 180.0;
    if r < 0.0 {
        r + 180.0
    } else {
        r
    }
}

fn angle_close(a: f64, b: f64) -> bool {
    let d = (a - b).abs() % 180.0;
    d.min(180.0 - d) < 0.05
}
