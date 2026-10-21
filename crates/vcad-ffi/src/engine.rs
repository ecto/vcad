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

    /// Rust-produced JSON for every engine type the Mac app's generated
    /// Swift Codable mirrors (`apple/VcadApp/Sources/VcadApp/Generated/`)
    /// decode. `DocumentEngineCodegenTests` round-trips each value through
    /// Swift; this test keeps the fixture byte-identical to what Rust emits
    /// (`VCAD_UPDATE_FIXTURES=1` rewrites it).
    #[test]
    fn swift_codegen_fixture_matches_rust_output() {
        use serde_json::{json, Value};
        let engine = vcad_engine_new(5);
        let mut envelopes = Vec::new();
        let mut run = |cmd: Value| {
            let c = CString::new(cmd.to_string()).unwrap();
            let out: Value =
                serde_json::from_str(&take(vcad_engine_apply(engine, c.as_ptr()))).unwrap();
            envelopes.push(out.clone());
            out
        };
        let cube = run(json!({"type":"add_feature","name":"Plate",
            "feature":{"type":"Cube","size_x":40.0,"size_y":20.0,"size_z":5.0}}));
        let a = cube["result"]["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        let pin = run(json!({"type":"add_feature",
            "feature":{"type":"Cylinder","radius":3.0,"height":12.0,"segments":32}}));
        let b = pin["result"]["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        let both = run(json!({"type":"add_feature","feature":{"type":"Boolean",
            "boolean_type":"difference","input_a":a,"input_b":b}}));
        let c = both["result"]["createdFeatureId"]
            .as_str()
            .unwrap()
            .to_string();
        run(json!({"type":"add_feature","feature":{"type":"Fillet","input":c,"radius":1.5}}));
        run(json!({"type":"delete_feature","id":"9:9"}));
        run(
            json!({"type":"batch","commands":[{"type":"undo"},{"type":"rename","id":"9:9","name":"x"}]}),
        );
        run(json!({"type":"set_param","id":a,"key":"size_x","value":"oops"}));
        vcad_engine_free(engine);

        let feat = json!({"type":"Cube","size_x":1.0,"size_y":2.0,"size_z":3.0});
        let commands: Vec<Value> = [
            json!({"type":"add_feature","feature":feat,"name":"Box"}),
            json!({"type":"add_feature","feature":{"type":"Sphere","radius":2.0}}),
            json!({"type":"create_feature","kind":"scene-settings","params":{"lights":{"String":"[]"}}}),
            json!({"type":"update_feature","id":"1:0","feature":{"type":"Extrude","sketch":"{}","depth":4.0,"direction":[0.0,0.0,1.0],"twist_angle":5.0}}),
            json!({"type":"set_param","id":"1:0","key":"radius","value":{"F64":2.5}}),
            json!({"type":"set_param","id":"1:0","key":"axis","value":{"Vec3":[0.0,1.0,0.0]}}),
            json!({"type":"set_param","id":"1:0","key":"inputs","value":{"FeatureRefList":["1:1","1:2"]}}),
            json!({"type":"delete_feature","id":"1:0"}),
            json!({"type":"set_translation","id":"1:0","offset":[1.0,2.0,3.0]}),
            json!({"type":"set_rotation","id":"1:0","angles":[0.0,90.0,0.0]}),
            json!({"type":"set_scale","id":"1:0","factor":[2.0,2.0,2.0]}),
            json!({"type":"set_material","id":"1:0","material":"steel"}),
            json!({"type":"define_material","key":"glass","definition":{"name":"glass","color":[0.9,0.95,1.0],"metallic":0.0,"roughness":0.05,"transmission":0.9,"ior":1.5}}),
            json!({"type":"set_visible","id":"1:0","visible":false}),
            json!({"type":"rename","id":"1:0","name":"Base"}),
            json!({"type":"reorder","id":"1:0","index":2}),
            json!({"type":"duplicate","ids":["1:0","1:1"]}),
            json!({"type":"set_joint_state","id":"1:3","state":45.0}),
            json!({"type":"import_ir","document":{"version":"0.1","nodes":{},"materials":{},"part_materials":{},"roots":[]}}),
            json!({"type":"undo"}),
            json!({"type":"redo"}),
            json!({"type":"batch","commands":[{"type":"undo"},{"type":"add_feature","feature":{"type":"Boolean","boolean_type":"union","input_a":"1:0","input_b":"1:1"}}]}),
        ]
        .into_iter()
        // Through the Rust type and back, so the fixture is Rust's own output.
        .map(|v| {
            let cmd: vcad_app::DocCommand = serde_json::from_value(v).unwrap();
            serde_json::to_value(cmd).unwrap()
        })
        .collect();
        let tool: vcad_app::command::ToolCall = serde_json::from_value(json!({
            "tool":"create","args":{"type":"fillet","parent_part_id":"1:0","params":{"radius":1}}
        }))
        .unwrap();
        let fixture = json!({
            "commands": commands,
            "envelopes": envelopes,
            "toolCalls": [serde_json::to_value(tool).unwrap()],
        });
        let text = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apple/VcadApp/Tests/Fixtures/engine-types.json");
        if std::env::var_os("VCAD_UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == text,
            "{} is stale; rerun with VCAD_UPDATE_FIXTURES=1",
            path.display()
        );
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
