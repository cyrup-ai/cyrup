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

use cyrup_codemode::identifier::IdentifierTable;
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
                identifiers: &IdentifierTable::default(),
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
            identifiers: &IdentifierTable::default(),
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
            identifiers: &IdentifierTable::default(),
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
            identifiers: &IdentifierTable::default(),
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
                identifiers: &IdentifierTable::default(),
            },
        )
    };
    let plain = describe(false);
    assert!(plain.starts_with(
        "Run JavaScript that calls other tools. The script is plain JavaScript source"
    ));
    // PROV-101: on a provider without grammar tools the input is `{"code": "..."}`, so the text
    // may not promise a raw, non-JSON channel (upstream's "not JSON" is true only for a grammar
    // tool) and must name the argument the script goes in.
    assert!(plain.contains("`code` argument"), "{plain}");
    assert!(!plain.contains("not JSON"), "{plain}");
    assert!(plain.contains("// @options: {\"max_output_tokens\": 10000, \"timeout_ms\": 60000}"));
    assert!(plain.contains("\n\nGlobals:\n- `text(value)`, `image(dataUrlOrImageBlock)`"));
    assert!(
        !plain.contains("Nested tools:"),
        "no tools, no tool section"
    );
    assert!(!plain.contains("`models`"));

    let with_models = describe(true);
    assert!(with_models.contains(&format!(
        "- `models`: classifiers and image generation. Read {CODEMODE_DOCS_PATH} first (with read, or tools.read in a script)."
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
            identifiers: &IdentifierTable::default(),
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
                identifiers: &IdentifierTable::default(),
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
        describe_script_call(echo.as_ref(), &IdentifierTable::default()),
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
        describe_script_call(stats.as_ref(), &IdentifierTable::default()),
        "Return structured stats\n\nCodemode: `tools.stats(args)` resolves to `{ files, names? }`."
    );

    let dashed = StubTool::new("my-tool", "  Padded.  ").arc();
    assert!(
        describe_script_call(dashed.as_ref(), &IdentifierTable::default())
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
                identifiers: &IdentifierTable::default(),
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
            identifiers: &IdentifierTable::default(),
        },
    );
    assert!(
        description.contains(
            "`exit()` ends the script. With several text items, each starts with a `==> text N/M <==` line, and `console` lines follow the other output in one `<console_output>` block. `image()` also saves the image to a temp file and the result names its path.\n"
        ),
        "{description}"
    );
}

/// pi `269121616` (v1.1.0, #10555; `tool.ts:147`): the lookup helpers are async, and the globals
/// line says `await`, because models that left it off serialized the unawaited promise as `{}`.
/// cyrup's helpers return promises too (`prelude.js` `caller`).
#[test]
fn the_globals_line_marks_the_lookup_helpers_as_async() {
    let description = create_codemode_description(
        &[],
        &DescriptionOptions {
            models: false,
            namespaces: &BTreeMap::new(),
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: None,
            docs_path: CODEMODE_DOCS_PATH,
            identifiers: &IdentifierTable::default(),
        },
    );
    assert!(
        description.contains("`await searchTools(query, { limit?, namespace? })`"),
        "{description}"
    );
    assert!(
        description.contains("`await describeTool(name)`"),
        "{description}"
    );
    assert!(
        description.contains("`await describeNamespace(name)`"),
        "{description}"
    );
}

// ── TOOL-SURFACE: markers for the namespace-less group, built-ins first, colliding identifiers ───

fn describe_with(
    tools: &[Arc<dyn Tool>],
    namespaces: &BTreeMap<String, ToolNamespace>,
    budget: Option<f64>,
) -> String {
    create_codemode_description(
        tools,
        &DescriptionOptions {
            models: false,
            namespaces,
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: budget,
            docs_path: CODEMODE_DOCS_PATH,
            identifiers: &IdentifierTable::assign(tools.iter().map(|tool| tool.name())),
        },
    )
}

/// [CYRUP-DELTA] A namespace heading carries "(some tools not listed)"; tools without a namespace
/// have no heading, so the budget cutting them left no trace, and cutting all of them left a bare
/// `Nested tools:`.
#[test]
fn tools_without_a_namespace_that_the_budget_cut_are_counted() {
    let tools: Vec<Arc<dyn Tool>> = (0..4)
        .map(|n| tool(&format!("ext_{n}"), "Does a thing."))
        .collect();
    let none = BTreeMap::new();

    let all = describe_with(&tools, &none, None);
    assert!(!all.contains("not listed"), "{all}");

    // Each section costs 37 tokens: two fit in 80, three in 115.
    let some = describe_with(&tools, &none, Some(80.0));
    assert!(
        some.contains("\n\n2 tools not listed; use ALL_TOOLS / searchTools"),
        "{some}"
    );
    assert_eq!(some.matches("codemode tool declaration:").count(), 2);

    let one = describe_with(&tools, &none, Some(115.0));
    assert!(
        one.ends_with("\n\n1 tool not listed; use ALL_TOOLS / searchTools"),
        "{one}"
    );

    let nothing = describe_with(&tools, &none, Some(0.0));
    assert!(
        nothing.ends_with("Nested tools:\n\n4 tools not listed; use ALL_TOOLS / searchTools"),
        "{nothing}"
    );
    assert!(!nothing.contains("codemode tool declaration:"));
}

