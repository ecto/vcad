//! Folded sheet-metal solid construction with true cylindrical bend faces.
//!
//! Turns a [`SheetMetalModel`] (flat panels + bend metadata) into a single
//! watertight B-rep [`Solid`] of the **folded** part, suitable for STEP
//! AP214 export. Each bend is realised as an analytic cylindrical sector
//! (inner radius `R`, outer radius `R + t`) tangent to the two panel slabs
//! it joins, so downstream 3D pipelines (e.g. SendCutSend) can detect bend
//! radii, angles, and directions from the cylindrical faces directly.
//!
//! # Construction
//!
//! The model stores a zero-radius idealisation: parent and child mid-planes
//! both pass through the hinge line. The flat pattern, however, keeps every
//! panel outline at full size and inserts the bend allowance *between*
//! panels (see `vcad_kernel_sheet::unfold`). To stay consistent with that
//! flat pattern — the thing a fab actually cuts — the folded solid keeps
//! panels at full size too and inserts a real cylindrical bend zone between
//! them:
//!
//! 1. Each panel (with its holes) is extruded by the sheet thickness,
//!    centred on its mid-plane, and posed via `frame_bent`.
//! 2. Each bend becomes an annular-sector profile (two analytic arcs,
//!    mid-surface radius `ρ = R + t/2`) extruded along the hinge axis. The
//!    sector is tangent to the parent mid-plane exactly at the hinge edge,
//!    sweeps the bend angle, and the child subtree is translated so the
//!    child's hinge edge lands on the far tangent line.
//! 3. The sector is oversized by a small epsilon (radially and angularly)
//!    so boolean unions see clean transversal overlaps instead of exact
//!    tangencies.
//! 4. Everything is unioned into one solid.

use vcad_kernel_math::{Point2, Point3, Transform, Vec3};
use vcad_kernel_sheet::model::{Bend, Frame, SheetMetalModel};
use vcad_kernel_sheet::poly2d::{self, Poly};
use vcad_kernel_sketch::{SketchProfile, SketchSegment};

use crate::Solid;

/// Overlap epsilon (mm): bend sectors are oversized radially and angularly
/// by this much so unions operate on proper volume overlaps, never exact
/// tangency. Also the size of the resulting cosmetic micro-lip along each
/// bend/panel junction.
const EPS_OVERLAP: f64 = 0.02;

/// Largest supported bend angle (radians). As θ → π the two panel planes
/// become parallel and the tangent construction degenerates, so hems
/// (180° folds) need a different construction and are rejected.
const MAX_BEND_ANGLE: f64 = 2.96; // ≈ 169.6°

