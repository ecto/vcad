//! The z×z cross-term ledger.
//!
//! For z coupled disciplines the discrete adjoint system is a z×z block
//! matrix. Block `(j, k)` is `(∂G(j)/∂u(k))ᵀ` — how discipline *j*'s
//! update responds to discipline *k*'s state. The diagonal is each
//! discipline's own adjoint; the off-diagonal blocks are the coupling,
//! and they are the ones that get silently dropped.
//!
//! This module makes the z² blocks an explicit, serializable object with
//! four possible states. Three of them are honest and one of them
//! poisons the gradient:
//!
//! | status | meaning | effect on the roll-up |
//! |---|---|---|
//! | [`BlockStatus::Absent`] | no interface exists; the block is structurally zero | none |
//! | [`BlockStatus::Implemented`] | registered and applied, by a stated method | none |
//! | [`BlockStatus::Frozen`] | deliberately dropped, assumption named, error bounded | downgrades to [`Completeness::Bounded`] |
//! | [`BlockStatus::Missing`] | known nonzero, not implemented | downgrades to [`Completeness::Incomplete`] |
//!
//! `Frozen` is the interesting one. SU2's slides spell out, for each
//! neglected term, the physical assumption it encodes — *"a change in
//! the flow-field temperature leaves interface heat fluxes
//! unchanged"* — which is the right discipline, but as prose it cannot
//! be checked or propagated. Here the assumption is a string on a
//! variant that also carries the relative error bound it costs, and the
//! bound flows into the receipt.

use serde::{Deserialize, Serialize};
use vcad_receipt::{ClaimBasis, ClaimVerdict};

/// How a Jacobian block is computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockMethod {
    /// Hand-derived transpose, exact to machine precision.
    Analytic,
    /// Dual numbers / forward-mode carried through the block.
    Dual,
    /// Reverse-mode tape.
    Tape,
    /// Directional finite differences on an opaque discipline. Legitimate
    /// when the block is low-rank (so the probe count is small and fixed)
    /// — but its accuracy is set by step size, not by the machine.
    FiniteDifference,
}

impl BlockMethod {
    /// Whether the method is exact up to round-off.
    pub fn is_exact(self) -> bool {
        !matches!(self, BlockMethod::FiniteDifference)
    }

    /// Human label.
    pub fn label(self) -> &'static str {
        match self {
            BlockMethod::Analytic => "analytic",
            BlockMethod::Dual => "dual",
            BlockMethod::Tape => "tape",
            BlockMethod::FiniteDifference => "finite-difference",
        }
    }
}

/// The state of one Jacobian block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BlockStatus {
    /// No interface couples these two disciplines: the block is
    /// structurally zero, and omitting it costs nothing.
    Absent,
    /// Implemented and registered with the driver.
    Implemented {
        /// How it is computed.
        method: BlockMethod,
    },
    /// Deliberately not implemented, with the physical assumption that
    /// justifies dropping it stated in full.
    Frozen {
        /// The assumption the omission encodes. Write it as a claim about
        /// physics that a reviewer could disagree with, not as an
        /// apology: *"a change in wall temperature leaves the film
        /// coefficient unchanged"*.
        assumption: String,
        /// Bound on the relative error this omission introduces into the
        /// gradient, if one has been established (by ablation, by an
        /// asymptotic argument). `None` means unbounded — which is not a
        /// bounded approximation, it is a hole, and it rolls up as
        /// [`Completeness::Incomplete`].
        relative_bound: Option<f64>,
    },
    /// The block is known to be nonzero and is not implemented. Any
    /// gradient computed through this ledger is wrong by an unknown
    /// amount.
    Missing {
        /// Why it is missing, and what would be needed to close it.
        note: String,
    },
}

impl BlockStatus {
    /// A frozen block with a named assumption and no established bound.
    pub fn frozen(assumption: impl Into<String>) -> Self {
        BlockStatus::Frozen {
            assumption: assumption.into(),
            relative_bound: None,
        }
    }

