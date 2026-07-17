//! Semantic diff and three-way merge for `.vcad` documents — "git for CAD".
//!
//! `.vcad` files are JSON documents holding a parametric DAG of operations,
//! part definitions, instances, joints, materials, and parameters. Line-based
//! text diff is useless for them: a reordered `HashMap` serialization or a
//! single parameter change produces a wall of noise. This crate diffs and
//! merges at the *feature* level instead.
//!
//! Every document is decomposed into a flat map of entities — nodes, scene
//! roots, materials, part definitions, instances, joints, parameters,
//! bindings, clearance specs, and document-level singletons — each keyed by
//! its stable id (never by array position). Two documents diff entity-by-
//! entity; modified entities report field-level changes as dotted paths with
//! old → new values.
//!
//! Three-way merge is **fail-closed**: non-conflicting changes from both
//! sides auto-merge, but when both sides touched the same entity field (or
//! one side deleted what the other modified) the merge reports an explicit
//! [`Conflict`] and produces no output document. It never silently picks a
//! side.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use vcad_ir::Document;

// ============================================================================
// Entity model
// ============================================================================

/// The kind of document entity a change applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntityKind {
    /// A node in the CSG/feature DAG (`nodes`), keyed by node id.
    Node,
    /// A material definition (`materials`), keyed by material name.
    Material,
    /// A part-name → material-name assignment (`part_materials`).
    PartMaterial,
    /// A scene root entry (`roots`), keyed by root node id.
    Root,
    /// An assembly part definition (`partDefs`), keyed by part-def id.
    PartDef,
    /// An assembly instance (`instances`), keyed by instance id.
    Instance,
    /// An assembly joint (`joints`), keyed by joint id.
    Joint,
    /// A named document parameter (`parameters`).
    Parameter,
    /// An expression binding (`bindings`), keyed by `(node, field)` key.
    Binding,
    /// A clearance assertion (`clearance_specs`), keyed by label.
    Clearance,
    /// A document-level singleton field (version, scene, schematic, …).
    DocField,
}

impl EntityKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Material => "material",
            Self::PartMaterial => "part-material",
            Self::Root => "root",
            Self::PartDef => "part-def",
            Self::Instance => "instance",
            Self::Joint => "joint",
            Self::Parameter => "parameter",
            Self::Binding => "binding",
            Self::Clearance => "clearance",
            Self::DocField => "doc",
        }
    }
}

/// Stable identity of an entity: its kind plus its id within that kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityKey {
    /// What table the entity lives in.
    pub kind: EntityKind,
    /// Stable id within the table (node id, joint id, material name, …).
    pub id: String,
}

impl EntityKey {
    fn new(kind: EntityKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

impl std::fmt::Display for EntityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.kind.as_str(), self.id)
    }
}

/// A document decomposed into id-keyed entities plus ordering hints for the
/// vector-backed tables (`roots`, `instances`, `joints`, `clearance_specs`).
#[derive(Debug, Clone)]
struct Decomposed {
    entities: BTreeMap<EntityKey, Value>,
    /// Original in-file ordering of ids, per vector-backed kind.
    order: BTreeMap<EntityKind, Vec<String>>,
}

/// Document-level singleton fields treated as one entity each.
const DOC_FIELDS: &[&str] = &[
    "version",
    "scene",
    "groundInstanceId",
    "schematic",
    "pcb",
    "molecule",
    "timeline",
];

/// Errors produced by diff/merge operations.
#[derive(Debug)]
pub enum DiffError {
    /// The document JSON did not have the expected shape.
    Shape(String),
    /// (De)serialization failed.
    Json(serde_json::Error),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape(msg) => write!(f, "unexpected document shape: {msg}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
        }
    }
}

impl std::error::Error for DiffError {}

impl From<serde_json::Error> for DiffError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

