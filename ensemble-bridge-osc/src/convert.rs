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
use rosc::{OscMessage, OscType};

/// Convert an Ensemble value to a list of OSC arguments.
///
/// Tuples and Lists are flattened into multiple arguments.
/// Maps are serialised as JSON strings (OSC has no associative type).
pub fn ensemble_to_osc_args(payload: &Value) -> Vec<OscType> {
    match payload {
        Value::Null => vec![OscType::Nil],
        Value::Bool(b) => vec![OscType::Bool(*b)],
        // Clamp rather than truncate so out-of-range integers stay ordered
        // instead of wrapping to a nonsense value.
        Value::Integer(i) => vec![OscType::Int(
            (*i).clamp(i32::MIN as i64, i32::MAX as i64) as i32
        )],
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
        OscType::Array(arr) => Value::List(arr.content.iter().map(osc_type_to_value).collect()),
        // Other OSC types (Midi, Color, Time) are rare; convert to string representation.
        other => Value::String(format!("{:?}", other)),
    }
}

/// Strip `prefix` from `addr` on a segment boundary.
///
/// Returns `Some(rest)` only when `addr` equals the prefix exactly or
/// continues with a `/`-delimited segment — so a prefix of "/osc/out" does
/// not match "/osc/output". A trailing '/' on the prefix is ignored, making
/// "/osc/out/" behave identically to "/osc/out".
fn strip_prefix_segment<'a>(addr: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = prefix.trim_end_matches('/');
    let rest = addr.strip_prefix(prefix)?;
    if rest.is_empty() || rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

/// Translate an Ensemble address to an OSC address (outbound: Ensemble → OSC).
///
/// Strips the Ensemble prefix and prepends the OSC prefix. The prefix only
/// strips on an exact match or a `/`-delimited segment boundary.
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
///
/// // "/osc/output" is NOT under "/osc/out" (no segment boundary)
/// translate_address_outbound("/osc/output/freq", "/osc/out", "")
///   → None
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
        strip_prefix_segment(ens_addr, ens_prefix)?
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
/// Strips the OSC prefix and prepends the Ensemble inbound prefix. The prefix
/// only strips on an exact match or a `/`-delimited segment boundary;
/// otherwise the address is kept whole.
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
///
/// // "/scx/y" does not match "/sc" on a segment boundary, so nothing strips
/// translate_address_inbound("/scx/y", "/sc", "/osc/in")
///   → "/osc/in/scx/y"
/// ```
pub fn translate_address_inbound(osc_addr: &str, osc_prefix: &str, ens_prefix: &str) -> String {
    // Strip the OSC prefix if present.
    let stripped = if osc_prefix.is_empty() {
        osc_addr
    } else {
        strip_prefix_segment(osc_addr, osc_prefix).unwrap_or(osc_addr)
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
pub fn to_osc_message(
    ens_addr: &str,
    payload: &Value,
    ens_prefix: &str,
    osc_prefix: &str,
) -> Option<OscMessage> {
    let osc_addr = translate_address_outbound(ens_addr, ens_prefix, osc_prefix)?;
    let args = ensemble_to_osc_args(payload);
    Some(OscMessage {
        addr: osc_addr,
        args,
    })
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
    fn test_ensemble_to_osc_integer_clamped() {
        // Out-of-range i64 values clamp to the i32 extremes rather than truncate.
        let args = ensemble_to_osc_args(&Value::Integer(i64::MAX));
        assert!(matches!(args[0], OscType::Int(i32::MAX)));

        let args = ensemble_to_osc_args(&Value::Integer(i64::MIN));
        assert!(matches!(args[0], OscType::Int(i32::MIN)));

        let args = ensemble_to_osc_args(&Value::Integer(i32::MAX as i64 + 1));
        assert!(matches!(args[0], OscType::Int(i32::MAX)));

        let args = ensemble_to_osc_args(&Value::Integer(i32::MIN as i64 - 1));
        assert!(matches!(args[0], OscType::Int(i32::MIN)));
    }

    #[test]
    fn test_ensemble_to_osc_float() {
        // 2.5 is exactly representable in f32, so compare directly.
        let args = ensemble_to_osc_args(&Value::Float(FloatValue::new(2.5)));
        assert_eq!(args.len(), 1);
        if let OscType::Float(f) = args[0] {
            assert_eq!(f, 2.5);
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
    fn test_address_outbound_segment_boundary() {
        // "/osc/output" merely shares a byte prefix with "/osc/out" — no strip.
        assert_eq!(
            translate_address_outbound("/osc/output/freq", "/osc/out", ""),
            None
        );
        assert_eq!(
            translate_address_outbound("/osc/out2/x", "/osc/out", ""),
            None
        );
        // An exact prefix match maps to the OSC root.
        assert_eq!(
            translate_address_outbound("/osc/out", "/osc/out", ""),
            Some("/".to_string())
        );
        // Arbitrarily deep addresses below the prefix strip fine.
        assert_eq!(
            translate_address_outbound("/osc/out/a/very/deep/address", "/osc/out", ""),
            Some("/a/very/deep/address".to_string())
        );
    }

    #[test]
    fn test_address_outbound_trailing_slash_prefix() {
        // A trailing slash on the prefix behaves like the bare prefix.
        assert_eq!(
            translate_address_outbound("/osc/out/synth", "/osc/out/", ""),
            Some("/synth".to_string())
        );
        assert_eq!(
            translate_address_outbound("/osc/output/synth", "/osc/out/", ""),
            None
        );
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
    fn test_address_inbound_segment_boundary() {
        // "/scx/y" does not match "/sc" on a segment boundary — kept whole.
        assert_eq!(
            translate_address_inbound("/scx/y", "/sc", "/osc/in"),
            "/osc/in/scx/y"
        );
        // An exact prefix match maps to the Ensemble prefix root.
        assert_eq!(
            translate_address_inbound("/sc", "/sc", "/osc/in"),
            "/osc/in"
        );
        // Deep addresses below the prefix strip fine.
        assert_eq!(
            translate_address_inbound("/sc/a/very/deep/address", "/sc", "/osc/in"),
            "/osc/in/a/very/deep/address"
        );
        // A trailing slash on the prefix behaves like the bare prefix.
        assert_eq!(
            translate_address_inbound("/sc/synth", "/sc/", "/osc/in"),
            "/osc/in/synth"
        );
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

    #[test]
    fn test_to_osc_message_deep_address() {
        let msg = to_osc_message(
            "/osc/out/deep/nested/addr",
            &Value::Integer(1),
            "/osc/out",
            "",
        )
        .unwrap();
        assert_eq!(msg.addr, "/deep/nested/addr");
        assert_eq!(msg.args.len(), 1);
    }

    #[test]
    fn test_to_osc_message_rejects_off_prefix_address() {
        assert!(to_osc_message("/osc/output/x", &Value::Integer(1), "/osc/out", "").is_none());
    }
}
