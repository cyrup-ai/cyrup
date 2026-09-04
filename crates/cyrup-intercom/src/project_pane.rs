//! `HerdrErrorCode` / `HerdrResult` / `HerdrClient` / `ProjectPaneLaunch` — the launcher half of
//! `pi-intercom/project-agent.ts` (`v0.12.0`, `:10-186`, `:227-253`), with upstream's Herdr-specific
//! client generalized to a trait so the backend is one `impl`, not a dependency baked into the types.
//!
//! ICOM-042 §5 resolved to **A — Herdr**, so [`HerdrLauncher`] below is upstream's client ported
//! verbatim and every string it produces is byte-identical to `formatHerdrError`'s. The trait stays
//! because it is what keeps the vendor noun out of the *types*: a second backend is one more `impl`
//! and a different binding site, with no change to [`crate::tools::intercom`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use cyrup_core::CancelToken;

use crate::identity::{ENV_CYRUP_BIN, ENV_HERDR_BIN, ENV_INTERCOM_CYRUP_BIN};

/// `DEFAULT_PROJECT_AGENT_TIMEOUT_MS` (`project-agent.ts:7`).
pub const DEFAULT_PROJECT_AGENT_TIMEOUT_MS: u64 = 20_000;
/// `DEFAULT_PROJECT_AGENT_POLL_MS` (`project-agent.ts:8`).
pub const DEFAULT_PROJECT_AGENT_POLL_MS: u64 = 250;

/// `{ timeoutMs: 3_000 }` on the `--version` probe (`project-agent.ts:155`).
const VERSION_PROBE_TIMEOUT_MS: u64 = 3_000;
/// `{ timeoutMs: 15_000 }` on `pane split` and `pane run` (`project-agent.ts:239`, `:246`).
const PANE_COMMAND_TIMEOUT_MS: u64 = 15_000;
/// `{ timeoutMs: 5_000 }` on the compensating `pane close` (`project-agent.ts:248`).
const PANE_CLOSE_TIMEOUT_MS: u64 = 5_000;

/// `HerdrErrorCode` (`project-agent.ts:10-16`), one variant per upstream member.
///
/// The names drop the vendor prefix; [`PaneErrorCode::as_str`] keeps upstream's **wire spelling**,
/// so a launcher named `Herdr` renders `formatHerdrError`'s string byte for byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneErrorCode {
    /// `HERDR_UNAVAILABLE` — the launcher binary is absent, not on PATH, or refused to start
    /// (`:79-82`, `:109-114`).
    Unavailable,
    /// `HERDR_UNSUPPORTED_VERSION` — present but too old to split a raw pane (`:150-152`, `:160`).
    UnsupportedVersion,
    /// `PANE_GONE` — the split reported success but named no pane (`:243`), or the launcher said
    /// `gone` (`:63`).
    PaneGone,
    /// `NOT_FOUND` — the launcher said `not_found` / `not-found` / `no_such_pane` (`:64`).
    NotFound,
    /// `TIMEOUT` — the command's own deadline elapsed, or the [`CancelToken`] fired (`:95-103`).
    Timeout,
    /// `VALIDATION_ERROR` — upstream's default (`:65`): an unparseable version (`:159`) or a
    /// non-zero exit carrying no error envelope (`:133-134`).
    ValidationError,
}

impl PaneErrorCode {
    /// Upstream's wire spelling. `HERDR_*` is preserved for the two launcher-identity codes because
    /// [`PaneLaunchError`]'s `Display` already names the backend, and changing the token would make
    /// a cyrup log unmatchable against an upstream one.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "HERDR_UNAVAILABLE",
            Self::UnsupportedVersion => "HERDR_UNSUPPORTED_VERSION",
            Self::PaneGone => "PANE_GONE",
            Self::NotFound => "NOT_FOUND",
            Self::Timeout => "TIMEOUT",
            Self::ValidationError => "VALIDATION_ERROR",
        }
    }
}

/// The `{ ok: false, error }` arm of `HerdrResult<T>` (`project-agent.ts:18-20`). The `ok: true` arm
/// is Rust's `Ok`, so the union is a `Result` and no envelope type is needed.
#[derive(Clone, Debug)]
pub struct PaneLaunchError {
    /// The backend that produced it — `"Herdr"`, … Renders in [`std::fmt::Display`].
    pub backend: &'static str,
    /// `error.code`.
    pub code: PaneErrorCode,
    /// `error.message`.
    pub message: String,
}

