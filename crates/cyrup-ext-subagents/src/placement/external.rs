//! The six code-owned external CLI profiles placed on a Herdr saved machine — pi-subagents'
//! `src/runs/shared/herdr-external-adapters.ts` @v0.68.0, driven the way
//! `src/runs/background/subagent-runner.ts:900-944` drives it.
//!
//! Upstream's shape, kept: a run-private remote runtime directory; a remote preflight that proves
//! the CANONICAL vendor binary and its version floor and help surface (`runHerdrExternalPreflight`
//! / `validateHerdrExternalPreflight`); a fresh herdr-owned pane with an EMPTY environment
//! (credentials stay machine-owned); `agent.start` with the adapter's own sandbox argv; one
//! `agent.prompt`; settlement read back from the pane as sanitized terminal text, marked
//! `[best-effort/unverified]` because nothing here can prove what the vendor TUI did. Every run
//! settles as upstream's `partial` outcome (exit 1, output present): the output is evidence, not a
//! verified success. Anything uncertain — a trust prompt, an approval, identity drift, a timeout —
//! RETAINS the pane for the operator ("truthful pane retained for inspection"); only an explicit
//! stop and an exact settlement are destructive.
//!
//! **Reconnect** (`HerdrExternalSession.reconnect` / `#performReconnect`, `:160-178`): when the
//! herdr forward is lost while the prompt, the settle-time process proof or a Codex snapshot is in
//! flight, the run reconnects — at most three candidates inside fifteen seconds — proving the SAME
//! herdr session, exactly one agent with the started terminal id, in the SAME pane, and (once
//! known) the same canonical native pid, before adopting the new connection and retrying. A
//! transport error while the forward is still up is not a loss upstream either: it stays the
//! needs-attention outcome. Past the budget the run fails "Pane-native external reconnect remains
//! unknown: …" with its pane retained.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use cyrup_herdr::HerdrClient;
use cyrup_herdr::remote::{
    MachineConnection, REMOTE_COMMAND_TIMEOUT, SshTransport, remote_shell_command,
    shell_quote_remote,
};
use cyrup_herdr::schema::{
    AgentInfo, AgentPromptParams, AgentPromptWaitOptions, AgentStartParams, AgentStatus,
    PaneReadParams, ReadSource,
};
use regex::Regex;

use super::HerdrMachineReference;
use super::native::{OwnedPane, safe_run_id, validate_run_id};
use crate::runner::contract::AdapterId;

/// `MAX_PREFLIGHT_BYTES` (`herdr-external-adapters.ts:28`).
pub const MAX_PREFLIGHT_BYTES: usize = 64 * 1024;
/// `CODEX_OUTPUT_BYTES` — the bound on the settled terminal text (`:29`).
pub const CODEX_OUTPUT_BYTES: usize = 64 * 1024;
/// `EXTERNAL_STARTUP_TIMEOUT_MS` (`:29`).
pub const EXTERNAL_STARTUP_TIMEOUT: Duration = Duration::from_secs(45);
/// `EXTERNAL_STARTUP_BACKOFF_AFTER_MS` (`:29`).
pub const EXTERNAL_STARTUP_BACKOFF_AFTER: Duration = Duration::from_secs(15);
/// `EXTERNAL_STARTUP_SLOW_POLL_MS` (`:29`).
pub const EXTERNAL_STARTUP_SLOW_POLL: Duration = Duration::from_millis(2_500);
/// `CODEX_STARTUP_GRACE_MS` (`:29`).
pub const CODEX_STARTUP_GRACE: Duration = Duration::from_secs(15);
/// `CODEX_QUIET_MS` (`:29`).
pub const CODEX_QUIET: Duration = Duration::from_secs(8);
/// `CODEX_POLL_MS` (`:29`).
pub const CODEX_POLL: Duration = Duration::from_secs(1);
/// `CODEX_DEFAULT_TIMEOUT_MS` (`:29`).
pub const CODEX_DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// `promptAndSettle`'s default wait (`:110`, `options.timeoutMs ?? 90_000`).
pub const PROMPT_WAIT_DEFAULT: Duration = Duration::from_secs(90);
/// How long a transport error waits for the forward it rode on to be seen exiting before the
/// connection is judged still up (see `ExternalPlacedRun::connection_lost`).
pub const FORWARD_EXIT_SETTLE: Duration = Duration::from_secs(1);

/// `HerdrExternalKind` (`:9`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalKind {
    /// Claude Code.
    Claude,
    /// Cursor Agent.
    Cursor,
    /// Codex.
    Codex,
}

impl ExternalKind {
    /// `adapterKind(adapter)` (`:50`).
    #[must_use]
    pub fn of(adapter: AdapterId) -> Self {
        match adapter {
            AdapterId::ClaudeCode | AdapterId::ClaudeCodeWriter => Self::Claude,
            AdapterId::CursorAgent | AdapterId::CursorAgentWriter => Self::Cursor,
            AdapterId::CodexExec | AdapterId::CodexExecWriter => Self::Codex,
        }
    }

    /// herdr's agent kind — the `kind` of `agent.start`.
    #[must_use]
    pub const fn herdr_kind(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
        }
    }

    /// `adapterBinary(adapter)` (`:51`).
    #[must_use]
    pub const fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor-agent",
            Self::Codex => "codex",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Cursor => "Cursor",
            Self::Codex => "Codex",
        }
    }
}

/// A failure that retains the pane for inspection (`HerdrExternalNeedsAttentionError`, `:82`), or
/// one that does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalError {
    /// Settle this as upstream settles `HerdrExternalNeedsAttentionError`: retain the pane and
    /// report the sentence as the run's output.
    NeedsAttention(String),
    /// Any other failure.
    Failed(String),
    /// A herdr request whose transport failed (anything but an answered error envelope —
    /// upstream's `HerdrTransportError`, as distinct from `HerdrRpcError`). Settled as a failure;
    /// the callers that can recover from it reconnect first.
    Transport(String),
}

impl ExternalError {
    /// The sentence.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::NeedsAttention(message) | Self::Failed(message) | Self::Transport(message) => {
                message
            }
        }
    }
}

/// A herdr request's failure: an answered refusal is an ordinary failure, anything else lost the
/// transport.
fn herdr_failure(error: cyrup_herdr::HerdrError) -> ExternalError {
    match error {
        cyrup_herdr::HerdrError::Api { .. } => ExternalError::Failed(error.to_string()),
        other => ExternalError::Transport(other.to_string()),
    }
}

impl From<String> for ExternalError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

/// `HerdrExternalCommandEvidence` (`:17`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandEvidence {
    /// The canonical binary path.
    pub binary: String,
    /// Its argv.
    pub args: Vec<String>,
    /// Its exit status (`-1` when none).
    pub status: i32,
    /// stdout.
    pub stdout: String,
    /// stderr.
    pub stderr: String,
}

