//! SUBA-045 — the child tool-availability diagnostic: a 1:1 port of
//! `pi-subagents/src/runs/shared/tool-availability.ts` @v0.43.0, brought to v0.71.0.
//!
//! # The failure this exists to name
//!
//! An agent's `tools:` list is a strict ALLOWLIST handed to the child on its command line. It does
//! not load anything. So when it names a tool the child's host never registers — an MCP server that
//! is down, a tool that was renamed, an extension whose provider path was never threaded — the
//! child starts perfectly happily, the model is told it may call a tool that does not exist, and
//! the run finishes with a model apology instead of a diagnosis. Nothing in the transcript says
//! "that tool was never there".
//!
//! Upstream turns exactly that case into the run's terminal error text. The mechanism is a
//! three-hop handshake, and all three hops are ported here:
//!
//! 1. **Parent, at spawn** (`pi-args.ts:610-616`): beside `REQUIRED_CHILD_TOOLS_ENV` it writes
//!    [`CHILD_TOOL_DIAGNOSTIC_PATH_ENV`] pointing at `tool-diagnostic.json` inside the attempt's
//!    private temp dir, and [`MCP_DIRECT_CHILD_TOOLS_ENV`] carrying the RESOLVED direct-MCP names.
//!    Both are written only when the required list is non-empty, exactly as upstream gates them.
//! 2. **Child, at `agent_start`** (`subagent-prompt-runtime.ts:496-501` @v0.71.0): it diffs the
//!    required list against its own live registry and writes [`write_child_tool_diagnostic`]'s 0600
//!    JSON — or DELETES the file when nothing is missing, so the file's mere existence is the
//!    signal — and, when something IS missing, REFUSES the run: upstream throws
//!    `formatChildToolDiagnostic(diagnostic)` from that handler (#1356, v0.55.0), and cyrup's child
//!    aborts its own in-flight run from the same hook (`prompt_runtime`'s
//!    `refresh_tool_diagnostic`), before its first model call.
//! 3. **Parent, at settle** (`foreground/execution.ts:1072-1079`,
//!    `background/subagent-runner.ts:1442`): `closeError = result.error ?? toolDiagnosticError ??
//!    assistantError`, so a missing tool outranks the model's own apology as the run's error.
//!
//! # No builtin floor
//!
//! The v0.43.0 port unioned pi's `PI_CORE_CHILD_TOOLS` (`bash, edit, find, grep, ls, read, write`)
//! into the available set before the diff. Upstream deleted that floor in #1356 (`51cca33e`,
//! v0.55.0, "fail when child write tools are unavailable"): `evaluateChildToolDiagnostic`
//! (`child-runtime-config.ts:123-137` @v0.71.0) diffs against the registry ALONE. It is also what
//! makes SUBA-114 safe — once the parent stopped predicting a child's builtins from its own
//! registry, this check is the one place a child whose builtin menu genuinely differs is caught,
//! and a floor would have waved exactly those seven names through.

use std::path::{Path, PathBuf};

/// pi `CHILD_TOOL_DIAGNOSTIC_PATH_ENV` (`tool-availability.ts:6`, `PI_SUBAGENT_TOOL_DIAGNOSTIC_PATH`)
/// under this crate's `CYRUP_SUBAGENT_*` rename.
pub const CHILD_TOOL_DIAGNOSTIC_PATH_ENV: &str = "CYRUP_SUBAGENT_TOOL_DIAGNOSTIC_PATH";

/// pi `MCP_DIRECT_CHILD_TOOLS_ENV` (`tool-availability.ts:5`, `PI_SUBAGENT_MCP_DIRECT_TOOLS`) under
/// the same rename.
///
/// Distinct from `exec::MCP_DIRECT_TOOLS_ENV` (`MCP_DIRECT_TOOLS`), which upstream keeps
/// un-namespaced because it is the MCP ADAPTER's own allowlist input (`pi-args.ts:216-220`). This
/// one is the subagent runtime's, carries the RESOLVED tool names rather than the `mcp:` selectors,
/// and exists only so the diagnostic can say which of the missing names came from MCP.
pub const MCP_DIRECT_CHILD_TOOLS_ENV: &str = "CYRUP_SUBAGENT_MCP_DIRECT_TOOLS";

/// The file name the parent gives the diagnostic inside the attempt's temp dir
/// (pi `path.join(tempDir, "tool-diagnostic.json")`, `pi-args.ts:614`).
pub const CHILD_TOOL_DIAGNOSTIC_FILE: &str = "tool-diagnostic.json";

