//! The FDTD simulation: Yee grid, leapfrog stepper, CPML, walls.
//!
//! # Yee staggering (square cells, pitch Δ, domain `nx × ny` cells)
//!
//! TM (Ez, Hx, Hy):
//!
//! ```text
//! Ez[i][j]  at (i·Δ,       j·Δ)        — (nx+1) × (ny+1) nodes
//! Hx[i][j]  at (i·Δ,       (j+½)·Δ)   — (nx+1) × ny
//! Hy[i][j]  at ((i+½)·Δ,   j·Δ)       — nx × (ny+1)
//! ```
//!
//! TE (Hz, Ex, Ey):
//!
//! ```text
//! Hz[i][j]  at ((i+½)·Δ,  (j+½)·Δ)    — nx × ny
//! Ex[i][j]  at ((i+½)·Δ,   j·Δ)       — nx × (ny+1)
//! Ey[i][j]  at (i·Δ,       (j+½)·Δ)   — (nx+1) × ny
//! ```
//!
//! Leapfrog: H lives at half-integer time steps, E at integer steps. One
//! [`Simulation::step`] advances H to (n+½)·dt then E to (n+1)·dt, with
//! soft sources injected at each field's native time.
//!
//! # Walls
//!
//! Each domain side is [`Wall::Pec`] (tangential E pinned to zero — the
//! default) or [`Wall::Pmc`] (tangential H zero, implemented by mirror
//! stencils). A PMC wall is a symmetry plane: a y-uniform TM wave between
//! PMC y-walls is **exactly** one-dimensional, which the validation tests
//! exploit to measure dispersion and reflection with no transverse
//! contamination (PEC plays the same role for TE by duality). CPML slabs
//! ([`crate::cpml`]) sit inside whichever sides have nonzero thickness.
//!
//! A `Simulation` is single-shot: configure (walls, CPML, Courant,
//! materials, sources, monitors), then step. The first step freezes the
//! configuration; reconfiguring afterwards panics. Parameter studies
//! rebuild the simulation — cheap, and it guarantees frozen-discretization
//! comparisons across runs (the finite-difference-vs-adjoint lesson from
//! `vcad-kernel-particle` encoded as API shape).

use crate::cpml::{AxisCoeffs, CpmlSpec, PsiFields};
use crate::grid::{Field2, GridSpec};
use crate::material::{paint_component, Shape2};
use crate::monitor::{Cplx, FluxSpec, FluxState};
use crate::source::{Source, SourcePlacement};

/// Which 2D Maxwell polarization is simulated (see crate docs for the
/// naming convention and the slab-literature cross-map).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarization {
    /// E out of plane: (Ez, Hx, Hy).
    Tm,
    /// H out of plane: (Hz, Ex, Ey).
    Te,
}

/// Outer-wall boundary condition for one side of the domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wall {
    /// Perfect electric conductor: tangential E = 0 (Yee default).
    Pec,
    /// Perfect magnetic conductor: tangential H = 0 (mirror stencils).
    Pmc,
}

/// Wall types for the four domain sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundarySpec {
    /// x = 0 side.
    pub x_lo: Wall,
    /// x = nx·Δ side.
    pub x_hi: Wall,
    /// y = 0 side.
    pub y_lo: Wall,
    /// y = ny·Δ side.
    pub y_hi: Wall,
}

impl Default for BoundarySpec {
    fn default() -> Self {
        Self {
            x_lo: Wall::Pec,
            x_hi: Wall::Pec,
            y_lo: Wall::Pec,
            y_hi: Wall::Pmc,
        }
    }
}

impl BoundarySpec {
    /// All four walls PEC.
    pub fn pec() -> Self {
        Self {
            x_lo: Wall::Pec,
            x_hi: Wall::Pec,
            y_lo: Wall::Pec,
            y_hi: Wall::Pec,
        }
    }

    /// PEC x-walls, PMC y-walls (the exact-1D TM configuration).
    pub fn pmc_y() -> Self {
        Self {
            x_lo: Wall::Pec,
            x_hi: Wall::Pec,
            y_lo: Wall::Pmc,
            y_hi: Wall::Pmc,
        }
    }
}

/// Handle to a flux monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluxId(usize);

/// Handle to a time probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeId(usize);

#[derive(Debug, Clone)]
struct ProbeState {
    i: usize,
    j: usize,
    series: Vec<f64>,
}

/// A single-shot 2D FDTD run. See the module docs for staggering, walls,
/// and the configure-then-step lifecycle.
#[derive(Debug, Clone)]
pub struct Simulation {
    spec: GridSpec,
    pol: Polarization,
    courant: f64,
    dt: f64,
    boundaries: BoundarySpec,
    cpml: CpmlSpec,
    // TM state (empty in TE mode).
    ez: Field2,
    hx: Field2,
    hy: Field2,
    eps_z: Field2,
    inv_eps_z: Field2,
    // TE state (empty in TM mode).
    hz: Field2,
    ex: Field2,
    ey: Field2,
    eps_x: Field2,
    eps_y: Field2,
    inv_eps_x: Field2,
    inv_eps_y: Field2,
    // CPML coefficients per axis and staggering, plus ψ memory.
    cx_n: AxisCoeffs,
    cx_h: AxisCoeffs,
    cy_n: AxisCoeffs,
    cy_h: AxisCoeffs,
    psi: PsiFields,
    sources: Vec<Source>,
    fluxes: Vec<FluxState>,
    probes: Vec<ProbeState>,
    step_index: usize,
    committed: bool,
}