/// `HerdrExternalPreflightEvidence` (`:18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreflightEvidence {
    /// `<binary> --version`.
    pub version: CommandEvidence,
    /// `<binary> --help`.
    pub help: CommandEvidence,
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn validate_command(
    record: &CommandEvidence,
    binary: &str,
    args: &[&str],
    label: &str,
) -> Result<(), String> {
    if !record.binary.starts_with('/')
        || basename(&record.binary) != binary
        || record.args.len() != args.len()
        || record.args.iter().zip(args).any(|(a, b)| a != b)
    {
        return Err(format!("{label} preflight command identity is invalid."));
    }
    if record.status != 0 {
        let head: String = record.stderr.chars().take(512).collect();
        return Err(format!(
            "{label} preflight exited {}: {head}",
            record.status
        ));
    }
    for (name, value) in [("stdout", &record.stdout), ("stderr", &record.stderr)] {
        if value.len() > MAX_PREFLIGHT_BYTES {
            return Err(format!("{label} preflight {name} exceeded its bound."));
        }
    }
    Ok(())
}

/// `requireHelp`'s option test: `(^|\s)OPTION(?=\s|[=,.;:]|$)`, multiline — without a lookahead,
/// which the `regex` crate does not have.
fn help_documents(help: &str, option: &str) -> bool {
    help.match_indices(option).any(|(index, _)| {
        let before_ok = help
            .get(..index)
            .and_then(|head| head.chars().next_back())
            .is_none_or(char::is_whitespace);
        let after = help
            .get(index + option.len()..)
            .and_then(|tail| tail.chars().next());
        let after_ok = after.is_none_or(|c| c.is_whitespace() || "=,.;:".contains(c));
        before_ok && after_ok
    })
}

fn require_help(help: &str, options: &[&str], label: &str) -> Result<(), String> {
    let head = help.trim_start();
    if !["Usage:", "Claude Code", "Codex", "Start the Cursor Agent"]
        .iter()
        .any(|prefix| head.starts_with(prefix))
    {
        return Err(format!("{label} help response has an unsupported header."));
    }
    for option in options {
        if !help_documents(help, option) {
            return Err(format!(
                "{label} help does not document required interactive option {}.",
                serde_json::to_string(option).unwrap_or_default()
            ));
        }
    }
    Ok(())
}

static CLAUDE_VERSION: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(\d+)\.(\d+)\.(\d+) \(Claude Code\)$").ok());
static CODEX_VERSION: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^codex-cli (\d+)\.(\d+)\.(\d+)$").ok());
static CURSOR_VERSION: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(\d{4})\.(\d{2})\.(\d{2})-[0-9a-f]+$").ok());

/// `validateHerdrExternalPreflight(adapter, evidence)` (`:57-64`).
///
/// # Errors
/// Upstream's sentence for each check, in its order.
pub fn validate_external_preflight(
    adapter: AdapterId,
    evidence: &PreflightEvidence,
) -> Result<(), String> {
    let kind = ExternalKind::of(adapter);
    let binary = kind.binary();
    validate_command(&evidence.version, binary, &["--version"], binary)?;
    validate_command(&evidence.help, binary, &["--help"], binary)?;
    if !evidence.version.binary.starts_with('/')
        || evidence.help.binary != evidence.version.binary
        || basename(&evidence.version.binary) != binary
    {
        return Err("External preflight canonical binary identity is inconsistent.".to_string());
    }
    let version = evidence.version.stdout.trim();
    let pattern = match kind {
        ExternalKind::Claude => CLAUDE_VERSION.as_ref(),
        ExternalKind::Codex => CODEX_VERSION.as_ref(),
        ExternalKind::Cursor => CURSOR_VERSION.as_ref(),
    };
    let parsed = pattern
        .and_then(|regex| regex.captures(version))
        .and_then(|caps| {
            let number = |i: usize| caps.get(i).and_then(|m| m.as_str().parse::<u64>().ok());
            Some([number(1)?, number(2)?, number(3)?])
        });
    let Some(parsed) = parsed else {
        return Err(format!(
            "Unsupported {binary} version response: {}.",
            serde_json::to_string(version).unwrap_or_default()
        ));
    };
    let floor = match kind {
        ExternalKind::Claude => [2, 1, 269],
        ExternalKind::Codex => [0, 154, 0],
        ExternalKind::Cursor => [2026, 9, 10],
    };
    if parsed < floor {
        return Err(format!(
            "Remote {binary} is below the tested pane-native capability floor."
        ));
    }
    let options: &[&str] = match kind {
        ExternalKind::Claude => &[
            "--session-id",
            "--restricted",
            "--permission-mode",
            "--tools",
            "--strict-mcp-config",
            "--mcp-config",
            "--disable-slash-commands",
            "--no-chrome",
        ],
        ExternalKind::Cursor => &["--mode", "--sandbox", "--workspace", "--trust"],
        ExternalKind::Codex => &["--sandbox", "--ask-for-approval", "--no-alt-screen"],
    };
    require_help(&evidence.help.stdout, options, binary)
}

/// `HerdrExternalLaunch` (`:12`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalLaunch {
    /// The adapter.
    pub adapter: AdapterId,
    /// Its kind.
    pub kind: ExternalKind,
    /// The native session identity (a v4 UUID).
    pub native_session_id: String,
    /// The argv after the kind's executable.
    pub args: Vec<String>,
}

/// `createHerdrExternalAdapterLaunch(input)` (`:65-79`), for the one input shape upstream's runner
/// ever passes (no environment, no resources, no model).
///
/// # Errors
/// A runtime directory that is not the accepted run-private one, or a Cursor launch without an
/// absolute workspace.
pub fn external_launch(
    adapter: AdapterId,
    remote_runtime_dir: &str,
    cwd: &str,
    native_session_id: Option<String>,
) -> Result<ExternalLaunch, String> {
    if !remote_runtime_dir.starts_with('/') || basename(remote_runtime_dir).len() < 8 {
        return Err(
            "External adapter requires the accepted run-private remote runtime root.".to_string(),
        );
    }
    let native_session_id =
        native_session_id.unwrap_or_else(|| uuid::Uuid::new_v4().hyphenated().to_string());
    let kind = ExternalKind::of(adapter);
    let args: Vec<String> = match adapter {
        AdapterId::ClaudeCode | AdapterId::ClaudeCodeWriter => {
            let writer = adapter == AdapterId::ClaudeCodeWriter;
            vec![
                "--session-id".to_string(),
                native_session_id.clone(),
                "--restricted".to_string(),
                "--permission-mode".to_string(),
                if writer { "acceptEdits" } else { "plan" }.to_string(),
                "--tools".to_string(),
                if writer {
                    "Read,Write,Edit,Glob,Grep"
                } else {
                    ""
                }
                .to_string(),
                "--strict-mcp-config".to_string(),
                "--mcp-config".to_string(),
                "{\"mcpServers\":{}}".to_string(),
                "--disable-slash-commands".to_string(),
                "--no-chrome".to_string(),
            ]
        }
        AdapterId::CursorAgent | AdapterId::CursorAgentWriter => {
            if !cwd.starts_with('/') {
                return Err(
                    "Pane-native Cursor requires the exact remote-absolute owned workspace."
                        .to_string(),
                );
            }
            let mut args = Vec::new();
            if adapter == AdapterId::CursorAgent {
                args.extend(["--mode".to_string(), "ask".to_string()]);
            }
            args.extend([
                "--sandbox".to_string(),
                "enabled".to_string(),
                "--workspace".to_string(),
                cwd.to_string(),
            ]);
            args
        }
        AdapterId::CodexExec | AdapterId::CodexExecWriter => vec![
            "--sandbox".to_string(),
            if adapter == AdapterId::CodexExecWriter {
                "workspace-write"
            } else {
                "read-only"
            }
            .to_string(),
            "--ask-for-approval".to_string(),
            "never".to_string(),
            "--no-alt-screen".to_string(),
        ],
    };
    Ok(ExternalLaunch {
        adapter,
        kind,
        native_session_id,
        args,
    })
}

