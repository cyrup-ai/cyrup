//! `LiveHostServices` — the concrete [`cyrup_ext::host::HostServices`] backend the session injects
//! (arch-08 §5.6; retires the cyrup-ext "outer-layer" ledger row). cyrup-ext ships the trait plus a
//! deny-all [`cyrup_ext::host::DenyServices`] default and a [`cyrup_ext::host::RecordingServices`]
//! test double; this is the REAL backend wired to the running session's provider + active model +
//! a command-tier control sink, so a loaded extension's `models`/`session`/`control` capabilities
//! reflect live runtime state instead of returning empty/denied.
//!
//! The `HostServices` trait methods are synchronous (the guest is suspended across the host call),
//! while the session's manager is async-locked. `LiveHostServices` therefore reads from a small
//! sync snapshot the session pushes on model/state changes, plus the provider's (sync) model list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, EntryId, ModelRef};
use cyrup_ext::caps::http::HttpCaps;
use cyrup_ext::caps::proc::ProcCaps;
use cyrup_ext::host::{
    ControlOp, DialogOptions, ExecOutput, HostServices, HttpRequest, HttpResponse,
    HttpStreamResponse, ProcSpawnSpec,
};
use cyrup_provider::Provider;
use cyrup_session::manager::SessionManager;
use cyrup_tools::{ArgvSpec, ExitStatus, ProcOps};
use serde_json::{json, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;

use crate::event::AgentSessionEvent;

/// A command-tier control sink: a loaded extension's `control` import (new/switch/fork/…) is routed
/// here so the runtime can act on it (Pi `createCommandContext`, agent-session.ts:1158). Set by the
/// runtime once it owns the session; until then control ops are reported as unavailable.
pub type ControlSink = Arc<dyn Fn(ControlOp) -> Result<(), String> + Send + Sync>;

/// Which dialog family a [`UiRequest`] carries (Pi `ExtensionUIContext.{confirm,input,select,editor}`,
/// types.ts:127-133,216).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKind {
    Confirm,
    Input,
    Select,
    Editor,
}

/// The value a dialog renderer sends back to the wasm-suspended guest (the REPLY half of the
/// request/reply [`UiSink`]). `Confirm` -> `confirm` bool; `Text` -> `input`/`editor`/`select`
/// `option<string>` (Pi `select(title, options, opts): Promise<string|undefined>`, types.ts:127,
/// and the WIT `select` return, world.wit:259 — the chosen option STRING, zero index bookkeeping).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiReply {
    Confirm(bool),
    Text(Option<String>),
}

/// A single dialog request routed from a loaded extension's `ui.{confirm,input,select,editor}`
/// capability to the mode's dialog renderer (the interactive TUI selector, or the RPC
/// `extension_ui_request`/`extension_ui_response` round-trip). This is the REQUEST/REPLY inverse of
/// the fire-and-forget [`ControlSink`]: the guest coroutine is wasm-suspended across the SYNC host
/// call (Pi's `ExtensionUIContext` methods RETURN a value the extension awaits, types.ts:127-133,216),
/// so the host BLOCKS on `reply` until the renderer answers, rather than queueing and returning `()`.
pub struct UiRequest {
    pub kind: UiKind,
    /// The dialog prompt/title (Pi `title`); for `editor`, the seed text (Pi `prefill`).
    pub prompt: String,
    /// For `select`, the JSON array of option strings (Pi `options`); `Null` for the other kinds.
    pub options: Value,
    /// `confirm`'s message body (Pi `confirm(title, message, opts)`, rpc-types.ts:232); empty string
    /// for the other kinds (L4 review §2.6).
    pub message: String,
    /// `input`'s placeholder (Pi `input(title, placeholder, opts)`, rpc-types.ts:233-240); `None` for
    /// the other kinds, or when the guest omitted it (L4 review §2.7).
    pub placeholder: Option<String>,
    /// The Pi `ExtensionUIDialogOptions` bag (`{timeoutMs, signalId}`, types.ts:89).
    pub opts: DialogOptions,
    /// The one-shot the renderer fulfils to resume the suspended guest.
    pub reply: tokio::sync::oneshot::Sender<UiReply>,
}

/// A request/reply dialog sink: a loaded extension's `ui.*` capability is routed here so the active
/// mode's renderer (TUI / RPC) can service it and reply. Set by the mode entry point via
/// [`LiveHostServices::set_ui_sink`]; absent (`None`) in headless (print/json), where the ui methods
/// fall back to the deny defaults (== Pi `noOpUIContext`, runner.ts:230-261).
pub type UiSink = UnboundedSender<UiRequest>;

/// The sync snapshot the session keeps current for the (sync) host-services reads.
#[derive(Clone, Debug, Default)]
struct LiveSnapshot {
    model: Option<ModelRef>,
    context_window: u64,
    used_tokens: u64,
    session_name: Option<String>,
    thinking_level: Option<String>,
}

