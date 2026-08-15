//! The internal `ApiImpl` trait + lazy `ApiRegistry` (arch-01 §3.4 / func-01 R-01-007/008/010).
//!
//! One `ApiImpl` per wire protocol. Multiple providers share one impl (R-01-007); a provider maps
//! `model.api → ApiImpl` per request (mixed-API, R-01-008). The registry constructs impls lazily on
//! first use of their api id (R-01-010/066).
//!
//! ## `[CYRUP-DELTA, mechanism]` — a fn-pointer factory table where pi uses dynamic `import()`
//!
//! PROV-067. **The id set is identical and nothing observable differs; the deferral MECHANISM is
//! substituted, and this note is the sign-off the port-mechanism-fidelity rule requires.**
//!
//! pi declares its wire apis as `KnownApi` (`packages/ai/src/types.ts:16-26` @v0.83.0, 10 ids) plus
//! `KnownImagesApi` (`:30`, `openrouter-images`), and makes each one lazy with a per-module
//! `api/<id>.lazy.ts` wrapper: `export const anthropicMessagesApi = (): ProviderStreams =>
//! lazyApi(() => import("./anthropic-messages.ts"))`. `lazyApi` (`api/lazy.ts:66-75` @v0.83.0)
//! returns a `ProviderStreams` **immediately** and defers the `import()` — the actual module load —
//! to the first `stream`/`streamSimple` call on it, relying on the JS host's import cache to
//! deduplicate. cyrup registers the same 10 ids ([`crate::known_api`], registered in
//! [`register_builtins`]) plus `openrouter-images` in [`crate::images`], and defers with
//! [`ApiFactory`] + [`ApiRegistry::get`]'s get-or-init.
//!
//! **Why the substitution.** Rust has no dynamic `import()`: a wire impl is a statically linked
//! type, so there is no module-load event to defer and no import cache to share. The only thing
//! left to defer is *construction of the impl value*, which is what the factory table does. The
//! deferral POINT is the same in observable terms — pi builds nothing of an api until a request
//! streams over it; cyrup constructs nothing of an api until a request resolves it out of the
//! registry — and both are once-per-process. Nothing in either codebase depends on module-load
//! timing (`lazy.ts` carries no top-level side effect beyond the import itself), which is what
//! makes the two equivalent rather than merely similar.
//!
//! Pinned by [`tests::prov067_registry_constructs_nothing_until_the_first_get`], so "same
//! observable laziness" stays a property rather than a claim in a comment.

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
pub mod anthropic_messages;
pub mod azure_openai_responses;
pub mod bedrock_converse_stream;
pub mod github_copilot_headers;
pub mod google_generative_ai;
pub mod google_vertex;
pub mod mistral_conversations;
pub mod openai_codex_responses;
pub mod openai_completions;
pub mod openai_responses;
pub mod pi_messages;

