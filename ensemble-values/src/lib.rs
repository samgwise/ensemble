//! Ensemble value model — the canonical type system for action payloads.
//!
//! This crate implements the Ensemble Value Model Specification (Draft v0.1).
//! It defines 10 value types: Null, Bool, Integer(i64), Float(f64), String,
//! Binary, Tuple, List, Map, and TypedBinary.
//!
//! The value model is serialization-independent: these types describe semantic
//! meaning, not wire format. MessagePack adapters are provided for the reference
//! implementation.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A value in the Ensemble type system.
///
/// This enum represents all 10 types defined by the Ensemble Value Model
/// Specification. Values may be nested arbitrarily (e.g. a Map containing
/// a List of Tuples).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    /// Explicit null value. Does not remove Param state (use `unset_param` for that).
    Null,

    /// Boolean value: true or false.
    Bool(bool),

    /// Signed 64-bit integer.
    Integer(i64),

    /// IEEE754 double-precision floating-point. NaN and ±Infinity are permitted.
    Float(FloatValue),

    /// UTF-8 encoded text.
    String(String),

    /// Opaque sequence of bytes. The hub does not interpret binary payloads.
    Binary(Vec<u8>),

    /// Ordered collection with fixed positional meaning.
    /// Example: (channel, note, velocity).
    Tuple(Vec<Value>),

    /// Ordered collection of values. Variable length.
    /// Example: [60, 62, 64, 67].
    List(Vec<Value>),

    /// Associative collection of key-value pairs. Keys are UTF-8 strings.
    /// Unordered: applications must not rely on insertion or iteration order.
    Map(BTreeMap<String, Value>),

    /// Typed binary data with an application-defined tag.
    /// The `ensemble/*` namespace is reserved for future Ensemble specifications.
    TypedBinary {
        /// Tag identifying the data type (e.g. "ensemble/f32", "org.example.matrix").
        tag: String,
        /// Opaque binary data.
        data: Vec<u8>,
    },
}

/// Wrapper for f64 that implements Eq and Hash by comparing bit patterns.
/// This allows NaN and ±Infinity to be stored in collections and compared.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FloatValue(f64);

impl FloatValue {
    pub fn new(v: f64) -> Self {
        Self(v)
    }

    pub fn value(self) -> f64 {
        self.0
    }
}

