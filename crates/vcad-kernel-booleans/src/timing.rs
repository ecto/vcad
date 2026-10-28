//! Opt-in wall-time accounting for boolean operations.
//!
//! A CLI's `--timings` breakdown wants to say how much of an evaluation was
//! spent in booleans, and how much of that in the mesh-boolean fallback
//! (the Difference re-cut and the soup/faceted-operand paths). Booleans nest
//! inside the evaluator's per-node timing, so they cannot be recovered from
//! it; the kernel counts them itself. Off by default — [`set_enabled`] turns
//! it on — so the hot path pays one relaxed load, and wasm32 (where
//! `Instant::now` panics) never reads a clock.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static BOOLEAN_N: AtomicU64 = AtomicU64::new(0);
static BOOLEAN_NS: AtomicU64 = AtomicU64::new(0);
static FALLBACK_N: AtomicU64 = AtomicU64::new(0);
static FALLBACK_NS: AtomicU64 = AtomicU64::new(0);

/// Start (or stop) accounting boolean wall time.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Totals since the last [`take`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BooleanTiming {
    /// Top-level `boolean_op_reported` calls.
    pub booleans: u64,
    /// Wall time inside them, in milliseconds (fallbacks included).
    pub boolean_ms: f64,
    /// Mesh-boolean fallback runs (re-cuts, soup and faceted operands).
    pub fallbacks: u64,
    /// Wall time inside the fallback, in milliseconds.
    pub fallback_ms: f64,
}

/// The totals since the last call, and reset them.
pub fn take() -> BooleanTiming {
    let ms = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / 1e6;
    BooleanTiming {
        booleans: BOOLEAN_N.swap(0, Ordering::Relaxed),
        boolean_ms: ms(&BOOLEAN_NS),
        fallbacks: FALLBACK_N.swap(0, Ordering::Relaxed),
        fallback_ms: ms(&FALLBACK_NS),
    }
}

/// Which counter a [`timed`] call charges.
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Boolean,
    Fallback,
}

/// Run `f`, charging its wall time to `kind` when accounting is on.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn timed<T>(kind: Kind, f: impl FnOnce() -> T) -> T {
    thread_local! {
        static ACTIVE: [std::cell::Cell<bool>; 2] =
            const { [std::cell::Cell::new(false), std::cell::Cell::new(false)] };
    }
    // Only the outermost call of each kind is charged: a boolean that runs
    // another internally would otherwise be counted twice.
    let k = kind as usize;
    if !ENABLED.load(Ordering::Relaxed) || ACTIVE.with(|a| a[k].get()) {
        return f();
    }
    ACTIVE.with(|a| a[k].set(true));
    // Cleared on unwind too: the evaluator catches per-root panics.
    struct Clear(usize);
    impl Drop for Clear {
        fn drop(&mut self) {
            ACTIVE.with(|a| a[self.0].set(false));
        }
    }
    let _clear = Clear(k);
    let t0 = std::time::Instant::now();
    let out = f();
    let ns = t0.elapsed().as_nanos() as u64;
    let (n, total) = match kind {
        Kind::Boolean => (&BOOLEAN_N, &BOOLEAN_NS),
        Kind::Fallback => (&FALLBACK_N, &FALLBACK_NS),
    };
    n.fetch_add(1, Ordering::Relaxed);
    total.fetch_add(ns, Ordering::Relaxed);
    out
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn timed<T>(_kind: Kind, f: impl FnOnce() -> T) -> T {
    f()
}