/// `formatHerdrError` (`project-agent.ts:141-143`):
/// `` `Herdr project pane error (${code}): ${message}` ``. With `backend == "Herdr"` this is byte
/// identical to upstream.
impl std::fmt::Display for PaneLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} project pane error ({}): {}",
            self.backend,
            self.code.as_str(),
            self.message
        )
    }
}

/// `normalizeCode(raw)` (`project-agent.ts:60-66`) — map a backend's own code/diagnostic onto the
/// union. Substring matching on a lowercased haystack, in upstream's order.
#[must_use]
pub fn normalize_code(raw: &str) -> PaneErrorCode {
    let code = raw.to_lowercase();
    if code.contains("timeout") || code.contains("timed_out") {
        PaneErrorCode::Timeout
    } else if code.contains("gone") {
        PaneErrorCode::PaneGone
    } else if code.contains("not_found") || code.contains("not-found") || code == "no_such_pane" {
        PaneErrorCode::NotFound
    } else {
        PaneErrorCode::ValidationError
    }
}

/// `ProjectPaneLaunch` (`project-agent.ts:28-33`). `herdrVersion` → `launcher_version`.
#[derive(Clone, Debug)]
pub struct ProjectPaneLaunch {
    /// `paneId` — the backend's own pane handle, echoed in the tool result and `details.paneId`.
    pub pane_id: String,
    /// `projectRoot` — the **canonicalized** directory (`resolveProjectRoot`, `:179-186`).
    pub project_root: String,
    /// `command` — what was actually started in the pane.
    pub command: String,
    /// `herdrVersion` — the version string the availability probe reported.
    pub launcher_version: String,
    /// The backend that opened THIS pane ([`ProjectPaneLauncher::name`], captured at launch).
    ///
    /// No upstream counterpart: pi has exactly one backend, so `index.ts:2394` hard-codes `Herdr` in
    /// the result line. Carrying the name on the launch is what lets the result name the launcher
    /// that actually opened this pane, rather than re-reading the launcher slot and assuming it
    /// still holds the same one. `&'static str` mirrors [`PaneLaunchError::backend`], which is set
    /// from `name()` the same way.
    pub launcher_name: &'static str,
}

/// `openProjectPane`'s `input`, minus the injected client (`project-agent.ts:227-232`).
pub struct ProjectPaneRequest<'a> {
    /// The already-canonicalized project root (see [`resolve_project_root`]).
    pub project_root: PathBuf,
    /// `input.focus !== false` (`:239`) — **already defaulted to `true`** by the caller.
    pub focus: bool,
    /// Upstream's `AbortSignal` (`:230`).
    pub cancel: &'a CancelToken,
}

/// `HerdrClient` (`project-agent.ts:22-24`) collapsed with `openProjectPane` (`:227-253`).
///
/// Upstream exposes a generic `run(args)` and keeps the pane choreography in a free function
/// because there is exactly one backend. Here the choreography IS the contract — split a pane at
/// `project_root`, start the agent in it, close the pane again if the agent fails to start
/// (`:248`) — so the trait is the whole operation and a backend cannot get the cleanup wrong.
#[async_trait::async_trait]
pub trait ProjectPaneLauncher: Send + Sync {
    /// The backend's display name — `"Herdr"`, or `"cyrup"` for [`UnavailableLauncher`].
    ///
    /// It reaches a reader through four frames and must read correctly in every one:
    ///
    /// | Frame | Where |
    /// | --- | --- |
    /// | `{name} project pane error (CODE): …` | [`PaneLaunchError`]'s `Display` |
    /// | `Opened {name} project pane <id> for <root> …` | the `send` result, via [`ProjectPaneLaunch::launcher_name`] |
    /// | `… to open a {name} project pane and start cyrup there.` | the missing-peer error in [`crate::tools::intercom`] |
    /// | `The {name} pane may still be starting, …` | [`crate::project_target::wait_for_project_session`] |
    ///
    /// The last frame is the only one where the name is NOT followed by the words "project pane".
    /// A value containing that phrase therefore doubles it in the first three — which is why this
    /// must be a bare backend noun, and why [`UnavailableLauncher`] answers `"cyrup"` rather than
    /// anything pane-shaped.
    fn name(&self) -> &'static str;

