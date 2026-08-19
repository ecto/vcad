//! Derive the evaluating kernel's identity for the root-mesh cache.
//!
//! `VCAD_EVAL_SOURCE_HASH` is a SHA-256 over the *contents* of every source
//! file that can change what a root evaluates to: the kernel crates, the IR,
//! this evaluator, the sibling `tang` math workspace, and the workspace
//! `Cargo.lock` (external deps such as the exact-predicate crate). Editing
//! any of them produces a new id, so a cache populated by one kernel build
//! is never read by another; two checkouts at the same source share an id.
//!
//! Content hashing (not mtimes, not a git sha) is what makes invalidation
//! boring: uncommitted kernel edits, rebased branches and `touch` all do the
//! right thing. If the source trees can't be found at all (a packaged
//! build), the hash is `unhashed` and `vcad_eval::cache` refuses to cache.
//!
//! No hashing crate here — `build-dependencies` would compile it twice. A
//! small FNV-1a-64 over the content is plenty to name a build; the cache key
//! itself (which must not collide) is SHA-256 in the library.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let crates = manifest.parent().unwrap().to_path_buf(); // <repo>/crates
    let repo = crates.parent().unwrap().to_path_buf();
    // `../tang` relative to the repo root, as Cargo.toml's path deps have it.
    let tang = repo.parent().map(|p| p.join("tang").join("crates"));

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&crates) {
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("vcad-kernel") || name == "vcad-ir" || name == "vcad-eval" {
                roots.push(e.path());
            }
        }
    }
    if let Some(t) = tang.filter(|t| t.is_dir()) {
        roots.push(t);
    }
    roots.sort();

    let mut files: Vec<PathBuf> = Vec::new();
    for r in &roots {
        println!("cargo:rerun-if-changed={}", r.display());
        collect(r, &mut files);
    }
    let lock = repo.join("Cargo.lock");
    if lock.is_file() {
        println!("cargo:rerun-if-changed={}", lock.display());
        files.push(lock);
    }
    files.sort();

    let mut h: u64 = 0xcbf29ce484222325;
    let mut hashed = 0usize;
    for f in &files {
        if let Ok(bytes) = std::fs::read(f) {
            // Path relative to the crates dir so the hash doesn't depend on
            // where the checkout lives.
            let rel = f.strip_prefix(&repo).unwrap_or(f);
            fnv(&mut h, rel.to_string_lossy().as_bytes());
            fnv(&mut h, &bytes);
            hashed += 1;
        }
    }
    let id = if hashed == 0 {
        "unhashed".to_string()
    } else {
        format!("{h:016x}")
    };
    println!("cargo:rustc-env=VCAD_EVAL_SOURCE_HASH={id}");
    println!(
        "cargo:rustc-env=VCAD_EVAL_TARGET_ARCH={}",
        std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default()
    );
}

/// Recursively gather the files whose content shapes evaluation results.
/// Source, shaders and manifests; never `target/`, tests' fixtures or docs.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if name == "target" || name == "tests" || name == "benches" || name == "examples" {
                continue;
            }
            collect(&p, out);
        } else if matches!(
            p.extension().and_then(|x| x.to_str()),
            Some("rs" | "toml" | "wgsl" | "loon")
        ) {
            out.push(p);
        }
    }
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}
