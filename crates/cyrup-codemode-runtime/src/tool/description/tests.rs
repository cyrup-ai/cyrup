//! The model-facing description and the per-tool script-call note (pi
//! `createCodemodeDescription` / `describeScriptCall`, `extensions/codemode/tool.ts` @v1.0.1).
//!
//! The first five cases are upstream's `describe("codemode description catalog")`
//! (`test/tool-search.test.ts:67-122` @v1.0.1), case for case.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cyrup_core::{Tool, ToolNamespace};
use serde_json::json;

use super::{DescriptionOptions, create_codemode_description, describe_script_call};
use crate::testkit::StubTool;
/// An absolute docs path other than the production one, so a test fails when the path is not the one
/// handed in.
const CODEMODE_DOCS_PATH: &str = "/opt/cyrup/docs/codemode.md";

fn tool(name: &str, description: &str) -> Arc<dyn Tool> {
    StubTool::new(name, description)
        .parameters(json!({ "type": "object", "properties": {} }))
        .arc()
}

struct Catalog {
    plain: Arc<dyn Tool>,
    github: Vec<Arc<dyn Tool>>,
    docs: Vec<Arc<dyn Tool>>,
    namespaces: BTreeMap<String, ToolNamespace>,
}

impl Catalog {
    fn new() -> Self {
        let plain = tool("read_notes", "Read notes.");
        let github: Vec<Arc<dyn Tool>> = ["a", "b", "c"]
            .iter()
            .map(|suffix| {
                tool(
                    &format!("mcp__github__{suffix}"),
                    &format!("GitHub {suffix}."),
                )
            })
            .collect();
        let docs = vec![
            tool("mcp__docs__search", "Search docs."),
            tool("mcp__docs__long", &"Long ".repeat(200)),
        ];
        let mut namespaces = BTreeMap::new();
        for entry in &github {
            namespaces.insert(
                entry.name().to_owned(),
                ToolNamespace {
                    name: "mcp__github".into(),
                    description: Some("GitHub server".into()),
                    instructions: None,
                },
            );
        }
        for entry in &docs {
            namespaces.insert(
                entry.name().to_owned(),
                ToolNamespace {
                    name: "mcp__docs".into(),
                    description: None,
                    instructions: None,
                },
            );
        }
        Self {
            plain,
            github,
            docs,
            namespaces,
        }
    }

    fn all(&self) -> Vec<Arc<dyn Tool>> {
        let mut all = vec![Arc::clone(&self.plain)];
        all.extend(self.github.iter().cloned());
        all.extend(self.docs.iter().cloned());
        all
    }

    fn describe(
        &self,
        tools: &[Arc<dyn Tool>],
        budget: Option<f64>,
        deferred: &BTreeSet<String>,
    ) -> String {
        create_codemode_description(
            tools,
            &DescriptionOptions {
                models: false,
                namespaces: &self.namespaces,
                deferred,
                guidelines: &BTreeMap::new(),
                inline_budget: budget,
                docs_path: CODEMODE_DOCS_PATH,
            },
        )
    }
}

#[test]
fn lists_everything_without_a_budget() {
    let catalog = Catalog::new();
    let description = catalog.describe(&catalog.all(), None, &BTreeSet::new());
    assert!(description.contains("Nested tools:"));
    assert!(description.contains("## mcp__github\nGitHub server"));
    assert!(description.contains("## mcp__docs\n\n### `mcp__docs"));
    // The search guidance is always there, so tools that appear later do not change it.
    assert!(description.contains("find unlisted tools, such as MCP tools"));
}

