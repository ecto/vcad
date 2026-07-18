//! JPL Horizons vector-table parsing (the sky-truth fixture).
//!
//! Parses the raw text a Horizons API `EPHEM_TYPE='VECTORS'` /
//! `CSV_FORMAT='YES'` query returns — kept raw in the fixture so the full
//! provenance header (target, center, frame, EOP file, query date) is
//! checked in alongside the numbers. Data rows live between `$$SOE` and
//! `$$EOE`: `JDTDB, date, X, Y, Z, VX, VY, VZ,` in km and km/s, ICRF,
//! geocentric.
//!
//! Time scale: Horizons stamps rows in **TDB**. [`Ephemeris::jd_utc`]
//! converts with the constant offset documented in
//! [`crate::constants::TDB_MINUS_UTC_S`].

use crate::constants::{SECONDS_PER_DAY, TDB_MINUS_UTC_S};
use crate::state::StateVector;
use crate::OrbitError;

/// One ephemeris row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EphemerisPoint {
    /// Epoch, JD TDB (as stamped by Horizons).
    pub jd_tdb: f64,
    /// Geocentric ICRF state, km and km/s.
    pub state: StateVector,
}

/// A parsed Horizons vector table.
#[derive(Debug, Clone, PartialEq)]
pub struct Ephemeris {
    /// Rows in time order.
    pub points: Vec<EphemerisPoint>,
}

impl Ephemeris {
    /// Convert a row's epoch to JD UTC (constant-offset TDB→UTC, see
    /// crate docs for the stated approximation).
    pub fn jd_utc(point: &EphemerisPoint) -> f64 {
        point.jd_tdb - TDB_MINUS_UTC_S / SECONDS_PER_DAY
    }

    /// Elapsed seconds between two rows (time-scale-free: durations in
    /// TDB and UTC agree to <2 ms over any fixture-length window).
    pub fn elapsed_s(a: &EphemerisPoint, b: &EphemerisPoint) -> f64 {
        (b.jd_tdb - a.jd_tdb) * SECONDS_PER_DAY
    }
}

/// Parse Horizons vector-table text. Fail-closed: missing `$$SOE`/`$$EOE`
/// markers, malformed rows, or non-monotonic times are errors.
pub fn parse(text: &str) -> Result<Ephemeris, OrbitError> {
    let start = text
        .find("$$SOE")
        .ok_or_else(|| OrbitError::Parse("no $$SOE marker".into()))?;
    let end = text
        .find("$$EOE")
        .ok_or_else(|| OrbitError::Parse("no $$EOE marker".into()))?;
    if end < start {
        return Err(OrbitError::Parse("$$EOE before $$SOE".into()));
    }
    let mut points = Vec::new();
    for line in text[start + 5..end].lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(',').map(str::trim).collect();
        // JDTDB, calendar date, X, Y, Z, VX, VY, VZ, (trailing empty)
        if cols.len() < 8 {
            return Err(OrbitError::Parse(format!("short ephemeris row: {line:?}")));
        }
        let num = |i: usize| -> Result<f64, OrbitError> {
            cols[i]
                .parse()
                .map_err(|_| OrbitError::Parse(format!("bad number {:?} in row {line:?}", cols[i])))
        };
        let jd_tdb = num(0)?;
        if let Some(prev) = points.last() {
            let p: &EphemerisPoint = prev;
            if jd_tdb <= p.jd_tdb {
                return Err(OrbitError::Parse(format!(
                    "non-monotonic ephemeris time at JD {jd_tdb}"
                )));
            }
        }
        points.push(EphemerisPoint {
            jd_tdb,
            state: StateVector {
                r: [num(2)?, num(3)?, num(4)?],
                v: [num(5)?, num(6)?, num(7)?],
            },
        });
    }
    if points.is_empty() {
        return Err(OrbitError::Parse("ephemeris contains no rows".into()));
    }
    Ok(Ephemeris { points })
}

/// Load the checked-in ISS fixture (2026-07-17, 72 h, 5-min steps,
/// geocentric ICRF). Tests and examples read it from disk; nothing in
/// this crate touches the network.
pub fn iss_fixture() -> Result<Ephemeris, OrbitError> {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/horizons_iss_2026-07-17_72h.txt"
    ))
    .map_err(|e| OrbitError::Parse(format!("fixture read: {e}")))?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::norm;

    #[test]
    fn parses_the_checked_in_iss_ephemeris() {
        let eph = iss_fixture().unwrap();
        // 72 h at 5-minute steps, inclusive of both endpoints.
        assert_eq!(eph.points.len(), 865);
        let first = &eph.points[0];
        assert!((first.jd_tdb - 2_461_238.5).abs() < 1e-9);
        // ISS orbital radius ~6790 km, speed ~7.66 km/s.
        let r = norm(first.state.r);
        let v = norm(first.state.v);
        assert!((6_760.0..6_820.0).contains(&r), "r = {r} km");
        assert!((7.5..7.8).contains(&v), "v = {v} km/s");
        // Every row is a bound LEO state.
        for p in &eph.points {
            let el = p.state.to_elements().unwrap();
            assert!((6_700.0..6_900.0).contains(&el.a));
            assert!(el.e < 0.01);
            // Inclination here is against the ICRF equator; the TLE's 51.63°
            // is against the true-of-date equator. ~26 years of precession
            // tilt them ~0.3° apart — the frame approximation the crate docs
            // disclose, visible in the data.
            assert!((el.i.to_degrees() - 51.63).abs() < 0.5);
        }
    }

    #[test]
    fn tdb_to_utc_offset_is_applied() {
        let eph = iss_fixture().unwrap();
        let jd_utc = Ephemeris::jd_utc(&eph.points[0]);
        let diff_s = (eph.points[0].jd_tdb - jd_utc) * SECONDS_PER_DAY;
        // JD-scale f64 granularity is ~4e-5 s; assert to 1 ms.
        assert!((diff_s - 69.184).abs() < 1e-3);
    }

    #[test]
    fn missing_markers_fail_closed() {
        assert!(parse("no markers here").is_err());
        assert!(parse("$$SOE\n1,2,3\n$$EOE").is_err()); // short row
    }
}
