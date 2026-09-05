//! SUBA-074 stage 2 — the `claude-code` / `claude-code-writer` adapter
//! (`pi-subagents/src/runs/shared/claude-code-adapter.ts` @v0.64.0).
//!
//! This is the first adapter this crate ships, and it was chosen because it is the only one of the
//! three whose launch needs NEITHER a final-message artifact read back from disk (codex-exec's
//! `--output-last-message`) NOR a prompt-file temp directory with `--add-dir` handling
//! (cursor-agent). Its parser is short and its delivery is plain stdin, so the runner's harder
//! paths are designed in — [`super::super::prompt::PromptDelivery::PromptFile`], the launch's
//! `final_output_path`, the [`AfterTerminal`] policy — while only the simple arm is exercised.

use serde_json::Value;

use crate::exec::external_cli::framing::{
    AfterTerminal, ParserProgress, ParserTerminal, parse_external_cli_jsonl_event,
};
use crate::runner::contract::AdapterId;

/// `MAX_EVENT_TYPE_LENGTH` (`claude-code-adapter.ts:4`).
const MAX_EVENT_TYPE_LENGTH: usize = 128;
/// `MAX_ERROR_LENGTH` (`:5`).
const MAX_ERROR_LENGTH: usize = 4_096;
/// `CLAUDE_CODE_WRITER_TOOLS` (`:9`) — the writer profile's entire tool surface, as one CSV
/// argument. Five read/write file tools; no bash, no web, no MCP.
pub const CLAUDE_CODE_WRITER_TOOLS: &str = "Read,Write,Edit,Glob,Grep";

/// `CLAUDE_CODE_ENV_ALLOWLIST` (`:10-43`) — the 32 keys the foreign process may see.
///
/// The list is the sandbox: everything else in the orchestrator's environment — this crate's
/// subagent permission policy, its capability-ceiling and tool-budget encodings, its
/// structured-output capture paths, and every credential held for another provider — is absent from
/// the child by construction. `CLAUDE_CONFIG_DIR` is on the list deliberately: the adapter's whole
/// authentication story is "use the CLI's existing login".
pub const CLAUDE_CODE_ENV_ALLOWLIST: [&str; 32] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "CLAUDE_CONFIG_DIR",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CLOUD_ML_REGION",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// `resolveClaudeCodeLaunch(input).args` (`:96-111`).
///
/// Every flag is load-bearing for the sandbox and none is optional: `--permission-mode` is the
/// access ceiling, `--tools` is the tool surface (EMPTY for the read-only profile), and
/// `--strict-mcp-config --mcp-config {"mcpServers":{}}` is what stops the foreign agent inheriting
/// the user's MCP servers.
#[must_use]
pub fn launch_args(adapter: AdapterId, command_prefix_args: &[String]) -> Vec<String> {
    let writer = adapter == AdapterId::ClaudeCodeWriter;
    let mut args: Vec<String> = command_prefix_args.to_vec();
    for arg in [
        "-p",
        "--input-format",
        "text",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        if writer { "acceptEdits" } else { "plan" },
        "--tools",
        if writer { CLAUDE_CODE_WRITER_TOOLS } else { "" },
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--setting-sources",
        "user",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--no-chrome",
    ] {
        args.push(arg.to_string());
    }
    args
}

/// The fourteen strings `--help` must document (`:122`), the seventh of which differs between the
/// read-only and writer profiles — a build of the CLI that does not document the permission mode
/// this adapter is about to request is not the build the adapter was written against.
#[must_use]
pub fn required_help(adapter: AdapterId) -> Vec<String> {
    let writer = adapter == AdapterId::ClaudeCodeWriter;
    [
        "Claude Code - starts an interactive session",
        "--print",
        "--input-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        if writer { "acceptEdits" } else { "plan" },
        "--tools",
        "--strict-mcp-config",
        "--mcp-config",
        "--setting-sources",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--no-chrome",
    ]
    .iter()
    .map(|value| (*value).to_string())
    .collect()
}

