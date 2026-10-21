//! File ▸ Export (STL / STEP / GLB) and File ▸ Render Photoreal… for the
//! native app.
//!
//! The exports mirror `vcad export`: STL is every root plus every posed
//! assembly instance merged into one soup; GLB keeps one named mesh per root
//! and instance; STEP serializes the analytic B-rep of each root and refuses
//! mesh-only roots by name (exactly the CLI's policy, so a "STEP" never
//! silently arrives as triangles of a mesh import).
//!
//! The photoreal render is `vcad render --photoreal`'s renderer, linked in
//! rather than shelled out to: GPU tracer first, CPU reference integrator
//! when no adapter is available. GPU renders report progress per sample
//! batch and can be cancelled between batches.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::ptr;

use vcad_eval::{EvalOptions, EvaluatedMesh};
use vcad_ir::Document;
use vcad_kernel::Solid;
use vcad_kernel_export::{build_glb, build_stl, GlbMeshSpec, GlbSpec, StlMeshSpec, StlSpec};

use crate::err;

/// Binary STL.
pub const VCAD_EXPORT_STL: u32 = 0;
/// AP214 STEP (analytic B-rep).
pub const VCAD_EXPORT_STEP: u32 = 1;
/// Binary glTF.
pub const VCAD_EXPORT_GLB: u32 = 2;

unsafe fn utf8<'a>(p: *const u8, len: usize, what: &str) -> Result<&'a str, String> {
    if p.is_null() {
        return Err(format!("{what} is null"));
    }
    std::str::from_utf8(std::slice::from_raw_parts(p, len))
        .map_err(|_| format!("{what} is not UTF-8"))
}

/// Parse a document and resolve relative mesh paths against `base_dir`.
pub(crate) fn load_document(json: &str, base_dir: Option<&str>) -> Result<Document, String> {
    let mut doc = Document::from_json(json).map_err(|e| format!("parse: {e}"))?;
    if let Some(dir) = base_dir.filter(|d| !d.is_empty()) {
        vcad_eval::resolve_mesh_paths(&mut doc, Path::new(dir));
    }
    Ok(doc)
}

/// Every root and every posed instance as `(name, mesh)`, in document order.
fn named_meshes(doc: &Document) -> Result<Vec<(String, EvaluatedMesh)>, String> {
    let opts = EvalOptions {
        skip_clash_detection: true,
        mesh_segments: 0,
        ..Default::default()
    };
    let scene = vcad_eval::evaluate_document_with_sheet_metal(doc, &opts)
        .map_err(|e| format!("evaluate: {e}"))?;
    let placed = vcad_eval::placed_instance_meshes(doc, &scene);
    let mut out: Vec<(String, EvaluatedMesh)> = scene
        .parts
        .into_iter()
        .enumerate()
        .map(|(i, part)| {
            let name = doc
                .roots
                .get(i)
                .and_then(|r| doc.nodes.get(&r.root))
                .and_then(|n| n.name.clone())
                .unwrap_or_else(|| format!("part_{}", i + 1));
            (name, part.mesh)
        })
        .collect();
    out.extend(placed);
    out.retain(|(_, m)| !m.positions.is_empty() && !m.indices.is_empty());
    if out.is_empty() {
        return Err("document has no geometry to export".into());
    }
    Ok(out)
}

/// Binary STL bytes of the whole document.
pub fn export_stl(doc: &Document, name: &str) -> Result<(Vec<u8>, usize), String> {
    let meshes = named_meshes(doc)?;
    let (mut f, mut u, mut specs) = (Vec::new(), Vec::new(), Vec::new());
    for (_, m) in &meshes {
        specs.push(StlMeshSpec {
            positions: [f.len(), m.positions.len()],
            indices: [u.len(), m.indices.len()],
        });
        f.extend_from_slice(&m.positions);
        u.extend_from_slice(&m.indices);
    }
    let spec = StlSpec {
        name: format!("vcad {name}"),
        meshes: specs,
    };
    let bytes = build_stl(&spec, &f, &u).map_err(|e| format!("STL: {e}"))?;
    Ok((bytes, meshes.len()))
}

