//! [`HerdrCli`] — the fallback that shells out to the `herdr` binary.
//!
//! ## Why a CLI path exists at all, when this crate speaks the socket
//!
//! herdr's own CLI is itself a socket client. `send_request` (`tmp/herdr/src/cli.rs:769-775`)
//! resolves a socket, makes a `ping` connection for `ensure_server_protocol_compatible`
//! (`:784-801`) and then a second connection for the verb — two connections per invocation, plus a
//! process. Everything it can do on the wire, [`crate::HerdrClient`] can do in-process and
//! cheaper.
//!
//! Its one capability a native client lacks is herdr's *own* path resolution, and
//! [`crate::env::resolve_socket_path`] replicates that ladder. So this module exists for the case
//! the replication cannot cover: **`HERDR_SOCKET_PATH` is unset and the user's herdr config lives
//! somewhere the replication got wrong** — a custom `--session`, a herdr built with a different
//! app-dir name, a socket reachable only through the binary's own view of the world. Shelling out
//! then asks herdr where herdr is, which is always right.
//!
//! ## What it can carry, and the one thing it cannot
//!
//! Every verb in `tmp/herdr/src/cli/` is reachable this way: `pane split|run|close|get|list|
//! current|focus|rename|report-agent|report-agent-session|report-metadata|release-agent`
//! (`tmp/herdr/src/cli/pane.rs:18-43`), `api snapshot` (`tmp/herdr/src/cli/api.rs:56-66`),
//! `tab get|rename`, `agent …`, `workspace …`, `worktree …`.
//!
//! **The event stream is not.** There is no `events` module in `tmp/herdr/src/cli/` at this pin —
//! the modules are `agent, api, completion, integration, machine, notification, pane, plugin,
//! protocol_guard, runtime, server, server_not_running, spec, status, tab, target, workspace,
//! worktree` — so `events.subscribe` has no CLI wrapper to spawn. An event consumer genuinely
//! requires the socket and must report [`Unavailable::NoSocket`] rather than pretend
//! ([`crate::stream`]).
//!
//! ## The envelope on stdout is herdr's whole response
//!
//! `print_response` (`tmp/herdr/src/cli.rs:745-752`) writes the **entire** response value —
//! `{"id":…,"result":{"type":…,…}}` — to stdout and exits 0, or writes
//! `{"id":…,"error":{"code":…,"message":…}}` to **stderr** and exits 1. So the JSON on a
//! successful line is the same [`crate::schema::ResponseResult`] the socket would have produced,
//! one frame further out, and the error envelope is the same `{code, message}` pair
//! ([`ApiErrorCode`] interprets it identically on both paths).
//!
//! ## The parsing rules are pi's, deliberately
//!
//! The scanning below — whole body first, then the last parsable line; the error envelope
//! outranking the exit code in both directions; `envelope.result ?? parsed` — is
//! `pi-intercom v0.12.0 project-agent.ts:52-59,71-137`, because
//! `crates/cyrup-intercom/src/project_pane.rs`'s `HerdrLauncher` is a port of that file and
//! renders upstream's sentences from exactly these results. Generalising the mechanics here rather
//! than re-deriving them is what keeps this workspace at **one** herdr client: the error
//! vocabulary and the upstream sentences stay in `project_pane.rs`, which is the only place that
//! owes a byte-identical string, and [`CliError`] below carries the *structure* each of those
//! sentences is built from.
//!
//! A herdr process is spawned only when a caller runs a verb. Constructing a [`HerdrCli`] resolves
//! a binary name and nothing else — no PATH lookup, no probe, no process.

use std::future::Future;
use std::process::Stdio;
use std::time::Duration;

use crate::env::EnvSource;
use crate::error::{ApiErrorCode, Unavailable};

