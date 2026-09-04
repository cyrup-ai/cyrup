//! Parse a caller-supplied JSON `config` object into a typed [`AgentFields`] delta or a parsed
//! chain-step list. Split out of `discovery/management.rs`'s own "config-object / package-name
//! parsing", "applyAgentConfig", and "parseStepList" sections. Every entry point here is called
//! only from `handlers.rs`'s `handle_create`/`handle_update` today, but the concern ("parse a JSON
//! blob into a typed shape, with zero I/O") is genuinely separable from CRUD orchestration — see
//! this task's own rationale for keeping it a distinct file despite the 1:1 caller relationship.
//! Exact pi error strings are reproduced verbatim (the tool test-suite pins several, e.g.
//! `config.completionGuard must be a boolean`).

use std::collections::HashSet;
use std::path::PathBuf;

use super::super::types::{
    ChainListBinding, ChainOutputBinding, ChainStepConfig, OutputSpec, SystemPromptMode,
};
use super::agent_crud::AgentFields;
use crate::fork_context::ContextMode;
use cyrup_core::ModelId;

/// pi `configObject` (`agent-management.ts:61-73`): a JSON-string config is `JSON.parse`d (parse
/// failure -> `config must be valid JSON: …`); a non-object (or array) yields `Ok(None)`; an object
/// yields `Ok(Some(map))`.
pub(crate) fn config_object(
    config: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
    let Some(value) = config else {
        return Ok(None);
    };
    let owned;
    let val: &serde_json::Value = if let serde_json::Value::String(s) = value {
        owned = serde_json::from_str::<serde_json::Value>(s)
            .map_err(|e| format!("config must be valid JSON: {e}"))?;
        &owned
    } else {
        value
    };
    match val {
        serde_json::Value::Object(map) => Ok(Some(map.clone())),
        _ => Ok(None),
    }
}

/// pi `parsePackageName(value, "config.package")` (`identity.ts:11-17`): absent/`false`/`""` ->
/// `Ok(None)`; a non-string -> `Err(must be a string or false)`; a string that fails to normalize to
/// a valid identifier -> `Err(is invalid after sanitization)`. Note this is a HARD error at the
/// management layer, unlike the low-level `create_agent`/`update_agent` silent-skip (which this
/// handler never reaches, since it pre-validates here).
pub(crate) fn parse_package_config(
    value: Option<&serde_json::Value>,
) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(serde_json::Value::Bool(false)) => Ok(None),
        Some(serde_json::Value::String(s)) if s.is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => {
            match super::helpers::normalize_package_identifier(Some(s)) {
                Some(pkg) => Ok(Some(pkg)),
                None => Err("config.package is invalid after sanitization.".to_string()),
            }
        }
        Some(_) => Err("config.package must be a string or false when provided.".to_string()),
    }
}

