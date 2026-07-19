//! Steady-state finite-volume conduction solver.
//!
//! Discretization: integrate ∇·(k∇T) + q‴ = 0 over each voxel. The flux
//! between voxels P and N through their shared face of area A at spacing d
//! is G·(T_N − T_P) with the **series-resistance (harmonic-mean) face
//! conductance**
//!
//! ```text
//! G = A / ( d/(2·k_P) + d/(2·k_N) )
//! ```
//!
//! — the two half-cells conduct in series. This is the standard
//! finite-volume interface treatment, and it is *exact* for 1D layered
//! composites (the composite-slab test reproduces the series-resistance
//! formula to machine precision). The arithmetic mean (k_P + k_N)/2 is
//! wrong at material interfaces: it lets the high-k side short across the
//! face — a copper|air face would conduct like half-strength copper
//! instead of like air, an error of ~4 orders of magnitude. Harmonic
//! recovers the limiting resistance of the worse conductor, which is the
//! physics.
//!
//! Boundary faces put the half-cell resistance d/(2k) in series with the
//! surface condition: a fixed-temperature face is G = 2kA/d to the surface
//! temperature; a convection face adds the 1/h film, G = A/(d/(2k) + 1/h),
//! so h → ∞ recovers Dirichlet. Because every internal face conductance is
//! shared symmetrically between its two voxels, the scheme is
//! **conservative by construction**: heat leaving P through a face enters
//! N exactly, and global energy balance closes to solver tolerance (it is
//! reported on every solution, never assumed).
//!
//! The linear system is symmetric positive definite (given every free
//! region has some path to a temperature reference — checked, fail-closed)
//! and is solved matrix-free by Jacobi-preconditioned conjugate gradients.
//! The stopping criterion is ‖r‖ ≤ tol·‖b‖ — **relative to the
//! right-hand-side norm** (sources + boundary drives), the scale-invariant
//! choice: an absolute epsilon would read a milliwatt problem as converged
//! at iteration zero.

use crate::model::{face_index, Boundary, ModelError, ThermalModel};
use serde::Serialize;

/// BC-slot index used for `ThermalModel::exposed` in convection links and
/// film-coefficient gradients (slots 0..=5 are the domain faces in
/// [`face_index`] order).
pub(crate) const EXPOSED_SLOT: usize = 6;

/// Options for [`solve_steady`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOptions {
    /// Relative residual tolerance: stop when ‖r‖₂ ≤ tol·‖b‖₂.
    pub tol: f64,
    /// Hard cap on CG iterations.
    pub max_iters: usize,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            tol: 1e-8,
            max_iters: 50_000,
        }
    }
}

/// Failure modes of [`solve_steady`] (all fail-closed).
#[derive(Debug, Clone, PartialEq)]
pub enum SolveError {
    /// The model failed validation.
    Model(ModelError),
    /// A connected region of free voxels has no path to any temperature
    /// reference (no Dirichlet face, reservoir, or convection link): its
    /// temperature is undefined (or unbounded, if it holds a source).
    FloatingRegion {
        /// Voxel count of the floating component.
        voxels: usize,
        /// Center of one voxel in the component, mm.
        near_mm: [f64; 3],
    },
    /// Sources are present but no θ reference is resolvable: set
    /// `ThermalModel::reference_c`, or use a single convection ambient.
    AmbiguousReference,
    /// CG did not reach `tol` within `max_iters`.
    NotConverged {
        /// Final relative residual ‖r‖/‖b‖.
        residual_rel: f64,
        /// Iterations performed.
        iterations: usize,
    },
    /// A transient solve was asked for a non-positive time step or zero
    /// steps.
    InvalidTimeStep,
    /// A transient schedule segment named a source, face, or fixed region
    /// it cannot override (unknown name, out-of-range slot, or an
    /// adiabatic face with no temperature to move).
    BadScheduleOverride(String),
    /// The smooth-max exponent must be finite and > 1.
    InvalidSmoothingExponent,
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolveError::Model(e) => write!(f, "invalid model: {e}"),
            SolveError::FloatingRegion { voxels, near_mm } => write!(
                f,
                "floating region of {voxels} voxels near ({:.2}, {:.2}, {:.2}) mm has no \
                 temperature reference (all faces adiabatic); its temperature is undefined",
                near_mm[0], near_mm[1], near_mm[2]
            ),
            SolveError::AmbiguousReference => write!(
                f,
                "theta reference unresolvable: set reference_c or use a single convection ambient"
            ),
            SolveError::NotConverged {
                residual_rel,
                iterations,
            } => write!(
                f,
                "CG not converged after {iterations} iterations (relative residual {residual_rel:.3e})"
            ),
            SolveError::InvalidTimeStep => {
                write!(f, "transient solve requires dt > 0 and at least one step")
            }
            SolveError::BadScheduleOverride(why) => {
                write!(f, "invalid schedule override: {why}")
            }
            SolveError::InvalidSmoothingExponent => {
                write!(f, "smooth-max exponent p must be finite and > 1")
            }
        }
    }
}

