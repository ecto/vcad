//! The claim-family registry: schema id → the family behind it.
//!
//! vcad has one unified receipt ([`vcad_receipt::DesignReceipt`], schema
//! `vcad.receipt/1`) and, by now, a dozen-odd per-domain **claim families** —
//! `vcad.cam-claims/1`, `vcad.tolerance-claims/1`, `vcad.thermal-claims/1`,
//! … — each a crate with its own `ClaimSet` and a `design_claims` function
//! that translates it into unified claims. Every one of those functions
//! existed and none of them was reachable: the MCP `build_receipt` hard-wired
//! three families and had nowhere to look the others up, so no Rust
//! `design_claims` family ever reached a receipt.
//!
//! This crate is that lookup, and it is deliberately the *only* thing here.
//! It owns no claim logic of its own — every adapter is three lines that
//! deserialize the family's own report and call the family's own
//! `design_claims`. A registry that re-implemented a family's ladder would be
//! a second answer to "does this claim hold", which is exactly what the
//! receipt discipline exists to prevent.
//!
//! # Why it is a separate crate
//!
//! It cannot live in `vcad-receipt`: every family crate *depends on*
//! `vcad-receipt` to build its claims, so a registry inside it would be a
//! dependency cycle. It sits above them instead, and every family is an
//! optional dependency behind a cargo feature named for it.
//!
//! # What is registered, and what is native-only
//!
//! Everything in the default feature set is wasm32-clean, and all of it is
//! already inside the kernel WASM bundle's dependency graph (`vcad-kernel`
//! depends on each of these crates), so a family costs the shipped bundle
//! nothing beyond its adapter. `spice` (`vcad-ecad-sim`) is off by default
//! and enabled by the host alongside its own `ecad` feature, so the registry
//! never claims a family its host did not compile in;
//! [`ClaimFamily::native_only`] reports any family that cannot reach wasm at
//! all (today: none).
//!
//! [`families`] lists exactly what was compiled in — never a hard-coded
//! catalog, so a build that trimmed a feature cannot advertise a family it
//! cannot serve.
//!
//! # The three entry points
//!
//! | Function | Answers |
//! |---|---|
//! | [`claims_for`] | this family's serialized report → unified claims |
//! | [`restate`] | …against these input digests, so a moved input reads `Stale` |
//! | [`bind`] | …with this measurement of the real part bound to a claim |
//!
//! `restate` and `bind` are optional per family — a family that has no
//! staleness or measurement machinery says so
//! ([`ClaimFamily::stale_aware`], [`ClaimFamily::bindable`]) rather than
//! pretending. Asking for one it does not have is an error, not a silent
//! no-op: a receipt that quietly skipped a staleness check would read clean
//! for a job that moved underneath it.

#![warn(missing_docs)]

use std::collections::BTreeMap;

use vcad_receipt::ReceiptClaim;

mod fingerprint;

pub use fingerprint::{digest, fingerprint_of};

/// What the registry refuses to guess at.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// No family is registered under this schema id.
    #[error("no claim family registered for schema {schema:?}; this build carries [{known}]")]
    UnknownSchema {
        /// The schema that was asked for.
        schema: String,
        /// The schemas this build does carry, comma-separated.
        known: String,
    },
    /// The report did not deserialize as this family's claim set.
    #[error("the report for {schema:?} is not a {schema:?} claim set: {reason}")]
    BadReport {
        /// The family the report was submitted under.
        schema: String,
        /// The deserializer's complaint.
        reason: String,
    },
    /// The report's own `schema` field names a different family.
    #[error("the report says it is {found:?} but it was submitted as {schema:?}")]
    SchemaMismatch {
        /// The family the report was submitted under.
        schema: String,
        /// The family the report claims to be.
        found: String,
    },
    /// The family has no staleness machinery.
    #[error(
        "claim family {schema:?} cannot be re-stated against changed inputs: \
         it records no per-claim input basis"
    )]
    NotStaleAware {
        /// The family asked.
        schema: String,
    },
    /// The family has no measurement binding.
    #[error("claim family {schema:?} binds no measurements: nothing in it is measurable")]
    NotBindable {
        /// The family asked.
        schema: String,
    },
    /// A deposit records none of the inputs its claims rest on.
    ///
    /// Fail-closed: a claim with no basis can never be re-stated, so it can
    /// never go `Stale` — it would go on certifying a design that has since
    /// been edited. Refusing at deposit time is the only point where the
    /// producer is still around to say what the claims rest on.
    #[error(
        "a {schema:?} deposit must record the inputs its claims rest on, and this one \
         records {recorded}. Required: [{required}]; missing: [{missing}]. A claim with \
         no basis can never go stale, so it would keep certifying a design that has \
         since been edited."
    )]
    MissingBasisInputs {
        /// The family the deposit was filed under.
        schema: String,
        /// What the deposit did record, in words ("none" or a key list).
        recorded: String,
        /// Every key this deposit had to carry.
        required: String,
        /// The ones it did not.
        missing: String,
    },
    /// The family's own binder refused the measurement.
    #[error("{0}")]
    Refused(String),
}

