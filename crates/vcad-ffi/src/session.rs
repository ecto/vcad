//! Incremental evaluation sessions over the C ABI.
//!
//! A host keeps one `VcadEvalSession` per open document and evaluates every
//! edit of that document through it (`vcad_scene_from_json_session`). The
//! session carries a [`vcad_eval::incremental::IncrementalCache`], so each
//! evaluation re-evaluates only the nodes the edit touched (and what depends
//! on them) and re-tessellates only the roots that changed. Results equal
//! [`crate::vcad_scene_from_json_in`].

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use vcad_eval::evaluate::evaluate_document_incremental;
use vcad_eval::incremental::IncrementalCache;
use vcad_eval::EvalOptions;
use vcad_ir::Document;

use crate::{err, VcadScene};

/// Opaque incremental-evaluation session (one per open document).
pub struct VcadEvalSession {
    cache: IncrementalCache,
}

/// Create a session. Free with `vcad_eval_session_free`.
#[no_mangle]
pub extern "C" fn vcad_eval_session_new() -> *mut VcadEvalSession {
    Box::into_raw(Box::new(VcadEvalSession {
        cache: IncrementalCache::new(),
    }))
}

/// Free a session from `vcad_eval_session_new`.
#[no_mangle]
pub extern "C" fn vcad_eval_session_free(session: *mut VcadEvalSession) {
    if !session.is_null() {
        unsafe { drop(Box::from_raw(session)) };
    }
}

/// Drop everything the session carries (the next evaluation is cold).
#[no_mangle]
pub extern "C" fn vcad_eval_session_clear(session: *mut VcadEvalSession) {
    if let Some(s) = unsafe { session.as_mut() } {
        s.cache.clear();
    }
}

/// What the session's last evaluation reused: solids carried over, solids
/// evaluated, and root meshes reused. Any out-pointer may be null.
#[no_mangle]
pub extern "C" fn vcad_eval_session_stats(
    session: *const VcadEvalSession,
    reused_solids: *mut usize,
    evaluated_solids: *mut usize,
    reused_meshes: *mut usize,
) {
    let Some(s) = (unsafe { session.as_ref() }) else {
        return;
    };
    let st = s.cache.stats();
    unsafe {
        if !reused_solids.is_null() {
            *reused_solids = st.reused_solids;
        }
        if !evaluated_solids.is_null() {
            *evaluated_solids = st.evaluated_solids;
        }
        if !reused_meshes.is_null() {
            *reused_meshes = st.reused_meshes;
        }
    }
}

/// [`crate::vcad_scene_from_json_in`] through `session`: same scene, but
/// only what changed since the session's previous evaluation is redone.
/// Returns null (see `vcad_last_error`) on a null argument, bad UTF-8, a
/// parse or evaluation error, or a panic — the session survives all of them.
#[no_mangle]
pub extern "C" fn vcad_scene_from_json_session(
    session: *mut VcadEvalSession,
    json: *const u8,
    json_len: usize,
    base_dir: *const u8,
    base_dir_len: usize,
) -> *mut VcadScene {
    if session.is_null() || json.is_null() {
        err::set_error("vcad_scene_from_json_session: null argument");
        return ptr::null_mut();
    }
    err::clear_error();
    let session = unsafe { &mut *session };
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
        let Ok(text) = std::str::from_utf8(bytes) else {
            err::set_error("document is not UTF-8");
            return ptr::null_mut();
        };
        let mut doc = match Document::from_json(text) {
            Ok(d) => d,
            Err(e) => {
                err::set_error(format!("parse: {e}"));
                return ptr::null_mut();
            }
        };
        if !base_dir.is_null() && base_dir_len > 0 {
            let db = unsafe { std::slice::from_raw_parts(base_dir, base_dir_len) };
            if let Ok(dir) = std::str::from_utf8(db) {
                vcad_eval::resolve_mesh_paths(&mut doc, std::path::Path::new(dir));
            }
        }
        match evaluate_document_incremental(&doc, &EvalOptions::default(), &mut session.cache) {
            Ok(scene) => Box::into_raw(Box::new(VcadScene {
                inner: scene,
                doc: Some(doc),
            })),
            Err(e) => {
                err::set_error(format!("evaluate: {e}"));
                ptr::null_mut()
            }
        }
    }))
    .unwrap_or_else(|_| {
        // A panic mid-evaluation can leave the cache half-updated; start over.
        session.cache.clear();
        err::set_error("vcad_scene_from_json_session panicked");
        ptr::null_mut()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_json(scene: *mut VcadScene) -> String {
        let s = unsafe { &*scene };
        let out = serde_json::to_string(&s.inner.parts.iter().map(|p| &p.mesh).collect::<Vec<_>>())
            .unwrap();
        crate::vcad_scene_free(scene);
        out
    }

    #[test]
    fn session_matches_a_cold_scene_and_reuses_work() {
        let doc = |x: f64| {
            format!(
                r#"{{"version":"0.1","nodes":{{
                "1":{{"id":1,"op":{{"type":"Cube","size":{{"x":10,"y":10,"z":10}}}}}},
                "2":{{"id":2,"op":{{"type":"Cylinder","radius":3,"height":12,"segments":32}}}},
                "3":{{"id":3,"op":{{"type":"Translate","child":2,"offset":{{"x":{x},"y":0,"z":0}}}}}},
                "4":{{"id":4,"op":{{"type":"Difference","left":1,"right":3}}}},
                "5":{{"id":5,"op":{{"type":"Sphere","radius":4,"segments":24}}}},
                "6":{{"id":6,"op":{{"type":"Translate","child":5,"offset":{{"x":30,"y":0,"z":0}}}}}}}},
                "materials":{{}},"part_materials":{{}},
                "roots":[{{"root":4,"material":"a"}},{{"root":6,"material":"b"}}]}}"#
            )
        };
        let session = vcad_eval_session_new();
        for x in [5.0, 6.0, 6.0] {
            let json = doc(x);
            let warm =
                vcad_scene_from_json_session(session, json.as_ptr(), json.len(), ptr::null(), 0);
            let cold = crate::vcad_scene_from_json(json.as_ptr(), json.len());
            assert!(!warm.is_null() && !cold.is_null());
            assert_eq!(scene_json(warm), scene_json(cold));
        }
        let (mut reused, mut evaluated, mut meshes) = (0, 0, 0);
        vcad_eval_session_stats(session, &mut reused, &mut evaluated, &mut meshes);
        // Third pass: identical document, nothing re-evaluated.
        assert_eq!(evaluated, 0);
        assert_eq!(meshes, 2);
        assert!(reused > 0);
        let bad = b"{";
        assert!(vcad_scene_from_json_session(session, bad.as_ptr(), 1, ptr::null(), 0).is_null());
        vcad_eval_session_free(session);
    }
}
