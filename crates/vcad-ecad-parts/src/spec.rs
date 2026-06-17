//! Typed component specifications and the SI/RKM value parser.
//!
//! A spec is a typed [`SpecValue`] (resistance in ohms, capacitance in farads,
//! …) so that `"10k"` and `"10000"` and `"0.01M"` all compare equal — the
//! catalog reasons over physical quantities, not strings.

use serde::{Deserialize, Serialize};

/// A typed, dimensioned component specification. All values are SI base units
/// (ohms, farads, henries, volts, amps, watts, hertz); tolerance is a fraction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "dim", content = "value")]
pub enum SpecValue {
    /// Resistance in ohms.
    Resistance(f64),
    /// Capacitance in farads.
    Capacitance(f64),
    /// Inductance in henries.
    Inductance(f64),
    /// Voltage rating in volts.
    Voltage(f64),
    /// Current rating in amps.
    Current(f64),
    /// Power rating in watts.
    Power(f64),
    /// Frequency in hertz.
    Frequency(f64),
    /// Tolerance as a fraction (0.01 = 1%).
    Tolerance(f64),
}

impl SpecValue {
    /// The scalar magnitude in SI base units.
    pub fn magnitude(&self) -> f64 {
        match self {
            SpecValue::Resistance(v)
            | SpecValue::Capacitance(v)
            | SpecValue::Inductance(v)
            | SpecValue::Voltage(v)
            | SpecValue::Current(v)
            | SpecValue::Power(v)
            | SpecValue::Frequency(v)
            | SpecValue::Tolerance(v) => *v,
        }
    }

    /// A short dimension tag for display/grouping.
    pub fn dim(&self) -> &'static str {
        match self {
            SpecValue::Resistance(_) => "resistance",
            SpecValue::Capacitance(_) => "capacitance",
            SpecValue::Inductance(_) => "inductance",
            SpecValue::Voltage(_) => "voltage",
            SpecValue::Current(_) => "current",
            SpecValue::Power(_) => "power",
            SpecValue::Frequency(_) => "frequency",
            SpecValue::Tolerance(_) => "tolerance",
        }
    }

    /// True if two specs measure the same physical dimension.
    pub fn same_dim(&self, other: &SpecValue) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// SI prefix multiplier for a single character, or `None` if not a prefix.
fn prefix_mult(c: char) -> Option<f64> {
    Some(match c {
        'p' => 1e-12,
        'n' => 1e-9,
        'u' | 'µ' => 1e-6,
        'm' => 1e-3,
        'k' | 'K' => 1e3,
        'M' => 1e6,
        'G' => 1e9,
        'R' | 'r' => 1.0,
        _ => return None,
    })
}

/// Parse an RKM/SI-coded magnitude such as `"10k"`, `"4u7"`, `"100n"`, `"4R7"`,
/// `"220"`, `"2.2"`. The prefix letter may sit at the end (`"10k"`) or act as a
/// decimal point (`"4u7"` = 4.7e-6, `"4R7"` = 4.7).
pub fn parse_magnitude(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Find an SI prefix letter inside the token.
    if let Some((idx, c)) = s.char_indices().find(|(_, c)| prefix_mult(*c).is_some()) {
        let mult = prefix_mult(c).unwrap();
        let head = &s[..idx];
        let tail = &s[idx + c.len_utf8()..];
        let num: f64 = if tail.is_empty() {
            head.parse().ok()?
        } else if head.is_empty() {
            // Prefix-led like "k5" — unusual; treat as 0.<tail>.
            format!("0.{tail}").parse().ok()?
        } else {
            // RKM infix: head + "." + tail.
            format!("{head}.{tail}").parse().ok()?
        };
        return Some(num * mult);
    }
    s.parse().ok()
}

