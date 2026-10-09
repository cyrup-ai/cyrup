//! Immediate-bash seam (Pi `executeBash`/`recordBashResult`/`abortBash`/`isBashRunning`/
//! `hasPendingBashMessages`/`_flushPendingBashMessages`, agent-session.ts:2582-2684). The out-of-loop
//! bash RPC path: a command runs against the session's process backend (NOT the agent loop's `bash`
//! tool), its result is recorded as a `bashExecution` custom message, and — when a run is streaming —
//! deferred into a pending queue flushed after the turn so tool_use/tool_result ordering is intact.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use cyrup_tools::ops::shell_env;
use cyrup_tools::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncOpts};
use cyrup_tools::{ExecSpec, ExitStatus, ProcOps, ShellConfig};

/// The outcome of an immediate bash execution (Pi `BashResult`, bash-executor.ts:29-40).
///
/// Every field is `#[serde(default)]` on read, and that is load-bearing rather than lax. This type
/// doubles as the deserialization target for an extension-supplied `user_bash` override
/// (`UserBashEventResult.result`), and Pi — being TypeScript with no runtime type enforcement —
/// short-circuits on ANY truthy `result` regardless of which fields it carries
/// (`runner.ts:955-981`, `rpc-mode.ts:566-571`; no completeness check anywhere).
///
/// If cyrup instead required every field, a sandbox or remote-exec extension returning the
/// perfectly-valid-in-Pi `{"output": "...", "exitCode": 0}` would deserialize to `None`, the
/// override would be discarded, and the caller would FALL THROUGH and run the command raw on the
/// local shell — the exact outcome the extension existed to prevent. A strict deserializer here is
/// a fail-open, so partial overrides must be accepted.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    /// Combined stdout+stderr — sanitized (ANSI-stripped, control/format-char-filtered, CR-
    /// normalized) and tail-truncated to `DEFAULT_MAX_BYTES`/`DEFAULT_MAX_LINES`, same as the real
    /// `BashResult.output` Pi returns (bash-executor.ts:107-108,138-139).
    #[serde(default)]
    pub output: String,
    /// Process exit code (`None` when killed/signaled without a code).
    ///
    /// SEAM-083 — `skip_serializing_if` is the WIRE half and is not cosmetic. Upstream declares
    /// `exitCode: number | undefined` (`core/bash-executor.ts:33` @v0.83.0): a required key whose
    /// `undefined` value `JSON.stringify` **drops**, so a killed or signalled command produces a
    /// `bash` response with no `exitCode` key at all. The RPC handler arm is a bare
    /// `serde_json::to_value(result)` (`cyrup-modes/src/rpc.rs`), so this attribute IS the contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Whether the command was cancelled via [`crate::AgentSession::abort_bash`].
    #[serde(default)]
    pub cancelled: bool,
    /// Whether `output` was tail-truncated (Pi `BashResult.truncated`, bash-executor.ts:35;
    /// `truncateTail`'s `truncated` flag, bash-executor.ts:108/138).
    #[serde(default)]
    pub truncated: bool,
    /// Path to a temp file holding the FULL (untruncated, sanitized) output, once the raw stream
    /// exceeds `DEFAULT_MAX_BYTES` (Pi `BashResult.fullOutputPath`, bash-executor.ts:37; lazily
    /// opened by `ensureTempFile`, bash-executor.ts:64-74). `#[serde(default)]` on read: Pi's own
    /// `UserBashEventResult.result` type marks this field optional (`fullOutputPath?: string`,
    /// `extensions/types.ts:1044-1048`), so an extension-supplied override may omit it.
    ///
    /// SEAM-083 — and `skip_serializing_if` on WRITE, for the same reason it is optional on read.
    /// pi's `fullOutputPath?: string` (`core/bash-executor.ts:39` @v0.83.0) is present ONLY when the
    /// output spilled: `docs/rpc.md:473-479` @v0.83.0 shows the normal `bash` response with no such
    /// key, and `:482-495` shows it appearing only in the truncated case. Without this, every
    /// response carried `"fullOutputPath":null`, and a client using the documented
    /// `"fullOutputPath" in data` / `!== undefined` test to detect truncation took the truncated
    /// branch on EVERY bash response and went looking for a temp file that does not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

/// A streaming sink for combined bash output chunks (Pi `onChunk`, agent-session.ts:2589).
pub type BashChunkSink = Option<Box<dyn FnMut(&str) + Send>>;

/// Options for [`crate::AgentSession::execute_bash`] (Pi `executeBash` options, agent-session.ts:2588).
#[derive(Clone, Default)]
pub struct BashOptions {
    /// `!!` prefix: keep the output out of the LLM context (still recorded for history).
    pub exclude_from_context: bool,
    /// Optional identifier echoed on every `bash_execution_update` event (Pi `options.id`,
    /// agent-session.ts:2769/2786), so a front-end driving several concurrent `executeBash` calls
    /// can route the deltas. Absent from the emitted JSON when `None`, matching Pi's `id?: string`.
    pub id: Option<String>,
    /// Per-call command-execution backend override — Pi's `options.operations?: BashOperations`
    /// (`agent-session.ts:2768` @v0.83.0), consumed one line later as
    /// `options?.operations ?? createLocalBashOperations({ shellPath })` (`:2782`). This is the
    /// remote-exec seam an `ssh` / sandbox / VM extension redirects a single user command through
    /// without re-implementing the bash pipeline: sanitization, rolling buffer, temp-file spill and
    /// history recording all stay here and only the *execution* is delegated.
    ///
    /// `None` is upstream's absent `operations` and takes the local-shell branch of that `??`,
    /// byte-for-byte the path this seam took before the field existed.
    ///
    /// Upstream fills this from the `user_bash` event result (`UserBashEventResult.operations`,
    /// `core/extensions/types.ts:1139` @v0.84.4; threaded at `modes/rpc/rpc-mode.ts:581`,
    /// `operations: eventResult?.operations`), and so does cyrup — `execute_bash_with_user_event`
    /// writes what `ExtensionHost::user_bash_operations` returns for the winning extension. BOTH
    /// extension tiers can produce one: a native extension returns the object, and a WASM guest —
    /// which ADR-0002 forbids returning a callable — declares one with
    /// `registration.register-bash-operations` and serves it over the `events.bash-operations-exec`
    /// export (DRIFT-004, `cyrup:ext@0.10`). Any in-host caller can supply one directly.
    pub operations: Option<Arc<dyn cyrup_tools::ops::BashOperations>>,
}

/// Hand-written because `Arc<dyn BashOperations>` is not [`Debug`] — the trait is a behavioural seam
/// with one method and giving it a `Debug` supertrait would tax every implementor for a derive that
/// only this line needs. Reports *whether* an override is installed, which is the only thing a debug
/// dump of the options bag can honestly say about a backend.
impl std::fmt::Debug for BashOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BashOptions")
            .field("exclude_from_context", &self.exclude_from_context)
            .field("id", &self.id)
            .field(
                "operations",
                &if self.operations.is_some() {
                    "Some(<override>)"
                } else {
                    "None"
                },
            )
            .finish()
    }
}

