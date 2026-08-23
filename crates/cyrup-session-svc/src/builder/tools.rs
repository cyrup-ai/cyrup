//! The tool-selection + tool-prompt-contribution helpers (Pi `defaultActiveToolNames` /
//! `definition.promptSnippet`) — pure functions over a tool set, independent of the builder.

use std::sync::Arc;

use cyrup_session::prompt::ToolPromptContribution;

use super::{NoTools, SessionConfig};

/// Pi's default active built-in tool names (sdk.ts:244).
const DEFAULT_BUILTIN_TOOLS: [&str; 4] = ["read", "bash", "edit", "write"];

/// Every tool `ToolRegistry::with_builtins` installs (`cyrup-tools/src/registry.rs:45-67`).
///
/// Needed to tell "a built-in pi does not activate by default" (`grep`/`find`/`ls`) apart from "a
/// non-built-in tool" (an extension- or embedder-supplied one), which must stay active: pi's
/// `defaultActiveToolNames` gates only its own built-ins and never suppresses a tool the host
/// registered.
const ALL_BUILTIN_TOOLS: [&str; 7] =
    ["read", "write", "edit", "bash", "grep", "find", "ls"];

/// Apply the `tools`/`noTools`/`excludeTools` selection over the Availability-visible tool set
/// (Pi sdk.ts:244-251). When none of the three is set the visible set passes through unchanged.
pub(super) fn select_active_tools(
    visible: &[Arc<dyn cyrup_core::Tool>],
    cfg: &SessionConfig,
) -> Vec<Arc<dyn cyrup_core::Tool>> {
    let exclude: std::collections::HashSet<&str> =
        cfg.exclude_tools.iter().map(String::as_str).collect();
    let keep = |name: &str| -> bool {
        match (&cfg.tools, cfg.no_tools) {
            // Explicit allowlist wins (Pi `options.tools`).
            (Some(allow), _) => allow.iter().any(|a| a == name),
            (None, Some(NoTools::All)) => false,
            (None, Some(NoTools::Builtin)) => !DEFAULT_BUILTIN_TOOLS.contains(&name),
            // pi `sdk.ts:244-250`: with no `tools`/`noTools` the active set is
            // `defaultActiveToolNames` — read/bash/edit/write — NOT every visible tool. Confirmed
            // at the same tag in `agent-session.ts:2592-2594`, and `_refreshToolRegistry`
            // (`:2524-2546`) only ever WIDENS it.
            //
            // This arm returned `true`, so every cyrup session advertised three tools pi does not
            // (`grep`, `find`, `ls`). That changed the tool array in every provider request AND the
            // system prompt (their `prompt_snippet`/`prompt_guidelines` are injected via
            // `tool_contribution`), so the model routed searches to `grep`/`find` instead of `bash`
            // — different transcripts, different token counts, different tool-call sequences than
            // pi for identical inputs — and it silently widened the surface a permission policy has
            // to cover.
            //
            // `registry.visible(...)` is deliberately NOT narrowed: grep/find/ls remain
            // ENABLE-able at runtime via `set_active_tools_by_name`, exactly as pi's
            // `_refreshToolRegistry` can widen its own active set. This changes the DEFAULT, not
            // what is reachable.
            (None, None) => {
                DEFAULT_BUILTIN_TOOLS.contains(&name) || !ALL_BUILTIN_TOOLS.contains(&name)
            }
        }
    };
    visible
        .iter()
        .filter(|t| keep(t.name()) && !exclude.contains(t.name()))
        .cloned()
        .collect()
}

/// Project a tool's OWN prompt contribution off its `Tool` vtable (arch-06 R-06-012/013). Pi reads
/// `definition.promptSnippet`/`definition.promptGuidelines` straight off the tool definition
/// (agent-session.ts:2490-2504) — never a name-keyed table — so a tool that declares no snippet is
/// simply absent from the "Available tools" section (system-prompt.ts:79-80: `tools.filter(name =>
/// !!toolSnippets?.[name])`), and one that declares guidelines contributes them as bullets.
pub(crate) fn tool_contribution(tool: &Arc<dyn cyrup_core::Tool>) -> ToolPromptContribution {
    ToolPromptContribution {
        tool: Arc::<str>::from(tool.name()),
        snippet: tool.prompt_snippet().map(Arc::<str>::from),
        guidelines: tool.prompt_guidelines().iter().copied().map(Arc::<str>::from).collect(),
    }
}
