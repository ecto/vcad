//! The bundled multigroup material library.
//!
//! **This is a design-estimate library, not an evaluated nuclear data
//! file.** Microscopic cross sections are single-point values read from
//! evaluated-data plots (ENDF/B-VIII.0 lineage) and standard references
//! (Sears 1992 for thermal scattering lengths; Mughabghab's atlas /
//! Lamarsh's tables for 2200 m/s absorption) at each group's
//! representative energy, with 1/v extrapolation for absorbers.
//! Resonance structure inside a group is averaged away by eye, not by
//! flux weighting over an evaluated file. Expect group constants to be
//! good to **±20–30%**, which bounds any dose prediction made with them;
//! every downstream claim must carry that caveat alongside its Monte
//! Carlo error bar. What this library is honestly good for: comparative
//! shield sizing (does 20 cm of HDPE buy 10× over 10 cm?) and
//! order-of-magnitude dose rates — exactly the design questions.
//!
//! Scattering model: isotropic-in-CM elastic scattering off free nuclei.
//! Group-transfer matrices are derived in `transfer_row` from the exact
//! single-collision energy kernel (E′ uniform on [αE, E], α=((A−1)/(A+1))²)
//! averaged over a flat-in-lethargy flux across the source group — the
//! standard multigroup construction, computed numerically at build time so
//! the derivation is *in the code*, not in an opaque table. The thermal
//! group has no downscatter (in-group scattering only; upscatter and
//! free-gas thermal motion are neglected — stated M0 limitation).
//!
//! Bound-hydrogen adjustment: free-atom σ_s(H) = 20.4 b at thermal
//! understates bound-proton scattering in water/polyethylene (molecular
//! totals ≈ 100 b per H₂O / per CH₂ at 25.3 meV — Sears 1992). Hydrogenous
//! materials therefore override the thermal-group σ_s of hydrogen with a
//! bound value (~45–48 b per proton) and a reduced mean scattering cosine
//! (0.35 instead of the free-atom 2/3 — chosen so water's thermal
//! diffusion coefficient lands near the published D ≈ 0.16 cm, Lamarsh
//! Table 5-2; a calibrated constant, declared as such).

use crate::groups::{GROUP_BOUNDS_EV, N_GROUPS, THERMAL_GROUP};

/// Library version tag, carried in provenance on every claim.
pub const LIBRARY_VERSION: &str = "vcad-neutronics-lib/0.1.0-design-estimate";

/// One element's design-estimate microscopic data (barns).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementData {
    /// Element symbol.
    pub symbol: &'static str,
    /// Atomic mass (amu) — sets the elastic-scatter kinematics α and the
    /// free-atom mean cosine μ̄ = 2/(3A).
    pub mass_amu: f64,
    /// Elastic scattering cross section per group, barns, at the group
    /// representative energies (2.45 MeV / 316 keV / 10 keV / 22.4 eV /
    /// 25.3 meV).
    pub sigma_s_b: [f64; N_GROUPS],
    /// Absorption cross section per group, barns (same energies).
    pub sigma_a_b: [f64; N_GROUPS],
}

// Per-value sources. "ENDF plot" = value read from the ENDF/B-VIII.0
// evaluated curve at the group representative energy (design-estimate
// precision); "Sears" = free-atom scattering from Sears, Neutron News 3
// (1992); "2200 m/s" = thermal absorption from Mughabghab's atlas as
// tabulated in Lamarsh, Introduction to Nuclear Engineering, App. II,
// with 1/v extrapolation √(0.0253 eV / E) to the other groups.

/// Hydrogen (¹H).
pub const HYDROGEN: ElementData = ElementData {
    symbol: "H",
    mass_amu: 1.008,
    // n-p elastic: 2.54 b @ 2.45 MeV, 7.8 b @ 316 keV, 19.4 b @ 10 keV
    // (ENDF plot of the smooth n-p curve); 20.4 b free-atom plateau
    // below ~1 keV (Sears: σ_free = σ_bound/(1+1/A)² = 81.7/4.02).
    sigma_s_b: [2.54, 7.8, 19.4, 20.4, 20.4],
    // ¹H(n,γ)²H = 0.332 b at 2200 m/s (Mughabghab); 1/v above
    // (3.0e-5 b at 2.45 MeV is the 1/v tail — the measured MeV capture
    // is of the same few-tens-of-µb order).
    sigma_a_b: [3.0e-5, 9.4e-5, 5.3e-4, 1.12e-2, 0.332],
};

