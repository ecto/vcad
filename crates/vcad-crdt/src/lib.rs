//! CRDT document model for vcad.
//!
//! Provides a conflict-free replicated data type (CRDT) for collaborative CAD
//! document editing. The model uses 4 operation types to express all document
//! mutations, with last-writer-wins (LWW) semantics per feature parameter.

mod fractional_index;
mod hlc;
mod types;

pub use fractional_index::FractionalIndex;
pub use hlc::HLC;
pub use types::*;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Unique identifier for a replica (session/user).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReplicaId(pub u64);

/// Sequence number for operations within a replica.
pub type SeqNo = u64;

/// Globally unique feature identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FeatureId(pub ReplicaId, pub u64);

/// Unique identifier for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpId(pub ReplicaId, pub SeqNo);

/// A single immutable operation in the document history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Op {
    /// Unique operation identifier.
    pub id: OpId,
    /// Hybrid logical clock timestamp.
    pub hlc: HLC,
    /// The action this operation performs.
    pub action: Action,
}

/// The 4 operation types that express all document mutations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    /// Create a new feature at a given position in the feature list.
    CreateFeature {
        /// The feature to create.
        id: FeatureId,
        /// Feature kind (e.g. "cube", "fillet", "boolean").
        kind: String,
        /// Position in the ordered feature list.
        position: FractionalIndex,
    },
    /// Soft-delete a feature.
    DeleteFeature {
        /// The feature to delete.
        id: FeatureId,
    },
    /// Set a parameter on a feature.
    SetParam {
        /// The feature to modify.
        feature: FeatureId,
        /// Parameter key.
        key: String,
        /// Parameter value.
        value: Value,
    },
    /// Move a feature to a new position in the feature list.
    MoveFeature {
        /// The feature to move.
        id: FeatureId,
        /// New position.
        position: FractionalIndex,
    },
}

/// The state of a feature, materialized from the op log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureState {
    /// Feature kind (e.g. "cube", "fillet").
    pub kind: String,
    /// Position in the ordered feature list.
    pub position: FractionalIndex,
    /// Whether this feature has been deleted.
    pub deleted: bool,
    /// Parameters with their values and LWW timestamps.
    pub params: HashMap<String, (Value, HLC)>,
}

/// Set of changes returned from every mutation — drives incremental re-evaluation.
#[derive(Debug, Clone, Default)]
pub struct ChangeSet {
    /// Features whose structure changed (created, deleted, moved).
    pub structural: HashSet<FeatureId>,
    /// Features whose parameters changed, with the set of changed keys.
    pub params: HashMap<FeatureId, HashSet<String>>,
}

/// Serializable format for CrdtDocument (avoids HashMap non-string key issues).
#[derive(Serialize, Deserialize)]
struct SavedDocument {
    replica_id: ReplicaId,
    hlc: HLC,
    next_seq: SeqNo,
    ops: Vec<Op>,
    features: Vec<(FeatureId, FeatureState)>,
    undo_stacks: Vec<(u64, Vec<Vec<OpId>>)>,
    redo_stacks: Vec<(u64, Vec<Vec<OpId>>)>,
    clock: Vec<(u64, SeqNo)>,
}

/// The CRDT document — single source of truth for all document state.
#[derive(Debug, Clone)]
pub struct CrdtDocument {
    replica_id: ReplicaId,
    hlc: HLC,
    next_seq: SeqNo,
    ops: Vec<Op>,
    features: HashMap<FeatureId, FeatureState>,
    undo_stacks: HashMap<ReplicaId, Vec<Vec<OpId>>>,
    redo_stacks: HashMap<ReplicaId, Vec<Vec<OpId>>>,
    clock: BTreeMap<ReplicaId, SeqNo>,
}

