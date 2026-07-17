//! Predicted-performance claims for the design receipt.
//!
//! Emits a serializable claim set — S11 at band, input impedance,
//! resonance, bandwidth, gain — with full solver provenance (segment
//! count, quadrature orders, frequency grid, **kernel validity margins**)
//! in the spirit of `vcad.receipt/1`: every number carries how it was
//! produced, every caveat is spelled out, and nothing is defaulted
//! silently. The physics this model does NOT include (substrate, ohmic
//! loss, finite ground) is stated on the claims that it would move.
//!
//! These are `basis: "predicted"` claims. Binding them to a NanoVNA sweep
//! is the measurement pack's job (M6, [`compare`]) — with fail-closed
//! Holds / Violated / Unmeasured verdicts, where an unmeasured receipt
//! never passes and a Violated claim is a publishable result about the
//! model, not an embarrassment to hide.

use serde::{Deserialize, Serialize};

use crate::error::AntennaError;
use crate::farfield::{gain_dbi, radiation_efficiency};
use crate::geometry::Mesh;
use crate::mom::{find_resonance, solve_driven, sweep, SolveOptions};

/// Schema tag for this claim family.
pub const CLAIM_SCHEMA: &str = "vcad.antenna-claims/1";

/// The frequency band a claim set is priced over.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrequencyBand {
    /// Band start, Hz.
    pub f_lo_hz: f64,
    /// Band end, Hz.
    pub f_hi_hz: f64,
    /// Sweep points (≥ 2).
    pub points: usize,
}

/// How the numbers were produced, including the thin-wire validity
/// margins at the least favorable frequency in the band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolverProvenance {
    /// Wire segments.
    pub segments: usize,
    /// Current unknowns (bases).
    pub bases: usize,
    /// Outer Gauss–Legendre order in the fill.
    pub quad_outer: usize,
    /// Inner Gauss–Legendre order in the fill.
    pub quad_inner: usize,
    /// Swept band.
    pub band: FrequencyBand,
    /// Ground plane (image theory) active.
    pub ground_plane: bool,
    /// min over segments of length/(4·radius) — the thin-wire kernel
    /// gate; must be ≥ 1 or the solve would have failed closed.
    pub min_seg_len_over_4a: f64,
    /// max over segments of length/(λ/8) at the band top — the sampling
    /// gate; must be ≤ 1.
    pub max_seg_len_over_lambda8: f64,
    /// max k·a at the band top; must be ≤ 0.1.
    pub max_ka: f64,
    /// What surrounds the wires. Always "none (free-space PEC)" until the
    /// M1.5 substrate correction lands.
    pub environment: String,
}

/// One predicted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// Claim name (snake_case).
    pub name: String,
    /// Value.
    pub value: f64,
    /// Unit ("1" for dimensionless).
    pub unit: String,
    /// Claim basis — always `"predicted"` here.
    pub basis: String,
    /// Assumptions and caveats, spelled out.
    pub note: String,
}

/// The full claim set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimSet {
    /// Schema tag ([`CLAIM_SCHEMA`]).
    pub schema: String,
    /// Solver provenance and validity margins.
    pub provenance: SolverProvenance,
    /// Reference impedance for S11 claims, Ω.
    pub reference_ohm: f64,
    /// The claims.
    pub claims: Vec<Claim>,
}

impl ClaimSet {
    /// Look up a claim by name.
    pub fn claim(&self, name: &str) -> Option<&Claim> {
        self.claims.iter().find(|c| c.name == name)
    }
}

fn claim(name: &str, value: f64, unit: &str, note: &str) -> Claim {
    Claim {
        name: name.to_string(),
        value,
        unit: unit.to_string(),
        basis: "predicted".to_string(),
        note: note.to_string(),
    }
}

const SUBSTRATE_CAVEAT: &str = "free-space PEC wire model: no substrate, no ohmic loss, \
     infinite ground plane if any. For PCB antennas this is first-order — FR-4 pulls \
     resonance down by ~1/sqrt(eps_eff); the correction is the flagged M1.5 milestone";

