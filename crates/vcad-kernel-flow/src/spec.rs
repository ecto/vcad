//! Serializable flow problem: JSON in, [`FlowModel`] out.
//!
//! Geometry is painted: the grid starts as `background` (solid by
//! default) and regions land in order, later regions winning — the same
//! painter's convention as `vcad-kernel-thermal`. Externally-voxelized
//! occupancy (the conjugate-grid seam) bypasses this module and writes
//! [`FlowModel::cells`] directly.

use serde::{Deserialize, Serialize};

use crate::model::{Cell, FlowModel, Fluid};

/// Fail-closed spec errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SpecError {
    /// A region shape has non-finite or negative dimensions.
    BadShape(String),
    /// The resolved model failed validation.
    Model(crate::model::ModelError),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::BadShape(why) => write!(f, "bad shape: {why}"),
            SpecError::Model(e) => write!(f, "resolved model invalid: {e}"),
        }
    }
}

impl std::error::Error for SpecError {}

/// Grid axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// X.
    X,
    /// Y.
    Y,
    /// Z.
    Z,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
        }
    }
}

/// Paintable region shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ShapeSpec {
    /// Axis-aligned box.
    Box {
        /// Minimum corner, mm.
        min_mm: [f64; 3],
        /// Extent per axis, mm.
        size_mm: [f64; 3],
    },
    /// Axis-aligned tube (cylinder when the inner radius is 0).
    Tube {
        /// Tube axis.
        axis: Axis,
        /// Cross-axis center, mm (ascending axis order).
        center_mm: [f64; 2],
        /// `[lo, hi]` extent along the axis, mm.
        span_mm: [f64; 2],
        /// Outer radius, mm.
        outer_radius_mm: f64,
        /// Inner radius, mm.
        inner_radius_mm: f64,
    },
}