impl std::error::Error for SolveError {}

impl From<ModelError> for SolveError {
    fn from(e: ModelError) -> Self {
        SolveError::Model(e)
    }
}

/// Per-source figures of merit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceReport {
    /// Source name.
    pub name: String,
    /// Total source power, W.
    pub power_w: f64,
    /// Hottest temperature over the source's voxels, °C.
    pub t_max_c: f64,
    /// Center of the hottest source voxel, mm.
    pub t_max_at_mm: [f64; 3],
    /// Thermal resistance θ = (t_max_c − reference_c)/power_w, K/W.
    /// `None` when the source power is zero.
    pub theta_c_per_w: Option<f64>,
}

/// Global energy bookkeeping, from the solved field.
///
/// Flows are positive **out of** the conducting system. In steady state the
/// source power must equal the net outflow; `residual_rel` is how far the
/// solved field misses that, normalized by the larger of the source power
/// and the gross boundary traffic. It closes to solver tolerance — if it
/// doesn't, the solution is wrong and the number says so.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnergyBalance {
    /// Total source power into free voxels, W.
    pub source_w: f64,
    /// Net heat out through fixed-temperature domain/exposed faces, W.
    pub fixed_face_out_w: f64,
    /// Net heat out through convection faces, W.
    pub convection_out_w: f64,
    /// Net heat out into fixed-temperature reservoirs, W.
    pub fixed_region_out_w: f64,
    /// Sum of the three outflows, W.
    pub net_out_w: f64,
    /// |source − net out| / max(|source|, gross flow).
    pub residual_rel: f64,
}

/// The solved temperature field and its figures of merit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Solution {
    /// Voxel counts per axis.
    pub divisions: [usize; 3],
    /// Domain minimum corner, mm.
    pub origin_mm: [f64; 3],
    /// Voxel edge lengths, mm.
    pub voxel_mm: [f64; 3],
    /// Temperature per voxel, °C; `NaN` for void voxels.
    /// Layout: `index = (k·ny + j)·nx + i`.
    pub t_c: Vec<f64>,
    /// Which voxels carry material.
    pub solid: Vec<bool>,
    /// CG iterations used.
    pub iterations: usize,
    /// Final relative residual ‖r‖/‖b‖.
    pub residual_rel: f64,
    /// Hottest solid-voxel temperature, °C.
    pub t_max_c: f64,
    /// Center of the hottest voxel, mm.
    pub t_max_at_mm: [f64; 3],
    /// θ reference temperature, °C (`None` when no sources needed one).
    pub reference_c: Option<f64>,
    /// Per-source reports, in model order.
    pub sources: Vec<SourceReport>,
    /// Per-reservoir heat flows, in `ThermalModel::fixed` order.
    pub reservoirs: Vec<ReservoirReport>,
    /// Energy balance of the solved field.
    pub energy: EnergyBalance,
}

impl Solution {
    /// Linear index of voxel `(i, j, k)`.
    pub fn index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.divisions[1] + j) * self.divisions[0] + i
    }

    /// Temperature of voxel `(i, j, k)`, °C (`NaN` for void).
    pub fn temperature_c(&self, i: usize, j: usize, k: usize) -> f64 {
        self.t_c[self.index(i, j, k)]
    }

    /// Center of voxel `(i, j, k)`, mm.
    pub fn voxel_center_mm(&self, i: usize, j: usize, k: usize) -> [f64; 3] {
        [
            self.origin_mm[0] + (i as f64 + 0.5) * self.voxel_mm[0],
            self.origin_mm[1] + (j as f64 + 0.5) * self.voxel_mm[1],
            self.origin_mm[2] + (k as f64 + 0.5) * self.voxel_mm[2],
        ]
    }
}

/// Heat absorbed by one fixed-temperature reservoir.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReservoirReport {
    /// The reservoir's pinned temperature, °C.
    pub temperature_c: f64,
    /// Net heat flowing from the free system into this reservoir, W
    /// (negative when the reservoir heats the part).
    pub heat_absorbed_w: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LinkKind {
    FixedFace,
    /// Convection link; the payload is the BC slot that created it:
    /// 0..=5 for the domain faces (in `face_index` order), 6 for
    /// `ThermalModel::exposed`.
    Convection(usize),
    /// Link into a fixed region; the payload is the region index in
    /// `ThermalModel::fixed` that owns the pinned voxel (painter's order).
    FixedRegion(usize),
}

pub(crate) struct BcLink {
    pub(crate) voxel: usize,
    /// Face axis (0, 1, 2) — the conductance's half-cell distance and
    /// area follow it.
    pub(crate) axis: usize,
    pub(crate) g: f64,
    pub(crate) t_ref: f64,
    pub(crate) kind: LinkKind,
}