impl RegistryError {
    fn bad_report(schema: &str, e: impl std::fmt::Display) -> Self {
        RegistryError::BadReport {
            schema: schema.to_string(),
            reason: e.to_string(),
        }
    }
}

/// The result of binding measurements to a family's report.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BindOutcome {
    /// The family's report with the measurements bound, re-serialized. Store
    /// this back where the original came from: it is the new state of the
    /// claim set, on a measured basis.
    pub report: String,
    /// Family-specific follow-ups the binding produced — for CAM, the cutter
    /// compensation a measurement over pins implies. Empty when the binding
    /// only moved statuses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived: Vec<serde_json::Value>,
    /// What the binding did, in a sentence, for a human reading the result.
    pub note: String,
}

/// What a re-state found.
///
/// More than the re-stated report, because the caller usually wants to *act*
/// on staleness rather than just carry it: `verify_receipt` reports a
/// Holds/Stale/Violated verdict, and reconstructing that by diffing two
/// serialized reports would be a second, weaker answer to a question the
/// family already answered exactly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RestateOutcome {
    /// The re-stated report, serialized. Claims whose basis moved now carry
    /// the family's own stale status.
    pub report: String,
    /// Names of the claims this re-state turned stale. Empty when the basis
    /// is intact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_claims: Vec<String>,
    /// The basis keys that moved, deduplicated — what to tell a human when
    /// asked *why* the receipt stopped vouching for the design.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drifted_inputs: Vec<String>,
}

impl RestateOutcome {
    /// Whether anything went stale.
    pub fn is_stale(&self) -> bool {
        !self.stale_claims.is_empty()
    }
}

/// Turn a family's serialized report into unified receipt claims.
type ToClaims = fn(&str) -> Result<Vec<ReceiptClaim>, RegistryError>;

/// Re-state a report against the digests of its inputs as they are *now*.
type Restate = fn(&str, &BTreeMap<String, String>) -> Result<RestateOutcome, RegistryError>;

/// Bind measurements (and whatever context the family needs) to a report.
type Bind = fn(&str, &str, &serde_json::Value) -> Result<BindOutcome, RegistryError>;

/// The basis keys a family's *own report* says its claims rest on.
///
/// Only families that track a per-claim basis implement this; the rest
/// declare their requirement statically in [`ClaimFamily::required_basis`].
type BasisOfReport = fn(&str) -> Result<Vec<String>, RegistryError>;

/// One registered claim family.
#[derive(Clone)]
pub struct ClaimFamily {
    /// The family's schema tag, e.g. `"vcad.cam-claims/1"`. The key.
    pub schema: &'static str,
    /// The `domain` the family's claims carry in the unified receipt,
    /// e.g. `"cam"`.
    pub domain: &'static str,
    /// The crate the family — and its `design_claims` — lives in.
    pub crate_name: &'static str,
    /// One line on what the family claims.
    pub summary: &'static str,
    /// True when this family's crate cannot compile to `wasm32`, so it is
    /// reachable natively (CLI, FFI) but never from the browser or MCP's
    /// WASM kernel. Registered anyway, and honest about it.
    pub native_only: bool,
    /// The basis keys a deposit of this family **must** record, at minimum.
    ///
    /// A deposit with no basis is a claim that can never go `Stale`: nothing
    /// can be compared against it, so it would certify a design that has
    /// since been edited out from under it. That is fail-open, and
    /// [`check_deposit`] refuses it.
    ///
    /// For the solver families this is `["spec"]` — the resolved model the
    /// claims were computed from. A family may record more than it must
    /// (CAM's job deposits carry `program`, `outline`, `tool` and `stock`),
    /// and a family that tracks a per-claim basis is held to *that* too,
    /// which is stricter than this list.
    pub required_basis: &'static [&'static str],
    to_claims: ToClaims,
    restate: Option<Restate>,
    bind: Option<Bind>,
    basis_of_report: Option<BasisOfReport>,
}