    /// `openProjectPane(input)` (`:227-253`).
    ///
    /// # Errors
    /// A [`PaneLaunchError`] whose `code` is the real condition, never a catch-all.
    async fn open(
        &self,
        request: ProjectPaneRequest<'_>,
    ) -> Result<ProjectPaneLaunch, PaneLaunchError>;
}

/// The launcher substituted when no backend is configured. Every call answers
/// [`PaneErrorCode::Unavailable`] — the same code upstream returns for a missing binary — so the
/// flag is honoured with a true statement rather than silently ignored.
///
/// **Reachable, not dead code.** [`crate::extension::IntercomExtension`]'s `set_host_services` binds
/// a [`HerdrLauncher`], but `SharedIntercomState::set_host_services` — the state method, which
/// embeddings and tests call directly — does not. Any state built without going through the
/// extension hook therefore has an empty launcher slot, and
/// `tools::intercom::resolve_cwd_delivery_target` substitutes this.
pub struct UnavailableLauncher {
    /// Why. Named so the message can say *which* backend is missing when one is expected.
    pub reason: String,
}

#[async_trait::async_trait]
impl ProjectPaneLauncher for UnavailableLauncher {
    /// `"cyrup"`, not a pane-backend noun: there is no backend here, and this name reaches the
    /// reader only through [`PaneLaunchError`]'s `"{backend} project pane error (…)"` prefix, where
    /// naming the product that could not find one is both accurate and grammatical. A phrase
    /// containing "project pane" would render it twice.
    fn name(&self) -> &'static str {
        "cyrup"
    }
    async fn open(
        &self,
        _request: ProjectPaneRequest<'_>,
    ) -> Result<ProjectPaneLaunch, PaneLaunchError> {
        Err(PaneLaunchError {
            backend: self.name(),
            code: PaneErrorCode::Unavailable,
            message: self.reason.clone(),
        })
    }
}

/// `resolveProjectRoot(cwd)` (`project-agent.ts:179-186`): `resolve()`, then **reject a non-directory**,
/// then `realpathSync`. Runs BEFORE anything is spawned, so a typo'd path costs no process.
///
/// [`crate::cwd::resolve_path`] is the ported `resolve()`; `std::fs::canonicalize` is `realpathSync`.
/// Unlike [`crate::cwd::normalize_cwd`], the failure here is NOT swallowed — upstream throws.
///
/// # Errors
/// `Project target '<path>' is not a directory.` (`:183`, verbatim).
pub fn resolve_project_root(base: &Path, cwd: &str) -> Result<PathBuf, String> {
    let resolved = crate::cwd::resolve_path(base, cwd);
    let is_dir = std::fs::metadata(&resolved)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    if !is_dir {
        return Err(format!(
            "Project target '{}' is not a directory.",
            resolved.display()
        ));
    }
    Ok(std::fs::canonicalize(&resolved).unwrap_or(resolved))
}

// ---------------------------------------------------------------------------------------------
// §5-A — the Herdr backend.
// ---------------------------------------------------------------------------------------------

/// `parseLastJson(value)` (`project-agent.ts:52-59`): the whole trimmed string first, then each
/// line from the LAST backwards. A backend that prints progress lines before its JSON envelope is
/// therefore still readable.
fn parse_last_json(value: &str) -> Option<serde_json::Value> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    // `trimmed.split(/\r?\n/).reverse()` — `str::lines` splits on the same pair.
    trimmed
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
}

