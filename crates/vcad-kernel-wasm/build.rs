//! Emit the current git short SHA into `VCAD_KERNEL_BUILD_REV` so the
//! WASM boot log identifies which build is loaded. Without this, a stale
//! hard-coded `KERNEL_VERSION` constant lies about the running build and
//! sends debuggers down dead ends (which happened — see PR #185).

use std::process::Command;

fn main() {
    let rev = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let suffix = if dirty { "-dirty" } else { "" };

    println!("cargo:rustc-env=VCAD_KERNEL_BUILD_REV={rev}{suffix}");

    // Re-run when HEAD moves or the index changes.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
}
