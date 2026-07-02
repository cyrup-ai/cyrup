//! Host-side backing for the guest capability imports (arch-08 §3.6). A loaded WASM extension
//! calls the `ui`/`session`/`models`/`exec`/`ext-fs`/`bus`/`control`/`registration` imports; those
//! land in [`GuestState`], which records registrations/observable effects and delegates interactive
//! capabilities to a pluggable [`HostServices`] backend. The default backend denies all interactive
//! capability (no ambient authority, R-ARCH-EXT-011); the session service injects a real one.

use crate::event::{EventKind, Subscriptions};
use crate::native::CtxTier;
use crate::registry::{CommandDescriptor, ExtensionRegistry};
use cyrup_core::{CancelToken, ExtensionId};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// Re-exported for downstream backends (e.g. `cyrup-session-svc::LiveHostServices`), which reach
// these DTOs the same way it reaches `ExecOutput`/`DialogOptions` (`cyrup_ext::host::{..}`).
pub use crate::caps::http::{HttpRequest, HttpResponse, HttpStreamResponse};
pub use crate::caps::proc::ProcSpawnSpec;

/// Bounds how many DISTINCT ids [`GuestState::aborted_signals`] can ever hold. `ui.abort-signal`
/// (the WIT `abort-signal: func(signal-id: string)`) carries no error channel and is reachable by
/// a guest at ANY trust tier (`live.rs`'s `ui::Host` impl applies no tier guard), unlike the
/// `http-client`/`proc` registries this same DoS-cap effort already bounded (`faaf191`/`907a6f8`):
/// an unbounded `Mutex<HashSet<String>>` that unconditionally `insert`s any guest-supplied string
/// lets a malicious/broken guest grow host memory without bound simply by calling `abort-signal`
/// with a fresh id in a loop. A legitimate guest only ever aborts signal ids it itself minted for
/// its OWN in-flight dialogs/tool calls (Pi `ExtensionUIDialogOptions.signal`), a count bounded by
/// how many concurrent dialogs/tool calls that guest could possibly have open — comfortably under
/// this cap. As with `MAX_OPEN_STREAMS`/`MAX_SPAWNED_PROCESSES`, the point is FINITE, not a
/// specific magic number, and there is no Pi-derived exact count to port (Pi's `AbortController`
/// objects are ordinary garbage-collected JS values with no analogous host-side registry at all).
/// Once at the cap, a NEW distinct id is silently dropped (never marked aborted) rather than
/// erroring — the WIT signature has no error channel to surface a rejection through, and a dialog
/// bound to a dropped id simply never observes a programmatic abort (degrades to "wait for the
/// human"/"wait for the real host-side timeout", never a host crash or unbounded growth). An id
/// already tracked stays tracked (this is a bound on DISTINCT ids, not a re-insert budget).
const MAX_ABORTED_SIGNALS: usize = 4096;

/// How stale [`GuestState::last_wait_touch`] is allowed to be, at the moment
/// [`GuestState::take_dialog_extra_ticks`] runs, before the recorded wait anchor is distrusted as
/// belonging to an already-finished dialog rather than the trap that is actually firing right now.
/// A genuinely live wait chain (the blocking `ui.*`/`exec`/`proc` call that is CAUSING this exact
/// trap, or a back-to-back batch of several such calls with no intervening checkpoint) always has
/// [`GuestState::note_dialog_wait`] run within microseconds of the trap firing — wasmtime's epoch
/// checkpoint fires at the very next instrumented point once the guest resumes wasm execution, and
/// nothing but host code (which just ran `note_dialog_wait`) executes in between. Anything wider than
/// a couple of ticks' worth of pure scheduling/host-call overhead means real, untracked guest cpu
/// execution happened after the last recorded wait ended — exactly the same-dispatch stale-anchor
/// class [`GuestState::last_wait_touch`]'s doc describes. Not a Pi-derived value (Pi has no analogous
/// host-side epoch budget at all): deliberately generous relative to `epoch::DEFAULT_TICK` (5ms) to
/// absorb scheduler jitter without ever mistaking a real wait for a stale one.
const STALE_WAIT_TOUCH_GAP: std::time::Duration = std::time::Duration::from_millis(20);

/// Result of a capability-scoped `exec.run` (mirrors the WIT `exec-result`; 1:1 with Pi `ExecResult`,
/// exec.ts:23-28). `killed` is true when the host SIGTERM/SIGKILLed the process on a timeout/abort.
#[derive(Clone, Debug, Default)]
pub struct ExecOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub killed: bool,
}

/// UI dialog options bag (Pi `ExtensionUIDialogOptions`, types.ts:89): a live-countdown `timeout_ms`
/// and/or a programmatic-dismiss `signal_id` for `confirm`/`input`/`select` (host gap-08-sdk #4).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogOptions {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub signal_id: Option<String>,
}

impl DialogOptions {
    /// Parse from the guest's `opts-json` (degrades to the empty bag, never panics).
    pub fn parse(opts_json: &str) -> Self {
        serde_json::from_str(opts_json).unwrap_or_default()
    }
}

/// A command-tier session/runtime mutation requested via the `control` import (arch-08 §6.3).
#[derive(Clone, Debug)]
pub enum ControlOp {
    NewSession { opts: Value },
    Switch { session_id: String, opts: Value },
    Fork { entry_id: String, opts: Value },
    Navigate { entry_id: String, opts: Value },
    Reload,
    Compact,
    WaitIdle,
    SendMessage { message: Value, opts: Value },
    SendUserMessage { content: String, opts: Value },
    SetModel(Value),
    SetThinkingLevel(String),
}

/// The pluggable host backend for interactive capabilities (arch-08 §3.6). Every method defaults to
/// "denied / empty" so the default host grants NO ambient authority; the session service overrides
/// the ones it wants to expose. All methods are sync (the host runs them on its own executor; the
/// guest is suspended across the call by Wasmtime's async support).
pub trait HostServices: Send + Sync {
    // --- ui (R-08-022) ---
    /// `message` is Pi's `confirm(title, message, opts)` body (rpc-types.ts:232) — distinct from
    /// `prompt` (the title); denied by default (empty confirm surfaces as `false`, same as before).
    fn confirm(&self, _prompt: &str, _message: &str, _opts: &DialogOptions) -> bool {
        false
    }
    /// `placeholder` is Pi's `input(title, placeholder, opts)` optional field (rpc-types.ts:233-240).
    fn input(&self, _prompt: &str, _placeholder: Option<&str>, _opts: &DialogOptions) -> Option<String> {
        None
    }
    fn select(&self, _prompt: &str, _options: &Value, _opts: &DialogOptions) -> Option<String> {
        None
    }

    // --- provider OAuth login callbacks (Pi OAuthLoginCallbacks, host gap-08 #1) ---
    /// Prompt for a value during a guest `login` flow (Pi `onPrompt`); `Err` = cancelled.
    fn oauth_prompt(
        &self,
        _message: &str,
        _placeholder: Option<&str>,
        _allow_empty: bool,
    ) -> Result<String, String> {
        Err("oauth prompt capability not granted".into())
    }
    /// Interactive selector during a guest `login` flow (Pi `onSelect`); returns the chosen id.
    fn oauth_select(&self, _message: &str, _options: &Value) -> Option<String> {
        None
    }
    fn editor(&self, _title: &str, _initial: &str) -> Option<String> {
        None
    }
    /// A custom overlay component; returns an optional serialized result (Pi `custom()`).
    fn custom(&self, _spec: &Value) -> Option<String> {
        None
    }
    /// Current editor buffer text (Pi `getEditorText`).
    fn editor_text(&self) -> String {
        String::new()
    }
    /// Active theme name (Pi `getTheme`).
    fn theme(&self) -> Option<String> {
        None
    }
    /// Available theme names (Pi `listThemes`).
    fn theme_list(&self) -> Value {
        json!([])
    }
    /// Switch the active theme (Pi `setTheme`); denied by default.
    fn set_theme(&self, _name: &str) -> Result<(), String> {
        Err("theme capability not granted".into())
    }
    /// Whether tool rows are expanded (Pi `getToolsExpanded`).
    fn tools_expanded(&self) -> bool {
        false
    }

    // --- session read-only view (R-08-027) ---
    fn entries(&self) -> Value {
        json!([])
    }
    fn branch(&self) -> Value {
        json!([])
    }
    fn tree(&self) -> Value {
        Value::Null
    }
    fn session_name(&self) -> Option<String> {
        None
    }

    // --- models ---
    fn models(&self) -> Value {
        json!([])
    }
    fn current_model(&self) -> Option<String> {
        None
    }
    fn context_usage(&self) -> Value {
        json!({})
    }
    fn thinking_level(&self) -> Option<String> {
        None
    }

