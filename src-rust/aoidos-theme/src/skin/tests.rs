use super::*;

#[test]
fn parse_rejects_hard_input_limits_and_hashes_original_content() {
    assert_eq!(
        parse("mistbell", &"x".repeat(MAX_SKIN_BYTES + 1)).unwrap_err(),
        SkinError::InvalidSkin
    );
    assert_eq!(parse("mistbell", "\0").unwrap_err(), SkinError::InvalidSkin);
    let parsed = parse("mistbell", "\u{feff}/* theme */").unwrap();
    assert_eq!(parsed.script_id, "mistbell");
    assert_eq!(parsed.status, SkinStatus::Valid);
    assert_eq!(parsed.source_hash.as_ref().unwrap().len(), 64);
    assert_eq!(
        parse("mistbell", "\u{feff}\u{feff}/* theme */")
            .unwrap()
            .status,
        SkinStatus::Valid
    );
    assert_eq!(parse("unknown", "").unwrap_err(), SkinError::UnknownScript);
}

#[test]
fn unloaded_skin_returns_only_static_error() {
    assert!(matches!(
        load(std::path::Path::new("."), "unknown"),
        Err(SkinError::UnknownScript)
    ));
}

#[test]
fn bounded_read_errors_keep_skin_domain_classification() {
    assert_eq!(
        map_read_error(aoidos_store::error::StoreError::Corrupt(
            "invalid text".into()
        )),
        SkinError::InvalidSkin
    );
    assert_eq!(
        map_read_error(aoidos_store::error::StoreError::from_io(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied)
        )),
        SkinError::Storage
    );
}

#[test]
fn missing_skin_file_is_empty_while_other_metadata_failures_are_storage_errors() {
    let missing = missing_or_storage_error(
        std::io::Error::from(std::io::ErrorKind::NotFound),
        "mistbell",
    )
    .unwrap();
    assert_eq!(missing.status, SkinStatus::Missing);
    assert_eq!(
        missing_or_storage_error(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            "mistbell"
        )
        .unwrap_err(),
        SkinError::Storage
    );
}

#[test]
fn valid_skin_normalizes_one_shared_token_set_and_resolves_references() {
    let skin = parse(
        "mistbell",
        r#"
        :root {
          --ink: #123;
          --accent: #abc;
          --muted: var(--ink);
          --bubble-user: var(--ink);
          --record-left: 12px;
          --dur-micro: 0.2s;
          --ease-signature: cubic-bezier(0.2, 0.8, 0.2, 1);
          --app-bg: linear-gradient(135deg, #000 0%, #fff 100%);
        }
    "#,
    )
    .unwrap();
    assert_eq!(skin.tokens["--accent"], "#aabbccff");
    assert_eq!(skin.tokens["--muted"], "#112233ff");
    assert_eq!(skin.tokens["--bubble-user"], "#112233ff");
    assert_eq!(skin.tokens["--record-left"], "12px");
    assert_eq!(skin.tokens["--dur-micro"], "200ms");
    assert_eq!(
        skin.tokens["--app-bg"],
        "linear-gradient(135deg, #000 0%, #fff 100%)"
    );
    assert!(!skin.tokens.contains_key("--accent-violet"));
}

#[test]
fn gradient_stops_resolve_color_references_and_preserve_base_theme_references() {
    let skin = parse(
        "mistbell",
        ":root { --accent: #abc; --app-bg: linear-gradient(var(--accent) 0%, var(--muted) 100%); }",
    )
    .unwrap();
    assert_eq!(
        skin.tokens["--app-bg"],
        "linear-gradient(#aabbccff 0%, var(--muted) 100%)"
    );

    let wrong_kind = parse(
        "mistbell",
        ":root { --app-bg: linear-gradient(var(--record-left) 0%, #fff 100%); }",
    )
    .unwrap();
    assert!(!wrong_kind.tokens.contains_key("--app-bg"));
    assert!(
        wrong_kind
            .warnings
            .iter()
            .any(|warning| warning.code == WarningCode::InvalidReference)
    );

    let cycle = parse("mistbell", ":root { --app-bg: var(--app-bg); }").unwrap();
    assert!(!cycle.tokens.contains_key("--app-bg"));
    assert!(
        cycle
            .warnings
            .iter()
            .any(|warning| warning.code == WarningCode::CyclicReference)
    );

    let dependent_cycle = parse(
        "mistbell",
        ":root { --accent: var(--muted); --muted: var(--accent); --app-bg: linear-gradient(var(--accent) 0%, #fff 100%); }",
    )
    .unwrap();
    assert!(!dependent_cycle.tokens.contains_key("--app-bg"));
    assert!(
        dependent_cycle
            .warnings
            .iter()
            .any(|warning| warning.token.as_deref() == Some("--app-bg"))
    );
}

#[test]
fn invalid_later_duplicate_does_not_erase_the_last_valid_declaration() {
    let skin = parse(
        "mistbell",
        ":root { --accent: #123456; --accent: 18px; --muted: #abc; --muted: var(--dur-micro); }",
    )
    .unwrap();
    assert_eq!(skin.tokens["--accent"], "#123456ff");
    assert_eq!(skin.tokens["--muted"], "#aabbccff");
    assert_eq!(skin.warnings.len(), 2);
    assert!(
        skin.warnings
            .iter()
            .all(|warning| warning.code == WarningCode::InvalidValue
                || warning.code == WarningCode::InvalidReference)
    );
}

#[test]
fn resolver_rejects_invalid_internal_candidates_with_typed_warnings() {
    let declarations = BTreeMap::from([
        (
            "--accent".into(),
            Declaration {
                name: "--accent".into(),
                value: "var(--dur-micro)".into(),
                line: 1,
                important: false,
            },
        ),
        (
            "--muted".into(),
            Declaration {
                name: "--muted".into(),
                value: "not-a-color".into(),
                line: 2,
                important: false,
            },
        ),
    ]);
    let mut warnings = Vec::new();
    resolve_tokens(&declarations, &mut warnings).unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.code == WarningCode::InvalidReference)
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.code == WarningCode::InvalidValue)
    );
}

