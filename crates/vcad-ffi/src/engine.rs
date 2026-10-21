//! Document engine over the C ABI: the same `DocCommand` vocabulary the web
//! app sends through `WasmDocumentEngine::apply`.
//!
//! A `VcadEngine` owns one `vcad_app::DocumentApi`. Commands go in as UTF-8
//! JSON; the shared result envelope comes back as owned JSON (free with
//! `vcad_engine_string_free`). Errors are part of the envelope, so the only
//! null return is a null/invalid argument or a panic (see `vcad_last_error`).

use std::ffi::{c_char, CStr, CString};

use vcad_app::DocumentApi;
use vcad_crdt::ReplicaId;

/// Opaque engine handle.
pub struct VcadEngine {
    api: DocumentApi,
}

/// Create an empty document engine. `replica_id` must be unique per
/// collaborating client. Free with `vcad_engine_free`.
#[no_mangle]
pub extern "C" fn vcad_engine_new(replica_id: u64) -> *mut VcadEngine {
    Box::into_raw(Box::new(VcadEngine {
        api: DocumentApi::new(ReplicaId(replica_id)),
    }))
}

/// Apply a JSON `DocCommand`; returns the owned JSON result envelope.
#[no_mangle]
pub extern "C" fn vcad_engine_apply(
    engine: *mut VcadEngine,
    command_json: *const c_char,
) -> *mut c_char {
    crate::err::clear_error();
    if engine.is_null() || command_json.is_null() {
        crate::err::set_error("vcad_engine_apply: null argument");
        return std::ptr::null_mut();
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let json = unsafe { CStr::from_ptr(command_json) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let engine = unsafe { &mut *engine };
        Ok::<_, String>(engine.api.apply_json_envelope(json))
    }));
    match result {
        Ok(Ok(out)) => CString::new(out).map_or(std::ptr::null_mut(), CString::into_raw),
        Ok(Err(e)) => {
            crate::err::set_error(e);
            std::ptr::null_mut()
        }
        Err(_) => {
            crate::err::set_error("vcad_engine_apply panicked");
            std::ptr::null_mut()
        }
    }
}

/// Lower and apply an AI tool call; same envelope as `vcad_engine_apply`.
#[no_mangle]
pub extern "C" fn vcad_engine_apply_tool(
    engine: *mut VcadEngine,
    tool: *const c_char,
    args_json: *const c_char,
) -> *mut c_char {
    crate::err::clear_error();
    if engine.is_null() || tool.is_null() || args_json.is_null() {
        crate::err::set_error("vcad_engine_apply_tool: null argument");
        return std::ptr::null_mut();
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let tool = unsafe { CStr::from_ptr(tool) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let args = unsafe { CStr::from_ptr(args_json) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let engine = unsafe { &mut *engine };
        Ok::<_, String>(engine.api.apply_tool_envelope(tool, args))
    }));
    match result {
        Ok(Ok(out)) => CString::new(out).map_or(std::ptr::null_mut(), CString::into_raw),
        Ok(Err(e)) => {
            crate::err::set_error(e);
            std::ptr::null_mut()
        }
        Err(_) => {
            crate::err::set_error("vcad_engine_apply_tool panicked");
            std::ptr::null_mut()
        }
    }
}

/// Deterministic state fingerprint (16 hex digits, owned; free with
/// `vcad_engine_string_free`). Matches `WasmDocumentEngine.stateHash`.
#[no_mangle]
pub extern "C" fn vcad_engine_state_hash(engine: *const VcadEngine) -> *mut c_char {
    if engine.is_null() {
        crate::err::set_error("vcad_engine_state_hash: null engine");
        return std::ptr::null_mut();
    }
    let engine = unsafe { &*engine };
    CString::new(engine.api.state_hash()).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// Current state as an `ok` result envelope (owned; free with
/// `vcad_engine_string_free`). Hosts call this after load/restore to render.
#[no_mangle]
pub extern "C" fn vcad_engine_state(engine: *const VcadEngine) -> *mut c_char {
    crate::err::clear_error();
    if engine.is_null() {
        crate::err::set_error("vcad_engine_state: null engine");
        return std::ptr::null_mut();
    }
    let engine = unsafe { &*engine };
    owned(engine.api.state_envelope())
}

/// Replace the engine's document with an IR `Document` JSON (a `.vcad`
/// file's contents), starting a fresh undo history. Returns the state
/// envelope, or null (see `vcad_last_error`) when the JSON isn't an IR
/// document — in which case the engine is left untouched.
#[no_mangle]
pub extern "C" fn vcad_engine_load_ir(
    engine: *mut VcadEngine,
    ir_json: *const c_char,
) -> *mut c_char {
    crate::err::clear_error();
    if engine.is_null() || ir_json.is_null() {
        crate::err::set_error("vcad_engine_load_ir: null argument");
        return std::ptr::null_mut();
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let json = unsafe { CStr::from_ptr(ir_json) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let engine = unsafe { &mut *engine };
        let replica = engine.api.crdt().replica_id();
        engine.api = DocumentApi::from_ir(json, replica)?;
        Ok::<_, String>(engine.api.state_envelope())
    }));
    finish(result, "vcad_engine_load_ir")
}