/// Binary glTF bytes, one named mesh per root / instance.
pub fn export_glb(doc: &Document, name: &str) -> Result<(Vec<u8>, usize), String> {
    let meshes = named_meshes(doc)?;
    let (mut f, mut u, mut specs) = (Vec::new(), Vec::new(), Vec::new());
    for (mesh_name, m) in &meshes {
        let pos = [f.len(), m.positions.len()];
        f.extend_from_slice(&m.positions);
        let normals = m.normals.as_ref().map(|n| {
            let span = [f.len(), n.len()];
            f.extend_from_slice(n);
            span
        });
        let idx = [u.len(), m.indices.len()];
        u.extend_from_slice(&m.indices);
        specs.push(GlbMeshSpec {
            name: mesh_name.clone(),
            positions: pos,
            indices: idx,
            normals,
            color: [0.71, 0.71, 0.75],
            metallic: 0.1,
            roughness: 0.7,
            emissive: None,
            emissive_strength: None,
            clearcoat: None,
            clearcoat_roughness: None,
            alpha: None,
            transform: None,
            mesh_key: None,
        });
    }
    let count = specs.len();
    let spec = GlbSpec {
        name: name.to_string(),
        meshes: specs,
        animation: None,
        scene_extras: None,
    };
    let bytes = build_glb(&spec, &f, &u).map_err(|e| format!("GLB: {e}"))?;
    Ok((bytes, count))
}

/// AP214 STEP bytes. Refuses mesh-only roots by name.
pub fn export_step(doc: &Document) -> Result<(Vec<u8>, usize), String> {
    let roots = vcad_eval::evaluate_root_solids(doc).map_err(|e| format!("evaluate: {e}"))?;
    if roots.is_empty() {
        return Err("document has no geometry to export".into());
    }
    let mesh_only: Vec<String> = roots
        .iter()
        .filter(|r| !r.solid.as_ref().is_some_and(Solid::can_export_step))
        .map(|r| {
            r.name
                .clone()
                .unwrap_or_else(|| format!("node {}", r.node_id))
        })
        .collect();
    if !mesh_only.is_empty() {
        return Err(format!(
            "STEP needs B-rep geometry, but {} of {} part(s) are mesh-only or empty: {}. \
             Export those as STL or GLB instead.",
            mesh_only.len(),
            roots.len(),
            mesh_only.join(", ")
        ));
    }
    let named: Vec<(&Solid, String)> = roots
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let name = r.name.clone().unwrap_or_else(|| format!("part_{}", i + 1));
            r.solid.as_ref().map(|s| (s, name))
        })
        .collect();
    let refs: Vec<(&Solid, &str)> = named.iter().map(|(s, n)| (*s, n.as_str())).collect();
    let bytes = Solid::solids_to_step_buffer(&refs).map_err(|e| format!("STEP: {e}"))?;
    Ok((bytes, named.len()))
}

/// Export a document to `out_path` as `format` (`VCAD_EXPORT_*`).
///
/// `base_dir` (nullable) resolves relative mesh-import paths, as in
/// `vcad_scene_from_json_in`. On success returns the number of parts written
/// (≥ 1); on failure returns -1 with `vcad_last_error` set. The file is
/// written atomically (temp file + rename) so a failed export never leaves a
/// truncated file behind.
#[no_mangle]
pub extern "C" fn vcad_export_document(
    json: *const u8,
    json_len: usize,
    base_dir: *const u8,
    base_dir_len: usize,
    out_path: *const u8,
    out_path_len: usize,
    format: u32,
) -> i32 {
    err::clear_error();
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<usize, String> {
        let text = unsafe { utf8(json, json_len, "document") }?;
        let dir = if base_dir.is_null() || base_dir_len == 0 {
            None
        } else {
            Some(unsafe { utf8(base_dir, base_dir_len, "base_dir") }?)
        };
        let out = Path::new(unsafe { utf8(out_path, out_path_len, "out_path") }?);
        let doc = load_document(text, dir)?;
        let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("vcad");
        let (bytes, count) = match format {
            VCAD_EXPORT_STL => export_stl(&doc, stem)?,
            VCAD_EXPORT_STEP => export_step(&doc)?,
            VCAD_EXPORT_GLB => export_glb(&doc, stem)?,
            other => return Err(format!("unknown export format {other}")),
        };
        write_atomic(out, &bytes)?;
        Ok(count)
    }));
    match result {
        Ok(Ok(n)) => n.min(i32::MAX as usize) as i32,
        Ok(Err(e)) => {
            err::set_error(e);
            -1
        }
        Err(_) => {
            err::set_error("export panicked");
            -1
        }
    }
}

