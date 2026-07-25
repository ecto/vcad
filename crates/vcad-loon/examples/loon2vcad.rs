//! Evaluate a `.loon` source file to a `.vcad` document on stdout.
//!
//! Usage: `cargo run -p vcad-loon --example loon2vcad -- input.loon > out.vcad`
//!
//! This is the `.loon` → `.vcad` half of the docs render workflow; the other
//! half is `cargo run -p vcad-render -- out.vcad --jpeg out.jpg`.
//!
//! Note the API pair: [`vcad_loon::eval_vcad`] returns a serde-serializable
//! `Document` (what you want here), while `eval_vcad_to_value` returns a raw
//! loon `Value`, which is *not* `Serialize`.

use std::path::Path;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: loon2vcad <input.loon>");
    let source = std::fs::read_to_string(&path).expect("read source");
    let doc = vcad_loon::eval_vcad(&source, Path::new(&path).parent())
        .unwrap_or_else(|e| panic!("loon eval failed: {e}"));
    println!("{}", serde_json::to_string_pretty(&doc).expect("serialize"));
}
