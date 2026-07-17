//! Boris-pusher particle tracing through a solved device.
//!
//! Particles are integrated with the standard Boris scheme (exact energy
//! behavior in static E, exact gyration in static B), with adaptive time
//! steps: displacement per step is capped at a fraction of the grid
//! spacing, and steps subdivide automatically where the coil field is
//! strong so gyration stays resolved.
//!
//! A trace ends when the particle hits a wire ring, reaches the chamber
//! wall, or survives to its pass/time budget. Passes through the central
//! core are counted — the recirculation statistic that grid shielding is
//! supposed to improve.

use crate::device::Device;
use crate::field::FieldMap;
use crate::poisson::Solution;

/// A charged species.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Species {
    /// Rest mass, kg.
    pub mass_kg: f64,
    /// Charge, C (signed).
    pub charge_c: f64,
}

/// Deuteron (D⁺).
pub const DEUTERON: Species = Species {
    mass_kg: crate::constants::DEUTERON_MASS,
    charge_c: crate::constants::ELEMENTARY_CHARGE,
};

/// How a trace ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// Intercepted by wire ring `k` (index into `Device::rings`).
    Wire(usize),
    /// Reached the chamber wall or an end cap.
    Wall,
    /// Still alive at the pass/time/step budget (censored).
    Survived,
}

/// Result of tracing one particle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceOutcome {
    /// Terminal event.
    pub fate: Fate,
    /// Number of entries into the central core.
    pub core_passes: u32,
    /// Flight time, s.
    pub time_s: f64,
    /// Integrator steps (including substeps).
    pub steps: u64,
    /// Worst relative energy drift observed in the far-field region,
    /// normalized by the deepest potential drop. Integration-quality
    /// diagnostic; should be ≪ 1.
    pub energy_drift_rel: f64,
    /// cos θ of the launch direction (θ from +z), for post-hoc binning.
    pub launch_cos_theta: f64,
}

/// Tracing options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceOptions {
    /// Stop after this many core passes (censor as [`Fate::Survived`]).
    pub max_passes: u32,
    /// Displacement per step as a fraction of the grid spacing.
    pub step_fraction: f64,
    /// Launch shell radius as a fraction of the smaller chamber dimension.
    pub launch_shell_fraction: f64,
    /// Launch directions span cos θ ∈ [−this, +this].
    pub launch_cos_max: f64,
    /// Flight-time budget in units of `max_passes` ideal crossing times.
    pub time_budget_factor: f64,
}

