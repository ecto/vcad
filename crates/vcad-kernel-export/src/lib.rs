//! Mesh-level GLB (binary glTF 2.0) and binary STL writers.
//!
//! Single source of truth for vcad's GLB/STL byte serialization. The legacy
//! `vcad` crate's exporters and the TypeScript packages (`@vcad/mcp`,
//! `@vcad/core`, via `vcad-kernel-wasm`) all delegate here, so chunk layout,
//! alignment, material dedup, and animation encoding live in exactly one
//! place.
//!
//! Inputs reference geometry by `[offset, len]` spans into two shared flat
//! buffers (one `f32`, one `u32`) so the WASM boundary is two typed arrays
//! plus a small JSON metadata string, regardless of mesh count.

#![warn(missing_docs)]

use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// A `[offset, len]` span into one of the shared flat data buffers.
pub type Span = [usize; 2];

/// Errors from the export writers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    /// A span points outside its backing buffer.
    SpanOutOfBounds(String),
    /// Metadata was structurally invalid (bad path, bad span shape, …).
    InvalidSpec(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::SpanOutOfBounds(m) => write!(f, "span out of bounds: {m}"),
            ExportError::InvalidSpec(m) => write!(f, "invalid export spec: {m}"),
        }
    }
}

impl std::error::Error for ExportError {}

/// Node TRS applied to part-local geometry (assembly instances). Identity
/// components are omitted from the emitted glTF node, so an identity
/// transform produces byte-identical output to no transform.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformSpec {
    /// Translation in mm.
    pub translation: [f64; 3],
    /// glTF quaternion `[x, y, z, w]` (see [`euler_xyz_deg_to_quat`]).
    pub rotation_quat: [f64; 4],
    /// Per-axis scale.
    pub scale: [f64; 3],
}

/// One renderable mesh: geometry spans plus an explicit PBR material.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlbMeshSpec {
    /// glTF node name — `"<part_id>:<name>"` for click-to-select.
    pub name: String,
    /// Positions span into the f32 buffer (3 floats per vertex).
    pub positions: Span,
    /// Indices span into the u32 buffer.
    pub indices: Span,
    /// Optional normals span into the f32 buffer; when absent the GLB
    /// carries no NORMAL attribute and viewers flat-shade.
    #[serde(default)]
    pub normals: Option<Span>,
    /// Base color RGB 0..1 (linear).
    pub color: [f64; 3],
    /// PBR metallic factor.
    pub metallic: f64,
    /// PBR roughness factor.
    pub roughness: f64,
    /// Emissive RGB 0..1 (linear); omitted/`[0,0,0]` = not emissive.
    #[serde(default)]
    pub emissive: Option<[f64; 3]>,
    /// KHR_materials_emissive_strength multiplier (>1 = glows past white).
    #[serde(default)]
    pub emissive_strength: Option<f64>,
    /// KHR_materials_clearcoat factor 0..1 (glossy soldermask wet-look).
    #[serde(default)]
    pub clearcoat: Option<f64>,
    /// Clearcoat roughness 0..1.
    #[serde(default)]
    pub clearcoat_roughness: Option<f64>,
    /// Base-color alpha 0..1; below 1 the material is alpha-BLENDed.
    #[serde(default)]
    pub alpha: Option<f64>,
    /// Node TRS applied to part-local geometry.
    #[serde(default)]
    pub transform: Option<TransformSpec>,
    /// Geometry-dedup key: inputs sharing a `meshKey` emit ONE glTF mesh
    /// referenced by multiple nodes. The first input carrying a key supplies
    /// the geometry and material for all of them.
    #[serde(default)]
    pub mesh_key: Option<String>,
}

/// One animation channel: keyframed TRS on a named node.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationChannelSpec {
    /// Target node name — must match a mesh node name (or the animation's
    /// `root_node_name`) exactly. Unknown names are skipped.
    pub node_name: String,
    /// `"translation"`, `"rotation"`, or `"scale"`.
    pub path: String,
    /// Keyframe times span (seconds, ascending) into the f32 buffer.
    pub times: Span,
    /// Flat keyframe values span into the f32 buffer: VEC3 per key for
    /// translation/scale, VEC4 quaternion per key for rotation.
    pub values: Span,
    /// Sampler interpolation, `"LINEAR"` (default) or `"STEP"`.
    #[serde(default)]
    pub interpolation: Option<String>,
}

