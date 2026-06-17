//! Regression: rendering a disc built entirely from cylinders (center bore +
//! circular-patterned mounting holes) must not panic.
//!
//! Every cylinder reaches the boolean splitter with the IR "auto" segment
//! sentinel (`segments == 0`). With no box in the document to bump the
//! boolean's `max(self, other)` to 32, the splitter once built a 0-vertex
//! circle loop and panicked in `Topology::add_loop`. On the wasm32 kernel
//! that panic compiles to an `unreachable` trap, which the MCP render_view
//! tool surfaced as a kernel crash. See `Solid::cylinder` /
//! `vcad_kernel::resolve_segments`.
//!
//! This is the exact document the bug report used.
const DISC: &str = "[difference [union [translate 0 0 -0.5 [cylinder 4 2.6]] [circular-pattern 0 0 0 0 0 1 3 360 [translate 25 0 -0.5 [cylinder 1.6 2.6]]]] [cylinder 30 1.6]]";

#[test]
fn renders_disc_with_cutouts() {
    let doc = vcad_loon::eval_vcad(DISC, None).expect("loon eval");
    let json = serde_json::to_string(&doc).expect("serialize document");

    // The render path tessellates the booleaned solid — the step that
    // previously trapped. It must return a real SVG, not panic or bail empty.
    let svg = vcad_render::render_svg_str(&json, 2.0).expect("render must succeed");
    assert!(
        svg.contains("<svg"),
        "expected an SVG document, got {svg:.80}"
    );
    assert!(
        svg.len() > 1_000,
        "expected non-trivial geometry, got {} bytes",
        svg.len()
    );
}
