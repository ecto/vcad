//! Print-then-measure calibration — the 3DP receipt-vs-reality delta engine.
//!
//! The user's own printer is the one fab rail with zero vendor dependency, so
//! every print can capture a (predicted, measured) pair for near-zero cost.
//! This crate owns the pure computation of that loop: a [`PrintPrediction`]
//! snapshot taken before printing, a set of caliper/scale measurements taken
//! after, and the [`CalibrationReport`] that joins them — per-feature deltas
//! plus the aggregates a printer profile can actually act on (axis scale
//! factors, hole undersize, wall/flow offset).
//!
//! Ported from `packages/core/src/utils/print-calibration.ts`, which is now a
//! thin wrapper over the WASM bindings. Output is pinned bit-for-bit against
//! the original TypeScript on the fixtures in `tests/ts_fixtures.json`:
//! rounding uses JS `Math.round` semantics (half toward +∞) and the document
//! fingerprint reproduces `JSON.stringify` number/string formatting and the
//! UTF-16 fnv1a-128 quad hash from `receipt/hash.ts`.
//!
//! No I/O, no kernel, no clock — same layering as the receipt engine next
//! door. The MCP tools (`predict_print`, `record_measurement`) wrap this via
//! `vcad-kernel-wasm`.
//!
//! Design doc: docs/plans/2026-07-07-3dp-print-then-measure.md

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Hash algorithm identifier baked into every report (matches
/// `receipt/hash.ts`).
pub const HASH_ALGO: &str = "fnv1a-128";

/// What kind of physical quantity a measurable is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasurableKind {
    /// A linear dimension (span, step, wall thickness).
    Dimension,
    /// A hole or boss diameter.
    Diameter,
    /// A mass reading from a scale.
    Mass,
}

/// Print-frame axis a linear measurable lies along (`XY` = in-plane, e.g. a
/// hole diameter — holes and bosses deform isotropically in the layer plane).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasurableAxis {
    /// Along the print-frame X axis.
    X,
    /// Along the print-frame Y axis.
    Y,
    /// Along the print-frame Z axis (layer stacking).
    Z,
    /// In-plane isotropic (hole/boss diameters).
    XY,
}

/// Aggregation bucket for a measurable — which calibration signature it
/// feeds. `hole` and `wall` power the offset aggregates; `overall`/`step`/
/// `boss` participate only in the axis scale fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasurableFeature {
    /// Overall bounding-box span.
    Overall,
    /// A step height on a staircase feature.
    Step,
    /// A hole diameter (feeds `hole_offset_mm`, excluded from scale fits).
    Hole,
    /// A boss diameter (excluded from scale fits).
    Boss,
    /// A thin wall (feeds `wall_offset_mm`, excluded from scale fits).
    Wall,
}

/// Measurement unit of a measurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeasureUnit {
    /// Millimeters.
    Mm,
    /// Grams.
    G,
}

/// One thing the human will measure on the printed part.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurable {
    /// Stable id joining prediction to measurement (e.g. "hole_3mm").
    pub id: String,
    /// Human instruction — what to measure and where.
    pub label: String,
    /// Physical quantity kind.
    pub kind: MeasurableKind,
    /// Required for dimensions/diameters; meaningless for mass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis: Option<MeasurableAxis>,
    /// Aggregation bucket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<MeasurableFeature>,
    /// Design-intent value in `unit`.
    pub predicted: f64,
    /// Unit of `predicted` (and of the measured value).
    pub unit: MeasureUnit,
    /// ± tolerance in `unit`; defaulted per kind when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
}

/// Material the prediction assumed (for the mass measurable).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PredictionMaterial {
    /// Material name (e.g. "PLA").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Density assumed for the mass prediction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub density_kg_m3: Option<f64>,
}

/// Bounding box of the predicted part, mm.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BboxMm {
    /// X span.
    pub x: f64,
    /// Y span.
    pub y: f64,
    /// Z span.
    pub z: f64,
}

