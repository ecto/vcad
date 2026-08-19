//! Content-addressed cache of evaluated root geometry.
//!
//! A scene root's tessellated mesh is a pure function of three things: the
//! IR subgraph under the root, the tessellation settings, and the kernel
//! build that evaluated it. [`root_fingerprint`] hashes exactly those, and a
//! [`RootMeshCache`] maps the hash to the mesh. `evaluate_document` consults
//! the cache before walking a root and stores the result after — so a
//! 108-root document where one root changed re-evaluates one root.
//!
//! What the key covers, and why:
//!
//! * **The subgraph, structurally.** Nodes are renumbered in a canonical
//!   post-order walk before hashing, so node ids (which shift whenever an
//!   earlier `let` is added) and node names (which never reach the kernel)
//!   do not perturb the key. Operands are found via
//!   [`CsgOp::child_ids`], whose match is exhaustive — a new variant with an
//!   operand is a compile error, not a silently stale cache.
//! * **The kernel identity** — [`KERNEL_ID`]: the evaluator's version, the
//!   target architecture (the kernel's torture baseline differs x86_64 vs
//!   aarch64), and a content hash of every kernel, IR, evaluator and `tang`
//!   source file, computed by `build.rs`. Editing a boolean in the kernel
//!   changes the id; two checkouts of the same source share it.
//! * **Tessellation settings** and the sheet-metal fold flag.
//! * **Kernel behaviour knobs** — any `VCAD_NO_*` environment variable (the
//!   kernel's escape hatches for its own algorithms) is folded into the key.
//!
//! What is never cached: a root whose subgraph reads something outside the
//! IR ([`CsgOp::reads_external_data`] — STEP/mesh imports, part instances),
//! a root that failed or panicked, and a root that produced no geometry.
//!
//! Only the *mesh* is cached, not the BRep: `vcad_kernel::Solid` has no
//! serialized form, so a cache hit yields an `EvaluatedPart` with
//! `solid: None`. Consumers that need the BRep (STEP export, ray tracing,
//! clash detection) must evaluate without a cache.

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use vcad_ir::{Node, NodeId};

use crate::EvaluatedMesh;

/// Identity of the evaluating kernel build, for cache keys. See the
/// module docs for what it covers. `build.rs` supplies the source hash; a
/// build that could not find the source trees reports `unhashed`, and
/// [`root_fingerprint`] then refuses to produce keys at all.
pub const KERNEL_ID: &str = concat!(
    "vcad-eval/",
    env!("CARGO_PKG_VERSION"),
    "/",
    env!("VCAD_EVAL_TARGET_ARCH"),
    "/",
    env!("VCAD_EVAL_SOURCE_HASH"),
);

/// Is the kernel identity trustworthy enough to key a cache on?
pub fn kernel_id_is_hashed() -> bool {
    !KERNEL_ID.ends_with("/unhashed")
}

/// A cache key: the SHA-256 of a root's fingerprint, lower-case hex.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RootKey(pub String);

impl std::fmt::Display for RootKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Settings that, together with the subgraph, determine a root's mesh.
#[derive(Debug, Clone)]
pub struct FingerprintSettings {
    /// Segment count passed to `Solid::to_mesh`.
    pub segments: u32,
    /// Whether sheet-metal ops fold to real solids (see
    /// `evaluate_document_with_sheet_metal`).
    pub fold_sheet_metal: bool,
}

/// Storage for evaluated root meshes, keyed by [`RootKey`].
///
/// Implementations are expected to be cheap to miss and safe to share: a
/// `get` that returns `None` for any reason (missing, corrupt, I/O error) is
/// correct, and `put` failures must be swallowed — the cache is an
/// accelerator, never a source of truth.
pub trait RootMeshCache {
    /// Look up a previously stored mesh.
    fn get(&self, key: &RootKey) -> Option<EvaluatedMesh>;
    /// Store a mesh. Errors are the implementation's to absorb.
    fn put(&self, key: &RootKey, mesh: &EvaluatedMesh);
}

