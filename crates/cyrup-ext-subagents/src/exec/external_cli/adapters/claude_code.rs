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
use crate::exec::model_scope::{ModelSource, ResolvedModelScope, check_model_scope};
use crate::exec::spawn_plan::split_known_thinking_suffix;
use crate::runner::contract::AdapterId;
use crate::watchdog::model_selection::THINKING_LEVELS;

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

/// SUBA-167 — `CLAUDE_CODE_EFFORT_BY_THINKING` (`claude-code-adapter.ts:53-66` @v0.76.1): Claude
/// Code's five-value `--effort` scale. Pi's wider thinking vocabulary collapses onto the nearest
/// supported value (`minimal` → `low`), and `off` means "pass no flag at all". `None` for anything
/// that is not a [`THINKING_LEVELS`] entry, which the resolver has already refused.
#[must_use]
pub fn claude_code_effort_by_thinking(level: &str) -> Option<&'static str> {
    match level {
        "minimal" | "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" => Some("max"),
        _ => None,
    }
}

/// SUBA-167 — `CLAUDE_CODE_MODEL_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:/-]*$/` (`:68-74` @v0.76.1):
/// aliases, `claude-*` ids, and Bedrock/Vertex prefixes. `:` stays legal because Bedrock inference
/// profiles and ARNs contain it, so the thinking suffix is split off BEFORE this is applied.
///
/// [CYRUP-DELTA] hand-rolled rather than compiled, for the same reason as [`validate_version`].
/// The pattern is what keeps every model token ONE argv element that cannot read as a flag: a
/// leading `-` and any whitespace are both refused.
fn is_valid_claude_code_model(model: &str) -> bool {
    let mut chars = model.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
}

/// `INVALID_CLAUDE_CODE_MODEL` (`:76` @v0.76.1).
const INVALID_CLAUDE_CODE_MODEL: &str =
    "expected an alias such as \"opus\" or a model id such as \"claude-opus-5.5\"";

/// SUBA-167 — `ClaudeCodeOverride` (`:78-83` @v0.76.1): the tokens appended after the adapter's
/// fixed argv, and the model id the launch pins when it pins one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodeOverride {
    /// Tokens appended after the adapter's fixed argv (`--model <id>`, `--effort <level>`).
    pub args: Vec<String>,
    /// Model id the launch pins, when it pins one — what an enforced `modelScope` is checked
    /// against ([`assert_claude_code_model_scope`]).
    pub model: Option<String>,
}

/// SUBA-167 — the inputs of `resolveClaudeCodeOverride` (`:132-139` @v0.76.1).
///
/// `agent_model`/`agent_model_is_settings_default` are upstream's structural
/// `ClaudeCodeAgentModel` (`:86-89`): cyrup's
/// [`crate::discovery::types::AgentModelSourceInfo::SettingsDefault`] is stamped only when the
/// model WAS the `subagents.defaultModel` fill, which is upstream's
/// `modelSource.type === "subagents.defaultModel" && modelSource.model === model` in one bit.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeCodeOverrideInput<'a> {
    /// The launch's model as the caller typed it (pre-inheritance, `:suffix` intact).
    pub model: Option<&'a str>,
    /// The agent's own resolved `model`.
    pub agent_model: Option<&'a str>,
    /// Whether [`Self::agent_model`] is the `subagents.defaultModel` fill.
    pub agent_model_is_settings_default: bool,
    /// The requested thinking level (caller param, else the agent's `thinking`).
    pub thinking: Option<&'a str>,
    /// The effective `maxThinking` ceiling.
    pub thinking_ceiling: Option<&'a str>,
    /// The agent name the ceiling refusal names.
    pub agent_name: Option<&'a str>,
    /// The run id the ceiling refusal names.
    pub run_id: Option<&'a str>,
}

/// `agentPinnedModel(agent)` (`:91-100` @v0.76.1). `subagents.defaultModel` is a Pi child model,
/// so it is a default for Pi children only and never becomes `--model`.
fn agent_pinned_model(model: Option<&str>, is_settings_default: bool) -> Option<&str> {
    model.filter(|model| !model.is_empty() && !is_settings_default)
}

