//! `searchTools()`, `describeTool()` and `describeNamespace()` as a script calls them: argument
//! validation, the thrown texts, and what reaches the script (pi `createDiscoveryGlobals`,
//! `extensions/codemode/execute.ts:455-517` @v1.0.1). The ranking and namespace matching they sit on
//! are pinned in `cyrup-codemode`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use cyrup_codemode::declarations::render_tool_sample;
use cyrup_core::{CancelToken, Tool};
use serde_json::{Value, json};

use super::discovery_globals;
use crate::testkit::StubTool;
use crate::tool::description::to_codemode_declaration;
use crate::types::{CodemodeToolContext, ToolResult};

struct Globals {
    globals: Vec<crate::types::CodemodeTool>,
}

impl Globals {
    fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        let samples: BTreeMap<String, String> = tools
            .iter()
            .map(|tool| {
                (
                    tool.name().to_owned(),
                    render_tool_sample(&to_codemode_declaration(tool.as_ref(), &[]), None),
                )
            })
            .collect();
        Self {
            globals: discovery_globals(Arc::new(tools), Arc::new(samples)),
        }
    }

    async fn call(&self, name: &str, args: Vec<Value>) -> ToolResult {
        let global = self
            .globals
            .iter()
            .find(|global| global.declaration.name == name)
            .unwrap();
        assert!(
            global.declaration.spread,
            "{name} takes its whole argument list"
        );
        (global.execute)(
            Some(Value::Array(args)),
            CodemodeToolContext {
                cancel: CancelToken::new(),
            },
        )
        .await
    }
}

fn catalog() -> Vec<Arc<dyn Tool>> {
    vec![
        StubTool::new("read_notes", "Read notes from disk.").arc(),
        StubTool::new(
            "mcp__dev-radius__search",
            "Search the radius index for services.",
        )
        .namespace("mcp__dev-radius", Some("Radius server"))
        .arc(),
        StubTool::new("mcp__dev-radius__open", "Open a radius record.")
            .namespace("mcp__dev-radius", Some("Radius server"))
            .arc(),
        StubTool::new("mcp__docs__search", "Search the documentation pages.")
            .namespace("mcp__docs", None)
            .arc(),
    ]
}

fn names(result: &ToolResult) -> Vec<String> {
    result
        .as_ref()
        .unwrap()
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn search_ranks_by_relevance_and_returns_identifiers_with_the_declaration_sample() {
    let globals = Globals::new(catalog());
    // BM25 ranks the shorter document first for a term both contain; the order itself is pinned in
    // `cyrup-codemode`. What this module owns is the shape of each entry.
    let both = globals.call("searchTools", vec![json!("radius")]).await;
    let mut ranked = names(&both);
    ranked.sort();
    assert_eq!(
        ranked,
        ["mcp__dev_radius__open", "mcp__dev_radius__search"],
        "identifiers, not raw names"
    );

    let found = globals.call("searchTools", vec![json!("services")]).await;
    assert_eq!(names(&found), ["mcp__dev_radius__search"]);
    let entry = &found.unwrap().unwrap()[0];
    let description = entry["description"].as_str().unwrap();
    assert!(description.starts_with("Search the radius index for services."));
    assert!(description.contains("codemode tool declaration:"));
    assert_eq!(entry.as_object().unwrap().len(), 2);

    let nothing = globals.call("searchTools", vec![json!("kubernetes")]).await;
    assert!(names(&nothing).is_empty());
}

#[tokio::test]
async fn search_respects_limit_and_the_namespace_filter_in_every_spelling() {
    let globals = Globals::new(catalog());
    let one = globals
        .call("searchTools", vec![json!("search"), json!({ "limit": 1 })])
        .await;
    assert_eq!(names(&one).len(), 1);

    // The namespace's name, its identifier, and the part after the last `__`, in either form.
    for spelling in [
        "mcp__dev-radius",
        "mcp__dev_radius",
        "dev-radius",
        "dev_radius",
    ] {
        let found = globals
            .call(
                "searchTools",
                vec![json!("search"), json!({ "namespace": spelling })],
            )
            .await;
        assert_eq!(
            names(&found),
            ["mcp__dev_radius__search"],
            "namespace {spelling}"
        );
    }
    // A tool without a namespace never matches a restriction.
    let none = globals
        .call(
            "searchTools",
            vec![json!("notes"), json!({ "namespace": "mcp__docs" })],
        )
        .await;
    assert!(names(&none).is_empty());
    // An empty namespace is no restriction.
    let all = globals
        .call(
            "searchTools",
            vec![json!("notes"), json!({ "namespace": "" })],
        )
        .await;
    assert_eq!(names(&all), ["read_notes"]);
}

#[tokio::test]
async fn search_defaults_to_eight_results() {
    let tools: Vec<Arc<dyn Tool>> = (0..12)
        .map(|n| StubTool::new(&format!("tool_{n}"), "common keyword").arc())
        .collect();
    let globals = Globals::new(tools);
    let found = globals.call("searchTools", vec![json!("keyword")]).await;
    assert_eq!(names(&found).len(), 8);
    // `searchOptions?.limit ?? DEFAULT`: null and absent members are the default.
    for options in [
        json!(null),
        json!({}),
        json!({ "limit": null }),
        json!("text"),
        json!(5),
    ] {
        let found = globals
            .call("searchTools", vec![json!("keyword"), options.clone()])
            .await;
        assert_eq!(names(&found).len(), 8, "{options}");
    }
}

#[tokio::test]
async fn search_rejects_bad_arguments_with_upstreams_texts() {
    let globals = Globals::new(catalog());
    for query in [json!(null), json!(7), json!({ "q": "x" })] {
        assert_eq!(
            globals.call("searchTools", vec![query]).await.unwrap_err(),
            "searchTools() expects a query string"
        );
    }
    assert_eq!(
        globals.call("searchTools", Vec::new()).await.unwrap_err(),
        "searchTools() expects a query string"
    );
    for limit in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("3"),
        json!(true),
        json!([2]),
    ] {
        assert_eq!(
            globals
                .call("searchTools", vec![json!("x"), json!({ "limit": limit })])
                .await
                .unwrap_err(),
            "searchTools() limit must be a positive integer",
            "limit {limit}"
        );
    }
    // `5.0` is an integer in JavaScript.
    globals
        .call("searchTools", vec![json!("x"), json!({ "limit": 5.0 })])
        .await
        .unwrap();
    for namespace in [json!(1), json!(true), json!(["a"]), json!({})] {
        assert_eq!(
            globals
                .call(
                    "searchTools",
                    vec![json!("x"), json!({ "namespace": namespace })]
                )
                .await
                .unwrap_err(),
            "searchTools() namespace must be a string",
            "namespace {namespace}"
        );
    }
}