impl std::fmt::Debug for ClaimFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimFamily")
            .field("schema", &self.schema)
            .field("domain", &self.domain)
            .field("crate_name", &self.crate_name)
            .field("native_only", &self.native_only)
            .field("stale_aware", &self.stale_aware())
            .field("bindable", &self.bindable())
            .finish()
    }
}

impl ClaimFamily {
    /// Whether [`restate`] works for this family: it records, per claim, the
    /// inputs that claim rests on, so a changed input can turn it `Stale`.
    pub fn stale_aware(&self) -> bool {
        self.restate.is_some()
    }

    /// Whether [`bind`] works for this family: it has predicted claims a
    /// measurement of the real part can close.
    pub fn bindable(&self) -> bool {
        self.bind.is_some()
    }

    /// This family's report → unified receipt claims.
    pub fn claims(&self, report_json: &str) -> Result<Vec<ReceiptClaim>, RegistryError> {
        (self.to_claims)(report_json)
    }
}

/// The families this build carries, in schema order.
///
/// Built from the compiled-in feature set, never a hard-coded catalog: a
/// build that trimmed a family cannot advertise one it could not serve.
// Every push is `cfg`-gated on its family's feature, so this cannot be a
// `vec![]` literal — attributes do not apply to elements inside the macro.
#[allow(clippy::vec_init_then_push)]
pub fn families() -> Vec<ClaimFamily> {
    let mut out: Vec<ClaimFamily> = Vec::new();
    #[cfg(feature = "cam")]
    out.push(adapters::cam::family());
    #[cfg(feature = "tolerance")]
    out.push(adapters::tolerance::family());
    #[cfg(feature = "thermal")]
    out.push(adapters::thermal::family());
    #[cfg(feature = "em")]
    out.push(adapters::em::family());
    #[cfg(feature = "photonics")]
    out.push(adapters::photonics::family());
    #[cfg(feature = "antenna")]
    out.push(adapters::antenna::family());
    #[cfg(feature = "neutronics")]
    out.push(adapters::neutronics::family());
    #[cfg(feature = "acoustics")]
    out.push(adapters::acoustics::family());
    #[cfg(feature = "particle")]
    out.push(adapters::particle::family());
    #[cfg(feature = "fea")]
    out.push(adapters::fea::family());
    #[cfg(feature = "flow")]
    out.push(adapters::flow::family());
    #[cfg(feature = "optics")]
    out.push(adapters::optics::family());
    #[cfg(feature = "orbit")]
    out.push(adapters::orbit::family());
    #[cfg(feature = "spice")]
    out.push(adapters::spice::family());
    out.sort_by_key(|f| f.schema);
    out
}

/// The family registered under `schema`, or `None`.
pub fn find(schema: &str) -> Option<ClaimFamily> {
    families().into_iter().find(|f| f.schema == schema)
}

fn known_schemas() -> String {
    families()
        .iter()
        .map(|f| f.schema)
        .collect::<Vec<_>>()
        .join(", ")
}

fn require(schema: &str) -> Result<ClaimFamily, RegistryError> {
    find(schema).ok_or_else(|| RegistryError::UnknownSchema {
        schema: schema.to_string(),
        known: known_schemas(),
    })
}

/// Unified receipt claims from one family's serialized report.
///
/// Fail-closed at both ends: an unregistered schema is an error rather than
/// an empty claim list (an empty list would let a receipt read clean for a
/// family nobody could check), and a report whose own `schema` field
/// disagrees with the one it was filed under is refused rather than
/// reinterpreted.
pub fn claims_for(schema: &str, report_json: &str) -> Result<Vec<ReceiptClaim>, RegistryError> {
    require(schema)?.claims(report_json)
}