fn write_atomic(out: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = out.with_extension(format!(
        "{}.partial",
        out.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("write {}: {e}", out.display())
    })
}

// MARK: photoreal

/// Progress callback: `(ctx, samples_done, samples_total)`. Return 0 to keep
/// going, non-zero to cancel. Called from the rendering thread.
pub type VcadRenderProgress = Option<extern "C" fn(*mut c_void, u32, u32) -> u8>;

/// Owned encoded bytes (a PNG). Read with `vcad_bytes_view`, free with
/// `vcad_bytes_free`.
pub struct VcadBytes {
    data: Vec<u8>,
}

/// Borrowed view of a [`VcadBytes`].
#[repr(C)]
pub struct VcadBytesView {
    /// First byte (null when empty).
    pub data: *const u8,
    /// Byte count.
    pub len: usize,
}

/// View the bytes. Valid until `vcad_bytes_free`.
#[no_mangle]
pub extern "C" fn vcad_bytes_view(bytes: *const VcadBytes) -> VcadBytesView {
    match unsafe { bytes.as_ref() } {
        Some(b) => VcadBytesView {
            data: b.data.as_ptr(),
            len: b.data.len(),
        },
        None => VcadBytesView {
            data: ptr::null(),
            len: 0,
        },
    }
}

/// Free bytes returned by this library. Null is a no-op.
#[no_mangle]
pub extern "C" fn vcad_bytes_free(bytes: *mut VcadBytes) {
    if !bytes.is_null() {
        drop(unsafe { Box::from_raw(bytes) });
    }
}

/// Photoreal render parameters.
#[repr(C)]
pub struct VcadPhotorealParams {
    /// Image width, pixels (1..=16384).
    pub width: u32,
    /// Image height, pixels (1..=16384).
    pub height: u32,
    /// Samples per pixel (≥ 1).
    pub samples: u32,
    /// Camera azimuth, degrees CCW from +X (kernel Z-up frame).
    pub azimuth_deg: f64,
    /// Camera elevation above the XY plane, degrees.
    pub elevation_deg: f64,
    /// Non-zero: skip the GPU and trace on the CPU reference integrator.
    pub force_cpu: u8,
}

/// Render a document photoreal (the `vcad render --photoreal` renderer) to a
/// PNG. Blocking: call it off the main thread. Returns null with
/// `vcad_last_error` set on failure; a cancelled render reports "cancelled".
#[no_mangle]
pub extern "C" fn vcad_render_photoreal_png(
    json: *const u8,
    json_len: usize,
    base_dir: *const u8,
    base_dir_len: usize,
    params: *const VcadPhotorealParams,
    progress: VcadRenderProgress,
    ctx: *mut c_void,
) -> *mut VcadBytes {
    err::clear_error();
    let ctx_addr = ctx as usize;
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, String> {
        let p = unsafe { params.as_ref() }.ok_or("params is null")?;
        let text = unsafe { utf8(json, json_len, "document") }?;
        let dir = if base_dir.is_null() || base_dir_len == 0 {
            None
        } else {
            Some(unsafe { utf8(base_dir, base_dir_len, "base_dir") }?)
        };
        // Resolve mesh paths up front: the renderer takes raw JSON.
        let doc = load_document(text, dir)?;
        let raw = doc.to_json().map_err(|e| format!("serialize: {e}"))?;
        let mut report = |done: u32, total: u32| -> bool {
            match progress {
                Some(cb) => cb(ctx_addr as *mut c_void, done, total) == 0,
                None => true,
            }
        };
        render_png(&raw, p, &mut report)
    }));
    match result {
        Ok(Ok(data)) => Box::into_raw(Box::new(VcadBytes { data })),
        Ok(Err(e)) => {
            err::set_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            err::set_error("render panicked");
            ptr::null_mut()
        }
    }
}