/// The pre-print snapshot of everything the design claims to be.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintPrediction {
    /// Schema version (always 1).
    pub version: u32,
    /// Session document id, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// fnv1a-128 of the canonicalized document IR — staleness detection.
    pub doc_fingerprint: String,
    /// ISO timestamp the prediction was taken.
    pub created_at: String,
    /// Material assumption for the mass row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<PredictionMaterial>,
    /// Kernel-evaluated solid volume.
    pub volume_mm3: f64,
    /// Bounding box of the part.
    pub bbox_mm: BboxMm,
    /// Honest caveats ("mass assumes 100% infill", …).
    pub assumptions: Vec<String>,
    /// The measurement worksheet.
    pub measurables: Vec<Measurable>,
}

/// Where and how the physical part was made and measured.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeasurementContext {
    /// Printer make/model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printer: Option<String>,
    /// Material actually printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// Free-form process notes: layer height, infill, temperature…
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    /// ISO timestamp of the physical measurement session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<String>,
}

/// One joined (predicted, measured) row of the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaRow {
    /// Measurable id.
    pub id: String,
    /// Human label.
    pub label: String,
    /// Physical quantity kind.
    pub kind: MeasurableKind,
    /// Axis, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis: Option<MeasurableAxis>,
    /// Feature bucket, when declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<MeasurableFeature>,
    /// Design-intent value.
    pub predicted: f64,
    /// Measured value.
    pub measured: f64,
    /// Unit of both values.
    pub unit: MeasureUnit,
    /// measured − predicted.
    pub delta: f64,
    /// 100 · delta / predicted.
    pub delta_pct: f64,
    /// ± tolerance applied.
    pub tolerance: f64,
    /// |delta| ≤ tolerance.
    pub within_tolerance: bool,
}

/// Least-squares fit measured ≈ scale·predicted over one axis's dimensions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AxisScale {
    /// Which axis the fit covers.
    pub axis: MeasurableAxis,
    /// Number of rows in the fit.
    pub n: usize,
    /// The fitted scale factor.
    pub scale: f64,
}

/// Overall report classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalibrationVerdict {
    /// Every row within tolerance.
    Pass,
    /// Some rows out of tolerance.
    Attention,
    /// A gross (>3×) error, or a majority of rows out of tolerance.
    Fail,
}

/// Predicted-vs-measured mass aggregate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MassAggregate {
    /// Predicted mass, grams.
    pub predicted_g: f64,
    /// Measured mass, grams.
    pub measured_g: f64,
    /// 100 · (measured − predicted) / predicted.
    pub delta_pct: f64,
}

/// The aggregates a printer profile can act on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Aggregates {
    /// Per-axis least-squares scale fits.
    pub axis_scales: Vec<AxisScale>,
    /// Mean(measured − predicted) over hole diameters, mm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hole_offset_mm: Option<f64>,
    /// Mean(measured − predicted) over thin walls, mm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_offset_mm: Option<f64>,
    /// Mass aggregate, when a mass row was measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass: Option<MassAggregate>,
}

/// The receipt-vs-reality delta artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationReport {
    /// Schema version (always 1).
    pub version: u32,
    /// Session document id, when the prediction carried one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// Fingerprint the prediction was taken against.
    pub doc_fingerprint: String,
    /// True when the document changed between prediction and recording.
    pub stale: bool,
    /// When the prediction was taken.
    pub prediction_created_at: String,
    /// When the measurement was recorded.
    pub recorded_at: String,
    /// Where and how the part was made and measured.
    pub context: MeasurementContext,
    /// Joined (predicted, measured) rows.
    pub rows: Vec<DeltaRow>,
    /// Measurable ids the human never measured.
    pub missing: Vec<String>,
    /// Measurement ids that match no measurable.
    pub unknown: Vec<String>,
    /// Profile-actionable aggregates.
    pub aggregates: Aggregates,
    /// Printer-profile corrections derived from the aggregates.
    pub suggestions: Vec<String>,
    /// Overall classification.
    pub verdict: CalibrationVerdict,
    /// One-line human summary.
    pub summary: String,
    /// Hash algorithm of `doc_fingerprint`.
    #[serde(rename = "fingerprintAlgo")]
    pub fingerprint_algo: String,
}

