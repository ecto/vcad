//! Atomic / molecular domain types for vcad.
//!
//! A [`MoleculeSystem`] is an optional field on the [`crate::Document`], living
//! alongside the CAD, assembly, and ECAD domains. It is deliberately a
//! lightweight, non-BRep, structure-of-arrays representation: a species table
//! plus flat position / species-index / velocity arrays, so a document can
//! carry 10^5–10^7 atoms as a few contiguous buffers rather than that many
//! nodes.
//!
//! Units are **Ångström** for length and **amu** for mass — the molecular
//! domain keeps its own unit convention and never routes through the
//! millimeter-oriented CAD converters. Charges are elementary charge `e`.
//!
//! Atoms are never expanded into IR `Sphere` nodes or patterns; the render and
//! simulation tracks consume this store directly.

use serde::{Deserialize, Serialize};

/// A chemical species referenced by atoms in a [`MoleculeSystem`].
///
/// The species table is the deduplicated set of element/charge kinds; each atom
/// stores an index into it. Radii and color are optional overrides — when
/// absent, consumers fall back to the built-in periodic-table defaults keyed by
/// [`Species::element`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct Species {
    /// Element symbol, e.g. "C", "Fe", "Au".
    pub element: String,
    /// Atomic number (protons). 0 for a dummy/virtual site.
    #[serde(rename = "atomicNumber")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "atomicNumber"))]
    pub atomic_number: u32,
    /// Atomic mass in amu.
    pub mass: f64,
    /// Partial charge in elementary-charge units. Defaults to 0.
    #[serde(default)]
    pub charge: f64,
    /// Optional label distinguishing chemically-distinct sites of the same
    /// element (e.g. force-field atom types "CA", "CB").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub label: Option<String>,
    /// Optional visual radius override in Å (else CPK/vdW default is used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub radius: Option<f64>,
    /// Optional sRGB color override `[r, g, b]` in 0..1 (else CPK default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub color: Option<[f64; 3]>,
}

/// A bond between two atoms, by atom index.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct Bond {
    /// First atom index.
    pub a: u32,
    /// Second atom index.
    pub b: u32,
    /// Bond order (1.0 single, 1.5 aromatic, 2.0 double, …). Defaults to 1.
    #[serde(default = "one")]
    pub order: f64,
}

fn one() -> f64 {
    1.0
}

/// A periodic simulation cell defined by three lattice vectors (Å).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct Cell {
    /// Lattice vector **a** in Å.
    pub a: [f64; 3],
    /// Lattice vector **b** in Å.
    pub b: [f64; 3],
    /// Lattice vector **c** in Å.
    pub c: [f64; 3],
    /// Per-axis periodicity flags.
    #[serde(default = "all_true")]
    pub periodic: [bool; 3],
}

fn all_true() -> [bool; 3] {
    [true, true, true]
}

/// A molecular / atomic system: the optional `molecule` domain on a `Document`.
///
/// Structure-of-arrays: `positions[i]`, `species_idx[i]`, and (optionally)
/// `velocities[i]` all describe atom `i`. `species_idx[i]` indexes `species`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct MoleculeSystem {
    /// Deduplicated species table; atoms index into this by `species_idx`.
    pub species: Vec<Species>,
    /// Atom positions in Å (structure-of-arrays).
    pub positions: Vec<[f64; 3]>,
    /// Per-atom index into [`MoleculeSystem::species`].
    #[serde(rename = "speciesIdx")]
    #[cfg_attr(feature = "ts-rs", ts(rename = "speciesIdx"))]
    pub species_idx: Vec<u32>,
    /// Optional per-atom velocities in Å/fs (structure-of-arrays).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub velocities: Vec<[f64; 3]>,
    /// Bond graph. May be empty (e.g. metals, ionic crystals).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bonds: Vec<Bond>,
    /// Optional periodic cell. `None` means a non-periodic (cluster) system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub cell: Option<Cell>,
    /// Free-form label, e.g. source filename or structure name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub name: Option<String>,
}

impl MoleculeSystem {
    /// Number of atoms in the system.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Whether the system has no atoms.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// The element symbol for atom `i`, or `""` if indices are inconsistent.
    pub fn element_of(&self, i: usize) -> &str {
        self.species_idx
            .get(i)
            .and_then(|&s| self.species.get(s as usize))
            .map(|s| s.element.as_str())
            .unwrap_or("")
    }

    /// Validate internal consistency: array lengths agree, species indices and
    /// bond endpoints are in range. Returns a human-readable error otherwise.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.positions.len();
        if self.species_idx.len() != n {
            return Err(format!(
                "species_idx len {} != positions len {n}",
                self.species_idx.len()
            ));
        }
        if !self.velocities.is_empty() && self.velocities.len() != n {
            return Err(format!(
                "velocities len {} != positions len {n}",
                self.velocities.len()
            ));
        }
        let ns = self.species.len() as u32;
        for (i, &s) in self.species_idx.iter().enumerate() {
            if s >= ns {
                return Err(format!(
                    "atom {i} species_idx {s} out of range (species {ns})"
                ));
            }
        }
        for (i, b) in self.bonds.iter().enumerate() {
            if b.a as usize >= n || b.b as usize >= n {
                return Err(format!("bond {i} endpoint out of range (atoms {n})"));
            }
        }
        Ok(())
    }
}
