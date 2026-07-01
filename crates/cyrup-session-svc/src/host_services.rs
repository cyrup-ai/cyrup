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

use std::sync::{Arc, Mutex};

use cyrup_core::{EntryId, ModelRef};
use cyrup_ext::host::{ControlOp, HostServices};
use cyrup_provider::Provider;
use cyrup_session::manager::SessionManager;
use serde_json::{json, Value};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;

use crate::event::AgentSessionEvent;

/// A command-tier control sink: a loaded extension's `control` import (new/switch/fork/…) is routed
/// here so the runtime can act on it (Pi `createCommandContext`, agent-session.ts:1158). Set by the
/// runtime once it owns the session; until then control ops are reported as unavailable.
pub type ControlSink = Arc<dyn Fn(ControlOp) -> Result<(), String> + Send + Sync>;

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
    snapshot: Mutex<LiveSnapshot>,
    control: Mutex<Option<ControlSink>>,
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
    /// Wire a backend to the session's `provider`. Model/state are seeded via [`Self::update_model`]
    /// and [`Self::update_state`]; the control sink is attached later by the runtime.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            snapshot: Mutex::new(LiveSnapshot::default()),
            control: Mutex::new(None),
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

    #[test]
    fn reflects_live_model_and_models_catalog() {
        let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let svc = LiveHostServices::new(provider.clone());

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
        let svc = LiveHostServices::new(provider);
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
}