/// pi `parseCsv` (`agent-management.ts:57-59`): split on `,`, trim, drop empties, dedup preserving
/// first occurrence.
fn parse_csv(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for part in value.split(',') {
        let trimmed = part.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// pi `parseTools` (`agent-management.ts:395-408`): split CSV; `mcp:`-prefixed entries become MCP
/// direct-tool refs (prefix stripped, verbatim otherwise), the rest builtin refs. cyrup unifies both
/// into one `Vec<ToolRef>` (MCP entries preserved as [`crate::discovery::types::ToolRef::Mcp`]
/// without the `mcp:` prefix, matching `frontmatter_write::tool_ref_to_frontmatter_entry`'s inverse).
fn parse_tools(value: &str) -> Vec<super::super::types::ToolRef> {
    use super::super::types::ToolRef;
    let mut out = Vec::new();
    for item in parse_csv(value) {
        if let Some(rest) = item.strip_prefix("mcp:") {
            let direct = rest.trim();
            if !direct.is_empty() {
                out.push(ToolRef::Mcp(direct.to_string()));
            }
        } else {
            out.push(ToolRef::Builtin(item));
        }
    }
    out
}

pub(crate) fn apply_agent_config(
    fields: &mut AgentFields,
    cfg: &serde_json::Map<String, serde_json::Value>,
    target_name: &str,
) -> Result<(), String> {
    use serde_json::Value;

    // pi `agent-management.ts:411-421` @ v0.43.0 — the FIRST branch of `applyAgentConfig`:
    //
    //   if (cfg.aliases === false || cfg.aliases === "") target.aliases = undefined;
    //   else if (typeof cfg.aliases === "string") { parseCsv(...).filter(a => a !== target.name) }
    //   else if (Array.isArray(...) && every string) { [...new Set(map(trim).filter(Boolean))].filter(a => a !== target.name) }
    //   else return "config.aliases must be a comma-separated string, string array, or false when provided.";
    //
    // `target_name` is the target's name AS IT STANDS WHEN THE CONFIG IS APPLIED, which on a rename
    // is still the OLD runtime name: upstream calls `applyAgentConfig` at `:1006`, six lines before
    // `updated.name = buildRuntimeName(newLocalName, newPackageName)` at `:1012`.
    if let Some(v) = cfg.get("aliases") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.aliases = Some(Vec::new());
        } else if let Some(raw) = v.as_str() {
            fields.aliases = Some(
                parse_csv(raw)
                    .into_iter()
                    .filter(|a| a != target_name)
                    .collect(),
            );
        } else if let Some(arr) = v.as_array()
            && arr.iter().all(Value::is_string)
        {
            let mut seen = HashSet::new();
            let mut aliases = Vec::new();
            for item in arr {
                let trimmed = item.as_str().unwrap_or_default().trim();
                if trimmed.is_empty() || trimmed == target_name {
                    continue;
                }
                if seen.insert(trimmed.to_string()) {
                    aliases.push(trimmed.to_string());
                }
            }
            fields.aliases = Some(aliases);
        } else {
            return Err(
                "config.aliases must be a comma-separated string, string array, or false when provided."
                    .to_string(),
            );
        }
    }
    if let Some(v) = cfg.get("systemPrompt") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.system_prompt_body = Some(String::new());
        } else if let Some(s) = v.as_str() {
            fields.system_prompt_body = Some(s.to_string());
        } else {
            return Err("config.systemPrompt must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("model") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.model = Some(None);
        } else if let Some(s) = v.as_str() {
            let trimmed = s.trim();
            fields.model = Some(if trimmed.is_empty() {
                None
            } else {
                Some(ModelId::from(trimmed))
            });
        } else {
            return Err("config.model must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("fallbackModels") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.fallback_models = Some(Vec::new());
        } else if let Some(s) = v.as_str() {
            fields.fallback_models = Some(
                parse_csv(s)
                    .into_iter()
                    .map(|m| ModelId::from(m.as_str()))
                    .collect(),
            );
        } else if let Some(arr) = v.as_array() {
            let mut seen = HashSet::new();
            let mut models = Vec::new();
            for item in arr {
                if let Some(s) = item.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                        models.push(ModelId::from(trimmed));
                    }
                }
            }
            fields.fallback_models = Some(models);
        } else {
            return Err("config.fallbackModels must be a comma-separated string, string array, or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("tools") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.tools = Some(None);
        } else if let Some(s) = v.as_str() {
            let parsed = parse_tools(s);
            fields.tools = Some(if parsed.is_empty() {
                None
            } else {
                Some(parsed)
            });
        } else {
            return Err(
                "config.tools must be a comma-separated string or false when provided.".to_string(),
            );
        }
    }
    if let Some(v) = cfg.get("skills") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.skills = Some(Vec::new());
        } else if let Some(s) = v.as_str() {
            fields.skills = Some(parse_csv(s));
        } else {
            return Err(
                "config.skills must be a comma-separated string or false when provided."
                    .to_string(),
            );
        }
    }
    if let Some(v) = cfg.get("extensions") {
        if v == &Value::Bool(false) {
            fields.extensions = Some(None);
        } else if v.as_str() == Some("") {
            fields.extensions = Some(Some(Vec::new()));
        } else if let Some(s) = v.as_str() {
            fields.extensions = Some(Some(parse_csv(s)));
        } else {
            return Err("config.extensions must be a comma-separated string, empty string, or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("subagentOnlyExtensions") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.subagent_only_extensions = Some(Vec::new());
        } else if let Some(s) = v.as_str() {
            fields.subagent_only_extensions = Some(parse_csv(s));
        } else {
            return Err("config.subagentOnlyExtensions must be a comma-separated string, empty string, or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("thinking") {
        // pi `applyAgentConfig` (`agent-management.ts:507-514`): `false`/`""` clears; any other
        // string sets the OPEN value (trimmed; a whitespace-only value clears). No closed-enum
        // coercion — an arbitrary/`off` value is preserved verbatim.
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.thinking = Some(None);
        } else if let Some(s) = v.as_str() {
            let trimmed = s.trim();
            fields.thinking = Some(if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            });
        } else {
            return Err("config.thinking must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("systemPromptMode") {
        match v.as_str() {
            Some("append") => fields.system_prompt_mode = Some(SystemPromptMode::Append),
            Some("replace") => fields.system_prompt_mode = Some(SystemPromptMode::Replace),
            _ => {
                return Err(
                    "config.systemPromptMode must be 'append' or 'replace' when provided."
                        .to_string(),
                );
            }
        }
    }
    if let Some(v) = cfg.get("inheritProjectContext") {
        match v.as_bool() {
            Some(b) => fields.inherit_project_context = Some(b),
            None => {
                return Err(
                    "config.inheritProjectContext must be a boolean when provided.".to_string(),
                );
            }
        }
    }
    if let Some(v) = cfg.get("inheritSkills") {
        match v.as_bool() {
            Some(b) => fields.inherit_skills = Some(b),
            None => return Err("config.inheritSkills must be a boolean when provided.".to_string()),
        }
    }
    if let Some(v) = cfg.get("defaultContext") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.default_context = Some(None);
        } else if v.as_str() == Some("fresh") {
            fields.default_context = Some(Some(ContextMode::Fresh));
        } else if v.as_str() == Some("fork") {
            fields.default_context = Some(Some(ContextMode::Fork));
        } else {
            return Err(
                "config.defaultContext must be 'fresh', 'fork', or false when provided."
                    .to_string(),
            );
        }
    }
    if let Some(v) = cfg.get("output") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.output = Some(None);
        } else if let Some(s) = v.as_str() {
            fields.output = Some(Some(OutputSpec {
                path: Some(PathBuf::from(s)),
                mode: None,
            }));
        } else {
            return Err("config.output must be a string or false when provided.".to_string());
        }
    }
    if let Some(v) = cfg.get("reads") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.default_reads = Some(None);
        } else if let Some(s) = v.as_str() {
            let reads: Vec<PathBuf> = parse_csv(s).into_iter().map(PathBuf::from).collect();
            fields.default_reads = Some(if reads.is_empty() { None } else { Some(reads) });
        } else {
            return Err(
                "config.reads must be a comma-separated string or false when provided.".to_string(),
            );
        }
    }
    if let Some(v) = cfg.get("progress") {
        match v.as_bool() {
            Some(b) => fields.default_progress = Some(Some(b)),
            None => return Err("config.progress must be a boolean when provided.".to_string()),
        }
    }
    if let Some(v) = cfg.get("maxSubagentDepth") {
        if v == &Value::Bool(false) || v.as_str() == Some("") {
            fields.max_subagent_depth = Some(None);
        } else if let Some(n) = v.as_u64() {
            match u32::try_from(n) {
                Ok(depth) => fields.max_subagent_depth = Some(Some(depth)),
                Err(_) => {
                    return Err(
                        "config.maxSubagentDepth must be an integer >= 0 or false when provided."
                            .to_string(),
                    );
                }
            }
        } else {
            return Err(
                "config.maxSubagentDepth must be an integer >= 0 or false when provided."
                    .to_string(),
            );
        }
    }
    if let Some(v) = cfg.get("completionGuard") {
        match v.as_bool() {
            Some(b) => fields.completion_guard = Some(Some(b)),
            None => {
                return Err("config.completionGuard must be a boolean when provided.".to_string());
            }
        }
    }
    Ok(())
}

