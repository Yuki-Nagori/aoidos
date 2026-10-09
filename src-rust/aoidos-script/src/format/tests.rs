use super::*;
#[test]
fn released_original_is_preserved_byte_for_byte() {
    let raw = include_bytes!("../../../../resources/scripts/mistbell/Mistbell.md");
    let s = decode("mistbell", raw).unwrap();
    assert_eq!(s.script_id, "mistbell");
    assert_eq!(s.title, "雾钟地窖");
    assert_eq!(s.body.as_bytes(), raw);
    assert_eq!(decode("a", b"# T\r\n\ttext").unwrap().body, "# T\r\n\ttext");
}
#[test]
fn malformed_sources_fail_with_static_errors() {
    assert_eq!(
        decode("a", &vec![b'a'; MAX_BYTES + 1]).unwrap_err(),
        Error::SizeLimit
    );
    assert_eq!(decode("con", b"# T").unwrap_err(), Error::InvalidId);
    assert_eq!(decode("a", &[255]).unwrap_err(), Error::InvalidUtf8);
    for raw in [b"".as_slice(), b"body", b"## T", b"# ", b"# T\0"] {
        assert_eq!(decode("a", raw).unwrap_err(), Error::InvalidHeader);
    }
    assert_eq!(
        decode("a", format!("# {}", "a".repeat(257)).as_bytes()).unwrap_err(),
        Error::InvalidHeader
    );
    for raw in [
        "# T\n[AOIDOS:PLAYER]",
        "# T\n[/AOIDOS:NARRATION]",
        "# T\nbody\0",
    ] {
        assert_eq!(decode("a", raw.as_bytes()).unwrap_err(), Error::InvalidText);
    }
    for error in [
        Error::SizeLimit,
        Error::InvalidUtf8,
        Error::InvalidId,
        Error::InvalidHeader,
        Error::InvalidText,
    ] {
        let _: &dyn std::error::Error = &error;
        assert!(!error.to_string().contains("private-input"));
    }
}
#[test]
fn ids_keep_the_same_spelling_on_every_platform() {
    for id in [
        "", "-a", "a-", "A", "a_b", "a/b", "con", "prn", "aux", "nul", "com1", "lpt9",
    ] {
        assert!(!valid_id(id), "{id}");
    }
    assert!(!valid_id(&"a".repeat(65)));
    for id in ["a", "a-1", "com0", "com10", "lpt0", "mistbell"] {
        assert!(valid_id(id), "{id}");
    }
}

#[test]
fn byte_limits_accept_exact_edges_and_reject_multibyte_overflow() {
    let mut body = String::from("# T\n");
    body.push_str(&"a".repeat(MAX_BYTES - body.len()));
    assert_eq!(decode("a", body.as_bytes()).unwrap().body.len(), MAX_BYTES);
    body.push('中');
    assert_eq!(decode("a", body.as_bytes()).unwrap_err(), Error::SizeLimit);
    let title = format!("{}a", "中".repeat(85));
    assert_eq!(title.len(), 256);
    assert_eq!(
        decode("a", format!("# {title}\n").as_bytes())
            .unwrap()
            .title,
        title
    );
    assert_eq!(
        decode("a", format!("# {title}中\n").as_bytes()).unwrap_err(),
        Error::InvalidHeader
    );
    assert!(valid_id(&"a".repeat(64)));
    assert!(!valid_id(&format!("{}中", "a".repeat(61))));
}
