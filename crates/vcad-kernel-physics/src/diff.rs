//! Physics-rollout gradients: differentiating a phyz simulation objective
//! with respect to CAD parameters θ (milestone M8 of the differentiable
//! seam).
//!
//! # What this computes
//!
//! For a **contact-free articulated rigid-body rollout**, CAD geometry enters
//! the dynamics through exactly one channel: each body's **mass properties** —
//! mass, center of mass, and inertia tensor about the COM (10 scalars per
//! body). Everything downstream (Featherstone ABA, the integrator, the
//! objective) is a smooth function of those 10 scalars per body. That gives an
//! exact chain rule
//!
//! ```text
//! dJ/dθ = Σ_bodies  ∂J/∂p_body · dp_body/dθ
//! ```
//!
//! where `p_body` is the 10-vector `[m, c_x, c_y, c_z, I_xx, I_yy, I_zz,
//! I_xy, I_xz, I_yz]` (COM-frame inertia). The two factors come from two
//! different places, each computed the correct way for its side of the seam:
//!
//! - **`dp_body/dθ`** — *exact*, from the differentiable seam. The frozen
//!   plan plus a per-parameter [`ParamSeeding`] feeds
//!   [`mass_properties_with_derivative`], which carries dual numbers through
//!   the polynomial mass-property integrals: every field's θ-derivative in
//!   one pass, to machine precision. No remeshing, no topology risk.
//!
//! - **`∂J/∂p_body`** — *central finite differences on the mass-property
//!   scalars themselves*. Each scalar is perturbed ±h and the rollout is
//!   re-run (≈20 rollouts per body: 10 scalars × 2). This is cheap and
//!   robust: no CAD rebuild, no re-tessellation, no boolean/fillet
//!   combinatorics under perturbation — only the physics integrator re-runs,
//!   on a body whose inertia scalar moved by a hair.
//!
//! ## Why finite differences for `∂J/∂p`, not a phyz adjoint
//!
//! phyz ships `phyz-diff`, but it differentiates a single dynamics step with
//! respect to **state and control** (`∂(q',v')/∂(q,v,ctrl)`), not with respect
//! to **model inertia parameters**. The factor M8 needs — sensitivity of the
//! rollout objective to a body's mass/COM/inertia — is not exposed by phyz at
//! any version currently vendored. So the live, and only, path for
//! `∂J/∂p_body` is central FD on the mass-property scalars, exactly the
//! fallback the seam design anticipated. If a future phyz grows a
//! parameter-adjoint, it drops in behind this same factorization: replace the
//! FD loop in [`rollout_gradient`] with the analytic `∂J/∂p` and the chain is
//! unchanged.
//!
//! # Contract and boundaries
//!
//! - **Contact-free only.** The factorization is exact *because* geometry
//!   reaches the dynamics solely through mass properties. The moment collision
//!   geometry participates (a contact, a joint limit that bottoms out under
//!   load, a ground penalty), contact forces depend on the *surface* — a
//!   channel this gradient does not see. The rollout closure you pass **must**
//!   build a contact-free model; if it does not, the returned gradient is
//!   silently incomplete. The **surface skin** is built: objective terms that
//!   read the surface directly go through [`rollout_gradient_with_surface`]
//!   (exact, one M5 pullback per body), and a future contact adjoint's
//!   `∂J/∂x` plugs into [`surface_gradient`] unchanged. What remains
//!   phyz-side is the adjoint itself — producing `∂J_dyn/∂x` when contact
//!   forces act *during* the rollout.
//! - **Anchor channel.** If θ also moves a **joint anchor / mount frame** (a
//!   pivot hole whose position scales with the part), use
//!   [`rollout_gradient_with_anchors`]: the anchor coordinates enter the
//!   factorization as additional scalars — `∂J/∂anchor` by the same
//!   central-FD-rollout pattern, `d(anchor)/dθ` by central FD on the caller's
//!   anchor map (a pure function of θ, no geometry rebuild) — and the two
//!   channels sum. [`rollout_gradient`] is the anchor-free special case.
//! - **Determinism.** phyz's ABA + semi-implicit Euler are deterministic;
//!   the FD estimate of `∂J/∂p` is only meaningful if the rollout closure is a
//!   pure function of its mass-property input (fixed initial state, fixed
//!   control law, fixed step count). Keep it so.

