//! Planetary trains: does it assemble, does it fit, what does it reduce by.

use super::{GearError, GearPair, MeshReport, SpurGear};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// A simple planetary stage: one sun, N identical planets, one ring.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanetaryTrain {
    /// The sun (external).
    pub sun: SpurGear,
    /// The planet (external).
    pub planet: SpurGear,
    /// The ring (internal).
    pub ring: SpurGear,
    /// How many planets are fitted.
    pub planets: u32,
}

/// Both meshes of a train, plus the things only the assembled train can say.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanetaryMesh {
    /// Reduction ratio, sun in and carrier out with the ring held.
    pub ratio: f64,
    /// Common centre distance, mm.
    pub centre_distance: f64,
    /// True when the planets can be placed at equal spacing.
    pub assembles: bool,
    /// Gap between the tip circles of adjacent planets, mm.
    pub neighbour_clearance: f64,
    /// The sun–planet mesh.
    pub sun_planet: MeshReport,
    /// The planet–ring mesh.
    pub planet_ring: MeshReport,
}

impl PlanetaryTrain {
    /// A train of `planets` planets.
    pub fn new(sun: SpurGear, planet: SpurGear, ring: SpurGear, planets: u32) -> Self {
        Self {
            sun,
            planet,
            ring,
            planets,
        }
    }

    /// Reduction ratio with the ring held, sun in, carrier out: `1 + z_r/z_s`.
    pub fn ratio(&self) -> f64 {
        1.0 + self.ring.teeth as f64 / self.sun.teeth as f64
    }

    /// The sun–planet mesh.
    pub fn sun_planet(&self) -> GearPair {
        GearPair::external(self.sun, self.planet)
    }

    /// The planet–ring mesh.
    pub fn planet_ring(&self) -> GearPair {
        GearPair::internal(self.planet, self.ring)
    }

    /// Equal spacing needs `(z_sun + z_ring)` to divide by the planet count;
    /// otherwise the planets can only go in at unequal angles.
    pub fn assembly_condition(&self) -> Result<(), GearError> {
        if self.planets < 2 {
            return Err(GearError::TooFewPlanets(self.planets));
        }
        let total = self.sun.teeth + self.ring.teeth;
        if !total.is_multiple_of(self.planets) {
            return Err(GearError::AssemblyCondition {
                sun: self.sun.teeth,
                ring: self.ring.teeth,
                planets: self.planets,
                quotient: total as f64 / self.planets as f64,
            });
        }
        Ok(())
    }

    /// The centre distance both meshes agree on, mm.
    ///
    /// A planetary train only closes if the ring's profile shift puts the
    /// planet–ring mesh at the same centre distance as the sun–planet mesh;
    /// disagreement is an error with both numbers rather than an average.
    pub fn centre_distance(&self) -> Result<f64, GearError> {
        let sp = self.sun_planet().centre_distance()?;
        let pr = self.planet_ring().centre_distance()?;
        let delta = (sp - pr).abs();
        if delta > 1e-6 {
            return Err(GearError::CentreDistanceMismatch {
                sun_planet: sp,
                planet_ring: pr,
                delta,
            });
        }
        Ok(0.5 * (sp + pr))
    }

    /// Gap between the tip circles of two adjacent planets, mm. Negative means
    /// they collide before anything turns.
    pub fn neighbour_clearance(&self) -> Result<f64, GearError> {
        if self.planets < 2 {
            return Err(GearError::TooFewPlanets(self.planets));
        }
        let a = self.centre_distance()?;
        let spacing = 2.0 * a * (PI / self.planets as f64).sin();
        Ok(spacing - 2.0 * self.planet.r_tip())
    }

    /// Everything above, in one answer. Fails closed on the assembly condition
    /// and on overlapping planets.
    pub fn mesh(&self) -> Result<PlanetaryMesh, GearError> {
        let centre_distance = self.centre_distance()?;
        let assembles = self.assembly_condition().is_ok();
        let neighbour_clearance = self.neighbour_clearance()?;
        if neighbour_clearance < 0.0 {
            return Err(GearError::PlanetsOverlap {
                overlap: -neighbour_clearance,
            });
        }
        Ok(PlanetaryMesh {
            ratio: self.ratio(),
            centre_distance,
            assembles,
            neighbour_clearance,
            sun_planet: self.sun_planet().mesh_report()?,
            planet_ring: self.planet_ring().mesh_report()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{planet, ring, sun};
    use super::*;

    fn train() -> PlanetaryTrain {
        PlanetaryTrain::new(sun(), planet(), ring(), 3)
    }

    #[test]
    fn fixture_train() {
        let t = train();
        let m = t.mesh().unwrap();
        assert!((m.ratio - 6.0).abs() < 1e-12);
        assert!((m.centre_distance - 15.427907472541792).abs() < 1e-9);
        assert!(m.assembles, "(10 + 50)/3 = 20");
        // 2·a·sin(60°) − 2·10.89
        assert!(
            (m.neighbour_clearance - 4.9419196).abs() < 1e-6,
            "neighbour clearance {}",
            m.neighbour_clearance
        );
    }

    /// The assembly condition is arithmetic, and it is checked, not assumed.
    /// This train divides by 3, 4, 5 and 6 but not 7 — and dividing is not the
    /// same as fitting: five planets assemble on paper and foul in metal.
    #[test]
    fn assembly_and_fit_are_separate_questions() {
        let t = PlanetaryTrain::new(sun(), planet(), ring(), 7);
        match t.assembly_condition() {
            Err(GearError::AssemblyCondition { quotient, .. }) => {
                assert!((quotient - 60.0 / 7.0).abs() < 1e-12)
            }
            other => panic!("expected AssemblyCondition, got {other:?}"),
        }
        // Four assemble and clear — by 0.038 mm, which is the sort of margin
        // worth knowing before cutting four planets.
        let t = PlanetaryTrain::new(sun(), planet(), ring(), 4);
        assert!(t.assembly_condition().is_ok());
        let gap = t.neighbour_clearance().unwrap();
        assert!((gap - 0.0381).abs() < 1e-3, "four-planet gap {gap}");
        // Five assemble and collide.
        let t = PlanetaryTrain::new(sun(), planet(), ring(), 5);
        assert!(t.assembly_condition().is_ok());
        assert!(matches!(t.mesh(), Err(GearError::PlanetsOverlap { .. })));
    }

    /// A ring whose shift does not close the train is refused with both centre
    /// distances, not quietly split.
    #[test]
    fn mismatched_centre_distance_fails_closed() {
        let t = PlanetaryTrain::new(sun(), planet(), ring().with_profile_shift(0.2), 3);
        match t.centre_distance() {
            Err(GearError::CentreDistanceMismatch { delta, .. }) => assert!(delta > 0.1),
            other => panic!("expected CentreDistanceMismatch, got {other:?}"),
        }
    }
}
