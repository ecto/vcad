//! Value types for CRDT document parameters.

use serde::{Deserialize, Serialize};

/// A parameter value in the CRDT document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    /// 64-bit floating point.
    F64(f64),
    /// 3D vector.
    Vec3([f64; 3]),
    /// Boolean.
    Bool(bool),
    /// String.
    String(String),
    /// Reference to another feature.
    FeatureRef(String),
    /// List of feature references.
    FeatureRefList(Vec<String>),
    /// Sketch data (serialized).
    Sketch(String),
}

/// CRDT-safe equality for `f64`: reflexive on NaN (two NaNs compare equal
/// so `impl Eq for Value` stays lawful) but normalizes `+0.0 == -0.0` so a
/// client that happens to produce `-0.0` by arithmetic isn't treated as
/// divergent from one that kept `0.0`.
#[inline]
fn f64_crdt_eq(a: f64, b: f64) -> bool {
    if a.is_nan() && b.is_nan() {
        true
    } else {
        a == b
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::F64(a), Value::F64(b)) => f64_crdt_eq(*a, *b),
            (Value::Vec3(a), Value::Vec3(b)) => {
                f64_crdt_eq(a[0], b[0]) && f64_crdt_eq(a[1], b[1]) && f64_crdt_eq(a[2], b[2])
            }
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::FeatureRef(a), Value::FeatureRef(b)) => a == b,
            (Value::FeatureRefList(a), Value::FeatureRefList(b)) => a == b,
            (Value::Sketch(a), Value::Sketch(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_value_equality() {
        assert_eq!(Value::F64(1.0), Value::F64(1.0));
        assert_ne!(Value::F64(1.0), Value::F64(2.0));
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_eq!(Value::String("hello".into()), Value::String("hello".into()));
        assert_eq!(Value::Vec3([1.0, 2.0, 3.0]), Value::Vec3([1.0, 2.0, 3.0]));
        assert_ne!(Value::F64(1.0), Value::Bool(true));
    }

    #[test]
    fn f64_crdt_eq_normalizes_zero_and_preserves_nan_reflexivity() {
        // +0.0 and -0.0 must compare equal so a client normalizing one to the
        // other doesn't cause spurious CRDT divergence.
        assert_eq!(Value::F64(0.0), Value::F64(-0.0));
        assert_eq!(
            Value::Vec3([0.0, -0.0, 0.0]),
            Value::Vec3([-0.0, 0.0, -0.0])
        );

        // NaN must compare equal to itself so `impl Eq for Value` remains
        // lawful (required by e.g. HashMap<Value, _>).
        assert_eq!(Value::F64(f64::NAN), Value::F64(f64::NAN));
    }

    #[test]
    fn test_value_serde_roundtrip() {
        let values = vec![
            Value::F64(42.0),
            Value::Vec3([1.0, 2.0, 3.0]),
            Value::Bool(false),
            Value::String("test".into()),
            Value::FeatureRef("feat_123".into()),
            Value::FeatureRefList(vec!["a".into(), "b".into()]),
            Value::Sketch("{\"segments\":[]}".into()),
        ];
        for v in &values {
            let json = serde_json::to_string(v).unwrap();
            let v2: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v, &v2);
        }
    }
}
