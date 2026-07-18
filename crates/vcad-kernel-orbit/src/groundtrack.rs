//! Earth rotation (GMST), sub-satellite points, sites, and elevation.
//!
//! Stated approximations (see the crate docs): GMST-only Earth rotation
//! (no precession/nutation/polar motion), so inertial coordinates are
//! rotated to ECEF by a single z-rotation. Good to ~0.4° of longitude
//! against an ICRF-frame state — fine for ±minutes pass prediction.
//!
//! GMST polynomial: Meeus, *Astronomical Algorithms*, Eq. 12.4 /
//! Vallado Eq. 3-45 (IAU 1982 lineage), argument JD(UT1) ≈ JD(UTC).

use crate::constants::{EARTH_FLATTENING, JD_J2000, R_EARTH_KM};
use crate::state::{dot, norm};

/// Greenwich Mean Sidereal Time, radians in [0, 2π), from JD (UT1≈UTC).
pub fn gmst_rad(jd_ut1: f64) -> f64 {
    let d = jd_ut1 - JD_J2000;
    let t = d / 36_525.0;
    let deg =
        280.460_618_37 + 360.985_647_366_29 * d + 0.000_387_933 * t * t - t * t * t / 38_710_000.0;
    let mut rad = deg.to_radians() % std::f64::consts::TAU;
    if rad < 0.0 {
        rad += std::f64::consts::TAU;
    }
    rad
}

/// Rotate an inertial (equatorial) vector into Earth-fixed (ECEF) axes at
/// the given time: a z-rotation by GMST.
pub fn eci_to_ecef(r_eci: [f64; 3], jd_ut1: f64) -> [f64; 3] {
    let (s, c) = gmst_rad(jd_ut1).sin_cos();
    [
        c * r_eci[0] + s * r_eci[1],
        -s * r_eci[0] + c * r_eci[1],
        r_eci[2],
    ]
}

/// Sub-satellite point: geocentric latitude and east longitude, radians.
///
/// The latitude is **geocentric** (not geodetic); the difference peaks at
/// ~0.19° at mid-latitudes — stated, not hidden.
pub fn subpoint(r_eci: [f64; 3], jd_ut1: f64) -> (f64, f64) {
    let e = eci_to_ecef(r_eci, jd_ut1);
    let lat = (e[2] / norm(e)).asin();
    let lon = e[1].atan2(e[0]);
    (lat, lon)
}

/// A ground site in geodetic (WGS84) coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Site {
    /// Geodetic latitude, rad (north positive).
    pub lat_rad: f64,
    /// East longitude, rad.
    pub lon_rad: f64,
    /// Height above the WGS84 ellipsoid, km.
    pub alt_km: f64,
}

impl Site {
    /// Site position in ECEF, km (standard WGS84 closed form).
    pub fn ecef_km(&self) -> [f64; 3] {
        let e2 = EARTH_FLATTENING * (2.0 - EARTH_FLATTENING);
        let (sl, cl) = self.lat_rad.sin_cos();
        let n = R_EARTH_KM / (1.0 - e2 * sl * sl).sqrt();
        let (slon, clon) = self.lon_rad.sin_cos();
        [
            (n + self.alt_km) * cl * clon,
            (n + self.alt_km) * cl * slon,
            (n * (1.0 - e2) + self.alt_km) * sl,
        ]
    }

    /// Local geodetic up (zenith) unit vector in ECEF.
    pub fn up_ecef(&self) -> [f64; 3] {
        let (sl, cl) = self.lat_rad.sin_cos();
        let (slon, clon) = self.lon_rad.sin_cos();
        [cl * clon, cl * slon, sl]
    }

