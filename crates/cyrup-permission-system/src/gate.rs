//! Gate helpers (port of pi `index.ts` gate internals): the config+session approval
//! overlay (`applyPatternApprovalState`), the approval subject (`getPatternApprovalSubject`), the
//! config evaluation rule (`createConfigEvaluationRule`), and the model-facing reason formatters
//! (`formatDenyReason` / `formatUserDeniedReason` / the ask-unavailable reason). The orchestration
//! that CALLS these (the async gate on every `tool_call`) lives in `extension/decide.rs`.

use serde_json::{Map, Value};

use crate::common::{self, get_non_empty_string, to_record};
use crate::evaluate;
use crate::types::{CheckSource, PatternRule, PermissionCheckResult, PermissionState};

const PATH_BEARING_TOOLS: [&str; 6] = ["read", "write", "edit", "find", "grep", "ls"];

/// JS truthiness for a `PermissionCheckResult`'s optional string fields (`command`, `target`,
/// `matchedPattern`).
///
/// Every pi guard over these fields is a bare truthiness test — `if (result.command)`,
/// `result.toolName === "bash" && result.command`, `result.matchedPattern ? … : "*"` — so in pi an
/// EMPTY STRING is indistinguishable from an absent field. Rust's `Option` is not: a
/// `Some(String::new())` passes `.is_some()`/`if let Some(_)` and leaks `command ''` into
/// model-facing denial text, or makes an approval subject the empty string (which
/// `extension/decide.rs`'s and `extension/prompt.rs`'s `!subject.is_empty()` guards then silently
/// drop).
///
/// This is reachable, not theoretical: [`crate::manager::PermissionManager::check_permission`]'s bash branch
/// mirrors pi's `const command = typeof record.command === "string" ? record.command : ""` and
/// always emits `command: Some(command)`, so a bash tool call whose input has a missing or
/// non-string `command` key produces `Some("")`.
///
/// Only `""` is falsy — a whitespace-only string is truthy in JS, so this deliberately does NOT
/// trim (unlike [`crate::common::get_non_empty_string`], which ports pi's separate
/// `getNonEmptyString` helper for raw tool INPUT and does trim).
fn truthy(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|s| !s.is_empty())
}

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

/// pi `STRUCTURED_EDIT_OPERATION_NAMES` (v0.8.0 `permission-prompts.ts:6`): the `op` values that
/// make an entry a recognized structured edit.
const STRUCTURED_EDIT_OPERATION_NAMES: [&str; 5] =
    ["replace", "append", "prepend", "delete", "replace_text"];

/// pi `hasStructuredEditPayload` (v0.8.0 `permission-prompts.ts:23-30`): true when ANY payload
/// returned by [`get_structured_edit_payloads`] is a RECOGNIZED edit — its `op` (defaulting to
/// `"replace_text"` when absent or non-string) is in [`STRUCTURED_EDIT_OPERATION_NAMES`], **or** it
/// carries both `oldText` and `newText` as strings.
///
/// This is deliberately NOT `input.contains_key("edits")`, which is what
/// [`get_path_bearing_tool_path`] used before and which diverges from pi in **both** directions:
///
/// * **Fail-open (the serious one).** A tool input with top-level `oldText`+`newText` strings and no
///   `edits` key IS a structured-edit payload upstream (`permission-prompts.ts:16-18` synthesizes a
///   one-element `replace_text` list), so pi recognizes the tool as path-bearing and runs the
///   external-directory guard. The old key test said `false`, so for any tool whose name is not in
///   [`PATH_BEARING_TOOLS`] and does not trip [`is_likely_filesystem_tool_name`] — say a
///   `patch_document` tool called with `{path, oldText, newText}` — cyrup skipped the
///   external-directory ask/deny entirely and let the write land outside the working directory
///   ungated.
/// * **Fail-closed.** `"edits": {}` / `"edits": "x"` (not an array), `"edits": []`, or an array whose
///   every entry carries an unrecognized `op` all made the old test say `true` where pi says `false`,
///   changing both the guard and the approval subject
///   ([`get_pattern_approval_subject`] resolves a path resource instead of falling through).
#[must_use]
pub fn has_structured_edit_payload(input: &Map<String, Value>) -> bool {
    get_structured_edit_payloads(input).iter().any(|edit| {
        let record = to_record(edit);
        // pi `typeof editRecord.op === "string" ? editRecord.op : "replace_text"`.
        let op = record.get("op").and_then(Value::as_str).unwrap_or("replace_text");
        STRUCTURED_EDIT_OPERATION_NAMES.contains(&op)
            || (record.get("oldText").and_then(Value::as_str).is_some()
                && record.get("newText").and_then(Value::as_str).is_some())
    })
}