/// `/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)? \(Claude Code\)$/` (`:121`).
///
/// [CYRUP-DELTA] hand-rolled rather than compiled: this crate carries no regex dependency, and the
/// pattern is a semver core plus an optional pre-release/build tail plus a fixed suffix.
///
/// # Errors
///
/// Upstream's `Unsupported Claude Code version response: <json>.`
pub fn validate_version(version: &str) -> Result<(), String> {
    const SUFFIX: &str = " (Claude Code)";
    let refuse = || {
        Err(format!(
            "Unsupported Claude Code version response: {}.",
            Value::String(version.to_string())
        ))
    };
    let Some(core) = version.strip_suffix(SUFFIX) else {
        return refuse();
    };
    // `[-+][0-9A-Za-z.-]+` — the optional tail, split off at the FIRST `-` or `+` after the core.
    let (numeric, tail) = match core.find(['-', '+']) {
        Some(index) => (&core[..index], Some(&core[index + 1..])),
        None => (core, None),
    };
    if let Some(tail) = tail
        && (tail.is_empty()
            || !tail
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
    {
        return refuse();
    }
    let segments: Vec<&str> = numeric.split('.').collect();
    if segments.len() != 3
        || segments
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        return refuse();
    }
    Ok(())
}

/// `createClaudeCodeJsonlParser()` (`:57-78`).
#[derive(Debug, Default)]
pub struct ClaudeCodeParser {
    event_count: u64,
    terminal: Option<ParserTerminal>,
}

impl ClaudeCodeParser {
    /// This adapter's after-terminal policy. Claude Code keeps STREAMING after its `result` event
    /// and only a second `result` is a protocol error (`:63`) — unlike codex-exec and cursor-agent,
    /// which reject any post-terminal event at all.
    pub const AFTER_TERMINAL: AfterTerminal = AfterTerminal::RejectDuplicateTerminal;

    /// A fresh parser.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `parseLine(line)` (`:61-73`).
    ///
    /// # Errors
    ///
    /// The framing refusals from [`parse_external_cli_jsonl_event`], plus `Claude Code emitted a
    /// duplicate terminal result.`
    pub fn parse_line(&mut self, line: &str) -> Result<ParserProgress, String> {
        let event = parse_external_cli_jsonl_event(line, "Claude Code", MAX_EVENT_TYPE_LENGTH)?;
        let is_result = event.get("type").and_then(Value::as_str) == Some("result");
        if self.terminal.is_some() && is_result {
            return Err("Claude Code emitted a duplicate terminal result.".to_string());
        }
        self.event_count += 1;
        if self.terminal.is_none() && is_result {
            let success = event.get("subtype").and_then(Value::as_str) == Some("success")
                && event.get("is_error") == Some(&Value::Bool(false));
            let result_text = event
                .get("result")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty());
            self.terminal = Some(match (success, result_text) {
                (true, Some(text)) => ParserTerminal {
                    completed: true,
                    output: Some(text.to_string()),
                    error: None,
                },
                _ => ParserTerminal {
                    completed: false,
                    output: None,
                    error: Some(terminal_error(&event)),
                },
            });
        }
        Ok(ParserProgress {
            phase: self
                .terminal
                .as_ref()
                .map_or("streaming", ParserTerminal::state)
                .to_string(),
            event_count: self.event_count,
        })
    }

    /// `finish()` (`:74-76`).
    #[must_use]
    pub fn finish(&mut self) -> Option<ParserTerminal> {
        self.terminal.clone()
    }
}

