//! How one `.loon` file uses definitions from another.
//!
//! A **module is a loon file**. Its top-level `let`s are its exports (or only
//! the `pub let`s, once any `pub` appears), and a file names the modules it
//! uses:
//!
//! ```loon
//! [use plates]                        ; plates.plate-x, plates.bore-z, …
//! [use plates [plate-x bore-x]]       ; just those two, unqualified
//! [use plates :as p]                  ; p.plate-x
//! [use hardware.fasteners]            ; hardware/fasteners.loon
//! ```
//!
//! A module is evaluated **once**, in its own environment, with the vcad
//! library (`cube`, `union`, `pipe`, …) already in scope — it exports
//! *values* (closures, solids, numbers), not text. That is the difference
//! from a preamble include: a module cannot see the importer's bindings,
//! two files that `use` it share one evaluation, and a `let` inside it that
//! is not exported stays private. Cycles are an error.
//!
//! **Resolution order**, the same in `vcad`, `vcad-render` and the MCP
//! server's `load_document`:
//!
//! 1. relative to the importing file's directory (`[use a.b]` →
//!    `<dir>/a/b.loon`, `.oo` also accepted);
//! 2. each directory in `$VCAD_LOON_PATH` (`:`-separated; `;` on Windows),
//!    in order — the *lib path*, for parts shared across projects;
//! 3. a host-supplied in-memory map, when the host has one (the MCP server
//!    hands the kernel the files it read; the browser has no filesystem).
//!
//! The in-memory map actually wins over the filesystem when both name the
//! same module — a host that supplies a source means it. Between (1) and
//! (2) the file next to the importer wins, so a project can shadow a lib
//! module by dropping a file of the same name beside its own.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use loon_lang::module::{MapProvider, ModuleProvider};

/// Environment variable naming the lib path: directories searched for a
/// module after the importing file's own directory.
pub const LIB_PATH_VAR: &str = "VCAD_LOON_PATH";

/// The lib path from the environment, in search order. Empty entries are
/// skipped; an unset variable is an empty path.
pub fn lib_dirs() -> Vec<PathBuf> {
    match std::env::var_os(LIB_PATH_VAR) {
        Some(v) => std::env::split_paths(&v)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        None => Vec::new(),
    }
}

/// A [`ModuleProvider`] that resolves a module against a list of
/// directories — but only after declining any module that exists next to
/// the importer, so the relative lookup keeps priority and keeps its
/// correct nested-resolution directory.
pub struct LibPathProvider {
    dirs: Vec<PathBuf>,
}

impl LibPathProvider {
    /// A provider over `dirs`, searched in order.
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }

    /// A provider over [`lib_dirs`].
    pub fn from_env() -> Self {
        Self::new(lib_dirs())
    }

    /// Is there anything to search?
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }

    /// `<dir>/<a>/<b>.loon` (or `.oo`) for a dotted module name, if present.
    fn candidate(dir: &Path, module_path: &str) -> Option<PathBuf> {
        let mut p = dir.to_path_buf();
        for part in module_path.split('.') {
            // A module name is a name, not a path: refuse anything that
            // would climb out of the directory.
            if part.is_empty() || part == ".." || part.contains(['/', '\\']) {
                return None;
            }
            p.push(part);
        }
        for ext in ["loon", "oo"] {
            let f = p.with_extension(ext);
            if f.is_file() {
                return Some(f);
            }
        }
        None
    }
}

impl ModuleProvider for LibPathProvider {
    fn fetch(&self, module_path: &str, from_dir: Option<&str>) -> Result<Option<String>, String> {
        // The module is beside the importer: decline, and let the ordinary
        // filesystem lookup load it (so its own `[use …]` resolve relative
        // to *it*, which a provider-loaded module's would not).
        if let Some(dir) = from_dir {
            if Self::candidate(Path::new(dir), module_path).is_some() {
                return Ok(None);
            }
        }
        for dir in &self.dirs {
            if let Some(f) = Self::candidate(dir, module_path) {
                return std::fs::read_to_string(&f).map(Some).map_err(|e| {
                    format!("cannot read module '{module_path}' at {}: {e}", f.display())
                });
            }
        }
        Ok(None)
    }
}

/// First provider that answers wins; all may decline.
struct Chain(Vec<Rc<dyn ModuleProvider>>);

impl ModuleProvider for Chain {
    fn fetch(&self, module_path: &str, from_dir: Option<&str>) -> Result<Option<String>, String> {
        for p in &self.0 {
            if let Some(src) = p.fetch(module_path, from_dir)? {
                return Ok(Some(src));
            }
        }
        Ok(None)
    }
}

/// The module provider every evaluation entry point uses: the host's
/// in-memory map (if any), then the lib path from the environment. `None`
/// when there is neither, so the plain relative-to-file lookup runs alone
/// at no cost.
pub(crate) fn provider(map: Option<MapProvider>) -> Option<Rc<dyn ModuleProvider>> {
    let mut chain: Vec<Rc<dyn ModuleProvider>> = Vec::new();
    if let Some(m) = map {
        chain.push(Rc::new(m));
    }
    let lib = LibPathProvider::from_env();
    if !lib.is_empty() {
        chain.push(Rc::new(lib));
    }
    match chain.len() {
        0 => None,
        1 => chain.pop(),
        _ => Some(Rc::new(Chain(chain))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vcad-loon-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lib_path_provider_prefers_importer_dir_then_lib() {
        let proj = tmp("proj");
        let lib = tmp("lib");
        std::fs::write(lib.join("plates.loon"), "[pub let which \"lib\"]").unwrap();
        let p = LibPathProvider::new(vec![lib.clone()]);
        // Not beside the importer: served from the lib path.
        let got = p.fetch("plates", proj.to_str()).unwrap().unwrap();
        assert!(got.contains("\"lib\""));
        // Beside the importer: declined, so the filesystem lookup wins.
        std::fs::write(proj.join("plates.loon"), "[pub let which \"proj\"]").unwrap();
        assert!(p.fetch("plates", proj.to_str()).unwrap().is_none());
        // Unknown anywhere: declined (loon reports the missing module).
        assert!(p.fetch("nope", proj.to_str()).unwrap().is_none());
        // Dotted names are subdirectories; traversal is refused.
        std::fs::create_dir_all(lib.join("hw")).unwrap();
        std::fs::write(lib.join("hw/bolts.loon"), "[pub let m3 3.0]").unwrap();
        assert!(p.fetch("hw.bolts", None).unwrap().is_some());
        assert!(p.fetch("..", None).unwrap().is_none());
        std::fs::remove_dir_all(&proj).ok();
        std::fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn chain_asks_in_order() {
        let mut m = HashMap::new();
        m.insert("a".to_string(), "[pub let x 1]".to_string());
        let lib = tmp("chain-lib");
        std::fs::write(lib.join("a.loon"), "[pub let x 2]").unwrap();
        std::fs::write(lib.join("b.loon"), "[pub let y 3]").unwrap();
        let chain = Chain(vec![
            Rc::new(MapProvider::new(m)),
            Rc::new(LibPathProvider::new(vec![lib.clone()])),
        ]);
        assert_eq!(chain.fetch("a", None).unwrap().unwrap(), "[pub let x 1]");
        assert_eq!(chain.fetch("b", None).unwrap().unwrap(), "[pub let y 3]");
        assert!(chain.fetch("c", None).unwrap().is_none());
        std::fs::remove_dir_all(&lib).ok();
    }
}