fn decompose(doc: &Document) -> Result<Decomposed, DiffError> {
    let value = serde_json::to_value(doc)?;
    let obj = value
        .as_object()
        .ok_or_else(|| DiffError::Shape("document is not a JSON object".into()))?;

    let mut entities = BTreeMap::new();
    let mut order: BTreeMap<EntityKind, Vec<String>> = BTreeMap::new();

    // Map-backed tables: id is the JSON object key.
    let map_tables: &[(&str, EntityKind)] = &[
        ("nodes", EntityKind::Node),
        ("materials", EntityKind::Material),
        ("part_materials", EntityKind::PartMaterial),
        ("partDefs", EntityKind::PartDef),
        ("parameters", EntityKind::Parameter),
        ("bindings", EntityKind::Binding),
    ];
    for (field, kind) in map_tables {
        if let Some(Value::Object(table)) = obj.get(*field) {
            for (id, v) in table {
                entities.insert(EntityKey::new(*kind, id.clone()), v.clone());
            }
        }
    }

    // Vector-backed tables: id is a stable field on each element.
    type IdOf = fn(&Value) -> Option<String>;
    let vec_tables: &[(&str, EntityKind, IdOf)] = &[
        ("roots", EntityKind::Root, |v| {
            v.get("root").map(json_id_string)
        }),
        ("instances", EntityKind::Instance, |v| {
            v.get("id").map(json_id_string)
        }),
        ("joints", EntityKind::Joint, |v| {
            v.get("id").map(json_id_string)
        }),
        ("clearance_specs", EntityKind::Clearance, |v| {
            v.get("label").map(json_id_string)
        }),
    ];
    for (field, kind, id_of) in vec_tables {
        if let Some(Value::Array(items)) = obj.get(*field) {
            let ids = order.entry(*kind).or_default();
            for item in items {
                let id = id_of(item).ok_or_else(|| {
                    DiffError::Shape(format!("element of `{field}` has no id field"))
                })?;
                ids.push(id.clone());
                entities.insert(EntityKey::new(*kind, id), item.clone());
            }
        }
    }

    // Document-level singletons.
    for field in DOC_FIELDS {
        if let Some(v) = obj.get(*field) {
            if !v.is_null() {
                entities.insert(EntityKey::new(EntityKind::DocField, *field), v.clone());
            }
        }
    }

    Ok(Decomposed { entities, order })
}

/// Render a JSON id value (string or number) as its string form.
fn json_id_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn recompose(
    entities: &BTreeMap<EntityKey, Value>,
    order: &BTreeMap<EntityKind, Vec<String>>,
) -> Result<Document, DiffError> {
    let mut obj = Map::new();

    let map_field: &[(EntityKind, &str)] = &[
        (EntityKind::Node, "nodes"),
        (EntityKind::Material, "materials"),
        (EntityKind::PartMaterial, "part_materials"),
        (EntityKind::PartDef, "partDefs"),
        (EntityKind::Parameter, "parameters"),
        (EntityKind::Binding, "bindings"),
    ];
    let vec_field: &[(EntityKind, &str)] = &[
        (EntityKind::Root, "roots"),
        (EntityKind::Instance, "instances"),
        (EntityKind::Joint, "joints"),
        (EntityKind::Clearance, "clearance_specs"),
    ];

    for (kind, field) in map_field {
        let mut table = Map::new();
        for (key, v) in entities.iter().filter(|(k, _)| k.kind == *kind) {
            table.insert(key.id.clone(), v.clone());
        }
        // partDefs is Option<HashMap> — omit when empty so old docs stay
        // wire-identical; nodes/materials/part_materials are required fields.
        if !table.is_empty()
            || matches!(
                *kind,
                EntityKind::Node | EntityKind::Material | EntityKind::PartMaterial
            )
        {
            obj.insert((*field).to_string(), Value::Object(table));
        }
    }

    for (kind, field) in vec_field {
        let present: BTreeMap<&str, &Value> = entities
            .iter()
            .filter(|(k, _)| k.kind == *kind)
            .map(|(k, v)| (k.id.as_str(), v))
            .collect();
        let mut items = Vec::new();
        let mut emitted: BTreeSet<&str> = BTreeSet::new();
        for id in order.get(kind).map(|v| v.as_slice()).unwrap_or(&[]) {
            if let Some(v) = present.get(id.as_str()) {
                if emitted.insert(id.as_str()) {
                    items.push((*v).clone());
                }
            }
        }
        for (id, v) in &present {
            if emitted.insert(id) {
                items.push((*v).clone());
            }
        }
        if !items.is_empty() || *kind == EntityKind::Root {
            obj.insert((*field).to_string(), Value::Array(items));
        }
    }

    for field in DOC_FIELDS {
        if let Some(v) = entities.get(&EntityKey::new(EntityKind::DocField, *field)) {
            obj.insert((*field).to_string(), v.clone());
        }
    }
    if !obj.contains_key("version") {
        obj.insert("version".into(), Value::String("0.1".into()));
    }

    Ok(serde_json::from_value(Value::Object(obj))?)
}

