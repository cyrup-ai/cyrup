//! The `bedrock-converse-stream` wire protocol (arch-01 §3.4 / func-01 R-01-*).
//!
//! 1:1 behavioural port of pi's `packages/ai/src/api/bedrock-converse-stream.ts` (v0.83.0) — the
//! Amazon Bedrock `ConverseStream` API. Covers the whole observable surface of that file: region /
//! endpoint / credential precedence, bearer-token auth, the proxy hook, caller-header injection,
//! the ConverseStream payload (messages, system, `inferenceConfig`, `toolConfig`,
//! `additionalModelRequestFields`, `requestMetadata`), prompt-cache points, extended-thinking
//! (adaptive and budget-based), the streaming event assembly, the stop-reason table and the
//! `formatBedrockError` display strings.
//!
//! # Mechanism divergence: no AWS SDK
//!
//! Upstream drives `@aws-sdk/client-bedrock-runtime`, which owns three things behaviourally
//! invisible to the caller: the REST binding (`POST {endpoint}/model/{modelId}/converse-stream`),
//! SigV4 request signing, and the `application/vnd.amazon.eventstream` binary framing of the
//! response. `cyrup-provider`'s manifest carries no AWS dependency — the workspace avoids adding a
//! dependency where a self-contained routine will do (see the justification comments in the root
//! `Cargo.toml`) — so all three are implemented here directly on `reqwest`, exactly as
//! `anthropic-messages` speaks Anthropic's HTTP+SSE protocol without the Anthropic SDK.
//!
//! What the SDK does for upstream and this module does inline:
//!
//! | SDK concern | here |
//! |---|---|
//! | `BedrockRuntimeClientConfig` `{region, endpoint, credentials, profile, token}` | `resolve_client_config` |
//! | default credential chain (env → shared config/credentials file) | `configured_bedrock_credentials` + `shared_profile_credentials` |
//! | SigV4 `build`-step signing | `sign_sigv4` |
//! | `ConverseStreamCommand` REST binding | `converse_stream_url` + `build_params` |
//! | `vnd.amazon.eventstream` decoding | `EventStreamDecoder` |
//! | `middlewareStack.add(..., {step:"build"})` header injection | `apply_custom_headers` |
//!
//! Smithy's `build` step runs after serialisation but **before** signing, which is why upstream's
//! comment says injected headers are covered by the signature; the same holds here because
//! `apply_custom_headers` mutates the header map that `sign_sigv4` then signs.
//!
//! # Scope notes
//!
//! * **`sanitizeSurrogates` is a no-op here.** A Rust `String` cannot hold a lone surrogate, so
//!   upstream's `sanitize-unicode.ts` pass (which strips them) has nothing to remove. cyrup's
//!   shared [`sanitize_surrogates`](crate::api::compat::sanitize_surrogates) is likewise the
//!   identity, and it is still called at each of upstream's call sites so the *shape* of the port
//!   stays diffable.
//! * **`resolveJsonSchemaStrictSampling` is unreachable.** Upstream reads
//!   `tool.constrainedSampling`; cyrup's [`ToolDef`](crate::context::ToolDef) has no such field, so
//!   the helper's `if (!config …) return undefined` arm is the only reachable one and no `strict`
//!   key can ever be emitted. `model.compat.supports_strict_mode` is therefore not consulted. This
//!   is a gap in `ToolDef`, not in this converter; closing it means adding the field to
//!   `context.rs` (out of scope for this file).
//! * Upstream's non-Node branch (`typeof process === "undefined"`) is unreachable in Rust; only the
//!   Node/Bun branch is ported.

mod blocks;
mod capabilities;
mod config;
mod convert;
mod driver;
mod env;
mod errors;
mod events;
mod failure;
mod framing;
mod headers;
mod options;
mod params;
mod sigv4;
mod url;

#[cfg(test)]
mod tests;

pub use options::{BedrockOptions, BedrockThinkingDisplay, BedrockToolChoice};

use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::context::Context;
use crate::model::Model;
use crate::stream::{StreamEvent, StreamOptions};
use cyrup_core::{ApiId, CancelToken, StopReason};
use driver::run_inner;
use failure::append_bedrock_failure_diagnostic;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::BEDROCK_CONVERSE_STREAM;

/// The `ApiImpl` for `"bedrock-converse-stream"`.
pub struct BedrockConverseStreamApi {
    api: ApiId,
}

impl Default for BedrockConverseStreamApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl BedrockConverseStreamApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(BedrockConverseStreamApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for BedrockConverseStreamApi {
    fn api(&self) -> &ApiId {
        &self.api
    }

    async fn run(
        &self,
        model: &Model,
        ctx: &Context,
        auth: &AuthResult,
        opts: &StreamOptions,
        cancel: CancelToken,
        sink: EventSink,
    ) {
        let api = self.api.clone();

        // The whole body of upstream's `(async () => { … })()` sits inside one `try` whose catch
        // sets `stopReason = options.signal?.aborted ? "aborted" : "error"`, folds the composed
        // `formatBedrockError` message in, and pushes ONE terminal `error` event (`:304-314`).
        // `run_inner` is that try block; this arm is that catch.
        if let Err(failure) = run_inner(model, ctx, auth, opts, &cancel, &sink, &api).await {
            let mut message = failure.partial;
            message.stop_reason = if cancel.is_cancelled() {
                StopReason::Aborted
            } else {
                failure.stop_reason
            };
            message.error_message = Some(failure.message);
            // pi `:318-320`: structured diagnostics ride along ONLY on the `error` terminal — an
            // aborted turn is not a provider failure and gets none.
            if message.stop_reason == StopReason::Error {
                append_bedrock_failure_diagnostic(
                    &mut message,
                    failure.status,
                    failure.error_code.as_deref(),
                    failure.request_id.as_deref(),
                );
            }
            sink.send(StreamEvent::terminal(message)).await;
        }
    }
}