/// Run `command` against `proc` in `cwd`, streaming combined output to `on_chunk`, honoring `cancel`
/// (Pi `executeBashWithOperations`). The default local backend kills the whole tree on cancel.
///
/// `bin_dir`, when set, is prepended to the child `PATH` exactly like the agent-loop `bash` tool
/// (Pi `createLocalBashOperations`'s `env: env ?? getShellEnv()`, `core/tools/bash.ts:100`, which
/// falls through to `getShellEnv()`'s unconditional `getBinDir()` prefix, `utils/shell.ts:122-128`).
/// Without this, the `!!`/RPC `executeBash` seam would silently diverge from the normal `bash` tool
/// (`cyrup-tools/src/tools/bash.rs:98`, `shell_env(opts.bin_dir)`) on which binaries resolve on PATH.
///
/// A genuine backend failure (spawn error, missing cwd, …) is returned as a real `Err`, NOT
/// fabricated into a "successful" [`BashResult`] — Pi's `executeBashWithOperations` only catches the
/// abort case in its `catch` block (`bash-executor.ts:122-128` @v1.1.0); every other error hits
/// `throw err` (`:126`), discarding whatever partial output had been captured. Mirror that exactly:
/// the caller must NOT record a history entry for a call that never really completed. A spill file
/// that could not be written fails the call the same way (see `BashOutputBuffer::spill_error`).
///
/// `operations` is Pi's `options?.operations ?? createLocalBashOperations({ shellPath })`
/// (`agent-session.ts:2782`): when `Some`, the command is handed to that backend instead of the
/// session's local process backend, and EVERYTHING else on this path — the env vector built below,
/// the sanitize/rolling-buffer/temp-spill pipeline, the [`ExitStatus`] → [`BashResult`] mapping —
/// is unchanged, because upstream delegates only the `exec` call itself
/// (`executeBashWithOperations(command, cwd, operations, {onChunk, signal})`, `bash-executor.ts`).
/// `None` takes the `??`'s right-hand branch, which is `proc`/`shell` exactly as before.
///
/// The eight parameters are pi's own `executeBashWithOperations(command, cwd, operations, {onChunk,
/// signal})` plus the three values cyrup must thread explicitly where upstream reads them off a
/// closure environment (`proc`, `shell`, `bin_dir`). They are unrelated to one another — a process
/// backend, a resolved shell, a per-call backend override, a path, a string, a cancel token and a
/// sink — so bundling them into a struct would only rename the same eight values at the single call
/// site (`session/bash.rs`) while hiding the correspondence with the upstream signature this
/// function is a line-for-line port of. Same disposition as the other `too_many_arguments` allows in
/// this workspace (e.g. `cyrup-ext-subagents/src/exec/mod.rs`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_bash(
    proc: &Arc<dyn ProcOps>,
    shell: &ShellConfig,
    operations: Option<&dyn cyrup_tools::ops::BashOperations>,
    cwd: PathBuf,
    command: String,
    bin_dir: Option<&std::path::Path>,
    cancel: cyrup_core::CancelToken,
    mut on_chunk: BashChunkSink,
) -> Result<BashResult, cyrup_core::ToolError> {
    // Pi's immediate-bash seam (`executeBashWithOperations`, bash-executor.ts) does NOT resolve a
    // spawn context and never touches the SESSION env — only the `bash` TOOL does
    // (`resolveSpawnContext`, bash.ts:158-184). So no scrub, no session-key injection here.
    let mut env = shell_env(bin_dir);
    // TOOL-031 / PARITY-GAPS PB-5, the immediate-bash half. The agent-identity markers are NOT
    // session keys: pi sets them on `process.env` in `cli.ts` before `main()` runs
    // (`PI_CODING_AGENT = "true"` at `cli.ts:13` @v0.83.0; `AI_AGENT = "pi"` at `:14` @v0.84.1,
    // mirrored in `rpc-entry.ts:7-8`), so EVERY child inherits them through `getShellEnv()`'s
    // `{...process.env}` (`utils/shell.ts:130-133`) — including this seam, which reaches
    // `getShellEnv()` by the same fall-through as the tool.
    //
    // cyrup's bin declines the process-GLOBAL mutation (`std::env::set_var` is `unsafe` under
    // edition 2024; see `crates/cyrup/src/main.rs`), so each spawn site pushes them per-child. The
    // `bash` tool already did (`cyrup-tools/src/tools/bash.rs`); this seam did not, so `!!cmd` and
    // the RPC `executeBash` saw a DIFFERENT environment from the identical command run as a tool.
    //
    // [CYRUP-DELTA — KEY; hard rename] pi's key is `PI_CODING_AGENT` (`cli.ts:13` @v0.83.0);
    // cyrup's public env surface is `CYRUP_*`-only, mirroring `cyrup-tools/src/tools/bash.rs`.
    env.push(("CYRUP_CODING_AGENT".to_string(), "true".to_string()));
    // [CYRUP-DELTA — KEY *and* value; the key is a FORWARD-PORT from `cli.ts:14` @v0.84.1, which is
    // AHEAD of the ported tag] `AI_AGENT` does not exist anywhere in pi @v0.83.0
    // (`git -C pi grep -n AI_AGENT v0.83.0 -- packages/` → 0 hits; `cli.ts:13` @v0.83.0 sets only
    // `PI_CODING_AGENT`). So cyrup writes a variable into every bash child that the ported baseline
    // never wrote, and the value additionally names WHICH agent is running (`"pi"` upstream).
    // Deliberate: the marker is how a hook or a script tells an agent shell from a human one, and
    // dropping it would leave the v0.84.1 uplift with a hole. Stated here rather than only in the
    // prose above (CFG-069) so a later v0.84.1 uplift reads this as ALREADY-PORTED-EARLY and not as
    // already-done-at-tag. Same class as the `working-start`/`working-stop` precedent.
    env.push(("AI_AGENT".to_string(), "cyrup".to_string()));
    let mut buffer = BashOutputBuffer::new();
    // ONE sink, shared by both branches: pi's `onChunk` wrapper is built once and handed to
    // `executeBashWithOperations` whichever backend it resolved (`agent-session.ts:2779-2789`), so
    // an overriding backend gets the identical sanitize→buffer→spill treatment. Hoisted out of the
    // call so the two arms below cannot drift on it.
    let mut sink = |data: &[u8]| {
        if let Some(sanitized) = buffer.push_raw(data)
            && let Some(cb) = on_chunk.as_mut()
        {
            cb(&sanitized);
        }
    };
    let status = match operations {
        // Pi's `options?.operations` branch. `timeout: None` mirrors this seam's call, which passes
        // no `timeout` (only the agent-loop `bash` TOOL has one); `env_remove` is empty for the same
        // reason the `ExecSpec` below leaves it empty — `executeBashWithOperations` never resolves a
        // spawn context and so never deletes session keys (`bash-executor.ts`; the deletions are
        // `resolveSpawnContext`'s, `bash.ts:158-184`, and belong to the tool).
        Some(ops) => {
            ops.exec(
                &command,
                &cwd,
                cyrup_tools::ops::BashExecOptions {
                    on_data: &mut sink,
                    cancel,
                    timeout: None,
                    env,
                    env_remove: Vec::new(),
                },
            )
            .await
        }
        // The `?? createLocalBashOperations({ shellPath })` branch. `shell` was already resolved by
        // the caller from the live `shellPath` setting, which is where the per-call resolution pi
        // does inside `createLocalBashOperations`' closure (`bash.ts:89`) happens here.
        None => {
            let spec = ExecSpec {
                command,
                cwd,
                env,
                env_remove: Vec::new(),
                shell: shell.clone(),
            };
            proc.exec(spec, cancel, None, &mut sink).await
        }
    };
    // `sink` is not dropped explicitly: it holds only a `&mut` borrow of `buffer` (and of
    // `on_chunk`), and that borrow already ends at its last use above, which is what lets
    // `settle` take `buffer` by value on the next line. The `on_chunk` box itself — and so the
    // caller's `chunk_tx` — is released when this function returns, which is what the caller
    // waits on before draining its event pump (`session/bash.rs`).
    settle(buffer, status, &mut on_chunk)
}