/// Snapshot the whole engine state (document + undo/redo history) as
/// bytes. Writes the length to `out_len`; free with `vcad_engine_bytes_free`.
#[no_mangle]
pub extern "C" fn vcad_engine_save(engine: *const VcadEngine, out_len: *mut usize) -> *mut u8 {
    crate::err::clear_error();
    if engine.is_null() || out_len.is_null() {
        crate::err::set_error("vcad_engine_save: null argument");
        return std::ptr::null_mut();
    }
    let engine = unsafe { &*engine };
    let bytes = engine.api.save().into_boxed_slice();
    unsafe { *out_len = bytes.len() };
    Box::into_raw(bytes).cast::<u8>()
}

/// Free bytes returned by `vcad_engine_save`.
#[no_mangle]
pub extern "C" fn vcad_engine_bytes_free(bytes: *mut u8, len: usize) {
    if !bytes.is_null() {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                bytes, len,
            )))
        };
    }
}

/// Restore a `vcad_engine_save` snapshot (document + history). Returns the
/// state envelope, or null on bad bytes (engine untouched).
#[no_mangle]
pub extern "C" fn vcad_engine_restore(
    engine: *mut VcadEngine,
    bytes: *const u8,
    len: usize,
) -> *mut c_char {
    crate::err::clear_error();
    if engine.is_null() || bytes.is_null() {
        crate::err::set_error("vcad_engine_restore: null argument");
        return std::ptr::null_mut();
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let data = unsafe { std::slice::from_raw_parts(bytes, len) };
        let engine = unsafe { &mut *engine };
        engine.api.restore_snapshot(data)?;
        Ok::<_, String>(engine.api.state_envelope())
    }));
    finish(result, "vcad_engine_restore")
}

/// Whether `{"type":"undo"}` would succeed.
#[no_mangle]
pub extern "C" fn vcad_engine_can_undo(engine: *const VcadEngine) -> bool {
    !engine.is_null() && unsafe { &*engine }.api.can_undo()
}

/// Whether `{"type":"redo"}` would succeed.
#[no_mangle]
pub extern "C" fn vcad_engine_can_redo(engine: *const VcadEngine) -> bool {
    !engine.is_null() && unsafe { &*engine }.api.can_redo()
}

fn owned(s: String) -> *mut c_char {
    CString::new(s).map_or(std::ptr::null_mut(), CString::into_raw)
}

fn finish(result: std::thread::Result<Result<String, String>>, what: &str) -> *mut c_char {
    match result {
        Ok(Ok(out)) => owned(out),
        Ok(Err(e)) => {
            crate::err::set_error(format!("{what}: {e}"));
            std::ptr::null_mut()
        }
        Err(_) => {
            crate::err::set_error(format!("{what} panicked"));
            std::ptr::null_mut()
        }
    }
}

/// Free a string returned by `vcad_engine_apply` / `vcad_engine_apply_tool`.
#[no_mangle]
pub extern "C" fn vcad_engine_string_free(value: *mut c_char) {
    if !value.is_null() {
        unsafe { drop(CString::from_raw(value)) };
    }
}

