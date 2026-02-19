//! Signal integrity analysis for PCB traces.
//!
//! Provides propagation delay calculation, crosstalk estimation between
//! parallel traces, and length-matching analysis for differential pairs
//! and bus groups.

/// Crosstalk estimation result between two parallel traces.
#[derive(Debug, Clone, PartialEq)]
pub struct CrosstalkResult {
    /// Near-end crosstalk in dB (NEXT).
    pub near_end_xtalk_db: f64,
    /// Far-end crosstalk in dB (FEXT).
    pub far_end_xtalk_db: f64,
}

/// Calculate propagation delay for a trace.
///
/// # Arguments
///
/// * `length_mm` - Trace length in mm
/// * `er_eff` - Effective dielectric constant (from impedance calculation)
///
/// # Returns
///
/// Propagation delay in picoseconds.
///
/// # Panics
///
/// Panics if `length_mm` is negative or `er_eff` is less than 1.
pub fn propagation_delay(length_mm: f64, er_eff: f64) -> f64 {
    assert!(length_mm >= 0.0, "trace length must be non-negative");
    assert!(
        er_eff >= 1.0,
        "effective dielectric constant must be >= 1.0"
    );

    // delay (ps) = length (mm) * 3.336 * sqrt(er_eff) ps/mm
    // 3.336 ps/mm is the free-space delay per mm (1/c in ps/mm).
    length_mm * 3.336 * er_eff.sqrt()
}

/// Estimate crosstalk between two parallel traces.
///
/// Uses an empirical model based on the coupling coefficient between two
/// microstrip traces. The coupling decays with the square of the ratio of
/// spacing to dielectric height.
///
/// # Arguments
///
/// * `spacing_mm` - Edge-to-edge spacing between traces in mm
/// * `parallel_length_mm` - Length of the parallel run in mm
/// * `h_mm` - Dielectric height (trace to reference plane) in mm
/// * `er` - Relative permittivity of the dielectric
///
/// # Returns
///
/// A [`CrosstalkResult`] with near-end (NEXT) and far-end (FEXT) crosstalk in dB.
///
/// # Panics
///
/// Panics if any input is non-positive.
pub fn estimate_crosstalk(
    spacing_mm: f64,
    parallel_length_mm: f64,
    h_mm: f64,
    er: f64,
) -> CrosstalkResult {
    assert!(spacing_mm > 0.0, "spacing must be positive");
    assert!(parallel_length_mm > 0.0, "parallel length must be positive");
    assert!(h_mm > 0.0, "dielectric height must be positive");
    assert!(er > 0.0, "relative permittivity must be positive");

    // Coupling coefficient (empirical approximation for microstrip).
    // Based on the model: Kb ~= 1 / (1 + (s/h)^2)
    // This captures the strong inverse-square decay of coupling with spacing.
    let s_over_h = spacing_mm / h_mm;
    let kb = 1.0 / (1.0 + s_over_h * s_over_h);

    // Length factor: longer parallel runs accumulate more coupling.
    // Saturates around 1.0 for very long runs.
    let length_factor = (parallel_length_mm / 25.0).min(1.0);

    // Dielectric factor: higher Er slightly increases coupling.
    let er_factor = (er / 4.0).sqrt();

    // Near-end crosstalk (NEXT) is proportional to coupling coefficient.
    // NEXT saturates with length and depends on backward coupling.
    let next_linear = kb * length_factor * er_factor * 0.25;
    let next_clamped = next_linear.min(0.999); // prevent log(0)
    let near_end_xtalk_db = if next_clamped > 0.0 {
        20.0 * next_clamped.log10()
    } else {
        -100.0
    };

    // Far-end crosstalk (FEXT) is typically lower than NEXT for microstrip
    // and depends on length and the imbalance between inductive and
    // capacitive coupling.
    let fext_linear = kb * kb * length_factor * er_factor * 0.10;
    let fext_clamped = fext_linear.min(0.999);
    let far_end_xtalk_db = if fext_clamped > 0.0 {
        20.0 * fext_clamped.log10()
    } else {
        -100.0
    };

    CrosstalkResult {
        near_end_xtalk_db,
        far_end_xtalk_db,
    }
}

