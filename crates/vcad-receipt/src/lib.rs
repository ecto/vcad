//! Unified verification receipts for vcad documents.
//!
//! A [`DesignReceipt`] is the machine-checkable proof that travels with a
//! `.vcad` document: a versioned list of [`ReceiptClaim`]s, each naming the
//! claim being made, the oracle (and oracle version) that checked it, the
//! predicted vs. measured values with explicit units, and a three-state
//! verdict.
//!
//! The house rule is **fail-closed**: an oracle that could not run reports
//! [`ClaimVerdict::Unverifiable`], which is never conflated with a clean
//! pass. Rolling up a receipt with [`DesignReceipt::overall`] propagates
//! that discipline — a receipt with no claims, or any unverifiable claim,
//! can never read as verified.
//!
//! Domain adapters translate existing per-domain oracle outputs into claims:
//! mechanical mass properties live in [`mechanical`]; the sheet-metal adapter
//! lives in `vcad-kernel-sheet::receipt` (next to the types it consumes); the
//! PCB adapter lives in the MCP server (TypeScript, over the generated types).
//!
//! Cryptographic signing is a planned follow-up: [`DesignReceipt::signature`]
//! reserves the field, nothing produces it yet.

#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

pub mod mechanical;

/// Current receipt schema identifier. Bump the trailing number on breaking
/// changes to the wire shape (same convention as the DFM packs' `vcad.dfm/1`).
pub const RECEIPT_SCHEMA: &str = "vcad.receipt/1";

/// Three-state claim verdict.
///
/// `Unverifiable` is load-bearing: it means the oracle could not check the
/// claim (missing inputs, engine unavailable, capped coverage). It is never
/// a pass and never silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum ClaimVerdict {
    /// The oracle ran and the claim holds.
    Pass,
    /// The oracle ran and the claim does not hold.
    Fail,
    /// The oracle could not check the claim. Not clean, not failed — unknown.
    Unverifiable,
}

/// The oracle that checked a claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct OracleRef {
    /// Stable oracle identifier, e.g. `"vcad-ecad-pcb/drc"`,
    /// `"mecheval-grade"`, `"vcad-kernel-sheet/manufacturability"`.
    pub id: String,
    /// Oracle version: crate version, rule-pack version, or backend build.
    pub version: String,
}

impl OracleRef {
    /// Construct an oracle reference.
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
        }
    }
}

/// A claim value: numeric, textual, or boolean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum ClaimValue {
    /// A numeric value.
    Number(f64),
    /// A boolean value.
    Flag(bool),
    /// A textual value.
    Text(String),
}

impl From<f64> for ClaimValue {
    fn from(v: f64) -> Self {
        ClaimValue::Number(v)
    }
}

impl From<bool> for ClaimValue {
    fn from(v: bool) -> Self {
        ClaimValue::Flag(v)
    }
}

impl From<&str> for ClaimValue {
    fn from(v: &str) -> Self {
        ClaimValue::Text(v.to_string())
    }
}

/// A value with an explicit unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct ClaimQuantity {
    /// The value.
    pub value: ClaimValue,
    /// Unit label, e.g. `"mm"`, `"g"`, `"mm^3"`, `"count"`, `"USD"`.
    /// `None` for dimensionless values (flags, ratios, text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub unit: Option<String>,
}

impl ClaimQuantity {
    /// A quantity with a unit.
    pub fn new(value: impl Into<ClaimValue>, unit: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            unit: Some(unit.into()),
        }
    }

    /// A dimensionless quantity.
    pub fn bare(value: impl Into<ClaimValue>) -> Self {
        Self {
            value: value.into(),
            unit: None,
        }
    }
}