/// Free an engine from `vcad_engine_new`.
#[no_mangle]
pub extern "C" fn vcad_engine_free(engine: *mut VcadEngine) {
    if !engine.is_null() {
        unsafe { drop(Box::from_raw(engine)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_app::parity::{outcome, Expect, Fixture, Step};

    fn take(raw: *mut c_char) -> String {
        assert!(!raw.is_null());
        let s = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_owned();
        vcad_engine_string_free(raw);
        s
    }

    /// C-ABI leg of the parity harness (`crates/vcad-app/tests/parity`).
    #[test]
    fn parity_fixtures_match_golden() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vcad-app/tests/parity");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let fixture: Fixture =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let engine = vcad_engine_new(fixture.replica);
            let outcomes = fixture
                .steps
                .iter()
                .map(|step| {
                    let raw = match step {
                        Step::Command { command } => {
                            let c = CString::new(command.to_string()).unwrap();
                            vcad_engine_apply(engine, c.as_ptr())
                        }
                        Step::Tool { tool, args } => {
                            let t = CString::new(tool.as_str()).unwrap();
                            let a = CString::new(args.to_string()).unwrap();
                            vcad_engine_apply_tool(engine, t.as_ptr(), a.as_ptr())
                        }
                    };
                    outcome(&take(raw))
                })
                .collect();
            let got = Expect {
                outcomes,
                hash: take(vcad_engine_state_hash(engine)),
            };
            vcad_engine_free(engine);
            assert_eq!(got, fixture.expect, "{}", path.display());
            seen += 1;
        }
        assert!(seen > 0);
    }

    fn apply(engine: *mut VcadEngine, json: &str) -> serde_json::Value {
        let c = CString::new(json).unwrap();
        serde_json::from_str(&take(vcad_engine_apply(engine, c.as_ptr()))).unwrap()
    }

    const IR: &str = r#"{"version":"0.1","nodes":{"1":{"id":1,"name":"Box","op":{"type":"Cube","size":{"x":10,"y":20,"z":30}}}},"materials":{},"part_materials":{},"roots":[{"root":1,"material":"steel"}]}"#;

    #[test]
    fn load_ir_starts_fresh_history_and_maps_part_ids() {
        let engine = vcad_engine_new(3);
        apply(
            engine,
            r#"{"type":"add_feature","feature":{"type":"Sphere","radius":1}}"#,
        );
        assert!(vcad_engine_can_undo(engine));
        let ir = CString::new(IR).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&take(vcad_engine_load_ir(engine, ir.as_ptr()))).unwrap();
        assert_eq!(v["ok"], true);
        assert!(!vcad_engine_can_undo(engine) && !vcad_engine_can_redo(engine));
        let doc = &v["result"]["document"];
        let roots = doc["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0]["material"], "steel");
        let root = roots[0]["root"].to_string();
        assert!(
            doc["partIds"][&root].as_str().is_some(),
            "root must map to a stable part id: {doc}"
        );
        // Bad JSON fails closed and leaves the document alone.
        let bad = CString::new(r#"{"nope":1}"#).unwrap();
        assert!(vcad_engine_load_ir(engine, bad.as_ptr()).is_null());
        let s: serde_json::Value = serde_json::from_str(&take(vcad_engine_state(engine))).unwrap();
        assert_eq!(
            s["result"]["document"]["roots"].as_array().unwrap().len(),
            1
        );
        vcad_engine_free(engine);
    }

    #[test]
    fn save_restore_round_trips_document_and_history() {
        let engine = vcad_engine_new(4);
        apply(
            engine,
            r#"{"type":"add_feature","feature":{"type":"Cube","size_x":1,"size_y":1,"size_z":1}}"#,
        );
        let hash = take(vcad_engine_state_hash(engine));
        let mut len = 0usize;
        let bytes = vcad_engine_save(engine, &mut len);
        assert!(!bytes.is_null() && len > 0);
        apply(
            engine,
            r#"{"type":"add_feature","feature":{"type":"Sphere","radius":2}}"#,
        );
        assert_ne!(take(vcad_engine_state_hash(engine)), hash);
        let out = take(vcad_engine_restore(engine, bytes, len));
        vcad_engine_bytes_free(bytes, len);
        assert!(out.contains(r#""ok":true"#));
        assert_eq!(take(vcad_engine_state_hash(engine)), hash);
        // History came back with the snapshot: the cube add is undoable.
        assert!(vcad_engine_can_undo(engine));
        assert_eq!(apply(engine, r#"{"type":"undo"}"#)["ok"], true);
        let junk = [1u8, 2, 3];
        assert!(vcad_engine_restore(engine, junk.as_ptr(), junk.len()).is_null());
        assert!(!vcad_engine_can_undo(std::ptr::null()));
        vcad_engine_free(engine);
    }

    #[test]
    fn apply_round_trips_through_c_abi() {
        let engine = vcad_engine_new(7);
        let cmd = CString::new(
            r#"{"type":"add_feature","feature":{"type":"Cube","size_x":2,"size_y":2,"size_z":2}}"#,
        )
        .unwrap();
        let out = vcad_engine_apply(engine, cmd.as_ptr());
        assert!(!out.is_null());
        let text = unsafe { CStr::from_ptr(out) }.to_str().unwrap().to_owned();
        vcad_engine_string_free(out);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["ok"], true);
        assert!(vcad_engine_apply(std::ptr::null_mut(), cmd.as_ptr()).is_null());
        vcad_engine_free(engine);
    }
}
