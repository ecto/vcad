//! Free-text query parsing: `"10k 0603 1%"` → typed constraints.
//!
//! Fails *visibly* — unrecognized tokens are kept in `unknown` rather than
//! silently dropped, so a caller can surface ambiguity instead of guessing.

use crate::spec::{parse_spec, SpecValue};

/// A parsed component query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedQuery {
    /// Typed specs found in the query (value, tolerance, ratings, …).
    pub specs: Vec<SpecValue>,
    /// A package code if one was recognized (e.g. "0603", "QFN-40", "SOIC-8").
    pub package: Option<String>,
    /// Tokens that matched nothing (surfaced, not dropped).
    pub unknown: Vec<String>,
}

impl ParsedQuery {
    /// The primary value spec (resistance/capacitance/inductance), if any.
    pub fn primary_value(&self) -> Option<SpecValue> {
        self.specs.iter().copied().find(|s| {
            matches!(
                s,
                SpecValue::Resistance(_) | SpecValue::Capacitance(_) | SpecValue::Inductance(_)
            )
        })
    }

    /// The tolerance spec, if any.
    pub fn tolerance(&self) -> Option<f64> {
        self.specs.iter().find_map(|s| match s {
            SpecValue::Tolerance(t) => Some(*t),
            _ => None,
        })
    }
}

/// True if a token looks like a package code we recognize.
fn is_package_token(tok: &str) -> bool {
    // Imperial chip codes: exactly four digits.
    if tok.len() == 4 && tok.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    let upper = tok.to_ascii_uppercase();
    const PREFIXES: &[&str] = &[
        "QFN", "DFN", "SON", "QFP", "LQFP", "TQFP", "SOIC", "SO-", "SOP", "SSOP", "TSSOP", "MSOP",
        "SOT", "SC-70", "DPAK", "D2PAK", "TO-", "DIP", "SOD", "BGA",
    ];
    PREFIXES.iter().any(|p| upper.starts_with(p))
}

/// Parse a free-text query into typed constraints.
pub fn parse(query: &str) -> ParsedQuery {
    let mut out = ParsedQuery::default();
    for tok in query.split_whitespace() {
        if is_package_token(tok) {
            // First package wins; later ones are noise.
            if out.package.is_none() {
                out.package = Some(tok.to_string());
            }
            continue;
        }
        if let Some(spec) = parse_spec(tok) {
            out.specs.push(spec);
            continue;
        }
        out.unknown.push(tok.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resistor_query() {
        let q = parse("10k 0603 1%");
        assert_eq!(q.package.as_deref(), Some("0603"));
        assert_eq!(q.primary_value(), Some(SpecValue::Resistance(10_000.0)));
        assert_eq!(q.tolerance(), Some(0.01));
        assert!(q.unknown.is_empty());
    }

    #[test]
    fn parses_capacitor_query() {
        let q = parse("100nF 0402 X7R 16V");
        assert_eq!(q.package.as_deref(), Some("0402"));
        let v = q.primary_value().unwrap();
        assert!(matches!(v, SpecValue::Capacitance(_)));
        assert!((v.magnitude() - 100e-9).abs() < 1e-15);
        assert!(q.specs.contains(&SpecValue::Voltage(16.0)));
        // "X7R" is a dielectric token we don't model yet — surfaced, not dropped.
        assert_eq!(q.unknown, vec!["X7R".to_string()]);
    }

    #[test]
    fn recognizes_ic_package() {
        let q = parse("QFN-40");
        assert_eq!(q.package.as_deref(), Some("QFN-40"));
    }
}
