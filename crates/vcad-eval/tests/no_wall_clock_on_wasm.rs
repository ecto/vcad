//! Source guard: nothing in this crate may read the wall clock directly.
//!
//! `std::time::Instant::now()` and `SystemTime::now()` are not implemented on
//! `wasm32-unknown-unknown` — they panic, and a panic inside a
//! `wasm_bindgen` export traps the module. A single such call on the
//! evaluation path takes the browser kernel and the WASM MCP kernel down for
//! every document that reaches it, and the web app's TypeScript fallback
//! makes that look like a slow render rather than a crash. The evaluator
//! therefore reads time only through [`vcad_eval::Clock`], and the one
//! remaining `Instant` lives in `budget.rs`'s `native_clock` module, which is
//! compiled out of every `wasm32` build.
//!
//! # Why a test and not a `compile_error!`
//!
//! `compile_error!` can be aimed at a `cfg`, not at a call: there is no
//! attribute that says "this module may not call `Instant::now`". The other
//! candidate, building for `wasm32-unknown-unknown`, does catch it — but only
//! if someone remembers to add the target, whereas this runs inside the
//! `cargo test -p vcad-eval` that every change to this crate already runs.

use std::fs;
use std::path::{Path, PathBuf};

/// Wall-clock constructors that trap on `wasm32-unknown-unknown`.
const BANNED: [&str; 2] = ["Instant::now", "SystemTime::now"];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).expect("read src/") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            out.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// Prose is allowed to name the thing it is explaining, so line comments do
/// not count. (A trailing comment on a line of code still counts — the line
/// has code on it.)
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')
}

/// The line range of `budget.rs`'s cfg-gated default clock — the one place
/// allowed to name `Instant`. Fails if the module has lost its `cfg`, moved,
/// or been renamed, because then the exemption no longer means anything.
fn native_clock_span(src: &str) -> (usize, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.trim() == "mod native_clock {")
        .expect("budget.rs no longer declares `mod native_clock`");
    assert!(
        start > 0 && lines[start - 1].trim() == "#[cfg(not(target_arch = \"wasm32\"))]",
        "`mod native_clock` is no longer gated on `#[cfg(not(target_arch = \"wasm32\"))]`, \
         so its `Instant` would be compiled into wasm32 builds"
    );
    let end = lines[start..]
        .iter()
        .position(|l| *l == "}")
        .map(|i| start + i)
        .expect("`mod native_clock` has no closing brace at column 0");
    (start, end)
}

#[test]
fn the_evaluator_never_reads_the_wall_clock_directly() {
    let src_dir = src_dir();
    let budget_rs = src_dir.join("budget.rs");
    let mut offences: Vec<String> = Vec::new();

    for path in rust_sources(&src_dir) {
        let src = fs::read_to_string(&path).expect("read source");
        let exempt = if path == budget_rs {
            Some(native_clock_span(&src))
        } else {
            None
        };
        for (i, line) in src.lines().enumerate() {
            if is_comment(line) || !BANNED.iter().any(|b| line.contains(b)) {
                continue;
            }
            if let Some((start, end)) = exempt {
                if (start..=end).contains(&i) {
                    continue;
                }
            }
            let name = path.strip_prefix(&src_dir).unwrap_or(&path).display();
            offences.push(format!("  {name}:{}: {}", i + 1, line.trim()));
        }
    }

    assert!(
        offences.is_empty(),
        "these read the wall clock directly, which panics and traps the module on \
         wasm32-unknown-unknown — take the time from `EvalOptions::clock` (see \
         `crate::budget`) instead:\n{}",
        offences.join("\n")
    );
}

#[test]
fn the_guard_would_catch_a_reintroduced_wall_clock() {
    // Mutation check for the guard itself: the same scan over a source that
    // does read the clock has to complain. Without this, a guard that silently
    // stopped matching would look exactly like a clean crate.
    let sample = "fn slow() {\n    let t0 = std::time::Instant::now();\n}\n";
    let hits: Vec<&str> = sample
        .lines()
        .filter(|l| !is_comment(l) && BANNED.iter().any(|b| l.contains(b)))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "the scan missed a real `Instant::now()` call"
    );
}