// ============================================================================
// Diff
// ============================================================================

/// One field-level change inside a modified entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldChange {
    /// Dotted path into the entity's JSON value, e.g. `op.size.2`.
    pub path: String,
    /// Value on the old side.
    pub old: Value,
    /// Value on the new side.
    pub new: Value,
}

/// How an entity changed between two documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ChangeKind {
    /// The entity exists only in the new document.
    Added {
        /// The full new entity value.
        value: Value,
    },
    /// The entity exists only in the old document.
    Removed {
        /// The full old entity value.
        value: Value,
    },
    /// The entity exists in both documents with different content.
    Modified {
        /// Field-level changes, old → new.
        fields: Vec<FieldChange>,
    },
}

/// A change to one entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityChange {
    /// Which entity changed.
    #[serde(flatten)]
    pub key: EntityKey,
    /// Optional human-readable name of the entity (from its `name` field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What happened to it.
    #[serde(flatten)]
    pub change: ChangeKind,
}

/// A structured, feature-level diff between two documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DocumentDiff {
    /// All entity changes, sorted by kind then id.
    pub changes: Vec<EntityChange>,
}

impl DocumentDiff {
    /// True when the two documents are semantically identical.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Recursively diff two JSON values, emitting dotted-path field changes.
///
/// Objects recurse per key; equal-length arrays recurse per index; anything
/// else (scalars, arrays of differing length, type changes) is reported
/// atomically at the current path.
fn value_diff(prefix: &str, old: &Value, new: &Value, out: &mut Vec<FieldChange>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
            for k in keys {
                let path = join_path(prefix, k);
                value_diff(
                    &path,
                    a.get(k).unwrap_or(&Value::Null),
                    b.get(k).unwrap_or(&Value::Null),
                    out,
                );
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (av, bv)) in a.iter().zip(b).enumerate() {
                value_diff(&join_path(prefix, &i.to_string()), av, bv, out);
            }
        }
        _ => out.push(FieldChange {
            path: prefix.to_string(),
            old: old.clone(),
            new: new.clone(),
        }),
    }
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

fn entity_name(v: &Value) -> Option<String> {
    v.get("name").and_then(Value::as_str).map(str::to_string)
}

/// Compute a structured feature-level diff between two documents.
///
/// Entities are matched by stable id (node id, joint id, material name, …),
/// never by array position, so reordering alone produces an empty diff.
pub fn diff(old: &Document, new: &Document) -> Result<DocumentDiff, DiffError> {
    let a = decompose(old)?;
    let b = decompose(new)?;

    let keys: BTreeSet<&EntityKey> = a.entities.keys().chain(b.entities.keys()).collect();
    let mut changes = Vec::new();
    for key in keys {
        let change = match (a.entities.get(key), b.entities.get(key)) {
            (None, Some(v)) => Some((ChangeKind::Added { value: v.clone() }, entity_name(v))),
            (Some(v), None) => Some((ChangeKind::Removed { value: v.clone() }, entity_name(v))),
            (Some(va), Some(vb)) if va != vb => {
                let mut fields = Vec::new();
                value_diff("", va, vb, &mut fields);
                Some((
                    ChangeKind::Modified { fields },
                    entity_name(vb).or_else(|| entity_name(va)),
                ))
            }
            _ => None,
        };
        if let Some((change, name)) = change {
            changes.push(EntityChange {
                key: key.clone(),
                name,
                change,
            });
        }
    }
    Ok(DocumentDiff { changes })
}

