//! Unit tests for the feature recogniser.
//!
//! Two layers: synthetic B-reps built here (fast, hermetic, exercise the
//! clustering and the relative-angle reporting), and real vendor STEP files
//! (the numbers a designer actually needs, verified independently against the
//! vendor drawings). The vendor fixtures live outside the repo — they are
//! copyrighted vendor CAD — so those tests skip with a printed note when the
//! files are absent, and run in full on a machine that has them.

use super::*;
use vcad_kernel_geom::{GeometryStore, Plane};
use vcad_kernel_topo::{Face, Loop, Orientation, ShellType, Topology, Vertex};

// ---------------------------------------------------------------------------
// Synthetic fixtures
// ---------------------------------------------------------------------------

/// A cylindrical face to synthesise: axis point, direction, radius, extent.
struct Cyl {
    center: Point3,
    axis: Vec3,
    radius: f64,
    z0: f64,
    z1: f64,
    concavity: Concavity,
}

/// Build a B-rep whose only faces are the given cylinders.
///
/// Each face gets a two-vertex loop at the ends of its axial extent — enough
/// for the recogniser, which reads extent from loop vertices and everything
/// else from the surface.
fn synth(cyls: &[Cyl]) -> BRepSolid {
    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    let shell = topo.shells.insert(vcad_kernel_topo::Shell {
        faces: Vec::new(),
        solid: None,
        shell_type: ShellType::Outer,
    });

    for c in cyls {
        let axis = c.axis.normalize();
        let surface_index = geom.surfaces.len();
        geom.surfaces.push(Box::new(CylinderSurface::with_axis(
            c.center, axis, c.radius,
        )));

        let (u, _v) = plane_basis(canonical_axis(axis));
        let p0 = c.center + axis * c.z0 + u * c.radius;
        let p1 = c.center + axis * c.z1 + u * c.radius;
        let v0 = topo.vertices.insert(Vertex {
            point: p0,
            half_edge: None,
        });
        let v1 = topo.vertices.insert(Vertex {
            point: p1,
            half_edge: None,
        });
        let he0 = topo.half_edges.insert(vcad_kernel_topo::HalfEdge {
            origin: v0,
            twin: None,
            next: None,
            prev: None,
            edge: None,
            loop_id: None,
        });
        let he1 = topo.half_edges.insert(vcad_kernel_topo::HalfEdge {
            origin: v1,
            twin: None,
            next: Some(he0),
            prev: None,
            edge: None,
            loop_id: None,
        });
        topo.half_edges[he0].next = Some(he1);
        let lp = topo.loops.insert(Loop {
            half_edge: he0,
            face: None,
        });
        let face = topo.faces.insert(Face {
            outer_loop: lp,
            inner_loops: Vec::new(),
            surface_index,
            orientation: match c.concavity {
                Concavity::Internal => Orientation::Reversed,
                Concavity::External => Orientation::Forward,
            },
            shell: Some(shell),
        });
        topo.loops[lp].face = Some(face);
        topo.shells[shell].faces.push(face);
    }

    // One planar face so the vertex set isn't only cylinder seams.
    let plane_index = geom.surfaces.len();
    geom.surfaces
        .push(Box::new(Plane::new(Point3::origin(), Vec3::x(), Vec3::y())));
    let _ = plane_index;

    let solid_id = topo.solids.insert(vcad_kernel_topo::Solid {
        outer_shell: shell,
        void_shells: Vec::new(),
    });
    topo.shells[shell].solid = Some(solid_id);

    BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id,
    }
}

