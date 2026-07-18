//! The acoustic medium: sound speed and density.
//!
//! Air properties follow the standard textbook relations (Kinsler & Frey,
//! *Fundamentals of Acoustics*, 4th ed., §5): the adiabatic sound speed
//! `c = c₀·√(1 + T/273.15)` with `c₀ = 331.3 m/s`, and the ideal-gas
//! density `ρ = ρ₀·273.15/(273.15 + T)` with `ρ₀ = 1.293 kg/m³`. All SI.

/// A homogeneous acoustic medium.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Medium {
    /// Sound speed, m/s.
    pub c: f64,
    /// Density, kg/m³.
    pub rho: f64,
}

impl Medium {
    /// Air at temperature `temp_c` (°C), standard atmospheric pressure.
    ///
    /// At 20 °C this is `c ≈ 343.2 m/s`, `ρ ≈ 1.204 kg/m³`.
    pub fn air(temp_c: f64) -> Self {
        let c = 331.3 * (1.0 + temp_c / 273.15).sqrt();
        let rho = 1.293 * 273.15 / (273.15 + temp_c);
        Self { c, rho }
    }

    /// Characteristic specific acoustic impedance `ρc`, Pa·s/m
    /// (rayl). ~413 for air at 20 °C.
    #[inline]
    pub fn impedance(&self) -> f64 {
        self.rho * self.c
    }

    /// Wavenumber `k = 2πf/c` at frequency `f` (Hz), rad/m.
    #[inline]
    pub fn wavenumber(&self, f_hz: f64) -> f64 {
        std::f64::consts::TAU * f_hz / self.c
    }

    /// Frequency (Hz) for a wavenumber `k` (rad/m).
    #[inline]
    pub fn frequency(&self, k: f64) -> f64 {
        k * self.c / std::f64::consts::TAU
    }
}

impl Default for Medium {
    /// Air at 20 °C.
    fn default() -> Self {
        Self::air(20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_at_twenty_c_is_textbook() {
        let air = Medium::air(20.0);
        assert!((air.c - 343.2).abs() < 0.5, "c = {}", air.c);
        assert!((air.rho - 1.204).abs() < 0.005, "rho = {}", air.rho);
        assert!(
            (air.impedance() - 413.0).abs() < 3.0,
            "z = {}",
            air.impedance()
        );
    }

    #[test]
    fn sound_speed_rises_with_temperature() {
        assert!(Medium::air(40.0).c > Medium::air(0.0).c);
    }

    #[test]
    fn wavenumber_round_trips_with_frequency() {
        let air = Medium::air(20.0);
        let k = air.wavenumber(100.0);
        assert!((air.frequency(k) - 100.0).abs() < 1e-9);
    }
}
