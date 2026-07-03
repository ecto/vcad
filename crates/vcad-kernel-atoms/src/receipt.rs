//! Reproducibility receipts — M6.
//!
//! A simulation is only scientifically useful if someone else can re-run it and
//! get the same number. A [`SimReceipt`] captures the inputs (structure hash,
//! force-field descriptor, run parameters) and the resulting scalar outputs,
//! plus a content hash over all of it. `verify` re-hashes a fresh receipt and
//! checks it against a stored one — the atomic-domain analog of the ECAD
//! `build_receipt` / `verify_receipt` flow.

use serde::{Deserialize, Serialize};
use vcad_ir::molecule::MoleculeSystem;

/// A reproducible record of one simulation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimReceipt {
    /// Stable hash of the input structure (species + positions + cell).
    pub structure_hash: u64,
    /// Human-readable force-field descriptor (e.g. "LJ(argon)+coulomb").
    pub force_field: String,
    /// Run kind, e.g. "minimize" or "md".
    pub run: String,
    /// Free-form run parameters (dt, steps, tolerances…).
    pub params: serde_json::Value,
    /// Scalar outputs keyed by name (energy, temperature, rmsd…).
    pub outputs: Vec<(String, f64)>,
    /// Content hash over every field above; the reproducibility fingerprint.
    pub digest: u64,
}

/// FNV-1a 64-bit hash — small, dependency-free, stable across runs/platforms.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Canonical hash of a structure: species table, positions (rounded to 1e-6 Å
/// to be robust to formatting), and cell.
pub fn hash_structure(mol: &MoleculeSystem) -> u64 {
    let mut buf: Vec<u8> = Vec::new();
    for s in &mol.species {
        buf.extend_from_slice(s.element.as_bytes());
        buf.extend_from_slice(&s.atomic_number.to_le_bytes());
        buf.extend_from_slice(&s.charge.to_le_bytes());
    }
    for (i, p) in mol.positions.iter().enumerate() {
        buf.extend_from_slice(&mol.species_idx[i].to_le_bytes());
        for &c in p {
            let q = (c / 1e-6).round() as i64;
            buf.extend_from_slice(&q.to_le_bytes());
        }
    }
    if let Some(c) = &mol.cell {
        for v in [c.a, c.b, c.c] {
            for &x in &v {
                let q = (x / 1e-6).round() as i64;
                buf.extend_from_slice(&q.to_le_bytes());
            }
        }
    }
    fnv1a(&buf)
}

impl SimReceipt {
    /// Build a receipt, computing the content digest over all fields.
    pub fn build(
        mol: &MoleculeSystem,
        force_field: impl Into<String>,
        run: impl Into<String>,
        params: serde_json::Value,
        outputs: Vec<(String, f64)>,
    ) -> Self {
        let structure_hash = hash_structure(mol);
        let force_field = force_field.into();
        let run = run.into();
        let mut r = Self {
            structure_hash,
            force_field,
            run,
            params,
            outputs,
            digest: 0,
        };
        r.digest = r.compute_digest();
        r
    }

    fn compute_digest(&self) -> u64 {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&self.structure_hash.to_le_bytes());
        buf.extend_from_slice(self.force_field.as_bytes());
        buf.extend_from_slice(self.run.as_bytes());
        buf.extend_from_slice(self.params.to_string().as_bytes());
        for (k, v) in &self.outputs {
            buf.extend_from_slice(k.as_bytes());
            // Round outputs to 1e-9 so bit-level float noise doesn't break
            // reproducibility across platforms.
            let q = (*v / 1e-9).round() as i64;
            buf.extend_from_slice(&q.to_le_bytes());
        }
        fnv1a(&buf)
    }

    /// Verify this receipt's digest is self-consistent (not tampered).
    pub fn verify_self(&self) -> bool {
        self.digest == self.compute_digest()
    }

    /// Verify a freshly-produced receipt reproduces a stored one.
    pub fn verify_against(&self, stored: &SimReceipt) -> bool {
        self.digest == stored.digest
    }
}