/// Build the folded sheet-metal solid for `model`.
///
/// `segments` is a tessellation-density hint used only for the final
/// validation mesh — all bend faces are analytic cylinders regardless, and
/// STEP output is unaffected.
///
/// Returns an error when the model is empty, a bend angle is out of range
/// (hems are not supported), or a boolean union fails to produce an
/// exportable B-rep.
pub fn folded_sheet_solid(model: &SheetMetalModel, segments: u32) -> Result<Solid, String> {
    if model.panels.is_empty() {
        return Err("sheet-metal model has no panels".to_string());
    }
    let t = model.thickness;
    if t <= 0.0 || t.is_nan() {
        return Err(format!("invalid sheet thickness {t}"));
    }
    for (i, bend) in model.bends.iter().enumerate() {
        if bend.angle.is_nan() || bend.angle <= 1e-6 {
            return Err(format!("bend #{i}: angle {} too small", bend.angle));
        }
        if bend.angle > MAX_BEND_ANGLE {
            return Err(format!(
                "bend #{i}: angle {:.4} rad too close to π — hems / closed folds are not \
                 supported by the folded-solid builder yet",
                bend.angle
            ));
        }
        if bend.radius.is_nan() || bend.radius <= 0.0 {
            return Err(format!("bend #{i}: non-positive radius {}", bend.radius));
        }
    }

    // ---- 1. Per-bend geometry (in nominal frame_bent coordinates) ----
    let mut bend_geos: Vec<BendGeo> = Vec::with_capacity(model.bends.len());
    for (bid, bend) in model.bends.iter().enumerate() {
        bend_geos.push(bend_geometry(model, bend).map_err(|e| format!("bend #{bid}: {e}"))?);
    }

    // ---- 2. Accumulate bend-zone translations down the panel tree ----
    // Each bend inserts a cylindrical zone between its panels: the child
    // subtree shifts by the vector taking the (zero-width) hinge to the
    // child-side tangent line. Offsets accumulate root → leaf.
    let mut offsets: Vec<Option<Vec3>> = vec![None; model.panels.len()];
    offsets[model.root] = Some(Vec3::new(0.0, 0.0, 0.0));
    for (pid, via) in model.bfs() {
        let Some(bid) = via else { continue };
        let bend = &model.bends[bid];
        let geo = &bend_geos[bid];
        if bend.child == pid {
            let parent_off = offsets[bend.parent]
                .ok_or_else(|| format!("panel #{pid}: parent visited out of order"))?;
            offsets[pid] = Some(parent_off + geo.child_shift);
        } else {
            // Traversed against the stored direction (bend.parent further
            // from the root) — undo the shift instead.
            let child_off = offsets[bend.child]
                .ok_or_else(|| format!("panel #{pid}: child visited out of order"))?;
            offsets[pid] = Some(child_off - geo.child_shift);
        }
    }

    // ---- 3. Extrude panels into slabs at their shifted poses ----
    let mut parts: Vec<Solid> = Vec::new();
    for (pid, panel) in model.panels.iter().enumerate() {
        if panel.outline.len() < 3 {
            return Err(format!("panel #{pid}: outline has < 3 points"));
        }
        let off = offsets[pid]
            .ok_or_else(|| format!("panel #{pid} is disconnected from the root panel"))?;
        let poly = Poly {
            outer: panel.outline.clone(),
            holes: panel.holes.clone(),
        };
        let slab = extrude_panel_poly(&poly, &panel.frame_bent, t)
            .map_err(|e| format!("panel #{pid}: {e}"))?;
        parts.push(slab.translate(off.x, off.y, off.z));
    }

    // ---- 4. Build the cylindrical bend sectors ----
    for (bid, bend) in model.bends.iter().enumerate() {
        let off = offsets[bend.parent]
            .ok_or_else(|| format!("bend #{bid}: parent panel is disconnected"))?;
        let sector = bend_sector_solid(model, bend, &bend_geos[bid])
            .map_err(|e| format!("bend #{bid}: {e}"))?;
        parts.push(sector.translate(off.x, off.y, off.z));
    }

    // ---- 5. Union everything into one solid ----
    let mut iter = parts.into_iter();
    let mut acc = iter.next().ok_or("model produced no solids")?;
    for (i, part) in iter.enumerate() {
        acc = acc.union(&part);
        if acc.is_empty() {
            return Err(format!("boolean union failed at part #{}", i + 1));
        }
    }
    if !acc.can_export_step() {
        return Err(
            "folded solid lost its B-rep during boolean unions and cannot be exported to STEP"
                .to_string(),
        );
    }
    // Final sanity: the result must tessellate into a non-empty mesh.
    let mesh = acc.to_mesh(segments.max(8));
    if mesh.num_triangles() == 0 {
        return Err("folded solid tessellated to an empty mesh".to_string());
    }
    Ok(acc)
}

/// Derived world-space geometry of one bend (in nominal, un-shifted
/// `frame_bent` coordinates of the bend's parent panel).
struct BendGeo {
    /// First hinge endpoint in world space.
    h0: Point3,
    /// Unit hinge direction (h0 → h1).
    dir: Vec3,
    /// Hinge length (mm).
    len: f64,
    /// Unit normal of the parent mid-plane on the bend's concave side.
    n_conc: Vec3,
    /// Unit vector from the bend axis toward the child-side tangent line.
    m_hat: Vec3,
    /// Mid-surface bend radius `ρ = R + t/2`.
    rho: f64,
    /// Translation applied to the child subtree: hinge → child tangent.
    child_shift: Vec3,
}