impl Simulation {
    /// New vacuum simulation (ε = 1 everywhere, PEC walls, no CPML,
    /// Courant factor 0.5 → dt = 0.5·Δ/√2).
    pub fn new(spec: GridSpec, pol: Polarization) -> Self {
        let (nx, ny) = (spec.nx, spec.ny);
        let empty = Field2::new(0, 0);
        let (ez, hx, hy, eps_z) = match pol {
            Polarization::Tm => (
                Field2::new(nx + 1, ny + 1),
                Field2::new(nx + 1, ny),
                Field2::new(nx, ny + 1),
                Field2::filled(nx + 1, ny + 1, 1.0),
            ),
            Polarization::Te => (empty.clone(), empty.clone(), empty.clone(), empty.clone()),
        };
        let (hz, ex, ey, eps_x, eps_y) = match pol {
            Polarization::Te => (
                Field2::new(nx, ny),
                Field2::new(nx, ny + 1),
                Field2::new(nx + 1, ny),
                Field2::filled(nx, ny + 1, 1.0),
                Field2::filled(nx + 1, ny, 1.0),
            ),
            Polarization::Tm => (
                empty.clone(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                empty.clone(),
            ),
        };
        let psi = match pol {
            Polarization::Tm => PsiFields {
                psi_a: Field2::new(nx + 1, ny + 1),
                psi_b: Field2::new(nx + 1, ny + 1),
                psi_c: Field2::new(nx + 1, ny),
                psi_d: Field2::new(nx, ny + 1),
            },
            Polarization::Te => PsiFields {
                psi_a: Field2::new(nx, ny + 1),
                psi_b: Field2::new(nx + 1, ny),
                psi_c: Field2::new(nx, ny),
                psi_d: Field2::new(nx, ny),
            },
        };
        let courant = 0.5;
        let dt = courant * spec.delta / 2f64.sqrt();
        Self {
            spec,
            pol,
            courant,
            dt,
            boundaries: BoundarySpec::pec(),
            cpml: CpmlSpec::none(),
            ez,
            hx,
            hy,
            eps_z,
            inv_eps_z: Field2::new(0, 0),
            hz,
            ex,
            ey,
            eps_x,
            eps_y,
            inv_eps_x: Field2::new(0, 0),
            inv_eps_y: Field2::new(0, 0),
            cx_n: AxisCoeffs::identity(0),
            cx_h: AxisCoeffs::identity(0),
            cy_n: AxisCoeffs::identity(0),
            cy_h: AxisCoeffs::identity(0),
            psi,
            sources: Vec::new(),
            fluxes: Vec::new(),
            probes: Vec::new(),
            step_index: 0,
            committed: false,
        }
    }

    fn assert_configurable(&self) {
        assert!(
            !self.committed,
            "simulation already stepped; configuration is frozen (build a new one)"
        );
    }

    /// Set the CPML configuration (before the first step).
    pub fn set_cpml(&mut self, cpml: CpmlSpec) {
        self.assert_configurable();
        let s = &self.spec;
        assert!(
            cpml.x_lo + cpml.x_hi < s.nx && cpml.y_lo + cpml.y_hi < s.ny,
            "CPML thicker than the domain"
        );
        self.cpml = cpml;
    }

    /// Set wall types (before the first step).
    pub fn set_boundaries(&mut self, b: BoundarySpec) {
        self.assert_configurable();
        self.boundaries = b;
    }

    /// Set the Courant factor S ∈ (0, 1]; dt = S·Δ/√2 (before the first
    /// step). The 2D vacuum stability bound is S ≤ 1; ε ≥ 1 only relaxes
    /// it.
    pub fn set_courant(&mut self, s: f64) {
        self.assert_configurable();
        assert!(s > 0.0 && s <= 1.0, "Courant factor must be in (0, 1]");
        self.courant = s;
        self.dt = s * self.spec.delta / 2f64.sqrt();
    }

    /// Fill the whole domain with relative permittivity `eps`.
    pub fn fill_epsilon(&mut self, eps: f64) {
        self.assert_configurable();
        assert!(
            eps >= 1.0,
            "relative permittivity must be ≥ 1 (lossless M0)"
        );
        match self.pol {
            Polarization::Tm => self.eps_z.fill(eps),
            Polarization::Te => {
                self.eps_x.fill(eps);
                self.eps_y.fill(eps);
            }
        }
    }

    /// Paint `shape` at relative permittivity `eps` over the background
    /// (area-weighted sub-pixel averaging; later paints win).
    pub fn paint(&mut self, shape: &Shape2, eps: f64) {
        self.assert_configurable();
        assert!(
            eps >= 1.0,
            "relative permittivity must be ≥ 1 (lossless M0)"
        );
        let d = self.spec.delta;
        match self.pol {
            Polarization::Tm => paint_component(&mut self.eps_z, 0.0, 0.0, d, shape, eps),
            Polarization::Te => {
                paint_component(&mut self.eps_x, 0.5 * d, 0.0, d, shape, eps);
                paint_component(&mut self.eps_y, 0.0, 0.5 * d, d, shape, eps);
            }
        }
    }

    /// Add a soft source (see [`crate::source`] for index semantics).
    pub fn add_source(&mut self, source: Source) {
        self.assert_configurable();
        let (max_i, max_j) = match self.pol {
            Polarization::Tm => (self.spec.nx, self.spec.ny), // Ez nodes
            Polarization::Te => (self.spec.nx - 1, self.spec.ny - 1), // Hz samples
        };
        match &source.placement {
            SourcePlacement::Point { i, j } => {
                assert!(*i <= max_i && *j <= max_j, "source out of range");
            }
            SourcePlacement::VerticalLine { i, j0, j1, profile } => {
                assert!(
                    *i <= max_i && *j1 <= max_j && j0 <= j1,
                    "source out of range"
                );
                assert_eq!(profile.len(), j1 - j0 + 1, "profile length mismatch");
            }
            SourcePlacement::TfsfVerticalLine {
                i0,
                j0,
                j1,
                profile,
                ..
            } => {
                assert_eq!(
                    self.pol,
                    Polarization::Tm,
                    "TF/SF mode injection is TM-only at M1"
                );
                assert!(
                    *i0 >= 1 && *i0 + 1 < self.spec.nx,
                    "TF/SF plane out of range"
                );
                // The correction terms skip the CPML κ/ψ machinery, which
                // is only exact where the coefficients are identity.
                assert!(
                    *i0 > self.cpml.x_lo + 1 && *i0 + 1 < self.spec.nx - self.cpml.x_hi,
                    "TF/SF plane must sit outside the CPML slabs (set CPML first)"
                );
                assert!(*j1 <= max_j && j0 <= j1, "source out of range");
                assert_eq!(profile.len(), j1 - j0 + 1, "profile length mismatch");
            }
        }
        self.sources.push(source);
    }

    /// Add a spectral flux monitor; returns its handle.
    pub fn add_flux(&mut self, spec: FluxSpec) -> FluxId {
        self.assert_configurable();
        let (nx, ny) = (self.spec.nx, self.spec.ny);
        match &spec {
            FluxSpec::Vertical { i, j0, j1, freqs } => {
                assert!(*i >= 1 && *i < nx, "vertical flux line needs 1 ≤ i ≤ nx−1");
                assert!(j0 <= j1 && !freqs.is_empty());
                let j_max = match self.pol {
                    Polarization::Tm => ny,     // Ez nodes
                    Polarization::Te => ny - 1, // Ey/Hz rows
                };
                assert!(*j1 <= j_max, "flux span out of range");
            }
            FluxSpec::Horizontal { j, i0, i1, freqs } => {
                assert!(
                    *j >= 1 && *j < ny,
                    "horizontal flux line needs 1 ≤ j ≤ ny−1"
                );
                assert!(i0 <= i1 && !freqs.is_empty());
                let i_max = match self.pol {
                    Polarization::Tm => nx,
                    Polarization::Te => nx - 1,
                };
                assert!(*i1 <= i_max, "flux span out of range");
            }
        }
        self.fluxes.push(FluxState::new(spec));
        FluxId(self.fluxes.len() - 1)
    }

    /// Add a time probe recording the out-of-plane field (Ez in TM at
    /// node `(i, j)`, Hz in TE at sample `(i+½, j+½)`) every step.
    pub fn add_probe(&mut self, i: usize, j: usize) -> ProbeId {
        self.assert_configurable();
        let (max_i, max_j) = match self.pol {
            Polarization::Tm => (self.spec.nx, self.spec.ny),
            Polarization::Te => (self.spec.nx - 1, self.spec.ny - 1),
        };
        assert!(i <= max_i && j <= max_j, "probe out of range");
        self.probes.push(ProbeState {
            i,
            j,
            series: Vec::new(),
        });
        ProbeId(self.probes.len() - 1)
    }

    /// Freeze configuration: build inverse-ε and CPML coefficient tables.
    fn commit(&mut self) {
        if self.committed {
            return;
        }
        let (nx, ny) = (self.spec.nx, self.spec.ny);
        let d = self.spec.delta;
        let dt = self.dt;
        let c = &self.cpml;
        self.cx_n = AxisCoeffs::build(nx + 1, nx, 0.0, c.x_lo, c.x_hi, c, d, dt);
        self.cx_h = AxisCoeffs::build(nx, nx, 0.5, c.x_lo, c.x_hi, c, d, dt);
        self.cy_n = AxisCoeffs::build(ny + 1, ny, 0.0, c.y_lo, c.y_hi, c, d, dt);
        self.cy_h = AxisCoeffs::build(ny, ny, 0.5, c.y_lo, c.y_hi, c, d, dt);
        let inv = |e: &Field2| {
            let mut f = e.clone();
            for v in f.as_mut_slice() {
                *v = 1.0 / *v;
            }
            f
        };
        match self.pol {
            Polarization::Tm => self.inv_eps_z = inv(&self.eps_z),
            Polarization::Te => {
                self.inv_eps_x = inv(&self.eps_x);
                self.inv_eps_y = inv(&self.eps_y);
            }
        }
        self.committed = true;
    }

    /// Advance one time step.
    pub fn step(&mut self) {
        self.commit();
        match self.pol {
            Polarization::Tm => self.step_tm(),
            Polarization::Te => self.step_te(),
        }
        self.record_monitors();
        self.step_index += 1;
    }

    /// Advance `n` steps.
    pub fn run(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn step_tm(&mut self) {
        let (nx, ny) = (self.spec.nx, self.spec.ny);
        let inv_d = 1.0 / self.spec.delta;
        let dt = self.dt;
        // H update: E^n → H^(n+½).
        for i in 0..=nx {
            for j in 0..ny {
                let dezdy = (self.ez.at(i, j + 1) - self.ez.at(i, j)) * inv_d;
                let p = self.psi.psi_c.at_mut(i, j);
                *p = self.cy_h.b[j] * *p + self.cy_h.a[j] * dezdy;
                let p = self.psi.psi_c.at(i, j);
                *self.hx.at_mut(i, j) -= dt * (self.cy_h.inv_kappa[j] * dezdy + p);
            }
        }
        for i in 0..nx {
            for j in 0..=ny {
                let dezdx = (self.ez.at(i + 1, j) - self.ez.at(i, j)) * inv_d;
                let p = self.psi.psi_d.at_mut(i, j);
                *p = self.cx_h.b[i] * *p + self.cx_h.a[i] * dezdx;
                let p = self.psi.psi_d.at(i, j);
                *self.hy.at_mut(i, j) += dt * (self.cx_h.inv_kappa[i] * dezdx + p);
            }
        }
        // TF/SF Hy correction: the Hy update at column i0 (scattered
        // field) consumed Ez[i0+1] which carries the incident mode —
        // subtract ez_inc evaluated at the E time level just used (n·dt).
        let t_e_used = self.step_index as f64 * dt;
        for k in 0..self.sources.len() {
            if let SourcePlacement::TfsfVerticalLine {
                i0,
                j0,
                j1,
                profile,
                ..
            } = &self.sources[k].placement
            {
                let s = self.sources[k].amplitude * self.sources[k].waveform.eval(t_e_used);
                if s == 0.0 {
                    continue;
                }
                for (kk, j) in (*j0..=*j1).enumerate() {
                    *self.hy.at_mut(*i0, j) -= dt * inv_d * profile[kk] * s;
                }
            }
        }
        // E update: H^(n+½) → E^(n+1). PMC walls extend the update to
        // boundary samples with mirror stencils (H_tan odd across a PMC).
        let i_lo = if self.boundaries.x_lo == Wall::Pmc {
            0
        } else {
            1
        };
        let i_hi = if self.boundaries.x_hi == Wall::Pmc {
            nx
        } else {
            nx - 1
        };
        let j_lo = if self.boundaries.y_lo == Wall::Pmc {
            0
        } else {
            1
        };
        let j_hi = if self.boundaries.y_hi == Wall::Pmc {
            ny
        } else {
            ny - 1
        };
        for i in i_lo..=i_hi {
            for j in j_lo..=j_hi {
                let dhydx = if i == 0 {
                    2.0 * self.hy.at(0, j) * inv_d
                } else if i == nx {
                    -2.0 * self.hy.at(nx - 1, j) * inv_d
                } else {
                    (self.hy.at(i, j) - self.hy.at(i - 1, j)) * inv_d
                };
                let dhxdy = if j == 0 {
                    2.0 * self.hx.at(i, 0) * inv_d
                } else if j == ny {
                    -2.0 * self.hx.at(i, ny - 1) * inv_d
                } else {
                    (self.hx.at(i, j) - self.hx.at(i, j - 1)) * inv_d
                };
                let pa = self.psi.psi_a.at_mut(i, j);
                *pa = self.cx_n.b[i] * *pa + self.cx_n.a[i] * dhydx;
                let pb = self.psi.psi_b.at_mut(i, j);
                *pb = self.cy_n.b[j] * *pb + self.cy_n.a[j] * dhxdy;
                let pa = self.psi.psi_a.at(i, j);
                let pb = self.psi.psi_b.at(i, j);
                *self.ez.at_mut(i, j) += dt
                    * self.inv_eps_z.at(i, j)
                    * (self.cx_n.inv_kappa[i] * dhydx + pa - self.cy_n.inv_kappa[j] * dhxdy - pb);
            }
        }
        // TF/SF Ez correction: the Ez update at column i0+1 (total
        // field) consumed Hy[i0+½] which lacks the incident mode — add
        // hy_inc = −n_eff·P·s(t + n_eff·Δ/2) at the H time level just
        // used ((n+½)·dt). Net sign: Ez += dt/ε·(−hy_inc)/Δ.
        let t_h_used = (self.step_index as f64 + 0.5) * dt;
        let delta = self.spec.delta;
        for k in 0..self.sources.len() {
            if let SourcePlacement::TfsfVerticalLine {
                i0,
                j0,
                j1,
                profile,
                n_eff,
            } = &self.sources[k].placement
            {
                let s = self.sources[k].amplitude
                    * self.sources[k]
                        .waveform
                        .eval(t_h_used + n_eff * delta / 2.0);
                if s == 0.0 {
                    continue;
                }
                for (kk, j) in (*j0..=*j1).enumerate() {
                    let i = *i0 + 1;
                    *self.ez.at_mut(i, j) +=
                        dt * self.inv_eps_z.at(i, j) * inv_d * n_eff * profile[kk] * s;
                }
            }
        }
        // Soft E sources at t = (n+1)·dt.
        let t_e = (self.step_index as f64 + 1.0) * dt;
        for k in 0..self.sources.len() {
            let s = self.sources[k].amplitude * self.sources[k].waveform.eval(t_e);
            if s == 0.0 {
                continue;
            }
            match self.sources[k].placement.clone() {
                SourcePlacement::Point { i, j } => *self.ez.at_mut(i, j) += s,
                SourcePlacement::VerticalLine { i, j0, j1, profile } => {
                    for (kk, j) in (j0..=j1).enumerate() {
                        *self.ez.at_mut(i, j) += s * profile[kk];
                    }
                }
                SourcePlacement::TfsfVerticalLine { .. } => {} // handled above
            }
        }
    }

    fn step_te(&mut self) {
        let (nx, ny) = (self.spec.nx, self.spec.ny);
        let inv_d = 1.0 / self.spec.delta;
        let dt = self.dt;
        // H update: E^n → Hz^(n+½).
        for i in 0..nx {
            for j in 0..ny {
                let dexdy = (self.ex.at(i, j + 1) - self.ex.at(i, j)) * inv_d;
                let deydx = (self.ey.at(i + 1, j) - self.ey.at(i, j)) * inv_d;
                let pc = self.psi.psi_c.at_mut(i, j);
                *pc = self.cy_h.b[j] * *pc + self.cy_h.a[j] * dexdy;
                let pd = self.psi.psi_d.at_mut(i, j);
                *pd = self.cx_h.b[i] * *pd + self.cx_h.a[i] * deydx;
                let pc = self.psi.psi_c.at(i, j);
                let pd = self.psi.psi_d.at(i, j);
                *self.hz.at_mut(i, j) += dt
                    * (self.cy_h.inv_kappa[j] * dexdy + pc - self.cx_h.inv_kappa[i] * deydx - pd);
            }
        }
        // Soft H sources at t = (n+½)·dt, before E consumes Hz.
        let t_h = (self.step_index as f64 + 0.5) * dt;
        for k in 0..self.sources.len() {
            let s = self.sources[k].amplitude * self.sources[k].waveform.eval(t_h);
            if s == 0.0 {
                continue;
            }
            match self.sources[k].placement.clone() {
                SourcePlacement::Point { i, j } => *self.hz.at_mut(i, j) += s,
                SourcePlacement::VerticalLine { i, j0, j1, profile } => {
                    for (kk, j) in (j0..=j1).enumerate() {
                        *self.hz.at_mut(i, j) += s * profile[kk];
                    }
                }
                // Rejected for TE at add_source.
                SourcePlacement::TfsfVerticalLine { .. } => unreachable!(),
            }
        }
        // E update: Hz^(n+½) → E^(n+1). Ex is tangential to y-walls (PEC
        // pins j = 0, ny; PMC mirrors Hz), Ey tangential to x-walls.
        let j_lo = if self.boundaries.y_lo == Wall::Pmc {
            0
        } else {
            1
        };
        let j_hi = if self.boundaries.y_hi == Wall::Pmc {
            ny
        } else {
            ny - 1
        };
        for i in 0..nx {
            for j in j_lo..=j_hi {
                let dhzdy = if j == 0 {
                    2.0 * self.hz.at(i, 0) * inv_d
                } else if j == ny {
                    -2.0 * self.hz.at(i, ny - 1) * inv_d
                } else {
                    (self.hz.at(i, j) - self.hz.at(i, j - 1)) * inv_d
                };
                let pa = self.psi.psi_a.at_mut(i, j);
                *pa = self.cy_n.b[j] * *pa + self.cy_n.a[j] * dhzdy;
                let pa = self.psi.psi_a.at(i, j);
                *self.ex.at_mut(i, j) +=
                    dt * self.inv_eps_x.at(i, j) * (self.cy_n.inv_kappa[j] * dhzdy + pa);
            }
        }
        let i_lo = if self.boundaries.x_lo == Wall::Pmc {
            0
        } else {
            1
        };
        let i_hi = if self.boundaries.x_hi == Wall::Pmc {
            nx
        } else {
            nx - 1
        };
        for i in i_lo..=i_hi {
            for j in 0..ny {
                let dhzdx = if i == 0 {
                    2.0 * self.hz.at(0, j) * inv_d
                } else if i == nx {
                    -2.0 * self.hz.at(nx - 1, j) * inv_d
                } else {
                    (self.hz.at(i, j) - self.hz.at(i - 1, j)) * inv_d
                };
                let pb = self.psi.psi_b.at_mut(i, j);
                *pb = self.cx_n.b[i] * *pb + self.cx_n.a[i] * dhzdx;
                let pb = self.psi.psi_b.at(i, j);
                *self.ey.at_mut(i, j) -=
                    dt * self.inv_eps_y.at(i, j) * (self.cx_n.inv_kappa[i] * dhzdx + pb);
            }
        }
    }

    /// Accumulate DFT monitors and probes. E phasors use t = (n+1)·dt, H
    /// phasors t = (n+½)·dt — the Yee half-step offset is carried here.
    fn record_monitors(&mut self) {
        let dt = self.dt;
        let t_e = (self.step_index as f64 + 1.0) * dt;
        let t_h = (self.step_index as f64 + 0.5) * dt;
        let pol = self.pol;
        for f in &mut self.fluxes {
            let freqs = f.spec.freqs().to_vec();
            let ns = f.spec.n_samples();
            for (fi, &freq) in freqs.iter().enumerate() {
                let w = 2.0 * std::f64::consts::PI * freq;
                let ph_e = Cplx::cis(w * t_e).scale(dt);
                let ph_h = Cplx::cis(w * t_h).scale(dt);
                match &f.spec {
                    FluxSpec::Vertical { i, j0, j1, .. } => {
                        for (k, j) in (*j0..=*j1).enumerate() {
                            let (e, h) = match pol {
                                Polarization::Tm => (
                                    self.ez.at(*i, j),
                                    0.5 * (self.hy.at(*i - 1, j) + self.hy.at(*i, j)),
                                ),
                                Polarization::Te => (
                                    self.ey.at(*i, j),
                                    0.5 * (self.hz.at(*i - 1, j) + self.hz.at(*i, j)),
                                ),
                            };
                            let idx = fi * ns + k;
                            f.e_acc[idx] = f.e_acc[idx] + ph_e.scale(e);
                            f.h_acc[idx] = f.h_acc[idx] + ph_h.scale(h);
                        }
                    }
                    FluxSpec::Horizontal { j, i0, i1, .. } => {
                        for (k, i) in (*i0..=*i1).enumerate() {
                            let (e, h) = match pol {
                                Polarization::Tm => (
                                    self.ez.at(i, *j),
                                    0.5 * (self.hx.at(i, *j - 1) + self.hx.at(i, *j)),
                                ),
                                Polarization::Te => (
                                    self.ex.at(i, *j),
                                    0.5 * (self.hz.at(i, *j - 1) + self.hz.at(i, *j)),
                                ),
                            };
                            let idx = fi * ns + k;
                            f.e_acc[idx] = f.e_acc[idx] + ph_e.scale(e);
                            f.h_acc[idx] = f.h_acc[idx] + ph_h.scale(h);
                        }
                    }
                }
            }
        }
        for p in &mut self.probes {
            let v = match pol {
                Polarization::Tm => self.ez.at(p.i, p.j),
                Polarization::Te => self.hz.at(p.i, p.j),
            };
            p.series.push(v);
        }
    }

    /// Spectral power through a flux monitor: `(freq, P)` pairs, positive
    /// along +x (vertical lines) / +y (horizontal lines).
    pub fn flux_power(&self, id: FluxId) -> Vec<(f64, f64)> {
        let f = &self.fluxes[id.0];
        let freqs = f.spec.freqs();
        let ns = f.spec.n_samples();
        let d = self.spec.delta;
        // Poynting sign per orientation × polarization (worked out from
        // the phasor convention in `monitor`): vertical TM −, vertical
        // TE +, horizontal TM +, horizontal TE −.
        let sign = match (&f.spec, self.pol) {
            (FluxSpec::Vertical { .. }, Polarization::Tm) => -1.0,
            (FluxSpec::Vertical { .. }, Polarization::Te) => 1.0,
            (FluxSpec::Horizontal { .. }, Polarization::Tm) => 1.0,
            (FluxSpec::Horizontal { .. }, Polarization::Te) => -1.0,
        };
        freqs
            .iter()
            .enumerate()
            .map(|(fi, &freq)| {
                let mut p = 0.0;
                for k in 0..ns {
                    let idx = fi * ns + k;
                    p += (f.e_acc[idx] * f.h_acc[idx].conj()).re;
                }
                (freq, sign * 0.5 * d * p)
            })
            .collect()
    }

    /// A flux monitor's accumulated DFT phasors `(E, H)`, freq-major
    /// (`freqs.len() × n_samples`), for cross-run subtraction.
    pub fn flux_phasors(&self, id: FluxId) -> (Vec<Cplx>, Vec<Cplx>) {
        let f = &self.fluxes[id.0];
        (f.e_acc.clone(), f.h_acc.clone())
    }

    /// Preload a flux monitor with the **negated** phasors of a reference
    /// run (before the first step): after this run, the monitor holds
    /// `fields − reference fields`, so its flux is the flux of the
    /// scattered field alone — the standard way to measure reflection
    /// exactly even when scattered and incident light co-propagate
    /// (Meep's `load_minus_flux`). The reference must come from an
    /// identically placed monitor with the same frequencies.
    pub fn subtract_flux_phasors(&mut self, id: FluxId, e: &[Cplx], h: &[Cplx]) {
        self.assert_configurable();
        let f = &mut self.fluxes[id.0];
        assert_eq!(f.e_acc.len(), e.len(), "reference monitor shape mismatch");
        assert_eq!(f.h_acc.len(), h.len(), "reference monitor shape mismatch");
        for (a, r) in f.e_acc.iter_mut().zip(e) {
            *a = *a + r.scale(-1.0);
        }
        for (a, r) in f.h_acc.iter_mut().zip(h) {
            *a = *a + r.scale(-1.0);
        }
    }

    /// A probe's recorded series (one sample per step).
    pub fn probe_series(&self, id: ProbeId) -> &[f64] {
        &self.probes[id.0].series
    }

    /// The time of probe sample `n` (0-based): `(n+1)·dt` for TM (Ez is
    /// recorded after its update), `(n+½)·dt` for TE (Hz).
    pub fn probe_sample_time(&self, n: usize) -> f64 {
        match self.pol {
            Polarization::Tm => (n as f64 + 1.0) * self.dt,
            Polarization::Te => (n as f64 + 0.5) * self.dt,
        }
    }

    /// Time step.
    pub fn dt(&self) -> f64 {
        self.dt
    }

    /// Courant factor.
    pub fn courant(&self) -> f64 {
        self.courant
    }

    /// Steps taken so far.
    pub fn step_index(&self) -> usize {
        self.step_index
    }

    /// Elapsed simulated time (of the E fields), `steps·dt`.
    pub fn time(&self) -> f64 {
        self.step_index as f64 * self.dt
    }

    /// Grid spec.
    pub fn grid(&self) -> GridSpec {
        self.spec
    }

    /// Polarization.
    pub fn polarization(&self) -> Polarization {
        self.pol
    }

    /// The CPML configuration.
    pub fn cpml_spec(&self) -> &CpmlSpec {
        &self.cpml
    }

    /// The registered sources.
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// Ez at node `(i, j)` (TM only).
    pub fn ez_at(&self, i: usize, j: usize) -> f64 {
        assert_eq!(self.pol, Polarization::Tm);
        self.ez.at(i, j)
    }

    /// Add `dv` to Ez at node `(i, j)` (TM only) — an externally driven
    /// soft source. Calling between steps is time-equivalent to a soft
    /// source applied at the end of the previous step; the adjoint driver
    /// ([`crate::adjoint`]) uses this to inject per-sample,
    /// per-ε-scaled monitor sources that the [`crate::source::Source`]
    /// list cannot express.
    pub fn inject_ez(&mut self, i: usize, j: usize, dv: f64) {
        assert_eq!(self.pol, Polarization::Tm);
        *self.ez.at_mut(i, j) += dv;
    }

    /// Set the ε_z sample at `(i, j)` absolutely (TM only, before the
    /// first step) — how [`crate::design::TopologyParam::apply`] stamps a
    /// realized topology into the grid.
    pub fn set_epsilon_at(&mut self, i: usize, j: usize, eps: f64) {
        self.assert_configurable();
        assert_eq!(self.pol, Polarization::Tm);
        assert!(eps >= 1.0, "relative permittivity must be ≥ 1");
        *self.eps_z.at_mut(i, j) = eps;
    }

    /// Perturb the committed-to-be ε_z sample at `(i, j)` by `d_eps`
    /// (TM only, before the first step): the finite-difference probe used
    /// to validate adjoint gradients cell by cell.
    pub fn perturb_epsilon_at(&mut self, i: usize, j: usize, d_eps: f64) {
        self.assert_configurable();
        assert_eq!(self.pol, Polarization::Tm);
        let v = self.eps_z.at(i, j) + d_eps;
        assert!(v >= 1.0, "perturbed ε must stay ≥ 1");
        *self.eps_z.at_mut(i, j) = v;
    }

    /// Hz at sample `(i+½, j+½)` (TE only).
    pub fn hz_at(&self, i: usize, j: usize) -> f64 {
        assert_eq!(self.pol, Polarization::Te);
        self.hz.at(i, j)
    }

    /// The painted permittivity sample lattice(s), for inspection:
    /// TM returns (ε_z, None); TE returns (ε_x, Some(ε_y)).
    pub fn epsilon(&self) -> (&Field2, Option<&Field2>) {
        match self.pol {
            Polarization::Tm => (&self.eps_z, None),
            Polarization::Te => (&self.eps_x, Some(&self.eps_y)),
        }
    }

    /// Step once and return the discrete Yee energy invariant
    ///
    /// ```text
    /// U^(n+½) = ½·(⟨ε·E^(n+1), E^n⟩ + ⟨H^(n+½), H^(n+½)⟩)
    /// ```
    ///
    /// which is **exactly conserved** (to rounding) by the leapfrog with
    /// PEC walls, no CPML, and no active sources — the staggered curl
    /// operators are adjoint under these boundary conditions, so the
    /// cross terms cancel identically. Any sign, indexing, or boundary
    /// bug destroys the invariance at first order; the validation suite
    /// asserts conservation at 1e−11 relative. Not an invariant under
    /// PML (by design — it absorbs) or PMC walls (the boundary samples
    /// would need half-cell weights).
    pub fn step_measuring_energy(&mut self) -> f64 {
        match self.pol {
            Polarization::Tm => {
                let prev = self.ez.clone();
                self.step();
                let mut ue = 0.0;
                for k in 0..prev.as_slice().len() {
                    ue += self.eps_z.as_slice()[k] * self.ez.as_slice()[k] * prev.as_slice()[k];
                }
                0.5 * (ue + self.hx.norm2() + self.hy.norm2())
            }
            Polarization::Te => {
                let prev_x = self.ex.clone();
                let prev_y = self.ey.clone();
                self.step();
                let mut ue = 0.0;
                for k in 0..prev_x.as_slice().len() {
                    ue += self.eps_x.as_slice()[k] * self.ex.as_slice()[k] * prev_x.as_slice()[k];
                }
                for k in 0..prev_y.as_slice().len() {
                    ue += self.eps_y.as_slice()[k] * self.ey.as_slice()[k] * prev_y.as_slice()[k];
                }
                0.5 * (ue + self.hz.norm2())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::Waveform;

    /// Chunked stepping (`run(a)` then `run(b)`) must be bit-identical to one
    /// `run(a + b)` — the contract the stepwise MCP progress seam relies on.
    #[test]
    fn chunked_run_matches_one_shot_run() {
        let n = 40;
        let build = || {
            let mut sim = Simulation::new(GridSpec::new(n, n, 0.05), Polarization::Tm);
            sim.set_cpml(crate::CpmlSpec::uniform(8));
            sim.add_source(Source::point(n / 2, n / 2, Waveform::gaussian(2.0, 0.6)));
            let f = sim.add_flux(crate::monitor::FluxSpec::Vertical {
                i: n - 12,
                j0: 10,
                j1: n - 10,
                freqs: vec![2.0],
            });
            (sim, f)
        };
        let (mut one_shot, f1) = build();
        one_shot.run(150);
        let (mut chunked, f2) = build();
        chunked.run(37);
        chunked.run(63);
        chunked.run(50);
        for i in 0..=n {
            for j in 0..=n {
                assert_eq!(
                    one_shot.ez_at(i, j).to_bits(),
                    chunked.ez_at(i, j).to_bits(),
                    "field diverged at ({i},{j})"
                );
            }
        }
        assert_eq!(one_shot.flux_power(f1), chunked.flux_power(f2));
    }

    /// A centered point source in a symmetric vacuum PEC box must produce
    /// exactly symmetric fields — mirror and transpose. Catches staggering
    /// and sign bugs at machine precision.
    #[test]
    fn tm_point_source_has_exact_fourfold_symmetry() {
        let n = 40;
        let mut sim = Simulation::new(GridSpec::new(n, n, 0.05), Polarization::Tm);
        sim.add_source(Source::point(n / 2, n / 2, Waveform::gaussian(2.0, 0.6)));
        sim.run(120);
        let c = n / 2;
        let mut max_v: f64 = 0.0;
        for i in 0..=n {
            for j in 0..=n {
                max_v = max_v.max(sim.ez_at(i, j).abs());
            }
        }
        assert!(max_v > 0.0, "fields never became nonzero");
        for d in 1..=(n / 2) {
            for j in 0..=n {
                let a = sim.ez_at(c + d, j);
                let b = sim.ez_at(c - d, j);
                assert!(
                    (a - b).abs() <= 1e-13 * max_v,
                    "x-mirror broken at d={d}, j={j}: {a} vs {b}"
                );
            }
        }
        for i in 0..=n {
            for j in 0..i {
                let a = sim.ez_at(i, j);
                let b = sim.ez_at(j, i);
                assert!(
                    (a - b).abs() <= 1e-13 * max_v,
                    "transpose broken at ({i},{j}): {a} vs {b}"
                );
            }
        }
    }

    #[test]
    fn te_point_source_has_exact_fourfold_symmetry() {
        // Odd cell count: Hz samples sit at half positions, so only an
        // odd grid centers the *box* on a sample — with an even grid the
        // walls sit 20.5Δ vs 19.5Δ from the source and their (tiny,
        // gate-discontinuity-amplitude) echoes break mirror symmetry.
        let n = 41;
        let c = n / 2;
        let mut sim = Simulation::new(GridSpec::new(n, n, 0.05), Polarization::Te);
        sim.add_source(Source::point(c, c, Waveform::gaussian(2.0, 0.6)));
        sim.run(120);
        let mut max_v: f64 = 0.0;
        for i in 0..n {
            for j in 0..n {
                max_v = max_v.max(sim.hz_at(i, j).abs());
            }
        }
        assert!(max_v > 0.0);
        for d in 1..=(n / 2 - 1) {
            for j in 0..n {
                let a = sim.hz_at(c + d, j);
                let b = sim.hz_at(c - d, j);
                assert!(
                    (a - b).abs() <= 1e-13 * max_v,
                    "x-mirror broken at d={d}, j={j}: {a} vs {b}"
                );
            }
        }
        for i in 0..n {
            for j in 0..i {
                let a = sim.hz_at(i, j);
                let b = sim.hz_at(j, i);
                assert!(
                    (a - b).abs() <= 1e-13 * max_v,
                    "transpose broken at ({i},{j})"
                );
            }
        }
    }

    #[test]
    fn stepping_freezes_configuration() {
        let mut sim = Simulation::new(GridSpec::new(10, 10, 0.1), Polarization::Tm);
        sim.step();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sim.set_courant(0.4);
        }));
        assert!(result.is_err());
    }

    #[test]
    fn fields_stay_bounded_with_cpml() {
        let mut sim = Simulation::new(GridSpec::new(60, 40, 0.05), Polarization::Tm);
        sim.set_cpml(CpmlSpec::uniform(10));
        sim.add_source(Source::point(30, 20, Waveform::gaussian(2.0, 0.5)));
        sim.run(800);
        let mut max_v: f64 = 0.0;
        for i in 0..=60 {
            for j in 0..=40 {
                max_v = max_v.max(sim.ez_at(i, j).abs());
            }
        }
        // Long after the pulse, everything must have been absorbed.
        assert!(max_v < 1e-6, "residual field {max_v} — CPML not absorbing");
    }
}