/// pi `ChildToolDiagnostic` (`tool-availability.ts:8-14`).
///
/// `agent` and `missing_mcp_direct_tools` are both `skip_serializing_if` because upstream spreads
/// them in conditionally (`...(missingMcpDirectTools.length > 0 ? … : {})`), and the reader below
/// distinguishes "absent" from "empty" the same way.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildToolDiagnostic {
    /// The child agent's name (pi `process.env[SUBAGENT_CHILD_AGENT_ENV]`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The allowlist the parent required.
    pub required: Vec<String>,
    /// What the child's registry actually had, raw.
    pub available: Vec<String>,
    /// `required` minus `available`.
    pub missing: Vec<String>,
    /// The subset of `missing` that came from resolved direct-MCP selectors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_mcp_direct_tools: Option<Vec<String>>,
}

/// pi `writeChildToolDiagnostic` (`tool-availability.ts:18-44` @v0.54.0; since #1844 upstream's
/// in-process `evaluateChildToolDiagnostic`, `child-runtime-config.ts:123-137` @v0.71.0), child-side.
///
/// Returns `Some(diagnostic)` and writes the 0600 JSON when something is missing; returns `None`
/// and REMOVES any stale file when nothing is. The removal is load-bearing and is upstream's
/// `fs.rmSync(filePath, { force: true })`: the parent's only test is whether the file exists, so a
/// diagnostic left behind by an earlier attempt of the same run would fail the NEXT one.
pub fn write_child_tool_diagnostic(
    file_path: &Path,
    required: &[String],
    available: &[String],
    agent: Option<&str>,
    mcp_direct_tools: Option<&[String]>,
) -> Option<ChildToolDiagnostic> {
    let available_names: std::collections::HashSet<&str> =
        available.iter().map(String::as_str).collect();
    let missing: Vec<String> = required
        .iter()
        .filter(|name| !available_names.contains(name.as_str()))
        .cloned()
        .collect();
    if missing.is_empty() {
        let _ = std::fs::remove_file(file_path);
        return None;
    }

    // pi `mcpDirectTools?.length ? missing.filter(...) : []`, then the conditional spread — so an
    // empty result is an ABSENT key, never `[]`.
    let missing_mcp: Vec<String> = mcp_direct_tools
        .filter(|tools| !tools.is_empty())
        .map(|tools| {
            missing
                .iter()
                .filter(|name| tools.iter().any(|t| t == *name))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let diagnostic = ChildToolDiagnostic {
        agent: agent.map(str::to_string),
        required: required.to_vec(),
        available: available.to_vec(),
        missing,
        missing_mcp_direct_tools: (!missing_mcp.is_empty()).then_some(missing_mcp),
    };
    if let Some(parent) = file_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(bytes) = serde_json::to_vec(&diagnostic) else {
        return Some(diagnostic);
    };
    if std::fs::write(file_path, bytes).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(file_path, std::fs::Permissions::from_mode(0o600));
        }
    }
    Some(diagnostic)
}

/// pi `readChildToolDiagnostic` (`tool-availability.ts:47-61`), parent-side.
///
/// `Ok(None)` is upstream's `undefined` for "no path configured, or no file" — the ordinary,
/// everything-was-present case. `Err` is upstream's `throw new Error("Malformed child tool
/// diagnostic at '…'.")`, which the caller below converts into its own reported text rather than
/// swallowing: a diagnostic file that cannot be parsed is itself worth surfacing.
///
/// # Errors
/// Upstream's malformed-file message when the JSON is unreadable, or when `required`/`available`/
/// `missing` are not arrays of non-empty strings.
pub fn read_child_tool_diagnostic(
    file_path: Option<&Path>,
) -> Result<Option<ChildToolDiagnostic>, String> {
    let Some(file_path) = file_path else {
        return Ok(None);
    };
    if !file_path.exists() {
        return Ok(None);
    }
    let malformed = || {
        format!(
            "Malformed child tool diagnostic at '{}'.",
            file_path.display()
        )
    };
    let bytes = std::fs::read(file_path).map_err(|_| malformed())?;
    let parsed: ChildToolDiagnostic = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
    // pi's `stringArray` guard: every entry must be a non-empty string. serde has already enforced
    // "array of strings"; the remaining half of upstream's predicate is `entry.length > 0`.
    let non_empty = |names: &[String]| names.iter().all(|name| !name.is_empty());
    if !non_empty(&parsed.required)
        || !non_empty(&parsed.available)
        || !non_empty(&parsed.missing)
        || parsed
            .missing_mcp_direct_tools
            .as_deref()
            .is_some_and(|names| !non_empty(names))
    {
        return Err(malformed());
    }
    Ok(Some(parsed))
}