use phyz::math::{Mat3, Vec3 as PVec3};
use phyz::SpatialInertia;
use vcad_kernel_diff::{
    evaluate_with_pullback, evaluate_with_sensitivity, mass_properties,
    mass_properties_with_derivative, DiffError, MassProperties, ParamSeeding,
};
use vcad_kernel_math::{Point3 as CadPoint3, Vec3 as CadVec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_tessellate::frozen::capture_plan;
use vcad_kernel_tessellate::TessellationParams;

/// Millimetres to metres.
const MM_TO_M: f64 = 1e-3;
/// mm² to m² (COM-frame inertia carries a squared length).
const MM2_TO_M2: f64 = 1e-6;
/// kg·m⁻³ to kg·mm⁻³ (1 m³ = 1e9 mm³), so seam integrals in mm come out in kg.
const KGM3_TO_KGMM3: f64 = 1e-9;

/// The mass properties of one rigid body, in **SI units** (kg, m, kg·m²),
/// expressed in the body's own CAD frame.
///
/// The inertia is about the **center of mass** (COM), matching
/// [`phyz::SpatialInertia`]. Field order for the packed 10-vector used by the
/// finite-difference loop is `[m, c_x, c_y, c_z, I_xx, I_yy, I_zz, I_xy,
/// I_xz, I_yz]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyMassProps {
    /// Mass (kg).
    pub mass: f64,
    /// Center of mass in the body frame (m).
    pub com: [f64; 3],
    /// Inertia about the COM, `[I_xx, I_yy, I_zz, I_xy, I_xz, I_yz]` (kg·m²).
    pub inertia: [f64; 6],
}

impl BodyMassProps {
    /// Pack into the canonical 10-vector `[m, c_x, c_y, c_z, I_xx, I_yy,
    /// I_zz, I_xy, I_xz, I_yz]`.
    pub fn scalars(&self) -> [f64; 10] {
        [
            self.mass,
            self.com[0],
            self.com[1],
            self.com[2],
            self.inertia[0],
            self.inertia[1],
            self.inertia[2],
            self.inertia[3],
            self.inertia[4],
            self.inertia[5],
        ]
    }

    /// Inverse of [`Self::scalars`].
    pub fn from_scalars(s: [f64; 10]) -> Self {
        Self {
            mass: s[0],
            com: [s[1], s[2], s[3]],
            inertia: [s[4], s[5], s[6], s[7], s[8], s[9]],
        }
    }

    /// Build a [`phyz::SpatialInertia`] (mass, COM offset, COM-frame inertia
    /// matrix) from these properties.
    pub fn to_spatial_inertia(&self) -> SpatialInertia {
        let [ixx, iyy, izz, ixy, ixz, iyz] = self.inertia;
        let m = Mat3::new(ixx, ixy, ixz, ixy, iyy, iyz, ixz, iyz, izz);
        SpatialInertia::new(
            self.mass,
            PVec3::new(self.com[0], self.com[1], self.com[2]),
            m,
        )
    }

    /// Convert seam mass properties (mm, seam density) to SI, given the body's
    /// physical density in kg/m³. `props.mass` is already in kg (the seam was
    /// fed `density · 1e-9`); the centroid and inertia carry length factors.
    fn from_seam(props: &MassProperties<f64>) -> Self {
        let c = props.centroid;
        let i = &props.inertia_centroid;
        Self {
            mass: props.mass,
            com: [c.x * MM_TO_M, c.y * MM_TO_M, c.z * MM_TO_M],
            inertia: [
                i[0][0] * MM2_TO_M2,
                i[1][1] * MM2_TO_M2,
                i[2][2] * MM2_TO_M2,
                i[0][1] * MM2_TO_M2,
                i[0][2] * MM2_TO_M2,
                i[1][2] * MM2_TO_M2,
            ],
        }
    }

