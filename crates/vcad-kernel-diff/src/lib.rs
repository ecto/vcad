#![warn(missing_docs)]

//! Differentiable seam for the vcad kernel: sensitivities of tessellated
//! mesh-node positions to CAD parameters (**dx/dθ**), computed the cheap
//! correct way.
//!
//! The chain is cut **at the mesh, not inside the B-rep combinatorics**: no
//! derivative is ever pushed through boolean intersection, classification,
//! or trimming code. Instead:
//!
//! - **Topology is frozen.** All discrete choices are made on the primal θ
//!   by [`vcad_kernel_tessellate::frozen`]; a topology-signature check turns
//!   any topology change under perturbation into a hard error.
//! - **Interior surface samples** (`NodeRecipe::SurfaceUv`) are forward-mode
//!   AD: the stored `f64` surface is lifted to `Dual<f64>` via the
//!   lift-bridge ([`lift_surface`]), the θ-dependent fields are seeded with
//!   dual parts ([`SurfaceSeed`]), and evaluation at the frozen `(u, v)`
//!   yields position and velocity together (Pillar 2).
//! - **Topology-vertex samples** (`NodeRecipe::TopoVertex`) sit on the
//!   intersection of adjacent surfaces and are differentiated implicitly:
//!   each adjacent surface contributes a row `∇g · ẋ = −∂g/∂θ` of a linear
//!   system; underdetermined directions (e.g. the tangential slide of a rim
//!   node) are frozen at zero, which is exactly the frozen-parameter branch
//!   choice (Pillar 3).
//!
//! Everything is validated against a central-difference oracle rebuilt under
//! the same frozen plan (see the milestone integration tests in `tests/`).

use std::collections::BTreeMap;

use vcad_kernel_geom::{GeometryStore, Surface, SurfaceKind};
use vcad_kernel_math::Vec3;
use vcad_kernel_tessellate::frozen::FrozenError;

mod contract;
mod fd;
mod implicit;
mod lift;
mod mass;
mod optimize;
mod seam;

pub use contract::{contract_sensitivity, volume_gradient};
pub use fd::{compare_velocities, fd_velocities, fd_volume_derivative, FdComparison};
pub use implicit::{constraint_row, solve_vertex_velocity, surface_residual, ConstraintRow};
pub use lift::{lift_surface, DualSurface};
pub use mass::{mass_properties, mass_properties_with_derivative, MassProperties};
pub use optimize::{
    minimize, objective_gradient, IterateRecord, OptimizeOptions, OptimizeResult, StopReason,
};
pub use seam::{evaluate_with_sensitivity, volume_with_derivative, SeamMesh};
pub use vcad_kernel_tessellate::frozen::mesh_volume;

/// Downcast a `dyn Surface` to its concrete struct, with the store's
/// reported kind carried into the error.
pub(crate) fn downcast<T: 'static>(
    surface: &dyn Surface,
    kind: SurfaceKind,
) -> Result<&T, DiffError> {
    surface
        .as_any()
        .downcast_ref::<T>()
        .ok_or(DiffError::DowncastFailed(kind))
}

/// Errors from the differentiable seam.
#[derive(Debug)]
pub enum DiffError {
    /// Frozen-tessellation error (topology change, unsupported capture, …).
    Frozen(FrozenError),
    /// A surface reported one kind but failed to downcast to its concrete
    /// struct (internal inconsistency in the geometry store).
    DowncastFailed(SurfaceKind),
    /// The requested seed does not apply to this surface kind (e.g. a radius
    /// seed on a plane). Seeding is deliberately explicit — a silently
    /// ignored seed would produce a plausible wrong derivative.
    UnsupportedSeed {
        /// The surface kind the seed was applied to.
        kind: SurfaceKind,
        /// The offending seed.
        seed: SurfaceSeed,
    },
    /// No implicit constraint form is implemented for this surface kind, so
    /// a topology vertex adjacent to it cannot be differentiated.
    UnsupportedConstraint(SurfaceKind),
    /// The implicit system at a topology vertex is inconsistent: dependent
    /// constraint rows disagree beyond tolerance. The vertex does not
    /// actually lie on a common intersection of its adjacent surfaces (or a
    /// seed is wrong).
    InconsistentConstraints {
        /// Residual of the dependent row after projection.
        residual: f64,
    },
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::Frozen(e) => write!(f, "{e}"),
            DiffError::DowncastFailed(kind) => {
                write!(f, "surface of kind {kind:?} failed to downcast")
            }
            DiffError::UnsupportedSeed { kind, seed } => {
                write!(f, "seed {seed:?} is not applicable to surface kind {kind:?}")
            }
            DiffError::UnsupportedConstraint(kind) => write!(
                f,
                "no implicit constraint form for surface kind {kind:?}; cannot differentiate an adjacent topology vertex"
            ),
            DiffError::InconsistentConstraints { residual } => write!(
                f,
                "implicit vertex system inconsistent (residual {residual:.3e}); vertex is not on the common intersection of its adjacent surfaces"
            ),
        }
    }
}

impl std::error::Error for DiffError {}

impl From<FrozenError> for DiffError {
    fn from(e: FrozenError) -> Self {
        DiffError::Frozen(e)
    }
}

/// How the CAD parameter θ perturbs one stored surface: the explicit
/// θ → field seeding of the lift-bridge.
///
/// Velocities are in mm per unit θ; rates are dimensionless (d field / dθ).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceSeed {
    /// The surface translates rigidly at `velocity` as θ varies (seeds the
    /// origin/center/apex/control points, depending on kind).
    Translate {
        /// d(origin)/dθ.
        velocity: Vec3,
    },
    /// The cylinder radius varies: d(radius)/dθ = `rate`.
    CylinderRadius {
        /// d(radius)/dθ.
        rate: f64,
    },
    /// The sphere radius varies: d(radius)/dθ = `rate`.
    SphereRadius {
        /// d(radius)/dθ.
        rate: f64,
    },
}

/// Maps surface indices of a [`GeometryStore`] to their θ-seeds.
///
/// Surfaces without an entry are θ-independent (lifted with zero dual
/// parts). This keeps the seeding honest about which surfaces a given θ
/// actually touches.
#[derive(Debug, Clone, Default)]
pub struct ParamSeeding {
    seeds: BTreeMap<usize, SurfaceSeed>,
}

impl ParamSeeding {
    /// Empty seeding: every surface is θ-independent.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the surface at `surface_index`.
    pub fn seed(&mut self, surface_index: usize, seed: SurfaceSeed) -> &mut Self {
        self.seeds.insert(surface_index, seed);
        self
    }

    /// The seed for a surface index, if any.
    pub fn get(&self, surface_index: usize) -> Option<SurfaceSeed> {
        self.seeds.get(&surface_index).copied()
    }

    /// Seed every surface in `geom` matching `pred`. Returns how many
    /// surfaces were seeded (callers should assert the expected count —
    /// boolean output stores may carry several copies of a moving surface).
    pub fn seed_where(
        &mut self,
        geom: &GeometryStore,
        pred: impl Fn(&dyn Surface) -> bool,
        seed: SurfaceSeed,
    ) -> usize {
        let mut count = 0;
        for (i, s) in geom.surfaces.iter().enumerate() {
            if pred(s.as_ref()) {
                self.seeds.insert(i, seed);
                count += 1;
            }
        }
        count
    }
}
