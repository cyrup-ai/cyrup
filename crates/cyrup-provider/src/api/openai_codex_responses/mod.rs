//! The `openai-codex-responses` wire protocol (arch-01 §3.4) — the ChatGPT-subscription Codex
//! backend (`POST {base}/codex/responses`).
//!
//! Port of pi v0.83.0 `packages/ai/src/api/openai-codex-responses.ts` (1636 lines). Codex speaks the
//! *same* Responses SSE wire format as [`openai-responses`](crate::api::openai_responses) — upstream
//! literally hands its event iterator to the shared `processResponsesStream`
//! (`openai-codex-responses.ts:664-669`) — so this module ports only what Codex adds on top:
//!
//! | upstream | ported here |
//! |---|---|
//! | `resolveCodexUrl` (`:637-643`) | [`resolve_codex_url`](url::resolve_codex_url) |
//! | `extractAccountId` (`:1564-1575`) | [`extract_account_id`](headers::extract_account_id) |
//! | `buildBaseCodexHeaders`/`buildSSEHeaders` (`:1577-1617`) | [`build_sse_headers`](headers::build_sse_headers) |
//! | `buildRequestBody` (`:529-596`) | [`build_request_body`](request::build_request_body) |
//! | `mapCodexEvents`/`normalizeCodexStatus` (`:721-757`) | [`map_codex_event`](events::map_codex_event) + [`map_codex_frames`](events::map_codex_frames) |
//! | `resolveCodexServiceTier` (`:627-635`) | [`resolve_codex_service_tier`](events::resolve_codex_service_tier) |
//! | `isRetryableError`/`isTerminalRateLimitError` (`:130-144`) | [`is_retryable_error`](retry::is_retryable_error) |
//! | `getRetryAfterDelayMs`/`validateRetryDelayMs` (`:146-183`) | [`get_retry_after_delay_ms`](retry::get_retry_after_delay_ms) |
//! | `parseErrorResponse` (`:1533-1558`) | [`parse_error_response`](retry::parse_error_response) |
//! | `stream`'s SSE attempt ladder (`:390-488`) | [`CodexResponsesApi::run`] |
//!
//! Everything below `processResponsesStream` — slot creation, reasoning/text/tool decoding, usage,
//! `mapStopReason`, service-tier pricing — is reached by delegating to
//! [`openai_responses::decode_stream`](crate::api::openai_responses), exactly as upstream shares
//! `openai-responses-shared.ts`. `getServiceTierCostMultiplier` (`:598-610`) is byte-identical to
//! `openai-responses.ts:281-293`, which that decoder already implements, so the codex-specific
//! `resolveCodexServiceTier` is applied by rewriting `response.service_tier` on the terminal event
//! before the shared decoder reads it (see [`map_codex_frames`](events::map_codex_frames)) rather than by duplicating the
//! pricing table.
//!
//! # Mechanism deltas (the language/dependency forces them; behaviour is unchanged)
//!
//! * **Request compression.** Upstream zstd-compresses the SSE body when `node:zlib` exposes
//!   `zstdCompressSync`, and *falls back to the uncompressed JSON when it does not*
//!   (`compressRequestBodyZstd`, `:225-238`, "Callers fall back to sending the uncompressed JSON
//!   when this returns null"). Cyrup's SSE transport carries a `serde_json::Value` body, not raw
//!   bytes, so this port always takes upstream's documented no-compression branch: the request is
//!   the same JSON and `content-encoding: zstd` is correspondingly not set.
//! * **WebSocket transport.** Upstream prefers a WebSocket for `transport != "sse"` and, when the
//!   runtime exposes no WebSocket constructor, throws
//!   `"WebSocket transport is not available in this runtime"` (`connectWebSocket`, `:1043-1045`),
//!   which is not a `CodexApiError`, so `stream` records the failure and **breaks to the SSE path**
//!   (`:358-377`). This port has no WebSocket client (the workspace has no ws dependency and adding
//!   one is outside this module), so every transport resolves to SSE — upstream's own
//!   no-WebSocket-runtime behaviour. The WS-only bookkeeping (connection cache, delta continuation
//!   via `previous_response_id`, `OpenAICodexWebSocketDebugStats`, the `provider_transport_failure`
//!   diagnostic) is therefore not reachable and not ported.
//! * **`extractAccountId` base64.** Upstream calls `atob`, i.e. the WHATWG *forgiving-base64*
//!   decode: the standard alphabet with optional padding, which rejects the URL-safe `-`/`_`.
//!   [`ATOB`] is configured to match that exactly rather than using a URL-safe engine.
//!
//! Wire JSON uses OpenAI's own field names (snake_case), NOT the cyrup camelCase convention.

mod driver;
mod events;
mod headers;
mod options;
mod request;
mod retry;
mod terminals;
mod url;

#[cfg(test)]
mod tests;

pub use options::{CodexReasoningSummary, CodexToolChoice, OpenAiCodexResponsesOptions};

use crate::api::ApiImpl;
use crate::error::ProviderError;
use crate::stream::sse::SseFrame;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use cyrup_core::ApiId;
use futures::Stream;
use std::sync::Arc;

/// The wire-protocol id this impl serves (pi `KnownApi`, `ai/src/types.ts:16-26`).
const API_ID: &str = "openai-codex-responses";

/// pi `DEFAULT_CODEX_BASE_URL` (`openai-codex-responses.ts:59`).
const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// pi `JWT_CLAIM_PATH` (`:60`) — the namespaced claim carrying the ChatGPT account id.
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

/// pi `DEFAULT_MAX_RETRIES` (`:61`). Zero: a single attempt unless the caller raises it.
const DEFAULT_MAX_RETRIES: u32 = 0;

/// pi `BASE_DELAY_MS` (`:62`) — the `BASE_DELAY_MS * 2 ** attempt` ladder, no jitter. This is a
/// *different* ladder from [`crate::utils::provider_retry`] (pi's shared `provider-retry.ts`,
/// which Codex does not use), which is why [`open_sse`](crate::stream::sse::open_sse) is driven with [`ProviderRetry::NONE`](crate::utils::provider_retry::ProviderRetry::NONE) and
/// the loop lives here.
const BASE_DELAY_MS: u64 = 1_000;

/// pi `DEFAULT_MAX_RETRY_DELAY_MS` (`:63`).
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// pi `CODEX_TOOL_CALL_PROVIDERS` (`:68`) — providers whose tool-call ids carry the
/// `call_id|item_id` Responses shape.
const CODEX_TOOL_CALL_PROVIDERS: &[&str] = &["openai", "openai-codex", "opencode"];

/// pi `CODEX_RESPONSE_STATUSES` (`:73-80`). A terminal `response.status` outside this set is
/// normalized to absent, which the shared `mapStopReason(undefined)` reads as `stop`.
const CODEX_RESPONSE_STATUSES: &[&str] = &[
    "completed",
    "incomplete",
    "failed",
    "cancelled",
    "queued",
    "in_progress",
];

/// WHATWG *forgiving-base64* decode, i.e. JS `atob` (`extractAccountId`, `:1568`): standard
/// alphabet, padding optional, `-`/`_` rejected.
const ATOB: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Boxed frame stream — the shape [`open_sse`](crate::stream::sse::open_sse) returns and [`decode_stream`](crate::api::openai_responses::decode_stream) consumes.
type FrameStream = std::pin::Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>;

/// The `ApiImpl` for `"openai-codex-responses"`.
pub struct CodexResponsesApi {
    api: ApiId,
}

impl Default for CodexResponsesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl CodexResponsesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(CodexResponsesApi::new())
}
