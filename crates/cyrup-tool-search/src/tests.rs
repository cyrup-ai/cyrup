//! `tool_search`, case by case from `extensions/tool-search/tool.ts` @v1.0.1 and the ledger row's
//! `Verify` list (TOOL-052).
//!
//! The upstream file `test/tool-search.test.ts` exercises the ranker, the tokenizer and the codemode
//! description catalog, not the tool. Those nine cases are ported in `cyrup-codemode`
//! (`rank/tests.rs`: `tokenize_splits_camel_case_and_snake_case_drops_stop_words_and_folds_plurals`,
//! `ranks_by_term_relevance_and_respects_the_limit`,
//! `returns_nothing_for_unknown_or_empty_queries`, `includes_the_namespace_in_the_search_text`) and in
//! `cyrup-codemode-runtime` (`tool/description/tests.rs`, the five `createCodemodeDescription`
//! cases). What the tool itself does is pinned here, and in the session tests of `cyrup-session-svc`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolExposure, ToolResult};
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig, HostServices};
use serde_json::{Value, json};

use crate::input::{SearchLimit, ToolSearchInput, ToolSearchRefusal};
use crate::search::{LoadedTool, RegisteredTool, is_searchable, plan_search, result_text};
use crate::{TOOL_SEARCH_DESCRIPTION, ToolSearchExtension, ToolSearchTool};

/// A session's tool registry: the `getAllTools` rows and the active set, recording every change.
struct Registry {
    rows: Vec<Value>,
    active: Mutex<Vec<String>>,
    changes: Mutex<Vec<Vec<String>>>,
}

impl Registry {
    fn new(rows: Vec<Value>, active: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            rows,
            active: Mutex::new(active.iter().map(|name| (*name).to_owned()).collect()),
            changes: Mutex::new(Vec::new()),
        })
    }

    fn changes(&self) -> Vec<Vec<String>> {
        self.changes.lock().unwrap().clone()
    }

    fn active(&self) -> Vec<String> {
        self.active.lock().unwrap().clone()
    }
}

impl HostServices for Registry {
    fn all_tools(&self) -> Option<Vec<Value>> {
        Some(self.rows.clone())
    }

    fn active_tools(&self) -> Option<Vec<String>> {
        Some(self.active())
    }

    fn set_active_tools(&self, names: &[String]) {
        *self.active.lock().unwrap() = names.to_vec();
        self.changes.lock().unwrap().push(names.to_vec());
    }
}

fn row(name: &str, description: &str, exposure: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "parameters": { "type": "object", "properties": {} },
        "exposure": exposure,
    })
}

/// One tool of each of the five exposures, every one matching the query `widget`.
fn one_of_each() -> Vec<Value> {
    vec![
        row("direct_widget", "Direct widget tool.", "direct"),
        row("model_only_widget", "Model-only widget tool.", "model-only"),
        row("codemode_widget", "Codemode widget tool.", "codemode"),
        row("deferred_widget", "Deferred widget tool.", "deferred"),
        row("hidden_widget", "Hidden widget tool.", "hidden"),
    ]
}

async fn call(tool: &dyn Tool, params: Value) -> Result<ToolResult, String> {
    tool.execute(
        ToolCallId::from("call-1"),
        params,
        CancelToken::new(),
        Box::new(|_| {}),
    )
    .await
    .map_err(|error| error.to_string())
}

fn text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn loaded(result: &ToolResult) -> Vec<String> {
    result.details.as_ref().unwrap()["loaded"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_owned())
        .collect()
}

fn tool_over(registry: &Arc<Registry>) -> ToolSearchTool {
    ToolSearchTool::new(Some(Arc::clone(registry) as Arc<dyn HostServices>))
}

// ---------------------------------------------------------------------------------- definition --

/// `TOOL_SEARCH_DESCRIPTION` (`tool.ts:220`) is pinned byte for byte, built here from its parts so a
/// slip in the constant cannot also be a slip in the expectation. Upstream's own tests only compare
/// against the imported constant.
#[test]
fn the_description_is_upstreams_text_byte_for_byte() {
    let name = "tool_search";
    let expected = [
        "# Tool discovery".to_owned(),
        String::new(),
        "Searches over deferred tool metadata with BM25 and exposes matching tools for the next model call.".to_owned(),
        String::new(),
        format!(
            "Some of the tools, such as tools of MCP servers, may not have been provided to you upfront, and you should use this tool (`{name}`) to search for the required tools. For MCP tool discovery, always use `{name}`."
        ),
    ]
    .join("\n");
    assert_eq!(TOOL_SEARCH_DESCRIPTION, expected);
    assert_eq!(TOOL_SEARCH_DESCRIPTION.len(), 338);
    assert_eq!(ToolSearchTool::new(None).description(), expected);
}