/// Carbon (natural).
pub const CARBON: ElementData = ElementData {
    symbol: "C",
    mass_amu: 12.011,
    // ENDF plot: ~1.6 b @ 2.45 MeV (between the 2.08 MeV resonance and
    // the 2.8 MeV dip), 3.9 b @ 316 keV, 4.7 b plateau; Sears free-atom
    // 4.74 b at thermal.
    sigma_s_b: [1.60, 3.9, 4.7, 4.74, 4.74],
    // 2200 m/s capture 3.53 mb (Mughabghab), 1/v; fast capture ~µb and
    // (n,α) threshold is 6.18 MeV — above our top group, so ~0.
    sigma_a_b: [0.0, 1.0e-6, 5.6e-6, 1.19e-4, 3.53e-3],
};

/// Oxygen (natural, ¹⁶O-dominated).
pub const OXYGEN: ElementData = ElementData {
    symbol: "O",
    mass_amu: 15.999,
    // ENDF plot: ~1.6 b @ 2.45 MeV (inter-resonance window), 3.5 b @
    // 316 keV, 3.78 b plateau (Sears free-atom 3.76 b).
    sigma_s_b: [1.6, 3.5, 3.78, 3.78, 3.78],
    // 2200 m/s capture 0.19 mb (Mughabghab), 1/v; 5 mb at 2.45 MeV for
    // ¹⁶O(n,α)¹³C just above its 2.35 MeV threshold (ENDF plot).
    sigma_a_b: [5.0e-3, 1.0e-6, 3.0e-7, 6.4e-6, 1.9e-4],
};

/// Nitrogen (natural).
pub const NITROGEN: ElementData = ElementData {
    symbol: "N",
    mass_amu: 14.007,
    // ENDF plot fast values; Sears free-atom σ_s(N) = 11.5 b at thermal.
    sigma_s_b: [1.6, 3.6, 9.0, 11.0, 11.5],
    // ¹⁴N(n,p)¹⁴C = 1.83 b at 2200 m/s (Mughabghab), ~1/v; ~20 mb
    // combined (n,p)+(n,α) at 2.45 MeV (ENDF plot).
    sigma_a_b: [2.0e-2, 5.2e-4, 2.9e-3, 6.2e-2, 1.83],
};

/// Boron (natural: 19.9 at% ¹⁰B).
pub const BORON: ElementData = ElementData {
    symbol: "B",
    mass_amu: 10.81,
    // Sears natural-B σ_s ≈ 4.3 b thermal; ENDF plot fast values.
    sigma_s_b: [2.0, 2.6, 3.5, 4.2, 4.27],
    // ¹⁰B(n,α) 3837 b at 2200 m/s → natural 767 b (Lamarsh App. II);
    // famously clean 1/v through the keV range: 25.8 b @ 22.4 eV,
    // 1.22 b @ 10 keV, 0.22 b @ 316 keV, ~0.08 b @ 2.45 MeV.
    sigma_a_b: [8.0e-2, 0.22, 1.22, 25.8, 767.0],
};

/// Silicon (natural).
pub const SILICON: ElementData = ElementData {
    symbol: "Si",
    mass_amu: 28.085,
    // Sears free-atom 2.17 b thermal; ENDF plot fast values. keV-region
    // resonance structure (55 keV etc.) is eye-averaged — design grade.
    sigma_s_b: [2.7, 3.0, 2.2, 2.17, 2.17],
    // 2200 m/s capture 0.171 b (Mughabghab), 1/v.
    sigma_a_b: [5.0e-3, 1.0e-3, 2.7e-4, 5.7e-3, 0.171],
};

/// Calcium (natural).
pub const CALCIUM: ElementData = ElementData {
    symbol: "Ca",
    mass_amu: 40.078,
    // Sears free-atom 2.83 b thermal; ENDF plot fast values.
    sigma_s_b: [2.3, 2.5, 3.0, 2.83, 2.83],
    // 2200 m/s capture 0.43 b (Mughabghab), 1/v.
    sigma_a_b: [5.0e-3, 2.0e-3, 6.8e-4, 1.44e-2, 0.43],
};

