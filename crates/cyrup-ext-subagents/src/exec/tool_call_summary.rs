//! `{text, expandedText}` tool-call argument previews (R-SA-043's compaction target), a 1:1 port
//! of pi-subagents' `ToolCallSummary` + `formatToolCall`/`shortenPath`
//! (`pi-subagents/src/shared/types.ts:225-228`, `src/shared/formatters.ts:99-133`,
//! `src/shared/utils.ts:309-326`).
//!
//! A completed run's `tool_calls` is NOT a bare list of tool names — pi surfaces one
//! [`ToolCallSummary`] per requested tool call, each carrying BOTH a short `text` preview (for a
//! collapsed transcript row) and a longer `expandedText` preview (for an expanded one). The
//! previews render the tool's own arguments (a `bash` command, a `read`/`write`/`edit` path, or a
//! JSON dump for any other tool), truncated to pi's exact per-mode length caps. Reproduced here so
//! an on-disk `SingleResult` / a rendered result row shows what an LLM caller and a terminal user
//! see in pi, not merely the tool's name.

/// One tool-call preview (pi `ToolCallSummary`, `types.ts:225-228`): a short `text` and a longer
/// `expanded_text`, both formatted from the tool name + its requested arguments via
/// [`format_tool_call`]. `#[serde(rename_all = "camelCase")]` so `expanded_text` round-trips as
/// `expandedText`, matching pi's on-disk/wire field name exactly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallSummary {
    pub text: String,
    pub expanded_text: String,
}

impl ToolCallSummary {
    /// Build both previews for `tool_name`/`args` in one shot — the short `text` and the longer
    /// `expanded_text` — exactly as pi's `extractToolCallSummaries` does
    /// (`utils.ts:319-322`).
    #[must_use]
    pub fn from_call(tool_name: &str, args: &serde_json::Value) -> Self {
        Self {
            text: format_tool_call(tool_name, args, false),
            expanded_text: format_tool_call(tool_name, args, true),
        }
    }
}

/// Format one tool call for display (pi `formatToolCall`, `formatters.ts:99-121`).
///
/// - `bash`: `$ <command>` truncated to 60 chars (or 240 when `expanded`), with a trailing `...`
///   when cut.
/// - `read`/`write`/`edit`: `<name> <shortened path>` from the call's `path`/`file_path` argument.
/// - any other tool: `<name> <JSON(args)>` truncated to 40 chars (or 160 when `expanded`).
///
/// `args` is treated as an object; a non-object (or `null`) `args` is handled as an empty object,
/// mirroring pi's `extractToolCallSummaries` guard (`utils.ts:316-318`) that coerces a
/// non-object/array `arguments` to `{}` before formatting.
#[must_use]
pub fn format_tool_call(name: &str, args: &serde_json::Value, expanded: bool) -> String {
    match name {
        "bash" => {
            let command = args.get("command").and_then(serde_json::Value::as_str).unwrap_or("");
            let max_length = if expanded { 240 } else { 60 };
            format!("$ {}", truncate_with_ellipsis(command, max_length))
        }
        "read" | "write" | "edit" => {
            let target = args
                .get("path")
                .and_then(serde_json::Value::as_str)
                .or_else(|| args.get("file_path").and_then(serde_json::Value::as_str))
                .unwrap_or("");
            format!("{name} {}", shorten_path(target))
        }
        _ => {
            // pi `JSON.stringify(args)` — for a non-object `args`, pi already coerced it to `{}`
            // upstream, so serialize an empty object in that case rather than the raw scalar.
            let serialized = if args.is_object() {
                serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
            } else {
                "{}".to_string()
            };
            let max_length = if expanded { 160 } else { 40 };
            format!("{name} {}", truncate_with_ellipsis(&serialized, max_length))
        }
    }
}

