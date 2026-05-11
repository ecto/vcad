//! `FaceIndex → NodeId` provenance map.
//!
//! Populated by the IR evaluator. v1 ships the data model only; the
//! engine will start writing entries when the evaluator pass lands.
//! Until then, callers pass `None` and issues get `origin_op: None` —
//! the agent loop still works for face-level highlights and manual
//! fixes, only the autofix-to-source-op edge is missing.

use std::collections::HashMap;
use vcad_ir::NodeId;

/// `face index → originating NodeId` map.
#[derive(Debug, Default, Clone)]
pub struct ProvenanceMap {
    /// Storage keyed by face index (matches BRep iteration order).
    pub by_face: HashMap<usize, NodeId>,
}

impl ProvenanceMap {
    /// Empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tag a face with its source op.
    pub fn tag(&mut self, face: usize, node: NodeId) {
        self.by_face.insert(face, node);
    }

    /// Look up the source op for a face.
    pub fn get(&self, face: usize) -> Option<NodeId> {
        self.by_face.get(&face).copied()
    }
}
