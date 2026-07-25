//! Assembling coils, magnets and iron into a machine, and extracting the
//! constants a motor is actually specified by.
//!
//! # The two routes to torque, and why both are computed
//!
//! Torque can be had from the **force** side — integrate `I dl × B` over the
//! conductors and take the z moment — or from the **energy** side — differentiate
//! flux linkage `λ = ∮A·dl` with respect to rotor angle. They share no code path:
//! one integrates `B`, the other `A`, from separately derived kernels.
//!
//! For a linear magnetostatic problem they must agree exactly. [`Machine::audit`]
//! computes both and reports the disagreement, so a wrong answer shows up as two
//! numbers that differ rather than one number that looks plausible. This is the
//! whole point of building an oracle: a single self-consistent formula can be
//! confidently wrong, two independent ones cannot be wrong the same way.
//!
//! # Sign and units
//!
//! Angles are **radians**, mechanical (not electrical). Torque is N·m about +z,
//! flux linkage is webers, `Kt` is N·m/A and `Ke` is V/(rad/s) — numerically equal
//! in SI for a machine with no reluctance torque, which is the identity
//! [`Machine::audit`] leans on.

use crate::filament::Filament;
use crate::iron::IronStack;
use crate::magnet::MagnetRing;
use crate::vec3::Vec3;

use std::f64::consts::PI;

/// One stator phase, described at a **1 A reference current**.
///
/// Scaling to a real current is linear, so the constants below are current-free.
#[derive(Debug, Clone, PartialEq)]
pub struct Phase {
    /// Phase label, e.g. `"A"`.
    pub name: String,
    /// Conductors carrying the reference current, series-connected.
    pub turns: Vec<Filament>,
}

impl Phase {
    /// A phase from its conductors, normalized to 1 A.
    pub fn new(name: impl Into<String>, turns: Vec<Filament>) -> Self {
        Self {
            name: name.into(),
            turns,
        }
    }

    /// Total conductor length, m — the resistance and copper-loss driver.
    pub fn length_m(&self) -> f64 {
        self.turns.iter().map(|t| t.length_m()).sum()
    }
}

/// A complete machine: stator phases, a rotor, and the iron that closes the
/// circuit.
#[derive(Debug, Clone)]
pub struct Machine {
    /// Stator phases at 1 A reference.
    pub phases: Vec<Phase>,
    /// The rotor at its zero position.
    pub rotor: MagnetRing,
    /// Back-iron. Use [`IronStack::none`] for a genuinely coreless machine.
    pub iron: IronStack,
    /// Axial slices per magnet in the bound-current model.
    pub magnet_slices: usize,
}

/// Torque from both routes, plus their disagreement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TorqueAudit {
    /// From `I dl × B` — the force route.
    pub lorentz_nm: f64,
    /// From `dλ/dθ` — the energy route.
    pub energy_nm: f64,
}

impl TorqueAudit {
    /// Relative disagreement between the two routes.
    ///
    /// Should sit at the differencing noise floor. A large value means one of the
    /// two integrals is wrong, and the result must not be quoted.
    pub fn residual(&self) -> f64 {
        let scale = self.lorentz_nm.abs().max(self.energy_nm.abs());
        if scale <= 0.0 {
            return 0.0;
        }
        (self.lorentz_nm - self.energy_nm).abs() / scale
    }
}

impl Machine {
    /// The rotor's bound currents at mechanical angle `theta`, already reflected
    /// through the iron.
    pub fn rotor_sources(&self, theta: f64) -> Vec<Filament> {
        let raw = self.rotor.rotated_z(theta).to_filaments(self.magnet_slices);
        if self.iron.is_empty() {
            return raw;
        }
        raw.iter().flat_map(|f| self.iron.expand(f)).collect()
    }

    /// Flux linkage of phase `idx` at rotor angle `theta`, webers.
    ///
    /// `λ = Σ_turns s·∮A·dl` with `A` from the rotor. Only the rotor contributes:
    /// the phase's own field is self-inductance, which carries no rotor-angle
    /// dependence and so no torque.
    ///
    /// The weight `s = current_a / 1 A` is **not** optional. `∮A·dl` is a purely
    /// geometric quantity — it does not know which way the turn is wound, because
    /// the turn's own current never enters the integral. A phase whose coils
    /// alternate winding sense (the normal arrangement, so that coils facing
    /// opposite rotor poles add rather than cancel) would otherwise sum to
    /// exactly zero: each backward-wound coil sees `−λ` from the south pole it
    /// faces, and without the sign it subtracts instead of adding. Encoding the
    /// series orientation in the reference current and applying it here keeps the
    /// force and energy routes consistent, since `lorentz_torque_z` uses the same
    /// signed current.
    pub fn flux_linkage(&self, idx: usize, theta: f64) -> f64 {
        let src = self.rotor_sources(theta);
        let a = |p: Vec3| src.iter().map(|f| f.a_at(p)).sum::<Vec3>();
        self.phases[idx]
            .turns
            .iter()
            .map(|t| t.current_a * t.flux_linkage(a))
            .sum()
    }