/// Animation options for [`build_glb`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationSpec {
    /// Animation name; defaults to `"timeline"`.
    #[serde(default)]
    pub name: Option<String>,
    /// Keyframed channels.
    pub channels: Vec<AnimationChannelSpec>,
    /// If set, a new parent node with this name wraps ALL scene nodes and
    /// becomes the sole scene root; channels may target it (turntable).
    #[serde(default)]
    pub root_node_name: Option<String>,
    /// Extra empty (meshless) nodes added as scene roots; channels may
    /// target them (e.g. a `__camera` orbit carrier).
    #[serde(default)]
    pub extra_nodes: Option<Vec<String>>,
}

/// Complete GLB build request: scene name + meshes + optional animation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlbSpec {
    /// glTF scene name.
    pub name: String,
    /// Meshes, in node order.
    pub meshes: Vec<GlbMeshSpec>,
    /// Optional keyframe animation.
    #[serde(default)]
    pub animation: Option<AnimationSpec>,
    /// Optional free-form JSON stamped on the glTF scene object as
    /// `scenes[0].extras`. `None` emits no `extras` key at all, so existing
    /// callers produce byte-identical output.
    #[serde(default)]
    pub scene_extras: Option<Value>,
}

/// STL build request: header name + per-mesh geometry spans.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StlSpec {
    /// Text placed in the 80-byte binary STL header.
    pub name: String,
    /// Meshes, concatenated into one triangle soup.
    pub meshes: Vec<StlMeshSpec>,
}

/// One mesh for [`build_stl`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StlMeshSpec {
    /// Positions span into the f32 buffer (3 floats per vertex).
    pub positions: Span,
    /// Indices span into the u32 buffer.
    pub indices: Span,
}

/// Convert a `Transform3D` Euler rotation in degrees (extrinsic XYZ) to the
/// glTF node quaternion `[x, y, z, w]`.
///
/// Composition AUTHORITY: the kernel applies Transform3D rotations as
/// `R = Rz·Ry·Rx` on column vectors — rotate about world X first, then world
/// Y, then world Z. See crates/vcad-eval/src/kinematics.rs `euler_to_matrix`.
/// The quaternion is therefore q = qz ⊗ qy ⊗ qx.
pub fn euler_xyz_deg_to_quat(x_deg: f64, y_deg: f64, z_deg: f64) -> [f64; 4] {
    let rad = std::f64::consts::PI / 180.0;
    let hx = x_deg * rad / 2.0;
    let hy = y_deg * rad / 2.0;
    let hz = z_deg * rad / 2.0;
    let (s1, c1) = hx.sin_cos();
    let (s2, c2) = hy.sin_cos();
    let (s3, c3) = hz.sin_cos();
    [
        s1 * c2 * c3 - c1 * s2 * s3,
        c1 * s2 * c3 + s1 * c2 * s3,
        c1 * c2 * s3 - s1 * s2 * c3,
        c1 * c2 * c3 + s1 * s2 * s3,
    ]
}

fn slice_f32<'a>(buf: &'a [f32], span: &Span, what: &str) -> Result<&'a [f32], ExportError> {
    buf.get(span[0]..span[0] + span[1]).ok_or_else(|| {
        ExportError::SpanOutOfBounds(format!(
            "{what} {span:?} in f32 buffer of len {}",
            buf.len()
        ))
    })
}

fn slice_u32<'a>(buf: &'a [u32], span: &Span, what: &str) -> Result<&'a [u32], ExportError> {
    buf.get(span[0]..span[0] + span[1]).ok_or_else(|| {
        ExportError::SpanOutOfBounds(format!(
            "{what} {span:?} in u32 buffer of len {}",
            buf.len()
        ))
    })
}

/// Default PBR material used when a GLB is built with zero meshes.
const DEFAULT_MATERIAL: (&str, [f64; 3], f64, f64) = ("default", [0.8, 0.8, 0.8], 0.1, 0.5);

#[derive(Clone, PartialEq)]
struct ResolvedMaterial {
    name: String,
    color: [f64; 3],
    metallic: f64,
    roughness: f64,
    emissive: [f64; 3],
    emissive_strength: f64,
    clearcoat: f64,
    clearcoat_roughness: f64,
    alpha: f64,
}

fn is_emissive(e: &[f64; 3]) -> bool {
    e[0] > 0.0 || e[1] > 0.0 || e[2] > 0.0
}

fn pad_to_4(buf: &mut Vec<u8>, pad: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(pad);
    }
}

