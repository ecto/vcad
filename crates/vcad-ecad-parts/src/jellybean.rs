//! Curated **jellybean** parts: named ICs whose pin definitions are universal
//! across designs (NE555, op-amps, logic gates, MCUs, regulators, ...).
//!
//! Unlike the parametric passive families in [`crate::catalog`] — where a part
//! is a *formula* (`family + value + package`) generating infinite values — a
//! jellybean *is* a hand-curated row. That is the right model for an IC: its
//! pinout is a published fact, not something you can derive. Resolving
//! `("NE555", "SOIC-8")` returns the eight pins every NE555 design shares, so a
//! caller (especially an LLM agent) no longer hand-types pin numbers and types.
//!
//! The database is compiled in from `lib/parts/jellybeans.json`, so the catalog
//! is fully offline. Resolution is pure lookup plus schematic-symbol
//! auto-layout — pins are stored in pin-number order and laid out in the
//! classic two-column (DIP) arrangement on demand.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Raw data-pack shapes (as authored in lib/parts/jellybeans.json)
// ---------------------------------------------------------------------------

/// A pin entry exactly as authored: number, name, and `PinType` variant name.
#[derive(Debug, Clone, Deserialize)]
struct RawPin {
    number: String,
    name: String,
    #[serde(rename = "type")]
    pin_type: String,
}

/// A package-specific pin override, selected when the requested footprint
/// matches one of `footprints` (e.g. an MCU's TQFP pinout differs from its DIP).
#[derive(Debug, Clone, Deserialize)]
struct RawVariant {
    footprints: Vec<String>,
    pins: Vec<RawPin>,
}

/// A part entry as authored.
#[derive(Debug, Clone, Deserialize)]
struct RawPart {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    footprints: Vec<String>,
    pins: Vec<RawPin>,
    #[serde(default)]
    variants: Vec<RawVariant>,
    #[serde(default)]
    datasheet_url: Option<String>,
    #[serde(default)]
    app_notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPack {
    parts: Vec<RawPart>,
}

/// The compiled-in jellybean database, parsed once on first use.
fn pack() -> &'static [RawPart] {
    static PACK: OnceLock<Vec<RawPart>> = OnceLock::new();
    PACK.get_or_init(|| {
        let raw = include_str!("../../../lib/parts/jellybeans.json");
        let parsed: RawPack =
            serde_json::from_str(raw).expect("lib/parts/jellybeans.json is malformed");
        parsed.parts
    })
}

// ---------------------------------------------------------------------------
// Resolved output shapes
// ---------------------------------------------------------------------------

/// A resolved schematic pin: the curated definition plus an auto-generated
/// component-local symbol position.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PartPin {
    /// Pin number (matches the footprint pad number).
    pub number: String,
    /// Pin name (e.g. `"VCC"`, `"OUT"`).
    pub name: String,
    /// `PinType` variant name (e.g. `"PowerInput"`, `"Output"`).
    pub pin_type: String,
    /// Schematic-symbol X position, component-local mm.
    pub x: f64,
    /// Schematic-symbol Y position, component-local mm.
    pub y: f64,
}

/// A fully-resolved jellybean part: pins for the requested footprint plus the
/// metadata a caller needs to place and document the component.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedPartDef {
    /// Canonical part name (e.g. `"NE555"`).
    pub name: String,
    /// The alias the query matched, when it was an alias rather than the
    /// canonical name (e.g. querying `"LM555"` → `Some("LM555")`).
    pub matched_alias: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// The footprint these pins are for: the requested one when known, else the
    /// part's default (first) footprint.
    pub footprint: String,
    /// Whether `footprint` is a package this part is curated for. `false` means
    /// the requested footprint was unrecognized and the default pinout was used
    /// (see `warnings`).
    pub footprint_known: bool,
    /// Every footprint this part is curated for.
    pub footprints: Vec<String>,
    /// Resolved pins, in pin-number order, each with a symbol position.
    pub pins: Vec<PartPin>,
    /// Datasheet URL, if known.
    pub datasheet_url: Option<String>,
    /// Application notes (e.g. recommended bypass caps, enable-pin handling).
    pub app_notes: Vec<String>,
    /// Non-fatal advisories raised during resolution.
    pub warnings: Vec<String>,
}

/// A one-line catalog summary, for surfacing the database in search/help.
#[derive(Debug, Clone, Serialize)]
pub struct JellybeanSummary {
    /// Canonical part name.
    pub name: String,
    /// Known aliases.
    pub aliases: Vec<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Curated packages.
    pub footprints: Vec<String>,
    /// Pin count of the default pinout.
    pub pin_count: usize,
}

// ---------------------------------------------------------------------------
// Layout + matching helpers
// ---------------------------------------------------------------------------

