//! `WireProvider` — a reusable `Provider` implementation backing real (and test) wire protocols
//! (arch-01 §3.4/§5). It holds a static model catalog + a `ProviderAuth` + a `CredentialStore` + an
//! `ApiRegistry`, and in `stream()` spawns a task that resolves auth, looks up the `ApiImpl` for
//! `model.api`, and drives `run(...)` into an `EventStream<StreamEvent>`.
//!
//! `stream()` returns IMMEDIATELY (func-01 R-01-009); the network lives behind the returned stream.
//! Auth/registry failures arrive as a terminal `StreamEvent::Error` (R-01-018/045) — never as a
//! thrown error.
//!
//! Concrete providers (anthropic/together/…) are `WireProvider` + a catalog + an api mapping; those
//! land in the next steps.

use crate::api::{ApiRegistry, channel};
use crate::auth::{
    AuthContext, AuthOverrides, CredentialStore, EnvAuthContext, ProviderAuth,
    resolve_provider_auth,
};
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions};
use cyrup_core::{AssistantMessage, EventStream, ProviderId, StopReason};
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

/// Channel buffer for the producer→consumer stream (bounded back-pressure, arch-01 §10).
const STREAM_BUFFER: usize = 64;

/// A provider that speaks one or more wire protocols via the `ApiRegistry` (arch-01 §3.4).
pub struct WireProvider {
    id: ProviderId,
    name: String,
    models: Vec<Model>,
    auth: ProviderAuth,
    store: Arc<dyn CredentialStore>,
    auth_ctx: Arc<dyn AuthContext>,
    registry: Arc<ApiRegistry>,
}

