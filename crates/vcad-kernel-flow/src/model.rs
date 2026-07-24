//! The flow problem: a voxel grid of solid/fluid/inlet/outlet cells plus
//! fluid properties and drive terms, validated fail-closed.

use serde::{Deserialize, Serialize};

/// What a voxel is. The grid is cell-centered; walls sit half a voxel
/// outside the last fluid cell (half-way bounce-back), so a channel that
/// is `n` cells wide is exactly `n · dx` wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cell {
    /// Impermeable stationary wall.
    Solid,
    /// Interior fluid.
    Fluid,
    /// Velocity inlet: behaves as a wall moving at
    /// [`FlowModel::inlet_velocity_m_s`] (moving-wall bounce-back), which
    /// injects the corresponding mass flux through its exposed faces.
    Inlet,
    /// Pressure outlet held at [`FlowModel::outlet_gauge_pa`]
    /// (anti-bounce-back).
    Outlet,
}

/// Newtonian fluid properties.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Fluid {
    /// Density, kg/m³.
    pub density_kg_m3: f64,
    /// Dynamic viscosity, Pa·s.
    pub viscosity_pa_s: f64,
}

impl Fluid {
    /// Air at 20 °C, 1 atm.
    pub const AIR_20C: Fluid = Fluid {
        density_kg_m3: 1.204,
        viscosity_pa_s: 1.825e-5,
    };
    /// Water at 20 °C.
    pub const WATER_20C: Fluid = Fluid {
        density_kg_m3: 998.2,
        viscosity_pa_s: 1.002e-3,
    };

    /// Kinematic viscosity ν = μ/ρ, m²/s.
    pub fn kinematic_viscosity_m2_s(&self) -> f64 {
        self.viscosity_pa_s / self.density_kg_m3
    }
}

/// Boussinesq buoyancy coupling (M3): the fluid acceleration gains
/// `g·β·(T − T_ref)` per cell. Valid for small density variations
/// (β·ΔT ≪ 1) — the incompressible-LBM regime.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Boussinesq {
    /// Volumetric thermal expansion coefficient β, 1/K.
    pub beta_per_k: f64,
    /// Reference temperature (zero-buoyancy datum), °C.
    pub t_ref_c: f64,
    /// Gravity vector, m/s² (Z-up house convention: `[0, 0, -9.81]`).
    pub gravity_m_s2: [f64; 3],
}

/// Thermal transport in the fluid (M1): a passive temperature scalar
/// advected by the lattice velocity and diffused at the fluid's thermal
/// diffusivity, co-evolved with the LBM steps (explicit upwind
/// advection + central diffusion). With [`Boussinesq`] set, the scalar
/// feeds back into the momentum equation (M3) and is no longer passive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThermalTransport {
    /// Temperature of fluid entering at inlets, °C.
    pub inlet_temp_c: f64,
    /// Initial fluid temperature, °C.
    pub initial_temp_c: f64,
    /// Wall temperature, °C. `Some(T)` = isothermal walls (half-cell
    /// Dirichlet flux); `None` = adiabatic walls.
    pub wall_temp_c: Option<f64>,
    /// Thermal diffusivity α = k/(ρ·c_p), m²/s.
    pub diffusivity_m2_s: f64,
    /// Specific heat c_p, J/(kg·K) — prices heat pickup in watts.
    pub heat_capacity_j_kg_k: f64,
    /// Buoyancy coupling (M3). `None` = forced convection only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buoyancy: Option<Boussinesq>,
}

impl ThermalTransport {
    /// Air at 20 °C: α = 2.12e-5 m²/s, c_p = 1006 J/(kg·K).
    pub const AIR_20C: ThermalTransport = ThermalTransport {
        inlet_temp_c: 20.0,
        initial_temp_c: 20.0,
        wall_temp_c: None,
        diffusivity_m2_s: 2.12e-5,
        heat_capacity_j_kg_k: 1006.0,
        buoyancy: None,
    };
}