    /// A frozen block whose relative error contribution is bounded.
    pub fn frozen_bounded(assumption: impl Into<String>, relative_bound: f64) -> Self {
        BlockStatus::Frozen {
            assumption: assumption.into(),
            relative_bound: Some(relative_bound),
        }
    }

    /// An implemented block.
    pub fn implemented(method: BlockMethod) -> Self {
        BlockStatus::Implemented { method }
    }

    /// A missing block.
    pub fn missing(note: impl Into<String>) -> Self {
        BlockStatus::Missing { note: note.into() }
    }

    /// Whether the driver should expect a registered [`crate::CrossTerm`]
    /// for this block.
    pub fn expects_registration(&self) -> bool {
        matches!(self, BlockStatus::Implemented { .. })
    }
}

/// How complete a coupled gradient is.
///
/// This is the roll-up the receipt consumes. It is deliberately
/// three-state and deliberately pessimistic: anything short of every
/// nonzero block implemented is *not* a clean gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "completeness", rename_all = "snake_case")]
pub enum Completeness {
    /// Every non-absent block is implemented. The gradient is exact up to
    /// the accuracy of the methods used.
    Complete {
        /// True when every implemented block uses an exact method. False
        /// when at least one block is finite-differenced, in which case
        /// the gradient is exact in *structure* but carries step-size
        /// error.
        all_exact: bool,
    },
    /// Some blocks are frozen, and every frozen block carries a bound.
    /// The gradient is an approximation whose error is bounded by the sum
    /// of the individual bounds (a triangle-inequality bound — loose, but
    /// it is a bound).
    Bounded {
        /// Sum of the frozen blocks' relative bounds.
        relative_bound: f64,
        /// The frozen blocks, as `(source, target, assumption)`.
        frozen: Vec<(String, String, String)>,
    },
    /// At least one block is missing, or frozen without a bound. The
    /// gradient's error is unknown. It may have the wrong sign.
    Incomplete {
        /// One line per offending block.
        reasons: Vec<String>,
    },
}

impl Completeness {
    /// The strongest [`ClaimBasis`] a gradient with this completeness may
    /// carry.
    ///
    /// A complete, all-exact gradient may be `Verified` — an oracle
    /// really ran and the answer is the answer. A bounded approximation
    /// is `Predicted`: good enough to steer a design, not to ship a
    /// claim on. Incomplete does not get a basis at all, because it does
    /// not get a passing verdict.
    pub fn max_basis(&self) -> ClaimBasis {
        match self {
            Completeness::Complete { all_exact: true } => ClaimBasis::Verified,
            Completeness::Complete { all_exact: false } => ClaimBasis::Predicted,
            Completeness::Bounded { .. } => ClaimBasis::Predicted,
            Completeness::Incomplete { .. } => ClaimBasis::Predicted,
        }
    }

    /// The verdict a sensitivity claim resting on this gradient may carry.
    ///
    /// [`ClaimVerdict::Unverifiable`] for an incomplete gradient — "the
    /// oracle could not check the claim", which is exactly the situation:
    /// the number exists, but nothing establishes it is the derivative.
    pub fn verdict(&self) -> ClaimVerdict {
        match self {
            Completeness::Incomplete { .. } => ClaimVerdict::Unverifiable,
            _ => ClaimVerdict::Pass,
        }
    }

    /// Whether a gradient with this completeness may steer an optimizer.
    ///
    /// Bounded approximations may — that is what a bound is for. An
    /// incomplete gradient may not: SU2's three-physics case returned the
    /// wrong sign, and a line search on a wrong-signed gradient does not
    /// fail loudly, it just diverges.
    pub fn may_optimize(&self) -> bool {
        !matches!(self, Completeness::Incomplete { .. })
    }

    /// One-line human summary.
    pub fn summary(&self) -> String {
        match self {
            Completeness::Complete { all_exact: true } => "complete (all blocks exact)".into(),
            Completeness::Complete { all_exact: false } => {
                "complete (some blocks finite-differenced)".into()
            }
            Completeness::Bounded {
                relative_bound,
                frozen,
            } => format!(
                "bounded approximation: {} frozen block(s), error <= {:.1}%",
                frozen.len(),
                relative_bound * 100.0
            ),
            Completeness::Incomplete { reasons } => {
                format!("INCOMPLETE: {}", reasons.join("; "))
            }
        }
    }
}