/// Schematic pin pitch (100-mil grid), component-local mm.
const PIN_PITCH: f64 = 2.54;
/// Horizontal gap between the two pin columns of the auto-generated symbol.
const SYMBOL_WIDTH: f64 = 10.16;

fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// Lenient footprint match: equal after normalization, or the requested id
/// contains the curated code as a substring (so a long KiCad id like
/// `"Package_SO:SOIC-8_3.9x4.9mm_P1.27mm"` still matches the code `"SOIC-8"`).
fn footprint_matches(requested: &str, candidate: &str) -> bool {
    let r = norm(requested);
    let c = norm(candidate);
    r == c || r.contains(&c)
}

/// Lay pins out in the classic two-column DIP arrangement: the first half runs
/// down the left edge top-to-bottom; the remainder runs up the right edge, so
/// the last pin sits top-right across from pin 1 (top-left).
fn layout(pins: &[RawPin]) -> Vec<PartPin> {
    let n = pins.len();
    let half = n.div_ceil(2);
    pins.iter()
        .enumerate()
        .map(|(i, p)| {
            let (x, y) = if i < half {
                (0.0, i as f64 * PIN_PITCH)
            } else {
                let k = i - half; // 0-based within the right column, bottom→top
                (SYMBOL_WIDTH, (half - 1 - k) as f64 * PIN_PITCH)
            };
            PartPin {
                number: p.number.clone(),
                name: p.name.clone(),
                pin_type: p.pin_type.clone(),
                x,
                y,
            }
        })
        .collect()
}

fn find_part(query: &str) -> Option<(&'static RawPart, Option<String>)> {
    let q = norm(query);
    if q.is_empty() {
        return None;
    }
    for p in pack() {
        if norm(&p.name) == q {
            return Some((p, None));
        }
        if let Some(alias) = p.aliases.iter().find(|a| norm(a) == q) {
            return Some((p, Some(alias.clone())));
        }
    }
    None
}