    /// Convert a seam **derivative** of mass properties into the SI 10-vector
    /// `dp/dθ`. Same linear unit factors as [`Self::from_seam`] (θ is a length
    /// in mm, so the factors carry through unchanged).
    fn deriv_scalars(dprops: &MassProperties<f64>) -> [f64; 10] {
        let dc = dprops.centroid;
        let di = &dprops.inertia_centroid;
        [
            dprops.mass,
            dc.x * MM_TO_M,
            dc.y * MM_TO_M,
            dc.z * MM_TO_M,
            di[0][0] * MM2_TO_M2,
            di[1][1] * MM2_TO_M2,
            di[2][2] * MM2_TO_M2,
            di[0][1] * MM2_TO_M2,
            di[0][2] * MM2_TO_M2,
            di[1][2] * MM2_TO_M2,
        ]
    }
}

/// Finite-difference step sizes for the `∂J/∂p` estimate, one policy for each
/// class of mass-property scalar.
///
/// Central differences trade truncation error `O(h²)` against roundoff
/// `O(ε·|J|/h)`; the defaults sit near the `~1e-5` relative sweet spot for a
/// smooth `f64` rollout. Mass and inertia use a **relative** step (scaled to
/// the body's own magnitude); the COM uses an **absolute** step (a centered
/// body's COM is ~0, where a relative step would collapse to nothing).
#[derive(Debug, Clone, Copy)]
pub struct MassPropFdSteps {
    /// Relative step for the mass scalar.
    pub mass_rel: f64,
    /// Floor for the mass step (kg).
    pub mass_min: f64,
    /// Absolute step for each COM component (m).
    pub com_abs: f64,
    /// Relative step for inertia components, scaled to the body's reference
    /// inertia (max |diagonal|).
    pub inertia_rel: f64,
    /// Floor for the inertia step (kg·m²).
    pub inertia_min: f64,
}

impl Default for MassPropFdSteps {
    fn default() -> Self {
        Self {
            mass_rel: 1e-5,
            mass_min: 1e-12,
            com_abs: 1e-7,
            inertia_rel: 1e-5,
            inertia_min: 1e-18,
        }
    }
}

impl MassPropFdSteps {
    /// The 10 per-scalar step sizes for one body's nominal properties.
    fn steps_for(&self, p: &BodyMassProps) -> [f64; 10] {
        let mass_h = (self.mass_rel * p.mass.abs()).max(self.mass_min);
        let ref_i = p.inertia[0]
            .abs()
            .max(p.inertia[1].abs())
            .max(p.inertia[2].abs());
        let inertia_h = (self.inertia_rel * ref_i).max(self.inertia_min);
        [
            mass_h,
            self.com_abs,
            self.com_abs,
            self.com_abs,
            inertia_h,
            inertia_h,
            inertia_h,
            inertia_h,
            inertia_h,
            inertia_h,
        ]
    }
}

/// A rigid body whose mass properties depend on CAD parameters θ through the
/// differentiable seam.
///
/// The three fields are the CAD side of the M8 factorization: a build
/// function over θ, a per-parameter seeding source (hand-written or
/// [`synthesize_seeding`](vcad_kernel_diff::synthesize_seeding)), and the
/// physical density.
#[allow(clippy::type_complexity)]
pub struct DiffBody<'a> {
    /// Build this body's B-rep at θ (geometry in millimetres).
    pub build: Box<dyn Fn(&[f64]) -> BRepSolid + 'a>,
    /// Surface seeding for parameter `k`, given the freshly built B-rep and the
    /// current θ. Returns a `Result` so
    /// [`synthesize_seeding`](vcad_kernel_diff::synthesize_seeding) composes
    /// directly — `|_brep, theta, k| synthesize_seeding(&build, theta, k, h)` —
    /// while a hand-written seeding just reads the B-rep and wraps in `Ok`.
    ///
    /// The seeding must key into the **passed** `BRepSolid` (the one the
    /// adapter captured its plan from). A synthesized seeding satisfies this
    /// only when `build` is deterministic in its surface ordering; the
    /// hand-written path always does.
    #[allow(clippy::type_complexity)]
    pub seeding_for: Box<dyn Fn(&BRepSolid, &[f64], usize) -> Result<ParamSeeding, DiffError> + 'a>,
    /// Physical density (kg/m³).
    pub density_kg_m3: f64,
    /// Tessellation parameters for the frozen plan.
    pub tess: TessellationParams,
}

impl DiffBody<'_> {
    /// Seam density (kg/mm³): fed to the mm-frame integrals so mass comes out
    /// in kg.
    fn seam_density(&self) -> f64 {
        self.density_kg_m3 * KGM3_TO_KGMM3
    }
}