/// The flow problem on a uniform cubic voxel grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowModel {
    /// Grid origin (min corner), mm.
    pub origin_mm: [f64; 3],
    /// Domain size, mm. Voxels must come out cubic:
    /// `size_mm[a] / divisions[a]` must agree across axes.
    pub size_mm: [f64; 3],
    /// Voxel counts per axis.
    pub divisions: [usize; 3],
    /// One [`Cell`] per voxel, layout `(k·ny + j)·nx + i`.
    pub cells: Vec<Cell>,
    /// The working fluid.
    pub fluid: Fluid,
    /// Inlet velocity vector, m/s (applied at every [`Cell::Inlet`]).
    pub inlet_velocity_m_s: [f64; 3],
    /// Outlet gauge pressure, Pa (reference 0 = outlet static pressure).
    pub outlet_gauge_pa: f64,
    /// Body force per unit volume, N/m³ (e.g. a pressure-gradient drive
    /// for periodic validation cases). Zero for normal inlet/outlet runs.
    pub body_force_n_m3: [f64; 3],
    /// Per-axis periodicity. A non-periodic domain face is a stationary
    /// wall half a voxel outside the last cell layer.
    pub periodic: [bool; 3],
    /// Thermal transport in the fluid (M1/M3). `None` = isothermal M0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal: Option<ThermalTransport>,
    /// Per-voxel solid surface temperature, °C — overrides
    /// [`ThermalTransport::wall_temp_c`] where finite. Non-finite (NaN)
    /// entries fall back to the global wall BC. This is both the M3
    /// differential-heating mechanism (paint hot/cold wall voxels) and
    /// the M2 conjugate seam (the thermal solver hands back its wall
    /// temperature field here). Length must be `nx·ny·nz` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid_temp_c: Option<Vec<f64>>,
    /// Laminar envelope: the highest Reynolds number this model may be
    /// solved at. Defaults to [`FlowModel::RE_LAMINAR_ENVELOPE`]; may be
    /// lowered, never raised past it (validation refuses).
    pub re_envelope: f64,
}

/// Fail-closed validation errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModelError {
    /// A division count is zero.
    EmptyGrid,
    /// `cells.len()` does not match `divisions`.
    CellCountMismatch {
        /// Expected `nx·ny·nz`.
        expected: usize,
        /// Actual length supplied.
        actual: usize,
    },
    /// Voxels are not cubic within 1 part in 10⁶.
    NonCubicVoxels {
        /// Edge lengths per axis, mm.
        voxel_mm: [f64; 3],
    },
    /// A size or division makes no sense (non-finite, non-positive).
    BadDomain,
    /// Fluid properties are non-finite or non-positive.
    BadFluid,
    /// No fluid cells at all.
    NoFluid,
    /// Inlets exist but the inlet velocity is zero (or vice versa) and
    /// there is no body force: nothing drives the flow.
    NoDrive,
    /// The inlet pushes net mass with no outlet to receive it (or
    /// outlets exist with no inlet): mass cannot balance in steady
    /// state. Tangentially-moving walls (zero net flux) are exempt.
    UnbalancedPorts {
        /// Number of inlet cells.
        inlets: usize,
        /// Number of outlet cells.
        outlets: usize,
    },
    /// The computed Reynolds number exceeds the validated laminar
    /// envelope. Refused rather than answered: M0 has no turbulence
    /// model, and a converged-looking laminar solve at this Re would be
    /// a wrong answer wearing a receipt.
    ReynoldsAboveEnvelope {
        /// Computed Re = ρ·|U|·D_h / μ at the inlet.
        re: f64,
        /// The envelope it exceeds.
        envelope: f64,
        /// Hydraulic diameter used, mm.
        hydraulic_diameter_mm: f64,
    },
    /// Thermal transport parameters are non-finite or non-positive.
    BadThermal,
    /// `solid_temp_c` present with the wrong length, or present without
    /// thermal transport enabled.
    BadSolidTemps,
    /// Buoyancy-driven Rayleigh number exceeds the validated laminar
    /// envelope — same honesty as the Re gate: no turbulent natural
    /// convection model exists here, so none is pretended.
    RayleighAboveEnvelope {
        /// Computed Ra = g·β·ΔT·L³/(ν·α).
        ra: f64,
        /// The envelope it exceeds.
        envelope: f64,
    },
    /// `re_envelope` was raised above the validated laminar limit.
    EnvelopeAboveValidated {
        /// Requested envelope.
        requested: f64,
        /// The validated maximum.
        validated: f64,
    },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelError::EmptyGrid => write!(f, "grid has a zero division count"),
            ModelError::CellCountMismatch { expected, actual } => {
                write!(f, "cells.len() = {actual}, expected nx*ny*nz = {expected}")
            }
            ModelError::NonCubicVoxels { voxel_mm } => write!(
                f,
                "voxels must be cubic; got {:.6} x {:.6} x {:.6} mm — adjust size_mm or divisions",
                voxel_mm[0], voxel_mm[1], voxel_mm[2]
            ),
            ModelError::BadDomain => write!(f, "domain size/divisions non-finite or non-positive"),
            ModelError::BadFluid => write!(f, "fluid density/viscosity non-finite or non-positive"),
            ModelError::BadThermal => write!(
                f,
                "thermal transport diffusivity/heat-capacity/buoyancy non-finite or non-positive"
            ),
            ModelError::BadSolidTemps => write!(
                f,
                "solid_temp_c must have one entry per voxel and requires thermal transport"
            ),
            ModelError::RayleighAboveEnvelope { ra, envelope } => write!(
                f,
                "Ra = {ra:.2e} exceeds the validated laminar natural-convection envelope \
                 {envelope:.0e}; M3 refuses turbulent buoyant flow rather than guessing — \
                 reduce the temperature difference or the cavity size"
            ),
            ModelError::NoFluid => write!(f, "no fluid cells in the grid"),
            ModelError::NoDrive => write!(
                f,
                "nothing drives the flow: zero inlet velocity and zero body force"
            ),
            ModelError::UnbalancedPorts { inlets, outlets } => write!(
                f,
                "steady flow needs both ports: {inlets} inlet cells, {outlets} outlet cells"
            ),
            ModelError::ReynoldsAboveEnvelope {
                re,
                envelope,
                hydraulic_diameter_mm,
            } => write!(
                f,
                "Re = {re:.0} (D_h = {hydraulic_diameter_mm:.2} mm) exceeds the validated \
                 laminar envelope of {envelope:.0}; M0 refuses rather than running an \
                 unvalidated turbulent regime — reduce velocity, shrink the duct, or wait \
                 for a milestone that validates this envelope"
            ),
            ModelError::EnvelopeAboveValidated {
                requested,
                validated,
            } => write!(
                f,
                "re_envelope {requested:.0} exceeds the validated laminar limit {validated:.0} \
                 and cannot be raised past it"
            ),
        }
    }
}