/// Parse a value token into a typed [`SpecValue`], inferring the dimension from
/// a trailing unit when present, else from EE shorthand convention:
/// prefixes p/n/u → capacitance, otherwise resistance.
pub fn parse_spec(token: &str) -> Option<SpecValue> {
    let t = token.trim();
    if t.is_empty() {
        return None;
    }
    // Tolerance.
    if let Some(num) = t.strip_suffix('%') {
        return num
            .trim()
            .parse::<f64>()
            .ok()
            .map(|p| SpecValue::Tolerance(p / 100.0));
    }
    // Explicit units (longest suffix first). Slice from the original string so
    // the magnitude core keeps its case (M = mega, not m = milli).
    if t.len() >= 2 && t[t.len() - 2..].eq_ignore_ascii_case("hz") {
        return parse_magnitude(&t[..t.len() - 2]).map(SpecValue::Frequency);
    }
    if t.ends_with('Ω') {
        return parse_magnitude(t.trim_end_matches('Ω')).map(SpecValue::Resistance);
    }
    if t.len() >= 3 && t[t.len() - 3..].eq_ignore_ascii_case("ohm") {
        return parse_magnitude(&t[..t.len() - 3]).map(SpecValue::Resistance);
    }
    // Single-letter units. Guard against the prefix letters that double as
    // units only when they trail (F farad, H henry, V volt, A amp, W watt).
    if let Some(last) = t.chars().last() {
        let unit = match last {
            'F' | 'f' => Some(SpecValue::Capacitance as fn(f64) -> SpecValue),
            'H' | 'h' => Some(SpecValue::Inductance as fn(f64) -> SpecValue),
            'V' | 'v' => Some(SpecValue::Voltage as fn(f64) -> SpecValue),
            'A' => Some(SpecValue::Current as fn(f64) -> SpecValue),
            'W' | 'w' => Some(SpecValue::Power as fn(f64) -> SpecValue),
            _ => None,
        };
        if let Some(ctor) = unit {
            let core = &t[..t.len() - last.len_utf8()];
            return parse_magnitude(core).map(ctor);
        }
    }
    // No explicit unit: infer from prefix. p/n/u → capacitance, else resistance.
    let mag = parse_magnitude(t)?;
    let is_cap = t.chars().any(|c| matches!(c, 'p' | 'n' | 'u' | 'µ'));
    Some(if is_cap {
        SpecValue::Capacitance(mag)
    } else {
        SpecValue::Resistance(mag)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnitudes() {
        assert!((parse_magnitude("10k").unwrap() - 10_000.0).abs() < 1e-6);
        assert!((parse_magnitude("4u7").unwrap() - 4.7e-6).abs() < 1e-12);
        assert!((parse_magnitude("4R7").unwrap() - 4.7).abs() < 1e-9);
        assert!((parse_magnitude("100n").unwrap() - 100e-9).abs() < 1e-15);
        assert!((parse_magnitude("1M").unwrap() - 1e6).abs() < 1e-3);
        assert!((parse_magnitude("220").unwrap() - 220.0).abs() < 1e-9);
        assert!((parse_magnitude("2.2").unwrap() - 2.2).abs() < 1e-9);
    }

    /// Assert the parse has the expected dimension and magnitude (within ULP).
    fn check(token: &str, expect: SpecValue) {
        let got = parse_spec(token).unwrap_or_else(|| panic!("failed to parse {token}"));
        assert!(got.same_dim(&expect), "{token}: dim {got:?} != {expect:?}");
        assert!(
            (got.magnitude() - expect.magnitude()).abs() <= expect.magnitude().abs() * 1e-9 + 1e-18,
            "{token}: {} != {}",
            got.magnitude(),
            expect.magnitude()
        );
    }

    #[test]
    fn typed_specs() {
        check("10k", SpecValue::Resistance(10_000.0));
        check("100nF", SpecValue::Capacitance(100e-9));
        check("4.7uF", SpecValue::Capacitance(4.7e-6));
        check("10uH", SpecValue::Inductance(10e-6));
        check("16V", SpecValue::Voltage(16.0));
        check("3A", SpecValue::Current(3.0));
        check("0.25W", SpecValue::Power(0.25));
        check("1%", SpecValue::Tolerance(0.01));
        check("16MHz", SpecValue::Frequency(16e6));
        check("4k7", SpecValue::Resistance(4700.0));
    }

    #[test]
    fn ten_k_equals_ten_thousand() {
        assert_eq!(parse_spec("10k"), parse_spec("10000"));
    }
}
