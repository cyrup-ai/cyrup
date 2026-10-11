//! `searchTools()`, `describeTool()` and `describeNamespace()`: ranked search and lookup over the
//! script's nested tools and their namespaces (pi `createDiscoveryGlobals`,
//! `extensions/codemode/execute.ts:455-517` @v1.0.1, CODE-010).
//!
//! This module is the argument validation, the thrown `Error` texts and the shape of what reaches
//! the script; the ranking, the namespace matching and the lookups are
//! [`cyrup_codemode::discovery`] and [`cyrup_codemode::rank`].
//!
//! # Production call path
//!
//! [`super::execute::execute_codemode`] passes [`discovery_globals`] to the sandbox on every script,
//! over the host's callable tools.

use std::collections::BTreeMap;
use std::sync::Arc;

use cyrup_codemode::declarations::{MCP_TYPESCRIPT_PREAMBLE, mcp_structured_content_schema};
use cyrup_codemode::discovery::{DiscoverableTool, describe_namespace, find_tool, search_tools};
use cyrup_codemode::identifier::IdentifierTable;
use cyrup_codemode::rank::{Bm25Ranker, DEFAULT_TOOL_SEARCH_LIMIT};
use cyrup_core::Tool;
use serde_json::{Value, json};

use super::globals::spread_global;
use crate::types::CodemodeTool;

/// `searchTools(query, { limit?, namespace? })`'s `{ name, description }` entry (`entry`,
/// `execute.ts:469`): the script identifier and the tool's sample, empty when it has none.
fn entry(name: &str, samples: &BTreeMap<String, String>, identifiers: &IdentifierTable) -> Value {
    json!({
        "name": identifiers.get(name).as_str(),
        "description": samples.get(name).map_or("", String::as_str),
    })
}

/// The member `key` of `options` when `options` is an object and the member is neither absent nor
/// `null` — JavaScript's `options?.key` followed by `??` (`execute.ts:476-481`).
fn member<'a>(options: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    options
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .filter(|value| !value.is_null())
}

/// `typeof limit === "number" && Number.isInteger(limit) && limit > 0`.
fn positive_integer(value: &Value) -> Option<usize> {
    let number = value.as_f64()?;
    if number.fract() == 0.0 && number > 0.0 {
        // Saturates for a number beyond `usize`.
        Some(number as usize)
    } else {
        None
    }
}

/// The three discovery globals over `tools` (the script's callable tools, `codemode` excluded).
/// `samples` maps a tool name to its declaration sample (the `ALL_TOOLS` description), and
/// `identifiers` is the table the sandbox registers the tools under.
#[must_use]
pub fn discovery_globals(
    tools: Arc<Vec<Arc<dyn Tool>>>,
    samples: Arc<BTreeMap<String, String>>,
    identifiers: Arc<IdentifierTable>,
) -> Vec<CodemodeTool> {
    let ranker = Bm25Ranker::default();
    let search = {
        let (tools, samples, identifiers) = (
            Arc::clone(&tools),
            Arc::clone(&samples),
            Arc::clone(&identifiers),
        );
        spread_global("searchTools", move |args, _context| {
            let (tools, samples, identifiers) = (
                Arc::clone(&tools),
                Arc::clone(&samples),
                Arc::clone(&identifiers),
            );
            async move {
                let Some(query) = args.first().and_then(Value::as_str) else {
                    return Err("searchTools() expects a query string".to_owned());
                };
                let options = args.get(1);
                let limit = match member(options, "limit") {
                    None => DEFAULT_TOOL_SEARCH_LIMIT,
                    Some(value) => positive_integer(value).ok_or_else(|| {
                        "searchTools() limit must be a positive integer".to_owned()
                    })?,
                };
                let namespace = match member(options, "namespace") {
                    None => None,
                    Some(Value::String(name)) => Some(name.as_str()),
                    Some(_) => return Err("searchTools() namespace must be a string".to_owned()),
                };
                let discoverable = discoverable(&tools, &identifiers);
                let found = search_tools(&ranker, &discoverable, query, limit, namespace);
                Ok(Some(Value::Array(
                    found
                        .iter()
                        .map(|found| entry(&found.name, &samples, &identifiers))
                        .collect(),
                )))
            }
        })
    };
    let describe_tool = {
        let (tools, samples, identifiers) = (
            Arc::clone(&tools),
            Arc::clone(&samples),
            Arc::clone(&identifiers),
        );
        spread_global("describeTool", move |args, _context| {
            let (tools, samples, identifiers) = (
                Arc::clone(&tools),
                Arc::clone(&samples),
                Arc::clone(&identifiers),
            );
            async move {
                let Some(name) = args.first().and_then(Value::as_str) else {
                    return Err("describeTool() expects a tool name".to_owned());
                };
                let discoverable = discoverable(&tools, &identifiers);
                let Some(found) = find_tool(&discoverable, name) else {
                    return Ok(None);
                };
                let Some(sample) = samples.get(found.name) else {
                    return Ok(None);
                };
                // [CYRUP-DELTA] A tool that resolves to an MCP `CallToolResult` is declared with
                // types the description only defines for the tools it lists. Deferred tools are not
                // listed, so the types come with the declaration that needs them.
                let schema = tools
                    .iter()
                    .find(|tool| tool.name() == found.name)
                    .and_then(|tool| tool.output_schema());
                Ok(Some(Value::String(
                    if mcp_structured_content_schema(schema).is_some() {
                        format!(
                            "{sample}\n\nShared MCP Types:\n```ts\n{MCP_TYPESCRIPT_PREAMBLE}\n```"
                        )
                    } else {
                        sample.clone()
                    },
                )))
            }
        })
    };
    let describe_namespace_global = {
        let (tools, identifiers) = (Arc::clone(&tools), Arc::clone(&identifiers));
        spread_global("describeNamespace", move |args, _context| {
            let (tools, identifiers) = (Arc::clone(&tools), Arc::clone(&identifiers));
            async move {
                let Some(name) = args.first().and_then(Value::as_str) else {
                    return Err("describeNamespace() expects a namespace name".to_owned());
                };
                let discoverable = discoverable(&tools, &identifiers);
                describe_namespace(&discoverable, name)
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| error.to_string())
            }
        })
    };
    vec![search, describe_tool, describe_namespace_global]
}

/// Discovery's view of each tool; borrows the tool's own strings and schema.
fn discoverable<'a>(
    tools: &'a [Arc<dyn Tool>],
    identifiers: &'a IdentifierTable,
) -> Vec<DiscoverableTool<'a>> {
    tools
        .iter()
        .map(|tool| DiscoverableTool {
            name: tool.name(),
            description: tool.description(),
            parameters: tool.parameters(),
            namespace: tool.namespace(),
            identifier: identifiers.assigned(tool.name()),
        })
        .collect()
}

#[cfg(test)]
mod tests;
