use super::*;

#[test]
fn preserves_values_and_rejects_duplicate_decoded_keys_at_every_depth() {
    let value = parse(
        r#" {"v":[null,true,false,-1,18446744073709551615,1.5,"中文"],"nested":{"a":1}} "#,
        1024,
    )
    .unwrap();
    assert_eq!(value["v"][4].as_u64(), Some(u64::MAX));
    assert_eq!(value["v"][6], "中文");
    for text in [
        r#"{"a":1,"a":2}"#,
        r#"{"a":null,"\u0061":true}"#,
        r#"[{"v":{"a":1,"a":2}}]"#,
        "{}{}",
        "{",
        "1e999",
        "NaN",
        "",
        "true trailing",
    ] {
        assert_eq!(parse(text, 1024), Err(Error::InvalidJson), "{text}");
    }
    let deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    assert_eq!(parse(&deep, 1024), Err(Error::InvalidJson));
}

#[test]
fn utf8_budget_and_domain_schema_are_independent_of_line_endings() {
    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Config {
        value: u64,
    }
    assert_eq!(parse("\"中\"", 5).unwrap(), "中");
    assert_eq!(parse("\"中\"", 4), Err(Error::SizeLimit));
    assert_eq!(parse("", 0), Err(Error::SizeLimit));
    assert_eq!(decode_bytes::<String>(b"\"ok\"", 4).unwrap(), "ok");
    assert_eq!(decode_bytes::<Value>(&[255], 8), Err(Error::InvalidUtf8));
    assert_eq!(decode_bytes::<Value>(&[255], 0), Err(Error::SizeLimit));
    assert_eq!(decode_bytes::<Value>(b"{}", 1), Err(Error::SizeLimit));
    assert_eq!(decode_bytes::<Value>(b"", 0), Err(Error::SizeLimit));
    assert_eq!(decode_bytes::<Value>(b"{", 8), Err(Error::InvalidJson));
    assert_eq!(
        decode::<Config>("{\"value\":1}\n", 32).unwrap(),
        Config { value: 1 }
    );
    assert_eq!(
        decode::<Config>("{\"value\":1}", 32).unwrap(),
        Config { value: 1 }
    );
    assert_eq!(
        decode::<Config>("{\"value\":\"1\"}", 32),
        Err(Error::InvalidShape)
    );
    assert_eq!(
        decode::<Config>("{\"value\":1,\"extra\":true}", 64),
        Err(Error::InvalidShape)
    );
    assert_eq!(
        decode::<Config>("{\"value\":1,\"value\":2}", 64),
        Err(Error::InvalidJson)
    );
    assert_eq!(decode::<Config>("{}", 1), Err(Error::SizeLimit));
    for (error, message) in [
        (Error::SizeLimit, "JSON input exceeds byte limit"),
        (Error::InvalidUtf8, "JSON input is not valid UTF-8"),
        (Error::InvalidJson, "invalid JSON or duplicate object key"),
        (
            Error::InvalidShape,
            "JSON does not match the requested schema",
        ),
    ] {
        assert_eq!(error.to_string(), message);
    }
}

#[test]
fn nonfinite_deserializer_values_cannot_create_json_numbers() {
    type Decoder = serde::de::value::F64Deserializer<serde::de::value::Error>;
    assert!(Unique::deserialize(Decoder::new(f64::INFINITY)).is_err());
    assert!(Unique::deserialize(Decoder::new(f64::NAN)).is_err());
}

#[test]
fn transferred_visitor_defenses_preserve_supported_numbers_and_reject_binary() {
    assert!(
        Unique::deserialize(serde::de::value::F64Deserializer::<serde::de::value::Error>::new(1.5))
            .is_ok()
    );
    assert!(
        Unique::deserialize(
            serde::de::value::F64Deserializer::<serde::de::value::Error>::new(f64::NAN)
        )
        .is_err()
    );
    assert!(
        Unique::deserialize(serde::de::value::F64Deserializer::<serde_json::Error>::new(
            1.5
        ))
        .is_ok()
    );
    let decoder = serde::de::value::BytesDeserializer::<serde::de::value::Error>::new(b"binary");
    assert!(
        Unique::deserialize(decoder)
            .err()
            .unwrap()
            .to_string()
            .contains("JSON without duplicate keys")
    );
    assert!(
        Unique::deserialize(serde::de::value::F64Deserializer::<serde_json::Error>::new(
            f64::NAN
        ))
        .is_err()
    );
}
