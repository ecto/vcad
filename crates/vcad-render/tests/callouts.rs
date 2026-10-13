//! `--callouts`: hole and bolt-circle annotations detected from the BRep.
//!
//! Fixtures are built from loon so the holes arrive as real boolean-cut
//! cylindrical faces, the way a modelled plate does — not as hand-placed
//! IR nodes. (Loon's `[difference A B]` is B − A: cutters first.)

use vcad_render::{
    render_png_str, render_svg_str_opts, RasterOptions, RenderAnnotations, SvgOptions, View,
};

/// 40×40×3 plate: a Ø8 centre bore and 6×Ø3.4 on a Ø27 PCD at 0° phase —
/// the gripper motor-mount plate that motivated the feature.
const PLATE_6X: &str = "[difference [union [circular-pattern 0 0 0 0 0 1 6 360 [translate 13.5 0 -1 [cylinder 1.7 5]]] [translate 0 0 -1 [cylinder 4 5]]] [translate -20 -20 0 [cube 40 40 3]]]";

/// Same plate with two hole sizes and no pattern: 2×Ø3.4 and 2×Ø6.
const PLATE_2SIZES: &str = "[difference [union [union [translate -12 0 -1 [cylinder 1.7 5]] [translate 12 0 -1 [cylinder 1.7 5]]] [union [translate 0 -10 -1 [cylinder 3 5]] [translate 0 10 -1 [cylinder 3 5]]]] [translate -20 -20 0 [cube 40 40 3]]]";

/// The 6× plate with its holes shifted to start at 30°
/// (13.5·cos 30° = 11.6913, 13.5·sin 30° = 6.75).
const PLATE_6X_PHASED: &str = "[difference [circular-pattern 0 0 0 0 0 1 6 360 [translate 11.6913 6.75 -1 [cylinder 1.7 5]]] [translate -20 -20 0 [cube 40 40 3]]]";

/// Two stacked parts sharing one bolt pattern: the plate plus a disc under
/// it with the same 6 holes. The holes must be called out once.
const STACKED: &str = "[root [difference [circular-pattern 0 0 0 0 0 1 6 360 [translate 13.5 0 -1 [cylinder 1.7 5]]] [translate -20 -20 0 [cube 40 40 3]]] \"aluminum\"] [root [difference [circular-pattern 0 0 0 0 0 1 6 360 [translate 13.5 0 -10 [cylinder 1.7 12]]] [translate 0 0 -6 [cylinder 18 6]]] \"steel\"]";

fn doc(loon: &str) -> String {
    let d = vcad_loon::eval_vcad(loon, None).expect("loon eval");
    serde_json::to_string(&d).expect("serialize document")
}

fn callouts(view: View) -> SvgOptions {
    SvgOptions {
        view,
        annotations: RenderAnnotations {
            callouts: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Text content of every `<text>` inside the callouts group.
fn callout_texts(svg: &str) -> Vec<String> {
    let Some(start) = svg.find(r#"<g class="annot-callouts">"#) else {
        return Vec::new();
    };
    let body = &svg[start..];
    let end = body.find("</g>").unwrap_or(body.len());
    let body = &body[..end];
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("<text") {
        let after = &rest[i..];
        let open = after.find('>').unwrap();
        let close = after.find("</text>").unwrap();
        out.push(after[open + 1..close].to_string());
        rest = &after[close..];
    }
    out
}

#[test]
fn six_hole_pcd_collapses_to_one_callout() {
    let svg = render_svg_str_opts(&doc(PLATE_6X), 2.0, &callouts(View::Top)).unwrap();
    let mut texts = callout_texts(&svg);
    texts.sort();
    assert_eq!(texts, vec!["6× Ø3.4 on Ø27 PCD", "Ø8"], "{svg}");
    assert!(
        svg.contains("stroke-dasharray=\"4 2.5\""),
        "pitch circle must be drawn dashed"
    );
    // Pitch circle radius is 13.5 mm × 2 px/mm.
    assert!(svg.contains(r#"r="27.00" fill="none""#), "{svg}");
}

#[test]
fn two_hole_sizes_get_individual_callouts() {
    let svg = render_svg_str_opts(&doc(PLATE_2SIZES), 2.0, &callouts(View::Top)).unwrap();
    let mut texts = callout_texts(&svg);
    texts.sort();
    assert_eq!(texts, vec!["Ø3.4", "Ø3.4", "Ø6", "Ø6"], "{svg}");
    assert!(!svg.contains("stroke-dasharray=\"4 2.5\""));
}

#[test]
fn phased_pattern_reports_first_hole_angle() {
    let svg = render_svg_str_opts(&doc(PLATE_6X_PHASED), 2.0, &callouts(View::Top)).unwrap();
    assert_eq!(
        callout_texts(&svg),
        vec!["6× Ø3.4 on Ø27 PCD @ 30°"],
        "{svg}"
    );
}

#[test]
fn stacked_coaxial_holes_are_called_out_once() {
    let svg = render_svg_str_opts(&doc(STACKED), 2.0, &callouts(View::Top)).unwrap();
    let mut texts = callout_texts(&svg);
    texts.sort();
    // The disc's own turned rim is a Ø36 boss (its axis is along the
    // view too); the twelve hole faces collapse to one pattern callout.
    assert_eq!(texts, vec!["6× Ø3.4 on Ø27 PCD", "Ø36"], "{svg}");
}

#[test]
fn holes_off_axis_to_the_view_are_not_called_out() {
    // Front view looks along Y; the Z-axis holes are edge-on.
    let svg = render_svg_str_opts(&doc(PLATE_6X), 2.0, &callouts(View::Front)).unwrap();
    assert!(callout_texts(&svg).is_empty(), "{svg}");
    assert!(
        svg.contains("annot-callouts"),
        "group is emitted, just empty"
    );
}

#[test]
fn no_cylinders_means_no_callouts_and_no_crash() {
    let svg = render_svg_str_opts(&doc("[sphere 10]"), 2.0, &callouts(View::Top)).unwrap();
    assert!(callout_texts(&svg).is_empty());
}

#[test]
fn default_render_is_unchanged_by_the_flag_existing() {
    let d = doc(PLATE_6X);
    let a = render_svg_str_opts(&d, 2.0, &SvgOptions::default()).unwrap();
    let b = render_svg_str_opts(
        &d,
        2.0,
        &SvgOptions {
            annotations: RenderAnnotations::default(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(a, b);
    assert!(!a.contains("annot-callouts"));
}

#[test]
fn raster_path_draws_callouts() {
    let d = doc(PLATE_6X);
    let render = |callouts| {
        render_png_str(
            &d,
            &RasterOptions {
                view: View::Top,
                size_px: 512,
                annotations: RenderAnnotations {
                    callouts,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap()
    };
    let plain = image::load_from_memory(&render(false)).unwrap().to_rgba8();
    let annotated = image::load_from_memory(&render(true)).unwrap().to_rgba8();
    assert_eq!(plain.dimensions(), annotated.dimensions());
    let changed = plain
        .pixels()
        .zip(annotated.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 200, "callouts drew almost nothing ({changed} px)");
}
