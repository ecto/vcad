//! Source provenance: record the loon program a document was evaluated from
//! (`Document.source`), so a session can be re-evaluated, saved alongside its
//! source, and told apart from one that was edited after evaluation.

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use vcad_ir::{Document, DocumentSource};

/// Lowercase hex SHA-256 of `text` — the `DocumentSource.hash` hosts compare
/// against a file's current contents to detect on-disk edits.
pub fn source_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Stamp `doc.source` with the loon `text` (the composed program, macro
/// prelude included, so it re-evaluates without a macro registry), its
/// resolved `modules`, and optionally the directory/file it came from. A
/// fresh stamp is never diverged.
pub fn attach_loon_source(
    doc: &mut Document,
    text: &str,
    modules: &HashMap<String, String>,
    base_dir: Option<&str>,
    path: Option<&str>,
) {
    doc.source = Some(DocumentSource {
        language: "loon".into(),
        text: text.to_string(),
        modules: modules.clone(),
        base_dir: base_dir.map(str::to_string),
        path: path.map(str::to_string),
        hash: source_hash(text),
        diverged: false,
        diverged_by: Vec::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_sha256_hex() {
        assert_eq!(
            source_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn attaches_a_fresh_source() {
        let mut doc = Document::default();
        let modules: HashMap<String, String> =
            [("m".to_string(), "[cube 1 1 1]".to_string())].into();
        attach_loon_source(&mut doc, "[use m]", &modules, Some("/p"), Some("/p/a.loon"));
        let src = doc.source.unwrap();
        assert_eq!(src.language, "loon");
        assert_eq!(src.hash, source_hash("[use m]"));
        assert_eq!(src.modules, modules);
        assert_eq!(src.base_dir.as_deref(), Some("/p"));
        assert_eq!(src.path.as_deref(), Some("/p/a.loon"));
        assert!(!src.diverged && src.diverged_by.is_empty());
    }
}
