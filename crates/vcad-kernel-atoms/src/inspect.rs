//! Structural analysis: formula, center of mass, radius of gyration, bounding
//! box, and species counts — the atomic-domain analog of `inspect_cad`.

use std::collections::BTreeMap;

use crate::vec3;
use vcad_ir::molecule::MoleculeSystem;

/// Summary metrics for a molecular system.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MoleculeReport {
    /// Total atom count.
    pub atom_count: usize,
    /// Hill-order chemical formula (e.g. "C2H6O").
    pub formula: String,
    /// Per-element counts.
    pub species_counts: BTreeMap<String, usize>,
    /// Total mass in amu.
    pub mass_amu: f64,
    /// Center of mass in Å.
    pub center_of_mass: [f64; 3],
    /// Radius of gyration in Å.
    pub radius_of_gyration: f64,
    /// Axis-aligned bounding box `[min, max]` in Å.
    pub bbox: [[f64; 3]; 2],
    /// Number of perceived/stored bonds.
    pub bond_count: usize,
    /// Whether the system is periodic.
    pub periodic: bool,
}

/// Compute a full report for a molecular system.
pub fn report(mol: &MoleculeSystem) -> MoleculeReport {
    let n = mol.len();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut mass = 0.0;
    for i in 0..n {
        let sp = &mol.species[mol.species_idx[i] as usize];
        *counts.entry(sp.element.clone()).or_insert(0) += 1;
        mass += if sp.mass > 0.0 {
            sp.mass
        } else {
            crate::element::lookup(&sp.element).mass
        };
    }

    // Center of mass (mass-weighted).
    let mut com = [0.0; 3];
    let mut total_m = 0.0;
    for i in 0..n {
        let m = {
            let sp = &mol.species[mol.species_idx[i] as usize];
            if sp.mass > 0.0 {
                sp.mass
            } else {
                crate::element::lookup(&sp.element).mass
            }
        };
        vec3::add_assign(&mut com, vec3::scale(mol.positions[i], m));
        total_m += m;
    }
    if total_m > 0.0 {
        com = vec3::scale(com, 1.0 / total_m);
    }

    // Radius of gyration (mass-weighted RMS distance from COM).
    let mut rg2_num = 0.0;
    for i in 0..n {
        let m = {
            let sp = &mol.species[mol.species_idx[i] as usize];
            if sp.mass > 0.0 {
                sp.mass
            } else {
                crate::element::lookup(&sp.element).mass
            }
        };
        rg2_num += m * vec3::norm2(vec3::sub(mol.positions[i], com));
    }
    let radius_of_gyration = if total_m > 0.0 {
        (rg2_num / total_m).sqrt()
    } else {
        0.0
    };

    // Bounding box.
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for p in &mol.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    if n == 0 {
        min = [0.0; 3];
        max = [0.0; 3];
    }

    MoleculeReport {
        atom_count: n,
        formula: hill_formula(&counts),
        species_counts: counts,
        mass_amu: mass,
        center_of_mass: com,
        radius_of_gyration,
        bbox: [min, max],
        bond_count: mol.bonds.len(),
        periodic: mol.cell.is_some(),
    }
}

/// Build a Hill-order formula: carbon first, hydrogen second, then the rest
/// alphabetically. Counts of 1 are written without a trailing digit.
fn hill_formula(counts: &BTreeMap<String, usize>) -> String {
    let mut out = String::new();
    let mut push = |sym: &str, n: usize| {
        if n == 0 {
            return;
        }
        out.push_str(sym);
        if n > 1 {
            out.push_str(&n.to_string());
        }
    };
    if let Some(&c) = counts.get("C") {
        push("C", c);
        if let Some(&h) = counts.get("H") {
            push("H", h);
        }
    } else if let Some(&h) = counts.get("H") {
        push("H", h);
    }
    // C and H (when present) are already emitted above in Hill order.
    for (sym, &n) in counts {
        if sym == "C" || sym == "H" {
            continue;
        }
        push(sym, n);
    }
    out
}

/// RMSD (Å) between two systems with identical atom ordering (no alignment).
pub fn rmsd(a: &MoleculeSystem, b: &MoleculeSystem) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut s = 0.0;
    for i in 0..a.len() {
        s += vec3::norm2(vec3::sub(a.positions[i], b.positions[i]));
    }
    Some((s / a.len() as f64).sqrt())
}
