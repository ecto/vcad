//! M6 — seeding synthesis: derive a [`ParamSeeding`] from the build function.
//!
//! Every milestone up to M5 hand-writes its seeding: the author knows "this θ
//! grows these twenty blend radii at rate 1 while each center retreats
//! `(±1, ±1, 0)`", transcribes it into a [`ParamSeeding`], and asserts the
//! surface census by hand ([`crate::ParamSeeding::seed_where`] returns the
//! count precisely so the author *can* assert it). That is exactly the kind of
//! bookkeeping a machine should do. M6 reads the seeding off the build
//! function itself.
//!
//! # Why probe the surface *fields*, not the mesh
//!
//! The obvious alternative — finite-difference the mesh node positions and
//! call that dx/dθ — is wrong for the same reason the whole seam exists: the
//! tessellator's discrete choices are functions of θ and flip under
//! perturbation, so node `i` at θ + h is not node `i` at θ − h. Even under a
//! *frozen* plan, mesh-level FD only reproduces what the analytic seam already
//! computes and inherits the plan's O(h) correspondence noise on top.
//!
//! Surface fields are different: they are **smooth in θ with no combinatorial
//! structure**. A plane's offset, a cylinder's radius and axis, a sphere's
//! center — each is an analytic function of θ that a central difference
//! recovers to O(h²), with no branch to cross. So M6 finite-differences the
//! *fields*, hands the per-surface velocities to the seam as a
//! [`ParamSeeding`], and lets the existing analytic machinery (lift-bridge,
//! implicit rows, tangency completion) do the geometry. The seam is still
//! exact in the fields; only the θ → field map is numerical, and that map is
//! the one part with no combinatorics to get wrong.
//!
//! # Matching and observability
//!
//! Two builds of the same model enumerate the geometry store in **different
//! order** (the boolean pipeline and the fillet kernel are both
//! order-nondeterministic — a rebuild flips blend-axis signs, rotates
//! reference directions, and slides cylinder centers along their axes). So a
//! base surface cannot be found in a perturbed build by store index; it is
//! matched by **frame-invariant geometric identity** (see [`same_surface`]),
//! and the extraction reads only the **observable** field components, in the
//! base surface's own frame:
//!
//! - **Plane** — only motion along the normal is observable (in-plane origin
//!   drift is gauge). The seed is `Translate { velocity = ṅ_offset · n }`.
//! - **Cylinder** — the along-axis position of `center` is gauge (the fillet
//!   kernel slides it freely between builds), so only the radial center
//!   velocity is seeded, plus a `CylinderRadius` rate from the radius delta.
//!   Both are read in the base axis frame, so an axis-sign flip in the
//!   perturbed build cannot corrupt them.
//! - **Sphere** — center and radius are fully observable: full `Translate`
//!   velocity plus a `SphereRadius` rate.
//! - **Cone** — the apex is a genuine point of the surface's geometry (an
//!   apex slid along the axis changes the radius at every height), so the
//!   full apex velocity is observable: full `Translate` plus a `ConeAngle`
//!   rate. Only the axis *sign* is gauge (the implicit form is symmetric
//!   under it).
//! - **Torus** — like the sphere, nothing translational is gauge: full
//!   center `Translate` plus independent `TorusMajorRadius` /
//!   `TorusMinorRadius` rates.
//!
//! Composite seeds (radius rate *and* center velocity on the same blend) fall
//! out for free — [`ParamSeeding`] composes, so both are pushed. Duplicate
//! copies of a moving surface in a boolean's store are each matched and seeded
//! independently, which is exactly the invariant the vertex solve needs (see
//! [`crate::ParamSeeding::seed_where`]).
//!
//! Any change of the surface census under perturbation — a differing topology
//! signature, or a base surface with no perturbed match — is a hard error, not
//! a silently partial seeding: a step that crosses a topology change has no
//! meaningful derivative.