/// pi `formatChildToolDiagnostic` (`tool-availability.ts:15-37` @v0.71.0, the runner/default arm)
/// — five lines, verbatim, with the MCP line present only when that key is.
///
/// Upstream's other arm (`host: "parent"`, "ran as a foreground child, which never loads the
/// parent's ambient extensions") describes an IN-PROCESS foreground child. A cyrup child is
/// always its own `cyrup` process that loads its own ambient extensions, so that arm's claim would
/// be false here and only this one is ported.
#[must_use]
pub fn format_child_tool_diagnostic(diagnostic: &ChildToolDiagnostic) -> String {
    let subject = match diagnostic.agent.as_deref() {
        Some(agent) if !agent.is_empty() => format!("Agent '{agent}'"),
        _ => "Subagent".to_string(),
    };
    let mut lines = vec![
        format!(
            "{subject} requested unavailable child tools: {}.",
            diagnostic.missing.join(", ")
        ),
        "The `tools` field is a strict allowlist; it does not load extension code.".to_string(),
    ];
    if let Some(mcp) = diagnostic
        .missing_mcp_direct_tools
        .as_deref()
        .filter(|names| !names.is_empty())
    {
        lines.push(format!(
            "Resolved MCP direct tools missing from the child registry: {}. Resolved names must \
             match what the host or pi-mcp-adapter registers; check the MCP direct-tool \
             registration before treating this as a tool-call failure.",
            mcp.join(", ")
        ));
    }
    lines.push(
        "For extension tools, add the provider path to `subagentOnlyExtensions` (child-only), \
         `extensions`, or as a path-like entry in `tools`, while keeping each registered tool name \
         in `tools`."
            .to_string(),
    );
    lines.push(
        "For MCP tools, verify the MCP adapter configuration and selected tool names. For builtin \
         tools, verify the name against the installed Pi version."
            .to_string(),
    );
    lines.join("\n")
}

/// pi `readChildToolDiagnosticError` (`tool-availability.ts:77-84`): the whole parent-side read as
/// one `Option<String>`, with the malformed case reported rather than thrown.
#[must_use]
pub fn read_child_tool_diagnostic_error(file_path: Option<&Path>) -> Option<String> {
    match read_child_tool_diagnostic(file_path) {
        Ok(Some(diagnostic)) => Some(format_child_tool_diagnostic(&diagnostic)),
        Ok(None) => None,
        Err(message) => Some(format!(
            "Failed to read child tool availability diagnostic: {message}"
        )),
    }
}

/// The parent-side path this attempt hands the child (pi `path.join(tempDir,
/// "tool-diagnostic.json")`, `pi-args.ts:614`).
#[must_use]
pub fn tool_diagnostic_path_in(temp_dir: &Path) -> PathBuf {
    temp_dir.join(CHILD_TOOL_DIAGNOSTIC_FILE)
}