#[test]
fn bundled_mistbell_example_is_a_valid_script_skin() {
    let skin = parse(
        "mistbell",
        include_str!("../../../../resources/scripts/mistbell/theme.css"),
    )
    .unwrap();

    assert_eq!(skin.status, SkinStatus::Valid);
    assert!(skin.warnings.is_empty());
    assert_eq!(skin.tokens["--accent"], "#86c9d0ff");
}

#[test]
fn invalid_semantic_values_are_warnings_and_protected_values_are_rejected() {
    let skin = parse(
        "mistbell",
        r#"
      :root {
        --accent: 18px;
        --warning: #000;
        --unknown: url(https://example.test/a.png);
        --muted: var(--dur-micro);
      }
    "#,
    )
    .unwrap();
    assert!(!skin.tokens.contains_key("--accent"));
    assert!(!skin.tokens.contains_key("--warning"));
    assert!(!skin.tokens.contains_key("--unknown"));
    assert_eq!(skin.warnings.len(), 4);
    assert!(
        skin.warnings
            .iter()
            .any(|warning| warning.code == WarningCode::ProtectedToken)
    );
    assert!(
        skin.warnings
            .iter()
            .any(|warning| warning.code == WarningCode::InvalidReference)
    );
}

#[test]
fn reference_cycles_and_unsafe_gradient_functions_do_not_reach_output() {
    let cycle = parse(
        "mistbell",
        r#":root { --accent: var(--muted); --muted: var(--accent); }"#,
    )
    .unwrap();
    assert!(!cycle.tokens.contains_key("--accent"));
    assert!(!cycle.tokens.contains_key("--muted"));
    assert!(
        cycle
            .warnings
            .iter()
            .any(|warning| warning.code == WarningCode::CyclicReference)
    );
    let unsafe_value = parse(
        "mistbell",
        r#":root { --app-bg: linear-gradient(0deg, url(https://example.test/x), #000); }"#,
    )
    .unwrap();
    assert!(!unsafe_value.tokens.contains_key("--app-bg"));
}

#[test]
fn stylesheet_structure_and_hard_budgets_reject_the_entire_file() {
    for source in [
        ":root[data-script=other] { --accent: #fff; }",
        "@import url(https://example.test/x);",
        ":root[data-theme=dark], :root[data-theme=light] { --accent: #fff; }",
        ":root[data-theme=dark] { .child { --accent: #fff; } }",
        ":root[data-theme=dark] { --accent: \"unterminated; }",
        ":root { --accent: #abc;",
        ":root { --accent: #abc; --app-bg: linear-gradient(0deg, #000, #fff; }",
        ":root { --accent: #abc; --unknown: [value; }",
        ":root { --accent: #abc; --app-bg: linear-gradient(0deg, #000], #fff); }",
        ":root { --accent: #abc; --unknown: \"unterminated",
        ":root { --accent: #abc; /* unterminated",
    ] {
        assert_eq!(
            parse("mistbell", source).unwrap_err(),
            SkinError::InvalidSkin,
            "{source}"
        );
    }
    let declarations = (0..129)
        .map(|index| format!("--unknown-{index}: #fff;"))
        .collect::<String>();
    let oversized_count = format!(":root {{{declarations}}}");
    assert_eq!(
        parse("mistbell", &oversized_count).unwrap_err(),
        SkinError::InvalidSkin
    );
    let too_many_rules = ":root {}".repeat(9);
    assert_eq!(
        parse("mistbell", &too_many_rules).unwrap_err(),
        SkinError::InvalidSkin
    );
}

#[test]
fn warning_lines_are_one_based_and_block_delimiters_ignore_comments_and_strings() {
    let skin = parse(
        "mistbell",
        ":root { /* } ] ) */\n  --unknown: \"} ] )\";\n  --accent: invalid;\n}",
    )
    .unwrap();

    assert!(!skin.tokens.contains_key("--accent"));
    assert_eq!(
        skin.warnings
            .iter()
            .map(|warning| warning.line)
            .collect::<Vec<_>>(),
        [Some(2), Some(3)]
    );
}

#[test]
fn structural_preflight_checks_each_block_kind_and_the_nesting_budget() {
    let valid = parse(
        "mistbell",
        ":root { --unknown: [value (nested)]; --accent: rgb(1, 2, 3); }",
    )
    .unwrap();
    assert_eq!(valid.tokens["--accent"], "#010203ff");
    assert_eq!(valid.warnings[0].code, WarningCode::UnknownToken);

    for source in ["]", ")", "}", "/* missing terminator", "url(bad\"url)"] {
        assert!(!has_balanced_blocks(source), "{source}");
    }
    for delimiters in [('(', ')'), ('[', ']'), ('{', '}')] {
        let source = format!(
            "{}value{}",
            delimiters.0.to_string().repeat(10),
            delimiters.1.to_string().repeat(10)
        );
        assert!(!has_balanced_blocks(&source), "{source}");
    }
}

#[test]
fn closing_delimiter_lookup_skips_css_whitespace_and_complete_comments() {
    assert_eq!(
        next_non_comment_char(" \t\n\r\u{c}/* ignored */ /* again */}", 0),
        Some('}')
    );
    assert_eq!(next_non_comment_char("/* incomplete", 0), None);
    assert_eq!(next_non_comment_char(" \t", 0), None);
}

#[test]
fn warnings_are_bounded_and_missing_skin_omits_source_hash() {
    let declarations = (0..40)
        .map(|index| format!("--unknown-{index}: #fff;"))
        .collect::<String>();
    let source = format!(":root {{{declarations}}}");
    let skin = parse("mistbell", &source).unwrap();
    assert_eq!(skin.warnings.len(), 32);
    assert!(skin.warnings_truncated);

    let root = std::env::temp_dir().join(format!("aoidos-theme-missing-{}", std::process::id()));
    std::fs::create_dir_all(root.join("mistbell")).unwrap();
    let missing = load(&root, "mistbell").unwrap();
    assert_eq!(missing.status, SkinStatus::Missing);
    assert!(missing.source_hash.is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn color_normalization_handles_supported_forms_and_rejects_invalid_components() {
    let cases = [
        ("transparent", Some("#00000000")),
        ("#AbC", Some("#aabbccff")),
        ("#abcd", Some("#aabbccdd")),
        ("#AABBCC", Some("#aabbccff")),
        ("#AABBCCDD", Some("#aabbccdd")),
        ("rgb(1, 2, 255)", Some("#0102ffff")),
        ("rgba(255, 0, 1, 0.5)", Some("#ff000180")),
        ("hsl(0, 100%, 50%)", Some("#ff0000ff")),
        ("hsl(120, 100%, 50%)", Some("#00ff00ff")),
        ("hsl(180, 100%, 50%)", Some("#00ffffff")),
        ("hsl(240, 100%, 50%)", Some("#0000ffff")),
        ("hsl(300, 100%, 50%)", Some("#ff00ffff")),
        ("hsl(360, 100%, 50%)", Some("#ff0000ff")),
        ("hsla(60, 100%, 50%, 0)", Some("#ffff0000")),
        ("hsl(1, 2)", None),
        ("#12", None),
        ("#ggg", None),
        ("rgb(1, 2)", None),
        ("rgb(1, 2, 256)", None),
        ("rgba(1, 2, 3, 2)", None),
        ("hsl(361, 50%, 50%)", None),
        ("hsl(10, 101%, 50%)", None),
        ("hsl(10, 50%, -1%)", None),
        ("rgba(1, 2, 3, NaN)", None),
        ("rgb(1, nested(2), 3)", None),
        ("var(--ink)", None),
    ];
    for (input, expected) in cases {
        assert_eq!(normalize_color(input).as_deref(), expected, "{input}");
    }
}

#[test]
fn scalar_normalizers_enforce_units_ranges_and_supported_easing() {
    assert_eq!(
        bounded_unit(" 12px ", "px", 4.0, 24.0).as_deref(),
        Some("12px")
    );
    for input in ["3px", "25px", "NaNpx", "12em", "12 px"] {
        assert!(bounded_unit(input, "px", 4.0, 24.0).is_none(), "{input}");
    }
    assert_eq!(
        normalize_literal(crate::catalog::ValueKind::Duration, "250ms").as_deref(),
        Some("250ms")
    );
    for input in ["79ms", "801ms", "NaNms", "0.07s", "0.81s", "NaNs", "0.2 s"] {
        assert!(
            normalize_literal(crate::catalog::ValueKind::Duration, input).is_none(),
            "{input}"
        );
    }
    for easing in ["linear", "ease", "ease-in", "ease-out", "ease-in-out"] {
        assert_eq!(normalize_easing(easing).as_deref(), Some(easing));
    }
    assert_eq!(
        normalize_easing("cubic-bezier(0.1, 0.2, 0.8, 1)").as_deref(),
        Some("cubic-bezier(0.1,0.2,0.8,1)")
    );
    for easing in [
        "cubic-bezier(0, 0, 1)",
        "cubic-bezier(-1, 0, 1, 1)",
        "spring",
    ] {
        assert!(normalize_easing(easing).is_none(), "{easing}");
    }
}

#[test]
fn gradient_validation_accepts_safe_layers_and_rejects_unsafe_or_malformed_values() {
    for gradient in [
        "linear-gradient(135deg, #000 0%, #fff 100%)",
        "radial-gradient(circle at 20% 30%, rgba(1, 2, 3, 0.5) 0% 8%, transparent 28%)",
        "linear-gradient(#000 0%, #fff 100%), radial-gradient(#111, #eee)",
        "linear-gradient(0deg, #000, #fff)",
        "linear-gradient(0deg, RGB(1, 2, 3), HSL(120, 100%, 50%))",
        "linear-gradient(0deg, TRANSPARENT\t0%, #fff 100%)",
        "linear-gradient(0deg, var(--accent) 0%, #fff 100%)",
    ] {
        assert!(normalize_gradient(gradient).is_some(), "{gradient}");
    }
    for gradient in [
        "",
        "linear-gradient(361deg, #000, #fff)",
        "linear-gradient(0deg, #000)",
        "conic-gradient(#000, #fff)",
        "linear-gradient(0deg, url(https://example.test), #fff)",
        "linear-gradient(0deg, #000, #fff",
        "linear-gradient(0deg, #000, #fff) trailing",
        "linear-gradient(0deg, #000 70% 20%, #fff)",
        "linear-gradient(0deg, #000 0% 1% 2%, #fff)",
        "linear-gradient(0deg, var(-accent) 0%, #fff 100%)",
        "radial-gradient(circle at 101% 20%, #000, #fff)",
        "linear-gradient(0deg, #000, #fff); color:red",
        "linear-gradient(0deg, #000, #fff), linear-gradient(0deg, #000, #fff), linear-gradient(0deg, #000, #fff), linear-gradient(0deg, #000, #fff), linear-gradient(0deg, #000, #fff)",
    ] {
        assert!(normalize_gradient(gradient).is_none(), "{gradient}");
    }
    assert_eq!(
        split_top_level("fn(a,b), c", ','),
        Some(vec!["fn(a,b)", "c"])
    );
    assert!(split_top_level("fn(a", ',').is_none());
    assert!(split_top_level("fn(a))", ',').is_none());
    assert!(percent("50%").is_some());
    for value in ["101%", "NaN%", "50px"] {
        assert!(percent(value).is_none(), "{value}");
    }
    assert_eq!(
        parse("mistbell", ":root { --accent: var(--muted); }")
            .unwrap()
            .tokens["--accent"],
        "var(--muted)"
    );
}

#[test]
fn stylesheet_parser_rejects_bad_root_tokens_and_preserves_warning_safety() {
    for source in [
        ":rootish { --accent: #fff; }",
        ":root.extra { --accent: #fff; }",
        ":root { @media (min-width: 1px) { --accent: #fff; } }",
        ":root { .nested { --accent: #fff; } }",
        ":root { --accent: #fff; --muted: 'bad\nstring'; }",
    ] {
        assert_eq!(
            parse("mistbell", source).unwrap_err(),
            SkinError::InvalidSkin,
            "{source}"
        );
    }
    assert!(is_important("#abc !important"));
    let important = parse("mistbell", ":root { --accent: #abc !important; }").unwrap();
    assert_eq!(important.warnings[0].code, WarningCode::InvalidDeclaration);
    assert!(!is_important("#abc !important extra"));

    assert_eq!(
        parse("mistbell", ":root { --bad$name: red; }").unwrap_err(),
        SkinError::InvalidSkin
    );
    assert_eq!(safe_token_name("--unknown").as_deref(), Some("--unknown"));
    assert!(safe_token_name("--bad$name").is_none());
}

#[test]
fn loading_rejects_missing_roots_non_files_invalid_utf8_and_oversize_files() {
    let root =
        std::env::temp_dir().join(format!("aoidos-theme-load-errors-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(load(&root, "mistbell").unwrap_err(), SkinError::Storage);

    let script = root.join("mistbell");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&script, "not a script directory").unwrap();
    assert_eq!(load(&root, "mistbell").unwrap_err(), SkinError::Storage);
    std::fs::remove_file(&script).unwrap();
    std::fs::create_dir_all(script.join("theme.css")).unwrap();
    assert_eq!(load(&root, "mistbell").unwrap_err(), SkinError::Storage);
    std::fs::remove_dir_all(script.join("theme.css")).unwrap();
    std::fs::write(script.join("theme.css"), [0xff, 0xfe]).unwrap();
    assert_eq!(load(&root, "mistbell").unwrap_err(), SkinError::InvalidSkin);
    std::fs::write(script.join("theme.css"), vec![b'x'; MAX_SKIN_BYTES + 1]).unwrap();
    assert_eq!(load(&root, "mistbell").unwrap_err(), SkinError::InvalidSkin);
    std::fs::write(script.join("theme.css"), ":root { --accent: #abc; }").unwrap();
    let loaded = load(&root, "mistbell").unwrap();
    assert_eq!(loaded.status, SkinStatus::Valid);
    assert_eq!(loaded.tokens["--accent"], "#aabbccff");
    assert!(loaded.source_hash.is_some());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn gradient_stop_rejects_an_unclosed_color_function() {
    assert!(validate_gradient_stop("rgb(1, 2, 3").is_none());
    assert!(validate_gradient_stop("rgba(1, nested(2), 3, 1").is_none());
}

#[cfg(unix)]
#[test]
fn loading_rejects_script_root_symlinks() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!("aoidos-theme-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("real")).unwrap();
    symlink(root.join("real"), root.join("mistbell")).unwrap();
    assert_eq!(load(&root, "mistbell").unwrap_err(), SkinError::Storage);
    let _ = std::fs::remove_dir_all(root);
}
