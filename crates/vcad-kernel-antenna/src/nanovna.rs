//! NanoVNA ingestion: Touchstone `.s1p` one-port sweeps → measurements
//! bound to the predicted claim set.
//!
//! A ~$100 NanoVNA exports exactly this format (directly or via
//! NanoVNA-Saver), which makes it the cheapest hardware-validation loop
//! in the vcad portfolio: fab the PCB antenna through the existing gerber
//! pipeline, calibrate at the SMA plane, save the sweep, and
//! [`measurements_from_s1p`] + [`crate::receipt::compare`] renders the
//! verdicts. Parsing is fail-closed: unknown option lines, malformed
//! rows, or an empty band are errors, never guesses.
//!
//! What a one-port VNA measures — and only that — becomes a measurement:
//! S11 magnitude/frequency structure and the impedance it implies. Gain
//! and radiation efficiency stay **Unmeasured** (that is the honest
//! verdict for a reflection-only instrument, and the report says so
//! rather than passing silently).

use crate::complex::Complex;
use crate::error::AntennaError;
use crate::receipt::{ClaimSet, Measurement};

/// A parsed one-port sweep.
#[derive(Debug, Clone)]
pub struct S11Sweep {
    /// Reference impedance of the recorded S11, Ω.
    pub z0: f64,
    /// `(frequency_hz, s11)` samples in file order.
    pub samples: Vec<(f64, Complex)>,
}

/// Parse Touchstone `.s1p` text (formats RI, MA, DB; units Hz–GHz;
/// comment lines `!`). Fail-closed on anything else.
pub fn parse_s1p(text: &str) -> Result<S11Sweep, AntennaError> {
    let mut unit_scale: Option<f64> = None;
    let mut format: Option<&str> = None;
    let mut z0 = 50.0;
    let mut samples = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('!') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let toks: Vec<String> = rest.split_whitespace().map(str::to_uppercase).collect();
            // "# <unit> S <fmt> R <z0>"
            unit_scale = Some(match toks.first().map(String::as_str) {
                Some("HZ") => 1.0,
                Some("KHZ") => 1e3,
                Some("MHZ") => 1e6,
                Some("GHZ") => 1e9,
                _ => {
                    return Err(AntennaError::MeasurementFormat {
                        line: i + 1,
                        reason: "unsupported frequency unit in option line",
                    })
                }
            });
            if toks.get(1).map(String::as_str) != Some("S") {
                return Err(AntennaError::MeasurementFormat {
                    line: i + 1,
                    reason: "option line is not an S-parameter declaration",
                });
            }
            format = Some(match toks.get(2).map(String::as_str) {
                Some("RI") => "RI",
                Some("MA") => "MA",
                Some("DB") => "DB",
                _ => {
                    return Err(AntennaError::MeasurementFormat {
                        line: i + 1,
                        reason: "unsupported S-parameter format (RI/MA/DB)",
                    })
                }
            });
            if let Some(r) = toks.get(4) {
                z0 = r.parse().map_err(|_| AntennaError::MeasurementFormat {
                    line: i + 1,
                    reason: "bad reference impedance in option line",
                })?;
            }
            continue;
        }
        let (Some(scale), Some(fmt)) = (unit_scale, format) else {
            return Err(AntennaError::MeasurementFormat {
                line: i + 1,
                reason: "data before the # option line",
            });
        };
        let nums: Vec<f64> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| AntennaError::MeasurementFormat {
                line: i + 1,
                reason: "non-numeric data line",
            })?;
        if nums.len() != 3 {
            return Err(AntennaError::MeasurementFormat {
                line: i + 1,
                reason: "expected 3 columns (freq, a, b)",
            });
        }
        let f = nums[0] * scale;
        let s = match fmt {
            "RI" => Complex::new(nums[1], nums[2]),
            "MA" => Complex::expj(nums[2].to_radians()).scale(nums[1]),
            _ => Complex::expj(nums[2].to_radians()).scale(10f64.powf(nums[1] / 20.0)),
        };
        samples.push((f, s));
    }

    if samples.is_empty() {
        return Err(AntennaError::MeasurementFormat {
            line: 0,
            reason: "no data rows",
        });
    }
    Ok(S11Sweep { z0, samples })
}

/// Instrument-grade acceptance tolerances for NanoVNA-derived
/// measurements. The measurer owns these — they encode "how wrong may
/// the prediction be before the claim is Violated", combining VNA
/// accuracy with design acceptance.
#[derive(Debug, Clone, Copy)]
pub struct NanoVnaTolerances {
    /// S11 depth tolerance, dB.
    pub s11_db: f64,
    /// Frequency tolerances, relative.
    pub freq_rel: f64,
    /// Impedance tolerance, Ω.
    pub z_ohm: f64,
    /// Bandwidth tolerance, relative.
    pub bandwidth_rel: f64,
}

impl Default for NanoVnaTolerances {
    fn default() -> Self {
        NanoVnaTolerances {
            s11_db: 2.0,
            freq_rel: 0.02,
            z_ohm: 10.0,
            bandwidth_rel: 0.25,
        }
    }
}