    /// Elevation of a satellite (inertial position `r_eci`, km) above this
    /// site's horizon at `jd_ut1`, radians. Negative below the horizon.
    pub fn elevation_rad(&self, r_eci: [f64; 3], jd_ut1: f64) -> f64 {
        let sat = eci_to_ecef(r_eci, jd_ut1);
        let s = self.ecef_km();
        let rho = [sat[0] - s[0], sat[1] - s[1], sat[2] - s[2]];
        (dot(rho, self.up_ecef()) / norm(rho))
            .clamp(-1.0, 1.0)
            .asin()
    }
}

/// Julian date from a UTC calendar date (Fliegel–Van Flandern).
pub fn jd_from_calendar(year: i32, month: u32, day: u32, ut_seconds: f64) -> f64 {
    let a = (14 - month as i32) / 12;
    let y = year + 4800 - a;
    let m = month as i32 + 12 * a - 3;
    let jdn = day as i32 + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32_045;
    jdn as f64 - 0.5 + ut_seconds / 86_400.0
}

/// Format a JD (UTC) as "YYYY-MM-DD HH:MM:SS UTC" (inverse Fliegel–Van
/// Flandern; seconds truncated).
pub fn format_jd_utc(jd: f64) -> String {
    let z = (jd + 0.5).floor() as i64;
    let frac = jd + 0.5 - z as f64;
    let a = z + 32_044;
    let b = (4 * a + 3) / 146_097;
    let c = a - 146_097 * b / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - 1461 * d / 4;
    let m = (5 * e + 2) / 153;
    let day = e - (153 * m + 2) / 5 + 1;
    let month = m + 3 - 12 * (m / 10);
    let year = 100 * b + d - 4800 + m / 10;
    let secs = (frac * 86_400.0).round() as i64;
    let (h, rem) = (secs / 3600, secs % 3600);
    format!(
        "{year:04}-{month:02}-{day:02} {h:02}:{:02}:{:02} UTC",
        rem / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmst_at_j2000_epoch_matches_the_textbook() {
        // Meeus: GMST at 2000-01-01 12:00 UT1 = 280.46062°.
        let g = gmst_rad(JD_J2000).to_degrees();
        assert!((g - 280.460_62).abs() < 1e-3, "GMST(J2000) = {g}°");
    }

    #[test]
    fn calendar_round_trip_and_known_jd() {
        // 2026-07-17 00:00 UTC → JD 2461238.5 (matches the Horizons
        // fixture header modulo the TDB offset).
        let jd = jd_from_calendar(2026, 7, 17, 0.0);
        assert!((jd - 2_461_238.5).abs() < 1e-9, "jd = {jd}");
        assert_eq!(format_jd_utc(jd), "2026-07-17 00:00:00 UTC");
    }

    #[test]
    fn site_ecef_matches_spherical_earth_at_equator() {
        let site = Site {
            lat_rad: 0.0,
            lon_rad: 0.0,
            alt_km: 0.0,
        };
        let p = site.ecef_km();
        assert!((p[0] - R_EARTH_KM).abs() < 1e-9);
        assert!(p[1].abs() < 1e-9 && p[2].abs() < 1e-9);
        assert_eq!(site.up_ecef(), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn satellite_at_zenith_reads_ninety_degrees() {
        let site = Site {
            lat_rad: 0.6,
            lon_rad: -2.0,
            alt_km: 0.0,
        };
        // Build a satellite directly above the site in ECEF, then spin it
        // into the inertial frame (inverse of eci_to_ecef).
        let jd = 2_461_238.5;
        let up = site.up_ecef();
        let s = site.ecef_km();
        let sat_ecef = [
            s[0] + 400.0 * up[0],
            s[1] + 400.0 * up[1],
            s[2] + 400.0 * up[2],
        ];
        let (sg, cg) = gmst_rad(jd).sin_cos();
        let sat_eci = [
            cg * sat_ecef[0] - sg * sat_ecef[1],
            sg * sat_ecef[0] + cg * sat_ecef[1],
            sat_ecef[2],
        ];
        let el = site.elevation_rad(sat_eci, jd);
        assert!(
            (el - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "zenith elevation = {}°",
            el.to_degrees()
        );
    }
}
