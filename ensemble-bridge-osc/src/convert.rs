//! Conversion between Ensemble values and OSC types, plus address translation.
//!
//! ## Value Mapping
//!
//! **OSC → Ensemble:**
//! - `i` (i32) → `Integer(i64)`
//! - `f` (f32) → `Float(f64)`
//! - `s` (String) → `String`
//! - `b` (Blob) → `Binary(Vec<u8>)`
//! - `T` / `F` → `Bool`
//! - `N` (Nil) → `Null`
//!
//! **Ensemble → OSC:**
//! - `Integer` → `i` (i32, clamped)
//! - `Float` → `f` (f32)
//! - `String` → `s`
//! - `Binary` → `b`
//! - `Bool` → `T` / `F`
//! - `Null` → `N`
//! - `Tuple` / `List` → multiple OSC args (flattened)
//! - `Map` → JSON string (not directly representable)

use ensemble_core::protocol::*;
use rosc::{OscType, OscMessage};

/// Convert an Ensemble value to a list of OSC arguments.
///
/// Tuples and Lists are flattened into multiple arguments.
/// Maps are serialised as JSON strings (OSC has no associative type).
pub fn ensemble_to_osc_args(payload: &Value) -> Vec<OscType> {
    match payload {
        Value::Null => vec![OscType::Nil],
        Value::Bool(b) => vec![OscType::Bool(*b)],
        Value::Integer(i) => vec![OscType::Int(*i as i32)],
        Value::Float(f) => vec![OscType::Float(f.value() as f32)],
        Value::String(s) => vec![OscType::String(s.clone())],
        Value::Binary(b) => vec![OscType::Blob(b.clone())],
        Value::Tuple(items) | Value::List(items) => {
            items.iter().flat_map(ensemble_to_osc_args).collect()
        }
        Value::Map(m) => {
            // Serialise as JSON string (OSC has no associative type).
            let json = serde_json::to_string(m).unwrap_or_else(|_| "{}".to_string());
            vec![OscType::String(json)]
        }
        Value::TypedBinary { tag, data } => {
            // Encode as blob with tag prefix (not standard OSC, but preserves info).
            let mut blob = tag.as_bytes().to_vec();
            blob.push(0); // null separator
            blob.extend_from_slice(data);
            vec![OscType::Blob(blob)]
        }
    }
}

/// Convert OSC arguments to an Ensemble value.
///
/// Single argument → single value.
/// Multiple arguments → Tuple.
/// No arguments → Null.
pub fn osc_to_ensemble_value(args: &[OscType]) -> Value {
    match args.len() {
        0 => Value::Null,
        1 => osc_type_to_value(&args[0]),
        _ => Value::Tuple(args.iter().map(osc_type_to_value).collect()),
    }
}

/// Convert a single OSC type to an Ensemble value.
fn osc_type_to_value(osc: &OscType) -> Value {
    match osc {
        OscType::Int(i) => Value::Integer(*i as i64),
        OscType::Float(f) => Value::Float(FloatValue::new(*f as f64)),
        OscType::String(s) => Value::String(s.clone()),
        OscType::Blob(b) => Value::Binary(b.clone()),
        OscType::Bool(b) => Value::Bool(*b),
        OscType::Nil => Value::Null,
        OscType::Array(arr) => {
            Value::List(arr.content.iter().map(osc_type_to_value).collect())
        }
        // Other OSC types (Midi, Color, Time) are rare; convert to string representation.
        other => Value::String(format!("{:?}", other)),
    }
}