/// pi `getPathBearingToolPath` (`index.ts:216-232` @v0.8.0): the `path`/`file_path` of a filesystem
/// tool. Recognizes the exact path-bearing name set ([`PATH_BEARING_TOOLS`]), a structured-edit
/// payload ([`has_structured_edit_payload`], pi `hasStructuredEditPayload`), OR the broader
/// [`is_likely_filesystem_tool_name`] heuristic — so a non-builtin filesystem tool (`read_file`,
/// `grep_files`, `fs_search`, a `*_read`/`*_write`/`*_search`/`*_list`) is recognized as
/// path-bearing. This is an ENFORCEMENT input, not merely cosmetic: `extension/decide.rs`'s
/// external-directory guard skips the whole ask/deny check when this returns `None`, so a
/// filesystem-like tool that went unrecognized would reach a path OUTSIDE the working directory
/// ungated.
#[must_use]
pub fn get_path_bearing_tool_path(tool_name: &str, input: &Value) -> Option<String> {
    let record = to_record(input);
    let path = get_non_empty_string(record.get("path"))
        .or_else(|| get_non_empty_string(record.get("file_path")))?;
    // pi checks `PATH_BEARING_TOOLS.has(toolName)` FIRST and returns early (`index.ts:224-226`),
    // then the structured-edit / filesystem-heuristic pair (`:228-230`); `||` makes the order
    // unobservable, but the arms are kept in upstream's sequence.
    if PATH_BEARING_TOOLS.contains(&tool_name)
        || has_structured_edit_payload(record)
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
        && let Some(cmd) = truthy(&result.command)
    {
        return cmd.to_string();
    }
    if (result.source == CheckSource::Mcp || result.tool_name == "mcp")
        && let Some(target) = truthy(&result.target)
    {
        return target.to_string();
    }
    if let Some(path) = get_path_bearing_tool_path(&result.tool_name, input) {
        let cwd = get_non_empty_string(to_record(input).get("cwd"))
            .unwrap_or_else(|| std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default());
        let resource = common::normalize_path_resource_for_permission(&path, &cwd);
        return if resource.is_empty() { path } else { resource };
    }
    if let Some(target) = truthy(&result.target) {
        let prefix = format!("{}:", result.tool_name);
        return match target.strip_prefix(&prefix) {
            Some(rest) => rest.to_string(),
            None => target.to_string(),
        };
    }
    // pi `return result.command || result.toolName;` — an empty `command` falls through to the tool
    // name, so an "Allow Always" on a malformed bash call still persists a `bash`/`bash` rule
    // instead of being dropped by `apply_decision`'s `!subject.is_empty()` guard.
    truthy(&result.command).unwrap_or(&result.tool_name).to_string()
}

/// pi `createConfigEvaluationRule` (`index.ts:841-848`): reuse the matched pattern only for
/// `bash|mcp|skill|special` sources, else `"*"`.
#[must_use]
pub fn create_config_evaluation_rule(result: &PermissionCheckResult) -> PatternRule {
    let can_reuse = matches!(
        result.source,
        CheckSource::Bash | CheckSource::Mcp | CheckSource::Skill | CheckSource::Special
    );
    // pi `canReuseMatchedPattern && result.matchedPattern ? result.matchedPattern : "*"` — an empty
    // matched pattern is falsy and falls back to `"*"`.
    let pattern = match (truthy(&result.matched_pattern), can_reuse) {
        (Some(p), true) => p.to_string(),
        _ => "*".to_string(),
    };
    PatternRule { tool: result.tool_name.clone(), pattern, action: result.state }
}

/// pi `applyPatternApprovalState` (v0.8.0 `index.ts:557-579`): fold the config result with the
/// SESSION store (ruleset order `[config, session]`, last-match-wins → session beats config). A
/// `deny` short-circuits (never relaxed).
///
/// v0.7.1 took a fourth ruleset — `permanentApprovals.getRules()` (v0.7.1 `index.ts:852-874`) — and
/// ranked it LAST, so an on-disk `cyrup-permission-system-approvals.json` could override both the
/// session store and the operator's config rule, in either direction (it was tri-state). Upstream
/// deleted `PermanentApprovalStore` in v0.8.0 (commit `a33ac2c`; CHANGELOG `### Removed`), so that
/// tier is gone here too — see [`crate::stores`] for the full removal note.
#[must_use]
pub fn apply_pattern_approval_state(
    result: PermissionCheckResult,
    input: &Value,
    session_rules: &[PatternRule],
) -> PermissionCheckResult {
    if result.state == PermissionState::Deny {
        return result;
    }
    let subject = get_pattern_approval_subject(&result, input);
    let config_rule = [create_config_evaluation_rule(&result)];
    let evaluated = evaluate::evaluate(&result.tool_name, &subject, &[&config_rule, session_rules]);
    PermissionCheckResult {
        state: evaluated.action,
        matched_pattern: evaluated.matched_pattern.or(result.matched_pattern),
        ..result
    }
}

// -------------------------------------------------------------------------- model-facing reasons