/// `n` holes of diameter `dia` on a bolt circle of diameter `bcd`, first hole
/// at `phase` degrees, about +Z.
fn bolt_circle(n: usize, bcd: f64, dia: f64, phase_deg: f64) -> Vec<Cyl> {
    (0..n)
        .map(|k| {
            let a = (phase_deg + 360.0 * k as f64 / n as f64).to_radians();
            Cyl {
                center: Point3::new(bcd / 2.0 * a.cos(), bcd / 2.0 * a.sin(), 0.0),
                axis: Vec3::z(),
                radius: dia / 2.0,
                z0: 0.0,
                z1: 10.0,
                concavity: Concavity::Internal,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Synthetic tests
// ---------------------------------------------------------------------------

#[test]
fn eight_holes_read_as_one_bolt_circle() {
    let brep = synth(&bolt_circle(8, 98.0, 4.2, 22.5));
    let report = recognize(&brep);
    let p = report.bolt_circles().next().expect("a bolt circle");

    assert_eq!(p.count, 8);
    assert!((p.hole_diameter_mm - 4.2).abs() < 1e-9);
    let PatternKind::BoltCircle {
        bolt_circle_diameter_mm,
        spacing_deg,
    } = p.kind
    else {
        unreachable!()
    };
    assert!((bolt_circle_diameter_mm - 98.0).abs() < 1e-6);
    assert!((spacing_deg.expect("even spacing") - 45.0).abs() < 1e-9);
    // Angles are reported relative to the first hole: 0, 45, 90, ...
    for (k, m) in p.members.iter().enumerate() {
        assert!((m.angle_deg - 45.0 * k as f64).abs() < 1e-6, "{m:?}");
    }
}

#[test]
fn relative_angles_survive_a_rotated_placement() {
    // The same pattern clocked to a different absolute phase must produce
    // identical relative angles — that is the whole point of reporting them.
    let a = recognize(&synth(&bolt_circle(8, 98.0, 4.2, 22.5)));
    let b = recognize(&synth(&bolt_circle(8, 98.0, 4.2, 91.3)));
    let (pa, pb) = (
        a.bolt_circles().next().unwrap(),
        b.bolt_circles().next().unwrap(),
    );
    for (ma, mb) in pa.members.iter().zip(&pb.members) {
        assert!((ma.angle_deg - mb.angle_deg).abs() < 1e-6);
    }
    // ... while the absolute angles differ, exactly as the placements do.
    let (aa, ab) = (
        pa.first_member_absolute_deg.unwrap(),
        pb.first_member_absolute_deg.unwrap(),
    );
    assert!((aa - ab).abs() > 1.0);
}

#[test]
fn dowels_bisecting_bolt_holes_are_reported_as_a_relation() {
    // Six holes at 60° spacing, plus three dowels at 60° + 30° — the RS03
    // output-face arrangement, in miniature.
    let mut cyls = bolt_circle(6, 30.36, 3.5, 0.0);
    cyls.extend(bolt_circle(3, 38.5, 4.2, 30.0));
    let report = recognize(&synth(&cyls));

    let six = report
        .patterns
        .iter()
        .position(|p| p.count == 6)
        .expect("six-hole circle");
    let three = report
        .patterns
        .iter()
        .position(|p| p.count == 3)
        .expect("three dowels");
    let rel = report
        .relations
        .iter()
        .find(|r| r.reference == six && r.subject == three)
        .expect("relation between the concentric circles");
    assert!((rel.phase_deg - 30.0).abs() < 1e-6);
    assert!(rel.bisects_adjacent);
}

#[test]
fn a_lone_bore_is_not_a_pattern() {
    let brep = synth(&[Cyl {
        center: Point3::origin(),
        axis: Vec3::z(),
        radius: 12.5,
        z0: 0.0,
        z1: 20.0,
        concavity: Concavity::Internal,
    }]);
    let report = recognize(&brep);
    assert_eq!(report.patterns.len(), 1);
    assert!(matches!(report.patterns[0].kind, PatternKind::Single));
    assert!((report.patterns[0].hole_diameter_mm - 25.0).abs() < 1e-9);
    assert_eq!(report.bolt_circles().count(), 0);
}

#[test]
fn a_row_of_holes_is_a_linear_pattern() {
    let cyls: Vec<Cyl> = (0..4)
        .map(|k| Cyl {
            center: Point3::new(10.0 * k as f64, 0.0, 0.0),
            axis: Vec3::z(),
            radius: 2.5,
            z0: 0.0,
            z1: 5.0,
            concavity: Concavity::Internal,
        })
        .collect();
    let report = recognize(&synth(&cyls));
    let p = &report.patterns[0];
    let PatternKind::Linear { spacing_mm, .. } = p.kind else {
        panic!("expected a linear pattern, got {:?}", p.kind);
    };
    assert_eq!(p.count, 4);
    assert!((spacing_mm - 10.0).abs() < 1e-9);
}

#[test]
fn body_od_comes_from_the_largest_coaxial_cylinder_not_the_bbox() {
    // A Ø80 body with an off-axis Ø20 connector boss hanging off the side: the
    // bounding box reads far wider than the body, which is the trap this
    // recogniser exists to avoid.
    let brep = synth(&[
        Cyl {
            center: Point3::origin(),
            axis: Vec3::z(),
            radius: 40.0,
            z0: 0.0,
            z1: 60.0,
            concavity: Concavity::External,
        },
        Cyl {
            center: Point3::new(45.0, 0.0, 30.0),
            axis: Vec3::z(),
            radius: 10.0,
            z0: -5.0,
            z1: 5.0,
            concavity: Concavity::External,
        },
    ]);
    let env = recognize(&brep).envelope;
    assert!((env.body_od_mm.expect("a body OD") - 80.0).abs() < 1e-9);
    assert!(
        env.bbox_across_axis_mm > 100.0,
        "bbox should overstate: {}",
        env.bbox_across_axis_mm
    );
}

#[test]
fn the_dominant_axis_is_not_assumed_to_be_z() {
    let brep = synth(&[
        Cyl {
            center: Point3::origin(),
            axis: Vec3::y(),
            radius: 40.0,
            z0: 0.0,
            z1: 60.0,
            concavity: Concavity::External,
        },
        Cyl {
            center: Point3::new(20.0, 5.0, 0.0),
            axis: Vec3::z(),
            radius: 3.0,
            z0: 0.0,
            z1: 4.0,
            concavity: Concavity::Internal,
        },
    ]);
    let env = recognize(&brep).envelope;
    assert!(env.dominant_axis.dot(Vec3::y()).abs() > 0.999);
    assert!((env.body_od_mm.unwrap() - 80.0).abs() < 1e-9);
}

#[test]
fn seam_split_faces_merge_into_one_hole() {
    // The same hole written as two half-cylinder faces on one axis line, plus
    // a counterbore run further down the same axis: one feature, three faces.
    let brep = synth(&[
        Cyl {
            center: Point3::origin(),
            axis: Vec3::z(),
            radius: 2.1,
            z0: 0.0,
            z1: 6.0,
            concavity: Concavity::Internal,
        },
        Cyl {
            center: Point3::origin(),
            axis: Vec3::z(),
            radius: 2.1,
            z0: 0.0,
            z1: 6.0,
            concavity: Concavity::Internal,
        },
        Cyl {
            center: Point3::origin(),
            axis: Vec3::z(),
            radius: 2.1,
            z0: 9.0,
            z1: 12.0,
            concavity: Concavity::Internal,
        },
    ]);
    let features = cylindrical_features(&brep);
    assert_eq!(features.len(), 1);
    assert_eq!(features[0].faces.len(), 3);
    assert_eq!(features[0].segments, 3);
    assert!((features[0].length_mm() - 12.0).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// Vendor STEP fixtures
// ---------------------------------------------------------------------------

#[cfg(feature = "step")]
mod vendor {
    use super::*;
    use crate::step::{recognize_step_file, StepFeatureReport};

    /// Vendor CAD isn't in the repo. Resolve it from `$HOME`, or skip.
    fn fixture(rel: &str) -> Option<std::path::PathBuf> {
        let path = std::path::PathBuf::from(std::env::var("HOME").ok()?).join(rel);
        if path.exists() {
            Some(path)
        } else {
            eprintln!("skipping: vendor fixture not present at {}", path.display());
            None
        }
    }

    fn find_bolt_circle(
        r: &StepFeatureReport,
        count: usize,
        dia: f64,
        bcd: f64,
    ) -> Option<&HolePattern> {
        r.components.iter().find_map(|c| {
            c.report.bolt_circles().find(|p| {
                p.count == count
                    && (p.hole_diameter_mm - dia).abs() < 0.05
                    && matches!(p.kind, PatternKind::BoltCircle { bolt_circle_diameter_mm, .. }
                        if (bolt_circle_diameter_mm - bcd).abs() < 0.05)
            })
        })
    }

    #[test]
    fn rs03_stator_flange_is_eight_m4_on_bcd_98() {
        let Some(path) = fixture("Developer/robstride_assets/Product Literature/RS03/RS03.stp")
        else {
            return;
        };
        let r = recognize_step_file(&path).expect("read RS03");

        // 8 x M4 clearance (Ø4.2) on BCD 98, evenly spaced at 45°.
        let p = find_bolt_circle(&r, 8, 4.2, 98.0).expect("8 x M4 on BCD 98");
        let PatternKind::BoltCircle { spacing_deg, .. } = p.kind else {
            unreachable!()
        };
        assert!((spacing_deg.expect("even spacing") - 45.0).abs() < 1e-6);
        // 22.5 + 45n: the absolute phase, mod the spacing, is 22.5.
        let phase = p.first_member_absolute_deg.unwrap() % 45.0;
        assert!((phase - 22.5).abs() < 0.05, "phase {phase}");
    }

    #[test]
    fn rs03_output_hub_has_six_holes_and_three_bisecting_dowels() {
        let Some(path) = fixture("Developer/robstride_assets/Product Literature/RS03/RS03.stp")
        else {
            return;
        };
        let r = recognize_step_file(&path).expect("read RS03");

        let hub = r
            .components
            .iter()
            .find(|c| {
                c.report
                    .bolt_circles()
                    .any(|p| p.count == 6 && matches!(p.kind, PatternKind::BoltCircle { bolt_circle_diameter_mm, .. } if (bolt_circle_diameter_mm - 30.36).abs() < 0.05))
            })
            .expect("the output hub");

        let six = hub
            .report
            .patterns
            .iter()
            .position(|p| {
                p.count == 6
                    && matches!(p.kind, PatternKind::BoltCircle { bolt_circle_diameter_mm, .. }
                        if (bolt_circle_diameter_mm - 30.36).abs() < 0.05)
            })
            .expect("6 x M4 on BCD 30.36");
        let PatternKind::BoltCircle { spacing_deg, .. } = hub.report.patterns[six].kind else {
            unreachable!()
        };
        assert!((spacing_deg.expect("even spacing") - 60.0).abs() < 1e-6);

        // Three dowels on BCD 38.5, bisecting adjacent output holes. Read as
        // absolute angles this is placement-dependent noise; read against the
        // six-hole circle it is always "30° = half of 60°".
        let dowels = hub
            .report
            .patterns
            .iter()
            .position(|p| {
                p.count == 3
                    && matches!(p.kind, PatternKind::BoltCircle { bolt_circle_diameter_mm, .. }
                        if (bolt_circle_diameter_mm - 38.5).abs() < 0.05)
            })
            .expect("3 dowels on BCD 38.5");
        let rel = hub
            .report
            .relations
            .iter()
            .find(|r| r.reference == six && r.subject == dowels)
            .expect("dowels related to the output bolt circle");
        assert!(
            (rel.phase_deg - 30.0).abs() < 0.05,
            "phase {}",
            rel.phase_deg
        );
        assert!(rel.bisects_adjacent);
    }

    #[test]
    fn rs03_envelope_od_is_the_coaxial_body_not_the_bbox() {
        let Some(path) = fixture("Developer/robstride_assets/Product Literature/RS03/RS03.stp")
        else {
            return;
        };
        let r = recognize_step_file(&path).expect("read RS03");
        let env = r.envelope();
        assert!(env.dominant_axis.dot(Vec3::z()).abs() > 0.999);
        assert!(
            (env.body_od_mm.expect("body OD") - 100.5).abs() < 0.05,
            "od {:?}",
            env.body_od_mm
        );
        // The flange corners push the bounding box past the body diameter.
        assert!(env.bbox_across_axis_mm > env.body_od_mm.unwrap());
    }

    #[test]
    fn x6_60_body_od_is_80_about_the_y_axis() {
        let Some(path) = fixture("Developer/myactuator_assets/X6-60/RMD-X6-P20-60 3D-A0.STEP")
        else {
            return;
        };
        let r = recognize_step_file(&path).expect("read X6-60");
        let env = r.envelope();
        // This part's dominant axis is Y. Assuming Z would report a slice of
        // the body as its diameter.
        assert!(
            env.dominant_axis.dot(Vec3::y()).abs() > 0.999,
            "axis {:?}",
            env.dominant_axis
        );
        assert!(
            (env.body_od_mm.expect("body OD") - 80.0).abs() < 0.05,
            "od {:?}",
            env.body_od_mm
        );
    }

    #[test]
    fn x4_36_body_od_is_55_about_the_z_axis() {
        let Some(path) = fixture("Developer/myactuator_assets/X4-36/RMD-X4-P36-36 3D-A0.STEP")
        else {
            return;
        };
        let r = recognize_step_file(&path).expect("read X4-36");
        let env = r.envelope();
        assert!(
            env.dominant_axis.dot(Vec3::z()).abs() > 0.999,
            "axis {:?}",
            env.dominant_axis
        );
        assert!(
            (env.body_od_mm.expect("body OD") - 55.0).abs() < 0.05,
            "od {:?}",
            env.body_od_mm
        );
    }
}
