//! SEAM-075 — a session must be able to launch with NO model.
//!
//! pi's `findInitialModel` legitimately returns `{ model: undefined }` when nothing is configured
//! (`packages/coding-agent/src/core/model-resolver.ts:648-650` @v0.83.0), `createAgentSession`
//! records that as a **banner** rather than an error —
//!
//! ```text
//! // packages/coding-agent/src/core/sdk.ts:216-218 @v0.83.0
//! model = result.model;
//! if (!model) {
//!     modelFallbackMessage = formatNoModelsAvailableMessage();
//! }
//! ```
//!
//! — and the hard stop lives one tier up, gated on the MODE:
//!
//! ```text
//! // packages/coding-agent/src/main.ts:852-855 @v0.83.0
//! if (appMode !== "interactive" && !session.model) {
//!     console.error(chalk.red(formatNoModelsAvailableMessage()));
//!     process.exit(1);
//! }
//! ```
//!
//! Interactive is excluded on purpose, so a credential-less first run gets a TUI to type `/login`
//! and then `/model` into. These tests pin the session-tier half of that: the build SUCCEEDS on an
//! empty catalog, `model()` is `None`, the fallback message is pi's exact text, and the first turn
//! attempted without a model answers `formatNoModelSelectedMessage()`
//! (`agent-session.ts:1178-1180`) instead of killing anything.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_config::AuthStore;
use cyrup_core::{ModelThinkingLevel, ProviderId};
use cyrup_provider::unconfigured::UnconfiguredProvider;
use cyrup_provider::{CreateModelsOptions, Provider};
use crate::{ProviderResolver, SessionBuilder, SessionConfig, SessionServiceError};
use tempfile::TempDir;

/// Resolves a provider id to the real built-in provider, as the bin's `select_provider` does —
/// this is what a `/model` selection targeting a newly-authenticated provider swaps in.
/// (Same helper as `model_registry.rs`'s cross-provider swap test.)
struct RegistryResolver;

impl ProviderResolver for RegistryResolver {
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        cyrup_provider::default_models(CreateModelsOptions {
            credentials: None,
            auth_context: None,
            catalog_overlay: None,
        })
        .get_provider(provider_id)
        .ok_or_else(|| format!("no built-in provider '{provider_id}'"))
    }
}

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// The state a bare `cyrup` with no credentials lands in: the zero-model provider.
fn unconfigured() -> Arc<dyn Provider> {
    Arc::new(UnconfiguredProvider::new())
}

/// The regression itself: `SessionBuilder::build` on an empty catalog used to return
/// `Err(NoModels("unconfigured"))`, which killed `cyrup --no-session` for every new user. It must
/// now SUCCEED with `model: None` + pi's `formatNoModelsAvailableMessage()` as the fallback banner
/// (sdk.ts:216-218).
#[tokio::test]
async fn empty_catalog_builds_a_modelless_session_with_the_no_models_banner() {
    let fx = fixture();
    let session = SessionBuilder::new(unconfigured(), base_config(&fx))
        .build()
        .await
        .expect("an empty catalog must NOT be fatal — pi launches interactive modelless");

    assert!(session.model().is_none(), "pi `session.model` is undefined with nothing configured");
    assert!(
        session.services().model.is_none(),
        "the resolved catalog `Model` is absent too (pi `AgentSession.model: Model | undefined`)"
    );
    assert_eq!(
        session.model_fallback_message(),
        Some(crate::auth_guidance::format_no_models_available_message().as_str()),
        "pi sdk.ts:218 assigns `modelFallbackMessage = formatNoModelsAvailableMessage()`"
    );
    // The banner text is the actionable `/login` guidance, verbatim pi auth-guidance.ts:14-16.
    assert_eq!(
        session.model_fallback_message().unwrap(),
        "No models available. Use /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md"
    );
}