/// Translate an Ensemble address to an OSC address (outbound: Ensemble → OSC).
///
/// Strips the Ensemble prefix and prepends the OSC prefix.
/// Returns `None` if the address doesn't match the Ensemble prefix.
///
/// # Examples
///
/// ```
/// // With ens_prefix="/osc/out", osc_prefix=""
/// translate_address_outbound("/osc/out/synth/freq", "/osc/out", "")
///   → Some("/synth/freq")
///
/// // With ens_prefix="/osc/out", osc_prefix="/sc"
/// translate_address_outbound("/osc/out/synth/freq", "/osc/out", "/sc")
///   → Some("/sc/synth/freq")
/// ```
pub fn translate_address_outbound(
    ens_addr: &str,
    ens_prefix: &str,
    osc_prefix: &str,
) -> Option<String> {
    // Check if the address starts with the Ensemble prefix.
    let stripped = if ens_prefix.is_empty() {
        ens_addr
    } else {
        ens_addr.strip_prefix(ens_prefix)?
    };

    // Build the OSC address.
    let osc_addr = if osc_prefix.is_empty() {
        if stripped.is_empty() {
            "/".to_string()
        } else if !stripped.starts_with('/') {
            format!("/{}", stripped)
        } else {
            stripped.to_string()
        }
    } else if stripped.is_empty() {
        osc_prefix.to_string()
    } else if stripped.starts_with('/') {
        format!("{}{}", osc_prefix, stripped)
    } else {
        format!("{}/{}", osc_prefix, stripped)
    };

    Some(osc_addr)
}

/// Translate an OSC address to an Ensemble address (inbound: OSC → Ensemble).
///
/// Strips the OSC prefix and prepends the Ensemble inbound prefix.
///
/// # Examples
///
/// ```
/// // With osc_prefix="", ens_prefix="/osc/in"
/// translate_address_inbound("/slider/1", "", "/osc/in")
///   → "/osc/in/slider/1"
///
/// // With osc_prefix="/sc", ens_prefix="/osc/in"
/// translate_address_inbound("/sc/synth/freq", "/sc", "/osc/in")
///   → "/osc/in/synth/freq"
/// ```
pub fn translate_address_inbound(
    osc_addr: &str,
    osc_prefix: &str,
    ens_prefix: &str,
) -> String {
    // Strip the OSC prefix if present.
    let stripped = if osc_prefix.is_empty() {
        osc_addr
    } else {
        osc_addr.strip_prefix(osc_prefix).unwrap_or(osc_addr)
    };

    // Build the Ensemble address.
    if ens_prefix.is_empty() {
        if stripped.is_empty() {
            "/".to_string()
        } else if !stripped.starts_with('/') {
            format!("/{}", stripped)
        } else {
            stripped.to_string()
        }
    } else if stripped.is_empty() {
        ens_prefix.to_string()
    } else if stripped.starts_with('/') {
        format!("{}{}", ens_prefix, stripped)
    } else {
        format!("{}/{}", ens_prefix, stripped)
    }
}