/// Aluminum.
pub const ALUMINUM: ElementData = ElementData {
    symbol: "Al",
    mass_amu: 26.982,
    // Sears free-atom 1.50 b thermal; ENDF plot fast values.
    sigma_s_b: [2.4, 3.0, 2.5, 1.5, 1.5],
    // 2200 m/s capture 0.231 b (Mughabghab), 1/v.
    sigma_a_b: [3.0e-3, 1.0e-3, 3.7e-4, 7.8e-3, 0.231],
};

/// Iron (natural).
pub const IRON: ElementData = ElementData {
    symbol: "Fe",
    mass_amu: 55.845,
    // Sears free-atom 11.6 b thermal; the 24 keV s-wave window makes the
    // 10 keV representative value (6 b) especially design-grade.
    sigma_s_b: [2.6, 2.8, 6.0, 11.4, 11.6],
    // 2200 m/s capture 2.56 b (Mughabghab), 1/v.
    sigma_a_b: [5.0e-3, 3.0e-3, 4.1e-3, 8.6e-2, 2.56],
};

/// Lead (natural).
pub const LEAD: ElementData = ElementData {
    symbol: "Pb",
    mass_amu: 207.2,
    // Sears free-atom ≈ 11.3 b thermal; elastic ~5 b @ 2.45 MeV (ENDF
    // plot). CAVEAT: Pb inelastic scattering (~1.7 b at 2.45 MeV, ENDF)
    // is NOT modeled at M0 — lead reads as nearly transparent to fast
    // neutrons here, which errs on the conservative (high-dose) side.
    sigma_s_b: [5.0, 6.5, 11.0, 11.26, 11.26],
    // 2200 m/s capture 0.171 b (Mughabghab), 1/v.
    sigma_a_b: [3.0e-3, 4.0e-4, 2.7e-4, 5.7e-3, 0.171],
};

/// Bound-atom thermal-group override for hydrogen in molecular solids
/// and liquids (see module docs — the calibrated constants).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundThermal {
    /// Effective thermal σ_s per bound proton, barns.
    pub sigma_s_b: f64,
    /// Effective thermal mean scattering cosine.
    pub mu_bar: f64,
}

/// H bound in water: molecular total ≈ 103 b per H₂O at 25.3 meV
/// (Sears 1992) → ≈ 48 b per proton after subtracting oxygen.
pub const H_IN_WATER: BoundThermal = BoundThermal {
    sigma_s_b: 48.0,
    mu_bar: 0.35,
};

/// H bound in CH₂ (polyethylene, paraffin): molecular total ≈ 95 b per
/// CH₂ at 25.3 meV (Sears-based estimate) → ≈ 45 b per proton.
pub const H_IN_CH2: BoundThermal = BoundThermal {
    sigma_s_b: 45.0,
    mu_bar: 0.35,
};

/// One constituent of a material.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Constituent {
    element: ElementData,
    atoms_per_cc: f64,
    bound_thermal: Option<BoundThermal>,
}

/// Macroscopic multigroup constants for one material.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// Human-readable name (also the library key).
    pub name: String,
    /// Mass density used to build the number densities, g/cm³.
    pub density_g_cc: f64,
    /// Total macroscopic cross section per group, 1/cm.
    pub sigma_t: [f64; N_GROUPS],
    /// Macroscopic scattering per group, 1/cm.
    pub sigma_s: [f64; N_GROUPS],
    /// Macroscopic absorption per group, 1/cm.
    pub sigma_a: [f64; N_GROUPS],
    /// Group-transfer probabilities: `transfer[g][g']` = P(scatter in g
    /// lands in g′). Rows sum to 1; strictly lower-triangular-plus-diagonal
    /// (downscatter only).
    pub transfer: [[f64; N_GROUPS]; N_GROUPS],
    /// Scatter-weighted mean lab cosine per group (free-atom 2/(3A) with
    /// bound-thermal overrides). Unused by M0's isotropic transport;
    /// consumed by the M1 anisotropy rung and the M2 diffusion companion.
    pub mu_bar: [f64; N_GROUPS],
    /// Collision-partner table for the M1 exact-kinematics mode: each
    /// entry is a nuclide mass with its share of Σ_s per group.
    pub scatterers: Vec<Scatterer>,
}

