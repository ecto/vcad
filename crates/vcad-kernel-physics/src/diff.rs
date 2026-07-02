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
//!   silently incomplete. Extending to contacts means pulling `∂J/∂x` back
//!   onto the collision-mesh nodes through the M5 pullback
//!   ([`evaluate_with_pullback`](vcad_kernel_diff::evaluate_with_pullback)) —
//!   future work, noted in `docs/differentiable-seam-m8.md`.
//! - **Mass-property channel only.** θ is assumed to move geometry, hence mass
//!   properties. If θ also moves a **joint anchor / mount frame**, that
//!   sensitivity is *not* included here (the rollout applies mounts as fixed
//!   transforms). The same FD-on-scalars pattern extends to anchor coordinates
//!   when a model needs it.
//! - **Determinism.** phyz's ABA + semi-implicit Euler are deterministic;
//!   the FD estimate of `∂J/∂p` is only meaningful if the rollout closure is a
//!   pure function of its mass-property input (fixed initial state, fixed
//!   control law, fixed step count). Keep it so.

use phyz::math::{Mat3, Vec3 as PVec3};
use phyz::SpatialInertia;
use vcad_kernel_diff::{
    evaluate_with_sensitivity, mass_properties, mass_properties_with_derivative, DiffError,
    MassProperties, ParamSeeding,
};
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
    let n = theta.len();

    // 1. Nominal mass properties per body (positions-only seam), and the
    //    frozen plan / B-rep kept for the exact dp/dθ below.
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

    // Primal objective at the nominal properties.
    let j0 = rollout(&nominal);

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
            let jp = rollout(&work);

            let mut sm = nom.scalars();
            sm[j] -= h;
            work[i] = BodyMassProps::from_scalars(sm);
            let jm = rollout(&work);

            grad_p[j] = (jp - jm) / (2.0 * h);
        }
        // Restore body i for the next body's perturbations.
        work[i] = *nom;
        djdp.push(grad_p);
    }

    // 3 + 4. Exact dp/dθ from the seam, contracted with ∂J/∂p and summed
    //         over bodies.
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