/// Optional inputs for [`build_calibration_report`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReportOptions {
    /// Measurement context to embed verbatim.
    #[serde(default)]
    pub context: Option<MeasurementContext>,
    /// Fingerprint of the document as it exists NOW (for staleness).
    #[serde(default)]
    pub current_doc_fingerprint: Option<String>,
    /// ISO timestamp of the recording. This crate has no clock — callers
    /// stamp it (the TS wrapper defaults to `new Date().toISOString()`).
    /// Empty string when omitted.
    #[serde(default)]
    pub recorded_at: Option<String>,
}

// ── JS-compatible primitives ─────────────────────────────────────────────

/// `Math.round` semantics: half rounds toward +∞ (so −2.5 → −2, unlike
/// Rust's `f64::round`).
fn js_math_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// The TS module's `round(n, places)`: `Math.round(n · 10^p) / 10^p`.
fn round_places(n: f64, places: i32) -> f64 {
    let p = 10f64.powi(places);
    js_math_round(n * p) / p
}

fn round4(n: f64) -> f64 {
    round_places(n, 4)
}

/// Format an f64 the way ECMAScript `Number::toString` / `JSON.stringify`
/// does: shortest round-trip digits, no trailing `.0`, decimal notation for
/// 1e-6 ≤ |v| < 1e21, otherwise `d.ddde±k`.
fn js_number(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string(); // covers -0: JSON.stringify(-0) === "0"
    }
    if v.fract() == 0.0 && v.abs() < 1e21 {
        // Exactly representable integral value below the exponent threshold.
        return format!("{}", v as i128);
    }
    let mut buf = ryu::Buffer::new();
    let s = buf.format_finite(v);
    let (sign, s) = match s.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", s),
    };
    // Normalize ryu output to (digits, exp) with value = 0.digits · 10^exp.
    let (mantissa, e) = match s.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().expect("ryu exponent")),
        None => (s, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits: String = format!("{int_part}{frac_part}");
    let mut exp = e + int_part.len() as i32; // value = 0.digits · 10^exp
                                             // Strip leading zeros (e.g. "0.00123" → digits "000123").
    let lead = digits.len() - digits.trim_start_matches('0').len();
    digits = digits[lead..].to_string();
    exp -= lead as i32;
    // Strip trailing zeros (e.g. "1.0" → "10" → "1").
    digits = digits.trim_end_matches('0').to_string();
    let n = digits.len() as i32;
    let out = if (-5..=21).contains(&exp) {
        if exp >= n {
            // Integral with trailing zeros: digits followed by (exp − n) zeros.
            format!("{digits}{}", "0".repeat((exp - n) as usize))
        } else if exp > 0 {
            format!("{}.{}", &digits[..exp as usize], &digits[exp as usize..])
        } else {
            format!("0.{}{digits}", "0".repeat((-exp) as usize))
        }
    } else {
        // Exponent notation: d.ddd e (exp − 1), with explicit '+'.
        let e10 = exp - 1;
        let head = &digits[..1];
        let tail = &digits[1..];
        let m = if tail.is_empty() {
            head.to_string()
        } else {
            format!("{head}.{tail}")
        };
        format!("{m}e{}{}", if e10 >= 0 { "+" } else { "-" }, e10.abs())
    };
    format!("{sign}{out}")
}

/// Serialize a JSON value exactly like `JSON.stringify(canonicalize(v))` in
/// the original TS: object keys sorted recursively, JS number formatting.
fn canonical_stringify(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&js_number(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).expect("string serializes"));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_stringify(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).expect("key serializes"));
                out.push(':');
                canonical_stringify(&map[k], out);
            }
            out.push('}');
        }
    }
}