/// The end of `executeBashWithOperations` once `exec` has settled (`bash-executor.ts:119-145`
/// @v1.1.0): a backend failure re-throws without flushing (`:122-127`); every other outcome, an
/// abort included, flushes the held-back text to the sink first (`:130`), then truncates and
/// reports.
fn settle(
    mut buffer: BashOutputBuffer,
    status: Result<ExitStatus, cyrup_core::ToolError>,
    on_chunk: &mut BashChunkSink,
) -> Result<BashResult, cyrup_core::ToolError> {
    let status = status?;
    if let Some(rest) = buffer.flush()
        && let Some(cb) = on_chunk.as_mut()
    {
        cb(&rest);
    }
    let FinishedOutput {
        output,
        truncated,
        full_output_path,
        spill_error,
    } = buffer.finish();
    // [CYRUP-DELTA] A spill that failed fails the call whatever the command did, because the full
    // output it names is incomplete: the agent-loop `bash` tool's `TOOL-059` choice, here for the
    // user `!` path (see `BashOutputBuffer::spill_error`). As with any backend failure, the caller
    // records no history entry. The output already streamed to the user, so the message leads with
    // how the command ended rather than repeating it.
    if let Some(message) = spill_error {
        let ended = match status {
            ExitStatus::Exited(code) => format!("Command exited with code {code}"),
            ExitStatus::Signaled(Some(signo)) => {
                format!("Command exited with code {}", 128 + signo)
            }
            ExitStatus::Signaled(None) => "Command terminated without an exit code".to_string(),
            ExitStatus::Killed => "Command aborted".to_string(),
            ExitStatus::TimedOut => "Command timed out".to_string(),
        };
        return Err(cyrup_core::ToolError::new(format!(
            "{ended}\n\nFull output could not be saved: {message}"
        )));
    }

    match status {
        ExitStatus::Exited(code) => Ok(BashResult {
            output,
            exit_code: Some(code),
            cancelled: false,
            truncated,
            full_output_path,
        }),
        // TOOL-047 — pi's user bash runs on the same local shell operations as the tool, which report
        // a signal-killed shell as `128 + signo` since v0.86.0 (`tools/bash.ts:139-142` @v0.87.1).
        // Without a signal number (a guest backend's `exitCode: null`) pi's `executeBashWithOperations`
        // keeps `exitCode: undefined` (`core/bash-executor.ts:125`).
        ExitStatus::Signaled(signo) => Ok(BashResult {
            output,
            exit_code: signo.map(|n| 128 + n),
            cancelled: false,
            truncated,
            full_output_path,
        }),
        ExitStatus::Killed => Ok(BashResult {
            output,
            exit_code: None,
            cancelled: true,
            truncated,
            full_output_path,
        }),
        ExitStatus::TimedOut => Ok(BashResult {
            output,
            exit_code: None,
            cancelled: false,
            truncated,
            full_output_path,
        }),
    }
}

/// Build the `bashExecution` custom-message payload Pi records (`recordBashResult`,
/// agent-session.ts:2803-2814 @v0.83.0).
///
/// SEAM-083 — the two optional fields are OMITTED when absent, not emitted as `null`. Upstream builds
/// this object with `exitCode: result.exitCode` and `fullOutputPath: result.fullOutputPath`
/// (`:2808`/`:2811`), both `undefined` when the command was killed or the output did not spill, and
/// the message is persisted and re-emitted through `JSON.stringify`, which drops an `undefined`
/// value. This payload rides the same stdout stream as the `bash` response (it becomes a custom
/// message inside `message_update`), so a `null` here is the same client-visible divergence the
/// response's own keys had.
pub(crate) fn bash_message_payload(
    command: &str,
    result: &BashResult,
    exclude_from_context: bool,
) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert("command".into(), serde_json::Value::from(command));
    payload.insert(
        "output".into(),
        serde_json::Value::from(result.output.clone()),
    );
    if let Some(code) = result.exit_code {
        payload.insert("exitCode".into(), serde_json::Value::from(code));
    }
    payload.insert(
        "cancelled".into(),
        serde_json::Value::from(result.cancelled),
    );
    payload.insert(
        "truncated".into(),
        serde_json::Value::from(result.truncated),
    );
    if let Some(path) = &result.full_output_path {
        payload.insert(
            "fullOutputPath".into(),
            serde_json::Value::from(path.clone()),
        );
    }
    payload.insert(
        "excludeFromContext".into(),
        serde_json::Value::from(exclude_from_context),
    );
    serde_json::Value::Object(payload)
}

/// Streaming sanitize + rolling-buffer + tempfile-spill for immediate-bash output — a direct port of
/// Pi's hand-rolled pipeline inside `executeBashWithOperations`'s `onData`/success path
/// (`bash-executor.ts:48-146` @v1.1.0). Deliberately NOT `cyrup_tools::output::OutputAccumulator`:
/// that type backs the AGENT-LOOP `bash` TOOL (Pi's shared `OutputAccumulator` class, `output-accumulator.ts`,
/// used by `tools/bash.ts` — which never sanitizes) and has a different spill threshold. THIS seam's
/// real Pi consumer, `bash-executor.ts`, sanitizes (strips ANSI, filters unsafe control/format
/// chars, drops CR) EVERY chunk before it ever lands in the rolling buffer or the temp file, and
/// gates the temp-file spill on raw byte count alone (no line-count trigger mid-stream).
struct BashOutputBuffer {
    /// Incomplete trailing UTF-8 sequence carried across chunks (mirrors `TextDecoder{stream:true}`
    /// state). [`Self::flush`] ends the stream as pi's final no-argument `decoder.decode()` does
    /// (`flushOutput`, `bash-executor.ts:113-117` @v1.1.0): an incomplete sequence left here
    /// becomes one U+FFFD.
    pending: Vec<u8>,
    /// Unfinished escape sequence at the end of the decoded text so far, held back until the next
    /// chunk completes it (Pi `pendingAnsi`, `bash-executor.ts:73-74` @v1.1.0, `27c7b6ff4`), so a
    /// color code split across chunks is stripped whole instead of leaving a stray `m`.
    pending_ansi: String,
    /// Raw bytes seen so far, PRE-sanitize (Pi `totalBytes`) — gates the lazy temp-file open.
    total_raw_bytes: usize,
    /// Rolling sanitized-text chunks kept in memory (Pi `outputChunks`).
    chunks: Vec<String>,
    /// `chunks`' total byte length (Pi `outputBytes`).
    chunks_bytes: usize,
    temp_file: Option<std::fs::File>,
    temp_path: Option<PathBuf>,
    /// Where the spill file is created: the OS temp directory (pi `tmpdir()`,
    /// `utils/output-files.ts:19` @v1.1.0). A test points it at a directory that does not exist.
    spill_dir: PathBuf,
    /// The first failure to create or write the spill file. [`run_bash`] fails the call with it, as
    /// the agent-loop `bash` tool does with `OutputAccumulator::take_spill_error` (`TOOL-059`).
    /// [CYRUP-DELTA] pi's `createOutputFileStream` (`utils/output-files.ts:31-34` @v1.1.0) adds no
    /// `error` listener and `executeBashWithOperations` never awaits the stream, so a failed open
    /// or write is an uncaught stream `error` there, not a result. Once set, the spill is not
    /// retried and later text goes only to the rolling buffer, like pi's errored stream.
    spill_error: Option<(PathBuf, std::io::Error)>,
}