/// The marker sits after the namespace-less sections and before the first namespace heading.
#[test]
fn the_not_listed_line_of_the_unnamespaced_group_comes_before_the_namespaces() {
    let catalog = Catalog::new();
    let tools = vec![
        tool("ext_a", "Does a."),
        tool("ext_b", &"Does b. ".repeat(80)),
    ];
    let mut all = tools.clone();
    all.extend(catalog.github.iter().cloned());
    let description = describe_with(&all, &catalog.namespaces, Some(120.0));
    let marker = description
        .find("1 tool not listed; use ALL_TOOLS / searchTools")
        .unwrap_or_else(|| panic!("no marker:\n{description}"));
    let heading = description.find("## mcp__github").unwrap();
    assert!(marker < heading, "{description}");
}

/// [CYRUP-DELTA] In `only` mode the model learns the built-ins from this listing alone, and the
/// round-robin budget used to spend itself on cheaper extension tools first.
#[test]
fn built_in_tools_are_listed_before_other_tools_spend_the_budget() {
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        tool("bash", &"Run a command. ".repeat(40)),
        tool("edit", &"Edit a file. ".repeat(40)),
    ];
    let mut namespaces = BTreeMap::new();
    for n in 0..5 {
        tools.push(tool(&format!("ext_{n}"), "Does a thing."));
    }
    for ns in 0..6 {
        let name = format!("mcp__server{ns}__ping");
        namespaces.insert(
            name.clone(),
            ToolNamespace {
                name: format!("mcp__server{ns}"),
                description: None,
                instructions: None,
            },
        );
        tools.push(tool(&name, "Ping."));
    }
    let description = describe_with(&tools, &namespaces, Some(700.0));
    assert!(description.contains("### `bash`"), "{description}");
    assert!(description.contains("### `edit`"), "{description}");
    // The rest of the budget still goes round-robin: every server keeps its tool.
    for ns in 0..6 {
        assert!(
            description.contains(&format!("### `mcp__server{ns}__ping`")),
            "server{ns} lost its tool:\n{description}"
        );
    }
    // Something was cut, and the line says so.
    assert!(description.contains("tools not listed; use ALL_TOOLS / searchTools"));
}

/// A built-in that does not fit a budget the user set that small is cut and counted, not forced.
#[test]
fn a_built_in_the_budget_cannot_take_is_counted_as_not_listed() {
    let tools = vec![tool("bash", &"Run a command. ".repeat(40))];
    let description = describe_with(&tools, &BTreeMap::new(), Some(10.0));
    assert!(
        description.ends_with("1 tool not listed; use ALL_TOOLS / searchTools"),
        "{description}"
    );
}

/// A namespace-less tool that is not a built-in does not get the priority, whatever its name looks
/// like, and a namespaced `read` is not the built-in: with room for one section, the namespace-less
/// tool the round-robin places first keeps it.
#[test]
fn only_namespace_less_built_ins_are_placed_first() {
    let mut namespaces = BTreeMap::new();
    namespaces.insert(
        "read".to_owned(),
        ToolNamespace {
            name: "reader".into(),
            description: None,
            instructions: None,
        },
    );
    let tools = vec![tool("read", "Reads."), tool("tiny", "Hi.")];
    let description = describe_with(&tools, &namespaces, Some(50.0));
    assert!(description.contains("### `tiny`"), "{description}");
    assert!(!description.contains("### `read`"), "{description}");
    assert!(
        description.contains("## reader (tools not listed)"),
        "{description}"
    );
}

/// `BUILT_IN_TOOL_NAMES` states the registry's list a second time so this crate need not depend on
/// the tools crate; this fails when they stop agreeing.
#[test]
fn built_in_names_match_the_tool_registry() {
    let mut ours = super::BUILT_IN_TOOL_NAMES.to_vec();
    let mut registry = cyrup_tools::BUILTIN_NAMES.to_vec();
    ours.sort_unstable();
    registry.sort_unstable();
    assert_eq!(ours, registry);
}

/// Two MCP servers `a-b` and `a_b` each with a tool `search`: both headings name the identifier the
/// sandbox registers the tool under, and the raw name of the one that was renamed.
#[test]
fn headings_name_the_identifier_the_sandbox_assigns_to_colliding_tools() {
    let tools = vec![
        tool("mcp__a-b__search", "Search a-b."),
        tool("mcp__a_b__search", "Search a_b."),
    ];
    let description = describe_with(&tools, &BTreeMap::new(), None);
    assert!(
        description.contains("### `mcp__a_b__search_2` (`mcp__a-b__search`)\nSearch a-b."),
        "{description}"
    );
    assert!(
        description.contains("### `mcp__a_b__search`\nSearch a_b."),
        "{description}"
    );
    assert!(description.contains("mcp__a_b__search_2(args:"));
}

