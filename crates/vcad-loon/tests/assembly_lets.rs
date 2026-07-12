//! Assembly built from let-bound solids must produce part defs + instances.

const SRC: &str = r#"
[let base [cube 120 70 3]]
[let cell [rotate 0 90 0 [cylinder 9.25 65]]]
[let tec [cube 40 40 3.8]]
[assembly
  #[[part "base" base "aluminum"]
    [part "cell" cell "chrome"]
    [part "tec" tec "ceramic"]]
  #[[instance "base1" "base" 0 0 0]
    [instance "cell1" "cell" 4.5 26 17]
    [instance "tec1" "tec" 70 15 6]]
  #[]
  "base1"]
"#;

#[test]
fn assembly_with_let_bindings_produces_parts() {
    let doc = vcad_loon::eval_vcad(SRC, None).expect("eval failed");
    let parts = doc.part_defs.as_ref().map(|p| p.len()).unwrap_or(0);
    let insts = doc.instances.as_ref().map(|i| i.len()).unwrap_or(0);
    assert_eq!(
        parts,
        3,
        "expected 3 part defs, doc: roots={}",
        doc.roots.len()
    );
    assert_eq!(insts, 3);
    assert_eq!(doc.ground_instance_id.as_deref(), Some("base1"));
}
