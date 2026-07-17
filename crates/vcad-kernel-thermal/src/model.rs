//! The thermal problem description: domain, regions, boundary conditions.
//!
//! A [`ThermalModel`] is a uniform voxel grid over an axis-aligned bounding
//! box. Geometry is *painted* onto the grid: each voxel takes the material
//! of the **last** region whose shape contains its center (painter's
//! order), and voxels covered by no material region are void — perfectly
//! insulating gaps that the solver excludes from the system. Power sources
//! and fixed-temperature reservoirs are regions too.
//!
//! Everything here is fail-closed: a source that lands on no conducting
//! voxel, a reservoir that pins nothing, a non-positive conductivity or
//! film coefficient — each is an error at solve time, never a silent drop.

use serde::{Deserialize, Serialize};

/// A coordinate axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// The x axis.
    X,
    /// The y axis.
    Y,
    /// The z axis (vcad vertical).
    Z,
}

impl Axis {
    /// Index of this axis (x = 0, y = 1, z = 2).
    pub fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
}

/// An axis-aligned region shape, in millimeters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Shape {
    /// Axis-aligned box: minimum corner and size.
    Box {
        /// Minimum corner, mm.
        min_mm: [f64; 3],
        /// Extent along each axis, mm (all > 0 for a non-empty box).
        size_mm: [f64; 3],
    },
    /// Axis-aligned tube (a solid cylinder when `inner_radius_mm` = 0).
    ///
    /// `center_mm` holds the two cross-axis coordinates of the tube axis in
    /// ascending axis order: for `axis = Z` that is `[x, y]`, for `Y` it is
    /// `[x, z]`, for `X` it is `[y, z]`. `span_mm` is the extent along the
    /// tube axis.
    Tube {
        /// Tube axis direction.
        axis: Axis,
        /// Cross-axis coordinates of the axis line, mm (ascending axis order).
        center_mm: [f64; 2],
        /// `[lo, hi]` extent along the axis, mm.
        span_mm: [f64; 2],
        /// Outer radius, mm.
        outer_radius_mm: f64,
        /// Inner radius, mm (0 for a solid cylinder).
        inner_radius_mm: f64,
    },
}

impl Shape {
    /// Does this shape contain the point `p_mm` (closed boundaries)?
    pub fn contains(&self, p_mm: [f64; 3]) -> bool {
        match self {
            Shape::Box { min_mm, size_mm } => {
                (0..3).all(|a| p_mm[a] >= min_mm[a] && p_mm[a] <= min_mm[a] + size_mm[a])
            }
            Shape::Tube {
                axis,
                center_mm,
                span_mm,
                outer_radius_mm,
                inner_radius_mm,
            } => {
                let a = axis.index();
                if p_mm[a] < span_mm[0] || p_mm[a] > span_mm[1] {
                    return false;
                }
                let (c0, c1) = match a {
                    0 => (p_mm[1], p_mm[2]),
                    1 => (p_mm[0], p_mm[2]),
                    _ => (p_mm[0], p_mm[1]),
                };
                let rho2 = (c0 - center_mm[0]).powi(2) + (c1 - center_mm[1]).powi(2);
                rho2 <= outer_radius_mm * outer_radius_mm
                    && rho2 >= inner_radius_mm * inner_radius_mm
            }
        }
    }
}

/// A material region: a shape filled with a (possibly anisotropic)
/// conductor.
///
/// Conductivity is a per-axis diagonal tensor `[k_x, k_y, k_z]` — the case
/// that matters for boards, where copper planes make in-plane conduction
/// ~30–60× stronger than through-plane (e.g. `[15, 15, 0.4]` for a
/// multilayer FR4 board vs `[0.3; 3]` for bare FR4). Off-diagonal
/// conductivity (rotated laminates) is out of scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialRegion {
    /// Region shape.
    pub shape: Shape,
    /// Thermal conductivity per axis, W/(m·K). All components must be
    /// positive — model a non-conductor by *not painting* a region there
    /// (void), not with k = 0.
    pub k_w_mk: [f64; 3],
    /// Volumetric heat capacity ρ·c_p, J/(m³·K). Steady solves ignore it;
    /// transient solves require it on every solid voxel (fail-closed).
    pub heat_capacity_j_m3k: Option<f64>,
}

impl MaterialRegion {
    /// An isotropic conductor with no heat capacity (steady-state only).
    pub fn isotropic(shape: Shape, k_w_mk: f64) -> Self {
        Self {
            shape,
            k_w_mk: [k_w_mk; 3],
            heat_capacity_j_m3k: None,
        }
    }

    /// An anisotropic conductor with no heat capacity (steady-state only).
    pub fn anisotropic(shape: Shape, k_w_mk: [f64; 3]) -> Self {
        Self {
            shape,
            k_w_mk,
            heat_capacity_j_m3k: None,
        }
    }