impl ShapeSpec {
    fn check(&self) -> Result<(), SpecError> {
        match self {
            ShapeSpec::Box { min_mm, size_mm } => {
                if !min_mm.iter().chain(size_mm.iter()).all(|v| v.is_finite()) {
                    return Err(SpecError::BadShape("box has non-finite values".into()));
                }
                if size_mm.iter().any(|s| *s <= 0.0) {
                    return Err(SpecError::BadShape("box size must be positive".into()));
                }
            }
            ShapeSpec::Tube {
                center_mm,
                span_mm,
                outer_radius_mm,
                inner_radius_mm,
                ..
            } => {
                if !center_mm
                    .iter()
                    .chain(span_mm.iter())
                    .chain([outer_radius_mm, inner_radius_mm])
                    .all(|v| v.is_finite())
                {
                    return Err(SpecError::BadShape("tube has non-finite values".into()));
                }
                if *outer_radius_mm <= 0.0
                    || *inner_radius_mm < 0.0
                    || inner_radius_mm >= outer_radius_mm
                    || span_mm[1] <= span_mm[0]
                {
                    return Err(SpecError::BadShape(
                        "tube needs 0 <= inner < outer radius and span hi > lo".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn contains(&self, p_mm: [f64; 3]) -> bool {
        match self {
            ShapeSpec::Box { min_mm, size_mm } => {
                (0..3).all(|a| p_mm[a] >= min_mm[a] && p_mm[a] < min_mm[a] + size_mm[a])
            }
            ShapeSpec::Tube {
                axis,
                center_mm,
                span_mm,
                outer_radius_mm,
                inner_radius_mm,
            } => {
                let ai = axis.index();
                if p_mm[ai] < span_mm[0] || p_mm[ai] >= span_mm[1] {
                    return false;
                }
                let cross: Vec<usize> = (0..3).filter(|a| *a != ai).collect();
                let dx = p_mm[cross[0]] - center_mm[0];
                let dy = p_mm[cross[1]] - center_mm[1];
                let r2 = dx * dx + dy * dy;
                r2 < outer_radius_mm * outer_radius_mm && r2 >= inner_radius_mm * inner_radius_mm
            }
        }
    }
}

/// What a painted region makes its voxels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Wall.
    Solid,
    /// Fluid.
    Fluid,
    /// Velocity inlet.
    Inlet,
    /// Pressure outlet.
    Outlet,
}

impl Role {
    fn cell(self) -> Cell {
        match self {
            Role::Solid => Cell::Solid,
            Role::Fluid => Cell::Fluid,
            Role::Inlet => Cell::Inlet,
            Role::Outlet => Cell::Outlet,
        }
    }
}

/// One painted region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionSpec {
    /// Region shape.
    pub shape: ShapeSpec,
    /// What the region's voxels become.
    pub role: Role,
}

/// Serializable fluid properties.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FluidSpec {
    /// Density, kg/m³.
    pub density_kg_m3: f64,
    /// Dynamic viscosity, Pa·s.
    pub viscosity_pa_s: f64,
}

fn default_background() -> Role {
    Role::Solid
}

/// Serializable flow problem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowSpec {
    /// Domain minimum corner, mm.
    pub origin_mm: [f64; 3],
    /// Domain extent, mm (voxels must come out cubic).
    pub size_mm: [f64; 3],
    /// Voxel counts (data, not parameters — the grid is provenance).
    pub divisions: [usize; 3],
    /// What unpainted voxels are (default solid).
    #[serde(default = "default_background")]
    pub background: Role,
    /// Regions, painted in order (later wins).
    pub regions: Vec<RegionSpec>,
    /// Working fluid.
    pub fluid: FluidSpec,
    /// Inlet velocity, m/s.
    #[serde(default)]
    pub inlet_velocity_m_s: [f64; 3],
    /// Outlet gauge pressure, Pa.
    #[serde(default)]
    pub outlet_gauge_pa: f64,
    /// Body force per unit volume, N/m³.
    #[serde(default)]
    pub body_force_n_m3: [f64; 3],
    /// Per-axis periodicity.
    #[serde(default)]
    pub periodic: [bool; 3],
    /// Optional lowered Reynolds envelope (never raisable past the
    /// validated laminar limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re_envelope: Option<f64>,
}

impl FlowSpec {
    /// Paint the grid and validate, fail-closed.
    pub fn resolve(&self) -> Result<FlowModel, SpecError> {
        for r in &self.regions {
            r.shape.check()?;
        }
        let mut model = FlowModel::new(self.origin_mm, self.size_mm, self.divisions);
        model.fluid = Fluid {
            density_kg_m3: self.fluid.density_kg_m3,
            viscosity_pa_s: self.fluid.viscosity_pa_s,
        };
        model.inlet_velocity_m_s = self.inlet_velocity_m_s;
        model.outlet_gauge_pa = self.outlet_gauge_pa;
        model.body_force_n_m3 = self.body_force_n_m3;
        model.periodic = self.periodic;
        if let Some(env) = self.re_envelope {
            model.re_envelope = env;
        }
        let background = self.background.cell();
        let (nx, ny, nz) = (self.divisions[0], self.divisions[1], self.divisions[2]);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let p = model.voxel_center_mm(i, j, k);
                    let mut cell = background;
                    for r in &self.regions {
                        if r.shape.contains(p) {
                            cell = r.role.cell();
                        }
                    }
                    let x = model.index(i, j, k);
                    model.cells[x] = cell;
                }
            }
        }
        model.validate().map_err(SpecError::Model)?;
        Ok(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duct_spec() -> FlowSpec {
        FlowSpec {
            origin_mm: [0.0; 3],
            size_mm: [20.0, 6.0, 6.0],
            divisions: [20, 6, 6],
            background: Role::Solid,
            regions: vec![
                RegionSpec {
                    shape: ShapeSpec::Box {
                        min_mm: [1.0, 1.0, 1.0],
                        size_mm: [18.0, 4.0, 4.0],
                    },
                    role: Role::Fluid,
                },
                RegionSpec {
                    shape: ShapeSpec::Box {
                        min_mm: [0.0, 1.0, 1.0],
                        size_mm: [1.0, 4.0, 4.0],
                    },
                    role: Role::Inlet,
                },
                RegionSpec {
                    shape: ShapeSpec::Box {
                        min_mm: [19.0, 1.0, 1.0],
                        size_mm: [1.0, 4.0, 4.0],
                    },
                    role: Role::Outlet,
                },
            ],
            fluid: FluidSpec {
                density_kg_m3: 1.204,
                viscosity_pa_s: 1.825e-5,
            },
            inlet_velocity_m_s: [0.2, 0.0, 0.0],
            outlet_gauge_pa: 0.0,
            body_force_n_m3: [0.0; 3],
            periodic: [false; 3],
            re_envelope: None,
        }
    }

    #[test]
    fn spec_round_trips_json_and_resolves() {
        let spec = duct_spec();
        let json = serde_json::to_string(&spec).unwrap();
        let back: FlowSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
        let model = back.resolve().unwrap();
        assert_eq!(model.count(Cell::Fluid), 18 * 4 * 4);
        assert_eq!(model.count(Cell::Inlet), 16);
        assert_eq!(model.count(Cell::Outlet), 16);
    }

    #[test]
    fn tube_paints_a_cylinder() {
        let mut spec = duct_spec();
        spec.regions = vec![
            RegionSpec {
                shape: ShapeSpec::Tube {
                    axis: Axis::X,
                    center_mm: [3.0, 3.0],
                    span_mm: [1.0, 19.0],
                    outer_radius_mm: 2.0,
                    inner_radius_mm: 0.0,
                },
                role: Role::Fluid,
            },
            RegionSpec {
                shape: ShapeSpec::Box {
                    min_mm: [0.0, 1.0, 1.0],
                    size_mm: [1.0, 4.0, 4.0],
                },
                role: Role::Inlet,
            },
            RegionSpec {
                shape: ShapeSpec::Box {
                    min_mm: [19.0, 1.0, 1.0],
                    size_mm: [1.0, 4.0, 4.0],
                },
                role: Role::Outlet,
            },
        ];
        let model = spec.resolve().unwrap();
        assert!(model.count(Cell::Fluid) > 0);
        // The cylinder is inscribed in the 4x4 square: fewer fluid cells
        // than the box version.
        assert!(model.count(Cell::Fluid) < 18 * 4 * 4);
    }

    #[test]
    fn bad_shapes_refused() {
        let mut spec = duct_spec();
        spec.regions[0] = RegionSpec {
            shape: ShapeSpec::Box {
                min_mm: [0.0; 3],
                size_mm: [-1.0, 1.0, 1.0],
            },
            role: Role::Fluid,
        };
        assert!(matches!(spec.resolve(), Err(SpecError::BadShape(_))));
    }
}