/// `combineClaudeCodeModel(launch, pinned)` (`:102-114` @v0.76.1): a launch that asks only for a
/// level keeps the agent's pinned model, so `model: ":high"` means the same thing as frontmatter
/// `thinking: high`.
fn combine_claude_code_model(launch: Option<&str>, pinned: Option<&str>) -> Option<String> {
    let Some(launch) = launch else {
        return pinned.map(str::to_string);
    };
    let (base_model, thinking_suffix) = split_known_thinking_suffix(launch);
    if !base_model.is_empty() || launch.is_empty() {
        return Some(launch.to_string());
    }
    // An empty base means the request was a bare level: run it on the agent's model.
    let pinned_base = pinned.map_or("", |pinned| split_known_thinking_suffix(pinned).0);
    if !pinned_base.is_empty() && !thinking_suffix.is_empty() {
        Some(format!("{pinned_base}{thinking_suffix}"))
    } else {
        Some(launch.to_string())
    }
}

/// SUBA-167 — `resolveClaudeCodeOverride(input)` (`:116-172` @v0.76.1): resolve an explicit model
/// and thinking level for a code-owned Claude Code adapter. `Ok(None)` when neither was requested,
/// so the CLI falls back to its own configured default.
///
/// The level travels as the same `:level` suffix Pi children use: `claude-opus-5.5:high` pins the
/// model and the effort, a bare `:high` takes the effort alone on the agent's own model, and a
/// suffix wins over `thinking`. Every returned token is its own argv element, so an unusable value
/// is a rejected launch rather than a silently different model. The ceiling is checked against the
/// REQUESTED level, not the effort it maps to.
///
/// # Errors
///
/// Upstream's three refusals, verbatim: `Invalid Claude Code model <json>; …`, `Invalid thinking
/// level <json>; expected one of …`, and the shared `Thinking level '<l>' exceeds configured
/// maximum '<c>'…` from [`crate::exec::thinking_ceiling::assert_thinking_within_ceiling`].
pub fn resolve_claude_code_override(
    input: ClaudeCodeOverrideInput<'_>,
) -> Result<Option<ClaudeCodeOverride>, String> {
    let mut args: Vec<String> = Vec::new();
    let mut model: Option<String> = None;
    let mut level: Option<String> = None;
    let requested = combine_claude_code_model(
        input.model.map(str::trim),
        agent_pinned_model(input.agent_model, input.agent_model_is_settings_default),
    );
    if let Some(requested) = requested.as_deref() {
        let invalid = || {
            format!(
                "Invalid Claude Code model {}; {INVALID_CLAUDE_CODE_MODEL}.",
                Value::String(requested.to_string())
            )
        };
        let (base_model, thinking_suffix) = split_known_thinking_suffix(requested);
        level = thinking_suffix.strip_prefix(':').map(str::to_string);
        if !base_model.is_empty() {
            if !is_valid_claude_code_model(base_model) {
                return Err(invalid());
            }
            model = Some(base_model.to_string());
            args.push("--model".to_string());
            args.push(base_model.to_string());
        } else if level.is_none() {
            return Err(invalid());
        }
    }
    if level.is_none()
        && let Some(raw) = input.thinking
    {
        let thinking = raw.trim();
        if !THINKING_LEVELS.contains(&thinking) {
            return Err(format!(
                "Invalid thinking level {}; expected one of {}.",
                Value::String(raw.to_string()),
                THINKING_LEVELS.join(", ")
            ));
        }
        level = Some(thinking.to_string());
    }
    if let Some(level) = level.as_deref() {
        // A level can arrive without a model, so it goes to the shared check as the suffix form
        // that check already understands (`:164-166`).
        crate::exec::thinking_ceiling::assert_thinking_within_ceiling(
            Some(&format!(":{level}")),
            None,
            input.thinking_ceiling,
            input.agent_name,
            input.run_id,
        )?;
        if let Some(effort) = claude_code_effort_by_thinking(level) {
            args.push("--effort".to_string());
            args.push(effort.to_string());
        }
    }
    if args.is_empty() {
        return Ok(None);
    }
    Ok(Some(ClaudeCodeOverride { args, model }))
}

/// SUBA-167 — `assertClaudeCodeModelScope(input)` (`:174-191` @v0.76.1). A Claude Code launch pins
/// a model the Pi registry does not know, so an enforced model scope can only be honored by
/// checking the id the CLI will actually run. A launch that pins nothing cannot be checked at all,
/// and an enforced scope must not be bypassed by staying silent. The agent's own pinned model is
/// checked as EXPLICIT too, so a violation is always an error.
///
/// # Errors
///
/// Upstream's `… does not name a Claude Code model …` refusal, or the first scope violation's
/// message.
pub fn assert_claude_code_model_scope(
    scopes: &[ResolvedModelScope],
    model: Option<&str>,
    agent: &str,
    run_id: Option<&str>,
) -> Result<(), String> {
    let enforced: Vec<&ResolvedModelScope> = scopes
        .iter()
        .filter(|scope| scope.enforce == Some(true))
        .collect();
    if enforced.is_empty() {
        return Ok(());
    }
    let Some(model) = model.filter(|model| !model.is_empty()) else {
        let subject = match run_id {
            Some(run_id) => format!("agent '{agent}' run '{run_id}'"),
            None => format!("agent '{agent}'"),
        };
        return Err(format!(
            "{subject} does not name a Claude Code model, so it cannot be checked against an enforced subagent model scope (modelScope). Name the model on the launch, or in the agent's frontmatter, or turn enforcement off."
        ));
    };
    for scope in enforced {
        if let Some(violation) = check_model_scope(Some(model), scope, ModelSource::Explicit) {
            return Err(violation.message);
        }
    }
    Ok(())
}