/// pi sdk.ts:238-240 — `if (!model) { thinkingLevel = "off"; }`; and the capability getters read the
/// optional model: `getAvailableThinkingLevels` returns the whole `THINKING_LEVEL_OPTIONS` constant
/// (`core/agent-session.ts:1816-1819` — `if (!this.model) return [...THINKING_LEVEL_OPTIONS];` —
/// whose seven rungs are defined at `core/defaults.ts:4-12`) and `supportsThinking` is
/// `!!this.model?.reasoning` (`core/agent-session.ts:1824`).
#[tokio::test]
async fn modelless_session_reports_off_thinking_and_the_full_level_ladder() {
    let fx = fixture();
    let session = SessionBuilder::new(unconfigured(), base_config(&fx)).build().await.unwrap();

    assert_eq!(session.thinking_level().await, ModelThinkingLevel::Off);
    assert!(!session.supports_thinking(), "pi `!!this.model?.reasoning` is false with no model");
    assert_eq!(
        session.available_thinking_levels(),
        vec![
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Minimal,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::Medium,
            ModelThinkingLevel::High,
            ModelThinkingLevel::Xhigh,
            ModelThinkingLevel::Max,
        ],
        "pi returns the seven-rung THINKING_LEVEL_OPTIONS when there is no model \
         (core/agent-session.ts:1817 -> core/defaults.ts:4-12)"
    );
    // Setting a level cannot escape `off` while there is nothing to clamp against
    // (pi `_clampThinkingLevel`, agent-session.ts:1608-1610).
    assert_eq!(
        session.set_thinking_level(ModelThinkingLevel::High).await.unwrap(),
        ModelThinkingLevel::Off
    );
}

/// The honest answer to "what happens on the first turn if the user never logs in": pi throws
/// `formatNoModelSelectedMessage()` from `prompt` (agent-session.ts:1178-1180). The turn fails with
/// the `/login` → `/model` instruction; the process keeps running.
#[tokio::test]
async fn prompting_a_modelless_session_returns_pi_no_model_selected() {
    let fx = fixture();
    let session = SessionBuilder::new(unconfigured(), base_config(&fx)).build().await.unwrap();

    let Err(err) = session.prompt("hello").await else {
        panic!("a modelless prompt must not be accepted");
    };
    assert!(
        matches!(err, SessionServiceError::NoModelSelected),
        "expected NoModelSelected, got {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "No model selected.\n\nUse /login to log into a provider via OAuth or API key. See:\n  docs/providers.md\n  docs/models.md\n\nThen use /model to select a model.",
        "verbatim pi `formatNoModelSelectedMessage` (auth-guidance.ts:18-20)"
    );
    // The session is still alive and still modelless — this is a failed turn, not a teardown.
    assert!(session.model().is_none());
}

/// pi sdk.ts:368-373 guards the seed on the model existing —
/// `if (model) { sessionManager.appendModelChange(model.provider, model.id); }` — while the
/// thinking-level entry is written unconditionally. A modelless session must persist NO
/// `model_change`, so a later resume has no bogus address to restore.
#[tokio::test]
async fn modelless_session_seeds_no_model_change_entry() {
    let fx = fixture();
    let session = SessionBuilder::new(unconfigured(), base_config(&fx)).build().await.unwrap();

    let kinds: Vec<String> = session
        .entries_json()
        .await
        .iter()
        .filter_map(|e| e.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .collect();
    assert!(
        !kinds.iter().any(|k| k == "model_change"),
        "no model was resolved, so no model_change may be seeded; got {kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k == "thinking_level_change"),
        "pi appends the thinking-level entry unconditionally (sdk.ts:373); got {kinds:?}"
    );
}

/// The onboarding path itself: a modelless session that later selects a model behaves exactly like
/// one that launched with it — `/login` then `/model` is pi's entire first-run flow.
#[tokio::test]
async fn selecting_a_model_promotes_a_modelless_session() {
    let fx = fixture();
    // `/login`: a credential lands in `auth.json` for a real provider, which is what makes its
    // models pass `hasConfiguredAuth` and become selectable in `/model`.
    let auth = Arc::new(AuthStore::at(fx.agent_dir.join("auth.json")));
    auth.set_runtime_api_key(ProviderId::from("together"), "sk-together-test".to_string());
    let session = SessionBuilder::new(unconfigured(), base_config(&fx))
        .auth(auth)
        .provider_resolver(Arc::new(RegistryResolver))
        .build()
        .await
        .unwrap();
    assert!(session.model().is_none(), "the session still launched modelless");

    // The model the user picks in `/model`, resolved out of the catalog that `/login` just made
    // reachable (`available_model_catalog` = pi `modelRuntime.getAvailable()`).
    let picked = session
        .available_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == "together")
        .expect("the freshly-authenticated provider's models are selectable");
    let installed = session
        .set_model_resolved(picked.clone())
        .await
        .expect("selecting a model on a modelless session must succeed");

    assert_eq!(installed.model.as_str(), picked.id.as_str());
    let now = session.model().expect("the session is no longer modelless");
    assert_eq!(now.model.as_str(), picked.id.as_str());
    assert_eq!(now.provider.as_str(), "together");
    // And the prompt gate has lifted: the refusal is no longer `NoModelSelected`.
    let after = session.prompt("hello").await;
    assert!(
        !matches!(after, Err(SessionServiceError::NoModelSelected)),
        "with a model installed the no-model gate must no longer fire"
    );
}