#[test]
fn the_tool_is_registered_inactive_model_only_with_upstreams_snippet_and_schema() {
    let tool = ToolSearchTool::new(None);
    assert_eq!(tool.name(), "tool_search");
    assert_eq!(tool.label(), Some("tool_search"));
    assert!(!tool.default_active(), "registered `defaultActive: false`");
    assert_eq!(tool.exposure(), ToolExposure::ModelOnly);
    assert_eq!(
        tool.prompt_snippet(),
        Some("Search for tools that are not loaded yet and load the matches")
    );
    assert_eq!(
        *tool.parameters(),
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query for deferred tools." },
                "limit": { "type": "number", "description": "Maximum number of tools to return. Defaults to 8." }
            },
            "required": ["query"]
        })
    );
}

/// The extension registers the tool inactive: the host's active set does not contain it, and the
/// registry does.
#[tokio::test]
async fn the_extension_registers_tool_search_inactive() {
    let host = ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    });
    let registry = Registry::new(one_of_each(), &[]);
    host.load_native_with_services(
        Arc::new(ToolSearchExtension::new()),
        registry.clone() as Arc<dyn HostServices>,
    )
    .await
    .unwrap();
    let none = HashSet::new();
    assert!(host.active_tools(&[]).unwrap().is_empty());
    let registered = host.registered_tools_filtered(&[], None, &none).unwrap();
    let names: Vec<&str> = registered.iter().map(|tool| tool.name()).collect();
    assert_eq!(names, ["tool_search"]);
    // The registered tool is bound to the session the extension was loaded into.
    let result = call(registered[0].as_ref(), json!({ "query": "widget" }))
        .await
        .unwrap();
    assert_eq!(loaded(&result), ["codemode_widget", "deferred_widget"]);
}

// ---------------------------------------------------------------------------------- validation --

#[tokio::test]
async fn an_empty_or_blank_query_is_refused() {
    let registry = Registry::new(one_of_each(), &[]);
    let tool = tool_over(&registry);
    for query in ["", "   ", "\n\t \u{a0}"] {
        assert_eq!(
            call(&tool, json!({ "query": query })).await.unwrap_err(),
            "query must not be empty",
            "{query:?}"
        );
    }
    assert!(registry.changes().is_empty());
}

#[tokio::test]
async fn a_limit_that_is_not_a_positive_integer_is_refused() {
    let registry = Registry::new(one_of_each(), &[]);
    let tool = tool_over(&registry);
    for limit in [json!(0), json!(-1), json!(2.5), json!("3"), json!(true)] {
        assert_eq!(
            call(&tool, json!({ "query": "widget", "limit": limit.clone() }))
                .await
                .unwrap_err(),
            "limit must be a positive integer",
            "{limit}"
        );
    }
    assert!(registry.changes().is_empty());
}

/// Upstream checks the query before the limit.
#[tokio::test]
async fn the_query_is_validated_before_the_limit() {
    let tool = ToolSearchTool::new(None);
    assert_eq!(
        call(&tool, json!({ "query": " ", "limit": 0 }))
            .await
            .unwrap_err(),
        "query must not be empty"
    );
}

#[test]
fn input_parsing_names_each_refusal() {
    assert_eq!(
        ToolSearchInput::parse(&json!({})),
        Err(ToolSearchRefusal::QueryNotAString)
    );
    assert_eq!(
        ToolSearchInput::parse(&json!({ "query": 3 })),
        Err(ToolSearchRefusal::QueryNotAString)
    );
    assert_eq!(
        ToolSearchInput::parse(&json!({ "query": "\u{feff} " })),
        Err(ToolSearchRefusal::EmptyQuery)
    );
    assert_eq!(
        ToolSearchInput::parse(&json!({ "query": "a", "limit": 0 })),
        Err(ToolSearchRefusal::LimitNotAPositiveInteger)
    );
    let defaulted = ToolSearchInput::parse(&json!({ "query": " a ", "limit": null })).unwrap();
    assert_eq!(defaulted.query, " a ", "the query is kept as written");
    assert_eq!(defaulted.limit, SearchLimit::default_limit());
    assert_eq!(defaulted.limit.get(), 8);
}

// ------------------------------------------------------------------------------------- search --

/// TOOL-052 `Verify`: a registry holding one tool of each of the five exposures; a query matching
/// all five loads only the `codemode` and `deferred` ones, in rank order, and a second identical
/// query finds nothing because they are now active.
#[tokio::test]
async fn a_query_matching_every_exposure_loads_only_codemode_and_deferred_tools() {
    let registry = Registry::new(one_of_each(), &["direct_widget", "model_only_widget"]);
    let tool = tool_over(&registry);

    let first = call(&tool, json!({ "query": "widget" })).await.unwrap();
    assert_eq!(loaded(&first), ["codemode_widget", "deferred_widget"]);
    assert_eq!(
        text(&first),
        "Loaded 2 tools. They are available from your next call:\n- codemode_widget: Codemode widget tool.\n- deferred_widget: Deferred widget tool."
    );
    // Activation appends the matches to the active set, which `set_active_tools` replaces.
    assert_eq!(
        registry.changes(),
        [[
            "direct_widget",
            "model_only_widget",
            "codemode_widget",
            "deferred_widget"
        ]]
    );

    let second = call(&tool, json!({ "query": "widget" })).await.unwrap();
    assert_eq!(text(&second), "No matching tools found.");
    assert!(loaded(&second).is_empty());
    assert_eq!(
        registry.changes().len(),
        1,
        "no change when nothing matched"
    );
}

