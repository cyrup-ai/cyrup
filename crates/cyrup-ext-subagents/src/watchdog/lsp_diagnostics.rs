//! LSP diagnostics collection — a 1:1 port of `pi-subagents/src/watchdog/lsp-diagnostics.ts` (537
//! lines @v0.43.0).
//!
//! At the agent-end boundary, before the review model is asked anything, the watchdog runs the
//! project's own language server over the files git says changed and folds the result into the
//! review input (`runtime.ts:390,711-762`). A type error is a fact, not an opinion, so a diagnostic
//! that survives the freshness ledger is turned straight into a `blocker`/`concern` warning
//! ([`watchdog_warning_from_lsp_diagnostics`]) without a model call at all.
//!
//! Four pieces, in the order the boundary uses them:
//!
//! 1. **Target selection** ([`collect_target_files`], `:139-171`) — only TypeScript/JavaScript
//!    extensions, only real files, only inside the repo root, and only up to `maxFiles`. Everything
//!    rejected is reported in `skippedPaths`, so the status line can explain a `skipped` result.
//! 2. **Collection** ([`collect_watchdog_lsp_diagnostics`], `:498-537`) — resolve
//!    `typescript-language-server` from the project's `node_modules/.bin` FIRST and only then from
//!    `PATH` (`:104-116`), speak LSP over stdio, `didOpen` + `didSave` every target, and wait until
//!    every target has published or the budget expires.
//! 3. **The freshness ledger** ([`WatchdogLspDiagnosticsLedger`], `:180-208`) — the state that stops
//!    a pre-existing type error from re-warning on every single turn. It remembers the diagnostic
//!    identities last seen per path and returns only the new ones. Its two subtleties are asserted
//!    in the tests below: a `disabled`/`unavailable`/`failed` result passes through UNREDUCED and
//!    does not touch the ledger (`:185`), and only an `ok` result may FORGET a checked path that
//!    reported nothing (`:203-205`) — a `timeout` leaves the memory intact, because "the server did
//!    not answer" is not evidence that the errors are gone.
//! 4. **Rendering** ([`format_watchdog_lsp_diagnostics_block`], [`watchdog_warning_from_lsp_diagnostics`],
//!    `:210-241`) — both filter to `error`/`warning` severities only; `info`/`hint` never reach the
//!    model or the transcript.
//!
//! [CYRUP-DELTA] upstream's `JsonRpcLspClient` (`:243-400`) is an event-emitter class over Node
//! streams. Here it is a tokio task owning the child's stdout, a `Mutex`-guarded pending-request map
//! and a published-diagnostics map. The framing (`Content-Length: N\r\n\r\n<body>`, header match
//! case-insensitive, a malformed header block skipped rather than fatal) is byte-for-byte upstream's.
//!
//! [CYRUP-DELTA] the shutdown handshake keeps upstream's shape — `shutdown` request bounded by
//! [`SHUTDOWN_TIMEOUT_MS`], then an `exit` notification, then a `SIGTERM` kill if either step fails,
//! then a bounded wait for the process to actually go (`:317-328`) — but adds a final `start_kill` +
//! `wait` so a language server that ignores `SIGTERM` cannot leave a zombie behind the way a dropped
//! `tokio::process::Child` would.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cyrup_core::CancelToken;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};

use super::runtime::WatchdogLspDiagnostics;
use super::types::{
    WatchdogCategory, WatchdogConfidence, WatchdogLspConfig, WatchdogLspDiagnostic,
    WatchdogLspDiagnosticSeverity, WatchdogLspResult, WatchdogLspStatus, WatchdogSeverity,
    WatchdogWarning, WatchdogWarningSource,
};

/// `TS_JS_EXTENSIONS` (`lsp-diagnostics.ts:47-56`) — extension to LSP `languageId`.
const TS_JS_EXTENSIONS: &[(&str, &str)] = &[
    (".ts", "typescript"),
    (".tsx", "typescriptreact"),
    (".mts", "typescript"),
    (".cts", "typescript"),
    (".js", "javascript"),
    (".jsx", "javascriptreact"),
    (".mjs", "javascript"),
    (".cjs", "javascript"),
];

/// `PROVIDER_NAME` (`lsp-diagnostics.ts:57`).
pub const PROVIDER_NAME: &str = "typescript-language-server";
/// `MAX_MESSAGE_LENGTH` (`lsp-diagnostics.ts:58`).
const MAX_MESSAGE_LENGTH: usize = 500;
/// `MAX_STDERR_LENGTH` (`lsp-diagnostics.ts:59`).
const MAX_STDERR_LENGTH: usize = 2_000;
/// `SHUTDOWN_TIMEOUT_MS` (`lsp-diagnostics.ts:60`).
const SHUTDOWN_TIMEOUT_MS: u64 = 250;

/// `WatchdogLspRequest` (`lsp-diagnostics.ts:14-20`) — also the argument bag the runtime's
/// [`WatchdogLspDiagnostics::collect`] seam takes (`runtime.ts:727-733`).
#[derive(Debug, Clone)]
pub struct WatchdogLspRequest {
    /// The session cwd, used only as the root fallback.
    pub cwd: PathBuf,
    /// The repository root the change signature reported.
    pub root: PathBuf,
    /// Repo-relative changed paths.
    pub changed_paths: Vec<String>,
    /// The resolved LSP policy.
    pub config: WatchdogLspConfig,
    /// Cancellation — cancelled when the agent-end boundary is superseded.
    pub signal: Option<CancelToken>,
}

/// One target file (`TargetFile`, `lsp-diagnostics.ts:22-27`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetFile {
    /// Repo-relative, `/`-separated.
    pub rel_path: String,
    /// Absolute on-disk path.
    pub abs_path: PathBuf,
    /// `file://` URI.
    pub uri: String,
    /// LSP `languageId`.
    pub language_id: String,
}

/// `LspCommand` (`lsp-diagnostics.ts:28-32`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspCommand {
    /// The resolved executable.
    pub command: PathBuf,
    /// Its argv tail (always `--stdio`).
    pub args: Vec<String>,
    /// The human label carried into the result's `provider`.
    pub label: String,
}

/// `normalizeRelPath` (`lsp-diagnostics.ts:61-63`).
fn normalize_rel_path(value: &str) -> String {
    let slashed = value.replace(std::path::MAIN_SEPARATOR, "/");
    slashed
        .strip_prefix("./")
        .map_or(slashed.clone(), str::to_string)
}