#[tokio::test]
async fn describe_tool_finds_a_tool_by_raw_name_or_identifier() {
    let globals = Globals::new(catalog());
    let raw = globals
        .call("describeTool", vec![json!("mcp__dev-radius__search")])
        .await
        .unwrap()
        .unwrap();
    let by_identifier = globals
        .call("describeTool", vec![json!("mcp__dev_radius__search")])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(raw, by_identifier);
    assert!(
        raw.as_str()
            .unwrap()
            .contains("tools: { mcp__dev_radius__search(args:")
    );

    assert_eq!(
        globals.call("describeTool", vec![json!("nope")]).await,
        Ok(None),
        "an unknown tool is undefined"
    );
    for bad in [json!(null), json!(3)] {
        assert_eq!(
            globals.call("describeTool", vec![bad]).await.unwrap_err(),
            "describeTool() expects a tool name"
        );
    }
}

#[tokio::test]
async fn describe_namespace_returns_the_group_and_its_tool_identifiers() {
    let globals = Globals::new(catalog());
    for spelling in [
        "mcp__dev-radius",
        "mcp__dev_radius",
        "dev-radius",
        "dev_radius",
    ] {
        let described = globals
            .call("describeNamespace", vec![json!(spelling)])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            described,
            json!({
                "name": "mcp__dev-radius",
                "description": "Radius server",
                "tools": ["mcp__dev_radius__search", "mcp__dev_radius__open"],
            }),
            "{spelling}"
        );
    }
    // `description` and `instructions` are omitted when empty.
    let docs = globals
        .call("describeNamespace", vec![json!("docs")])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        docs,
        json!({ "name": "mcp__docs", "tools": ["mcp__docs__search"] })
    );

    assert_eq!(
        globals
            .call("describeNamespace", vec![json!("ghost")])
            .await,
        Ok(None)
    );
    assert_eq!(
        globals
            .call("describeNamespace", vec![json!(1)])
            .await
            .unwrap_err(),
        "describeNamespace() expects a namespace name"
    );
}

#[tokio::test]
async fn namespace_instructions_are_returned_when_present() {
    let tool = StubTool::new("mcp__x__run", "Run.")
        .namespace("mcp__x", None)
        .namespace_instructions("Long usage guide.")
        .arc();
    let globals = Globals::new(vec![tool]);
    let described = globals
        .call("describeNamespace", vec![json!("mcp__x")])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        described,
        json!({ "name": "mcp__x", "instructions": "Long usage guide.", "tools": ["mcp__x__run"] })
    );
}
