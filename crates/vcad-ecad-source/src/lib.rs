//! Optional sourcing leaf — stock/price/datasheet for a manufacturer part.
//!
//! This crate is *structurally amputable*: nothing in the durable core
//! (`vcad-ecad-package`/`-parts`/`-verify`/`-pcb`) depends on it, and the live
//! adapter is gated behind the off-by-default `live` feature. With `live` off
//! the crate is fully offline — [`source_part`] returns only what a compiled-in
//! datasheet cross-reference knows (no stock, no price), so a design's
//! verification never depends on a sourcing service being reachable.

#![warn(missing_docs)]

use vcad_ir::ecad::{SourcingLine, SourcingSnapshot};

/// Sourcing information for a single manufacturer part number.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcingInfo {
    /// Manufacturer part number.
    pub mpn: String,
    /// Manufacturer name, if known.
    pub manufacturer: Option<String>,
    /// Datasheet URL, if known.
    pub datasheet: Option<String>,
    /// Stock quantity (only a live provider supplies this).
    pub stock: Option<u32>,
    /// Unit price (only a live provider supplies this).
    pub unit_price: Option<f64>,
    /// Currency code for `unit_price`.
    pub currency: Option<String>,
    /// Which provider produced this record (e.g. "offline", "live").
    pub source: String,
}

impl SourcingInfo {
    /// True when the record carries live availability (stock or price).
    pub fn is_live(&self) -> bool {
        self.stock.is_some() || self.unit_price.is_some()
    }

    /// Convert to an IR [`SourcingLine`] for embedding in a Receipt.
    pub fn to_line(&self) -> SourcingLine {
        SourcingLine {
            mpn: self.mpn.clone(),
            stock: self.stock,
            unit_price: self.unit_price,
            currency: self.currency.clone(),
        }
    }
}

/// A source of part availability/pricing.
pub trait SourcingProvider {
    /// Provider name (recorded on each [`SourcingInfo`]).
    fn name(&self) -> &str;
    /// Look up a part by MPN. Returns `None` when the provider has no record.
    fn lookup(&self, mpn: &str) -> Option<SourcingInfo>;
}

/// The default offline provider: a tiny compiled-in datasheet cross-reference.
/// No stock or price — those require a live provider.
pub struct OfflineProvider;

impl SourcingProvider for OfflineProvider {
    fn name(&self) -> &str {
        "offline"
    }

    fn lookup(&self, mpn: &str) -> Option<SourcingInfo> {
        // Sparse, durable, hand-curated bridge — never a scraped DB.
        let (manufacturer, datasheet) = match mpn {
            "RC0603FR-0710KL" => (
                "Yageo",
                "https://www.yageo.com/upload/media/product/productsearch/datasheet/rchip/PYu-RC_Group_51_RoHS_L_12.pdf",
            ),
            "CL05B104KO5NNNC" => (
                "Samsung",
                "https://product.samsungsem.com/mlcc/CL05B104KO5NNNC.do",
            ),
            _ => return None,
        };
        Some(SourcingInfo {
            mpn: mpn.to_string(),
            manufacturer: Some(manufacturer.to_string()),
            datasheet: Some(datasheet.to_string()),
            stock: None,
            unit_price: None,
            currency: None,
            source: "offline".to_string(),
        })
    }
}

/// A live distributor adapter (opt-in via the `live` feature).
///
/// This is a scaffold: it carries the configuration a real adapter would need
/// and reports whether it is configured, but performs no network I/O here. A
/// real implementation would issue an HTTP request to the configured endpoint.
#[cfg(feature = "live")]
pub struct LiveProvider {
    /// API key for the distributor, if configured.
    pub api_key: Option<String>,
    /// Provider label (e.g. "octopart").
    pub label: String,
}

#[cfg(feature = "live")]
impl SourcingProvider for LiveProvider {
    fn name(&self) -> &str {
        &self.label
    }

    fn lookup(&self, _mpn: &str) -> Option<SourcingInfo> {
        // Without a key, a live provider cannot answer — degrade silently so
        // the caller falls back to offline.
        self.api_key.as_ref()?;
        // A real adapter performs the network request here. The scaffold has no
        // I/O, so it returns None (no record) rather than fabricating data.
        None
    }
}

/// Look up a part, preferring `provider`, degrading to the offline datasheet
/// cross-reference when the provider has no record. Always returns *something*
/// for a known MPN (at minimum datasheet/manufacturer), with a note via the
/// `source` field; returns `None` only when nothing knows the MPN.
pub fn source_part(mpn: &str, provider: &dyn SourcingProvider) -> Option<SourcingInfo> {
    if let Some(info) = provider.lookup(mpn) {
        return Some(info);
    }
    OfflineProvider.lookup(mpn)
}

/// Build an IR [`SourcingSnapshot`] from a set of MPNs using `provider`.
/// Unknown MPNs are skipped. The snapshot is informational — it never gates a
/// [`vcad_ir::ecad::Receipt`]'s DRC verdict.
pub fn snapshot<'a>(
    mpns: impl IntoIterator<Item = &'a str>,
    provider: &dyn SourcingProvider,
) -> SourcingSnapshot {
    let lines = mpns
        .into_iter()
        .filter_map(|m| source_part(m, provider))
        .map(|i| i.to_line())
        .collect();
    SourcingSnapshot { lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_returns_datasheet_only() {
        let info = source_part("RC0603FR-0710KL", &OfflineProvider).unwrap();
        assert_eq!(info.manufacturer.as_deref(), Some("Yageo"));
        assert!(info.datasheet.is_some());
        // Offline never carries live availability.
        assert!(!info.is_live());
        assert_eq!(info.source, "offline");
    }

    #[test]
    fn unknown_mpn_is_none() {
        assert!(source_part("NO-SUCH-PART-123", &OfflineProvider).is_none());
    }

    #[test]
    fn snapshot_skips_unknowns() {
        let snap = snapshot(
            ["RC0603FR-0710KL", "NO-SUCH-PART", "CL05B104KO5NNNC"],
            &OfflineProvider,
        );
        assert_eq!(snap.lines.len(), 2);
        // No live stock/price offline.
        assert!(snap
            .lines
            .iter()
            .all(|l| l.stock.is_none() && l.unit_price.is_none()));
    }

    #[cfg(feature = "live")]
    #[test]
    fn unconfigured_live_provider_degrades_to_offline() {
        let live = LiveProvider {
            api_key: None,
            label: "octopart".into(),
        };
        // No key → provider answers nothing → falls back to offline datasheet.
        let info = source_part("RC0603FR-0710KL", &live).unwrap();
        assert_eq!(info.source, "offline");
    }
}