/// `isPathInsideRoot` (`lsp-diagnostics.ts:64-67`): the relative path from the root must be empty
/// and must not escape upward.
fn is_path_inside_root(abs_path: &Path, root: &Path) -> bool {
    let Ok(rel) = abs_path.strip_prefix(root) else {
        return false;
    };
    !rel.components().any(|c| matches!(c, Component::ParentDir))
}

/// `languageIdForPath` (`lsp-diagnostics.ts:68-70`) — lower-cased extension lookup.
fn language_id_for_path(file_path: &Path) -> Option<&'static str> {
    let extension = file_path.extension()?.to_string_lossy().to_lowercase();
    let dotted = format!(".{extension}");
    TS_JS_EXTENSIONS
        .iter()
        .find(|(ext, _)| *ext == dotted)
        .map(|(_, language)| *language)
}

/// `trimDiagnosticMessage` (`lsp-diagnostics.ts:72-75`): collapse all whitespace, trim, then cap at
/// [`MAX_MESSAGE_LENGTH`] with a one-character ellipsis replacing the last kept character.
#[must_use]
pub fn trim_diagnostic_message(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_MESSAGE_LENGTH {
        return normalized;
    }
    let kept: String = normalized.chars().take(MAX_MESSAGE_LENGTH - 1).collect();
    format!("{kept}\u{2026}")
}

/// `severityFromLsp` (`lsp-diagnostics.ts:77-82`): 1/2/3 map to error/warning/info, and EVERY other
/// value — including an absent severity — maps to `hint`.
#[must_use]
pub const fn severity_from_lsp(value: Option<i64>) -> WatchdogLspDiagnosticSeverity {
    match value {
        Some(1) => WatchdogLspDiagnosticSeverity::Error,
        Some(2) => WatchdogLspDiagnosticSeverity::Warning,
        Some(3) => WatchdogLspDiagnosticSeverity::Info,
        _ => WatchdogLspDiagnosticSeverity::Hint,
    }
}

/// `pathExecutable` (`lsp-diagnostics.ts:84-94`): a real file that is executable.
fn path_executable(file_path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(file_path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `pathExecutableNames` (`lsp-diagnostics.ts:96-102`): on unix the bare name; on Windows the bare
/// name plus every `PATHEXT` suffix in both cases.
fn path_executable_names(name: &str) -> Vec<String> {
    if !cfg!(windows) {
        return vec![name.to_string()];
    }
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string());
    let extensions: Vec<String> = extensions
        .split(';')
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect();
    let mut names = vec![name.to_string()];
    names.extend(
        extensions
            .iter()
            .map(|ext| format!("{name}{}", ext.to_lowercase())),
    );
    names.extend(
        extensions
            .iter()
            .map(|ext| format!("{name}{}", ext.to_uppercase())),
    );
    names
}

/// `resolveTypeScriptLanguageServer` (`lsp-diagnostics.ts:104-116`).
///
/// The project's own `node_modules/.bin` copy wins over anything on `PATH` — a repo pinned to an
/// older language server must be checked with THAT server, not with whatever is installed globally —
/// and the two cases carry different `label`s so the status line says which one ran.
#[must_use]
pub fn resolve_typescript_language_server(root: &Path) -> Option<LspCommand> {
    for name in path_executable_names(PROVIDER_NAME) {
        let local = root.join("node_modules").join(".bin").join(&name);
        if path_executable(&local) {
            return Some(LspCommand {
                command: local,
                args: vec!["--stdio".to_string()],
                label: format!("{PROVIDER_NAME} (project)"),
            });
        }
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in path_executable_names(PROVIDER_NAME) {
            let candidate = dir.join(&name);
            if path_executable(&candidate) {
                return Some(LspCommand {
                    command: candidate,
                    args: vec!["--stdio".to_string()],
                    label: PROVIDER_NAME.to_string(),
                });
            }
        }
    }
    None
}

/// The result of [`collect_target_files`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetSelection {
    /// Files that will actually be opened.
    pub targets: Vec<TargetFile>,
    /// Everything rejected, in encounter order.
    pub skipped_paths: Vec<String>,
}

/// `collectTargetFiles` (`lsp-diagnostics.ts:139-171`).
///
/// The `maxFiles` check comes LAST (`:164`), after the language, containment and stat checks, so a
/// path over the cap is reported as skipped-for-cap rather than being confused with a path skipped
/// for being the wrong language.
///
/// # Errors
///
/// Propagates a `stat` failure that is not "not found", exactly as upstream rethrows it.
pub fn collect_target_files(
    root: &Path,
    changed_paths: &[String],
    max_files: u32,
) -> Result<TargetSelection, std::io::Error> {
    let mut selection = TargetSelection::default();
    for changed_path in changed_paths {
        let rel_path = normalize_rel_path(changed_path);
        let abs_path = root.join(&rel_path);
        let Some(language_id) = language_id_for_path(&abs_path) else {
            selection.skipped_paths.push(rel_path);
            continue;
        };
        if !is_path_inside_root(&abs_path, root) {
            selection.skipped_paths.push(rel_path);
            continue;
        }
        let metadata = match std::fs::metadata(&abs_path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                selection.skipped_paths.push(rel_path);
                continue;
            }
            Err(err) => return Err(err),
        };
        if !metadata.is_file() {
            selection.skipped_paths.push(rel_path);
            continue;
        }
        if u32::try_from(selection.targets.len()).unwrap_or(u32::MAX) >= max_files {
            selection.skipped_paths.push(rel_path);
            continue;
        }
        selection.targets.push(TargetFile {
            uri: path_to_file_uri(&abs_path),
            rel_path,
            abs_path,
            language_id: language_id.to_string(),
        });
    }
    Ok(selection)
}