/// One collision partner: nuclide mass and its share of the scattering
/// cross section per group.
#[derive(Debug, Clone, PartialEq)]
pub struct Scatterer {
    /// Nuclide mass, amu (validation fictions use a huge mass — elastic
    /// scatter off it changes nothing, matching their in-group isotropic
    /// semantics exactly).
    pub mass_amu: f64,
    /// Share of Σ_s in each group (sums to 1 over scatterers per group
    /// wherever Σ_s > 0).
    pub share: [f64; N_GROUPS],
}

/// Single-collision downscatter row for elastic scattering off a free
/// nucleus of mass `a_amu`, from group `g`, averaged over a flat-in-
/// lethargy flux across the group (33-point log quadrature).
///
/// E′ is uniform on [αE, E] for isotropic-CM elastic scatter; energy
/// leaking below the thermal floor is banked into the thermal group.
fn transfer_row(a_amu: f64, g: usize) -> [f64; N_GROUPS] {
    let mut row = [0.0; N_GROUPS];
    if g == THERMAL_GROUP {
        row[g] = 1.0; // no downscatter out of thermal (no upscatter either)
        return row;
    }
    let alpha = ((a_amu - 1.0) / (a_amu + 1.0)).powi(2);
    let (e_hi, e_lo) = (GROUP_BOUNDS_EV[g], GROUP_BOUNDS_EV[g + 1]);
    const M: usize = 33;
    for k in 0..M {
        // Log-spaced midpoints = flat-in-lethargy source weighting.
        let e = e_hi * (e_lo / e_hi).powf((k as f64 + 0.5) / M as f64);
        let (lo, hi) = (alpha * e, e);
        let width = hi - lo;
        for (g2, r) in row.iter_mut().enumerate().skip(g) {
            // The thermal group swallows everything below its lower bound.
            let bin_lo = if g2 == THERMAL_GROUP {
                0.0
            } else {
                GROUP_BOUNDS_EV[g2 + 1]
            };
            let bin_hi = GROUP_BOUNDS_EV[g2];
            let overlap = (hi.min(bin_hi) - lo.max(bin_lo)).max(0.0);
            *r += overlap / width / M as f64;
        }
    }
    // Guard tiny quadrature loss so rows are exactly stochastic.
    let sum: f64 = row.iter().sum();
    for r in row.iter_mut() {
        *r /= sum;
    }
    row
}

impl Material {
    fn from_constituents(name: &str, density_g_cc: f64, parts: &[Constituent]) -> Material {
        let mut sigma_s = [0.0; N_GROUPS];
        let mut sigma_a = [0.0; N_GROUPS];
        let mut transfer = [[0.0; N_GROUPS]; N_GROUPS];
        let mut mu_bar = [0.0; N_GROUPS];
        let mut scatterers: Vec<Scatterer> = parts
            .iter()
            .map(|p| Scatterer {
                mass_amu: p.element.mass_amu,
                share: [0.0; N_GROUPS],
            })
            .collect();
        for g in 0..N_GROUPS {
            for (i, p) in parts.iter().enumerate() {
                let (s_b, mu) = if g == THERMAL_GROUP {
                    match p.bound_thermal {
                        Some(bt) => (bt.sigma_s_b, bt.mu_bar),
                        None => (p.element.sigma_s_b[g], 2.0 / (3.0 * p.element.mass_amu)),
                    }
                } else {
                    (p.element.sigma_s_b[g], 2.0 / (3.0 * p.element.mass_amu))
                };
                let s = p.atoms_per_cc * s_b * 1.0e-24;
                sigma_s[g] += s;
                scatterers[i].share[g] = s;
                sigma_a[g] += p.atoms_per_cc * p.element.sigma_a_b[g] * 1.0e-24;
                mu_bar[g] += s * mu;
                let row = transfer_row(p.element.mass_amu, g);
                for (t, r) in transfer[g].iter_mut().zip(row.iter()) {
                    *t += s * r;
                }
            }
            if sigma_s[g] > 0.0 {
                mu_bar[g] /= sigma_s[g];
                for t in transfer[g].iter_mut() {
                    *t /= sigma_s[g];
                }
                for sc in scatterers.iter_mut() {
                    sc.share[g] /= sigma_s[g];
                }
            } else {
                transfer[g][g] = 1.0; // vacuous (never sampled)
            }
        }
        let mut sigma_t = [0.0; N_GROUPS];
        for g in 0..N_GROUPS {
            sigma_t[g] = sigma_s[g] + sigma_a[g];
        }
        Material {
            name: name.to_string(),
            density_g_cc,
            sigma_t,
            sigma_s,
            sigma_a,
            transfer,
            mu_bar,
            scatterers,
        }
    }

