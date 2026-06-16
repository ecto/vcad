//! The generative parts catalog: parametric families resolved into concrete,
//! fully-derived parts. A part is `family + value + package`, not a scraped row.

use serde::{Deserialize, Serialize};
use vcad_ecad_package::{derive, presets};
use vcad_ir::ecad::DerivedPart;

use crate::eseries::{neighbors, snap, ESeries};
use crate::query::{self, ParsedQuery};
use crate::spec::SpecValue;

/// The electrical class of a component family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComponentClass {
    /// Fixed resistor.
    Resistor,
    /// Capacitor.
    Capacitor,
    /// Inductor.
    Inductor,
}

impl ComponentClass {
    /// Reference-designator prefix.
    pub fn ref_prefix(&self) -> &'static str {
        match self {
            ComponentClass::Resistor => "R",
            ComponentClass::Capacitor => "C",
            ComponentClass::Inductor => "L",
        }
    }

    /// Infer the class from a value spec's dimension.
    fn from_value(v: &SpecValue) -> Option<Self> {
        match v {
            SpecValue::Resistance(_) => Some(ComponentClass::Resistor),
            SpecValue::Capacitance(_) => Some(ComponentClass::Capacitor),
            SpecValue::Inductance(_) => Some(ComponentClass::Inductor),
            _ => None,
        }
    }

    fn rebuild_value(&self, magnitude: f64) -> SpecValue {
        match self {
            ComponentClass::Resistor => SpecValue::Resistance(magnitude),
            ComponentClass::Capacitor => SpecValue::Capacitance(magnitude),
            ComponentClass::Inductor => SpecValue::Inductance(magnitude),
        }
    }
}

/// A parametric component family.
#[derive(Debug, Clone, Serialize)]
pub struct PartFamily {
    /// Dotted id (e.g. "passive.resistor.chip").
    pub id: &'static str,
    /// Electrical class.
    pub class: ComponentClass,
    /// Human name.
    pub name: &'static str,
    /// Allowed chip package codes (the first is the default).
    pub packages: &'static [&'static str],
}

/// The compile-time family registry. Passives generate infinite value coverage
/// from these few entries — no scraped rows.
pub fn all_families() -> Vec<PartFamily> {
    vec![
        PartFamily {
            id: "passive.resistor.chip",
            class: ComponentClass::Resistor,
            name: "Chip resistor",
            packages: &[
                "0402", "0603", "0805", "1206", "1210", "2010", "2512", "0201",
            ],
        },
        PartFamily {
            id: "passive.capacitor.mlcc",
            class: ComponentClass::Capacitor,
            name: "MLCC capacitor",
            packages: &["0402", "0603", "0805", "1206", "1210", "0201"],
        },
        PartFamily {
            id: "passive.inductor.chip",
            class: ComponentClass::Inductor,
            name: "Chip inductor",
            packages: &["0402", "0603", "0805", "1206"],
        },
    ]
}

/// A manufacturer cross-reference (the sparse, durable bridge to a real MPN).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ElecXref {
    /// Manufacturer part number.
    pub mpn: String,
    /// Manufacturer name.
    pub manufacturer: String,
    /// Optional datasheet URL.
    pub datasheet: Option<String>,
}

/// A fully-resolved part: the binding plus its derived geometry.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedPart {
    /// Source family id.
    pub family_id: String,
    /// Electrical class.
    pub class: ComponentClass,
    /// Display value (e.g. "10k", "100nF").
    pub value: String,
    /// Snapped value in SI base units.
    pub value_si: SpecValue,
    /// Tolerance fraction, if specified.
    pub tolerance: Option<f64>,
    /// Package code.
    pub package: String,
    /// Footprint + symbol + body + courtyard, generated from one PackageClass.
    pub derived: DerivedPart,
    /// Cross-references to real manufacturer parts (may be empty).
    pub mpns: Vec<ElecXref>,
}

