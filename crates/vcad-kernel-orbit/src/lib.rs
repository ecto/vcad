#![warn(missing_docs)]

//! Astrodynamics for the vcad kernel (M0).
//!
//! Orbit propagation, figures of merit, and pass prediction with the
//! receipt discipline of the other solver crates — and one thing none of
//! them have: **the bench is the sky**. Public ephemeris data (TLEs,
//! JPL Horizons) is a free, continuously-updating measurement stream, so
//! the predicted-vs-measured loop closes with zero hardware. The checked-in
//! ISS fixture (`tests/fixtures/`) is real sky data and the
//! `position_error_km` claim it feeds is the first claim family in the
//! workspace born with a `basis: measured` entry.
//!
//! The pipeline:
//!
//! 1. [`state`] — osculating orbital elements ↔ Cartesian state vectors
//!    (Vallado, *Fundamentals of Astrodynamics and Applications*, 4th ed.,
//!    Algorithms 9/10).
//! 2. [`kepler`] — exact analytic two-body propagation via the elliptic
//!    Kepler equation (Newton, fail-closed on non-convergence). This is
//!    the oracle the numeric integrator is validated against.
//! 3. [`propagate`] — fixed-step RK4 on Cartesian state with the J2
//!    (Earth oblateness) acceleration (Vallado Eq. 8-30), plus energy and
//!    angular-momentum conservation diagnostics.
//! 4. [`secular`] — closed-form J2 secular rates: nodal regression dΩ/dt
//!    and apsidal rotation dω/dt (Vallado Eqs. 9-38/9-39), and
//!    sun-synchronous inclination by root-finding on them.
//! 5. [`groundtrack`] / [`pass`] — GMST Earth rotation, sub-satellite
//!    point, topocentric elevation, and rise/set pass prediction over a
//!    geodetic site.
//! 6. [`tle`] / [`ephemeris`] — parse the checked-in fixtures: a NORAD
//!    two-line element set and a JPL Horizons vector table. Tests never
//!    touch the network.
//! 7. [`receipt`] — `vcad.orbit-claims/1`: predicted claims (period,
//!    secular rates, pass windows) and the sky-measured position-error
//!    claim, fail-closed in the `vcad.receipt/1` vocabulary.
//!
//! # Units — read this before calling anything
//!
//! Orbital mechanics' classic bug is unit confusion, so the contract is
//! loud and uniform: **kilometers, kilometers/second, seconds, radians**
//! everywhere in this crate, `f64` throughout. (The rest of vcad models
//! parts in millimeters; orbits are not parts. Conversion happens at
//! whatever seam eventually co-locates a spacecraft part with its orbit.)
//! Degrees appear only in clearly-suffixed fields (`*_deg`,
//! `*_deg_per_day`) on reporting types. Times are Julian Dates (`jd_*`)
//! with the time scale in the name.
//!
//! # Frames and time scales — stated approximations (M0)
//!
//! - The inertial frame is treated as an Earth-centered frame whose
//!   equator is the Earth's equator. The Horizons fixture is ICRF; the
//!   J2000/ICRF mean equator differs from the true-of-date equator by
//!   precession/nutation (≈0.36° between 2000 and 2026). We ignore that:
//!   it perturbs the J2 torque direction by ~0.4% and ground-track
//!   longitudes by up to ~0.4° (~44 km) — inside M0's honesty budget and
//!   stated on every receipt.
//! - Earth rotation is GMST-only (IAU 1982-style polynomial); no
//!   polar motion, no equation of the equinoxes (≲0.005° effect).
//! - The Horizons fixture is stamped in TDB. We convert with
//!   TDB ≈ TT = UTC + 69.184 s (37 leap seconds + 32.184 s, valid 2017–
//!   at least 2026); the periodic TDB−TT term (≤1.7 ms) and UT1−UTC
//!   (≤0.9 s) are ignored. 0.9 s of Earth rotation is 0.0038° of
//!   longitude — irrelevant at ±minutes pass accuracy.
//! - Force model is two-body + J2 **only**: no drag, no SRP, no
//!   third-body, no higher harmonics. Against the real ISS the model gap
//!   (drag above all) grows visibly with time — the flagship example
//!   `iss_pass` measures that gap against the sky instead of hiding it.
//!   M1 adds drag and an SGP4-compatibility mode (see `docs/orbit-m0.md`).

pub mod ephemeris;
pub mod kepler;
pub mod pass;
pub mod propagate;
pub mod receipt;
pub mod secular;
pub mod state;
pub mod tle;

pub mod groundtrack;

/// Physical and astronomical constants. Sources: WGS84 and Vallado 4th ed.
pub mod constants {
    /// Earth gravitational parameter μ⊕, km³/s² (WGS84 / Vallado).
    pub const MU_EARTH_KM3_S2: f64 = 398_600.441_8;
    /// Earth equatorial radius, km (WGS84).
    pub const R_EARTH_KM: f64 = 6378.137;
    /// Earth flattening (WGS84).
    pub const EARTH_FLATTENING: f64 = 1.0 / 298.257_223_563;
    /// Earth second zonal harmonic J2 (Vallado).
    pub const J2_EARTH: f64 = 1.082_626_68e-3;
    /// Earth rotation rate, rad/s (WGS84).
    pub const OMEGA_EARTH_RAD_S: f64 = 7.292_115_855_3e-5;
    /// Seconds per day.
    pub const SECONDS_PER_DAY: f64 = 86_400.0;
    /// Julian date of the J2000.0 epoch (2000-01-01 12:00 TT).
    pub const JD_J2000: f64 = 2_451_545.0;
    /// TDB − UTC offset, seconds, valid 2017 through at least 2026
    /// (37 leap seconds + 32.184 s TT−TAI; periodic TDB−TT ≤ 1.7 ms
    /// ignored).
    pub const TDB_MINUS_UTC_S: f64 = 69.184;
    /// Mean tropical-year length, days — sets the sun-synchronous nodal
    /// rate of 360°/year.
    pub const TROPICAL_YEAR_DAYS: f64 = 365.242_189_7;
}

/// Errors from orbit computations. Fail-closed: no silent defaults.
#[derive(Debug, Clone, PartialEq)]
pub enum OrbitError {
    /// Kepler-equation Newton iteration failed to converge.
    KeplerNoConvergence {
        /// Mean anomaly requested, rad.
        mean_anomaly: f64,
        /// Eccentricity.
        eccentricity: f64,
    },
    /// Orbit is not elliptic (e ≥ 1 or a ≤ 0); M0 handles bound orbits only.
    NotElliptic {
        /// Semi-major axis, km.
        a: f64,
        /// Eccentricity.
        e: f64,
    },
    /// A fixture or input failed to parse.
    Parse(String),
    /// An invariant a caller relied on does not hold.
    Invalid(String),
}

impl std::fmt::Display for OrbitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrbitError::KeplerNoConvergence {
                mean_anomaly,
                eccentricity,
            } => write!(
                f,
                "Kepler equation did not converge (M = {mean_anomaly}, e = {eccentricity})"
            ),
            OrbitError::NotElliptic { a, e } => {
                write!(f, "orbit not elliptic (a = {a} km, e = {e})")
            }
            OrbitError::Parse(s) => write!(f, "parse error: {s}"),
            OrbitError::Invalid(s) => write!(f, "invalid input: {s}"),
        }
    }
}

impl std::error::Error for OrbitError {}