impl WireProvider {
    /// Construct a provider. `auth_ctx` defaults to the real-env [`EnvAuthContext`] via
    /// [`WireProvider::new`]; use [`WireProvider::with_auth_context`] to inject a test context.
    pub fn new(
        id: impl Into<ProviderId>,
        name: impl Into<String>,
        models: Vec<Model>,
        auth: ProviderAuth,
        store: Arc<dyn CredentialStore>,
        registry: Arc<ApiRegistry>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            models,
            auth,
            store,
            auth_ctx: Arc::new(EnvAuthContext),
            registry,
        }
    }

    /// Override the ambient auth context (for tests / custom env sources).
    pub fn with_auth_context(mut self, ctx: Arc<dyn AuthContext>) -> Self {
        self.auth_ctx = ctx;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Provider for WireProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let (sink, rx) = channel(STREAM_BUFFER);

        // Snapshot everything the task needs (the returned stream owns the work).
        let id = self.id.clone();
        let auth = self.auth.clone();
        let store = self.store.clone();
        let auth_ctx = self.auth_ctx.clone();
        let registry = self.registry.clone();
        let model = model.clone();
        let context = context.clone();
        let options = options.clone();
        let cancel = options.cancel.clone().unwrap_or_default();

        tokio::spawn(async move {
            let model_id = model.id.as_str().to_string();

            // 1. Resolve auth (failures → terminal Error, never thrown — R-01-018). The
            // provider-scoped env overlay (Pi `options.env`) participates in env-key resolution /
            // base-url (Pi `applyAuth`, models.ts:240-241).
            let overrides = AuthOverrides {
                api_key: options.api_key.as_deref(),
                env: options.env.as_ref(),
                min_oauth_validity_ms: None,
            };
            let mut auth_result = match resolve_provider_auth(
                &id,
                &auth,
                &model,
                store.as_ref(),
                auth_ctx.as_ref(),
                overrides,
            )
            .await
            {
                Ok(Some(result)) => result,
                Ok(None) => {
                    let msg = AssistantMessage::errored(
                        id.clone(),
                        &model_id,
                        Some(model.api.clone()),
                        StopReason::Error,
                        format!("provider '{id}' is not configured (no credential or env key)"),
                    );
                    sink.send(StreamEvent::terminal(msg)).await;
                    return;
                }
                Err(e) => {
                    let pe = ProviderError::from(e);
                    sink.send(pe.into_error_event(id.clone(), &model_id, Some(model.api.clone())))
                        .await;
                    return;
                }
            };

            // Merge the per-request env overlay into the resolved env so the request path
            // (cache-retention / base-url resolution) sees it, with `options.env` winning per key
            // (Pi `applyAuth`, models.ts:252: `{ ...(resolution.env ?? {}), ...(options.env ?? {}) }`).
            if let Some(req_env) = &options.env {
                let merged = auth_result.env.get_or_insert_with(Default::default);
                for (k, v) in req_env {
                    merged.insert(k.clone(), v.clone());
                }
            }

            // 2. Look up the ApiImpl for model.api (missing → terminal Error — R-01-008).
            let api_impl = match registry.get(&model.api) {
                Some(imp) => imp,
                None => {
                    let pe = ProviderError::NoApiImpl(model.api.clone());
                    sink.send(pe.into_error_event(id.clone(), &model_id, Some(model.api.clone())))
                        .await;
                    return;
                }
            };

            // 3. Drive the wire protocol. `run` emits Start..deltas..terminal into the sink.
            api_impl
                .run(&model, &context, &auth_result, &options, cancel, sink)
                .await;
        });

        Box::pin(ReceiverStream::new(rx))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::api::{ApiImpl, EventSink};
    use crate::auth::types::{AuthContext, AuthResult, Credential};
    use crate::auth::{InMemoryCredentialStore, env_key};
    use crate::model::{Modality, ModelCost};
    use crate::stream::collect_message;
    use crate::stream::sse::{SseRequest, open_sse};
    use cyrup_core::{ApiId, CancelToken, Content, ToolCall, ToolCallId, Usage};
    use futures::StreamExt;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A neutral non-terminal `partial` snapshot for scripted test events.
    fn test_partial() -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            provider: ProviderId::from("p"),
            model: "m1".into(),
            api: ApiId::from("scripted"),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    fn test_model(api: &str) -> Model {
        Model {
            id: "m1".into(),
            name: "M1".into(),
            api: api.into(),
            provider: "p".into(),
            base_url: String::new(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 100,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    struct MapCtx(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    fn store_with_key() -> Arc<dyn CredentialStore> {
        Arc::new(
            InMemoryCredentialStore::new()
                .with_credential(ProviderId::from("p"), Credential::api_key("sk-test")),
        )
    }

    fn provider_with(registry: ApiRegistry, model: &Model) -> WireProvider {
        WireProvider::new(
            "p",
            "Test Provider",
            vec![model.clone()],
            ProviderAuth::with_api_key(env_key(["P_API_KEY"])),
            store_with_key(),
            Arc::new(registry),
        )
        .with_auth_context(Arc::new(MapCtx(BTreeMap::new())))
    }

    // ---- A fake in-process ApiImpl producing a scripted StreamEvent sequence ----

    struct ScriptedApi {
        api: ApiId,
        seen_key: Arc<Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl ApiImpl for ScriptedApi {
        fn api(&self) -> &ApiId {
            &self.api
        }
        async fn run(
            &self,
            _model: &Model,
            _ctx: &Context,
            auth: &AuthResult,
            _opts: &StreamOptions,
            _cancel: CancelToken,
            sink: EventSink,
        ) {
            if let Ok(mut g) = self.seen_key.lock() {
                *g = auth.auth.api_key.clone();
            }
            sink.send(StreamEvent::Start {
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::TextStart {
                content_index: 0,
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::TextEnd {
                content_index: 0,
                content: "hi".into(),
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::ToolCallStart {
                content_index: 1,
                partial: test_partial(),
            })
            .await;
            sink.send(StreamEvent::ToolCallDelta {
                content_index: 1,
                delta: "{\"x\":1}".into(),
                partial: test_partial(),
            })
            .await;
            let tc = ToolCall {
                id: ToolCallId::from("c1"),
                name: "echo".into(),
                arguments: serde_json::json!({"x": 1})
                    .as_object()
                    .cloned()
                    .expect("object"),
                thought_signature: None,
            };
            sink.send(StreamEvent::ToolCallEnd {
                content_index: 1,
                tool_call: tc,
                partial: test_partial(),
            })
            .await;
            let msg = AssistantMessage {
                content: vec![
                    Content::text("hi"),
                    Content::ToolCall(ToolCall {
                        id: ToolCallId::from("c1"),
                        name: "echo".into(),
                        arguments: serde_json::json!({"x": 1})
                            .as_object()
                            .cloned()
                            .expect("object"),
                        thought_signature: None,
                    }),
                ],
                provider: ProviderId::from("p"),
                model: "m1".into(),
                api: ApiId::from("scripted"),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 0,
            };
            sink.send(StreamEvent::terminal(msg)).await;
        }
    }

    #[tokio::test]
    async fn wire_provider_drives_fake_api_end_to_end() {
        let model = test_model("scripted");
        let seen_key = Arc::new(Mutex::new(None));
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(ScriptedApi {
            api: ApiId::from("scripted"),
            seen_key: seen_key.clone(),
        }));
        let provider = provider_with(registry, &model);

        let mut stream = provider.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }

        assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::TextDelta {
                content_index: 0,
                ..
            }
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolCallEnd {
                content_index: 1,
                ..
            }
        )));
        // Stored credential flowed through to the ApiImpl (auth resolution wired end-to-end).
        assert_eq!(seen_key.lock().unwrap().as_deref(), Some("sk-test"));
    }

    #[tokio::test]
    async fn missing_api_impl_yields_error_terminal() {
        let model = test_model("no-such-api");
        let registry = ApiRegistry::new(); // nothing registered
        let provider = provider_with(registry, &model);
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("no API implementation"));
    }

    #[tokio::test]
    async fn unconfigured_provider_yields_error_terminal() {
        let model = test_model("scripted");
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(ScriptedApi {
            api: ApiId::from("scripted"),
            seen_key: Arc::new(Mutex::new(None)),
        }));
        // No stored credential, empty env → unconfigured.
        let provider = WireProvider::new(
            "p",
            "P",
            vec![model.clone()],
            ProviderAuth::with_api_key(env_key(["P_API_KEY"])),
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(registry),
        )
        .with_auth_context(Arc::new(MapCtx(BTreeMap::new())));
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    // ---- A TEST ApiImpl that drives the real SSE transport against a local mock server ----

    struct SseEchoApi {
        api: ApiId,
    }

    #[async_trait::async_trait]
    impl ApiImpl for SseEchoApi {
        fn api(&self) -> &ApiId {
            &self.api
        }
        async fn run(
            &self,
            model: &Model,
            _ctx: &Context,
            _auth: &AuthResult,
            _opts: &StreamOptions,
            cancel: CancelToken,
            sink: EventSink,
        ) {
            let provider = model.provider.clone();
            let model_id = model.id.as_str().to_string();
            let url = model.base_url.clone();

            let client = match crate::stream::sse::build_client() {
                Ok(c) => c,
                Err(e) => {
                    sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                        .await;
                    return;
                }
            };
            let req = SseRequest::post_json(url, serde_json::json!({"stream": true}));
            let mut frames = match open_sse(&client, req, cancel, None, None).await {
                Ok(s) => s,
                Err(e) => {
                    // transport / non-2xx / abort-during-connect → terminal Error (R-01-018/045)
                    sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                        .await;
                    return;
                }
            };

            sink.send(StreamEvent::Start {
                partial: test_partial(),
            })
            .await;
            let mut idx = 0usize;
            let mut text = String::new();
            while let Some(frame) = frames.next().await {
                match frame {
                    Ok(f) if f.data == "[DONE]" => break,
                    Ok(f) => {
                        sink.send(StreamEvent::TextStart {
                            content_index: idx,
                            partial: test_partial(),
                        })
                        .await;
                        sink.send(StreamEvent::TextDelta {
                            content_index: idx,
                            delta: f.data.clone(),
                            partial: test_partial(),
                        })
                        .await;
                        sink.send(StreamEvent::TextEnd {
                            content_index: idx,
                            content: f.data.clone(),
                            partial: test_partial(),
                        })
                        .await;
                        text.push_str(&f.data);
                        idx += 1;
                    }
                    Err(e) => {
                        sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                            .await;
                        return;
                    }
                }
            }
            let msg = AssistantMessage {
                content: vec![Content::text(text)],
                provider,
                model: model_id,
                api: self.api.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sink.send(StreamEvent::terminal(msg)).await;
        }
    }

    /// Spawn a one-shot mock HTTP server that writes `raw_response` then closes. Returns its URL.
    async fn spawn_mock(raw_response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                // Drain the request (best-effort) then write the canned response.
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(raw_response).await;
                let _ = sock.flush().await;
            }
        });
        format!("http://{addr}/v1/stream")
    }

    fn sse_provider(base_url: String) -> (WireProvider, Model) {
        let mut model = test_model("sse-echo");
        model.base_url = base_url;
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(SseEchoApi {
            api: ApiId::from("sse-echo"),
        }));
        (provider_with(registry, &model), model)
    }

    #[tokio::test]
    async fn sse_frames_decode_to_stream_events_via_test_api() {
        let body = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: alpha\n\ndata: beta\n\ndata: [DONE]\n\n";
        let url = spawn_mock(body).await;
        let (provider, model) = sse_provider(url);
        let mut stream = provider.stream(&model, &Context::default(), &StreamOptions::default());
        let mut deltas = Vec::new();
        let mut terminal = None;
        while let Some(ev) = stream.next().await {
            match ev {
                StreamEvent::TextDelta { delta, .. } => deltas.push(delta),
                StreamEvent::Done { message, .. } => terminal = Some(message),
                _ => {}
            }
        }
        assert_eq!(deltas, vec!["alpha".to_string(), "beta".to_string()]);
        let msg = terminal.expect("done terminal");
        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert_eq!(msg.content, vec![Content::text("alphabeta")]);
    }

    #[tokio::test]
    async fn http_non_2xx_yields_error_terminal() {
        let body = b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: 9\r\n\r\noh no!!!!";
        let url = spawn_mock(body).await;
        let (provider, model) = sse_provider(url);
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("http 500"));
    }

    #[tokio::test]
    async fn transport_error_yields_error_terminal() {
        // Bind then immediately drop the listener to get a refused-connection address.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let (provider, model) = sse_provider(format!("http://{addr}/v1/stream"));
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(
            msg.error_message
                .unwrap()
                .chars()
                .take(9)
                .collect::<String>(),
            "transport".to_string()
        );
    }

    #[tokio::test]
    async fn cancellation_yields_aborted_terminal() {
        // A server that accepts but never sends a response body → the stream blocks until cancel.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((sock, _)) = listener.accept().await {
                // Hold the connection open without responding.
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                drop(sock);
            }
        });

        let cancel = CancelToken::new();
        let mut model = test_model("sse-echo");
        model.base_url = format!("http://{addr}/v1/stream");
        let registry = ApiRegistry::new();
        registry.register_impl(Arc::new(SseEchoApi {
            api: ApiId::from("sse-echo"),
        }));
        let provider = provider_with(registry, &model);

        let opts = StreamOptions {
            cancel: Some(cancel.clone()),
            ..Default::default()
        };
        let stream = provider.stream(&model, &Context::default(), &opts);

        // Cancel shortly after starting.
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel.cancel();
        });

        let msg = collect_message(stream).await;
        canceller.await.unwrap();
        assert_eq!(msg.stop_reason, StopReason::Aborted);
        assert_eq!(msg.error_message.as_deref(), Some("aborted"));
    }
}