/// Create an OSC message from an address and Ensemble payload.
pub fn to_osc_message(ens_addr: &str, payload: &Value, ens_prefix: &str, osc_prefix: &str) -> Option<OscMessage> {
    let osc_addr = translate_address_outbound(ens_addr, ens_prefix, osc_prefix)?;
    let args = ensemble_to_osc_args(payload);
    Some(OscMessage { addr: osc_addr, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensemble_to_osc_null() {
        let args = ensemble_to_osc_args(&Value::Null);
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0], OscType::Nil));
    }

    #[test]
    fn test_ensemble_to_osc_bool() {
        let args = ensemble_to_osc_args(&Value::Bool(true));
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0], OscType::Bool(true)));

        let args = ensemble_to_osc_args(&Value::Bool(false));
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0], OscType::Bool(false)));
    }

    #[test]
    fn test_ensemble_to_osc_integer() {
        let args = ensemble_to_osc_args(&Value::Integer(42));
        assert_eq!(args.len(), 1);
        if let OscType::Int(i) = args[0] {
            assert_eq!(i, 42);
        } else {
            panic!("Expected Int");
        }
    }

    #[test]
    fn test_ensemble_to_osc_float() {
        let args = ensemble_to_osc_args(&Value::Float(FloatValue::new(3.14)));
        assert_eq!(args.len(), 1);
        if let OscType::Float(f) = args[0] {
            assert!((f - 3.14).abs() < 0.001);
        } else {
            panic!("Expected Float");
        }
    }

    #[test]
    fn test_ensemble_to_osc_string() {
        let args = ensemble_to_osc_args(&Value::String("hello".into()));
        assert_eq!(args.len(), 1);
        if let OscType::String(s) = &args[0] {
            assert_eq!(s, "hello");
        } else {
            panic!("Expected String");
        }
    }

    #[test]
    fn test_ensemble_to_osc_binary() {
        let args = ensemble_to_osc_args(&Value::Binary(vec![1, 2, 3]));
        assert_eq!(args.len(), 1);
        if let OscType::Blob(b) = &args[0] {
            assert_eq!(b, &vec![1, 2, 3]);
        } else {
            panic!("Expected Blob");
        }
    }

    #[test]
    fn test_ensemble_to_osc_tuple_flattened() {
        let args = ensemble_to_osc_args(&Value::Tuple(vec![
            Value::Integer(1),
            Value::Float(FloatValue::new(2.0)),
            Value::String("three".into()),
        ]));
        assert_eq!(args.len(), 3);
        assert!(matches!(args[0], OscType::Int(1)));
        assert!(matches!(args[1], OscType::Float(_)));
        assert!(matches!(args[2], OscType::String(_)));
    }

    #[test]
    fn test_osc_to_ensemble_empty() {
        let val = osc_to_ensemble_value(&[]);
        assert!(matches!(val, Value::Null));
    }

    #[test]
    fn test_osc_to_ensemble_single_int() {
        let val = osc_to_ensemble_value(&[OscType::Int(42)]);
        if let Value::Integer(i) = val {
            assert_eq!(i, 42);
        } else {
            panic!("Expected Integer");
        }
    }

    #[test]
    fn test_osc_to_ensemble_multiple_becomes_tuple() {
        let val = osc_to_ensemble_value(&[
            OscType::Int(1),
            OscType::Float(2.0),
            OscType::String("three".into()),
        ]);
        if let Value::Tuple(items) = val {
            assert_eq!(items.len(), 3);
            assert!(matches!(items[0], Value::Integer(1)));
            assert!(matches!(items[1], Value::Float(_)));
            assert!(matches!(items[2], Value::String(_)));
        } else {
            panic!("Expected Tuple");
        }
    }

    #[test]
    fn test_address_outbound_basic() {
        let result = translate_address_outbound("/osc/out/synth/freq", "/osc/out", "");
        assert_eq!(result, Some("/synth/freq".to_string()));
    }

    #[test]
    fn test_address_outbound_with_osc_prefix() {
        let result = translate_address_outbound("/osc/out/synth/freq", "/osc/out", "/sc");
        assert_eq!(result, Some("/sc/synth/freq".to_string()));
    }

    #[test]
    fn test_address_outbound_no_match() {
        let result = translate_address_outbound("/other/path", "/osc/out", "");
        assert_eq!(result, None);
    }

    #[test]
    fn test_address_inbound_basic() {
        let result = translate_address_inbound("/slider/1", "", "/osc/in");
        assert_eq!(result, "/osc/in/slider/1");
    }

    #[test]
    fn test_address_inbound_with_osc_prefix() {
        let result = translate_address_inbound("/sc/synth/freq", "/sc", "/osc/in");
        assert_eq!(result, "/osc/in/synth/freq");
    }

    #[test]
    fn test_address_roundtrip() {
        let ens_addr = "/osc/out/synth/freq";
        let ens_prefix = "/osc/out";
        let osc_prefix = "";

        let osc_addr = translate_address_outbound(ens_addr, ens_prefix, osc_prefix).unwrap();
        assert_eq!(osc_addr, "/synth/freq");

        let back = translate_address_inbound(&osc_addr, osc_prefix, "/osc/in");
        assert_eq!(back, "/osc/in/synth/freq");
    }
}