impl Default for TraceOptions {
    fn default() -> Self {
        Self {
            max_passes: 40,
            step_fraction: 0.25,
            launch_shell_fraction: 0.85,
            launch_cos_max: 0.95,
            time_budget_factor: 12.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct WireHit {
    r0: f64,
    z0: f64,
    a2: f64,
}

/// Prepared tracer for one device + solution.
#[derive(Debug)]
pub struct Tracer<'a> {
    fields: &'a FieldMap<'a>,
    wires: Vec<WireHit>,
    r_wall: f64,
    z_wall: f64,
    core_radius: f64,
    shell_radius: f64,
    drop_v: f64,
    h: f64,
    opts: TraceOptions,
}

const MAX_STEPS: u64 = 20_000_000;

impl<'a> Tracer<'a> {
    /// Prepare a tracer. `solution` supplies the grid spacing (for step
    /// sizing and the wire hit radius, which matches the Dirichlet mask).
    pub fn new(
        device: &Device,
        fields: &'a FieldMap<'a>,
        solution: &Solution,
        opts: TraceOptions,
    ) -> Self {
        let h = solution.dr.min(solution.dz);
        let mask_floor = 0.75 * solution.dr.max(solution.dz);
        let wires = device
            .rings
            .iter()
            .map(|ring| {
                let a = (ring.wire_radius_mm * 1e-3).max(mask_floor);
                WireHit {
                    r0: ring.ring_radius_mm * 1e-3,
                    z0: ring.z_mm * 1e-3,
                    a2: a * a,
                }
            })
            .collect();
        let r_wall = device.chamber_radius_mm * 1e-3 - 0.6 * solution.dr;
        let z_wall = device.chamber_half_height_mm * 1e-3 - 0.6 * solution.dz;
        let min_dim = (device.chamber_radius_mm.min(device.chamber_half_height_mm)) * 1e-3;
        let shell_radius = opts.launch_shell_fraction * min_dim;
        let core_radius = 0.35 * device.min_ring_spherical_radius_mm() * 1e-3;
        let drop_v = device.max_potential_drop_v().max(1.0);
        Self {
            fields,
            wires,
            r_wall,
            z_wall,
            core_radius,
            shell_radius,
            drop_v,
            h,
            opts,
        }
    }

    /// Trace one particle from `pos` (m) with velocity `vel` (m/s).
    pub fn trace_from(&self, species: Species, pos: [f64; 3], vel: [f64; 3]) -> TraceOutcome {
        let mut p = pos;
        let mut v = vel;
        let qm = species.charge_c / species.mass_kg;
        let v_ref = (2.0 * species.charge_c.abs() * self.drop_v / species.mass_kg).sqrt();
        let dv_scale = species.charge_c.abs() * self.drop_v;
        let t_max = self.opts.time_budget_factor
            * self.opts.max_passes as f64
            * (2.0 * self.shell_radius / v_ref);

        let h0 = 0.5 * species.mass_kg * dot(v, v) + species.charge_c * self.fields.potential(p);
        let far2 = (0.5 * self.shell_radius).powi(2);

        let mut fate = Fate::Survived;
        let mut passes = 0_u32;
        let mut inside_core = sq_len(p) < self.core_radius * self.core_radius;
        let mut time = 0.0_f64;
        let mut steps = 0_u64;
        let mut drift = 0.0_f64;
        let launch_cos_theta = p[2] / sq_len(p).sqrt().max(1e-300);

        'outer: while time < t_max && steps < MAX_STEPS {
            let speed = dot(v, v).sqrt();
            let dt = self.opts.step_fraction * self.h / speed.max(0.05 * v_ref);
            let b = self.fields.b_cart(p);
            let bmag = dot(b, b).sqrt();
            let n_sub = ((qm.abs() * bmag * dt / 0.3).ceil() as u64).clamp(1, 64);
            let dth = dt / n_sub as f64;
            for _ in 0..n_sub {
                let e = self.fields.e_cart(p);
                let b = self.fields.b_cart(p);
                let half = 0.5 * qm * dth;
                let vm = add(v, scale(e, half));
                let t = scale(b, half);
                let t2 = dot(t, t);
                let vprime = add(vm, cross(vm, t));
                let s = 2.0 / (1.0 + t2);
                let vp = add(vm, scale(cross(vprime, t), s));
                v = add(vp, scale(e, half));
                p = add(p, scale(v, dth));
                time += dth;
                steps += 1;

                let r2 = p[0] * p[0] + p[1] * p[1];
                if r2 > self.r_wall * self.r_wall || p[2].abs() > self.z_wall {
                    fate = Fate::Wall;
                    break 'outer;
                }
                let r = r2.sqrt();
                for (k, w) in self.wires.iter().enumerate() {
                    let dr = r - w.r0;
                    let dz = p[2] - w.z0;
                    if dr * dr + dz * dz <= w.a2 {
                        fate = Fate::Wire(k);
                        break 'outer;
                    }
                }
                let s2 = sq_len(p);
                let now_inside = s2 < self.core_radius * self.core_radius;
                if now_inside && !inside_core {
                    passes += 1;
                    if passes >= self.opts.max_passes {
                        break 'outer;
                    }
                }
                inside_core = now_inside;
                if s2 > far2 {
                    let hnow = 0.5 * species.mass_kg * dot(v, v)
                        + species.charge_c * self.fields.potential(p);
                    let d = (hnow - h0).abs() / dv_scale;
                    if d > drift {
                        drift = d;
                    }
                }
            }
        }

        TraceOutcome {
            fate,
            core_passes: passes,
            time_s: time,
            steps,
            energy_drift_rel: drift,
            launch_cos_theta,
        }
    }

    /// Launch `n` particles at rest on the launch shell, on a deterministic
    /// grid of polar angles (uniform in cos θ), and trace each.
    pub fn launch_ensemble(&self, species: Species, n: usize) -> Vec<TraceOutcome> {
        let n = n.max(2);
        (0..n)
            .map(|k| {
                let c = -self.opts.launch_cos_max
                    + 2.0 * self.opts.launch_cos_max * k as f64 / (n - 1) as f64;
                let s = (1.0 - c * c).max(0.0).sqrt();
                let pos = [self.shell_radius * s, 0.0, self.shell_radius * c];
                self.trace_from(species, pos, [0.0, 0.0, 0.0])
            })
            .collect()
    }

    /// Core radius used for pass counting, m.
    pub fn core_radius_m(&self) -> f64 {
        self.core_radius
    }

    /// Launch shell radius, m.
    pub fn shell_radius_m(&self) -> f64 {
        self.shell_radius
    }
}

#[inline]
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn sq_len(a: [f64; 3]) -> f64 {
    dot(a, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Device;
    use crate::field::FieldMap;
    use crate::poisson::{solve, SolveOptions};

    fn trace_fusor(n_particles: usize, max_passes: u32) -> Vec<TraceOutcome> {
        let device = Device::classic_fusor(120.0, 40.0, 5, 1.0, -5_000.0);
        let sol = solve(&device, 81, 161, &SolveOptions::default()).unwrap();
        let fields = FieldMap::new(&device, &sol);
        let opts = TraceOptions {
            max_passes,
            ..TraceOptions::default()
        };
        let tracer = Tracer::new(&device, &fields, &sol, opts);
        tracer.launch_ensemble(DEUTERON, n_particles)
    }

    #[test]
    fn ions_fall_inward_and_recirculate() {
        let outcomes = trace_fusor(12, 8);
        let total_passes: u32 = outcomes.iter().map(|o| o.core_passes).sum();
        assert!(
            total_passes >= 12,
            "ions did not recirculate: {total_passes} total passes"
        );
        // Nobody should reach the wall in a well-formed fusor well.
        let walls = outcomes.iter().filter(|o| o.fate == Fate::Wall).count();
        assert!(walls <= 2, "too many wall deaths: {walls}/12");
    }

    #[test]
    fn energy_is_approximately_conserved_far_from_wires() {
        let outcomes = trace_fusor(6, 6);
        for o in &outcomes {
            assert!(
                o.energy_drift_rel < 0.08,
                "energy drift {:.3} too large (fate {:?}, passes {})",
                o.energy_drift_rel,
                o.fate,
                o.core_passes
            );
        }
    }

    #[test]
    fn wires_intercept_most_ions_in_a_classic_fusor() {
        let outcomes = trace_fusor(16, 30);
        let wires = outcomes
            .iter()
            .filter(|o| matches!(o.fate, Fate::Wire(_)))
            .count();
        assert!(
            wires * 2 >= outcomes.len(),
            "expected interception to dominate: {wires}/{}",
            outcomes.len()
        );
    }
}
