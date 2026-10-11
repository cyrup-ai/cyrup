//! The model-facing `codemode` description, its token budget, and the per-tool "how scripts call
//! this" note (pi `extensions/codemode/tool.ts:131-326` @v1.0.1, CODE-007).
//!
//! Everything here is a pure function of the tools it is handed: the description is the helper
//! list, guidance for finding unlisted tools, the shared MCP types when a listed tool needs them,
//! the `models` line, and one section per listed tool grouped by namespace
//! ([`create_codemode_description`]). Deferred tools are never listed and do not influence it at
//! all, so the text stays the same while MCP servers connect or change their tools.
//!
//! # Production call path
//!
//! [`super::CodemodeTool::prepare_loadout`](cyrup_core::Tool::prepare_loadout) calls
//! [`create_codemode_description`] and [`describe_script_call`] on every loadout resolution
//! (session build, `set_active_tools_by_name`, tool registration), and
//! [`super::execute::execute_codemode`] calls [`to_codemode_declaration`] and [`callable_tools`] on
//! every script.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use cyrup_codemode::declarations::{
    MCP_TYPESCRIPT_PREAMBLE, mcp_structured_content_schema, render_tool_output_type,
    render_tool_sample,
};
use cyrup_codemode::identifier::IdentifierTable;
use cyrup_codemode::js::{js_trim, utf16_len};
use cyrup_codemode::types::ToolDeclaration;
use cyrup_core::{Tool, ToolNamespace};
use serde_json::{Value, json};

use super::CODEMODE_TOOL_NAME;

/// Characters per token when estimating the cost of a tool section (`tool.ts:156`).
const CHARS_PER_TOKEN: usize = 4;

/// The names of cyrup's built-in tools, which the catalog budget never spends on another tool
/// first. `cyrup_tools::BUILTIN_NAMES` states the same list; the test
/// `built_in_names_match_the_tool_registry` fails when they drift apart.
const BUILT_IN_TOOL_NAMES: [&str; 8] = [
    "bash",
    "powershell",
    "read",
    "write",
    "edit",
    "grep",
    "find",
    "ls",
];

// [CYRUP-DELTA] "a V8 sandbox" where upstream says "a QuickJS sandbox" (`tool.ts:119`): the engine
// differs (ADR-0031), everything the sentence promises — top-level `await` and `return`, no Node,
// file system, network or timers — is what the sandbox provides. The two bullets after the options
// line are [CYRUP-DELTA]s: the default limits (upstream's tool has none) and the globals a script
// must not shadow. Everything else is verbatim.
//
// [CYRUP-DELTA, PROV-101] upstream says "The input is raw JavaScript (not JSON, no code fence)"
// (`tool.ts:137`). That is only true on a route that sends the tool as a grammar-constrained
// `custom` tool; everywhere else the tool is a JSON function tool, whose input is `{"code": "..."}`
// and whose script is a JSON string. A model that read "not JSON" there would send bare
// JavaScript as the argument object, so the sentence names the `code` argument and the raw-text
// form both, and the schema's own property description stops claiming a raw channel too.
//
// [CYRUP-DELTA] The two sentences after "rejects with an Error on failure" (`tool.ts:138`) say what
// upstream only writes in a source comment on `bashOutputSchema` (`bash.ts`): a non-zero exit is an
// error result for the model, but a script still gets the structured result, while a timeout
// rejects. Without them "rejects on failure" reads as if `exit 3` threw. See also the `exit_code`
// description of `bash`'s output schema, which the declaration renders.
const DESCRIPTION_INTRO: &str = "Run JavaScript that calls other tools. The script is plain JavaScript source, never wrapped in a code fence: it is the `code` argument, or the whole input where this tool takes raw text instead of JSON. It runs as an async function body in a V8 sandbox: top-level `await` and `return` work. No Node, file system, network, or timers.\n- `await tools.<name>({ ...args })` resolves to a string, or an object if the tool's declaration says so, and rejects with an Error on failure. A `bash` command that exits non-zero is not a failure: the call resolves to its result, so check `exit_code`. A timeout still rejects. Calls still running when the script ends are cancelled.\n- Optional first line: `// @options: {\"max_output_tokens\": 10000, \"timeout_ms\": 60000}`\n- Without `timeout_ms` a script stops after 120 s of its own running time (waiting for tool calls does not count) or 30 min in all; `timeout_ms` replaces both limits.\n- `text`, `image`, `store`, `load`, `exit` and the other globals below are plain variables: do not declare your own with those names (`const text = await tools.read(...)` makes the next `text(...)` call your string). `tools` and `console` may be redeclared.";