/// Apply a diff produced by [`diff`]`(old, new)` to `old`, reconstructing a
/// document semantically equal to `new`. Used for round-trip verification
/// and patch-style workflows.
pub fn apply(old: &Document, diff: &DocumentDiff) -> Result<Document, DiffError> {
    let mut d = decompose(old)?;
    for ec in &diff.changes {
        match &ec.change {
            ChangeKind::Added { value } => {
                d.entities.insert(ec.key.clone(), value.clone());
                if let Some(ids) = d.order.get_mut(&ec.key.kind) {
                    ids.push(ec.key.id.clone());
                }
            }
            ChangeKind::Removed { .. } => {
                d.entities.remove(&ec.key);
            }
            ChangeKind::Modified { fields } => {
                let v = d.entities.get_mut(&ec.key).ok_or_else(|| {
                    DiffError::Shape(format!("patch modifies missing entity {}", ec.key))
                })?;
                for fc in fields {
                    set_path(v, &fc.path, fc.new.clone())?;
                }
            }
        }
    }
    recompose(&d.entities, &d.order)
}

/// Set a dotted path inside a JSON value, creating object keys as needed.
fn set_path(root: &mut Value, path: &str, new: Value) -> Result<(), DiffError> {
    if path.is_empty() {
        *root = new;
        return Ok(());
    }
    let mut cur = root;
    let segments: Vec<&str> = path.split('.').collect();
    for (i, seg) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match cur {
            Value::Object(map) => {
                if last {
                    if new.is_null() {
                        map.remove(*seg);
                    } else {
                        map.insert((*seg).to_string(), new);
                    }
                    return Ok(());
                }
                cur = map
                    .entry((*seg).to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
            }
            Value::Array(items) => {
                let idx: usize = seg.parse().map_err(|_| {
                    DiffError::Shape(format!("non-numeric array index `{seg}` in path `{path}`"))
                })?;
                let slot = items.get_mut(idx).ok_or_else(|| {
                    DiffError::Shape(format!("index {idx} out of bounds in path `{path}`"))
                })?;
                if last {
                    *slot = new;
                    return Ok(());
                }
                cur = slot;
            }
            _ => {
                return Err(DiffError::Shape(format!(
                    "path `{path}` descends into a scalar"
                )))
            }
        }
    }
    Ok(())
}

// ============================================================================
// Three-way merge
// ============================================================================

/// Why a merge conflict occurred on an entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ConflictKind {
    /// Both sides added the same entity id with different content.
    BothAdded {
        /// Our side's value.
        ours: Value,
        /// Their side's value.
        theirs: Value,
    },
    /// One side deleted the entity while the other modified it.
    DeleteModify {
        /// True when *our* side deleted it (theirs modified), false for the
        /// reverse.
        deleted_by_ours: bool,
        /// The surviving (modified) value.
        modified: Value,
    },
    /// Both sides modified the same field to different values.
    Field {
        /// Dotted path of the contested field.
        path: String,
        /// Common-ancestor value.
        base: Value,
        /// Our side's value.
        ours: Value,
        /// Their side's value.
        theirs: Value,
    },
}

/// A genuine merge conflict: both sides changed the same thing differently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    /// Which entity is contested.
    #[serde(flatten)]
    pub key: EntityKey,
    /// The nature of the conflict.
    #[serde(flatten)]
    pub kind: ConflictKind,
}

/// Result of a three-way merge. Fail-closed: conflicts and a merged document
/// are mutually exclusive — a conflicted merge produces no document.
#[derive(Debug)]
pub enum MergeResult {
    /// All changes merged cleanly.
    Merged(Box<Document>),
    /// Genuine conflicts were found; nothing was merged.
    Conflicts(Vec<Conflict>),
}