/// The assembled discrete system (crate-internal; the transient solver
/// reuses it with a time term added to the diagonal).
pub(crate) struct System {
    pub(crate) n: [usize; 3],
    pub(crate) solid: Vec<bool>,
    pub(crate) free: Vec<bool>,
    pub(crate) tfix: Vec<f64>,
    /// Pair conductance to the +axis neighbor, W/K (0 when absent).
    pub(crate) g_pos: [Vec<f64>; 3],
    pub(crate) diag: Vec<f64>,
    pub(crate) b: Vec<f64>,
    pub(crate) links: Vec<BcLink>,
    /// (name, total power, voxel ids) per source.
    pub(crate) sources: Vec<(String, f64, Vec<usize>)>,
    pub(crate) source_w_total: f64,
    /// Volumetric heat capacity per voxel, J/(m³·K); −1 marks a painted
    /// material that declared none (steady solves never read this).
    pub(crate) rc: Vec<f64>,
    /// Index into `ThermalModel::materials` that painted each voxel.
    pub(crate) mat_id: Vec<usize>,
    pub(crate) cell_volume_m3: f64,
    /// Per-axis conductivity per voxel (0 for void) — kept for the
    /// adjoint's conductance chain rules.
    pub(crate) kfield: [Vec<f64>; 3],
    /// Voxel spacing per axis, m.
    pub(crate) d_m: [f64; 3],
    /// Face area per axis, m².
    pub(crate) area: [f64; 3],
}

impl System {
    fn idx(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.n[1] + j) * self.n[0] + i
    }

    /// y = diag·x − Σ G·x_neighbor on the free subspace (x, y are
    /// full-grid vectors; non-free entries are ignored/zeroed). `diag`
    /// is passed in so the transient solver can add its C/Δt time term
    /// without rebuilding the links.
    pub(crate) fn apply(&self, diag: &[f64], x: &[f64], y: &mut [f64]) {
        let [nx, ny, nz] = self.n;
        for (p, yy) in y.iter_mut().enumerate() {
            *yy = if self.free[p] { diag[p] * x[p] } else { 0.0 };
        }
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let p = self.idx(i, j, k);
                    for (axis, stride) in [(0usize, 1usize), (1, nx), (2, nx * ny)] {
                        let last = match axis {
                            0 => i + 1 >= nx,
                            1 => j + 1 >= ny,
                            _ => k + 1 >= nz,
                        };
                        if last {
                            continue;
                        }
                        let g = self.g_pos[axis][p];
                        if g <= 0.0 {
                            continue;
                        }
                        let q = p + stride;
                        if self.free[p] && self.free[q] {
                            y[p] -= g * x[q];
                            y[q] -= g * x[p];
                        }
                    }
                }
            }
        }
    }

    /// Net heat leaving the free system through every boundary link
    /// (Dirichlet faces, convection faces, reservoir contacts) for the
    /// free-node temperature vector `t`, W.
    pub(crate) fn boundary_outflow_w(&self, t: &[f64]) -> f64 {
        self.links
            .iter()
            .map(|l| l.g * (t[l.voxel] - l.t_ref))
            .sum()
    }
}