#[test]
fn fills_the_budget_round_robin_cheapest_first_and_says_what_is_missing() {
    let catalog = Catalog::new();
    // Each small section costs about 42 tokens: one tool per group, then one more.
    let description = catalog.describe(&catalog.all(), Some(170.0), &BTreeSet::new());
    assert!(description.contains("### `read_notes`"));
    assert!(description.contains("## mcp__docs (some tools not listed)"));
    assert!(description.contains("### `mcp__docs__search`"));
    assert!(!description.contains("### `mcp__docs__long`"));
    assert!(description.contains("## mcp__github (some tools not listed)"));
    assert!(description.contains("find unlisted tools, such as MCP tools"));
    // Deterministic: the same input gives the same description.
    assert_eq!(
        catalog.describe(&catalog.all(), Some(170.0), &BTreeSet::new()),
        description
    );
}

#[test]
fn leaves_deferred_tools_and_their_namespaces_out_entirely() {
    let catalog = Catalog::new();
    let deferred: BTreeSet<String> = catalog
        .github
        .iter()
        .map(|entry| entry.name().to_owned())
        .collect();
    let description = catalog.describe(&catalog.all(), None, &deferred);
    assert!(!description.contains("mcp__github"));
    // Deferred tools, such as those of a server that connects later, do not change the description.
    let mut without = vec![Arc::clone(&catalog.plain)];
    without.extend(catalog.docs.iter().cloned());
    assert_eq!(
        description,
        catalog.describe(&without, None, &BTreeSet::new())
    );
}

#[test]
fn leaves_namespace_instructions_out() {
    let catalog = Catalog::new();
    let namespaces: BTreeMap<String, ToolNamespace> = catalog
        .github
        .iter()
        .map(|entry| {
            (
                entry.name().to_owned(),
                ToolNamespace {
                    name: "mcp__github".into(),
                    description: None,
                    instructions: Some("Long usage guide.".into()),
                },
            )
        })
        .collect();
    let description = create_codemode_description(
        &catalog.github,
        &DescriptionOptions {
            models: false,
            namespaces: &namespaces,
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: None,
            docs_path: CODEMODE_DOCS_PATH,
        },
    );
    assert!(description.contains("## mcp__github\n\n### `mcp__github__a`"));
    assert!(!description.contains("Long usage guide."));
}

#[test]
fn lists_only_namespaces_with_a_zero_budget() {
    let catalog = Catalog::new();
    let description = catalog.describe(&catalog.all(), Some(0.0), &BTreeSet::new());
    assert!(description.contains("## mcp__docs (tools not listed)"));
    assert!(!description.contains("codemode tool declaration:"));
}

/// CODE-007 verify: three namespaces whose cheapest tools fit but whose totals do not — every
/// namespace keeps at least one listed section, which a global "cheapest first" would not.
#[test]
fn every_namespace_is_represented_before_any_namespace_is_complete() {
    let mut namespaces = BTreeMap::new();
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for ns in ["alpha", "beta", "gamma"] {
        for n in 0..6 {
            let name = format!("{ns}__t{n}");
            // The first tool of each namespace is the longest, so a global cheapest-first fill
            // would take every namespace's short tools before any namespace's long one.
            let description = if n == 0 {
                "x ".repeat(60)
            } else {
                "short".to_owned()
            };
            namespaces.insert(
                name.clone(),
                ToolNamespace {
                    name: ns.into(),
                    description: None,
                    instructions: None,
                },
            );
            tools.push(tool(&name, &description));
        }
    }
    let description = create_codemode_description(
        &tools,
        &DescriptionOptions {
            models: false,
            namespaces: &namespaces,
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: Some(200.0),
            docs_path: CODEMODE_DOCS_PATH,
        },
    );
    for ns in ["alpha", "beta", "gamma"] {
        assert!(
            description.contains(&format!("### `{ns}__t")),
            "{ns} lost every section:\n{description}"
        );
        assert!(
            description.contains(&format!("## {ns} (some tools not listed)")),
            "{ns} is not marked partial:\n{description}"
        );
    }
}