    /// Torque about +z from the Lorentz force, N·m, with `currents[i]` amperes
    /// in phase `i`.
    ///
    /// Reported **on the rotor**, matching [`Machine::torque_energy`] and the
    /// usual motor convention. The integral itself runs over the *stator*
    /// conductors — they are the ones whose current is known — so it yields the
    /// torque on the stator, and the negation converts it to the reaction on the
    /// rotor. For two closed current distributions the pair is exactly equal and
    /// opposite, which is not an assumption here but a measured result: before
    /// this sign was applied the two routes matched to eight significant figures
    /// with opposite signs.
    pub fn torque_lorentz(&self, currents: &[f64], theta: f64) -> f64 {
        let src = self.rotor_sources(theta);
        let b = |p: Vec3| src.iter().map(|f| f.b_at(p)).sum::<Vec3>();
        let on_stator: f64 = self
            .phases
            .iter()
            .zip(currents)
            .map(|(ph, &i)| i * ph.turns.iter().map(|t| t.lorentz_torque_z(b)).sum::<f64>())
            .sum();
        -on_stator
    }

    /// Torque about +z from `dλ/dθ`, N·m — the energy route.
    ///
    /// Central difference on `h`; the default in [`Machine::audit`] is sized to
    /// balance truncation against cancellation.
    pub fn torque_energy(&self, currents: &[f64], theta: f64, h: f64) -> f64 {
        currents
            .iter()
            .enumerate()
            .map(|(i, &cur)| {
                let up = self.flux_linkage(i, theta + h);
                let dn = self.flux_linkage(i, theta - h);
                cur * (up - dn) / (2.0 * h)
            })
            .sum()
    }

    /// Both torque routes at one operating point.
    pub fn audit(&self, currents: &[f64], theta: f64) -> TorqueAudit {
        TorqueAudit {
            lorentz_nm: self.torque_lorentz(currents, theta),
            energy_nm: self.torque_energy(currents, theta, 1e-4),
        }
    }

    /// Back-EMF constant of phase `idx` at `theta`, V/(rad/s) — equivalently the
    /// per-ampere torque contribution of that phase, N·m/A.
    pub fn ke_at(&self, idx: usize, theta: f64) -> f64 {
        let h = 1e-4;
        (self.flux_linkage(idx, theta + h) - self.flux_linkage(idx, theta - h)) / (2.0 * h)
    }

    /// Flux linkage of every phase sampled over one full mechanical revolution.
    ///
    /// Returns `[phase][sample]`, `samples` points on `[0, 2π)`.
    pub fn linkage_sweep(&self, samples: usize) -> Vec<Vec<f64>> {
        let samples = samples.max(4);
        (0..self.phases.len())
            .map(|i| {
                (0..samples)
                    .map(|k| {
                        let th = 2.0 * PI * (k as f64) / (samples as f64);
                        self.flux_linkage(i, th)
                    })
                    .collect()
            })
            .collect()
    }

    /// Peak `Kt` over a sweep, N·m/A, driving phase `idx` alone at 1 A.
    ///
    /// This is the honest single-phase peak. A real drive commutating three
    /// phases sees a different — generally flatter — effective constant; do not
    /// compare this directly against a datasheet `Kt` without matching the
    /// excitation.
    pub fn kt_peak_single_phase(&self, idx: usize, samples: usize) -> f64 {
        let samples = samples.max(4);
        (0..samples)
            .map(|k| {
                let th = 2.0 * PI * (k as f64) / (samples as f64);
                self.ke_at(idx, th).abs()
            })
            .fold(0.0, f64::max)
    }
}

/// Discrete Fourier amplitudes of a periodic signal.
///
/// `harmonics(x, n)[k]` is the amplitude of the `k`-th harmonic over the period
/// the samples span. Index 0 is the mean. Used for cogging and ripple, where the
/// *spectrum* is the specification — a peak-to-peak number hides which order is
/// responsible, and therefore which geometry change would fix it.
pub fn harmonics(samples: &[f64], n_harmonics: usize) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return vec![0.0; n_harmonics];
    }
    (0..n_harmonics)
        .map(|k| {
            if k == 0 {
                return samples.iter().sum::<f64>() / n as f64;
            }
            let (mut re, mut im) = (0.0, 0.0);
            for (j, &v) in samples.iter().enumerate() {
                let a = 2.0 * PI * (k as f64) * (j as f64) / (n as f64);
                re += v * a.cos();
                im += v * a.sin();
            }
            2.0 * (re * re + im * im).sqrt() / n as f64
        })
        .collect()
}

/// Peak-to-peak of a sampled signal.
pub fn peak_to_peak(samples: &[f64]) -> f64 {
    let mx = samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mn = samples.iter().cloned().fold(f64::INFINITY, f64::min);
    if mx.is_finite() && mn.is_finite() {
        mx - mn
    } else {
        0.0
    }
}
