//! `.loon` source is *not* evaluated by this binary. The loon interpreter
//! lives in `vcad-loon`, which is unpublished (it depends on a git rev of
//! loon-lang), and cargo makes every `[dependencies]` entry resolve from the
//! registry — so vcad-render cannot carry it and stay publishable. A `.loon`
//! input is therefore a clear error pointing at the `vcad` CLI, rather than a
//! parse failure on IR JSON that reads like a corrupt document.
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
fn loon_input_is_refused_with_a_pointer_to_the_vcad_cli() {
    let dir = TempDir::new("plain");
    let input = dir.write("part.loon", "[cube 30.0 20.0 10.0]");

    let out = render(&input);
    assert!(!out.status.success(), "`.loon` input must not render");
    assert!(out.stdout.is_empty(), "no partial SVG on stdout");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("vcad CLI") && err.contains(".vcad"),
        "the error should name the way out, got {err}"
    );
}

#[test]
fn a_vcad_document_still_renders() {
    let dir = TempDir::new("doc");
    let doc = vcad_loon::eval_vcad("[cube 30.0 20.0 10.0]", None).expect("loon eval");
    let input = dir.write(
        "part.vcad",
        &serde_json::to_string(&doc).expect("serialize document"),
    );

    let out = render(&input);
    assert!(
        out.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let svg = String::from_utf8(out.stdout).expect("utf-8 svg");
    assert!(svg.contains("<svg"), "expected an SVG, got {svg:.80}");
}
