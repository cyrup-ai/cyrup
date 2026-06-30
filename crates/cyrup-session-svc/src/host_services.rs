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

use cyrup_core::ModelRef;
use cyrup_ext::host::{ControlOp, HostServices};
use cyrup_provider::Provider;
use serde_json::{json, Value};

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
}

impl LiveHostServices {
    /// Wire a backend to the session's `provider`. Model/state are seeded via [`Self::update_model`]
    /// and [`Self::update_state`]; the control sink is attached later by the runtime.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self { provider, snapshot: Mutex::new(LiveSnapshot::default()), control: Mutex::new(None) }
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