/// Objective value and full CAD-parameter gradient of a physics rollout,
/// `(J, dJ/dθ)`, via the mass-property factorization.
///
/// This is shaped exactly like
/// [`objective_gradient`](vcad_kernel_diff::objective_gradient) — a value plus
/// a `Vec<f64>` gradient, `Result` on the seam errors — so a projected
/// gradient-descent / L-BFGS driver can call it as its oracle.
///
/// # Arguments
///
/// - `bodies` — the differentiable rigid bodies. Body `i`'s mass properties
///   are `bodies[i]` evaluated at θ; the rollout receives them in the same
///   order.
/// - `rollout` — builds a **contact-free** phyz model from the per-body mass
///   properties, simulates a fixed trajectory, and returns the scalar
///   objective `J`. Must be deterministic and a pure function of its input
///   (see the module contract).
/// - `theta` — the CAD parameters.
/// - `fd` — finite-difference step policy for `∂J/∂p`.
///
/// # What runs
///
/// One frozen-plan capture per body, one positions-only seam pass per body
/// for the nominal properties, `n·10·2` rollouts for the per-body `∂J/∂p`
/// (`n` = number of bodies), and one seam pass per (body, parameter) for the
/// exact `dp/dθ`. The rollout count is independent of the θ dimension; adding
/// a CAD parameter costs one extra seam pass per body, nothing more.
///
/// # Errors
///
/// Any seam error — capture failure, a topology change under the frozen plan,
/// an inconsistent implicit vertex system, a synthesis ambiguity — surfaces as
/// [`DiffError`], exactly as the seam optimizer's does.
pub fn rollout_gradient(
    bodies: &[DiffBody],
    rollout: &impl Fn(&[BodyMassProps]) -> f64,
    theta: &[f64],
    fd: &MassPropFdSteps,
) -> Result<(f64, Vec<f64>), DiffError> {
    rollout_gradient_with_anchors(
        bodies,
        &|_theta: &[f64]| Vec::new(),
        &|props: &[BodyMassProps], _anchors: &[f64]| rollout(props),
        theta,
        fd,
        &AnchorFdSteps::default(),
    )
}

/// Finite-difference step policy for the **anchor channel** of
/// [`rollout_gradient_with_anchors`].
///
/// Two independent steps, because the two derivatives probe different maps:
/// `value_abs` perturbs an anchor scalar inside the *rollout* (`∂J/∂a`,
/// meters in the phyz frame), while `theta_rel`/`theta_min` step θ inside the
/// caller's *anchor map* (`da/dθ`, a pure function — usually linear — so the
/// step only needs to clear roundoff).
#[derive(Debug, Clone, Copy)]
pub struct AnchorFdSteps {
    /// Absolute step (m) for each anchor scalar in the `∂J/∂a` rollouts.
    pub value_abs: f64,
    /// Relative step (of `|θ_k|`) for the `da/dθ_k` central difference.
    pub theta_rel: f64,
    /// Floor for the θ step.
    pub theta_min: f64,
}

impl Default for AnchorFdSteps {
    fn default() -> Self {
        Self {
            value_abs: 1e-7,
            theta_rel: 1e-6,
            theta_min: 1e-9,
        }
    }
}

