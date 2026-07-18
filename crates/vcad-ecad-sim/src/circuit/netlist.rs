//! Schematic connectivity → simulatable [`Circuit`] — the netlist-from-ecad
//! seam (spice-m0 M1 item 4).
//!
//! A board designed via `create_schematic` (a [`vcad_ir::ecad::SchematicSheet`])
//! already carries everything a lumped simulation needs: components with
//! reference designators and value strings, and nets (explicit `nets` map or
//! wire/label geometry, extracted by [`vcad_ecad_schematic::generate_netlist`]).
//! This module maps that netlist onto [`Circuit`] nodes and [`Device`]s so the
//! schematic simulates and differentiates without re-entry.
//!
//! # Component-mapping convention
//!
//! Components are classified by **reference-designator prefix**, values parsed
//! from the component's `value` string with SI suffixes (see
//! [`parse_si_value`]):
//!
//! | refdes | device | value unit | pin convention |
//! |---|---|---|---|
//! | `R*` | [`Device::Resistor`] | Ω (`"4.7k"`, `"4R7"`) | `1`/`2` (symmetric) |
//! | `C*` | [`Device::Capacitor`] | F (`"100n"`) | `1`/`2` (symmetric) |
//! | `L*` | [`Device::Inductor`] | H (`"10u"`) | `1`/`2` (symmetric) |
//! | `V*` | [`Device::VSource`] | V (`"5"`, `"3.3"`) | `1` = +, `2` = − |
//! | `I*` | [`Device::ISource`] | A (`"10m"`) | `1` = out (+), `2` = return (−) |
//! | `D*` | [`Device::Diode`] (silicon default; LED model if the value contains `"LED"`) | — | `A` = anode, `K` = cathode |
//!
//! Nets whose name is ground-like (`GND`, `AGND`, `DGND`, `VSS`, `0`, `0V`,
//! case-insensitive — the same family the ERC power-net check recognizes) all
//! collapse onto node 0. Distinct grounds (`AGND` vs `DGND`) are therefore
//! shorted at M1; the collapsed set is reported in
//! [`MappedCircuit::ground_nets`] so a caller can notice.
//!
//! # Fail-closed
//!
//! Anything that cannot be mapped — an IC, a connector, an unparsable value, a
//! diode without `A`/`K` pins — **fails the whole conversion** with a typed,
//! per-component [`ComponentBlocker`] list. Nothing is silently skipped. To
//! simulate around a known-irrelevant component (a debug connector, say), name
//! it in [`ConvertOptions::stub_as_open`]: it is omitted (an open circuit) and
//! recorded in [`MappedCircuit::stubbed`].
//!
//! # Honesty (M1)
//!
//! This is the *schematic's* circuit, not the board's: no layout parasitics
//! (trace resistance/inductance, pad capacitance, plane coupling). Extracting
//! those from the routed PCB is the M2 item.

use std::collections::BTreeMap;

use vcad_ecad_schematic::{generate_netlist, Netlist};
use vcad_ir::ecad::SchematicSheet;

use super::{Circuit, Device, DiodeModel};

/// Net names that mean "ground" (compared case-insensitively). The same
/// family the ERC power-net check recognizes as ground rails.
const GROUND_NAMES: &[&str] = &["GND", "AGND", "DGND", "VSS", "GNDA", "GNDD", "0", "0V"];

/// Is this net name a ground rail (→ node 0)?
pub fn is_ground_net(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    GROUND_NAMES.contains(&upper.as_str())
}

// ============================================================================
// SI value parsing
// ============================================================================

/// Why a component value string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueParseError {
    /// The string was empty (or whitespace only).
    #[error("empty value string")]
    Empty,
    /// The numeric part did not parse.
    #[error("unparsable number in {0:?}")]
    BadNumber(String),
    /// An unrecognized suffix followed the number.
    #[error("unrecognized suffix {suffix:?} in {value:?}")]
    BadSuffix {
        /// The full value string.
        value: String,
        /// The offending suffix.
        suffix: String,
    },
    /// More than one SI prefix / infix (e.g. `"4k7n"`).
    #[error("ambiguous value {0:?}: multiple SI prefixes")]
    Ambiguous(String),
}