/// `String(value)` for the two error fields upstream stringifies (`:117`, `:134`). A JSON string
/// yields itself; anything else yields its JSON form.
///
/// [CYRUP-DELTA] JS `String({})` is `"[object Object]"`, which carries no information; this yields
/// the object's JSON instead. The divergence is only reachable when a backend puts a non-string in
/// `error.message`, and the Rust form is strictly more diagnosable.
fn js_string(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

/// `extractPaneId(value)` (`project-agent.ts:164-172`): look inside `value.pane` when that is an
/// object, else at `value` itself, then take the first of `pane_id` / `paneId` / `id` that is a
/// string. Arrays are rejected — `serde_json`'s `as_object` already does that.
fn extract_pane_id(value: &serde_json::Value) -> Option<String> {
    let record = value.as_object()?;
    let pane = record
        .get("pane")
        .and_then(serde_json::Value::as_object)
        .unwrap_or(record);
    ["pane_id", "paneId", "id"].into_iter().find_map(|key| {
        pane.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

/// `shellQuote(value)` (`project-agent.ts:174-177`). The pane runs the command through a shell, so
/// a path with a space must survive; `cfg!(windows)` is upstream's `process.platform === "win32"`.
fn shell_quote(value: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// The leading run of ASCII digits, and how many bytes it spans. `None` when there is none, or when
/// the run overflows `u64` — an overflowing "version" is not one.
fn leading_digits(rest: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut len = 0usize;
    for byte in rest {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value
            .checked_mul(10)?
            .checked_add(u64::from(*byte - b'0'))?;
        len = len.checked_add(1)?;
    }
    (len > 0).then_some((value, len))
}

/// `parseHerdrVersion(value)` (`project-agent.ts:145-148`) — `/(\d+)\.(\d+)\.(\d+)/.exec(value)`.
///
/// Hand-rolled rather than pulling in a regex dependency for one pattern. The scan reproduces the
/// engine's semantics exactly: leftmost match wins, and each `\d+` is greedy (no backtracking is
/// needed here, because a greedy digit run can only ever be followed by a non-digit, which is the
/// `.` the pattern wants next).
fn parse_herdr_version(value: &str) -> Option<(u64, u64, u64)> {
    let bytes = value.as_bytes();
    for start in 0..bytes.len() {
        let Some(rest) = bytes.get(start..) else {
            continue;
        };
        let Some((major, major_len)) = leading_digits(rest) else {
            continue;
        };
        let Some(rest) = rest.get(major_len..) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(b".") else {
            continue;
        };
        let Some((minor, minor_len)) = leading_digits(rest) else {
            continue;
        };
        let Some(rest) = rest.get(minor_len..) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(b".") else {
            continue;
        };
        let Some((patch, _)) = leading_digits(rest) else {
            continue;
        };
        return Some((major, minor, patch));
    }
    None
}

/// `supportsRawPanes(version)` (`project-agent.ts:150-152`) — Herdr 0.7.5+.
const fn supports_raw_panes((major, minor, patch): (u64, u64, u64)) -> bool {
    major > 0 || minor > 7 || (minor == 7 && patch >= 5)
}

/// `PI_INTERCOM_PI_BIN?.trim() || PI_BIN?.trim() || "pi"` (`project-agent.ts:245`), in cyrup's
/// spelling and with the crate's own re-exec fallback appended.
///
/// The two env rungs are upstream's, renamed by the `CYRUP_` prefix rule. The third rung is cyrup's
/// and has no upstream counterpart: pi can name `"pi"` and rely on PATH, but a cyrup built from
/// source may not be on PATH at all, so `current_exe()` — the same answer
/// [`crate::transport::spawn::resolve_broker_command`] gives — is tried before the bare name.
pub(crate) fn resolve_agent_command(env: impl Fn(&str) -> Option<String>) -> String {
    for key in [ENV_INTERCOM_CYRUP_BIN, ENV_CYRUP_BIN] {
        if let Some(value) = env(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "cyrup".to_string())
}

/// What one `herdr` invocation produced. Upstream's `run<T>` is generic and switches on `textOk`;
/// both shapes are carried here at once so the caller picks, which is the same information with no
/// type parameter.
struct HerdrOutput {
    /// `envelope.result ?? parsed` (`:125`) when stdout parsed as JSON.
    json: Option<serde_json::Value>,
    /// `stdout.trim()` — upstream's `textOk` arm (`:127`).
    text: String,
}

/// `createHerdrClient` (`project-agent.ts:68-139`) plus `openProjectPane` (`:227-253`).
pub struct HerdrLauncher {
    /// `options.bin ?? process.env.HERDR_BIN ?? "herdr"` (`:69`).
    bin: String,
    /// The agent binary a launched pane runs — see [`resolve_agent_command`].
    agent_command: String,
}

impl Default for HerdrLauncher {
    fn default() -> Self {
        Self::from_env()
    }
}

impl HerdrLauncher {
    /// The launcher this build uses, resolved against the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::with_env(|k| std::env::var(k).ok())
    }

    /// The pure core of [`Self::from_env`].
    #[must_use]
    pub fn with_env(env: impl Fn(&str) -> Option<String>) -> Self {
        let bin = env(ENV_HERDR_BIN)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "herdr".to_string());
        Self {
            bin,
            agent_command: resolve_agent_command(env),
        }
    }

    /// The message every `ENOENT` path shares (`:79-81`, `:111-113`).
    fn not_installed(&self) -> PaneLaunchError {
        self.fail(
            PaneErrorCode::Unavailable,
            "Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN."
                .to_string(),
        )
    }

    fn fail(&self, code: PaneErrorCode, message: String) -> PaneLaunchError {
        PaneLaunchError {
            backend: self.name(),
            code,
            message,
        }
    }

    /// `client.run(args, { timeoutMs, signal })` (`project-agent.ts:71-137`).
    ///
    /// # Errors
    /// Every arm of upstream's `error(...)`: `HERDR_UNAVAILABLE` on a spawn failure, `TIMEOUT` on
    /// the deadline or the token, the backend's own normalized code when stdout/stderr carried an
    /// `error` envelope, and `VALIDATION_ERROR` for a bare non-zero exit.
    async fn run(
        &self,
        args: &[&str],
        timeout_ms: u64,
        cancel: &CancelToken,
    ) -> Result<HerdrOutput, PaneLaunchError> {
        let mut command = tokio::process::Command::new(&self.bin);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // `windowsHide: true` (`:75`). Same constant and rationale as
            // `transport::spawn::spawn_detached_broker`; DETACHED_PROCESS is deliberately NOT set,
            // because this child's stdout is the result.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        // Upstream distinguishes a synchronous `spawn` throw (`:77-84`) from a later `error` event
        // (`:109-115`); in Rust both surface here, so the two messages collapse into the one that
        // names the actionable fix.
        let child = match command.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(self.not_installed()),
            Err(e) => {
                return Err(self.fail(
                    PaneErrorCode::Unavailable,
                    format!("Failed to start Herdr: {e}"),
                ));
            }
        };

        // `setTimeout(…, timeoutMs ?? 15_000)` (`:100-103`) and
        // `signal.addEventListener("abort", abort)` (`:96-98`) race the child. `kill_on_drop`
        // above is the `child.kill()` both handlers call: dropping the future drops the child.
        //
        // Upstream's `?? 15_000` covers an ABSENT `timeoutMs`; here the parameter is required, so
        // there is nothing to default and `timeout_ms` is both waited and reported. A fallback
        // would only let the deadline and the message below disagree.
        let deadline = Duration::from_millis(timeout_ms);
        let joined = args.join(" ");
        let output = tokio::select! {
            result = child.wait_with_output() => result,
            () = tokio::time::sleep(deadline) => {
                return Err(self.fail(
                    PaneErrorCode::Timeout,
                    format!("Herdr command '{joined}' timed out after {timeout_ms}ms."),
                ));
            }
            () = cancel.cancelled() => {
                return Err(self.fail(
                    PaneErrorCode::Timeout,
                    format!("Herdr command '{joined}' was aborted."),
                ));
            }
        };
        let output = match output {
            Ok(output) => output,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(self.not_installed()),
            Err(e) => {
                return Err(self.fail(
                    PaneErrorCode::Unavailable,
                    format!("Failed to run Herdr: {e}"),
                ));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code();
        let succeeded = exit_code == Some(0);

        // `parseLastJson(stdout) ?? (exitCode === 0 ? undefined : parseLastJson(stderr))` (`:118`).
        let parsed = parse_last_json(&stdout).or_else(|| {
            if succeeded {
                None
            } else {
                parse_last_json(&stderr)
            }
        });

        // `if (parsed && typeof parsed === "object" && !Array.isArray(parsed) && "error" in parsed)`
        // (`:119-123`) — the envelope wins over the exit code, in both directions.
        if let Some(envelope) = parsed.as_ref().and_then(serde_json::Value::as_object)
            && let Some(raw) = envelope.get("error")
        {
            let code = raw.get("code").map_or_else(String::new, js_string);
            let message = raw
                .get("message")
                .filter(|m| !m.is_null())
                .map_or_else(|| "Herdr command failed.".to_string(), js_string);
            return Err(self.fail(normalize_code(&code), message));
        }

        if succeeded {
            // `envelope.result ?? parsed` (`:125`).
            let json = parsed.map(|p| {
                p.as_object()
                    .and_then(|o| o.get("result"))
                    .filter(|r| !r.is_null())
                    .cloned()
                    .unwrap_or(p)
            });
            return Ok(HerdrOutput {
                json,
                text: stdout.trim().to_string(),
            });
        }

        // `stderr.split(/\r?\n/).find(line => line.trim())?.trim() ?? …` (`:133`).
        let message = stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .unwrap_or_else(|| {
                // `Herdr exited with code ${exitCode}.` — a signal-killed child has a null code in
                // node too, so the literal `null` is upstream's own rendering.
                let rendered = exit_code.map_or_else(|| "null".to_string(), |c| c.to_string());
                format!("Herdr exited with code {rendered}.")
            });
        Err(self.fail(PaneErrorCode::ValidationError, message))
    }

    /// `detectHerdr(client, signal)` (`project-agent.ts:154-162`) — availability THEN version, each
    /// with its own code. Returns the version text `ProjectPaneLaunch` carries.
    async fn detect(&self, cancel: &CancelToken) -> Result<String, PaneLaunchError> {
        let probe = self
            .run(&["--version"], VERSION_PROBE_TIMEOUT_MS, cancel)
            .await?;
        // `typeof result.data === "string" ? result.data : JSON.stringify(result.data)` (`:157`).
        let version_text = probe.json.map_or(probe.text, |v| js_string(&v));
        let Some(version) = parse_herdr_version(&version_text) else {
            return Err(self.fail(
                PaneErrorCode::ValidationError,
                format!("Could not parse the Herdr version from '{version_text}'."),
            ));
        };
        if !supports_raw_panes(version) {
            return Err(self.fail(
                PaneErrorCode::UnsupportedVersion,
                format!(
                    "Herdr {version_text} does not support raw panes. Upgrade to Herdr 0.7.5 or newer."
                ),
            ));
        }
        Ok(version_text)
    }
}

#[async_trait::async_trait]
impl ProjectPaneLauncher for HerdrLauncher {
    fn name(&self) -> &'static str {
        "Herdr"
    }

    async fn open(
        &self,
        request: ProjectPaneRequest<'_>,
    ) -> Result<ProjectPaneLaunch, PaneLaunchError> {
        // `resolveProjectRoot` already ran at the call site (`tools::intercom`), so a non-directory
        // never reaches a backend at all — upstream's `:233` moved one frame out.
        let project_root = request.project_root.display().to_string();
        let launcher_version = self.detect(request.cancel).await?;

        // `["pane", "split", "--current", "--direction", "right", "--cwd", projectRoot]` (`:237`),
        // `--focus` only when `input.focus !== false` (`:238`).
        let mut split_args = vec![
            "pane",
            "split",
            "--current",
            "--direction",
            "right",
            "--cwd",
            &project_root,
        ];
        if request.focus {
            split_args.push("--focus");
        }
        let split = self
            .run(&split_args, PANE_COMMAND_TIMEOUT_MS, request.cancel)
            .await?;
        let Some(pane_id) = split.json.as_ref().and_then(extract_pane_id) else {
            // `:243` — the literal upstream sentence, produced through `fail` so the prefix stays
            // in one place.
            return Err(self.fail(
                PaneErrorCode::PaneGone,
                "pane split returned no pane id.".to_string(),
            ));
        };

        let command = shell_quote(&self.agent_command);
        if let Err(e) = self
            .run(
                &["pane", "run", &pane_id, &command],
                PANE_COMMAND_TIMEOUT_MS,
                request.cancel,
            )
            .await
        {
            // `await client.run(["pane", "close", paneId], { timeoutMs: 5_000 })` (`:248`) — note
            // upstream passes NO signal here, so a cancelled launch still cleans its pane up. A
            // FRESH token reproduces that: reusing `request.cancel` would skip the cleanup in
            // exactly the case that needs it most.
            let _ = self
                .run(
                    &["pane", "close", &pane_id],
                    PANE_CLOSE_TIMEOUT_MS,
                    &CancelToken::new(),
                )
                .await;
            return Err(e);
        }

        Ok(ProjectPaneLaunch {
            pane_id,
            project_root,
            command,
            launcher_version,
            launcher_name: self.name(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    #[test]
    fn normalize_code_follows_upstreams_order_and_defaults_to_validation_error() {
        assert_eq!(normalize_code("TIMED_OUT"), PaneErrorCode::Timeout);
        assert_eq!(normalize_code("pane_timeout"), PaneErrorCode::Timeout);
        assert_eq!(normalize_code("PANE_GONE"), PaneErrorCode::PaneGone);
        assert_eq!(normalize_code("not-found"), PaneErrorCode::NotFound);
        assert_eq!(normalize_code("no_such_pane"), PaneErrorCode::NotFound);
        assert_eq!(normalize_code("whatever"), PaneErrorCode::ValidationError);
        assert_eq!(normalize_code(""), PaneErrorCode::ValidationError);
        // Upstream's order is load-bearing: a code carrying BOTH tokens resolves to TIMEOUT.
        assert_eq!(normalize_code("gone_after_timeout"), PaneErrorCode::Timeout);
    }

    #[test]
    fn the_error_string_is_upstreams_format_herdr_error() {
        let e = PaneLaunchError {
            backend: "Herdr",
            code: PaneErrorCode::PaneGone,
            message: "pane split returned no pane id.".to_string(),
        };
        assert_eq!(
            e.to_string(),
            "Herdr project pane error (PANE_GONE): pane split returned no pane id."
        );
    }

    #[test]
    fn parse_last_json_prefers_the_whole_body_then_the_last_parsable_line() {
        assert_eq!(parse_last_json("  "), None);
        assert_eq!(parse_last_json("{\"a\":1}").unwrap()["a"], 1);
        // Progress noise before the envelope: the LAST parsable line wins.
        let v = parse_last_json("starting\n{\"a\":1}\n{\"a\":2}").unwrap();
        assert_eq!(v["a"], 2);
        assert_eq!(parse_last_json("not json at all"), None);
    }

    #[test]
    fn extract_pane_id_checks_the_nested_pane_then_the_record_in_key_order() {
        let nested = serde_json::json!({ "pane": { "id": "%1", "pane_id": "%2" } });
        // `pane_id` precedes `id` in upstream's key list.
        assert_eq!(extract_pane_id(&nested).unwrap(), "%2");
        let flat = serde_json::json!({ "paneId": "%3" });
        assert_eq!(extract_pane_id(&flat).unwrap(), "%3");
        // An array is not an object.
        assert_eq!(extract_pane_id(&serde_json::json!([1, 2])), None);
        // A non-object `pane` falls back to the record itself.
        assert_eq!(
            extract_pane_id(&serde_json::json!({ "pane": 7, "id": "%4" })).unwrap(),
            "%4"
        );
        assert_eq!(extract_pane_id(&serde_json::json!({ "other": "x" })), None);
    }

    #[test]
    fn parse_herdr_version_takes_the_leftmost_greedy_triple() {
        assert_eq!(parse_herdr_version("herdr 0.7.5"), Some((0, 7, 5)));
        assert_eq!(parse_herdr_version("12.34.56"), Some((12, 34, 56)));
        assert_eq!(parse_herdr_version("v1.2.3.4"), Some((1, 2, 3)));
        assert_eq!(parse_herdr_version("1.2"), None);
        assert_eq!(parse_herdr_version("no digits"), None);
        assert_eq!(parse_herdr_version(""), None);
    }

    #[test]
    fn supports_raw_panes_is_zero_seven_five_and_up() {
        assert!(!supports_raw_panes((0, 7, 4)));
        assert!(supports_raw_panes((0, 7, 5)));
        assert!(supports_raw_panes((0, 8, 0)));
        assert!(supports_raw_panes((1, 0, 0)));
        assert!(!supports_raw_panes((0, 6, 9)));
    }

    #[test]
    fn shell_quote_wraps_and_escapes_for_the_host_platform() {
        if cfg!(windows) {
            assert_eq!(shell_quote(r#"a"b"#), r#""a\"b""#);
        } else {
            assert_eq!(shell_quote("/usr/bin/my cyrup"), "'/usr/bin/my cyrup'");
            assert_eq!(shell_quote("it's"), r"'it'\''s'");
        }
    }

    #[test]
    fn the_agent_command_ladder_prefers_the_intercom_scoped_variable() {
        let both = |k: &str| match k {
            ENV_INTERCOM_CYRUP_BIN => Some("  /opt/cyrup  ".to_string()),
            ENV_CYRUP_BIN => Some("/usr/bin/cyrup".to_string()),
            _ => None,
        };
        assert_eq!(resolve_agent_command(both), "/opt/cyrup");
        // Blank is not a value: it falls through to the next rung.
        let blank_first = |k: &str| match k {
            ENV_INTERCOM_CYRUP_BIN => Some("   ".to_string()),
            ENV_CYRUP_BIN => Some("/usr/bin/cyrup".to_string()),
            _ => None,
        };
        assert_eq!(resolve_agent_command(blank_first), "/usr/bin/cyrup");
        // Neither set: `current_exe()` is the third rung, and it is never blank.
        assert!(!resolve_agent_command(|_| None).is_empty());
    }

    #[test]
    fn the_herdr_binary_honours_its_vendor_env_var_and_defaults_to_the_bare_name() {
        let l =
            HerdrLauncher::with_env(|k| (k == ENV_HERDR_BIN).then(|| " /opt/herdr ".to_string()));
        assert_eq!(l.bin, "/opt/herdr");
        let d = HerdrLauncher::with_env(|_| None);
        assert_eq!(d.bin, "herdr");
        assert_eq!(d.name(), "Herdr");
    }

    #[tokio::test]
    async fn an_absent_binary_is_unavailable_with_the_install_hint() {
        let l = HerdrLauncher::with_env(|k| {
            (k == ENV_HERDR_BIN).then(|| "cyrup-herdr-does-not-exist".to_string())
        });
        let err = l
            .open(ProjectPaneRequest {
                project_root: std::env::temp_dir(),
                focus: true,
                cancel: &CancelToken::new(),
            })
            .await
            .expect_err("a missing binary cannot open a pane");
        assert_eq!(err.code, PaneErrorCode::Unavailable);
        assert_eq!(
            err.to_string(),
            "Herdr project pane error (HERDR_UNAVAILABLE): Herdr is not installed or is not on \
             PATH. Install Herdr 0.7.5+ or set HERDR_BIN."
        );
    }

    #[tokio::test]
    async fn an_already_cancelled_token_aborts_before_the_deadline() {
        let cancel = CancelToken::new();
        cancel.cancel();
        // `sleep` is the only other branch and it is 3 s away, so a prompt return proves the token
        // won the race rather than the timeout.
        let l = HerdrLauncher::with_env(|k| (k == ENV_HERDR_BIN).then(|| "sleep".to_string()));
        let started = std::time::Instant::now();
        let err = l
            .detect(&cancel)
            .await
            .expect_err("a cancelled launch cannot detect");
        assert_eq!(err.code, PaneErrorCode::Timeout);
        assert!(started.elapsed() < Duration::from_millis(2_500));
    }

    #[test]
    fn the_unavailable_launcher_answers_every_request_with_its_reason() {
        let l = UnavailableLauncher {
            reason: "nothing configured".to_string(),
        };
        assert_eq!(l.name(), "cyrup");
        let err = futures_lite_block(l.open(ProjectPaneRequest {
            project_root: PathBuf::from("/tmp"),
            focus: true,
            cancel: &CancelToken::new(),
        }));
        let err = err.expect_err("the unavailable launcher never succeeds");
        assert_eq!(err.code, PaneErrorCode::Unavailable);
        // The prefix names the product, not a backend: there is no backend, and a name containing
        // "project pane" would render the phrase twice.
        assert_eq!(
            err.to_string(),
            "cyrup project pane error (HERDR_UNAVAILABLE): nothing configured"
        );
    }

    /// The one future in these tests that touches no IO, driven without a runtime.
    fn futures_lite_block<F: std::future::Future>(fut: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(fut)
    }

    #[test]
    fn resolve_project_root_rejects_a_non_directory_with_upstreams_sentence() {
        let dir = std::env::temp_dir();
        let missing = resolve_project_root(&dir, "definitely-not-here-icom-042");
        let err = missing.expect_err("a non-existent path is not a directory");
        assert!(err.starts_with("Project target '"), "{err}");
        assert!(err.ends_with("' is not a directory."), "{err}");
        // A real directory canonicalizes.
        assert!(resolve_project_root(&dir, ".").is_ok());
    }
}
