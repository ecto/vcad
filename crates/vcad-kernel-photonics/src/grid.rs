//! Uniform rectangular grid and flat `f64` field storage.
//!
//! The Yee staggering itself lives in [`crate::sim`]; this module only
//! provides the domain description ([`GridSpec`]) and a dense row-major
//! array type ([`Field2`]) used for every field, permittivity, and CPML
//! ψ accumulator in the crate.

/// The rectangular simulation domain: `nx × ny` square cells of side
/// `delta`, spanning `[0, nx·delta] × [0, ny·delta]`.
///
/// Sample positions (Yee staggering, per polarization) are documented on
/// [`crate::sim::Simulation`]; integer node `(i, j)` sits at
/// `(i·delta, j·delta)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpec {
    /// Cell count along x.
    pub nx: usize,
    /// Cell count along y.
    pub ny: usize,
    /// Cell side (grid pitch), in length units. `dx = dy = delta`.
    pub delta: f64,
}

impl GridSpec {
    /// Create a grid spec. Panics on zero cells or non-positive pitch.
    pub fn new(nx: usize, ny: usize, delta: f64) -> Self {
        assert!(nx > 0 && ny > 0, "grid must have at least one cell");
        assert!(delta > 0.0, "grid pitch must be positive");
        Self { nx, ny, delta }
    }

    /// Domain extent along x.
    pub fn lx(&self) -> f64 {
        self.nx as f64 * self.delta
    }

    /// Domain extent along y.
    pub fn ly(&self) -> f64 {
        self.ny as f64 * self.delta
    }
}

/// A dense row-major 2D `f64` array: `ns_x × ns_y` samples, `y` fastest.
///
/// "Samples", not "cells" — different Yee components have different sample
/// counts on the same grid (e.g. Ez has `(nx+1) × (ny+1)` nodes).
#[derive(Debug, Clone, PartialEq)]
pub struct Field2 {
    ns_x: usize,
    ns_y: usize,
    data: Vec<f64>,
}

impl Field2 {
    /// Allocate a zero-filled field of `ns_x × ns_y` samples.
    pub fn new(ns_x: usize, ns_y: usize) -> Self {
        Self {
            ns_x,
            ns_y,
            data: vec![0.0; ns_x * ns_y],
        }
    }

    /// Allocate a constant-filled field.
    pub fn filled(ns_x: usize, ns_y: usize, value: f64) -> Self {
        Self {
            ns_x,
            ns_y,
            data: vec![value; ns_x * ns_y],
        }
    }

    /// Sample count along x.
    pub fn ns_x(&self) -> usize {
        self.ns_x
    }

    /// Sample count along y.
    pub fn ns_y(&self) -> usize {
        self.ns_y
    }

    /// Flat index of sample `(i, j)`.
    #[inline]
    pub fn idx(&self, i: usize, j: usize) -> usize {
        debug_assert!(i < self.ns_x && j < self.ns_y);
        i * self.ns_y + j
    }

    /// Read sample `(i, j)`.
    #[inline]
    pub fn at(&self, i: usize, j: usize) -> f64 {
        self.data[self.idx(i, j)]
    }

    /// Mutable sample `(i, j)`.
    #[inline]
    pub fn at_mut(&mut self, i: usize, j: usize) -> &mut f64 {
        let k = self.idx(i, j);
        &mut self.data[k]
    }

    /// The whole array, row-major (`y` fastest).
    pub fn as_slice(&self) -> &[f64] {
        &self.data
    }

    /// The whole array, mutable.
    pub fn as_mut_slice(&mut self) -> &mut [f64] {
        &mut self.data
    }

    /// Set every sample to `value`.
    pub fn fill(&mut self, value: f64) {
        self.data.fill(value);
    }

    /// Sum of `self[i]·other[i]` (plain ℓ², no cell-volume weights).
    pub fn dot(&self, other: &Field2) -> f64 {
        assert_eq!(self.ns_x, other.ns_x);
        assert_eq!(self.ns_y, other.ns_y);
        self.data
            .iter()
            .zip(other.data.iter())
            .map(|(a, b)| a * b)
            .sum()
    }

    /// Sum of squares of all samples.
    pub fn norm2(&self) -> f64 {
        self.data.iter().map(|v| v * v).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_extents() {
        let g = GridSpec::new(10, 20, 0.5);
        assert_eq!(g.lx(), 5.0);
        assert_eq!(g.ly(), 10.0);
    }

    #[test]
    fn field_indexing_round_trip() {
        let mut f = Field2::new(3, 4);
        *f.at_mut(2, 3) = 7.5;
        *f.at_mut(0, 1) = -2.0;
        assert_eq!(f.at(2, 3), 7.5);
        assert_eq!(f.at(0, 1), -2.0);
        assert_eq!(f.as_slice().len(), 12);
        assert_eq!(f.at(1, 0), 0.0);
    }

    #[test]
    fn dot_and_norm() {
        let mut a = Field2::new(2, 2);
        let mut b = Field2::new(2, 2);
        *a.at_mut(0, 0) = 2.0;
        *b.at_mut(0, 0) = 3.0;
        *a.at_mut(1, 1) = -1.0;
        *b.at_mut(1, 1) = 5.0;
        assert_eq!(a.dot(&b), 1.0);
        assert_eq!(a.norm2(), 5.0);
    }
}