    // --- exec capability (R-08-030); denied by default ---
    /// Run a DIRECT argv (shell:false) command (Pi `execCommand`, exec.ts:34-46). `opts` is the
    /// `ExecOptions` bag (`{cwd, timeoutMs, signalId}`; NO `env` — Pi's real `execCommand` never
    /// accepts an env override, `exec.ts:41-45`); `cancel` carries an already-aborted `signalId`
    /// (the guest is wasm-suspended across this call, so the signal can only have been aborted
    /// beforehand — Pi `signal.aborted`). Denied by default (no ambient authority, R-ARCH-EXT-011).
    fn exec(
        &self,
        _cmd: &str,
        _args: &[String],
        _opts: &Value,
        _cancel: CancelToken,
    ) -> Result<ExecOutput, String> {
        Err("exec capability not granted".into())
    }

    // --- http-client capability (arch-08 §3.2 draft; pi-mcp-adapter-port.md §3.2); gated by the
    // SAME load-time trust check `exec`/`ui` use (no new bool, no per-host allowlist) — denied by
    // default (no ambient network authority, R-ARCH-EXT-011) ---

    /// A bounded HTTP request/response round trip (the WIT `http-client.request`). Denied by default.
    fn http_request(&self, _req: &HttpRequest) -> Result<HttpResponse, String> {
        Err("http-client capability not granted".into())
    }
    /// Start a streaming HTTP request (the WIT `http-client.request-stream`); returns the initiating
    /// response's status+headers TOGETHER with an opaque stream handle the guest drains via
    /// [`Self::http_poll_stream_chunk`] — the host owns the live Rust stream (a guest cannot hold one
    /// across the wasm boundary, arch-08 §5.2). Denied by default.
    fn http_request_stream(&self, _req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        Err("http-client capability not granted".into())
    }
    /// Drain the next chunk of a stream opened via [`Self::http_request_stream`] (the WIT
    /// `http-client.poll-stream-chunk`); `Ok(None)` = EOF. Denied by default.
    fn http_poll_stream_chunk(&self, _handle: u32) -> Result<Option<Vec<u8>>, String> {
        Err("http-client capability not granted".into())
    }
    /// Close (drop/cancel) a stream opened via [`Self::http_request_stream`] (the WIT
    /// `http-client.close-stream`). No-op by default.
    fn http_close_stream(&self, _handle: u32) {}

    // --- proc capability (arch-08 §5.2 request/poll bridge; pi-mcp-adapter-port.md §3.1); gated by
    // the SAME load-time trust check `exec`/`http-client`/`ui` use (no new bool, no per-host
    // allowlist) — denied by default (no ambient process authority, R-ARCH-EXT-011). A long-lived,
    // duplex-pipe child process (MCP stdio transport), distinct from the bounded `exec` grant. ---

    /// Spawn a long-lived child (the WIT `proc.spawn`); returns an opaque handle the guest polls
    /// via [`Self::proc_read_stdout`]/[`Self::proc_poll_exit`]. Denied by default.
    fn proc_spawn(&self, _spec: &ProcSpawnSpec) -> Result<u32, String> {
        Err("proc capability not granted".into())
    }
    /// Write to a spawned child's stdin (the WIT `proc.write-stdin`); returns bytes written.
    /// Denied by default.
    fn proc_write_stdin(&self, _handle: u32, _data: &[u8]) -> Result<u32, String> {
        Err("proc capability not granted".into())
    }
    /// Drain currently-buffered stdout (the WIT `proc.read-stdout`; non-blocking poll, empty = no
    /// data yet, NOT EOF). Denied by default.
    fn proc_read_stdout(&self, _handle: u32, _max_bytes: u32) -> Result<Vec<u8>, String> {
        Err("proc capability not granted".into())
    }
    /// Drain currently-buffered stderr (the WIT `proc.read-stderr`). Denied by default.
    fn proc_read_stderr(&self, _handle: u32, _max_bytes: u32) -> Result<Vec<u8>, String> {
        Err("proc capability not granted".into())
    }
    /// Poll whether a spawned child has exited (the WIT `proc.poll-exit`); `Some(code)` once
    /// terminated. No error channel in the WIT signature; an ungranted/unknown handle degrades to
    /// `None` ("not exited") rather than a hang, matching an always-running-forever illusion for a
    /// capability that was never granted a process to begin with.
    fn proc_poll_exit(&self, _handle: u32) -> Option<i32> {
        None
    }
    /// Terminate a spawned child (SIGTERM then SIGKILL after a grace period; the WIT `proc.kill`).
    /// Denied by default.
    fn proc_kill(&self, _handle: u32) -> Result<(), String> {
        Err("proc capability not granted".into())
    }

    // --- command-tier control (arch-08 §6.3); the deadlock guard is applied BEFORE this is called ---
    fn control(&self, _op: ControlOp) -> Result<(), String> {
        Err("control capability not available".into())
    }

    /// Persist a custom (non-LLM) entry (R-08-026); returns the new entry id.
    fn append_entry(&self, _custom_type: &str, _data: &Value) -> Result<String, String> {
        Err("append_entry not available".into())
    }

    /// Rename the running session (Pi `setSessionName`, agent-session.ts:2272-2274). No-op by
    /// default (the default host grants no session-mutation authority); the session service routes
    /// this to the live tree's `set_session_name`/`append_session_info`.
    fn set_session_name(&self, _name: &str) {}

    /// Set (or replace) an entry's label (Pi `setLabel`, agent-session.ts:2276-2279). No-op by
    /// default; the session service routes this to the live tree's `append_label`.
    fn set_label(&self, _entry_id: &str, _label: &str) {}
}

/// Recorded extended-UI chrome effects (Pi `ExtensionUIContext` mutators, types.ts:124-275). These
/// are observable host-side (tests/diagnostics) and would drive the TUI widget protocol (arch-11).
#[derive(Clone, Debug, Default)]
pub struct UiChrome {
    pub header: Option<String>,
    pub footer: Option<String>,
    pub title: Option<String>,
    /// `set-editor-text`/`paste-editor-text` writes (text, is_paste).
    pub editor_writes: Vec<(String, bool)>,
    /// `theme-set` requests.
    pub theme_sets: Vec<String>,
    /// `working-start`(Some(label)) / `working-stop`(None) toggles.
    pub working: Vec<Option<String>>,
    /// last `set-tools-expanded` value.
    pub tools_expanded: Option<bool>,
}

/// The default backend: grants nothing (no ambient authority, R-ARCH-EXT-011).
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyServices;
impl HostServices for DenyServices {}

/// Canned responses a [`RecordingServices`] returns for the interactive (host→user) capabilities.
#[derive(Clone, Debug)]
pub struct CannedResponses {
    pub confirm: bool,
    pub input: Option<String>,
    pub select: Option<String>,
    pub editor: Option<String>,
    pub custom: Option<String>,
    pub exec: ExecOutput,
    /// The canned answer to `http_request` (default: a bare `200` with an empty body).
    pub http_response: HttpResponse,
    /// The canned status a `http_request_stream` grant returns alongside its handle (default 200;
    /// `HttpStreamResponse.status`).
    pub http_stream_status: u16,
    /// The canned headers a `http_request_stream` grant returns alongside its handle (default empty;
    /// `HttpStreamResponse.headers`).
    pub http_stream_headers: Vec<(String, String)>,
    /// The canned chunks a `http_request_stream` grant yields in order, then EOF (`Ok(None)`).
    pub http_stream_chunks: Vec<Vec<u8>>,
    /// The canned chunks a `proc_spawn` grant yields across repeated `proc_read_stdout` calls (one
    /// chunk per call, then empty forever — an empty read is never a fabricated EOF; see
    /// [`Self::proc_exit_code`]).
    pub proc_stdout_chunks: Vec<Vec<u8>>,
    /// As [`Self::proc_stdout_chunks`], for `proc_read_stderr`.
    pub proc_stderr_chunks: Vec<Vec<u8>>,
    /// The canned answer `proc_poll_exit` returns for every spawned handle (`None` = still
    /// running, the default — a canned process never "exits" on its own).
    pub proc_exit_code: Option<i32>,
    pub current_model: Option<String>,
    pub models: Value,
    pub theme: Option<String>,
    pub themes: Value,
    pub editor_text: String,
    /// Answer returned from a guest `login` flow's `onPrompt` (Pi OAuth). `None` = cancelled.
    pub oauth_prompt: Option<String>,
    /// Id returned from a guest `login` flow's `onSelect`.
    pub oauth_select: Option<String>,
}

impl Default for CannedResponses {
    fn default() -> Self {
        Self {
            confirm: true,
            input: Some(String::new()),
            select: Some(String::new()),
            editor: Some(String::new()),
            custom: None,
            exec: ExecOutput::default(),
            http_response: HttpResponse { status: 200, headers: Vec::new(), body: Vec::new() },
            http_stream_status: 200,
            http_stream_headers: Vec::new(),
            http_stream_chunks: Vec::new(),
            proc_stdout_chunks: Vec::new(),
            proc_stderr_chunks: Vec::new(),
            proc_exit_code: None,
            current_model: None,
            models: json!([]),
            theme: None,
            themes: json!([]),
            editor_text: String::new(),
            oauth_prompt: Some(String::new()),
            oauth_select: None,
        }
    }
}

