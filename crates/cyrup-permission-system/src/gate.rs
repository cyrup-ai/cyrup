//! Gate helpers (port of pi `index.ts` gate internals): the config+session+permanent approval
//! overlay (`applyPatternApprovalState`), the approval subject (`getPatternApprovalSubject`), the
//! config evaluation rule (`createConfigEvaluationRule`), and the model-facing reason formatters
//! (`formatDenyReason` / `formatUserDeniedReason` / the ask-unavailable reason). The orchestration
//! that CALLS these (the async gate on every `tool_call`) lives in `extension.rs`.

use serde_json::Value;

use crate::common::{self, get_non_empty_string, to_record};
use crate::evaluate;
use crate::types::{CheckSource, PatternRule, PermissionCheckResult, PermissionState};

const PATH_BEARING_TOOLS: [&str; 6] = ["read", "write", "edit", "find", "grep", "ls"];

/// pi `FILESYSTEM_TOOL_NAME_SUFFIXES` (`index.ts:141`): the filesystem-ish suffixes a tool name is
/// matched against by [`is_likely_filesystem_tool_name`].
const FILESYSTEM_TOOL_NAME_SUFFIXES: [&str; 8] =
    ["read", "write", "edit", "find", "grep", "search", "list", "ls"];

/// pi `isLikelyFilesystemToolName` (`index.ts:206-217`): heuristic recognition of a filesystem-like
/// tool name. Lowercases + trims, splits on any run of non-alphanumeric characters (pi's
/// `[^a-z0-9]+`), and reports a match when the normalized name ENDS WITH one of
/// [`FILESYSTEM_TOOL_NAME_SUFFIXES`] OR one of the split parts IS exactly a suffix — so
/// `read_file`, `grep_files`, `fs_search`, `list-dir`, and any `*_read`/`*_write` all match.
#[must_use]
pub fn is_likely_filesystem_tool_name(tool_name: &str) -> bool {
    let normalized = tool_name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let name_parts: Vec<&str> = normalized
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    FILESYSTEM_TOOL_NAME_SUFFIXES
        .iter()
        .any(|&suffix| normalized.ends_with(suffix) || name_parts.contains(&suffix))
}

/// pi `getPathBearingToolPath` (`index.ts:218-233`): the `path`/`file_path` of a filesystem tool.
/// Recognizes the exact path-bearing name set ([`PATH_BEARING_TOOLS`]), a structured-edit payload
/// (an `edits` key, pi `hasStructuredEditPayload`), OR the broader [`is_likely_filesystem_tool_name`]
/// heuristic — so a non-builtin filesystem tool (`read_file`, `grep_files`, `fs_search`, a
/// `*_read`/`*_write`/`*_search`/`*_list`) is recognized as path-bearing. This is an ENFORCEMENT
/// input, not merely cosmetic: `extension.rs`'s external-directory guard skips the whole ask/deny
/// check when this returns `None`, so a filesystem-like tool that went unrecognized would reach a
/// path OUTSIDE the working directory ungated.
#[must_use]
pub fn get_path_bearing_tool_path(tool_name: &str, input: &Value) -> Option<String> {
    let record = to_record(input);
    let path = get_non_empty_string(record.get("path"))
        .or_else(|| get_non_empty_string(record.get("file_path")))?;
    if PATH_BEARING_TOOLS.contains(&tool_name)
        || record.contains_key("edits")
        || is_likely_filesystem_tool_name(tool_name)
    {
        Some(path)
    } else {
        None
    }
}

/// pi `index.ts:2305-2309`: when a path-bearing tool input carries a `path`/`file_path` but no `cwd`
/// of its own, inject the session `cwd` so downstream path-resource resolution
/// ([`crate::manager`]'s `path_resource_from_input`) anchors to the SESSION cwd instead of falling
/// back to the process cwd (`std::env::current_dir`). A non-path input, an input that already carries
/// `cwd`, or an empty session cwd is passed through unchanged.
#[must_use]
pub fn inject_cwd(input: &Value, cwd: &str) -> Value {
    if cwd.is_empty() {
        return input.clone();
    }
    let record = to_record(input);
    let has_path = get_non_empty_string(record.get("path")).is_some()
        || get_non_empty_string(record.get("file_path")).is_some();
    let has_cwd = get_non_empty_string(record.get("cwd")).is_some();
    if has_path && !has_cwd {
        let mut m = record.clone();
        m.insert("cwd".to_string(), Value::String(cwd.to_string()));
        Value::Object(m)
    } else {
        input.clone()
    }
}

