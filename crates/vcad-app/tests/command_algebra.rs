//! Property tests for the `DocCommand` algebra: random command sequences
//! (seeded, reproducible — no extra dependencies) checked against the
//! invariants every host relies on.
//!
//! Set `VCAD_ALGEBRA_CASES` to run more sequences (CI nightly can raise it).

use serde_json::{json, Value};
use vcad_app::DocumentApi;
use vcad_crdt::ReplicaId;

/// xorshift64* — tiny, deterministic, good enough to explore sequences.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.below(10_000) as f64 / 10_000.0) * (hi - lo)
    }
}

fn cases() -> u64 {
    std::env::var("VCAD_ALGEBRA_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200)
}

/// Stable ids that currently exist, plus one that never will.
fn ids(api: &DocumentApi) -> Vec<String> {
    let mut v: Vec<String> = api
        .get_state()
        .parts
        .iter()
        .map(|p| p.id().to_string())
        .collect();
    v.push("999:999".into());
    v
}

/// One random step: `(is_tool, name_or_command_json, args_json)`.
fn step(rng: &mut Rng, api: &DocumentApi) -> (bool, String, String) {
    let ids = ids(api);
    let id = || ids[0].clone();
    let pick = |rng: &mut Rng| ids[rng.below(ids.len() as u64) as usize].clone();
    let v3 = |rng: &mut Rng| json!({"x": rng.f(-20.0, 20.0), "y": rng.f(-20.0, 20.0), "z": rng.f(-20.0, 20.0)});
    let _ = id;
    match rng.below(14) {
        0 => (true, "create".into(), json!({"type": "cube", "name": format!("C{}", rng.below(9)), "params": {"size": {"x": rng.f(1.0, 9.0), "y": rng.f(1.0, 9.0), "z": rng.f(1.0, 9.0)}}}).to_string()),
        1 => (true, "create".into(), json!({"type": "cylinder", "params": {"radius": rng.f(0.5, 5.0), "height": rng.f(1.0, 9.0)}}).to_string()),
        2 => (true, "create".into(), json!({"type": "sphere", "params": {"radius": rng.f(0.5, 5.0)}}).to_string()),
        3 => (true, "create".into(), json!({"type": "translate", "params": {"child": pick(rng), "offset": v3(rng)}}).to_string()),
        4 => (true, "create".into(), json!({"type": "rotate", "params": {"child": pick(rng), "angles": v3(rng)}}).to_string()),
        5 => (true, "create".into(), json!({"type": "difference", "params": {"left": pick(rng), "right": pick(rng)}}).to_string()),
        6 => (true, "create".into(), json!({"type": "fillet", "parent_part_id": pick(rng), "params": {"radius": rng.f(-1.0, 2.0)}}).to_string()),
        7 => (true, "set_material".into(), json!({"part_id": pick(rng), "material": "steel"}).to_string()),
        8 => (true, "delete".into(), json!({"part_id": pick(rng)}).to_string()),
        9 => (false, json!({"type": "rename", "id": pick(rng), "name": "R"}).to_string(), String::new()),
        10 => (false, json!({"type": "set_visible", "id": pick(rng), "visible": rng.below(2) == 0}).to_string(), String::new()),
        11 => (false, json!({"type": "batch", "commands": [
            {"type": "set_material", "id": pick(rng), "material": "brass"},
            {"type": "set_scale", "id": pick(rng), "factor": [2, 2, 2]}
        ]}).to_string(), String::new()),
        12 => (false, json!({"type": "reorder", "id": pick(rng), "index": rng.below(4)}).to_string(), String::new()),
        _ => (true, "linear_pattern".into(), json!({"child": pick(rng), "direction": {"x": 1, "y": 0, "z": 0}, "count": rng.below(4), "spacing": 5}).to_string()),
    }
}

fn run(api: &mut DocumentApi, s: &(bool, String, String)) -> Value {
    let raw = if s.0 {
        api.apply_tool_envelope(&s.1, &s.2)
    } else {
        api.apply_json_envelope(&s.1)
    };
    serde_json::from_str(&raw).unwrap()
}

/// A refused command never changes state; a successful one is reverted by a
/// single undo, and the undo by a single redo.
#[test]
fn refusals_are_inert_and_one_command_is_one_undo_step() {
    for seed in 1..=cases() {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut api = DocumentApi::new(ReplicaId(1));
        for i in 0..12 {
            let s = step(&mut rng, &api);
            let before = api.state_hash();
            let out = run(&mut api, &s);
            if out["ok"] != true {
                assert_eq!(
                    api.state_hash(),
                    before,
                    "seed {seed} step {i}: refused {s:?} changed state: {out}"
                );
                continue;
            }
            let after = api.state_hash();
            if after == before {
                continue;
            }
            assert_eq!(
                run(
                    &mut api,
                    &(false, r#"{"type":"undo"}"#.into(), String::new())
                )["ok"],
                true
            );
            assert_eq!(
                api.state_hash(),
                before,
                "seed {seed} step {i}: one undo didn't revert {s:?}"
            );
            assert_eq!(
                run(
                    &mut api,
                    &(false, r#"{"type":"redo"}"#.into(), String::new())
                )["ok"],
                true
            );
            assert_eq!(
                api.state_hash(),
                after,
                "seed {seed} step {i}: redo didn't restore {s:?}"
            );
        }
    }
}

/// Two replicas editing independently converge to the same state no
/// matter which direction their operations are exchanged in first.
#[test]
fn replicas_converge_in_any_merge_order() {
    for seed in 1..=cases() / 4 {
        let mut rng = Rng(seed.wrapping_mul(0xd1b5_4a32_d192_ed03) | 1);
        let mut a = DocumentApi::new(ReplicaId(1));
        let mut b = DocumentApi::new(ReplicaId(2));
        for _ in 0..6 {
            let s = step(&mut rng, &a);
            run(&mut a, &s);
            let s = step(&mut rng, &b);
            run(&mut b, &s);
        }
        let empty = Default::default();
        let ops_a = a.crdt().ops_since(&empty);
        let ops_b = b.crdt().ops_since(&empty);
        a.crdt_mut().merge(ops_b);
        b.crdt_mut().merge(ops_a);
        a.rebuild_stable_ids();
        b.rebuild_stable_ids();
        assert_eq!(
            a.state_hash(),
            b.state_hash(),
            "seed {seed}: replicas diverged"
        );
    }
}
