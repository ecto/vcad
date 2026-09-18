//! The `vcad.cam-claims/1` deposit every CAM entry point can hand back.
//!
//! `vcad-kernel-cam` has built CAM claims since wave 2, and nothing carried
//! them anywhere: the oracle's verdict reached the caller as a verification
//! report, the *claims* about the part it makes did not. This module is the
//! envelope that closes that gap — a claim set, serialized, alongside the
//! live inputs it rests on, in the shape a document's `claim_reports` slot
//! stores and `vcad-claim-registry` reads back.
//!
//! The inputs are the load-bearing half. A claim set knows the digests it was
//! made against; only the inputs let a later `build_receipt` recompute them
//! and notice that the program, the outline or the tool moved — the
//! difference between a receipt that says `Stale` and one that says `Holds`
//! about a job that no longer exists. They are hashed by
//! [`vcad_claim_registry::fingerprint_of`], the same function the receipt
//! builder calls, because two hashers would be two answers.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Value};

use vcad_kernel_cam::receipt::{Claim, ClaimSet, ClaimStatus, Fingerprint, Provenance};

/// Collects the inputs a claim set rests on, as the JSON text that is both
/// stored on the document and hashed into the fingerprint.
#[derive(Debug, Default, Clone)]
pub struct Inputs(BTreeMap<String, String>);

impl Inputs {
    /// An empty input set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an input under a `vcad_kernel_cam::receipt::BASIS_*` key.
    ///
    /// A value that will not serialize is recorded as [`Self::absent`],
    /// because a key that is simply *missing* is worse than one recorded as
    /// nothing: `restate` treats a basis key the fingerprint does not carry
    /// as changed, so the claim would read `Stale` forever with no way to
    /// clear it.
    pub fn with<T: Serialize>(mut self, key: &str, value: &T) -> Self {
        let text = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        self.0.insert(key.to_string(), text);
        self
    }

    /// Record a basis key the claims name but this deposit has nothing for
    /// *yet* — a gear's `program`, before anyone has posted the job that cuts
    /// it.
    ///
    /// This is not the same as leaving the key out. "There is no program"
    /// is a state the claim can rest on: it hashes to a stable digest, so the
    /// claim settles onto its own rung instead of being permanently `Stale`,
    /// and the moment a real program is deposited under the key the digest
    /// moves and the claim re-opens — which is exactly right, because a
    /// compensation worked out for one program is not evidence about another.
    pub fn absent(mut self, key: &str) -> Self {
        self.0.insert(key.to_string(), "null".to_string());
        self
    }

    /// The digests these inputs imply, for the claim set's fingerprint.
    fn fingerprint(&self) -> Fingerprint {
        let mut fp = Fingerprint::new();
        for (key, digest) in vcad_claim_registry::fingerprint_of(&self.0) {
            fp = fp.with(key, digest);
        }
        fp
    }
}

/// Count the claims by status, so a caller can see where a job stands
/// without parsing the set.
fn summary(set: &ClaimSet) -> Value {
    let count = |s: ClaimStatus| set.claims.iter().filter(|c| c.status == s).count();
    json!({
        "total": set.claims.len(),
        "holds": count(ClaimStatus::Holds),
        "provisional": count(ClaimStatus::Provisional),
        "violated": count(ClaimStatus::Violated),
        "unverified": count(ClaimStatus::Unverified),
        "stale": count(ClaimStatus::Stale),
        "status": format!("{:?}", set.status()),
        "all_hold": set.all_hold(),
    })
}

/// Build the deposit envelope: the claim set plus the inputs it rests on.
///
/// The returned document is what a caller stores in a document's
/// `claim_reports` slot verbatim. `oracles` names the oracles that
/// contributed, so a reader can tell a job replay from a gear calculation
/// without decoding the claims.
pub fn deposit(
    claims: Vec<Claim>,
    inputs: Inputs,
    oracles: &[&str],
    context: Option<String>,
) -> Value {
    let set = ClaimSet::new(
        inputs.fingerprint(),
        Provenance {
            oracles: oracles.iter().map(|s| (*s).to_string()).collect(),
            context,
            ..Default::default()
        },
        claims,
    );
    let summary = summary(&set);
    json!({
        "schema": vcad_kernel_cam::receipt::CLAIM_SCHEMA,
        // Text, not a nested object: the registry's entry points take a
        // serialized report, and hashing exact bytes keeps a digest from
        // drifting on key order or float formatting.
        "report": serde_json::to_string(&set).unwrap_or_default(),
        "inputs": inputs.0,
        "summary": summary,
        "note": "Deposit this on a document (claim_reports) and build_receipt \
                 merges it into the unified receipt. Predicted claims stay \
                 Provisional until a measurement closes them.",
    })
}
