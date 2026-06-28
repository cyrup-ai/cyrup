//! The internal `ApiImpl` trait + lazy `ApiRegistry` (arch-01 §3.4 / func-01 R-01-007/008/010).
//!
//! One `ApiImpl` per wire protocol. Multiple providers share one impl (R-01-007); a provider maps
//! `model.api → ApiImpl` per request (mixed-API, R-01-008). The registry constructs impls lazily on
//! first use of their api id (R-01-010/066).

use crate::auth::AuthResult;
use crate::context::Context;
use crate::model::Model;
use crate::stream::{StreamEvent, StreamOptions};
use cyrup_core::{ApiId, CancelToken};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;

/// OpenAI-completions compatibility matrix (shared by every OpenAI-compatible provider).
pub mod compat;

/// Concrete wire-protocol implementations (one `ApiImpl` per submodule).
pub mod openai_completions;

/// Producer side of the provider stream channel. `ApiImpl::run` pushes the EXISTING
/// `cyrup_provider::StreamEvent` here; the receiver is wrapped as the returned `EventStream`.
#[derive(Clone)]
pub struct EventSink {
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
}

impl EventSink {
    /// Push one event. Returns `false` if the consumer has dropped the stream (the producer should
    /// stop). Never panics.
    pub async fn send(&self, event: StreamEvent) -> bool {
        self.tx.send(event).await.is_ok()
    }

    /// `true` once the consumer has dropped the receiver.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Create a bounded producer/consumer channel (bounded for back-pressure, arch-01 §10).
pub fn channel(buffer: usize) -> (EventSink, tokio::sync::mpsc::Receiver<StreamEvent>) {
    let (tx, rx) = tokio::sync::mpsc::channel(buffer.max(1));
    (EventSink { tx }, rx)
}

/// One wire protocol (arch-01 §3.4). Builds the payload, opens the SSE transport, assembles events,
/// and pushes them into `sink`. Failures are pushed as a terminal `StreamEvent::Error` (R-01-018) —
/// `run` returns `()` and never propagates an error to the caller.
#[async_trait::async_trait]
pub trait ApiImpl: Send + Sync {
    fn api(&self) -> &ApiId;

    async fn run(
        &self,
        model: &Model,
        ctx: &Context,
        auth: &AuthResult,
        opts: &StreamOptions,
        cancel: CancelToken,
        sink: EventSink,
    );
}

/// Lazily-constructed factory for an `ApiImpl` (R-01-010).
pub type ApiFactory = fn() -> Arc<dyn ApiImpl>;

/// Maps `ApiId → Arc<dyn ApiImpl>` with lazy get-or-init (arch-01 §3.4).
#[derive(Default)]
pub struct ApiRegistry {
    factories: HashMap<ApiId, ApiFactory>,
    live: DashMap<ApiId, Arc<dyn ApiImpl>>,
}

impl ApiRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a lazy factory; the impl is constructed on first `get` of this api id (R-01-010).
    pub fn register(&mut self, api: ApiId, factory: ApiFactory) {
        self.factories.insert(api, factory);
    }

    /// Register an already-constructed impl (e.g. a shared client, or a test impl).
    pub fn register_impl(&self, imp: Arc<dyn ApiImpl>) {
        self.live.insert(imp.api().clone(), imp);
    }

    /// Get-or-init the impl for `api`. Returns `None` when no impl/factory is registered — the
    /// caller then emits a terminal `StreamEvent::Error` (R-01-008/017).
    pub fn get(&self, api: &ApiId) -> Option<Arc<dyn ApiImpl>> {
        if let Some(found) = self.live.get(api) {
            return Some(found.clone());
        }
        let factory = self.factories.get(api)?;
        let imp = factory();
        self.live.insert(api.clone(), imp.clone());
        Some(imp)
    }

    /// `true` if an impl is available (live or via a factory) for `api`.
    pub fn contains(&self, api: &ApiId) -> bool {
        self.live.contains_key(api) || self.factories.contains_key(api)
    }
}

/// A registry pre-seeded with every built-in wire-protocol factory (lazy — nothing is constructed
/// until a request uses an api id). Concrete providers share one such registry (R-01-007/010).
pub fn builtin_registry() -> ApiRegistry {
    let mut reg = ApiRegistry::new();
    register_builtins(&mut reg);
    reg
}

/// Register the built-in wire-protocol factories into `reg`.
pub fn register_builtins(reg: &mut ApiRegistry) {
    reg.register(ApiId::from(crate::known_api::OPENAI_COMPLETIONS), openai_completions::factory);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);

    struct NoopApi(ApiId);
    #[async_trait::async_trait]
    impl ApiImpl for NoopApi {
        fn api(&self) -> &ApiId {
            &self.0
        }
        async fn run(
            &self,
            _model: &Model,
            _ctx: &Context,
            _auth: &AuthResult,
            _opts: &StreamOptions,
            _cancel: CancelToken,
            sink: EventSink,
        ) {
            sink.send(StreamEvent::Start).await;
        }
    }

    fn make() -> Arc<dyn ApiImpl> {
        FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);
        Arc::new(NoopApi(ApiId::from("lazy")))
    }

    #[test]
    fn lazy_factory_runs_once() {
        let mut reg = ApiRegistry::new();
        reg.register(ApiId::from("lazy"), make);
        assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 0); // not yet constructed
        let _ = reg.get(&ApiId::from("lazy")).unwrap();
        let _ = reg.get(&ApiId::from("lazy")).unwrap();
        assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 1); // get-or-init: built once
    }

    #[test]
    fn missing_api_returns_none() {
        let reg = ApiRegistry::new();
        assert!(reg.get(&ApiId::from("nope")).is_none());
        assert!(!reg.contains(&ApiId::from("nope")));
    }
}
