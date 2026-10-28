//! Opt-in wall-time breakdown for the CLIs' `--timings` flag.
//!
//! A render or export records the stages it runs (parse, evaluation, BVH
//! build, trace, encode, ...) and the per-root evaluation times here, and
//! prints [`report`] once at exit: stages in run order with their share of
//! the total, the boolean / mesh-fallback / tessellation split of
//! evaluation, and the slowest roots. Process-global, because the stages
//! live in different crates and the report is one per process.
//!
//! Off unless [`set_enabled`] is called or `VCAD_TIMINGS` is set, and then
//! nothing here reads a clock. Never enable it on wasm32, where
//! `Instant::now` panics.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use crate::{Clock, EvalTiming};

static ENABLED: AtomicBool = AtomicBool::new(false);
static LOG: Mutex<Log> = Mutex::new(Log::new());

struct Log {
    start: Option<Instant>,
    stages: Vec<(String, f64)>,
    roots: Vec<(String, f64)>,
    tessellate_ms: f64,
    cache: Option<(u64, u64)>,
}

impl Log {
    const fn new() -> Self {
        Log {
            start: None,
            stages: Vec::new(),
            roots: Vec::new(),
            tessellate_ms: 0.0,
            cache: None,
        }
    }
}

fn log() -> std::sync::MutexGuard<'static, Log> {
    LOG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Turn the breakdown on (also turns on the kernel's boolean accounting).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    vcad_kernel::vcad_kernel_booleans::timing::set_enabled(on);
    if on {
        log().start.get_or_insert_with(Instant::now);
    }
}

/// Whether timings are being recorded. Picks up `VCAD_TIMINGS` (any value
/// but `0`) on first use.
pub fn enabled() -> bool {
    static ENV: std::sync::Once = std::sync::Once::new();
    ENV.call_once(|| {
        if std::env::var("VCAD_TIMINGS").is_ok_and(|v| v != "0") {
            set_enabled(true);
        }
    });
    ENABLED.load(Ordering::Relaxed)
}

/// Run `f` as the named stage.
pub fn stage<T>(name: &str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let t0 = Instant::now();
    let out = f();
    record(name, t0.elapsed().as_secs_f64() * 1e3);
    out
}

/// Record a stage that was timed elsewhere.
pub fn record(name: &str, ms: f64) {
    if enabled() {
        log().stages.push((name.to_string(), ms));
    }
}

/// Record the evaluator's own breakdown: tessellation time, and each
/// evaluated root's (and part definition's) evaluate + tessellate time,
/// named from `doc`. Cache hits cost nothing and are not listed.
pub fn record_eval(timing: Option<&EvalTiming>, doc: &vcad_ir::Document) {
    let (true, Some(t)) = (enabled(), timing) else {
        return;
    };
    let mut log = log();
    log.tessellate_ms += t.tessellate_ms;
    log.roots.extend(t.roots.iter().map(|&(id, ms)| {
        let name = doc
            .nodes
            .get(&id)
            .and_then(|n| n.name.clone())
            .unwrap_or_else(|| format!("node {id}"));
        (name, ms)
    }));
}

/// Record root-mesh cache counts, `(hits, misses)`.
pub fn record_cache(hits: u64, misses: u64) {
    if enabled() {
        log().cache = Some((hits, misses));
    }
}

/// A clock for [`crate::EvalOptions::clock`] when timings are on.
pub fn clock() -> Option<Box<dyn Clock>> {
    struct InstantClock(Instant);
    impl Clock for InstantClock {
        fn now_ms(&self) -> f64 {
            self.0.elapsed().as_secs_f64() * 1e3
        }
    }
    enabled().then(|| Box::new(InstantClock(Instant::now())) as Box<dyn Clock>)
}

/// The breakdown recorded so far, `top_n` slowest roots included, or `None`
/// when timings are off.
pub fn report(top_n: usize) -> Option<String> {
    if !enabled() {
        return None;
    }
    use std::fmt::Write;
    let log = log();
    let total = log
        .start
        .map(|t| t.elapsed().as_secs_f64() * 1e3)
        .unwrap_or_else(|| log.stages.iter().map(|s| s.1).sum());
    let pct = |ms: f64| if total > 0.0 { 100.0 * ms / total } else { 0.0 };
    let mut out = String::new();
    let _ = writeln!(out, "timings: total {:.2}s", total / 1e3);
    for (name, ms) in &log.stages {
        let _ = writeln!(out, "  {name:<24} {:>9.2}s  {:>5.1}%", ms / 1e3, pct(*ms));
    }
    let b = vcad_kernel::vcad_kernel_booleans::timing::take();
    let _ = writeln!(
        out,
        "  within evaluation: booleans {:.2}s ({}), mesh fallback {:.2}s ({}), tessellation {:.2}s",
        b.boolean_ms / 1e3,
        b.booleans,
        b.fallback_ms / 1e3,
        b.fallbacks,
        log.tessellate_ms / 1e3,
    );
    if let Some((hits, misses)) = log.cache {
        let _ = writeln!(out, "  root cache: {hits} hit, {misses} miss");
    }
    let mut roots = log.roots.clone();
    roots.sort_by(|a, b| b.1.total_cmp(&a.1));
    if !roots.is_empty() {
        let root_total: f64 = roots.iter().map(|r| r.1).sum();
        let _ = writeln!(
            out,
            "  slowest roots ({} of {}, {:.2}s across all):",
            top_n.min(roots.len()),
            roots.len(),
            root_total / 1e3
        );
        for (name, ms) in roots.iter().take(top_n) {
            let _ = writeln!(out, "    {:>8.2}s  {name}", ms / 1e3);
        }
    }
    Some(out.trim_end().to_string())
}