/// Check that a deposit records the inputs its claims rest on.
///
/// This is the gate the whole staleness story rests on. A report can be
/// re-stated only against inputs somebody wrote down; a deposit that records
/// none is a claim that can never move, and "never moves" is
/// indistinguishable from "still true" when a receipt reads it a month later.
/// So an input-less deposit is refused at the one moment the producer is
/// still there to say what the claims rest on.
///
/// Two requirements, and a deposit must satisfy both:
///
/// 1. every key in the family's [`ClaimFamily::required_basis`];
/// 2. for a family that tracks a per-claim basis (CAM), every key the
///    *report itself* says its claims depend on — which is stricter, and
///    catches a job deposit that recorded a gear instead of a program.
///
/// `inputs` maps basis key → that input's JSON text, exactly as it is stored
/// on the document.
pub fn check_deposit(
    schema: &str,
    report_json: &str,
    inputs: &BTreeMap<String, String>,
) -> Result<(), RegistryError> {
    let family = require(schema)?;

    let mut required: Vec<String> = family
        .required_basis
        .iter()
        .map(|k| (*k).to_string())
        .collect();
    if let Some(basis_of) = family.basis_of_report {
        for key in basis_of(report_json)? {
            if !required.contains(&key) {
                required.push(key);
            }
        }
    }
    required.sort();

    let missing: Vec<&str> = required
        .iter()
        .filter(|k| !inputs.contains_key(*k))
        .map(String::as_str)
        .collect();
    if missing.is_empty() && !inputs.is_empty() {
        return Ok(());
    }
    // A family that declares nothing and whose report declares nothing still
    // may not deposit empty — see the type-level note on `required_basis`.
    Err(RegistryError::MissingBasisInputs {
        schema: schema.to_string(),
        recorded: if inputs.is_empty() {
            "none".to_string()
        } else {
            inputs.keys().cloned().collect::<Vec<_>>().join(", ")
        },
        required: if required.is_empty() {
            "at least one input".to_string()
        } else {
            required.join(", ")
        },
        missing: if missing.is_empty() {
            "—".to_string()
        } else {
            missing.join(", ")
        },
    })
}

/// Re-state a stored report against the digests of its inputs as they stand
/// now, so a claim whose basis moved comes back `Stale`.
///
/// `inputs` maps the family's own basis keys (for CAM: `program`, `outline`,
/// `tool`, `stock`, `gear`, `material`, `solid`) to that input's JSON text.
/// Hashing happens here, with [`fingerprint_of`] — the same function the
/// depositing side used — so the two cannot hash differently.
///
/// [`check_deposit`] runs first: re-stating a basis-less deposit would
/// cheerfully report that nothing had changed, which is the one answer that
/// must never be reachable without evidence.
pub fn restate(
    schema: &str,
    report_json: &str,
    inputs: &BTreeMap<String, String>,
) -> Result<RestateOutcome, RegistryError> {
    let family = require(schema)?;
    let f = family.restate.ok_or_else(|| RegistryError::NotStaleAware {
        schema: schema.to_string(),
    })?;
    check_deposit(schema, report_json, inputs)?;
    f(report_json, &fingerprint_of(inputs))
}

/// Bind measurements of the real part to a family's predicted claims.
///
/// `measurements_json` is the family's own measurement wire form (for CAM, a
/// `Measurement` or an array of them); `context` carries whatever else the
/// family needs to derive a follow-up — for CAM's `gear.over_pins` that is
/// the `gear` definition under the key of the same name, which is exactly
/// the basis input the deposit already recorded.
pub fn bind(
    schema: &str,
    report_json: &str,
    measurements_json: &str,
    context: &serde_json::Value,
) -> Result<BindOutcome, RegistryError> {
    let family = require(schema)?;
    let f = family.bind.ok_or_else(|| RegistryError::NotBindable {
        schema: schema.to_string(),
    })?;
    f(report_json, measurements_json, context)
}

// ---------------------------------------------------------------------------
// Adapters
// ---------------------------------------------------------------------------

mod adapters;

#[cfg(test)]
mod tests;