    /// Build from mass fractions (the library path): number densities
    /// N_i = ρ wᵢ N_A / Aᵢ.
    fn from_mass_fractions(
        name: &str,
        density_g_cc: f64,
        parts: &[(ElementData, f64, Option<BoundThermal>)],
    ) -> Material {
        let total: f64 = parts.iter().map(|(_, w, _)| w).sum();
        assert!(
            (total - 1.0).abs() < 1.0e-6,
            "mass fractions must sum to 1 (got {total})"
        );
        let cs: Vec<Constituent> = parts
            .iter()
            .map(|(el, w, bt)| Constituent {
                element: *el,
                atoms_per_cc: density_g_cc * w * crate::constants::AVOGADRO / el.mass_amu,
                bound_thermal: *bt,
            })
            .collect();
        Material::from_constituents(name, density_g_cc, &cs)
    }

    /// The collision-partner table shared by the validation fictions: a
    /// single effectively infinite mass, so exact-kinematics scattering
    /// degenerates to isotropic-in-lab with no energy change — matching
    /// the fictions' in-group semantics in **both** energy models.
    fn fiction_scatterers() -> Vec<Scatterer> {
        vec![Scatterer {
            mass_amu: 1.0e12,
            share: [1.0; N_GROUPS],
        }]
    }

    /// Fictitious pure absorber with a group-independent Σ_a (1/cm).
    /// **Validation fiction, not a physical material** — it makes the
    /// uncollided-flux tests exact (`φ = S·e^{−Σ_t r}/4πr²`).
    pub fn pure_absorber(sigma_a_per_cm: f64) -> Material {
        let mut transfer = [[0.0; N_GROUPS]; N_GROUPS];
        for (g, row) in transfer.iter_mut().enumerate() {
            row[g] = 1.0;
        }
        Material {
            name: format!("pure-absorber({sigma_a_per_cm}/cm)"),
            density_g_cc: 0.0,
            sigma_t: [sigma_a_per_cm; N_GROUPS],
            sigma_s: [0.0; N_GROUPS],
            sigma_a: [sigma_a_per_cm; N_GROUPS],
            transfer,
            mu_bar: [0.0; N_GROUPS],
            scatterers: Material::fiction_scatterers(),
        }
    }

    /// Fictitious one-group medium: group-independent Σ_s and Σ_a with
    /// isotropic in-group scattering. **Validation/benchmark fiction** —
    /// it turns the transport into the exactly-solvable one-speed problem
    /// (diffusion φ = S·e^{−r/L}/4πDr far from sources/boundaries).
    pub fn one_group(sigma_s_per_cm: f64, sigma_a_per_cm: f64) -> Material {
        let mut transfer = [[0.0; N_GROUPS]; N_GROUPS];
        for (g, row) in transfer.iter_mut().enumerate() {
            row[g] = 1.0;
        }
        Material {
            name: format!("one-group(s={sigma_s_per_cm},a={sigma_a_per_cm})"),
            density_g_cc: 0.0,
            sigma_t: [sigma_s_per_cm + sigma_a_per_cm; N_GROUPS],
            sigma_s: [sigma_s_per_cm; N_GROUPS],
            sigma_a: [sigma_a_per_cm; N_GROUPS],
            transfer,
            mu_bar: [0.0; N_GROUPS],
            scatterers: Material::fiction_scatterers(),
        }
    }

    /// True vacuum (Σ = 0): flights cross without interacting.
    pub fn void() -> Material {
        let mut transfer = [[0.0; N_GROUPS]; N_GROUPS];
        for (g, row) in transfer.iter_mut().enumerate() {
            row[g] = 1.0;
        }
        Material {
            name: "void".to_string(),
            density_g_cc: 0.0,
            sigma_t: [0.0; N_GROUPS],
            sigma_s: [0.0; N_GROUPS],
            sigma_a: [0.0; N_GROUPS],
            transfer,
            mu_bar: [0.0; N_GROUPS],
            scatterers: Material::fiction_scatterers(),
        }
    }
}