/// pi `formatPermissionHardStopHint` (`index.ts:352-358`).
fn hard_stop_hint(result: &PermissionCheckResult) -> String {
    if (result.source == CheckSource::Mcp || result.tool_name == "mcp")
        && truthy(&result.target).is_some()
    {
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
    match truthy(&result.target) {
        Some(target) if result.source == CheckSource::Mcp || result.tool_name == "mcp" => {
            parts.push(format!("is not permitted to run MCP target '{target}'"));
        }
        _ => parts.push(format!("is not permitted to run '{}'", result.tool_name)),
    }
    // pi `if (result.command)` / `if (result.matchedPattern)` — an empty string contributes nothing.
    if let Some(command) = truthy(&result.command) {
        parts.push(format!("command '{command}'"));
    }
    if let Some(pattern) = truthy(&result.matched_pattern) {
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
    let mcp = result.source == CheckSource::Mcp || result.tool_name == "mcp";
    let base = match (truthy(&result.target), truthy(&result.command)) {
        (Some(target), _) if mcp => format!("User denied MCP target '{target}'."),
        // pi `result.toolName === "bash" && result.command ? … : `User denied tool '…'.`` — an
        // empty command falls through to the generic tool form.
        (_, Some(command)) if result.tool_name == "bash" => {
            format!("User denied bash command '{command}'.")
        }
        _ => format!("User denied tool '{}'.", result.tool_name),
    };
    let suffix = denial_reason.map(|r| format!(" Reason: {r}.")).unwrap_or_default();
    format!("{base}{suffix} {}", hard_stop_hint(result))
}

/// Max inline tool-input preview length (pi `TOOL_INPUT_PREVIEW_MAX_LENGTH`, `index.ts:395`).
const TOOL_INPUT_PREVIEW_MAX_LENGTH: usize = 200;

/// Max inline sanitized-text summary length (pi `TOOL_TEXT_SUMMARY_MAX_LENGTH`, `index.ts:396`).
const TOOL_TEXT_SUMMARY_MAX_LENGTH: usize = 80;

/// pi `truncateInlineText` (v0.8.0 `permission-prompts.ts:91-93`):
/// `value.length > maxLength ? `${value.slice(0, maxLength)}…` : value`.
///
/// **PERM-030 — the unit is UTF-16 code units**, both for the test and for the cut point, because
/// that is what JS `String.length` and `String.slice` operate on. This previously used
/// `chars().count()` / `chars().take()` (Unicode scalars), so any astral-plane character — an emoji
/// in a bash command, a path, or `write` content — moved the truncation boundary relative to pi
/// for byte-identical input. Same convention as `wildcard.rs:81` and
/// `logging::sensitive_log_metadata`; this is the surface a HUMAN reads before allowing or denying,
/// so it is the one place the three counting units in this crate mattered most.
///
/// `slice(0, maxLength)` in JS can split a surrogate pair, yielding a lone surrogate. Rust `String`
/// cannot hold one, so a cut that would land mid-pair backs up to the preceding boundary — the only
/// possible divergence, it is one code unit, and it is in the direction of showing less rather than
/// producing an unpaired surrogate. \[CYRUP-DELTA]
fn truncate_inline_text(value: &str, max_length: usize) -> String {
    if value.encode_utf16().count() <= max_length {
        return value.to_string();
    }
    // Walk char boundaries accumulating UTF-16 width, stopping at the last boundary that does not
    // exceed `max_length`.
    let mut units = 0usize;
    let mut end = 0usize;
    for (index, ch) in value.char_indices() {
        let width = ch.len_utf16();
        if units + width > max_length {
            end = index;
            break;
        }
        units += width;
        end = index + ch.len_utf8();
    }
    format!("{}…", &value[..end])
}

/// pi `sanitizeInlineText` (`index.ts:402-405`): whitespace-collapsed, trimmed, truncated inline
/// text; `"empty text"` when nothing remains after normalization.
fn sanitize_inline_text(value: &str, max_length: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        "empty text".to_string()
    } else {
        truncate_inline_text(&normalized, max_length)
    }
}

/// pi `countTextLines` (`index.ts:407-413`): the number of `\r\n`/`\r`/`\n`-separated segments;
/// `0` for an empty string (pi's falsy-string guard).
fn count_text_lines(value: &str) -> usize {
    if value.is_empty() {
        return 0;
    }
    let bytes = value.as_bytes();
    let mut count = 1usize;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\r' => {
                count += 1;
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => count += 1,
            _ => {}
        }
        i += 1;
    }
    count
}

/// pi `formatCount` (`index.ts:415-417`): `"{n} {singular|plural}"`.
fn format_count(value: usize, singular: &str, plural: &str) -> String {
    format!("{value} {}", if value == 1 { singular } else { plural })
}

/// pi `getPromptPath` (`index.ts:419-421`): the `path`, else `file_path`.
fn get_prompt_path(input: &Map<String, Value>) -> Option<String> {
    get_non_empty_string(input.get("path")).or_else(|| get_non_empty_string(input.get("file_path")))
}

/// pi `countEditPayloadLines` (`index.ts:423-431`): the line count of an edit's `lines` payload —
/// the count of string entries for an array, `countTextLines` (minus one trailing `\n`) for a
/// string, else `0`.
fn count_edit_payload_lines(value: Option<&Value>) -> usize {
    match value {
        Some(Value::Array(items)) => items.iter().filter(|v| v.is_string()).count(),
        Some(Value::String(s)) => {
            let trimmed = s.strip_suffix('\n').unwrap_or(s);
            count_text_lines(trimmed)
        }
        _ => 0,
    }
}