    /// Attach a volumetric heat capacity ρ·c_p (J/(m³·K)) for transient
    /// solves.
    pub fn with_heat_capacity(mut self, rc_j_m3k: f64) -> Self {
        self.heat_capacity_j_m3k = Some(rc_j_m3k);
        self
    }
}

/// A volumetric power source: `power_w` watts distributed uniformly over
/// the free (conducting, not temperature-pinned) voxels its shape covers.
///
/// Negative power is allowed (a thermoelectric cooler pumps heat out).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PowerSource {
    /// Name used in per-source reporting ("U7", "die", …).
    pub name: String,
    /// Region the power is deposited into.
    pub shape: Shape,
    /// Total power, watts.
    pub power_w: f64,
}

/// A fixed-temperature (Dirichlet) reservoir: every solid voxel whose
/// center falls inside is pinned to `temperature_c` and removed from the
/// unknowns. Later reservoirs win where regions overlap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixedTemperature {
    /// Region to pin.
    pub shape: Shape,
    /// Pinned temperature, °C.
    pub temperature_c: f64,
}

/// A boundary condition on solid surface faces.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Boundary {
    /// No heat crosses the face (the default surface).
    Adiabatic,
    /// The face is held at a temperature (Dirichlet). The half-cell
    /// conduction resistance from the voxel center to the face is included.
    FixedTemperature {
        /// Surface temperature, °C.
        temperature_c: f64,
    },
    /// Newton cooling q = h·(T_surface − T_ambient) (Robin). The half-cell
    /// conduction resistance is in series with the 1/h film resistance, so
    /// h → ∞ recovers the fixed-temperature limit.
    Convection {
        /// Film coefficient, W/(m²·K). Must be > 0. This number is
        /// *supplied, not derived* — it is the dominant uncertainty in any
        /// prediction built on it.
        h_w_m2k: f64,
        /// Ambient temperature, °C.
        ambient_c: f64,
    },
}

/// A steady conduction problem on a uniform voxel grid.
///
/// The domain box starts at `origin_mm` and extends `size_mm`, divided
/// into `divisions` voxels per axis. The six domain-face boundary
/// conditions are indexed `[-x, +x, -y, +y, -z, +z]` (see [`face_index`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermalModel {
    /// Minimum corner of the domain box, mm.
    pub origin_mm: [f64; 3],
    /// Domain extent along each axis, mm.
    pub size_mm: [f64; 3],
    /// Voxel count along each axis.
    pub divisions: [usize; 3],
    /// Material regions, painter's order (later wins).
    pub materials: Vec<MaterialRegion>,
    /// Power sources.
    pub sources: Vec<PowerSource>,
    /// Fixed-temperature reservoirs, painter's order (later wins).
    pub fixed: Vec<FixedTemperature>,
    /// Boundary condition per domain face, `[-x, +x, -y, +y, -z, +z]`.
    pub domain_faces: [Boundary; 6],
    /// Boundary condition applied to solid faces that touch a void voxel
    /// (an exposed internal surface).
    pub exposed: Boundary,
    /// Reference temperature for θ = (T_source,max − T_ref)/P. `None`
    /// derives it from the convection ambients, which must then all agree;
    /// with sources present and no resolvable reference, solving fails
    /// closed rather than guessing.
    pub reference_c: Option<f64>,
}

impl ThermalModel {
    /// A model with the given box and grid and no regions: all faces
    /// adiabatic, exposed faces adiabatic, no reference override.
    pub fn new(origin_mm: [f64; 3], size_mm: [f64; 3], divisions: [usize; 3]) -> Self {
        Self {
            origin_mm,
            size_mm,
            divisions,
            materials: Vec::new(),
            sources: Vec::new(),
            fixed: Vec::new(),
            domain_faces: [Boundary::Adiabatic; 6],
            exposed: Boundary::Adiabatic,
            reference_c: None,
        }
    }

    /// Voxel edge lengths, mm.
    pub fn voxel_mm(&self) -> [f64; 3] {
        [
            self.size_mm[0] / self.divisions[0] as f64,
            self.size_mm[1] / self.divisions[1] as f64,
            self.size_mm[2] / self.divisions[2] as f64,
        ]
    }

    /// Center of voxel `(i, j, k)`, mm.
    pub fn voxel_center_mm(&self, i: usize, j: usize, k: usize) -> [f64; 3] {
        let d = self.voxel_mm();
        [
            self.origin_mm[0] + (i as f64 + 0.5) * d[0],
            self.origin_mm[1] + (j as f64 + 0.5) * d[1],
            self.origin_mm[2] + (k as f64 + 0.5) * d[2],
        ]
    }
}

