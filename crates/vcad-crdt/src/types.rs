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

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::F64(a), Value::F64(b)) => a.to_bits() == b.to_bits(),
            (Value::Vec3(a), Value::Vec3(b)) => {
                a[0].to_bits() == b[0].to_bits()
                    && a[1].to_bits() == b[1].to_bits()
                    && a[2].to_bits() == b[2].to_bits()
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
