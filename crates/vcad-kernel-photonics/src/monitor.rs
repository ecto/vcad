//! Running-DFT monitors: spectral flux through lines, and time probes.
//!
//! Each monitored frequency accumulates `F̂(ω) = Σₙ F(tₙ)·e^{+iωtₙ}·dt`
//! during the run (fields are sampled at their native Yee times: E at
//! integer steps, H at half steps — the half-step phase is carried by the
//! accumulator, not ignored). Time-averaged Poynting flux through a
//! monitor line is then
//!
//! ```text
//! P(ω) = ½·Σ_line Re(Ê_t · conj(Ĥ_t))·Δ · (orientation sign)
//! ```
//!
//! with the tangential E and H phasors spatially co-located by averaging
//! the two staggered H columns/rows onto the E line. For superposed
//! counter-propagating waves the cross terms cancel identically in
//! `Re(Ê·conj(Ĥ))`, which is what makes the two-run reflection
//! subtraction exact (see `tests/validation.rs::fresnel_half_space_tm`).

/// Minimal complex arithmetic for DFT accumulators (hand-rolled — the
/// crate has no dependencies).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cplx {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Cplx {
    /// 0 + 0i.
    pub const ZERO: Cplx = Cplx { re: 0.0, im: 0.0 };

    /// Construct from parts.
    pub fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// e^{iθ}.
    pub fn cis(theta: f64) -> Self {
        Self {
            re: theta.cos(),
            im: theta.sin(),
        }
    }

    /// Complex conjugate.
    pub fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    /// Scale by a real.
    pub fn scale(self, s: f64) -> Self {
        Self {
            re: self.re * s,
            im: self.im * s,
        }
    }

    /// |z|².
    pub fn abs2(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    /// Argument (radians).
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }
}

impl std::ops::Add for Cplx {
    type Output = Cplx;
    fn add(self, o: Cplx) -> Cplx {
        Cplx {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }
}

impl std::ops::Mul for Cplx {
    type Output = Cplx;
    fn mul(self, o: Cplx) -> Cplx {
        Cplx {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
}

/// Where a flux monitor lives and what it resolves.
///
/// Index semantics are polarization-dependent (Yee staggering): for a
/// vertical line at node column `i`, samples run over `j0..=j1` meaning
/// Ez nodes `y = j·Δ` in TM, and Ey samples `y = (j+½)·Δ` in TE. The
/// tangential H is averaged from the two adjacent staggered columns, so
/// `1 ≤ i ≤ nx−1` is required (mirror constraints for horizontal lines).
#[derive(Debug, Clone, PartialEq)]
pub enum FluxSpec {
    /// Line x = i·Δ, samples j0..=j1, positive flux = +x power.
    Vertical {
        /// Node column.
        i: usize,
        /// First transverse sample.
        j0: usize,
        /// Last transverse sample (inclusive).
        j1: usize,
        /// Monitored frequencies.
        freqs: Vec<f64>,
    },
    /// Line y = j·Δ, samples i0..=i1, positive flux = +y power.
    Horizontal {
        /// Node row.
        j: usize,
        /// First transverse sample.
        i0: usize,
        /// Last transverse sample (inclusive).
        i1: usize,
        /// Monitored frequencies.
        freqs: Vec<f64>,
    },
}

impl FluxSpec {
    /// The monitored frequency list.
    pub fn freqs(&self) -> &[f64] {
        match self {
            FluxSpec::Vertical { freqs, .. } => freqs,
            FluxSpec::Horizontal { freqs, .. } => freqs,
        }
    }

    /// Number of transverse samples.
    pub fn n_samples(&self) -> usize {
        match self {
            FluxSpec::Vertical { j0, j1, .. } => j1 - j0 + 1,
            FluxSpec::Horizontal { i0, i1, .. } => i1 - i0 + 1,
        }
    }
}

/// A flux monitor's DFT state (owned by the simulation).
#[derive(Debug, Clone)]
pub(crate) struct FluxState {
    pub spec: FluxSpec,
    /// Tangential-E phasors, `[freq-major][sample]`.
    pub e_acc: Vec<Cplx>,
    /// Co-located tangential-H phasors, same layout.
    pub h_acc: Vec<Cplx>,
}

impl FluxState {
    pub fn new(spec: FluxSpec) -> Self {
        let n = spec.freqs().len() * spec.n_samples();
        Self {
            spec,
            e_acc: vec![Cplx::ZERO; n],
            h_acc: vec![Cplx::ZERO; n],
        }
    }
}

/// DFT one real time series: `Σ x(tₙ)·e^{+iω tₙ}·dt` with `tₙ = t0 + n·dt`.
///
/// The same convention the monitors use; with it, a +x-propagating wave
/// has phasor phase `+k·x`, so `arg(X̂(x₂)) − arg(X̂(x₁)) = k·(x₂−x₁)`.
pub fn dft_of_series(series: &[f64], dt: f64, t0: f64, freq: f64) -> Cplx {
    let omega = 2.0 * std::f64::consts::PI * freq;
    let mut acc = Cplx::ZERO;
    for (n, &x) in series.iter().enumerate() {
        let t = t0 + n as f64 * dt;
        acc = acc + Cplx::cis(omega * t).scale(x * dt);
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dft_recovers_amplitude_and_phase_of_a_cosine() {
        // x(t) = A·cos(ωt − φ) over an integer number of periods:
        // X̂ = (A/2)·e^{−iφ}·T ... with the e^{+iωt} convention the
        // stationary term is (A/2)·e^{+iφ}? Work it out: cos(ωt − φ) =
        // ½(e^{i(ωt−φ)} + e^{−i(ωt−φ)}); times e^{iωt} the DC term is
        // ½e^{+iφ}. So arg(X̂) = +φ and |X̂| = A·T/2.
        let freq = 2.0f64;
        let phi = 0.7;
        let dt = 1e-3f64;
        let periods = 8.0f64;
        let n = (periods / freq / dt).round() as usize;
        let series: Vec<f64> = (0..n)
            .map(|k| {
                let t = k as f64 * dt;
                3.0 * (2.0 * std::f64::consts::PI * freq * t - phi).cos()
            })
            .collect();
        let x = dft_of_series(&series, dt, 0.0, freq);
        let t_total = n as f64 * dt;
        assert!((x.abs2().sqrt() - 3.0 * t_total / 2.0).abs() < 1e-2);
        assert!((x.arg() - phi).abs() < 1e-2);
    }

    #[test]
    fn cplx_algebra() {
        let a = Cplx::new(1.0, 2.0);
        let b = Cplx::new(3.0, -1.0);
        let p = a * b;
        assert_eq!(p, Cplx::new(5.0, 5.0));
        assert_eq!(a.conj().im, -2.0);
        assert!((Cplx::cis(0.5).abs2() - 1.0).abs() < 1e-15);
    }
}