/// Calculate length-matching deltas for a group of nets.
///
/// Given a set of trace lengths (e.g., DDR data bus), computes how much
/// each trace needs to be extended to match the longest trace.
///
/// # Arguments
///
/// * `trace_lengths` - Slice of (net name, trace length in mm) tuples
///
/// # Returns
///
/// A vector of (net name, delta in mm) where delta is the amount the trace
/// needs to grow. The longest trace will have delta = 0.
///
/// Returns an empty vector if the input is empty.
pub fn length_matching(trace_lengths: &[(String, f64)]) -> Vec<(String, f64)> {
    if trace_lengths.is_empty() {
        return Vec::new();
    }

    let max_length = trace_lengths
        .iter()
        .map(|(_, len)| *len)
        .fold(f64::NEG_INFINITY, f64::max);

    trace_lengths
        .iter()
        .map(|(name, len)| (name.clone(), max_length - len))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_propagation_delay_basic() {
        // 100mm trace in FR-4 with er_eff ~3.0
        // Expected: 100 * 3.336 * sqrt(3.0) = ~577.8 ps
        let delay = propagation_delay(100.0, 3.0);
        let expected = 100.0 * 3.336 * 3.0_f64.sqrt();
        assert!(
            (delay - expected).abs() < 0.01,
            "delay should be {expected:.2} ps, got {delay:.2} ps"
        );
    }

    #[test]
    fn test_propagation_delay_zero_length() {
        assert!((propagation_delay(0.0, 4.0)).abs() < 1e-10);
    }

    #[test]
    fn test_propagation_delay_vacuum() {
        // er_eff = 1.0 (vacuum): delay = length * 3.336 ps/mm
        let delay = propagation_delay(10.0, 1.0);
        assert!((delay - 33.36).abs() < 0.01);
    }

    #[test]
    fn test_propagation_delay_longer_trace_more_delay() {
        let short = propagation_delay(50.0, 3.5);
        let long = propagation_delay(100.0, 3.5);
        assert!(long > short);
        // Should be exactly 2x
        assert!((long / short - 2.0).abs() < 1e-10);
    }

    #[test]
    #[should_panic(expected = "trace length must be non-negative")]
    fn test_propagation_delay_negative_length() {
        propagation_delay(-1.0, 4.0);
    }

    #[test]
    #[should_panic(expected = "effective dielectric constant must be >= 1.0")]
    fn test_propagation_delay_invalid_er() {
        propagation_delay(10.0, 0.5);
    }

    #[test]
    fn test_crosstalk_close_traces() {
        // Close spacing (0.1mm) with h=0.2mm: significant coupling
        let close = estimate_crosstalk(0.1, 20.0, 0.2, 4.3);
        // Far spacing (1.0mm) with h=0.2mm: much less coupling
        let far = estimate_crosstalk(1.0, 20.0, 0.2, 4.3);

        // Close traces should have worse (higher, less negative) NEXT
        assert!(
            close.near_end_xtalk_db > far.near_end_xtalk_db,
            "closer traces should have more NEXT"
        );
        assert!(
            close.far_end_xtalk_db > far.far_end_xtalk_db,
            "closer traces should have more FEXT"
        );
    }

    #[test]
    fn test_crosstalk_values_are_negative_db() {
        // Crosstalk should always be negative in dB (attenuated)
        let result = estimate_crosstalk(0.15, 25.0, 0.15, 4.3);
        assert!(result.near_end_xtalk_db < 0.0, "NEXT should be negative dB");
        assert!(result.far_end_xtalk_db < 0.0, "FEXT should be negative dB");
    }

    #[test]
    fn test_crosstalk_next_greater_than_fext() {
        // For microstrip, NEXT is typically worse than FEXT
        let result = estimate_crosstalk(0.15, 20.0, 0.15, 4.3);
        assert!(
            result.near_end_xtalk_db > result.far_end_xtalk_db,
            "NEXT should be greater (less negative) than FEXT"
        );
    }

    #[test]
    fn test_crosstalk_longer_parallel_more_coupling() {
        let short = estimate_crosstalk(0.15, 5.0, 0.15, 4.3);
        let long = estimate_crosstalk(0.15, 25.0, 0.15, 4.3);
        assert!(
            long.near_end_xtalk_db > short.near_end_xtalk_db,
            "longer parallel run should have more NEXT"
        );
    }

    #[test]
    fn test_length_matching_basic() {
        let traces = vec![
            ("D0".to_string(), 45.0),
            ("D1".to_string(), 50.0),
            ("D2".to_string(), 47.5),
        ];
        let deltas = length_matching(&traces);

        assert_eq!(deltas.len(), 3);
        // D1 is the longest -> delta = 0
        assert_eq!(deltas[1].0, "D1");
        assert!((deltas[1].1).abs() < 1e-10);
        // D0 needs +5mm
        assert_eq!(deltas[0].0, "D0");
        assert!((deltas[0].1 - 5.0).abs() < 1e-10);
        // D2 needs +2.5mm
        assert_eq!(deltas[2].0, "D2");
        assert!((deltas[2].1 - 2.5).abs() < 1e-10);
    }

    #[test]
    fn test_length_matching_all_equal() {
        let traces = vec![("CLK+".to_string(), 30.0), ("CLK-".to_string(), 30.0)];
        let deltas = length_matching(&traces);
        for (_, delta) in &deltas {
            assert!(delta.abs() < 1e-10, "equal lengths should have zero delta");
        }
    }

    #[test]
    fn test_length_matching_empty() {
        let deltas = length_matching(&[]);
        assert!(deltas.is_empty());
    }

    #[test]
    fn test_length_matching_single() {
        let traces = vec![("NET0".to_string(), 42.0)];
        let deltas = length_matching(&traces);
        assert_eq!(deltas.len(), 1);
        assert!((deltas[0].1).abs() < 1e-10);
    }
}