/// What [`BashOutputBuffer::finish`] hands [`run_bash`].
struct FinishedOutput {
    /// The tail-truncated output (Pi `BashResult.output`).
    output: String,
    truncated: bool,
    full_output_path: Option<String>,
    /// `"{path}: {error}"` when the spill file lost part of the output.
    spill_error: Option<String>,
}

/// Rolling in-memory preview cap (Pi `maxOutputBytes`, `bash-executor.ts:56` @v1.1.0: `DEFAULT_MAX_BYTES * 2`).
const ROLLING_MAX_BYTES: usize = DEFAULT_MAX_BYTES * 2;

impl BashOutputBuffer {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            pending_ansi: String::new(),
            total_raw_bytes: 0,
            chunks: Vec::new(),
            chunks_bytes: 0,
            temp_file: None,
            temp_path: None,
            spill_dir: std::env::temp_dir(),
            spill_error: None,
        }
    }

    /// Pi's `onData` (`bash-executor.ts:106-111` @v1.1.0): decode one raw chunk, hold back an
    /// unfinished escape sequence at its end, and pass the rest to [`Self::append_text`]. Returns
    /// the sanitized text for `on_chunk`, or `None` when there is none.
    fn push_raw(&mut self, data: &[u8]) -> Option<String> {
        self.total_raw_bytes += data.len();
        let decoded = self.decode_streaming(data);
        let mut text = std::mem::take(&mut self.pending_ansi);
        text.push_str(&decoded);
        let (complete, pending) = split_incomplete_ansi_suffix(&text);
        self.pending_ansi = pending.to_owned();
        self.append_text(complete)
    }

    /// Pi's `flushOutput` (`bash-executor.ts:113-117` @v1.1.0), run once the command has ended:
    /// the held-back escape sequence and the decoder's end of stream (one U+FFFD for an incomplete
    /// UTF-8 sequence), through [`Self::append_text`]. Returns the text for `on_chunk`, if any.
    fn flush(&mut self) -> Option<String> {
        let mut rest = std::mem::take(&mut self.pending_ansi);
        if !self.pending.is_empty() {
            self.pending.clear();
            rest.push('\u{FFFD}');
        }
        self.append_text(&rest)
    }

    /// Sanitize decoded text and fold it into the temp file / rolling buffer (Pi `appendText`,
    /// `bash-executor.ts:76-104` @v1.1.0, in the SAME order: spill-check, then temp-file write,
    /// then rolling-buffer push+evict). Text that sanitizes to nothing is dropped before any of it,
    /// and is not streamed.
    fn append_text(&mut self, raw: &str) -> Option<String> {
        let sanitized = sanitize_chunk(raw);
        if sanitized.is_empty() {
            return None;
        }

        // Lazily open the temp file once RAW bytes exceed the spill threshold (Pi:
        // `if (totalBytes > DEFAULT_MAX_BYTES) ensureTempFile();`, bash-executor.ts:84-86) — BEFORE
        // this chunk is folded into `chunks`, exactly mirroring Pi's ordering.
        if self.total_raw_bytes > DEFAULT_MAX_BYTES {
            self.ensure_temp_file();
        }
        self.write_spill(&sanitized);

        self.chunks_bytes += sanitized.len();
        self.chunks.push(sanitized.clone());
        // Rolling cap: drop the oldest chunks once the in-memory preview exceeds 2x the spill
        // threshold (Pi `maxOutputBytes`, bash-executor.ts:95-98).
        while self.chunks_bytes > ROLLING_MAX_BYTES && self.chunks.len() > 1 {
            let removed = self.chunks.remove(0);
            self.chunks_bytes = self.chunks_bytes.saturating_sub(removed.len());
        }

        Some(sanitized)
    }

    /// Write to the open spill file, recording the first failure (see [`Self::spill_error`]).
    fn write_spill(&mut self, text: &str) {
        let Some(file) = self.temp_file.as_mut() else {
            return;
        };
        if let Err(e) = file.write_all(text.as_bytes())
            && let Some(path) = self.temp_path.clone()
        {
            self.temp_file = None;
            self.spill_error = Some((path, e));
        }
    }

    /// Streaming UTF-8 decode with a carried-over incomplete-sequence tail (mirrors
    /// `TextDecoder.decode(data, { stream: true })`). Never indexes/panics: uses `get`/`drain` with
    /// lengths `str::from_utf8`'s own `Utf8Error` guarantees are in-bounds.
    fn decode_streaming(&mut self, data: &[u8]) -> String {
        self.pending.extend_from_slice(data);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if let Some(good) = self.pending.get(..valid) {
                        out.push_str(&String::from_utf8_lossy(good));
                    }
                    match e.error_len() {
                        Some(bad) => {
                            // A complete-but-invalid subsequence → one replacement char now, then
                            // keep scanning the rest of `pending` for more valid/invalid runs.
                            out.push('\u{FFFD}');
                            let drain_to = valid.saturating_add(bad).min(self.pending.len());
                            self.pending.drain(..drain_to);
                        }
                        None => {
                            // Incomplete trailing sequence: keep it for the next chunk.
                            self.pending.drain(..valid.min(self.pending.len()));
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Pi `ensureTempFile` (`bash-executor.ts:62-70` @v1.1.0): open the spill file once and write
    /// the rolling buffer into it. A failure is recorded (see [`Self::spill_error`]) and the spill
    /// is not tried again.
    fn ensure_temp_file(&mut self) {
        if self.temp_file.is_some() || self.spill_error.is_some() {
            return;
        }
        let path = self
            .spill_dir
            .join(format!("cyrup-bash-{}.log", unique_temp_suffix()));
        let opened = cyrup_tools::output::create_output_file(&path).and_then(|mut file| {
            for chunk in &self.chunks {
                file.write_all(chunk.as_bytes())?;
            }
            Ok(file)
        });
        match opened {
            Ok(file) => {
                self.temp_file = Some(file);
                self.temp_path = Some(path);
            }
            Err(e) => self.spill_error = Some((path, e)),
        }
    }

    /// Compute the final tail-truncated `output` (Pi `truncateTail(fullOutput)`,
    /// `bash-executor.ts:131-132` @v1.1.0), force the temp file open if truncation demands one Pi's
    /// raw-byte spill check never triggered on (a many-short-lines overflow, `:133-135`), and
    /// report `full_output_path` as Pi's unconditional `fullOutputPath: tempFilePath` (`:144`):
    /// whatever path is open, regardless of the final `truncated` value.
    fn finish(mut self) -> FinishedOutput {
        let full_output = self.chunks.concat();
        let truncation = cyrup_tools::truncate::truncate_tail(
            &full_output,
            TruncOpts::new(DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES),
        );
        if truncation.info.truncated {
            self.ensure_temp_file();
        }
        if let Some(f) = self.temp_file.as_mut()
            && let Err(e) = f.flush()
            && let Some(path) = self.temp_path.clone()
        {
            self.spill_error = Some((path, e));
        }
        let output = if truncation.info.truncated {
            truncation.content
        } else {
            full_output
        };
        FinishedOutput {
            output,
            truncated: truncation.info.truncated,
            full_output_path: self.temp_path.map(|p| p.to_string_lossy().into_owned()),
            spill_error: self
                .spill_error
                .map(|(path, e)| format!("{}: {e}", path.display())),
        }
    }
}

/// Pi's per-chunk sanitize pipeline (`bash-executor.ts:82`): strip ANSI, filter unsafe control/
/// format characters, then drop every carriage return.
fn sanitize_chunk(text: &str) -> String {
    sanitize_binary_output(&strip_ansi(text)).replace('\r', "")
}

/// Filter characters that crash string-width / break terminal rendering (Pi `sanitizeBinaryOutput`,
/// `utils/shell.ts:144-174`): keep tab/newline/CR, drop other C0 control chars (0x00-0x1F) and the
/// Unicode format-character range U+FFF9..=U+FFFB. Iterates by Rust `char` (= Unicode scalar value),
/// the same code-point granularity as Pi's `Array.from(str)` — and, unlike a JS UTF-16 string, a
/// Rust `str` cannot contain a lone surrogate at all, so no extra filtering is needed for that case.
fn sanitize_binary_output(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            let code = c as u32;
            if code == 0x09 || code == 0x0A || code == 0x0D {
                return true;
            }
            if code <= 0x1F {
                return false;
            }
            !(0xFFF9..=0xFFFB).contains(&code)
        })
        .collect()
}