/// Format an SI magnitude with an engineering suffix appropriate to the class.
fn format_value(class: ComponentClass, magnitude: f64) -> String {
    let (suffixes, unit_omit) = match class {
        ComponentClass::Resistor => (&[(1e6, "M"), (1e3, "k"), (1.0, ""), (1e-3, "m")][..], true),
        ComponentClass::Capacitor => (
            &[
                (1.0, "F"),
                (1e-3, "mF"),
                (1e-6, "uF"),
                (1e-9, "nF"),
                (1e-12, "pF"),
            ][..],
            false,
        ),
        ComponentClass::Inductor => (
            &[(1.0, "H"), (1e-3, "mH"), (1e-6, "uH"), (1e-9, "nH")][..],
            false,
        ),
    };
    for &(scale, suffix) in suffixes {
        if magnitude >= scale * 0.999 {
            let v = magnitude / scale;
            let s = if (v.round() - v).abs() < 1e-6 {
                format!("{}", v.round() as i64)
            } else {
                format!("{v:.2}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            };
            return format!("{s}{suffix}");
        }
    }
    let _ = unit_omit;
    format!("{magnitude}")
}

/// Look up seeded MPN cross-references for a resolved passive. Sparse by design
/// — the catalog generates geometry; xrefs are a durable, hand-curated bridge.
fn seeded_xrefs(class: ComponentClass, package: &str, value_si: f64) -> Vec<ElecXref> {
    match (class, package) {
        (ComponentClass::Resistor, "0603") if (value_si - 10_000.0).abs() < 1.0 => vec![ElecXref {
            mpn: "RC0603FR-0710KL".into(),
            manufacturer: "Yageo".into(),
            datasheet: None,
        }],
        (ComponentClass::Capacitor, "0402") if (value_si - 100e-9).abs() < 1e-12 => {
            vec![ElecXref {
                mpn: "CL05B104KO5NNNC".into(),
                manufacturer: "Samsung".into(),
                datasheet: None,
            }]
        }
        _ => vec![],
    }
}

fn build(
    class: ComponentClass,
    magnitude: f64,
    package: &str,
    tolerance: Option<f64>,
) -> Option<ResolvedPart> {
    let pc = presets::chip(package);
    let mut derived = derive(&pc).ok()?;
    let value_si = class.rebuild_value(magnitude);
    let value = format_value(class, magnitude);
    // Stamp the symbol with the class's identity.
    derived.symbol.prefix = class.ref_prefix().to_string();
    derived.symbol.default_value = value.clone();
    derived.symbol.name = format!("{} {}", value, package);
    let family_id = all_families()
        .into_iter()
        .find(|f| f.class == class)
        .map(|f| f.id.to_string())
        .unwrap_or_default();
    Some(ResolvedPart {
        family_id,
        class,
        value,
        value_si,
        tolerance,
        package: package.to_string(),
        derived,
        mpns: seeded_xrefs(class, package, magnitude),
    })
}

/// Default package per class when the query omits one.
fn default_package(class: ComponentClass) -> &'static str {
    all_families()
        .into_iter()
        .find(|f| f.class == class)
        .and_then(|f| f.packages.first().copied())
        .unwrap_or("0603")
}

/// Resolve a free-text query into one fully-specified part, E-series-snapped.
///
/// Returns `None` when the query carries no resolvable passive value (e.g. an
/// IC package alone — ICs need an MPN, not a parametric value).
pub fn resolve(query: &str) -> Option<ResolvedPart> {
    let q = query::parse(query);
    resolve_parsed(&q)
}

fn resolve_parsed(q: &ParsedQuery) -> Option<ResolvedPart> {
    let value = q.primary_value()?;
    let class = ComponentClass::from_value(&value)?;
    let series = match q.tolerance() {
        Some(t) if t <= 0.0101 => ESeries::E96,
        _ => ESeries::E24,
    };
    let magnitude = snap(value.magnitude(), series);
    let package = q
        .package
        .clone()
        .unwrap_or_else(|| default_package(class).to_string());
    build(class, magnitude, &package, q.tolerance())
}

/// Search the catalog, returning the best-resolved part plus its nearest
/// E-series neighbours (spec-distance ranked) as alternatives. Fully offline.
pub fn search(query: &str, limit: usize) -> Vec<ResolvedPart> {
    let q = query::parse(query);
    let Some(value) = q.primary_value() else {
        return vec![];
    };
    let Some(class) = ComponentClass::from_value(&value) else {
        return vec![];
    };
    let series = match q.tolerance() {
        Some(t) if t <= 0.0101 => ESeries::E96,
        _ => ESeries::E24,
    };
    let package = q
        .package
        .clone()
        .unwrap_or_else(|| default_package(class).to_string());
    let mut out = Vec::new();
    for mag in neighbors(value.magnitude(), series, limit.max(1)) {
        if let Some(p) = build(class, mag, &package, q.tolerance()) {
            out.push(p);
        }
    }
    out
}

/// JSON manifest of all families (for surfacing in MCP `search`/help).
pub fn manifest_json() -> String {
    serde_json::to_string(&all_families()).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_resistor() {
        let p = resolve("10k 0603 1%").unwrap();
        assert_eq!(p.class, ComponentClass::Resistor);
        assert_eq!(p.package, "0603");
        assert_eq!(p.value, "10k");
        assert_eq!(p.tolerance, Some(0.01));
        assert_eq!(p.derived.footprint.pads.len(), 2);
        assert_eq!(p.derived.symbol.prefix, "R");
        // Seeded MPN xref present for this common part.
        assert_eq!(p.mpns.len(), 1);
        assert_eq!(p.mpns[0].manufacturer, "Yageo");
    }

    #[test]
    fn resolves_a_capacitor_with_default_package() {
        let p = resolve("100nF").unwrap();
        assert_eq!(p.class, ComponentClass::Capacitor);
        assert_eq!(p.value, "100nF");
        assert_eq!(p.derived.symbol.prefix, "C");
    }

    #[test]
    fn snaps_off_value_to_e_series() {
        // 10.4k with 1% → E96 snaps to 10.5k (nearest E96), not 10.4k.
        let p = resolve("10.4k 1%").unwrap();
        assert_ne!(p.value, "10.4k");
    }

    #[test]
    fn ic_package_alone_does_not_resolve() {
        assert!(resolve("QFN-40").is_none());
    }

    #[test]
    fn search_returns_neighbors() {
        let r = search("10k", 5);
        assert_eq!(r.len(), 5);
        assert!(r.iter().all(|p| p.class == ComponentClass::Resistor));
    }

    #[test]
    fn manifest_lists_families() {
        let m = manifest_json();
        assert!(m.contains("passive.resistor.chip"));
        assert!(m.contains("passive.capacitor.mlcc"));
    }
}