/// `HERDR_BIN` — the binary override, read verbatim.
///
/// **Not `CYRUP_HERDR_BIN`.** The variable belongs to the herdr vendor and upstream reads this
/// exact spelling (`pi-intercom v0.12.0 project-agent.ts:68`,
/// `options.bin ?? process.env.HERDR_BIN ?? "herdr"`); a user who has already set it for herdr
/// itself must not have to set a second, cyrup-private one. `crates/cyrup-intercom/src/identity.rs`
/// states the same rule for the same variable.
pub const HERDR_BIN: &str = "HERDR_BIN";

/// The binary name used when [`HERDR_BIN`] is unset or blank — resolved through `PATH` by the OS.
pub const HERDR_BIN_DEFAULT: &str = "herdr";

/// What one `herdr` invocation produced.
///
/// Both shapes at once rather than a type parameter: upstream's `run<T>` is generic and switches on
/// a `textOk` flag (`project-agent.ts:71-137`), which is the same information with a parameter
/// bolted on. A caller takes whichever field it asked for.
#[derive(Debug, Clone)]
pub struct CliOutput {
    /// `envelope.result ?? parsed` (`project-agent.ts:125`) — the response's `result` when stdout
    /// parsed as one of herdr's envelopes, else whatever JSON it did parse as, else `None`.
    pub json: Option<serde_json::Value>,
    /// `stdout.trim()` — upstream's `textOk` arm (`:127`). `herdr --version` is the case that needs
    /// it: clap prints `herdr 0.9.1`, which is not JSON.
    pub text: String,
}

/// Every way a `herdr` invocation can fail, kept structural.
///
/// No arm carries a rendered sentence, and that is the point: `cyrup-intercom`'s `HerdrLauncher`
/// owes `formatHerdrError`'s strings byte for byte and builds each one from the fields here, so the
/// sentences live in exactly one place and this crate's other consumers are free to report the same
/// conditions in their own words.
///
/// **Not `#[non_exhaustive]`, unlike [`Unavailable`] and [`ApiErrorCode`].** Those two are open
/// because herdr's own vocabulary is open — it invents error codes between releases and this client
/// must carry an unknown one as a value rather than a parse failure. This enum is closed because it
/// is *this crate's* account of what can go wrong running a subprocess, it is consumed inside one
/// workspace, and every consumer renders each arm as a sentence a user reads. A new arm must
/// therefore be a compile error at every consumer, not a value that silently folds into someone's
/// catch-all and reports a spawn failure as a validation error.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// There was no herdr to run. Today this is always [`Unavailable::BinaryMissing`]; the variant
    /// carries the open enum so a future rung of the fallback (a resolved socket that refuses, say)
    /// does not need a new arm here.
    #[error("{0}")]
    Unavailable(#[source] Unavailable),
    /// The process could not be started, for a reason other than "not found".
    #[error("could not start {bin}: {source}")]
    SpawnFailed {
        /// The binary that was tried.
        bin: String,
        /// The spawn error.
        #[source]
        source: std::io::Error,
    },
    /// The process started but its output could not be collected.
    #[error("could not collect the output of {bin} {command}: {source}")]
    WaitFailed {
        /// The binary that was run.
        bin: String,
        /// The argv, space-joined.
        command: String,
        /// The wait error.
        #[source]
        source: std::io::Error,
    },
    /// The caller's deadline elapsed. The child is killed — [`tokio::process::Command::kill_on_drop`]
    /// is set, and returning here drops it.
    #[error("herdr {command} did not finish within {timeout:?}")]
    TimedOut {
        /// The argv, space-joined.
        command: String,
        /// The deadline that elapsed.
        timeout: Duration,
    },
    /// The caller's cancellation future resolved first. The child is killed, as above.
    #[error("herdr {command} was cancelled")]
    Cancelled {
        /// The argv, space-joined.
        command: String,
    },
    /// herdr answered with an error envelope — `{"error":{"code":…,"message":…}}` — on stdout, or
    /// on stderr with a non-zero exit.
    ///
    /// **The envelope outranks the exit code in both directions** (`project-agent.ts:118-123`): an
    /// envelope on a zero exit is still a failure, and a non-zero exit carrying one reports the
    /// code rather than the exit status.
    #[error("herdr {command} failed: {code}")]
    Api {
        /// The argv, space-joined.
        command: String,
        /// herdr's `error.code`, interpreted exactly as the socket path interprets it.
        code: ApiErrorCode,
        /// herdr's `error.message`, verbatim. `None` when the envelope carried none, or carried
        /// `null` — a caller that must print something supplies its own default.
        message: Option<String>,
    },
    /// A non-zero exit that carried no error envelope at all.
    #[error("herdr {command} exited with {status:?}")]
    Exit {
        /// The argv, space-joined.
        command: String,
        /// The exit code, or `None` when a signal killed the child.
        status: Option<i32>,
        /// The first non-blank line of stderr, trimmed (`project-agent.ts:133`), when there was
        /// one. herdr's own `--help`-style failures put their reason here.
        stderr_line: Option<String>,
    },
}