/// The options `vcad render --photoreal` uses, at the app's camera.
// The trailing `..Default::default()` is deliberate (future renderer fields
// default) even while every field happens to be listed.
#[allow(clippy::needless_update)]
fn options(
    p: &VcadPhotorealParams,
    cpu: bool,
) -> Result<
    (
        vcad_render::RasterOptions,
        vcad_render::photoreal::PhotorealOptions,
    ),
    String,
> {
    if p.width == 0 || p.height == 0 || p.width > 16384 || p.height > 16384 {
        return Err(format!(
            "size {}x{} out of range (1..=16384 per side)",
            p.width, p.height
        ));
    }
    if p.samples == 0 {
        return Err("samples must be at least 1".into());
    }
    let view = format!("orbit:{},{}", p.azimuth_deg, p.elevation_deg)
        .parse::<vcad_render::View>()
        .map_err(|e| format!("camera: {e}"))?;
    let ro = vcad_render::RasterOptions {
        view,
        size_px: p.width,
        height_px: Some(p.height),
        ..Default::default()
    };
    let pr = vcad_render::photoreal::PhotorealOptions {
        environment: vcad_render::envmap::parse_env_arg("gradient"),
        env_rotation_deg: 0.0,
        spp: p.samples,
        max_depth: 6,
        exposure: 1.0,
        fov_deg: 34.0,
        orthographic: false,
        aperture_frac: 0.0,
        focus_dist: None,
        backdrop: vcad_render::photoreal::Backdrop::Studio,
        seed: 0x5eed_1234,
        denoise: cpu,
        adaptive: true,
        exact: false,
        // Options added to the renderer later take its defaults, so a new
        // field cannot break this crate the way focus_dist and lighting did.
        ..Default::default()
    };
    Ok((ro, pr))
}