/// Errors from ledger construction and lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum LedgerError {
    /// Fewer than two disciplines: there is nothing to couple.
    TooFewDisciplines(usize),
    /// A discipline name appeared twice.
    DuplicateDiscipline(String),
    /// Block index out of range.
    OutOfRange {
        /// Source discipline index.
        source: usize,
        /// Target discipline index.
        target: usize,
        /// Discipline count.
        z: usize,
    },
    /// A diagonal block was set to something other than `Implemented`. A
    /// discipline that cannot differentiate itself has no business in a
    /// coupled system.
    DiagonalNotImplemented(String),
}

impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LedgerError::TooFewDisciplines(z) => {
                write!(f, "a coupling ledger needs at least 2 disciplines, got {z}")
            }
            LedgerError::DuplicateDiscipline(n) => write!(f, "duplicate discipline name {n:?}"),
            LedgerError::OutOfRange { source, target, z } => write!(
                f,
                "block ({source}, {target}) out of range for {z} disciplines"
            ),
            LedgerError::DiagonalNotImplemented(n) => write!(
                f,
                "discipline {n:?} does not implement its own adjoint (diagonal block)"
            ),
        }
    }
}

impl std::error::Error for LedgerError {}

/// The z×z ledger of Jacobian blocks.
///
/// Block `(source, target)` is `(∂G(source)/∂u(target))ᵀ` — the term that
/// discipline `source`'s adjoint contributes to discipline `target`'s
/// right-hand side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CouplingLedger {
    /// Schema id.
    pub schema: String,
    /// Discipline names, in index order.
    pub disciplines: Vec<String>,
    /// Row-major z×z blocks: `blocks[source * z + target]`.
    pub blocks: Vec<BlockStatus>,
}

impl CouplingLedger {
    /// A ledger over the named disciplines, every off-diagonal block
    /// initialized to [`BlockStatus::Absent`] and every diagonal block to
    /// [`BlockStatus::Implemented`] with `method`.
    ///
    /// Start from `Absent` and declare what couples, rather than starting
    /// from `Missing` and declaring what does not: an interface you forgot
    /// to think about should show up as a *wrong* ledger under ablation,
    /// not as a permanently red one that everybody learns to ignore.
    pub fn new(
        disciplines: impl IntoIterator<Item = impl Into<String>>,
        diagonal: BlockMethod,
    ) -> Result<Self, LedgerError> {
        let names: Vec<String> = disciplines.into_iter().map(Into::into).collect();
        if names.len() < 2 {
            return Err(LedgerError::TooFewDisciplines(names.len()));
        }
        for (i, n) in names.iter().enumerate() {
            if names[..i].contains(n) {
                return Err(LedgerError::DuplicateDiscipline(n.clone()));
            }
        }
        let z = names.len();
        let mut blocks = vec![BlockStatus::Absent; z * z];
        for k in 0..z {
            blocks[k * z + k] = BlockStatus::Implemented { method: diagonal };
        }
        Ok(CouplingLedger {
            schema: crate::LEDGER_SCHEMA.to_string(),
            disciplines: names,
            blocks,
        })
    }

    /// Number of disciplines.
    pub fn len(&self) -> usize {
        self.disciplines.len()
    }

    /// Whether the ledger is empty (never true for a constructed ledger).
    pub fn is_empty(&self) -> bool {
        self.disciplines.is_empty()
    }