impl CrdtDocument {
    /// Create a new empty document for the given replica.
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            replica_id,
            hlc: HLC::new(replica_id),
            next_seq: 0,
            ops: Vec::new(),
            features: HashMap::new(),
            undo_stacks: HashMap::new(),
            redo_stacks: HashMap::new(),
            clock: BTreeMap::new(),
        }
    }

    /// Get the replica ID.
    pub fn replica_id(&self) -> ReplicaId {
        self.replica_id
    }

    // -- Op allocation --

    fn alloc_op(&mut self, action: Action) -> Op {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.hlc.tick();
        let op = Op {
            id: OpId(self.replica_id, seq),
            hlc: self.hlc,
            action,
        };
        self.clock.insert(self.replica_id, seq);
        op
    }

    fn apply_op(&mut self, op: &Op) -> ChangeSet {
        let mut cs = ChangeSet::default();
        match &op.action {
            Action::CreateFeature { id, kind, position } => {
                self.features.insert(
                    *id,
                    FeatureState {
                        kind: kind.clone(),
                        position: position.clone(),
                        deleted: false,
                        params: HashMap::new(),
                    },
                );
                cs.structural.insert(*id);
            }
            Action::DeleteFeature { id } => {
                if let Some(f) = self.features.get_mut(id) {
                    f.deleted = true;
                    cs.structural.insert(*id);
                }
            }
            Action::SetParam {
                feature,
                key,
                value,
            } => {
                if let Some(f) = self.features.get_mut(feature) {
                    let should_update = match f.params.get(key.as_str()) {
                        Some((_, existing_hlc)) => op.hlc > *existing_hlc,
                        None => true,
                    };
                    if should_update {
                        f.params.insert(key.clone(), (value.clone(), op.hlc));
                        cs.params.entry(*feature).or_default().insert(key.clone());
                    }
                }
            }
            Action::MoveFeature { id, position } => {
                if let Some(f) = self.features.get_mut(id) {
                    f.position = position.clone();
                    cs.structural.insert(*id);
                }
            }
        }
        cs
    }

    fn push_undo_group(&mut self, op_ids: Vec<OpId>) {
        self.undo_stacks
            .entry(self.replica_id)
            .or_default()
            .push(op_ids);
        // Clear redo on new action.
        self.redo_stacks.entry(self.replica_id).or_default().clear();
    }

    // -- Public mutations --

    /// Create a new feature with the given kind, position, and initial parameters.
    pub fn create_feature(
        &mut self,
        kind: &str,
        position: FractionalIndex,
        params: HashMap<String, Value>,
    ) -> (FeatureId, ChangeSet) {
        let feat_count = self.features.len() as u64;
        let fid = FeatureId(self.replica_id, feat_count);

        let create_op = self.alloc_op(Action::CreateFeature {
            id: fid,
            kind: kind.to_string(),
            position,
        });
        let create_id = create_op.id;
        let mut cs = self.apply_op(&create_op);
        self.ops.push(create_op);

        let mut op_ids = vec![create_id];

        // Sort params by key for deterministic ordering.
        let mut sorted_params: Vec<_> = params.into_iter().collect();
        sorted_params.sort_by(|a, b| a.0.cmp(&b.0));

        for (key, value) in sorted_params {
            let param_op = self.alloc_op(Action::SetParam {
                feature: fid,
                key,
                value,
            });
            op_ids.push(param_op.id);
            let param_cs = self.apply_op(&param_op);
            self.ops.push(param_op);
            cs.merge(param_cs);
        }

        self.push_undo_group(op_ids);
        (fid, cs)
    }

    /// Soft-delete a feature.
    pub fn delete_feature(&mut self, id: FeatureId) -> ChangeSet {
        let op = self.alloc_op(Action::DeleteFeature { id });
        let op_id = op.id;
        let cs = self.apply_op(&op);
        self.ops.push(op);
        self.push_undo_group(vec![op_id]);
        cs
    }

    /// Set a parameter on a feature.
    pub fn set_param(&mut self, feature: FeatureId, key: &str, value: Value) -> ChangeSet {
        let op = self.alloc_op(Action::SetParam {
            feature,
            key: key.to_string(),
            value,
        });
        let op_id = op.id;
        let cs = self.apply_op(&op);
        self.ops.push(op);
        self.push_undo_group(vec![op_id]);
        cs
    }

    /// Move a feature to a new position in the ordered list.
    pub fn move_feature(&mut self, id: FeatureId, position: FractionalIndex) -> ChangeSet {
        let op = self.alloc_op(Action::MoveFeature { id, position });
        let op_id = op.id;
        let cs = self.apply_op(&op);
        self.ops.push(op);
        self.push_undo_group(vec![op_id]);
        cs
    }

    /// Undo the last operation group for this replica.
    ///
    /// Per-user undo: pops the undo stack, computes inverse operations,
    /// and emits new ops with higher HLC that restore previous values.
    pub fn undo(&mut self) -> Option<ChangeSet> {
        let stack = self.undo_stacks.get_mut(&self.replica_id)?;
        let group = stack.pop()?;

        let mut cs = ChangeSet::default();

        // Process ops in reverse order for correct undo semantics.
        for op_id in group.iter().rev() {
            if let Some(original_op) = self.ops.iter().find(|o| o.id == *op_id).cloned() {
                let op_cs = self.apply_inverse(&original_op);
                cs.merge(op_cs);
            }
        }

        // Push the original group to redo stack.
        self.redo_stacks
            .entry(self.replica_id)
            .or_default()
            .push(group);

        Some(cs)
    }

    /// Redo the last undone operation group for this replica.
    pub fn redo(&mut self) -> Option<ChangeSet> {
        let stack = self.redo_stacks.get_mut(&self.replica_id)?;
        let group = stack.pop()?;

        let mut cs = ChangeSet::default();

        // Re-apply the original operations.
        for op_id in &group {
            if let Some(original_op) = self.ops.iter().find(|o| o.id == *op_id).cloned() {
                let op_cs = self.reapply(&original_op);
                cs.merge(op_cs);
            }
        }

        // Push back to undo stack.
        self.undo_stacks
            .entry(self.replica_id)
            .or_default()
            .push(group);

        Some(cs)
    }

    /// Apply the inverse of an operation for undo.
    fn apply_inverse(&mut self, original: &Op) -> ChangeSet {
        match &original.action {
            Action::CreateFeature { id, .. } => {
                // Inverse of create = soft delete.
                if let Some(f) = self.features.get_mut(id) {
                    f.deleted = true;
                }
                let mut cs = ChangeSet::default();
                cs.structural.insert(*id);
                cs
            }
            Action::DeleteFeature { id } => {
                // Inverse of delete = un-delete.
                if let Some(f) = self.features.get_mut(id) {
                    f.deleted = false;
                }
                let mut cs = ChangeSet::default();
                cs.structural.insert(*id);
                cs
            }
            Action::SetParam {
                feature,
                key,
                value: _,
            } => {
                // Restore previous value.
                if let Some(prev) = self.find_previous_value(*feature, key, original.id) {
                    let op = self.alloc_op(Action::SetParam {
                        feature: *feature,
                        key: key.clone(),
                        value: prev,
                    });
                    let cs = self.apply_op(&op);
                    self.ops.push(op);
                    cs
                } else {
                    // No previous value — remove the param entirely.
                    if let Some(f) = self.features.get_mut(feature) {
                        f.params.remove(key.as_str());
                    }
                    let mut cs = ChangeSet::default();
                    cs.params.entry(*feature).or_default().insert(key.clone());
                    cs
                }
            }
            Action::MoveFeature { id, .. } => {
                if let Some(prev) = self.find_previous_position(*id, original.id) {
                    let op = self.alloc_op(Action::MoveFeature {
                        id: *id,
                        position: prev,
                    });
                    let cs = self.apply_op(&op);
                    self.ops.push(op);
                    cs
                } else {
                    ChangeSet::default()
                }
            }
        }
    }

    /// Re-apply an original operation for redo.
    fn reapply(&mut self, original: &Op) -> ChangeSet {
        match &original.action {
            Action::CreateFeature { id, .. } => {
                // Re-create = un-delete.
                if let Some(f) = self.features.get_mut(id) {
                    f.deleted = false;
                }
                let mut cs = ChangeSet::default();
                cs.structural.insert(*id);
                cs
            }
            Action::DeleteFeature { id } => {
                // Re-delete.
                if let Some(f) = self.features.get_mut(id) {
                    f.deleted = true;
                }
                let mut cs = ChangeSet::default();
                cs.structural.insert(*id);
                cs
            }
            Action::SetParam {
                feature,
                key,
                value,
            } => {
                let op = self.alloc_op(Action::SetParam {
                    feature: *feature,
                    key: key.clone(),
                    value: value.clone(),
                });
                let cs = self.apply_op(&op);
                self.ops.push(op);
                cs
            }
            Action::MoveFeature { id, position } => {
                let op = self.alloc_op(Action::MoveFeature {
                    id: *id,
                    position: position.clone(),
                });
                let cs = self.apply_op(&op);
                self.ops.push(op);
                cs
            }
        }
    }

    /// Whether undo is available for this replica.
    pub fn can_undo(&self) -> bool {
        self.undo_stacks
            .get(&self.replica_id)
            .is_some_and(|s| !s.is_empty())
    }

    /// Whether redo is available for this replica.
    pub fn can_redo(&self) -> bool {
        self.redo_stacks
            .get(&self.replica_id)
            .is_some_and(|s| !s.is_empty())
    }

    // -- Merge --

    /// Merge remote operations into this document.
    pub fn merge(&mut self, remote_ops: Vec<Op>) -> ChangeSet {
        let mut cs = ChangeSet::default();
        for op in remote_ops {
            // Update our HLC to be at least as large as the remote.
            self.hlc.receive(&op.hlc);
            // Track remote clock.
            let OpId(replica, seq) = op.id;
            let current = self.clock.get(&replica).copied();
            match current {
                None => {
                    self.clock.insert(replica, seq);
                }
                Some(cur) if seq > cur => {
                    self.clock.insert(replica, seq);
                }
                _ => {}
            }
            let op_cs = self.apply_op(&op);
            self.ops.push(op);
            cs.merge(op_cs);
        }
        cs
    }

    // -- Queries --

    /// Get all non-deleted features, ordered by position.
    pub fn ordered_features(&self) -> Vec<(FeatureId, &FeatureState)> {
        let mut features: Vec<_> = self
            .features
            .iter()
            .filter(|(_, f)| !f.deleted)
            .map(|(id, f)| (*id, f))
            .collect();
        features.sort_by(|a, b| a.1.position.cmp(&b.1.position));
        features
    }

    /// Get a feature by ID.
    pub fn get_feature(&self, id: FeatureId) -> Option<&FeatureState> {
        self.features.get(&id)
    }

    /// Get the full feature map (including deleted).
    pub fn features(&self) -> &HashMap<FeatureId, FeatureState> {
        &self.features
    }

    // -- Sync --

    /// Get all operations since a remote clock state.
    pub fn ops_since(&self, remote_clock: &BTreeMap<ReplicaId, SeqNo>) -> Vec<Op> {
        self.ops
            .iter()
            .filter(|op| {
                let OpId(replica, seq) = op.id;
                match remote_clock.get(&replica) {
                    // Remote hasn't seen any ops from this replica.
                    None => true,
                    // Include if seq is newer than what remote has seen.
                    Some(&remote_seq) => seq > remote_seq,
                }
            })
            .cloned()
            .collect()
    }

    /// Get the current clock state (for sync protocol).
    pub fn clock(&self) -> &BTreeMap<ReplicaId, SeqNo> {
        &self.clock
    }

    // -- Persistence --

    /// Serialize to bytes for storage.
    pub fn save(&self) -> Vec<u8> {
        let saved = SavedDocument {
            replica_id: self.replica_id,
            hlc: self.hlc,
            next_seq: self.next_seq,
            ops: self.ops.clone(),
            features: self.features.iter().map(|(k, v)| (*k, v.clone())).collect(),
            undo_stacks: self
                .undo_stacks
                .iter()
                .map(|(k, v)| (k.0, v.clone()))
                .collect(),
            redo_stacks: self
                .redo_stacks
                .iter()
                .map(|(k, v)| (k.0, v.clone()))
                .collect(),
            clock: self.clock.iter().map(|(k, v)| (k.0, *v)).collect(),
        };
        serde_json::to_vec(&saved).expect("serialization should not fail")
    }

    /// Deserialize from bytes.
    pub fn load(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let saved: SavedDocument = serde_json::from_slice(bytes)?;
        Ok(Self {
            replica_id: saved.replica_id,
            hlc: saved.hlc,
            next_seq: saved.next_seq,
            ops: saved.ops,
            features: saved.features.into_iter().collect(),
            undo_stacks: saved
                .undo_stacks
                .into_iter()
                .map(|(k, v)| (ReplicaId(k), v))
                .collect(),
            redo_stacks: saved
                .redo_stacks
                .into_iter()
                .map(|(k, v)| (ReplicaId(k), v))
                .collect(),
            clock: saved
                .clock
                .into_iter()
                .map(|(k, v)| (ReplicaId(k), v))
                .collect(),
        })
    }

    // -- Helpers --

    /// Find the value of a parameter before a given operation.
    fn find_previous_value(&self, feature: FeatureId, key: &str, before: OpId) -> Option<Value> {
        let mut prev: Option<(Value, HLC)> = None;
        for op in &self.ops {
            if op.id == before {
                break;
            }
            if let Action::SetParam {
                feature: f,
                key: k,
                value: v,
            } = &op.action
            {
                if *f == feature && k == key {
                    match &prev {
                        Some((_, hlc)) if op.hlc <= *hlc => {}
                        _ => prev = Some((v.clone(), op.hlc)),
                    }
                }
            }
        }
        prev.map(|(v, _)| v)
    }

    /// Find the position of a feature before a given operation.
    fn find_previous_position(&self, id: FeatureId, before: OpId) -> Option<FractionalIndex> {
        let mut prev: Option<FractionalIndex> = None;
        for op in &self.ops {
            if op.id == before {
                break;
            }
            match &op.action {
                Action::CreateFeature {
                    id: fid, position, ..
                }
                | Action::MoveFeature {
                    id: fid, position, ..
                } if *fid == id => {
                    prev = Some(position.clone());
                }
                _ => {}
            }
        }
        prev
    }
}