/// `terminalError(event)` (`:45-55`) — the failure text for a non-success `result`, in upstream's
/// own precedence: `error`, then `result`, then a joined `errors[]`, then a subtype sentence.
///
/// [CYRUP-DELTA] upstream's `.slice(0, MAX_ERROR_LENGTH)` counts UTF-16 code units; this truncates
/// on a char boundary at the same count of `char`s, which is the nearest Rust equivalent that
/// cannot split a codepoint.
fn terminal_error(event: &serde_json::Map<String, Value>) -> String {
    let truncate = |text: &str| -> String { text.chars().take(MAX_ERROR_LENGTH).collect() };
    for key in ["error", "result"] {
        if let Some(text) = event
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return truncate(text);
        }
    }
    if let Some(Value::Array(items)) = event.get("errors") {
        let messages: Vec<&str> = items
            .iter()
            .filter_map(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .collect();
        if !messages.is_empty() {
            return truncate(&messages.join("; "));
        }
    }
    let subtype = event
        .get("subtype")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    format!("Claude Code reported terminal result {subtype}.")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// The read-only profile's argv is the sandbox: plan mode, an EMPTY tool list, and a strict
    /// empty MCP config (`:96-111`).
    #[test]
    fn the_read_only_profile_requests_plan_mode_no_tools_and_no_mcp() {
        let args = launch_args(AdapterId::ClaudeCode, &[]);
        assert_eq!(
            args,
            vec![
                "-p",
                "--input-format",
                "text",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "plan",
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                r#"{"mcpServers":{}}"#,
                "--setting-sources",
                "user",
                "--no-session-persistence",
                "--disable-slash-commands",
                "--no-chrome",
            ]
        );
    }

    /// The writer profile differs in exactly two argv slots — the permission mode and the tool CSV
    /// — and in nothing else.
    #[test]
    fn the_writer_profile_differs_only_in_permission_mode_and_tools() {
        let read_only = launch_args(AdapterId::ClaudeCode, &[]);
        let writer = launch_args(AdapterId::ClaudeCodeWriter, &[]);
        let differing: Vec<usize> = read_only
            .iter()
            .zip(&writer)
            .enumerate()
            .filter_map(|(index, (a, b))| (a != b).then_some(index))
            .collect();
        assert_eq!(differing, vec![7, 9]);
        assert_eq!(writer[7], "acceptEdits");
        assert_eq!(writer[9], CLAUDE_CODE_WRITER_TOOLS);
    }

    /// The test seam: a command prefix goes in FRONT of the adapter's own argv (`:96-98`), so an
    /// end-to-end test can point `command` at an interpreter and still get the real flags.
    #[test]
    fn a_command_prefix_precedes_the_adapters_own_argv() {
        let args = launch_args(AdapterId::ClaudeCode, &["/tmp/fake.sh".to_string()]);
        assert_eq!(args[0], "/tmp/fake.sh");
        assert_eq!(args[1], "-p");
    }

    /// The 32-key allowlist, pinned by count and by the keys that carry a credential — a key added
    /// or dropped here changes what the foreign process can see.
    #[test]
    fn the_env_allowlist_is_upstreams_thirty_two_keys() {
        assert_eq!(CLAUDE_CODE_ENV_ALLOWLIST.len(), 32);
        for required in [
            "PATH",
            "HOME",
            "CLAUDE_CONFIG_DIR",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "AWS_BEARER_TOKEN_BEDROCK",
            "SSL_CERT_DIR",
        ] {
            assert!(CLAUDE_CODE_ENV_ALLOWLIST.contains(&required), "{required}");
        }
        for forbidden in [
            "CYRUP_SUBAGENT_PERMISSION_POLICY",
            "CYRUP_SUBAGENT_BINARY",
            "OPENAI_API_KEY",
        ] {
            assert!(
                !CLAUDE_CODE_ENV_ALLOWLIST.contains(&forbidden),
                "{forbidden}"
            );
        }
    }

    /// The version pattern accepts a semver core with an optional pre-release/build tail and the
    /// fixed product suffix, and refuses everything else (`:121`).
    #[test]
    fn the_version_response_must_be_semver_plus_the_product_suffix() {
        for accepted in [
            "1.2.3 (Claude Code)",
            "0.0.1 (Claude Code)",
            "1.2.3-beta.1 (Claude Code)",
            "1.2.3+build.5 (Claude Code)",
        ] {
            assert!(validate_version(accepted).is_ok(), "{accepted}");
        }
        for refused in [
            "1.2 (Claude Code)",
            "1.2.3",
            "1.2.3 (Claude Code) extra",
            "v1.2.3 (Claude Code)",
            "1.2.3- (Claude Code)",
            "1.2.3-bad_tail (Claude Code)",
        ] {
            assert!(validate_version(refused).is_err(), "{refused}");
        }
        assert_eq!(
            validate_version("nope").unwrap_err(),
            "Unsupported Claude Code version response: \"nope\"."
        );
    }

    /// The happy path: a `result` event with `subtype:"success"`, `is_error:false` and a non-blank
    /// `result` string is the terminal output (`:65-67`).
    #[test]
    fn a_successful_result_event_is_the_terminal_output() {
        let mut parser = ClaudeCodeParser::new();
        let progress = parser
            .parse_line(r#"{"type":"system","subtype":"init"}"#)
            .unwrap();
        assert_eq!(progress.phase, "streaming");
        assert_eq!(progress.event_count, 1);
        let progress = parser
            .parse_line(
                r#"{"type":"result","subtype":"success","is_error":false,"result":"  done  "}"#,
            )
            .unwrap();
        assert_eq!(progress.phase, "completed");
        assert_eq!(
            parser.finish(),
            Some(ParserTerminal {
                completed: true,
                output: Some("done".to_string()),
                error: None
            })
        );
    }

    /// A non-success `result` is a FAILURE, and the failure text follows upstream's precedence
    /// (`:45-55`).
    #[test]
    fn a_failed_result_takes_its_message_from_error_then_result_then_errors_then_subtype() {
        let failure = |line: &str| {
            let mut parser = ClaudeCodeParser::new();
            parser.parse_line(line).unwrap();
            parser.finish().unwrap().error.unwrap()
        };
        assert_eq!(
            failure(r#"{"type":"result","subtype":"error","error":" boom "}"#),
            "boom"
        );
        assert_eq!(
            failure(r#"{"type":"result","subtype":"error","result":"partial"}"#),
            "partial"
        );
        assert_eq!(
            failure(r#"{"type":"result","subtype":"error","errors":["a","","b"]}"#),
            "a; b"
        );
        assert_eq!(
            failure(r#"{"type":"result","subtype":"error_max_turns"}"#),
            "Claude Code reported terminal result error_max_turns."
        );
        assert_eq!(
            failure(r#"{"type":"result"}"#),
            "Claude Code reported terminal result unknown."
        );
        // `is_error` must be literally `false`; a success subtype with a missing flag still fails.
        assert_eq!(
            failure(r#"{"type":"result","subtype":"success","result":"x"}"#),
            "x"
        );
    }

    /// Claude Code's after-terminal policy: NON-result events keep streaming past the terminal, and
    /// only a SECOND `result` is a protocol error (`:63`).
    #[test]
    fn only_a_duplicate_result_is_a_protocol_error() {
        assert_eq!(
            ClaudeCodeParser::AFTER_TERMINAL,
            AfterTerminal::RejectDuplicateTerminal
        );
        let mut parser = ClaudeCodeParser::new();
        parser
            .parse_line(r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#)
            .unwrap();
        let progress = parser.parse_line(r#"{"type":"assistant"}"#).unwrap();
        assert_eq!(
            progress.event_count, 2,
            "a non-result event after the terminal keeps counting"
        );
        assert_eq!(
            parser
                .parse_line(r#"{"type":"result","subtype":"success"}"#)
                .unwrap_err(),
            "Claude Code emitted a duplicate terminal result."
        );
        assert_eq!(
            parser.finish().unwrap().output,
            Some("ok".to_string()),
            "the FIRST terminal wins"
        );
    }

    /// A parser that never saw a `result` has no terminal at all, which the runner treats as a
    /// protocol failure (`external-cli-runner.ts:371-372`).
    #[test]
    fn a_stream_with_no_result_event_produces_no_terminal() {
        let mut parser = ClaudeCodeParser::new();
        parser.parse_line(r#"{"type":"assistant"}"#).unwrap();
        assert_eq!(parser.finish(), None);
    }

    /// The fourteen required help strings, with the permission mode varying by profile (`:122`).
    #[test]
    fn the_required_help_strings_name_every_flag_the_argv_uses() {
        let read_only = required_help(AdapterId::ClaudeCode);
        assert_eq!(read_only.len(), 14);
        assert!(read_only.contains(&"plan".to_string()));
        assert!(read_only.contains(&"--strict-mcp-config".to_string()));
        let writer = required_help(AdapterId::ClaudeCodeWriter);
        assert!(writer.contains(&"acceptEdits".to_string()));
        assert!(!writer.contains(&"plan".to_string()));
    }
}
