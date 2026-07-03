//! Structure builders: lattices and simple molecules, as data. These are the
//! atomic-domain analog of primitive CAD operations and the natural things a
//! parametric design parameter drives (lattice constant, supercell size).

use vcad_ir::molecule::{Cell, MoleculeSystem, Species};

use crate::element;

/// Build a monatomic FCC crystal supercell of the given element.
///
/// `a` is the conventional cubic lattice constant (Å); `nx/ny/nz` are the
/// number of conventional cells along each axis. The result is periodic.
pub fn fcc(element_symbol: &str, a: f64, nx: usize, ny: usize, nz: usize) -> MoleculeSystem {
    let data = element::lookup(element_symbol);
    let basis = [
        [0.0, 0.0, 0.0],
        [0.5, 0.5, 0.0],
        [0.5, 0.0, 0.5],
        [0.0, 0.5, 0.5],
    ];
    let mut positions = Vec::with_capacity(nx * ny * nz * 4);
    for ix in 0..nx {
        for iy in 0..ny {
            for iz in 0..nz {
                for b in &basis {
                    positions.push([
                        (ix as f64 + b[0]) * a,
                        (iy as f64 + b[1]) * a,
                        (iz as f64 + b[2]) * a,
                    ]);
                }
            }
        }
    }
    let n = positions.len();
    MoleculeSystem {
        species: vec![Species {
            element: element_symbol.to_string(),
            atomic_number: data.number,
            mass: data.mass,
            charge: 0.0,
            label: None,
            radius: None,
            color: None,
        }],
        positions,
        species_idx: vec![0; n],
        velocities: Vec::new(),
        bonds: Vec::new(),
        cell: Some(Cell {
            a: [a * nx as f64, 0.0, 0.0],
            b: [0.0, a * ny as f64, 0.0],
            c: [0.0, 0.0, a * nz as f64],
            periodic: [true, true, true],
        }),
        name: Some(format!("{element_symbol} FCC {nx}x{ny}x{nz}")),
    }
}

/// Build a diatomic molecule of `element_symbol` separated by `distance` Å along
/// x, with one bond. Non-periodic.
pub fn diatomic(element_symbol: &str, distance: f64) -> MoleculeSystem {
    use vcad_ir::molecule::Bond;
    let data = element::lookup(element_symbol);
    MoleculeSystem {
        species: vec![Species {
            element: element_symbol.to_string(),
            atomic_number: data.number,
            mass: data.mass,
            charge: 0.0,
            label: None,
            radius: None,
            color: None,
        }],
        positions: vec![[0.0, 0.0, 0.0], [distance, 0.0, 0.0]],
        species_idx: vec![0, 0],
        velocities: Vec::new(),
        bonds: vec![Bond {
            a: 0,
            b: 1,
            order: 1.0,
        }],
        cell: None,
        name: Some(format!("{element_symbol}2")),
    }
}