/// The live host-services backend (arch-08 §5.6).
pub struct LiveHostServices {
    provider: Arc<dyn Provider>,
    /// The process backend the `exec` capability grant runs argv (shell:false) commands through
    /// (Pi `execCommand`, exec.ts:34-46). Shared with the session's `bash` seam (the same
    /// [`cyrup_tools::ProcOps`]), so a granted extension execs through the real local process ops.
    proc: Arc<dyn ProcOps>,
    /// The session cwd — the default working directory for an `exec` with no `cwd` option (Pi's
    /// `execCommand(..., opts?.cwd ?? cwd)` where `cwd` is the extension's cwd, loader.ts:317-320).
    cwd: PathBuf,
    /// The `http-client` capability grant's real `reqwest`-backed engine (arch-08 §3.2 draft;
    /// pi-mcp-adapter-port.md §3.2). Gated by the SAME load-time trust check as `exec` (reaching this
    /// backend at all means the guest already passed the trust gate) — no per-call check here either.
    http: HttpCaps,
    /// The `proc` capability grant's real long-lived-child engine (arch-08 §5.2 request/poll bridge;
    /// pi-mcp-adapter-port.md §3.1). Gated by the SAME load-time trust check as `exec`/`http-client`.
    proc_caps: ProcCaps,
    snapshot: Mutex<LiveSnapshot>,
    control: Mutex<Option<ControlSink>>,
    /// The active mode's dialog renderer (interactive TUI / RPC), attached post-build via
    /// [`Self::set_ui_sink`]. A guest's `ui.{confirm,input,select,editor}` capability reaches the SYNC
    /// [`HostServices`] method (the guest is wasm-suspended and cannot await), which forwards a
    /// [`UiRequest`] here and BLOCKS on the one-shot reply. `None` in headless (print/json): the
    /// overrides then fall through to the trait deny defaults (== Pi `noOpUIContext`) and never block.
    ui_sink: Mutex<Option<UiSink>>,
    /// Receiver half of the command-tier control channel (see [`Self::wire_control_channel`]). A
    /// guest's `control` capability call reaches the SYNC [`HostServices::control`] method (the
    /// guest is wasm-suspended and cannot await), which forwards the [`ControlOp`] here; the session
    /// drains + applies it at a command-tier-safe point (Pi runs `createCommandContext` ops directly,
    /// agent-session.ts:1158 — cyrup bridges the sync→async gap via this queue).
    control_rx: Mutex<Option<UnboundedReceiver<ControlOp>>>,
    /// The running session's tree manager, attached post-build via [`Self::attach_session`]. A guest's
    /// `append_entry`/`set_session_name`/`set_label` capability mutates it DIRECTLY (Pi appends
    /// synchronously — `SessionManager.appendCustomEntry`/`setSessionName`/`setLabel`,
    /// agent-session.ts:2265-2279). `None` until attached (default host: no session-mutation authority).
    manager: Mutex<Option<Arc<AsyncMutex<SessionManager>>>>,
    /// Facade events a guest state-mutation queued (`entry_appended`/`session_info_changed`), drained
    /// and fanned out by [`crate::AgentSession::apply_pending_control`] after the guest call settles —
    /// the same sync→async bridge point the control queue uses.
    pending_events: Mutex<Vec<AgentSessionEvent>>,
}

impl LiveHostServices {
    /// Wire a backend to the session's `provider`, process ops (`proc`), and session `cwd`. Model/state
    /// are seeded via [`Self::update_model`] and [`Self::update_state`]; the control sink is attached
    /// later by the runtime. `proc` + `cwd` back the `exec` capability grant (Pi `execCommand`).
    pub fn new(provider: Arc<dyn Provider>, proc: Arc<dyn ProcOps>, cwd: PathBuf) -> Self {
        Self {
            provider,
            proc,
            cwd,
            http: HttpCaps::new(),
            proc_caps: ProcCaps::new(),
            snapshot: Mutex::new(LiveSnapshot::default()),
            control: Mutex::new(None),
            ui_sink: Mutex::new(None),
            control_rx: Mutex::new(None),
            manager: Mutex::new(None),
            pending_events: Mutex::new(Vec::new()),
        }
    }

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Push the active model + its context window (the session calls this on build + `set_model`).
    pub fn update_model(&self, model: ModelRef, context_window: u64, thinking_level: Option<String>) {
        let mut g = Self::lock(&self.snapshot);
        g.model = Some(model);
        g.context_window = context_window;
        g.thinking_level = thinking_level;
    }

    /// Push session-level state (name + last-turn token occupancy) for the read views.
    pub fn update_state(&self, session_name: Option<String>, used_tokens: u64) {
        let mut g = Self::lock(&self.snapshot);
        g.session_name = session_name;
        g.used_tokens = used_tokens;
    }

    /// Attach the command-tier control sink (the runtime owns it once the session is live).
    pub fn set_control_sink(&self, sink: ControlSink) {
        *Self::lock(&self.control) = Some(sink);
    }

    /// Attach the mode's dialog renderer (the interactive TUI selector arm, or the RPC
    /// `extension_ui_request` emitter). Only interactive/rpc call this; headless (print/json) leaves it
    /// `None`, which is what keeps the ui overrides returning the deny defaults WITHOUT blocking — the
    /// absence of a sink IS the headless policy, mirroring Pi's absence of a `uiContext`.
    pub fn set_ui_sink(&self, sink: UiSink) {
        *Self::lock(&self.ui_sink) = Some(sink);
    }