/// One line per global; the details live in the docs (`tool.ts:122-135` `describeGlobals`).
fn describe_globals(models: bool, docs_path: &str) -> String {
    let mut lines = vec![
        "Globals:".to_owned(),
        "- `text(value)`, `image(dataUrlOrImageBlock)`, `console.log(...)`, and top-level `return` add output; `exit()` ends the script. With several text items, each starts with a `==> text N/M <==` line, and `console` lines follow the other output in one `<console_output>` block. `image()` also saves the image to a temp file and the result names its path.".to_owned(),
        "- `store(key, value)` and `load(key)` keep JSON values across codemode calls.".to_owned(),
        "- `ALL_TOOLS`, `await searchTools(query, { limit?, namespace? })`, `await describeTool(name)`, `await describeNamespace(name)`: find unlisted tools, such as MCP tools.".to_owned(),
    ];
    if models {
        // [CYRUP-DELTA] "with read, or tools.read": in `only` mode the model has no `read` tool of
        // its own, and the docs path is where cyrup materialises the page (`docs::materialise`).
        lines.push(format!(
            "- `models`: classifiers and image generation. Read {docs_path} first (with read, or tools.read in a script)."
        ));
    }
    lines.join("\n")
}

/// What a tool resolves to when its declaration has no output schema: its text output.
fn text_output_schema() -> Value {
    json!({ "type": "string" })
}

/// What a script sees of a tool (`toCodemodeDeclaration`, `tool.ts:159-166` @v1.0.1,
/// `tool.ts:160-176` @v1.0.4): its description followed by its prompt `guidelines` as bullets, which
/// the system prompt only has for declared tools (CODE-020). Tools without an output schema resolve
/// to their text output. `identifiers` is the table over every tool the script can call.
#[must_use]
pub fn to_codemode_declaration(
    tool: &dyn Tool,
    guidelines: &[String],
    identifiers: &IdentifierTable,
) -> ToolDeclaration {
    let bullets: Vec<String> = guidelines
        .iter()
        .map(|guideline| guideline.trim())
        .filter(|guideline| !guideline.is_empty())
        .map(|guideline| format!("- {guideline}"))
        .collect();
    let description = if bullets.is_empty() {
        tool.description().to_owned()
    } else {
        format!("{}\n\n{}", js_trim(tool.description()), bullets.join("\n"))
    };
    ToolDeclaration {
        name: tool.name().to_owned(),
        // [CYRUP-DELTA] The identifier the session assigned, which differs from the derived one
        // only when another tool would have the same.
        identifier: Some(identifiers.get(tool.name())),
        description: Some(description),
        input_schema: Some(tool.parameters().clone()),
        output_schema: Some(
            tool.output_schema()
                .cloned()
                .unwrap_or_else(text_output_schema),
        ),
        spread: false,
        signature: None,
    }
}

/// Tools a script may call: every given tool except the codemode tool itself
/// (`getCodemodeCallableTools`, `tool.ts:169-171`).
#[must_use]
pub fn callable_tools(tools: &[Arc<dyn Tool>]) -> Vec<Arc<dyn Tool>> {
    tools
        .iter()
        .filter(|tool| tool.name() != CODEMODE_TOOL_NAME)
        .cloned()
        .collect()
}

/// Inputs of [`create_codemode_description`] beyond the tools (`CodemodeDescriptionOptions`,
/// `tool.ts:173-185`).
#[derive(Clone, Copy, Debug)]
pub struct DescriptionOptions<'a> {
    /// Declare the `models` namespace; only for a host with model access.
    pub models: bool,
    /// Namespace of each tool, by tool name. Tools of one namespace are listed under one heading.
    pub namespaces: &'a BTreeMap<String, ToolNamespace>,
    /// Tools that are callable but never listed with their declaration (`deferred` exposure).
    pub deferred: &'a std::collections::BTreeSet<String>,
    /// The prompt guidelines of each tool, by tool name, listed after its description.
    pub guidelines: &'a BTreeMap<String, Vec<String>>,
    /// Estimated tokens (characters / 4) the tool sections may use. Tools that do not fit are left
    /// out, like deferred tools. `None` lists every tool that is not deferred.
    pub inline_budget: Option<f64>,
    /// The docs the `models` line points the model to (`CODEMODE_DOCS_PATH`).
    pub docs_path: &'a str,
    /// [CYRUP-DELTA] The identifier of every tool a script can call, listed or not, so a heading
    /// names the identifier the sandbox registers the tool under.
    pub identifiers: &'a IdentifierTable,
}

/// `### \`id\` (\`raw name\`)` followed by the tool's description and declaration
/// (`renderToolSection`, `tool.ts:187-191`).
fn render_tool_section(declaration: &ToolDeclaration) -> String {
    let id = declaration.script_identifier();
    let heading = if id.as_str() == declaration.name {
        format!("### `{id}`")
    } else {
        format!("### `{id}` (`{}`)", declaration.name)
    };
    format!(
        "{heading}\n{}",
        js_trim(&render_tool_sample(declaration, None))
    )
}

