//! Axisymmetric device description: a grounded cylindrical chamber
//! containing biased (and optionally current-carrying) wire rings.
//!
//! Geometry is in **millimeters** (vcad convention); potentials in volts;
//! ring currents in ampere-turns. Everything is a body of revolution about
//! the z axis, which covers fusors, shielded-grid IEC devices, ring traps,
//! and einzel-lens-like stacks built from rings.

/// One circular wire ring electrode, coaxial with z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WireRing {
    /// Ring radius (distance from the z axis to the wire centerline), mm.
    pub ring_radius_mm: f64,
    /// Axial position of the ring plane, mm.
    pub z_mm: f64,
    /// Wire (minor) radius, mm.
    pub wire_radius_mm: f64,
    /// Electrode potential, volts.
    pub potential_v: f64,
    /// Circulating current, ampere-turns. Positive = counter-clockwise
    /// viewed from +z (right-hand rule: B along +z at the ring center).
    /// Zero for a plain electrostatic ring.
    pub ampere_turns: f64,
}

/// A complete axisymmetric device: grounded chamber + wire rings.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// Chamber (anode) inner radius, mm.
    pub chamber_radius_mm: f64,
    /// Chamber half-height: the chamber spans z ∈ [−h, +h], mm.
    pub chamber_half_height_mm: f64,
    /// Chamber wall potential, volts (normally 0 — grounded anode).
    pub wall_potential_v: f64,
    /// Wire ring electrodes.
    pub rings: Vec<WireRing>,
}

impl Device {
    /// A classic gridded fusor: a spherical cathode "globe" approximated by
    /// wire rings at evenly spaced polar angles, inside a grounded chamber.
    ///
    /// `n_rings` rings are placed at polar angles strictly between the
    /// poles, on a sphere of radius `cathode_radius_mm`, each with
    /// `wire_radius_mm` wire at `cathode_v` volts and zero current.
    pub fn classic_fusor(
        chamber_radius_mm: f64,
        cathode_radius_mm: f64,
        n_rings: usize,
        wire_radius_mm: f64,
        cathode_v: f64,
    ) -> Self {
        let mut rings = Vec::with_capacity(n_rings);
        for k in 0..n_rings {
            let theta = std::f64::consts::PI * (k as f64 + 1.0) / (n_rings as f64 + 1.0);
            rings.push(WireRing {
                ring_radius_mm: cathode_radius_mm * theta.sin(),
                z_mm: cathode_radius_mm * theta.cos(),
                wire_radius_mm,
                potential_v: cathode_v,
                ampere_turns: 0.0,
            });
        }
        Self {
            chamber_radius_mm,
            chamber_half_height_mm: chamber_radius_mm,
            wall_potential_v: 0.0,
            rings,
        }
    }

    /// The two-ring magnetically shielded cathode (spindle-cusp
    /// configuration): two coaxial rings at ±`z_mm` carrying opposed
    /// currents, both biased to `cathode_v`.
    ///
    /// With `ampere_turns = 0` this degenerates to a plain two-ring fusor
    /// cathode, which is the control case for shielding experiments.
    pub fn shielded_two_ring(
        chamber_radius_mm: f64,
        ring_radius_mm: f64,
        z_mm: f64,
        wire_radius_mm: f64,
        cathode_v: f64,
        ampere_turns: f64,
    ) -> Self {
        let ring = |z: f64, at: f64| WireRing {
            ring_radius_mm,
            z_mm: z,
            wire_radius_mm,
            potential_v: cathode_v,
            ampere_turns: at,
        };
        Self {
            chamber_radius_mm,
            chamber_half_height_mm: chamber_radius_mm,
            wall_potential_v: 0.0,
            rings: vec![ring(z_mm, ampere_turns), ring(-z_mm, -ampere_turns)],
        }
    }

    /// Deepest electrode-to-wall potential difference, volts (absolute).
    /// Sets the velocity scale for tracing.
    pub fn max_potential_drop_v(&self) -> f64 {
        self.rings
            .iter()
            .map(|r| (r.potential_v - self.wall_potential_v).abs())
            .fold(0.0, f64::max)
    }

    /// Smallest spherical radius √(r² + z²) of any ring centerline, mm.
    /// Used to size the "core" region for pass counting.
    pub fn min_ring_spherical_radius_mm(&self) -> f64 {
        self.rings
            .iter()
            .map(|r| (r.ring_radius_mm.powi(2) + r.z_mm.powi(2)).sqrt())
            .fold(f64::INFINITY, f64::min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_fusor_rings_sit_on_the_cathode_sphere() {
        let d = Device::classic_fusor(150.0, 50.0, 5, 1.0, -30_000.0);
        assert_eq!(d.rings.len(), 5);
        for ring in &d.rings {
            let s = (ring.ring_radius_mm.powi(2) + ring.z_mm.powi(2)).sqrt();
            assert!((s - 50.0).abs() < 1e-9, "ring off sphere: {s}");
            assert!(ring.ring_radius_mm > 0.0);
        }
        assert!((d.max_potential_drop_v() - 30_000.0).abs() < 1e-9);
        assert!((d.min_ring_spherical_radius_mm() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn shielded_two_ring_currents_oppose() {
        let d = Device::shielded_two_ring(150.0, 45.0, 25.0, 3.0, -2_000.0, 5_000.0);
        assert_eq!(d.rings.len(), 2);
        assert!((d.rings[0].ampere_turns + d.rings[1].ampere_turns).abs() < 1e-12);
        assert!((d.rings[0].z_mm + d.rings[1].z_mm).abs() < 1e-12);
    }
}
