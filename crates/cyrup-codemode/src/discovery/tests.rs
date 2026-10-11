#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::{Value, json};

use super::*;
use crate::rank::{Bm25Ranker, DEFAULT_TOOL_SEARCH_LIMIT};

struct Fixture {
    parameters: Value,
    namespaces: Vec<Option<ToolNamespace>>,
    specs: Vec<(&'static str, &'static str)>,
}

impl Fixture {
    fn new() -> Self {
        let github = ToolNamespace {
            name: "mcp__github".into(),
            description: Some("GitHub server".into()),
            instructions: Some("Use for repos.".into()),
        };
        let radius = ToolNamespace {
            name: "mcp__dev-radius".into(),
            description: Some(String::new()),
            instructions: Some("Radius usage guide.".into()),
        };
        Self {
            parameters: json!({"type": "object", "properties": {}}),
            namespaces: vec![
                None,
                Some(github.clone()),
                Some(github),
                Some(radius.clone()),
                Some(radius),
            ],
            specs: vec![
                ("read_notes", "Read notes."),
                ("mcp__github__list_issues", "List issues in a repository."),
                ("mcp__github__create_pull_request", "Open a pull request."),
                ("mcp__dev-radius__search", "Search the radius."),
                ("mcp__dev-radius__list_issues", "List radius issues."),
            ],
        }
    }

    fn tools(&self) -> Vec<DiscoverableTool<'_>> {
        self.specs
            .iter()
            .zip(&self.namespaces)
            .map(|((name, description), namespace)| DiscoverableTool {
                name,
                description,
                parameters: &self.parameters,
                namespace: namespace.as_ref(),
                identifier: None,
            })
            .collect()
    }
}

fn names(matches: &[ToolSearchMatch]) -> Vec<&str> {
    matches.iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn a_namespace_is_named_by_its_name_identifier_or_suffix_in_either_form() {
    let cases = [
        ("mcp__dev-radius", "mcp__dev-radius", true),
        ("mcp__dev-radius", "mcp__dev_radius", true),
        ("mcp__dev-radius", "dev-radius", true),
        ("mcp__dev-radius", "dev_radius", true),
        ("mcp__dev-radius", "radius", false),
        ("mcp__dev-radius", "mcp", false),
        ("mcp__dev-radius", "", false),
        ("plain", "plain", true),
        ("plain", "pl-ain", false),
        // The suffix is after the LAST `__`.
        ("a__b__c", "c", true),
        ("a__b__c", "b__c", false),
        ("a___b", "b", true),
        // Two different names with the same identifier are the same namespace.
        ("a-b", "a_b", true),
        ("a-b", "a b", true),
        // `a__b_-c` is the identifier `a__b__c`, whose last `__` is later than the name's own: the
        // suffix of the name itself reaches `b_-c`, the suffix of the identifier reaches `c`.
        ("a__b_-c", "b_-c", true),
        ("a__b_-c", "c", true),
    ];
    for (namespace, query, expected) in cases {
        assert_eq!(
            is_namespace_name(namespace, query),
            expected,
            "{namespace} / {query}"
        );
    }
}

#[test]
fn search_ranks_all_tools_by_default() {
    let fixture = Fixture::new();
    let matches = search_tools(
        &Bm25Ranker::default(),
        &fixture.tools(),
        "issues",
        DEFAULT_TOOL_SEARCH_LIMIT,
        None,
    );
    let mut found = names(&matches);
    found.sort_unstable();
    assert_eq!(
        found,
        ["mcp__dev-radius__list_issues", "mcp__github__list_issues"]
    );
}

#[test]
fn search_limit_applies() {
    let fixture = Fixture::new();
    let matches = search_tools(&Bm25Ranker::default(), &fixture.tools(), "issues", 1, None);
    assert_eq!(matches.len(), 1);
}

#[test]
fn search_namespace_filter_applies_through_the_three_way_match() {
    let fixture = Fixture::new();
    let ranker = Bm25Ranker::default();
    for namespace in [
        "mcp__dev-radius",
        "mcp__dev_radius",
        "dev-radius",
        "dev_radius",
    ] {
        let matches = search_tools(&ranker, &fixture.tools(), "issues", 8, Some(namespace));
        assert_eq!(
            names(&matches),
            ["mcp__dev-radius__list_issues"],
            "{namespace}"
        );
    }
    // A tool without a namespace never matches a restriction; an unknown namespace finds nothing.
    assert!(search_tools(&ranker, &fixture.tools(), "notes", 8, Some("mcp__github")).is_empty());
    assert!(search_tools(&ranker, &fixture.tools(), "issues", 8, Some("nope")).is_empty());
    // An empty restriction is no restriction, as a falsy `namespace` is upstream.
    assert_eq!(
        search_tools(&ranker, &fixture.tools(), "notes", 8, Some("")).len(),
        1
    );
}

#[test]
fn the_namespace_text_is_searchable() {
    let fixture = Fixture::new();
    // "usage guide" appears only in the radius namespace's instructions.
    let matches = search_tools(
        &Bm25Ranker::default(),
        &fixture.tools(),
        "usage guide",
        8,
        None,
    );
    assert_eq!(
        names(&matches),
        ["mcp__dev-radius__search", "mcp__dev-radius__list_issues"]
    );
}

#[test]
fn describe_tool_resolves_the_raw_name_and_the_identifier() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    for name in ["mcp__dev-radius__search", "mcp__dev_radius__search"] {
        assert_eq!(
            find_tool(&tools, name).unwrap().name,
            "mcp__dev-radius__search",
            "{name}"
        );
    }
    assert_eq!(find_tool(&tools, "read_notes").unwrap().name, "read_notes");
    assert!(find_tool(&tools, "mcp__dev-radius").is_none());
    assert!(find_tool(&tools, "").is_none());
}

