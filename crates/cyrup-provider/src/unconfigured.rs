//! The **unconfigured** provider — the zero-model stand-in for "nothing is authenticated yet".
//!
//! **Always compiled** — unlike [`crate::faux`], this is production code.
//!
//! ## What pi does
//!
//! pi has no provider object at all in this state. `ModelRuntime.getAvailable()` returns the models
//! whose provider has a resolvable credential; with no `auth.json`, no `models.json` and no provider
//! env var, that list is empty, so `findInitialModel` falls through every step and returns
//! `{ model: undefined }` at its step 5
//! (`packages/coding-agent/src/core/model-resolver.ts:650-651` @v0.83.0), and `createAgentSession`
//! records `modelFallbackMessage = formatNoModelsAvailableMessage()`
//! (`packages/coding-agent/src/core/sdk.ts:216-218`). `main.ts` then hard-stops every
//! non-interactive mode — and ONLY the non-interactive modes:
//!
//! ```text
//! // packages/coding-agent/src/main.ts:852-855 @v0.83.0
//! if (appMode !== "interactive" && !session.model) {
//!     console.error(chalk.red(formatNoModelsAvailableMessage()));
//!     process.exit(1);
//! }
//! ```
//!
//! `formatNoModelsAvailableMessage()` is `No models available. ` + `getProviderLoginHelp()`
//! (`packages/coding-agent/src/core/auth-guidance.ts:6-16`) — an actionable message naming `/login`
//! and the docs.
//!
//! pi **never** selects a test double here. Its own scripted double (`packages/ai/src/providers/faux.ts`)
//! is exported from the `pi-ai` package for tests only: it is absent from `providers/all.ts`, it is
//! not a `KnownProvider`, and `git grep faux v0.83.0 -- packages/coding-agent/src/` matches zero
//! files. Nothing a user can type reaches it.
//!
//! ## CYRUP-DELTA — why a provider object exists at all
//!
//! pi's `AgentSession` holds `model?: Model` and tolerates `undefined`. cyrup's `SessionBuilder`
//! takes a non-optional `Arc<dyn Provider>` (`cyrup-session-svc/src/builder.rs`), so the
//! "no credential anywhere" state is carried by a provider whose catalog is **empty** rather than
//! by an absent provider.
//!
//! **That is the whole of the delta, and it is a representational one only** — it changes where the
//! `None` lives, not what any mode observes. The empty catalog is not itself the stop; it flows
//! through `resolve_model`, which returns `model: None` plus
//! `format_no_models_available_message()` as the `modelFallbackMessage`
//! (`cyrup-session-svc/src/builder.rs:1601-1612` — the `resolved.or_else(available.first())` match,
//! whose `None` arm sets the fallback), exactly as pi's `sdk.ts:216-218` does.
//!
//! The hard stop then lives one tier up and is **mode-gated**, as pi's is:
//! `crates/cyrup/src/main.rs`'s rpc (`:741-745`) and print/json (`:862-866`) arms each check
//! `session.model().is_none()` and return `no_models_available()` — pi's exact
//! `formatNoModelsAvailableMessage()` on stderr, exit 1. The interactive arm (`:552-560`)
//! deliberately has no such check, so a credential-less first run still opens the TUI and can type
//! `/login`. Same message, same stream, same exit code, same set of modes pi stops.
//!
//! (Historical note: an earlier revision of the PROV-052 fix made an empty catalog fatal inside
//! `resolve_model` for *every* mode, which broke exactly that onboarding path. That was SEAM-075;
//! it is closed. Do not reintroduce an `Err` on the empty-catalog branch — the emptiness is data,
//! not an error.)
//!
//! [`UnconfiguredProvider::stream`] exists only to satisfy the trait, and a modelless session never
//! routes a turn to it: the send path returns `SessionServiceError::NoModelSelected` before it ever
//! touches the agent (`cyrup-session-svc/src/builder.rs:1218`, pi `formatNoModelSelectedMessage`,
//! `packages/coding-agent/src/core/auth-guidance.ts:18-20`). If it is ever reached directly it
//! yields the same actionable text as a terminal `error` event — never a scripted answer.

use crate::context::Context;
use crate::model::Model;
use crate::provider::Provider;
use crate::stream::{ErrorReason, StreamEvent, StreamOptions};
use cyrup_core::{ApiId, AssistantMessage, EventStream, ProviderId, StopReason, Usage};