/// fnv1a-128 from `receipt/hash.ts`: four independent 32-bit FNV-1a-style
/// mixers over UTF-16 code units → 128 bits of hex. Not cryptographic.
pub fn hash_hex(s: &str) -> String {
    let mut h1: i32 = 0x811c9dc5u32 as i32;
    let mut h2: i32 = 0x9e3779b9u32 as i32;
    let mut h3: i32 = 0xdeadbeefu32 as i32;
    let mut h4: i32 = 0xcafebabeu32 as i32;
    for c in s.encode_utf16() {
        let c = c as i32;
        h1 = (h1 ^ c).wrapping_mul(0x01000193);
        h2 = (h2 ^ c).wrapping_mul(0x85ebca6bu32 as i32);
        h3 = (h3 ^ c).wrapping_mul(0xc2b2ae35u32 as i32);
        h4 = (h4 ^ c).wrapping_mul(0x27d4eb2f);
    }
    format!(
        "{:08x}{:08x}{:08x}{:08x}",
        h1 as u32, h2 as u32, h3 as u32, h4 as u32
    )
}

/// Content fingerprint of a document IR (or any JSON value). Deterministic
/// across key order; used to pair a measurement with the exact design it
/// measured. Matches the TS `fingerprintDocument` bit-for-bit.
pub fn fingerprint_document(doc: &Value) -> String {
    let mut s = String::new();
    canonical_stringify(doc, &mut s);
    hash_hex(&s)
}

/// Default ± tolerance for a measurable that doesn't declare one:
/// dimensions/diameters get a well-tuned-FDM envelope, mass gets 5%.
pub fn default_tolerance(kind: MeasurableKind, predicted: f64) -> f64 {
    if kind == MeasurableKind::Mass {
        return predicted.abs() * 0.05;
    }
    f64::max(0.1, predicted.abs() * 0.002)
}

