//! IPC-7351 land-pattern fillet goals and the pad-sizing equations that turn a
//! terminal + density target into a copper land.
//!
//! The goal constants below are engineering defaults that approximate the
//! IPC-7351B producibility levels for each lead family. They are intentionally
//! the *only* place magic numbers live, so a footprint is `terminal + goals`
//! rather than a hand-tuned lookup table. Matching specific KiCad `.kicad_mod`
//! land patterns to ±0.05 mm (the regression gate before this generator
//! *replaces* the legacy tables) is layered on top in the P1 wiring phase; the
//! relationships these equations satisfy (pad length = terminal + toe + heel,
//! pitch preserved, courtyard = max(body, land) + excess) are exact and tested.

use vcad_ir::ecad::{DensityLevel, IpcGoals, PackageFamily};

/// IPC fillet goals for a given lead family at a given density level.
///
/// `toe` extends the land outward (away from the body), `heel` inward, `side`
/// adjusts the land width relative to the terminal (negative at fine pitch to
/// preserve clearance), and `courtyard_excess` is added beyond the larger of
/// the body and land extents.
pub fn goals(family: PackageFamily, density: DensityLevel) -> IpcGoals {
    use PackageFamily::*;
    match family {
        // No-lead (QFN/DFN/SON): small toe, ~zero heel, slightly negative side
        // at fine pitch. Courtyard hugs the package.
        NoLead => scaled(density, 0.30, 0.00, -0.02, 0.40, 0.20, 0.12),
        // Gull-wing (SOIC/QFP): larger toe/heel for the protruding lead.
        GullWing => scaled(density, 0.55, 0.45, 0.05, 0.50, 0.35, 0.25),
        // Chip passives: symmetric toe/heel, generous courtyard.
        Chip => scaled(density, 0.30, 0.20, 0.05, 0.40, 0.25, 0.15),
        // J-lead (PLCC/SOJ).
        JLead => scaled(density, 0.55, 0.10, 0.05, 0.50, 0.35, 0.25),
        // Tabbed power SMD (DPAK/D2PAK).
        TabbedSmd => scaled(density, 0.55, 0.45, 0.05, 0.50, 0.35, 0.25),
        // Through-hole / headers / terminals / BGA: pad sizing is annular-ring
        // or ball-based rather than fillet-based; we still return a courtyard
        // excess and zero fillet goals as a sane default.
        ThroughHole | Header | Terminal | Bga => scaled(density, 0.0, 0.0, 0.0, 0.30, 0.20, 0.15),
    }
}

/// Pick the per-density triple, then return goals. The three `*_excess` args
/// are the courtyard excess at Most/Nominal/Least respectively; the toe/heel/
/// side are nominal values scaled by a per-density factor.
fn scaled(
    density: DensityLevel,
    toe_nom: f64,
    heel_nom: f64,
    side_nom: f64,
    cy_most: f64,
    cy_nominal: f64,
    cy_least: f64,
) -> IpcGoals {
    // Most material (level A) grows lands ~15%; least (level C) shrinks ~15%.
    let (k, cy) = match density {
        DensityLevel::Most => (1.15, cy_most),
        DensityLevel::Nominal => (1.0, cy_nominal),
        DensityLevel::Least => (0.85, cy_least),
    };
    IpcGoals {
        toe: toe_nom * k,
        heel: heel_nom * k,
        side: side_nom * k,
        courtyard_excess: cy,
    }
}

/// Land dimensions for one terminal, computed from its contact length/width and
/// the fillet goals.
///
/// Returns `(land_length, land_width, outer_edge)` where `land_length` is the
/// radial extent (in/out from the body), `land_width` is tangential, and
/// `outer_edge` is the distance from the package center-line to the land's
/// outer edge (used to place the land so its outer edge sits `toe` beyond the
/// terminal's outer edge).
pub fn land_for_terminal(
    lead_length: f64,
    lead_width: f64,
    terminal_outer_edge: f64,
    goals: &IpcGoals,
) -> (f64, f64, f64) {
    let land_length = lead_length + goals.toe + goals.heel;
    let land_width = (lead_width + 2.0 * goals.side).max(0.05);
    let outer_edge = terminal_outer_edge + goals.toe;
    (land_length, land_width, outer_edge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_scales_lands_monotonically() {
        let most = goals(PackageFamily::NoLead, DensityLevel::Most);
        let nom = goals(PackageFamily::NoLead, DensityLevel::Nominal);
        let least = goals(PackageFamily::NoLead, DensityLevel::Least);
        assert!(most.toe > nom.toe && nom.toe > least.toe);
        assert!(most.courtyard_excess >= nom.courtyard_excess);
        assert!(nom.courtyard_excess >= least.courtyard_excess);
    }

    #[test]
    fn land_width_never_collapses() {
        let g = IpcGoals {
            toe: 0.3,
            heel: 0.0,
            side: -0.5, // absurd negative side
            courtyard_excess: 0.1,
        };
        let (_, w, _) = land_for_terminal(0.4, 0.2, 2.5, &g);
        assert!(w >= 0.05, "land width must stay positive, got {w}");
    }

    #[test]
    fn outer_edge_places_land_past_terminal_by_toe() {
        let g = goals(PackageFamily::NoLead, DensityLevel::Nominal);
        let (_, _, outer) = land_for_terminal(0.4, 0.2, 2.5, &g);
        assert!((outer - (2.5 + g.toe)).abs() < 1e-12);
    }
}