/// A concrete NON-deny backend (arch-08 §3.6): it GRANTS the interactive/exec/control/append
/// capabilities, returning canned responses and RECORDING the effects. This is the in-crate analog
/// of the live cyrup-session/cyrup-tui backend the session injects at runtime — unlike
/// [`DenyServices`], it proves every capability seam end-to-end with real (observable) effects.
#[derive(Default)]
pub struct RecordingServices {
    responses: CannedResponses,
    state: Mutex<RecordingState>,
}

#[derive(Default)]
struct RecordingState {
    control_ops: Vec<ControlOp>,
    exec_calls: Vec<(String, Vec<String>)>,
    /// Whether `cancel.is_cancelled()` was already true at the moment each `exec` call reached the
    /// `HostServices` boundary, in call order — proves a `signalId` that was aborted before the call
    /// actually produced a pre-cancelled token (Pi `options.signal.aborted`, `exec.ts:66-68`), not
    /// just that the id was recorded somewhere.
    exec_call_pre_cancelled: Vec<bool>,
    /// The `message` body of each `confirm` call (L4 review §2.6), in call order.
    confirm_messages: Vec<String>,
    /// The `placeholder` of each `input` call (L4 review §2.7), in call order.
    input_placeholders: Vec<Option<String>>,
    /// Requests recorded via `http_request`/`http_request_stream`.
    http_requests: Vec<HttpRequest>,
    /// Open streaming grants: handle -> cursor into `responses.http_stream_chunks`.
    http_streams: HashMap<u32, usize>,
    next_http_stream_handle: u32,
    /// `(cmd, args)` of each `proc_spawn` grant.
    proc_spawns: Vec<(String, Vec<String>)>,
    /// `(handle, data)` of each `proc_write_stdin` call.
    proc_writes: Vec<(u32, Vec<u8>)>,
    /// Handles `proc_kill` was called on.
    proc_kills: Vec<u32>,
    next_proc_handle: u32,
    /// Open spawn grants: handle -> cursor into `responses.proc_stdout_chunks`/`proc_stderr_chunks`.
    proc_stdout_cursors: HashMap<u32, usize>,
    proc_stderr_cursors: HashMap<u32, usize>,
    entries: Vec<(String, Value)>,
    next_entry: u64,
    /// The last session name set via `set_session_name` (Pi `setSessionName`).
    session_name: Option<String>,
    /// The `(entry_id, label)` pairs set via `set_label` (Pi `setLabel`).
    labels: Vec<(String, String)>,
}

impl RecordingServices {
    pub fn new(responses: CannedResponses) -> Self {
        Self { responses, state: Mutex::new(RecordingState::default()) }
    }

    /// The control ops requested via the `control` import (command tier).
    pub fn control_ops(&self) -> Vec<ControlOp> {
        self.state.lock().map(|g| g.control_ops.clone()).unwrap_or_default()
    }

    /// The `(cmd, args)` of each capability-scoped `exec.run`.
    pub fn exec_calls(&self) -> Vec<(String, Vec<String>)> {
        self.state.lock().map(|g| g.exec_calls.clone()).unwrap_or_default()
    }

    /// Whether `cancel.is_cancelled()` was already true when each `exec` call arrived, in call order.
    pub fn exec_call_pre_cancelled(&self) -> Vec<bool> {
        self.state.lock().map(|g| g.exec_call_pre_cancelled.clone()).unwrap_or_default()
    }

    /// The requests recorded via `http_request`/`http_request_stream`.
    pub fn http_requests(&self) -> Vec<HttpRequest> {
        self.state.lock().map(|g| g.http_requests.clone()).unwrap_or_default()
    }

    /// The `(cmd, args)` of each `proc_spawn` grant.
    pub fn proc_spawns(&self) -> Vec<(String, Vec<String>)> {
        self.state.lock().map(|g| g.proc_spawns.clone()).unwrap_or_default()
    }

    /// The `(handle, data)` of each `proc_write_stdin` call.
    pub fn proc_writes(&self) -> Vec<(u32, Vec<u8>)> {
        self.state.lock().map(|g| g.proc_writes.clone()).unwrap_or_default()
    }

    /// The handles `proc_kill` was called on.
    pub fn proc_kills(&self) -> Vec<u32> {
        self.state.lock().map(|g| g.proc_kills.clone()).unwrap_or_default()
    }

    /// The persisted custom entries (R-08-026).
    pub fn entries_persisted(&self) -> Vec<(String, Value)> {
        self.state.lock().map(|g| g.entries.clone()).unwrap_or_default()
    }

    /// The `(entry_id, label)` pairs set via `set_label` (Pi `setLabel`).
    pub fn labels_set(&self) -> Vec<(String, String)> {
        self.state.lock().map(|g| g.labels.clone()).unwrap_or_default()
    }

    /// The `message` body of each `confirm` call, in call order (L4 review §2.6 live proof: a guest
    /// `confirm_with(title, message, ..)` call's `message` reaches the host distinct from `title`).
    pub fn confirm_messages(&self) -> Vec<String> {
        self.state.lock().map(|g| g.confirm_messages.clone()).unwrap_or_default()
    }

    /// The `placeholder` of each `input` call, in call order (L4 review §2.7 live proof: a guest
    /// `input_with(title, placeholder, ..)` call's `placeholder` reaches the host).
    pub fn input_placeholders(&self) -> Vec<Option<String>> {
        self.state.lock().map(|g| g.input_placeholders.clone()).unwrap_or_default()
    }
}

impl HostServices for RecordingServices {
    fn confirm(&self, _prompt: &str, message: &str, _opts: &DialogOptions) -> bool {
        if let Ok(mut g) = self.state.lock() {
            g.confirm_messages.push(message.to_string());
        }
        self.responses.confirm
    }
    fn input(&self, _prompt: &str, placeholder: Option<&str>, _opts: &DialogOptions) -> Option<String> {
        if let Ok(mut g) = self.state.lock() {
            g.input_placeholders.push(placeholder.map(str::to_string));
        }
        self.responses.input.clone()
    }
    fn select(&self, _prompt: &str, _options: &Value, _opts: &DialogOptions) -> Option<String> {
        self.responses.select.clone()
    }
    fn oauth_prompt(
        &self,
        _message: &str,
        _placeholder: Option<&str>,
        _allow_empty: bool,
    ) -> Result<String, String> {
        self.responses.oauth_prompt.clone().ok_or_else(|| "oauth prompt cancelled".into())
    }
    fn oauth_select(&self, _message: &str, _options: &Value) -> Option<String> {
        self.responses.oauth_select.clone()
    }
    fn editor(&self, _title: &str, _initial: &str) -> Option<String> {
        self.responses.editor.clone()
    }
    fn custom(&self, _spec: &Value) -> Option<String> {
        self.responses.custom.clone()
    }
    fn editor_text(&self) -> String {
        self.responses.editor_text.clone()
    }
    fn theme(&self) -> Option<String> {
        self.responses.theme.clone()
    }
    fn theme_list(&self) -> Value {
        self.responses.themes.clone()
    }
    fn set_theme(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }
    fn models(&self) -> Value {
        self.responses.models.clone()
    }
    fn current_model(&self) -> Option<String> {
        self.responses.current_model.clone()
    }
    fn exec(
        &self,
        cmd: &str,
        args: &[String],
        _opts: &Value,
        cancel: CancelToken,
    ) -> Result<ExecOutput, String> {
        if let Ok(mut g) = self.state.lock() {
            g.exec_calls.push((cmd.to_string(), args.to_vec()));
            g.exec_call_pre_cancelled.push(cancel.is_cancelled());
        }
        Ok(self.responses.exec.clone())
    }
    fn http_request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        if let Ok(mut g) = self.state.lock() {
            g.http_requests.push(req.clone());
        }
        Ok(self.responses.http_response.clone())
    }
    fn http_request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        g.http_requests.push(req.clone());
        let handle = g.next_http_stream_handle;
        g.next_http_stream_handle += 1;
        g.http_streams.insert(handle, 0);
        Ok(HttpStreamResponse {
            handle,
            status: self.responses.http_stream_status,
            headers: self.responses.http_stream_headers.clone(),
        })
    }
    fn http_poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        let cursor = g
            .http_streams
            .get_mut(&handle)
            .ok_or_else(|| format!("no open http stream for handle {handle}"))?;
        match self.responses.http_stream_chunks.get(*cursor) {
            Some(chunk) => {
                let chunk = chunk.clone();
                *cursor += 1;
                Ok(Some(chunk))
            }
            None => Ok(None),
        }
    }
    fn http_close_stream(&self, handle: u32) {
        if let Ok(mut g) = self.state.lock() {
            g.http_streams.remove(&handle);
        }
    }
    fn proc_spawn(&self, spec: &ProcSpawnSpec) -> Result<u32, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        g.proc_spawns.push((spec.cmd.clone(), spec.args.clone()));
        let handle = g.next_proc_handle;
        g.next_proc_handle += 1;
        g.proc_stdout_cursors.insert(handle, 0);
        g.proc_stderr_cursors.insert(handle, 0);
        Ok(handle)
    }
    fn proc_write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        g.proc_writes.push((handle, data.to_vec()));
        Ok(u32::try_from(data.len()).unwrap_or(u32::MAX))
    }
    fn proc_read_stdout(&self, handle: u32, _max_bytes: u32) -> Result<Vec<u8>, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        let cursor = g
            .proc_stdout_cursors
            .get_mut(&handle)
            .ok_or_else(|| format!("no live process for handle {handle}"))?;
        Ok(match self.responses.proc_stdout_chunks.get(*cursor) {
            Some(chunk) => {
                let chunk = chunk.clone();
                *cursor += 1;
                chunk
            }
            None => Vec::new(),
        })
    }
    fn proc_read_stderr(&self, handle: u32, _max_bytes: u32) -> Result<Vec<u8>, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        let cursor = g
            .proc_stderr_cursors
            .get_mut(&handle)
            .ok_or_else(|| format!("no live process for handle {handle}"))?;
        Ok(match self.responses.proc_stderr_chunks.get(*cursor) {
            Some(chunk) => {
                let chunk = chunk.clone();
                *cursor += 1;
                chunk
            }
            None => Vec::new(),
        })
    }
    fn proc_poll_exit(&self, _handle: u32) -> Option<i32> {
        self.responses.proc_exit_code
    }
    fn proc_kill(&self, handle: u32) -> Result<(), String> {
        if let Ok(mut g) = self.state.lock() {
            g.proc_kills.push(handle);
        }
        Ok(())
    }
    fn control(&self, op: ControlOp) -> Result<(), String> {
        if let Ok(mut g) = self.state.lock() {
            g.control_ops.push(op);
        }
        Ok(())
    }
    fn append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        g.next_entry += 1;
        let id = format!("entry-{}", g.next_entry);
        g.entries.push((custom_type.to_string(), data.clone()));
        Ok(id)
    }
    fn session_name(&self) -> Option<String> {
        // Read back whatever `set_session_name` last recorded (proves the rename round-trips).
        self.state.lock().ok().and_then(|g| g.session_name.clone())
    }
    fn set_session_name(&self, name: &str) {
        if let Ok(mut g) = self.state.lock() {
            g.session_name = Some(name.to_string());
        }
    }
    fn set_label(&self, entry_id: &str, label: &str) {
        if let Ok(mut g) = self.state.lock() {
            g.labels.push((entry_id.to_string(), label.to_string()));
        }
    }
}