/// Cross-converter regression suite: a truncated stream must never be reported as a completed turn
/// (PROV-010 / AGENT-014 / DRIFT-012). Lives beside the decoders so it can drive all five directly.
#[cfg(test)]
mod truncation_parity;

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
    reg.register(
        ApiId::from(crate::known_api::OPENAI_COMPLETIONS),
        openai_completions::factory,
    );
    reg.register(
        ApiId::from(crate::known_api::ANTHROPIC_MESSAGES),
        anthropic_messages::factory,
    );
    reg.register(
        ApiId::from(crate::known_api::OPENAI_RESPONSES),
        openai_responses::factory,
    );
    reg.register(
        ApiId::from(crate::known_api::AZURE_OPENAI_RESPONSES),
        azure_openai_responses::factory,
    );
    reg.register(
        ApiId::from(crate::known_api::GOOGLE_GENERATIVE_AI),
        google_generative_ai::factory,
    );
    reg.register(
        ApiId::from(crate::known_api::GOOGLE_VERTEX),
        google_vertex::factory,
    );
    reg.register(ApiId::from(crate::known_api::PI_MESSAGES), pi_messages::factory);
    reg.register(
        ApiId::from(crate::known_api::BEDROCK_CONVERSE_STREAM),
        bedrock_converse_stream::factory,
    );
    reg.register(
        ApiId::from(crate::known_api::OPENAI_CODEX_RESPONSES),
        openai_codex_responses::factory,
    );
    reg.register(
        ApiId::from(crate::known_api::MISTRAL_CONVERSATIONS),
        mistral_conversations::factory,
    );
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
            let partial = cyrup_core::AssistantMessage::errored(
                _model.provider.clone(),
                _model.id.as_str(),
                Some(_model.api.clone()),
                cyrup_core::StopReason::Stop,
                "",
            );
            sink.send(StreamEvent::Start { partial }).await;
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

    /// PROV-067 — pins the `[CYRUP-DELTA, mechanism]` note in this module's header.
    ///
    /// The substituted mechanism (fn-pointer factory table vs pi's per-module dynamic `import()`
    /// in `api/*.lazy.ts` @v0.83.0) is only defensible while two things hold: the id set is
    /// identical, and construction really is deferred. `lazy_factory_runs_once` above proves the
    /// second for a hand-built registry; this proves BOTH for the registry that actually ships.
    ///
    /// **This test does not go red before any change in this pass** — PROV-067 proposed no code
    /// change and the fix is the header note. It goes red if a factory is replaced by an eager
    /// `register_impl` (construction at registry-build time, which pi's `lazyApi` never does) or if
    /// the registered id set drifts from pi's `KnownApi`.
    #[test]
    fn prov067_registry_constructs_nothing_until_the_first_get() {
        let reg = builtin_registry();

        // pi `KnownApi` (`packages/ai/src/types.ts:16-26` @v0.83.0) — all ten, no more, no fewer.
        // `openrouter-images` is `KnownImagesApi` (`:30`) and lives in `crate::images`, not here.
        let mut registered: Vec<String> = reg.factories.keys().map(|a| a.to_string()).collect();
        registered.sort();
        let mut expected = vec![
            crate::known_api::ANTHROPIC_MESSAGES,
            crate::known_api::AZURE_OPENAI_RESPONSES,
            crate::known_api::BEDROCK_CONVERSE_STREAM,
            crate::known_api::GOOGLE_GENERATIVE_AI,
            crate::known_api::GOOGLE_VERTEX,
            crate::known_api::MISTRAL_CONVERSATIONS,
            crate::known_api::OPENAI_CODEX_RESPONSES,
            crate::known_api::OPENAI_COMPLETIONS,
            crate::known_api::OPENAI_RESPONSES,
            crate::known_api::PI_MESSAGES,
        ];
        expected.sort_unstable();
        assert_eq!(registered, expected, "registered wire-api ids drifted from pi's KnownApi");

        // Nothing is constructed by building the registry — pi's `lazyApi` likewise runs no
        // `import()` until a stream call.
        assert_eq!(reg.live.len(), 0, "builtin_registry() constructed an impl eagerly");

        // `contains` answers from the factory table, so a capability probe stays free.
        for id in &expected {
            assert!(reg.contains(&ApiId::from(*id)), "{id} not registered");
        }
        assert_eq!(reg.live.len(), 0, "contains() constructed an impl");

        // The first `get` constructs exactly one impl, and only the one asked for.
        let anthropic = ApiId::from(crate::known_api::ANTHROPIC_MESSAGES);
        let imp = reg.get(&anthropic).expect("anthropic-messages registered");
        assert_eq!(imp.api(), &anthropic);
        assert_eq!(reg.live.len(), 1, "a single get constructed more than its own impl");
    }

    #[test]
    fn missing_api_returns_none() {
        let reg = ApiRegistry::new();
        assert!(reg.get(&ApiId::from("nope")).is_none());
        assert!(!reg.contains(&ApiId::from("nope")));
    }
}
