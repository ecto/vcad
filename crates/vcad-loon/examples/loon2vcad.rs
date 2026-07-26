//! Evaluate a `.loon` source file to a `.vcad` document on stdout.
//!
//! Usage: `cargo run -p vcad-loon --example loon2vcad -- input.loon > out.vcad`
//!
//! `vcad-render` and the `vcad` CLI both read `.loon` directly, so this is for
//! the cases that want the IR itself: diffing, piping into other tools, CI.
//! Errors go to stderr with a nonzero exit so it composes in a pipeline.
//!
//! Note the API pair: [`vcad_loon::eval_vcad`] returns a serde-serializable
//! `Document` (what you want here), while `eval_vcad_to_value` returns a raw
//! loon `Value`, which is *not* `Serialize`.

use std::path::PathBuf;

fn main() {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: loon2vcad <input.loon>");
        std::process::exit(2);
    };
    let doc = vcad_loon::eval_vcad_file(&path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    match serde_json::to_string_pretty(&doc) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("serialize {}: {e}", path.display());
            std::process::exit(1);
        }
    }
}
