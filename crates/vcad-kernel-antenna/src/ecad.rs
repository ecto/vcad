//! The ecad seam: PCB copper as thin wires.
//!
//! A flat trace of width `w` is electromagnetically equivalent (outside
//! its immediate vicinity) to a round wire of radius `w/4` — the
//! conformal-mapping result for a zero-thickness strip (Balanis, *Antenna
//! Theory*, equivalent-radius table). PCB copper thickness (35 µm for
//! 1 oz) is a few percent of typical trace widths and is neglected; it is
//! far below the substrate effect, which is the honest headline:
//!
//! **A PCB antenna prediction from this adapter is first-order only.**
//! FR-4 under a trace pulls the resonant frequency down by roughly
//! `1/√ε_eff` (tens of percent), and no free-space wire model sees that.
//! The quasi-static ε_eff correction is the flagged M1.5 milestone; until
//! it lands, use these predictions for trends and pre-tuning, and expect
//! the fabricated board to resonate *below* the free-space prediction.
//! The M4 receipt claims carry this caveat on every number.
//!
//! Board-side extraction (trace centerlines from a `.vcad` PCB document)
//! lands on the vcad side of the seam, emitting [`crate::spec`] elements
//! with radii from [`strip_equivalent_radius_mm`].

use crate::error::AntennaError;
use crate::geometry::WireGrid;

/// Equivalent round-wire radius of a flat strip of width `w`: `w/4`
/// (zero-thickness conformal-mapping equivalence).
pub fn strip_equivalent_radius_mm(width_mm: f64) -> f64 {
    0.25 * width_mm
}

/// Add a PCB trace (centerline polyline, mm) to a wire grid as an
/// equivalent round wire. Thin-wire validity gates apply at solve time:
/// wide traces with short segments will fail closed like any other wire.
pub fn add_trace_as_wire(
    grid: &mut WireGrid,
    centerline_mm: &[[f64; 3]],
    width_mm: f64,
    segments_per_leg: &[usize],
) -> Result<(), AntennaError> {
    grid.add_path(
        centerline_mm,
        strip_equivalent_radius_mm(width_mm),
        segments_per_leg,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Mesh;
    use crate::mom::{find_resonance, SolveOptions};

    #[test]
    fn strip_rule_is_a_quarter_width() {
        assert_eq!(strip_equivalent_radius_mm(2.0), 0.5);
    }

    #[test]
    fn trace_monopole_resonates_like_its_equivalent_wire() {
        // A 78 mm straight trace, 1.6 mm wide, base-fed over ground —
        // the 915 MHz PCB monopole shape (free-space model: expect
        // resonance near c/(4ℓ)·0.96; the fabricated FR-4 board will sit
        // lower — that is the documented M1.5 gap, not a bug).
        let mut g = WireGrid::new();
        g.set_ground_plane(true);
        add_trace_as_wire(&mut g, &[[0.0, 0.0, 0.0], [0.0, 0.0, 78.0]], 1.6, &[12]).unwrap();
        let mesh = Mesh::build(&g).unwrap();
        let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();
        let f_quarter = crate::constants::C0 / (4.0 * 0.078);
        let f_res = find_resonance(
            &mesh,
            feed,
            0.85 * f_quarter,
            1.02 * f_quarter,
            &SolveOptions::default(),
        )
        .unwrap();
        let l_over_lambda = 0.078 * f_res / crate::constants::C0;
        assert!(
            (0.23..=0.25).contains(&l_over_lambda),
            "trace monopole ℓ/λ = {l_over_lambda:.4}"
        );
    }
}
