//! Collapsed reporting for the Difference re-cut fallback.
//!
//! A Difference that removed no volume from a subtrahend it overlaps is
//! rejected and re-cut with the mesh boolean (see `api.rs`). On a large
//! document that fires dozens of times — a 122-part humanoid printed the same
//! sentence once per cut, burying everything else on stderr. The kernel now
//! only counts; a CLI calls [`take_summary`] once at the end of its run and
//! prints a single line. [`set_verbose`] (or `VCAD_BOOLEAN_VERBOSE=1` /
//! `VCAD_BOOLEAN_WARN=1` in the environment) restores the per-cut line.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static RECUTS: AtomicUsize = AtomicUsize::new(0);
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Print every re-cut as it happens instead of only counting it.
pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
        || std::env::var_os("VCAD_BOOLEAN_VERBOSE").is_some()
        || std::env::var_os("VCAD_BOOLEAN_WARN").is_some()
}

/// Record one rejected-and-re-cut Difference.
pub(crate) fn difference_recut() {
    RECUTS.fetch_add(1, Ordering::Relaxed);
    if verbose() {
        eprintln!(
            "vcad boolean: Difference removed no volume from a subtrahend that \
             overlaps the minuend — rejecting the B-rep result and re-cutting \
             with the mesh boolean"
        );
    }
}

/// Number of re-cuts recorded since the last [`take_summary`].
pub fn recut_count() -> usize {
    RECUTS.load(Ordering::Relaxed)
}

/// The one-line summary of re-cuts since the last call (and reset the
/// count), or `None` when there were none.
pub fn take_summary() -> Option<String> {
    let n = RECUTS.swap(0, Ordering::Relaxed);
    (n > 0).then(|| {
        let detail = if verbose() {
            ""
        } else {
            " (pass --verbose or set VCAD_BOOLEAN_VERBOSE=1 for each)"
        };
        format!(
            "vcad boolean: {n} Difference(s) removed no volume from an overlapping \
             subtrahend and were re-cut with the mesh boolean{detail}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_and_resets() {
        let _ = take_summary();
        difference_recut();
        difference_recut();
        difference_recut();
        // Other tests in this binary may cut concurrently, so bound below.
        assert!(recut_count() >= 3);
        let line = take_summary().expect("three re-cuts recorded");
        assert!(line.contains("Difference(s)"), "{line}");
    }
}