    /// Index of a discipline by name.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.disciplines.iter().position(|d| d == name)
    }

    /// Set block `(source, target)`.
    pub fn set(
        &mut self,
        source: usize,
        target: usize,
        status: BlockStatus,
    ) -> Result<&mut Self, LedgerError> {
        let z = self.len();
        if source >= z || target >= z {
            return Err(LedgerError::OutOfRange { source, target, z });
        }
        if source == target && !status.expects_registration() {
            return Err(LedgerError::DiagonalNotImplemented(
                self.disciplines[source].clone(),
            ));
        }
        self.blocks[source * z + target] = status;
        Ok(self)
    }

    /// Read block `(source, target)`.
    pub fn get(&self, source: usize, target: usize) -> Option<&BlockStatus> {
        let z = self.len();
        if source >= z || target >= z {
            return None;
        }
        Some(&self.blocks[source * z + target])
    }

    /// Every off-diagonal block, as `(source, target, status)`.
    pub fn cross_blocks(&self) -> impl Iterator<Item = (usize, usize, &BlockStatus)> {
        let z = self.len();
        (0..z).flat_map(move |s| {
            (0..z).filter_map(move |t| {
                if s == t {
                    None
                } else {
                    Some((s, t, &self.blocks[s * z + t]))
                }
            })
        })
    }

    /// Roll the ledger up to a [`Completeness`].
    pub fn completeness(&self) -> Completeness {
        let mut reasons = Vec::new();
        let mut frozen = Vec::new();
        let mut bound = 0.0_f64;
        let mut all_exact = true;

        for (s, t, status) in self.cross_blocks() {
            let (sn, tn) = (&self.disciplines[s], &self.disciplines[t]);
            match status {
                BlockStatus::Absent => {}
                BlockStatus::Implemented { method } => {
                    if !method.is_exact() {
                        all_exact = false;
                    }
                }
                BlockStatus::Frozen {
                    assumption,
                    relative_bound,
                } => match relative_bound {
                    Some(b) => {
                        bound += b.abs();
                        frozen.push((sn.clone(), tn.clone(), assumption.clone()));
                    }
                    None => reasons.push(format!(
                        "d{sn}/d{tn} frozen with no established bound ({assumption})"
                    )),
                },
                BlockStatus::Missing { note } => {
                    reasons.push(format!("d{sn}/d{tn} missing ({note})"))
                }
            }
        }
        // Diagonal methods count toward exactness too.
        for k in 0..self.len() {
            if let Some(BlockStatus::Implemented { method }) = self.get(k, k) {
                if !method.is_exact() {
                    all_exact = false;
                }
            }
        }

        if !reasons.is_empty() {
            return Completeness::Incomplete { reasons };
        }
        if !frozen.is_empty() {
            return Completeness::Bounded {
                relative_bound: bound,
                frozen,
            };
        }
        Completeness::Complete { all_exact }
    }

    /// Render the ledger as a fixed-width matrix for logs and receipts.
    ///
    /// Rows are sources, columns are targets; the cell glyph is `=` for a
    /// diagonal, `#` implemented, `~` frozen, `.` absent, `!` missing.
    pub fn render(&self) -> String {
        let z = self.len();
        let w = self.disciplines.iter().map(|d| d.len()).max().unwrap_or(1);
        let mut out = String::new();
        out.push_str(&format!("{:w$} | targets\n", "source", w = w));
        for s in 0..z {
            out.push_str(&format!("{:w$} | ", self.disciplines[s], w = w));
            for t in 0..z {
                out.push(match (s == t, &self.blocks[s * z + t]) {
                    (true, _) => '=',
                    (_, BlockStatus::Implemented { .. }) => '#',
                    (_, BlockStatus::Frozen { .. }) => '~',
                    (_, BlockStatus::Absent) => '.',
                    (_, BlockStatus::Missing { .. }) => '!',
                });
                out.push(' ');
            }
            out.push('\n');
        }
        out.push_str(&format!("{}\n", self.completeness().summary()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> CouplingLedger {
        CouplingLedger::new(["flow", "thermal"], BlockMethod::Analytic).unwrap()
    }

    #[test]
    fn fresh_ledger_is_complete_but_uncoupled() {
        let l = two();
        // Absent everywhere off-diagonal: structurally uncoupled, which is
        // a complete description of an uncoupled system.
        assert_eq!(l.completeness(), Completeness::Complete { all_exact: true });
    }

    #[test]
    fn a_missing_block_poisons_the_rollup() {
        let mut l = two();
        l.set(0, 1, BlockStatus::missing("no thermal-lattice adjoint"))
            .unwrap();
        let c = l.completeness();
        assert!(matches!(c, Completeness::Incomplete { .. }));
        assert_eq!(c.verdict(), ClaimVerdict::Unverifiable);
        assert!(!c.may_optimize(), "an incomplete gradient must not steer");
    }

    #[test]
    fn frozen_without_a_bound_is_a_hole_not_an_approximation() {
        let mut l = two();
        l.set(
            0,
            1,
            BlockStatus::frozen("wall temp does not move the film"),
        )
        .unwrap();
        assert!(matches!(l.completeness(), Completeness::Incomplete { .. }));
    }

    #[test]
    fn frozen_with_a_bound_is_a_bounded_approximation() {
        let mut l = two();
        l.set(
            0,
            1,
            BlockStatus::frozen_bounded("wall temp does not move the film", 0.02),
        )
        .unwrap();
        let c = l.completeness();
        match &c {
            Completeness::Bounded {
                relative_bound,
                frozen,
            } => {
                assert!((relative_bound - 0.02).abs() < 1e-12);
                assert_eq!(frozen.len(), 1);
            }
            other => panic!("expected bounded, got {other:?}"),
        }
        // Bounded may steer, but must never claim Verified.
        assert!(c.may_optimize());
        assert_eq!(c.max_basis(), ClaimBasis::Predicted);
    }

    #[test]
    fn bounds_accumulate_across_frozen_blocks() {
        let mut l = two();
        l.set(0, 1, BlockStatus::frozen_bounded("a", 0.02)).unwrap();
        l.set(1, 0, BlockStatus::frozen_bounded("b", 0.05)).unwrap();
        match l.completeness() {
            Completeness::Bounded { relative_bound, .. } => {
                assert!((relative_bound - 0.07).abs() < 1e-12)
            }
            other => panic!("expected bounded, got {other:?}"),
        }
    }

    #[test]
    fn a_finite_differenced_block_is_complete_but_not_exact() {
        let mut l = two();
        l.set(
            0,
            1,
            BlockStatus::implemented(BlockMethod::FiniteDifference),
        )
        .unwrap();
        let c = l.completeness();
        assert_eq!(c, Completeness::Complete { all_exact: false });
        // Structure is complete, arithmetic is not exact -> Predicted.
        assert_eq!(c.max_basis(), ClaimBasis::Predicted);
        assert!(c.may_optimize());
    }

    #[test]
    fn diagonal_cannot_be_dropped() {
        let mut l = two();
        assert!(matches!(
            l.set(0, 0, BlockStatus::missing("x")),
            Err(LedgerError::DiagonalNotImplemented(_))
        ));
    }

    #[test]
    fn duplicate_and_undersized_ledgers_are_refused() {
        assert!(matches!(
            CouplingLedger::new(["a", "a"], BlockMethod::Analytic),
            Err(LedgerError::DuplicateDiscipline(_))
        ));
        assert!(matches!(
            CouplingLedger::new(["a"], BlockMethod::Analytic),
            Err(LedgerError::TooFewDisciplines(1))
        ));
    }

    #[test]
    fn render_is_a_readable_matrix() {
        let mut l =
            CouplingLedger::new(["flow", "thermal", "solid"], BlockMethod::Analytic).unwrap();
        l.set(0, 1, BlockStatus::implemented(BlockMethod::Analytic))
            .unwrap();
        l.set(
            1,
            0,
            BlockStatus::implemented(BlockMethod::FiniteDifference),
        )
        .unwrap();
        l.set(2, 0, BlockStatus::missing("no FSI adjoint")).unwrap();
        let r = l.render();
        assert!(r.contains('#') && r.contains('!') && r.contains('='));
        assert!(r.contains("INCOMPLETE"));
    }

    #[test]
    fn ledger_round_trips_through_json() {
        let mut l = two();
        l.set(0, 1, BlockStatus::frozen_bounded("a", 0.02)).unwrap();
        let s = serde_json::to_string(&l).unwrap();
        let back: CouplingLedger = serde_json::from_str(&s).unwrap();
        assert_eq!(l, back);
        assert!(s.contains("vcad.coupling-ledger/1"));
    }
}