/// Strip ANSI escape sequences (Pi `stripAnsi`, `utils/ansi.ts`): OSC sequences (`ESC ] ... ST`,
/// non-greedy up to the first terminator) and CSI/related sequences (`ESC`/C1 CSI, optional
/// intermediates, optional numeric params, one final byte) — ported from the exact `ansi-regex`
/// grammar Pi vendors, as a hand-rolled scanner (no indexing; `Chars::as_str()`/`strip_prefix`
/// only) since this crate has no general-purpose regex dependency.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let mut it = rest.chars();
        let Some(c) = it.next() else { break };
        let tail = it.as_str();

        if c == '\u{1B}' {
            if let Some(after) = rest.strip_prefix("\u{1B}]")
                && let Some(end) = find_osc_terminator(after)
            {
                rest = end;
                continue;
            }
            if let Some(end) = try_csi(rest) {
                rest = end;
                continue;
            }
        } else if c == '\u{9B}'
            && let Some(end) = try_csi(rest)
        {
            rest = end;
            continue;
        }
        out.push(c);
        rest = tail;
    }
    out
}

/// Scan past an OSC sequence's terminator (BEL, `ESC \`, or C1 ST 0x9C), non-greedy — `after` is
/// everything following the `ESC ]` introducer. Returns the remaining slice past the terminator, or
/// `None` if no terminator exists before the end of the string (the regex's `osc` alternative then
/// simply fails to match at this position, exactly like a JS regex with no backtracking-past-end).
fn find_osc_terminator(after: &str) -> Option<&str> {
    let mut rest = after;
    loop {
        let mut it = rest.chars();
        let c = it.next()?;
        let tail = it.as_str();
        match c {
            '\u{07}' | '\u{9C}' => return Some(tail),
            '\u{1B}' => {
                if let Some(t2) = tail.strip_prefix('\u{5C}') {
                    return Some(t2);
                }
                rest = tail;
            }
            _ => rest = tail,
        }
    }
}

/// Match a CSI/related sequence starting at `rest`'s first char (already known to be the ESC/0x9B
/// introducer). Returns the slice past the match, or `None` if no valid final byte is found (the
/// whole CSI alternative fails to match at this position).
fn try_csi(rest: &str) -> Option<&str> {
    let mut it = rest.chars();
    it.next()?; // the introducer itself (ESC or 0x9B), already checked by the caller
    let mut cur = it.as_str();

    // Intermediates: zero or more of `[ ] ( ) # ; ?` (the regex's `[[\]()#;?]*`).
    loop {
        let mut it2 = cur.chars();
        match it2.next() {
            Some('[' | ']' | '(' | ')' | '#' | ';' | '?') => cur = it2.as_str(),
            _ => break,
        }
    }

    // Optional numeric params: `(?:\d{1,4}(?:[;:]\d{0,4})*)?`.
    cur = consume_params(cur);

    // Exactly one final byte.
    let mut it3 = cur.chars();
    let final_byte = it3.next()?;
    if is_csi_final_byte(final_byte) {
        Some(it3.as_str())
    } else {
        None
    }
}

/// `(?:\d{1,4}(?:[;:]\d{0,4})*)?` — present only if the FIRST char is a digit.
fn consume_params(input: &str) -> &str {
    let starts_with_digit = input.chars().next().is_some_and(|c| c.is_ascii_digit());
    if !starts_with_digit {
        return input;
    }
    let mut cur = consume_digits(input, 4);
    loop {
        let mut it = cur.chars();
        match it.next() {
            Some(';') | Some(':') => cur = consume_digits(it.as_str(), 4),
            _ => break,
        }
    }
    cur
}

/// Consume up to `max` ASCII digits, returning the slice past them.
fn consume_digits(input: &str, max: usize) -> &str {
    let mut cur = input;
    let mut count = 0;
    while count < max {
        let mut it = cur.chars();
        match it.next() {
            Some(c) if c.is_ascii_digit() => {
                cur = it.as_str();
                count += 1;
            }
            _ => break,
        }
    }
    cur
}

/// The regex's `[\dA-PR-TZcf-nq-uy=><~]` final-byte class.
fn is_csi_final_byte(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(c,
            'A'..='P' | 'R'..='T' | 'Z' | 'c'..='n' | 'q'..='u' | 'y' | '=' | '>' | '<' | '~'
        )
}

/// Longest unfinished sequence held back while streaming, in UTF-16 units; a longer one is
/// processed as-is (Pi `MAX_PENDING_ANSI_LENGTH`, `utils/ansi.ts:49` @v1.1.0).
const MAX_PENDING_ANSI_LENGTH: usize = 256;

/// Split streamed text into the part that is safe to strip now and a trailing unfinished escape
/// sequence to prepend to the next chunk (Pi `splitIncompleteAnsiSuffix`, `utils/ansi.ts:55-66`
/// @v1.1.0). The pending part is the leftmost suffix, starting within the last
/// [`MAX_PENDING_ANSI_LENGTH`] UTF-16 units, that `unfinishedAnsiAtEndRegex` (`:44-47`) matches:
/// see [`is_unfinished_ansi`].
fn split_incomplete_ansi_suffix(value: &str) -> (&str, &str) {
    if !value.contains(['\u{1B}', '\u{9B}']) {
        return (value, "");
    }
    let window_start = value
        .encode_utf16()
        .count()
        .saturating_sub(MAX_PENDING_ANSI_LENGTH);
    let mut units = 0;
    for (index, c) in value.char_indices() {
        if units >= window_start
            && matches!(c, '\u{1B}' | '\u{9B}')
            && let Some((complete, pending)) = value.split_at_checked(index)
            && is_unfinished_ansi(pending)
        {
            return (complete, pending);
        }
        units += c.len_utf16();
    }
    (value, "")
}