/// Price the predicted claim set for a fed mesh over a band.
///
/// Sweeps the band, finds the best-match point, brackets the resonance if
/// `Im(Z)` crosses zero in-band, integrates the pattern at the best-match
/// frequency, and reports −10 dB bandwidth (0 with an explicit note when
/// the dip never reaches −10 dB). Fails closed on validity-gate
/// violations anywhere in the band — a claim set is never emitted for a
/// mesh outside kernel validity.
pub fn predicted_claims(
    mesh: &Mesh,
    feed_basis: usize,
    band: FrequencyBand,
    z0: f64,
    opts: &SolveOptions,
) -> Result<ClaimSet, AntennaError> {
    if band.points < 2 || band.f_hi_hz <= band.f_lo_hz || !band.f_hi_hz.is_finite() {
        return Err(AntennaError::InvalidFrequency {
            freq_hz: band.f_hi_hz,
        });
    }
    // Validate at both band edges up front (λ gates are worst at f_hi,
    // and f_lo must be a legal solve too).
    mesh.validate_for(band.f_lo_hz)?;
    mesh.validate_for(band.f_hi_hz)?;

    let freqs: Vec<f64> = (0..band.points)
        .map(|i| band.f_lo_hz + (band.f_hi_hz - band.f_lo_hz) * i as f64 / (band.points - 1) as f64)
        .collect();
    let pts = sweep(mesh, feed_basis, &freqs, z0, opts)?;

    let (i_best, best) = pts
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.s11_db.partial_cmp(&b.1.s11_db).unwrap())
        .expect("band has points");

    // Resonance: bracket a zero crossing of Im(Z) on the grid.
    let crossing = pts
        .windows(2)
        .find(|w| w[0].z_in.im == 0.0 || (w[0].z_in.im.signum() != w[1].z_in.im.signum()));
    let resonance = match crossing {
        Some(w) => Some(find_resonance(
            mesh,
            feed_basis,
            w[0].freq_hz,
            w[1].freq_hz,
            opts,
        )?),
        None => None,
    };

    // −10 dB bandwidth by linear interpolation on the grid.
    let thresh = -10.0;
    let mut bw = 0.0;
    let mut bw_note = format!("S11 never reaches {thresh} dB against {z0} Ω in the swept band");
    if best.s11_db < thresh {
        let interp = |a: &crate::mom::SweepPoint, b: &crate::mom::SweepPoint| -> f64 {
            a.freq_hz + (thresh - a.s11_db) / (b.s11_db - a.s11_db) * (b.freq_hz - a.freq_hz)
        };
        let mut lo = band.f_lo_hz;
        let mut lo_clipped = true;
        for w in pts[..=i_best].windows(2) {
            if w[0].s11_db >= thresh && w[1].s11_db < thresh {
                lo = interp(&w[0], &w[1]);
                lo_clipped = false;
            }
        }
        let mut hi = band.f_hi_hz;
        let mut hi_clipped = true;
        for w in pts[i_best..].windows(2) {
            if w[0].s11_db < thresh && w[1].s11_db >= thresh {
                hi = interp(&w[0], &w[1]);
                hi_clipped = false;
                break;
            }
        }
        bw = hi - lo;
        bw_note = format!(
            "span where S11 < {thresh} dB vs {z0} Ω{}{}",
            if lo_clipped {
                "; low edge clipped by band"
            } else {
                ""
            },
            if hi_clipped {
                "; high edge clipped by band"
            } else {
                ""
            },
        );
    }

    // Pattern at the best-match frequency: max gain over the (hemi)sphere.
    let sol_best = solve_driven(mesh, feed_basis, best.freq_hz, opts)?;
    let theta_max = if mesh.ground_plane {
        std::f64::consts::FRAC_PI_2
    } else {
        std::f64::consts::PI
    };
    let mut g_max = f64::NEG_INFINITY;
    let n_th = 37;
    let n_ph = 72;
    for i in 0..=n_th {
        // Keep a hair off the poles (exact nulls are −inf dB).
        let theta = (theta_max * i as f64 / n_th as f64).clamp(1e-3, theta_max - 1e-3);
        for j in 0..n_ph {
            let phi = std::f64::consts::TAU * j as f64 / n_ph as f64;
            g_max = g_max.max(gain_dbi(mesh, &sol_best, theta, phi));
        }
    }
    let eff = radiation_efficiency(mesh, &sol_best, 32);

    let lambda_hi = crate::constants::C0 / band.f_hi_hz;
    let k_hi = 2.0 * std::f64::consts::PI / lambda_hi;
    let min_seg_len_over_4a = mesh
        .segments
        .iter()
        .map(|s| s.len / (4.0 * s.radius))
        .fold(f64::INFINITY, f64::min);
    let max_seg_len_over_lambda8 = mesh
        .segments
        .iter()
        .map(|s| s.len / (lambda_hi / 8.0))
        .fold(0.0, f64::max);
    let max_ka = mesh
        .segments
        .iter()
        .map(|s| k_hi * s.radius)
        .fold(0.0, f64::max);

    let mut claims = vec![
        claim(
            "s11_db_at_band",
            best.s11_db,
            "dB",
            &format!("minimum |S11| over the swept band vs {z0} Ω; {SUBSTRATE_CAVEAT}"),
        ),
        claim(
            "s11_min_freq",
            best.freq_hz,
            "Hz",
            "frequency of the S11 minimum on the sweep grid",
        ),
        claim(
            "z_in_re",
            best.z_in.re,
            "ohm",
            &format!("Re(Z_in) at the S11 minimum; {SUBSTRATE_CAVEAT}"),
        ),
        claim(
            "z_in_im",
            best.z_in.im,
            "ohm",
            "Im(Z_in) at the S11 minimum",
        ),
        claim(
            "resonance_in_band",
            if resonance.is_some() { 1.0 } else { 0.0 },
            "1",
            "1 when Im(Z_in) crosses zero inside the swept band; the \
             resonant_frequency claim exists only when this is 1",
        ),
        claim("bandwidth_10db", bw, "Hz", &bw_note),
        claim(
            "gain_dbi",
            g_max,
            "dBi",
            &format!(
                "maximum gain over the {} at the S11 minimum; lossless PEC, so gain = \
                 directivity; {SUBSTRATE_CAVEAT}",
                if mesh.ground_plane {
                    "upper hemisphere"
                } else {
                    "sphere"
                }
            ),
        ),
        claim(
            "radiation_efficiency",
            eff,
            "1",
            "far-zone power / feed power — an energy-balance cross-check that must \
             read ≈ 1 for this lossless model; deviation is discretization error, \
             not physics. Real boards add copper and dielectric loss",
        ),
    ];
    if let Some(f_res) = resonance {
        claims.insert(
            5,
            claim(
                "resonant_frequency",
                f_res,
                "Hz",
                &format!("bisected Im(Z_in) = 0 in-band; {SUBSTRATE_CAVEAT}"),
            ),
        );
    }

    Ok(ClaimSet {
        schema: CLAIM_SCHEMA.to_string(),
        provenance: SolverProvenance {
            segments: mesh.segments.len(),
            bases: mesh.bases.len(),
            quad_outer: opts.quad_outer,
            quad_inner: opts.quad_inner,
            band,
            ground_plane: mesh.ground_plane,
            min_seg_len_over_4a,
            max_seg_len_over_lambda8,
            max_ka,
            environment: "none (free-space PEC)".to_string(),
        },
        reference_ohm: z0,
        claims,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::WireGrid;

    fn dipole_claims() -> ClaimSet {
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 30)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        predicted_claims(
            &mesh,
            feed,
            FrequencyBand {
                f_lo_hz: 120e6,
                f_hi_hz: 165e6,
                points: 46,
            },
            50.0,
            &SolveOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn dipole_claim_set_reads_like_the_validation_ladder() {
        let cs = dipole_claims();
        assert_eq!(cs.schema, CLAIM_SCHEMA);
        assert!(cs.claim("s11_db_at_band").unwrap().value < -14.0);
        assert_eq!(cs.claim("resonance_in_band").unwrap().value, 1.0);
        let f_res = cs.claim("resonant_frequency").unwrap().value;
        assert!((143.0e6..145.0e6).contains(&f_res));
        let r = cs.claim("z_in_re").unwrap().value;
        assert!((60.0..80.0).contains(&r));
        let g = cs.claim("gain_dbi").unwrap().value;
        assert!((2.0..2.3).contains(&g));
        assert!(cs.claim("bandwidth_10db").unwrap().value > 5e6);
        let eff = cs.claim("radiation_efficiency").unwrap().value;
        assert!((0.99..1.01).contains(&eff));
        // Validity margins made it into provenance, inside the gates.
        assert!(cs.provenance.min_seg_len_over_4a >= 1.0);
        assert!(cs.provenance.max_seg_len_over_lambda8 <= 1.0);
        assert!(cs.provenance.max_ka <= 0.1);
        // Caveats are spelled out where the missing physics would bite.
        assert!(cs.claim("s11_db_at_band").unwrap().note.contains("M1.5"));
    }

    #[test]
    fn off_resonance_band_says_so_instead_of_defaulting() {
        // A band far below resonance: no zero crossing, shallow S11 —
        // the claim set must state both, not omit or invent.
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 30)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        let cs = predicted_claims(
            &mesh,
            feed,
            FrequencyBand {
                f_lo_hz: 60e6,
                f_hi_hz: 90e6,
                points: 16,
            },
            50.0,
            &SolveOptions::default(),
        )
        .unwrap();
        assert_eq!(cs.claim("resonance_in_band").unwrap().value, 0.0);
        assert!(cs.claim("resonant_frequency").is_none());
        let bwc = cs.claim("bandwidth_10db").unwrap();
        assert_eq!(bwc.value, 0.0);
        assert!(bwc.note.contains("never reaches"));
    }

    #[test]
    fn claim_set_round_trips_through_json() {
        let cs = dipole_claims();
        let json = serde_json::to_string(&cs).unwrap();
        let back: ClaimSet = serde_json::from_str(&json).unwrap();
        assert_eq!(cs, back);
    }

    #[test]
    fn out_of_validity_band_fails_closed() {
        let mut g = WireGrid::new();
        g.add_wire([0.0, 0.0, -500.0], [0.0, 0.0, 500.0], 1.0, 30)
            .unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        // 3 GHz: 33 mm segments vs λ/8 = 12.5 mm → gate fires, no claims.
        let res = predicted_claims(
            &mesh,
            feed,
            FrequencyBand {
                f_lo_hz: 2.9e9,
                f_hi_hz: 3.1e9,
                points: 5,
            },
            50.0,
            &SolveOptions::default(),
        );
        assert!(matches!(res, Err(AntennaError::SegmentTooLong { .. })));
    }
}