/// pi `extractToolArgsPreview` (`pi-subagents/src/shared/utils.ts:521-573`): the SHORT argument
/// preview pi stores on `AgentProgress.currentToolArgs` and then copies verbatim onto each
/// `recentTools[].args` entry (`runs/foreground/execution.ts:794,807`). Distinct from
/// [`format_tool_call`], which renders `<tool> <args>` for a transcript row; this renders the
/// arguments ALONE for a live activity line.
///
/// The cascade is pi's, in pi's order: MCP `{server, tool, args}` first, then `queries[]`/`query`/
/// `workflow`, then `url`/`urls[]`/`prompt`, then the fixed `previewKeys` list, then a final
/// `key=value` fallback over the first string/array-valued member.
///
/// **[CYRUP-DELTA]** the final fallback's iteration order is `serde_json::Map`'s (alphabetical by
/// key, since this workspace builds `serde_json` without `preserve_order`) where pi's is JS object
/// insertion order. Every earlier rung of the cascade is keyed by explicit name and therefore
/// order-independent, so this only shows up for an args object whose members are ALL unlisted keys
/// and which has more than one string member.
#[must_use]
pub fn extract_tool_args_preview(args: &serde_json::Value) -> String {
    let Some(map) = args.as_object() else {
        return String::new();
    };

    // pi `stringifyPreviewValue` (`utils.ts:526-530`): a non-blank string, or a number/boolean
    // rendered as a string; anything else is "no preview".
    fn stringify_preview_value(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    // pi `previewArray` (`utils.ts:532-538`): the first element's preview plus an `(+N more)`
    // suffix when the array carries more than one entry.
    fn preview_array(value: Option<&serde_json::Value>) -> Option<String> {
        let items = value?.as_array()?;
        let first = stringify_preview_value(items.first()?)?;
        let suffix = if items.len() > 1 {
            format!(" (+{} more)", items.len() - 1)
        } else {
            String::new()
        };
        Some(format!("{first}{suffix}"))
    }

    // pi `truncatePreview` (`utils.ts:522-523`): `slice(0, maxLength - 3) + "..."`, i.e. the
    // ellipsis is INSIDE the budget (unlike `truncate_with_ellipsis`, which appends past it).
    fn truncate_preview(value: &str, max_length: usize) -> String {
        if value.chars().count() <= max_length {
            return value.to_string();
        }
        let keep = max_length.saturating_sub(3);
        let head: String = value.chars().take(keep).collect();
        format!("{head}...")
    }

    let str_field = |key: &str| -> Option<&str> {
        map.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
    };

    // MCP tool calls: `<server>/<tool> <args>` (`utils.ts:541-546`). Note pi's guard here is
    // `args.tool && typeof args.tool === "string"`, i.e. a NON-EMPTY string (JS falsiness), so an
    // empty `tool` falls through to the rest of the cascade.
    if let Some(tool) = map.get("tool").and_then(serde_json::Value::as_str)
        && !tool.is_empty()
    {
        let server = map
            .get("server")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| format!("{s}/"))
            .unwrap_or_default();
        let tool_args = map
            .get("args")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| format!(" {}", s.chars().take(40).collect::<String>()))
            .unwrap_or_default();
        return format!("{server}{tool}{tool_args}");
    }

    if let Some(preview) = preview_array(map.get("queries")) {
        return truncate_preview(&preview, 60);
    }
    if let Some(query) = str_field("query") {
        return truncate_preview(query, 60);
    }
    if let Some(workflow) = str_field("workflow") {
        return format!("workflow={}", truncate_preview(workflow, 48));
    }
    if let Some(url) = str_field("url") {
        return truncate_preview(url, 60);
    }
    if let Some(preview) = preview_array(map.get("urls")) {
        return truncate_preview(&preview, 60);
    }
    if let Some(prompt) = str_field("prompt") {
        return truncate_preview(prompt, 60);
    }

    // pi `previewKeys` (`utils.ts:555`), in pi's own order. Note pi's guard here is
    // `args[key] && typeof args[key] === "string"` — non-EMPTY, but NOT trim-checked, so a
    // whitespace-only value wins this rung where it would have lost the named rungs above.
    for key in ["command", "path", "file_path", "pattern", "query", "url", "task", "describe", "search"]
    {
        if let Some(value) = map.get(key).and_then(serde_json::Value::as_str)
            && !value.is_empty()
        {
            return truncate_preview(value, 60);
        }
    }

    // Fallback: the first array- or string-valued member, rendered `key=value` (`utils.ts:564-571`).
    for (key, value) in map {
        if let Some(preview) = preview_array(Some(value)) {
            return format!("{key}={}", truncate_preview(&preview, 50));
        }
        if let Some(text) = value.as_str()
            && !text.is_empty()
        {
            return format!("{key}={}", truncate_preview(text, 50));
        }
    }
    String::new()
}