#[test]
fn describe_tool_takes_the_first_of_two_tools_with_one_identifier() {
    let parameters = json!({});
    let tools = [
        DiscoverableTool {
            name: "a-b",
            description: "dash",
            parameters: &parameters,
            namespace: None,
            identifier: None,
        },
        DiscoverableTool {
            name: "a_b",
            description: "underscore",
            parameters: &parameters,
            namespace: None,
            identifier: None,
        },
    ];
    assert_eq!(find_tool(&tools, "a_b").unwrap().description, "dash");
    assert_eq!(find_tool(&tools, "a-b").unwrap().description, "dash");
}

#[test]
fn describe_namespace_resolves_by_any_name_and_returns_its_instructions() {
    let fixture = Fixture::new();
    let description = describe_namespace(&fixture.tools(), "dev-radius").unwrap();
    assert_eq!(description.name, "mcp__dev-radius");
    // An empty description is left out, like an absent one.
    assert_eq!(description.description, None);
    assert_eq!(
        description.instructions.as_deref(),
        Some("Radius usage guide.")
    );
    let tools: Vec<&str> = description
        .tools
        .iter()
        .map(CodemodeIdentifier::as_str)
        .collect();
    assert_eq!(
        tools,
        ["mcp__dev_radius__search", "mcp__dev_radius__list_issues"]
    );
    assert_eq!(
        serde_json::to_value(&description).unwrap(),
        json!({
            "name": "mcp__dev-radius",
            "instructions": "Radius usage guide.",
            "tools": ["mcp__dev_radius__search", "mcp__dev_radius__list_issues"],
        })
    );
    let github = describe_namespace(&fixture.tools(), "mcp__github").unwrap();
    assert_eq!(github.description.as_deref(), Some("GitHub server"));
    assert_eq!(github.tools.len(), 2);
    assert_eq!(describe_namespace(&fixture.tools(), "missing"), None);
}

#[test]
fn describe_namespace_is_described_by_the_first_tool_that_names_it() {
    let parameters = json!({});
    let first = ToolNamespace {
        name: "mcp__x".into(),
        description: Some("first".into()),
        instructions: None,
    };
    let second = ToolNamespace {
        description: Some("second".into()),
        ..first.clone()
    };
    let tools = [
        DiscoverableTool {
            name: "mcp__x__a",
            description: "",
            parameters: &parameters,
            namespace: Some(&first),
            identifier: None,
        },
        DiscoverableTool {
            name: "mcp__x__b",
            description: "",
            parameters: &parameters,
            namespace: Some(&second),
            identifier: None,
        },
    ];
    let description = describe_namespace(&tools, "x").unwrap();
    assert_eq!(description.description.as_deref(), Some("first"));
    assert_eq!(description.tools.len(), 2);
}

/// With a table identifier, `describeTool` finds a tool by its own identifier, and by its raw name,
/// and `describeNamespace` lists the identifiers scripts call the tools by.
#[test]
fn discovery_follows_the_identifier_table() {
    let parameters = json!({});
    let namespace = ToolNamespace {
        name: "x".into(),
        description: None,
        instructions: None,
    };
    let dash = to_codemode_identifier("a_b_2");
    let tools = [
        DiscoverableTool {
            name: "a-b",
            description: "dash",
            parameters: &parameters,
            namespace: Some(&namespace),
            identifier: Some(&dash),
        },
        DiscoverableTool {
            name: "a_b",
            description: "underscore",
            parameters: &parameters,
            namespace: Some(&namespace),
            identifier: None,
        },
    ];
    assert_eq!(find_tool(&tools, "a_b").unwrap().description, "underscore");
    assert_eq!(find_tool(&tools, "a_b_2").unwrap().description, "dash");
    assert_eq!(find_tool(&tools, "a-b").unwrap().description, "dash");
    let listed = describe_namespace(&tools, "x").unwrap().tools;
    assert_eq!(
        listed
            .iter()
            .map(CodemodeIdentifier::as_str)
            .collect::<Vec<_>>(),
        ["a_b_2", "a_b"]
    );
}