static ANSI: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]").ok()
});

/// `sanitizeTerminal(input)` (`:83`): strip VT sequences, carriage returns and control characters
/// other than tab/newline, trailing blanks per line, then trim — and keep only the LAST
/// [`CODEX_OUTPUT_BYTES`], on a character boundary.
#[must_use]
pub fn sanitize_terminal(input: &str) -> String {
    let stripped = match ANSI.as_ref() {
        Some(regex) => regex.replace_all(input, "").into_owned(),
        None => input.to_string(),
    };
    let cleaned: String = stripped
        .chars()
        .filter(|&c| c != '\r' && (c == '\t' || c == '\n' || !(c <= '\u{1f}' || c == '\u{7f}')))
        .collect();
    let lines: Vec<&str> = cleaned
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect();
    let clean = lines.join("\n").trim().to_string();
    if clean.len() <= CODEX_OUTPUT_BYTES {
        return clean;
    }
    let mut start = clean.len() - CODEX_OUTPUT_BYTES;
    while !clean.is_char_boundary(start) {
        start += 1;
    }
    clean.get(start..).unwrap_or_default().to_string()
}

fn regex(pattern: &str) -> Option<Regex> {
    Regex::new(pattern).ok()
}

static CODEX_ATTENTION: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(
        r"(?i)(?:trust this|do you trust the contents of this directory|approval required|approve\?|review (?:hook|action)|action required|permission required|error:|fatal:)",
    )
});
static CODEX_WORKING: LazyLock<Option<Regex>> = LazyLock::new(|| {
    regex(r"(?i)(?:working|thinking|running|esc to interrupt|ctrl\+c to interrupt|•\s*[^\n]+…)")
});
static CODEX_READY: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"(?i)(?:›|>)\s*(?:Ask|Describe|Type)\b|(?:Ask|Describe) anything"));
static CURSOR_TRUST: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex(r"(?i)trust (?:this|workspace|folder)|untrusted"));
static CURSOR_READY: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"(?i)(?:Ask|Plan|Agent)\b"));
static TOKENS_USED: LazyLock<Option<Regex>> = LazyLock::new(|| regex(r"(?i)^tokens? used\b"));

fn matches(pattern: &LazyLock<Option<Regex>>, text: &str) -> bool {
    pattern.as_ref().is_some_and(|regex| regex.is_match(text))
}

/// `codexAttention(text)` (`:84`).
#[must_use]
pub fn codex_attention(text: &str) -> bool {
    matches(&CODEX_ATTENTION, text)
}
/// `codexWorking(text)` (`:85`).
#[must_use]
pub fn codex_working(text: &str) -> bool {
    matches(&CODEX_WORKING, text)
}
/// `codexReady(text)` (`:86`).
#[must_use]
pub fn codex_ready(text: &str) -> bool {
    matches(&CODEX_READY, text)
}

/// `codexSubmissionEnd(text, task)` (`:87-94`): the line index after the echoed task, or `None`.
#[must_use]
pub fn codex_submission_end(text: &str, task: &str) -> Option<usize> {
    let lines: Vec<&str> = text.split('\n').map(str::trim_end).collect();
    let task_lines: Vec<&str> = task
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim_end())
        .collect();
    let first_task = task_lines.first().copied().unwrap_or_default();
    for (index, line) in lines.iter().enumerate() {
        let body = line
            .strip_prefix('›')
            .or_else(|| line.strip_prefix('>'))
            .map(|rest| rest.strip_prefix(' ').unwrap_or(rest));
        if body != Some(first_task) {
            continue;
        }
        let rest_matches = task_lines
            .iter()
            .skip(1)
            .enumerate()
            .all(|(offset, task_line)| lines.get(index + offset + 1) == Some(task_line));
        if rest_matches {
            return Some(index + task_lines.len());
        }
    }
    None
}

