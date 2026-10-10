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

fn table(names: &[&str]) -> IdentifierTable {
    IdentifierTable::assign(names.iter().copied())
}

fn ids(table: &IdentifierTable, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| table.get(name).into_string())
        .collect()
}

/// Two MCP servers `a-b` and `a_b` each with a tool `search` are the tools `mcp__a-b__search` and
/// `mcp__a_b__search`; both are `mcp__a_b__search` to `to_codemode_identifier`.
#[test]
fn colliding_names_get_one_identifier_each() {
    let names = ["mcp__a-b__search", "mcp__a_b__search"];
    let table = table(&names);
    // The name that already is an identifier keeps it; the normalised one takes the suffix.
    assert_eq!(
        ids(&table, &names),
        ["mcp__a_b__search_2", "mcp__a_b__search"]
    );
    assert_eq!(
        table.renamed().collect::<Vec<_>>(),
        [(
            "mcp__a-b__search",
            &to_codemode_identifier("mcp__a_b__search_2")
        )]
    );
}

/// The registration order of tools changes between sessions (MCP servers connect late), and the
/// identifiers must not.
#[test]
fn the_assignment_does_not_depend_on_the_order_of_the_names() {
    let names = ["a b", "a-b", "a_b", "a_b_2", "a.b"];
    let forward = table(&names);
    let mut reversed = names;
    reversed.reverse();
    assert_eq!(forward, table(&reversed));
    // Every name has a distinct identifier.
    let mut all = ids(&forward, &names);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), names.len(), "{all:?}");
}

/// The names that do not collide get what upstream gives them, whatever else is around.
#[test]
fn names_that_do_not_collide_keep_the_derived_identifier() {
    let names = [
        "read",
        "bash",
        "mcp__dev-radius__search",
        "9lives",
        "",
        "my-tool",
        "mcp__a-b__search",
        "mcp__a_b__search",
    ];
    let table = table(&names);
    for name in &names[..6] {
        assert_eq!(
            table.get(name),
            to_codemode_identifier(name),
            "{name:?} must not change"
        );
    }
    assert_eq!(
        table.renamed().map(|(name, _)| name).collect::<Vec<_>>(),
        ["mcp__a-b__search"]
    );
}

/// A suffixed identifier never lands on one that a tool has or would get without a collision.
#[test]
fn a_suffix_never_takes_an_identifier_another_tool_has() {
    let table = table(&["a b", "a-b", "a-b-2"]);
    // "a b" sorts first and takes `a_b`; "a-b-2" is `a_b_2` without any help, so "a-b" goes on.
    assert_eq!(
        ids(&table, &["a b", "a-b", "a-b-2"]),
        ["a_b", "a_b_3", "a_b_2"]
    );
}

#[test]
fn a_name_the_table_was_not_made_over_gets_the_derived_identifier() {
    assert_eq!(table(&["read"]).get("my-tool").as_str(), "my_tool");
}