/// Multiplier for a single SI prefix character, if it is one.
///
/// Case matters exactly where SPICE tradition says it's dangerous: `m` is
/// milli (1e-3) and `M` is mega (1e6). `k`/`K` both mean kilo (no other
/// reading exists). `u` and `µ` are micro.
fn si_multiplier(ch: char) -> Option<f64> {
    match ch {
        'p' => Some(1e-12),
        'n' => Some(1e-9),
        'u' | 'µ' => Some(1e-6),
        'm' => Some(1e-3),
        'k' | 'K' => Some(1e3),
        'M' => Some(1e6),
        'G' => Some(1e9),
        'T' => Some(1e12),
        _ => None,
    }
}

/// Parse a component value with SI suffixes into a plain number.
///
/// Accepted grammar (whitespace trimmed):
/// - plain numbers: `"10"`, `"4.7"`, `"1e-6"`
/// - SI suffix: `"4.7k"`, `"100n"`, `"2M"` (`m` = milli, `M` = mega —
///   case-sensitive exactly there)
/// - infix (R-as-decimal) notation: `"4R7"` = 4.7, `"4k7"` = 4700,
///   `"2n2"` = 2.2e-9
/// - an optional trailing unit word from `unit_letters` (e.g. `"5V"`,
///   `"10kΩ"`, `"100nF"`), which is stripped before parsing
///
/// Anything else — including double prefixes (`"4k7n"`) — is a loud, typed
/// [`ValueParseError`]. No guessing.
pub fn parse_si_value(raw: &str, unit_letters: &[&str]) -> Result<f64, ValueParseError> {
    let mut s = raw.trim();
    if s.is_empty() {
        return Err(ValueParseError::Empty);
    }
    // Strip one trailing unit word (longest match first), case-insensitively —
    // but never let the unit strip eat a lone SI prefix ambiguity: units are
    // whole words like "V", "F", "H", "ohm", "Ω", so strip only when what
    // remains still contains a digit.
    let mut units: Vec<&str> = unit_letters.to_vec();
    units.sort_by_key(|u| std::cmp::Reverse(u.len()));
    for unit in units {
        let sl = s.to_lowercase();
        let ul = unit.to_lowercase();
        if sl.ends_with(&ul) {
            let cut = s.len() - ul.len();
            if s.is_char_boundary(cut) {
                let head = s[..cut].trim_end();
                if head.chars().any(|c| c.is_ascii_digit()) {
                    s = &s[..cut];
                    s = s.trim_end();
                    break;
                }
            }
        }
    }

    // Split into leading numeric-ish part and trailing suffix.
    let is_numeric = |c: char| c.is_ascii_digit() || c == '.' || c == '+' || c == '-';
    // Handle scientific notation by trying full-string parse first.
    if let Ok(v) = s.parse::<f64>() {
        return Ok(v);
    }

    // Infix notation: digits [R or SI prefix] digits, e.g. 4R7 / 4k7 / 2n2.
    let chars: Vec<char> = s.chars().collect();
    let infix_pos = chars.iter().position(|&c| !is_numeric(c)).filter(|&i| {
        i > 0 && i + 1 < chars.len() && chars[i + 1..].iter().all(|c| c.is_ascii_digit())
    });
    if let Some(i) = infix_pos {
        let ch = chars[i];
        let mult = if ch == 'R' || ch == 'r' {
            Some(1.0)
        } else {
            si_multiplier(ch)
        };
        if let Some(mult) = mult {
            let int_part: String = chars[..i].iter().collect();
            let frac_part: String = chars[i + 1..].iter().collect();
            let composed = format!("{int_part}.{frac_part}");
            return composed
                .parse::<f64>()
                .map(|v| v * mult)
                .map_err(|_| ValueParseError::BadNumber(raw.to_string()));
        }
    }

    // Suffix notation: number then exactly one SI prefix (or "R" for ohms).
    let num_end = chars
        .iter()
        .position(|&c| !is_numeric(c) && c != 'e' && c != 'E')
        .unwrap_or(chars.len());
    // Re-scan: accept scientific notation inside the head.
    let head: String = chars[..num_end].iter().collect();
    let tail: String = chars[num_end..].iter().collect();
    let value: f64 = head
        .parse()
        .map_err(|_| ValueParseError::BadNumber(raw.to_string()))?;
    let tail = tail.trim();
    if tail.is_empty() {
        return Ok(value);
    }
    let mut tail_chars = tail.chars();
    let first = tail_chars.next().expect("non-empty");
    let rest: String = tail_chars.collect();
    if !rest.is_empty() {
        // A second alphabetic char after the prefix means either a double
        // prefix (ambiguous) or an unknown unit that survived stripping.
        if rest.chars().any(|c| si_multiplier(c).is_some()) {
            return Err(ValueParseError::Ambiguous(raw.to_string()));
        }
        return Err(ValueParseError::BadSuffix {
            value: raw.to_string(),
            suffix: tail.to_string(),
        });
    }
    if first == 'R' || first == 'r' {
        return Ok(value);
    }
    match si_multiplier(first) {
        Some(m) => Ok(value * m),
        None => Err(ValueParseError::BadSuffix {
            value: raw.to_string(),
            suffix: tail.to_string(),
        }),
    }
}