/// Three-way merge of two documents against a common ancestor.
///
/// Changes that touch different entities — or different fields of the same
/// entity — merge automatically. When both sides edited the same field (or
/// one deleted what the other modified, or both added the same id with
/// different content), every such conflict is reported and **no** merged
/// document is produced; the caller decides how to resolve.
pub fn merge(
    base: &Document,
    ours: &Document,
    theirs: &Document,
) -> Result<MergeResult, DiffError> {
    let db = decompose(base)?;
    let do_ = decompose(ours)?;
    let dt = decompose(theirs)?;

    let keys: BTreeSet<&EntityKey> = db
        .entities
        .keys()
        .chain(do_.entities.keys())
        .chain(dt.entities.keys())
        .collect();

    let mut merged: BTreeMap<EntityKey, Value> = BTreeMap::new();
    let mut conflicts: Vec<Conflict> = Vec::new();

    for key in keys {
        let b = db.entities.get(key);
        let o = do_.entities.get(key);
        let t = dt.entities.get(key);

        let resolved: Option<Value> = if o == t {
            // Same on both sides (both kept, both edited identically, or
            // both deleted).
            o.cloned()
        } else if o == b {
            // Only theirs changed.
            t.cloned()
        } else if t == b {
            // Only ours changed.
            o.cloned()
        } else {
            // Both sides changed it, differently.
            match (b, o, t) {
                (Some(bv), Some(ov), Some(tv)) => {
                    match merge_entity_fields(key, bv, ov, tv, &mut conflicts)? {
                        Some(v) => Some(v),
                        None => continue, // conflicts recorded
                    }
                }
                (None, Some(ov), Some(tv)) => {
                    conflicts.push(Conflict {
                        key: key.clone(),
                        kind: ConflictKind::BothAdded {
                            ours: ov.clone(),
                            theirs: tv.clone(),
                        },
                    });
                    continue;
                }
                (Some(_), one_side, other) => {
                    // Exactly one of o/t is None here (o==t and o==b both
                    // failed), so this is delete-vs-modify.
                    let (deleted_by_ours, modified) = match (one_side, other) {
                        (None, Some(tv)) => (true, tv.clone()),
                        (Some(ov), None) => (false, ov.clone()),
                        _ => unreachable!("delete/modify arm requires exactly one side present"),
                    };
                    conflicts.push(Conflict {
                        key: key.clone(),
                        kind: ConflictKind::DeleteModify {
                            deleted_by_ours,
                            modified,
                        },
                    });
                    continue;
                }
                // Remaining shapes (one side absent with base absent) satisfy
                // `o == b` or `t == b` and were resolved above.
                _ => None,
            }
        };
        if let Some(v) = resolved {
            merged.insert(key.clone(), v);
        }
    }

    if !conflicts.is_empty() {
        return Ok(MergeResult::Conflicts(conflicts));
    }

    // Ordering: ours' order first, then theirs' additions.
    let mut order: BTreeMap<EntityKind, Vec<String>> = do_.order.clone();
    for (kind, ids) in &dt.order {
        let list = order.entry(*kind).or_default();
        for id in ids {
            if !list.contains(id) {
                list.push(id.clone());
            }
        }
    }

    Ok(MergeResult::Merged(Box::new(recompose(&merged, &order)?)))
}

/// Field-level merge of one entity edited on both sides. Returns the merged
/// value, or `None` after recording conflicts for contested fields.
fn merge_entity_fields(
    key: &EntityKey,
    base: &Value,
    ours: &Value,
    theirs: &Value,
    conflicts: &mut Vec<Conflict>,
) -> Result<Option<Value>, DiffError> {
    let mut ours_patch = Vec::new();
    value_diff("", base, ours, &mut ours_patch);
    let mut theirs_patch = Vec::new();
    value_diff("", base, theirs, &mut theirs_patch);

    let mut contested = false;
    for oc in &ours_patch {
        for tc in &theirs_patch {
            if paths_overlap(&oc.path, &tc.path) && oc.new != tc.new {
                contested = true;
                conflicts.push(Conflict {
                    key: key.clone(),
                    kind: ConflictKind::Field {
                        path: oc.path.clone(),
                        base: oc.old.clone(),
                        ours: oc.new.clone(),
                        theirs: tc.new.clone(),
                    },
                });
            }
        }
    }
    if contested {
        return Ok(None);
    }

    let mut merged = base.clone();
    for fc in ours_patch.iter().chain(&theirs_patch) {
        set_path(&mut merged, &fc.path, fc.new.clone())?;
    }
    Ok(Some(merged))
}