/// pi `getPatternApprovalSubject` (`index.ts:817-839`): the subject the approval stores match on.
#[must_use]
pub fn get_pattern_approval_subject(result: &PermissionCheckResult, input: &Value) -> String {
    if result.tool_name == "bash"
        && let Some(cmd) = &result.command
        && !cmd.is_empty()
    {
        return cmd.clone();
    }
    if (result.source == CheckSource::Mcp || result.tool_name == "mcp")
        && let Some(target) = &result.target
        && !target.is_empty()
    {
        return target.clone();
    }
    if let Some(path) = get_path_bearing_tool_path(&result.tool_name, input) {
        let cwd = get_non_empty_string(to_record(input).get("cwd"))
            .unwrap_or_else(|| std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default());
        let resource = common::normalize_path_resource_for_permission(&path, &cwd);
        return if resource.is_empty() { path } else { resource };
    }
    if let Some(target) = &result.target {
        let prefix = format!("{}:", result.tool_name);
        return match target.strip_prefix(&prefix) {
            Some(rest) => rest.to_string(),
            None => target.clone(),
        };
    }
    result.command.clone().unwrap_or_else(|| result.tool_name.clone())
}

/// pi `createConfigEvaluationRule` (`index.ts:841-848`): reuse the matched pattern only for
/// `bash|mcp|skill|special` sources, else `"*"`.
#[must_use]
pub fn create_config_evaluation_rule(result: &PermissionCheckResult) -> PatternRule {
    let can_reuse = matches!(
        result.source,
        CheckSource::Bash | CheckSource::Mcp | CheckSource::Skill | CheckSource::Special
    );
    let pattern = match (&result.matched_pattern, can_reuse) {
        (Some(p), true) => p.clone(),
        _ => "*".to_string(),
    };
    PatternRule { tool: result.tool_name.clone(), pattern, action: result.state }
}

/// pi `applyPatternApprovalState` (`index.ts:850-874`): fold the config result with the session +
/// permanent stores (ruleset order `[config, session, permanent]`, last-match-wins → permanent beats
/// session beats config). A `deny` short-circuits (never relaxed).
#[must_use]
pub fn apply_pattern_approval_state(
    result: PermissionCheckResult,
    input: &Value,
    session_rules: &[PatternRule],
    permanent_rules: &[PatternRule],
) -> PermissionCheckResult {
    if result.state == PermissionState::Deny {
        return result;
    }
    let subject = get_pattern_approval_subject(&result, input);
    let config_rule = [create_config_evaluation_rule(&result)];
    let evaluated =
        evaluate::evaluate(&result.tool_name, &subject, &[&config_rule, session_rules, permanent_rules]);
    PermissionCheckResult {
        state: evaluated.action,
        matched_pattern: evaluated.matched_pattern.or(result.matched_pattern),
        ..result
    }
}

// -------------------------------------------------------------------------- model-facing reasons

/// pi `formatPermissionHardStopHint` (`index.ts:352-358`).
fn hard_stop_hint(result: &PermissionCheckResult) -> String {
    if (result.source == CheckSource::Mcp || result.tool_name == "mcp") && result.target.is_some() {
        "Hard stop: this MCP permission denial is policy-enforced. Do not retry this target, do not run discovery/investigation to bypass it, and report the block to the user.".to_string()
    } else {
        "Hard stop: this permission denial is policy-enforced. Do not retry or investigate bypasses; report the block to the user.".to_string()
    }
}

/// pi `formatDenyReason` (`index.ts:360-382`).
#[must_use]
pub fn format_deny_reason(result: &PermissionCheckResult, agent_name: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(agent) = agent_name {
        parts.push(format!("Agent '{agent}'"));
    }
    if (result.source == CheckSource::Mcp || result.tool_name == "mcp") && result.target.is_some() {
        if let Some(target) = &result.target {
            parts.push(format!("is not permitted to run MCP target '{target}'"));
        }
    } else {
        parts.push(format!("is not permitted to run '{}'", result.tool_name));
    }
    if let Some(command) = &result.command {
        parts.push(format!("command '{command}'"));
    }
    if let Some(pattern) = &result.matched_pattern {
        parts.push(format!("(matched '{pattern}')"));
    }
    format!("{}. {}", parts.join(" "), hard_stop_hint(result))
}

