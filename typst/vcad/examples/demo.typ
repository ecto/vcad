#import "../lib.typ": *

#set page(width: 190mm, height: auto, margin: 12mm)
#set text(font: "Helvetica", size: 10pt)

#let fmt(x, digits: 1) = str(calc.round(x, digits: digits))

// One parametric model, defined inline. Every render, number, and
// assertion below recomputes from this source on each compile.
// loon booleans are subject-last: [difference tool body].
#let bracket(w) = "
[difference
  [translate " + str(w / 2) + " 20 -1 [cylinder 4 12]]
  [union
    [cube " + str(w) + " 40 8]
    [cube 8 40 30]]]
"

= vcad in Typst: the document is the model

== See
#grid(columns: (1fr, 1fr), gutter: 4mm,
  vcad-loon(bracket(60), dims: true),
  vcad-loon(bracket(60), view: "orbit:150,25", section: "y=20"),
)

== Measure
#let m = vcad-inspect(bracket(60), format: "loon")
#let mass = vcad-mass(m, 1.04) // ABS, g/cm³
The bracket displaces #fmt(m.volume / 1000, digits: 2) cm³,
spans #fmt(m.bbox.size.at(0)) × #fmt(m.bbox.size.at(1)) ×
#fmt(m.bbox.size.at(2)) mm, and weighs *#fmt(mass) g* in ABS.
Its center of mass sits at
(#m.center-of-mass.map(c => fmt(c)).join(", ")) mm.

== Parametric variants
#table(
  columns: 4,
  table.header([*w (mm)*], [*view*], [*volume (cm³)*], [*mass, ABS (g)*]),
  ..for w in (40, 60, 80) {
    let mi = vcad-inspect(bracket(w), format: "loon")
    (
      str(w),
      vcad-loon(bracket(w), height: 18mm),
      fmt(mi.volume / 1000, digits: 2),
      fmt(vcad-mass(mi, 1.04)),
    )
  },
)

== Prove
// The compile fails if the model stops satisfying the printed spec.
#vcad-assert(mass < 30, message: "bracket over mass budget (30 g)")
#vcad-assert(calc.abs(m.bbox.size.at(2) - 30) < 0.01, message: "overall height drifted")
All printed values are recomputed from the model at compile time —
this PDF cannot disagree with its geometry. #text(fill: green)[✓ verified]

== Drawing sheet
#vcad-sheet(bracket(60), format: "loon", title: "BRACKET-60")
