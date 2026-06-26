//! Re-runnable verification receipts.
//!
//! A [`Receipt`] is the durable proof that a board was checked: a content hash
//! of the board, the DRC backend that ran, a canonicalized summary of the
//! violations found, and per-part provenance. It round-trips in `.vcad` so the
//! claim "this board is DRC-clean and these are the parts" survives any service
//! dying — re-running it against the current board yields [`ReceiptStatus`].
//!
//! Geometric/DRC drift is reported separately from sourcing drift: a price
//! change must never read as an electrical failure.

use serde::{Deserialize, Serialize};

/// The result of re-running a receipt against the current board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum ReceiptStatus {
    /// The board is unchanged and the DRC result still matches the receipt.
    Holds,
    /// The board changed since the receipt, but no new violations appeared.
    Stale,
    /// New DRC violations exist that the receipt did not record (a regression).
    Violated,
}

/// Count of violations of one rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct RuleCount {
    /// Rule name (e.g. "Clearance", "CourtyardOverlap").
    pub rule: String,
    /// Number of violations of this rule.
    pub count: u32,
}

/// A canonicalized DRC summary: total, per-rule counts (sorted), and the
/// ordered set of violation keys, so the receipt hashes deterministically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct DrcSummary {
    /// Total violation count.
    pub total: u32,
    /// Per-rule counts, sorted by rule name.
    pub by_rule: Vec<RuleCount>,
    /// Canonical violation keys (sorted) — `rule|message|x|y`.
    pub violations: Vec<String>,
}

/// Provenance for one placed part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct PartReceiptLine {
    /// Reference designator.
    pub reference: String,
    /// Footprint name.
    pub footprint: String,
    /// Component value.
    pub value: String,
    /// Manufacturer part number, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub mpn: Option<String>,
}

/// A single sourcing line captured at receipt-build time (optional leaf).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct SourcingLine {
    /// Manufacturer part number.
    pub mpn: String,
    /// Stock quantity, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub stock: Option<u32>,
    /// Unit price, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub unit_price: Option<f64>,
    /// Currency code (e.g. "USD"), if a price is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub currency: Option<String>,
}

/// Realized-copper continuity for one power/plane net, captured in a receipt.
///
/// A split power plane is an electrically *open* PDN even when DRC reports zero
/// clearance/short violations, so the receipt records continuity explicitly
/// rather than letting a closed-form sizing PASS imply a healthy plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct PowerIntegrityLine {
    /// Net name.
    pub net: String,
    /// Number of disjoint galvanic islands the net's copper forms (1 = sound).
    pub islands: u32,
    /// True when the net's copper forms a single continuous island.
    pub continuous: bool,
    /// Pads reaching the main island / total pads, in `[0, 1]`.
    pub coverage: f64,
    /// Pads on the net's main (largest) island.
    pub connected_pads: u32,
    /// Total pads assigned to the net.
    pub total_pads: u32,
    /// Stitching vias on the net.
    pub vias: u32,
}

/// A sourcing snapshot — informational, never gates the DRC verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct SourcingSnapshot {
    /// The captured sourcing lines.
    pub lines: Vec<SourcingLine>,
}

/// A re-runnable verification receipt for a board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct Receipt {
    /// Content hash of the DRC-relevant board geometry (hex).
    pub board_hash: String,
    /// Content hash of the design rules (hex).
    pub design_rules_hash: String,
    /// The DRC backend + version that produced this receipt.
    pub drc_backend: String,
    /// The canonicalized DRC summary.
    pub drc: DrcSummary,
    /// Realized-copper continuity for power/plane nets. Empty when the board has
    /// none. A `continuous: false` line means a sized/closed-form PDN verdict
    /// for that net is unverifiable — the plane is electrically open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub power_integrity: Vec<PowerIntegrityLine>,
    /// Per-part provenance.
    pub parts: Vec<PartReceiptLine>,
    /// Optional sourcing snapshot (separate from the DRC verdict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub sourcing: Option<SourcingSnapshot>,
}
