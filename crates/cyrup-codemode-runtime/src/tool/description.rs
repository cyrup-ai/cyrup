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
use cyrup_codemode::identifier::to_codemode_identifier;
use cyrup_codemode::js::{js_trim, utf16_len};
use cyrup_codemode::types::ToolDeclaration;
use cyrup_core::{Tool, ToolNamespace};
use serde_json::{Value, json};

use super::CODEMODE_TOOL_NAME;

/// Characters per token when estimating the cost of a tool section (`tool.ts:156`).
const CHARS_PER_TOKEN: usize = 4;

// [CYRUP-DELTA] "a V8 sandbox" where upstream says "a QuickJS sandbox" (`tool.ts:119`): the engine
// differs (ADR-0031), everything the sentence promises — top-level `await` and `return`, no Node,
// file system, network or timers — is what the sandbox provides. Everything else is verbatim.
const DESCRIPTION_INTRO: &str = "Run JavaScript that calls other tools. The input is raw JavaScript (not JSON, no code fence), run as an async function body in a V8 sandbox: top-level `await` and `return` work. No Node, file system, network, or timers.\n- `await tools.<name>({ ...args })` resolves to a string, or an object if the tool's declaration says so, and rejects with an Error on failure. Calls still running when the script ends are cancelled.\n- Optional first line: `// @options: {\"max_output_tokens\": 10000, \"timeout_ms\": 60000}`";

/// One line per global; the details live in the docs (`tool.ts:122-135` `describeGlobals`).
fn describe_globals(models: bool, docs_path: &str) -> String {
    let mut lines = vec![
        "Globals:".to_owned(),
        "- `text(value)`, `image(dataUrlOrImageBlock)`, `console.log(...)`, and top-level `return` add output; `exit()` ends the script.".to_owned(),
        "- `store(key, value)` and `load(key)` keep JSON values across codemode calls.".to_owned(),
        "- `ALL_TOOLS`, `searchTools(query, { limit?, namespace? })`, `describeTool(name)`, `describeNamespace(name)`: find unlisted tools, such as MCP tools.".to_owned(),
    ];
    if models {
        lines.push(format!(
            "- `models`: classifiers and image generation. Read {docs_path} first."
        ));
    }
    lines.join("\n")
}

/// What a tool resolves to when its declaration has no output schema: its text output.
fn text_output_schema() -> Value {
    json!({ "type": "string" })
}

/// What a script sees of a tool (`toCodemodeDeclaration`, `tool.ts:159-166`). Tools without an
/// output schema resolve to their text output.
#[must_use]
pub fn to_codemode_declaration(tool: &dyn Tool) -> ToolDeclaration {
    ToolDeclaration {
        name: tool.name().to_owned(),
        description: Some(tool.description().to_owned()),
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
    /// Estimated tokens (characters / 4) the tool sections may use. Tools that do not fit are left
    /// out, like deferred tools. `None` lists every tool that is not deferred.
    pub inline_budget: Option<f64>,
    /// The docs the `models` line points the model to (`CODEMODE_DOCS_PATH`).
    pub docs_path: &'a str,
}

/// `### \`id\` (\`raw name\`)` followed by the tool's description and declaration
/// (`renderToolSection`, `tool.ts:187-191`).
fn render_tool_section(declaration: &ToolDeclaration) -> String {
    let id = to_codemode_identifier(&declaration.name);
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
}

struct CatalogGroup {
    namespace: Option<ToolNamespace>,
    entries: Vec<CatalogEntry>,
}

/// Pick the tool sections that fit the budget, like OpenCode's catalog (`selectCatalog`,
/// `tool.ts:206-226`): in each round every group (tools without a namespace first, then namespaces
/// by name) places its cheapest remaining tool; a group whose next tool does not fit drops out
/// while the others continue. Every namespace is represented before any namespace is complete.
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
    let mut queues: Vec<VecDeque<&CatalogEntry>> = groups
        .iter()
        .map(|group| {
            let mut sorted: Vec<&CatalogEntry> = group.entries.iter().collect();
            // `Array.prototype.sort` is stable; so is this.
            sorted.sort_by(|a, b| a.cost.total_cmp(&b.cost));
            sorted.into()
        })
        .collect();
    let mut shown = std::collections::BTreeSet::new();
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
        .map(|tool| to_codemode_declaration(tool.as_ref()))
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
        tool_sections.extend(visible.into_iter().map(|entry| entry.section.clone()));
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
pub fn describe_script_call(tool: &dyn Tool) -> String {
    let declaration = to_codemode_declaration(tool);
    format!(
        "{}\n\nCodemode: `tools.{}(args)` resolves to {}.",
        js_trim(tool.description()),
        to_codemode_identifier(tool.name()),
        describe_output(declaration.output_schema.as_ref())
    )
}

#[cfg(test)]
mod tests;
