use super::*;

const APP_CSS: &str = include_str!("../../../../src-web/app.css");
const GENERATED_THEME_CSS: &str =
    include_str!("../../../../src-web/styles/generated/theme-defaults.css");

fn css_block<'a>(source: &'a str, selector: &str) -> &'a str {
    let start = source
        .find(selector)
        .unwrap_or_else(|| panic!("missing CSS selector: {selector}"));
    let open = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .expect("CSS block opening brace");
    let close = source[open + 1..]
        .find('}')
        .map(|offset| open + 1 + offset)
        .expect("CSS block closing brace");
    &source[open + 1..close]
}

fn declarations(block: &str) -> BTreeMap<String, String> {
    let mut declarations = BTreeMap::new();
    for declaration in block
        .split(';')
        .map(str::trim)
        .filter(|declaration| !declaration.is_empty())
    {
        let (name, value) = declaration
            .split_once(':')
            .unwrap_or_else(|| panic!("malformed CSS declaration: {declaration}"));
        assert!(
            declarations
                .insert(name.trim().to_owned(), value.trim().to_owned())
                .is_none(),
            "duplicate CSS declaration: {name}"
        );
    }
    declarations
}

fn normalized(values: BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .into_iter()
        .map(|(name, value)| (name, value.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect()
}

#[test]
fn catalog_has_unique_ids_names_and_aliases() {
    for (index, token) in TOKENS.iter().enumerate() {
        assert!(TOKENS[index + 1..].iter().all(|other| {
            token.id != other.id
                && token.name != other.name
                && (token.alias.is_none() || other.alias.is_none() || token.alias != other.alias)
        }));
    }
    assert_eq!(TOKENS.len(), 19);
    assert_eq!(find("--on-accent").unwrap().id, "C19");
    assert_eq!(find("--accent").unwrap().id, "C03");
    assert!(find("--color-accent").is_none());
    assert!(find("--Accent").is_none());
}

#[test]
fn skin_write_permission_is_explicit() {
    assert!(find("--ink").unwrap().skin_writable);
    assert!(!find("--warning").unwrap().skin_writable);
    assert_eq!(find("--app-bg").unwrap().alias, None);
}

#[test]
fn rust_catalog_matches_generated_theme_values_and_tailwind_aliases() {
    let inline = css_block(APP_CSS, "@theme inline {");
    let aliases = declarations(inline);
    let expected_aliases = TOKENS
        .iter()
        .filter_map(|token| {
            token
                .alias
                .map(|alias| (alias.to_owned(), format!("var({})", token.name)))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        aliases, expected_aliases,
        "@theme inline must exactly match TOKENS"
    );

    let themes = theme_files();
    for (id, source) in themes {
        let (scheme, source_values) = theme_values(source).expect("registered theme is valid");
        let selector = format!(":root[data-theme=\"{id}\"]");
        let mut generated = declarations(css_block(GENERATED_THEME_CSS, &selector));
        assert_eq!(
            generated.remove("color-scheme").as_deref(),
            Some(scheme.as_str())
        );
        assert_eq!(
            normalized(generated),
            normalized(source_values),
            "generated values drifted for {id}"
        );
    }
}

#[test]
fn every_discovered_theme_is_complete_and_uses_the_shared_css_format() {
    let themes = theme_files();
    assert!(!themes.is_empty());
    for (index, (id, source)) in themes.iter().enumerate() {
        assert!(
            themes[index + 1..].iter().all(|(other, _)| id != other),
            "duplicate theme id: {id}"
        );
        assert!(
            matches!(theme_values(source), Some((scheme, values)) if matches!(scheme.as_str(), "light" | "dark") && values.len() == TOKENS.len()),
            "theme {id} must define one color scheme and every token"
        );
    }
}

#[test]
fn every_discovered_theme_uses_values_matching_the_registered_token_kind() {
    let source = theme_files()[0].1;
    for (name, invalid) in [
        ("--record-left", "red"),
        ("--record-left", "12 px"),
        ("--dur-micro", "12px"),
        ("--dur-micro", "0.2 s"),
        ("--app-bg", "rgb(1, 2, 3)"),
        ("--ease-signature", "#fff"),
        ("--ink", "12px"),
    ] {
        let start = source.find(&format!("{name}:")).unwrap() + name.len() + 1;
        let end = start + source[start..].find(';').unwrap();
        let malformed = format!("{}{invalid}{}", &source[..start], &source[end..]);
        assert!(theme_values(&malformed).is_none(), "{name}: {invalid}");
    }
}

#[test]
fn themes_exposes_the_discovered_ids_and_display_metadata() {
    let themes = themes().unwrap();
    assert_eq!(themes.len(), theme_files().len());
    assert!(themes.iter().any(|theme| {
        theme.id == "dark" && theme.name == "深色" && theme.color_scheme == "dark"
    }));
    assert!(themes.iter().any(|theme| {
        theme.id == "light-purple" && theme.name == "浅紫" && theme.color_scheme == "light"
    }));
}

#[test]
fn theme_discovery_rejects_invalid_ids_headers_and_incomplete_files() {
    let complete = theme_files()[0].1;
    for (id, source) in [
        ("Bad ID", complete),
        ("valid", ":root { color-scheme: light; }"),
        ("valid", "no display name or root block"),
        ("valid", "/* name */ :root { color-scheme: light; }"),
    ] {
        assert!(parse_themes(&[(id, source)]).is_none(), "{id}: {source}");
    }
}

#[test]
fn theme_ids_accept_only_bounded_lowercase_slugs() {
    for id in ["dark", "light-purple", "a1", &"a".repeat(64)] {
        assert!(valid_theme_id(id), "{id}");
    }
    for id in [
        "",
        "-dark",
        "dark-",
        "dark--purple",
        "Dark",
        "dark_1",
        "dark 1",
        &"a".repeat(65),
    ] {
        assert!(!valid_theme_id(id), "{id}");
    }
}

#[test]
fn theme_values_rejects_incomplete_or_ambiguous_css() {
    let one_token = ":root { color-scheme: light; --ink: #fff; }";
    for source in [
        "",
        "/* missing close",
        ":root { color-scheme: light; }",
        ":root { color-scheme: system; --ink: #fff; }",
        ":root { color-scheme: light; color-scheme: dark; --ink: #fff; }",
        ":root { color-scheme: light; --unknown: #fff; }",
        ":root { color-scheme: light; --ink: #fff; --ink: #000; }",
        "body { color-scheme: light; --ink: #fff; }",
    ] {
        assert!(theme_values(source).is_none(), "{source}");
    }
    assert!(theme_values(one_token).is_none());
}