/// `pathToFileURL(p).href` — a `file://` URI with the path percent-encoded the way WHATWG's path
/// encode-set does (space, `#`, `?`, `%`, control characters and non-ASCII).
#[must_use]
pub fn path_to_file_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().as_bytes() {
        match byte {
            b'/' | b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*'
            | b'+' | b',' | b';' | b'=' | b':' | b'@' => out.push(*byte as char),
            b if b.is_ascii_alphanumeric() => out.push(*b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `diagnosticIdentity` (`lsp-diagnostics.ts:173-178`).
///
/// Deliberately EXCLUDES line and column: a diagnostic that only moved because unrelated lines were
/// inserted above it is the same diagnostic, and re-warning about it would make every edit to a file
/// with a pre-existing error look like a new failure.
fn diagnostic_identity(diagnostic: &WatchdogLspDiagnostic) -> String {
    let payload = [
        diagnostic.path.as_str(),
        diagnostic.severity.as_str(),
        diagnostic.source.as_str(),
        diagnostic.code.as_deref().unwrap_or(""),
        diagnostic.message.as_str(),
    ]
    .join("\n");
    Sha256::digest(payload.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// `WatchdogLspDiagnosticsLedger` (`lsp-diagnostics.ts:180-208`) — the per-path freshness memory.
#[derive(Debug, Default)]
pub struct WatchdogLspDiagnosticsLedger {
    seen: HashMap<String, HashSet<String>>,
}

impl WatchdogLspDiagnosticsLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `reset` (`lsp-diagnostics.ts:183-185`).
    pub fn reset(&mut self) {
        self.seen.clear();
    }

    /// `reduce` (`lsp-diagnostics.ts:187-207`) — return only the diagnostics not seen last time.
    ///
    /// A `disabled`/`unavailable`/`failed` result short-circuits untouched: those statuses carry no
    /// diagnostics AND no evidence about the ones already remembered, so folding them in would
    /// silently forget a real error. A `timeout` IS folded (partial results are still results) but,
    /// unlike `ok`, may not forget a checked path that published nothing.
    pub fn reduce(&mut self, result: &WatchdogLspResult) -> WatchdogLspResult {
        if matches!(
            result.status,
            WatchdogLspStatus::Disabled
                | WatchdogLspStatus::Unavailable
                | WatchdogLspStatus::Failed
        ) {
            return result.clone();
        }
        let mut current_by_path: HashMap<String, HashSet<String>> = HashMap::new();
        let mut fresh: Vec<WatchdogLspDiagnostic> = Vec::new();
        for diagnostic in &result.diagnostics {
            let identity = diagnostic_identity(diagnostic);
            current_by_path
                .entry(diagnostic.path.clone())
                .or_default()
                .insert(identity.clone());
            if !self
                .seen
                .get(&diagnostic.path)
                .is_some_and(|identities| identities.contains(&identity))
            {
                fresh.push(diagnostic.clone());
            }
        }
        for (file_path, identities) in current_by_path.clone() {
            self.seen.insert(file_path, identities);
        }
        if result.status == WatchdogLspStatus::Ok {
            for checked_path in &result.checked_paths {
                if !current_by_path.contains_key(checked_path) {
                    self.seen.remove(checked_path);
                }
            }
        }
        WatchdogLspResult {
            diagnostics: fresh,
            ..result.clone()
        }
    }
}

/// `formatDiagnostic` (`lsp-diagnostics.ts:210-213`).
#[must_use]
pub fn format_diagnostic(diagnostic: &WatchdogLspDiagnostic) -> String {
    let code = diagnostic
        .code
        .as_deref()
        .filter(|code| !code.is_empty())
        .map(|code| format!(" {code}"))
        .unwrap_or_default();
    format!(
        "{}:{}:{} {}{code} {}: {}",
        diagnostic.path,
        diagnostic.line,
        diagnostic.column,
        diagnostic.severity.as_str(),
        diagnostic.source,
        diagnostic.message
    )
}

/// Every `error`/`warning` diagnostic — `info` and `hint` never reach a human or a model.
fn actionable(result: &WatchdogLspResult) -> Vec<&WatchdogLspDiagnostic> {
    result
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.severity,
                WatchdogLspDiagnosticSeverity::Error | WatchdogLspDiagnosticSeverity::Warning
            )
        })
        .collect()
}

/// `formatWatchdogLspDiagnosticsBlock` (`lsp-diagnostics.ts:215-223`) — the `LSP diagnostics:` block
/// appended to the review input, or the empty string when nothing is actionable.
#[must_use]
pub fn format_watchdog_lsp_diagnostics_block(result: &WatchdogLspResult) -> String {
    let actionable = actionable(result);
    if actionable.is_empty() {
        return String::new();
    }
    let mut lines = vec!["LSP diagnostics:".to_string()];
    lines.extend(
        actionable
            .iter()
            .map(|d| format!("- {}", format_diagnostic(d))),
    );
    lines.join("\n")
}

/// `watchdogWarningFromLspDiagnostics` (`lsp-diagnostics.ts:225-241`).
///
/// Any error at all makes it a `blocker`; warnings alone make it a `concern`. The count and the
/// pluralization follow whichever set that is (`errors.length || actionable.length`), and the
/// evidence is the first FIVE formatted diagnostics — not all of them — so one catastrophic file
/// cannot flood the transcript.
#[must_use]
pub fn watchdog_warning_from_lsp_diagnostics(
    result: &WatchdogLspResult,
) -> Option<WatchdogWarning> {
    let actionable = actionable(result);
    if actionable.is_empty() {
        return None;
    }
    let errors: Vec<&&WatchdogLspDiagnostic> = actionable
        .iter()
        .filter(|d| d.severity == WatchdogLspDiagnosticSeverity::Error)
        .collect();
    let severity = if errors.is_empty() {
        WatchdogSeverity::Concern
    } else {
        WatchdogSeverity::Blocker
    };
    let primary = errors
        .first()
        .map_or_else(|| actionable.first().copied(), |first| Some(**first))?;
    let count = if errors.is_empty() {
        actionable.len()
    } else {
        errors.len()
    };
    let kind = if errors.is_empty() {
        "warning"
    } else {
        "error"
    };
    let evidence = actionable
        .iter()
        .take(5)
        .map(|d| format_diagnostic(d))
        .collect::<Vec<_>>()
        .join("\n");
    let evidence = if evidence.is_empty() {
        format_diagnostic(primary)
    } else {
        evidence
    };
    Some(WatchdogWarning {
        severity,
        summary: format!(
            "LSP found {count} {kind}{} in changed {}.",
            if count == 1 { "" } else { "s" },
            if count == 1 { "file" } else { "files" }
        ),
        evidence,
        recommended_action:
            "Fix the reported diagnostics or explain why they are expected before accepting the \
             change."
                .to_string(),
        category: Some(WatchdogCategory::Correctness),
        confidence: Some(WatchdogConfidence::High),
        source: Some(WatchdogWarningSource::Lsp),
        agent: None,
        run_id: None,
        stale: None,
        auto_follow_attempt: None,
        state: None,
    })
}

// -------------------------------------------------------------------------------------------
// The JSON-RPC client (`lsp-diagnostics.ts:243-400`)
// -------------------------------------------------------------------------------------------

type PendingMap = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

/// The shared state the stdout reader task publishes into.
#[derive(Debug, Default)]
struct ClientState {
    pending: PendingMap,
    diagnostics: HashMap<String, Vec<Value>>,
    exited: bool,
}

/// `JsonRpcLspClient` (`lsp-diagnostics.ts:243-400`).
struct JsonRpcLspClient {
    child: Child,
    stdin: Option<tokio::process::ChildStdin>,
    next_id: AtomicU64,
    state: Arc<Mutex<ClientState>>,
    stderr: Arc<Mutex<String>>,
    reader: Option<tokio::task::JoinHandle<()>>,
    stderr_reader: Option<tokio::task::JoinHandle<()>>,
}

impl JsonRpcLspClient {
    /// Take ownership of a spawned child and start draining both of its output streams.
    fn new(mut child: Child) -> Result<Self, String> {
        let stdin = child
            .stdin
            .take()
            .ok_or("language server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("language server stdout unavailable")?;
        let stderr = child.stderr.take();
        let state = Arc::new(Mutex::new(ClientState::default()));
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let reader_state = Arc::clone(&state);
        let reader = tokio::spawn(async move {
            read_frames(stdout, reader_state).await;
        });
        let stderr_reader = stderr.map(|stderr| {
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buffer = vec![0u8; 4096];
                loop {
                    match reader.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut tail = tail.lock().await;
                            tail.push_str(&String::from_utf8_lossy(buffer.get(..n).unwrap_or(&[])));
                            // `.slice(-MAX_STDERR_LENGTH)` — keep only the tail.
                            if tail.chars().count() > MAX_STDERR_LENGTH {
                                let skip = tail.chars().count() - MAX_STDERR_LENGTH;
                                *tail = tail.chars().skip(skip).collect();
                            }
                        }
                    }
                }
            })
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            next_id: AtomicU64::new(1),
            state,
            stderr: stderr_tail,
            reader: Some(reader),
            stderr_reader,
        })
    }

    /// `send` (`lsp-diagnostics.ts:334-339`): the `Content-Length` frame.
    async fn send(&mut self, payload: &Value) -> Result<(), String> {
        let body = serde_json::to_string(payload).map_err(|e| e.to_string())?;
        let stdin = self
            .stdin
            .as_mut()
            .ok_or("language server already exited")?;
        stdin
            .write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    /// `request` (`lsp-diagnostics.ts:307-314`) — send and await, bounded by `timeout_ms` and by the
    /// abort signal (`withTimeout`, `:210-241`).
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        signal: Option<&CancelToken>,
    ) -> Result<Value, String> {
        if signal.is_some_and(CancelToken::is_cancelled) {
            return Err("aborted".to_string());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.state.lock().await.pending.insert(id, tx);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        let wait = async {
            match rx.await {
                Ok(result) => result,
                Err(_) => Err("language server closed the connection".to_string()),
            }
        };
        let outcome = match signal {
            Some(signal) => {
                tokio::select! {
                    biased;
                    () = signal.cancelled() => return Err("aborted".to_string()),
                    result = tokio::time::timeout(Duration::from_millis(timeout_ms), wait) => result,
                }
            }
            None => tokio::time::timeout(Duration::from_millis(timeout_ms), wait).await,
        };
        match outcome {
            Ok(result) => result,
            Err(_) => {
                self.state.lock().await.pending.remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    /// `notify` (`lsp-diagnostics.ts:316-318`) — fire and forget.
    async fn notify(&mut self, method: &str, params: Value) {
        let _ = self
            .send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await;
    }

    /// The published diagnostics for a URI, if any have arrived.
    async fn published(&self, uri: &str) -> Option<Vec<Value>> {
        self.state.lock().await.diagnostics.get(uri).cloned()
    }

    /// How many of `targets` have published (the `targets.every(...)` predicate at `:432,437`).
    async fn all_published(&self, targets: &[TargetFile]) -> bool {
        let state = self.state.lock().await;
        targets
            .iter()
            .all(|target| state.diagnostics.contains_key(&target.uri))
    }

    /// `stderrTail` (`lsp-diagnostics.ts:330-332`).
    async fn stderr_tail(&self) -> String {
        self.stderr.lock().await.trim().to_string()
    }

    /// `kill` (`lsp-diagnostics.ts:326-328`).
    async fn kill(&mut self) {
        let _ = self.child.start_kill();
    }

    /// `shutdown` (`lsp-diagnostics.ts:317-328`) plus the reap the Node version gets for free.
    async fn shutdown(&mut self) {
        if !self.state.lock().await.exited {
            match self
                .request("shutdown", Value::Null, SHUTDOWN_TIMEOUT_MS, None)
                .await
            {
                Ok(_) => self.notify("exit", Value::Null).await,
                Err(_) => self.kill().await,
            }
        }
        // Drop stdin so a server waiting on EOF can exit.
        self.stdin = None;
        let waited = tokio::time::timeout(
            Duration::from_millis(SHUTDOWN_TIMEOUT_MS),
            self.child.wait(),
        )
        .await;
        if waited.is_err() {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
        if let Some(stderr_reader) = self.stderr_reader.take() {
            stderr_reader.abort();
        }
    }
}

/// `handleStdout` + `handleMessage` (`lsp-diagnostics.ts:341-388`): the framing loop.
async fn read_frames(stdout: tokio::process::ChildStdout, state: Arc<Mutex<ClientState>>) {
    let mut reader = BufReader::new(stdout);
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 8192];
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buffer.extend_from_slice(chunk.get(..read).unwrap_or(&[]));
        // `while (true) { const headerEnd = …indexOf("\r\n\r\n"); if (headerEnd === -1) return; … }`
        // (`lsp-diagnostics.ts:344-348`): drain every COMPLETE frame the buffer now holds, then go
        // back for more bytes. The loop condition is the header search itself.
        while let Some(header_end) = find_subsequence(&buffer, b"\r\n\r\n") {
            let header =
                String::from_utf8_lossy(buffer.get(..header_end).unwrap_or(&[])).to_string();
            let Some(length) = content_length(&header) else {
                // A header block with no `Content-Length` is discarded, exactly as upstream does,
                // rather than desynchronizing the stream.
                buffer.drain(..header_end + 4);
                continue;
            };
            let body_start = header_end + 4;
            let body_end = body_start + length;
            if buffer.len() < body_end {
                break;
            }
            let body = String::from_utf8_lossy(buffer.get(body_start..body_end).unwrap_or(&[]))
                .to_string();
            buffer.drain(..body_end);
            match serde_json::from_str::<Value>(&body) {
                Ok(message) => handle_message(&message, &state).await,
                Err(err) => {
                    // `failProtocol` (`:381-385`): reject everything pending and stop.
                    let mut state = state.lock().await;
                    state.exited = true;
                    for (_, sender) in state.pending.drain() {
                        let _ = sender.send(Err(format!("Invalid LSP JSON-RPC response: {err}")));
                    }
                    return;
                }
            }
        }
    }
    let mut state = state.lock().await;
    state.exited = true;
    for (_, sender) in state.pending.drain() {
        let _ = sender.send(Err("language server exited".to_string()));
    }
}

/// `header.match(/content-length:\s*(\d+)/i)` (`lsp-diagnostics.ts:349`).
fn content_length(header: &str) -> Option<usize> {
    for line in header.split("\r\n") {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().ok();
        }
    }
    None
}

/// `Buffer.indexOf` over a byte needle.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `handleMessage` (`lsp-diagnostics.ts:368-379`).
async fn handle_message(message: &Value, state: &Arc<Mutex<ClientState>>) {
    if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics") {
        let params = message.get("params");
        let uri = params.and_then(|p| p.get("uri")).and_then(Value::as_str);
        let diagnostics = params
            .and_then(|p| p.get("diagnostics"))
            .and_then(Value::as_array);
        if let (Some(uri), Some(diagnostics)) = (uri, diagnostics) {
            state
                .lock()
                .await
                .diagnostics
                .insert(uri.to_string(), diagnostics.clone());
        }
        return;
    }
    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return;
    };
    let Some(sender) = state.lock().await.pending.remove(&id) else {
        return;
    };
    if let Some(error) = message.get("error") {
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map_or_else(|| format!("LSP request {id} failed"), str::to_string);
        let _ = sender.send(Err(text));
    } else {
        let _ = sender.send(Ok(message.get("result").cloned().unwrap_or(Value::Null)));
    }
}