use vcad_kernel_geom::{
    ConeSurface, CylinderSurface, Plane, SphereSurface, Surface, SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::{topology_signature, FrozenError};

use crate::{DiffError, ParamSeeding, SurfaceSeed};

/// Matching tolerance (mm) for geometric surface identity across a rebuild.
///
/// The house convention: far above the O(h · velocity) ≈ 1e-6 mm motion of a
/// derivative step, far below feature separation. Distinct features never both
/// fall inside this ball of a base surface; a rebuilt copy of the *same*
/// surface always does.
const MATCH_TOL: f64 = 1e-3;

/// Alignment tolerance for axes / normals: two directions are "the same up to
/// sign" when `|a · b| > 1 − ANGLE_TOL`. The geometric axis of a blend or the
/// normal of a face is exact across rebuilds (only the *reference* direction
/// rotates), so this only has to reject genuinely different orientations.
const ANGLE_TOL: f64 = 1e-4;

/// Two matched candidates in the *same* perturbed build are accepted as
/// duplicate copies (rather than an ambiguous match) only if they agree to
/// this tolerance. Duplicate store copies are bit-identical (~1e-12 apart);
/// two distinct features close enough to both match a base surface would sit
/// ~`MATCH_TOL` apart and trip the ambiguity error instead.
const SAME_EPS: f64 = 1e-6;

/// A seed component below this magnitude (in every element) is treated as
/// numerically zero and dropped, keeping seedings sparse and honest.
///
/// The central-difference roundoff floor at `h = 1e-6` on mm-scale geometry is
/// ≈ ε · |field| / (2h) ≈ 1e-9; this threshold sits an order or two above that
/// noise and seven orders below the genuine O(1) seed magnitudes these models
/// produce, so it never drops a real seed nor keeps a spurious one.
const ZERO_TOL: f64 = 1e-7;

/// The frame-invariant geometric identity of a surface, extracted once per
/// surface per build. Only the kinds the seed vocabulary can express are
/// represented; other kinds are rejected up front ([`DiffError::UnsupportedSynthesis`]).
#[derive(Debug, Clone, Copy)]
enum SurfInfo {
    /// Plane as `{x : normal · (x − origin) = 0}` (normal defined up to sign).
    Plane {
        /// Unit normal.
        normal: Vec3,
        /// A point on the plane.
        origin: Point3,
    },
    /// Cylinder by axis line, radius (axis defined up to sign; `center` sits
    /// anywhere on the axis line).
    Cylinder {
        /// A point on the axis line.
        center: Point3,
        /// Unit axis.
        axis: Vec3,
        /// Radius.
        radius: f64,
    },
    /// Sphere by center and radius.
    Sphere {
        /// Center.
        center: Point3,
        /// Radius.
        radius: f64,
    },
    /// Cone by apex, axis (up to sign — the implicit form is symmetric under
    /// a flip) and half-angle.
    Cone {
        /// Apex point.
        apex: Point3,
        /// Unit axis.
        axis: Vec3,
        /// Half-angle (radians).
        half_angle: f64,
    },
    /// Torus by center, axis (up to sign) and both radii.
    Torus {
        /// Center of the ring.
        center: Point3,
        /// Unit axis.
        axis: Vec3,
        /// Major radius.
        major: f64,
        /// Minor radius.
        minor: f64,
    },
}

impl SurfInfo {
    /// The kind tag, for census / mismatch reporting.
    fn kind(&self) -> SurfaceKind {
        match self {
            SurfInfo::Plane { .. } => SurfaceKind::Plane,
            SurfInfo::Cylinder { .. } => SurfaceKind::Cylinder,
            SurfInfo::Sphere { .. } => SurfaceKind::Sphere,
            SurfInfo::Cone { .. } => SurfaceKind::Cone,
            SurfInfo::Torus { .. } => SurfaceKind::Torus,
        }
    }
}

/// Extract the frame-invariant identity of a surface, or reject a kind the
/// seed vocabulary cannot express.
fn surf_info(s: &dyn Surface) -> Result<SurfInfo, DiffError> {
    match s.surface_type() {
        SurfaceKind::Plane => {
            let p = crate::downcast::<Plane>(s, SurfaceKind::Plane)?;
            Ok(SurfInfo::Plane {
                normal: *p.normal_dir.as_ref(),
                origin: p.origin,
            })
        }
        SurfaceKind::Cylinder => {
            let c = crate::downcast::<CylinderSurface>(s, SurfaceKind::Cylinder)?;
            Ok(SurfInfo::Cylinder {
                center: c.center,
                axis: *c.axis.as_ref(),
                radius: c.radius,
            })
        }
        SurfaceKind::Sphere => {
            let sp = crate::downcast::<SphereSurface>(s, SurfaceKind::Sphere)?;
            Ok(SurfInfo::Sphere {
                center: sp.center,
                radius: sp.radius,
            })
        }
        SurfaceKind::Cone => {
            let c = crate::downcast::<ConeSurface>(s, SurfaceKind::Cone)?;
            Ok(SurfInfo::Cone {
                apex: c.apex,
                axis: *c.axis.as_ref(),
                half_angle: c.half_angle,
            })
        }
        SurfaceKind::Torus => {
            let t = crate::downcast::<TorusSurface>(s, SurfaceKind::Torus)?;
            Ok(SurfInfo::Torus {
                center: t.center,
                axis: *t.axis.as_ref(),
                major: t.major_radius,
                minor: t.minor_radius,
            })
        }
        kind => Err(DiffError::UnsupportedSynthesis(kind)),
    }
}

/// Extract every surface's identity from a build.
fn build_infos(brep: &BRepSolid) -> Result<Vec<SurfInfo>, DiffError> {
    brep.geometry
        .surfaces
        .iter()
        .map(|s| surf_info(s.as_ref()))
        .collect()
}

/// Perpendicular component of `v` with respect to the unit axis `axis`.
fn reject(v: Vec3, axis: Vec3) -> Vec3 {
    v - axis * v.dot(axis)
}

/// Whether two surface identities are the same surface, up to `MATCH_TOL` and
/// axis/normal sign. Only compares frame-invariant quantities.
fn same_surface(a: &SurfInfo, b: &SurfInfo) -> bool {
    match (a, b) {
        (
            SurfInfo::Plane {
                normal: na,
                origin: oa,
            },
            SurfInfo::Plane {
                normal: nb,
                origin: ob,
            },
        ) => {
            // Same normal up to sign, and the same plane (perpendicular
            // distance between them is small).
            na.dot(*nb).abs() > 1.0 - ANGLE_TOL && (*oa - *ob).dot(*na).abs() < MATCH_TOL
        }
        (
            SurfInfo::Cylinder {
                center: ca,
                axis: aa,
                radius: ra,
            },
            SurfInfo::Cylinder {
                center: cb,
                axis: ab,
                radius: rb,
            },
        ) => {
            // Same axis line up to sign (parallel axes + coincident lines) and
            // same radius. The line distance is the perpendicular part of the
            // center offset, so a center slid along the axis does not matter.
            aa.dot(*ab).abs() > 1.0 - ANGLE_TOL
                && (ra - rb).abs() < MATCH_TOL
                && reject(*ca - *cb, *aa).norm() < MATCH_TOL
        }
        (
            SurfInfo::Sphere {
                center: ca,
                radius: ra,
            },
            SurfInfo::Sphere {
                center: cb,
                radius: rb,
            },
        ) => (*ca - *cb).norm() < MATCH_TOL && (ra - rb).abs() < MATCH_TOL,
        (
            SurfInfo::Cone {
                apex: pa,
                axis: aa,
                half_angle: ha,
            },
            SurfInfo::Cone {
                apex: pb,
                axis: ab,
                half_angle: hb,
            },
        ) => {
            // The apex is a real point of the geometry (no along-axis gauge);
            // only the axis sign is free.
            aa.dot(*ab).abs() > 1.0 - ANGLE_TOL
                && (*pa - *pb).norm() < MATCH_TOL
                && (ha - hb).abs() < MATCH_TOL
        }
        (
            SurfInfo::Torus {
                center: ca,
                axis: aa,
                major: ma,
                minor: na,
            },
            SurfInfo::Torus {
                center: cb,
                axis: ab,
                major: mb,
                minor: nb,
            },
        ) => {
            aa.dot(*ab).abs() > 1.0 - ANGLE_TOL
                && (*ca - *cb).norm() < MATCH_TOL
                && (ma - mb).abs() < MATCH_TOL
                && (na - nb).abs() < MATCH_TOL
        }
        _ => false,
    }
}

/// Find the perturbed counterpart of `base` among `others`.
///
/// Returns the matched identity. Multiple candidates are accepted only when
/// they are mutually identical (duplicate store copies); genuinely distinct
/// candidates inside `MATCH_TOL` are a hard [`DiffError::AmbiguousMatch`]. No
/// candidate is a census change, reported as a topology change.
fn find_match(
    base_index: usize,
    base: &SurfInfo,
    base_sig: vcad_kernel_tessellate::frozen::TopologySignature,
    actual_sig: vcad_kernel_tessellate::frozen::TopologySignature,
    others: &[SurfInfo],
) -> Result<SurfInfo, DiffError> {
    let mut candidates = others.iter().filter(|o| same_surface(base, o));
    let Some(first) = candidates.next() else {
        // A base surface with no perturbed counterpart: the census changed
        // even if the (cheap) signature happened to collide.
        return Err(DiffError::Frozen(FrozenError::TopologyChanged {
            expected: base_sig,
            actual: actual_sig,
        }));
    };
    for other in candidates {
        if !same_surface(first, other) || !close_enough(first, other, SAME_EPS) {
            return Err(DiffError::AmbiguousMatch { base_index });
        }
    }
    Ok(*first)
}

/// Tight geometric agreement of two identities (for the duplicate-vs-ambiguous
/// decision): same kind and all frame-invariant fields within `eps`.
fn close_enough(a: &SurfInfo, b: &SurfInfo, eps: f64) -> bool {
    match (a, b) {
        (
            SurfInfo::Plane {
                normal: na,
                origin: oa,
            },
            SurfInfo::Plane {
                normal: nb,
                origin: ob,
            },
        ) => na.dot(*nb).abs() > 1.0 - eps && (*oa - *ob).dot(*na).abs() < eps,
        (
            SurfInfo::Cylinder {
                center: ca,
                axis: aa,
                radius: ra,
            },
            SurfInfo::Cylinder {
                center: cb,
                axis: ab,
                radius: rb,
            },
        ) => {
            aa.dot(*ab).abs() > 1.0 - eps
                && (ra - rb).abs() < eps
                && reject(*ca - *cb, *aa).norm() < eps
        }
        (
            SurfInfo::Sphere {
                center: ca,
                radius: ra,
            },
            SurfInfo::Sphere {
                center: cb,
                radius: rb,
            },
        ) => (*ca - *cb).norm() < eps && (ra - rb).abs() < eps,
        (
            SurfInfo::Cone {
                apex: pa,
                axis: aa,
                half_angle: ha,
            },
            SurfInfo::Cone {
                apex: pb,
                axis: ab,
                half_angle: hb,
            },
        ) => aa.dot(*ab).abs() > 1.0 - eps && (*pa - *pb).norm() < eps && (ha - hb).abs() < eps,
        (
            SurfInfo::Torus {
                center: ca,
                axis: aa,
                major: ma,
                minor: na,
            },
            SurfInfo::Torus {
                center: cb,
                axis: ab,
                major: mb,
                minor: nb,
            },
        ) => {
            aa.dot(*ab).abs() > 1.0 - eps
                && (*ca - *cb).norm() < eps
                && (ma - mb).abs() < eps
                && (na - nb).abs() < eps
        }
        _ => false,
    }
}

/// Read the observable seed values of one base surface off its matched
/// `plus`/`minus` counterparts (central difference over `2h`), pushing every
/// non-zero seed onto `seeding` at `index`.
fn extract_seeds(
    seeding: &mut ParamSeeding,
    index: usize,
    base: &SurfInfo,
    plus: &SurfInfo,
    minus: &SurfInfo,
    h: f64,
) {
    let two_h = 2.0 * h;
    match (base, plus, minus) {
        (
            SurfInfo::Plane { normal, .. },
            SurfInfo::Plane { origin: op, .. },
            SurfInfo::Plane { origin: om, .. },
        ) => {
            // Only the normal component of the plane's motion is observable;
            // in-plane origin drift is gauge and vanishes under the dot.
            let rate = (*op - *om).dot(*normal) / two_h;
            let velocity = *normal * rate;
            push_translate(seeding, index, velocity);
        }
        (
            SurfInfo::Cylinder { axis, .. },
            SurfInfo::Cylinder {
                center: cp,
                radius: rp,
                ..
            },
            SurfInfo::Cylinder {
                center: cm,
                radius: rm,
                ..
            },
        ) => {
            let radius_rate = (rp - rm) / two_h;
            if radius_rate.abs() > ZERO_TOL {
                seeding.seed(index, SurfaceSeed::CylinderRadius { rate: radius_rate });
            }
            // Along-axis center motion is gauge (the fillet kernel slides the
            // center freely); keep only the radial part, in the base frame.
            let velocity = reject(*cp - *cm, *axis) / two_h;
            push_translate(seeding, index, velocity);
        }
        (
            SurfInfo::Sphere { .. },
            SurfInfo::Sphere {
                center: cp,
                radius: rp,
            },
            SurfInfo::Sphere {
                center: cm,
                radius: rm,
            },
        ) => {
            let radius_rate = (rp - rm) / two_h;
            if radius_rate.abs() > ZERO_TOL {
                seeding.seed(index, SurfaceSeed::SphereRadius { rate: radius_rate });
            }
            let velocity = (*cp - *cm) / two_h;
            push_translate(seeding, index, velocity);
        }
        (
            SurfInfo::Cone { .. },
            SurfInfo::Cone {
                apex: pp,
                half_angle: hp,
                ..
            },
            SurfInfo::Cone {
                apex: pm,
                half_angle: hm,
                ..
            },
        ) => {
            // The apex has no translational gauge: seed its full velocity.
            let angle_rate = (hp - hm) / two_h;
            if angle_rate.abs() > ZERO_TOL {
                seeding.seed(index, SurfaceSeed::ConeAngle { rate: angle_rate });
            }
            let velocity = (*pp - *pm) / two_h;
            push_translate(seeding, index, velocity);
        }
        (
            SurfInfo::Torus { .. },
            SurfInfo::Torus {
                center: cp,
                major: mp,
                minor: np,
                ..
            },
            SurfInfo::Torus {
                center: cm,
                major: mm,
                minor: nm,
                ..
            },
        ) => {
            let major_rate = (mp - mm) / two_h;
            if major_rate.abs() > ZERO_TOL {
                seeding.seed(index, SurfaceSeed::TorusMajorRadius { rate: major_rate });
            }
            let minor_rate = (np - nm) / two_h;
            if minor_rate.abs() > ZERO_TOL {
                seeding.seed(index, SurfaceSeed::TorusMinorRadius { rate: minor_rate });
            }
            let velocity = (*cp - *cm) / two_h;
            push_translate(seeding, index, velocity);
        }
        // find_match guarantees kinds agree, so the mixed arms are unreachable.
        _ => unreachable!("matched surfaces share a kind"),
    }
}

/// Push a `Translate` seed unless it is numerically zero in every component.
fn push_translate(seeding: &mut ParamSeeding, index: usize, velocity: Vec3) {
    if velocity.x.abs() > ZERO_TOL || velocity.y.abs() > ZERO_TOL || velocity.z.abs() > ZERO_TOL {
        seeding.seed(index, SurfaceSeed::Translate { velocity });
    }
}

/// Synthesize the [`ParamSeeding`] for parameter `k` of a build function, by
/// central-difference probing the *surface fields* at θ_k ± h.
///
/// The returned seeding is keyed by store index into `build(theta)`. Because
/// the boolean/fillet stores are order-nondeterministic, that seeding is only
/// meaningful against a base built **identically** — evaluate it against the
/// very `BRepSolid` the caller obtains from `build(theta)` (or a clone), never
/// a separately enumerated one. (In the seam's optimizer loop, `build(theta)`
/// is called once per iterate and both the plan and the seeding are derived
/// from that one instance.)
///
/// # Errors
///
/// - [`DiffError::ParameterOutOfRange`] if `k` is out of bounds.
/// - [`DiffError::Frozen`] with [`FrozenError::TopologyChanged`] if either
///   probe crosses a topology change (a differing signature, or a base
///   surface with no perturbed counterpart) — a subgradient with no
///   meaningful derivative.
/// - [`DiffError::AmbiguousMatch`] if two genuinely distinct surfaces both
///   fall within `MATCH_TOL` of a base surface (a violated feature-separation
///   assumption, surfaced rather than guessed).
/// - [`DiffError::UnsupportedSynthesis`] for a surface kind outside the seed
///   vocabulary (plane / cylinder / sphere / cone / torus).
pub fn synthesize_seeding(
    build: &dyn Fn(&[f64]) -> BRepSolid,
    theta: &[f64],
    k: usize,
    h: f64,
) -> Result<ParamSeeding, DiffError> {
    if k >= theta.len() {
        return Err(DiffError::ParameterOutOfRange {
            k,
            len: theta.len(),
        });
    }
    let base = build(theta);
    synthesize_one(build, theta, &base, k, h)
}

/// Synthesize a seeding per parameter of a build function in one base build.
///
/// Equivalent to calling [`synthesize_seeding`] for every `k`, but builds the
/// base once. As with [`synthesize_seeding`], the returned seedings key into
/// the internally built `build(theta)` and must be evaluated against a base
/// built identically.
pub fn synthesize_all(
    build: &dyn Fn(&[f64]) -> BRepSolid,
    theta: &[f64],
    h: f64,
) -> Result<Vec<ParamSeeding>, DiffError> {
    let base = build(theta);
    (0..theta.len())
        .map(|k| synthesize_one(build, theta, &base, k, h))
        .collect()
}

/// Core probe: given an already-built `base`, difference parameter `k`.
fn synthesize_one(
    build: &dyn Fn(&[f64]) -> BRepSolid,
    theta: &[f64],
    base: &BRepSolid,
    k: usize,
    h: f64,
) -> Result<ParamSeeding, DiffError> {
    let mut theta_plus = theta.to_vec();
    theta_plus[k] += h;
    let mut theta_minus = theta.to_vec();
    theta_minus[k] -= h;
    let plus = build(&theta_plus);
    let minus = build(&theta_minus);

    // Cheap first check: a changed signature is an unambiguous topology change.
    let base_sig = topology_signature(base);
    let plus_sig = topology_signature(&plus);
    let minus_sig = topology_signature(&minus);
    if plus_sig != base_sig {
        return Err(DiffError::Frozen(FrozenError::TopologyChanged {
            expected: base_sig,
            actual: plus_sig,
        }));
    }
    if minus_sig != base_sig {
        return Err(DiffError::Frozen(FrozenError::TopologyChanged {
            expected: base_sig,
            actual: minus_sig,
        }));
    }

    let base_infos = build_infos(base)?;
    let plus_infos = build_infos(&plus)?;
    let minus_infos = build_infos(&minus)?;

    let mut seeding = ParamSeeding::new();
    for (i, base_info) in base_infos.iter().enumerate() {
        let mp = find_match(i, base_info, base_sig, plus_sig, &plus_infos)?;
        let mm = find_match(i, base_info, base_sig, minus_sig, &minus_infos)?;
        debug_assert_eq!(base_info.kind(), mp.kind());
        debug_assert_eq!(base_info.kind(), mm.kind());
        extract_seeds(&mut seeding, i, base_info, &mp, &mm, h);
    }
    Ok(seeding)
}