/// pi `formatUserDeniedReason` (`index.ts:384-393`).
#[must_use]
pub fn format_user_denied_reason(
    result: &PermissionCheckResult,
    denial_reason: Option<&str>,
) -> String {
    let base = if (result.source == CheckSource::Mcp || result.tool_name == "mcp")
        && result.target.is_some()
    {
        result
            .target
            .as_ref()
            .map(|t| format!("User denied MCP target '{t}'."))
            .unwrap_or_default()
    } else if result.tool_name == "bash" && result.command.is_some() {
        result
            .command
            .as_ref()
            .map(|c| format!("User denied bash command '{c}'."))
            .unwrap_or_default()
    } else {
        format!("User denied tool '{}'.", result.tool_name)
    };
    let suffix = denial_reason.map(|r| format!(" Reason: {r}.")).unwrap_or_default();
    format!("{base}{suffix} {}", hard_stop_hint(result))
}

/// Max inline tool-input preview length (pi `TOOL_INPUT_PREVIEW_MAX_LENGTH`, `index.ts:395`).
const TOOL_INPUT_PREVIEW_MAX_LENGTH: usize = 200;

/// pi `serializeToolInputPreview` + `truncateInlineText` (`index.ts:535-541,398-400`): a whitespace-
/// collapsed JSON one-liner, truncated to 200 chars with an ellipsis; empty for `{}`/`null`/empty.
fn serialize_tool_input_preview(input: &Value) -> String {
    if input.is_null() {
        return String::new();
    }
    let Ok(serialized) = serde_json::to_string(input) else {
        return String::new();
    };
    if serialized == "{}" || serialized == "null" {
        return String::new();
    }
    let collapsed = serialized.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return String::new();
    }
    if collapsed.chars().count() > TOOL_INPUT_PREVIEW_MAX_LENGTH {
        let head: String = collapsed.chars().take(TOOL_INPUT_PREVIEW_MAX_LENGTH).collect();
        format!("{head}…")
    } else {
        collapsed
    }
}

/// pi `formatAskPrompt` (`index.ts:570-590`): the human-facing **prompt** message shown in the live
/// dialog (distinct from [`format_ask_unavailable_reason`], the headless block reason). Ports the
/// `bash` / `mcp` / generic branches; the detailed per-tool input previews (edit/write/read/find/
/// grep/ls, `formatToolInputForPrompt`) are DEFERRED to a follow-up — cosmetic (they only shape the
/// dialog display string + the dedup fingerprint, never the enforcement decision) — the generic
/// branch falls back to the compact JSON preview pi uses for every unrecognized tool.
#[must_use]
pub fn format_ask_prompt(result: &PermissionCheckResult, agent_name: Option<&str>, input: &Value) -> String {
    let subject = match agent_name {
        Some(agent) => format!("Agent '{agent}'"),
        None => "Current agent".to_string(),
    };
    let pattern_info = result
        .matched_pattern
        .as_ref()
        .map(|p| format!(" (matched '{p}')"))
        .unwrap_or_default();

    if result.tool_name == "bash" {
        let command = result.command.clone().unwrap_or_default();
        return format!("{subject} requested bash command '{command}'{pattern_info}. Allow this command?");
    }
    if (result.source == CheckSource::Mcp || result.tool_name == "mcp")
        && let Some(target) = &result.target
    {
        return format!("{subject} requested MCP target '{target}'{pattern_info}. Allow this call?");
    }
    let preview = serialize_tool_input_preview(input);
    let input_suffix = if preview.is_empty() { String::new() } else { format!(" with input {preview}") };
    format!(
        "{subject} requested tool '{}'{pattern_info}{input_suffix}. Allow this call?",
        result.tool_name
    )
}

/// pi `index.ts:2445-2449` — the "requires approval, but no interactive UI is available" reason the
/// gate returns when an `ask` cannot reach a human (the fail-closed Phase-0 path).
#[must_use]
pub fn format_ask_unavailable_reason(result: &PermissionCheckResult) -> String {
    if result.tool_name == "bash"
        && let Some(command) = &result.command
    {
        return format!(
            "Running bash command '{command}' requires approval, but no interactive UI is available."
        );
    }
    if result.tool_name == "mcp" {
        return "Using tool 'mcp' requires approval, but no interactive UI is available.".to_string();
    }
    format!("Using tool '{}' requires approval, but no interactive UI is available.", result.tool_name)
}

