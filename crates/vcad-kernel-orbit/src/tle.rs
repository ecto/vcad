//! NORAD two-line element set (TLE) parsing.
//!
//! M0 parses the classical fields and converts mean motion → semi-major
//! axis via Kepler's third law. **Honesty:** TLE elements are
//! Brouwer/Kozai *mean* elements in the TEME frame — feeding them into an
//! osculating propagator as if they were osculating elements introduces
//! tens of kilometers of error immediately. M0 therefore uses the TLE
//! only for cross-checks (epoch, inclination, mean motion) against the
//! Horizons ephemeris; SGP4-compatible mean-element handling is M1.

use crate::constants::{MU_EARTH_KM3_S2, SECONDS_PER_DAY};
use crate::groundtrack::jd_from_calendar;
use crate::OrbitError;

/// Parsed TLE (angles in degrees as printed in the element set — the
/// only place this crate reports degrees on a non-`_deg`-suffixed field,
/// because the TLE format itself defines them so).
#[derive(Debug, Clone, PartialEq)]
pub struct Tle {
    /// Satellite name (line 0), trimmed.
    pub name: String,
    /// NORAD catalog number.
    pub norad_id: u32,
    /// Epoch, JD UTC.
    pub epoch_jd_utc: f64,
    /// Inclination, degrees.
    pub inclination_deg: f64,
    /// RAAN, degrees.
    pub raan_deg: f64,
    /// Eccentricity.
    pub eccentricity: f64,
    /// Argument of perigee, degrees.
    pub argp_deg: f64,
    /// Mean anomaly, degrees.
    pub mean_anomaly_deg: f64,
    /// Mean motion, revolutions per day.
    pub mean_motion_rev_day: f64,
}

impl Tle {
    /// Semi-major axis implied by the mean motion via Kepler's third law
    /// (no J2 mean-element correction — a ~10 km-level approximation,
    /// documented above).
    pub fn semi_major_axis_km(&self) -> f64 {
        let n_rad_s = self.mean_motion_rev_day * std::f64::consts::TAU / SECONDS_PER_DAY;
        (MU_EARTH_KM3_S2 / (n_rad_s * n_rad_s)).cbrt()
    }
}

fn field<T: std::str::FromStr>(line: &str, range: std::ops::Range<usize>) -> Result<T, OrbitError> {
    line.get(range.clone())
        .ok_or_else(|| OrbitError::Parse(format!("TLE line too short for cols {range:?}")))?
        .trim()
        .parse()
        .map_err(|_| {
            OrbitError::Parse(format!(
                "bad TLE field at cols {range:?}: {:?}",
                line.get(range.clone()).unwrap_or("")
            ))
        })
}

/// Parse a 3-line TLE (name + line 1 + line 2). Fail-closed on any
/// malformed field; the standard mod-10 checksum on both lines is
/// verified.
pub fn parse(text: &str) -> Result<Tle, OrbitError> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 3 {
        return Err(OrbitError::Parse(format!(
            "expected 3 TLE lines, got {}",
            lines.len()
        )));
    }
    let (name, l1, l2) = (lines[0].trim(), lines[1], lines[2]);
    if !l1.starts_with("1 ") || !l2.starts_with("2 ") {
        return Err(OrbitError::Parse(
            "lines 1/2 must start with '1 '/'2 '".into(),
        ));
    }
    for l in [l1, l2] {
        if l.len() < 69 {
            return Err(OrbitError::Parse(format!(
                "TLE line too short ({} chars)",
                l.len()
            )));
        }
        let sum: u32 = l[..68]
            .chars()
            .map(|c| match c {
                '0'..='9' => c as u32 - '0' as u32,
                '-' => 1,
                _ => 0,
            })
            .sum();
        let check: u32 = field(l, 68..69)?;
        if sum % 10 != check {
            return Err(OrbitError::Parse(format!(
                "TLE checksum mismatch on line {:?}",
                &l[..1]
            )));
        }
    }
    let norad_id: u32 = field(l1, 2..7)?;
    let epoch_yy: u32 = field(l1, 18..20)?;
    let epoch_day: f64 = field(l1, 20..32)?;
    let year = if epoch_yy < 57 {
        2000 + epoch_yy as i32
    } else {
        1900 + epoch_yy as i32
    };
    // Day-of-year 1.0 = Jan 1 00:00 UTC.
    let epoch_jd_utc = jd_from_calendar(year, 1, 1, 0.0) + (epoch_day - 1.0);

    let ecc_str = l2
        .get(26..33)
        .ok_or_else(|| OrbitError::Parse("line 2 too short".into()))?
        .trim();
    let eccentricity: f64 = format!("0.{ecc_str}")
        .parse()
        .map_err(|_| OrbitError::Parse(format!("bad eccentricity {ecc_str:?}")))?;

    Ok(Tle {
        name: name.to_string(),
        norad_id,
        epoch_jd_utc,
        inclination_deg: field(l2, 8..16)?,
        raan_deg: field(l2, 17..25)?,
        eccentricity,
        argp_deg: field(l2, 34..42)?,
        mean_anomaly_deg: field(l2, 43..51)?,
        mean_motion_rev_day: field(l2, 52..63)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/iss_2026-07-17.tle"
        ))
        .expect("checked-in fixture")
    }

    #[test]
    fn parses_the_checked_in_iss_tle() {
        let tle = parse(&fixture()).unwrap();
        assert_eq!(tle.norad_id, 25_544);
        assert!(tle.name.starts_with("ISS"));
        assert!((tle.inclination_deg - 51.6316).abs() < 1e-6);
        assert!((tle.eccentricity - 0.000_679_5).abs() < 1e-9);
        assert!((tle.mean_motion_rev_day - 15.490_331_31).abs() < 1e-8);
        // Epoch: 2026 day 198.57280181 = 2026-07-17 ~13:44:50 UTC.
        let jd0 = jd_from_calendar(2026, 7, 17, 0.0);
        let frac_day = tle.epoch_jd_utc - jd0;
        assert!(
            (frac_day - 0.572_801_81).abs() < 1e-8,
            "epoch fraction {frac_day}"
        );
        // Mean motion → a ≈ 6795–6800 km (ISS altitude band).
        let a = tle.semi_major_axis_km();
        assert!((6_780.0..6_810.0).contains(&a), "a = {a} km");
    }

    #[test]
    fn corrupted_checksum_fails_closed() {
        let good = fixture();
        let mut lines: Vec<String> = good.lines().map(String::from).collect();
        // Flip a digit in line 1 (not the checksum column).
        lines[1].replace_range(20..21, "9");
        assert!(parse(&lines.join("\n")).is_err());
    }

    #[test]
    fn truncated_input_fails_closed() {
        assert!(parse("ISS\n1 25544U\n2 25544").is_err());
    }
}
