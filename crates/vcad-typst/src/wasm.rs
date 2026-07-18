//! wasm-minimal-protocol exports — the surface Typst's `plugin()` sees.
//!
//! Every export takes UTF-8 byte buffers and returns UTF-8 bytes (SVG or
//! JSON); errors become protocol-level errors that Typst reports at the
//! call site.

use wasm_minimal_protocol::{initiate_protocol, wasm_func};

initiate_protocol!();

fn utf8(bytes: &[u8], what: &str) -> Result<String, String> {
    String::from_utf8(bytes.to_vec()).map_err(|_| format!("{what} is not valid UTF-8"))
}

#[wasm_func]
fn render(config: &[u8], source: &[u8]) -> Result<Vec<u8>, String> {
    crate::render(&utf8(config, "config")?, &utf8(source, "source")?).map(String::into_bytes)
}

#[wasm_func]
fn sheet(config: &[u8], source: &[u8]) -> Result<Vec<u8>, String> {
    crate::sheet(&utf8(config, "config")?, &utf8(source, "source")?).map(String::into_bytes)
}

#[wasm_func]
fn inspect(config: &[u8], source: &[u8]) -> Result<Vec<u8>, String> {
    crate::inspect(&utf8(config, "config")?, &utf8(source, "source")?).map(String::into_bytes)
}

#[wasm_func]
fn eval_loon(source: &[u8]) -> Result<Vec<u8>, String> {
    crate::eval_loon(&utf8(source, "source")?).map(String::into_bytes)
}

#[wasm_func]
fn version() -> Result<Vec<u8>, String> {
    Ok(crate::version().into_bytes())
}