/// pi `formatEditReference` (`index.ts:433-437`): a sanitized inline reference, or `"anchor"` when
/// the value is not a non-empty string.
fn format_edit_reference(value: Option<&Value>) -> String {
    match value.and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => sanitize_inline_text(s, 40),
        _ => "anchor".to_string(),
    }
}

/// pi `STRUCTURED_EDIT_OPERATION_NAMES`-adjacent op-default: `edit.op` if a string, else
/// `"replace_text"` (`index.ts:441`).
fn structured_edit_op(edit: &Map<String, Value>) -> String {
    edit.get("op").and_then(Value::as_str).unwrap_or("replace_text").to_string()
}

/// pi `formatStructuredEditSummary` (`index.ts:439-470`): the human-readable summary of a single
/// structured edit payload, `None` for an unrecognized `op`.
fn format_structured_edit_summary(edit: &Map<String, Value>, index: usize) -> Option<String> {
    let ordinal = format!("edit #{}", index + 1);
    let op = structured_edit_op(edit);

    if let (Some(old_text), Some(new_text)) =
        (edit.get("oldText").and_then(Value::as_str), edit.get("newText").and_then(Value::as_str))
        && op == "replace_text"
    {
        return Some(format!(
            "{ordinal} replaces {} with {}",
            format_count(count_text_lines(old_text), "line", "lines"),
            format_count(count_text_lines(new_text), "line", "lines")
        ));
    }

    let line_count = format_count(count_edit_payload_lines(edit.get("lines")), "line", "lines");
    match op.as_str() {
        "replace" => {
            let start = format_edit_reference(edit.get("pos"));
            let end = match edit.get("end").and_then(Value::as_str) {
                Some(e) if !e.trim().is_empty() => format!(" through {}", format_edit_reference(edit.get("end"))),
                _ => String::new(),
            };
            Some(format!("{ordinal} replaces {line_count} at {start}{end}"))
        }
        "append" => {
            let suffix = match edit.get("pos").and_then(Value::as_str) {
                Some(_) => format!(" after {}", format_edit_reference(edit.get("pos"))),
                None => " at EOF".to_string(),
            };
            Some(format!("{ordinal} appends {line_count}{suffix}"))
        }
        "prepend" => {
            let suffix = match edit.get("pos").and_then(Value::as_str) {
                Some(_) => format!(" before {}", format_edit_reference(edit.get("pos"))),
                None => " at BOF".to_string(),
            };
            Some(format!("{ordinal} prepends {line_count}{suffix}"))
        }
        "delete" => {
            let start = format_edit_reference(edit.get("pos"));
            let end = match edit.get("end").and_then(Value::as_str) {
                Some(e) if !e.trim().is_empty() => format!(" through {}", format_edit_reference(edit.get("end"))),
                _ => String::new(),
            };
            Some(format!("{ordinal} deletes at {start}{end}"))
        }
        _ => None,
    }
}

/// pi `getStructuredEditPayloads` (`index.ts:185-195`): the `edits` array verbatim, else a
/// single-element `replace_text` payload synthesized from top-level `oldText`/`newText`, else empty.
fn get_structured_edit_payloads(input: &Map<String, Value>) -> Vec<Value> {
    if let Some(Value::Array(edits)) = input.get("edits") {
        return edits.clone();
    }
    if let (Some(old_text), Some(new_text)) =
        (input.get("oldText").and_then(Value::as_str), input.get("newText").and_then(Value::as_str))
    {
        return vec![serde_json::json!({
            "op": "replace_text",
            "oldText": old_text,
            "newText": new_text,
        })];
    }
    Vec::new()
}

/// pi `formatStructuredEditInputForPrompt` (`index.ts:472-489`): the `(N edits: ...)` summary for a
/// structured-edit input, prefixed with `for 'path'` when a path is present; `fallback` (optionally
/// path-prefixed) when there are no recognized edit summaries, `None` when there is no fallback either.
fn format_structured_edit_input_for_prompt(
    input: &Map<String, Value>,
    fallback: Option<&str>,
) -> Option<String> {
    let path = get_prompt_path(input);
    let summaries: Vec<String> = get_structured_edit_payloads(input)
        .iter()
        .enumerate()
        .filter_map(|(index, edit)| format_structured_edit_summary(to_record(edit), index))
        .collect();
    let path_part = path.map(|p| format!("for '{p}'"));

    if summaries.is_empty() {
        let fallback = fallback?;
        return Some(match &path_part {
            Some(pp) => format!("{pp} {fallback}"),
            None => fallback.to_string(),
        });
    }

    let extra_edits = if summaries.len() > 1 {
        format!(", plus {}", format_count(summaries.len() - 1, "additional edit", "additional edits"))
    } else {
        String::new()
    };
    // `summaries` is provably non-empty here (the `is_empty()` early-return above), so `first()`
    // always yields `Some`; `unwrap_or_default` just avoids an indexing-slicing panic path for a case
    // that cannot occur, without reaching for `unwrap`/`expect`.
    let first_summary = summaries.first().cloned().unwrap_or_default();
    let summary =
        format!("({}: {}{extra_edits})", format_count(summaries.len(), "edit", "edits"), first_summary);
    Some(match &path_part {
        Some(pp) => format!("{pp} {summary}"),
        None => summary,
    })
}

