use super::*;
use serde::Serializer;

struct Fails;
impl Serialize for Fails {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "sensitive input must stay private",
        ))
    }
}
#[derive(Serialize)]
struct Ordered {
    z: u64,
    a: Value,
}

#[test]
fn ordinary_encoding_keeps_field_order_and_canonical_encoding_sorts_nested_objects_only() {
    let value = Ordered {
        z: u64::MAX,
        a: serde_json::json!({"nested":{"z":"中\n文", "a":1},"items":[2,1]}),
    };
    let ordinary = to_string(&value).unwrap();
    assert!(ordinary.starts_with("{\"z\":18446744073709551615,\"a\":"));
    assert_eq!(to_vec(&value).unwrap(), ordinary.as_bytes());
    let encoded = to_value(&value).unwrap();
    assert_eq!(encoded["a"]["items"], serde_json::json!([2, 1]));
    let canonical = canonical_string(&value).unwrap();
    assert_eq!(
        canonical,
        "{\"a\":{\"items\":[2,1],\"nested\":{\"a\":1,\"z\":\"中\\n文\"}},\"z\":18446744073709551615}"
    );
    assert_eq!(
        from_value::<u64>(serde_json::json!(u64::MAX)).unwrap(),
        u64::MAX
    );
    assert_eq!(
        from_value::<u64>(serde_json::json!("1")),
        Err(Error::InvalidShape)
    );
    assert_eq!(decode_value::<u64>(&serde_json::json!(1)).unwrap(), 1);
    assert_eq!(
        decode_value::<u64>(&serde_json::json!("1")),
        Err(Error::InvalidShape)
    );
}

#[test]
fn serialization_errors_have_one_stable_diagnostic_without_source_contents() {
    assert_eq!(to_value(&Fails), Err(Error::InvalidEncoding));
    assert_eq!(to_string(&Fails), Err(Error::InvalidEncoding));
    assert_eq!(to_vec(&Fails), Err(Error::InvalidEncoding));
    assert_eq!(canonical_string(&Fails), Err(Error::InvalidEncoding));
    assert_eq!(
        Error::InvalidEncoding.to_string(),
        "value cannot be encoded as JSON"
    );
}