/// Two dotted paths overlap when equal or when one is a prefix segment-wise
/// of the other (an edit to `op.points` overlaps an edit to `op.points.0`).
fn paths_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    long.starts_with(short) && long.as_bytes().get(short.len()) == Some(&b'.')
}

// ============================================================================
// Human-readable rendering
// ============================================================================

/// Render a diff as a compact human-readable report.
pub fn render_human(diff: &DocumentDiff) -> String {
    if diff.is_empty() {
        return "documents are semantically identical\n".to_string();
    }
    let mut out = String::new();
    for ec in &diff.changes {
        let label = match &ec.name {
            Some(n) => format!("{} ({n})", ec.key),
            None => ec.key.to_string(),
        };
        match &ec.change {
            ChangeKind::Added { value } => {
                out.push_str(&format!("+ added   {label}{}\n", summarize(value)));
            }
            ChangeKind::Removed { value } => {
                out.push_str(&format!("- removed {label}{}\n", summarize(value)));
            }
            ChangeKind::Modified { fields } => {
                out.push_str(&format!("~ changed {label}\n"));
                for fc in fields {
                    out.push_str(&format!(
                        "    {}: {} \u{2192} {}\n",
                        if fc.path.is_empty() {
                            "(value)"
                        } else {
                            &fc.path
                        },
                        compact(&fc.old),
                        compact(&fc.new)
                    ));
                }
            }
        }
    }
    out
}

/// Render merge conflicts as a human-readable report.
pub fn render_conflicts(conflicts: &[Conflict]) -> String {
    let mut out = format!(
        "merge failed: {} conflict{}\n",
        conflicts.len(),
        if conflicts.len() == 1 { "" } else { "s" }
    );
    for c in conflicts {
        match &c.kind {
            ConflictKind::BothAdded { ours, theirs } => {
                out.push_str(&format!(
                    "! {}: added on both sides with different content\n    ours:   {}\n    theirs: {}\n",
                    c.key,
                    compact(ours),
                    compact(theirs)
                ));
            }
            ConflictKind::DeleteModify {
                deleted_by_ours,
                modified,
            } => {
                let (deleter, modifier) = if *deleted_by_ours {
                    ("ours", "theirs")
                } else {
                    ("theirs", "ours")
                };
                out.push_str(&format!(
                    "! {}: deleted by {deleter}, modified by {modifier}\n    modified: {}\n",
                    c.key,
                    compact(modified)
                ));
            }
            ConflictKind::Field {
                path,
                base,
                ours,
                theirs,
            } => {
                out.push_str(&format!(
                    "! {} field `{}`: base {} \u{2192} ours {} vs theirs {}\n",
                    c.key,
                    if path.is_empty() { "(value)" } else { path },
                    compact(base),
                    compact(ours),
                    compact(theirs)
                ));
            }
        }
    }
    out
}

/// Short type/name summary for added/removed entities.
fn summarize(v: &Value) -> String {
    match v
        .get("op")
        .and_then(|op| op.get("type"))
        .and_then(Value::as_str)
    {
        Some(t) => format!(" [{t}]"),
        None => String::new(),
    }
}

