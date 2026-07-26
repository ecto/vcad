//! The CLI renders `.loon` source directly, with `[use ...]` module imports
//! resolved against the input file's own directory — the native renderer is
//! the one place file-based imports work (the WASM/MCP path evaluates with no
//! base dir at all). Without this, "see my model" meant hand-writing a
//! loon → IR converter first.
#![cfg(feature = "cli")]

use std::path::PathBuf;
use std::process::Command;

/// A scratch directory unique to this test, cleaned up on the way out.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("vcad-render-loon-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, contents).expect("write fixture");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn render(input: &PathBuf) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vcad-render"))
        .arg(input)
        .output()
        .expect("run vcad-render")
}

#[test]
fn renders_loon_source_directly() {
    let dir = TempDir::new("plain");
    let input = dir.write("part.loon", "[cube 30.0 20.0 10.0]");

    let out = render(&input);
    assert!(
        out.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let svg = String::from_utf8(out.stdout).expect("utf-8 svg");
    assert!(svg.contains("<svg"), "expected an SVG, got {svg:.80}");
}

#[test]
fn resolves_use_imports_against_the_input_directory() {
    let dir = TempDir::new("modules");
    dir.write("parts.loon", "[pub let widget [cube 30.0 20.0 10.0]]");
    let input = dir.write("main.loon", "[use parts]\n[root parts.widget \"steel\"]");

    let out = render(&input);
    assert!(
        out.status.success(),
        "module import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("<svg"));
}

#[test]
fn loon_errors_exit_nonzero_on_stderr() {
    let dir = TempDir::new("bad");
    let input = dir.write("bad.loon", "[cube 1.0");

    let out = render(&input);
    assert!(!out.status.success(), "a broken document must not succeed");
    assert!(out.stdout.is_empty(), "no partial SVG on stdout");
    assert!(!out.stderr.is_empty(), "the error belongs on stderr");
}