/// Domain-face index for `axis` (0..3) on the negative (`false`) or
/// positive (`true`) side: `[-x, +x, -y, +y, -z, +z]`.
pub fn face_index(axis: usize, positive: bool) -> usize {
    axis * 2 + usize::from(positive)
}

/// Model validation failures (all fail-closed).
#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    /// A division is zero or an extent is not positive.
    EmptyDomain,
    /// A material region has conductivity ≤ 0 (index into `materials`).
    NonPositiveConductivity {
        /// Index of the offending region in `ThermalModel::materials`.
        index: usize,
    },
    /// A convection boundary has h ≤ 0.
    NonPositiveFilmCoefficient,
    /// No voxel got a material — the grid is entirely void.
    NoSolidVoxels,
    /// A power source covers no free solid voxel; its power would vanish.
    SourceCoversNoFreeSolid {
        /// Name of the offending source.
        name: String,
    },
    /// A fixed-temperature region pins no solid voxel.
    FixedCoversNoSolid {
        /// Index of the offending region in `ThermalModel::fixed`.
        index: usize,
    },
    /// A transient solve needs ρc_p on every solid voxel, but a material
    /// region that owns voxels declared none (or a non-positive value).
    MissingHeatCapacity {
        /// Index of the offending region in `ThermalModel::materials`.
        index: usize,
    },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::EmptyDomain => {
                write!(
                    f,
                    "domain must have positive size and at least 1 voxel per axis"
                )
            }
            ModelError::NonPositiveConductivity { index } => write!(
                f,
                "material region {index} has a conductivity component <= 0; model insulation as \
                 void, not k = 0"
            ),
            ModelError::NonPositiveFilmCoefficient => {
                write!(f, "convection boundary requires h > 0")
            }
            ModelError::NoSolidVoxels => {
                write!(f, "no voxel is covered by any material region")
            }
            ModelError::SourceCoversNoFreeSolid { name } => write!(
                f,
                "power source {name:?} covers no free solid voxel; its power would be silently lost"
            ),
            ModelError::FixedCoversNoSolid { index } => {
                write!(f, "fixed-temperature region {index} pins no solid voxel")
            }
            ModelError::MissingHeatCapacity { index } => write!(
                f,
                "transient solve requires a positive heat capacity on every solid voxel; \
                 material region {index} declared none"
            ),
        }
    }
}

impl std::error::Error for ModelError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_containment_is_closed() {
        let b = Shape::Box {
            min_mm: [0.0, 0.0, 0.0],
            size_mm: [10.0, 20.0, 30.0],
        };
        assert!(b.contains([0.0, 0.0, 0.0]));
        assert!(b.contains([10.0, 20.0, 30.0]));
        assert!(b.contains([5.0, 5.0, 5.0]));
        assert!(!b.contains([10.1, 5.0, 5.0]));
        assert!(!b.contains([-0.1, 5.0, 5.0]));
    }

    #[test]
    fn tube_containment_and_axis_conventions() {
        // Z tube centered at (x, y) = (5, -5), annulus 2..4, z in [0, 10].
        let t = Shape::Tube {
            axis: Axis::Z,
            center_mm: [5.0, -5.0],
            span_mm: [0.0, 10.0],
            outer_radius_mm: 4.0,
            inner_radius_mm: 2.0,
        };
        assert!(t.contains([8.0, -5.0, 5.0])); // rho = 3
        assert!(!t.contains([5.0, -5.0, 5.0])); // on axis, inside the bore
        assert!(!t.contains([8.0, -5.0, 11.0])); // past the span
        assert!(!t.contains([9.5, -5.0, 5.0])); // rho = 4.5 > 4

        // X tube: center_mm is [y, z].
        let tx = Shape::Tube {
            axis: Axis::X,
            center_mm: [1.0, 2.0],
            span_mm: [-1.0, 1.0],
            outer_radius_mm: 0.5,
            inner_radius_mm: 0.0,
        };
        assert!(tx.contains([0.0, 1.0, 2.0]));
        assert!(tx.contains([0.0, 1.4, 2.0]));
        assert!(!tx.contains([0.0, 1.0, 2.6]));
    }

    #[test]
    fn voxel_centers_land_mid_cell() {
        let m = ThermalModel::new([-5.0, 0.0, 0.0], [10.0, 4.0, 2.0], [10, 4, 2]);
        assert_eq!(m.voxel_mm(), [1.0, 1.0, 1.0]);
        assert_eq!(m.voxel_center_mm(0, 0, 0), [-4.5, 0.5, 0.5]);
        assert_eq!(m.voxel_center_mm(9, 3, 1), [4.5, 3.5, 1.5]);
    }

    #[test]
    fn face_index_convention() {
        assert_eq!(face_index(0, false), 0);
        assert_eq!(face_index(0, true), 1);
        assert_eq!(face_index(2, true), 5);
    }
}
