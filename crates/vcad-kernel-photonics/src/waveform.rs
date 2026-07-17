//! Source time signatures.

/// A scalar drive signal s(t) for soft sources.
///
/// Absolute amplitude is arbitrary units (see the crate docs); every
/// shipped quantity is a spectral ratio, so the waveform's spectrum
/// cancels between monitors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Waveform {
    /// Gaussian-modulated sinusoid
    /// `sin(ω₀(t − t0))·exp(−(t − t0)²/(2σ²))`, σ = 1/(2π·fwidth),
    /// **hard-gated to exactly zero** for |t − t0| > cutoff·σ (so the
    /// source has a definite end and post-source invariants hold exactly;
    /// the gate discontinuity is ≤ e^(−cutoff²/2) of peak).
    Gaussian {
        /// Center frequency f₀ (= 1/λ₀ in normalized units).
        freq: f64,
        /// Gaussian width in frequency (σ_f); bandwidth of the pulse.
        fwidth: f64,
        /// Envelope peak time.
        t0: f64,
        /// Gate half-width in units of σ (6 is the constructor default).
        cutoff: f64,
    },
    /// Continuous wave `sin(ω₀t)` ramped on over `ramp` time units with a
    /// raised-cosine turn-on (avoids the broadband step transient).
    Continuous {
        /// Frequency.
        freq: f64,
        /// Turn-on duration; 0 = hard start.
        ramp: f64,
    },
}

impl Waveform {
    /// Gaussian pulse with the default gate (`cutoff = 6`) and the peak
    /// placed at `t0 = cutoff·σ` so the signal starts (essentially) at 0.
    pub fn gaussian(freq: f64, fwidth: f64) -> Self {
        assert!(freq > 0.0 && fwidth > 0.0);
        let sigma = 1.0 / (2.0 * std::f64::consts::PI * fwidth);
        let cutoff = 6.0;
        Waveform::Gaussian {
            freq,
            fwidth,
            t0: cutoff * sigma,
            cutoff,
        }
    }

    /// Ramped continuous wave.
    pub fn continuous(freq: f64, ramp: f64) -> Self {
        assert!(freq > 0.0 && ramp >= 0.0);
        Waveform::Continuous { freq, ramp }
    }

    /// Evaluate s(t).
    pub fn eval(&self, t: f64) -> f64 {
        match *self {
            Waveform::Gaussian {
                freq,
                fwidth,
                t0,
                cutoff,
            } => {
                let sigma = 1.0 / (2.0 * std::f64::consts::PI * fwidth);
                let u = t - t0;
                if u.abs() > cutoff * sigma {
                    return 0.0;
                }
                let w0 = 2.0 * std::f64::consts::PI * freq;
                (w0 * u).sin() * (-u * u / (2.0 * sigma * sigma)).exp()
            }
            Waveform::Continuous { freq, ramp } => {
                if t < 0.0 {
                    return 0.0;
                }
                let w0 = 2.0 * std::f64::consts::PI * freq;
                let env = if t >= ramp || ramp == 0.0 {
                    1.0
                } else {
                    0.5 * (1.0 - (std::f64::consts::PI * t / ramp).cos())
                };
                env * (w0 * t).sin()
            }
        }
    }

    /// Time after which a gated Gaussian is identically zero (`f64::MAX`
    /// for continuous waves).
    pub fn end_time(&self) -> f64 {
        match *self {
            Waveform::Gaussian {
                fwidth, t0, cutoff, ..
            } => {
                let sigma = 1.0 / (2.0 * std::f64::consts::PI * fwidth);
                t0 + cutoff * sigma
            }
            Waveform::Continuous { .. } => f64::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_gates_to_exact_zero() {
        let w = Waveform::gaussian(1.0, 0.2);
        assert_eq!(w.eval(-1.0), 0.0);
        assert_eq!(w.eval(w.end_time() + 1e-9), 0.0);
        // Near the peak it is alive.
        let sigma = 1.0 / (2.0 * std::f64::consts::PI * 0.2);
        let mut peak: f64 = 0.0;
        let mut t = 6.0 * sigma - sigma;
        while t < 6.0 * sigma + sigma {
            peak = peak.max(w.eval(t).abs());
            t += 0.01;
        }
        assert!(peak > 0.5);
    }

    #[test]
    fn continuous_ramps_smoothly() {
        let w = Waveform::continuous(1.0, 5.0);
        assert_eq!(w.eval(-0.1), 0.0);
        assert!(w.eval(0.05).abs() < 0.01);
        // Fully on after the ramp: envelope 1.
        let v = w.eval(7.25); // sin(2π·7.25) = sin(π/2) = 1
        assert!((v - 1.0).abs() < 1e-12);
    }
}