/// pi `shortenPath` (`formatters.ts:127-133`): replace a leading `$HOME` prefix with `~`.
#[must_use]
pub fn shorten_path(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME")
        && let Some(home) = home.to_str()
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}

/// pi's `s.slice(0, maxLength) + (s.length > maxLength ? "..." : "")` — truncate to `max_length`
/// **characters** (not bytes) with a trailing `...` only when the string was actually longer.
fn truncate_with_ellipsis(s: &str, max_length: usize) -> String {
    for (char_count, (byte_idx, _)) in s.char_indices().enumerate() {
        if char_count == max_length {
            // `s` has MORE than `max_length` chars — cut at this char boundary and append `...`.
            let head = s.get(..byte_idx).unwrap_or(s);
            return format!("{head}...");
        }
    }
    // `s` had `max_length` or fewer chars — return it unchanged (no ellipsis).
    s.to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn bash_call_previews_the_command_with_per_mode_length_caps() {
        let args = serde_json::json!({ "command": "ls -la" });
        let summary = ToolCallSummary::from_call("bash", &args);
        assert_eq!(summary.text, "$ ls -la");
        assert_eq!(summary.expanded_text, "$ ls -la");
    }

    #[test]
    fn bash_call_truncates_the_short_preview_at_sixty_chars_but_not_the_expanded_one() {
        let command = "echo ".to_string() + &"x".repeat(300);
        let args = serde_json::json!({ "command": command });
        let text = format_tool_call("bash", &args, false);
        let expanded = format_tool_call("bash", &args, true);
        // "$ " + 60 chars + "..."
        assert!(text.starts_with("$ "));
        assert!(text.ends_with("..."));
        assert_eq!(text.chars().count(), 2 + 60 + 3);
        // expanded caps at 240
        assert!(expanded.ends_with("..."));
        assert_eq!(expanded.chars().count(), 2 + 240 + 3);
    }

    #[test]
    fn read_write_edit_calls_preview_the_path_argument() {
        let args = serde_json::json!({ "path": "/etc/hosts" });
        assert_eq!(format_tool_call("read", &args, false), "read /etc/hosts");
        let args2 = serde_json::json!({ "file_path": "/tmp/out.txt" });
        assert_eq!(format_tool_call("edit", &args2, false), "edit /tmp/out.txt");
    }

    #[test]
    fn edit_call_with_no_path_argument_renders_the_name_with_an_empty_target() {
        // Matches pi: `${name} ${shortenPath("")}` == "edit " (trailing space).
        let args = serde_json::Value::Null;
        assert_eq!(format_tool_call("edit", &args, false), "edit ");
    }

    #[test]
    fn unknown_tool_call_previews_the_json_arguments_truncated() {
        let args = serde_json::json!({ "a": 1, "b": "two" });
        let text = format_tool_call("custom", &args, false);
        assert!(text.starts_with("custom "), "got: {text}");
        assert!(text.contains("\"a\":1"), "got: {text}");
    }

    #[test]
    fn unknown_tool_call_with_non_object_args_serializes_an_empty_object() {
        let args = serde_json::json!("just a string");
        assert_eq!(format_tool_call("custom", &args, false), "custom {}");
    }

    #[test]
    fn shorten_path_replaces_the_home_prefix_with_tilde() {
        // Only assert the tilde-substitution when HOME is actually set in this test environment.
        if let Some(home) = std::env::var_os("HOME").and_then(|h| h.into_string().ok())
            && !home.is_empty()
        {
            let full = format!("{home}/projects/x");
            assert_eq!(shorten_path(&full), "~/projects/x");
        }
        // A path with no home prefix is returned unchanged regardless.
        assert_eq!(shorten_path("/no/home/here"), "/no/home/here");
    }
}