/// Build binary GLB bytes from meshes + PBR materials referenced through the
/// shared flat buffers. Behavior-compatible with the historical TypeScript
/// writer in `@vcad/mcp`: writes POSITION, NORMAL (when present), u32
/// indices, de-dupes materials by full PBR value, de-dupes geometry by
/// `mesh_key`, emits node TRS with identity components omitted, and encodes
/// keyframe animations (channels targeting unknown nodes are skipped).
pub fn build_glb(
    spec: &GlbSpec,
    f32_data: &[f32],
    u32_data: &[u32],
) -> Result<Vec<u8>, ExportError> {
    // Materials, deduped by full PBR value.
    let mut material_key_to_idx: HashMap<String, usize> = HashMap::new();
    let mut materials: Vec<ResolvedMaterial> = Vec::new();
    let mut material_index_for =
        |m: &GlbMeshSpec, materials: &mut Vec<ResolvedMaterial>| -> usize {
            let emissive = m.emissive.unwrap_or([0.0; 3]);
            let emissive_strength = m.emissive_strength.unwrap_or(1.0);
            let clearcoat = m.clearcoat.unwrap_or(0.0);
            let clearcoat_roughness = m.clearcoat_roughness.unwrap_or(0.0);
            let alpha = m.alpha.unwrap_or(1.0);
            let key = format!(
                "{},{},{},{},{},{},{},{},{},{},{},{}",
                m.color[0],
                m.color[1],
                m.color[2],
                m.metallic,
                m.roughness,
                emissive[0],
                emissive[1],
                emissive[2],
                emissive_strength,
                clearcoat,
                clearcoat_roughness,
                alpha
            );
            if let Some(&idx) = material_key_to_idx.get(&key) {
                return idx;
            }
            let idx = materials.len();
            material_key_to_idx.insert(key, idx);
            materials.push(ResolvedMaterial {
                name: m.name.split(':').next().unwrap_or(&m.name).to_string(),
                color: m.color,
                metallic: m.metallic,
                roughness: m.roughness,
                emissive,
                emissive_strength,
                clearcoat,
                clearcoat_roughness,
                alpha,
            });
            idx
        };

    if spec.meshes.is_empty() {
        materials.push(ResolvedMaterial {
            name: DEFAULT_MATERIAL.0.to_string(),
            color: DEFAULT_MATERIAL.1,
            metallic: DEFAULT_MATERIAL.2,
            roughness: DEFAULT_MATERIAL.3,
            emissive: [0.0; 3],
            emissive_strength: 1.0,
            clearcoat: 0.0,
            clearcoat_roughness: 0.0,
            alpha: 1.0,
        });
    }

    let mut bin: Vec<u8> = Vec::new();
    let mut buffer_views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut meshes_json: Vec<Value> = Vec::new();
    let mut nodes_json: Vec<Value> = Vec::new();

    // Geometry dedup: inputs sharing a mesh_key emit one glTF mesh.
    let mut mesh_key_to_idx: HashMap<String, usize> = HashMap::new();

    let make_node = |input: &GlbMeshSpec, mesh_idx: usize| -> Value {
        let mut node = Map::new();
        node.insert("mesh".into(), json!(mesh_idx));
        node.insert("name".into(), json!(input.name));
        if let Some(t) = &input.transform {
            let [tx, ty, tz] = t.translation;
            if tx != 0.0 || ty != 0.0 || tz != 0.0 {
                node.insert("translation".into(), json!(t.translation));
            }
            let [qx, qy, qz, qw] = t.rotation_quat;
            if qx != 0.0 || qy != 0.0 || qz != 0.0 || qw != 1.0 {
                node.insert("rotation".into(), json!(t.rotation_quat));
            }
            let [sx, sy, sz] = t.scale;
            if sx != 1.0 || sy != 1.0 || sz != 1.0 {
                node.insert("scale".into(), json!(t.scale));
            }
        }
        Value::Object(node)
    };

    for input in &spec.meshes {
        if let Some(key) = &input.mesh_key {
            if let Some(&idx) = mesh_key_to_idx.get(key) {
                nodes_json.push(make_node(input, idx));
                continue;
            }
        }

        let positions = slice_f32(f32_data, &input.positions, "positions")?;
        let vertex_count = positions.len() / 3;
        let indices = slice_u32(u32_data, &input.indices, "indices")?;
        let index_count = indices.len();
        let normals = match &input.normals {
            Some(span) if span[1] == positions.len() => Some(slice_f32(f32_data, span, "normals")?),
            _ => None,
        };

        // Bounds
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in positions.as_chunks::<3>().0 {
            for a in 0..3 {
                min[a] = min[a].min(v[a]);
                max[a] = max[a].max(v[a]);
            }
        }

        // Indices → BIN
        let indices_offset = bin.len();
        for &i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        pad_to_4(&mut bin, 0);
        let indices_bv = buffer_views.len();
        buffer_views.push(json!({
            "buffer": 0, "byteOffset": indices_offset,
            "byteLength": index_count * 4, "target": 34963
        }));
        let indices_acc = accessors.len();
        accessors.push(json!({
            "bufferView": indices_bv, "componentType": 5125,
            "count": index_count, "type": "SCALAR"
        }));

        // Positions → BIN
        let positions_offset = bin.len();
        for &v in positions {
            bin.extend_from_slice(&v.to_le_bytes());
        }
        pad_to_4(&mut bin, 0);
        let positions_bv = buffer_views.len();
        buffer_views.push(json!({
            "buffer": 0, "byteOffset": positions_offset,
            "byteLength": vertex_count * 12, "target": 34962
        }));
        let positions_acc = accessors.len();
        accessors.push(json!({
            "bufferView": positions_bv, "componentType": 5126,
            "count": vertex_count, "type": "VEC3",
            "min": [min[0], min[1], min[2]], "max": [max[0], max[1], max[2]]
        }));

        let mut attributes = Map::new();
        attributes.insert("POSITION".into(), json!(positions_acc));

        // Normals (optional)
        if let Some(normals) = normals {
            let normals_offset = bin.len();
            for &v in normals {
                bin.extend_from_slice(&v.to_le_bytes());
            }
            pad_to_4(&mut bin, 0);
            let normals_bv = buffer_views.len();
            buffer_views.push(json!({
                "buffer": 0, "byteOffset": normals_offset,
                "byteLength": vertex_count * 12, "target": 34962
            }));
            let normals_acc = accessors.len();
            accessors.push(json!({
                "bufferView": normals_bv, "componentType": 5126,
                "count": vertex_count, "type": "VEC3"
            }));
            attributes.insert("NORMAL".into(), json!(normals_acc));
        }

        let mesh_idx = meshes_json.len();
        let mat_idx = material_index_for(input, &mut materials);
        meshes_json.push(json!({
            "name": format!("mesh_{mesh_idx}"),
            "primitives": [{
                "attributes": Value::Object(attributes),
                "indices": indices_acc,
                "material": mat_idx
            }]
        }));
        if let Some(key) = &input.mesh_key {
            mesh_key_to_idx.insert(key.clone(), mesh_idx);
        }
        nodes_json.push(make_node(input, mesh_idx));
    }

    // Scene roots: default = every mesh node; with an animation
    // root_node_name, a new parent wraps them all as the sole scene root.
    let mut scene_node_indices: Vec<usize> = (0..nodes_json.len()).collect();
    if let Some(anim) = &spec.animation {
        if let Some(root_name) = &anim.root_node_name {
            let root_idx = nodes_json.len();
            nodes_json.push(json!({ "name": root_name, "children": scene_node_indices }));
            scene_node_indices = vec![root_idx];
        }
        for extra in anim.extra_nodes.as_deref().unwrap_or(&[]) {
            let idx = nodes_json.len();
            nodes_json.push(json!({ "name": extra }));
            scene_node_indices.push(idx);
        }
    }

    // Animations: sampler data lands in the same BIN chunk. Animation
    // bufferViews must NOT carry a `target` (not vertex attributes).
    let mut animations_json: Option<Value> = None;
    if let Some(anim) = &spec.animation {
        if !anim.channels.is_empty() {
            let node_index_by_name: HashMap<&str, usize> = nodes_json
                .iter()
                .enumerate()
                .filter_map(|(i, n)| n.get("name").and_then(Value::as_str).map(|s| (s, i)))
                .collect();

            let mut write_f32_accessor = |data: &[f32], ty: &str, with_min_max: bool| -> usize {
                let offset = bin.len();
                for &v in data {
                    bin.extend_from_slice(&v.to_le_bytes());
                }
                pad_to_4(&mut bin, 0);
                let bv_idx = buffer_views.len();
                buffer_views.push(json!({
                    "buffer": 0, "byteOffset": offset, "byteLength": data.len() * 4
                }));
                let components = match ty {
                    "SCALAR" => 1,
                    "VEC3" => 3,
                    _ => 4,
                };
                let acc_idx = accessors.len();
                let mut acc = Map::new();
                acc.insert("bufferView".into(), json!(bv_idx));
                acc.insert("componentType".into(), json!(5126));
                acc.insert("count".into(), json!(data.len() / components));
                acc.insert("type".into(), json!(ty));
                if with_min_max {
                    // Spec requires min/max on animation sampler input accessors.
                    let mn = data.iter().cloned().fold(f32::INFINITY, f32::min);
                    let mx = data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                    acc.insert("min".into(), json!([mn]));
                    acc.insert("max".into(), json!([mx]));
                }
                accessors.push(Value::Object(acc));
                acc_idx
            };

            let mut samplers: Vec<Value> = Vec::new();
            let mut channels: Vec<Value> = Vec::new();
            for ch in &anim.channels {
                let Some(&node_idx) = node_index_by_name.get(ch.node_name.as_str()) else {
                    // Unknown target node — skipped, matching the historical
                    // TS writer's non-throwing behavior.
                    continue;
                };
                let times = slice_f32(f32_data, &ch.times, "animation times")?;
                let values = slice_f32(f32_data, &ch.values, "animation values")?;
                let input_acc = write_f32_accessor(times, "SCALAR", true);
                let out_ty = if ch.path == "rotation" {
                    "VEC4"
                } else {
                    "VEC3"
                };
                let output_acc = write_f32_accessor(values, out_ty, false);
                let sampler_idx = samplers.len();
                samplers.push(json!({
                    "input": input_acc,
                    "output": output_acc,
                    "interpolation": ch.interpolation.as_deref().unwrap_or("LINEAR")
                }));
                channels.push(json!({
                    "sampler": sampler_idx,
                    "target": { "node": node_idx, "path": ch.path }
                }));
            }
            if !channels.is_empty() {
                animations_json = Some(json!([{
                    "name": anim.name.as_deref().unwrap_or("timeline"),
                    "samplers": samplers,
                    "channels": channels
                }]));
            }
        }
    }

    // Materials JSON, with KHR extensions where used.
    let mut uses_clearcoat = false;
    let mut uses_emissive_strength = false;
    let materials_json: Vec<Value> = materials
        .iter()
        .map(|m| {
            let mut mat = Map::new();
            mat.insert("name".into(), json!(m.name));
            mat.insert(
                "pbrMetallicRoughness".into(),
                json!({
                    "baseColorFactor": [m.color[0], m.color[1], m.color[2], m.alpha],
                    "metallicFactor": m.metallic,
                    "roughnessFactor": m.roughness
                }),
            );
            if m.alpha < 1.0 {
                // Translucent shell: alpha-blended and double-sided so the
                // underside doesn't vanish at grazing angles.
                mat.insert("alphaMode".into(), json!("BLEND"));
                mat.insert("doubleSided".into(), json!(true));
            }
            if is_emissive(&m.emissive) {
                mat.insert("emissiveFactor".into(), json!(m.emissive));
            }
            let mut extensions = Map::new();
            if m.clearcoat > 0.0 {
                uses_clearcoat = true;
                extensions.insert(
                    "KHR_materials_clearcoat".into(),
                    json!({
                        "clearcoatFactor": m.clearcoat,
                        "clearcoatRoughnessFactor": m.clearcoat_roughness
                    }),
                );
            }
            if is_emissive(&m.emissive) && m.emissive_strength != 1.0 {
                uses_emissive_strength = true;
                extensions.insert(
                    "KHR_materials_emissive_strength".into(),
                    json!({ "emissiveStrength": m.emissive_strength }),
                );
            }
            if !extensions.is_empty() {
                mat.insert("extensions".into(), Value::Object(extensions));
            }
            Value::Object(mat)
        })
        .collect();

    let mut root = Map::new();
    root.insert(
        "asset".into(),
        json!({ "version": "2.0", "generator": "vcad" }),
    );
    root.insert("scene".into(), json!(0));
    let mut scene_obj = Map::new();
    scene_obj.insert("name".into(), json!(spec.name));
    scene_obj.insert("nodes".into(), json!(scene_node_indices));
    if let Some(extras) = &spec.scene_extras {
        scene_obj.insert("extras".into(), extras.clone());
    }
    root.insert("scenes".into(), json!([Value::Object(scene_obj)]));
    root.insert("nodes".into(), Value::Array(nodes_json));
    root.insert("meshes".into(), Value::Array(meshes_json));
    root.insert("materials".into(), Value::Array(materials_json));
    root.insert("accessors".into(), Value::Array(accessors));
    root.insert("bufferViews".into(), Value::Array(buffer_views));
    root.insert("buffers".into(), json!([{ "byteLength": bin.len() }]));
    let mut extensions_used: Vec<&str> = Vec::new();
    if uses_clearcoat {
        extensions_used.push("KHR_materials_clearcoat");
    }
    if uses_emissive_strength {
        extensions_used.push("KHR_materials_emissive_strength");
    }
    if !extensions_used.is_empty() {
        root.insert("extensionsUsed".into(), json!(extensions_used));
    }
    if let Some(a) = animations_json {
        root.insert("animations".into(), a);
    }

    let mut json_chunk = serde_json::to_vec(&Value::Object(root))
        .map_err(|e| ExportError::InvalidSpec(e.to_string()))?;
    pad_to_4(&mut json_chunk, b' ');
    pad_to_4(&mut bin, 0);

    let total_length = 12 + 8 + json_chunk.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total_length);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total_length as u32).to_le_bytes());
    glb.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // "JSON"
    glb.extend_from_slice(&json_chunk);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E4942u32.to_le_bytes()); // "BIN\0"
    glb.extend_from_slice(&bin);
    Ok(glb)
}

