//! Incremental evaluation: keep evaluated solids (and root meshes) across
//! evaluations of an edited document, re-evaluating only what changed.
//!
//! The materializer allocates IR node ids in per-feature blocks, so an edit
//! leaves every untouched node's id *and* content as they were. Each node
//! gets a subtree fingerprint (its op, its id, and its children's
//! fingerprints, recursively); a cached solid is reused only while its
//! fingerprint matches. An edit therefore invalidates exactly the edited
//! nodes and everything downstream of them in the DAG — the features that
//! consume the edited one — and nothing else.
//!
//! Use it by putting one [`IncrementalCache`] in
//! [`EvalOptions::incremental`](crate::EvalOptions::incremental) and reusing
//! it for every evaluation of the same document. Results equal a cold
//! evaluation (see the property test in `tests/incremental.rs`).

use std::collections::HashMap;

use vcad_ir::{Node, NodeId};
use vcad_kernel::Solid;

use crate::EvaluatedMesh;

/// Evaluated state carried between evaluations of one document.
#[derive(Default)]
pub struct IncrementalCache {
    solids: HashMap<NodeId, (u64, Option<Solid>)>,
    meshes: HashMap<NodeId, (u64, EvaluatedMesh)>,
    /// Clash results between two roots, keyed by both roots' subtree
    /// fingerprints (which cover their placement): `None` = no clash.
    clashes: HashMap<(u64, u64), Option<EvaluatedMesh>>,
    stats: IncrementalStats,
}

/// What the last evaluation reused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncrementalStats {
    /// Node solids carried over from the previous evaluation.
    pub reused_solids: usize,
    /// Node solids evaluated (or re-evaluated) this time.
    pub evaluated_solids: usize,
    /// Root meshes carried over (no re-tessellation).
    pub reused_meshes: usize,
}

impl IncrementalCache {
    /// An empty cache: the first evaluation through it is a cold one.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reuse counts from the most recent evaluation.
    pub fn stats(&self) -> IncrementalStats {
        self.stats
    }

    /// Drop everything (the next evaluation is cold).
    pub fn clear(&mut self) {
        self.solids.clear();
        self.meshes.clear();
        self.clashes.clear();
    }

    /// Hand the clash results over to this evaluation (it puts back the
    /// pairs it still tests, so unchanged pairs are never re-intersected).
    pub(crate) fn take_clashes(&mut self) -> HashMap<(u64, u64), Option<EvaluatedMesh>> {
        std::mem::take(&mut self.clashes)
    }

    pub(crate) fn put_clashes(&mut self, clashes: HashMap<(u64, u64), Option<EvaluatedMesh>>) {
        self.clashes = clashes;
    }

    /// Move out the solids still valid under `hashes`, discarding stale ones.
    pub(crate) fn take_valid(
        &mut self,
        hashes: &HashMap<NodeId, u64>,
    ) -> HashMap<NodeId, Option<Solid>> {
        self.meshes.retain(|id, (h, _)| hashes.get(id) == Some(h));
        let valid: HashMap<NodeId, Option<Solid>> = self
            .solids
            .drain()
            .filter(|(id, (h, _))| hashes.get(id) == Some(h))
            .map(|(id, (_, s))| (id, s))
            .collect();
        self.stats = IncrementalStats {
            reused_solids: valid.len(),
            ..Default::default()
        };
        valid
    }

    /// Store this evaluation's solids back, fingerprinted.
    pub(crate) fn put_back(
        &mut self,
        cache: HashMap<NodeId, Option<Solid>>,
        hashes: &HashMap<NodeId, u64>,
    ) {
        self.stats.evaluated_solids = cache.len().saturating_sub(self.stats.reused_solids);
        self.solids = cache
            .into_iter()
            .filter_map(|(id, s)| hashes.get(&id).map(|h| (id, (*h, s))))
            .collect();
    }

