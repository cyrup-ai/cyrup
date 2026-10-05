#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value;

use super::*;

#[test]
fn replaces_characters_that_are_not_valid_in_an_identifier() {
    let cases = [
        ("mcp__docs__search", "mcp__docs__search"),
        ("my-tool", "my_tool"),
        ("9lives", "_lives"),
        ("", "_"),
        ("$x", "$x"),
        ("a b", "a_b"),
        ("é", "_"),
        // One Unicode scalar value is one `_`, not its two UTF-16 units.
        ("a\u{1f600}b", "a_b"),
        ("x-9", "x_9"),
    ];
    for (name, expected) in cases {
        assert_eq!(to_codemode_identifier(name).as_str(), expected, "{name:?}");
    }
}

#[test]
fn is_identifier_matches_the_upstream_pattern() {
    assert!(is_identifier("city"));
    assert!(is_identifier("$_9"));
    assert!(!is_identifier(""));
    assert!(!is_identifier("9a"));
    assert!(!is_identifier("max-lines"));
    assert!(!is_identifier("é"));
    assert!(!is_identifier("a\n"));
}

#[test]
fn agrees_with_upstream_on_the_corpus() {
    let corpus: Value =
        serde_json::from_str(include_str!("../../testdata/identifiers.json")).unwrap();
    let cases = corpus.as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let expected = case["id"].as_str().unwrap();
        assert_eq!(to_codemode_identifier(name).as_str(), expected, "{name:?}");
    }
}