/// High-density polyethylene (CH₂)ₙ, ρ = 0.95 g/cm³ (typical HDPE;
/// vendor sheets 0.94–0.97).
pub fn hdpe() -> Material {
    // CH₂ unit: wt H = 2·1.008/14.027 = 0.1437, wt C = 0.8563.
    Material::from_mass_fractions(
        "HDPE",
        0.95,
        &[(HYDROGEN, 0.1437, Some(H_IN_CH2)), (CARBON, 0.8563, None)],
    )
}

/// Paraffin wax (≈C₂₅H₅₂), ρ = 0.90 g/cm³.
pub fn paraffin() -> Material {
    // C₂₅H₅₂: wt H = 52·1.008/352.7 = 0.1486, wt C = 0.8514.
    Material::from_mass_fractions(
        "paraffin",
        0.90,
        &[(HYDROGEN, 0.1486, Some(H_IN_CH2)), (CARBON, 0.8514, None)],
    )
}

/// 5 wt% borated polyethylene, ρ = 1.00 g/cm³ (commercial 5% borated
/// PE sheet, e.g. SWX-201-class).
pub fn borated_hdpe_5() -> Material {
    Material::from_mass_fractions(
        "borated-HDPE-5%",
        1.00,
        &[
            (HYDROGEN, 0.95 * 0.1437, Some(H_IN_CH2)),
            (CARBON, 0.95 * 0.8563, None),
            (BORON, 0.05, None),
        ],
    )
}

/// Light water, ρ = 0.998 g/cm³ (20 °C).
pub fn water() -> Material {
    // H₂O: wt H = 2·1.008/18.015 = 0.1119, wt O = 0.8881.
    Material::from_mass_fractions(
        "water",
        0.998,
        &[(HYDROGEN, 0.1119, Some(H_IN_WATER)), (OXYGEN, 0.8881, None)],
    )
}

/// Lead, ρ = 11.35 g/cm³. **Gamma shield, not a neutron shield** — and at
/// M0 its inelastic scattering is unmodeled (see [`LEAD`] caveat), so it
/// reads even more neutron-transparent than it is.
pub fn lead() -> Material {
    Material::from_mass_fractions("lead", 11.35, &[(LEAD, 1.0, None)])
}

/// Ordinary concrete, ρ = 2.30 g/cm³. NIST ordinary-concrete composition
/// with minor elements (Na, Mg, K) folded into Al — a documented
/// simplification: wt% H 1.0, O 53.2, Al 6.5, Si 33.5, Ca 4.4, Fe 1.4.
/// Hydrogen content varies with cure/water history — the single most
/// dose-relevant uncertainty in any concrete estimate (flagged on
/// claims).
pub fn concrete() -> Material {
    Material::from_mass_fractions(
        "concrete",
        2.30,
        &[
            (HYDROGEN, 0.010, Some(H_IN_WATER)),
            (OXYGEN, 0.532, None),
            (ALUMINUM, 0.065, None),
            (SILICON, 0.335, None),
            (CALCIUM, 0.044, None),
            (IRON, 0.014, None),
        ],
    )
}

/// Air, ρ = 1.205 mg/cm³ (20 °C, 1 atm), argon folded into nitrogen:
/// wt% N 76.8, O 23.2.
pub fn air() -> Material {
    Material::from_mass_fractions(
        "air",
        1.205e-3,
        &[(NITROGEN, 0.768, None), (OXYGEN, 0.232, None)],
    )
}