/// Build binary STL bytes: 80-byte header, u32 triangle count, then 50 bytes
/// per triangle (normal + 3 vertices + attribute count). All meshes are
/// merged into one triangle soup; facet normals are recomputed from vertex
/// winding.
pub fn build_stl(
    spec: &StlSpec,
    f32_data: &[f32],
    u32_data: &[u32],
) -> Result<Vec<u8>, ExportError> {
    let mut total_triangles = 0usize;
    for m in &spec.meshes {
        total_triangles += m.indices[1] / 3;
    }

    let mut buf = Vec::with_capacity(84 + total_triangles * 50);
    let mut header: Vec<u8> = spec.name.bytes().take(80).collect();
    header.resize(80, b' ');
    buf.extend_from_slice(&header);
    buf.extend_from_slice(&(total_triangles as u32).to_le_bytes());

    for m in &spec.meshes {
        let positions = slice_f32(f32_data, &m.positions, "positions")?;
        let indices = slice_u32(u32_data, &m.indices, "indices")?;
        for tri in indices.as_chunks::<3>().0 {
            let mut v = [[0f32; 3]; 3];
            for (k, &idx) in tri.iter().enumerate() {
                let base = idx as usize * 3;
                let p = positions.get(base..base + 3).ok_or_else(|| {
                    ExportError::SpanOutOfBounds(format!(
                        "index {idx} beyond {} vertices",
                        positions.len() / 3
                    ))
                })?;
                v[k] = [p[0], p[1], p[2]];
            }
            let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
            let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
            let mut n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len > 1e-10 {
                n = [n[0] / len, n[1] / len, n[2] / len];
            }
            for &c in &n {
                buf.extend_from_slice(&c.to_le_bytes());
            }
            for vert in &v {
                for &c in vert {
                    buf.extend_from_slice(&c.to_le_bytes());
                }
            }
            buf.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    Ok(buf)
}

/// Parse a JSON [`GlbSpec`] and build GLB bytes — the WASM-boundary entry.
pub fn build_glb_json(
    spec_json: &str,
    f32_data: &[f32],
    u32_data: &[u32],
) -> Result<Vec<u8>, ExportError> {
    let spec: GlbSpec =
        serde_json::from_str(spec_json).map_err(|e| ExportError::InvalidSpec(e.to_string()))?;
    build_glb(&spec, f32_data, u32_data)
}

/// Parse a JSON [`StlSpec`] and build STL bytes — the WASM-boundary entry.
pub fn build_stl_json(
    spec_json: &str,
    f32_data: &[f32],
    u32_data: &[u32],
) -> Result<Vec<u8>, ExportError> {
    let spec: StlSpec =
        serde_json::from_str(spec_json).map_err(|e| ExportError::InvalidSpec(e.to_string()))?;
    build_stl(&spec, f32_data, u32_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_mesh(name: &str, pos_off: usize, idx_off: usize) -> GlbMeshSpec {
        GlbMeshSpec {
            name: name.into(),
            positions: [pos_off, 9],
            indices: [idx_off, 3],
            normals: None,
            color: [0.8, 0.2, 0.2],
            metallic: 0.1,
            roughness: 0.5,
            emissive: None,
            emissive_strength: None,
            clearcoat: None,
            clearcoat_roughness: None,
            alpha: None,
            transform: None,
            mesh_key: None,
        }
    }

    fn tri_buffers() -> (Vec<f32>, Vec<u32>) {
        (
            vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            vec![0, 1, 2],
        )
    }

    fn parse_glb(glb: &[u8]) -> (Value, usize, usize) {
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
        let total = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, glb.len());
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(
            u32::from_le_bytes(glb[16..20].try_into().unwrap()),
            0x4E4F534A
        );
        let json: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        let bin_header = 20 + json_len;
        let bin_len =
            u32::from_le_bytes(glb[bin_header..bin_header + 4].try_into().unwrap()) as usize;
        assert_eq!(
            u32::from_le_bytes(glb[bin_header + 4..bin_header + 8].try_into().unwrap()),
            0x004E4942
        );
        (json, bin_header + 8, bin_len)
    }

    #[test]
    fn glb_scene_extras_round_trip() {
        let (f32d, u32d) = tri_buffers();
        // None emits no `extras` key at all.
        let bare = GlbSpec {
            name: "test".into(),
            meshes: vec![tri_mesh("1:base", 0, 0)],
            animation: None,
            scene_extras: None,
        };
        let (json, ..) = parse_glb(&build_glb(&bare, &f32d, &u32d).unwrap());
        assert!(json["scenes"][0].get("extras").is_none());

        let stamped = GlbSpec {
            scene_extras: Some(json!({
                "io.vcad/source_up_axis": "+Z",
                "io.vcad/converted_to_y_up": true,
            })),
            ..bare
        };
        let (json, ..) = parse_glb(&build_glb(&stamped, &f32d, &u32d).unwrap());
        assert_eq!(json["scenes"][0]["extras"]["io.vcad/source_up_axis"], "+Z");
        assert_eq!(
            json["scenes"][0]["extras"]["io.vcad/converted_to_y_up"],
            true
        );
        // The rest of the scene object survives.
        assert_eq!(json["scenes"][0]["nodes"], json!([0]));
        assert_eq!(json["scenes"][0]["name"], "test");
    }

    #[test]
    fn glb_basic_structure() {
        let (f32d, u32d) = tri_buffers();
        let spec = GlbSpec {
            name: "test".into(),
            meshes: vec![tri_mesh("1:base", 0, 0)],
            animation: None,
            scene_extras: None,
        };
        let glb = build_glb(&spec, &f32d, &u32d).unwrap();
        let (json, bin_offset, bin_len) = parse_glb(&glb);
        assert_eq!(bin_offset + bin_len, glb.len());
        assert_eq!(json["nodes"][0]["name"], "1:base");
        assert_eq!(json["scenes"][0]["nodes"], json!([0]));
        assert_eq!(json["accessors"][1]["min"], json!([0.0, 0.0, 0.0]));
        assert_eq!(
            json["buffers"][0]["byteLength"].as_u64().unwrap() as usize,
            bin_len
        );
        // 4-byte aligned buffer views.
        for bv in json["bufferViews"].as_array().unwrap() {
            assert_eq!(bv["byteOffset"].as_u64().unwrap() % 4, 0);
        }
    }

    #[test]
    fn glb_material_dedup_and_extensions() {
        let (f32d, u32d) = tri_buffers();
        let mut a = tri_mesh("1:a", 0, 0);
        a.clearcoat = Some(0.8);
        a.alpha = Some(0.5);
        let b = a.clone();
        let mut c = tri_mesh("3:c", 0, 0);
        c.emissive = Some([1.0, 0.0, 0.0]);
        c.emissive_strength = Some(3.0);
        let spec = GlbSpec {
            name: "s".into(),
            meshes: vec![a, b, c],
            animation: None,
            scene_extras: None,
        };
        let glb = build_glb(&spec, &f32d, &u32d).unwrap();
        let (json, _, _) = parse_glb(&glb);
        assert_eq!(json["materials"].as_array().unwrap().len(), 2);
        assert_eq!(json["materials"][0]["alphaMode"], "BLEND");
        assert!(json["materials"][0]["extensions"]["KHR_materials_clearcoat"].is_object());
        assert!(json["materials"][1]["extensions"]["KHR_materials_emissive_strength"].is_object());
        let used = json["extensionsUsed"].as_array().unwrap();
        assert_eq!(used.len(), 2);
    }

    #[test]
    fn glb_mesh_key_dedup_and_transform() {
        let (f32d, u32d) = tri_buffers();
        let mut a = tri_mesh("1:a", 0, 0);
        a.mesh_key = Some("def1".into());
        let mut b = tri_mesh("2:b", 0, 0);
        b.mesh_key = Some("def1".into());
        b.transform = Some(TransformSpec {
            translation: [5.0, 0.0, 0.0],
            rotation_quat: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        });
        let spec = GlbSpec {
            name: "s".into(),
            meshes: vec![a, b],
            animation: None,
            scene_extras: None,
        };
        let glb = build_glb(&spec, &f32d, &u32d).unwrap();
        let (json, _, _) = parse_glb(&glb);
        assert_eq!(json["meshes"].as_array().unwrap().len(), 1);
        assert_eq!(json["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(json["nodes"][1]["mesh"], 0);
        assert_eq!(json["nodes"][1]["translation"], json!([5.0, 0.0, 0.0]));
        // Identity rotation/scale omitted.
        assert!(json["nodes"][1].get("rotation").is_none());
        assert!(json["nodes"][1].get("scale").is_none());
    }

    #[test]
    fn glb_animation_channels() {
        let (mut f32d, u32d) = tri_buffers();
        let times_off = f32d.len();
        f32d.extend_from_slice(&[0.0, 1.0, 2.0]);
        let values_off = f32d.len();
        // Three quaternion keyframes: identity, 90° about Z, 180° about Z.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        f32d.extend_from_slice(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, h, h, 0.0, 0.0, 1.0, 0.0]);
        let spec = GlbSpec {
            name: "s".into(),
            meshes: vec![tri_mesh("1:a", 0, 0), tri_mesh("2:lid", 0, 0)],
            animation: Some(AnimationSpec {
                name: Some("spin".into()),
                channels: vec![
                    AnimationChannelSpec {
                        node_name: "2:lid".into(),
                        path: "rotation".into(),
                        times: [times_off, 3],
                        values: [values_off, 12],
                        interpolation: None,
                    },
                    AnimationChannelSpec {
                        node_name: "missing".into(),
                        path: "rotation".into(),
                        times: [times_off, 3],
                        values: [values_off, 12],
                        interpolation: None,
                    },
                ],
                root_node_name: Some("__scene_root__".into()),
                extra_nodes: None,
            }),
            scene_extras: None,
        };
        let glb = build_glb(&spec, &f32d, &u32d).unwrap();
        let (json, bin_offset, _) = parse_glb(&glb);
        let root_idx = json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|n| n["name"] == "__scene_root__")
            .unwrap();
        assert_eq!(json["scenes"][0]["nodes"], json!([root_idx]));
        assert_eq!(json["nodes"][root_idx]["children"], json!([0, 1]));
        let anim = &json["animations"][0];
        assert_eq!(anim["name"], "spin");
        assert_eq!(anim["channels"].as_array().unwrap().len(), 1);
        let input = &json["accessors"][anim["samplers"][0]["input"].as_u64().unwrap() as usize];
        assert_eq!(input["min"], json!([0.0]));
        assert_eq!(input["max"], json!([2.0]));
        // Round-trip times through the BIN chunk.
        let bv = &json["bufferViews"][input["bufferView"].as_u64().unwrap() as usize];
        assert!(bv.get("target").is_none());
        let off = bin_offset + bv["byteOffset"].as_u64().unwrap() as usize;
        let t0 = f32::from_le_bytes(glb[off..off + 4].try_into().unwrap());
        assert_eq!(t0, 0.0);
    }

    #[test]
    fn glb_all_channels_skipped_omits_animation() {
        let (mut f32d, u32d) = tri_buffers();
        let times_off = f32d.len();
        f32d.extend_from_slice(&[0.0, 1.0]);
        let values_off = f32d.len();
        f32d.extend_from_slice(&[0.0; 8]);
        let spec = GlbSpec {
            name: "s".into(),
            meshes: vec![tri_mesh("1:a", 0, 0)],
            animation: Some(AnimationSpec {
                name: None,
                channels: vec![AnimationChannelSpec {
                    node_name: "nope".into(),
                    path: "rotation".into(),
                    times: [times_off, 2],
                    values: [values_off, 8],
                    interpolation: None,
                }],
                root_node_name: None,
                extra_nodes: None,
            }),
            scene_extras: None,
        };
        let glb = build_glb(&spec, &f32d, &u32d).unwrap();
        let (json, _, _) = parse_glb(&glb);
        assert!(json.get("animations").is_none());
    }

    #[test]
    fn stl_structure() {
        let (f32d, u32d) = tri_buffers();
        let spec = StlSpec {
            name: "part".into(),
            meshes: vec![StlMeshSpec {
                positions: [0, 9],
                indices: [0, 3],
            }],
        };
        let stl = build_stl(&spec, &f32d, &u32d).unwrap();
        assert_eq!(stl.len(), 84 + 50);
        assert_eq!(&stl[0..4], b"part");
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 1);
        // Normal of CCW triangle in XY plane is +Z.
        let nz = f32::from_le_bytes(stl[92..96].try_into().unwrap());
        assert!((nz - 1.0).abs() < 1e-6);
    }

    #[test]
    fn euler_quat_matches_zyx_composition() {
        // 90° about Z only.
        let q = euler_xyz_deg_to_quat(0.0, 0.0, 90.0);
        assert!((q[2] - (std::f64::consts::FRAC_1_SQRT_2)).abs() < 1e-12);
        assert!((q[3] - (std::f64::consts::FRAC_1_SQRT_2)).abs() < 1e-12);
        // Identity.
        let q = euler_xyz_deg_to_quat(0.0, 0.0, 0.0);
        assert_eq!(q, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn span_out_of_bounds_is_an_error() {
        let spec = GlbSpec {
            name: "s".into(),
            meshes: vec![tri_mesh("1:a", 0, 0)],
            animation: None,
            scene_extras: None,
        };
        assert!(build_glb(&spec, &[0.0; 3], &[0, 1, 2]).is_err());
    }
}