/// Reduce a parsed sweep to measurements named after the predicted
/// claims, restricted to the claim set's own band. S11 is renormalized
/// from the instrument reference to the claim set's reference through
/// the implied impedance, so a 50 Ω NanoVNA can test claims priced
/// against any real reference.
pub fn measurements_from_s1p(
    sweep: &S11Sweep,
    claims: &ClaimSet,
    tol: &NanoVnaTolerances,
) -> Result<Vec<Measurement>, AntennaError> {
    let band = claims.provenance.band;
    let zref = claims.reference_ohm;
    // (freq, z, s11_db vs claim reference) inside the claimed band.
    let mut rows: Vec<(f64, Complex, f64)> = Vec::new();
    for &(f, s_meas) in &sweep.samples {
        if f < band.f_lo_hz || f > band.f_hi_hz {
            continue;
        }
        let one = Complex::ONE;
        let denom = one - s_meas;
        if denom.abs() < 1e-12 {
            continue; // |Γ| = 1 open — no impedance information
        }
        let z = (one + s_meas) / denom * Complex::real(sweep.z0);
        let s_ref = crate::mom::s11(z, zref);
        rows.push((f, z, crate::mom::s11_db(z, zref)));
        let _ = s_ref;
    }
    if rows.len() < 2 {
        return Err(AntennaError::MeasurementFormat {
            line: 0,
            reason: "fewer than 2 sweep points inside the claimed band",
        });
    }

    let (f_min, z_min, s_min) = rows
        .iter()
        .cloned()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .unwrap();

    let mut out = vec![
        Measurement {
            claim: "s11_db_at_band".into(),
            value: s_min,
            tolerance: tol.s11_db,
        },
        Measurement {
            claim: "s11_min_freq".into(),
            value: f_min,
            tolerance: tol.freq_rel * f_min,
        },
        Measurement {
            claim: "z_in_re".into(),
            value: z_min.re,
            tolerance: tol.z_ohm,
        },
        Measurement {
            claim: "z_in_im".into(),
            value: z_min.im,
            tolerance: tol.z_ohm,
        },
    ];

    // Resonance: Im(Z) zero crossing inside the band, linearly
    // interpolated between samples.
    let mut f_res = None;
    for w in rows.windows(2) {
        let (f0, z0i, _) = w[0];
        let (f1, z1i, _) = w[1];
        if z0i.im == 0.0 || z0i.im.signum() != z1i.im.signum() {
            let t = z0i.im / (z0i.im - z1i.im);
            f_res = Some(f0 + t * (f1 - f0));
            break;
        }
    }
    out.push(Measurement {
        claim: "resonance_in_band".into(),
        value: if f_res.is_some() { 1.0 } else { 0.0 },
        tolerance: 0.5,
    });
    if let Some(fr) = f_res {
        out.push(Measurement {
            claim: "resonant_frequency".into(),
            value: fr,
            tolerance: tol.freq_rel * fr,
        });
    }

    // −10 dB bandwidth on the measured grid (0 when never below).
    let thresh = -10.0;
    let below: Vec<&(f64, Complex, f64)> = rows.iter().filter(|r| r.2 < thresh).collect();
    let bw = match (below.first(), below.last()) {
        (Some(a), Some(b)) => b.0 - a.0,
        _ => 0.0,
    };
    let bw_pred = claims.claim("bandwidth_10db").map_or(0.0, |c| c.value);
    out.push(Measurement {
        claim: "bandwidth_10db".into(),
        value: bw,
        tolerance: (tol.bandwidth_rel * bw_pred).max(2.0 * band_step(&rows)),
    });

    Ok(out)
}

fn band_step(rows: &[(f64, Complex, f64)]) -> f64 {
    if rows.len() < 2 {
        return 0.0;
    }
    (rows.last().unwrap().0 - rows[0].0) / (rows.len() - 1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ri_and_db_formats() {
        let ri = "! NanoVNA sweep\n# MHz S RI R 50\n900 -0.4 0.2\n915 -0.1 0.05\n930 0.3 0.1\n";
        let s = parse_s1p(ri).unwrap();
        assert_eq!(s.samples.len(), 3);
        assert_eq!(s.z0, 50.0);
        assert!((s.samples[1].0 - 915e6).abs() < 1.0);
        assert!((s.samples[1].1.re + 0.1).abs() < 1e-12);

        let db = "# HZ S DB R 50\n915000000 -20.0 45.0\n916000000 -19.0 40.0\n";
        let s = parse_s1p(db).unwrap();
        let mag = s.samples[0].1.abs();
        assert!((mag - 0.1).abs() < 1e-9, "-20 dB → |Γ| = 0.1, got {mag}");
    }

    #[test]
    fn rejects_malformed_files() {
        assert!(matches!(
            parse_s1p("915000000 0.1 0.2\n"),
            Err(AntennaError::MeasurementFormat { .. })
        ));
        assert!(matches!(
            parse_s1p("# PARSEC S RI R 50\n1 2 3\n"),
            Err(AntennaError::MeasurementFormat { .. })
        ));
        assert!(matches!(
            parse_s1p("# HZ S RI R 50\n915000000 0.1\n"),
            Err(AntennaError::MeasurementFormat { .. })
        ));
        assert!(matches!(
            parse_s1p("! nothing\n"),
            Err(AntennaError::MeasurementFormat { .. })
        ));
    }
}
