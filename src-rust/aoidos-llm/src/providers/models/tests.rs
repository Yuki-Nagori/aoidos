use super::*;

#[test]
fn constructor_preserves_identity_protocol_and_numeric_boundaries() {
    for protocol in [
        Protocol::ChatCompletions,
        Protocol::Responses,
        Protocol::Messages,
        Protocol::Gemini,
    ] {
        for (context_window, max_output_tokens) in [
            (0, 0),
            (1, 1),
            (u32::MAX, u32::MAX),
            (0, u32::MAX),
            (u32::MAX, 0),
        ] {
            let spec = ModelSpec::new("", protocol, context_window, max_output_tokens);
            assert_eq!(spec.id, "");
            assert_eq!(spec.protocol, protocol);
            assert_eq!(spec.context_window, context_window);
            assert_eq!(spec.max_output_tokens, max_output_tokens);
        }
    }
    let spec = ModelSpec::new("fixture/model", Protocol::Messages, 100, 20);
    assert_eq!(spec.id, "fixture/model");
}

#[test]
fn catalogs_have_unique_models_and_positive_bounded_limits() {
    for catalog in [DEEPSEEK_MODELS, GO_MODELS, ZEN_MODELS] {
        let mut ids = std::collections::HashSet::new();
        for spec in catalog {
            assert!(!spec.id.is_empty());
            assert!(ids.insert(spec.id), "duplicate {}", spec.id);
            assert!(spec.context_window > 0);
            assert!(spec.max_output_tokens > 0);
            assert!(spec.max_output_tokens <= spec.context_window);
        }
    }
}