/// pi `formatEditInputForPrompt` (`index.ts:491-493`).
fn format_edit_input_for_prompt(input: &Map<String, Value>) -> String {
    format_structured_edit_input_for_prompt(input, Some("with edit input"))
        .unwrap_or_else(|| "with edit input".to_string())
}

/// pi `formatWriteInputForPrompt` (v0.8.0 `permission-prompts.ts:189-194`).
fn format_write_input_for_prompt(input: &Map<String, Value>) -> String {
    let path = get_prompt_path(input);
    let content = input.get("content").and_then(Value::as_str).unwrap_or("");
    let summary = format!(
        "({}, {})",
        format_count(count_text_lines(content), "line", "lines"),
        // PERM-030: pi `formatCount(content.length, …)` (`permission-prompts.ts:193`) — UTF-16 code
        // units, so a single emoji reads "2 characters" upstream. `chars().count()` said 1.
        format_count(content.encode_utf16().count(), "character", "characters")
    );
    match path {
        Some(p) => format!("for '{p}' {summary}"),
        None => summary,
    }
}

/// pi `formatReadInputForPrompt` (`index.ts:502-512`).
fn format_read_input_for_prompt(input: &Map<String, Value>) -> String {
    let path = get_prompt_path(input);
    let mut parts: Vec<String> = Vec::new();
    if let Some(p) = &path {
        parts.push(format!("path '{p}'"));
    }
    if let Some(offset) = input.get("offset")
        && offset.is_number()
    {
        parts.push(format!("offset {offset}"));
    }
    if let Some(limit) = input.get("limit")
        && limit.is_number()
    {
        parts.push(format!("limit {limit}"));
    }
    if parts.is_empty() { String::new() } else { format!("for {}", parts.join(", ")) }
}

/// pi `formatSearchInputForPrompt` (`index.ts:514-533`).
fn format_search_input_for_prompt(tool_name: &str, input: &Map<String, Value>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let path = get_prompt_path(input);
    let pattern = get_non_empty_string(input.get("pattern"));
    let glob = get_non_empty_string(input.get("glob"));

    if let Some(p) = &pattern {
        parts.push(format!("pattern '{}'", sanitize_inline_text(p, TOOL_TEXT_SUMMARY_MAX_LENGTH)));
    }
    if let Some(g) = &glob {
        parts.push(format!("glob '{}'", sanitize_inline_text(g, TOOL_TEXT_SUMMARY_MAX_LENGTH)));
    }
    if let Some(p) = &path {
        parts.push(format!("path '{p}'"));
    } else if matches!(tool_name, "find" | "grep" | "ls") {
        parts.push("current working directory".to_string());
    }

    if parts.is_empty() { String::new() } else { format!("for {}", parts.join(", ")) }
}

/// pi `serializeToolInputPreview` (`index.ts:535-542`): a whitespace-collapsed JSON one-liner; empty
/// for `{}`/`null`/empty.
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
    serialized.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// pi `formatJsonInputForPrompt` (`index.ts:544-547`): the generic `with input {json}` preview,
/// truncated to [`TOOL_INPUT_PREVIEW_MAX_LENGTH`].
fn format_json_input_for_prompt(input: &Value) -> String {
    let inline = serialize_tool_input_preview(input);
    if inline.is_empty() {
        String::new()
    } else {
        format!("with input {}", truncate_inline_text(&inline, TOOL_INPUT_PREVIEW_MAX_LENGTH))
    }
}

/// pi `formatToolInputForPrompt` (`index.ts:549-568`): dispatch on `tool_name` to the per-tool
/// structured summary (`edit`/`write`/`read`/`find`/`grep`/`ls`), else a structured-edit summary for
/// a non-builtin tool that still carries an `edits`/`oldText`+`newText` payload, else the generic
/// JSON preview.
fn format_tool_input_for_prompt(tool_name: &str, input: &Value) -> String {
    let record = to_record(input);
    match tool_name {
        "edit" => format_edit_input_for_prompt(record),
        "write" => format_write_input_for_prompt(record),
        "read" => format_read_input_for_prompt(record),
        "find" | "grep" | "ls" => format_search_input_for_prompt(tool_name, record),
        _ => format_structured_edit_input_for_prompt(record, None)
            .unwrap_or_else(|| format_json_input_for_prompt(input)),
    }
}