/// The `herdr` binary, and the verbs cyrup runs through it.
///
/// Cheap and inert: it is a binary name. Constructing one spawns nothing, exactly as
/// [`crate::HerdrClient`] connects to nothing.
#[derive(Debug, Clone)]
pub struct HerdrCli {
    bin: String,
}

impl HerdrCli {
    /// A CLI bound to a specific binary — an absolute path, or a bare name for `PATH` to resolve.
    #[must_use]
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }

    /// [`HERDR_BIN`] trimmed, if it is not blank; else [`HERDR_BIN_DEFAULT`]
    /// (`project-agent.ts:68`).
    ///
    /// A blank value falls through rather than becoming the binary name: `HERDR_BIN=` in a shell
    /// profile is a variable that was unset badly, not a request to exec the empty string.
    #[must_use]
    pub fn with_env(env: &impl EnvSource) -> Self {
        let bin = env
            .var(HERDR_BIN)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| HERDR_BIN_DEFAULT.to_string());
        Self { bin }
    }

    /// The binary this CLI runs.
    #[must_use]
    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// Run `herdr <args>` under `timeout`, with no way to cancel it early.
    ///
    /// This is the right method for a **compensating** action — the `pane close` that cleans up
    /// after a failed `pane run`, say. Upstream passes no `AbortSignal` to that cleanup on purpose
    /// (`project-agent.ts:248`), because the case that most needs the pane closed is the cancelled
    /// launch; here the absence is structural rather than a token that is never fired.
    ///
    /// # Errors
    /// Any [`CliError`]: the binary is missing, the process failed to start or be collected, the
    /// deadline elapsed, herdr answered with an error envelope, or it exited non-zero.
    pub async fn run(
        &self,
        args: &[&str],
        timeout: Duration,
    ) -> std::result::Result<CliOutput, CliError> {
        self.run_cancellable(args, timeout, std::future::pending())
            .await
    }

    /// Run `herdr <args>` under `timeout`, racing `cancel`.
    ///
    /// `cancel` resolving is not an error about herdr — it is the caller withdrawing — so it gets
    /// its own [`CliError::Cancelled`] arm rather than being folded into
    /// [`CliError::TimedOut`]. The child is killed either way:
    /// [`tokio::process::Command::kill_on_drop`] is set and every early return drops it.
    ///
    /// # Errors
    /// As [`Self::run`], plus [`CliError::Cancelled`].
    pub async fn run_cancellable(
        &self,
        args: &[&str],
        timeout: Duration,
        cancel: impl Future<Output = ()> + Send,
    ) -> std::result::Result<CliOutput, CliError> {
        let mut command = tokio::process::Command::new(&self.bin);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // `windowsHide: true` (`project-agent.ts:75`). DETACHED_PROCESS is deliberately NOT
            // set: this child's stdout is the answer, so it must stay attached to these pipes.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let joined = args.join(" ");
        // A synchronous spawn failure (`:77-84`) and a later `error` event (`:109-115`) are two
        // paths in node and one here. `NotFound` is the only one a user can act on, and it is the
        // whole reason `Unavailable::BinaryMissing` exists.
        let child = match command.spawn() {
            Ok(child) => child,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(CliError::Unavailable(Unavailable::BinaryMissing {
                    bin: self.bin.clone(),
                }));
            }
            Err(source) => {
                return Err(CliError::SpawnFailed {
                    bin: self.bin.clone(),
                    source,
                });
            }
        };

        // `setTimeout(…, timeoutMs)` (`:100-103`) and `signal.addEventListener("abort", abort)`
        // (`:96-98`) race the child, and both handlers call `child.kill()` — which is what
        // `kill_on_drop` above does when either arm below returns.
        let output = tokio::select! {
            result = child.wait_with_output() => result,
            () = tokio::time::sleep(timeout) => {
                return Err(CliError::TimedOut { command: joined, timeout });
            }
            () = cancel => {
                return Err(CliError::Cancelled { command: joined });
            }
        };
        let output = match output {
            Ok(output) => output,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(CliError::Unavailable(Unavailable::BinaryMissing {
                    bin: self.bin.clone(),
                }));
            }
            Err(source) => {
                return Err(CliError::WaitFailed {
                    bin: self.bin.clone(),
                    command: joined,
                    source,
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let status = output.status.code();
        let succeeded = status == Some(0);

        // `parseLastJson(stdout) ?? (exitCode === 0 ? undefined : parseLastJson(stderr))` (`:118`).
        // stderr is read only on a failure because herdr writes its error envelope there
        // (`tmp/herdr/src/cli.rs:747`) while a successful verb may print progress noise there that
        // happens to be JSON.
        let parsed = parse_last_json(&stdout).or_else(|| {
            if succeeded {
                None
            } else {
                parse_last_json(&stderr)
            }
        });

        // `if (parsed && … && "error" in parsed)` (`:119-123`) — the envelope wins over the exit
        // code, in both directions.
        if let Some(envelope) = parsed.as_ref().and_then(serde_json::Value::as_object)
            && let Some(raw) = envelope.get("error")
        {
            let code = raw.get("code").map_or_else(String::new, json_to_string);
            return Err(CliError::Api {
                command: joined,
                code: ApiErrorCode::from_wire(&code),
                message: raw
                    .get("message")
                    .filter(|message| !message.is_null())
                    .map(json_to_string),
            });
        }

        if succeeded {
            // `envelope.result ?? parsed` (`:125`) — unwrap herdr's response frame when there is
            // one, so a caller sees the same object the socket would have handed it.
            let json = parsed.map(|value| {
                value
                    .as_object()
                    .and_then(|object| object.get("result"))
                    .filter(|result| !result.is_null())
                    .cloned()
                    .unwrap_or(value)
            });
            return Ok(CliOutput {
                json,
                text: stdout.trim().to_string(),
            });
        }

        // `stderr.split(/\r?\n/).find(line => line.trim())?.trim()` (`:133`).
        Err(CliError::Exit {
            command: joined,
            status,
            stderr_line: stderr
                .lines()
                .find(|line| !line.trim().is_empty())
                .map(|line| line.trim().to_string()),
        })
    }

    /// `herdr --version`, as text.
    ///
    /// The **availability probe**: it is the cheapest verb herdr has that proves the binary exists
    /// and runs, and unlike every other verb it needs no server — clap answers it before
    /// `send_request` is ever reached (`tmp/herdr/src/cli.rs:769`). A caller that wants to know
    /// *which* herdr this is parses the answer with [`parse_herdr_version`]; a caller that only
    /// wants to know whether there is one ignores it.
    ///
    /// The text is `json.map(json_to_string)` before `stdout.trim()` (`project-agent.ts:157`,
    /// `typeof result.data === "string" ? result.data : JSON.stringify(result.data)`), because a
    /// herdr that learns to answer `--version` as JSON must not start reporting `{"version":…}` as
    /// an unparseable version string.
    ///
    /// # Errors
    /// As [`Self::run_cancellable`]. [`Unavailable::BinaryMissing`] is the answer when herdr is not
    /// installed at all, which is the state this feature must stay honest about.
    pub async fn version_text(
        &self,
        timeout: Duration,
        cancel: impl Future<Output = ()> + Send,
    ) -> std::result::Result<String, CliError> {
        let probe = self
            .run_cancellable(&["--version"], timeout, cancel)
            .await?;
        Ok(probe
            .json
            .map_or(probe.text, |value| json_to_string(&value)))
    }
}

/// `parseLastJson(value)` (`project-agent.ts:52-59`): the whole trimmed string first, then each
/// line from the LAST backwards.
///
/// A herdr that prints progress lines, a deprecation notice or a log line before its envelope is
/// therefore still readable, and the *last* envelope wins because that is the one the verb
/// produced.
#[must_use]
pub fn parse_last_json(value: &str) -> Option<serde_json::Value> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(parsed);
    }
    // `trimmed.split(/\r?\n/).reverse()` — `str::lines` splits on the same pair.
    trimmed
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
}