/// Compute the cache key for the subgraph rooted at `root`, or `None` when
/// the root is not cacheable (an operand reads external data, a node is
/// missing, a value fails to serialize, the kernel id is unhashed).
pub fn root_fingerprint(
    root: NodeId,
    nodes: &HashMap<NodeId, Node>,
    settings: &FingerprintSettings,
) -> Option<RootKey> {
    if !kernel_id_is_hashed() {
        return None;
    }
    // Canonical post-order: children before parents, each node numbered by
    // its position in the walk. Shared subgraphs (a `let` used twice) are
    // visited once and referenced by the same canonical index both times,
    // which is exactly the DAG the kernel evaluates.
    let mut order: Vec<NodeId> = Vec::new();
    let mut index: HashMap<NodeId, u64> = HashMap::new();
    let mut stack: Vec<(NodeId, bool)> = vec![(root, false)];
    while let Some((id, expanded)) = stack.pop() {
        if index.contains_key(&id) {
            continue;
        }
        let node = nodes.get(&id)?;
        if expanded {
            index.insert(id, order.len() as u64);
            order.push(id);
            continue;
        }
        if node.op.reads_external_data() {
            return None;
        }
        stack.push((id, true));
        // Push in reverse so children are expanded in `child_ids` order.
        for child in node.op.child_ids().into_iter().rev() {
            if !index.contains_key(&child) {
                stack.push((child, false));
            }
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(b"vcad-root-mesh/1\0");
    hasher.update(KERNEL_ID.as_bytes());
    hasher.update(b"\0");
    hasher.update(settings.segments.to_le_bytes());
    hasher.update([settings.fold_sheet_metal as u8]);
    hasher.update(b"\0");
    for (name, value) in kernel_knobs() {
        hasher.update(name.as_bytes());
        hasher.update(b"=");
        hasher.update(value.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"\0");
    for id in &order {
        let mut op = nodes.get(id)?.op.clone();
        for slot in op.child_ids_mut() {
            *slot = *index.get(slot)?;
        }
        // serde_json writes struct fields in declaration order and floats
        // in their shortest round-trip form, so equal ops hash equal. A
        // non-finite float fails to serialize, which makes the root
        // uncacheable rather than mis-keyed.
        let bytes = serde_json::to_vec(&op).ok()?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    Some(RootKey(hex))
}

/// `VCAD_NO_*` environment variables, sorted — the kernel's own algorithm
/// escape hatches, which change geometry and so must change the key.
fn kernel_knobs() -> Vec<(String, String)> {
    let mut knobs: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.starts_with("VCAD_NO_"))
        .collect();
    knobs.sort();
    knobs
}

// ── binary mesh encoding ──────────────────────────────────────────────────
//
// A small self-describing little-endian record, so a cache file is readable
// without serde and a truncated or foreign file decodes to `None`.

const MESH_MAGIC: &[u8; 4] = b"VCRM";
const MESH_FORMAT: u32 = 1;

/// Serialize a mesh to the cache's on-disk record format.
pub fn encode_mesh(mesh: &EvaluatedMesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        16 + mesh.positions.len() * 4
            + mesh.indices.len() * 4
            + mesh.normals.as_ref().map_or(0, |n| n.len() * 4)
            + mesh.face_kinds.as_ref().map_or(0, |k| k.len()),
    );
    out.extend_from_slice(MESH_MAGIC);
    out.extend_from_slice(&MESH_FORMAT.to_le_bytes());
    put_f32s(&mut out, &mesh.positions);
    put_u32s(&mut out, &mesh.indices);
    match &mesh.normals {
        Some(n) => {
            out.push(1);
            put_f32s(&mut out, n);
        }
        None => out.push(0),
    }
    match &mesh.face_kinds {
        Some(k) => {
            out.push(1);
            out.extend_from_slice(&(k.len() as u64).to_le_bytes());
            out.extend_from_slice(k);
        }
        None => out.push(0),
    }
    out
}

/// Parse a record written by [`encode_mesh`]. `None` for anything that is
/// not a complete, well-formed record of the current format.
pub fn decode_mesh(bytes: &[u8]) -> Option<EvaluatedMesh> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != MESH_MAGIC {
        return None;
    }
    if r.u32()? != MESH_FORMAT {
        return None;
    }
    let positions = r.f32s()?;
    let indices = r.f32s_as_u32s()?;
    let normals = match r.u8()? {
        0 => None,
        1 => Some(r.f32s()?),
        _ => return None,
    };
    let face_kinds = match r.u8()? {
        0 => None,
        1 => {
            let n = r.len()?;
            Some(r.take(n)?.to_vec())
        }
        _ => return None,
    };
    if r.at != bytes.len() {
        return None;
    }
    // Structural sanity: every index must address a vertex.
    let nverts = positions.len() / 3;
    if indices.iter().any(|&i| i as usize >= nverts) {
        return None;
    }
    Some(EvaluatedMesh {
        positions,
        indices,
        normals,
        face_kinds,
    })
}

fn put_f32s(out: &mut Vec<u8>, v: &[f32]) {
    out.extend_from_slice(&(v.len() as u64).to_le_bytes());
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

fn put_u32s(out: &mut Vec<u8>, v: &[u32]) {
    out.extend_from_slice(&(v.len() as u64).to_le_bytes());
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn len(&mut self) -> Option<usize> {
        let n = u64::from_le_bytes(self.take(8)?.try_into().ok()?);
        usize::try_from(n).ok()
    }
    fn f32s(&mut self) -> Option<Vec<f32>> {
        let n = self.len()?;
        let raw = self.take(n.checked_mul(4)?)?;
        Some(
            raw.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }
    fn f32s_as_u32s(&mut self) -> Option<Vec<u32>> {
        let n = self.len()?;
        let raw = self.take(n.checked_mul(4)?)?;
        Some(
            raw.chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }
}

// ── on-disk cache ─────────────────────────────────────────────────────────

/// A [`RootMeshCache`] on the local filesystem.
///
/// Layout: `<dir>/geom/<kernel id>/<key>.mesh`, one file per root, written
/// atomically (temp file + rename) so a crash mid-write never leaves a
/// half-record that a later reader could mistake for geometry. Putting the
/// kernel id in the path means a kernel upgrade simply starts a new
/// directory; old ones can be deleted wholesale.
///
/// Location, first match wins: `$VCAD_CACHE_DIR`, `$XDG_CACHE_HOME/vcad`,
/// `$HOME/.cache/vcad`. `VCAD_CACHE=0` disables the cache entirely
/// ([`DiskMeshCache::from_env`] returns `None`).
#[cfg(not(target_arch = "wasm32"))]
pub struct DiskMeshCache {
    dir: std::path::PathBuf,
    stats: std::cell::RefCell<CacheStats>,
}

/// Hit/miss counters for one process's use of a cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Roots served from the cache.
    pub hits: u64,
    /// Roots looked up and not found.
    pub misses: u64,
    /// Roots written after evaluation.
    pub stored: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl DiskMeshCache {
    /// The cache the environment asks for, or `None` when it is disabled
    /// (`VCAD_CACHE=0`), no cache directory can be determined, or the
    /// kernel identity is unhashed (see [`kernel_id_is_hashed`]).
    pub fn from_env() -> Option<Self> {
        if matches!(
            std::env::var("VCAD_CACHE").as_deref(),
            Ok("0") | Ok("off") | Ok("false") | Ok("no")
        ) {
            return None;
        }
        if !kernel_id_is_hashed() {
            return None;
        }
        Self::at(cache_root()?)
    }

    /// A cache rooted at `dir` (the `geom/<kernel id>` layer is added
    /// underneath). Creates the directory; `None` if that fails.
    pub fn at(dir: impl Into<std::path::PathBuf>) -> Option<Self> {
        let dir = dir.into().join("geom").join(kernel_dir_name());
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self {
            dir,
            stats: Default::default(),
        })
    }

    /// Where this cache's records live.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Counters so far.
    pub fn stats(&self) -> CacheStats {
        *self.stats.borrow()
    }

    fn path_for(&self, key: &RootKey) -> std::path::PathBuf {
        self.dir.join(format!("{key}.mesh"))
    }
}

/// The cache's directory component for this kernel: the id with path
/// separators replaced, so `vcad-eval/0.9.4/aarch64/abc…` becomes
/// `vcad-eval_0.9.4_aarch64_abc…`.
#[cfg(not(target_arch = "wasm32"))]
fn kernel_dir_name() -> String {
    KERNEL_ID.replace(['/', '\\'], "_")
}

/// `$VCAD_CACHE_DIR`, else `$XDG_CACHE_HOME/vcad`, else `$HOME/.cache/vcad`.
#[cfg(not(target_arch = "wasm32"))]
pub fn cache_root() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("VCAD_CACHE_DIR").filter(|d| !d.is_empty()) {
        return Some(d.into());
    }
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME").filter(|d| !d.is_empty()) {
        return Some(std::path::PathBuf::from(d).join("vcad"));
    }
    let home = std::env::var_os("HOME").filter(|d| !d.is_empty())?;
    Some(std::path::PathBuf::from(home).join(".cache").join("vcad"))
}

#[cfg(not(target_arch = "wasm32"))]
impl RootMeshCache for DiskMeshCache {
    fn get(&self, key: &RootKey) -> Option<EvaluatedMesh> {
        let path = self.path_for(key);
        let bytes = std::fs::read(&path).ok();
        let mesh = bytes.as_deref().and_then(decode_mesh);
        if mesh.is_none() && bytes.is_some() {
            // A file we can't decode is a corrupt record; clear it so the
            // next run re-evaluates and rewrites rather than retrying.
            let _ = std::fs::remove_file(&path);
        }
        let mut s = self.stats.borrow_mut();
        if mesh.is_some() {
            s.hits += 1;
        } else {
            s.misses += 1;
        }
        mesh
    }

    fn put(&self, key: &RootKey, mesh: &EvaluatedMesh) {
        let path = self.path_for(key);
        let tmp = self
            .dir
            .join(format!(".{}.{}.tmp", key, std::process::id()));
        let bytes = encode_mesh(mesh);
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.stats.borrow_mut().stored += 1;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// An in-memory [`RootMeshCache`] — for tests and for callers that want
/// within-process reuse without touching disk.
#[derive(Default)]
pub struct MemoryMeshCache {
    inner: std::cell::RefCell<HashMap<RootKey, EvaluatedMesh>>,
    stats: std::cell::RefCell<CacheStats>,
}

impl MemoryMeshCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }
    /// Counters so far.
    pub fn stats(&self) -> CacheStats {
        *self.stats.borrow()
    }
}

impl RootMeshCache for MemoryMeshCache {
    fn get(&self, key: &RootKey) -> Option<EvaluatedMesh> {
        let hit = self.inner.borrow().get(key).cloned();
        let mut s = self.stats.borrow_mut();
        if hit.is_some() {
            s.hits += 1;
        } else {
            s.misses += 1;
        }
        hit
    }
    fn put(&self, key: &RootKey, mesh: &EvaluatedMesh) {
        self.inner.borrow_mut().insert(key.clone(), mesh.clone());
        self.stats.borrow_mut().stored += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{CsgOp, Document, Vec3};

    fn settings() -> FingerprintSettings {
        FingerprintSettings {
            segments: 32,
            fold_sheet_metal: false,
        }
    }

    fn cube(id: NodeId, name: &str, sx: f64) -> Node {
        Node {
            id,
            name: Some(name.to_string()),
            op: CsgOp::Cube {
                size: Vec3::new(sx, 2.0, 3.0),
            },
        }
    }

    fn translate(id: NodeId, child: NodeId, x: f64) -> Node {
        Node {
            id,
            name: None,
            op: CsgOp::Translate {
                child,
                offset: Vec3::new(x, 0.0, 0.0),
            },
        }
    }

    fn doc_nodes(nodes: Vec<Node>) -> HashMap<NodeId, Node> {
        nodes.into_iter().map(|n| (n.id, n)).collect()
    }

    #[test]
    fn fingerprint_ignores_ids_and_names() {
        let a = doc_nodes(vec![cube(1, "a", 1.0), translate(2, 1, 5.0)]);
        let b = doc_nodes(vec![cube(40, "renamed", 1.0), translate(41, 40, 5.0)]);
        assert_eq!(
            root_fingerprint(2, &a, &settings()),
            root_fingerprint(41, &b, &settings())
        );
    }

    #[test]
    fn fingerprint_sees_every_operand() {
        let a = doc_nodes(vec![cube(1, "a", 1.0), translate(2, 1, 5.0)]);
        let b = doc_nodes(vec![cube(1, "a", 1.5), translate(2, 1, 5.0)]);
        let c = doc_nodes(vec![cube(1, "a", 1.0), translate(2, 1, 6.0)]);
        let ka = root_fingerprint(2, &a, &settings());
        assert_ne!(ka, root_fingerprint(2, &b, &settings()));
        assert_ne!(ka, root_fingerprint(2, &c, &settings()));
    }

    #[test]
    fn fingerprint_distinguishes_operand_order() {
        let nodes = doc_nodes(vec![
            cube(1, "a", 1.0),
            cube(2, "b", 2.0),
            Node {
                id: 3,
                name: None,
                op: CsgOp::Difference { left: 1, right: 2 },
            },
            Node {
                id: 4,
                name: None,
                op: CsgOp::Difference { left: 2, right: 1 },
            },
        ]);
        assert_ne!(
            root_fingerprint(3, &nodes, &settings()),
            root_fingerprint(4, &nodes, &settings())
        );
    }

    #[test]
    fn fingerprint_tracks_settings() {
        let nodes = doc_nodes(vec![cube(1, "a", 1.0)]);
        let k32 = root_fingerprint(1, &nodes, &settings());
        let k16 = root_fingerprint(
            1,
            &nodes,
            &FingerprintSettings {
                segments: 16,
                fold_sheet_metal: false,
            },
        );
        let kfold = root_fingerprint(
            1,
            &nodes,
            &FingerprintSettings {
                segments: 32,
                fold_sheet_metal: true,
            },
        );
        assert_ne!(k32, k16);
        assert_ne!(k32, kfold);
    }

    #[test]
    fn fingerprint_refuses_external_data() {
        let nodes = doc_nodes(vec![Node {
            id: 1,
            name: None,
            op: CsgOp::MeshImport {
                path: "part.stl".into(),
                scale: None,
            },
        }]);
        assert_eq!(root_fingerprint(1, &nodes, &settings()), None);
        let nodes = doc_nodes(vec![
            Node {
                id: 1,
                name: None,
                op: CsgOp::StepImport {
                    path: "part.step".into(),
                    solid_index: None,
                },
            },
            translate(2, 1, 1.0),
        ]);
        assert_eq!(root_fingerprint(2, &nodes, &settings()), None);
    }

    #[test]
    fn fingerprint_refuses_missing_node() {
        let nodes = doc_nodes(vec![translate(2, 1, 1.0)]);
        assert_eq!(root_fingerprint(2, &nodes, &settings()), None);
    }

    #[test]
    fn kernel_id_is_hashed_in_this_build() {
        assert!(kernel_id_is_hashed(), "KERNEL_ID = {KERNEL_ID}");
        assert!(KERNEL_ID.starts_with("vcad-eval/"));
    }

    #[test]
    fn mesh_round_trips() {
        let mesh = EvaluatedMesh {
            positions: vec![0.0, 1.0, 2.0, 3.5, -4.0, 5.25],
            indices: vec![0, 1, 0],
            normals: Some(vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
            face_kinds: Some(vec![1]),
        };
        let bytes = encode_mesh(&mesh);
        let back = decode_mesh(&bytes).unwrap();
        assert_eq!(back.positions, mesh.positions);
        assert_eq!(back.indices, mesh.indices);
        assert_eq!(back.normals, mesh.normals);
        assert_eq!(back.face_kinds, mesh.face_kinds);

        let bare = EvaluatedMesh {
            positions: vec![0.0; 9],
            indices: vec![0, 1, 2],
            normals: None,
            face_kinds: None,
        };
        let back = decode_mesh(&encode_mesh(&bare)).unwrap();
        assert_eq!(back.normals, None);
        assert_eq!(back.face_kinds, None);
    }

    #[test]
    fn decode_rejects_garbage_and_truncation() {
        assert!(decode_mesh(b"").is_none());
        assert!(decode_mesh(b"VCRM").is_none());
        assert!(decode_mesh(b"nope nope nope nope").is_none());
        let mesh = EvaluatedMesh {
            positions: vec![0.0; 9],
            indices: vec![0, 1, 2],
            normals: None,
            face_kinds: None,
        };
        let bytes = encode_mesh(&mesh);
        assert!(decode_mesh(&bytes[..bytes.len() - 1]).is_none());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode_mesh(&trailing).is_none());
        // Out-of-range index.
        let bad = EvaluatedMesh {
            positions: vec![0.0; 9],
            indices: vec![0, 1, 7],
            normals: None,
            face_kinds: None,
        };
        assert!(decode_mesh(&encode_mesh(&bad)).is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn disk_cache_round_trip_and_corruption() {
        let dir = std::env::temp_dir().join(format!("vcad-eval-cache-{}", std::process::id()));
        let cache = DiskMeshCache::at(&dir).unwrap();
        let key = RootKey("ab".repeat(32));
        assert!(cache.get(&key).is_none());
        let mesh = EvaluatedMesh {
            positions: vec![0.0; 9],
            indices: vec![0, 1, 2],
            normals: None,
            face_kinds: None,
        };
        cache.put(&key, &mesh);
        assert_eq!(cache.get(&key).unwrap().indices, vec![0, 1, 2]);
        // Corrupt the record: the next get is a miss and the file is gone.
        std::fs::write(cache.path_for(&key), b"garbage").unwrap();
        assert!(cache.get(&key).is_none());
        assert!(!cache.path_for(&key).exists());
        assert_eq!(
            cache.stats(),
            CacheStats {
                hits: 1,
                misses: 2,
                stored: 1
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn evaluate_document_uses_cache() {
        // Two evaluations of the same document: the second is served
        // entirely from the cache, and produces the same meshes.
        let mut doc = Document::new();
        for (id, n) in [(1, "a"), (2, "b")] {
            doc.nodes.insert(id, cube(id, n, id as f64));
            doc.roots.push(vcad_ir::SceneEntry {
                root: id,
                material: "default".into(),
                visible: None,
            });
        }
        let cache = std::rc::Rc::new(MemoryMeshCache::new());
        let opts = |c: std::rc::Rc<MemoryMeshCache>| crate::EvalOptions {
            skip_clash_detection: true,
            root_cache: Some(c),
            ..Default::default()
        };
        let first = crate::evaluate_document(&doc, &opts(cache.clone())).unwrap();
        assert_eq!(
            cache.stats(),
            CacheStats {
                hits: 0,
                misses: 2,
                stored: 2
            }
        );
        assert!(first.parts.iter().all(|p| p.solid.is_some()));
        let second = crate::evaluate_document(&doc, &opts(cache.clone())).unwrap();
        assert_eq!(
            cache.stats(),
            CacheStats {
                hits: 2,
                misses: 2,
                stored: 2
            }
        );
        assert!(second.parts.iter().all(|p| p.solid.is_none()));
        for (a, b) in first.parts.iter().zip(&second.parts) {
            assert_eq!(a.mesh.positions, b.mesh.positions);
            assert_eq!(a.mesh.indices, b.mesh.indices);
            assert_eq!(a.material, b.material);
        }
        // Touch one root: exactly one miss, one store.
        doc.nodes.get_mut(&2).unwrap().op = CsgOp::Cube {
            size: Vec3::new(9.0, 9.0, 9.0),
        };
        let third = crate::evaluate_document(&doc, &opts(cache.clone())).unwrap();
        assert_eq!(
            cache.stats(),
            CacheStats {
                hits: 3,
                misses: 3,
                stored: 3
            }
        );
        assert!(third.parts[0].solid.is_none());
        assert!(third.parts[1].solid.is_some());
    }
}
