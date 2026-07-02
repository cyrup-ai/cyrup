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
use cyrup_ext::host::{ControlOp, DialogOptions, ExecOutput, HostServices};
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
    /// when there is no sink (headless: the ui method then yields its deny default WITHOUT blocking) or
    /// when the renderer dropped the reply (cancelled / shut down). Uses the SAME `block_in_place` +
    /// `block_on` pattern the `exec` grant uses ([`Self::exec`]); requires a multi-threaded runtime,
    /// which interactive/rpc guarantee (`#[tokio::main(flavor = "multi_thread")]`, main.rs:40).
    fn ui_roundtrip(
        &self,
        kind: UiKind,
        prompt: &str,
        options: Value,
        opts: &DialogOptions,
    ) -> Option<UiReply> {
        let sink = Self::lock(&self.ui_sink).clone()?;
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = UiRequest { kind, prompt: prompt.to_string(), options, opts: opts.clone(), reply: reply_tx };
        if sink.send(request).is_err() {
            // The renderer (TUI loop / RPC loop) is gone — degrade to the deny default, never a panic.
            return None;
        }
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(reply_rx)).ok()
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

    fn confirm(&self, prompt: &str, opts: &DialogOptions) -> bool {
        match self.ui_roundtrip(UiKind::Confirm, prompt, Value::Null, opts) {
            Some(UiReply::Confirm(b)) => b,
            _ => false,
        }
    }

    fn input(&self, prompt: &str, opts: &DialogOptions) -> Option<String> {
        match self.ui_roundtrip(UiKind::Input, prompt, Value::Null, opts) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn select(&self, prompt: &str, options: &Value, opts: &DialogOptions) -> Option<String> {
        match self.ui_roundtrip(UiKind::Select, prompt, options.clone(), opts) {
            Some(UiReply::Text(t)) => t,
            _ => None,
        }
    }

    fn editor(&self, initial: &str) -> Option<String> {
        // The WIT `editor(initial) -> option<string>` carries no options bag (world.wit:261); use the
        // empty default so the roundtrip signature stays uniform.
        match self.ui_roundtrip(UiKind::Editor, initial, Value::Null, &DialogOptions::default()) {
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
        // (exec.ts:34-46): shell:false argv, `cwd ?? sessionCwd`, `env` overrides, and a `timeoutMs`
        // that SIGTERM/SIGKILLs (killed=true) on expiry.
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
        let env: Vec<(String, String)> = opts
            .get("env")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let spec = ArgvSpec { program: cmd.to_string(), args: args.to_vec(), cwd, env };
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
        // Map the process status onto Pi's `{code, killed}` (exec.ts:52-63,97): a natural exit keeps
        // its code; a host-driven kill (cancel/timeout) is `killed=true` with `code 0` (Pi's SIGTERM
        // ⇒ exit null ⇒ `code ?? 0`); an external signal we did NOT send is `killed=false, code 0`.
        let (code, killed) = match out.status {
            ExitStatus::Exited(n) => (n, false),
            ExitStatus::Signaled => (0, false),
            ExitStatus::Killed | ExitStatus::TimedOut => (0, true),
        };
        Ok(ExecOutput {
            code,
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            killed,
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

        // 4) `env` overrides threaded to the child (Pi `ExecOptions.env`).
        let out = svc
            .exec(
                "printenv",
                &["CYRUP_EXEC_TEST".to_string()],
                &json!({ "env": { "CYRUP_EXEC_TEST": "grant" } }),
                CancelToken::new(),
            )
            .expect("printenv runs");
        assert_eq!(out.stdout, "grant\n");

        // 5) `timeoutMs` ⇒ the host kills the process (SIGTERM/SIGKILL the group) and reports
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
    }

    /// With NO ui sink attached (headless print/json: `set_ui_sink` is never called), the ui grant
    /// falls through to the trait deny defaults WITHOUT blocking — byte-for-byte Pi `noOpUIContext`
    /// (confirm=false, input/select/editor=None). A single-thread runtime proves it never touches
    /// `block_in_place` (which would panic here) on the headless path.
    #[test]
    fn headless_ui_returns_deny_defaults_without_a_sink() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = svc_with(provider);
        assert!(!svc.confirm("ok?", &DialogOptions::default()));
        assert_eq!(svc.input("name?", &DialogOptions::default()), None);
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

        // The scripted renderer: reply to each request by kind (like a user picking in the selector).
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
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
        let confirm = tokio::task::spawn_blocking(move || s1.confirm("proceed?", &DialogOptions::default()))
            .await
            .expect("confirm task");
        assert!(confirm, "confirm round-trips the scripted `true`");

        let s2 = svc.clone();
        let input = tokio::task::spawn_blocking(move || s2.input("name?", &DialogOptions::default()))
            .await
            .expect("input task");
        assert_eq!(input.as_deref(), Some("answer:name?"));

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