/// Compute the cylindrical-zone geometry for `bend`.
fn bend_geometry(model: &SheetMetalModel, bend: &Bend) -> Result<BendGeo, String> {
    let t = model.thickness;
    let parent = &model.panels[bend.parent];
    let child = &model.panels[bend.child];
    let pf = &parent.frame_bent;
    let cf = &child.frame_bent;

    // Hinge in world space.
    let h0 = pf.to_world(bend.edge_parent.0);
    let h1 = pf.to_world(bend.edge_parent.1);
    let hinge = Vec3::new(h1.x - h0.x, h1.y - h0.y, h1.z - h0.z);
    let len = hinge.norm();
    if len < 1e-9 {
        return Err("degenerate hinge".to_string());
    }
    let dir = hinge / len;

    // In-plane unit directions from the hinge into each panel's material.
    let u_p = material_dir(pf, &parent.outline, h0, dir)?;
    let u_c = material_dir(cf, &child.outline, h0, dir)?;

    // Concave-side normal of the parent mid-plane: the child material
    // rises off the parent plane toward the bend's concave side.
    let n_p = pf.normal();
    let lift = u_c.dot(n_p);
    if lift.abs() < 1e-9 {
        return Err("panels are coplanar/parallel at bend — angle ≈ 0 or π".to_string());
    }
    let n_conc = if lift > 0.0 { n_p } else { -n_p };

    // Signed rotation taking the parent's flat continuation (−u_p) onto
    // the child material direction u_c. Try both signs and keep the match.
    let axis_dir = vcad_kernel_math::Dir3::new_normalize(dir);
    let cont = -u_p;
    let mut chosen: Option<Transform> = None;
    for sgn in [1.0, -1.0] {
        let rot = Transform::rotation_about_axis(&axis_dir, sgn * bend.angle);
        if rot.apply_vec(&cont).dot(u_c) > 1.0 - 1e-6 {
            chosen = Some(rot);
            break;
        }
    }
    let rot = chosen.ok_or_else(|| {
        "bend rotation does not map the parent continuation onto the child \
         material direction — inconsistent frames"
            .to_string()
    })?;

    // The bend cylinder is tangent to the parent mid-plane exactly at the
    // hinge edge, so its axis sits ρ above the hinge on the concave side.
    let rho = bend.radius + t * 0.5;
    // Direction from the axis to the child-side tangent line.
    let m_hat = rot.apply_vec(&(-n_conc));
    // The child subtree moves so its hinge edge lands on that tangent line:
    // hinge + ρ·n_conc (up to the axis) + ρ·m̂ (down to the tangent point).
    let child_shift = n_conc * rho + m_hat * rho;

    Ok(BendGeo {
        h0,
        dir,
        len,
        n_conc,
        m_hat,
        rho,
        child_shift,
    })
}

/// Extrude one panel polygon (outer ring + holes) into a slab of
/// thickness `t` centred on the panel mid-plane defined by `frame`.
fn extrude_panel_poly(poly: &Poly, frame: &Frame, t: f64) -> Result<Solid, String> {
    let n = frame.normal();
    let base = offset_point(frame.origin, n, -t * 0.5);
    let profile = ring_profile(base, frame.x_dir, frame.y_dir, &poly.outer)?;
    let mut slab = Solid::extrude(profile, n * t).map_err(|e| format!("panel extrude: {e:?}"))?;
    // Holes: subtract prisms that overshoot the slab on both sides so the
    // top/bottom intersections are transversal.
    for hole in &poly.holes {
        if hole.len() < 3 {
            continue;
        }
        let mut ring = hole.clone();
        // Hole rings are CW (panel convention); the prism profile wants CCW.
        if poly2d::signed_area_f(&ring) < 0.0 {
            ring.reverse();
        }
        let hole_base = offset_point(frame.origin, n, -t * 0.5 - 1.0);
        let hole_profile = ring_profile(hole_base, frame.x_dir, frame.y_dir, &ring)?;
        let prism = Solid::extrude(hole_profile, n * (t + 2.0))
            .map_err(|e| format!("hole extrude: {e:?}"))?;
        slab = slab.difference(&prism);
    }
    Ok(slab)
}

/// Build a closed line-segment [`SketchProfile`] from a 2D ring.
fn ring_profile(
    origin: Point3,
    x_dir: Vec3,
    y_dir: Vec3,
    ring: &[Point2],
) -> Result<SketchProfile, String> {
    if ring.len() < 3 {
        return Err("ring has < 3 points".to_string());
    }
    let mut segments = Vec::with_capacity(ring.len());
    for i in 0..ring.len() {
        let s = ring[i];
        let e = ring[(i + 1) % ring.len()];
        if (e - s).norm() < 1e-9 {
            continue;
        }
        segments.push(SketchSegment::Line { start: s, end: e });
    }
    SketchProfile::new(origin, x_dir, y_dir, segments).map_err(|e| format!("profile: {e:?}"))
}

