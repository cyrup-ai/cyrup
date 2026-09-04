//! Renderers (pi `formatAgentDetail`/`formatChainDetail`/`formatChainStepDetail`,
//! `agent-management.ts:463-537`, plus `formatModelSource`, `:790-800`). Split out of
//! `discovery/management.rs`'s own "Renderers" section. Pure `&T -> String` formatting, zero
//! mutation, zero discovery I/O — called only from `handlers.rs`.

use super::super::types::{
    AgentDefinition, AgentModelSourceInfo, AgentSource, ChainDefinition, ChainListBinding,
    ChainOutputBinding, ChainStepConfig, SystemPromptMode, ToolRef,
};
use super::helpers::{context_str, override_scope_str, source_str};

/// pi `formatAgentDetail` (`agent-management.ts:665-701`).
pub(crate) fn format_agent_detail(a: &AgentDefinition) -> String {
    let mut tools_out: Vec<String> = Vec::new();
    if let Some(tools) = &a.tools {
        for tool in tools {
            match tool {
                ToolRef::Builtin(n) | ToolRef::ExtensionPath(n) => tools_out.push(n.clone()),
                ToolRef::Mcp(_) => {}
            }
        }
        for tool in tools {
            if let ToolRef::Mcp(n) = tool {
                tools_out.push(format!("mcp:{n}"));
            }
        }
    }

    let mut lines: Vec<String> = vec![
        format!("Agent: {} ({})", a.name, source_str(a.source)),
        format!("Path: {}", a.file_path.display()),
        format!("Description: {}", a.description),
    ];
    if a.package_name.is_some() {
        lines.push(format!("Local name: {}", a.local_name));
        if let Some(pkg) = &a.package_name {
            lines.push(format!("Package: {pkg}"));
        }
    }
    // pi `agent-management.ts:672` @ v0.43.0: `if (agent.aliases?.length) lines.push(...)` — between
    // the package block and the model line.
    if !a.aliases.is_empty() {
        lines.push(format!("Aliases: {}", a.aliases.join(", ")));
    }
    if let Some(model) = &a.model {
        lines.push(format!("Model: {model}"));
    }
    if !a.fallback_models.is_empty() {
        lines.push(format!(
            "Fallback models: {}",
            a.fallback_models
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !tools_out.is_empty() {
        lines.push(format!("Tools: {}", tools_out.join(", ")));
    }
    if !a.skills.is_empty() {
        lines.push(format!("Skills: {}", a.skills.join(", ")));
    }
    lines.push(format!(
        "System prompt mode: {}",
        match a.system_prompt_mode {
            SystemPromptMode::Append => "append",
            SystemPromptMode::Replace => "replace",
        }
    ));
    lines.push(format!(
        "Inherit project context: {}",
        if a.inherit_project_context { "true" } else { "false" }
    ));
    lines.push(format!(
        "Inherit skills: {}",
        if a.inherit_skills { "true" } else { "false" }
    ));
    if let Some(ctx) = a.default_context {
        lines.push(format!("Default context: {}", context_str(ctx)));
    }
    // SUBA-082 (`agent-management.ts:901-902` @v0.64.0): `Acceptance:` renders the launch
    // default as compact JSON for an object and `String(value)` for a scalar; `Acceptance role:`
    // only when a role is declared (upstream's truthiness test).
    if let Some(acceptance) = &a.default_acceptance {
        let rendered = match acceptance {
            serde_json::Value::String(text) => text.clone(),
            value @ serde_json::Value::Object(_) => {
                serde_json::to_string(value).unwrap_or_default()
            }
            other => other.to_string(),
        };
        lines.push(format!("Acceptance: {rendered}"));
    }
    if let Some(role) = a.acceptance_role {
        lines.push(format!("Acceptance role: {}", role.as_str()));
    }
    if a.source == AgentSource::Builtin {
        lines.push(format!(
            "Disabled: {}",
            if a.disabled.unwrap_or(false) { "true" } else { "false" }
        ));
    }
    if let Some(exts) = &a.extensions {
        lines.push(format!(
            "Extensions: {}",
            if exts.is_empty() { "(none)".to_string() } else { exts.join(", ") }
        ));
    }
    // pi renders `Subagent-only extensions` whenever the field is defined (even empty -> "(none)").
    // cyrup flattens it to a `Vec` with no defined/empty distinction, and its own serializer only
    // writes the key when non-empty, so a round-tripped file's non-empty <=> pi's "defined": render
    // only when non-empty (documented minor divergence limited to the defined-but-empty edge).
    if !a.subagent_only_extensions.is_empty() {
        lines.push(format!(
            "Subagent-only extensions: {}",
            a.subagent_only_extensions.join(", ")
        ));
    }
    if let Some(thinking) = &a.thinking {
        lines.push(format!("Thinking: {thinking}"));
    }
    if let Some(output) = &a.output
        && let Some(path) = &output.path
    {
        lines.push(format!("Output: {}", path.display()));
    }
    if let Some(reads) = &a.default_reads
        && !reads.is_empty()
    {
        lines.push(format!(
            "Reads: {}",
            reads
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if a.default_progress == Some(true) {
        lines.push("Progress: true".to_string());
    }
    if let Some(depth) = a.max_subagent_depth {
        lines.push(format!("Max subagent depth: {depth}"));
    }
    if a.completion_guard == Some(false) {
        lines.push("Completion guard: false".to_string());
    }
    if !a.system_prompt_body.trim().is_empty() {
        lines.push(String::new());
        lines.push("System Prompt:".to_string());
        lines.push(a.system_prompt_body.clone());
    }
    lines.join("\n")
}

/// pi `formatChainStepDetail` (`agent-management.ts:703-738`).
fn format_chain_step_detail(step: &ChainStepConfig, index: usize) -> Vec<String> {
    let n = index + 1;
    let mut lines: Vec<String> = Vec::new();
    if step.expand.is_some() || step.collect.is_some() {
        let collect_as = step
            .collect
            .as_ref()
            .and_then(|v| v.get("as"))
            .and_then(|v| v.as_str());
        lines.push(match collect_as {
            Some(a) => format!("{n}. Dynamic fanout -> {a}"),
            None => format!("{n}. Dynamic fanout"),
        });
        if let Some(expand) = &step.expand {
            if let Some(from) = expand.get("from") {
                let out = from.get("output").and_then(|v| v.as_str()).unwrap_or("?");
                let path = from.get("path").and_then(|v| v.as_str()).unwrap_or("");
                lines.push(format!("   Expand: {out}{path}"));
            }
            if let Some(item) = expand.get("item").and_then(|v| v.as_str()) {
                lines.push(format!("   Item variable: {item}"));
            }
            if let Some(key) = expand.get("key").and_then(|v| v.as_str()) {
                lines.push(format!("   Key: {key}"));
            }
            if let Some(max_items) = expand.get("maxItems").and_then(|v| v.as_i64()) {
                lines.push(format!("   Max items: {max_items}"));
            }
            if let Some(on_empty) = expand.get("onEmpty").and_then(|v| v.as_str()) {
                lines.push(format!("   On empty: {on_empty}"));
            }
        }
        if let Some(parallel) = &step.parallel {
            if let Some(agent) = parallel.get("agent").and_then(|v| v.as_str()) {
                lines.push(format!("   Agent: {agent}"));
            }
            if let Some(label) = parallel.get("label").and_then(|v| v.as_str()) {
                lines.push(format!("   Label: {label}"));
            }
            if let Some(task) = parallel.get("task").and_then(|v| v.as_str())
                && !task.trim().is_empty()
            {
                lines.push(format!("   Task: {task}"));
            }
            if parallel.get("outputSchema").is_some() {
                lines.push("   Structured output: true".to_string());
            }
        }
        if let Some(collect) = &step.collect
            && collect.get("outputSchema").is_some()
        {
            lines.push("   Collect schema: true".to_string());
        }
        if let Some(concurrency) = step.concurrency {
            lines.push(format!("   Concurrency: {concurrency}"));
        }
        if let Some(fail_fast) = step.fail_fast {
            lines.push(format!("   Fail fast: {}", if fail_fast { "true" } else { "false" }));
        }
        return lines;
    }

    lines.push(format!("{n}. {}", step.agent.as_deref().unwrap_or("")));
    if let Some(task) = &step.task
        && !task.trim().is_empty()
    {
        lines.push(format!("   Task: {task}"));
    }
    match &step.output {
        Some(ChainOutputBinding::Toggle(false)) => lines.push("   Output: false".to_string()),
        Some(ChainOutputBinding::Name(s)) => lines.push(format!("   Output: {s}")),
        _ => {}
    }
    if let Some(mode) = &step.output_mode {
        lines.push(format!("   Output mode: {mode}"));
    }
    match &step.reads {
        Some(ChainListBinding::Toggle(false)) => lines.push("   Reads: false".to_string()),
        Some(ChainListBinding::List(v)) if !v.is_empty() => {
            lines.push(format!("   Reads: {}", v.join(", ")))
        }
        _ => {}
    }
    if let Some(model) = &step.model {
        lines.push(format!("   Model: {model}"));
    }
    match &step.skills {
        Some(ChainListBinding::Toggle(false)) => lines.push("   Skills: false".to_string()),
        Some(ChainListBinding::List(v)) if !v.is_empty() => {
            lines.push(format!("   Skills: {}", v.join(", ")))
        }
        _ => {}
    }
    if let Some(progress) = step.progress {
        lines.push(format!("   Progress: {}", if progress { "true" } else { "false" }));
    }
    lines
}

/// pi `formatChainDetail` (`agent-management.ts:740-751`).
pub(crate) fn format_chain_detail(c: &ChainDefinition) -> String {
    let mut lines: Vec<String> = vec![
        format!("Chain: {} ({})", c.name, source_str(c.source)),
        format!("Path: {}", c.file_path.display()),
        format!("Description: {}", c.description),
    ];
    if c.package_name.is_some() {
        lines.push(format!("Local name: {}", c.local_name));
        if let Some(pkg) = &c.package_name {
            lines.push(format!("Package: {pkg}"));
        }
    }
    lines.push(String::new());
    lines.push("Steps:".to_string());
    for (i, step) in c.steps.iter().enumerate() {
        lines.extend(format_chain_step_detail(step, i));
    }
    lines.join("\n")
}

/// Port of pi `formatModelSource` (`agent-management.ts:790-800`). The live parent session model is
/// now threaded in as `current_session_model` (from
/// [`crate::discovery::management::ManagementRequest::current_session_model`] /
/// [`cyrup_ext::host::HostServices::current_model`]), so when the persona declares no `model` but a
/// live session model is bound this reports "inherits current session model" (pi's own wording,
/// agent-management.ts:798); otherwise it classifies from discovery-time provenance (`override_info`
/// / `model_source`) and the agent's own resolved `model`.
pub(crate) fn format_model_source(agent: &AgentDefinition, current_session_model: Option<&str>) -> String {
    if let Some(info) = &agent.override_info
        && agent.model != info.base_snapshot.model
    {
        return format!("{} override", override_scope_str(info.scope));
    }
    if matches!(agent.model_source, Some(AgentModelSourceInfo::SettingsDefault)) {
        return "settings defaultModel".to_string();
    }
    if agent.model.is_some() {
        return "builtin agent config".to_string();
    }
    if current_session_model.is_some() {
        return "inherits current session model".to_string();
    }
    "inherit requested, but no current session model is available".to_string()
}