/// [`rollout_gradient`] with the **anchor channel**: `(J, dJ/dθ)` where θ
/// moves both the bodies' mass properties *and* joint anchor / mount-frame
/// coordinates.
///
/// `anchor_map` maps θ to a flat vector of anchor scalars (layout is the
/// caller's contract with its own `rollout` — e.g. `[pivot_x_m, pivot_y_m]`).
/// The chain gains one term per anchor scalar:
///
/// ```text
/// dJ/dθ = Σ_bodies ∂J/∂p·dp/dθ  +  Σ_anchors ∂J/∂a·da/dθ
/// ```
///
/// with `∂J/∂a` from central-FD rollouts (like `∂J/∂p` — no CAD rebuild) and
/// `da/dθ` from a central difference of `anchor_map` itself, which is a pure
/// function of θ (no geometry, no remeshing; for the common linear anchor —
/// a hole position proportional to a dimension — the FD is exact to
/// roundoff). Everything in the [`rollout_gradient`] contract carries over;
/// the rollout closure must be pure in *both* inputs.
pub fn rollout_gradient_with_anchors(
    bodies: &[DiffBody],
    anchor_map: &impl Fn(&[f64]) -> Vec<f64>,
    rollout: &impl Fn(&[BodyMassProps], &[f64]) -> f64,
    theta: &[f64],
    fd: &MassPropFdSteps,
    anchor_fd: &AnchorFdSteps,
) -> Result<(f64, Vec<f64>), DiffError> {
    let n = theta.len();

    // 1. Nominal mass properties per body (positions-only seam), and the
    //    frozen plan / B-rep kept for the exact dp/dθ below; nominal anchors.
    let mut breps = Vec::with_capacity(bodies.len());
    let mut plans = Vec::with_capacity(bodies.len());
    let mut nominal = Vec::with_capacity(bodies.len());
    for body in bodies {
        let brep = (body.build)(theta);
        let plan = capture_plan(&brep, &body.tess)?;
        let seam0 = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new())?;
        let props = mass_properties(&seam0.positions, &seam0.triangles, body.seam_density());
        nominal.push(BodyMassProps::from_seam(&props));
        breps.push(brep);
        plans.push(plan);
    }
    let anchors0 = anchor_map(theta);

    // Primal objective at the nominal properties and anchors.
    let j0 = rollout(&nominal, &anchors0);

    // 2. ∂J/∂p per body by central FD on the 10 mass-property scalars.
    //    Each body is perturbed independently; the others hold their nominal
    //    values, so the estimate is the true partial.
    let mut djdp: Vec<[f64; 10]> = Vec::with_capacity(bodies.len());
    for (i, nom) in nominal.iter().enumerate() {
        let steps = fd.steps_for(nom);
        let mut grad_p = [0.0f64; 10];
        let mut work = nominal.clone();
        for j in 0..10 {
            let h = steps[j];
            let mut sp = nom.scalars();
            sp[j] += h;
            work[i] = BodyMassProps::from_scalars(sp);
            let jp = rollout(&work, &anchors0);

            let mut sm = nom.scalars();
            sm[j] -= h;
            work[i] = BodyMassProps::from_scalars(sm);
            let jm = rollout(&work, &anchors0);

            grad_p[j] = (jp - jm) / (2.0 * h);
        }
        // Restore body i for the next body's perturbations.
        work[i] = *nom;
        djdp.push(grad_p);
    }

    // 2b. ∂J/∂a per anchor scalar, same central-FD-rollout pattern.
    let mut djda = vec![0.0f64; anchors0.len()];
    for (a, g) in djda.iter_mut().enumerate() {
        let h = anchor_fd.value_abs;
        let mut work = anchors0.clone();
        work[a] = anchors0[a] + h;
        let jp = rollout(&nominal, &work);
        work[a] = anchors0[a] - h;
        let jm = rollout(&nominal, &work);
        *g = (jp - jm) / (2.0 * h);
    }

    // 3 + 4. Exact dp/dθ from the seam, contracted with ∂J/∂p and summed
    //         over bodies; then the anchor channel — da/dθ by central FD on
    //         the (geometry-free) anchor map, contracted with ∂J/∂a.
    let mut gradient = vec![0.0f64; n];
    for (i, body) in bodies.iter().enumerate() {
        for (k, g) in gradient.iter_mut().enumerate() {
            let seeding = (body.seeding_for)(&breps[i], theta, k)?;
            let seam_k = evaluate_with_sensitivity(&breps[i], &plans[i], &seeding)?;
            let (_props, dprops) = mass_properties_with_derivative(&seam_k, body.seam_density());
            let dp = BodyMassProps::deriv_scalars(&dprops);
            let mut acc = 0.0;
            for j in 0..10 {
                acc += djdp[i][j] * dp[j];
            }
            *g += acc;
        }
    }
    if !anchors0.is_empty() {
        for (k, g) in gradient.iter_mut().enumerate() {
            let h = (anchor_fd.theta_rel * theta[k].abs()).max(anchor_fd.theta_min);
            let mut tp = theta.to_vec();
            tp[k] += h;
            let ap = anchor_map(&tp);
            let mut tm = theta.to_vec();
            tm[k] -= h;
            let am = anchor_map(&tm);
            debug_assert_eq!(
                ap.len(),
                anchors0.len(),
                "anchor map length must be θ-invariant"
            );
            for (a, dj) in djda.iter().enumerate() {
                *g += dj * (ap[a] - am[a]) / (2.0 * h);
            }
        }
    }

    Ok((j0, gradient))
}

