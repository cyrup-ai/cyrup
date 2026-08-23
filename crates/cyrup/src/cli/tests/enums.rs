use super::*;

#[test]
fn split_model_level_peels_a_trailing_thinking_suffix() {
    assert_eq!(
        split_model_level("sonnet:high"),
        ("sonnet".to_string(), Some(ThinkingArg::High))
    );
    assert_eq!(
        split_model_level("anthropic/claude"),
        ("anthropic/claude".to_string(), None)
    );
    // A non-level suffix is preserved.
    assert_eq!(split_model_level("a:b"), ("a:b".to_string(), None));
}