/// pi `formatMissingToolNameReason` (`index.ts:336-338`).
#[must_use]
pub fn format_missing_tool_name_reason() -> String {
    "Tool call was blocked because no tool name was provided. Use a registered tool name."
        .to_string()
}

// -------------------------------------------------------------------- registry / unknown-tool gate

/// pi `checkRequestedToolRegistration` (`tool-registry.ts:87-131`), cyrup no-alias form: `None` when
/// `requested` IS one of the `registered` tool names (proceed), `Some(reason)` (the model-facing
/// unknown-tool block reason, pi `formatUnknownToolReason`) when it is not. cyrup registers no tool
/// aliases, so the registered-lookup collapses to a direct membership test over the full registry
/// ([`cyrup_ext::HostServices::all_tool_names`], the `pi.getAllTools()` analog). Checked BEFORE any
/// permission check (pi `index.ts:2218-2228`), so an unregistered tool is blocked pre-policy.
#[must_use]
pub fn check_requested_tool_registration(requested: &str, registered: &[String]) -> Option<String> {
    if registered.iter().any(|n| n == requested) {
        return None;
    }
    // pi builds `availableToolNames` as a de-duplicated set sorted by `localeCompare`; the registry
    // is already unique-by-name, and a lexicographic sort is faithful enough for a display-only list.
    let mut available: Vec<String> = registered.to_vec();
    available.sort();
    available.dedup();
    Some(format_unknown_tool_reason(requested, &available))
}

/// pi `formatUnknownToolReason` (`index.ts:340-350`): the block reason for a tool the runtime never
/// registered — a 10-name preview of the available tools (`, ...` when truncated, `none` when empty)
/// plus the `mcp` server-tool hint (omitted when the unknown tool IS `mcp`).
#[must_use]
pub fn format_unknown_tool_reason(tool_name: &str, available_tool_names: &[String]) -> String {
    let preview: Vec<&String> = available_tool_names.iter().take(10).collect();
    let suffix = if available_tool_names.len() > preview.len() { ", ..." } else { "" };
    let available_list = if preview.is_empty() {
        "none".to_string()
    } else {
        format!("{}{suffix}", preview.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
    };
    let mcp_hint = if tool_name == "mcp" {
        ""
    } else {
        " If this was intended as an MCP server tool, call the registered 'mcp' tool when available (for example: {\"tool\":\"server:tool\"})."
    };
    format!(
        "Tool '{tool_name}' is not registered in this runtime and was blocked before permission checks.{mcp_hint} Registered tools: {available_list}."
    )
}

// ------------------------------------------------------------------------- external-directory guard

/// pi `isPathOutsideWorkingDirectory` (`index.ts:236-240`): true when both `path_value` and `cwd`
/// normalize to non-empty and the path is NOT within the working directory.
#[must_use]
pub fn is_path_outside_working_directory(path_value: &str, cwd: &str) -> bool {
    let normalized_cwd = common::normalize_path_for_comparison(cwd, cwd);
    let normalized_path = common::normalize_path_for_comparison(path_value, cwd);
    !normalized_cwd.is_empty()
        && !normalized_path.is_empty()
        && !common::is_path_within_directory(&normalized_path, &normalized_cwd)
}

/// pi `formatExternalDirectoryHardStopHint` (`index.ts:649-651`).
fn external_directory_hard_stop_hint() -> &'static str {
    "Hard stop: this external directory permission denial is policy-enforced. Do not retry this path, do not attempt a filesystem bypass, and report the block to the user."
}

/// pi `formatExternalDirectoryAskPrompt` (`index.ts:653-661`).
#[must_use]
pub fn format_external_directory_ask_prompt(
    tool_name: &str,
    path_value: &str,
    cwd: &str,
    agent_name: Option<&str>,
) -> String {
    let subject = match agent_name {
        Some(a) => format!("Agent '{a}'"),
        None => "Current agent".to_string(),
    };
    format!(
        "{subject} requested tool '{tool_name}' for path '{path_value}' outside working directory '{cwd}'. Allow this external directory access?"
    )
}

/// pi `formatExternalDirectoryDenyReason` (`index.ts:663-671`).
#[must_use]
pub fn format_external_directory_deny_reason(
    tool_name: &str,
    path_value: &str,
    cwd: &str,
    agent_name: Option<&str>,
) -> String {
    let subject = match agent_name {
        Some(a) => format!("Agent '{a}'"),
        None => "Current agent".to_string(),
    };
    format!(
        "{subject} is not permitted to run tool '{tool_name}' for path '{path_value}' outside working directory '{cwd}'. {}",
        external_directory_hard_stop_hint()
    )
}

