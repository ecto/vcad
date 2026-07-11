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

/// How a claim's verdict was produced — the evidentiary weight behind it.
///
/// A surrogate model and a real solver can check the same claim; the verdict
/// alone does not say which one did. `Predicted` marks fast-path estimates
/// (neural surrogates, analytic approximations) that have not been confirmed
/// by the trusted oracle. A receipt whose passing claims rest on predictions
/// rolls up as [`ReceiptVerdict::Provisional`], never `Pass`.
///
/// Absent on the wire means [`ClaimBasis::Verified`] — every claim written
/// before this field existed came from a real oracle run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum ClaimBasis {
    /// A fast estimate (surrogate model, analytic approximation) that the
    /// trusted oracle has not confirmed. Good enough to steer, not to ship.
    Predicted,
    /// The trusted oracle (solver, DRC engine, rule pack) ran for real.
    Verified,
    /// Confirmed against the physical world (calipers, scale, spectrum
    /// analyzer) — e.g. via `record_measurement`. The strongest basis.
    Measured,
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
    /// How the verdict was produced. Absent means [`ClaimBasis::Verified`]
    /// (see [`ClaimBasis`] for the back-compat rationale).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub basis: Option<ClaimBasis>,
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
            basis: None,
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

    /// Mark how this verdict was produced.
    pub fn with_basis(mut self, basis: ClaimBasis) -> Self {
        self.basis = Some(basis);
        self
    }

    /// The basis, resolving the wire default: absent means `Verified`.
    pub fn effective_basis(&self) -> ClaimBasis {
        self.basis.unwrap_or(ClaimBasis::Verified)
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

/// Basis-aware fail-closed rollup verdict for a whole receipt.
///
/// Extends [`ClaimVerdict`] with `Provisional`: the receipt *would* pass,
/// but at least one passing claim rests on a [`ClaimBasis::Predicted`]
/// estimate the trusted oracle has not confirmed. Provisional is never a
/// pass — it is a promissory note, redeemed by re-running the slow oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum ReceiptVerdict {
    /// Every claim passed on verified or measured basis.
    Pass,
    /// Every claim passed, but at least one only on predicted basis.
    Provisional,
    /// At least one claim failed (on any basis — a predicted fail is still
    /// a fail: the fast path saying "no" is actionable).
    Fail,
    /// No evidence, or at least one claim could not be checked.
    Unverifiable,
}

impl Default for ReceiptVerdict {
    /// Fail-closed: absence of a computed verdict reads as unverifiable.
    fn default() -> Self {
        ReceiptVerdict::Unverifiable
    }
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
    /// Claims whose verdict rests on a predicted (surrogate) basis.
    #[serde(default)]
    pub predicted_basis: u32,
    /// Fail-closed rollup: `Fail` if anything failed, else `Unverifiable`
    /// if anything (or everything — zero claims) is unverified, else `Pass`.
    /// Basis-blind; see [`ReceiptSummary::verdict`] for the basis-aware view.
    pub overall: ClaimVerdict,
    /// Basis-aware rollup ([`DesignReceipt::verdict`]): like `overall`, but
    /// an all-pass receipt leaning on predicted claims reads `Provisional`.
    #[serde(default)]
    pub verdict: ReceiptVerdict,
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

    /// Basis-aware fail-closed rollup.
    ///
    /// Same lattice as [`DesignReceipt::overall`], with one refinement: a
    /// receipt that would pass but has any claim on
    /// [`ClaimBasis::Predicted`] rolls up as
    /// [`ReceiptVerdict::Provisional`]. Predictions can steer a design; only
    /// verified or measured evidence can certify one.
    pub fn verdict(&self) -> ReceiptVerdict {
        match self.overall() {
            ClaimVerdict::Fail => ReceiptVerdict::Fail,
            ClaimVerdict::Unverifiable => ReceiptVerdict::Unverifiable,
            ClaimVerdict::Pass => {
                if self
                    .claims
                    .iter()
                    .any(|c| c.effective_basis() == ClaimBasis::Predicted)
                {
                    ReceiptVerdict::Provisional
                } else {
                    ReceiptVerdict::Pass
                }
            }
        }
    }

