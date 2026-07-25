//! The gauge-group abstraction: everything the lattice, updates, and
//! observables need from a compact group in its fundamental
//! representation.
//!
//! Two implementors: [`crate::su2::Su2`] (quaternions) and
//! [`crate::su3::Su3`] (3×3 complex matrices, Cabibbo–Marinari
//! updates). The same struct with non-unit "norm" doubles as a staple
//! accumulator in both cases — sums of group elements are what the
//! heatbath algorithms consume.
//!
//! Convention: the Wilson action is `S = β Σ_p (1 − (1/N)Re Tr U_p)`,
//! so the link-local weight is `exp((β/N)·Re Tr(U·A))` with `A` the
//! staple sum. Group-specific update rules ([`GaugeGroup::heatbath`],
//! [`GaugeGroup::overrelax`], [`GaugeGroup::cool`]) own that convention
//! internally.

use crate::rng::Rng;

/// A compact gauge group element (or a linear combination of elements,
/// for staple accumulation).
pub trait GaugeGroup: Copy + Clone + PartialEq + std::fmt::Debug + Send + Sync + 'static {
    /// Fundamental-representation dimension N (2 or 3).
    const NC: usize;

    /// The identity element.
    fn identity() -> Self;

    /// The additive zero (staple accumulator start).
    fn zero() -> Self;

    /// Group (matrix) product.
    fn mul(&self, o: &Self) -> Self;

    /// Hermitian conjugate.
    fn dagger(&self) -> Self;

    /// Componentwise sum (leaves the group; staple accumulation).
    fn add(&self, o: &Self) -> Self;

    /// Componentwise real scaling (leaves the group; smearing).
    fn scale(&self, s: f64) -> Self;

    /// `Re Tr U`.
    fn re_trace(&self) -> f64;

    /// Normalized trace `(1/N)Re Tr U` — the plaquette/loop observable.
    fn norm_trace(&self) -> f64 {
        self.re_trace() / Self::NC as f64
    }

    /// Normalized imaginary trace `(1/N)Im Tr U`. Identically 0 for
    /// SU(2) (real characters); the Z₃ phase content for SU(3).
    fn norm_trace_im(&self) -> f64 {
        0.0
    }

    /// Project back onto the group (unit-normalize / Gram–Schmidt).
    /// Also used to turn a staple-sum direction into a group element.
    fn reunitarize(&self) -> Self;

    /// Haar-distributed (or well-mixed, for SU(3)) random element.
    fn random(rng: &mut Rng) -> Self;

    /// Draw a new link from the local conditional
    /// `P(U) ∝ exp((β/N)Re Tr(U·A))` given the current link `u` and
    /// staple sum `a`. Exact for SU(2) (Kennedy–Pendleton); one
    /// Cabibbo–Marinari subgroup cycle for SU(3).
    fn heatbath(u: &Self, a: &Self, beta: f64, rng: &mut Rng) -> Self;

    /// Microcanonical overrelaxation: an action-preserving reflection
    /// of `u` about the staple direction.
    fn overrelax(u: &Self, a: &Self, rng: &mut Rng) -> Self;

    /// The link maximizing the local action given staple sum `a`
    /// (cooling step). For SU(2) this is exactly `Ā†`.
    fn cool(u: &Self, a: &Self) -> Self;
}
