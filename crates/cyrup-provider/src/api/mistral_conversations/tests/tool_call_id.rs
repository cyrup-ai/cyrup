//! The 9-char tool-call-id normalizer.

use super::*;

#[test]
fn tool_call_id_normalizer_is_9_chars_and_stable() {
    let n = MistralToolCallIdNormalizer::default();
    let a = n.normalize("call_abc/def!");
    assert_eq!(a.chars().count(), 9);
    assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    // stable for the same source id.
    assert_eq!(n.normalize("call_abc/def!"), a);
    // an already-9-char alphanumeric id passes through unchanged.
    assert_eq!(n.normalize("abcdefghi"), "abcdefghi");
}