/// Capability-scoped filesystem roots for the `ext-fs` import (preopened dirs; no ambient fs).
#[derive(Clone, Debug, Default)]
pub struct FsCaps {
    /// A single granted root the guest may read/write under. `None` => all fs access denied.
    pub root: Option<PathBuf>,
}

impl FsCaps {
    /// Resolve `path` under the granted root, rejecting escapes (`..`). Returns the absolute path or
    /// an error string surfaced to the guest as a WIT `result` error (never a host panic).
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let root = self.root.as_ref().ok_or("filesystem capability not granted")?;
        let candidate = PathBuf::from(path);
        // Reject absolute paths and parent-dir escapes (capability scoping, R-ARCH-EXT-011).
        if candidate.is_absolute() || candidate.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(format!("path `{path}` escapes the granted capability root"));
        }
        Ok(root.join(candidate))
    }
}

/// Host-side state backing one loaded WASM extension's imports (arch-08 §3.5/§3.6). Shared (via
/// `Arc`) between the extension's `Store<HostState>` (so the import Host impls reach it) and the
/// [`crate::host::WasmExtension`] handle (so the loader reads back what `init` registered).
pub struct GuestState {
    pub owner: ExtensionId,
    pub registry: Arc<ExtensionRegistry>,
    pub services: Arc<dyn HostServices>,
    pub fs: FsCaps,
    /// The current dispatch tier; control ops are legal only at [`CtxTier::Command`] (R-08-008).
    tier: Mutex<CtxTier>,
    /// Subscriptions declared via the `subscribe` import (read back after `init`).
    subs: Mutex<Subscriptions>,
    /// Commands registered via `register-command` (drained into the registry after `init`).
    commands: Mutex<Vec<(String, CommandDescriptor)>>,
    /// Flags registered via `register-flag` (name -> spec JSON).
    flags: Mutex<HashMap<String, Value>>,
    /// Autocomplete providers added via `add-autocomplete` (command names).
    autocomplete: Mutex<Vec<String>>,
    /// Message renderers registered via `register-message-renderer` (custom types).
    renderers: Mutex<Vec<String>>,
    /// `ui.notify` log — observable host effect (used by tests + diagnostics). Each entry carries
    /// the Pi `type` severity (`info`|`warning`|`error`, types.ts:135).
    notifications: Mutex<Vec<(String, NotifyKind)>>,
    /// `ui.set-status` log: keyed status segments (Pi `setStatus(key, text?)`, types.ts:141). A
    /// `None` text clears that key (Pi `setStatus(key, undefined)`).
    statuses: Mutex<Vec<(String, Option<String>)>>,
    /// `ui.set-widget` payloads.
    widgets: Mutex<Vec<Value>>,
    /// Extended UI chrome effects (header/footer/title/editor/theme/working/tools-expanded).
    chrome: Mutex<UiChrome>,
    /// `bus.emit` topics + payloads (R-08-029).
    bus_emits: Mutex<Vec<(String, Value)>>,
    /// `host-tool.emit-update` chunks emitted during a guest tool's `execute` (call_id, chunk).
    /// Drained to the runtime `ToolUpdateSink` after the execute call settles (Pi `onUpdate`).
    tool_updates: Mutex<Vec<(String, Value)>>,
    /// Count of stacked global autocomplete providers (Pi addAutocompleteProvider, host gap #3).
    autocomplete_providers: Mutex<u32>,
    /// OAuth login-flow callbacks the guest invoked during `provider-login` (observable host-side).
    oauth_events: Mutex<Vec<OAuthEvent>>,
    /// `provider-stream.emit-event` events pushed during a guest `streamSimple` (stream_id, event).
    stream_events: Mutex<Vec<(String, Value)>>,
    /// The active-tool restriction set via `ext-tools.set-active-tools` (Pi `setActiveTools`).
    active_tools_restriction: Mutex<Option<Vec<String>>>,
    /// Named abort signals dismissed via `ui.abort-signal` (Pi `ExtensionUIDialogOptions.signal`,
    /// sdk gap #2): a dialog opened carrying an aborted signal id returns cancelled; a tool whose
    /// `call-id` matches polls `is-cancelled` true (Pi `ToolDefinition.execute` `signal`, sdk gap #1).
    aborted_signals: Mutex<HashSet<String>>,
    /// The `CancelToken` of the currently-executing guest tool, backing the `host-tool.is-cancelled`
    /// poll (Pi `signal` param). Set before `execute-tool`, cleared after (sdk gap #1).
    tool_cancel: Mutex<Option<CancelToken>>,
    /// `withSession` callback ids the guest scheduled via a `control.*` op carrying a
    /// `withSessionCallbackId`; the host invokes the `with-session` export for each after the command
    /// body returns (Pi `finishSessionReplacement`, agent-session-runtime.ts:184; sdk gap #3).
    pending_with_session: Mutex<Vec<String>>,
    /// Wall-clock ESTIMATE of when the currently-armed `wasmtime::Store::set_epoch_deadline` will be
    /// reached — our own mirror of wasmtime's epoch bookkeeping, since `Store`/`Engine` expose NO
    /// public getter for either the live epoch counter or the armed deadline (verified against
    /// wasmtime 46.0.1's `store.rs`/`engine.rs`: `current_epoch`/`get_epoch_deadline` are both
    /// `pub(crate)`). Computed as `Instant::now() + ticks * epoch::DEFAULT_TICK` and (re)armed by
    /// [`Self::arm_epoch_deadline_estimate`] every time the host calls `set_epoch_deadline` — see
    /// [`Self::take_dialog_extra_ticks`]'s doc for why this is needed to compute forgiveness
    /// correctly instead of doubling it.
    deadline_estimate: Mutex<Option<std::time::Instant>>,
    /// Wall-clock instant the FIRST `ui.{confirm,select,input,editor}` call in the current
    /// forgiveness batch began blocking (`None` between batches) — anchors the "how much budget was
    /// still unused when the guest chose to block" computation to when it ACTUALLY stopped consuming
    /// budget, not to whichever (possibly later, if several dialogs chain back-to-back with no
    /// intervening checkpoint) dialog happened to resolve last. Set by [`Self::note_dialog_wait`],
    /// consumed + reset by [`Self::take_dialog_extra_ticks`] — and ALSO reset by every fresh dispatch
    /// boundary via [`Self::arm_epoch_deadline_estimate`], so a dialog that resolved quickly (never
    /// tripping `epoch_deadline_callback`, so `take_dialog_extra_ticks` never ran) can't leave a stale
    /// anchor that a LATER, unrelated dispatch's genuine runaway would exploit for outsized forgiveness.
    first_wait_started: Mutex<Option<std::time::Instant>>,
    /// Wall-clock instant [`Self::note_dialog_wait`] most recently confirmed a dialog call had JUST
    /// returned (`None` between batches). This closes the SAME-dispatch sibling of the stale-anchor
    /// bug `first_wait_started`'s doc describes for the CROSS-dispatch case: a dialog that resolves
    /// comfortably inside its OWN budget leaves `first_wait_started` set (no dispatch boundary and no
    /// `epoch_deadline_callback` have run yet to clear it) — if the guest then burns cpu on wholly
    /// UNRELATED work for the rest of the SAME dispatch with no further host call at all, the eventual
    /// trap would read that ancient timestamp as "still owed" and grant forgiveness computed from a
    /// wait that already finished, potentially re-granting close to a full fresh budget for zero
    /// actual waiting. A genuinely live back-to-back dialog batch (several `ui.*` calls chained with
    /// no intervening checkpoint — [`Self::first_wait_started`]'s doc) keeps this touched on every
    /// call, so the gap [`Self::take_dialog_extra_ticks`] observes between the last touch and "now"
    /// stays near-zero (just scheduler/checkpoint latency); only an anchor left stale by real,
    /// untracked cpu execution opens up a gap wider than [`STALE_WAIT_TOUCH_GAP`].
    last_wait_touch: Mutex<Option<std::time::Instant>>,
}