/// The provider id reported by [`UnconfiguredProvider`]. Not a `KnownProvider` in pi terms and not
/// a catalog entry — it can never be reached by a `--provider`/`provider/model` selection, because
/// `select_provider` resolves named providers out of the built-in registry.
pub const UNCONFIGURED_PROVIDER_ID: &str = "unconfigured";

/// pi `getProviderLoginHelp()` — `packages/coding-agent/src/core/auth-guidance.ts:6-12` @v0.83.0
/// (re-read at the tag: `:6` is `export function getProviderLoginHelp(): string {`, `:12` its
/// closing `}`).
///
/// Kept here (rather than only in the bin's `diagnostics.rs`) so the provider layer can produce the
/// same actionable text on the one path that does not go through the bin.
const PROVIDER_LOGIN_HELP: &str = "Use /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md";

/// pi `formatNoModelsAvailableMessage()` —
/// `packages/coding-agent/src/core/auth-guidance.ts:14-16` @v0.83.0.
pub fn format_no_models_available_message() -> String {
    format!("No models available. {PROVIDER_LOGIN_HELP}")
}

/// A provider with an **empty catalog**, installed when neither `--provider`/`--model` nor any
/// stored/env credential names a real provider. See the module docs for the pi citations.
#[derive(Debug)]
pub struct UnconfiguredProvider {
    id: ProviderId,
    /// Always empty. `Provider::models` returns `&[Model]`, so the empty slice needs an owner.
    models: Vec<Model>,
}

impl UnconfiguredProvider {
    pub fn new() -> Self {
        Self {
            id: ProviderId::from(UNCONFIGURED_PROVIDER_ID),
            models: Vec::new(),
        }
    }
}

impl Default for UnconfiguredProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Provider for UnconfiguredProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    /// Empty — this is the whole point. `cyrup-session-svc`'s `resolve_model` maps an empty catalog
    /// to `model: None` + the fallback banner (`cyrup-session-svc/src/builder.rs:1601-1612`), which
    /// is pi's `!session.model` at `packages/coding-agent/src/main.ts:852`.
    fn models(&self) -> &[Model] {
        &self.models
    }

    fn stream(
        &self,
        model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let message = AssistantMessage {
            content: Vec::new(),
            provider: self.id.clone(),
            model: model.id.as_str().to_string(),
            api: ApiId::from(UNCONFIGURED_PROVIDER_ID),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Error,
            deferred: None,
            error_message: Some(format_no_models_available_message()),
            raw_stop_reason: None,
            timestamp: 0,
        };
        let event = StreamEvent::Error {
            reason: ErrorReason::Error,
            error: message,
        };
        Box::pin(tokio_stream::once(event))
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
    use crate::stream::collect_message;

    /// The load-bearing property: the catalog is empty, which is what `resolve_model`
    /// (`cyrup-session-svc/src/builder.rs:1601-1612`) turns into a modelless session + pi's
    /// `modelFallbackMessage` → in the non-interactive modes, `main.rs`'s mode-gated stop
    /// (`:741-745`, `:862-866`) prints `packages/coding-agent/src/main.ts:852-855`'s message and
    /// exits 1; interactive opens the TUI with the banner instead.
    #[test]
    fn catalog_is_empty() {
        let p = UnconfiguredProvider::new();
        assert!(p.models().is_empty());
        assert_eq!(p.id().as_str(), "unconfigured");
    }

    /// pi's exact `formatNoModelsAvailableMessage()` text
    /// (`packages/coding-agent/src/core/auth-guidance.ts:14-16` @v0.83.0), including the `/login`
    /// guidance a user can act on.
    #[test]
    fn message_matches_pi_auth_guidance() {
        let msg = format_no_models_available_message();
        assert_eq!(
            msg,
            "No models available. Use /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
        );
    }

    /// The unreachable-in-practice direct-stream path still answers with the actionable message on
    /// an `error` terminal — never a scripted response.
    #[tokio::test]
    async fn direct_stream_is_an_actionable_error_terminal() {
        let p = UnconfiguredProvider::new();
        let model = Model {
            id: "none".into(),
            name: "None".into(),
            api: ApiId::from(UNCONFIGURED_PROVIDER_ID),
            provider: ProviderId::from(UNCONFIGURED_PROVIDER_ID),
            base_url: "http://localhost:0".into(),
            reasoning: false,
            input: Vec::new(),
            cost: crate::model::ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        };
        let msg =
            collect_message(p.stream(&model, &Context::default(), &StreamOptions::default())).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(
            msg.error_message.as_deref(),
            Some(format_no_models_available_message().as_str())
        );
    }
}