#[tokio::test]
async fn limit_caps_the_number_of_loaded_tools() {
    let rows: Vec<Value> = (0..12)
        .map(|n| row(&format!("deferred_widget_{n}"), "A widget.", "deferred"))
        .collect();
    let registry = Registry::new(rows, &[]);
    let tool = tool_over(&registry);

    let capped = call(&tool, json!({ "query": "widget", "limit": 3 }))
        .await
        .unwrap();
    assert_eq!(loaded(&capped).len(), 3);
    assert_eq!(registry.active().len(), 3);

    let default = call(&tool, json!({ "query": "widget" })).await.unwrap();
    assert_eq!(loaded(&default).len(), 8, "the default limit is 8");
    assert_eq!(registry.active().len(), 11);
}

#[tokio::test]
async fn a_single_match_is_reported_in_the_singular() {
    let registry = Registry::new(
        vec![row(
            "only_one",
            "Does the one thing.\r\nMore detail.",
            "deferred",
        )],
        &[],
    );
    let result = call(&tool_over(&registry), json!({ "query": "one" }))
        .await
        .unwrap();
    assert_eq!(
        text(&result),
        "Loaded 1 tool. They are available from your next call:\n- only_one: Does the one thing."
    );
}

#[test]
fn the_result_line_is_the_first_line_of_the_trimmed_description() {
    let tools = [
        LoadedTool {
            name: "a".to_owned(),
            description: "  \n\nFirst line\nsecond line".to_owned(),
        },
        LoadedTool {
            name: "b".to_owned(),
            description: String::new(),
        },
    ];
    assert_eq!(
        result_text(&tools),
        "Loaded 2 tools. They are available from your next call:\n- a: First line\n- b: "
    );
    assert_eq!(result_text(&[]), "No matching tools found.");
}

#[test]
fn only_codemode_and_deferred_tools_are_searchable() {
    for (exposure, searchable) in [
        (ToolExposure::Direct, false),
        (ToolExposure::ModelOnly, false),
        (ToolExposure::Codemode, true),
        (ToolExposure::Deferred, true),
        (ToolExposure::Hidden, false),
    ] {
        assert_eq!(is_searchable(exposure), searchable, "{exposure:?}");
    }
}

/// Candidates exclude the ACTIVE set: a `deferred` tool the model already has is not offered again,
/// and the activation keeps the order of what was already active.
#[test]
fn candidates_exclude_the_active_set_and_activation_keeps_it_first() {
    let tools: Vec<RegisteredTool> = [
        row("a_widget", "Widget a.", "deferred"),
        row("b_widget", "Widget b.", "deferred"),
        row("c_widget", "Widget c.", "codemode"),
    ]
    .iter()
    .map(|row| RegisteredTool::from_row(row).unwrap())
    .collect();
    let input = ToolSearchInput::parse(&json!({ "query": "widget" })).unwrap();
    let active = vec!["z_first".to_owned(), "b_widget".to_owned()];
    let plan = plan_search(&tools, &active, &input);
    let names: Vec<&str> = plan.loaded.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["a_widget", "c_widget"]);
    assert_eq!(
        plan.activate,
        Some(vec![
            "z_first".to_owned(),
            "b_widget".to_owned(),
            "a_widget".to_owned(),
            "c_widget".to_owned()
        ])
    );
    let nothing = plan_search(
        &tools,
        &active,
        &ToolSearchInput::parse(&json!({ "query": "kubernetes" })).unwrap(),
    );
    assert!(nothing.loaded.is_empty());
    assert_eq!(nothing.activate, None, "the active set is left alone");
}

/// The namespace's name, description and instructions are part of a tool's search text.
#[tokio::test]
async fn the_namespace_is_searchable_text() {
    let mut with_namespace = row("mcp__x__run", "Run it.", "deferred");
    with_namespace["namespace"] =
        json!({ "name": "mcp__x", "description": "Kubernetes cluster tools" });
    let registry = Registry::new(
        vec![with_namespace, row("other", "Run another.", "deferred")],
        &[],
    );
    let result = call(&tool_over(&registry), json!({ "query": "kubernetes" }))
        .await
        .unwrap();
    assert_eq!(loaded(&result), ["mcp__x__run"]);
}

#[tokio::test]
async fn without_a_session_nothing_is_found() {
    let result = call(&ToolSearchTool::new(None), json!({ "query": "widget" }))
        .await
        .unwrap();
    assert_eq!(text(&result), "No matching tools found.");
    assert!(loaded(&result).is_empty());
}

#[test]
fn a_row_the_search_cannot_read_is_not_a_candidate() {
    assert_eq!(
        RegisteredTool::from_row(&json!({ "description": "x", "exposure": "deferred" })),
        None
    );
    assert_eq!(
        RegisteredTool::from_row(&json!({ "name": "x", "exposure": "sometimes" })),
        None
    );
    assert_eq!(RegisteredTool::from_row(&json!({ "name": "x" })), None);
}