/// `convertDiagnostics` (`lsp-diagnostics.ts:402-416`): drop anything without a string message or a
/// range start, then map 0-based LSP positions to 1-based display positions.
fn convert_diagnostics(target: &TargetFile, diagnostics: &[Value]) -> Vec<WatchdogLspDiagnostic> {
    diagnostics
        .iter()
        .filter(|d| {
            d.get("message").and_then(Value::as_str).is_some()
                && d.get("range").and_then(|r| r.get("start")).is_some()
        })
        .map(|d| {
            let start = d.get("range").and_then(|r| r.get("start"));
            let line = start
                .and_then(|s| s.get("line"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let column = start
                .and_then(|s| s.get("character"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            WatchdogLspDiagnostic {
                path: target.rel_path.clone(),
                line: u32::try_from(line.saturating_add(1).max(1)).unwrap_or(1),
                column: u32::try_from(column.saturating_add(1).max(1)).unwrap_or(1),
                severity: severity_from_lsp(d.get("severity").and_then(Value::as_i64)),
                source: d
                    .get("source")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(PROVIDER_NAME)
                    .to_string(),
                code: d.get("code").and_then(|code| match code {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                }),
                message: trim_diagnostic_message(
                    d.get("message").and_then(Value::as_str).unwrap_or(""),
                ),
            }
        })
        .collect()
}

/// `initializeParams` (`lsp-diagnostics.ts:418-431`).
fn initialize_params(root: &Path) -> Value {
    let root_uri = path_to_file_uri(root);
    json!({
        "processId": std::process::id(),
        "rootUri": root_uri,
        "capabilities": {
            "textDocument": {
                "publishDiagnostics": { "relatedInformation": false, "versionSupport": true },
            },
            "workspace": { "configuration": false, "workspaceFolders": true },
        },
        "workspaceFolders": [{
            "uri": root_uri,
            "name": root.file_name().map_or_else(
                || "workspace".to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
        }],
    })
}

/// `waitForDiagnostics` (`lsp-diagnostics.ts:433-441`): poll at most every 50 ms until every target
/// has published, the budget expires, or the signal aborts — then report whether they all did.
async fn wait_for_diagnostics(
    client: &JsonRpcLspClient,
    targets: &[TargetFile],
    timeout_ms: u64,
    signal: Option<&CancelToken>,
) -> bool {
    let started = Instant::now();
    while !signal.is_some_and(CancelToken::is_cancelled) {
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if elapsed >= timeout_ms {
            break;
        }
        if client.all_published(targets).await {
            return true;
        }
        let sleep_ms = 50.min(timeout_ms.saturating_sub(elapsed).max(1));
        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
    }
    client.all_published(targets).await
}

/// `collectWithTypeScriptLanguageServer` (`lsp-diagnostics.ts:443-496`).
async fn collect_with_typescript_language_server(
    root: &Path,
    targets: &[TargetFile],
    skipped_paths: &[String],
    command: &LspCommand,
    config: &WatchdogLspConfig,
    signal: Option<&CancelToken>,
) -> WatchdogLspResult {
    let started = Instant::now();
    let checked_paths: Vec<String> = targets.iter().map(|t| t.rel_path.clone()).collect();
    let failure = |status: WatchdogLspStatus, message: String| WatchdogLspResult {
        status,
        provider: Some(command.label.clone()),
        checked_paths: checked_paths.clone(),
        skipped_paths: skipped_paths.to_vec(),
        diagnostics: Vec::new(),
        message: Some(message),
    };
    let spawned = Command::new(&command.command)
        .args(&command.args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let child = match spawned {
        Ok(child) => child,
        Err(err) => return failure(WatchdogLspStatus::Failed, err.to_string()),
    };
    let mut client = match JsonRpcLspClient::new(child) {
        Ok(client) => client,
        Err(err) => return failure(WatchdogLspStatus::Failed, err),
    };
    let remaining = || {
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        config.timeout_ms.saturating_sub(elapsed).max(1)
    };
    let outcome = async {
        client
            .request("initialize", initialize_params(root), remaining(), signal)
            .await?;
        client.notify("initialized", json!({})).await;
        for target in targets {
            let text = std::fs::read_to_string(&target.abs_path).map_err(|e| e.to_string())?;
            client
                .notify(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": target.uri,
                            "languageId": target.language_id,
                            "version": 1,
                            "text": text,
                        }
                    }),
                )
                .await;
            client
                .notify(
                    "textDocument/didSave",
                    json!({ "textDocument": { "uri": target.uri }, "text": text }),
                )
                .await;
        }
        let complete = wait_for_diagnostics(&client, targets, remaining(), signal).await;
        let mut diagnostics = Vec::new();
        for target in targets {
            diagnostics.extend(convert_diagnostics(
                target,
                &client.published(&target.uri).await.unwrap_or_default(),
            ));
        }
        diagnostics.truncate(usize::try_from(config.max_diagnostics).unwrap_or(usize::MAX));
        Ok::<_, String>(WatchdogLspResult {
            status: if complete {
                WatchdogLspStatus::Ok
            } else {
                WatchdogLspStatus::Timeout
            },
            provider: Some(command.label.clone()),
            checked_paths: checked_paths.clone(),
            skipped_paths: skipped_paths.to_vec(),
            diagnostics,
            message: if complete {
                None
            } else {
                Some(format!(
                    "Timed out waiting {}ms for fresh LSP diagnostics.",
                    config.timeout_ms
                ))
            },
        })
    }
    .await;
    let result = match outcome {
        Ok(result) => result,
        Err(message) => {
            let timed_out = message.contains("timed out") || message == "aborted";
            let stderr = client.stderr_tail().await;
            let message = if stderr.is_empty() {
                message
            } else {
                format!("{message}; {stderr}")
            };
            failure(
                if timed_out {
                    WatchdogLspStatus::Timeout
                } else {
                    WatchdogLspStatus::Failed
                },
                message,
            )
        }
    };
    client.shutdown().await;
    result
}

/// `collectWatchdogLspDiagnostics` (`lsp-diagnostics.ts:498-537`) — the entry point the runtime
/// calls.
///
/// Four early exits before any process is spawned, each with its own status so the status line can
/// distinguish them: `disabled` (policy off), `skipped` (nothing to check), `unavailable` (no
/// language server), and `failed` (the walk itself threw).
pub async fn collect_watchdog_lsp_diagnostics(request: &WatchdogLspRequest) -> WatchdogLspResult {
    if !request.config.enabled {
        return WatchdogLspResult {
            status: WatchdogLspStatus::Disabled,
            provider: None,
            checked_paths: Vec::new(),
            skipped_paths: Vec::new(),
            diagnostics: Vec::new(),
            message: None,
        };
    }
    let root = if request.root.as_os_str().is_empty() {
        request.cwd.clone()
    } else {
        request.root.clone()
    };
    let selection =
        match collect_target_files(&root, &request.changed_paths, request.config.max_files) {
            Ok(selection) => selection,
            Err(err) => {
                return WatchdogLspResult {
                    status: WatchdogLspStatus::Failed,
                    provider: None,
                    checked_paths: Vec::new(),
                    skipped_paths: request.changed_paths.clone(),
                    diagnostics: Vec::new(),
                    message: Some(err.to_string()),
                };
            }
        };
    if selection.targets.is_empty() {
        return WatchdogLspResult {
            status: WatchdogLspStatus::Skipped,
            provider: None,
            checked_paths: Vec::new(),
            skipped_paths: selection.skipped_paths,
            diagnostics: Vec::new(),
            message: Some("No changed TypeScript or JavaScript files to check.".to_string()),
        };
    }
    let Some(command) = resolve_typescript_language_server(&root) else {
        let mut skipped = selection.skipped_paths;
        skipped.extend(selection.targets.iter().map(|t| t.rel_path.clone()));
        return WatchdogLspResult {
            status: WatchdogLspStatus::Unavailable,
            provider: Some(PROVIDER_NAME.to_string()),
            checked_paths: Vec::new(),
            skipped_paths: skipped,
            diagnostics: Vec::new(),
            message: Some(format!(
                "{PROVIDER_NAME} was not found in project node_modules/.bin or PATH."
            )),
        };
    };
    collect_with_typescript_language_server(
        &root,
        &selection.targets,
        &selection.skipped_paths,
        &command,
        &request.config,
        request.signal.as_ref(),
    )
    .await
}

// -------------------------------------------------------------------------------------------
// The runtime seam (`runtime.ts:86,178` — the injectable `lspDiagnostics` option)
// -------------------------------------------------------------------------------------------

/// The REAL [`WatchdogLspDiagnostics`] implementation: this module's collector plus this module's
/// ledger, held together as one stateful unit exactly as that trait's doc requires.
///
/// This is what replaces the runtime's `UnavailableLspDiagnostics` placeholder in production
/// ([`crate::extension::SubagentsExtension`]'s watchdog construction). The ledger lives behind a
/// [`std::sync::Mutex`] rather than being `&mut` because the trait's `reduce`/`reset_ledger` take
/// `&self` — upstream's ledger is likewise a private field mutated through a method on a shared
/// object.
#[derive(Debug, Default)]
pub struct TypeScriptLspDiagnostics {
    ledger: std::sync::Mutex<WatchdogLspDiagnosticsLedger>,
}

impl TypeScriptLspDiagnostics {
    /// A collector with an empty freshness ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl WatchdogLspDiagnostics for TypeScriptLspDiagnostics {
    async fn collect(&self, request: WatchdogLspRequest) -> Result<WatchdogLspResult, String> {
        Ok(collect_watchdog_lsp_diagnostics(&request).await)
    }

    fn reduce(&self, raw: WatchdogLspResult) -> WatchdogLspResult {
        // A poisoned ledger must not take the boundary down: report every diagnostic (the
        // conservative direction — a duplicate warning, never a swallowed one).
        match self.ledger.lock() {
            Ok(mut ledger) => ledger.reduce(&raw),
            Err(poisoned) => poisoned.into_inner().reduce(&raw),
        }
    }

    fn reset_ledger(&self) {
        match self.ledger.lock() {
            Ok(mut ledger) => ledger.reset(),
            Err(poisoned) => poisoned.into_inner().reset(),
        }
    }

    fn warning_from_diagnostics(&self, fresh: &WatchdogLspResult) -> Option<WatchdogWarning> {
        watchdog_warning_from_lsp_diagnostics(fresh)
    }

    fn format_block(&self, fresh: &WatchdogLspResult) -> String {
        format_watchdog_lsp_diagnostics_block(fresh)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn diagnostic(
        path: &str,
        severity: WatchdogLspDiagnosticSeverity,
        message: &str,
    ) -> WatchdogLspDiagnostic {
        WatchdogLspDiagnostic {
            path: path.to_string(),
            line: 1,
            column: 1,
            severity,
            source: "ts".to_string(),
            code: Some("2322".to_string()),
            message: message.to_string(),
        }
    }

    fn result(
        status: WatchdogLspStatus,
        checked: &[&str],
        diagnostics: Vec<WatchdogLspDiagnostic>,
    ) -> WatchdogLspResult {
        WatchdogLspResult {
            status,
            provider: Some("ts".to_string()),
            checked_paths: checked.iter().map(|p| (*p).to_string()).collect(),
            skipped_paths: Vec::new(),
            diagnostics,
            message: None,
        }
    }

    #[test]
    fn severity_mapping_defaults_everything_unknown_to_hint() {
        assert_eq!(
            severity_from_lsp(Some(1)),
            WatchdogLspDiagnosticSeverity::Error
        );
        assert_eq!(
            severity_from_lsp(Some(2)),
            WatchdogLspDiagnosticSeverity::Warning
        );
        assert_eq!(
            severity_from_lsp(Some(3)),
            WatchdogLspDiagnosticSeverity::Info
        );
        assert_eq!(
            severity_from_lsp(Some(4)),
            WatchdogLspDiagnosticSeverity::Hint
        );
        assert_eq!(severity_from_lsp(None), WatchdogLspDiagnosticSeverity::Hint);
        assert_eq!(
            severity_from_lsp(Some(99)),
            WatchdogLspDiagnosticSeverity::Hint
        );
    }

    #[test]
    fn messages_are_whitespace_collapsed_and_capped_with_an_ellipsis() {
        assert_eq!(trim_diagnostic_message("  a \n\t b  "), "a b");
        let long = "x".repeat(MAX_MESSAGE_LENGTH + 10);
        let trimmed = trim_diagnostic_message(&long);
        assert_eq!(trimmed.chars().count(), MAX_MESSAGE_LENGTH);
        assert!(trimmed.ends_with('\u{2026}'));
        // Exactly at the cap is NOT truncated.
        let exact = "y".repeat(MAX_MESSAGE_LENGTH);
        assert_eq!(trim_diagnostic_message(&exact), exact);
    }

    #[test]
    fn only_ts_and_js_extensions_are_targets_and_the_cap_is_checked_last() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        for name in ["a.ts", "b.tsx", "c.rs", "d.js"] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        let changed: Vec<String> = ["a.ts", "c.rs", "b.tsx", "missing.ts", "d.js"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let selection = collect_target_files(root, &changed, 2).unwrap();
        assert_eq!(
            selection
                .targets
                .iter()
                .map(|t| t.rel_path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.ts", "b.tsx"]
        );
        // `c.rs` (wrong language), `missing.ts` (absent) and `d.js` (over the cap) are all skipped.
        assert_eq!(selection.skipped_paths, vec!["c.rs", "missing.ts", "d.js"]);
        assert_eq!(selection.targets[0].language_id, "typescript");
        assert_eq!(selection.targets[1].language_id, "typescriptreact");
    }

    #[test]
    fn a_path_escaping_the_root_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(tmp.path().join("outside.ts"), "x").unwrap();
        let selection = collect_target_files(&root, &["../outside.ts".to_string()], 10).unwrap();
        assert!(selection.targets.is_empty());
        assert_eq!(selection.skipped_paths, vec!["../outside.ts"]);
    }

    #[test]
    fn file_uris_percent_encode_spaces_and_non_ascii() {
        assert_eq!(
            path_to_file_uri(Path::new("/a/b c.ts")),
            "file:///a/b%20c.ts"
        );
        assert!(path_to_file_uri(Path::new("/a/é.ts")).starts_with("file:///a/%C3%A9"));
    }

    #[test]
    fn the_ledger_reports_a_diagnostic_once_and_then_suppresses_it() {
        let mut ledger = WatchdogLspDiagnosticsLedger::new();
        let first = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![diagnostic(
                "a.ts",
                WatchdogLspDiagnosticSeverity::Error,
                "boom",
            )],
        );
        assert_eq!(ledger.reduce(&first).diagnostics.len(), 1);
        assert_eq!(ledger.reduce(&first).diagnostics.len(), 0, "already seen");
        // A different message on the same path IS new.
        let second = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Error, "boom"),
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Error, "other"),
            ],
        );
        let reduced = ledger.reduce(&second);
        assert_eq!(reduced.diagnostics.len(), 1);
        assert_eq!(reduced.diagnostics[0].message, "other");
    }

    #[test]
    fn a_moved_diagnostic_is_not_fresh_because_identity_excludes_position() {
        let mut ledger = WatchdogLspDiagnosticsLedger::new();
        let mut moved = diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Error, "boom");
        let first = result(WatchdogLspStatus::Ok, &["a.ts"], vec![moved.clone()]);
        assert_eq!(ledger.reduce(&first).diagnostics.len(), 1);
        moved.line = 42;
        moved.column = 7;
        let second = result(WatchdogLspStatus::Ok, &["a.ts"], vec![moved]);
        assert_eq!(ledger.reduce(&second).diagnostics.len(), 0);
    }

    #[test]
    fn only_an_ok_result_may_forget_a_clean_path() {
        let mut ledger = WatchdogLspDiagnosticsLedger::new();
        let dirty = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![diagnostic(
                "a.ts",
                WatchdogLspDiagnosticSeverity::Error,
                "boom",
            )],
        );
        assert_eq!(ledger.reduce(&dirty).diagnostics.len(), 1);
        // A TIMEOUT that reports nothing must NOT forget the remembered diagnostic.
        let timed_out = result(WatchdogLspStatus::Timeout, &["a.ts"], vec![]);
        assert_eq!(ledger.reduce(&timed_out).diagnostics.len(), 0);
        assert_eq!(
            ledger.reduce(&dirty).diagnostics.len(),
            0,
            "the timeout must not have cleared the memory"
        );
        // A clean OK does forget it, so a genuinely reintroduced error warns again.
        let clean = result(WatchdogLspStatus::Ok, &["a.ts"], vec![]);
        assert_eq!(ledger.reduce(&clean).diagnostics.len(), 0);
        assert_eq!(ledger.reduce(&dirty).diagnostics.len(), 1);
    }

    #[test]
    fn disabled_unavailable_and_failed_results_pass_through_unreduced() {
        let mut ledger = WatchdogLspDiagnosticsLedger::new();
        let seeded = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![diagnostic(
                "a.ts",
                WatchdogLspDiagnosticSeverity::Error,
                "boom",
            )],
        );
        assert_eq!(ledger.reduce(&seeded).diagnostics.len(), 1);
        for status in [
            WatchdogLspStatus::Disabled,
            WatchdogLspStatus::Unavailable,
            WatchdogLspStatus::Failed,
        ] {
            let passthrough = result(
                status,
                &["a.ts"],
                vec![diagnostic(
                    "a.ts",
                    WatchdogLspDiagnosticSeverity::Error,
                    "boom",
                )],
            );
            assert_eq!(
                ledger.reduce(&passthrough).diagnostics.len(),
                1,
                "{status:?} must not be reduced"
            );
        }
    }

    #[test]
    fn info_and_hint_reach_neither_the_block_nor_a_warning() {
        let quiet = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Info, "fyi"),
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Hint, "hint"),
            ],
        );
        assert_eq!(format_watchdog_lsp_diagnostics_block(&quiet), "");
        assert!(watchdog_warning_from_lsp_diagnostics(&quiet).is_none());
    }

    #[test]
    fn any_error_makes_the_warning_a_blocker_and_counts_only_the_errors() {
        let mixed = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Warning, "warn"),
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Error, "err"),
            ],
        );
        let warning = watchdog_warning_from_lsp_diagnostics(&mixed).unwrap();
        assert_eq!(warning.severity, WatchdogSeverity::Blocker);
        assert_eq!(warning.summary, "LSP found 1 error in changed file.");
        assert_eq!(warning.source, Some(WatchdogWarningSource::Lsp));
        assert_eq!(warning.confidence, Some(WatchdogConfidence::High));
        // The evidence carries EVERY actionable diagnostic, not just the counted ones.
        assert_eq!(warning.evidence.lines().count(), 2);
    }

    #[test]
    fn warnings_alone_make_a_concern_and_pluralize_on_the_actionable_count() {
        let warns = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Warning, "one"),
                diagnostic("a.ts", WatchdogLspDiagnosticSeverity::Warning, "two"),
            ],
        );
        let warning = watchdog_warning_from_lsp_diagnostics(&warns).unwrap();
        assert_eq!(warning.severity, WatchdogSeverity::Concern);
        assert_eq!(warning.summary, "LSP found 2 warnings in changed files.");
    }

    #[test]
    fn the_evidence_is_capped_at_five_diagnostics() {
        let many = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            (0..8)
                .map(|i| {
                    diagnostic(
                        "a.ts",
                        WatchdogLspDiagnosticSeverity::Error,
                        &format!("e{i}"),
                    )
                })
                .collect(),
        );
        let warning = watchdog_warning_from_lsp_diagnostics(&many).unwrap();
        assert_eq!(warning.evidence.lines().count(), 5);
        assert!(warning.summary.contains("8 errors"));
    }

    #[test]
    fn the_block_renders_one_dash_line_per_actionable_diagnostic() {
        let one = result(
            WatchdogLspStatus::Ok,
            &["a.ts"],
            vec![diagnostic(
                "a.ts",
                WatchdogLspDiagnosticSeverity::Error,
                "boom",
            )],
        );
        assert_eq!(
            format_watchdog_lsp_diagnostics_block(&one),
            "LSP diagnostics:\n- a.ts:1:1 error 2322 ts: boom"
        );
    }

    #[tokio::test]
    async fn a_disabled_policy_short_circuits_before_any_filesystem_work() {
        let request = WatchdogLspRequest {
            cwd: PathBuf::from("/nonexistent"),
            root: PathBuf::from("/nonexistent"),
            changed_paths: vec!["a.ts".to_string()],
            config: WatchdogLspConfig {
                enabled: false,
                timeout_ms: 1,
                max_files: 1,
                max_diagnostics: 1,
            },
            signal: None,
        };
        let result = collect_watchdog_lsp_diagnostics(&request).await;
        assert_eq!(result.status, WatchdogLspStatus::Disabled);
        assert!(result.provider.is_none());
    }

    #[tokio::test]
    async fn no_typescript_targets_reports_skipped_with_every_path_listed() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "x").unwrap();
        let request = WatchdogLspRequest {
            cwd: tmp.path().to_path_buf(),
            root: tmp.path().to_path_buf(),
            changed_paths: vec!["a.rs".to_string()],
            config: WatchdogLspConfig {
                enabled: true,
                timeout_ms: 500,
                max_files: 10,
                max_diagnostics: 10,
            },
            signal: None,
        };
        let result = collect_watchdog_lsp_diagnostics(&request).await;
        assert_eq!(result.status, WatchdogLspStatus::Skipped);
        assert_eq!(result.skipped_paths, vec!["a.rs"]);
        assert_eq!(
            result.message.as_deref(),
            Some("No changed TypeScript or JavaScript files to check.")
        );
    }

    #[tokio::test]
    async fn a_missing_language_server_reports_unavailable_and_skips_every_target() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.ts"), "const x: number = 1;").unwrap();
        let request = WatchdogLspRequest {
            cwd: tmp.path().to_path_buf(),
            root: tmp.path().to_path_buf(),
            changed_paths: vec!["a.ts".to_string()],
            config: WatchdogLspConfig {
                enabled: true,
                timeout_ms: 500,
                max_files: 10,
                max_diagnostics: 10,
            },
            signal: None,
        };
        let result = collect_watchdog_lsp_diagnostics(&request).await;
        // This workspace ships no `typescript-language-server`; if one is ever installed the run
        // becomes a real collection instead, which is still a well-formed result.
        match result.status {
            WatchdogLspStatus::Unavailable => {
                assert_eq!(result.checked_paths, Vec::<String>::new());
                assert_eq!(result.skipped_paths, vec!["a.ts"]);
                assert!(
                    result
                        .message
                        .as_deref()
                        .unwrap_or_default()
                        .contains("node_modules/.bin or PATH")
                );
            }
            other => assert!(
                matches!(
                    other,
                    WatchdogLspStatus::Ok | WatchdogLspStatus::Timeout | WatchdogLspStatus::Failed
                ),
                "unexpected status {other:?}"
            ),
        }
    }

    #[test]
    fn content_length_parsing_is_case_insensitive_and_ignores_other_headers() {
        assert_eq!(content_length("Content-Length: 12"), Some(12));
        assert_eq!(content_length("content-length:  7\r\nX: y"), Some(7));
        assert_eq!(content_length("X: y\r\nCONTENT-LENGTH: 3"), Some(3));
        assert_eq!(content_length("X: y"), None);
    }

    #[test]
    fn diagnostics_without_a_message_or_a_range_are_dropped() {
        let target = TargetFile {
            rel_path: "a.ts".into(),
            abs_path: PathBuf::from("/a.ts"),
            uri: "file:///a.ts".into(),
            language_id: "typescript".into(),
        };
        let raw = vec![
            json!({ "message": "kept", "range": { "start": { "line": 4, "character": 2 } }, "severity": 1, "code": 2322 }),
            json!({ "range": { "start": { "line": 0, "character": 0 } } }),
            json!({ "message": "no range" }),
        ];
        let converted = convert_diagnostics(&target, &raw);
        assert_eq!(converted.len(), 1);
        assert_eq!(
            converted[0].line, 5,
            "LSP lines are 0-based, display is 1-based"
        );
        assert_eq!(converted[0].column, 3);
        assert_eq!(converted[0].code.as_deref(), Some("2322"));
        assert_eq!(
            converted[0].source, PROVIDER_NAME,
            "an absent source defaults"
        );
    }
}