impl std::error::Error for ModelError {}

impl FlowModel {
    /// The validated laminar envelope for internal flow (pipe-flow
    /// transition). `re_envelope` may be set lower but never higher.
    pub const RE_LAMINAR_ENVELOPE: f64 = 2300.0;

    /// The validated laminar envelope for buoyancy-driven flow (cavity
    /// natural convection stays laminar to Ra ~ 10⁸).
    pub const RA_LAMINAR_ENVELOPE: f64 = 1.0e8;

    /// A model with all-solid cells to be painted, air, and no drive.
    pub fn new(origin_mm: [f64; 3], size_mm: [f64; 3], divisions: [usize; 3]) -> Self {
        let n = divisions[0] * divisions[1] * divisions[2];
        FlowModel {
            origin_mm,
            size_mm,
            divisions,
            cells: vec![Cell::Solid; n],
            fluid: Fluid::AIR_20C,
            inlet_velocity_m_s: [0.0; 3],
            outlet_gauge_pa: 0.0,
            body_force_n_m3: [0.0; 3],
            periodic: [false; 3],
            thermal: None,
            solid_temp_c: None,
            re_envelope: Self::RE_LAMINAR_ENVELOPE,
        }
    }

    /// Voxel edge length, mm (validated cubic).
    pub fn voxel_mm(&self) -> f64 {
        self.size_mm[0] / self.divisions[0] as f64
    }

