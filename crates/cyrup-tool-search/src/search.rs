//! Which tools a query finds and what the model is told about them (`isSearchable`,
//! `searchAndLoad` and the result text, `extensions/tool-search/tool.ts:187-216,247-252` @v1.0.1).
//!
//! Everything here is a function over explicit inputs. The session is touched in one place,
//! [`search_and_load`].

use cyrup_codemode::js::{js_trim, split_lines};
use cyrup_codemode::rank::{Bm25Ranker, ToolRanker, create_tool_search_document};
use cyrup_core::{ToolExposure, ToolNamespace};
use cyrup_ext::HostServices;
use serde_json::Value;

use crate::input::ToolSearchInput;

/// A registered tool as `tool_search` sees it: one `getAllTools` row (`ToolInfo` with `exposure`
/// and `namespace`, `agent-session.ts:1462-1475`).
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub exposure: ToolExposure,
    pub namespace: Option<ToolNamespace>,
}

impl RegisteredTool {
    /// Reads one `HostServices::all_tools` row. A row without a name or with an exposure that is
    /// not one of the five is not a tool this search can offer, so it is `None`.
    #[must_use]
    pub fn from_row(row: &Value) -> Option<Self> {
        let name = row.get("name")?.as_str()?.to_owned();
        let exposure = row.get("exposure")?.as_str()?.parse().ok()?;
        Some(Self {
            name,
            description: row
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            parameters: row.get("parameters").cloned().unwrap_or(Value::Null),
            exposure,
            namespace: row
                .get("namespace")
                .and_then(|namespace| serde_json::from_value(namespace.clone()).ok()),
        })
    }
}

/// Whether `tool_search` can load a tool with this exposure: `codemode` or `deferred`, nothing else
/// (`isSearchable`, `tool.ts:192-194`). `direct`, `model-only` and `hidden` tools are never results.
#[must_use]
pub fn is_searchable(exposure: ToolExposure) -> bool {
    matches!(exposure, ToolExposure::Codemode | ToolExposure::Deferred)
}

/// A tool a search loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedTool {
    pub name: String,
    pub description: String,
}

/// What a search over `tools` decides: the matches, best first, and the active set that includes
/// them (`None` when nothing matched, in which case the active set is not touched).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchPlan {
    pub loaded: Vec<LoadedTool>,
    pub activate: Option<Vec<String>>,
}

/// Rank the searchable tools that are not active yet (`searchAndLoad`, `tool.ts:200-216`).
/// Candidates exclude the active set, not the declared one, so a tool an earlier search loaded is
/// not offered again.
#[must_use]
pub fn plan_search(
    tools: &[RegisteredTool],
    active: &[String],
    input: &ToolSearchInput,
) -> SearchPlan {
    let candidates: Vec<&RegisteredTool> = tools
        .iter()
        .filter(|tool| is_searchable(tool.exposure) && !active.contains(&tool.name))
        .collect();
    let documents: Vec<_> = candidates
        .iter()
        .map(|tool| {
            create_tool_search_document(
                &tool.name,
                &tool.description,
                &tool.parameters,
                tool.namespace.as_ref(),
            )
        })
        .collect();
    let matches = Bm25Ranker::default().rank(&input.query, &documents, input.limit.get());
    let loaded: Vec<LoadedTool> = matches
        .iter()
        .map(|found| LoadedTool {
            description: candidates
                .iter()
                .find(|tool| tool.name == found.name)
                .map(|tool| tool.description.clone())
                .unwrap_or_default(),
            name: found.name.clone(),
        })
        .collect();
    let activate = (!loaded.is_empty()).then(|| {
        active
            .iter()
            .cloned()
            .chain(loaded.iter().map(|tool| tool.name.clone()))
            .collect()
    });
    SearchPlan { loaded, activate }
}

/// Search the session's tools and activate the matches, so the next model call declares them.
/// A session without a live tool registry (`all_tools` or `active_tools` is `None`) finds nothing.
pub fn search_and_load(session: &dyn HostServices, input: &ToolSearchInput) -> Vec<LoadedTool> {
    let (Some(rows), Some(active)) = (session.all_tools(), session.active_tools()) else {
        return Vec::new();
    };
    let tools: Vec<RegisteredTool> = rows.iter().filter_map(RegisteredTool::from_row).collect();
    let plan = plan_search(&tools, &active, input);
    if let Some(names) = &plan.activate {
        session.set_active_tools(names);
    }
    plan.loaded
}

/// The text the model reads (`tool.ts:247-252`): `No matching tools found.`, or the count and one
/// `- <name>: <first line of the description>` per tool.
#[must_use]
pub fn result_text(loaded: &[LoadedTool]) -> String {
    if loaded.is_empty() {
        return "No matching tools found.".to_owned();
    }
    let lines: Vec<String> = loaded
        .iter()
        .map(|tool| {
            let first_line = split_lines(js_trim(&tool.description))
                .first()
                .copied()
                .unwrap_or_default();
            format!("- {}: {first_line}", tool.name)
        })
        .collect();
    format!(
        "Loaded {} tool{}. They are available from your next call:\n{}",
        loaded.len(),
        if loaded.len() == 1 { "" } else { "s" },
        lines.join("\n")
    )
}