#[test]
fn the_script_call_note_names_the_assigned_identifier() {
    let dashed = tool("gh-search", "Search.");
    let plain = tool("gh_search", "Search.");
    let identifiers = IdentifierTable::assign(["gh-search", "gh_search"]);
    assert!(
        describe_script_call(dashed.as_ref(), &identifiers).contains("`tools.gh_search_2(args)`")
    );
    assert!(describe_script_call(plain.as_ref(), &identifiers).contains("`tools.gh_search(args)`"));
}

/// TOOL-054/TOOL-058: the declarations of the built-ins are the types their calls resolve to. `read`
/// declares text or an image block, `bash` the structured result that also covers a non-zero exit;
/// the others (upstream declares no output schema for them) resolve to text.
#[test]
fn the_built_in_declarations_state_what_their_calls_resolve_to() {
    let tools = cyrup_tools::all_tools(
        std::path::PathBuf::from("."),
        cyrup_tools::Backend::default(),
        cyrup_tools::ToolsOptions::default(),
    );
    let description = describe_with(&tools, &BTreeMap::new(), None);
    // The declaration of one tool: from its `declare const tools: { name(` to the closing fence.
    let declaration = |name: &str| {
        let start = description
            .find(&format!("declare const tools: {{ {name}(args:"))
            .unwrap_or_else(|| panic!("{name} is declared:\n{description}"));
        let rest = &description[start..];
        rest[..rest.find("```").unwrap_or(rest.len())].to_owned()
    };
    assert!(
        declaration("read")
            .contains("}): Promise<string | { data: string; mimeType: string; note: string; type: \"image\"; }>;"),
        "{}",
        declaration("read")
    );
    let bash = declaration("bash");
    for field in [
        "exit_code: number;",
        "full_output_path?: string;",
        "output: string;",
        "truncated: boolean;",
        "wall_time_seconds: number;",
    ] {
        assert!(bash.contains(field), "{field} missing: {bash}");
    }
    assert!(bash.contains("}): Promise<{\n"), "{bash}");
    for text_tool in ["edit", "write", "grep", "find", "ls"] {
        let declaration = declaration(text_tool);
        assert!(
            declaration.contains("}): Promise<string>;"),
            "{text_tool}: {declaration}"
        );
    }
    // The call note of a declared tool says the same.
    let identifiers = IdentifierTable::assign(tools.iter().map(|tool| tool.name()));
    let note = |name: &str| {
        let tool = tools.iter().find(|tool| tool.name() == name).unwrap();
        describe_script_call(tool.as_ref(), &identifiers)
    };
    assert!(note("bash").contains(
        "`tools.bash(args)` resolves to `{ output, truncated, full_output_path?, exit_code, wall_time_seconds }`."
    ), "{}", note("bash"));
    assert!(
        note("read").contains("`tools.read(args)` resolves to "),
        "{}",
        note("read")
    );
}

/// "Rejects with an Error on failure" (upstream's wording, kept) must not read as if a failing
/// command throws: the description says a non-zero `bash` exit still resolves, and that a timeout
/// is what rejects. (Live acceptance, scenario "bash-nonzero": the behaviour was right, but nothing
/// the model reads said so.)
#[test]
fn the_intro_says_a_non_zero_bash_exit_resolves_and_a_timeout_rejects() {
    let none = BTreeMap::new();
    let description = create_codemode_description(
        &[],
        &DescriptionOptions {
            models: false,
            namespaces: &none,
            deferred: &BTreeSet::new(),
            guidelines: &BTreeMap::new(),
            inline_budget: None,
            docs_path: CODEMODE_DOCS_PATH,
            identifiers: &IdentifierTable::default(),
        },
    );
    assert!(
        description.contains(
            "rejects with an Error on failure. A `bash` command that exits non-zero is not a failure: the call resolves to its result, so check `exit_code`. A timeout still rejects."
        ),
        "{description}"
    );
}

/// The declaration of `bash` carries the same fact where the field is typed, so a model working
/// from `describeTool("bash")` or the `only`-mode listing sees it beside `exit_code`.
#[test]
fn the_bash_declaration_comments_that_a_non_zero_exit_code_still_resolves() {
    let tools = cyrup_tools::all_tools(
        std::path::PathBuf::from("."),
        cyrup_tools::Backend::default(),
        cyrup_tools::ToolsOptions::default(),
    );
    let description = describe_with(&tools, &BTreeMap::new(), None);
    let start = description
        .find("declare const tools: { bash(args:")
        .unwrap_or_else(|| panic!("bash is declared:\n{description}"));
    let rest = &description[start..];
    let bash = &rest[..rest.find("```").unwrap_or(rest.len())];
    let comment = "// Exit code. A non-zero code is an error for the model, but the call still resolves to this result; a timeout rejects\n";
    let at = bash
        .find(comment)
        .unwrap_or_else(|| panic!("the exit_code comment is missing: {bash}"));
    assert!(
        bash[at + comment.len()..]
            .trim_start()
            .starts_with("exit_code: number;"),
        "the comment sits on exit_code: {bash}"
    );
}