/// One machine-checkable claim about a design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct ReceiptClaim {
    /// Stable claim identifier, dotted by domain: `"pcb.drc.clean"`,
    /// `"mech.mass"`, `"sheet.bend_radius"`.
    pub id: String,
    /// The domain making the claim: `"mechanical"`, `"pcb"`,
    /// `"sheet_metal"`, … (open vocabulary — new domains plug in without a
    /// schema bump).
    pub domain: String,
    /// Human-readable statement of the claim.
    pub description: String,
    /// What the claim is about, when narrower than the whole document,
    /// e.g. `"bend:3"`, `"net:GND"`, `"part:base_plate"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub subject: Option<String>,
    /// The oracle that checked this claim.
    pub oracle: OracleRef,
    /// The verdict.
    pub verdict: ClaimVerdict,
    /// The claimed/required value — what the design must meet (a spec bound,
    /// a rule limit, a declared target).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub predicted: Option<ClaimQuantity>,
    /// What the oracle actually observed or computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub measured: Option<ClaimQuantity>,
    /// Free-form context. For `Unverifiable` this carries the reason the
    /// oracle could not run (always present by construction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub details: Option<String>,
}

impl ReceiptClaim {
    fn new(
        id: impl Into<String>,
        domain: impl Into<String>,
        description: impl Into<String>,
        oracle: OracleRef,
        verdict: ClaimVerdict,
    ) -> Self {
        Self {
            id: id.into(),
            domain: domain.into(),
            description: description.into(),
            subject: None,
            oracle,
            verdict,
            predicted: None,
            measured: None,
            details: None,
        }
    }

    /// A passing claim.
    pub fn pass(
        id: impl Into<String>,
        domain: impl Into<String>,
        description: impl Into<String>,
        oracle: OracleRef,
    ) -> Self {
        Self::new(id, domain, description, oracle, ClaimVerdict::Pass)
    }

    /// A failing claim.
    pub fn fail(
        id: impl Into<String>,
        domain: impl Into<String>,
        description: impl Into<String>,
        oracle: OracleRef,
    ) -> Self {
        Self::new(id, domain, description, oracle, ClaimVerdict::Fail)
    }

    /// An unverifiable claim. The reason is mandatory — an oracle that
    /// couldn't run must say why.
    pub fn unverifiable(
        id: impl Into<String>,
        domain: impl Into<String>,
        description: impl Into<String>,
        oracle: OracleRef,
        reason: impl Into<String>,
    ) -> Self {
        let mut c = Self::new(id, domain, description, oracle, ClaimVerdict::Unverifiable);
        c.details = Some(reason.into());
        c
    }

    /// Attach the claimed/required value.
    pub fn with_predicted(mut self, q: ClaimQuantity) -> Self {
        self.predicted = Some(q);
        self
    }

    /// Attach the observed value.
    pub fn with_measured(mut self, q: ClaimQuantity) -> Self {
        self.measured = Some(q);
        self
    }

    /// Attach a subject.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Attach free-form details.
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// Placeholder for a cryptographic signature over the receipt.
///
/// Reserved for the signing follow-up; nothing produces this yet. The field
/// exists now so signed and unsigned receipts share one schema version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct ReceiptSignature {
    /// Signature algorithm identifier, e.g. `"ed25519"`.
    pub algorithm: String,
    /// Identifier of the signing key, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub key_id: Option<String>,
    /// The signature, base64-encoded.
    pub signature: String,
}

/// Aggregate view of a receipt's claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct ReceiptSummary {
    /// Total claim count.
    pub total: u32,
    /// Claims that passed.
    pub passed: u32,
    /// Claims that failed.
    pub failed: u32,
    /// Claims that could not be verified.
    pub unverifiable: u32,
    /// Fail-closed rollup: `Fail` if anything failed, else `Unverifiable`
    /// if anything (or everything — zero claims) is unverified, else `Pass`.
    pub overall: ClaimVerdict,
}

/// The unified, versioned verification receipt for a design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct DesignReceipt {
    /// Schema identifier, [`RECEIPT_SCHEMA`].
    pub schema: String,
    /// Identity of the document the receipt is about, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub document_id: Option<String>,
    /// Content hash of the design snapshot the claims were checked against
    /// (hex). A receipt without a fingerprint cannot prove *which* design it
    /// certifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub document_fingerprint: Option<String>,
    /// RFC 3339 timestamp of receipt generation, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub generated_at: Option<String>,
    /// The claims.
    pub claims: Vec<ReceiptClaim>,
    /// Cryptographic signature (reserved; see [`ReceiptSignature`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub signature: Option<ReceiptSignature>,
}