/// Whether all of `tail` (which starts with ESC or 0x9B) is an unfinished sequence, the
/// `unfinishedAnsiAtEndRegex` of `utils/ansi.ts:44-47` @v1.1.0: an OSC without its terminator
/// (`ESC ]` then no BEL, no 0x9C and no `ESC \`; a trailing ESC may start `ESC \`), or a CSI
/// introducer with intermediates and params but no final byte.
fn is_unfinished_ansi(tail: &str) -> bool {
    if let Some(body) = tail.strip_prefix("\u{1B}]") {
        let mut chars = body.chars().peekable();
        let mut terminated = false;
        while let Some(c) = chars.next() {
            if matches!(c, '\u{07}' | '\u{9C}') || (c == '\u{1B}' && chars.peek() == Some(&'\\')) {
                terminated = true;
                break;
            }
        }
        if !terminated {
            return true;
        }
    }
    // `CSI_START` alone up to the end: `[\u001B\u009B][[\]()#;?]*(?:\d{1,4}(?:[;:]\d{0,4})*)?$`.
    // The params cannot start with an intermediate, so the greedy intermediates decide the split.
    let mut it = tail.chars();
    if !matches!(it.next(), Some('\u{1B}' | '\u{9B}')) {
        return false;
    }
    let rest = it
        .as_str()
        .trim_start_matches(['[', ']', '(', ')', '#', ';', '?']);
    rest.is_empty()
        || (rest.starts_with(|c: char| c.is_ascii_digit()) && consume_params(rest).is_empty())
}