/// pi `readMcpDirectChildTools` (`subagent-prompt-runtime.ts:85-95`), child-side: the JSON array
/// from [`MCP_DIRECT_CHILD_TOOLS_ENV`], or `None` for absent/blank/malformed/non-string-bearing —
/// upstream swallows every failure here because the value only enriches the message.
#[must_use]
pub fn read_mcp_direct_child_tools(get: &dyn Fn(&str) -> Option<String>) -> Option<Vec<String>> {
    let raw = get(MCP_DIRECT_CHILD_TOOLS_ENV)?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed: Vec<String> = serde_json::from_str(raw).ok()?;
    if parsed.iter().any(String::is_empty) {
        return None;
    }
    Some(parsed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// The whole point of the module: a required tool the child's registry never had becomes a
    /// file, and one that IS there does not.
    ///
    /// The present-case leg asserts the file is ABSENT — so it is written first, then the
    /// everything-present call is made over the same path, which is the stale-file case upstream's
    /// `rmSync(..., { force: true })` exists for. Asserting absence over a path nothing ever wrote
    /// would have passed vacuously.
    #[test]
    fn a_missing_tool_writes_the_diagnostic_and_a_present_one_removes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tool_diagnostic_path_in(dir.path());

        let written = write_child_tool_diagnostic(
            &path,
            &names(&["read", "mcp__nonexistent__x"]),
            &names(&["read"]),
            Some("researcher"),
            None,
        )
        .expect("a missing tool must produce a diagnostic");
        assert_eq!(written.missing, names(&["mcp__nonexistent__x"]));
        assert!(path.exists(), "the diagnostic file must be on disk");

        // Same path, nothing missing now: the stale file must go, because the parent's only test is
        // existence.
        assert!(
            write_child_tool_diagnostic(&path, &names(&["read"]), &names(&["read"]), None, None)
                .is_none()
        );
        assert!(
            !path.exists(),
            "a stale diagnostic must be removed, not left"
        );
    }

    /// No builtin floor (pi #1356, v0.55.0): a child registry without `bash`/`edit` — the "fork
    /// host whose builtin menu differs" upstream's SUBA-114 commit relies on this check to catch —
    /// is reported like any other missing tool. The v0.43.0 floor used to wave these through.
    #[test]
    fn a_missing_core_builtin_is_reported_there_is_no_floor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tool_diagnostic_path_in(dir.path());
        let diagnostic = write_child_tool_diagnostic(
            &path,
            &names(&["read", "bash", "edit"]),
            &names(&["read", "subagent"]),
            Some("worker"),
            None,
        )
        .expect("a registry without bash/edit must be reported");
        assert_eq!(diagnostic.missing, names(&["bash", "edit"]));
        assert_eq!(
            format_child_tool_diagnostic(&diagnostic).lines().next(),
            Some("Agent 'worker' requested unavailable child tools: bash, edit.")
        );
        assert!(path.exists());
    }

    /// The MCP half: `missingMcpDirectTools` is the intersection of `missing` with the resolved
    /// direct-MCP names, it is an ABSENT key when that intersection is empty (upstream's
    /// conditional spread, not `[]`), and its presence adds upstream's extra MCP registration line
    /// to the formatted text.
    #[test]
    fn the_mcp_subset_is_intersected_omitted_when_empty_and_drives_the_extra_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tool_diagnostic_path_in(dir.path());

        let with_mcp = write_child_tool_diagnostic(
            &path,
            &names(&["mcp__srv__a", "ext_tool"]),
            &[],
            Some("worker"),
            Some(&names(&["mcp__srv__a", "mcp__srv__b"])),
        )
        .expect("two missing tools");
        assert_eq!(
            with_mcp.missing_mcp_direct_tools,
            Some(names(&["mcp__srv__a"])),
            "only the MISSING mcp name belongs in the subset"
        );
        let text = format_child_tool_diagnostic(&with_mcp);
        assert!(
            text.starts_with(
                "Agent 'worker' requested unavailable child tools: mcp__srv__a, ext_tool."
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "Resolved MCP direct tools missing from the child registry: mcp__srv__a. Resolved \
                 names must match what the host or pi-mcp-adapter registers; check the MCP \
                 direct-tool registration before treating this as a tool-call failure."
            ),
            "{text}"
        );

        // Round-trip: what the child wrote is what the parent reads and formats.
        let read = read_child_tool_diagnostic(Some(&path))
            .expect("well-formed")
            .expect("present");
        assert_eq!(read, with_mcp);
        assert_eq!(
            read_child_tool_diagnostic_error(Some(&path)).as_deref(),
            Some(text.as_str())
        );

        // No MCP overlap: the key is absent and the extra line is gone.
        let no_mcp = write_child_tool_diagnostic(
            &path,
            &names(&["ext_tool"]),
            &[],
            None,
            Some(&names(&["mcp__srv__a"])),
        )
        .expect("one missing tool");
        assert_eq!(no_mcp.missing_mcp_direct_tools, None);
        let text = format_child_tool_diagnostic(&no_mcp);
        assert!(
            text.starts_with("Subagent requested unavailable child tools: ext_tool."),
            "{text}"
        );
        assert!(!text.contains("pi-mcp-adapter"), "{text}");
        let raw = std::fs::read_to_string(&path).expect("readable");
        assert!(
            !raw.contains("missingMcpDirectTools"),
            "an empty subset is an ABSENT key: {raw}"
        );
    }

    /// No path configured, and a configured path with no file, are both "nothing to report" —
    /// while a corrupt file is reported rather than swallowed.
    #[test]
    fn absent_is_silent_and_malformed_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tool_diagnostic_path_in(dir.path());
        assert_eq!(read_child_tool_diagnostic_error(None), None);
        assert_eq!(read_child_tool_diagnostic_error(Some(&path)), None);

        std::fs::write(
            &path,
            b"{\"required\": [\"\"], \"available\": [], \"missing\": [\"x\"]}",
        )
        .expect("write");
        let reported =
            read_child_tool_diagnostic_error(Some(&path)).expect("malformed is reported");
        assert!(
            reported.starts_with("Failed to read child tool availability diagnostic: Malformed child tool diagnostic at "),
            "{reported}"
        );
    }

    #[test]
    fn the_mcp_child_env_reader_swallows_every_bad_shape() {
        let get = |value: Option<&str>| {
            let value = value.map(str::to_string);
            move |key: &str| {
                if key == MCP_DIRECT_CHILD_TOOLS_ENV {
                    value.clone()
                } else {
                    None
                }
            }
        };
        assert_eq!(read_mcp_direct_child_tools(&get(None)), None);
        assert_eq!(read_mcp_direct_child_tools(&get(Some("  "))), None);
        assert_eq!(read_mcp_direct_child_tools(&get(Some("not json"))), None);
        assert_eq!(read_mcp_direct_child_tools(&get(Some("[1,2]"))), None);
        assert_eq!(
            read_mcp_direct_child_tools(&get(Some("[\"a\",\"\"]"))),
            None
        );
        assert_eq!(
            read_mcp_direct_child_tools(&get(Some("[\"a\",\"b\"]"))),
            Some(names(&["a", "b"]))
        );
    }
}
