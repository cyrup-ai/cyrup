//! The pure half of the script discovery globals `searchTools()`, `describeTool()` and
//! `describeNamespace()` (pi `packages/coding-agent/src/extensions/codemode/execute.ts:437-517`
//! `isNamespaceName` and `createDiscoveryGlobals` @v1.0.1, CODE-010).
//!
//! What a global does with the script's arguments (type checks, `limit` validation, the thrown
//! `Error` text) and how it reaches the script is the globals lane's; this module is what each of
//! them computes once the arguments are valid: which tools are candidates, in what order, and what
//! a namespace lookup returns.
//!
//! # Production call path
//!
//! The globals lane's `searchTools`, `describeTool` and `describeNamespace` implementations call
//! [`search_tools`], [`find_tool`] and [`describe_namespace`] respectively. Nothing in this crate
//! calls them: they are the crate's public API for that lane.

use cyrup_core::ToolNamespace;
use serde::Serialize;
use serde_json::Value;

use crate::identifier::{CodemodeIdentifier, to_codemode_identifier};
use crate::rank::{ToolRanker, ToolSearchMatch, create_tool_search_document};

/// A tool as discovery sees it: what [`create_tool_search_document`] reads, plus the namespace the
/// session registered for it (pi `options.getToolNamespace(tool.name)`).
#[derive(Clone, Copy, Debug)]
pub struct DiscoverableTool<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub parameters: &'a Value,
    pub namespace: Option<&'a ToolNamespace>,
}

/// Whether `query` names the namespace: its name, its script identifier (`mcp__dev-radius` is
/// `mcp__dev_radius`), or the part after its last `__` in either form (`dev-radius`, `dev_radius`)
/// (`execute.ts:440-445`).
#[must_use]
pub fn is_namespace_name(namespace: &str, query: &str) -> bool {
    let id = to_codemode_identifier(namespace);
    let query_id = to_codemode_identifier(query);
    fn suffix(name: &str) -> Option<&str> {
        name.rfind("__").and_then(|at| name.get(at + 2..))
    }
    namespace == query
        || id == query_id
        || suffix(namespace) == Some(query)
        || suffix(id.as_str()) == Some(query_id.as_str())
}

/// `searchTools(query, { limit, namespace })` once its arguments are valid (`execute.ts:473-478`):
/// build a search document per tool in `tools` order, restricted to one namespace when `namespace`
/// is a non-empty string (a tool with no namespace never matches a restriction), and rank them.
#[must_use]
pub fn search_tools(
    ranker: &impl ToolRanker,
    tools: &[DiscoverableTool<'_>],
    query: &str,
    limit: usize,
    namespace: Option<&str>,
) -> Vec<ToolSearchMatch> {
    let restriction = namespace.filter(|name| !name.is_empty());
    let documents: Vec<_> = tools
        .iter()
        .filter(|tool| match restriction {
            None => true,
            Some(wanted) => tool
                .namespace
                .is_some_and(|namespace| is_namespace_name(&namespace.name, wanted)),
        })
        .map(|tool| {
            create_tool_search_document(
                tool.name,
                tool.description,
                tool.parameters,
                tool.namespace,
            )
        })
        .collect();
    ranker.rank(query, &documents, limit)
}

/// `describeTool(name)`'s lookup (`execute.ts:487-490`): the first tool whose raw name or codemode
/// identifier is `name`. `describeTool("mcp__dev_radius__search")` and
/// `describeTool("mcp__dev-radius__search")` find the same tool.
#[must_use]
pub fn find_tool<'a, 'b>(
    tools: &'b [DiscoverableTool<'a>],
    name: &str,
) -> Option<&'b DiscoverableTool<'a>> {
    tools
        .iter()
        .find(|tool| tool.name == name || to_codemode_identifier(tool.name).as_str() == name)
}

/// What `describeNamespace(name)` returns to the script (`execute.ts:508-513`): `name`,
/// `description` and `instructions` when non-empty, and the codemode identifiers of the namespace's
/// tools in `tools` order. Serialises to exactly that object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NamespaceDescription {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub tools: Vec<CodemodeIdentifier>,
}

/// `describeNamespace(name)` (`execute.ts:499-513`): the first matching tool's namespace
/// describes the group, every tool whose namespace matches `name` is listed. `None` when no tool
/// has a matching namespace.
#[must_use]
pub fn describe_namespace(
    tools: &[DiscoverableTool<'_>],
    name: &str,
) -> Option<NamespaceDescription> {
    let mut namespace: Option<&ToolNamespace> = None;
    let mut names: Vec<CodemodeIdentifier> = Vec::new();
    for tool in tools {
        let Some(tool_namespace) = tool.namespace else {
            continue;
        };
        if !is_namespace_name(&tool_namespace.name, name) {
            continue;
        }
        namespace.get_or_insert(tool_namespace);
        names.push(to_codemode_identifier(tool.name));
    }
    let namespace = namespace?;
    Some(NamespaceDescription {
        name: namespace.name.clone(),
        description: namespace
            .description
            .clone()
            .filter(|text| !text.is_empty()),
        instructions: namespace
            .instructions
            .clone()
            .filter(|text| !text.is_empty()),
        tools: names,
    })
}

#[cfg(test)]
mod tests;
