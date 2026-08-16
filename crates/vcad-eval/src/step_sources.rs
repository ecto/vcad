//! In-memory registry of STEP file contents, keyed by the path a
//! [`CsgOp::StepImport`](vcad_ir::CsgOp::StepImport) node stores.
//!
//! A `StepImport` node is a *lazy reference*: the document keeps a path and
//! the evaluator opens the file. That works natively (CLI, desktop), but the
//! WASM kernel — which is what the MCP server and the browser run — has no
//! filesystem, so the same node would evaluate to nothing at all.
//!
//! Registering the bytes under the node's path closes that gap: the evaluator
//! prefers a registered source and only falls back to the filesystem. The
//! caller that *has* the bytes (an MCP import, a browser drag-drop) registers
//! them once; every later evaluation of the document resolves B-rep, so
//! analytic faces survive into booleans, fillets, and STEP export.
//!
//! Parsed solids are cached alongside the bytes — a vendor assembly is
//! megabytes of STEP text, and a document is re-evaluated on every edit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use vcad_kernel::Solid;

/// A registered STEP source: the raw bytes plus the solids parsed from them.
struct StepSource {
    bytes: Arc<Vec<u8>>,
    /// Parsed lazily on first evaluation, then reused.
    solids: Option<Arc<Vec<Solid>>>,
}

fn registry() -> &'static Mutex<HashMap<String, StepSource>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, StepSource>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register STEP file contents under `path`.
///
/// `path` must match the string stored in the `StepImport` node verbatim.
/// Re-registering the same path replaces the bytes and drops the parse cache.
pub fn register(path: impl Into<String>, bytes: Vec<u8>) {
    let mut reg = registry().lock().expect("step source registry poisoned");
    reg.insert(
        path.into(),
        StepSource {
            bytes: Arc::new(bytes),
            solids: None,
        },
    );
}

/// Register STEP contents together with an already-parsed solid list.
///
/// The importer parses the file anyway to report skipped faces; handing the
/// result over here means the first evaluation doesn't re-parse megabytes.
pub fn register_parsed(path: impl Into<String>, bytes: Vec<u8>, solids: Vec<Solid>) {
    let mut reg = registry().lock().expect("step source registry poisoned");
    reg.insert(
        path.into(),
        StepSource {
            bytes: Arc::new(bytes),
            solids: Some(Arc::new(solids)),
        },
    );
}

/// Whether `path` has registered contents.
pub fn is_registered(path: &str) -> bool {
    registry()
        .lock()
        .expect("step source registry poisoned")
        .contains_key(path)
}

/// Raw registered bytes for `path`, if any.
pub fn bytes(path: &str) -> Option<Arc<Vec<u8>>> {
    registry()
        .lock()
        .expect("step source registry poisoned")
        .get(path)
        .map(|s| s.bytes.clone())
}

/// Forget the source registered under `path`.
pub fn unregister(path: &str) {
    registry()
        .lock()
        .expect("step source registry poisoned")
        .remove(path);
}

/// Forget every registered source.
pub fn clear() {
    registry()
        .lock()
        .expect("step source registry poisoned")
        .clear();
}

/// Parse (or reuse the cached parse of) the solids registered under `path`.
///
/// Returns `Ok(None)` when nothing is registered for `path` — the caller
/// falls back to the filesystem. A parse failure is an error, never a silent
/// empty result.
pub fn solids(path: &str) -> Result<Option<Arc<Vec<Solid>>>, String> {
    let bytes = {
        let reg = registry().lock().expect("step source registry poisoned");
        match reg.get(path) {
            None => return Ok(None),
            Some(src) => {
                if let Some(cached) = &src.solids {
                    return Ok(Some(cached.clone()));
                }
                src.bytes.clone()
            }
        }
    };

    // Parse outside the lock: a large assembly takes seconds, and holding the
    // registry mutex across it would serialize unrelated evaluations.
    let parsed = Solid::from_step_buffer_all(&bytes).map_err(|e| e.to_string())?;
    let parsed = Arc::new(parsed);

    let mut reg = registry().lock().expect("step source registry poisoned");
    if let Some(src) = reg.get_mut(path) {
        // Only cache if the entry still holds the bytes we parsed.
        if Arc::ptr_eq(&src.bytes, &bytes) {
            src.solids = Some(parsed.clone());
        }
    }
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unregistered_path_is_none() {
        assert!(solids("/no/such/registered.step").unwrap().is_none());
    }

    #[test]
    fn bad_bytes_error_rather_than_vanish() {
        register("/tmp/bogus-registry-test.step", b"not a step file".to_vec());
        assert!(solids("/tmp/bogus-registry-test.step").is_err());
        unregister("/tmp/bogus-registry-test.step");
        assert!(!is_registered("/tmp/bogus-registry-test.step"));
    }
}