// ============================================================================
// Conversion errors (fail closed)
// ============================================================================

/// Why one specific component blocked the conversion.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BlockerReason {
    /// The refdes prefix doesn't map to any simulatable device (ICs,
    /// connectors, …).
    #[error("no device mapping for refdes prefix {prefix:?} (ICs/connectors are not simulatable at M1; stub via `stub_as_open` if it doesn't matter electrically)")]
    UnknownKind {
        /// The unrecognized refdes prefix (leading letters).
        prefix: String,
    },
    /// The value string didn't parse.
    #[error("value {value:?} did not parse: {source}")]
    BadValue {
        /// The raw value string.
        value: String,
        /// The parse failure.
        source: ValueParseError,
    },
    /// The parsed value is non-physical for the device kind (≤ 0 for R/C/L).
    #[error("value {value} is out of range for this device kind")]
    OutOfRange {
        /// The parsed numeric value.
        value: f64,
    },
    /// The component doesn't expose the pins the mapping needs.
    #[error("expected pins {expected:?}, found {found:?}")]
    BadPins {
        /// Pin numbers the mapping requires.
        expected: Vec<String>,
        /// Pin numbers actually present in the netlist for this component.
        found: Vec<String>,
    },
}

/// One component that blocked simulation, with its reason.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentBlocker {
    /// Reference designator (e.g. `"U3"`).
    pub reference: String,
    /// Why it blocked.
    pub reason: BlockerReason,
}

impl std::fmt::Display for ComponentBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reference, self.reason)
    }
}

/// Conversion failure. Fail-closed: if *any* component can't be mapped, the
/// whole conversion fails and every blocker is listed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConvertError {
    /// One or more components blocked simulation.
    #[error("{} component(s) blocked simulation: {}", blockers.len(), blockers.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("; "))]
    Unmappable {
        /// Every blocking component (not just the first).
        blockers: Vec<ComponentBlocker>,
    },
    /// A name in `stub_as_open` matches no component in the schematic —
    /// refusing to silently ignore it (it's probably a typo).
    #[error("stub_as_open names unknown component(s): {0:?}")]
    UnknownStub(Vec<String>),
}

// ============================================================================
// Conversion
// ============================================================================