/// Process-unique-ish suffix for the spill temp-file name (no rng dependency) — the same scheme as
/// `cyrup_tools::ops::local::unique_suffix`, duplicated locally rather than widening that
/// `pub(crate)` helper's visibility for one caller.
fn unique_temp_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{pid:x}-{nanos:x}-{n:x}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod sanitize_tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_sgr_color_codes() {
        assert_eq!(strip_ansi("\u{1B}[31mred\u{1B}[0m"), "red");
        assert_eq!(strip_ansi("\u{1B}[1;31mbold red\u{1B}[0m"), "bold red");
    }

    #[test]
    fn strip_ansi_removes_cursor_movement() {
        assert_eq!(strip_ansi("a\u{1B}[2Kb\u{1B}[1Gc"), "abc");
    }

    #[test]
    fn strip_ansi_removes_osc_hyperlink() {
        // `ESC ] 8 ; ; url BEL text ESC ] 8 ; ; BEL` (OSC 8 hyperlink).
        let input = "\u{1B}]8;;http://example.com\u{07}text\u{1B}]8;;\u{07}";
        assert_eq!(strip_ansi(input), "text");
    }

    #[test]
    fn strip_ansi_removes_osc_terminated_by_esc_backslash() {
        let input = "\u{1B}]0;title\u{1B}\u{5C}rest";
        assert_eq!(strip_ansi(input), "rest");
    }

    #[test]
    fn strip_ansi_passthrough_when_no_escape_present() {
        let plain = "just plain text, no escapes\nhere";
        assert_eq!(strip_ansi(plain), plain);
    }

    #[test]
    fn strip_ansi_leaves_a_fully_unmatched_escape_untouched() {
        // No ST ever arrives, so the `osc` alternative fails; the CSI alternative is then tried at
        // the SAME position (`]` is a valid CSI intermediate byte) but ALSO fails here since `X` is
        // not in the final-byte class `[\dA-PR-TZcf-nq-uy=><~]` — so nothing at all matches and the
        // whole literal string survives byte-for-byte, exactly like a JS global regex replace that
        // never matches.
        let input = "\u{1B}]XYZrest";
        assert_eq!(strip_ansi(input), input);
    }

    #[test]
    fn strip_ansi_a_lone_final_byte_char_right_after_osc_introducer_still_matches_as_csi() {
        // A faithful-to-the-regex quirk, not a bug: `ESC ] u` matches the CSI alternative (`]` as an
        // intermediate byte, `u` as a valid final byte in the `q-u` range) even though it was
        // "meant" to start an OSC sequence — Pi's real `ansi-regex` grammar has the exact same
        // behavior (alternation tries `osc` first, falls back to `csi` at the same position).
        assert_eq!(strip_ansi("\u{1B}]unterminated"), "nterminated");
    }

    #[test]
    fn sanitize_binary_output_keeps_tab_newline_cr_drops_other_controls() {
        let input = "a\tb\nc\rd\u{01}e\u{1F}f";
        assert_eq!(sanitize_binary_output(input), "a\tb\nc\rdef");
    }

    #[test]
    fn sanitize_binary_output_drops_unicode_format_chars() {
        let input = "a\u{FFF9}b\u{FFFA}c\u{FFFB}d";
        assert_eq!(sanitize_binary_output(input), "abcd");
    }

    #[test]
    fn sanitize_chunk_strips_ansi_then_sanitizes_then_drops_cr() {
        assert_eq!(sanitize_chunk("\u{1B}[31mred\u{1B}[0m\r\n"), "red\n");
    }

    /// Deterministic xorshift PRNG (no external `rand` dependency) so this is reproducible.
    struct Xorshift(u64);
    impl Xorshift {
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
    }

    /// Robustness fuzz: `strip_ansi`/`sanitize_binary_output`/`sanitize_chunk` must NEVER panic,
    /// no matter how adversarial the input — heavy on ESC/0x9B/BEL/digits/separators (the exact
    /// alphabet the hand-rolled scanner branches on) so the fuzz actually stresses every branch,
    /// plus arbitrary Unicode scalar values to cover the sanitize filter. Bounded to a fixed
    /// iteration count for a fast, deterministic `cargo test` run.
    #[test]
    fn strip_ansi_and_sanitize_never_panic_on_adversarial_input() {
        let alphabet = [
            '\u{1B}', '\u{9B}', '\u{07}', '\u{9C}', '\\', '[', ']', '(', ')', '#', ';', ':', '?',
            '0', '1', '9', 'm', 'A', 'Z', 'c', 'n', 'q', 'u', 'y', 'X', '=', '>', '<', '~', '\r',
            '\n', '\t', 'a', ' ',
        ];
        let mut rng = Xorshift(0x9E3779B97F4A7C15);
        for _ in 0..2000 {
            let len = (rng.next_u64() % 40) as usize;
            let mut s = String::new();
            for _ in 0..len {
                // Mostly draw from the adversarial alphabet; occasionally inject an arbitrary
                // Unicode scalar value (via `char::from_u32` over the full valid range, retrying on
                // a surrogate-range miss) to exercise `sanitize_binary_output`'s code-point filter.
                if rng.next_u64().is_multiple_of(5) {
                    let mut cp = (rng.next_u64() % 0x11_0000) as u32;
                    while char::from_u32(cp).is_none() {
                        cp = cp.wrapping_add(1) % 0x11_0000;
                    }
                    if let Some(c) = char::from_u32(cp) {
                        s.push(c);
                    }
                } else {
                    let idx = (rng.next_u64() as usize) % alphabet.len();
                    if let Some(&c) = alphabet.get(idx) {
                        s.push(c);
                    }
                }
            }
            // Must not panic; the exact content isn't asserted here (correctness is covered by the
            // targeted cases above and the live Node cross-check) — this test is purely a
            // no-panic/robustness guard.
            let stripped = strip_ansi(&s);
            let _ = sanitize_binary_output(&stripped);
            let _ = sanitize_chunk(&s);
            // The split never loses or reorders text, and holds back only a sequence's start.
            let (complete, pending) = split_incomplete_ansi_suffix(&s);
            assert_eq!(format!("{complete}{pending}"), s);
            assert!(pending.is_empty() || pending.starts_with(['\u{1B}', '\u{9B}']));
        }
    }

    /// `splitIncompleteAnsiSuffix` (`utils/ansi.ts:55-66` @v1.1.0, `27c7b6ff4`).
    #[test]
    fn split_holds_back_only_an_unfinished_sequence_at_the_end() {
        let cases = [
            ("plain", ("plain", "")),
            ("a\u{1B}[31mb", ("a\u{1B}[31mb", "")),
            ("ERROR\u{1B}[0", ("ERROR", "\u{1B}[0")),
            ("before\u{1B}", ("before", "\u{1B}")),
            ("x\u{9B}1;2:", ("x", "\u{9B}1;2:")),
            ("x\u{1B}[?", ("x", "\u{1B}[?")),
            // Five digits cannot be params, and the fifth is a final byte: complete.
            ("x\u{1B}[12345", ("x\u{1B}[12345", "")),
            ("a\u{1B}]0;window ", ("a", "\u{1B}]0;window ")),
            // A trailing ESC may start `ESC \`.
            ("a\u{1B}]0;title\u{1B}", ("a", "\u{1B}]0;title\u{1B}")),
            ("a\u{1B}]0;t\u{07}b", ("a\u{1B}]0;t\u{07}b", "")),
            ("a\u{1B}]0;t\u{1B}\\b", ("a\u{1B}]0;t\u{1B}\\b", "")),
            // The leftmost start wins: the OSC swallows the later CSI introducer.
            ("a\u{1B}]t\u{1B}[3", ("a", "\u{1B}]t\u{1B}[3")),
        ];
        for (input, expected) in cases {
            assert_eq!(split_incomplete_ansi_suffix(input), expected, "{input:?}");
        }
        // Only the last 256 UTF-16 units are searched, so a longer unterminated OSC is not held.
        let long = format!("\u{1B}]{}", "x".repeat(300));
        assert_eq!(split_incomplete_ansi_suffix(&long), (long.as_str(), ""));
        let short = format!("ok\u{1B}]{}", "x".repeat(250));
        assert_eq!(split_incomplete_ansi_suffix(&short).0, "ok");
    }

    /// Feed `chunks` through the buffer as a command's output, then settle it with exit 0;
    /// returns the result's output and everything streamed to the sink.
    fn run_chunks(chunks: &[&[u8]]) -> (String, String) {
        let streamed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink_out = streamed.clone();
        let mut sink: BashChunkSink = Some(Box::new(move |delta: &str| {
            sink_out.lock().unwrap().push_str(delta);
        }));
        let mut buffer = BashOutputBuffer::new();
        for chunk in chunks {
            if let Some(text) = buffer.push_raw(chunk)
                && let Some(cb) = sink.as_mut()
            {
                cb(&text);
            }
        }
        let result = settle(buffer, Ok(ExitStatus::Exited(0)), &mut sink).unwrap();
        let streamed = streamed.lock().unwrap().clone();
        (result.output, streamed)
    }

    /// pi `27c7b6ff4` (#10504): an escape sequence split across output chunks is stripped whole,
    /// in the result and in what was streamed, instead of leaving a stray `m`
    /// (`agent-session-bash-persistence.test.ts` "escape sequences split across output chunks").
    /// RED before: `"ERROR: file.py:1[0m\n"` (the ESC was dropped, its `[0` and `m` kept).
    #[test]
    fn escape_sequences_split_across_chunks_are_stripped_whole() {
        assert_eq!(
            run_chunks(&[b"\x1b[31mERROR: file.py:1\x1b[0", b"m\n"]),
            (
                "ERROR: file.py:1\n".to_owned(),
                "ERROR: file.py:1\n".to_owned()
            )
        );
        assert_eq!(
            run_chunks(&[b"before\x1b", b"[32mafter\n"]),
            ("beforeafter\n".to_owned(), "beforeafter\n".to_owned())
        );
        assert_eq!(
            run_chunks(&[b"a\x1b]0;window ", b"title\x1b", b"\\b\n"]).0,
            "ab\n"
        );
    }

    /// pi `flushOutput` (`bash-executor.ts:113-117` @v1.1.0): the decoder's end of stream turns an
    /// incomplete multi-byte character into U+FFFD, in the result and the stream.
    /// RED before: the trailing byte was dropped, `"ok"`.
    #[test]
    fn an_incomplete_multi_byte_character_at_the_end_is_flushed() {
        let e_acute = "\u{e9}".as_bytes();
        assert_eq!(
            run_chunks(&[b"ok", &e_acute[..1]]),
            ("ok\u{FFFD}".to_owned(), "ok\u{FFFD}".to_owned())
        );
    }

    /// A long unterminated sequence is not held back: it streams before the command ends.
    #[test]
    fn a_long_unterminated_sequence_is_not_held_back() {
        let long = "x".repeat(300);
        let mut buffer = BashOutputBuffer::new();
        let streamed = buffer.push_raw(format!("\x1b]{long}").as_bytes());
        assert_eq!(streamed, Some(format!("]{long}")));
    }

    /// Text that sanitizes to nothing is neither buffered nor streamed (`if (!text) return`).
    #[test]
    fn text_that_sanitizes_to_nothing_is_not_streamed() {
        let mut buffer = BashOutputBuffer::new();
        assert_eq!(buffer.push_raw(b"\x1b[0m"), None);
        assert_eq!(buffer.push_raw(b"\x1b[3"), None);
        assert_eq!(buffer.push_raw(b"1m\r"), None);
        assert!(buffer.chunks.is_empty());
        assert_eq!(buffer.flush(), None);
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod spill_file_tests {
    use super::{BashOutputBuffer, DEFAULT_MAX_BYTES, settle};
    use cyrup_tools::ExitStatus;
    use std::os::unix::fs::PermissionsExt as _;

    /// TOOL-057 — the user-`!` bash spill (`bash-executor.ts:66` `createOutputFileStream`, v1.0.4)
    /// is readable only by the user. RED before: `File::create`, measured `644`.
    #[test]
    fn the_full_output_file_is_readable_only_by_its_owner() {
        let mut buffer = BashOutputBuffer::new();
        buffer.push_raw(&vec![b'x'; DEFAULT_MAX_BYTES + 1]);
        let finished = buffer.finish();
        let path = finished.full_output_path.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&path);
        assert!(finished.truncated);
        assert!(finished.spill_error.is_none());
        assert_eq!(mode, 0o600);
    }

    /// TOOL-060 — a spill file that cannot be created is reported, not dropped: the call fails with
    /// how the command ended and `"{path}: {error}"` whatever the exit code, as the agent-loop
    /// `bash` tool does (TOOL-059).
    /// RED before: `ensure_temp_file` ignored the failed create and the call returned `Ok`.
    #[test]
    fn a_spill_file_that_cannot_be_created_fails_the_call() {
        for code in [0, 3] {
            let mut buffer = BashOutputBuffer::new();
            buffer.spill_dir = std::env::temp_dir().join("cyrup-missing-dir-for-bash-spill");
            buffer.push_raw(&vec![b'x'; DEFAULT_MAX_BYTES + 1]);
            buffer.push_raw(b"more");
            let error = settle(buffer, Ok(ExitStatus::Exited(code)), &mut None).unwrap_err();
            assert!(
                error.message.starts_with(&format!(
                    "Command exited with code {code}\n\nFull output could not be saved: "
                )),
                "{error:?}"
            );
            assert!(
                error.message.contains("cyrup-missing-dir-for-bash-spill"),
                "{error:?}"
            );
        }
    }

    /// A spill file that was created but then refuses a write is reported the same way, and the
    /// spill stops there: later text goes only to the rolling buffer.
    #[test]
    fn a_spill_write_that_fails_is_reported_and_ends_the_spill() {
        let mut buffer = BashOutputBuffer::new();
        buffer.push_raw(&vec![b'x'; DEFAULT_MAX_BYTES + 1]);
        let path = buffer.temp_path.clone().unwrap();
        // Swap the writable handle for a read-only one so the next write fails with EBADF.
        buffer.temp_file = Some(std::fs::File::open(&path).unwrap());
        buffer.push_raw(b"lost");
        assert!(
            buffer.temp_file.is_none(),
            "a failed write closes the spill"
        );
        buffer.push_raw(b"later");
        let finished = buffer.finish();
        let kept = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let message = finished.spill_error.unwrap();
        assert!(
            message.starts_with(&format!("{}: ", path.display())),
            "{message}"
        );
        assert_eq!(kept.len(), DEFAULT_MAX_BYTES + 1);
        assert!(finished.output.ends_with("lostlater"));
    }

    /// The control: the same output with a writable spill directory settles as a result that
    /// names the file.
    #[test]
    fn a_spill_that_succeeds_settles_as_a_result() {
        let mut buffer = BashOutputBuffer::new();
        buffer.push_raw(&vec![b'x'; DEFAULT_MAX_BYTES + 1]);
        let result = settle(buffer, Ok(ExitStatus::Exited(3)), &mut None).unwrap();
        let path = result.full_output_path.unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(result.exit_code, Some(3));
        assert!(result.truncated);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod wire_shape_tests {
    use super::{BashResult, bash_message_payload};

    /// SEAM-083 — RED before this pass on the FIRST assertion of each half.
    ///
    /// The RPC `bash` handler arm is a bare `serde_json::to_value(result)`
    /// (`cyrup-modes/src/rpc.rs`), so `BashResult`'s serde attributes ARE the wire contract. Both
    /// optionals carried `#[serde(default)]` (a READ-side allowance for an extension-supplied
    /// `user_bash` override) but no `skip_serializing_if`, so every response shipped
    /// `"fullOutputPath":null`, and a killed command shipped `"exitCode":null`.
    ///
    /// Upstream: `exitCode: number | undefined` (`core/bash-executor.ts:33` @v0.83.0 — a required key
    /// whose `undefined` value `JSON.stringify` drops) and `fullOutputPath?: string` (`:39`).
    /// `docs/rpc.md:473-479` @v0.83.0 shows the normal response with NO `fullOutputPath` key and
    /// `:482-495` shows it appearing only when truncated — which is why the documented client test
    /// for truncation is `"fullOutputPath" in data`, a test that was true on every cyrup response.
    #[test]
    fn bash_response_omits_absent_optionals_rather_than_sending_null() {
        let normal = BashResult {
            output: "total 48\n".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
        };
        let v = serde_json::to_value(&normal).expect("BashResult serializes");
        let obj = v.as_object().expect("object");
        assert!(
            !obj.contains_key("fullOutputPath"),
            "a non-truncated bash response must omit the key entirely; got {v}"
        );
        // Presence before absence: the keys that ARE present upstream must stay present.
        assert_eq!(obj.get("exitCode"), Some(&serde_json::json!(0)));
        assert_eq!(obj.get("cancelled"), Some(&serde_json::json!(false)));
        assert_eq!(obj.get("truncated"), Some(&serde_json::json!(false)));
        assert_eq!(obj.get("output"), Some(&serde_json::json!("total 48\n")));

        let killed = BashResult {
            output: String::new(),
            exit_code: None,
            cancelled: true,
            truncated: false,
            full_output_path: None,
        };
        let v = serde_json::to_value(&killed).expect("BashResult serializes");
        let obj = v.as_object().expect("object");
        assert!(
            !obj.contains_key("exitCode"),
            "a killed/signalled command must omit exitCode, not send null; got {v}"
        );

        let spilled = BashResult {
            output: "truncated output...".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: true,
            full_output_path: Some("/tmp/cyrup-bash-abc123.log".into()),
        };
        let v = serde_json::to_value(&spilled).expect("BashResult serializes");
        assert_eq!(
            v.get("fullOutputPath"),
            Some(&serde_json::json!("/tmp/cyrup-bash-abc123.log")),
            "the truncated branch DOES carry the key — the fix must not delete it"
        );
    }

    /// SEAM-083's other half: the persisted/`message_update`-borne `bashExecution` payload, built by
    /// hand rather than through serde, had the same two `null`s. Upstream's object literal
    /// (`recordBashResult`, `agent-session.ts:2803-2814` @v0.83.0) assigns `result.exitCode` and
    /// `result.fullOutputPath` straight through, both `undefined` on these paths.
    #[test]
    fn bash_execution_message_omits_absent_optionals_too() {
        let killed = BashResult {
            output: "partial".into(),
            exit_code: None,
            cancelled: true,
            truncated: false,
            full_output_path: None,
        };
        let payload = bash_message_payload("sleep 100", &killed, false);
        let obj = payload.as_object().expect("object");
        assert!(!obj.contains_key("exitCode"), "got {payload}");
        assert!(!obj.contains_key("fullOutputPath"), "got {payload}");
        assert_eq!(obj.get("command"), Some(&serde_json::json!("sleep 100")));
        assert_eq!(obj.get("cancelled"), Some(&serde_json::json!(true)));
        assert_eq!(
            obj.get("excludeFromContext"),
            Some(&serde_json::json!(false))
        );

        let ok = BashResult {
            output: "hi".into(),
            exit_code: Some(3),
            cancelled: false,
            truncated: true,
            full_output_path: Some("/tmp/x.log".into()),
        };
        let payload = bash_message_payload("echo hi", &ok, true);
        assert_eq!(payload.get("exitCode"), Some(&serde_json::json!(3)));
        assert_eq!(
            payload.get("fullOutputPath"),
            Some(&serde_json::json!("/tmp/x.log"))
        );
        assert_eq!(
            payload.get("excludeFromContext"),
            Some(&serde_json::json!(true))
        );
    }
}