/// Pick the pin set for a part given an optional requested footprint: a
/// matching variant wins, then a curated top-level footprint, then the default
/// pinout with a warning when the footprint is unrecognized.
fn select_pins<'a>(
    part: &'a RawPart,
    footprint: Option<&str>,
) -> (&'a [RawPin], String, bool, Vec<String>) {
    let fp = footprint.map(str::trim).filter(|s| !s.is_empty());
    let Some(fp) = fp else {
        // No footprint requested — default pinout, default package.
        let default_fp = part.footprints.first().cloned().unwrap_or_default();
        return (&part.pins, default_fp, true, Vec::new());
    };

    for v in &part.variants {
        if v.footprints.iter().any(|c| footprint_matches(fp, c)) {
            return (&v.pins, fp.to_string(), true, Vec::new());
        }
    }
    if part.footprints.iter().any(|c| footprint_matches(fp, c)) {
        return (&part.pins, fp.to_string(), true, Vec::new());
    }

    let warning = format!(
        "Footprint '{}' is not a curated package for {} (known: {}). Using the \
         default {}-pin pinout — verify the pin numbering matches your part.",
        fp,
        part.name,
        part.footprints.join(", "),
        part.pins.len(),
    );
    (&part.pins, fp.to_string(), false, vec![warning])
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve a jellybean part by name or alias (case-insensitive) into its pins
/// for the requested footprint — or its default package when `footprint` is
/// `None`. Returns `None` when the name is not in the database (the caller can
/// then fall back to the parametric passive catalog or require explicit pins).
pub fn resolve_part_def(query: &str, footprint: Option<&str>) -> Option<ResolvedPartDef> {
    let (part, matched_alias) = find_part(query)?;
    let (pins, chosen_fp, footprint_known, warnings) = select_pins(part, footprint);
    Some(ResolvedPartDef {
        name: part.name.clone(),
        matched_alias,
        description: part.description.clone(),
        footprint: chosen_fp,
        footprint_known,
        footprints: part.footprints.clone(),
        pins: layout(pins),
        datasheet_url: part.datasheet_url.clone(),
        app_notes: part.app_notes.clone(),
        warnings,
    })
}

/// Every canonical jellybean part name, sorted — for listing and help.
pub fn jellybean_names() -> Vec<String> {
    let mut names: Vec<String> = pack().iter().map(|p| p.name.clone()).collect();
    names.sort();
    names
}

/// One-line summaries of the whole jellybean catalog, sorted by name.
pub fn jellybean_summaries() -> Vec<JellybeanSummary> {
    let mut out: Vec<JellybeanSummary> = pack()
        .iter()
        .map(|p| JellybeanSummary {
            name: p.name.clone(),
            aliases: p.aliases.clone(),
            description: p.description.clone(),
            footprints: p.footprints.clone(),
            pin_count: p.pins.len(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// JSON manifest of the jellybean catalog (for MCP `search`/help surfaces).
pub fn jellybean_manifest_json() -> String {
    serde_json::to_string(&jellybean_summaries()).unwrap_or_else(|_| "[]".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical set of `vcad_ir::ecad::PinType` variant names. Kept in
    /// sync with the enum so the data pack can't introduce an unknown type.
    const VALID_PIN_TYPES: &[&str] = &[
        "Input",
        "Output",
        "Bidirectional",
        "TriState",
        "Passive",
        "PowerInput",
        "PowerOutput",
        "OpenCollector",
        "OpenEmitter",
        "NotConnected",
        "Free",
    ];

    /// Extract the pin count a footprint code implies, for the families where
    /// the trailing integer is unambiguously the pin count (DIP-8, TQFP-32,
    /// LGA-8, ...). Returns `None` for codes where a number is part of the
    /// package *name* (SOT-223, TO-220, DO-214) or absent.
    fn pins_from_footprint(code: &str) -> Option<usize> {
        let c = code.to_ascii_uppercase();
        const FAMILIES: &[&str] = &[
            "DIP-", "SOIC-", "SO-", "SSOP-", "TSSOP-", "VSSOP-", "MSOP-", "QFP-", "TQFP-", "LQFP-",
            "QFN-", "VQFN-", "MLF-", "DFN-", "LGA-",
        ];
        for fam in FAMILIES {
            if let Some(rest) = c.strip_prefix(fam) {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                if let Ok(n) = digits.parse::<usize>() {
                    return Some(n);
                }
            }
        }
        None
    }

    #[test]
    fn pack_parses_and_is_nonempty() {
        assert!(!pack().is_empty(), "jellybean pack should not be empty");
    }

    /// Every authored pin uses a real PinType, numbers are unique within a
    /// pinout, names/footprints are non-empty, and each variant lists pins.
    #[test]
    fn every_part_is_structurally_valid() {
        for p in pack() {
            assert!(!p.name.is_empty(), "part has empty name");
            assert!(!p.footprints.is_empty(), "{} lists no footprints", p.name);
            assert!(!p.pins.is_empty(), "{} has no pins", p.name);

            let check_pins = |label: &str, pins: &[RawPin]| {
                let mut seen = std::collections::HashSet::new();
                for pin in pins {
                    assert!(
                        !pin.number.is_empty(),
                        "{} ({label}) has a pin with an empty number",
                        p.name
                    );
                    assert!(
                        !pin.name.is_empty(),
                        "{} ({label}) pin {} has an empty name",
                        p.name,
                        pin.number
                    );
                    assert!(
                        VALID_PIN_TYPES.contains(&pin.pin_type.as_str()),
                        "{} ({label}) pin {} has unknown type '{}'",
                        p.name,
                        pin.number,
                        pin.pin_type
                    );
                    assert!(
                        seen.insert(pin.number.clone()),
                        "{} ({label}) has duplicate pin number {}",
                        p.name,
                        pin.number
                    );
                }
            };

            check_pins("default", &p.pins);
            for (i, v) in p.variants.iter().enumerate() {
                assert!(
                    !v.footprints.is_empty(),
                    "{} variant #{i} lists no footprints",
                    p.name
                );
                check_pins("variant", &v.pins);
            }
        }
    }

    /// The pin count must match the footprint code wherever the code implies
    /// one — the cheapest guard against a miscounted pinout. A footprint listed
    /// on a variant is checked against that variant's pin count.
    #[test]
    fn footprint_pin_counts_are_consistent() {
        for p in pack() {
            for fp in &p.footprints {
                let Some(expected) = pins_from_footprint(fp) else {
                    continue;
                };
                let variant = p
                    .variants
                    .iter()
                    .find(|v| v.footprints.iter().any(|c| c == fp));
                let actual = variant.map_or(p.pins.len(), |v| v.pins.len());
                assert_eq!(
                    actual, expected,
                    "{} footprint {fp} implies {expected} pins but the pinout has {actual}",
                    p.name
                );
            }
        }
    }

    #[test]
    fn resolves_ne555_with_eight_pins() {
        let part = resolve_part_def("NE555", Some("SOIC-8")).unwrap();
        assert_eq!(part.name, "NE555");
        assert_eq!(part.footprint, "SOIC-8");
        assert!(part.footprint_known);
        assert!(part.warnings.is_empty());
        assert_eq!(part.pins.len(), 8);
        // Spot-check the published pinout.
        assert_eq!(part.pins[0].name, "GND");
        assert_eq!(part.pins[0].pin_type, "PowerInput");
        assert_eq!(part.pins[2].name, "OUT");
        assert_eq!(part.pins[2].pin_type, "Output");
        assert_eq!(part.pins[7].name, "VCC");
    }

    #[test]
    fn fc_parts_resolve_with_expected_pin_counts() {
        // (query, footprint, expected pad count). Counts must equal the package
        // pad count so place_components maps every pin to a pad.
        let cases = [
            ("SN65HVD230", "SOIC-8", 8),
            ("TCAN1042HGV", "SOIC-8", 8),
            ("TCAN1042", "SOIC-8", 8), // alias
            ("MCP2515", "SOIC-18", 18),
            ("MCP2515", "DIP-18", 18),
            ("TPS562200", "SOT-23-6", 6),
            ("STM32G431CBT6", "LQFP-48", 48),
            ("STM32G431", "LQFP-48", 48), // alias
            ("RP2040", "QFN-56", 56),
        ];
        for (name, fp, n) in cases {
            let def = resolve_part_def(name, Some(fp))
                .unwrap_or_else(|| panic!("{name} did not resolve"));
            assert_eq!(def.pins.len(), n, "{name}/{fp} pin count");
            assert!(def.footprint_known, "{name}/{fp} footprint should be curated");
            assert!(def.datasheet_url.is_some(), "{name} should carry a datasheet");
            assert!(!def.app_notes.is_empty(), "{name} should carry app notes");
            assert!(
                def.pins
                    .iter()
                    .any(|p| p.pin_type == "PowerInput" || p.pin_type == "PowerOutput"),
                "{name} should have at least one power pin",
            );
        }
    }

    #[test]
    fn resolves_by_case_insensitive_alias() {
        let part = resolve_part_def("lm555", None).unwrap();
        assert_eq!(part.name, "NE555");
        assert_eq!(part.matched_alias.as_deref(), Some("LM555"));
        // Default footprint when none requested.
        assert_eq!(part.footprint, "DIP-8");
    }

    #[test]
    fn comparator_outputs_are_open_collector() {
        let part = resolve_part_def("LM393", None).unwrap();
        let out1 = part.pins.iter().find(|p| p.name == "OUT1").unwrap();
        assert_eq!(out1.pin_type, "OpenCollector");
    }

    #[test]
    fn mcu_variant_pinout_is_footprint_selected() {
        let dip = resolve_part_def("ATmega328P", Some("DIP-28")).unwrap();
        assert_eq!(dip.pins.len(), 28);
        assert_eq!(dip.pins[0].name, "PC6/~RESET");

        let tqfp = resolve_part_def("ATmega328P", Some("TQFP-32")).unwrap();
        assert_eq!(tqfp.pins.len(), 32);
        assert!(tqfp.footprint_known);
        // The TQFP map starts at PD3, not RESET.
        assert_eq!(tqfp.pins[0].name, "PD3");
    }

    #[test]
    fn long_kicad_footprint_id_still_matches() {
        let part = resolve_part_def("NE555", Some("Package_SO:SOIC-8_3.9x4.9mm_P1.27mm")).unwrap();
        assert!(part.footprint_known);
        assert_eq!(part.pins.len(), 8);
    }

    #[test]
    fn unknown_footprint_warns_but_resolves() {
        let part = resolve_part_def("NE555", Some("BGA-99")).unwrap();
        assert!(!part.footprint_known);
        assert_eq!(part.pins.len(), 8, "falls back to the default pinout");
        assert_eq!(part.warnings.len(), 1);
        assert!(part.warnings[0].contains("BGA-99"));
    }

    #[test]
    fn unknown_part_returns_none() {
        assert!(resolve_part_def("DEFINITELY_NOT_A_PART", None).is_none());
        assert!(resolve_part_def("", None).is_none());
    }

    #[test]
    fn auto_layout_is_two_columns() {
        let part = resolve_part_def("NE555", Some("DIP-8")).unwrap();
        // Pin 1 top-left, pins 1-4 down the left column.
        assert_eq!((part.pins[0].x, part.pins[0].y), (0.0, 0.0));
        assert_eq!(part.pins[3].x, 0.0);
        // Pin 8 top-right (across from pin 1); pin 5 at the bottom-right.
        assert_eq!(part.pins[7].x, SYMBOL_WIDTH);
        assert_eq!(part.pins[7].y, 0.0);
        assert_eq!(part.pins[4].x, SYMBOL_WIDTH);
        assert!(part.pins[4].y > part.pins[7].y);
    }

    #[test]
    fn names_and_manifest_are_populated() {
        let names = jellybean_names();
        assert!(names.contains(&"NE555".to_string()));
        assert!(names.contains(&"LM358".to_string()));
        let manifest = jellybean_manifest_json();
        assert!(manifest.contains("NE555"));
    }
}