/// Options for [`circuit_from_netlist`].
#[derive(Debug, Clone, Default)]
pub struct ConvertOptions {
    /// Reference designators to stub as **open circuits**: the named
    /// components contribute no device (their pins still anchor their nets).
    /// The explicit escape hatch for components that don't matter
    /// electrically (connectors, test points, an unpopulated IC).
    pub stub_as_open: Vec<String>,
}

/// A [`Circuit`] mapped from a schematic, with the name↔id bookkeeping a
/// caller needs to pose questions ("what's the voltage on net OUT?").
#[derive(Debug, Clone)]
pub struct MappedCircuit {
    /// The simulatable circuit.
    pub circuit: Circuit,
    /// Net name → circuit node id. Ground-like nets all map to 0.
    pub node_of_net: BTreeMap<String, usize>,
    /// Reference designator → device id in [`Circuit::devices`].
    pub device_of_ref: BTreeMap<String, usize>,
    /// Ground-like net names that were collapsed onto node 0.
    pub ground_nets: Vec<String>,
    /// Components stubbed as opens per [`ConvertOptions::stub_as_open`].
    pub stubbed: Vec<String>,
}

/// Kinds the refdes prefix can map to.
enum Kind {
    Resistor,
    Capacitor,
    Inductor,
    VSource,
    ISource,
    Diode,
}

fn classify(reference: &str) -> Option<Kind> {
    let prefix: String = reference
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    match prefix.to_ascii_uppercase().as_str() {
        "R" => Some(Kind::Resistor),
        "C" => Some(Kind::Capacitor),
        "L" => Some(Kind::Inductor),
        "V" => Some(Kind::VSource),
        "I" => Some(Kind::ISource),
        "D" => Some(Kind::Diode),
        _ => None,
    }
}

/// The slice of a schematic component the converter needs: refdes + value.
/// Pin connectivity comes from the [`Netlist`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimComponent {
    /// Reference designator (e.g. `"R1"`).
    pub reference: String,
    /// Value string (e.g. `"4.7k"`, `"100n"`, `"5"`).
    pub value: String,
}

/// Convert a full schematic sheet to a simulatable circuit: extract the
/// netlist ([`generate_netlist`] — explicit `nets` map and wire geometry both
/// work) and map it via [`circuit_from_netlist`].
pub fn circuit_from_schematic(
    sheet: &SchematicSheet,
    options: &ConvertOptions,
) -> Result<MappedCircuit, ConvertError> {
    let netlist = generate_netlist(sheet);
    let components: Vec<SimComponent> = sheet
        .components
        .iter()
        .map(|c| SimComponent {
            reference: c.reference.clone(),
            value: c.value.clone(),
        })
        .collect();
    circuit_from_netlist(&components, &netlist, options)
}