/// `String(value)` for a JSON value that should have been a string (`project-agent.ts:117`,
/// `:134`, `:157`). A JSON string yields itself; anything else yields its JSON form.
///
/// `[CYRUP-DELTA]` JS `String({})` is `"[object Object]"`, which carries no information; this
/// yields the object's JSON instead. **Premise:** the divergence is reachable only when herdr puts
/// a non-string where a string was expected, and in that case the JSON form is strictly more
/// diagnosable than a constant.
#[must_use]
pub fn json_to_string(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

/// `extractPaneId(value)` (`project-agent.ts:164-172`): look inside `value.pane` when that is an
/// object, else at `value` itself, then take the first of `pane_id` / `paneId` / `id` that is a
/// string.
///
/// All three spellings are live. herdr's own `PaneInfo` field is `pane_id`
/// (`tmp/herdr/src/api/schema/panes.rs:527-528`) and a `pane.split` answer arrives as
/// `{"type":"pane_info","pane":{"pane_id":…}}`, so the nested `pane_id` rung is the one that fires
/// against a real herdr; the other two are upstream's tolerance for older and hand-rolled shapes.
/// Arrays are rejected — `as_object` already does that.
#[must_use]
pub fn extract_pane_id(value: &serde_json::Value) -> Option<String> {
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

/// `shellQuote(value)` (`project-agent.ts:174-177`).
///
/// `herdr pane run` hands its command to the pane's shell (`tmp/herdr/src/cli/pane.rs:1047-1051`
/// sends it as `pane.send_input{text, keys:["Enter"]}`), so a path with a space must survive the
/// round trip. `cfg!(windows)` is upstream's `process.platform === "win32"`.
#[must_use]
pub fn shell_quote(value: &str) -> String {
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

/// `parseHerdrVersion(value)` (`project-agent.ts:145-148`) — `/(\d+)\.(\d+)\.(\d+)/.exec(value)`,
/// against `herdr --version`'s `herdr 0.9.1` (`tmp/herdr/Cargo.toml:3`).
///
/// Hand-rolled rather than pulling in a regex dependency for one pattern. The scan reproduces the
/// engine's semantics exactly: leftmost match wins, and each `\d+` is greedy (no backtracking is
/// needed here, because a greedy digit run can only ever be followed by a non-digit, which is the
/// `.` the pattern wants next).
///
/// This is a **version** reading, not a capability one. herdr's stability rule is per-method —
/// *"JSON API clients should ignore unknown fields and handle unsupported methods as normal
/// errors"* (`socket-api.mdx:951-959`) — so a socket caller gates on
/// [`crate::HerdrError::is_unsupported_method`], not on a number. The number is for the one gate
/// that predates the socket path and backs an upstream sentence:
/// `crates/cyrup-intercom/src/project_pane.rs:273-276`.
#[must_use]
pub fn parse_herdr_version(value: &str) -> Option<(u64, u64, u64)> {
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