fn fit_scale(pairs: &[(f64, f64)]) -> f64 {
    let mut num = 0.0;
    let mut den = 0.0;
    for (predicted, measured) in pairs {
        num += measured * predicted;
        den += predicted * predicted;
    }
    if den > 0.0 {
        num / den
    } else {
        1.0
    }
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Join a prediction with measured values into the delta report.
///
/// `measurements` is an ordered list of (measurable id, measured value) —
/// order matters only for the `unknown` list, which preserves it the way the
/// TS original preserved object insertion order. Non-finite / non-numeric
/// values count as missing. Ids present in only one side land in
/// `missing`/`unknown` rather than failing — a partial worksheet is still
/// data.
pub fn build_calibration_report(
    prediction: &PrintPrediction,
    measurements: &[(String, Value)],
    options: &ReportOptions,
) -> CalibrationReport {
    let context = options.context.clone().unwrap_or_default();
    let recorded_at = options.recorded_at.clone().unwrap_or_default();

    let mut rows: Vec<DeltaRow> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    let lookup: std::collections::HashMap<&str, f64> = measurements
        .iter()
        .filter_map(|(id, v)| {
            v.as_f64()
                .filter(|f| f.is_finite())
                .map(|f| (id.as_str(), f))
        })
        .collect();

    for m in &prediction.measurables {
        seen.insert(m.id.as_str());
        let Some(&value) = lookup.get(m.id.as_str()) else {
            missing.push(m.id.clone());
            continue;
        };
        let tolerance = m
            .tolerance
            .unwrap_or_else(|| default_tolerance(m.kind, m.predicted));
        let delta = value - m.predicted;
        rows.push(DeltaRow {
            id: m.id.clone(),
            label: m.label.clone(),
            kind: m.kind,
            axis: m.axis,
            feature: m.feature,
            predicted: m.predicted,
            measured: value,
            unit: m.unit,
            delta: round4(delta),
            delta_pct: if m.predicted != 0.0 {
                round4(100.0 * delta / m.predicted)
            } else {
                0.0
            },
            tolerance: round4(tolerance),
            within_tolerance: delta.abs() <= tolerance,
        });
    }

    let unknown: Vec<String> = measurements
        .iter()
        .filter(|(id, _)| !seen.contains(id.as_str()))
        .map(|(id, _)| id.clone())
        .collect();

    // ── Aggregates ───────────────────────────────────────────────────────
    // Axis scale fits use only span-like rows (overall, steps, undeclared).
    // Holes, bosses, and thin walls carry systematic process offsets
    // (undersize, over-extrusion) that have their own aggregates below —
    // letting them into the fit would misread flow error as shrinkage.
    let scale_excluded = |f: Option<MeasurableFeature>| {
        matches!(
            f,
            Some(MeasurableFeature::Hole | MeasurableFeature::Boss | MeasurableFeature::Wall)
        )
    };
    let mut axis_scales: Vec<AxisScale> = Vec::new();
    for axis in [
        MeasurableAxis::X,
        MeasurableAxis::Y,
        MeasurableAxis::Z,
        MeasurableAxis::XY,
    ] {
        let pairs: Vec<(f64, f64)> = rows
            .iter()
            .filter(|r| {
                r.kind != MeasurableKind::Mass
                    && r.axis == Some(axis)
                    && r.predicted > 0.0
                    && !scale_excluded(r.feature)
            })
            .map(|r| (r.predicted, r.measured))
            .collect();
        if pairs.is_empty() {
            continue;
        }
        axis_scales.push(AxisScale {
            axis,
            n: pairs.len(),
            scale: round_places(fit_scale(&pairs), 5),
        });
    }

    let hole_deltas: Vec<f64> = rows
        .iter()
        .filter(|r| {
            r.feature == Some(MeasurableFeature::Hole) && r.kind == MeasurableKind::Diameter
        })
        .map(|r| r.delta)
        .collect();
    let wall_deltas: Vec<f64> = rows
        .iter()
        .filter(|r| r.feature == Some(MeasurableFeature::Wall))
        .map(|r| r.delta)
        .collect();
    let mass_row = rows
        .iter()
        .find(|r| r.kind == MeasurableKind::Mass)
        .cloned();

    let aggregates = Aggregates {
        axis_scales: axis_scales.clone(),
        hole_offset_mm: (!hole_deltas.is_empty()).then(|| round4(mean(&hole_deltas))),
        wall_offset_mm: (!wall_deltas.is_empty()).then(|| round4(mean(&wall_deltas))),
        mass: mass_row.as_ref().map(|r| MassAggregate {
            predicted_g: r.predicted,
            measured_g: r.measured,
            delta_pct: r.delta_pct,
        }),
    };

    // ── Suggestions — the aggregates translated into profile knobs ───────
    let mut suggestions: Vec<String> = Vec::new();
    let in_plane: Vec<f64> = axis_scales
        .iter()
        .filter(|s| {
            matches!(
                s.axis,
                MeasurableAxis::X | MeasurableAxis::Y | MeasurableAxis::XY
            )
        })
        .map(|s| s.scale)
        .collect();
    if !in_plane.is_empty() {
        let s = mean(&in_plane);
        if (s - 1.0).abs() > 0.001 {
            let comp = round_places(100.0 / s, 2);
            suggestions.push(format!(
                "XY prints {} by {}% — set shrinkage/scale compensation to {}%",
                if s < 1.0 { "small" } else { "large" },
                js_number(round_places((1.0 - s).abs() * 100.0, 2)),
                js_number(comp),
            ));
        }
    }
    let z_scale = axis_scales
        .iter()
        .find(|s| s.axis == MeasurableAxis::Z)
        .copied();
    if let Some(z) = z_scale {
        if (z.scale - 1.0).abs() > 0.001 {
            suggestions.push(format!(
                "Z prints {} by {}% — check first-layer squish and Z scale compensation",
                if z.scale < 1.0 { "short" } else { "tall" },
                js_number(round_places((1.0 - z.scale).abs() * 100.0, 2)),
            ));
        }
    }
    if let Some(hole) = aggregates.hole_offset_mm {
        if hole < -0.05 {
            suggestions.push(format!(
                "holes print {}mm undersize — enable hole compensation or drill/ream to size",
                js_number(round_places(-hole, 2)),
            ));
        }
    }
    if let Some(wall) = aggregates.wall_offset_mm {
        if wall.abs() > 0.05 {
            suggestions.push(format!(
                "thin walls print {}mm {} the flow ratio",
                js_number(round_places(wall.abs(), 2)),
                if wall > 0.0 {
                    "thick — lower"
                } else {
                    "thin — raise"
                },
            ));
        }
    }
    if let Some(m) = &mass_row {
        if !m.within_tolerance {
            let density = prediction
                .material
                .as_ref()
                .and_then(|mat| mat.density_kg_m3)
                .map_or_else(|| "unknown".to_string(), js_number);
            suggestions.push(format!(
                "mass is {}% {} prediction — check infill density and material density (assumed {} kg/m³)",
                js_number(round_places(m.delta_pct.abs(), 1)),
                if m.delta > 0.0 { "over" } else { "under" },
                density,
            ));
        }
    }

    // ── Verdict ──────────────────────────────────────────────────────────
    let out_count = rows.iter().filter(|r| !r.within_tolerance).count();
    let gross = rows
        .iter()
        .any(|r| r.delta.abs() > 3.0 * r.tolerance && r.tolerance > 0.0);
    let mut verdict = CalibrationVerdict::Pass;
    if out_count > 0 {
        verdict = CalibrationVerdict::Attention;
    }
    if gross || (!rows.is_empty() && out_count as f64 > rows.len() as f64 / 2.0) {
        verdict = CalibrationVerdict::Fail;
    }

    let stale = options
        .current_doc_fingerprint
        .as_ref()
        .is_some_and(|fp| *fp != prediction.doc_fingerprint);

    let mut bits: Vec<String> = vec![format!(
        "{}/{} within tolerance",
        rows.len() - out_count,
        rows.len()
    )];
    if !in_plane.is_empty() {
        bits.push(format!(
            "XY scale {}%",
            js_number(round_places(mean(&in_plane) * 100.0, 2))
        ));
    }
    if let Some(z) = z_scale {
        bits.push(format!(
            "Z scale {}%",
            js_number(round_places(z.scale * 100.0, 2))
        ));
    }
    if let Some(hole) = aggregates.hole_offset_mm {
        bits.push(format!(
            "holes {}{}mm",
            if hole > 0.0 { "+" } else { "" },
            js_number(round_places(hole, 2))
        ));
    }
    if stale {
        bits.push("STALE — design changed after prediction".to_string());
    }

    CalibrationReport {
        version: 1,
        document_id: prediction.document_id.clone(),
        doc_fingerprint: prediction.doc_fingerprint.clone(),
        stale,
        prediction_created_at: prediction.created_at.clone(),
        recorded_at,
        context,
        rows,
        missing,
        unknown,
        aggregates,
        suggestions,
        verdict,
        summary: bits.join("; "),
        fingerprint_algo: HASH_ALGO.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_number_formats() {
        assert_eq!(js_number(0.0), "0");
        assert_eq!(js_number(-0.0), "0");
        assert_eq!(js_number(100.0), "100");
        assert_eq!(js_number(-0.225), "-0.225");
        assert_eq!(js_number(0.1), "0.1");
        assert_eq!(js_number(1.24e2), "124");
        assert_eq!(js_number(1e21), "1e+21");
        assert_eq!(js_number(1.2e-7), "1.2e-7");
        assert_eq!(js_number(0.000001), "0.000001");
        assert_eq!(js_number(1.2400000000000002), "1.2400000000000002");
    }

    #[test]
    fn js_round_negative_half_rounds_up() {
        assert_eq!(js_math_round(-2.5), -2.0);
        assert_eq!(js_math_round(2.5), 3.0);
    }

    #[test]
    fn default_tolerances() {
        assert_eq!(default_tolerance(MeasurableKind::Mass, 24.8), 24.8 * 0.05);
        assert_eq!(default_tolerance(MeasurableKind::Dimension, 80.0), 0.16);
        assert_eq!(default_tolerance(MeasurableKind::Dimension, 3.0), 0.1);
    }
}
