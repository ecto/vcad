//! `FaceIndex → NodeId` provenance map.
//!
//! Populated by the IR evaluator. v1 ships the data model only; the
//! engine will start writing entries when the evaluator pass lands.
//! Until then, callers pass `None` and issues get `origin_op: None` —
//! the agent loop still works for face-level highlights and manual
//! fixes, only the autofix-to-source-op edge is missing.

use std::collections::HashMap;
use vcad_ir::NodeId;
use vcad_kernel_primitives::BRepSolid;

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

    /// Build a provenance map that attributes every face in `brep` to a
    /// single root [`NodeId`].
    ///
    /// This is the v1 coarse fallback the engine uses when the
    /// per-feature lineage pass hasn't run. It's exactly correct for a
    /// part whose root op is a primitive (cube → 6 faces all from the
    /// cube node) and "good enough" for booleans (every face attributed
    /// to the boolean root, which is also the node a `set_param` fix
    /// most often wants to mutate).
    ///
    /// Future work: replace with per-feature tagging during evaluation
    /// so booleans can attribute new edges/faces to the contributing
    /// operands rather than the boolean root.
    pub fn single_root(brep: &BRepSolid, root: NodeId) -> Self {
        let mut by_face = HashMap::with_capacity(brep.topology.faces.len());
        for (idx, _) in brep.topology.faces.iter().enumerate() {
            by_face.insert(idx, root);
        }
        Self { by_face }
    }
}
