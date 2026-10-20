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