/// A surface-dependent objective term on one body's frozen-plan mesh.
///
/// Given the seam node positions (mm, CAD frame) and the frozen triangles,
/// return the term's value and its analytic node gradient `∂J_surf/∂x`
/// (J-units per mm, one vector per node). This is the **contact skin** of the
/// M8 factorization: any objective contribution that reads the *surface*
/// rather than the mass properties — a ground-clearance penalty, a
/// penetration term, or (when one exists) a contact adjoint's `∂J/∂x`.
pub type SurfaceTerm<'a> = Box<dyn Fn(&[CadPoint3], &[[u32; 3]]) -> (f64, Vec<CadVec3>) + 'a>;

/// Price a mesh cotangent through the CAD seam: given `∂J/∂x` on `body`'s
/// frozen-plan nodes (mm frame), return the per-parameter `dJ/dθ`
/// contribution.
///
/// One M5 pullback ([`evaluate_with_pullback`]) prices the whole cotangent;
/// each parameter then costs only a seeding synthesis and a contraction —
/// the reverse-mode economics the skin was designed around. This is the raw
/// entry point a **contact adjoint** plugs into: whatever produces `∂J/∂x`
/// on the collision surface (an analytic penalty today, a phyz contact
/// adjoint when one exists), this function turns it into CAD-parameter
/// sensitivities.
pub fn surface_gradient(
    body: &DiffBody,
    theta: &[f64],
    djdx: &[CadVec3],
) -> Result<Vec<f64>, DiffError> {
    let brep = (body.build)(theta);
    let plan = capture_plan(&brep, &body.tess)?;
    let cots = evaluate_with_pullback(&brep, &plan, djdx)?;
    let mut out = Vec::with_capacity(theta.len());
    for k in 0..theta.len() {
        let seeding = (body.seeding_for)(&brep, theta, k)?;
        out.push(cots.contract(&seeding));
    }
    Ok(out)
}

