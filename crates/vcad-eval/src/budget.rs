//! The budget that bounds boolean batching.
//!
//! The evaluator batches a boolean chain — `A - B - C - …` cut in one
//! difference against `B ∪ C ∪ …`, a stalled union chain reassociated — and
//! abandons the attempt when it costs more than the re-trims it would save.
//! [`Budget`] is what "costs more" means, and every such check goes through
//! it.
//!
//! # Why this is not `Instant::now()`
//!
//! `std::time::Instant::now()` is not implemented on
//! `wasm32-unknown-unknown`: it panics, and a panic in a `wasm_bindgen`
//! export traps the whole module. Calling it anywhere on the evaluation path
//! therefore means every document that reaches a batched chain dies in the
//! browser and in the WASM MCP kernel. The web app hid that for a long time
//! by catching the trap and re-evaluating in TypeScript — correct meshes, no
//! B-rep, none of the kernel's speed — so the only visible symptom was one
//! console line.
//!
//! So the clock is an input, not a call. [`crate::EvalOptions::clock`]
//! already carries one (the WASM bindings pass `performance.now()`), and a
//! caller that supplies none gets:
//!
//! * **natively**, a default monotonic clock backed by `Instant` — the same
//!   wall-clock budget as before, so nothing about native behaviour or
//!   performance changes;
//! * **on `wasm32`**, no clock at all, and the budget falls back to counting
//!   operations instead of milliseconds (see [`BATCH_BUDGET_STEPS`]).
//!
//! There is one code path either way: [`Budget::spend`] answers the same
//! question for both.

use std::cell::Cell;

use crate::Clock;

/// Wall-clock budget for the batched attempt on one boolean chain (ms).
///
/// 20 s. Every chain measured either fuses far inside it (the rana-60-cnc
/// `can`'s thirty tools) or runs orders of magnitude past it (the `rotor`'s
/// twenty extruded sketches, which had not finished in 30 minutes), so the
/// exact figure is not delicate — anything from a few seconds to a minute
/// separates the two populations. It sits at the high end of that range
/// because a batched win is worth waiting for: the `can` goes from 539 s and
/// 247 002 facets to 109 s and 379 analytic faces.
///
/// Override with `VCAD_BATCH_BUDGET_MS` when profiling.
pub const BATCH_BUDGET_MS: f64 = 20_000.0;

/// Budget for one boolean chain when no clock is available, counted in
/// kernel boolean operations rather than milliseconds.
///
/// **What "budget" means here.** A deterministic budget can bound how many
/// merges the batching *attempts*, not how long each one takes: a single
/// `Solid::union` is uninterruptible under either budget (the wall-clock one
/// cannot stop one that has already started either — see
/// [`crate::evaluate`]'s difference-chain notes). So this is a hard cap on
/// attempts, and it is deterministic: the same document batches the same way
/// on every run, which the wall-clock budget cannot promise.
///
/// The cap is 256 charges. Every chain measured that *wins* from batching
/// spends far less — the `can`'s thirty tools cost 29, and a reassociated union chain
/// costs about one per operand per level — so the cap never changes the
/// outcome for a document that was going to batch successfully. It exists to
/// stop a reduction that would otherwise run without bound.
///
/// Override with `VCAD_BATCH_BUDGET_STEPS`.
pub const BATCH_BUDGET_STEPS: u32 = 256;

/// The native default clock. Absent on `wasm32`, where `Instant` traps.
#[cfg(not(target_arch = "wasm32"))]
mod native_clock {
    use crate::Clock;
    use std::sync::OnceLock;
    use std::time::Instant;

    /// Monotonic milliseconds since this process first asked the time.
    ///
    /// The **only** place in this crate allowed to name `Instant`, and it is
    /// compiled out of every `wasm32` build. `tests/no_wall_clock_on_wasm.rs`
    /// fails if another one appears.
    pub(super) struct MonotonicClock;

    impl Clock for MonotonicClock {
        fn now_ms(&self) -> f64 {
            static ORIGIN: OnceLock<Instant> = OnceLock::new();
            ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
        }
    }

    pub(super) static DEFAULT: MonotonicClock = MonotonicClock;
}

/// The clock a caller that supplied none gets.
fn default_clock() -> Option<&'static dyn Clock> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(&native_clock::DEFAULT)
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