/// Notification severity (Pi `notify` `type`: `"info" | "warning" | "error"`, types.ts:135).
/// `info` is Pi's default when the guest omits the argument.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotifyKind {
    #[default]
    Info,
    Warning,
    Error,
}

/// An OAuth login-flow callback the guest invoked during `provider-login` (Pi `OAuthLoginCallbacks`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthEvent {
    Auth { url: String, instructions: Option<String> },
    DeviceCode { user_code: String, verification_uri: String },
    Prompt { message: String },
    Progress { message: String },
    Select { message: String },
}

impl GuestState {
    pub fn new(owner: ExtensionId, registry: Arc<ExtensionRegistry>) -> Self {
        Self::with_services(owner, registry, Arc::new(DenyServices))
    }

    pub fn with_services(
        owner: ExtensionId,
        registry: Arc<ExtensionRegistry>,
        services: Arc<dyn HostServices>,
    ) -> Self {
        Self {
            owner,
            registry,
            services,
            fs: FsCaps::default(),
            tier: Mutex::new(CtxTier::Command), // init runs at command tier (load time)
            subs: Mutex::new(Subscriptions::empty()),
            commands: Mutex::new(Vec::new()),
            flags: Mutex::new(HashMap::new()),
            autocomplete: Mutex::new(Vec::new()),
            renderers: Mutex::new(Vec::new()),
            notifications: Mutex::new(Vec::new()),
            statuses: Mutex::new(Vec::new()),
            widgets: Mutex::new(Vec::new()),
            chrome: Mutex::new(UiChrome::default()),
            bus_emits: Mutex::new(Vec::new()),
            tool_updates: Mutex::new(Vec::new()),
            autocomplete_providers: Mutex::new(0),
            oauth_events: Mutex::new(Vec::new()),
            stream_events: Mutex::new(Vec::new()),
            active_tools_restriction: Mutex::new(None),
            aborted_signals: Mutex::new(HashSet::new()),
            tool_cancel: Mutex::new(None),
            pending_with_session: Mutex::new(Vec::new()),
            deadline_estimate: Mutex::new(None),
            first_wait_started: Mutex::new(None),
            last_wait_touch: Mutex::new(None),
        }
    }

    pub fn with_fs(mut self, root: PathBuf) -> Self {
        self.fs = FsCaps { root: Some(root) };
        self
    }

    /// Set the dispatch tier (the loader sets `Event` before dispatching an event handler, keeps
    /// `Command` for init/command handlers). A poisoned lock degrades to a no-op (never a panic).
    pub fn set_tier(&self, tier: CtxTier) {
        if let Ok(mut g) = self.tier.lock() {
            *g = tier;
        }
    }

    pub fn tier(&self) -> CtxTier {
        self.tier.lock().map(|g| *g).unwrap_or(CtxTier::Event)
    }

    /// Deadlock guard (R-08-008): control ops require the command tier.
    pub fn require_command_tier(&self) -> Result<(), String> {
        if self.tier() == CtxTier::Command {
            Ok(())
        } else {
            Err("deadlock guard: session-mutating control op from an event handler".into())
        }
    }

    pub fn add_subscription(&self, kind: EventKind) {
        if let Ok(mut g) = self.subs.lock() {
            g.add(kind);
        }
    }

    pub fn subscriptions(&self) -> Subscriptions {
        self.subs.lock().map(|g| *g).unwrap_or_else(|_| Subscriptions::empty())
    }

    pub fn push_command(&self, name: String, desc: CommandDescriptor) {
        if let Ok(mut g) = self.commands.lock() {
            g.push((name, desc));
        }
    }

    /// Drain the commands registered during `init` (the loader writes them into the registry).
    pub fn take_commands(&self) -> Vec<(String, CommandDescriptor)> {
        self.commands.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }

    pub fn set_flag(&self, name: String, spec: Value) {
        if let Ok(mut g) = self.flags.lock() {
            g.insert(name, spec);
        }
    }

    /// Resolve a flag's VALUE (Pi `getFlag`, loader.ts:256-262): returns the registered flag's
    /// resolved value (its `default`, the analog of Pi `runtime.flagValues.get(name)` seeded from
    /// `options.default`), serialized as JSON — NOT the whole `{type,default,description}` spec.
    /// `None` when the flag is unregistered OR was registered without a default (Pi: `flagValues`
    /// only gets an entry when `options.default !== undefined`, loader.ts:259).
    pub fn get_flag(&self, name: &str) -> Option<String> {
        let g = self.flags.lock().ok()?;
        let spec = g.get(name)?;
        // A bare (non-object) stored value is itself the resolved value (defensive).
        let value = match spec.as_object() {
            Some(_) => spec.get("default")?,
            None => spec,
        };
        if value.is_null() {
            return None;
        }
        Some(value.to_string())
    }

    pub fn add_autocomplete(&self, command: String) {
        if let Ok(mut g) = self.autocomplete.lock() {
            g.push(command);
        }
    }

