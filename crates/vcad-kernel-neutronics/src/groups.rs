//! The multigroup energy structure.
//!
//! Five groups spanning the D-D source line (2.45 MeV) down to thermal.
//! Boundaries (eV), documented so every tally and cross section states
//! its energy meaning:
//!
//! | group | range | representative energy |
//! |---|---|---|
//! | 0 (source) | 1 MeV – 3 MeV | 2.45 MeV (the D-D line, not the log-midpoint) |
//! | 1 | 100 keV – 1 MeV | 316 keV (log-midpoint) |
//! | 2 | 1 keV – 100 keV | 10 keV (log-midpoint) |
//! | 3 (epithermal) | 0.5 eV – 1 keV | 22.4 eV (log-midpoint) |
//! | 4 (thermal) | 0.1 meV – 0.5 eV | 25.3 meV (2200 m/s Maxwellian anchor) |
//!
//! Cross sections are evaluated at the representative energies
//! ([`representative_energy_ev`]); the source group is anchored at the
//! 2.45 MeV line because first flights dominate fast-dose penetration.
//! Group-coarseness is a stated limitation of the whole library, not a
//! hidden one: see `materials` module docs.

/// Number of energy groups.
pub const N_GROUPS: usize = 5;

/// Group boundaries in eV, descending; group `g` spans
/// `[GROUP_BOUNDS_EV[g+1], GROUP_BOUNDS_EV[g]]`.
pub const GROUP_BOUNDS_EV: [f64; N_GROUPS + 1] = [3.0e6, 1.0e6, 1.0e5, 1.0e3, 0.5, 1.0e-4];

/// The group containing the 2.45 MeV D-D source line.
pub const SOURCE_GROUP: usize = 0;

/// The thermal group (no further downscatter).
pub const THERMAL_GROUP: usize = N_GROUPS - 1;

/// Representative energy (eV) at which group constants are evaluated.
pub fn representative_energy_ev(g: usize) -> f64 {
    assert!(g < N_GROUPS, "group index {g} out of range");
    match g {
        SOURCE_GROUP => crate::constants::DD_NEUTRON_EV,
        THERMAL_GROUP => crate::constants::THERMAL_EV,
        _ => (GROUP_BOUNDS_EV[g] * GROUP_BOUNDS_EV[g + 1]).sqrt(),
    }
}

/// Group index containing energy `e_ev`, or `None` outside the structure.
pub fn group_of_energy_ev(e_ev: f64) -> Option<usize> {
    if !(GROUP_BOUNDS_EV[N_GROUPS]..=GROUP_BOUNDS_EV[0]).contains(&e_ev) {
        return None;
    }
    Some(
        (0..N_GROUPS)
            .find(|&g| e_ev >= GROUP_BOUNDS_EV[g + 1])
            .expect("bounds are exhaustive"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_descend_and_cover_source() {
        for g in 0..N_GROUPS {
            assert!(GROUP_BOUNDS_EV[g] > GROUP_BOUNDS_EV[g + 1]);
        }
        assert_eq!(
            group_of_energy_ev(crate::constants::DD_NEUTRON_EV),
            Some(SOURCE_GROUP)
        );
        assert_eq!(
            group_of_energy_ev(crate::constants::THERMAL_EV),
            Some(THERMAL_GROUP)
        );
        assert_eq!(group_of_energy_ev(5.0e6), None);
    }

    #[test]
    fn representative_energies_inside_their_groups() {
        for g in 0..N_GROUPS {
            let e = representative_energy_ev(g);
            assert_eq!(group_of_energy_ev(e), Some(g), "group {g} rep {e}");
        }
    }
}