fn render_png(
    raw: &str,
    p: &VcadPhotorealParams,
    report: &mut dyn FnMut(u32, u32) -> bool,
) -> Result<Vec<u8>, String> {
    if p.force_cpu == 0 {
        let (ro, pr) = options(p, false)?;
        match vcad_render::photoreal_gpu::render_photoreal_gpu_png_str_progress(
            raw, &ro, &pr, report,
        ) {
            Err(e)
                if e.contains("no compatible GPU adapter")
                    || e.contains("could not initialise the GPU") => {}
            other => return other,
        }
    }
    // CPU reference integrator: no per-batch hook, so progress is
    // start/finish and cancel is honoured before the trace begins.
    if !report(0, p.samples) {
        return Err("cancelled".into());
    }
    let (ro, pr) = options(p, true)?;
    let png = vcad_render::photoreal::render_photoreal_png_str(raw, &ro, &pr)?;
    report(p.samples, p.samples);
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{
      "version": "0.1",
      "nodes": {
        "1": { "id": 1, "name": "Block", "op": { "type": "Cube", "size": { "x": 10, "y": 20, "z": 5 } } }
      },
      "materials": {},
      "part_materials": {},
      "roots": [{ "root": 1, "material": "default" }]
    }"#;

    fn doc() -> Document {
        load_document(DOC, None).expect("fixture parses")
    }

    #[test]
    fn stl_has_header_count_and_triangles() {
        let (bytes, parts) = export_stl(&doc(), "block").unwrap();
        assert_eq!(parts, 1);
        let tris = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        assert!(tris >= 12, "a box is at least 12 triangles, got {tris}");
        assert_eq!(bytes.len(), 84 + tris * 50);
    }

    #[test]
    fn glb_is_gltf2_binary() {
        let (bytes, parts) = export_glb(&doc(), "block").unwrap();
        assert_eq!(parts, 1);
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    }

    #[test]
    fn step_is_ap214_with_named_solid() {
        let (bytes, parts) = export_step(&doc()).unwrap();
        assert_eq!(parts, 1);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("ISO-10303-21;"));
        assert!(text.contains("MANIFOLD_SOLID_BREP"));
        assert!(text.contains("Block"));
    }

    #[test]
    fn ffi_export_writes_file_and_counts_parts() {
        let dir = std::env::temp_dir().join(format!("vcad-ffi-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (fmt, ext) in [
            (VCAD_EXPORT_STL, "stl"),
            (VCAD_EXPORT_STEP, "step"),
            (VCAD_EXPORT_GLB, "glb"),
        ] {
            let out = dir.join(format!("block.{ext}"));
            let p = out.to_str().unwrap();
            let n = vcad_export_document(
                DOC.as_ptr(),
                DOC.len(),
                ptr::null(),
                0,
                p.as_ptr(),
                p.len(),
                fmt,
            );
            assert_eq!(n, 1, "{ext}: {:?}", err::last_error_string());
            assert!(std::fs::metadata(&out).unwrap().len() > 84);
            assert!(!out.with_extension(format!("{ext}.partial")).exists());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ffi_export_reports_errors() {
        let out = "/nonexistent-dir/x.stl";
        let n = vcad_export_document(
            b"{".as_ptr(),
            1,
            ptr::null(),
            0,
            out.as_ptr(),
            out.len(),
            VCAD_EXPORT_STL,
        );
        assert_eq!(n, -1);
        assert!(err::last_error_string().unwrap().contains("parse"));
        let n = vcad_export_document(
            DOC.as_ptr(),
            DOC.len(),
            ptr::null(),
            0,
            out.as_ptr(),
            out.len(),
            99,
        );
        assert_eq!(n, -1);
        assert!(err::last_error_string()
            .unwrap()
            .contains("unknown export format"));
    }

    #[test]
    fn photoreal_rejects_bad_params() {
        let p = VcadPhotorealParams {
            width: 0,
            height: 10,
            samples: 4,
            azimuth_deg: 35.0,
            elevation_deg: 20.0,
            force_cpu: 1,
        };
        let r = vcad_render_photoreal_png(
            DOC.as_ptr(),
            DOC.len(),
            ptr::null(),
            0,
            &p,
            None,
            ptr::null_mut(),
        );
        assert!(r.is_null());
        assert!(err::last_error_string().unwrap().contains("out of range"));
    }

    extern "C" fn cancel_now(_: *mut c_void, _: u32, _: u32) -> u8 {
        1
    }

    #[test]
    fn photoreal_cancel_is_reported() {
        let p = VcadPhotorealParams {
            width: 32,
            height: 24,
            samples: 4,
            azimuth_deg: 35.0,
            elevation_deg: 20.0,
            force_cpu: 1,
        };
        let r = vcad_render_photoreal_png(
            DOC.as_ptr(),
            DOC.len(),
            ptr::null(),
            0,
            &p,
            Some(cancel_now),
            ptr::null_mut(),
        );
        assert!(r.is_null());
        assert_eq!(err::last_error_string().as_deref(), Some("cancelled"));
    }

    #[test]
    fn photoreal_cpu_renders_png() {
        let p = VcadPhotorealParams {
            width: 32,
            height: 24,
            samples: 2,
            azimuth_deg: -30.0,
            elevation_deg: 25.0,
            force_cpu: 1,
        };
        let r = vcad_render_photoreal_png(
            DOC.as_ptr(),
            DOC.len(),
            ptr::null(),
            0,
            &p,
            None,
            ptr::null_mut(),
        );
        assert!(!r.is_null(), "{:?}", err::last_error_string());
        let v = vcad_bytes_view(r);
        let png = unsafe { std::slice::from_raw_parts(v.data, v.len) };
        assert_eq!(&png[1..4], b"PNG");
        vcad_bytes_free(r);
    }
}