    /// Route one dialog request to the attached renderer and BLOCK (the guest is wasm-suspended) on the
    /// reply — the request/reply counterpart to the fire-and-forget [`Self::control`]. Returns `None`
    /// when there is no sink (headless: the ui method then yields its deny default WITHOUT blocking),
    /// when the renderer dropped the reply (cancelled / shut down), OR when `opts.timeout_ms` elapses
    /// with no reply — the caller then falls through to its per-kind deny default, matching Pi's
    /// `createDialogPromise`'s host-armed `setTimeout(() => resolve(defaultValue), opts.timeout)`
    /// (`rpc-mode.ts:114-119`), which ALWAYS settles the dialog within `opts.timeout` ms regardless of
    /// client behavior (closes L4 review §2.2). A renderer can ALSO force-resolve an already-open
    /// dialog early by sending on the SAME `reply` one-shot this call is waiting on (e.g. the RPC loop's
    /// `pending` map on `abort`/`abort_retry`, `rpc.rs` — closes L4 review §2.5); that arrives on the
    /// `reply_rx` branch below like any ordinary answer, no extra wiring needed here. Uses the SAME
    /// `block_in_place` + `block_on` pattern the `exec` grant uses ([`Self::exec`]); requires a
    /// multi-threaded runtime, which interactive/rpc guarantee
    /// (`#[tokio::main(flavor = "multi_thread")]`, main.rs:40).
    fn ui_roundtrip(
        &self,
        kind: UiKind,
        prompt: &str,
        options: Value,
        message: String,
        placeholder: Option<String>,
        opts: &DialogOptions,
    ) -> Option<UiReply> {
        let sink = Self::lock(&self.ui_sink).clone()?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = UiRequest {
            kind,
            prompt: prompt.to_string(),
            options,
            message,
            placeholder,
            opts: opts.clone(),
            reply: reply_tx,
        };
        if sink.send(request).is_err() {
            // The renderer (TUI loop / RPC loop) is gone — degrade to the deny default, never a panic.
            return None;
        }
        // `0` means NO timeout, not an instant one — Pi's `createDialogPromise` only arms the timer
        // `if (opts?.timeout)` (`rpc-mode.ts:114`; falsy-zero in JS ⇒ no timer at all), and both real
        // dialog callers double down on the same check (`opts.timeout && opts.timeout > 0`,
        // `extension-selector.ts:51`, `extension-input.ts:54`). Mirror the `> 0` guard the sibling
        // `exec` grant already applies to `timeoutMs` just below ([`Self::exec`]) and the TUI's own
        // countdown applies to the same field (`cyrup-tui/src/app.rs`'s `.filter(|&ms| ms > 0)`).
        let timeout = opts.timeout_ms.filter(|ms| *ms > 0).map(Duration::from_millis);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                match timeout {
                    // Race the reply against a live countdown — Pi's `setTimeout` safety net. Whichever
                    // settles first wins; on timeout the reply half is dropped (never polled again), so
                    // a late answer simply finds its `reply.send` fail harmlessly (`Err`, never a panic).
                    Some(d) => tokio::select! {
                        biased;
                        reply = reply_rx => reply.ok(),
                        () = tokio::time::sleep(d) => None,
                    },
                    None => reply_rx.await.ok(),
                }
            })
        })
    }

    /// Wire the command-tier control channel: a loaded extension's `control` capability (new/switch/
    /// fork/compact/set-model/…) is forwarded onto an in-process queue the session drains via
    /// [`Self::take_pending_control`]. This is the bridge that lets a wasm guest (suspended across
    /// the SYNC `control()` call) drive a real, ASYNC session effect. Idempotent: re-wiring replaces
    /// the channel (a fresh session generation gets a fresh queue).
    pub fn wire_control_channel(&self) {
        let (tx, rx): (UnboundedSender<ControlOp>, UnboundedReceiver<ControlOp>) =
            tokio::sync::mpsc::unbounded_channel();
        self.set_control_sink(Arc::new(move |op| {
            tx.send(op).map_err(|e| format!("control channel closed: {e}"))
        }));
        *Self::lock(&self.control_rx) = Some(rx);
    }

    /// Drain every queued control op (non-blocking). The session applies the session-tier ops and
    /// hands the rest to the runtime (Pi `createCommandContext`, agent-session.ts:1158).
    pub fn take_pending_control(&self) -> Vec<ControlOp> {
        let mut g = Self::lock(&self.control_rx);
        let mut out = Vec::new();
        if let Some(rx) = g.as_mut() {
            while let Ok(op) = rx.try_recv() {
                out.push(op);
            }
        }
        out
    }

    /// Attach the running session's tree manager so a guest's state-mutating capabilities
    /// (`append_entry`/`set_session_name`/`set_label`) reach the REAL session tree (arch-08 §5.6).
    /// The builder calls this once the `Arc<AsyncMutex<SessionManager>>` exists (step 10).
    pub fn attach_session(&self, manager: Arc<AsyncMutex<SessionManager>>) {
        *Self::lock(&self.manager) = Some(manager);
    }

    /// Drain the facade events queued by guest state mutations (entry_appended/session_info_changed);
    /// [`crate::AgentSession::apply_pending_control`] fans them out on the live streams. The guest is
    /// wasm-suspended across the SYNC mutation, so — mirroring the control queue — the ASYNC fan-out
    /// runs at the next command-tier-safe drain.
    pub fn take_pending_events(&self) -> Vec<AgentSessionEvent> {
        std::mem::take(&mut *Self::lock(&self.pending_events))
    }

    /// Acquire the attached manager without blocking (the guest host call runs on the session task
    /// while the manager lock is free — Pi appends synchronously). `Err` (never a panic) when the
    /// session is unattached or transiently busy, surfaced to the guest as a WIT `result` error.
    fn with_manager<R>(
        &self,
        f: impl FnOnce(&mut SessionManager) -> Result<R, String>,
    ) -> Result<R, String> {
        let manager = Self::lock(&self.manager).clone().ok_or("session not attached")?;
        let mut guard = manager.try_lock().map_err(|_| "session busy".to_string())?;
        f(&mut guard)
    }
}

impl HostServices for LiveHostServices {
    // --- ui dialog grant (arch-08 §5.6; Pi `ExtensionUIContext`, types.ts:127-133,216) ---
    // Reaching here means the load-time trust gate already passed (an untrusted extension gets
    // `DenyServices`, whose ui methods return false/None), so like the `exec` grant there is NO extra
    // per-call trust/tier check: ui works at both Event and Command tier, purely a function of whether
    // a mode installed a `ui_sink`. With NO sink (headless print/json) every method falls through to
    // the deny default WITHOUT blocking — byte-for-byte Pi `noOpUIContext` (runner.ts:230-261).

    fn confirm(&self, prompt: &str, message: &str, opts: &DialogOptions) -> bool {
        match self.ui_roundtrip(UiKind::Confirm, prompt, Value::Null, message.to_string(), None, opts) {
            Some(UiReply::Confirm(b)) => b,
            _ => false,
        }
    }