/// Build the cylindrical bend sector for one bend: an annular sector
/// swept about the bend axis over the bend angle, extruded along the
/// hinge for its full length. The two arcs become analytic
/// `CYLINDRICAL_SURFACE` faces in STEP output.
///
/// The sector is oversized by [`EPS_OVERLAP`] both radially (inner radius
/// `R − ε`, outer `R + t + ε`) and angularly (`ε` of arc length past each
/// tangent plane) so that the union with the panel slabs — which are
/// exactly tangent to the nominal `R`/`R + t` cylinders — intersects
/// transversally instead of tangentially. The price is a cosmetic ~ε lip
/// along each junction line and bend radii that read `R ∓ ε` in STEP.
fn bend_sector_solid(model: &SheetMetalModel, bend: &Bend, geo: &BendGeo) -> Result<Solid, String> {
    let t = model.thickness;
    let axis_pt = offset_point(geo.h0, geo.n_conc, geo.rho);

    // Local profile frame in the plane perpendicular to the hinge:
    // ex points from the axis to the parent-side tangent line (the hinge).
    let ex = -geo.n_conc;
    let mut ey = geo.dir.cross(ex);
    let mut sweep = geo.m_hat.dot(ey).atan2(geo.m_hat.dot(ex));
    let mut extrude_dir = geo.dir;
    let mut origin = axis_pt;
    if sweep < 0.0 {
        // Mirror the local frame so the sweep is CCW; the extrusion then
        // runs from the far hinge end back toward h0.
        ey = -ey;
        sweep = -sweep;
        extrude_dir = -geo.dir;
        origin = offset_point(axis_pt, geo.dir, geo.len);
    }
    if (sweep - bend.angle).abs() > 1e-5 {
        return Err(format!(
            "derived sweep {:.6} rad disagrees with bend angle {:.6} rad",
            sweep, bend.angle
        ));
    }

    let eps_ang = EPS_OVERLAP / geo.rho;
    let phi0 = -eps_ang;
    let phi1 = sweep + eps_ang;
    let r_in = (bend.radius - EPS_OVERLAP).max(EPS_OVERLAP);
    let r_out = bend.radius + t + EPS_OVERLAP;
    let pt = |phi: f64, r: f64| Point2::new(r * phi.cos(), r * phi.sin());
    let center = Point2::new(0.0, 0.0);
    let segments = vec![
        SketchSegment::Arc {
            start: pt(phi0, r_out),
            end: pt(phi1, r_out),
            center,
            ccw: true,
        },
        SketchSegment::Line {
            start: pt(phi1, r_out),
            end: pt(phi1, r_in),
        },
        SketchSegment::Arc {
            start: pt(phi1, r_in),
            end: pt(phi0, r_in),
            center,
            ccw: false,
        },
        SketchSegment::Line {
            start: pt(phi0, r_in),
            end: pt(phi0, r_out),
        },
    ];
    let profile = SketchProfile::new(origin, ex, ey, segments)
        .map_err(|e| format!("sector profile: {e:?}"))?;
    Solid::extrude(profile, extrude_dir * geo.len).map_err(|e| format!("sector extrude: {e:?}"))
}

/// Unit vector in `frame`'s plane, perpendicular to the hinge direction
/// `d`, pointing from the hinge point `h0` into the panel's material.
fn material_dir(frame: &Frame, outline: &[Point2], h0: Point3, d: Vec3) -> Result<Vec3, String> {
    let n = frame.normal();
    let cand = n.cross(d);
    let len = cand.norm();
    if len < 1e-9 {
        return Err("hinge is perpendicular to panel plane".to_string());
    }
    let cand = cand / len;
    let mut best = 0.0_f64;
    for p in outline {
        let w = frame.to_world(*p);
        let dist = (w.x - h0.x) * cand.x + (w.y - h0.y) * cand.y + (w.z - h0.z) * cand.z;
        if dist.abs() > best.abs() {
            best = dist;
        }
    }
    if best.abs() < 1e-9 {
        return Err("cannot determine panel material side of hinge".to_string());
    }
    Ok(if best > 0.0 { cand } else { -cand })
}

