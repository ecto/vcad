//! A gizmo scrub through the Mac app's FFI evaluation path, cold vs session.
//!
//! `cargo run --release -p vcad-ffi --example scrub_bench -- doc.vcad [frames] [part-index]`
//!
//! Each frame moves one part a little (an engine `set_translation`, as the
//! gizmo does) and evaluates the materialized document twice: through
//! `vcad_scene_from_json` (what every frame cost before) and through one
//! `VcadEvalSession` (what the app does now). Prints per-frame medians.

use std::ffi::CString;
use std::time::{Duration, Instant};

use vcad_ffi::engine::*;
use vcad_ffi::session::*;
use vcad_ffi::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: scrub_bench <doc.vcad> [frames] [part]");
    let frames: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    let ir = CString::new(std::fs::read_to_string(&path).expect("read")).unwrap();

    let engine = vcad_engine_new(1);
    let state = take(vcad_engine_load_ir(engine, ir.as_ptr()));
    let parts = state["result"]["parts"].as_array().unwrap().clone();
    let pick: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(parts.len() / 2);
    let id = parts[pick]["id"].as_str().unwrap().to_string();

    let session = vcad_eval_session_new();
    let doc = state["result"]["document"].to_string();
    let t = Instant::now();
    vcad_scene_free(vcad_scene_from_json_session(
        session,
        doc.as_ptr(),
        doc.len(),
        std::ptr::null(),
        0,
    ));
    println!(
        "parts {} | open (cold, priming the session): {:?}",
        parts.len(),
        t.elapsed()
    );

    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for f in 0..frames {
        let cmd = CString::new(format!(
            r#"{{"type":"set_translation","id":"{id}","offset":[{},0,0]}}"#,
            f as f64 * 0.5
        ))
        .unwrap();
        let out = take(vcad_engine_apply(engine, cmd.as_ptr()));
        let doc = out["result"]["document"].to_string();
        let t = Instant::now();
        let a = vcad_scene_from_json(doc.as_ptr(), doc.len());
        cold.push(t.elapsed());
        let t = Instant::now();
        let b = vcad_scene_from_json_session(session, doc.as_ptr(), doc.len(), std::ptr::null(), 0);
        warm.push(t.elapsed());
        assert!(!a.is_null() && !b.is_null());
        assert_eq!(vcad_scene_part_count(a), vcad_scene_part_count(b));
        vcad_scene_free(a);
        vcad_scene_free(b);
    }
    let (mut r, mut e, mut m) = (0, 0, 0);
    vcad_eval_session_stats(session, &mut r, &mut e, &mut m);
    let (c, w) = (median(&mut cold), median(&mut warm));
    println!(
        "scrub, {frames} frames: cold median {c:?}/frame | session median {w:?}/frame ({:.1}x) | last frame reused {r} solids, evaluated {e}, reused {m} meshes",
        c.as_secs_f64() / w.as_secs_f64().max(1e-9)
    );
    vcad_eval_session_free(session);
    vcad_engine_free(engine);
}

fn take(raw: *mut std::os::raw::c_char) -> serde_json::Value {
    assert!(!raw.is_null(), "engine call failed");
    let s = unsafe { std::ffi::CStr::from_ptr(raw) }
        .to_str()
        .unwrap()
        .to_owned();
    vcad_engine_string_free(raw);
    serde_json::from_str(&s).unwrap()
}

fn median(v: &mut [Duration]) -> Duration {
    v.sort();
    v[v.len() / 2]
}