impl Default for DesignReceipt {
    fn default() -> Self {
        Self::new()
    }
}

impl DesignReceipt {
    /// An empty receipt at the current schema version.
    pub fn new() -> Self {
        Self {
            schema: RECEIPT_SCHEMA.to_string(),
            document_id: None,
            document_fingerprint: None,
            generated_at: None,
            claims: Vec::new(),
            signature: None,
        }
    }

    /// A receipt carrying the given claims.
    pub fn with_claims(claims: Vec<ReceiptClaim>) -> Self {
        Self {
            claims,
            ..Self::new()
        }
    }

    /// Fail-closed rollup verdict.
    ///
    /// Any failing claim fails the receipt. Otherwise any unverifiable claim
    /// — or an empty claim list, which is *no evidence*, not clean — makes
    /// the receipt unverifiable. Only a non-empty, all-passing claim list
    /// reads as `Pass`.
    pub fn overall(&self) -> ClaimVerdict {
        if self.claims.iter().any(|c| c.verdict == ClaimVerdict::Fail) {
            return ClaimVerdict::Fail;
        }
        if self.claims.is_empty()
            || self
                .claims
                .iter()
                .any(|c| c.verdict == ClaimVerdict::Unverifiable)
        {
            return ClaimVerdict::Unverifiable;
        }
        ClaimVerdict::Pass
    }