struct CatalogEntry {
    name: String,
    section: String,
    cost: f64,
    /// A built-in tool of the coding agent, which [`select_catalog`] places before anything else.
    built_in: bool,
}

struct CatalogGroup {
    namespace: Option<ToolNamespace>,
    entries: Vec<CatalogEntry>,
}

/// Pick the tool sections that fit the budget, like OpenCode's catalog (`selectCatalog`,
/// `tool.ts:206-226`): in each round every group (tools without a namespace first, then namespaces
/// by name) places its cheapest remaining tool; a group whose next tool does not fit drops out
/// while the others continue. Every namespace is represented before any namespace is complete.
///
/// [CYRUP-DELTA] The built-in tools are placed first, cheapest first, before the rounds start. In
/// `codemode.mode = only` they are hidden from the model, which then learns that `tools.edit` exists
/// from this listing alone, and a round-robin over many extension tools spent the budget on cheaper
/// ones first. Anything the budget cannot take is still counted as not listed in the description.
fn select_catalog(
    groups: &[CatalogGroup],
    budget: Option<f64>,
) -> std::collections::BTreeSet<String> {
    let Some(mut remaining) = budget else {
        return groups
            .iter()
            .flat_map(|group| group.entries.iter().map(|entry| entry.name.clone()))
            .collect();
    };
    let mut shown = std::collections::BTreeSet::new();
    let mut built_ins: Vec<&CatalogEntry> = groups
        .iter()
        .flat_map(|group| group.entries.iter())
        .filter(|entry| entry.built_in)
        .collect();
    built_ins.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    for entry in built_ins {
        if entry.cost <= remaining {
            remaining -= entry.cost;
            shown.insert(entry.name.clone());
        }
    }
    let mut queues: Vec<VecDeque<&CatalogEntry>> = groups
        .iter()
        .map(|group| {
            let mut sorted: Vec<&CatalogEntry> = group
                .entries
                .iter()
                .filter(|entry| !entry.built_in)
                .collect();
            // `Array.prototype.sort` is stable; so is this.
            sorted.sort_by(|a, b| a.cost.total_cmp(&b.cost));
            sorted.into()
        })
        .collect();
    queues.retain(|queue| !queue.is_empty());
    while !queues.is_empty() {
        queues.retain_mut(|queue| {
            let Some(next) = queue.front() else {
                return false;
            };
            if next.cost > remaining {
                return false;
            }
            remaining -= next.cost;
            shown.insert(next.name.clone());
            queue.pop_front();
            !queue.is_empty()
        });
    }
    shown
}