pub(crate) fn build(model: &ThermalModel) -> Result<System, SolveError> {
    let n = model.divisions;
    if n.contains(&0) || model.size_mm.iter().any(|&s| s <= 0.0) {
        return Err(ModelError::EmptyDomain.into());
    }
    for (index, m) in model.materials.iter().enumerate() {
        if m.k_w_mk.iter().any(|&k| k <= 0.0) {
            return Err(ModelError::NonPositiveConductivity { index }.into());
        }
    }
    for bc in model
        .domain_faces
        .iter()
        .chain(std::iter::once(&model.exposed))
    {
        if let Boundary::Convection { h_w_m2k, .. } = bc {
            if *h_w_m2k <= 0.0 {
                return Err(ModelError::NonPositiveFilmCoefficient.into());
            }
        }
    }

    let [nx, ny, nz] = n;
    let nvox = nx * ny * nz;
    let d_mm = model.voxel_mm();
    let d_m = [d_mm[0] * 1e-3, d_mm[1] * 1e-3, d_mm[2] * 1e-3];
    // Face areas per axis, m².
    let area = [d_m[1] * d_m[2], d_m[0] * d_m[2], d_m[0] * d_m[1]];
    let idx = |i: usize, j: usize, k: usize| (k * ny + j) * nx + i;

    // Paint materials. Conductivity is a per-axis diagonal tensor; heat
    // capacity rides along for the transient solver (-1.0 marks "painted
    // with no capacity"). Two producers: an external per-voxel index
    // (the tessellated-part seam — overrides shapes entirely), or region
    // shapes in painter's order (later regions win).
    let mut kfield = [
        vec![0.0_f64; nvox],
        vec![0.0_f64; nvox],
        vec![0.0_f64; nvox],
    ];
    let mut rc = vec![-1.0_f64; nvox];
    let mut mat_id = vec![usize::MAX; nvox];
    if let Some(vm) = &model.voxel_materials {
        if vm.material_index.len() != nvox {
            return Err(ModelError::VoxelFieldWrongLength {
                expected: nvox,
                got: vm.material_index.len(),
            }
            .into());
        }
        for (p, &raw) in vm.material_index.iter().enumerate() {
            if raw < 0 {
                continue;
            }
            let mi = raw as usize;
            let Some(m) = model.materials.get(mi) else {
                return Err(ModelError::VoxelFieldBadIndex {
                    index: raw,
                    materials: model.materials.len(),
                }
                .into());
            };
            for (axis, kf) in kfield.iter_mut().enumerate() {
                kf[p] = m.k_w_mk[axis];
            }
            rc[p] = m.heat_capacity_j_m3k.unwrap_or(-1.0);
            mat_id[p] = mi;
        }
    } else {
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let c = model.voxel_center_mm(i, j, k);
                    for (mi, m) in model.materials.iter().enumerate() {
                        if m.shape.contains(c) {
                            let p = idx(i, j, k);
                            for (axis, kf) in kfield.iter_mut().enumerate() {
                                kf[p] = m.k_w_mk[axis];
                            }
                            rc[p] = m.heat_capacity_j_m3k.unwrap_or(-1.0);
                            mat_id[p] = mi;
                        }
                    }
                }
            }
        }
    }
    let solid: Vec<bool> = kfield[0].iter().map(|&k| k > 0.0).collect();
    if !solid.iter().any(|&s| s) {
        return Err(ModelError::NoSolidVoxels.into());
    }

    // Pin reservoirs (later regions win); only solid voxels can be pinned.
    let mut fixed = vec![false; nvox];
    let mut tfix = vec![0.0_f64; nvox];
    let mut fixed_id = vec![usize::MAX; nvox];
    for (index, fx) in model.fixed.iter().enumerate() {
        let mut hits = 0usize;
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let p = idx(i, j, k);
                    if solid[p] && fx.shape.contains(model.voxel_center_mm(i, j, k)) {
                        fixed[p] = true;
                        tfix[p] = fx.temperature_c;
                        fixed_id[p] = index;
                        hits += 1;
                    }
                }
            }
        }
        if hits == 0 {
            return Err(ModelError::FixedCoversNoSolid { index }.into());
        }
    }
    let free: Vec<bool> = (0..nvox).map(|p| solid[p] && !fixed[p]).collect();

    // Sources: total watts split equally over covered free voxels.
    let mut b = vec![0.0_f64; nvox];
    let mut sources = Vec::with_capacity(model.sources.len());
    let mut source_w_total = 0.0;
    for s in &model.sources {
        let mut ids = Vec::new();
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let p = idx(i, j, k);
                    if free[p] && s.shape.contains(model.voxel_center_mm(i, j, k)) {
                        ids.push(p);
                    }
                }
            }
        }
        if ids.is_empty() {
            return Err(ModelError::SourceCoversNoFreeSolid {
                name: s.name.clone(),
            }
            .into());
        }
        let share = s.power_w / ids.len() as f64;
        for &p in &ids {
            b[p] += share;
        }
        source_w_total += s.power_w;
        sources.push((s.name.clone(), s.power_w, ids));
    }

    // Internal pair links: harmonic-mean face conductance between solid
    // voxels (the two half-cells in series — see module docs).
    let mut g_pos = [vec![0.0; nvox], vec![0.0; nvox], vec![0.0; nvox]];
    let mut diag = vec![0.0_f64; nvox];
    let mut links: Vec<BcLink> = Vec::new();
    let mut grounded = vec![false; nvox];
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let p = idx(i, j, k);
                if !solid[p] {
                    continue;
                }
                for axis in 0..3 {
                    let (ii, jj, kk) = match axis {
                        0 => (i + 1, j, k),
                        1 => (i, j + 1, k),
                        _ => (i, j, k + 1),
                    };
                    if ii >= nx || jj >= ny || kk >= nz {
                        continue;
                    }
                    let q = idx(ii, jj, kk);
                    if !solid[q] {
                        continue;
                    }
                    let g = area[axis]
                        / (0.5 * d_m[axis] / kfield[axis][p] + 0.5 * d_m[axis] / kfield[axis][q]);
                    g_pos[axis][p] = g;
                    match (free[p], free[q]) {
                        (true, true) => {
                            diag[p] += g;
                            diag[q] += g;
                        }
                        (true, false) => {
                            diag[p] += g;
                            b[p] += g * tfix[q];
                            grounded[p] = true;
                            links.push(BcLink {
                                voxel: p,
                                axis,
                                g,
                                t_ref: tfix[q],
                                kind: LinkKind::FixedRegion(fixed_id[q]),
                            });
                        }
                        (false, true) => {
                            diag[q] += g;
                            b[q] += g * tfix[p];
                            grounded[q] = true;
                            links.push(BcLink {
                                voxel: q,
                                axis,
                                g,
                                t_ref: tfix[p],
                                kind: LinkKind::FixedRegion(fixed_id[p]),
                            });
                        }
                        (false, false) => {}
                    }
                }
            }
        }
    }

    // Boundary faces: domain-box faces and exposed solid↔void faces.
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let p = idx(i, j, k);
                if !free[p] {
                    continue;
                }
                for axis in 0..3 {
                    for positive in [false, true] {
                        let coord = [i, j, k][axis];
                        let limit = n[axis];
                        let at_domain_edge = if positive {
                            coord + 1 >= limit
                        } else {
                            coord == 0
                        };
                        let (bc, slot) = if at_domain_edge {
                            let s = face_index(axis, positive);
                            (model.domain_faces[s], s)
                        } else {
                            let (ii, jj, kk) = match (axis, positive) {
                                (0, true) => (i + 1, j, k),
                                (0, false) => (i - 1, j, k),
                                (1, true) => (i, j + 1, k),
                                (1, false) => (i, j - 1, k),
                                (2, true) => (i, j, k + 1),
                                _ => (i, j, k - 1),
                            };
                            if solid[idx(ii, jj, kk)] {
                                continue; // internal face, handled above
                            }
                            (model.exposed, EXPOSED_SLOT)
                        };
                        let half = 0.5 * d_m[axis] / kfield[axis][p];
                        match bc {
                            Boundary::Adiabatic => {}
                            Boundary::FixedTemperature { temperature_c } => {
                                let g = area[axis] / half;
                                diag[p] += g;
                                b[p] += g * temperature_c;
                                grounded[p] = true;
                                links.push(BcLink {
                                    voxel: p,
                                    axis,
                                    g,
                                    t_ref: temperature_c,
                                    kind: LinkKind::FixedFace,
                                });
                            }
                            Boundary::Convection { h_w_m2k, ambient_c } => {
                                let g = area[axis] / (half + 1.0 / h_w_m2k);
                                diag[p] += g;
                                b[p] += g * ambient_c;
                                grounded[p] = true;
                                links.push(BcLink {
                                    voxel: p,
                                    axis,
                                    g,
                                    t_ref: ambient_c,
                                    kind: LinkKind::Convection(slot),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    let sys = System {
        n,
        solid,
        free,
        tfix,
        g_pos,
        diag,
        b,
        links,
        sources,
        source_w_total,
        rc,
        mat_id,
        cell_volume_m3: d_m[0] * d_m[1] * d_m[2],
        kfield,
        d_m,
        area,
    };

    // Fail closed on floating components: BFS over free voxels through
    // positive pair conductances; every component must touch a reference.
    check_grounding(&sys, &grounded, model)?;
    Ok(sys)
}

fn check_grounding(
    sys: &System,
    grounded: &[bool],
    model: &ThermalModel,
) -> Result<(), SolveError> {
    let [nx, ny, nz] = sys.n;
    let nvox = nx * ny * nz;
    let mut visited = vec![false; nvox];
    let mut stack = Vec::new();
    for start in 0..nvox {
        if !sys.free[start] || visited[start] {
            continue;
        }
        let mut component = Vec::new();
        let mut ok = false;
        visited[start] = true;
        stack.push(start);
        while let Some(p) = stack.pop() {
            component.push(p);
            if grounded[p] {
                ok = true;
            }
            let k = p / (nx * ny);
            let j = (p / nx) % ny;
            let i = p % nx;
            for (axis, stride) in [(0usize, 1usize), (1, nx), (2, nx * ny)] {
                let coord = [i, j, k][axis];
                if coord + 1 < sys.n[axis] && sys.g_pos[axis][p] > 0.0 {
                    let q = p + stride;
                    if sys.free[q] && !visited[q] {
                        visited[q] = true;
                        stack.push(q);
                    }
                }
                if coord > 0 {
                    let q = p - stride;
                    if sys.g_pos[axis][q] > 0.0 && sys.free[q] && !visited[q] {
                        visited[q] = true;
                        stack.push(q);
                    }
                }
            }
        }
        if !ok {
            let p = component[0];
            let k = p / (nx * ny);
            let j = (p / nx) % ny;
            let i = p % nx;
            return Err(SolveError::FloatingRegion {
                voxels: component.len(),
                near_mm: model.voxel_center_mm(i, j, k),
            });
        }
    }
    Ok(())
}

/// Jacobi-preconditioned conjugate gradients, matrix-free, warm-startable.
///
/// `diag` and `b` are passed explicitly so the transient solver can reuse
/// the assembled links with a C/Δt time term folded into the diagonal and
/// a per-step right-hand side; `x0` seeds the iteration (the previous time
/// step in transient runs — steady solves start from zero).
pub(crate) fn pcg(
    sys: &System,
    diag: &[f64],
    b: &[f64],
    x0: &[f64],
    opts: &SolveOptions,
) -> Result<(Vec<f64>, usize, f64), SolveError> {
    let nvox = b.len();
    let b_norm = norm(b, &sys.free);
    if b_norm == 0.0 {
        return Ok((vec![0.0_f64; nvox], 0, 0.0));
    }
    let mut x = x0.to_vec();
    let mut r = vec![0.0_f64; nvox];
    sys.apply(diag, &x, &mut r);
    for p in 0..nvox {
        r[p] = if sys.free[p] { b[p] - r[p] } else { 0.0 };
    }
    let mut residual_rel = norm(&r, &sys.free) / b_norm;
    if residual_rel <= opts.tol {
        return Ok((x, 0, residual_rel));
    }
    let mut z = vec![0.0_f64; nvox];
    precondition(&sys.free, diag, &r, &mut z);
    let mut d = z.clone();
    let mut rz = dot(&r, &z, &sys.free);
    let mut ad = vec![0.0_f64; nvox];
    for iter in 1..=opts.max_iters {
        sys.apply(diag, &d, &mut ad);
        let dad = dot(&d, &ad, &sys.free);
        if dad <= 0.0 {
            // SPD violation: numerically broken-down search direction.
            return Err(SolveError::NotConverged {
                residual_rel,
                iterations: iter,
            });
        }
        let alpha = rz / dad;
        for p in 0..nvox {
            if sys.free[p] {
                x[p] += alpha * d[p];
                r[p] -= alpha * ad[p];
            }
        }
        residual_rel = norm(&r, &sys.free) / b_norm;
        if residual_rel <= opts.tol {
            return Ok((x, iter, residual_rel));
        }
        precondition(&sys.free, diag, &r, &mut z);
        let rz_new = dot(&r, &z, &sys.free);
        let beta = rz_new / rz;
        rz = rz_new;
        for p in 0..nvox {
            if sys.free[p] {
                d[p] = z[p] + beta * d[p];
            }
        }
    }
    Err(SolveError::NotConverged {
        residual_rel,
        iterations: opts.max_iters,
    })
}

fn precondition(free: &[bool], diag: &[f64], r: &[f64], z: &mut [f64]) {
    for (p, zz) in z.iter_mut().enumerate() {
        *zz = if free[p] { r[p] / diag[p] } else { 0.0 };
    }
}

fn dot(a: &[f64], b: &[f64], free: &[bool]) -> f64 {
    let mut s = 0.0;
    for p in 0..a.len() {
        if free[p] {
            s += a[p] * b[p];
        }
    }
    s
}

fn norm(a: &[f64], free: &[bool]) -> f64 {
    dot(a, a, free).sqrt()
}

/// Resolve the θ reference: an explicit `reference_c` wins; otherwise the
/// convection ambients must be unique. With sources present and no
/// resolvable reference, this is an error (fail-closed).
pub(crate) fn resolve_reference(
    sys: &System,
    model: &ThermalModel,
) -> Result<Option<f64>, SolveError> {
    if let Some(r) = model.reference_c {
        return Ok(Some(r));
    }
    let mut ambient: Option<f64> = None;
    let mut unique = true;
    for l in &sys.links {
        if matches!(l.kind, LinkKind::Convection(_)) {
            match ambient {
                None => ambient = Some(l.t_ref),
                Some(a) if (a - l.t_ref).abs() > 1e-9 => unique = false,
                _ => {}
            }
        }
    }
    match (ambient, unique) {
        (Some(a), true) => Ok(Some(a)),
        _ if sys.sources.is_empty() => Ok(None),
        _ => Err(SolveError::AmbiguousReference),
    }
}

/// Solve the steady conduction problem.
pub fn solve_steady(model: &ThermalModel, opts: &SolveOptions) -> Result<Solution, SolveError> {
    let sys = build(model)?;
    let reference_c = resolve_reference(&sys, model)?;
    let x0 = vec![0.0_f64; sys.b.len()];
    let (x, iterations, residual_rel) = pcg(&sys, &sys.diag, &sys.b, &x0, opts)?;
    Ok(assemble_solution(
        &sys,
        model,
        &x,
        iterations,
        residual_rel,
        reference_c,
    ))
}

/// Assemble the public [`Solution`] (field, T_max, per-source θ,
/// reservoirs, energy balance) from a solved free-node vector. Shared
/// between the steady solver and the transient solver's snapshots.
pub(crate) fn assemble_solution(
    sys: &System,
    model: &ThermalModel,
    x: &[f64],
    iterations: usize,
    residual_rel: f64,
    reference_c: Option<f64>,
) -> Solution {
    let [nx, ny, nz] = sys.n;
    let nvox = nx * ny * nz;
    let mut t_c = vec![f64::NAN; nvox];
    for (p, t) in t_c.iter_mut().enumerate() {
        if sys.free[p] {
            *t = x[p];
        } else if sys.solid[p] {
            *t = sys.tfix[p];
        }
    }

    // Hottest solid voxel.
    let mut t_max_c = f64::NEG_INFINITY;
    let mut t_max_p = 0usize;
    for (p, &t) in t_c.iter().enumerate() {
        if sys.solid[p] && t > t_max_c {
            t_max_c = t;
            t_max_p = p;
        }
    }
    let coords = |p: usize| {
        let k = p / (nx * ny);
        let j = (p / nx) % ny;
        let i = p % nx;
        (i, j, k)
    };
    let center = |p: usize| {
        let (i, j, k) = coords(p);
        model.voxel_center_mm(i, j, k)
    };
    let t_max_at_mm = center(t_max_p);

    // Per-source reports.
    let sources = sys
        .sources
        .iter()
        .map(|(name, power_w, ids)| {
            let mut hot = ids[0];
            for &p in ids {
                if t_c[p] > t_c[hot] {
                    hot = p;
                }
            }
            let t_src = t_c[hot];
            let theta = reference_c.and_then(|r| {
                if *power_w != 0.0 {
                    Some((t_src - r) / power_w)
                } else {
                    None
                }
            });
            SourceReport {
                name: name.clone(),
                power_w: *power_w,
                t_max_c: t_src,
                t_max_at_mm: center(hot),
                theta_c_per_w: theta,
            }
        })
        .collect::<Vec<_>>();

    // Energy balance from the solved field: flows positive out of the
    // conducting system.
    let mut fixed_face_out_w = 0.0;
    let mut convection_out_w = 0.0;
    let mut fixed_region_out_w = 0.0;
    let mut gross = 0.0;
    let mut reservoirs: Vec<ReservoirReport> = model
        .fixed
        .iter()
        .map(|f| ReservoirReport {
            temperature_c: f.temperature_c,
            heat_absorbed_w: 0.0,
        })
        .collect();
    for l in &sys.links {
        let flow = l.g * (t_c[l.voxel] - l.t_ref);
        gross += flow.abs();
        match l.kind {
            LinkKind::FixedFace => fixed_face_out_w += flow,
            LinkKind::Convection(_) => convection_out_w += flow,
            LinkKind::FixedRegion(region) => {
                fixed_region_out_w += flow;
                reservoirs[region].heat_absorbed_w += flow;
            }
        }
    }
    let net_out_w = fixed_face_out_w + convection_out_w + fixed_region_out_w;
    let scale = sys.source_w_total.abs().max(gross).max(1e-30);
    let energy = EnergyBalance {
        source_w: sys.source_w_total,
        fixed_face_out_w,
        convection_out_w,
        fixed_region_out_w,
        net_out_w,
        residual_rel: (sys.source_w_total - net_out_w).abs() / scale,
    };

    Solution {
        divisions: sys.n,
        origin_mm: model.origin_mm,
        voxel_mm: model.voxel_mm(),
        t_c,
        solid: sys.solid.clone(),
        iterations,
        residual_rel,
        t_max_c,
        t_max_at_mm,
        reference_c,
        sources,
        reservoirs,
        energy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Boundary, MaterialRegion, PowerSource, Shape, ThermalModel};

    fn slab_model(nx: usize) -> ThermalModel {
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [20.0, 5.0, 5.0], [nx, 1, 1]);
        m.materials.push(MaterialRegion::isotropic(
            Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [20.0, 5.0, 5.0],
            },
            50.0,
        ));
        m
    }

    #[test]
    fn dirichlet_slab_reproduces_the_linear_profile_exactly() {
        // 1D slab, T = 100 at x = 0, T = 0 at x = L: the finite-volume
        // solution is exact at voxel centers for a linear profile (second
        // differences of a linear function vanish; the half-cell boundary
        // conductance closes the end equations exactly).
        let mut m = slab_model(8);
        m.domain_faces[0] = Boundary::FixedTemperature {
            temperature_c: 100.0,
        };
        m.domain_faces[1] = Boundary::FixedTemperature { temperature_c: 0.0 };
        let sol = solve_steady(&m, &SolveOptions::default()).unwrap();
        let dx = 20.0 / 8.0;
        for i in 0..8 {
            let x = (i as f64 + 0.5) * dx;
            let exact = 100.0 * (1.0 - x / 20.0);
            let got = sol.temperature_c(i, 0, 0);
            assert!(
                (got - exact).abs() < 1e-9,
                "voxel {i}: computed {got}, exact {exact}"
            );
        }
        // No sources: net Dirichlet flow must vanish (heat in one face,
        // out the other) — the conservation statement itself.
        assert!(
            sol.energy.fixed_face_out_w.abs() < 1e-9,
            "net Dirichlet outflow must vanish: {}",
            sol.energy.fixed_face_out_w
        );
        assert!(sol.energy.residual_rel < 1e-9);
    }

    #[test]
    fn convection_face_is_the_exact_series_circuit() {
        // Dirichlet 100 °C at x = 0, convection (h, T∞ = 0) at x = L: the
        // exact 1D solution is a series circuit with flux per area
        // q = ΔT / (L/k + 1/h), and T(x) = q·(1/h + (L − x)/k). The
        // finite-volume solution matches it at voxel centers to machine
        // precision — the film resistance rides in series with the
        // half-cell, so h → ∞ recovers the Dirichlet limit smoothly.
        for h in [50.0, 5_000.0, 1e9] {
            let mut m = slab_model(8);
            m.domain_faces[0] = Boundary::FixedTemperature {
                temperature_c: 100.0,
            };
            m.domain_faces[1] = Boundary::Convection {
                h_w_m2k: h,
                ambient_c: 0.0,
            };
            let sol = solve_steady(&m, &SolveOptions::default()).unwrap();
            let (l, k) = (0.020, 50.0);
            let q = 100.0 / (l / k + 1.0 / h);
            let dx = l / 8.0;
            for i in 0..8 {
                let x = (i as f64 + 0.5) * dx;
                let exact = q * (1.0 / h + (l - x) / k);
                let got = sol.temperature_c(i, 0, 0);
                assert!(
                    (got - exact).abs() < 1e-8,
                    "h={h}, voxel {i}: computed {got:.9}, exact {exact:.9}"
                );
            }
        }
    }

    #[test]
    fn floating_region_fails_closed() {
        // Two solid islands; only one is pinned. The other must be
        // reported as floating, not silently solved.
        let mut m = ThermalModel::new([0.0, 0.0, 0.0], [30.0, 5.0, 5.0], [6, 1, 1]);
        for min_x in [0.0, 20.0] {
            m.materials.push(MaterialRegion::isotropic(
                Shape::Box {
                    min_mm: [min_x, 0.0, 0.0],
                    size_mm: [10.0, 5.0, 5.0],
                },
                10.0,
            ));
        }
        m.domain_faces[0] = Boundary::FixedTemperature {
            temperature_c: 50.0,
        };
        let err = solve_steady(&m, &SolveOptions::default()).unwrap_err();
        match err {
            SolveError::FloatingRegion { voxels, near_mm } => {
                assert_eq!(voxels, 2);
                assert!(near_mm[0] > 20.0, "floating island is the right one");
            }
            other => panic!("expected FloatingRegion, got {other:?}"),
        }
    }

    #[test]
    fn source_on_no_solid_fails_closed() {
        let mut m = slab_model(4);
        m.domain_faces[0] = Boundary::FixedTemperature { temperature_c: 0.0 };
        m.sources.push(PowerSource {
            name: "ghost".into(),
            shape: Shape::Box {
                min_mm: [100.0, 0.0, 0.0],
                size_mm: [1.0, 1.0, 1.0],
            },
            power_w: 1.0,
        });
        let err = solve_steady(&m, &SolveOptions::default()).unwrap_err();
        assert!(matches!(
            err,
            SolveError::Model(ModelError::SourceCoversNoFreeSolid { .. })
        ));
    }

    #[test]
    fn iteration_cap_reports_not_converged() {
        let mut m = slab_model(16);
        m.domain_faces[0] = Boundary::FixedTemperature {
            temperature_c: 100.0,
        };
        m.domain_faces[1] = Boundary::FixedTemperature { temperature_c: 0.0 };
        let err = solve_steady(
            &m,
            &SolveOptions {
                tol: 1e-12,
                max_iters: 1,
            },
        )
        .unwrap_err();
        match err {
            SolveError::NotConverged {
                residual_rel,
                iterations,
            } => {
                assert_eq!(iterations, 1);
                assert!(residual_rel > 0.0);
            }
            other => panic!("expected NotConverged, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_reference_fails_closed_only_when_needed() {
        // Two different convection ambients + a source: ambiguous.
        let mut m = slab_model(8);
        m.domain_faces[0] = Boundary::Convection {
            h_w_m2k: 10.0,
            ambient_c: 25.0,
        };
        m.domain_faces[1] = Boundary::Convection {
            h_w_m2k: 10.0,
            ambient_c: 60.0,
        };
        m.sources.push(PowerSource {
            name: "die".into(),
            shape: Shape::Box {
                min_mm: [0.0, 0.0, 0.0],
                size_mm: [20.0, 5.0, 5.0],
            },
            power_w: 1.0,
        });
        assert!(matches!(
            solve_steady(&m, &SolveOptions::default()),
            Err(SolveError::AmbiguousReference)
        ));
        // An explicit reference resolves it.
        m.reference_c = Some(25.0);
        let sol = solve_steady(&m, &SolveOptions::default()).unwrap();
        assert_eq!(sol.reference_c, Some(25.0));
        assert!(sol.sources[0].theta_c_per_w.is_some());
        // Without sources the ambiguity is irrelevant — no error.
        let mut m2 = slab_model(8);
        m2.domain_faces[0] = Boundary::Convection {
            h_w_m2k: 10.0,
            ambient_c: 25.0,
        };
        m2.domain_faces[1] = Boundary::Convection {
            h_w_m2k: 10.0,
            ambient_c: 60.0,
        };
        let sol2 = solve_steady(&m2, &SolveOptions::default()).unwrap();
        assert_eq!(sol2.reference_c, None);
    }
}