/// [`rollout_gradient`] with the **surface skin**: `(J, dJ/dθ)` for a
/// composite objective
///
/// ```text
/// J = J_dyn(mass props)  +  Σ_bodies J_surf(surface nodes)
/// ```
///
/// where `J_dyn` is the contact-free rollout (mass-property channel, exactly
/// as [`rollout_gradient`]) and each `J_surf` reads the body's tessellated
/// surface directly. The gradient sums both channels:
///
/// ```text
/// dJ/dθ = Σ ∂J/∂p·dp/dθ  +  Σ pullback(∂J_surf/∂x)·seeding
/// ```
///
/// The surface channel is **exact** (analytic node gradient through the M5
/// pullback — no FD anywhere in it) and costs one pullback per body
/// regardless of the θ dimension.
///
/// `surface_terms` is parallel to `bodies`; `None` for bodies without a
/// surface term. What this does **not** do: surface-dependent *dynamics*.
/// If contact forces act during the rollout, `∂J_dyn/∂(surface)` needs a
/// contact adjoint phyz does not currently expose; when one exists, its
/// `∂J/∂x` enters through [`surface_gradient`] and the factorization is
/// unchanged (documented in `docs/differentiable-seam-m8.md`).
pub fn rollout_gradient_with_surface(
    bodies: &[DiffBody],
    surface_terms: &[Option<SurfaceTerm>],
    rollout: &impl Fn(&[BodyMassProps]) -> f64,
    theta: &[f64],
    fd: &MassPropFdSteps,
) -> Result<(f64, Vec<f64>), DiffError> {
    assert_eq!(
        surface_terms.len(),
        bodies.len(),
        "one surface-term slot per body"
    );
    let n = theta.len();

    // 1. Nominal seam per body; keep positions for the surface terms and the
    //    B-rep/plan for both channels' seam passes.
    let mut breps = Vec::with_capacity(bodies.len());
    let mut plans = Vec::with_capacity(bodies.len());
    let mut nominal = Vec::with_capacity(bodies.len());
    let mut j_surf = 0.0;
    let mut cotangents = Vec::with_capacity(bodies.len());
    for (body, term) in bodies.iter().zip(surface_terms) {
        let brep = (body.build)(theta);
        let plan = capture_plan(&brep, &body.tess)?;
        let seam0 = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new())?;
        let props = mass_properties(&seam0.positions, &seam0.triangles, body.seam_density());
        nominal.push(BodyMassProps::from_seam(&props));
        // Surface term: value into J, node gradient priced by one pullback.
        cotangents.push(match term {
            Some(t) => {
                let (value, djdx) = t(&seam0.positions, &seam0.triangles);
                j_surf += value;
                Some(evaluate_with_pullback(&brep, &plan, &djdx)?)
            }
            None => None,
        });
        breps.push(brep);
        plans.push(plan);
    }

    let j0 = rollout(&nominal) + j_surf;

    // 2. ∂J_dyn/∂p per body by central FD (J_surf does not depend on p, so
    //    perturbing the scalars probes exactly the dynamic term).
    let mut djdp: Vec<[f64; 10]> = Vec::with_capacity(bodies.len());
    for (i, nom) in nominal.iter().enumerate() {
        let steps = fd.steps_for(nom);
        let mut grad_p = [0.0f64; 10];
        let mut work = nominal.clone();
        for j in 0..10 {
            let h = steps[j];
            let mut sp = nom.scalars();
            sp[j] += h;
            work[i] = BodyMassProps::from_scalars(sp);
            let jp = rollout(&work);

            let mut sm = nom.scalars();
            sm[j] -= h;
            work[i] = BodyMassProps::from_scalars(sm);
            let jm = rollout(&work);

            grad_p[j] = (jp - jm) / (2.0 * h);
        }
        work[i] = *nom;
        djdp.push(grad_p);
    }

    // 3. Per (body, parameter): the mass-property channel (forward seam) and
    //    the surface channel (contraction of the body's one pullback).
    let mut gradient = vec![0.0f64; n];
    for (i, body) in bodies.iter().enumerate() {
        for (k, g) in gradient.iter_mut().enumerate() {
            let seeding = (body.seeding_for)(&breps[i], theta, k)?;
            let seam_k = evaluate_with_sensitivity(&breps[i], &plans[i], &seeding)?;
            let (_props, dprops) = mass_properties_with_derivative(&seam_k, body.seam_density());
            let dp = BodyMassProps::deriv_scalars(&dprops);
            let mut acc = 0.0;
            for j in 0..10 {
                acc += djdp[i][j] * dp[j];
            }
            if let Some(cots) = &cotangents[i] {
                acc += cots.contract(&seeding);
            }
            *g += acc;
        }
    }

    Ok((j0, gradient))
}

/// The nominal SI mass properties of each body at θ (positions-only seam).
///
/// A convenience for callers that want the primal properties — to build the
/// rollout model at θ, or to check a target QoI — without the gradient.
pub fn nominal_mass_props(
    bodies: &[DiffBody],
    theta: &[f64],
) -> Result<Vec<BodyMassProps>, DiffError> {
    let mut out = Vec::with_capacity(bodies.len());
    for body in bodies {
        let brep = (body.build)(theta);
        let plan = capture_plan(&brep, &body.tess)?;
        let seam = evaluate_with_sensitivity(&brep, &plan, &ParamSeeding::new())?;
        let props = mass_properties(&seam.positions, &seam.triangles, body.seam_density());
        out.push(BodyMassProps::from_seam(&props));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_inertia_roundtrip() {
        let p = BodyMassProps {
            mass: 2.0,
            com: [0.1, -0.2, 0.3],
            inertia: [1.0, 2.0, 3.0, 0.1, 0.2, 0.3],
        };
        let si = p.to_spatial_inertia();
        assert!((si.mass - 2.0).abs() < 1e-15);
        assert!((si.com.x - 0.1).abs() < 1e-15);
        // Symmetric off-diagonals mirror into the matrix.
        assert!((si.inertia[(0, 1)] - 0.1).abs() < 1e-15);
        assert!((si.inertia[(1, 0)] - 0.1).abs() < 1e-15);
        assert!((si.inertia[(2, 2)] - 3.0).abs() < 1e-15);
        // Pack / unpack.
        assert_eq!(BodyMassProps::from_scalars(p.scalars()), p);
    }
}