/// Look up a library material by name (the M3 spec seam's vocabulary).
/// Fail-closed: unknown names are `None`, never a default.
pub fn by_name(name: &str) -> Option<Material> {
    match name {
        "hdpe" | "HDPE" => Some(hdpe()),
        "paraffin" => Some(paraffin()),
        "borated-hdpe-5" | "borated-HDPE-5%" => Some(borated_hdpe_5()),
        "water" => Some(water()),
        "lead" => Some(lead()),
        "concrete" => Some(concrete()),
        "air" => Some(air()),
        "void" => Some(Material::void()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::SOURCE_GROUP;

    fn check_material(m: &Material) {
        for g in 0..N_GROUPS {
            assert!(
                (m.sigma_t[g] - m.sigma_s[g] - m.sigma_a[g]).abs() < 1.0e-12,
                "{}: Σt ≠ Σs+Σa in group {g}",
                m.name
            );
            let row_sum: f64 = m.transfer[g].iter().sum();
            assert!(
                (row_sum - 1.0).abs() < 1.0e-9,
                "{}: transfer row {g} sums to {row_sum}",
                m.name
            );
            for g2 in 0..g {
                assert_eq!(
                    m.transfer[g][g2], 0.0,
                    "{}: upscatter {g}→{g2} must be zero",
                    m.name
                );
            }
        }
    }

    #[test]
    fn library_materials_are_consistent() {
        for m in [
            hdpe(),
            paraffin(),
            borated_hdpe_5(),
            water(),
            lead(),
            concrete(),
            air(),
            Material::void(),
            Material::pure_absorber(0.2),
            Material::one_group(1.0, 0.02),
        ] {
            check_material(&m);
        }
    }

    #[test]
    fn hydrogen_downscatter_reaches_far_groups() {
        // From 2.45 MeV, one H collision lands below 1 MeV with
        // probability ≈ mean over the group of (1 MeV)/E — far from zero.
        let m = hdpe();
        let out_of_group: f64 = m.transfer[SOURCE_GROUP][1..].iter().sum();
        assert!(
            out_of_group > 0.3,
            "H-rich downscatter out of source group = {out_of_group}"
        );
        // Lead barely moderates: single elastic collision at 2.45 MeV
        // loses ≤ 1.9% of energy (α = 0.981) — it cannot leave a
        // decade-wide group in one scatter from the middle.
        let pb = lead();
        assert!(
            pb.transfer[SOURCE_GROUP][SOURCE_GROUP] > 0.9,
            "Pb in-group fraction = {}",
            pb.transfer[SOURCE_GROUP][SOURCE_GROUP]
        );
    }

    #[test]
    fn macroscopic_totals_match_hand_calculations() {
        // HDPE fast: N_H·σ + N_C·σ with N_H = 8.16e22, N_C = 4.08e22.
        let m = hdpe();
        assert!(
            (m.sigma_t[SOURCE_GROUP] - 0.272).abs() < 0.01,
            "HDPE Σt(2.45 MeV) = {} (expect ≈0.27/cm, mfp ≈ 3.7 cm)",
            m.sigma_t[SOURCE_GROUP]
        );
        // Water fast: 0.223/cm → mfp 4.5 cm (textbook fast-neutron mfp).
        let w = water();
        assert!(
            (w.sigma_t[SOURCE_GROUP] - 0.223).abs() < 0.01,
            "water Σt(2.45 MeV) = {}",
            w.sigma_t[SOURCE_GROUP]
        );
        // Borated poly thermal absorption is boron-dominated:
        // N_B·767 b = 2.79e21·767e-24 ≈ 2.1/cm.
        let b = borated_hdpe_5();
        assert!(
            b.sigma_a[THERMAL_GROUP] > 1.5 && b.sigma_a[THERMAL_GROUP] < 3.0,
            "borated poly thermal Σa = {}",
            b.sigma_a[THERMAL_GROUP]
        );
        // Air is nearly transparent: Σt ~ 5e-5/cm fast.
        let a = air();
        assert!(a.sigma_t[SOURCE_GROUP] < 2.0e-4);
    }

    #[test]
    fn water_thermal_diffusion_length_near_published() {
        // L = √(D/Σa), D = 1/(3Σtr), Σtr = Σt − μ̄Σs. Published: L ≈ 2.85
        // cm, D ≈ 0.16 cm (Lamarsh Table 5-2). The bound-H thermal
        // override is calibrated to land here — this test pins the
        // calibration.
        let w = water();
        let g = THERMAL_GROUP;
        let sigma_tr = w.sigma_t[g] - w.mu_bar[g] * w.sigma_s[g];
        let d = 1.0 / (3.0 * sigma_tr);
        let l = (d / w.sigma_a[g]).sqrt();
        assert!(
            (d - 0.16).abs() < 0.05,
            "water thermal D = {d} cm (published ≈ 0.16)"
        );
        assert!(
            (l - 2.85).abs() < 0.6,
            "water thermal L = {l} cm (published ≈ 2.85)"
        );
    }

    #[test]
    fn unknown_material_is_none() {
        assert!(by_name("unobtainium").is_none());
    }
}