pub(crate) fn parse_step_list(
    raw: Option<&serde_json::Value>,
) -> Result<Vec<ChainStepConfig>, String> {
    use serde_json::Value;
    let Some(Value::Array(arr)) = raw else {
        return Err("config.steps must be an array.".to_string());
    };
    if arr.is_empty() {
        return Err("config.steps must include at least one step.".to_string());
    }
    let mut steps = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            return Err(format!("config.steps[{i}] must be an object."));
        };
        let agent = match obj.get("agent").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return Err(format!(
                    "config.steps[{i}].agent must be a non-empty string."
                ));
            }
        };
        let mut step = ChainStepConfig {
            agent: Some(agent),
            task: Some(
                obj.get("task")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
            ..ChainStepConfig::default()
        };
        if let Some(v) = obj.get("phase") {
            match v.as_str() {
                Some(s) => step.phase = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].phase must be a string.")),
            }
        }
        if let Some(v) = obj.get("label") {
            match v.as_str() {
                Some(s) => step.label = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].label must be a string.")),
            }
        }
        if let Some(v) = obj.get("as") {
            match v.as_str() {
                Some(s) => step.as_ = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].as must be a string.")),
            }
        }
        if let Some(v) = obj.get("outputSchema") {
            match v.as_str() {
                Some(s) => step.output_schema = Some(Value::String(s.to_string())),
                None => {
                    return Err(format!(
                        "config.steps[{i}].outputSchema must be a schema file path string for saved chains."
                    ));
                }
            }
        }
        if let Some(v) = obj.get("output") {
            if v == &Value::Bool(false) {
                step.output = Some(ChainOutputBinding::Toggle(false));
            } else if let Some(s) = v.as_str() {
                step.output = Some(ChainOutputBinding::Name(s.to_string()));
            } else {
                return Err(format!(
                    "config.steps[{i}].output must be a string or false."
                ));
            }
        }
        if let Some(v) = obj.get("outputMode") {
            match v.as_str() {
                Some("inline") => step.output_mode = Some("inline".to_string()),
                Some("file-only") => step.output_mode = Some("file-only".to_string()),
                _ => {
                    return Err(format!(
                        "config.steps[{i}].outputMode must be 'inline' or 'file-only'."
                    ));
                }
            }
        }
        if let Some(v) = obj.get("reads") {
            if v == &Value::Bool(false) {
                step.reads = Some(ChainListBinding::Toggle(false));
            } else if let Some(a) = v.as_array() {
                let list = a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                step.reads = Some(ChainListBinding::List(list));
            } else {
                return Err(format!(
                    "config.steps[{i}].reads must be an array or false."
                ));
            }
        }
        if let Some(v) = obj.get("model") {
            match v.as_str() {
                Some(s) => step.model = Some(s.to_string()),
                None => return Err(format!("config.steps[{i}].model must be a string.")),
            }
        }
        if let Some(v) = obj.get("skills") {
            if v == &Value::Bool(false) {
                step.skills = Some(ChainListBinding::Toggle(false));
            } else if let Some(a) = v.as_array() {
                let list = a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                step.skills = Some(ChainListBinding::List(list));
            } else {
                return Err(format!(
                    "config.steps[{i}].skills must be an array or false."
                ));
            }
        }
        if let Some(v) = obj.get("progress") {
            match v.as_bool() {
                Some(b) => step.progress = Some(b),
                None => return Err(format!("config.steps[{i}].progress must be a boolean.")),
            }
        }
        steps.push(step);
    }
    Ok(steps)
}