impl ChangeSet {
    /// Merge another changeset into this one.
    pub fn merge(&mut self, other: ChangeSet) {
        self.structural.extend(other.structural);
        for (fid, keys) in other.params {
            self.params.entry(fid).or_default().extend(keys);
        }
    }

    /// Whether this changeset is empty.
    pub fn is_empty(&self) -> bool {
        self.structural.is_empty() && self.params.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_query_feature() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        let params = HashMap::from([
            ("size_x".to_string(), Value::F64(10.0)),
            ("size_y".to_string(), Value::F64(20.0)),
            ("size_z".to_string(), Value::F64(30.0)),
        ]);
        let (fid, cs) = doc.create_feature("cube", FractionalIndex::between(None, None), params);

        assert!(cs.structural.contains(&fid));
        let features = doc.ordered_features();
        assert_eq!(features.len(), 1);
        assert_eq!(features[0].1.kind, "cube");

        let f = doc.get_feature(fid).unwrap();
        assert_eq!(f.params.get("size_x").unwrap().0, Value::F64(10.0));
        assert_eq!(f.params.get("size_y").unwrap().0, Value::F64(20.0));
        assert_eq!(f.params.get("size_z").unwrap().0, Value::F64(30.0));
    }

    #[test]
    fn test_delete_feature() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        let (fid, _) =
            doc.create_feature("cube", FractionalIndex::between(None, None), HashMap::new());
        assert_eq!(doc.ordered_features().len(), 1);