    /// Count claims by verdict and compute the rollup.
    pub fn summary(&self) -> ReceiptSummary {
        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut unverifiable = 0u32;
        for c in &self.claims {
            match c.verdict {
                ClaimVerdict::Pass => passed += 1,
                ClaimVerdict::Fail => failed += 1,
                ClaimVerdict::Unverifiable => unverifiable += 1,
            }
        }
        ReceiptSummary {
            total: self.claims.len() as u32,
            passed,
            failed,
            unverifiable,
            overall: self.overall(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle() -> OracleRef {
        OracleRef::new("test-oracle", "1")
    }

    #[test]
    fn empty_receipt_is_unverifiable_not_clean() {
        let r = DesignReceipt::new();
        assert_eq!(r.overall(), ClaimVerdict::Unverifiable);
        assert_eq!(r.summary().overall, ClaimVerdict::Unverifiable);
        assert_eq!(r.schema, RECEIPT_SCHEMA);
    }

    #[test]
    fn any_fail_dominates() {
        let r = DesignReceipt::with_claims(vec![
            ReceiptClaim::pass("a", "mechanical", "a", oracle()),
            ReceiptClaim::unverifiable("b", "pcb", "b", oracle(), "engine down"),
            ReceiptClaim::fail("c", "sheet_metal", "c", oracle()),
        ]);
        assert_eq!(r.overall(), ClaimVerdict::Fail);
        let s = r.summary();
        assert_eq!((s.total, s.passed, s.failed, s.unverifiable), (3, 1, 1, 1));
    }

    #[test]
    fn unverifiable_never_reads_as_pass() {
        let r = DesignReceipt::with_claims(vec![
            ReceiptClaim::pass("a", "mechanical", "a", oracle()),
            ReceiptClaim::unverifiable("b", "mechanical", "b", oracle(), "no density"),
        ]);
        assert_eq!(r.overall(), ClaimVerdict::Unverifiable);
    }

    #[test]
    fn all_pass_is_pass() {
        let r = DesignReceipt::with_claims(vec![
            ReceiptClaim::pass("a", "mechanical", "a", oracle()),
            ReceiptClaim::pass("b", "pcb", "b", oracle()),
        ]);
        assert_eq!(r.overall(), ClaimVerdict::Pass);
    }

    #[test]
    fn unverifiable_carries_its_reason() {
        let c = ReceiptClaim::unverifiable("x", "pcb", "drc clean", oracle(), "engine down");
        assert_eq!(c.verdict, ClaimVerdict::Unverifiable);
        assert_eq!(c.details.as_deref(), Some("engine down"));
    }

    #[test]
    fn wire_shape_round_trips() {
        let receipt = DesignReceipt {
            document_id: Some("doc-1".into()),
            document_fingerprint: Some("abc123".into()),
            generated_at: Some("2026-07-07T00:00:00Z".into()),
            claims: vec![ReceiptClaim::fail(
                "mech.mass",
                "mechanical",
                "mass under 50 g",
                OracleRef::new("vcad.mass_properties", "0.9.4"),
            )
            .with_predicted(ClaimQuantity::new(50.0, "g"))
            .with_measured(ClaimQuantity::new(61.2, "g"))
            .with_subject("part:bracket")],
            ..DesignReceipt::new()
        };
        let json = serde_json::to_value(&receipt).unwrap();
        assert_eq!(json["schema"], "vcad.receipt/1");
        assert_eq!(json["claims"][0]["verdict"], "fail");
        assert_eq!(json["claims"][0]["predicted"]["unit"], "g");
        assert_eq!(json["claims"][0]["measured"]["value"], 61.2);
        // signature is reserved: absent, not null
        assert!(json.get("signature").is_none());

        let back: DesignReceipt = serde_json::from_value(json).unwrap();
        assert_eq!(back, receipt);
    }

    #[test]
    fn claim_value_untagged_wire_forms() {
        let n: ClaimValue = serde_json::from_str("41.3").unwrap();
        assert_eq!(n, ClaimValue::Number(41.3));
        let i: ClaimValue = serde_json::from_str("3").unwrap();
        assert_eq!(i, ClaimValue::Number(3.0));
        let b: ClaimValue = serde_json::from_str("true").unwrap();
        assert_eq!(b, ClaimValue::Flag(true));
        let t: ClaimValue = serde_json::from_str("\"clean\"").unwrap();
        assert_eq!(t, ClaimValue::Text("clean".into()));
    }

    #[test]
    fn signature_field_round_trips_when_present() {
        let mut r = DesignReceipt::new();
        r.claims.push(ReceiptClaim::pass("a", "pcb", "a", oracle()));
        r.signature = Some(ReceiptSignature {
            algorithm: "ed25519".into(),
            key_id: Some("k1".into()),
            signature: "c2ln".into(),
        });
        let json = serde_json::to_string(&r).unwrap();
        let back: DesignReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }
}

#[cfg(all(test, feature = "ts-rs"))]
mod ts_tests {
    use super::*;
    use ts_rs::TS;

    /// Generate TypeScript definitions for the receipt types.
    ///
    /// Run with: `cargo test -p vcad-receipt --features ts-rs export_bindings -- --ignored`
    /// (bundled into `packages/ir/src/generated.ts` by `npm run ir:gen`).
    #[test]
    #[ignore = "requires --features ts-rs; produces bindings/ output, opt-in only"]
    fn export_bindings() {
        // DesignReceipt's dependency graph pulls in every other type; export
        // the rest explicitly so the .ts files always exist regardless.
        DesignReceipt::export_all().expect("DesignReceipt export failed");
        ReceiptClaim::export_all().expect("ReceiptClaim export failed");
        ClaimVerdict::export_all().expect("ClaimVerdict export failed");
        ClaimQuantity::export_all().expect("ClaimQuantity export failed");
        ClaimValue::export_all().expect("ClaimValue export failed");
        OracleRef::export_all().expect("OracleRef export failed");
        ReceiptSignature::export_all().expect("ReceiptSignature export failed");
        ReceiptSummary::export_all().expect("ReceiptSummary export failed");
        // Mechanical clearance assertions (not reachable from DesignReceipt —
        // they ride in claim `details` as JSON).
        crate::mechanical::ClearanceClaim::export_all().expect("ClearanceClaim export failed");
    }
}