    /// Linear index of voxel `(i, j, k)`.
    pub fn index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.divisions[1] + j) * self.divisions[0] + i
    }

    /// Center of voxel `(i, j, k)`, mm.
    pub fn voxel_center_mm(&self, i: usize, j: usize, k: usize) -> [f64; 3] {
        let d = self.voxel_mm();
        [
            self.origin_mm[0] + (i as f64 + 0.5) * d,
            self.origin_mm[1] + (j as f64 + 0.5) * d,
            self.origin_mm[2] + (k as f64 + 0.5) * d,
        ]
    }

    /// Count of cells of a given kind.
    pub fn count(&self, kind: Cell) -> usize {
        self.cells.iter().filter(|c| **c == kind).count()
    }

    /// Hydraulic diameter of the inlet patch, mm: `D_h = 4A/P` where `A`
    /// is the exposed inlet face area and `P` the patch perimeter,
    /// measured on the inlet faces adjacent to fluid.
    ///
    /// Returns `None` when there are no inlet faces adjacent to fluid.
    pub fn inlet_hydraulic_diameter_mm(&self) -> Option<f64> {
        let (nx, ny, nz) = (self.divisions[0], self.divisions[1], self.divisions[2]);
        let dx = self.voxel_mm();
        // Exposed inlet faces: (inlet cell, axis face) pairs whose
        // neighbor is fluid. Area = count · dx². Perimeter: each exposed
        // face contributes edges shared with a face that is NOT exposed
        // in the same orientation.
        let mut faces: Vec<(usize, usize, usize, usize, isize)> = Vec::new();
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    if self.cells[self.index(i, j, k)] != Cell::Inlet {
                        continue;
                    }
                    let neighbors: [(isize, isize, isize, usize, isize); 6] = [
                        (-1, 0, 0, 0, -1),
                        (1, 0, 0, 0, 1),
                        (0, -1, 0, 1, -1),
                        (0, 1, 0, 1, 1),
                        (0, 0, -1, 2, -1),
                        (0, 0, 1, 2, 1),
                    ];
                    for (di, dj, dk, axis, side) in neighbors {
                        let (ni, nj, nk) = (i as isize + di, j as isize + dj, k as isize + dk);
                        if ni < 0
                            || nj < 0
                            || nk < 0
                            || ni >= nx as isize
                            || nj >= ny as isize
                            || nk >= nz as isize
                        {
                            continue;
                        }
                        if self.cells[self.index(ni as usize, nj as usize, nk as usize)]
                            == Cell::Fluid
                        {
                            faces.push((i, j, k, axis, side));
                        }
                    }
                }
            }
        }
        if faces.is_empty() {
            return None;
        }
        let area = faces.len() as f64 * dx * dx;
        // Perimeter: for each exposed face, check its 4 in-plane
        // neighbors; an edge not shared with another exposed face of the
        // same orientation is boundary.
        let face_set: std::collections::HashSet<(usize, usize, usize, usize, isize)> =
            faces.iter().copied().collect();
        let mut boundary_edges = 0usize;
        for &(i, j, k, axis, side) in &faces {
            let tangents: [usize; 2] = match axis {
                0 => [1, 2],
                1 => [0, 2],
                _ => [0, 1],
            };
            for t in tangents {
                for step in [-1isize, 1] {
                    let mut c = [i as isize, j as isize, k as isize];
                    c[t] += step;
                    let inb = c[0] >= 0
                        && c[1] >= 0
                        && c[2] >= 0
                        && c[0] < nx as isize
                        && c[1] < ny as isize
                        && c[2] < nz as isize;
                    let shared = inb
                        && face_set.contains(&(
                            c[0] as usize,
                            c[1] as usize,
                            c[2] as usize,
                            axis,
                            side,
                        ));
                    if !shared {
                        boundary_edges += 1;
                    }
                }
            }
        }
        let perimeter = boundary_edges as f64 * dx;
        Some(4.0 * area / perimeter)
    }

    /// Net volumetric flux the inlet velocity pushes through the exposed
    /// inlet faces, m³/s (`Σ u·n̂ · A` over inlet faces adjacent to
    /// fluid). Zero for tangentially-moving walls (lid-driven cases).
    pub fn inlet_net_flux_m3_s(&self) -> f64 {
        let (nx, ny, nz) = (
            self.divisions[0] as isize,
            self.divisions[1] as isize,
            self.divisions[2] as isize,
        );
        let dx = self.voxel_mm() / 1000.0;
        let idx = |i: isize, j: isize, k: isize| -> usize {
            ((k * self.divisions[1] as isize + j) * self.divisions[0] as isize + i) as usize
        };
        let dirs: [(isize, isize, isize); 6] = [
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ];
        let mut flux = 0.0;
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    if self.cells[idx(i, j, k)] != Cell::Inlet {
                        continue;
                    }
                    for (di, dj, dk) in dirs {
                        let (fi, fj, fk) = (i + di, j + dj, k + dk);
                        if fi < 0 || fj < 0 || fk < 0 || fi >= nx || fj >= ny || fk >= nz {
                            continue;
                        }
                        if self.cells[idx(fi, fj, fk)] == Cell::Fluid {
                            let un = self.inlet_velocity_m_s[0] * di as f64
                                + self.inlet_velocity_m_s[1] * dj as f64
                                + self.inlet_velocity_m_s[2] * dk as f64;
                            flux += un * dx * dx;
                        }
                    }
                }
            }
        }
        flux
    }

    /// Reynolds number at the inlet, `Re = ρ·|U|·D_h / μ`, when an inlet
    /// exists; falls back to the largest open cross-section dimension for
    /// pure body-force drives (periodic validation cases).
    pub fn reynolds(&self) -> Option<f64> {
        let speed = norm(self.inlet_velocity_m_s);
        if speed > 0.0 {
            let dh_mm = self.inlet_hydraulic_diameter_mm()?;
            Some(self.fluid.density_kg_m3 * speed * (dh_mm / 1000.0) / self.fluid.viscosity_pa_s)
        } else {
            None
        }
    }

    /// Validate the model, fail-closed. Every solve calls this first.
    pub fn validate(&self) -> Result<(), ModelError> {
        let (nx, ny, nz) = (self.divisions[0], self.divisions[1], self.divisions[2]);
        if nx == 0 || ny == 0 || nz == 0 {
            return Err(ModelError::EmptyGrid);
        }
        let expected = nx * ny * nz;
        if self.cells.len() != expected {
            return Err(ModelError::CellCountMismatch {
                expected,
                actual: self.cells.len(),
            });
        }
        if !self
            .size_mm
            .iter()
            .chain(self.origin_mm.iter())
            .all(|v| v.is_finite())
            || self.size_mm.iter().any(|s| *s <= 0.0)
        {
            return Err(ModelError::BadDomain);
        }
        let voxel = [
            self.size_mm[0] / nx as f64,
            self.size_mm[1] / ny as f64,
            self.size_mm[2] / nz as f64,
        ];
        let mean = (voxel[0] + voxel[1] + voxel[2]) / 3.0;
        if voxel.iter().any(|v| (v - mean).abs() > 1e-6 * mean) {
            return Err(ModelError::NonCubicVoxels { voxel_mm: voxel });
        }
        if !(self.fluid.density_kg_m3.is_finite()
            && self.fluid.viscosity_pa_s.is_finite()
            && self.fluid.density_kg_m3 > 0.0
            && self.fluid.viscosity_pa_s > 0.0)
        {
            return Err(ModelError::BadFluid);
        }
        if self.count(Cell::Fluid) == 0 {
            return Err(ModelError::NoFluid);
        }
        if let Some(t) = &self.thermal {
            let finite = t.inlet_temp_c.is_finite()
                && t.initial_temp_c.is_finite()
                && t.diffusivity_m2_s.is_finite()
                && t.heat_capacity_j_kg_k.is_finite()
                && t.wall_temp_c.map(|w| w.is_finite()).unwrap_or(true);
            let positive = t.diffusivity_m2_s > 0.0 && t.heat_capacity_j_kg_k > 0.0;
            let buoy_ok = t.buoyancy.is_none_or(|b| {
                b.beta_per_k.is_finite()
                    && b.beta_per_k >= 0.0
                    && b.t_ref_c.is_finite()
                    && b.gravity_m_s2.iter().all(|g| g.is_finite())
            });
            if !(finite && positive && buoy_ok) {
                return Err(ModelError::BadThermal);
            }
        }
        let inlets = self.count(Cell::Inlet);
        let outlets = self.count(Cell::Outlet);
        let inlet_speed = norm(self.inlet_velocity_m_s);
        let force = norm(self.body_force_n_m3);
        // Ports must balance only when the inlet actually pushes net
        // mass: a tangentially-moving wall (lid-driven cavity) is a
        // valid closed domain.
        let net_flux = if inlets > 0 && inlet_speed > 0.0 {
            self.inlet_net_flux_m3_s()
        } else {
            0.0
        };
        if (net_flux.abs() > 1e-15 && outlets == 0) || (outlets > 0 && inlets == 0) {
            return Err(ModelError::UnbalancedPorts { inlets, outlets });
        }
        let driven_by_buoyancy = self
            .thermal
            .as_ref()
            .and_then(|t| t.buoyancy)
            .map(|b| b.beta_per_k > 0.0 && norm(b.gravity_m_s2) > 0.0)
            .unwrap_or(false);
        let driven_by_inlet = inlets > 0 && inlet_speed > 0.0;
        if !driven_by_inlet && force == 0.0 && !driven_by_buoyancy {
            return Err(ModelError::NoDrive);
        }
        if let Some(st) = &self.solid_temp_c {
            if st.len() != expected || self.thermal.is_none() {
                return Err(ModelError::BadSolidTemps);
            }
        }
        // Rayleigh gate for buoyancy-driven flow: Ra = g·β·ΔT·L³/(ν·α)
        // with L the largest domain dimension and ΔT the spread of the
        // thermal boundary temperatures.
        if driven_by_buoyancy {
            let t = self.thermal.as_ref().expect("buoyancy implies thermal");
            let b = t.buoyancy.expect("checked above");
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            let mut push = |v: f64| {
                if v.is_finite() {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            };
            push(t.inlet_temp_c);
            push(t.initial_temp_c);
            if let Some(w) = t.wall_temp_c {
                push(w);
            }
            if let Some(st) = &self.solid_temp_c {
                for v in st {
                    push(*v);
                }
            }
            let dt = (hi - lo).max(0.0);
            let l = self.size_mm.iter().cloned().fold(0.0, f64::max) / 1000.0;
            let nu = self.fluid.kinematic_viscosity_m2_s();
            let ra =
                norm(b.gravity_m_s2) * b.beta_per_k * dt * l * l * l / (nu * t.diffusivity_m2_s);
            if ra > Self::RA_LAMINAR_ENVELOPE {
                return Err(ModelError::RayleighAboveEnvelope {
                    ra,
                    envelope: Self::RA_LAMINAR_ENVELOPE,
                });
            }
        }
        if self.re_envelope > Self::RE_LAMINAR_ENVELOPE {
            return Err(ModelError::EnvelopeAboveValidated {
                requested: self.re_envelope,
                validated: Self::RE_LAMINAR_ENVELOPE,
            });
        }
        if driven_by_inlet {
            if let (Some(re), Some(dh)) = (self.reynolds(), self.inlet_hydraulic_diameter_mm()) {
                if re > self.re_envelope {
                    return Err(ModelError::ReynoldsAboveEnvelope {
                        re,
                        envelope: self.re_envelope,
                        hydraulic_diameter_mm: dh,
                    });
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duct(nx: usize, ny: usize, nz: usize) -> FlowModel {
        // Straight duct along x: inlet slab at i=0, outlet slab at
        // i=nx-1, fluid between, solid handled by domain walls.
        let mut m = FlowModel::new([0.0; 3], [nx as f64, ny as f64, nz as f64], [nx, ny, nz]);
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let idx = m.index(i, j, k);
                    m.cells[idx] = if i == 0 {
                        Cell::Inlet
                    } else if i == nx - 1 {
                        Cell::Outlet
                    } else {
                        Cell::Fluid
                    };
                }
            }
        }
        m.inlet_velocity_m_s = [0.1, 0.0, 0.0];
        m
    }

    #[test]
    fn valid_duct_passes() {
        assert_eq!(duct(10, 5, 5).validate(), Ok(()));
    }

    #[test]
    fn non_cubic_refused() {
        let mut m = duct(10, 5, 5);
        m.size_mm = [10.0, 5.0, 7.0];
        assert!(matches!(
            m.validate(),
            Err(ModelError::NonCubicVoxels { .. })
        ));
    }

    #[test]
    fn inlet_without_outlet_refused() {
        let mut m = duct(10, 5, 5);
        for c in m.cells.iter_mut() {
            if *c == Cell::Outlet {
                *c = Cell::Fluid;
            }
        }
        assert!(matches!(
            m.validate(),
            Err(ModelError::UnbalancedPorts { .. })
        ));
    }

    #[test]
    fn no_drive_refused() {
        let mut m = duct(10, 5, 5);
        m.inlet_velocity_m_s = [0.0; 3];
        assert!(matches!(m.validate(), Err(ModelError::NoDrive)));
    }

    #[test]
    fn reynolds_gate_refuses_fast_flow() {
        let mut m = duct(10, 5, 5);
        // 5x5 voxel inlet at 1 mm/voxel -> D_h = 5 mm; air at 10 m/s ->
        // Re ~ 3300 > 2300.
        m.inlet_velocity_m_s = [10.0, 0.0, 0.0];
        match m.validate() {
            Err(ModelError::ReynoldsAboveEnvelope { re, envelope, .. }) => {
                assert!(re > envelope);
            }
            other => panic!("expected Reynolds refusal, got {other:?}"),
        }
    }

    #[test]
    fn envelope_cannot_be_raised() {
        let mut m = duct(10, 5, 5);
        m.re_envelope = 10_000.0;
        assert!(matches!(
            m.validate(),
            Err(ModelError::EnvelopeAboveValidated { .. })
        ));
    }

    #[test]
    fn hydraulic_diameter_square_patch() {
        // A square 5x5 patch: D_h = 4A/P = 4·25/20 = 5 (in voxel units).
        let m = duct(10, 5, 5);
        let dh = m.inlet_hydraulic_diameter_mm().unwrap();
        assert!((dh - 5.0).abs() < 1e-9, "dh = {dh}");
    }
}