/// `p + v * s` for `Point3` / `Vec3`.
fn offset_point(p: Point3, v: Vec3, s: f64) -> Point3 {
    Point3::new(p.x + v.x * s, p.y + v.y * s, p.z + v.z * s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;
    use vcad_kernel_sheet::{
        add_edge_flange, base_flange_polygon_with_holes, base_flange_rect, bend_table::BendTable,
        edge_flange::EdgeFlangeParams, BendDirection, FlangePosition, FlatPattern,
    };

    const T: f64 = 2.0;
    const R: f64 = 2.0;
    const K: f64 = 0.44;

    fn flange(
        panel: usize,
        edge: usize,
        length: f64,
        direction: BendDirection,
    ) -> EdgeFlangeParams {
        EdgeFlangeParams {
            panel,
            edge_index: edge,
            length,
            angle: FRAC_PI_2,
            radius: R,
            direction,
            position: FlangePosition::MaterialInside,
            material: "Al-soft".to_string(),
            manual_k: Some(K),
        }
    }

    fn l_bracket(direction: BendDirection) -> SheetMetalModel {
        let mut m = base_flange_rect(60.0, 40.0, T).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 30.0, direction)).unwrap();
        m
    }

    /// Exact analytic volume of the folded L-bracket: full-size slabs +
    /// bend sector (mid-surface arc length × thickness — exact for an
    /// annulus centred on ρ).
    fn expected_l_bracket_volume() -> f64 {
        let rho = R + T / 2.0;
        let slabs = T * (60.0 * 40.0 + 60.0 * 30.0);
        let sector = FRAC_PI_2 * rho * T * 60.0;
        slabs + sector
    }

    #[test]
    fn l_bracket_folds_to_expected_volume_and_bbox() {
        let model = l_bracket(BendDirection::Up);
        let solid = folded_sheet_solid(&model, 32).unwrap();
        assert!(solid.can_export_step());

        let vol = solid.volume();
        let expected = expected_l_bracket_volume();
        let rel = (vol - expected).abs() / expected;
        assert!(
            rel < 0.01,
            "volume {vol:.2} vs expected {expected:.2} (rel err {rel:.4})"
        );

        // Base panel mid-plane is z = 0 (slab z ∈ [-1, 1]) spanning
        // y ∈ [0, 40]. The 90° flange off edge 0 (y = 0) hangs from a bend
        // zone of mid-radius ρ: flange plane at y = -ρ ± t/2, tip at
        // z = -(ρ + 30).
        let rho = R + T / 2.0;
        let (min, max) = solid.bounding_box();
        assert!((min[0] - 0.0).abs() < 0.1 && (max[0] - 60.0).abs() < 0.1);
        assert!((max[1] - 40.0).abs() < 0.1);
        assert!((max[2] - T / 2.0).abs() < 0.1);
        assert!(
            (min[2] + rho + 30.0).abs() < 0.1,
            "flange tip at z = {}",
            min[2]
        );
        assert!(
            (min[1] + rho + T / 2.0).abs() < 0.1,
            "flange outer face at y = {}",
            min[1]
        );
    }

    #[test]
    fn l_bracket_volume_matches_flat_pattern_allowance() {
        let mut model = l_bracket(BendDirection::Up);
        let solid = folded_sheet_solid(&model, 32).unwrap();
        let vol = solid.volume();
        vcad_kernel_sheet::unfold(&mut model).unwrap();
        let flat = FlatPattern::from_model(&model);
        let flat_vol = flat.area_mm2 * T;
        let rel = (vol - flat_vol).abs() / flat_vol;
        // Neutral axis (K = 0.44) ≠ mid-plane, so allow ~2%.
        assert!(
            rel < 0.02,
            "folded volume {vol:.2} vs flat-pattern volume {flat_vol:.2} (rel err {rel:.4})"
        );
    }

    #[test]
    fn down_bend_mirrors_up_bend() {
        let up = folded_sheet_solid(&l_bracket(BendDirection::Up), 32).unwrap();
        let down = folded_sheet_solid(&l_bracket(BendDirection::Down), 32).unwrap();
        let vu = up.volume();
        let vd = down.volume();
        assert!(((vu - vd) / vu).abs() < 1e-3, "up {vu:.3} vs down {vd:.3}");
        // The Down flange folds to the opposite side of the base plane.
        let rho = R + T / 2.0;
        let (umin, _umax) = up.bounding_box();
        let (_dmin, dmax) = down.bounding_box();
        assert!((umin[2] + rho + 30.0).abs() < 0.1);
        assert!((dmax[2] - rho - 30.0).abs() < 0.1);
    }

    #[test]
    fn u_channel_folds_to_expected_volume() {
        let mut m = base_flange_rect(80.0, 40.0, T).unwrap();
        let table = BendTable::builtin();
        // Edge 0 is y=0, edge 2 is y=40 — opposite walls of a U channel.
        add_edge_flange(&mut m, &table, flange(0, 0, 25.0, BendDirection::Up)).unwrap();
        add_edge_flange(&mut m, &table, flange(0, 2, 25.0, BendDirection::Up)).unwrap();
        let solid = folded_sheet_solid(&m, 32).unwrap();
        assert!(solid.can_export_step());

        let rho = R + T / 2.0;
        let expected = T * (80.0 * 40.0 + 2.0 * 80.0 * 25.0) + 2.0 * FRAC_PI_2 * rho * T * 80.0;
        let vol = solid.volume();
        let rel = (vol - expected).abs() / expected;
        assert!(
            rel < 0.01,
            "volume {vol:.2} vs expected {expected:.2} (rel err {rel:.4})"
        );

        // Both walls fold to the same side; bend zones widen the channel
        // by ρ + t/2 on each side.
        let (min, max) = solid.bounding_box();
        assert!((max[2] - T / 2.0).abs() < 0.1);
        assert!((min[2] + rho + 25.0).abs() < 0.1);
        assert!((min[1] + rho + T / 2.0).abs() < 0.1);
        assert!((max[1] - 40.0 - rho - T / 2.0).abs() < 0.1);
    }

    #[test]
    fn panel_hole_is_subtracted() {
        let outline = vec![
            Point2::new(0.0, 0.0),
            Point2::new(60.0, 0.0),
            Point2::new(60.0, 40.0),
            Point2::new(0.0, 40.0),
        ];
        // CW hole (panel convention), 10×10 in the middle of the base.
        let hole = vec![
            Point2::new(20.0, 15.0),
            Point2::new(20.0, 25.0),
            Point2::new(30.0, 25.0),
            Point2::new(30.0, 15.0),
        ];
        let mut m = base_flange_polygon_with_holes(outline, vec![hole], T).unwrap();
        let table = BendTable::builtin();
        add_edge_flange(&mut m, &table, flange(0, 0, 30.0, BendDirection::Up)).unwrap();
        let solid = folded_sheet_solid(&m, 32).unwrap();

        let expected = expected_l_bracket_volume() - 10.0 * 10.0 * T;
        let vol = solid.volume();
        let rel = (vol - expected).abs() / expected;
        assert!(
            rel < 0.01,
            "volume {vol:.2} vs expected {expected:.2} (rel err {rel:.4})"
        );
    }

    #[test]
    fn step_round_trip_preserves_volume_and_cylinders() {
        let model = l_bracket(BendDirection::Up);
        let solid = folded_sheet_solid(&model, 32).unwrap();
        let step = solid.to_step_buffer().unwrap();
        let text = String::from_utf8(step.clone()).unwrap();
        assert!(
            text.contains("CYLINDRICAL_SURFACE"),
            "STEP output must contain true cylindrical bend faces"
        );
        let reimported = Solid::from_step_buffer(&step).unwrap();
        let v0 = solid.volume();
        let v1 = reimported.volume();
        let rel = (v0 - v1).abs() / v0;
        assert!(
            rel < 0.01,
            "round-trip volume {v1:.2} vs original {v0:.2} (rel err {rel:.4})"
        );
    }

    #[test]
    fn hem_angle_is_rejected() {
        let mut m = base_flange_rect(60.0, 40.0, T).unwrap();
        let table = BendTable::builtin();
        let mut params = flange(0, 0, 30.0, BendDirection::Up);
        params.angle = std::f64::consts::PI;
        add_edge_flange(&mut m, &table, params).unwrap();
        let err = folded_sheet_solid(&m, 32).unwrap_err();
        assert!(err.contains("hems"), "unexpected error: {err}");
    }
}