    /// Count claims by verdict and compute the rollup.
    pub fn summary(&self) -> ReceiptSummary {
        let mut passed = 0u32;
        let mut failed = 0u32;
        let mut unverifiable = 0u32;
        let mut predicted_basis = 0u32;
        for c in &self.claims {
            match c.verdict {
                ClaimVerdict::Pass => passed += 1,
                ClaimVerdict::Fail => failed += 1,
                ClaimVerdict::Unverifiable => unverifiable += 1,
            }
            if c.effective_basis() == ClaimBasis::Predicted {
                predicted_basis += 1;
            }
        }
        ReceiptSummary {
            total: self.claims.len() as u32,
            passed,
            failed,
            unverifiable,
            predicted_basis,
            overall: self.overall(),
            verdict: self.verdict(),
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
    fn predicted_basis_pass_is_provisional_never_pass() {
        let r = DesignReceipt::with_claims(vec![
            ReceiptClaim::pass("a", "mechanical", "stiffness ok", oracle()),
            ReceiptClaim::pass("b", "mechanical", "first mode ok", oracle())
                .with_basis(ClaimBasis::Predicted),
        ]);
        // Basis-blind rollup still reads pass; basis-aware one does not.
        assert_eq!(r.overall(), ClaimVerdict::Pass);
        assert_eq!(r.verdict(), ReceiptVerdict::Provisional);
        let s = r.summary();
        assert_eq!(s.predicted_basis, 1);
        assert_eq!(s.verdict, ReceiptVerdict::Provisional);
        assert_eq!(s.overall, ClaimVerdict::Pass);
    }

    #[test]
    fn verified_and_measured_basis_pass_cleanly() {
        let r = DesignReceipt::with_claims(vec![
            ReceiptClaim::pass("a", "mechanical", "a", oracle()).with_basis(ClaimBasis::Verified),
            ReceiptClaim::pass("b", "mechanical", "b", oracle()).with_basis(ClaimBasis::Measured),
            // absent basis defaults to verified
            ReceiptClaim::pass("c", "pcb", "c", oracle()),
        ]);
        assert_eq!(r.verdict(), ReceiptVerdict::Pass);
        assert_eq!(r.summary().predicted_basis, 0);
    }

    #[test]
    fn predicted_fail_and_unverifiable_dominate_provisional() {
        let fail =
            DesignReceipt::with_claims(vec![ReceiptClaim::fail("a", "mechanical", "a", oracle())
                .with_basis(ClaimBasis::Predicted)]);
        assert_eq!(fail.verdict(), ReceiptVerdict::Fail);

        let unv = DesignReceipt::with_claims(vec![
            ReceiptClaim::pass("a", "mechanical", "a", oracle()).with_basis(ClaimBasis::Predicted),
            ReceiptClaim::unverifiable("b", "pcb", "b", oracle(), "engine down"),
        ]);
        assert_eq!(unv.verdict(), ReceiptVerdict::Unverifiable);

        // Empty stays fail-closed on both axes.
        assert_eq!(DesignReceipt::new().verdict(), ReceiptVerdict::Unverifiable);
        assert_eq!(ReceiptVerdict::default(), ReceiptVerdict::Unverifiable);
    }

    #[test]
    fn basis_wire_form_and_back_compat() {
        let c =
            ReceiptClaim::pass("a", "mechanical", "a", oracle()).with_basis(ClaimBasis::Predicted);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["basis"], "predicted");

        // Pre-basis wire shape (no field) parses and reads as verified.
        let legacy: ReceiptClaim = serde_json::from_value(serde_json::json!({
            "id": "a", "domain": "pcb", "description": "d",
            "oracle": {"id": "o", "version": "1"}, "verdict": "pass"
        }))
        .unwrap();
        assert_eq!(legacy.basis, None);
        assert_eq!(legacy.effective_basis(), ClaimBasis::Verified);
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
        ClaimBasis::export_all().expect("ClaimBasis export failed");
        ReceiptVerdict::export_all().expect("ReceiptVerdict export failed");
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