    pub fn autocomplete_commands(&self) -> Vec<String> {
        self.autocomplete.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn add_renderer(&self, custom_type: String) {
        if let Ok(mut g) = self.renderers.lock() {
            g.push(custom_type);
        }
    }

    pub fn renderers(&self) -> Vec<String> {
        self.renderers.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn notify(&self, message: String, kind: NotifyKind) {
        if let Ok(mut g) = self.notifications.lock() {
            g.push((message, kind));
        }
    }

    /// Recorded notification messages (severity-agnostic; back-compat for message-text assertions).
    pub fn notifications(&self) -> Vec<String> {
        self.notifications
            .lock()
            .map(|g| g.iter().map(|(m, _)| m.clone()).collect())
            .unwrap_or_default()
    }

    /// Recorded notifications with their Pi `type` severity (types.ts:135).
    pub fn notifications_with_kind(&self) -> Vec<(String, NotifyKind)> {
        self.notifications.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Record a keyed status update (Pi `setStatus(key, text?)`, types.ts:141). A `None` `text`
    /// clears that key.
    pub fn set_status(&self, key: String, text: Option<String>) {
        if let Ok(mut g) = self.statuses.lock() {
            g.push((key, text));
        }
    }

    /// Recorded status segments as `(key, text)` pairs (`text` `None` = a clear of that key).
    pub fn statuses(&self) -> Vec<(String, Option<String>)> {
        self.statuses.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// The current resolved value of keyed status segment `key` after replaying the recorded
    /// `setStatus` calls (Pi keeps a per-key map; a `None` text clears the key). `None` means the
    /// key is unset/cleared.
    pub fn status_for(&self, key: &str) -> Option<String> {
        self.statuses
            .lock()
            .ok()
            .and_then(|g| g.iter().rfind(|(k, _)| k == key).and_then(|(_, t)| t.clone()))
    }

    pub fn set_widget(&self, widget: Value) {
        if let Ok(mut g) = self.widgets.lock() {
            g.push(widget);
        }
    }

    // --- extended UI chrome (Pi ExtensionUIContext mutators) ---

    fn with_chrome<R>(&self, f: impl FnOnce(&mut UiChrome) -> R) -> Option<R> {
        self.chrome.lock().ok().map(|mut g| f(&mut g))
    }

    /// Snapshot of the recorded chrome effects (tests/diagnostics).
    pub fn chrome(&self) -> UiChrome {
        self.chrome.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_header(&self, content: String) {
        self.with_chrome(|c| c.header = Some(content));
    }
    pub fn set_footer(&self, content: String) {
        self.with_chrome(|c| c.footer = Some(content));
    }
    pub fn set_title(&self, title: String) {
        self.with_chrome(|c| c.title = Some(title));
    }
    pub fn editor_write(&self, text: String, is_paste: bool) {
        self.with_chrome(|c| c.editor_writes.push((text, is_paste)));
    }
    pub fn theme_set(&self, name: String) {
        self.with_chrome(|c| c.theme_sets.push(name));
    }
    pub fn working(&self, label: Option<String>) {
        self.with_chrome(|c| c.working.push(label));
    }
    pub fn set_tools_expanded(&self, expanded: bool) {
        self.with_chrome(|c| c.tools_expanded = Some(expanded));
    }

    pub fn bus_emit(&self, topic: String, payload: Value) {
        if let Ok(mut g) = self.bus_emits.lock() {
            g.push((topic, payload));
        }
    }

    pub fn bus_emits(&self) -> Vec<(String, Value)> {
        self.bus_emits.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn push_tool_update(&self, call_id: String, chunk: Value) {
        if let Ok(mut g) = self.tool_updates.lock() {
            g.push((call_id, chunk));
        }
    }

    /// Drain the streamed updates for a settled tool execution.
    pub fn take_tool_updates(&self) -> Vec<(String, Value)> {
        self.tool_updates.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }

    // --- global autocomplete provider stacking (Pi addAutocompleteProvider, host gap #3) ---

    pub fn add_autocomplete_provider(&self) {
        if let Ok(mut g) = self.autocomplete_providers.lock() {
            *g += 1;
        }
    }

    /// How many global autocomplete providers the guest stacked (drives the host fold).
    pub fn autocomplete_provider_count(&self) -> u32 {
        self.autocomplete_providers.lock().map(|g| *g).unwrap_or(0)
    }

    // --- OAuth login-flow callbacks (Pi OAuthLoginCallbacks, host gap #1) ---

    pub fn record_oauth_event(&self, ev: OAuthEvent) {
        if let Ok(mut g) = self.oauth_events.lock() {
            g.push(ev);
        }
    }

    /// The OAuth login callbacks the guest invoked during `provider-login` (tests/diagnostics).
    pub fn oauth_events(&self) -> Vec<OAuthEvent> {
        self.oauth_events.lock().map(|g| g.clone()).unwrap_or_default()
    }

    // --- provider streamSimple events (Pi createAssistantMessageEventStream, host gap #1) ---

    pub fn push_stream_event(&self, stream_id: String, event: Value) {
        if let Ok(mut g) = self.stream_events.lock() {
            g.push((stream_id, event));
        }
    }

    /// The assistant-message stream events a guest `streamSimple` pushed (stream_id, event).
    pub fn stream_events(&self) -> Vec<(String, Value)> {
        self.stream_events.lock().map(|g| g.clone()).unwrap_or_default()
    }

    // --- active-tool restriction (Pi setActiveTools, host gap-08-sdk #7) ---

    pub fn set_active_tools_restriction(&self, names: Vec<String>) {
        if let Ok(mut g) = self.active_tools_restriction.lock() {
            *g = Some(names);
        }
    }

    /// The active-tool restriction the guest set, if any (the merge is applied host-side).
    pub fn active_tools_restriction(&self) -> Option<Vec<String>> {
        self.active_tools_restriction.lock().ok().and_then(|g| g.clone())
    }

    // --- named abort signals (Pi ExtensionUIDialogOptions.signal / execute signal; sdk gap #1/#2) ---

    /// Mark a named signal aborted (Pi `signal.abort()`): the guest called `ui.abort-signal`.
    /// Bounded by [`MAX_ABORTED_SIGNALS`] (its doc explains why no Pi-derived exact value exists):
    /// once at the cap, a NEW distinct id is silently dropped rather than tracked — an id already
    /// present is unaffected (this bounds DISTINCT ids, not re-inserts of the same one).
    pub fn abort_signal(&self, id: String) {
        if let Ok(mut g) = self.aborted_signals.lock()
            && (g.contains(&id) || g.len() < MAX_ABORTED_SIGNALS)
        {
            g.insert(id);
        }
    }

    /// Whether a named signal id has been aborted (drives dialog dismissal + tool cancellation).
    pub fn is_signal_aborted(&self, id: &str) -> bool {
        self.aborted_signals.lock().map(|g| g.contains(id)).unwrap_or(false)
    }

    /// The set of aborted signal ids (tests/diagnostics).
    pub fn aborted_signals(&self) -> Vec<String> {
        self.aborted_signals.lock().map(|g| g.iter().cloned().collect()).unwrap_or_default()
    }

    /// Whether a dialog opened with `opts` is already dismissed by a programmatic signal (sdk gap #2).
    pub fn dialog_dismissed(&self, opts: &DialogOptions) -> bool {
        opts.signal_id.as_deref().is_some_and(|id| self.is_signal_aborted(id))
    }

    /// (Re)arm [`Self::deadline_estimate`] to `Instant::now() + ticks * epoch::DEFAULT_TICK` — call
    /// this EVERY time the host calls `store.set_epoch_deadline(ticks)` (every dispatch entry point
    /// in `live.rs`, plus every forgiveness grant via [`Self::take_dialog_extra_ticks`]) so the
    /// estimate never drifts out of sync with wasmtime's real (unreadable) internal deadline.
    ///
    /// ALSO clears any stale [`Self::first_wait_started`] anchor. Every call site except the
    /// self-re-arm inside [`Self::take_dialog_extra_ticks`] marks a brand-new dispatch boundary (it
    /// is always paired 1:1 with a real `store.set_epoch_deadline` in `live.rs`) — a dialog wait that
    /// resolved comfortably inside its OWN dispatch's budget never trips `epoch_deadline_callback`,
    /// so `take_dialog_extra_ticks` (the only other thing that clears the anchor) never runs, and
    /// without this the anchor would sit there indefinitely. A LATER, wholly unrelated dispatch that
    /// then genuinely runs long would read that ancient timestamp as "still owed" and be granted
    /// forgiveness computed from a deadline that has nothing to do with it — potentially far exceeding
    /// its own per-dispatch budget and defeating the epoch trap entirely. Clearing here bounds the
    /// anchor to at most the CURRENT dispatch (the self-re-arm inside `take_dialog_extra_ticks` is a
    /// no-op against this field: it always runs immediately after that function's own `.take()`, so
    /// the field is already `None`).
    pub fn arm_epoch_deadline_estimate(&self, ticks: u64) {
        let ticks_u32 = u32::try_from(ticks).unwrap_or(u32::MAX);
        let budget = crate::host::epoch::DEFAULT_TICK.checked_mul(ticks_u32).unwrap_or(std::time::Duration::MAX);
        let deadline = std::time::Instant::now().checked_add(budget);
        if let Ok(mut g) = self.deadline_estimate.lock() {
            *g = deadline;
        }
        if let Ok(mut g) = self.first_wait_started.lock() {
            *g = None;
        }
        if let Ok(mut g) = self.last_wait_touch.lock() {
            *g = None;
        }
    }

    /// Record that a `ui.{confirm,select,input,editor}` call is about to block on a human answer,
    /// starting at `started` (real wall-clock `Instant`). Only the FIRST call of a back-to-back batch
    /// (no intervening successful checkpoint) is kept — see [`Self::first_wait_started`]'s doc. ALSO
    /// touches [`Self::last_wait_touch`] on EVERY call (not just the first) — [`Self::take_dialog_extra_ticks`]
    /// uses that to tell a still-live batch apart from a stale anchor left by an already-finished wait
    /// (see [`Self::last_wait_touch`]'s doc for the same-dispatch bug class this closes).
    pub fn note_dialog_wait(&self, started: std::time::Instant) {
        if let Ok(mut g) = self.first_wait_started.lock()
            && g.is_none()
        {
            *g = Some(started);
        }
        if let Ok(mut g) = self.last_wait_touch.lock() {
            *g = Some(std::time::Instant::now());
        }
    }

    /// Atomically take (reset) the wait batch recorded by [`Self::note_dialog_wait`] and compute how
    /// many epoch ticks to forgive — the `epoch_deadline_callback` (`live.rs`) calls this exactly
    /// once per deadline-reached event.
    ///
    /// Returns the REMAINING (unused) budget the store still had, in ticks, at the moment the guest
    /// entered its dialog wait — NOT the wait duration itself. This is the fix for the CRITICAL
    /// double-forgiveness bug: `wasmtime::UpdateDeadline::Continue(delta)` extends the deadline from
    /// the CURRENT epoch (`Store::set_epoch_deadline`, wasmtime 46.0.1 `store.rs:2366-2375`:
    /// `epoch_deadline = current_epoch + delta`), and by the time this callback fires, `current_epoch`
    /// has ALREADY advanced by (approximately) the full wait duration — so passing the wait duration
    /// itself as `delta` double-counts it (`current_epoch(≈old_deadline+wait) + wait ≈
    /// old_deadline + 2*wait`, roughly DOUBLE the intended forgiveness). Passing the pre-wait
    /// REMAINING budget instead gives `current_epoch(≈old_deadline+wait) + remaining ≈
    /// old_deadline + wait` — extended by EXACTLY the wait, as originally intended — and is
    /// inherently bounded by the per-dispatch tick budget (remaining can never exceed what was
    /// originally granted), so it cannot be exploited by chaining many quick dialogs to accumulate
    /// unbounded compute time the way a flat "always grant a fresh full budget" policy could.
    ///
    /// A recorded wait that is still LIVE (see [`Self::last_wait_touch`]) NEVER produces zero
    /// forgiveness (floor of 1 tick): the whole point of this mechanism (`845f707`) is that a genuine
    /// dialog wait must never trap — a trap here permanently wedges the instance (component-model
    /// reentrance bookkeeping never sees a clean completion). A recorded wait that has gone STALE
    /// (already finished, with real untracked guest execution since) is treated as no wait at all —
    /// zero forgiveness, a real trap — which is exactly correct: nothing is actually being waited on.
    pub fn take_dialog_extra_ticks(&self) -> u64 {
        let Some(first_started) = self.first_wait_started.lock().ok().and_then(|mut g| g.take()) else {
            return 0;
        };
        // Same-dispatch stale-anchor guard (see [`Self::last_wait_touch`]'s doc): `first_started` only
        // proves SOME wait began at that instant, not that it is the wait actually causing THIS trap —
        // a wait that resolved well inside its own budget leaves `first_started` sitting there, and
        // without this check a later, wholly UNRELATED cpu-bound stretch of the same dispatch (no
        // further host call at all) would be handed forgiveness computed from that ancient, irrelevant
        // timestamp. A genuinely live wait (this one, or the tail of a back-to-back batch) always has
        // `note_dialog_wait` touch `last_wait_touch` within microseconds of this callback running, so
        // require that gap to be small; anything wider means real, untracked guest execution happened
        // in between, and this is treated exactly like "no recorded wait" (0 forgiveness ⇒ a real
        // trap, matching a genuine runaway/looping guest).
        let touch_is_fresh = self
            .last_wait_touch
            .lock()
            .ok()
            .and_then(|mut g| g.take())
            .is_some_and(|touched| std::time::Instant::now().saturating_duration_since(touched) <= STALE_WAIT_TOUCH_GAP);
        if !touch_is_fresh {
            return 0;
        }
        let deadline = self.deadline_estimate.lock().ok().and_then(|g| *g);
        let remaining_ticks = deadline
            .map(|d| {
                let remaining = d.saturating_duration_since(first_started);
                let tick_ms = crate::host::epoch::DEFAULT_TICK.as_millis().max(1);
                u64::try_from(remaining.as_millis().div_ceil(tick_ms)).unwrap_or(u64::MAX)
            })
            .unwrap_or(0)
            .max(1);
        // Re-arm for the NEXT forgiveness round (chained dialogs within the same dispatch): the fresh
        // deadline this grant produces is (approximately) `Instant::now() + remaining_ticks`.
        self.arm_epoch_deadline_estimate(remaining_ticks);
        remaining_ticks
    }

    /// Bind the currently-executing tool's `CancelToken` for the `is-cancelled` poll (sdk gap #1).
    pub fn set_tool_cancel(&self, token: Option<CancelToken>) {
        if let Ok(mut g) = self.tool_cancel.lock() {
            *g = token;
        }
    }

    /// The tool `signal` poll (Pi `signal.aborted`): true if the active tool's `CancelToken` is
    /// cancelled OR a named signal matching this `call_id` was aborted (sdk gap #1).
    pub fn tool_is_cancelled(&self, call_id: &str) -> bool {
        let token_cancelled =
            self.tool_cancel.lock().map(|g| g.as_ref().is_some_and(|t| t.is_cancelled())).unwrap_or(false);
        token_cancelled || self.is_signal_aborted(call_id)
    }

    // --- withSession re-binding callbacks (Pi finishSessionReplacement; sdk gap #3) ---

    /// Schedule a guest `with-session` callback id to run after the current command body returns.
    pub fn push_pending_with_session(&self, id: String) {
        if let Ok(mut g) = self.pending_with_session.lock() {
            g.push(id);
        }
    }

    /// Drain the scheduled `with-session` callback ids (the loader invokes the export for each).
    pub fn take_pending_with_session(&self) -> Vec<String> {
        self.pending_with_session.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn state() -> GuestState {
        GuestState::new(ExtensionId::from("test"), Arc::new(ExtensionRegistry::new()))
    }

    #[test]
    fn notify_records_pi_severity() {
        let s = state();
        s.notify("plain".into(), NotifyKind::Info);
        s.notify("careful".into(), NotifyKind::Warning);
        s.notify("boom".into(), NotifyKind::Error);

        // Message-text view is back-compat (severity-agnostic).
        assert_eq!(s.notifications(), vec!["plain", "careful", "boom"]);
        // Severity is preserved 1:1 with Pi's notify `type` (types.ts:135).
        assert_eq!(
            s.notifications_with_kind(),
            vec![
                ("plain".into(), NotifyKind::Info),
                ("careful".into(), NotifyKind::Warning),
                ("boom".into(), NotifyKind::Error),
            ]
        );
    }

    #[test]
    fn set_status_addresses_and_clears_keyed_segments() {
        let s = state();
        // Two independent keyed segments (Pi `setStatus(key, text)`, types.ts:141).
        s.set_status("lint".into(), Some("12 warnings".into()));
        s.set_status("build".into(), Some("compiling".into()));
        // Update one key, then clear it (Pi `setStatus(key, undefined)`).
        s.set_status("build".into(), Some("done".into()));
        s.set_status("lint".into(), None);

        // The raw log keeps every keyed write (incl. the clear).
        let log = s.statuses();
        assert_eq!(log.len(), 4);
        assert!(log.contains(&("lint".into(), None)));

        // Replay resolves each key to its last value; a None clears it.
        assert_eq!(s.status_for("build"), Some("done".into()));
        assert_eq!(s.status_for("lint"), None); // cleared
        assert_eq!(s.status_for("absent"), None); // never set
    }

    /// THE CRITICAL fix this closes: `take_dialog_extra_ticks` must forgive the guest's REMAINING
    /// (unused) budget at the moment it entered the dialog wait — NEVER the wait duration itself,
    /// and NEVER more than the original per-dispatch budget. The pre-fix bug returned the wait
    /// duration as `owed`, which `wasmtime::UpdateDeadline::Continue(owed)` then added to the
    /// CURRENT epoch (already advanced by the full wait) — roughly DOUBLING (or, for a wait much
    /// longer than the budget, far exceeding double) the intended forgiveness. Verified with REAL
    /// `std::time::Instant`/wall-clock timing (not a mocked clock — `arm_epoch_deadline_estimate`
    /// is a wall-clock-only mechanism specifically because wasmtime exposes no public epoch/deadline
    /// getter, confirmed against wasmtime 46.0.1's source).
    #[test]
    fn take_dialog_extra_ticks_forgives_remaining_budget_not_the_disproportionate_wait_duration() {
        let s = state();
        // A 20-tick (100ms) per-dispatch budget, mirroring `LiveExtension::load`'s
        // `guest.arm_epoch_deadline_estimate(epoch_ticks)` call.
        s.arm_epoch_deadline_estimate(20);

        // The guest burns roughly 40ms of its 100ms budget on real work before blocking on a dialog
        // (leaving ~60ms / ~12 ticks of budget unused at wait-start).
        std::thread::sleep(std::time::Duration::from_millis(40));
        let wait_started = std::time::Instant::now();
        // The "human" takes 500ms to answer — 5x the ENTIRE original budget, and the exact class of
        // scenario (`845f707`'s own live repro used 6s against a ~5s budget) that trips the epoch
        // deadline and invokes this forgiveness path. `note_dialog_wait` is called AFTER the block
        // resolves, mirroring `live.rs`'s `let started = Instant::now(); let result = guest.services
        // .confirm(...); guest.note_dialog_wait(started);` — the callback then runs immediately after,
        // exactly like the real `epoch_deadline_callback` firing the instant wasm resumes.
        std::thread::sleep(std::time::Duration::from_millis(500));
        s.note_dialog_wait(wait_started);

        let forgiven = s.take_dialog_extra_ticks();
        // The bug: `owed` (wait duration in ticks) would be ~100 ticks (500ms / 5ms) here — the
        // fixed value must be decisively smaller, bounded by the ORIGINAL 20-tick budget.
        assert!(
            forgiven <= 20,
            "forgiveness must never exceed the ORIGINAL per-dispatch budget (20 ticks): got \
             {forgiven} — granting more re-introduces the double-forgiveness bug (old code would \
             have granted ~100, the wait duration in ticks)"
        );
        // Never zero (a recorded wait must never trap — that permanently wedges the instance).
        assert!(forgiven >= 1, "a recorded dialog wait must never produce zero forgiveness");
        // Roughly matches the ~12 remaining ticks (60ms / 5ms), not the ~100-tick wait duration —
        // generous bounds to absorb real scheduler jitter from the `thread::sleep` calls above.
        assert!(
            forgiven <= 16,
            "forgiveness ({forgiven} ticks) should track the ~12 ticks of budget actually left \
             unused at wait-start, not the unrelated 500ms wait duration"
        );
    }

    /// A guest that enters a dialog wait with ALREADY-exhausted budget (e.g. it spun right up to its
    /// deadline before calling the dialog) still gets the floor of 1 forgiven tick — enough to reach
    /// its post-dialog return path without trapping, never zero.
    #[test]
    fn take_dialog_extra_ticks_floors_at_one_tick_even_with_no_remaining_budget() {
        let s = state();
        s.arm_epoch_deadline_estimate(0); // deadline is effectively "now" already
        let wait_started = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        s.note_dialog_wait(wait_started); // called right after the (simulated) block, like live.rs
        assert_eq!(s.take_dialog_extra_ticks(), 1, "a recorded wait floors at 1 tick, never 0");
    }

    /// No recorded dialog wait ⇒ zero forgiveness (a genuine runaway/looping guest, which never
    /// calls `note_dialog_wait`, must still trap at its original budget — unchanged by this fix).
    #[test]
    fn take_dialog_extra_ticks_is_zero_with_no_recorded_wait() {
        let s = state();
        s.arm_epoch_deadline_estimate(20);
        assert_eq!(s.take_dialog_extra_ticks(), 0);
    }

    /// Back-to-back dialogs with no intervening checkpoint (`confirm()` immediately followed by
    /// `input()`) are treated as ONE batch, anchored to the FIRST wait's start — not the second's
    /// (later) start, which would forgive LESS than the guest is actually owed.
    #[test]
    fn take_dialog_extra_ticks_anchors_to_the_first_wait_in_a_batch() {
        let s = state();
        s.arm_epoch_deadline_estimate(20);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let first = std::time::Instant::now();
        // First dialog in the batch blocks ~15ms, then `note_dialog_wait` runs right after it
        // resolves — mirroring live.rs's `let started = Instant::now(); <blocking call>;
        // guest.note_dialog_wait(started);` pattern for each of `ui.confirm`/`input`/`select`/`editor`.
        std::thread::sleep(std::time::Duration::from_millis(15));
        s.note_dialog_wait(first); // first dialog in the batch
        // Second dialog immediately follows with NO intervening checkpoint (no cpu burn between the
        // two calls) and itself blocks ~15ms — should NOT move the anchor off `first`.
        let second = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(15));
        s.note_dialog_wait(second); // second dialog — should NOT move the anchor

        let forgiven = s.take_dialog_extra_ticks();
        // Remaining budget computed from `first` (~90ms / 18 ticks) must be used, not from the
        // second call's later start (~70ms / 14 ticks) — i.e. `forgiven` skews toward the larger,
        // first-anchored value.
        assert!(forgiven >= 15, "must anchor to the FIRST wait in the batch, got {forgiven} ticks");
    }

    /// THE same-dispatch stale-anchor fix this closes (the sibling of the CROSS-dispatch case below):
    /// a dialog that resolves comfortably inside its OWN dispatch's budget must NOT leave a stale
    /// anchor that a LATER, wholly unrelated stretch of cpu-bound work in the SAME dispatch (no
    /// further host call of any kind — no dialog, no `exec`, no `proc`) can exploit for a near-full
    /// re-grant of the original budget. Pre-fix, `take_dialog_extra_ticks` computed `remaining` from
    /// the fast call's ancient `first_wait_started` regardless of what happened since, handing the
    /// guest close to a full fresh budget for a wait that had already finished.
    #[test]
    fn take_dialog_extra_ticks_does_not_reward_a_fast_dialog_followed_by_an_unrelated_cpu_runaway() {
        let s = state();
        // A 20-tick (100ms) per-dispatch budget.
        s.arm_epoch_deadline_estimate(20);

        // A dialog resolves almost instantly, well inside budget.
        let wait_started = std::time::Instant::now();
        s.note_dialog_wait(wait_started);

        // The guest then runs long on wholly UNRELATED work for the rest of the dispatch — no further
        // host call of any kind, so no further `note_dialog_wait`. This alone crosses the 100ms budget.
        std::thread::sleep(std::time::Duration::from_millis(90));

        let forgiven = s.take_dialog_extra_ticks();
        assert_eq!(
            forgiven, 0,
            "an unrelated cpu-bound stretch with no further host call must trap (0 forgiveness) — \
             got {forgiven}, meaning a fast, already-finished dialog left a stale anchor that let a \
             genuine SAME-dispatch runaway be handed a near-full re-grant of the original budget"
        );
    }

    /// THE CRITICAL cross-dispatch fix this closes: a dialog wait that resolves comfortably inside its
    /// OWN dispatch's budget never trips `epoch_deadline_callback`, so `take_dialog_extra_ticks` (the
    /// only other thing that used to clear the anchor) never runs — pre-fix, `first_wait_started`
    /// stayed set indefinitely. A LATER, wholly unrelated dispatch (fresh `arm_epoch_deadline_estimate`
    /// call, mirroring `live.rs`'s `store.set_epoch_deadline` at every dispatch entry point) that then
    /// genuinely runs long past ITS OWN budget must trap normally — it must NOT be handed forgiveness
    /// computed from the ancient, unrelated anchor.
    #[test]
    fn arm_epoch_deadline_estimate_clears_a_stale_anchor_left_by_a_fast_dialog_in_an_earlier_dispatch() {
        let s = state();

        // Dispatch A: a small budget, and a dialog that resolves FAST (well inside budget) — the
        // epoch deadline is never reached during dispatch A, so `take_dialog_extra_ticks` never runs
        // and the anchor set by `note_dialog_wait` is never consumed by it.
        s.arm_epoch_deadline_estimate(20);
        s.note_dialog_wait(std::time::Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(5));
        // Dispatch A ends normally (no epoch trap) — nothing ever called `take_dialog_extra_ticks`.

        // Time passes between dispatches (e.g. the guest is idle, or busy with unrelated host work).
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Dispatch M: wholly unrelated to any dialog, mirrors a fresh `live.rs` dispatch entry point
        // (`store.set_epoch_deadline` + `arm_epoch_deadline_estimate`) with its OWN small budget.
        s.arm_epoch_deadline_estimate(20);
        // The guest genuinely runs long in dispatch M (a runaway loop), well past its own 20-tick
        // (100ms) budget, with NO dialog wait of its own.
        std::thread::sleep(std::time::Duration::from_millis(150));

        let forgiven = s.take_dialog_extra_ticks();
        assert_eq!(
            forgiven, 0,
            "a genuine runaway in an UNRELATED later dispatch must trap (0 forgiveness) — got \
             {forgiven}, meaning a fast dialog in an earlier dispatch left a stale anchor that let \
             this dispatch's own epoch trap be defeated"
        );
    }

    /// THE regression this fix closes: `abort_signal` used to unconditionally `insert` any
    /// guest-supplied string with no cap at all — a guest calling it with a fresh id in a loop
    /// could grow `aborted_signals` without bound. With the fix, once `MAX_ABORTED_SIGNALS`
    /// distinct ids are tracked, a NEW distinct id is silently dropped (never marked aborted).
    #[test]
    fn abort_signal_caps_total_distinct_ids_tracked() {
        let s = state();
        for i in 0..MAX_ABORTED_SIGNALS {
            s.abort_signal(format!("sig-{i}"));
        }
        assert_eq!(s.aborted_signals().len(), MAX_ABORTED_SIGNALS, "primed exactly at the cap");
        for id in 0..MAX_ABORTED_SIGNALS {
            assert!(s.is_signal_aborted(&format!("sig-{id}")), "every id under the cap stays tracked");
        }

        // One more DISTINCT id beyond the cap must be silently dropped, not tracked.
        s.abort_signal("sig-over-the-cap".to_string());
        assert_eq!(
            s.aborted_signals().len(),
            MAX_ABORTED_SIGNALS,
            "the registry must never grow past the cap"
        );
        assert!(
            !s.is_signal_aborted("sig-over-the-cap"),
            "an id that arrived after the cap was reached must not be marked aborted"
        );
    }

    /// Re-aborting an id that is ALREADY tracked is always a no-op success, even once the registry
    /// is fully at the cap — the cap bounds DISTINCT ids, not re-inserts of the same one.
    #[test]
    fn abort_signal_re_marking_an_already_tracked_id_at_the_cap_still_works() {
        let s = state();
        for i in 0..MAX_ABORTED_SIGNALS {
            s.abort_signal(format!("sig-{i}"));
        }
        s.abort_signal("sig-0".to_string()); // already tracked — must remain a harmless no-op.
        assert!(s.is_signal_aborted("sig-0"));
        assert_eq!(s.aborted_signals().len(), MAX_ABORTED_SIGNALS);
    }
}