/// Compact single-line JSON, truncated for readability.
fn compact(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "<unprintable>".into());
    const MAX: usize = 120;
    if s.len() > MAX {
        let cut = s
            .char_indices()
            .take_while(|(i, _)| *i < MAX)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}\u{2026}", &s[..cut])
    } else {
        s
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{CsgOp, Node, SceneEntry, Vec3};

    fn cube_doc(size: f64) -> Document {
        let mut doc = Document::default();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("base".into()),
                op: CsgOp::Cube {
                    size: Vec3::new(size, size, size),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "aluminum".into(),
            visible: None,
        });
        doc
    }

    fn add_cylinder(doc: &mut Document, id: u64, radius: f64) {
        doc.nodes.insert(
            id,
            Node {
                id,
                name: Some(format!("cyl-{id}")),
                op: CsgOp::Cylinder {
                    radius,
                    height: 30.0,
                    segments: 0,
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: id,
            material: "steel".into(),
            visible: None,
        });
    }

    #[test]
    fn identical_docs_empty_diff() {
        let a = cube_doc(10.0);
        let d = diff(&a, &a.clone()).unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn parameter_change_reports_old_new() {
        let a = cube_doc(10.0);
        let b = cube_doc(25.0);
        let d = diff(&a, &b).unwrap();
        assert_eq!(d.changes.len(), 1);
        let ec = &d.changes[0];
        assert_eq!(ec.key, EntityKey::new(EntityKind::Node, "1"));
        let ChangeKind::Modified { fields } = &ec.change else {
            panic!("expected Modified, got {:?}", ec.change);
        };
        // size.x/y/z each changed 10 → 25
        assert_eq!(fields.len(), 3);
        assert!(fields.iter().all(|f| f.path.starts_with("op.size.")));
        assert_eq!(fields[0].old, serde_json::json!(10.0));
        assert_eq!(fields[0].new, serde_json::json!(25.0));
    }

    #[test]
    fn added_and_removed_nodes() {
        let a = cube_doc(10.0);
        let mut b = cube_doc(10.0);
        add_cylinder(&mut b, 2, 5.0);
        let d = diff(&a, &b).unwrap();
        assert_eq!(d.changes.len(), 2); // node 2 + root 2
        assert!(d
            .changes
            .iter()
            .all(|c| matches!(c.change, ChangeKind::Added { .. })));

        let d_rev = diff(&b, &a).unwrap();
        assert!(d_rev
            .changes
            .iter()
            .all(|c| matches!(c.change, ChangeKind::Removed { .. })));
    }

    #[test]
    fn diff_then_apply_round_trips() {
        let a = cube_doc(10.0);
        let mut b = cube_doc(40.0);
        add_cylinder(&mut b, 2, 5.0);
        b.part_materials.insert("part-1".into(), "brass".into());
        b.parameters
            .insert("wall".into(), vcad_ir::Parameter::literal(3.0));

        let d = diff(&a, &b).unwrap();
        let patched = apply(&a, &d).unwrap();
        // Semantic equality: diff(patched, b) must be empty.
        assert!(diff(&patched, &b).unwrap().is_empty());
    }

    #[test]
    fn merge_disjoint_edits() {
        let base = cube_doc(10.0);

        // ours: resize the cube
        let ours = cube_doc(20.0);

        // theirs: add a cylinder part + a material assignment
        let mut theirs = cube_doc(10.0);
        add_cylinder(&mut theirs, 2, 5.0);
        theirs
            .part_materials
            .insert("part-2".into(), "steel".into());

        let result = merge(&base, &ours, &theirs).unwrap();
        let MergeResult::Merged(merged) = result else {
            panic!("expected clean merge");
        };
        // Cube got ours' size, cylinder from theirs is present.
        let cube = &merged.nodes[&1];
        assert!(
            matches!(&cube.op, CsgOp::Cube { size, .. } if size.x == 20.0),
            "cube should carry ours' resize"
        );
        assert!(merged.nodes.contains_key(&2));
        assert_eq!(merged.roots.len(), 2);
        assert_eq!(merged.part_materials.get("part-2").unwrap(), "steel");
    }

    #[test]
    fn merge_same_entity_different_fields() {
        let base = cube_doc(10.0);
        // ours: change size.x via a fresh doc where only x differs
        let mut ours = cube_doc(10.0);
        if let CsgOp::Cube { size, .. } = &mut ours.nodes.get_mut(&1).unwrap().op {
            size.x = 99.0;
        }
        // theirs: rename the node
        let mut theirs = cube_doc(10.0);
        theirs.nodes.get_mut(&1).unwrap().name = Some("renamed".into());

        let MergeResult::Merged(merged) = merge(&base, &ours, &theirs).unwrap() else {
            panic!("different fields of one node must merge cleanly");
        };
        let node = &merged.nodes[&1];
        assert_eq!(node.name.as_deref(), Some("renamed"));
        assert!(matches!(&node.op, CsgOp::Cube { size, .. } if size.x == 99.0));
    }

    #[test]
    fn merge_conflict_same_parameter() {
        let base = cube_doc(10.0);
        let ours = cube_doc(20.0);
        let theirs = cube_doc(30.0);

        let MergeResult::Conflicts(conflicts) = merge(&base, &ours, &theirs).unwrap() else {
            panic!("same-field edits must conflict, never silently pick a side");
        };
        assert!(!conflicts.is_empty());
        let c = &conflicts[0];
        assert_eq!(c.key, EntityKey::new(EntityKind::Node, "1"));
        assert!(matches!(
            &c.kind,
            ConflictKind::Field { path, .. } if path.starts_with("op.size.")
        ));
    }

    #[test]
    fn merge_conflict_delete_vs_modify() {
        let mut base = cube_doc(10.0);
        add_cylinder(&mut base, 2, 5.0);

        // ours: delete the cylinder
        let ours = cube_doc(10.0);

        // theirs: fatten the cylinder
        let mut theirs = base.clone();
        if let CsgOp::Cylinder { radius, .. } = &mut theirs.nodes.get_mut(&2).unwrap().op {
            *radius = 9.0;
        }

        let MergeResult::Conflicts(conflicts) = merge(&base, &ours, &theirs).unwrap() else {
            panic!("delete vs modify must conflict");
        };
        assert!(conflicts.iter().any(|c| matches!(
            &c.kind,
            ConflictKind::DeleteModify {
                deleted_by_ours: true,
                ..
            }
        )));
    }

    #[test]
    fn merge_identical_edits_no_conflict() {
        let base = cube_doc(10.0);
        let ours = cube_doc(20.0);
        let theirs = cube_doc(20.0);
        let MergeResult::Merged(merged) = merge(&base, &ours, &theirs).unwrap() else {
            panic!("identical edits merge cleanly");
        };
        assert!(matches!(&merged.nodes[&1].op, CsgOp::Cube { size, .. } if size.x == 20.0));
    }

    #[test]
    fn merge_both_added_same_id_conflicts() {
        let base = cube_doc(10.0);
        let mut ours = cube_doc(10.0);
        add_cylinder(&mut ours, 2, 5.0);
        let mut theirs = cube_doc(10.0);
        add_cylinder(&mut theirs, 2, 7.0);

        let MergeResult::Conflicts(conflicts) = merge(&base, &ours, &theirs).unwrap() else {
            panic!("both-added with different content must conflict");
        };
        assert!(conflicts
            .iter()
            .any(|c| matches!(c.kind, ConflictKind::BothAdded { .. })));
    }

    #[test]
    fn merge_both_added_identical_ok() {
        let base = cube_doc(10.0);
        let mut ours = cube_doc(10.0);
        add_cylinder(&mut ours, 2, 5.0);
        let theirs = ours.clone();
        let MergeResult::Merged(merged) = merge(&base, &ours, &theirs).unwrap() else {
            panic!("identical additions merge cleanly");
        };
        assert!(merged.nodes.contains_key(&2));
        assert_eq!(merged.roots.len(), 2);
    }

    #[test]
    fn doc_json_round_trip_through_decompose() {
        let mut doc = cube_doc(10.0);
        add_cylinder(&mut doc, 2, 5.0);
        doc.part_materials.insert("part-1".into(), "brass".into());
        let d = decompose(&doc).unwrap();
        let back = recompose(&d.entities, &d.order).unwrap();
        assert!(diff(&doc, &back).unwrap().is_empty());
    }

    #[test]
    fn human_render_mentions_change() {
        let a = cube_doc(10.0);
        let b = cube_doc(25.0);
        let d = diff(&a, &b).unwrap();
        let text = render_human(&d);
        assert!(text.contains("node 1"));
        assert!(text.contains("op.size"));
        assert!(text.contains("\u{2192}"));
    }
}