/// pi `formatAskPrompt` (`index.ts:570-590`): the human-facing **prompt** message shown in the live
/// dialog (distinct from [`format_ask_unavailable_reason`], the headless block reason). Ports the
/// `bash` / `mcp` branches verbatim, and the generic branch dispatches through
/// [`format_tool_input_for_prompt`] for the per-tool structured input preview (edit/write/read/find/
/// grep/ls), falling back to the compact JSON preview for every other tool.
#[must_use]
pub fn format_ask_prompt(result: &PermissionCheckResult, agent_name: Option<&str>, input: &Value) -> String {
    let subject = match agent_name {
        Some(agent) => format!("Agent '{agent}'"),
        None => "Current agent".to_string(),
    };
    // pi `result.matchedPattern ? ` (matched '…')` : ""` — truthiness, so an empty pattern is omitted.
    let pattern_info =
        truthy(&result.matched_pattern).map(|p| format!(" (matched '{p}')")).unwrap_or_default();

    if result.tool_name == "bash" {
        // pi `${result.command || ""}` — the bash ask prompt DOES render an empty command inline.
        let command = result.command.clone().unwrap_or_default();
        return format!("{subject} requested bash command '{command}'{pattern_info}. Allow this command?");
    }
    if (result.source == CheckSource::Mcp || result.tool_name == "mcp")
        && let Some(target) = truthy(&result.target)
    {
        return format!("{subject} requested MCP target '{target}'{pattern_info}. Allow this call?");
    }
    let input_preview = format_tool_input_for_prompt(&result.tool_name, input);
    let input_suffix = if input_preview.is_empty() { String::new() } else { format!(" {input_preview}") };
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

/// pi `formatMissingToolNameReason` (v0.8.0 `permission-prompts.ts:32-34`; v0.7.1
/// `index.ts:336-338`).
///
/// **\[CYRUP-DELTA]** upstream's literal ends `"Use a registered tool name from pi.getAllTools()."`
/// (`permission-prompts.ts:33`). The trailing clause names pi's JavaScript extension-runtime global,
/// which does not exist in cyrup — the analog is the Rust
/// [`cyrup_ext::HostServices::all_tool_names`], not something a model could be told to "call" — so
/// the sentence is truncated after `"Use a registered tool name."` rather than shipping a
/// model-facing instruction to invoke an API that is absent from this runtime. Recorded here
/// because it was previously an undocumented silent divergence in a model-facing block reason.
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
    use serde_json::json;

    // ------------------------------------------------------- PERM-030: UTF-16 counting units

    /// PERM-030 (RED before the fix). pi truncates with `value.slice(0, maxLength)` against
    /// `value.length` (v0.8.0 `permission-prompts.ts:91-93`) — UTF-16 CODE UNITS. Cyrup used
    /// `chars().count()` / `chars().take()` (Unicode SCALARS), so an astral-plane character moved
    /// the boundary: with `max_length = 2`, pi's `"a😀b".slice(0,2)` keeps `"a"` plus the emoji's
    /// HIGH surrogate, i.e. one whole scalar's worth of budget consumed by the emoji, while
    /// `chars().take(2)` kept `"a😀"` — a visibly different approval dialog for identical input.
    #[test]
    fn truncate_inline_text_counts_utf16_code_units() {
        // Under the cap in scalars (3) but OVER it in UTF-16 units (4) ⇒ pi truncates, cyrup used
        // not to.
        assert_eq!("a\u{1F600}b".chars().count(), 3);
        assert_eq!("a\u{1F600}b".encode_utf16().count(), 4);
        let out = truncate_inline_text("a\u{1F600}b", 3);
        assert!(out.ends_with('…'), "3 UTF-16 units < 4 ⇒ pi truncates; got {out:?}");
        // "a" costs 1 unit and the emoji costs 2, so the pair fits a 3-unit budget EXACTLY — pi's
        // `"a😀b".slice(0, 3)` keeps both code units of the surrogate pair and yields "a😀", the
        // only character dropped being the trailing "b".
        assert_eq!(out, "a\u{1F600}\u{2026}");

        // `max_length = 2` is the case where the budget lands INSIDE the surrogate pair, and the
        // one the doc comment above is about. pi's `slice(0, 2)` keeps "a" plus the emoji's lone
        // HIGH surrogate — an unpaired surrogate, which is a well-formed JS string but has no
        // UTF-8 representation at all, so Rust cannot reproduce it byte-for-byte and stops at the
        // last whole scalar instead. Same visible result: a lone high surrogate has no glyph and
        // renders as U+FFFD in any terminal pi's prompt would be drawn in, so "a…" is what both
        // sides show. This is the boundary `chars().take(2)` used to get wrong by keeping "a😀".
        assert_eq!(truncate_inline_text("a\u{1F600}b", 2), "a\u{2026}");

        // Exactly at the cap in UTF-16 units ⇒ untouched (pi's test is `>`, not `>=`).
        assert_eq!(truncate_inline_text("a\u{1F600}b", 4), "a\u{1F600}b");

        // Pure ASCII is unchanged in every unit, so the classic case still behaves.
        assert_eq!(truncate_inline_text("abcdef", 3), "abc…");
        assert_eq!(truncate_inline_text("abc", 3), "abc");

        // A BMP non-ASCII char is 1 UTF-16 unit (and 2 UTF-8 bytes) — the byte-count reading would
        // have truncated here and must not.
        assert_eq!(truncate_inline_text("café", 4), "café");
    }

    /// The write-tool summary's character count is `content.length` upstream
    /// (`permission-prompts.ts:193`), so one emoji reads "2 characters".
    #[test]
    fn the_write_summary_counts_characters_in_utf16_units() {
        let input: Map<String, Value> =
            serde_json::from_value(json!({ "path": "/w/a.txt", "content": "\u{1F600}" })).unwrap();
        let summary = format_write_input_for_prompt(&input);
        assert!(
            summary.contains("2 characters"),
            "pi's `formatCount(content.length, …)` is UTF-16 units; got {summary:?}"
        );
    }

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
        let out = apply_pattern_approval_state(ask, &serde_json::json!({}), &session);
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

    // ------------------------------- pi `hasStructuredEditPayload` (`permission-prompts.ts:23-30`)
    //
    // These pin the predicate that `get_path_bearing_tool_path` consults. It used to be the bare
    // key test `input.contains_key("edits")`, which is neither necessary nor sufficient upstream.

    /// RED before the fix, the FAIL-OPEN direction. `getStructuredEditPayloads`
    /// (`permission-prompts.ts:16-18`) synthesizes a one-element `replace_text` payload from
    /// top-level `oldText`+`newText` when there is no `edits` array, so pi's
    /// `getPathBearingToolPath` (`index.ts:227-230`) returns the path for a tool whose NAME says
    /// nothing filesystem-ish. The old `contains_key("edits")` test said `false`, so
    /// `extension/decide.rs`'s `resolve_external_directory` guard was skipped entirely and the path
    /// reached the main check ungated by the `external_directory` special policy.
    #[test]
    fn top_level_old_new_text_is_a_structured_edit_payload_without_an_edits_key() {
        let input = serde_json::json!({
            "path": "/outside/secret.txt",
            "oldText": "a",
            "newText": "b",
        });
        assert!(has_structured_edit_payload(to_record(&input)));
        // `patch_document` is not in PATH_BEARING_TOOLS and does not trip the filesystem-name
        // heuristic, so the ONLY thing that can recognize it is the structured-edit payload.
        assert!(!is_likely_filesystem_tool_name("patch_document"));
        assert_eq!(
            get_path_bearing_tool_path("patch_document", &input).as_deref(),
            Some("/outside/secret.txt"),
            "pi recognizes this as path-bearing; the bare `edits` key test did not"
        );
    }

    /// RED before the fix, the FAIL-CLOSED direction: an `edits` key that is not an array, an empty
    /// array, and an array of unrecognized ops are all `false` upstream (`Array.isArray` fails, or
    /// `.some()` finds nothing), where the bare key test said `true`.
    #[test]
    fn an_edits_key_alone_is_not_a_structured_edit_payload() {
        for edits in [
            serde_json::json!("replace"),          // not an array
            serde_json::json!({ "op": "replace" }), // not an array
            serde_json::json!([]),                  // empty array — `.some()` is false
            serde_json::json!([{ "op": "frobnicate" }]), // unrecognized op, no oldText/newText
        ] {
            let input = serde_json::json!({ "path": "/x/f.txt", "edits": edits });
            assert!(
                !has_structured_edit_payload(to_record(&input)),
                "pi's `.some()` finds no recognized edit in {edits}"
            );
            assert!(
                get_path_bearing_tool_path("patch_document", &input).is_none(),
                "and so the tool is not path-bearing for {edits}"
            );
        }
    }

    /// The recognized shapes, all five `STRUCTURED_EDIT_OPERATION_NAMES` plus the missing-`op`
    /// default (`typeof editRecord.op === "string" ? … : "replace_text"`), and the
    /// `oldText`+`newText` escape hatch on an otherwise-unrecognized op.
    #[test]
    fn recognized_structured_edit_ops_and_the_replace_text_default() {
        for op in ["replace", "append", "prepend", "delete", "replace_text"] {
            let input = serde_json::json!({ "edits": [{ "op": op }] });
            assert!(has_structured_edit_payload(to_record(&input)), "op {op} is recognized");
        }
        // Missing `op` defaults to "replace_text", which IS in the set.
        let defaulted = serde_json::json!({ "edits": [{ "lines": ["x"] }] });
        assert!(has_structured_edit_payload(to_record(&defaulted)));
        // A non-string `op` defaults the same way.
        let non_string_op = serde_json::json!({ "edits": [{ "op": 7 }] });
        assert!(has_structured_edit_payload(to_record(&non_string_op)));
        // An unrecognized op still counts when it carries both texts (pi's `||` arm).
        let escape_hatch =
            serde_json::json!({ "edits": [{ "op": "frobnicate", "oldText": "a", "newText": "b" }] });
        assert!(has_structured_edit_payload(to_record(&escape_hatch)));
        // `.some()` — one recognized entry among unrecognized ones is enough.
        let mixed = serde_json::json!({ "edits": [{ "op": "frobnicate" }, { "op": "delete" }] });
        assert!(has_structured_edit_payload(to_record(&mixed)));
        // A non-string oldText/newText pair does NOT satisfy pi's `typeof === "string"` test.
        let non_string_texts =
            serde_json::json!({ "edits": [{ "op": "frobnicate", "oldText": 1, "newText": 2 }] });
        assert!(!has_structured_edit_payload(to_record(&non_string_texts)));
    }

    /// The structured-edit recognition also decides the APPROVAL SUBJECT
    /// (`get_pattern_approval_subject` → `get_path_bearing_tool_path`), so an "Allow always" on a
    /// `{path, oldText, newText}` call now persists against the path resource pi uses rather than
    /// falling through to the bare tool name.
    #[test]
    fn approval_subject_uses_the_path_for_a_structured_edit_payload() {
        let input = serde_json::json!({
            "path": "/w/src/a.rs",
            "cwd": "/w",
            "oldText": "a",
            "newText": "b",
        });
        let subject = get_pattern_approval_subject(&ask_result("patch_document"), &input);
        assert_ne!(subject, "patch_document", "the path must win, as it does upstream");
        assert!(subject.contains("a.rs"), "got {subject}");
    }

    #[test]
    fn overlay_never_relaxes_deny() {
        let out = apply_pattern_approval_state(
            bash_deny(),
            &serde_json::json!({}),
            &[PatternRule { tool: "bash".into(), pattern: "*".into(), action: PermissionState::Allow }],
        );
        assert_eq!(out.state, PermissionState::Deny);
    }

    fn ask_result(tool_name: &str) -> PermissionCheckResult {
        PermissionCheckResult {
            tool_name: tool_name.into(),
            state: PermissionState::Ask,
            matched_pattern: None,
            command: None,
            target: None,
            source: CheckSource::Special,
        }
    }

    /// pi `formatEditInputForPrompt` via `formatToolInputForPrompt` (`index.ts:491-493,552-554`):
    /// the ask dialog shows a structured "(N edits: ...)" summary, not raw JSON. Fails against the
    /// pre-fix generic-JSON fallback (which would render `with input {"path":...,"edits":[...]}`).
    #[test]
    fn ask_prompt_edit_uses_structured_summary_not_raw_json() {
        let input = serde_json::json!({
            "path": "src/lib.rs",
            "edits": [{ "op": "replace_text", "oldText": "a\nb", "newText": "x\ny\nz" }],
        });
        let prompt = format_ask_prompt(&ask_result("edit"), None, &input);
        assert!(
            prompt.contains("for 'src/lib.rs' (1 edit: edit #1 replaces 2 lines with 3 lines)"),
            "prompt was: {prompt}"
        );
        assert!(!prompt.contains("oldText"), "prompt leaked raw JSON: {prompt}");
    }

    /// pi `formatWriteInputForPrompt` (`index.ts:495-500`): shows path + line/char counts.
    #[test]
    fn ask_prompt_write_uses_structured_summary_not_raw_json() {
        let input = serde_json::json!({ "path": "notes.txt", "content": "hello\nworld" });
        let prompt = format_ask_prompt(&ask_result("write"), None, &input);
        assert!(
            prompt.contains("for 'notes.txt' (2 lines, 11 characters)"),
            "prompt was: {prompt}"
        );
        assert!(!prompt.contains("\"content\""), "prompt leaked raw JSON: {prompt}");
    }

    /// pi `formatReadInputForPrompt` (`index.ts:502-512`): shows path/offset/limit.
    #[test]
    fn ask_prompt_read_uses_structured_summary_not_raw_json() {
        let input = serde_json::json!({ "path": "notes.txt", "offset": 10, "limit": 50 });
        let prompt = format_ask_prompt(&ask_result("read"), None, &input);
        assert!(
            prompt.contains("for path 'notes.txt', offset 10, limit 50"),
            "prompt was: {prompt}"
        );
    }

    /// pi `formatSearchInputForPrompt` (`index.ts:514-533`): grep with no path falls back to "current
    /// working directory".
    #[test]
    fn ask_prompt_grep_without_path_shows_cwd_fallback() {
        let input = serde_json::json!({ "pattern": "TODO" });
        let prompt = format_ask_prompt(&ask_result("grep"), None, &input);
        assert!(
            prompt.contains("for pattern 'TODO', current working directory"),
            "prompt was: {prompt}"
        );
    }

    /// pi `formatToolInputForPrompt` default branch (`index.ts:563-566`): an unrecognized tool with a
    /// structured `oldText`/`newText` payload still gets the structured summary, not raw JSON.
    #[test]
    fn ask_prompt_unknown_tool_with_edit_payload_uses_structured_summary() {
        let input = serde_json::json!({ "oldText": "a", "newText": "b\nc" });
        let prompt = format_ask_prompt(&ask_result("custom_patch"), None, &input);
        assert!(
            prompt.contains("(1 edit: edit #1 replaces 1 line with 2 lines)"),
            "prompt was: {prompt}"
        );
    }

    /// pi `formatToolInputForPrompt` default branch falling through to `formatJsonInputForPrompt`
    /// (`index.ts:565`): a genuinely unrecognized tool still gets the generic JSON preview.
    #[test]
    fn ask_prompt_unknown_tool_without_edit_payload_falls_back_to_json() {
        let input = serde_json::json!({ "foo": "bar" });
        let prompt = format_ask_prompt(&ask_result("some_other_tool"), None, &input);
        assert!(prompt.contains("with input"), "prompt was: {prompt}");
        assert!(prompt.contains("\"foo\":\"bar\""), "prompt was: {prompt}");
    }
}