/// SUBA-167 — `assertClaudeCodeOverrideIsLocal(agent, machine, override)`
/// (`runs/background/async-execution.ts:891-896` @v0.76.1): a pinned Claude Code model cannot
/// survive pane-native saved-machine placement, which owns the remote model registry, so the
/// combination fails instead of losing both flags.
///
/// # Errors
///
/// Upstream's refusal, verbatim, when both a machine and an override are present.
pub fn assert_claude_code_override_is_local(
    agent: &str,
    machine: Option<&str>,
    claude_code_override: Option<&ClaudeCodeOverride>,
) -> Result<(), String> {
    let (Some(machine), Some(_)) = (machine, claude_code_override) else {
        return Ok(());
    };
    Err(format!(
        "Agent '{agent}' requested machine '{machine}', but a Claude Code model or thinking level cannot be honored on a saved machine. Remove the model or thinking request, or run the agent locally."
    ))
}

/// `resolveClaudeCodeLaunch(input).args` (`:96-111`).
///
/// Every flag is load-bearing for the sandbox and none is optional: `--permission-mode` is the
/// access ceiling, `--tools` is the tool surface (EMPTY for the read-only profile), and
/// `--strict-mcp-config --mcp-config {"mcpServers":{}}` is what stops the foreign agent inheriting
/// the user's MCP servers.
///
/// SUBA-167 — `override_args` ([`resolve_claude_code_override`]'s tokens) go LAST, after
/// `--no-chrome` (`:261-263` @v0.76.1): "Session options go last: preflight builds
/// versionArgs/helpArgs from `prefix` only, so these tokens never reach the --version/--help
/// probes."
#[must_use]
pub fn launch_args(
    adapter: AdapterId,
    command_prefix_args: &[String],
    override_args: &[String],
) -> Vec<String> {
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
    args.extend_from_slice(override_args);
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
        let args = launch_args(AdapterId::ClaudeCode, &[], &[]);
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
        let read_only = launch_args(AdapterId::ClaudeCode, &[], &[]);
        let writer = launch_args(AdapterId::ClaudeCodeWriter, &[], &[]);
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
        let args = launch_args(AdapterId::ClaudeCode, &["/tmp/fake.sh".to_string()], &[]);
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

    /// An override vector's result, as `(args, model)`.
    type Resolved = Result<Option<(Vec<String>, Option<String>)>, String>;

    fn resolve(input: ClaudeCodeOverrideInput<'_>) -> Resolved {
        resolve_claude_code_override(input)
            .map(|resolved| resolved.map(|resolved| (resolved.args, resolved.model)))
    }

    /// The expected `Ok(Some(..))` for [`resolve`].
    fn ok(args: &[&str], model: Option<&str>) -> Resolved {
        Ok(Some((
            args.iter().map(|arg| (*arg).to_string()).collect(),
            model.map(str::to_string),
        )))
    }

    fn model(model: &str) -> ClaudeCodeOverrideInput<'_> {
        ClaudeCodeOverrideInput {
            model: Some(model),
            ..ClaudeCodeOverrideInput::default()
        }
    }

    fn thinking(level: &str) -> ClaudeCodeOverrideInput<'_> {
        ClaudeCodeOverrideInput {
            thinking: Some(level),
            ..ClaudeCodeOverrideInput::default()
        }
    }

    /// SUBA-167 — upstream `derives the model and the effort from the model suffix`
    /// (`test/unit/claude-code-adapter.test.ts:235-249` @v0.76.1).
    #[test]
    fn the_override_derives_model_and_effort_from_the_suffix() {
        assert_eq!(resolve(ClaudeCodeOverrideInput::default()), Ok(None));
        assert_eq!(
            resolve(model("claude-opus-5.5")),
            ok(&["--model", "claude-opus-5.5"], Some("claude-opus-5.5"))
        );
        assert_eq!(
            resolve(model(" sonnet ")),
            ok(&["--model", "sonnet"], Some("sonnet"))
        );
        assert_eq!(
            resolve(model("claude-opus-5.5:high")),
            ok(
                &["--model", "claude-opus-5.5", "--effort", "high"],
                Some("claude-opus-5.5")
            )
        );
        // A bare ":level" asks for the effort without pinning a model.
        assert_eq!(resolve(model(":medium")), ok(&["--effort", "medium"], None));
        // The suffix wins over the frontmatter value, which is the Pi child precedence.
        assert_eq!(
            resolve(ClaudeCodeOverrideInput {
                model: Some("opus:low"),
                thinking: Some("max"),
                ..ClaudeCodeOverrideInput::default()
            }),
            ok(&["--model", "opus", "--effort", "low"], Some("opus"))
        );
        assert_eq!(
            resolve(thinking("medium")),
            ok(&["--effort", "medium"], None)
        );
        // Pi's wider thinking scale collapses onto Claude Code's five-value effort scale.
        assert_eq!(resolve(thinking("off")), Ok(None));
        assert_eq!(
            resolve(model("haiku:minimal")),
            ok(&["--model", "haiku", "--effort", "low"], Some("haiku"))
        );
        assert_eq!(resolve(thinking("xhigh")), ok(&["--effort", "xhigh"], None));
    }

    /// SUBA-167 — upstream `runs a bare level on the agent's own model and keeps a Pi default out
    /// of --model` (`:251-263` @v0.76.1).
    #[test]
    fn a_bare_level_runs_on_the_agents_model_and_a_settings_default_never_becomes_model() {
        let pinned = |launch: Option<&'static str>| ClaudeCodeOverrideInput {
            model: launch,
            agent_model: Some("claude-opus-5.5"),
            ..ClaudeCodeOverrideInput::default()
        };
        // A bare level means the agent's model at that effort, so the two forms agree.
        assert_eq!(
            resolve(pinned(Some(":high"))),
            ok(
                &["--model", "claude-opus-5.5", "--effort", "high"],
                Some("claude-opus-5.5")
            )
        );
        assert_eq!(
            resolve(pinned(None)),
            ok(&["--model", "claude-opus-5.5"], Some("claude-opus-5.5"))
        );
        // A launch model replaces the agent's model entirely.
        assert_eq!(
            resolve(pinned(Some("haiku:low"))),
            ok(&["--model", "haiku", "--effort", "low"], Some("haiku"))
        );
        // subagents.defaultModel is a Pi child model, so the CLI never sees it.
        let from_settings = ClaudeCodeOverrideInput {
            agent_model: Some("anthropic/claude-sonnet-4-5"),
            agent_model_is_settings_default: true,
            ..ClaudeCodeOverrideInput::default()
        };
        assert_eq!(resolve(from_settings), Ok(None));
        assert_eq!(
            resolve(ClaudeCodeOverrideInput {
                thinking: Some("low"),
                ..from_settings
            }),
            ok(&["--effort", "low"], None)
        );
        // Upstream's `:262` vector (a frontmatter model beside a `defaultModel` provenance naming
        // a DIFFERENT model stays pinned) has no separate cyrup shape: discovery stamps
        // `SettingsDefault` only when the fill IS the model, so it is the `pinned(None)` case above.
    }

    /// SUBA-167 — upstream `rejects a model or thinking value it cannot pass as its own argv
    /// element` (`:265-278` @v0.76.1), with the full text pinned for one case of each.
    #[test]
    fn the_override_rejects_values_it_cannot_pass_as_one_argv_element() {
        for refused in [
            "",
            ":",
            ":turbo",
            "--dangerously-skip-permissions",
            "opus --tools Bash",
        ] {
            let error = resolve(model(refused)).unwrap_err();
            assert!(
                error.starts_with("Invalid Claude Code model "),
                "{refused:?}: {error}"
            );
        }
        assert_eq!(
            resolve(model("opus --tools Bash")).unwrap_err(),
            "Invalid Claude Code model \"opus --tools Bash\"; expected an alias such as \"opus\" \
             or a model id such as \"claude-opus-5.5\"."
        );
        for refused in ["turbo", "off; rm -rf /"] {
            let error = resolve(thinking(refused)).unwrap_err();
            assert!(
                error.starts_with("Invalid thinking level "),
                "{refused:?}: {error}"
            );
        }
        assert_eq!(
            resolve(thinking("off; rm -rf /")).unwrap_err(),
            "Invalid thinking level \"off; rm -rf /\"; expected one of off, minimal, low, medium, \
             high, xhigh, max."
        );
        // A Bedrock inference profile keeps its colon, because "0" is not a level.
        let bedrock = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";
        assert_eq!(
            resolve(model(bedrock)),
            ok(&["--model", bedrock], Some(bedrock))
        );
    }

    /// SUBA-167 — upstream `rejects an effort above the ceiling and keeps the boundary level`
    /// (`:280-291` @v0.76.1): the ceiling compares the REQUESTED level, not the effort it maps to.
    #[test]
    fn the_ceiling_checks_the_requested_level_not_the_effort() {
        let under = |launch: &'static str, ceiling: &'static str| ClaudeCodeOverrideInput {
            model: Some(launch),
            thinking_ceiling: Some(ceiling),
            ..ClaudeCodeOverrideInput::default()
        };
        assert_eq!(
            resolve(ClaudeCodeOverrideInput {
                agent_name: Some("cc"),
                run_id: Some("run-1"),
                ..under("sonnet:max", "low")
            })
            .unwrap_err(),
            "Thinking level 'max' exceeds configured maximum 'low' for agent 'cc' run 'run-1'."
        );
        // "minimal" stays legal under a "minimal" ceiling although it emits low.
        assert_eq!(
            resolve(under("sonnet:minimal", "minimal")),
            ok(&["--model", "sonnet", "--effort", "low"], Some("sonnet"))
        );
        assert_eq!(
            resolve(under("sonnet:max", "max")),
            ok(&["--model", "sonnet", "--effort", "max"], Some("sonnet"))
        );
        // No requested level means no ceiling to check.
        assert_eq!(
            resolve(under("sonnet", "low")),
            ok(&["--model", "sonnet"], Some("sonnet"))
        );
    }

    /// SUBA-167 — `assertClaudeCodeModelScope` (`claude-code-adapter.ts:174-191` @v0.76.1): only an
    /// ENFORCED scope counts, a launch naming no model cannot be checked and is refused, and the
    /// agent's own pinned model is checked as explicit — so it errors, not warns.
    #[test]
    fn an_enforced_scope_refuses_an_unnamed_or_out_of_scope_claude_model() {
        let scope = |enforce: Option<bool>| ResolvedModelScope {
            enforce,
            strict: None,
            allow: vec!["claude-opus-*".to_string()],
            origin: "modelScope".to_string(),
        };
        let enforced = [scope(Some(true))];
        assert_eq!(
            assert_claude_code_model_scope(&enforced, None, "cc", Some("run-1")).unwrap_err(),
            "agent 'cc' run 'run-1' does not name a Claude Code model, so it cannot be checked \
             against an enforced subagent model scope (modelScope). Name the model on the launch, \
             or in the agent's frontmatter, or turn enforcement off."
        );
        assert!(
            assert_claude_code_model_scope(&enforced, None, "cc", None)
                .unwrap_err()
                .starts_with("agent 'cc' does not name")
        );
        assert_eq!(
            assert_claude_code_model_scope(&enforced, Some("haiku"), "cc", None).unwrap_err(),
            "Model 'haiku' is outside the configured subagent model scope (modelScope). Allowed \
             patterns: claude-opus-*."
        );
        assert!(
            assert_claude_code_model_scope(&enforced, Some("claude-opus-5.5"), "cc", None).is_ok()
        );
        // A scope that is not enforced is a no-op, model or not.
        for idle in [scope(None), scope(Some(false))] {
            assert!(
                assert_claude_code_model_scope(std::slice::from_ref(&idle), None, "cc", None)
                    .is_ok()
            );
            assert!(assert_claude_code_model_scope(&[idle], Some("haiku"), "cc", None).is_ok());
        }
    }

    /// SUBA-167 — `assertClaudeCodeOverrideIsLocal` (`async-execution.ts:891-896` @v0.76.1).
    #[test]
    fn a_claude_code_override_cannot_run_on_a_saved_machine() {
        let resolved = ClaudeCodeOverride {
            args: vec!["--effort".to_string(), "high".to_string()],
            model: None,
        };
        assert_eq!(
            assert_claude_code_override_is_local("cc", Some("lab"), Some(&resolved)).unwrap_err(),
            "Agent 'cc' requested machine 'lab', but a Claude Code model or thinking level cannot \
             be honored on a saved machine. Remove the model or thinking request, or run the agent \
             locally."
        );
        assert!(assert_claude_code_override_is_local("cc", None, Some(&resolved)).is_ok());
        assert!(assert_claude_code_override_is_local("cc", Some("lab"), None).is_ok());
    }
}