        doc.delete_feature(fid);
        assert_eq!(doc.ordered_features().len(), 0);
    }

    #[test]
    fn test_set_param_new() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        let (fid, _) =
            doc.create_feature("cube", FractionalIndex::between(None, None), HashMap::new());

        let cs = doc.set_param(fid, "size_x", Value::F64(42.0));
        assert!(cs.params.contains_key(&fid));

        let f = doc.get_feature(fid).unwrap();
        assert_eq!(f.params.get("size_x").unwrap().0, Value::F64(42.0));
    }

    #[test]
    fn test_set_param_overwrite() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        let (fid, _) = doc.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([("size_x".to_string(), Value::F64(10.0))]),
        );

        // Verify initial value was set by create_feature.
        assert_eq!(
            doc.get_feature(fid)
                .unwrap()
                .params
                .get("size_x")
                .unwrap()
                .0,
            Value::F64(10.0),
            "initial param should be 10.0"
        );

        // Overwrite the param.
        let cs = doc.set_param(fid, "size_x", Value::F64(20.0));
        assert!(
            cs.params.contains_key(&fid),
            "changeset should include the feature"
        );

        assert_eq!(
            doc.get_feature(fid)
                .unwrap()
                .params
                .get("size_x")
                .unwrap()
                .0,
            Value::F64(20.0),
            "param should be updated to 20.0"
        );
    }

    #[test]
    fn test_move_feature() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        let (fid1, _) =
            doc.create_feature("cube", FractionalIndex::between(None, None), HashMap::new());
        let (fid2, _) = doc.create_feature(
            "cylinder",
            FractionalIndex::between(Some(&doc.get_feature(fid1).unwrap().position), None),
            HashMap::new(),
        );

        // Move fid2 before fid1
        let new_pos =
            FractionalIndex::between(None, Some(&doc.get_feature(fid1).unwrap().position));
        doc.move_feature(fid2, new_pos);

        let features = doc.ordered_features();
        assert_eq!(features[0].0, fid2);
        assert_eq!(features[1].0, fid1);
    }

    #[test]
    fn test_undo_redo_set_param() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        let (fid, _) = doc.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([("size_x".to_string(), Value::F64(10.0))]),
        );

        doc.set_param(fid, "size_x", Value::F64(20.0));
        assert_eq!(
            doc.get_feature(fid)
                .unwrap()
                .params
                .get("size_x")
                .unwrap()
                .0,
            Value::F64(20.0)
        );

        // Undo set_param
        assert!(doc.can_undo());
        let cs = doc.undo().unwrap();
        assert!(!cs.is_empty());
        assert_eq!(
            doc.get_feature(fid)
                .unwrap()
                .params
                .get("size_x")
                .unwrap()
                .0,
            Value::F64(10.0)
        );

        // Redo
        assert!(doc.can_redo());
        doc.redo().unwrap();
        assert_eq!(
            doc.get_feature(fid)
                .unwrap()
                .params
                .get("size_x")
                .unwrap()
                .0,
            Value::F64(20.0)
        );
    }

    #[test]
    fn test_undo_create() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        let (fid, _) =
            doc.create_feature("cube", FractionalIndex::between(None, None), HashMap::new());
        assert_eq!(doc.ordered_features().len(), 1);

        doc.undo().unwrap();
        assert_eq!(doc.ordered_features().len(), 0);

        doc.redo().unwrap();
        let f = doc.get_feature(fid).unwrap();
        assert!(!f.deleted);
        assert_eq!(doc.ordered_features().len(), 1);
    }

    #[test]
    fn test_merge_two_replicas() {
        let mut doc1 = CrdtDocument::new(ReplicaId(1));
        let mut doc2 = CrdtDocument::new(ReplicaId(2));

        let (fid1, _) = doc1.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([("size_x".to_string(), Value::F64(10.0))]),
        );

        let (fid2, _) = doc2.create_feature(
            "cylinder",
            FractionalIndex::between(None, None),
            HashMap::from([("radius".to_string(), Value::F64(5.0))]),
        );

        // Sync: doc1 gets doc2's ops
        let ops_for_1 = doc2.ops_since(doc1.clock());
        let cs = doc1.merge(ops_for_1);
        assert!(!cs.is_empty());
        assert_eq!(doc1.ordered_features().len(), 2);

        // Sync: doc2 gets doc1's ops
        let ops_for_2 = doc1.ops_since(doc2.clock());
        doc2.merge(ops_for_2);
        assert_eq!(doc2.ordered_features().len(), 2);

        // Both should see both features
        assert!(doc1.get_feature(fid1).is_some());
        assert!(doc1.get_feature(fid2).is_some());
        assert!(doc2.get_feature(fid1).is_some());
        assert!(doc2.get_feature(fid2).is_some());
    }

    #[test]
    fn test_lww_conflict_resolution() {
        let mut doc1 = CrdtDocument::new(ReplicaId(1));
        let mut doc2 = CrdtDocument::new(ReplicaId(2));

        // Create on doc1, sync to doc2
        let (fid, _) = doc1.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([("size_x".to_string(), Value::F64(10.0))]),
        );
        let ops = doc1.ops_since(doc2.clock());
        doc2.merge(ops);

        // Concurrent edits
        doc1.set_param(fid, "size_x", Value::F64(20.0));
        doc2.set_param(fid, "size_x", Value::F64(30.0));

        // Sync both ways — LWW should pick a consistent winner
        let ops_for_2 = doc1.ops_since(doc2.clock());
        let ops_for_1 = doc2.ops_since(doc1.clock());
        doc1.merge(ops_for_1);
        doc2.merge(ops_for_2);

        // Both docs should agree
        let v1 = &doc1
            .get_feature(fid)
            .unwrap()
            .params
            .get("size_x")
            .unwrap()
            .0;
        let v2 = &doc2
            .get_feature(fid)
            .unwrap()
            .params
            .get("size_x")
            .unwrap()
            .0;
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_save_load_roundtrip() {
        let mut doc = CrdtDocument::new(ReplicaId(1));
        doc.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("size_x".to_string(), Value::F64(10.0)),
                ("size_y".to_string(), Value::F64(20.0)),
            ]),
        );

        let bytes = doc.save();
        let loaded = CrdtDocument::load(&bytes).unwrap();

        assert_eq!(loaded.ordered_features().len(), 1);
        let f = loaded.ordered_features()[0].1;
        assert_eq!(f.kind, "cube");
        assert_eq!(f.params.get("size_x").unwrap().0, Value::F64(10.0));
    }

    /// Regression: the IR-Document round-trip silently dropped any CRDT param
    /// that the materializer didn't happen to surface (e.g. `name` was the
    /// immediate bug). The raw CRDT bytes format MUST preserve every param
    /// verbatim — this test locks that contract for future-added params.
    #[test]
    fn test_save_load_preserves_arbitrary_params() {
        let mut doc = CrdtDocument::new(ReplicaId(7));
        let (_fid, _) = doc.create_feature(
            "cube",
            FractionalIndex::between(None, None),
            HashMap::from([
                ("size_x".to_string(), Value::F64(10.0)),
                ("size_y".to_string(), Value::F64(20.0)),
                ("size_z".to_string(), Value::F64(30.0)),
                ("name".to_string(), Value::String("MyCube".into())),
                ("material".to_string(), Value::String("brass".into())),
                ("visible".to_string(), Value::Bool(false)),
                ("offset".to_string(), Value::Vec3([1.0, 2.0, 3.0])),
                // A hypothetical future param the materializer doesn't know
                // about — it must still survive a save/load cycle.
                (
                    "custom_future_param".to_string(),
                    Value::String("hello".into()),
                ),
            ]),
        );

        let bytes = doc.save();
        let loaded = CrdtDocument::load(&bytes).unwrap();
        let features = loaded.ordered_features();
        assert_eq!(features.len(), 1);
        let f = features[0].1;
        assert_eq!(f.kind, "cube");
        assert_eq!(
            f.params.get("name").map(|(v, _)| v),
            Some(&Value::String("MyCube".into())),
        );
        assert_eq!(
            f.params.get("material").map(|(v, _)| v),
            Some(&Value::String("brass".into())),
        );
        assert_eq!(
            f.params.get("visible").map(|(v, _)| v),
            Some(&Value::Bool(false)),
        );
        assert_eq!(
            f.params.get("offset").map(|(v, _)| v),
            Some(&Value::Vec3([1.0, 2.0, 3.0])),
        );
        assert_eq!(
            f.params.get("custom_future_param").map(|(v, _)| v),
            Some(&Value::String("hello".into())),
        );

        // The op log must also survive so future sync / merge still works.
        assert!(!loaded.ops.is_empty(), "op log must be preserved on load");
        assert_eq!(loaded.clock(), doc.clock(), "vector clock must roundtrip");
    }

    #[test]
    fn test_feature_ordering() {
        let mut doc = CrdtDocument::new(ReplicaId(1));

        let pos_a = FractionalIndex::between(None, None);
        let (fid_a, _) = doc.create_feature("a", pos_a, HashMap::new());

        let pos_b = FractionalIndex::between(Some(&doc.get_feature(fid_a).unwrap().position), None);
        let (fid_b, _) = doc.create_feature("b", pos_b, HashMap::new());

        let pos_c = FractionalIndex::between(
            Some(&doc.get_feature(fid_a).unwrap().position),
            Some(&doc.get_feature(fid_b).unwrap().position),
        );
        let (_fid_c, _) = doc.create_feature("c", pos_c, HashMap::new());

        let ordered = doc.ordered_features();
        assert_eq!(ordered[0].1.kind, "a");
        assert_eq!(ordered[1].1.kind, "c");
        assert_eq!(ordered[2].1.kind, "b");
    }
}