/// Namespace headings are ordered by `localeCompare` (ICU), not by code unit: `a` sorts before
/// `B`, and the namespace-less group comes first regardless of name.
#[test]
fn namespaces_are_ordered_like_locale_compare_after_the_unnamespaced_group() {
    let mut namespaces = BTreeMap::new();
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for (name, ns) in [("z_tool", "Beta"), ("y_tool", "alpha"), ("x_tool", "Gamma")] {
        namespaces.insert(
            name.to_owned(),
            ToolNamespace {
                name: ns.into(),
                description: None,
                instructions: None,
            },
        );
        tools.push(tool(name, "d"));
    }
    tools.push(tool("plain", "d"));
    let description = create_codemode_description(
        &tools,
        &DescriptionOptions {
            models: false,
            namespaces: &namespaces,
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: None,
            docs_path: CODEMODE_DOCS_PATH,
        },
    );
    let position = |needle: &str| description.find(needle).unwrap();
    assert!(position("### `plain`") < position("## alpha"));
    assert!(position("## alpha") < position("## Beta"));
    assert!(position("## Beta") < position("## Gamma"));
}

#[test]
fn the_description_has_the_intro_the_globals_and_the_models_line_only_when_declared() {
    let none = BTreeMap::new();
    let describe = |models: bool| {
        create_codemode_description(
            &[],
            &DescriptionOptions {
                models,
                namespaces: &none,
                deferred: &BTreeSet::new(),
                guidelines: &BTreeMap::new(),
                inline_budget: None,
                docs_path: CODEMODE_DOCS_PATH,
            },
        )
    };
    let plain = describe(false);
    assert!(
        plain.starts_with("Run JavaScript that calls other tools. The input is raw JavaScript")
    );
    assert!(plain.contains("// @options: {\"max_output_tokens\": 10000, \"timeout_ms\": 60000}"));
    assert!(plain.contains("\n\nGlobals:\n- `text(value)`, `image(dataUrlOrImageBlock)`"));
    assert!(
        !plain.contains("Nested tools:"),
        "no tools, no tool section"
    );
    assert!(!plain.contains("`models`"));

    let with_models = describe(true);
    assert!(with_models.contains(&format!(
        "- `models`: classifiers and image generation. Read {CODEMODE_DOCS_PATH} first."
    )));
}

/// `getCodemodeCallableTools`: the codemode tool is never listed, even when handed in.
#[test]
fn the_codemode_tool_is_not_listed_among_its_own_nested_tools() {
    let catalog = Catalog::new();
    let mut tools = catalog.all();
    tools.push(tool("codemode", "Run JavaScript."));
    let description = catalog.describe(&tools, None, &BTreeSet::new());
    assert!(!description.contains("### `codemode`"));
    assert_eq!(
        description,
        catalog.describe(&catalog.all(), None, &BTreeSet::new())
    );
}

/// `renderToolSection`: a tool whose name is not an identifier is shown as `id` (`raw name`).
#[test]
fn a_tool_with_a_non_identifier_name_shows_both_names() {
    let tools = vec![tool("my-tool", "Does things.")];
    let none = BTreeMap::new();
    let description = create_codemode_description(
        &tools,
        &DescriptionOptions {
            models: false,
            namespaces: &none,
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: None,
            docs_path: CODEMODE_DOCS_PATH,
        },
    );
    assert!(
        description
            .contains("### `my_tool` (`my-tool`)\nDoes things.\n\ncodemode tool declaration:")
    );
}

/// The MCP preamble appears only when a LISTED tool's output is an MCP `CallToolResult`.
#[test]
fn the_mcp_preamble_follows_the_listed_tools() {
    let mcp = StubTool::new("mcp__x__run", "Run.")
        .parameters(json!({ "type": "object", "properties": {} }))
        .output_schema(json!({
            "type": "object",
            "properties": {
                "content": { "type": "array", "items": { "type": "object" } },
                "isError": { "type": "boolean" },
                "_meta": { "type": "object" }
            },
            "required": ["content"]
        }))
        .arc();
    let none = BTreeMap::new();
    let describe = |deferred: &BTreeSet<String>| {
        create_codemode_description(
            &[Arc::clone(&mcp)],
            &DescriptionOptions {
                models: false,
                namespaces: &none,
                deferred,
                guidelines: &BTreeMap::new(),
                inline_budget: None,
                docs_path: CODEMODE_DOCS_PATH,
            },
        )
    };
    assert!(describe(&BTreeSet::new()).contains("Shared MCP Types:\n```ts\n"));
    let deferred: BTreeSet<String> = ["mcp__x__run".to_owned()].into();
    assert!(!describe(&deferred).contains("Shared MCP Types"));
}

