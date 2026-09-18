//! Input digests, computed the one way both sides of a deposit agree on.
//!
//! A claim goes `Stale` when an input it rests on moved. That comparison is
//! only meaningful if the tool that *deposited* the report and the receipt
//! builder that *re-states* it hash the same bytes the same way — two
//! hashers is two answers, and the one that disagrees silently reports a
//! stale job as clean.
//!
//! So there is exactly one hasher here, and both sides call it:
//! [`fingerprint_of`] over the deposited inputs, each of which is the JSON
//! **text** the depositor wrote. Hashing the text rather than a re-serialized
//! value means the digest cannot drift on a map's key order or a float's
//! formatting. It also means a re-serialization that changes nothing but
//! whitespace reads as a change — which errs towards `Stale`, never towards
//! a false `Holds`.

use std::collections::BTreeMap;

/// FNV-1a over bytes.
///
/// A change detector, not a cryptographic hash — the same choice, and the
/// same constants, as `vcad_kernel_cam::receipt`, so a CAM fingerprint built
/// here is byte-identical to one built there.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Hex digest of a byte slice.
pub fn digest(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// Digest every input, keyed by the family's own basis key.
///
/// Values are the serialized JSON text of each input, exactly as the
/// depositing tool recorded it.
pub fn fingerprint_of(inputs: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    inputs
        .iter()
        .map(|(k, v)| (k.clone(), digest(v.as_bytes())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_and_differs_on_a_single_byte() {
        assert_eq!(digest(b"G1 X10 Y10"), digest(b"G1 X10 Y10"));
        assert_ne!(digest(b"G1 X10 Y10"), digest(b"G1 X10 Y11"));
        // 16 hex chars, always — a short digest would collide on display.
        assert_eq!(digest(b"").len(), 16);
    }

    #[test]
    fn fingerprint_keys_survive_and_values_are_digests() {
        let mut inputs = BTreeMap::new();
        inputs.insert("program".to_string(), "\"G1 X1\"".to_string());
        inputs.insert("tool".to_string(), "3.175".to_string());
        let fp = fingerprint_of(&inputs);
        assert_eq!(fp.len(), 2);
        assert_eq!(fp["program"], digest(b"\"G1 X1\""));
        assert_ne!(fp["program"], fp["tool"]);
    }

    /// The whole point: the registry's hasher and the CAM crate's hasher have
    /// to produce the same digest for the same bytes, or a deposit made by
    /// one and re-stated by the other reads `Stale` every single time.
    #[cfg(feature = "cam")]
    #[test]
    fn matches_the_cam_crates_own_digest() {
        for probe in [b"".as_slice(), b"G21", b"{\"outer\":[[0.0,0.0]]}"] {
            assert_eq!(digest(probe), vcad_kernel_cam::receipt::digest(probe));
        }
    }
}