/// `codexAssistantOutput(text, task)` (`:95-97`).
#[must_use]
pub fn codex_assistant_output(text: &str, task: &str) -> String {
    let Some(end) = codex_submission_end(text, task) else {
        return String::new();
    };
    text.split('\n')
        .map(str::trim_end)
        .skip(end)
        .filter(|line| {
            !line.trim().is_empty() && !codex_ready(line) && !matches(&TOKENS_USED, line)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// `normalizePlacedTerminal(input, output, adapter)`'s output (`:103-106`):
/// `[best-effort/unverified]` over the bounded sanitized text.
///
/// # Errors
/// `"sanitized terminal output is missing."` for empty text.
pub fn normalize_placed_terminal(output: &str) -> Result<String, String> {
    let clean = sanitize_terminal(output);
    if clean.trim().is_empty() {
        return Err("sanitized terminal output is missing.".to_string());
    }
    Ok(format!("[best-effort/unverified]\n{clean}"))
}

/// One Codex terminal sample (`HerdrCodexSnapshot`, `:80`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexSnapshot {
    /// Owning workspace.
    pub workspace_id: String,
    /// Owned pane.
    pub pane_id: String,
    /// Owned terminal.
    pub terminal_id: String,
    /// The retained native pid.
    pub pid: u32,
    /// The pane text.
    pub text: String,
    /// `visible` or `recent_unwrapped`.
    pub source: ReadSource,
}

/// `monitorHerdrCodex(input)` (`:98-102`): sample until the echoed task is followed by stable,
/// quiet, ready, non-working assistant output read from `recent_unwrapped`, past the startup
/// grace — then `stop` and return it. Attention, identity drift or timeout retain the pane.
///
/// # Errors
/// [`ExternalError::NeedsAttention`] with upstream's sentences.
pub async fn monitor_codex<S, F, T, G>(
    task: &str,
    pre_submit_text: &str,
    identity: (&str, &str, &str, u32),
    mut snapshot: S,
    stop: T,
    timeout: Duration,
) -> Result<String, ExternalError>
where
    S: FnMut() -> F,
    F: std::future::Future<Output = Result<CodexSnapshot, ExternalError>>,
    T: FnOnce() -> G,
    G: std::future::Future<Output = ()>,
{
    let (workspace_id, pane_id, terminal_id, pid) = identity;
    let started = tokio::time::Instant::now();
    let deadline = started + timeout;
    let before = sanitize_terminal(pre_submit_text);
    if codex_submission_end(&before, task).is_some() {
        return Err(ExternalError::NeedsAttention(
            "Codex pre-submit screen already contained the task; correlation is ambiguous."
                .to_string(),
        ));
    }
    let mut previous = before;
    let mut previous_source: Option<ReadSource> = None;
    let mut last_change = started;
    let mut meaningful = false;
    let mut stable = 0u32;
    while tokio::time::Instant::now() <= deadline {
        let sample = snapshot().await?;
        if sample.workspace_id != workspace_id
            || sample.pane_id != pane_id
            || sample.terminal_id != terminal_id
            || sample.pid != pid
        {
            return Err(ExternalError::NeedsAttention(
                "Placed Codex identity drifted; truthful pane retained for inspection.".to_string(),
            ));
        }
        let text = sanitize_terminal(&sample.text);
        let source_changed = previous_source.is_some_and(|source| source != sample.source);
        if codex_attention(&text) {
            return Err(ExternalError::NeedsAttention(
                "Placed Codex requires attention; truthful pane retained for inspection."
                    .to_string(),
            ));
        }
        let now = tokio::time::Instant::now();
        if text != previous || source_changed {
            if text != previous {
                meaningful = true;
            }
            previous = text.clone();
            previous_source = Some(sample.source);
            last_change = now;
            stable = 0;
        } else {
            previous_source = Some(sample.source);
            stable += 1;
        }
        let output = codex_assistant_output(&text, task);
        let quiet = now.duration_since(last_change) >= CODEX_QUIET;
        if sample.source == ReadSource::RecentUnwrapped
            && now.duration_since(started) >= CODEX_STARTUP_GRACE
            && meaningful
            && quiet
            && stable >= 1
            && !codex_working(&text)
            && codex_ready(&text)
            && !output.is_empty()
        {
            stop().await;
            return Ok(sanitize_terminal(&output));
        }
        tokio::time::sleep(CODEX_POLL).await;
    }
    Err(ExternalError::NeedsAttention(
        "Placed Codex settlement remained ambiguous until timeout; truthful pane retained for inspection."
            .to_string(),
    ))
}

/// `ownsHerdrPane(agent, terminalId, paneId)` (`herdr-placed-run.ts:53`).
fn owns_pane(agent: &AgentInfo, terminal_id: &str, pane_id: &str) -> bool {
    agent.terminal_id == terminal_id && agent.pane_id == pane_id
}

fn status_word(status: &AgentStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// The owner of one placed external run.
#[derive(Debug)]
pub struct ExternalPlacedRun {
    machine: HerdrMachineReference,
    transport: SshTransport,
    /// The live forward. Behind a lock because a reconnect REPLACES it while a borrowed run is
    /// mid-settle (the Codex monitor holds `&self`); never held across an await.
    connection: std::sync::Mutex<Option<MachineConnection>>,
    /// The herdr session the run was started in — a reconnect must land in the same one.
    session: Option<String>,
    /// The canonical native pid once proven (`#nativePid`).
    native_pid: std::sync::Mutex<Option<u32>>,
    /// `#reconnectCandidatesRemaining` / `#reconnectDeadline` / `#reconnectFailure`.
    reconnect: std::sync::Mutex<ReconnectBudget>,
    run_id: String,
    runtime_dir: String,
    owned: Option<OwnedPane>,
    launch: Option<ExternalLaunch>,
    preflight: Option<PreflightEvidence>,
    agent_name: Option<String>,
    terminal_id: Option<String>,
}

/// `HerdrExternalSession`'s reconnect bookkeeping (`herdr-external-adapters.ts:84`).
#[derive(Debug)]
struct ReconnectBudget {
    remaining: u32,
    deadline: Option<tokio::time::Instant>,
    failure: Option<String>,
}

impl Default for ReconnectBudget {
    fn default() -> Self {
        Self {
            remaining: 3,
            deadline: None,
            failure: None,
        }
    }
}

/// How a placed external run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalPlacedOutcome {
    /// The run's output — `[best-effort/unverified]` terminal evidence, or the needs-attention /
    /// stop / timeout sentence.
    pub output: String,
    /// Upstream's `timedOut`.
    pub timed_out: bool,
    /// Upstream's `stopped`.
    pub stopped: bool,
    /// A failure that is not an outcome (setup could not complete). When set, `output` is empty.
    pub error: Option<String>,
    /// The owned pane, for the receipt.
    pub pane: Option<OwnedPane>,
}

impl ExternalPlacedRun {
    fn client(&self) -> Result<HerdrClient, ExternalError> {
        self.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|connection| connection.client.clone())
            .ok_or_else(|| {
                ExternalError::Failed("Pane-native external owner has no connection.".to_string())
            })
    }

    /// Whether the forward is gone — upstream's `owner.snapshot.connection === "unknown"`, which pi
    /// learns from its subscription socket closing WITH the transport. cyrup observes the forward's
    /// own process, and a transport error can surface a moment before that process has exited
    /// (the socket closes as ssh exits), so a forward still up is watched for
    /// [`FORWARD_EXIT_SETTLE`] before it is judged alive.
    async fn connection_lost(&self) -> bool {
        let deadline = tokio::time::Instant::now() + FORWARD_EXIT_SETTLE;
        loop {
            let lost = self
                .connection
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
                .is_none_or(MachineConnection::is_lost);
            if lost || tokio::time::Instant::now() >= deadline {
                return lost;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// `reconnect()` / `#performReconnect()` (`herdr-external-adapters.ts:160-178`): up to three
    /// candidate connections inside fifteen seconds (the budget spans the run, as upstream's
    /// fields do), each proven to be the same session, terminal, pane and — once known — native
    /// pid before it replaces the lost one.
    async fn reconnect(&self) -> Result<(), ExternalError> {
        let deadline = {
            let mut budget = self
                .reconnect
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(failure) = &budget.failure {
                return Err(ExternalError::Failed(failure.clone()));
            }
            *budget
                .deadline
                .get_or_insert_with(|| tokio::time::Instant::now() + Duration::from_secs(15))
        };
        let mut last = "Reconnect did not run.".to_string();
        loop {
            {
                let mut budget = self
                    .reconnect
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if budget.remaining == 0 || tokio::time::Instant::now() >= deadline {
                    break;
                }
                budget.remaining -= 1;
            }
            match self.reconnect_candidate().await {
                Ok(connection) => {
                    let old = self
                        .connection
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .replace(connection);
                    if let Some(old) = old {
                        old.close().await;
                    }
                    return Ok(());
                }
                Err(error) => last = error,
            }
        }
        let failure = format!("Pane-native external reconnect remains unknown: {last}");
        self.reconnect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failure = Some(failure.clone());
        Err(ExternalError::Failed(failure))
    }

    /// One reconnect candidate: connect, then `reconcileHerdrExternalRun` (`:56`) and the native
    /// pid proof (`:158`).
    async fn reconnect_candidate(&self) -> Result<MachineConnection, String> {
        let connection = self
            .transport
            .connect(&self.machine.target, self.machine.session.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        match self.prove_candidate(&connection).await {
            Ok(()) => Ok(connection),
            Err(error) => {
                connection.close().await;
                Err(error)
            }
        }
    }

    async fn prove_candidate(&self, connection: &MachineConnection) -> Result<(), String> {
        if connection.endpoint.session != self.session {
            return Err("Herdr session changed during reconnect.".to_string());
        }
        let snapshot = connection
            .client
            .session_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let terminal = self.terminal_id.clone().unwrap_or_default();
        let matches: Vec<&AgentInfo> = snapshot
            .agents
            .iter()
            .filter(|agent| agent.terminal_id == terminal)
            .collect();
        let [agent] = matches.as_slice() else {
            return Err(if matches.is_empty() {
                "Owned terminal is missing.".to_string()
            } else {
                "Terminal identity is ambiguous.".to_string()
            });
        };
        let owned = self
            .owned
            .as_ref()
            .ok_or("External reconnect has no persisted owner identity.")?;
        if agent.pane_id != owned.pane_id {
            return Err("External reconnect refused pane identity drift.".to_string());
        }
        let native_pid = *self
            .native_pid
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pid) = native_pid {
            let info = connection
                .client
                .pane_process_info(cyrup_herdr::schema::PaneProcessInfoParams {
                    pane_id: Some(owned.pane_id.clone()),
                })
                .await
                .map_err(|error| error.to_string())?;
            let canonical = self.canonical_of(&info);
            if info.pane_id != owned.pane_id || canonical.as_slice() != [pid] {
                return Err(
                    "External canonical native PID, cwd, or argv0 changed during reconnect."
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    fn owned(&self) -> Result<&OwnedPane, ExternalError> {
        self.owned.as_ref().ok_or_else(|| {
            ExternalError::Failed("Pane-native external owner identity is incomplete.".to_string())
        })
    }

    fn needs_attention(message: String) -> ExternalError {
        ExternalError::NeedsAttention(message)
    }

    async fn run_remote(
        &self,
        script: &str,
        args: &[&str],
        max: usize,
    ) -> cyrup_herdr::remote::RemoteOutput {
        self.transport
            .run(
                &self.machine.target,
                &remote_shell_command(script, args),
                Duration::from_secs(10),
                max,
                None,
            )
            .await
    }

    /// `runHerdrExternalPreflight(owner, adapter)` (`herdr-external-adapters.ts:61-64`).
    async fn preflight(&self, adapter: AdapterId) -> Result<PreflightEvidence, String> {
        let kind = ExternalKind::of(adapter);
        let name = kind.binary();
        let located = self
            .run_remote(&format!("command -v {name}"), &[], 4096)
            .await;
        let binary = located.stdout.trim().to_string();
        if located.status != Some(0)
            || !binary.starts_with('/')
            || basename(&binary) != name
            || binary.contains('\n')
        {
            return Err(format!(
                "Remote canonical {name} binary could not be resolved."
            ));
        }
        let script = if kind == ExternalKind::Cursor {
            format!(
                "exec /usr/bin/env AGENT_CLI_CREDENTIAL_STORE=file {} \"$@\"",
                shell_quote_remote(&binary)
            )
        } else {
            format!("exec {} \"$@\"", shell_quote_remote(&binary))
        };
        let invoke = |arg: &'static str| {
            let script = script.clone();
            let binary = binary.clone();
            async move {
                let output = self.run_remote(&script, &[arg], MAX_PREFLIGHT_BYTES).await;
                CommandEvidence {
                    binary,
                    args: vec![arg.to_string()],
                    status: output.status.unwrap_or(-1),
                    stdout: output.stdout,
                    stderr: output.stderr,
                }
            }
        };
        let evidence = PreflightEvidence {
            version: invoke("--version").await,
            help: invoke("--help").await,
        };
        validate_external_preflight(adapter, &evidence)?;
        Ok(evidence)
    }

    /// `start(kind, name, args)` (`herdr-placed-run.ts:172-180`), with the transient
    /// "not an available shell" retry.
    async fn start(&mut self, launch: &ExternalLaunch) -> Result<(), String> {
        let owned = self
            .owned
            .clone()
            .ok_or("Herdr placement owner has not provisioned its pane.")?;
        let client = self.client().map_err(|error| error.message().to_string())?;
        let tail: String = {
            let chars: Vec<char> = self.run_id.chars().collect();
            chars
                .get(chars.len().saturating_sub(20)..)
                .map(|slice| slice.iter().collect())
                .unwrap_or_default()
        };
        let name = format!("{}-{tail}", launch.kind.herdr_kind());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let agent = loop {
            let attempt = client
                .agent_start(
                    AgentStartParams {
                        name: name.clone(),
                        kind: launch.kind.herdr_kind().to_string(),
                        pane_id: owned.pane_id.clone(),
                        args: launch.args.clone(),
                        timeout_ms: Some(45_000),
                    },
                    Duration::from_secs(60),
                )
                .await;
            match attempt {
                Ok((agent, _argv)) => break agent,
                Err(error) if error.to_string().contains("is not an available shell") => {
                    let pane = client
                        .pane_get(owned.pane_id.clone())
                        .await
                        .map_err(|e| e.to_string())?;
                    if pane.pane_id != owned.pane_id
                        || pane.workspace_id != owned.workspace_id
                        || pane.tab_id != owned.tab_id
                    {
                        return Err("Herdr startup pane identity changed after a transient start rejection.".to_string());
                    }
                    if pane.agent.is_some() || pane.agent_session.is_some() {
                        return Err(
                            "Herdr startup pane became occupied after a transient start rejection."
                                .to_string(),
                        );
                    }
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        return Err(
                            "Herdr owned pane remained unavailable until the startup deadline."
                                .to_string(),
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(50).min(deadline - now)).await;
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        if agent.pane_id != owned.pane_id {
            return Err("Herdr started the native agent in an unexpected pane.".to_string());
        }
        self.agent_name = Some(name);
        self.terminal_id = Some(agent.terminal_id);
        Ok(())
    }

    async fn agent_get(&self) -> Result<AgentInfo, ExternalError> {
        let name = self.agent_name.clone().unwrap_or_default();
        self.client()?.agent_get(name).await.map_err(herdr_failure)
    }

    /// `#processProjection` (`herdr-external-adapters.ts:127`): the canonical processes in the
    /// owned pane — argv0 the preflight binary's basename, cwd the machine cwd.
    async fn canonical_processes(&self) -> Result<Vec<u32>, ExternalError> {
        let owned = self.owned()?;
        let info = self
            .client()?
            .pane_process_info(cyrup_herdr::schema::PaneProcessInfoParams {
                pane_id: Some(owned.pane_id.clone()),
            })
            .await
            .map_err(herdr_failure)?;
        if info.pane_id != owned.pane_id {
            return Err(ExternalError::Failed(
                "External process evidence pane identity changed.".to_string(),
            ));
        }
        Ok(self.canonical_of(&info))
    }

    /// `matchesExternalProcess` over a process projection: argv0 the preflight binary's basename,
    /// cwd the machine cwd.
    fn canonical_of(&self, info: &cyrup_herdr::schema::PaneProcessInfo) -> Vec<u32> {
        let argv0 = self
            .preflight
            .as_ref()
            .map(|evidence| basename(&evidence.version.binary).to_string())
            .unwrap_or_default();
        info.foreground_processes
            .iter()
            .filter(|entry| {
                !entry.name.trim().is_empty()
                    && entry.name.len() <= 128
                    && !entry.name.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
                    && entry.argv0.as_deref() == Some(argv0.as_str())
                    && entry.cwd.as_deref() == Some(self.machine.cwd.as_str())
            })
            .map(|entry| entry.pid)
            .collect()
    }

    /// `#exactProcess(requiredPid)` (`:128`).
    async fn exact_process(&self, required: Option<u32>) -> Result<u32, ExternalError> {
        let canonical = self.canonical_processes().await?;
        match canonical.as_slice() {
            [pid] if required.is_none_or(|want| want == *pid) => Ok(*pid),
            _ => Err(ExternalError::Failed(format!(
                "Could not prove exactly one canonical {} native PID with the exact cwd{}.",
                self.preflight
                    .as_ref()
                    .map(|evidence| basename(&evidence.version.binary).to_string())
                    .unwrap_or_default(),
                if required.is_some() {
                    " and retained PID"
                } else {
                    ""
                }
            ))),
        }
    }

    /// `#exactProcessAfterReconnect(requiredPid)` (`:137-138`).
    async fn exact_process_after_reconnect(&self, required: u32) -> Result<u32, ExternalError> {
        match self.exact_process(Some(required)).await {
            Err(ExternalError::Transport(error)) => {
                if !self.connection_lost().await {
                    return Err(ExternalError::Transport(error));
                }
                self.reconnect().await?;
                self.exact_process(Some(required)).await
            }
            other => other,
        }
    }

    async fn read_pane(
        &self,
        source: ReadSource,
        lines: Option<u32>,
    ) -> Result<String, ExternalError> {
        let owned = self.owned()?;
        let mut params = PaneReadParams::new(owned.pane_id.clone(), source);
        params.lines = lines;
        params.strip_ansi = true;
        let read = self
            .client()?
            .pane_read(params)
            .await
            .map_err(herdr_failure)?;
        if read.pane_id != owned.pane_id || read.source != source {
            return Err(ExternalError::Failed(
                "External terminal evidence identity or source changed.".to_string(),
            ));
        }
        Ok(read.text)
    }

    /// `#proveCursorWorkspace()` (`:122`): the workspace must be the exact canonical, owned,
    /// non-group/world-writable directory — read with node on the machine, as upstream reads it.
    async fn prove_cursor_workspace(&self) -> Result<serde_json::Value, String> {
        let script = r#"const fs=require("node:fs"),p=process.argv[1],s=fs.lstatSync(p),r=fs.realpathSync(p),u=process.getuid();if(!s.isDirectory()||s.isSymbolicLink()||r!==p||s.uid!==u||(s.mode&18)!==0)process.exit(65);process.stdout.write(JSON.stringify({realpath:r,device:s.dev,inode:s.ino,uid:s.uid}))"#;
        let output = self
            .run_remote(
                &format!("exec node -e {} \"$@\"", shell_quote_remote(script)),
                &[&self.machine.cwd],
                4 * 1024 * 1024,
            )
            .await;
        if output.status != Some(0) || !output.stderr.is_empty() {
            let head: String = output.stderr.chars().take(512).collect();
            return Err(format!(
                "Could not read exact owned external evidence: {head}"
            ));
        }
        let value: serde_json::Value = serde_json::from_str(&output.stdout)
            .map_err(|_| "Cursor workspace identity evidence is malformed.".to_string())?;
        let int = |key: &str| value.get(key).is_some_and(serde_json::Value::is_i64);
        if value.get("realpath").and_then(serde_json::Value::as_str)
            != Some(self.machine.cwd.as_str())
            || !int("device")
            || !int("inode")
            || !int("uid")
        {
            return Err(
                "Cursor workspace is not the exact canonical owned safe directory.".to_string(),
            );
        }
        Ok(value)
    }

    /// `#waitForStartup(clock)` (`:130`) for Cursor and Codex.
    async fn wait_for_startup(&self, kind: ExternalKind) -> Result<(u32, String), ExternalError> {
        let started = tokio::time::Instant::now();
        let deadline = started + EXTERNAL_STARTUP_TIMEOUT;
        let label = kind.label();
        let mut last;
        loop {
            let text = self.read_pane(ReadSource::Visible, None).await?;
            let ui_ready = if kind == ExternalKind::Cursor {
                if matches(&CURSOR_TRUST, &text) || codex_attention(&text) {
                    return Err(Self::needs_attention("Placed Cursor requires trust or other attention before input; truthful pane retained for inspection.".to_string()));
                }
                matches(&CURSOR_READY, &text)
            } else {
                if codex_attention(&text) {
                    return Err(Self::needs_attention("Placed Codex requires attention before input; truthful pane retained for inspection.".to_string()));
                }
                codex_ready(&text)
            };
            let canonical = self.canonical_processes().await?;
            let current = self.agent_get().await?;
            let owned = self.owned()?;
            let terminal = self.terminal_id.clone().unwrap_or_default();
            if !owns_pane(&current, &terminal, &owned.pane_id)
                || current.name.as_deref() != self.agent_name.as_deref()
            {
                return Err(ExternalError::Failed(
                    "External startup agent owner, pane, terminal, or name identity changed."
                        .to_string(),
                ));
            }
            let status = status_word(&current.agent_status);
            if status == "blocked" || status == "action_required" {
                return Err(Self::needs_attention(format!(
                    "Placed {label} requires attention before input; truthful pane retained for inspection."
                )));
            }
            if current.interactive_ready && current.agent.as_deref() != Some(kind.herdr_kind()) {
                return Err(ExternalError::Failed(
                    "External startup agent kind identity changed.".to_string(),
                ));
            }
            let managed_ready = current.agent.as_deref() == Some(kind.herdr_kind())
                && current.interactive_ready
                && !current.launch_pending
                && (status == "idle" || status == "done");
            if ui_ready && canonical.len() == 1 && managed_ready {
                return Ok((canonical.first().copied().unwrap_or_default(), text));
            }
            last = format!(
                "{}; {} canonical processes; managed {}",
                if ui_ready { "ready UI" } else { "startup UI" },
                canonical.len(),
                if managed_ready {
                    "ready".to_string()
                } else {
                    status
                }
            );
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            let poll = if now.duration_since(started) < EXTERNAL_STARTUP_BACKOFF_AFTER {
                CODEX_POLL
            } else {
                EXTERNAL_STARTUP_SLOW_POLL
            };
            tokio::time::sleep(poll.min(deadline - now)).await;
        }
        Err(Self::needs_attention(format!(
            "Placed {label} startup did not stabilize ({last}); truthful pane retained for inspection."
        )))
    }

    /// `#codexSnapshotOnce(pid)` (`:131`).
    async fn codex_snapshot(&self, pid: u32) -> Result<CodexSnapshot, ExternalError> {
        let current = self.agent_get().await?;
        let owned = self.owned()?.clone();
        let terminal = self.terminal_id.clone().unwrap_or_default();
        if !owns_pane(&current, &terminal, &owned.pane_id) {
            return Err(ExternalError::Failed(
                "Codex owner identity changed.".to_string(),
            ));
        }
        let status = status_word(&current.agent_status);
        let source = if status == "idle" || status == "done" {
            ReadSource::RecentUnwrapped
        } else {
            ReadSource::Visible
        };
        let pid = self.exact_process(Some(pid)).await?;
        let text = self.read_pane(source, Some(400)).await.map_err(|_| {
            ExternalError::Failed("Codex pane read identity or source changed.".to_string())
        })?;
        Ok(CodexSnapshot {
            workspace_id: owned.workspace_id,
            pane_id: owned.pane_id,
            terminal_id: terminal,
            pid,
            text,
            source,
        })
    }

    /// `#codexSnapshot(pid)` (`:155`): a snapshot whose transport was lost with the forward
    /// reconnects and is taken again.
    async fn codex_snapshot_after_reconnect(
        &self,
        pid: u32,
    ) -> Result<CodexSnapshot, ExternalError> {
        match self.codex_snapshot(pid).await {
            Err(ExternalError::Transport(error)) => {
                if !self.connection_lost().await {
                    return Err(ExternalError::Transport(error));
                }
                self.reconnect().await?;
                self.codex_snapshot(pid).await
            }
            other => other,
        }
    }

    /// `promptAndSettle(input)` (`:107-121`).
    async fn prompt_and_settle(&mut self, task: &str) -> Result<String, ExternalError> {
        let launch = self.launch.clone().ok_or_else(|| {
            ExternalError::Failed("Pane-native external owner identity is incomplete.".to_string())
        })?;
        let kind = launch.kind;
        let mut cursor_workspace = None;
        if kind == ExternalKind::Cursor {
            cursor_workspace = Some(self.prove_cursor_workspace().await.map_err(|error| {
                Self::needs_attention(format!(
                    "Placed Cursor requires attention before input: {error}; truthful pane retained for inspection."
                ))
            })?);
        }
        let mut pre_submit_text = String::new();
        let native_pid = if kind == ExternalKind::Claude {
            self.exact_process(None).await?
        } else {
            let (pid, text) = self.wait_for_startup(kind).await?;
            pre_submit_text = text;
            pid
        };
        *self
            .native_pid
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(native_pid);
        let wait = PROMPT_WAIT_DEFAULT.min(CODEX_DEFAULT_TIMEOUT);
        let name = self.agent_name.clone().unwrap_or_default();
        let params = AgentPromptParams {
            target: name,
            text: task.to_string(),
            wait: (kind != ExternalKind::Codex).then(|| AgentPromptWaitOptions {
                until: vec![AgentStatus::Idle, AgentStatus::Done, AgentStatus::Blocked],
                timeout_ms: u64::try_from(wait.as_millis()).ok(),
            }),
        };
        let prompted = match self
            .client()?
            .agent_prompt(params, wait + Duration::from_secs(5))
            .await
        {
            Ok(prompted) => prompted,
            // herdr ANSWERED with a refusal: an ordinary failure (`HerdrRpcError` upstream).
            Err(error @ cyrup_herdr::HerdrError::Api { .. }) => {
                return Err(ExternalError::Failed(error.to_string()));
            }
            // The transport lost the answer. With the forward still up, whether the prompt
            // landed is unknown; with it gone, reconnect and read the agent's settled state
            // (`recovered`, `:100`).
            Err(error) => {
                if !self.connection_lost().await {
                    return Err(Self::needs_attention(format!(
                        "Placed external prompt settlement is unresolved: {error}; truthful pane retained for inspection."
                    )));
                }
                self.reconnect().await?;
                self.agent_get().await?
            }
        };
        let owned = self.owned()?.clone();
        let terminal = self.terminal_id.clone().unwrap_or_default();
        if !owns_pane(&prompted, &terminal, &owned.pane_id) {
            return Err(ExternalError::Failed(
                "Herdr prompt settlement identity changed.".to_string(),
            ));
        }
        let status = status_word(&prompted.agent_status);
        if kind != ExternalKind::Codex && status != "idle" && status != "done" {
            return Err(Self::needs_attention(format!(
                "Herdr external prompt did not settle safely (status {status}); truthful pane retained for inspection."
            )));
        }
        self.exact_process_after_reconnect(native_pid).await?;
        if let Some(expected) = cursor_workspace {
            let current = self
                .prove_cursor_workspace()
                .await
                .map_err(ExternalError::Failed)?;
            if current != expected {
                return Err(ExternalError::Failed(
                    "Cursor workspace identity changed before settlement.".to_string(),
                ));
            }
        }
        if kind != ExternalKind::Codex {
            let terminal = self
                .read_pane(ReadSource::RecentUnwrapped, Some(400))
                .await?;
            return normalize_placed_terminal(&terminal).map_err(ExternalError::Failed);
        }
        let identity_owned = owned.clone();
        let this = &*self;
        let output = monitor_codex(
            task,
            &pre_submit_text,
            (
                identity_owned.workspace_id.as_str(),
                identity_owned.pane_id.as_str(),
                terminal.as_str(),
                native_pid,
            ),
            || this.codex_snapshot_after_reconnect(native_pid),
            || async {
                let _ = this.close_pane().await;
            },
            wait,
        )
        .await?;
        normalize_placed_terminal(&output).map_err(ExternalError::Failed)
    }

    async fn close_pane(&self) -> Result<(), String> {
        let (Some(owned), Ok(client)) = (&self.owned, self.client()) else {
            return Ok(());
        };
        match client.pane_get(owned.pane_id.clone()).await {
            Ok(pane) if pane.workspace_id == owned.workspace_id && pane.tab_id == owned.tab_id => {
                client
                    .pane_close(owned.pane_id.clone())
                    .await
                    .map_err(|error| error.to_string())
            }
            Ok(_) => {
                Err("Refusing cleanup because startup pane ownership is uncertain.".to_string())
            }
            Err(_) => Ok(()),
        }
    }

    /// `cleanup(closeStartedPane)` / `retain()`.
    async fn dispose(self, close: bool) {
        if close {
            let _ = self.close_pane().await;
            let _ = self
                .transport
                .run(
                    &self.machine.target,
                    &remote_shell_command(
                        "p=$1; case \"${p##*/}\" in cyrup-subagents-herdr-*) test -d \"$p\" && test ! -L \"$p\" && rm -rf -- \"$p\";; *) exit 64;; esac",
                        &[&self.runtime_dir],
                    ),
                    REMOTE_COMMAND_TIMEOUT,
                    4096,
                    None,
                )
                .await;
        }
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(connection) = connection {
            connection.close().await;
        }
    }
}

/// `prepareInternalHerdrExternalAdapter(input)` (`herdr-external-adapters.ts:165-169`): runtime
/// dir, connection, preflight (before any pane), launch descriptor, owned pane with an empty env,
/// `agent.start`, journal. Tears down on failure.
async fn prepare(
    adapter: AdapterId,
    machine: &HerdrMachineReference,
    run_id: &str,
    transport: SshTransport,
    agent_dir: &Path,
) -> Result<ExternalPlacedRun, String> {
    let made = transport
        .run(
            &machine.target,
            &remote_shell_command(
                "umask 077; mktemp -d \"${TMPDIR:-/tmp}/cyrup-subagents-herdr-$1-XXXXXXXX\"",
                &[run_id],
            ),
            Duration::from_secs(10),
            4096,
            None,
        )
        .await;
    let runtime_dir = made.stdout.trim().to_string();
    if made.error.is_some()
        || made.status != Some(0)
        || !runtime_dir.starts_with('/')
        || runtime_dir.contains('\n')
    {
        return Err("Could not provision a run-private remote runtime directory.".to_string());
    }
    let mut run = ExternalPlacedRun {
        machine: machine.clone(),
        transport: transport.clone(),
        connection: std::sync::Mutex::new(None),
        session: None,
        native_pid: std::sync::Mutex::new(None),
        reconnect: std::sync::Mutex::new(ReconnectBudget::default()),
        run_id: run_id.to_string(),
        runtime_dir,
        owned: None,
        launch: None,
        preflight: None,
        agent_name: None,
        terminal_id: None,
    };
    let result = async {
        let connection = transport
            .connect(&machine.target, machine.session.as_deref())
            .await
            .map_err(|error| error.to_string())?;
        run.session = connection.endpoint.session.clone();
        *run.connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(connection);
        run.preflight = Some(run.preflight(adapter).await?);
        let launch = external_launch(adapter, &run.runtime_dir, &machine.cwd, None)?;
        let client = run.client().map_err(|error| error.message().to_string())?;
        let session = run.session.clone();
        let key =
            super::lock::pane_allocation_key(&machine.target, session.as_deref(), &machine.cwd);
        run.owned = Some(
            super::native::provision_pane(
                &client,
                &machine.cwd,
                &run.run_id,
                std::collections::BTreeMap::new(),
                agent_dir,
                &key,
            )
            .await?,
        );
        run.start(&launch).await?;
        run.launch = Some(launch);
        Ok::<(), String>(())
    }
    .await;
    match result {
        Ok(()) => Ok(run),
        Err(error) => {
            run.dispose(true).await;
            Err(error)
        }
    }
}

/// Everything [`run_placed_external`] needs.
#[derive(Debug)]
pub struct PlacedExternalInput<'a> {
    /// The adapter.
    pub adapter: AdapterId,
    /// The machine.
    pub machine: &'a HerdrMachineReference,
    /// The run id (`${ctx.id}-${ctx.flatIndex}` upstream).
    pub run_id: Option<&'a str>,
    /// The full prompt (`buildExternalCliPrompt(systemPrompt, task)`).
    pub prompt: &'a str,
    /// The run deadline, if any.
    pub deadline: Option<tokio::time::Instant>,
    /// The stop signal.
    pub cancel: &'a cyrup_core::CancelToken,
    /// The ssh transport.
    pub transport: SshTransport,
    /// The agent dir (allocation locks).
    pub agent_dir: &'a Path,
}

/// Run one placed external profile end to end, racing the run's deadline and stop signal, and
/// settle it the way `subagent-runner.ts:900-944` settles it.
pub async fn run_placed_external(input: PlacedExternalInput<'_>) -> ExternalPlacedOutcome {
    let failure = |error: String| ExternalPlacedOutcome {
        output: String::new(),
        timed_out: false,
        stopped: false,
        error: Some(error),
        pane: None,
    };
    let run_id = match validate_run_id(&safe_run_id(input.run_id)) {
        Ok(run_id) => run_id,
        Err(error) => return failure(error),
    };
    let mut run = match prepare(
        input.adapter,
        input.machine,
        &run_id,
        input.transport.clone(),
        input.agent_dir,
    )
    .await
    {
        Ok(run) => run,
        Err(error) => return failure(error),
    };
    let pane = run.owned.clone();
    enum Raced {
        Settled(Result<String, ExternalError>),
        Timeout,
        Stop,
    }
    let raced = {
        let settle = run.prompt_and_settle(input.prompt);
        let timeout = async {
            match input.deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = settle => Raced::Settled(result),
            () = timeout => Raced::Timeout,
            () = input.cancel.cancelled() => Raced::Stop,
        }
    };
    match raced {
        Raced::Settled(Ok(output)) => {
            run.dispose(true).await;
            ExternalPlacedOutcome {
                output,
                timed_out: false,
                stopped: false,
                error: None,
                pane,
            }
        }
        Raced::Settled(Err(ExternalError::NeedsAttention(message))) => {
            run.dispose(false).await;
            ExternalPlacedOutcome {
                output: normalize_placed_terminal(&message).unwrap_or(message),
                timed_out: false,
                stopped: false,
                error: None,
                pane,
            }
        }
        Raced::Settled(Err(ExternalError::Failed(message) | ExternalError::Transport(message))) => {
            run.dispose(false).await;
            ExternalPlacedOutcome {
                pane,
                ..failure(message)
            }
        }
        Raced::Timeout => {
            run.dispose(false).await;
            let message = "Placed external run timed out; truthful pane retained for inspection.";
            ExternalPlacedOutcome {
                output: normalize_placed_terminal(message).unwrap_or_else(|_| message.to_string()),
                timed_out: true,
                stopped: false,
                error: None,
                pane,
            }
        }
        Raced::Stop => {
            run.dispose(true).await;
            ExternalPlacedOutcome {
                output: "Placed external run stopped by user.".to_string(),
                timed_out: false,
                stopped: true,
                error: None,
                pane,
            }
        }
    }
}
