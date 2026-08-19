//! Built-in material library: PBR values for well-known material names.
//!
//! Hardware documents overwhelmingly name a material rather than define one.
//! A `.loon` file says `[root frame-rails "aluminum"]` and stops there — the
//! name *is* the spec, the same way a drawing note saying "6061-T6" is. The
//! document's `materials` table stays empty, so before this module every one
//! of those parts resolved to `None` and the path tracer fell back to its
//! default clay grey: an entire machine rendered as one lump of putty.
//!
//! This is a **render-time fallback**, deliberately not a parser-time
//! rewrite. The IR keeps saying exactly what the author wrote — a name, no
//! def — and the renderer supplies a plausible appearance for names it
//! recognises. A document that *does* declare `[material aluminum ...]`
//! always wins; nothing here can override authored intent.
//!
//! Both render paths resolve through [`resolve`], so the drafting fills and
//! the photoreal shading agree on hue by construction.
//!
//! Values follow the app's material picker (`packages/app/src/data/
//! materials.ts`) in hue and intent, with base colours for the metals moved
//! to measured normal-incidence reflectance — under a path tracer, F0 is what
//! the number means, and the picker's swatch values render chalky.

use vcad_ir::MaterialDef;

/// Normalise a material key for lookup: case-insensitive, and `-`, `_` and
/// spaces are all the same separator, so `abs-black`, `abs_black` and
/// `ABS Black` are one material.
fn normalize(name: &str) -> String {
    name.chars()
        .filter_map(|c| match c {
            '-' | '_' | ' ' => Some('-'),
            c if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// `(key, color, metallic, roughness)`.
type Entry = (&'static str, [f64; 3], f64, f64);

/// The library. Metals carry measured-ish F0 reflectance; dielectrics carry
/// albedo. Roughness is perceptual (squared to GGX alpha downstream).
const LIBRARY: &[Entry] = &[
    // ── metals ───────────────────────────────────────────────────────────
    // Bare aluminium's true F0 is ~0.91; a machined part at that value blows
    // out to white paper in a bright studio and stops reading as metal at
    // all. Pulled down and roughened until the form shading survives.
    ("aluminum", [0.86, 0.87, 0.88], 1.0, 0.40),
    ("steel", [0.68, 0.69, 0.71], 1.0, 0.30),
    ("stainless", [0.78, 0.79, 0.80], 1.0, 0.22),
    ("chrome", [0.95, 0.95, 0.95], 1.0, 0.08),
    ("copper", [0.95, 0.54, 0.33], 1.0, 0.25),
    ("brass", [0.89, 0.75, 0.39], 1.0, 0.30),
    ("bronze", [0.80, 0.57, 0.34], 1.0, 0.35),
    ("gold", [1.00, 0.77, 0.34], 1.0, 0.18),
    ("silver", [0.97, 0.96, 0.92], 1.0, 0.12),
    ("titanium", [0.62, 0.60, 0.57], 1.0, 0.42),
    ("nickel", [0.66, 0.61, 0.53], 1.0, 0.22),
    // ── plastics ─────────────────────────────────────────────────────────
    // Injection-moulded and printed plastics are never as dark or as light
    // as their nominal colour chip: black ABS reflects a few percent, white
    // reflects ~85%. Pushing to 0/1 is the tell of an untuned render.
    ("abs-black", [0.03, 0.03, 0.03], 0.0, 0.50),
    ("abs-white", [0.85, 0.85, 0.84], 0.0, 0.45),
    ("abs-grey", [0.30, 0.30, 0.31], 0.0, 0.50),
    ("abs-red", [0.55, 0.06, 0.04], 0.0, 0.50),
    ("abs-blue", [0.05, 0.12, 0.45], 0.0, 0.50),
    ("abs-green", [0.07, 0.32, 0.12], 0.0, 0.50),
    ("abs-yellow", [0.70, 0.52, 0.04], 0.0, 0.50),
    ("abs-orange", [0.72, 0.26, 0.03], 0.0, 0.50),
    ("pla", [0.72, 0.72, 0.68], 0.0, 0.45),
    ("petg", [0.60, 0.68, 0.72], 0.0, 0.35),
    ("nylon", [0.78, 0.76, 0.70], 0.0, 0.55),
    ("delrin", [0.80, 0.80, 0.79], 0.0, 0.40),
    ("pom", [0.80, 0.80, 0.79], 0.0, 0.40),
    ("resin", [0.48, 0.44, 0.40], 0.0, 0.20),
    ("acrylic", [0.85, 0.85, 0.88], 0.0, 0.10),
    ("rubber", [0.05, 0.05, 0.05], 0.0, 0.80),
    // ── composites & other ───────────────────────────────────────────────
    ("carbon-fiber", [0.05, 0.05, 0.06], 0.2, 0.30),
    ("fiberglass", [0.72, 0.72, 0.62], 0.0, 0.40),
    ("kevlar", [0.58, 0.53, 0.20], 0.0, 0.60),
    ("fr4", [0.05, 0.35, 0.18], 0.0, 0.80),
    ("concrete", [0.42, 0.42, 0.40], 0.0, 0.85),
    ("ceramic", [0.85, 0.83, 0.80], 0.0, 0.25),
    ("foam", [0.20, 0.20, 0.23], 0.0, 0.95),
    ("glass", [0.85, 0.88, 0.90], 0.0, 0.05),
    // ── organic ──────────────────────────────────────────────────────────
    ("oak", [0.42, 0.30, 0.18], 0.0, 0.70),
    ("walnut", [0.20, 0.13, 0.09], 0.0, 0.65),
    ("leather", [0.22, 0.13, 0.08], 0.0, 0.75),
    ("cork", [0.52, 0.37, 0.24], 0.0, 0.90),
    ("bamboo", [0.66, 0.54, 0.33], 0.0, 0.60),
    // ── shorthands the hardware repos actually use ───────────────────────
    // `actuator` is a whole class of part (servo/gearmotor housing) rather
    // than a substance, but it is written as a material and it has a
    // consistent look: dark filled nylon, matte, slightly warm.
    ("actuator", [0.10, 0.10, 0.11], 0.0, 0.55),
    ("motor", [0.10, 0.10, 0.11], 0.0, 0.55),
    ("plastic", [0.55, 0.55, 0.55], 0.0, 0.50),
    ("metal", [0.85, 0.86, 0.87], 1.0, 0.35),
    ("default", [0.62, 0.64, 0.67], 0.0, 0.40),
];

/// PBR values for a well-known material name, or `None` if unrecognised.
///
/// Case- and separator-insensitive: `abs-black`, `abs_black` and `ABS Black`
/// all resolve to the same definition. The returned [`MaterialDef`] carries
/// the caller's spelling as its `name`, so downstream finish heuristics
/// (`brushed_aluminum`, `turned_shaft`) still see what the author wrote.
pub fn builtin(name: &str) -> Option<MaterialDef> {
    let key = normalize(name);
    let (_, color, metallic, roughness) = LIBRARY.iter().find(|(k, ..)| *k == key)?;
    Some(MaterialDef {
        name: name.to_string(),
        color: *color,
        metallic: *metallic,
        roughness: *roughness,
        ..Default::default()
    })
}

/// Resolve a part's material: the document's own definition if it has one,
/// otherwise the built-in for that name.
///
/// Authored definitions always win — a document that declares `aluminum` as
/// hot pink renders hot pink.
pub fn resolve(
    materials: &std::collections::HashMap<String, MaterialDef>,
    name: &str,
) -> Option<MaterialDef> {
    materials.get(name).cloned().or_else(|| builtin(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn separator_and_case_insensitive() {
        let a = builtin("abs-black").expect("abs-black");
        for spelling in ["abs_black", "ABS-BLACK", "Abs-Black", "abs black"] {
            let b = builtin(spelling).unwrap_or_else(|| panic!("{spelling} should resolve"));
            assert_eq!(a.color, b.color, "{spelling} disagrees with abs-black");
            assert_eq!(b.name, spelling, "the caller's spelling should survive");
        }
    }

    #[test]
    fn metals_are_metallic_and_plastics_are_not() {
        for m in ["aluminum", "steel", "copper", "brass", "chrome"] {
            assert_eq!(builtin(m).expect(m).metallic, 1.0, "{m} should be metal");
        }
        for m in ["abs-black", "abs-white", "abs-red", "abs-blue", "actuator"] {
            assert_eq!(
                builtin(m).expect(m).metallic,
                0.0,
                "{m} should be dielectric"
            );
        }
    }

    #[test]
    fn copper_reads_warm() {
        let c = builtin("copper").expect("copper");
        assert!(
            c.color[0] > c.color[1] && c.color[1] > c.color[2],
            "copper should fall off r > g > b, got {:?}",
            c.color
        );
    }

    #[test]
    fn unknown_names_do_not_resolve() {
        assert!(builtin("unobtainium").is_none());
        assert!(builtin("").is_none());
    }

    #[test]
    fn authored_definitions_beat_builtins() {
        let mut materials = HashMap::new();
        materials.insert(
            "copper".to_string(),
            MaterialDef {
                name: "copper".to_string(),
                color: [1.0, 0.0, 1.0],
                metallic: 0.0,
                roughness: 0.9,
                ..Default::default()
            },
        );
        let resolved = resolve(&materials, "copper").expect("copper");
        assert_eq!(resolved.color, [1.0, 0.0, 1.0], "authored def must win");

        // …and a name the document says nothing about still gets the builtin.
        let fallback = resolve(&materials, "brass").expect("brass");
        assert_eq!(fallback.color, builtin("brass").unwrap().color);
    }

    #[test]
    fn every_key_is_already_normalized_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (k, ..) in LIBRARY {
            assert_eq!(normalize(k), *k, "library key {k} is not in normal form");
            assert!(seen.insert(*k), "duplicate library key {k}");
        }
    }
}
