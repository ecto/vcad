// vcad for Typst — parametric CAD rendered and measured at compile time.
//
// #import "@preview/vcad:0.1.0": vcad-view, vcad-loon, vcad-sheet, vcad-inspect
//
// Every function accepts either .vcad document JSON (str or bytes, e.g.
// `read("part.vcad")`) or, with `format: "loon"`, loon CAD source. Renders
// come back as drafting-style SVG images; `vcad-inspect` returns a
// dictionary of exact model measurements so prose, tables, and assertions
// recompute from the geometry on every compile.

#let _plugin = plugin("vcad.wasm")

#let _to-bytes(source) = {
  if type(source) == bytes { source } else { bytes(source) }
}

#let _cfg(format, pairs) = {
  let d = (:)
  if format != none { d.insert("format", format) }
  for (k, v) in pairs {
    if v != none { d.insert(k, v) }
  }
  bytes(json.encode(d))
}

/// Render a model to a single drafting-style view.
///
/// - source: .vcad JSON (str/bytes) or loon source with `format: "loon"`
/// - view: "iso" | "front" | "side" | "top" | "hero" | "orbit:AZ,EL"
/// - scale: px per mm
/// - section: cutaway plane, "x=N" | "y=N" | "z=N"
/// - dims / labels / axes: engineering annotation overlays
/// - focus: frame on a named part
/// - ..image-args: forwarded to `image` (width, height, alt, ...)
#let vcad-view(
  source,
  format: none,
  view: "iso",
  scale: none,
  section: none,
  dims: false,
  labels: false,
  axes: false,
  focus: none,
  transparent: true,
  ..image-args,
) = {
  let cfg = _cfg(format, (
    ("view", view),
    ("scale", scale),
    ("section", section),
    ("dims", if dims { true } else { none }),
    ("labels", if labels { true } else { none }),
    ("axes", if axes { true } else { none }),
    ("focus", focus),
    ("transparent", if transparent { true } else { none }),
  ))
  image(_plugin.render(cfg, _to-bytes(source)), format: "svg", ..image-args)
}

/// Render inline loon source. Sugar for `vcad-view(src, format: "loon")`;
/// also accepts a raw block: `vcad-loon(```[cube 40 20 10]```)`.
#let vcad-loon(source, ..args) = {
  let src = if type(source) == content and source.has("text") { source.text } else { source }
  vcad-view(src, format: "loon", ..args)
}

/// Render a third-angle multi-view drawing sheet (front/side/top/iso with
/// title block).
#let vcad-sheet(source, format: none, title: none, width: none, ..image-args) = {
  let cfg = _cfg(format, (("title", title), ("sheet-width", width)))
  image(_plugin.sheet(cfg, _to-bytes(source)), format: "svg", ..image-args)
}

/// Measure a model. Returns a dictionary:
/// (volume, area, bbox: (min, max, size), center-of-mass, parts: (..))
/// Volumes are mm³, areas mm², lengths mm.
#let vcad-inspect(source, format: none) = {
  json(_plugin.inspect(_cfg(format, ()), _to-bytes(source)))
}

/// Mass in grams for a measured model: `vcad-mass(vcad-inspect(..), 1.04)`
/// (density in g/cm³).
#let vcad-mass(inspection, density) = inspection.volume / 1000 * density

/// Compile-time engineering assertion. Fails the document build with
/// `message` when `condition` is false — a spec printed by the document
/// cannot ship violated.
#let vcad-assert(condition, message: "vcad assertion failed") = {
  assert(condition, message: message)
}

/// Evaluate loon source to a .vcad document JSON string (chain into
/// `vcad-view`/`vcad-inspect`, or `json.decode` it for raw access).
#let vcad-eval-loon(source) = {
  str(_plugin.eval_loon(_to-bytes(source)))
}

/// Plugin version string.
#let vcad-version() = str(_plugin.version())
