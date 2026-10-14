//! `--theme dark` CLI contract: `--photoreal` refuses it (studio-lit, its own
//! backdrop) instead of silently rendering a light image.
#![cfg(feature = "cli")]

use std::process::Command;

#[test]
fn photoreal_refuses_theme_dark() {
    let dir = std::env::temp_dir().join(format!("vcad-render-theme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let doc = vcad_loon::eval_vcad("[cube 30.0 20.0 10.0]", None).expect("loon eval");
    let input = dir.join("part.vcad");
    std::fs::write(&input, serde_json::to_string(&doc).expect("serialize")).expect("write");
    let output = dir.join("out.png");

    let out = Command::new(env!("CARGO_BIN_EXE_vcad-render"))
        .arg(&input)
        .args(["--photoreal", "--theme", "dark", "-o"])
        .arg(&output)
        .output()
        .expect("run vcad-render");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!out.status.success(), "--photoreal --theme dark must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--theme dark does not apply to --photoreal"),
        "unexpected stderr: {stderr}"
    );
    assert!(!output.exists(), "no image may be written on refusal");
}
