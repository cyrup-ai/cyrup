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