/// `describeScriptCall` (`tool.ts:318-322`): the description, then how scripts call the tool.
#[test]
fn a_declared_tool_says_how_scripts_call_it() {
    let echo = StubTool::new("echo", "Echo text back.\n\nSecond paragraph.").arc();
    assert_eq!(
        describe_script_call(echo.as_ref()),
        "Echo text back.\n\nSecond paragraph.\n\nCodemode: `tools.echo(args)` resolves to a string."
    );

    let stats = StubTool::new("stats", "Return structured stats")
        .parameters(json!({ "type": "object", "properties": {} }))
        .output_schema(json!({
            "type": "object",
            "properties": {
                "files": { "type": "number" },
                "names": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["files"]
        }))
        .arc();
    assert_eq!(
        describe_script_call(stats.as_ref()),
        "Return structured stats\n\nCodemode: `tools.stats(args)` resolves to `{ files, names? }`."
    );

    let dashed = StubTool::new("my-tool", "  Padded.  ").arc();
    assert!(
        describe_script_call(dashed.as_ref())
            .starts_with("Padded.\n\nCodemode: `tools.my_tool(args)`")
    );
}

/// CODE-020 (pi `toCodemodeDeclaration(tool, guidelines)`, `tool.ts:160-176` @v1.0.4): the
/// description of a listed tool is followed by its guidelines as bullets — the system prompt only
/// carries them for declared tools. Blank guidelines are skipped, the description is trimmed when
/// bullets follow it, and without bullets it is left as it was.
#[test]
fn a_listed_tool_carries_its_guidelines_after_its_description() {
    let listed = tool("lister", "Lists things.  \n");
    let namespaces = BTreeMap::new();
    let none: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let deferred = BTreeSet::new();
    let describe = |guidelines: &BTreeMap<String, Vec<String>>| {
        create_codemode_description(
            &[Arc::clone(&listed)],
            &DescriptionOptions {
                models: false,
                namespaces: &namespaces,
                deferred: &deferred,
                guidelines,
                inline_budget: None,
                docs_path: CODEMODE_DOCS_PATH,
            },
        )
    };

    let guided = BTreeMap::from([(
        "lister".to_owned(),
        vec![
            "  Use it first.  ".to_owned(),
            "   ".to_owned(),
            "Then stop.".to_owned(),
        ],
    )]);
    let with = describe(&guided);
    assert!(
        with.contains("Lists things.\n\n- Use it first.\n- Then stop."),
        "{with}"
    );
    assert!(
        !with.contains("- \n"),
        "a blank guideline is no bullet: {with}"
    );

    // Another tool's guidelines, or none, add nothing.
    let other = BTreeMap::from([("other".to_owned(), vec!["Not mine.".to_owned()])]);
    for map in [&none, &other] {
        let without = describe(map);
        assert!(!without.contains("Not mine."), "{without}");
        assert!(!without.contains("Lists things.\n\n- "), "{without}");
    }
}

/// pi `d677d0ee7` (v1.0.3): the globals line tells the model that `image()` also saves the image
/// and the result names its path, which is how a later turn learns it can reference the file.
#[test]
fn the_globals_line_says_image_saves_the_image_and_names_its_path() {
    let description = create_codemode_description(
        &[],
        &DescriptionOptions {
            models: false,
            namespaces: &BTreeMap::new(),
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: None,
            docs_path: CODEMODE_DOCS_PATH,
        },
    );
    assert!(
        description.contains(
            "`exit()` ends the script. `image()` also saves the image to a temp file and the result names its path.\n"
        ),
        "{description}"
    );
}