fn budget_ms() -> f64 {
    std::env::var("VCAD_BATCH_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|ms| ms.is_finite() && *ms >= 0.0)
        .unwrap_or(BATCH_BUDGET_MS)
}

fn budget_steps() -> u32 {
    std::env::var("VCAD_BATCH_BUDGET_STEPS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(BATCH_BUDGET_STEPS)
}

thread_local! {
    /// Charges attempted on this thread, granted or not.
    static CHARGES: Cell<u64> = const { Cell::new(0) };
    /// Refused charges on this thread, for tests and diagnostics.
    static EXHAUSTIONS: Cell<u64> = const { Cell::new(0) };
}

/// How many times a batching budget has refused a charge on this thread.
///
/// One refusal is one boolean the evaluator declined to attempt, so a
/// non-zero count means at least one chain gave up batching and was cut (or
/// folded) as authored. Thread-local: a test owns its own count.
pub fn exhaustions() -> u64 {
    EXHAUSTIONS.with(|c| c.get())
}

/// How many charges a batching budget has been asked for on this thread,
/// refusals included.
///
/// The other half of [`exhaustions`], and the half that says a chain
/// *did* batch: a refusal count of zero is also what a document that never
/// reached the batching path at all reports, so a test comparing the batched
/// and unbatched results of the same document needs this to know it really
/// compared two paths. Thread-local, like [`exhaustions`].
pub fn charges() -> u64 {
    CHARGES.with(|c| c.get())
}

/// Zero [`exhaustions`] and [`charges`] for this thread.
pub fn reset_exhaustions() {
    CHARGES.with(|c| c.set(0));
    EXHAUSTIONS.with(|c| c.set(0));
}

/// What the evaluator spends when it tries to batch a boolean chain.
///
/// One budget per chain: constructed where the old code wrote
/// `Instant::now() + batch_budget()`, and charged immediately before every
/// kernel boolean the batching attempt would perform.
pub(crate) struct Budget<'a> {
    clock: Option<&'a dyn Clock>,
    /// Only meaningful when `clock` is `Some`.
    deadline_ms: f64,
    /// Only meaningful when `clock` is `None`.
    steps: Cell<u32>,
}

impl<'a> Budget<'a> {
    /// Open a budget for one chain, against the caller's clock if it gave
    /// one and the platform default otherwise.
    pub(crate) fn start(clock: Option<&'a dyn Clock>) -> Self {
        // The closure's return type pins the coercion: the default clock is
        // `&'static`, which outlives any `'a` the caller's clock has.
        let clock = clock.or_else(|| -> Option<&'a dyn Clock> { default_clock() });
        let deadline_ms = match clock {
            Some(c) => c.now_ms() + budget_ms(),
            None => f64::INFINITY,
        };
        Budget {
            clock,
            deadline_ms,
            steps: Cell::new(budget_steps()),
        }
    }

    /// Charge one kernel boolean against the budget.
    ///
    /// `true` when the batching attempt may proceed, `false` when the budget
    /// is spent — in which case the caller must fall back to the authored
    /// chain rather than doing the work anyway.
    pub(crate) fn spend(&self) -> bool {
        CHARGES.with(|c| c.set(c.get() + 1));
        let ok = match self.clock {
            Some(c) => c.now_ms() < self.deadline_ms,
            None => {
                let left = self.steps.get();
                self.steps.set(left.saturating_sub(1));
                left > 0
            }
        };
        if !ok {
            EXHAUSTIONS.with(|c| c.set(c.get() + 1));
        }
        ok
    }

    /// The clock's reading, for a trace line. `None` when there is no clock:
    /// a budget that counts operations cannot report milliseconds, and
    /// inventing a zero would put a wrong number in a diagnostic.
    pub(crate) fn now_ms(&self) -> Option<f64> {
        self.clock.map(|c| c.now_ms())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    /// A clock the test moves by hand. `Clock` is `Send + Sync`, so the
    /// reading lives in an atomic rather than a `Cell`.
    struct Fixed(AtomicU64);

    impl Fixed {
        fn at(ms: f64) -> Self {
            Fixed(AtomicU64::new(ms.to_bits()))
        }
        fn set(&self, ms: f64) {
            self.0.store(ms.to_bits(), Ordering::Relaxed);
        }
    }

    impl Clock for Fixed {
        fn now_ms(&self) -> f64 {
            f64::from_bits(self.0.load(Ordering::Relaxed))
        }
    }

    #[test]
    fn a_clock_budget_runs_out_when_the_clock_passes_the_deadline() {
        reset_exhaustions();
        let clock = Fixed::at(0.0);
        let budget = Budget::start(Some(&clock));
        assert!(budget.spend(), "budget refuses at t=0");
        clock.set(BATCH_BUDGET_MS + 1.0);
        assert!(!budget.spend(), "budget still open past the deadline");
        assert_eq!(exhaustions(), 1);
        assert_eq!(charges(), 2, "a refused charge is still a charge");
    }

    #[test]
    fn a_clockless_budget_allows_exactly_its_step_count() {
        // The `wasm32`-with-no-clock shape. `start` cannot produce it here —
        // natively the default clock always fills in — so the clockless
        // budget is built directly, with a short ladder to keep it readable.
        let budget = Budget {
            clock: None,
            deadline_ms: f64::INFINITY,
            steps: Cell::new(3),
        };
        reset_exhaustions();
        assert!(budget.spend() && budget.spend() && budget.spend());
        assert!(
            !budget.spend(),
            "a fourth boolean got through a 3-step budget"
        );
        assert!(!budget.spend(), "an exhausted budget re-opened");
        assert_eq!(exhaustions(), 2);
    }

    #[test]
    fn the_native_default_clock_advances() {
        let budget = Budget::start(None);
        let t0 = budget.now_ms().expect("native builds have a default clock");
        let t1 = budget.now_ms().expect("native builds have a default clock");
        assert!(t1 >= t0, "default clock went backwards: {t0} -> {t1}");
        assert!(
            budget.spend(),
            "a fresh 20 s budget refused its first charge"
        );
    }
}