    /// A root mesh tessellated from the same subtree last time.
    pub(crate) fn mesh(&mut self, root: NodeId, hash: u64) -> Option<EvaluatedMesh> {
        let hit = self
            .meshes
            .get(&root)
            .filter(|(h, _)| *h == hash)
            .map(|(_, m)| m.clone());
        if hit.is_some() {
            self.stats.reused_meshes += 1;
        }
        hit
    }

    pub(crate) fn put_mesh(&mut self, root: NodeId, hash: u64, mesh: &EvaluatedMesh) {
        self.meshes.insert(root, (hash, mesh.clone()));
    }
}

/// Subtree fingerprint of every node: FNV-1a over the node's id and op JSON
/// plus its children's fingerprints, salted with the evaluation `settings`
/// (anything else that changes a node's result, e.g. segment count).
pub fn subtree_hashes(nodes: &HashMap<NodeId, Node>, settings: u64) -> HashMap<NodeId, u64> {
    fn visit(
        id: NodeId,
        nodes: &HashMap<NodeId, Node>,
        settings: u64,
        out: &mut HashMap<NodeId, u64>,
        stack: &mut Vec<NodeId>,
    ) -> u64 {
        if let Some(h) = out.get(&id) {
            return *h;
        }
        let Some(node) = nodes.get(&id) else {
            return 0; // missing child: hashes as absent
        };
        if stack.contains(&id) {
            return 1; // cycle guard; evaluation reports the error
        }
        stack.push(id);
        let mut h = fnv(0xcbf2_9ce4_8422_2325 ^ settings, &id.to_le_bytes());
        h = fnv(
            h,
            serde_json::to_string(&node.op)
                .unwrap_or_default()
                .as_bytes(),
        );
        for child in node.op.child_ids() {
            let c = visit(child, nodes, settings, out, stack);
            h = fnv(h, &c.to_le_bytes());
        }
        stack.pop();
        out.insert(id, h);
        h
    }
    let mut out = HashMap::with_capacity(nodes.len());
    let mut stack = Vec::new();
    for id in nodes.keys() {
        visit(*id, nodes, settings, &mut out, &mut stack);
    }
    out
}

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::evaluate_document_incremental;
    use crate::{evaluate_document, EvalOptions};

    fn doc(x: f64) -> vcad_ir::Document {
        vcad_ir::Document::from_json(&format!(
            r#"{{"version":"0.1","nodes":{{
            "1":{{"id":1,"op":{{"type":"Cube","size":{{"x":10,"y":10,"z":10}}}}}},
            "2":{{"id":2,"op":{{"type":"Cube","size":{{"x":10,"y":10,"z":10}}}}}},
            "3":{{"id":3,"op":{{"type":"Translate","child":2,"offset":{{"x":{x},"y":0,"z":0}}}}}},
            "4":{{"id":4,"op":{{"type":"Cube","size":{{"x":4,"y":4,"z":4}}}}}},
            "5":{{"id":5,"op":{{"type":"Translate","child":4,"offset":{{"x":2,"y":2,"z":8}}}}}}}},
            "materials":{{}},"part_materials":{{}},
            "roots":[{{"root":1,"material":"a"}},{{"root":3,"material":"b"}},{{"root":5,"material":"c"}}]}}"#
        ))
        .unwrap()
    }

    /// Clashes through a reused cache equal a cold evaluation's while one
    /// part slides in and out of contact, and unchanged pairs are cached.
    #[test]
    fn incremental_clashes_match_cold() {
        let opts = EvalOptions::default();
        let mut inc = IncrementalCache::new();
        for x in [5.0, 5.0, 8.0, 20.0, 20.0, 5.0] {
            let d = doc(x);
            let cold = evaluate_document(&d, &opts).unwrap();
            let warm = evaluate_document_incremental(&d, &opts, &mut inc).unwrap();
            assert_eq!(warm.clashes.len(), cold.clashes.len(), "x = {x}");
            for (w, c) in warm.clashes.iter().zip(&cold.clashes) {
                assert_eq!(w.positions, c.positions, "x = {x}");
                assert_eq!(w.indices, c.indices, "x = {x}");
            }
        }
        assert!(!inc.clashes.is_empty());
    }
}