/// pi `formatExternalDirectoryUserDeniedReason` (`index.ts:673-680`).
#[must_use]
pub fn format_external_directory_user_denied_reason(
    tool_name: &str,
    path_value: &str,
    denial_reason: Option<&str>,
) -> String {
    let reason_suffix = denial_reason.map(|r| format!(" Reason: {r}.")).unwrap_or_default();
    format!(
        "User denied external directory access for tool '{tool_name}' path '{path_value}'.{reason_suffix} {}",
        external_directory_hard_stop_hint()
    )
}

/// pi external-directory confirmation-unavailable reason (`index.ts:2365`).
#[must_use]
pub fn format_external_directory_unavailable_reason(path_value: &str) -> String {
    format!(
        "Accessing '{path_value}' outside the working directory requires approval, but no interactive UI is available."
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn bash_deny() -> PermissionCheckResult {
        PermissionCheckResult {
            tool_name: "bash".into(),
            state: PermissionState::Deny,
            matched_pattern: Some("rm *".into()),
            command: Some("rm -rf /".into()),
            target: None,
            source: CheckSource::Bash,
        }
    }

    #[test]
    fn deny_reason_names_command_and_pattern_and_hard_stop() {
        let reason = format_deny_reason(&bash_deny(), None);
        assert!(reason.contains("is not permitted to run 'bash'"));
        assert!(reason.contains("command 'rm -rf /'"));
        assert!(reason.contains("(matched 'rm *')"));
        assert!(reason.contains("Hard stop"));
    }

    #[test]
    fn overlay_session_allow_promotes_ask_to_allow() {
        let ask = PermissionCheckResult {
            tool_name: "bash".into(),
            state: PermissionState::Ask,
            matched_pattern: None,
            command: Some("git push".into()),
            target: None,
            source: CheckSource::Bash,
        };
        let session = [PatternRule { tool: "bash".into(), pattern: "git *".into(), action: PermissionState::Allow }];
        let out = apply_pattern_approval_state(ask, &serde_json::json!({}), &session, &[]);
        assert_eq!(out.state, PermissionState::Allow);
    }

    #[test]
    fn likely_filesystem_tool_name_matches_non_builtin_fs_names() {
        // Builtin exact names.
        assert!(is_likely_filesystem_tool_name("read"));
        assert!(is_likely_filesystem_tool_name("ls"));
        // Non-builtin filesystem-like names (split-part match).
        assert!(is_likely_filesystem_tool_name("read_file"));
        assert!(is_likely_filesystem_tool_name("grep_files"));
        assert!(is_likely_filesystem_tool_name("fs_search"));
        assert!(is_likely_filesystem_tool_name("list-dir"));
        assert!(is_likely_filesystem_tool_name("workspace_write"));
        // endsWith match (no separator).
        assert!(is_likely_filesystem_tool_name("myread"));
        // Non-filesystem names do NOT match.
        assert!(!is_likely_filesystem_tool_name("bash"));
        assert!(!is_likely_filesystem_tool_name("web_fetch"));
        assert!(!is_likely_filesystem_tool_name(""));
    }

    #[test]
    fn path_bearing_recognizes_non_builtin_fs_tool_via_heuristic() {
        // A non-builtin filesystem tool name (`read_file`) carrying a `path` is recognized as
        // path-bearing purely by the heuristic — this is the enforcement input the external-dir
        // guard keys on, so failing to recognize it would leave the path ungated.
        let path = get_path_bearing_tool_path("read_file", &serde_json::json!({ "path": "/x/secret" }));
        assert_eq!(path.as_deref(), Some("/x/secret"));
        // A non-filesystem tool with a `path`-shaped field is NOT treated as path-bearing.
        assert!(get_path_bearing_tool_path("bash", &serde_json::json!({ "path": "/x" })).is_none());
    }

    #[test]
    fn overlay_never_relaxes_deny() {
        let out = apply_pattern_approval_state(
            bash_deny(),
            &serde_json::json!({}),
            &[PatternRule { tool: "bash".into(), pattern: "*".into(), action: PermissionState::Allow }],
            &[],
        );
        assert_eq!(out.state, PermissionState::Deny);
    }
}