/// The model-facing description (`createCodemodeDescription`, `tool.ts:237-289`): the helper list,
/// guidance for finding tools that are not listed, the shared MCP types when listed tools need them,
/// the `models` API, and one section per listed tool, grouped by namespace. Deferred tools are never
/// listed and do not affect the description at all. Tool sections are limited to
/// [`DescriptionOptions::inline_budget`].
#[must_use]
pub fn create_codemode_description(
    tools: &[Arc<dyn Tool>],
    options: &DescriptionOptions<'_>,
) -> String {
    let declarations: Vec<ToolDeclaration> = callable_tools(tools)
        .iter()
        .filter(|tool| !options.deferred.contains(tool.name()))
        .map(|tool| {
            let guidelines = options
                .guidelines
                .get(tool.name())
                .map_or(&[][..], Vec::as_slice);
            to_codemode_declaration(tool.as_ref(), guidelines, options.identifiers)
        })
        .collect();

    // Insertion-ordered groups, the namespace-less one first; then ordered for display.
    let mut groups: Vec<(String, CatalogGroup)> = vec![(
        String::new(),
        CatalogGroup {
            namespace: None,
            entries: Vec::new(),
        },
    )];
    for declaration in &declarations {
        let namespace = options.namespaces.get(&declaration.name);
        let key = namespace.map_or_else(String::new, |ns| format!("ns:{}", ns.name));
        let at = match groups.iter().position(|(existing, _)| *existing == key) {
            Some(at) => at,
            None => {
                groups.push((
                    key,
                    CatalogGroup {
                        namespace: namespace.cloned(),
                        entries: Vec::new(),
                    },
                ));
                groups.len() - 1
            }
        };
        let section = render_tool_section(declaration);
        let cost = utf16_len(&section).div_ceil(CHARS_PER_TOKEN) as f64;
        if let Some((_, group)) = groups.get_mut(at) {
            group.entries.push(CatalogEntry {
                name: declaration.name.clone(),
                section,
                cost,
                built_in: namespace.is_none()
                    && BUILT_IN_TOOL_NAMES.contains(&declaration.name.as_str()),
            });
        }
    }
    let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
    let mut ordered: Vec<CatalogGroup> = groups.into_iter().map(|(_, group)| group).collect();
    ordered.sort_by(|a, b| match (&a.namespace, &b.namespace) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Less,
        (_, None) => std::cmp::Ordering::Greater,
        (Some(a), Some(b)) => collator.collate(a.name.as_str(), b.name.as_str()),
    });
    let shown = select_catalog(&ordered, options.inline_budget);

    let mut sections = vec![
        DESCRIPTION_INTRO.to_owned(),
        describe_globals(options.models, options.docs_path),
    ];
    if declarations.iter().any(|declaration| {
        shown.contains(&declaration.name)
            && mcp_structured_content_schema(declaration.output_schema.as_ref()).is_some()
    }) {
        sections.push(format!(
            "Shared MCP Types:\n```ts\n{MCP_TYPESCRIPT_PREAMBLE}\n```"
        ));
    }
    if declarations.is_empty() {
        return sections.join("\n\n");
    }

    let mut tool_sections = vec!["Nested tools:".to_owned()];
    for group in &ordered {
        let visible: Vec<&CatalogEntry> = group
            .entries
            .iter()
            .filter(|entry| shown.contains(&entry.name))
            .collect();
        if let Some(namespace) = &group.namespace {
            // Only tools that did not fit the budget are counted as not listed here.
            let listing = if visible.len() == group.entries.len() {
                ""
            } else if visible.is_empty() {
                " (tools not listed)"
            } else {
                " (some tools not listed)"
            };
            let description = namespace.description.as_deref().map(js_trim);
            tool_sections.push(match description.filter(|text| !text.is_empty()) {
                Some(description) => format!("## {}{listing}\n{description}", namespace.name),
                None => format!("## {}{listing}", namespace.name),
            });
        }
        let listed = visible.len();
        tool_sections.extend(visible.into_iter().map(|entry| entry.section.clone()));
        // [CYRUP-DELTA] Tools without a namespace have no heading to carry the "not listed" note, so
        // the note is a line of its own. Without it a budget that cut them left no trace, and with
        // nothing listed at all the section was a bare "Nested tools:".
        if group.namespace.is_none() && listed < group.entries.len() {
            let hidden = group.entries.len() - listed;
            tool_sections.push(format!(
                "{hidden} {} not listed; use ALL_TOOLS / searchTools",
                if hidden == 1 { "tool" } else { "tools" }
            ));
        }
    }
    sections.push(tool_sections.join("\n\n"));
    sections.join("\n\n")
}

/// What a script call resolves to, in one line (`describeOutput`, `tool.ts:296-312`): `a string`,
/// the field names of an object (`{ output, exit_code, full_output_path? }`), or the rendered type
/// for anything else.
fn describe_output(schema: Option<&Value>) -> String {
    let rendered = render_tool_output_type(schema);
    if rendered == "string" {
        return "a string".to_owned();
    }
    if let Some(object) = schema.and_then(Value::as_object)
        && object.get("type").and_then(Value::as_str) == Some("object")
        && let Some(properties) = object.get("properties").and_then(Value::as_object)
        && mcp_structured_content_schema(schema).is_none()
    {
        let required: std::collections::BTreeSet<&str> = object
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let fields: Vec<String> = cyrup_codemode::js::own_keys(properties)
            .into_iter()
            .map(|name| {
                if required.contains(name) {
                    name.to_owned()
                } else {
                    format!("{name}?")
                }
            })
            .collect();
        return format!("`{{ {} }}`", fields.join(", "));
    }
    // `type.replace(/\s+/g, " ")`: every run of whitespace, at either end too, becomes one space.
    let mut collapsed = String::with_capacity(rendered.len());
    let mut in_whitespace = false;
    for c in rendered.chars() {
        if cyrup_codemode::js::is_js_whitespace(c) {
            if !in_whitespace {
                collapsed.push(' ');
            }
            in_whitespace = true;
        } else {
            collapsed.push(c);
            in_whitespace = false;
        }
    }
    format!("`{collapsed}`")
}

/// A declared tool's description followed by how scripts call it and what the call resolves to
/// (`describeScriptCall`, `tool.ts:318-322`). The arguments are the tool's declared parameters, so
/// they are not repeated.
#[must_use]
pub fn describe_script_call(tool: &dyn Tool, identifiers: &IdentifierTable) -> String {
    // No guidelines: a declared tool's are rules of the system prompt (`describeScriptCall` is
    // unchanged at v1.0.4).
    let declaration = to_codemode_declaration(tool, &[], identifiers);
    format!(
        "{}\n\nCodemode: `tools.{}(args)` resolves to {}.",
        js_trim(tool.description()),
        declaration.script_identifier(),
        describe_output(declaration.output_schema.as_ref())
    )
}

#[cfg(test)]
mod tests;