impl PartialEq for FloatValue {
    fn eq(&self, other: &Self) -> bool {
        // Compare bit patterns so NaN == NaN and -0.0 != 0.0
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for FloatValue {}

impl std::hash::Hash for FloatValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl From<f64> for FloatValue {
    fn from(v: f64) -> Self {
        Self(v)
    }
}

impl From<FloatValue> for f64 {
    fn from(v: FloatValue) -> Self {
        v.0
    }
}

// ---------------------------------------------------------------------------
// Tests — value model conformance corpus
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Null --

    #[test]
    fn null_roundtrip() {
        let v = Value::Null;
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Bool --

    #[test]
    fn bool_true_roundtrip() {
        let v = Value::Bool(true);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn bool_false_roundtrip() {
        let v = Value::Bool(false);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Integer --

    #[test]
    fn integer_roundtrip() {
        let v = Value::Integer(42);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn integer_negative_roundtrip() {
        let v = Value::Integer(-1);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn integer_i64_max_roundtrip() {
        let v = Value::Integer(i64::MAX);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn integer_i64_min_roundtrip() {
        let v = Value::Integer(i64::MIN);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Float --

    #[test]
    fn float_roundtrip() {
        let v = Value::Float(FloatValue::new(3.14159));
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn float_nan_roundtrip() {
        let v = Value::Float(FloatValue::new(f64::NAN));
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        // NaN != NaN by IEEE754, but our FloatValue compares bits
        assert_eq!(v, decoded);
    }

    #[test]
    fn float_positive_infinity_roundtrip() {
        let v = Value::Float(FloatValue::new(f64::INFINITY));
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn float_negative_infinity_roundtrip() {
        let v = Value::Float(FloatValue::new(f64::NEG_INFINITY));
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- String --

    #[test]
    fn string_roundtrip() {
        let v = Value::String("hello".into());
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn string_utf8_roundtrip() {
        let v = Value::String("音量".into());
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn string_arabic_roundtrip() {
        let v = Value::String("مستوى".into());
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Binary --

    #[test]
    fn binary_roundtrip() {
        let v = Value::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn binary_empty_roundtrip() {
        let v = Value::Binary(vec![]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Tuple --

    #[test]
    fn tuple_roundtrip() {
        let v = Value::Tuple(vec![
            Value::Integer(0),
            Value::Integer(60),
            Value::Integer(100),
        ]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn tuple_empty_roundtrip() {
        let v = Value::Tuple(vec![]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- List --

    #[test]
    fn list_roundtrip() {
        let v = Value::List(vec![
            Value::Integer(60),
            Value::Integer(62),
            Value::Integer(64),
            Value::Integer(67),
        ]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn list_empty_roundtrip() {
        let v = Value::List(vec![]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Tuple vs List distinction --

    #[test]
    fn tuple_and_list_remain_distinct_through_roundtrip() {
        let tuple = Value::Tuple(vec![Value::Integer(1), Value::Integer(2)]);
        let list = Value::List(vec![Value::Integer(1), Value::Integer(2)]);

        let tuple_encoded = rmp_serde::to_vec(&tuple).unwrap();
        let list_encoded = rmp_serde::to_vec(&list).unwrap();

        let tuple_decoded: Value = rmp_serde::from_slice(&tuple_encoded).unwrap();
        let list_decoded: Value = rmp_serde::from_slice(&list_encoded).unwrap();

        // They must decode back to their original types
        assert!(matches!(tuple_decoded, Value::Tuple(_)));
        assert!(matches!(list_decoded, Value::List(_)));

        // And they must not be equal
        assert_ne!(tuple_decoded, list_decoded);
    }

    // -- Map --

    #[test]
    fn map_roundtrip() {
        let mut m = BTreeMap::new();
        m.insert("note".into(), Value::Integer(60));
        m.insert("velocity".into(), Value::Integer(100));
        let v = Value::Map(m);

        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn map_empty_roundtrip() {
        let v = Value::Map(BTreeMap::new());
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn map_ordering_does_not_affect_equality() {
        // BTreeMap is sorted by key, so insertion order doesn't matter
        let mut m1 = BTreeMap::new();
        m1.insert("a".into(), Value::Integer(1));
        m1.insert("b".into(), Value::Integer(2));

        let mut m2 = BTreeMap::new();
        m2.insert("b".into(), Value::Integer(2));
        m2.insert("a".into(), Value::Integer(1));

        assert_eq!(Value::Map(m1), Value::Map(m2));
    }

    // -- TypedBinary --

    #[test]
    fn typed_binary_roundtrip() {
        let v = Value::TypedBinary {
            tag: "ensemble/f32".into(),
            data: vec![0x00, 0x00, 0x80, 0x3F], // 1.0f32 in little-endian
        };
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn typed_binary_tag_preservation() {
        let v = Value::TypedBinary {
            tag: "org.example.matrix".into(),
            data: vec![1, 2, 3, 4],
        };
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();

        if let Value::TypedBinary { tag, data } = decoded {
            assert_eq!(tag, "org.example.matrix");
            assert_eq!(data, vec![1, 2, 3, 4]);
        } else {
            panic!("Expected TypedBinary");
        }
    }

    // -- Nesting --

    #[test]
    fn nested_map_with_list_roundtrip() {
        let mut inner_map = BTreeMap::new();
        inner_map.insert("name".into(), Value::String("Bass".into()));
        inner_map.insert("notes".into(), Value::List(vec![
            Value::Integer(60),
            Value::Integer(62),
            Value::Integer(64),
        ]));

        let mut outer_map = BTreeMap::new();
        outer_map.insert("track".into(), Value::Map(inner_map));

        let v = Value::Map(outer_map);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn list_of_tuples_roundtrip() {
        let v = Value::List(vec![
            Value::Tuple(vec![Value::Integer(0), Value::Integer(60), Value::Integer(100)]),
            Value::Tuple(vec![Value::Integer(0), Value::Integer(64), Value::Integer(100)]),
            Value::Tuple(vec![Value::Integer(0), Value::Integer(67), Value::Integer(100)]),
        ]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Conformance fixtures from conformance-testing.md --

    #[test]
    fn conformance_integer() {
        let v = Value::Integer(42);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn conformance_float() {
        let v = Value::Float(FloatValue::new(3.14159));
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn conformance_nan() {
        let v = Value::Float(FloatValue::new(f64::NAN));
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn conformance_utf8_string() {
        let v = Value::String("音量".into());
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn conformance_tuple() {
        let v = Value::Tuple(vec![
            Value::Integer(0),
            Value::Integer(60),
            Value::Integer(100),
        ]);
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn conformance_map() {
        let mut m = BTreeMap::new();
        m.insert("note".into(), Value::Integer(60));
        m.insert("velocity".into(), Value::Integer(100));
        let v = Value::Map(m);

        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn conformance_typed_binary() {
        let v = Value::TypedBinary {
            tag: "ensemble/f32".into(),
            data: vec![0x00, 0x00, 0x80, 0x3F],
        };
        let encoded = rmp_serde::to_vec(&v).unwrap();
        let decoded: Value = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(v, decoded);
    }

    // -- Type discrimination --

    #[test]
    fn different_types_are_not_equal() {
        let int_val = Value::Integer(42);
        let float_val = Value::Float(FloatValue::new(42.0));
        let string_val = Value::String("42".into());

        assert_ne!(int_val, float_val);
        assert_ne!(int_val, string_val);
        assert_ne!(float_val, string_val);
    }

    #[test]
    fn null_is_distinct_from_other_types() {
        assert_ne!(Value::Null, Value::Bool(false));
        assert_ne!(Value::Null, Value::Integer(0));
        assert_ne!(Value::Null, Value::String("".into()));
    }
}