/// Map components + an extracted [`Netlist`] to a [`Circuit`] under the
/// component-mapping convention (see module docs). Fail-closed: every
/// unmappable component is reported; nothing is skipped silently.
pub fn circuit_from_netlist(
    components: &[SimComponent],
    netlist: &Netlist,
    options: &ConvertOptions,
) -> Result<MappedCircuit, ConvertError> {
    // Validate the stub allowlist against real refs first — a typo there must
    // not silently un-stub anything.
    let known_refs: std::collections::BTreeSet<&str> =
        components.iter().map(|c| c.reference.as_str()).collect();
    let unknown_stubs: Vec<String> = options
        .stub_as_open
        .iter()
        .filter(|s| !known_refs.contains(s.as_str()))
        .cloned()
        .collect();
    if !unknown_stubs.is_empty() {
        return Err(ConvertError::UnknownStub(unknown_stubs));
    }
    let stub_set: std::collections::BTreeSet<&str> =
        options.stub_as_open.iter().map(|s| s.as_str()).collect();

    // Per-component pin → net-name map from the netlist.
    let mut net_of_pin: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for net in &netlist.nets {
        for conn in &net.connections {
            net_of_pin.insert(
                (conn.component_ref.as_str(), conn.pin_number.as_str()),
                net.name.as_str(),
            );
        }
    }

    // Resolve one component's terminal pair to net names, trying pin-name
    // pairs in order (e.g. ("1","2") then ("+","-")). Nodes are allocated
    // afterwards, and only for nets a mapped device actually touches — a
    // floating net (e.g. the pins of a stubbed connector) must not become a
    // dangling MNA node (that would make the system singular).
    let net_of =
        |reference: &str, pin: &str| -> Option<&str> { net_of_pin.get(&(reference, pin)).copied() };
    let found_pins = |reference: &str| -> Vec<String> {
        net_of_pin
            .keys()
            .filter(|(r, _)| *r == reference)
            .map(|(_, p)| p.to_string())
            .collect()
    };
    let resolve_pair =
        |reference: &str, pairs: &[(&str, &str)]| -> Result<(&str, &str), BlockerReason> {
            for (a, b) in pairs {
                if let (Some(p), Some(n)) = (net_of(reference, a), net_of(reference, b)) {
                    return Ok((p, n));
                }
            }
            Err(BlockerReason::BadPins {
                expected: pairs
                    .iter()
                    .flat_map(|(a, b)| [a.to_string(), b.to_string()])
                    .collect(),
                found: found_pins(reference),
            })
        };

    let mut blockers: Vec<ComponentBlocker> = Vec::new();
    let mut stubbed: Vec<String> = Vec::new();
    // Phase A: resolve every component to (kind, value, net names).
    struct Planned<'a> {
        reference: &'a str,
        kind: Kind,
        value: f64,
        value_str: &'a str,
        net_p: &'a str,
        net_n: &'a str,
    }
    let mut planned: Vec<Planned> = Vec::new();

    for comp in components {
        let reference = comp.reference.as_str();
        if stub_set.contains(reference) {
            stubbed.push(reference.to_string());
            continue;
        }
        let mut block = |reason: BlockerReason| {
            blockers.push(ComponentBlocker {
                reference: reference.to_string(),
                reason,
            });
        };
        let Some(kind) = classify(reference) else {
            let prefix: String = reference
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            block(BlockerReason::UnknownKind { prefix });
            continue;
        };

        // Value parse (diodes carry a model name, not a number).
        let (unit_words, needs_positive): (&[&str], bool) = match kind {
            Kind::Resistor => (&["ohm", "Ω", "R"], true),
            Kind::Capacitor => (&["F"], true),
            Kind::Inductor => (&["H"], true),
            Kind::VSource => (&["V"], false),
            Kind::ISource => (&["A"], false),
            Kind::Diode => (&[], false),
        };
        let value = if matches!(kind, Kind::Diode) {
            0.0
        } else {
            match parse_si_value(&comp.value, unit_words) {
                Ok(v) => {
                    if needs_positive && v <= 0.0 {
                        block(BlockerReason::OutOfRange { value: v });
                        continue;
                    }
                    v
                }
                Err(source) => {
                    block(BlockerReason::BadValue {
                        value: comp.value.clone(),
                        source,
                    });
                    continue;
                }
            }
        };

        let pin_pairs: &[(&str, &str)] = match kind {
            Kind::Diode => &[("A", "K"), ("1", "2")],
            Kind::VSource | Kind::ISource => &[("1", "2"), ("+", "-"), ("P", "N")],
            _ => &[("1", "2")],
        };
        let (p, n) = match resolve_pair(reference, pin_pairs) {
            Ok(pn) => pn,
            Err(reason) => {
                block(reason);
                continue;
            }
        };

        planned.push(Planned {
            reference,
            kind,
            value,
            value_str: comp.value.as_str(),
            net_p: p,
            net_n: n,
        });
    }

    if !blockers.is_empty() {
        return Err(ConvertError::Unmappable { blockers });
    }

    // Phase B: allocate nodes only for nets a mapped device touches
    // (deterministic sorted order), then emit devices.
    let mut used_nets: Vec<&str> = planned.iter().flat_map(|d| [d.net_p, d.net_n]).collect();
    used_nets.sort_unstable();
    used_nets.dedup();

    let mut circuit = Circuit::new();
    let mut node_of_net: BTreeMap<String, usize> = BTreeMap::new();
    let mut ground_nets: Vec<String> = Vec::new();
    for name in used_nets {
        let node = if is_ground_net(name) {
            ground_nets.push(name.to_string());
            0
        } else {
            circuit.node()
        };
        node_of_net.insert(name.to_string(), node);
    }

    let mut device_of_ref: BTreeMap<String, usize> = BTreeMap::new();
    for d in planned {
        let (p, n) = (node_of_net[d.net_p], node_of_net[d.net_n]);
        let device = match d.kind {
            Kind::Resistor => Device::Resistor { p, n, r: d.value },
            Kind::Capacitor => Device::Capacitor { p, n, c: d.value },
            Kind::Inductor => Device::Inductor { p, n, l: d.value },
            Kind::VSource => Device::VSource { p, n, v: d.value },
            Kind::ISource => Device::ISource { p, n, i: d.value },
            Kind::Diode => {
                let model = if d.value_str.to_ascii_uppercase().contains("LED") {
                    DiodeModel::led()
                } else {
                    DiodeModel::silicon()
                };
                Device::Diode { p, n, model }
            }
        };
        let id = circuit.add(device);
        device_of_ref.insert(d.reference.to_string(), id);
    }

    Ok(MappedCircuit {
        circuit,
        node_of_net,
        device_of_ref,
        ground_nets,
        stubbed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn si_values_parse() {
        let cases: &[(&str, f64)] = &[
            ("10", 10.0),
            ("4.7k", 4_700.0),
            ("4R7", 4.7),
            ("4k7", 4_700.0),
            ("2n2", 2.2e-9),
            ("100n", 1e-7),
            ("2.2u", 2.2e-6),
            ("3.3µ", 3.3e-6),
            ("10m", 1e-2),
            ("2M", 2e6),
            ("1G", 1e9),
            ("1p", 1e-12),
            ("1e-6", 1e-6),
            ("5V", 5.0),
            ("100nF", 1e-7),
            ("10kΩ", 10_000.0),
            ("10 k", 10_000.0),
        ];
        for (s, want) in cases {
            let units: &[&str] = &["V", "F", "ohm", "Ω"];
            let got = parse_si_value(s, units).unwrap_or_else(|e| panic!("{s:?}: {e}"));
            assert!(
                (got - want).abs() <= 1e-12 * want.abs().max(1.0),
                "{s:?} parsed to {got}, want {want}"
            );
        }
    }

    #[test]
    fn milli_vs_mega_is_case_sensitive() {
        assert_eq!(parse_si_value("1m", &[]).unwrap(), 1e-3);
        assert_eq!(parse_si_value("1M", &[]).unwrap(), 1e6);
        assert_eq!(parse_si_value("1k", &[]).unwrap(), 1e3);
        assert_eq!(parse_si_value("1K", &[]).unwrap(), 1e3);
    }

    #[test]
    fn bad_values_are_loud() {
        assert!(matches!(
            parse_si_value("", &[]),
            Err(ValueParseError::Empty)
        ));
        assert!(matches!(
            parse_si_value("abc", &[]),
            Err(ValueParseError::BadNumber(_))
        ));
        assert!(matches!(
            parse_si_value("4k7n", &[]),
            Err(ValueParseError::Ambiguous(_))
        ));
        assert!(matches!(
            parse_si_value("10x", &[]),
            Err(ValueParseError::BadSuffix { .. })
        ));
    }

    #[test]
    fn ground_names_recognized() {
        for g in ["GND", "gnd", "AGND", "0", "0V", "VSS"] {
            assert!(is_ground_net(g), "{g} should be ground");
        }
        assert!(!is_ground_net("VCC"));
        assert!(!is_ground_net("OUT"));
    }
}