    fn input(&self, prompt: &str, placeholder: Option<&str>, opts: &DialogOptions) -> Option<String> {
        let placeholder = placeholder.map(str::to_string);
        match self.ui_roundtrip(UiKind::Input, prompt, Value::Null, String::new(), placeholder, opts) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn select(&self, prompt: &str, options: &Value, opts: &DialogOptions) -> Option<String> {
        match self.ui_roundtrip(UiKind::Select, prompt, options.clone(), String::new(), None, opts) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn editor(&self, initial: &str) -> Option<String> {
        // The WIT `editor(initial) -> option<string>` carries no options bag (world.wit:261); use the
        // empty default so the roundtrip signature stays uniform.
        match self.ui_roundtrip(
            UiKind::Editor,
            initial,
            Value::Null,
            String::new(),
            None,
            &DialogOptions::default(),
        ) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn models(&self) -> Value {
        serde_json::to_value(self.provider.models()).unwrap_or_else(|_| json!([]))
    }

    fn current_model(&self) -> Option<String> {
        Self::lock(&self.snapshot)
            .model
            .as_ref()
            .map(|m| format!("{}/{}", m.provider.as_str(), m.model.as_str()))
    }

    fn thinking_level(&self) -> Option<String> {
        Self::lock(&self.snapshot).thinking_level.clone()
    }

    fn context_usage(&self) -> Value {
        let g = Self::lock(&self.snapshot);
        let fraction = if g.context_window == 0 {
            0.0
        } else {
            (g.used_tokens as f64 / g.context_window as f64).clamp(0.0, 1.0)
        };
        json!({
            "usedTokens": g.used_tokens,
            "contextWindow": g.context_window,
            "fraction": fraction,
        })
    }

    fn session_name(&self) -> Option<String> {
        Self::lock(&self.snapshot).session_name.clone()
    }

    fn control(&self, op: ControlOp) -> Result<(), String> {
        let sink = Self::lock(&self.control).clone();
        match sink {
            Some(f) => f(op),
            None => Err("control capability not yet wired to a runtime".into()),
        }
    }

    fn exec(
        &self,
        cmd: &str,
        args: &[String],
        opts: &Value,
        cancel: CancelToken,
    ) -> Result<ExecOutput, String> {
        // The `exec` GRANT (arch-08 §5.6): reaching here means the load-time trust gate already said
        // yes (`is_trusted = origin.is_pre_trust() || project_trusted`, loader.rs:57-60, enforced
        // facade.rs:563) — an untrusted extension gets `DenyServices` and never lands here. So this
        // adds NO extra trust/tier check; it just runs the command, 1:1 with Pi `execCommand`
        // (exec.ts:34-46): shell:false argv, `cwd ?? sessionCwd`, and a `timeoutMs` that SIGTERMs
        // then, after a 5s grace period, SIGKILLs (killed=true) the process GROUP on expiry — Pi's
        // exact `killProcess` escalation (exec.ts:52-63), implemented by `LocalProc::exec_argv`'s
        // SIGTERM/grace/SIGKILL loop (`cyrup-tools/src/ops/local.rs`). Deliberately does NOT honor a
        // guest-supplied `env` key: Pi's real `execCommand` never passes an `env` override to
        // `spawn()` at all (`exec.ts:41-45`) — the child only ever inherits the host's own ambient
        // environment. Accepting one here would be new ambient authority (arbitrary env injection
        // for a spawned process) with no Pi equivalent (`cyrup-ext-sdk::descriptor::ExecOptions` has
        // no `env` field for exactly this reason) — do not re-add without a real Pi citation.
        let cwd = opts
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cwd.clone());
        let timeout = opts
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis);
        let spec =
            ArgvSpec { program: cmd.to_string(), args: args.to_vec(), cwd, env: Vec::new() };
        let proc = self.proc.clone();
        // The `HostServices` trait is sync (the guest is wasm-suspended across the call); drive the
        // async process ops to completion on the current multi-threaded runtime worker.
        let outcome = tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(proc.exec_argv(spec, cancel, timeout))
        });
        let out = match outcome {
            Ok(o) => o,
            // Pi `execCommand` never rejects: a spawn/wait failure resolves `{code:1}` (exec.ts:99-105).
            Err(_) => {
                return Ok(ExecOutput { code: 1, stdout: String::new(), stderr: String::new(), killed: false });
            }
        };
        // Map onto Pi's `{code, killed}` (exec.ts:49,97; `child-process.ts:73-80`): `killed` is set
        // the instant a SIGTERM/SIGKILL escalation is INITIATED and is completely orthogonal to
        // `code` — a process that catches SIGTERM and exits itself mid-grace still reports its REAL
        // exit code, `killed` never masks it. `out.killed`/`out.status` already preserve exactly that
        // split (`LocalProc::exec_argv`); do not re-derive `killed` from the status variant.
        let killed = out.killed;
        let code = match out.status {
            ExitStatus::Exited(n) => n,
            ExitStatus::Signaled | ExitStatus::Killed | ExitStatus::TimedOut => 0,
        };
        Ok(ExecOutput {
            code,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            killed,
        })
    }

    // --- http-client GRANT (arch-08 §3.2 draft; pi-mcp-adapter-port.md §3.2) ---
    // Reaching here means the load-time trust gate already said yes (the SAME gate `exec` uses,
    // `is_trusted = origin.is_pre_trust() || project_trusted`, loader.rs:57-60) — an untrusted
    // extension gets `DenyServices` and never lands here. So, like `exec`, this adds NO extra
    // trust/tier check or per-host allowlist; it just runs the request through the real `HttpCaps`
    // engine. The `HostServices` trait is sync (the guest is wasm-suspended across the call); drive
    // the async `reqwest` calls to completion on the current multi-threaded runtime worker — the SAME
    // `block_in_place` + `block_on` bridge the `exec` grant uses.

    fn http_request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.http.request(req))
        })
    }

    fn http_request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.http.request_stream(req))
        })
    }

    fn http_poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.http.poll_stream_chunk(handle))
        })
    }

    fn http_close_stream(&self, handle: u32) {
        self.http.close_stream(handle);
    }

    // --- proc GRANT (arch-08 §5.2 request/poll bridge; pi-mcp-adapter-port.md §3.1) ---
    // Reaching here means the load-time trust gate already said yes (the SAME gate `exec`/
    // `http-client` use) — no extra trust/tier check. `spawn`/`read_stdout`/`read_stderr`/
    // `poll_exit` are sync (no `.await` in `ProcCaps` for those); `write_stdin`/`kill` need to
    // `.await` real pipe I/O / process-termination confirmation, driven to completion via the SAME
    // `block_in_place` + `block_on` bridge `exec`/`http_request` use.

    fn proc_spawn(&self, spec: &ProcSpawnSpec) -> Result<u32, String> {
        // Default an omitted `cwd` to the session's own project directory — the SAME fallback
        // `exec` applies (`opts.cwd ?? self.cwd` above), for the SAME reason: the real consumer's
        // own default (`server-manager.ts:110`'s `resolveConfigPath(definition.cwd)`, which is
        // `undefined` when `definition.cwd` is `undefined`, `utils.ts:78-80`) relies on ITS
        // coordinating process's OWN ambient `process.cwd()` reliably already BEING the project
        // directory, since pi-mcp-adapter runs as part of the per-invocation coding-agent process
        // rooted there. `cyrup-session-svc` is architected as a long-lived MULTI-session service
        // with an explicit per-session `cwd` field precisely because the ambient host-process cwd
        // is NOT a reliable stand-in for a given session's project directory here — so, unlike Pi,
        // omitting `cwd` must not silently fall through to `tokio::process::Command`'s own default
        // (inheriting the HOST's ambient cwd, not the calling session's), or a guest-authored
        // MCP-client extension that (correctly, matching Pi) omits `cwd` could spawn the server in
        // the wrong directory under concurrent multi-session deployment.
        let spec = if spec.cwd.is_none() {
            ProcSpawnSpec { cwd: Some(self.cwd.clone()), ..spec.clone() }
        } else {
            spec.clone()
        };
        self.proc_caps.spawn(&spec)
    }

    fn proc_write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.proc_caps.write_stdin(handle, data))
        })
    }

    fn proc_read_stdout(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.proc_caps.read_stdout(handle, max_bytes)
    }

    fn proc_read_stderr(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.proc_caps.read_stderr(handle, max_bytes)
    }

    fn proc_poll_exit(&self, handle: u32) -> Option<i32> {
        self.proc_caps.poll_exit(handle)
    }

    fn proc_kill(&self, handle: u32) -> Result<(), String> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.proc_caps.kill(handle))
        })
    }

    fn append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String> {
        // Persist the custom (non-LLM) entry into the LIVE session tree (Pi
        // `sessionManager.appendCustomEntry`, agent-session.ts:2265-2271) and snapshot the persisted
        // entry for the `entry_appended` fan-out.
        let (id, entry) = self.with_manager(|mgr| {
            let id = mgr.append_custom_entry(custom_type, Some(data.clone())).map_err(|e| e.to_string())?;
            let entry = mgr.entry(&id).and_then(|e| serde_json::to_value(e).ok()).unwrap_or(Value::Null);
            Ok((id, entry))
        })?;
        Self::lock(&self.pending_events).push(AgentSessionEvent::EntryAppended { entry });
        Ok(id.to_string())
    }

    fn set_session_name(&self, name: &str) {
        // Rename the live session (Pi `setSessionName` → `appendSessionInfo`, agent-session.ts:2690).
        let resolved = self.with_manager(|mgr| {
            mgr.append_session_info(name).map_err(|e| e.to_string())?;
            Ok(mgr.session_name())
        });
        if let Ok(resolved) = resolved {
            // Keep the sync read-view snapshot current (guest `getSessionName` reflects the rename)
            // and queue the `session_info_changed` fan-out (Pi `_emit`, agent-session.ts:2714).
            Self::lock(&self.snapshot).session_name = resolved.clone();
            Self::lock(&self.pending_events)
                .push(AgentSessionEvent::SessionInfoChanged { name: resolved });
        }
    }

    fn set_label(&self, entry_id: &str, label: &str) {
        // Set/replace the entry's label on the live tree (Pi `setLabel` → `appendLabel`,
        // agent-session.ts:2276-2279). A no-op result (unknown id / busy) degrades silently.
        let _ = self.with_manager(|mgr| {
            mgr.append_label(&EntryId::from(entry_id), Some(label)).map_err(|e| e.to_string())?;
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_provider::faux::FauxProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A backend seeded with the real local process ops + a temp cwd (the `exec` grant path).
    fn svc_with(provider: Arc<dyn Provider>) -> LiveHostServices {
        LiveHostServices::new(provider, cyrup_tools::Backend::default().proc, std::env::temp_dir())
    }

    #[test]
    fn reflects_live_model_and_models_catalog() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider.clone());

        // Before wiring: no current model, control denied, but the catalog is live from the provider.
        assert!(svc.current_model().is_none());
        assert!(svc.control(ControlOp::Reload).is_err());
        let models = svc.models();
        assert!(models.is_array(), "models() must serialize the provider catalog");
        assert!(!models.as_array().unwrap().is_empty(), "faux provider has at least one model");

        // After the session pushes its active model, the read reflects it.
        let m = ModelRef { provider: "faux".into(), api: None, model: "faux-1".into() };
        svc.update_model(m, 128_000, Some("medium".into()));
        svc.update_state(Some("my session".into()), 42);
        assert_eq!(svc.current_model().as_deref(), Some("faux/faux-1"));
        assert_eq!(svc.thinking_level().as_deref(), Some("medium"));
        assert_eq!(svc.session_name().as_deref(), Some("my session"));
        let usage = svc.context_usage();
        assert_eq!(usage["usedTokens"], json!(42));
        assert_eq!(usage["contextWindow"], json!(128_000));
    }

    #[test]
    fn control_routes_to_the_wired_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        svc.set_control_sink(Arc::new(move |_op| {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        svc.control(ControlOp::Reload).expect("control routes to the sink");
        svc.control(ControlOp::Compact).expect("control routes to the sink");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    /// The `exec` grant runs a DIRECT argv (shell:false) command and returns the REAL captured
    /// output/code/killed — 1:1 with Pi `execCommand` (exec.ts:34-46). Multi-thread runtime so the
    /// sync grant can `block_in_place` on the async process ops.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_runs_argv_with_cwd_env_and_reports_killed_on_timeout() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);

        // 1) Real stdout + exit code, NO shell (argv `echo hi`).
        let out = svc
            .exec("echo", &["hi".to_string()], &json!({}), CancelToken::new())
            .expect("echo runs via the exec grant");
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.code, 0);
        assert!(!out.killed, "a natural exit is not `killed`");

        // 2) shell:false — an argv that a shell would splice is passed literally, so `echo` prints the
        //    metacharacters verbatim (proves no `bash -c` word-splitting).
        let out = svc
            .exec("echo", &["a; echo b".to_string()], &json!({}), CancelToken::new())
            .expect("echo runs");
        assert_eq!(out.stdout, "a; echo b\n", "argv is literal — no shell interpretation");

        // 3) `cwd` option honored (Pi `opts?.cwd ?? cwd`).
        let tmp = std::env::temp_dir();
        let out = svc
            .exec("pwd", &[], &json!({ "cwd": tmp.to_string_lossy() }), CancelToken::new())
            .expect("pwd runs");
        let printed = std::fs::canonicalize(out.stdout.trim_end()).unwrap_or_default();
        assert_eq!(printed, std::fs::canonicalize(&tmp).unwrap_or(tmp), "exec ran in the given cwd");

        // 4) a guest-supplied `env` key is IGNORED — Pi's real `execCommand` (exec.ts:41-45) never
        //    accepts an env override at all; the child only inherits the host's own ambient
        //    environment (Node `spawn()`'s default when no `env` key is passed). If the `exec` grant
        //    honored a guest's `env`, `printenv` would see the injected value; instead the lookup
        //    variable must be genuinely UNSET in the child (nonzero exit, empty stdout) — proving
        //    this is NOT new ambient authority beyond Pi's real surface.
        let out = svc
            .exec(
                "printenv",
                &["CYRUP_EXEC_TEST_ENV_MUST_BE_IGNORED".to_string()],
                &json!({ "env": { "CYRUP_EXEC_TEST_ENV_MUST_BE_IGNORED": "injected" } }),
                CancelToken::new(),
            )
            .expect("printenv runs (even though the variable it looks up is unset)");
        assert_ne!(
            out.code, 0,
            "a guest-supplied `env` override must be ignored — printenv must NOT find an injected \
             value"
        );
        assert!(out.stdout.is_empty(), "no injected value may ever reach the child's environment");

        // 5) `timeoutMs` ⇒ the host SIGTERMs the group, then (since `sleep` obeys SIGTERM and dies
        //    well within the 5s grace period, no SIGKILL escalation needed here) reports
        //    `killed=true` (Pi `killProcess` sets `killed`, exec.ts:52-63).
        let out = svc
            .exec("sleep", &["30".to_string()], &json!({ "timeoutMs": 100 }), CancelToken::new())
            .expect("sleep runs then is killed on timeout");
        assert!(out.killed, "a timed-out exec is `killed`");

        // 6) an already-aborted signal (pre-cancelled token) kills immediately ⇒ `killed=true`.
        let cancelled = CancelToken::new();
        cancelled.cancel();
        let out = svc
            .exec("sleep", &["30".to_string()], &json!({}), cancelled)
            .expect("a pre-cancelled exec resolves");
        assert!(out.killed, "a pre-aborted signal kills the exec");

        // 7) a well-behaved child that TRAPS SIGTERM and exits itself with its OWN real code must
        //    have that REAL code surfaced through the grant end-to-end — `killed` is orthogonal,
        //    never masking it — 1:1 with Pi's `{code, killed}` (`exec.ts:97`; `child-process.ts:73-
        //    80`'s `finalize(exitCode)` always carries the real observed code).
        let out = svc
            .exec(
                "sh",
                &["-c".to_string(), "trap 'exit 7' TERM; while true; do sleep 1; done".to_string()],
                &json!({ "timeoutMs": 100 }),
                CancelToken::new(),
            )
            .expect("the SIGTERM-trapping child runs then exits itself");
        assert_eq!(out.code, 7, "the child's own real exit code survives a host-initiated kill");
        assert!(out.killed, "a timeout-initiated kill is still `killed`, independent of `code`");
    }

    /// The `proc` grant's `spawn` defaults an OMITTED `cwd` to the session's own project directory —
    /// the SAME fallback `exec` applies (test 3 above, `opts.cwd ?? self.cwd`) — rather than
    /// silently inheriting the HOST PROCESS's own ambient cwd (`tokio::process::Command`'s default
    /// when no `.current_dir()` call is made at all). Verified by actually running `pwd` inside the
    /// spawned child and reading its REAL stdout, not asserting on `ProcSpawnSpec` construction.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn proc_spawn_defaults_omitted_cwd_to_the_session_cwd() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        let session_cwd = std::env::temp_dir();

        // No `cwd` in the spec at all — must run in the SESSION's cwd, not the host's ambient one
        // (this test binary's own cwd is the crate root, which must NOT be what `pwd` prints).
        let spec = ProcSpawnSpec {
            cmd: "pwd".to_string(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
            capture_stderr: false,
        };
        let handle = svc.proc_spawn(&spec).expect("pwd spawns with no cwd override");
        let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
        let printed = std::fs::canonicalize(stdout.trim_end()).unwrap_or_default();
        assert_eq!(
            printed,
            std::fs::canonicalize(&session_cwd).unwrap_or(session_cwd),
            "an omitted cwd must default to the SESSION's cwd, not the host process's ambient one"
        );

        // An EXPLICIT `cwd` in the spec is still honored verbatim (the fallback only fires when
        // `cwd` is `None`, never overriding a guest-supplied value).
        let explicit = std::env::current_dir().expect("host has a cwd");
        let spec = ProcSpawnSpec {
            cmd: "pwd".to_string(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: Some(explicit.clone()),
            capture_stderr: false,
        };
        let handle = svc.proc_spawn(&spec).expect("pwd spawns with an explicit cwd");
        let stdout = wait_for_exit_and_read_stdout(&svc, handle).await;
        let printed = std::fs::canonicalize(stdout.trim_end()).unwrap_or_default();
        assert_eq!(
            printed,
            std::fs::canonicalize(&explicit).unwrap_or(explicit),
            "an explicit cwd is honored verbatim, not overridden by the session-cwd fallback"
        );
    }

    /// Poll `proc_poll_exit` until the child reaps, then drain its real stdout — used by tests that
    /// need a spawned child's actual captured output rather than just an `Ok` handle.
    #[cfg(unix)]
    async fn wait_for_exit_and_read_stdout(svc: &LiveHostServices, handle: u32) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if svc.proc_poll_exit(handle).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let bytes = svc.proc_read_stdout(handle, 65536).expect("read_stdout on a live handle");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// With NO ui sink attached (headless print/json: `set_ui_sink` is never called), the ui grant
    /// falls through to the trait deny defaults WITHOUT blocking — byte-for-byte Pi `noOpUIContext`
    /// (confirm=false, input/select/editor=None). A single-thread runtime proves it never touches
    /// `block_in_place` (which would panic here) on the headless path.
    #[test]
    fn headless_ui_returns_deny_defaults_without_a_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        assert!(!svc.confirm("ok?", "body", &DialogOptions::default()));
        assert_eq!(svc.input("name?", Some("placeholder"), &DialogOptions::default()), None);
        assert_eq!(svc.select("pick", &json!(["a", "b"]), &DialogOptions::default()), None);
        assert_eq!(svc.editor("seed"), None);
    }

    /// The ui GRANT round-trips a dialog through a scripted [`UiSink`] renderer: the guest-facing
    /// (sync) `confirm`/`input`/`select`/`editor` block on a one-shot while a concurrent responder
    /// answers each [`UiRequest`], exactly as the interactive TUI selector / RPC round-trip does at
    /// runtime. Multi-thread so the `block_in_place` + `block_on` reply-wait is legal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_round_trips_through_a_scripted_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);

        // L4 review §2.6/§2.7 live proof: capture each request's `message`/`placeholder` as the
        // scripted renderer sees them, so the test can assert they arrived distinct from `prompt`.
        #[derive(Clone, Debug)]
        struct Seen {
            kind: UiKind,
            prompt: String,
            message: String,
            placeholder: Option<String>,
        }
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        // The scripted renderer: reply to each request by kind (like a user picking in the selector).
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                seen2.lock().unwrap_or_else(|e| e.into_inner()).push(Seen {
                    kind: req.kind,
                    prompt: req.prompt.clone(),
                    message: req.message.clone(),
                    placeholder: req.placeholder.clone(),
                });
                let reply = match req.kind {
                    UiKind::Confirm => UiReply::Confirm(true),
                    UiKind::Input => UiReply::Text(Some(format!("answer:{}", req.prompt))),
                    UiKind::Select => {
                        // Echo back the LAST option string as the chosen value proof.
                        let chosen = req
                            .options
                            .as_array()
                            .and_then(|a| a.last())
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        UiReply::Text(chosen)
                    }
                    UiKind::Editor => UiReply::Text(Some(format!("edited:{}", req.prompt))),
                };
                let _ = req.reply.send(reply);
            }
        });

        // Each guest-facing call blocks until the responder answers (run on a blocking-capable worker).
        let s1 = svc.clone();
        let confirm = tokio::task::spawn_blocking(move || {
            s1.confirm("proceed?", "a large formatted body, distinct from the title", &DialogOptions::default())
        })
        .await
        .expect("confirm task");
        assert!(confirm, "confirm round-trips the scripted `true`");

        let s2 = svc.clone();
        let input = tokio::task::spawn_blocking(move || {
            s2.input("name?", Some("e.g. Ada Lovelace"), &DialogOptions::default())
        })
        .await
        .expect("input task");
        assert_eq!(input.as_deref(), Some("answer:name?"));

        // §2.6: the confirm `message` reached the renderer verbatim, distinct from `prompt` (title).
        // §2.7: the input `placeholder` reached the renderer verbatim (`Some`, not dropped).
        let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(
            seen.iter()
                .find(|s| s.kind == UiKind::Confirm)
                .map(|s| (s.prompt.as_str(), s.message.as_str())),
            Some(("proceed?", "a large formatted body, distinct from the title")),
            "confirm's message body round-trips separately from its title: {seen:?}"
        );
        assert_eq!(
            seen.iter().find(|s| s.kind == UiKind::Input).map(|s| s.placeholder.clone()),
            Some(Some("e.g. Ada Lovelace".to_string())),
            "input's placeholder round-trips instead of being dropped: {seen:?}"
        );

        let s3 = svc.clone();
        let select = tokio::task::spawn_blocking(move || {
            s3.select("pick one", &json!(["x", "y", "z"]), &DialogOptions::default())
        })
        .await
        .expect("select task");
        assert_eq!(
            select.as_deref(),
            Some("z"),
            "select returns the chosen option STRING (Pi types.ts:127, world.wit:259)"
        );

        let s4 = svc.clone();
        let editor = tokio::task::spawn_blocking(move || s4.editor("hello"))
            .await
            .expect("editor task");
        assert_eq!(editor.as_deref(), Some("edited:hello"));
    }

    /// L4 review §2.2: a dialog whose renderer NEVER answers still resolves within `opts.timeout_ms` —
    /// Pi's `createDialogPromise` host-armed `setTimeout(() => resolve(defaultValue), opts.timeout)`
    /// (`rpc-mode.ts:114-119`) ALWAYS settles the awaited Promise regardless of client behavior. The
    /// scripted renderer here receives every request and drops it on the floor (never replies), proving
    /// `ui_roundtrip` races the reply against a REAL timer rather than blocking forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_honors_timeout_ms_and_resolves_to_the_default_on_no_response() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        // The "hung client": receives every request and HOLDS it (keeping `req.reply` open, exactly
        // like the RPC loop's `pending` map keeps a live entry) but never sends a reply — the real
        // shape of a non-responding client, as opposed to a dropped sender (which would resolve the
        // receiver immediately with an error and prove nothing about the timeout race).
        let held: Arc<Mutex<Vec<UiRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let held2 = held.clone();
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                held2.lock().unwrap_or_else(|e| e.into_inner()).push(req);
            }
        });

        let opts = DialogOptions { timeout_ms: Some(50), signal_id: None };

        let s1 = svc.clone();
        let o1 = opts.clone();
        let started = tokio::time::Instant::now();
        let confirm = tokio::task::spawn_blocking(move || s1.confirm("proceed?", "body", &o1))
            .await
            .expect("confirm task");
        let elapsed = started.elapsed();
        assert!(!confirm, "an unanswered confirm resolves to Pi's `false` default, not a hang");
        assert!(
            elapsed < Duration::from_secs(2),
            "confirm must settle close to the 50ms timeout, not hang indefinitely (took {elapsed:?})"
        );

        let s2 = svc.clone();
        let o2 = opts.clone();
        let input = tokio::task::spawn_blocking(move || s2.input("name?", Some("placeholder"), &o2))
            .await
            .expect("input task");
        assert_eq!(input, None, "an unanswered input resolves to Pi's `undefined` default");

        let s3 = svc.clone();
        let o3 = opts;
        let select = tokio::task::spawn_blocking(move || s3.select("pick", &json!(["a", "b"]), &o3))
            .await
            .expect("select task");
        assert_eq!(select, None, "an unanswered select resolves to Pi's `undefined` default");
    }

    /// `timeout_ms: 0` means NO timeout, not an instant one — Pi's `createDialogPromise` only arms
    /// its `setTimeout` `if (opts?.timeout)` (`rpc-mode.ts:114`; falsy-zero ⇒ no timer at all). Proven
    /// here the same way the honors-timeout test proves the OPPOSITE: a REAL (delayed, non-default)
    /// reply arrives well after `Duration::from_millis(0)` would already have elapsed under the old
    /// unconditional `.map(Duration::from_millis)` — if `0` were mistakenly armed as a real timer, the
    /// race would resolve to the default (`false`) near-instantly and NEVER see this later reply.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_timeout_ms_zero_means_no_timeout_not_an_instant_one() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                // A REAL answer, deliberately delayed well past when a (bugged) 0ms timer would have
                // already fired and resolved the call to the default.
                tokio::time::sleep(Duration::from_millis(150)).await;
                let _ = req.reply.send(UiReply::Confirm(true));
            }
        });

        let opts = DialogOptions { timeout_ms: Some(0), signal_id: None };
        let started = tokio::time::Instant::now();
        let confirm = tokio::task::spawn_blocking(move || svc.confirm("proceed?", "body", &opts))
            .await
            .expect("confirm task");
        let elapsed = started.elapsed();

        assert!(
            confirm,
            "timeout_ms:0 must wait for the REAL reply (true), not short-circuit to the `false` \
             default the way a genuine 0ms timeout would"
        );
        assert!(
            elapsed >= Duration::from_millis(120),
            "the call must have actually WAITED for the delayed reply, not resolved near-instantly \
             to the default (took {elapsed:?}, expected >= ~150ms)"
        );
    }

    /// L4 review §2.5 (the shared mechanism half): a reply sent on the SAME one-shot `ui_roundtrip` is
    /// waiting on unblocks it immediately, well before a long `timeout_ms` would otherwise elapse. This
    /// is exactly what the RPC loop's `force_resolve_pending` (`rpc.rs`, wired to `abort`/`abort_retry`)
    /// does to LIVE-dismiss an already-open dialog — no separate cancellation channel is needed because
    /// forcing the existing reply is sufficient, and this proves that path is genuinely live, not merely
    /// a pre-flight snapshot check.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ui_grant_force_resolved_reply_unblocks_before_a_long_timeout_elapses() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = Arc::new(svc_with(provider));

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        svc.set_ui_sink(tx);
        // Simulate a live "abort": as soon as the dialog opens, force-resolve it directly (the same
        // action `force_resolve_pending` takes) instead of waiting for a real user response.
        tokio::spawn(async move {
            if let Some(req) = rx.recv().await {
                let _ = req.reply.send(UiReply::Confirm(false));
            }
        });

        // A 10-second timeout that must NOT be what unblocks this call.
        let opts = DialogOptions { timeout_ms: Some(10_000), signal_id: None };
        let started = tokio::time::Instant::now();
        let confirm = tokio::task::spawn_blocking(move || svc.confirm("proceed?", "body", &opts))
            .await
            .expect("confirm task");
        let elapsed = started.elapsed();
        assert!(!confirm);
        assert!(
            elapsed < Duration::from_secs(2),
            "a force-resolved reply must win the race immediately, not wait out the 10s timeout (took {elapsed:?})"
        );
    }

    /// The DEFAULT (deny-all) backend denies exec with Pi's "not granted" message — the untrusted
    /// analog (an untrusted extension gets `DenyServices`, arch-08 §5.6).
    #[test]
    fn deny_services_refuses_exec() {
        use cyrup_ext::host::{DenyServices, HostServices as _};
        let err = DenyServices
            .exec("echo", &["hi".to_string()], &json!({}), CancelToken::new())
            .expect_err("deny-all backend refuses exec");
        assert!(err.contains("not granted"), "denied with the Pi message: {err}");
    }
}
