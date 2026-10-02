//! The llama.cpp chain through a REAL assembled session and the REAL extension host.
//!
//! Every test builds its session with `cyrup::session_launch::build_factory`, the one function the
//! binary builds every mode's factory with, so the llama.cpp built-in is attached exactly the way
//! `cyrup` attaches it (`attach_native_extensions`). The server is a loopback fake in router mode
//! ([`super::fake`]); nothing else is simulated: the provider, the catalog refresh engine, the
//! availability predicate, the model switch, the agent loop and the wire encoding are the shipped
//! code.
//!
//! What this file is, and is not. It drives the chain at the session seam, one step short of the
//! binary: a model's catalog reaches the session through `GuestProviderRegistry::refresh`, the
//! engine behind `ctx.modelRegistry.refresh` that `/llama` calls after every catalog change
//! (`extensions/llama/index.ts:54-60`). The binary-level versions of these scenarios are in
//! [`super::binary`]. The sessions here run on the production builder's own wiring (models store,
//! refresh credentials, refresher, provider auth); nothing is attached by hand.
//!
//! Upstream behaviours covered, with their `tmp/pi` locations (v0.99.2-17):
//!
//! * the catalog is filtered to selectable models and read from the configured server
//!   (`provider.ts:49-56` `modelIsSelectable`, `:228-230` `credentialServerUrl`);
//! * only loaded models are asked for their template (`provider.ts:236-239`);
//! * a thinking-capable chat template makes a model a reasoning model whose requests carry
//!   `chat_template_kwargs.enable_thinking` (`provider.ts:97-98`, `:119-125` `thinkingFormat:
//!   "qwen-chat-template"`);
//! * the provider is available only when a server is configured (`provider.ts:181-186` `check`);
//! * `--no-extensions` drops the built-in (`extensions/index.ts`, `package-manager.ts:972-974`) and
//!   the startup `[Extensions]` list never shows it (`resource-loader.ts:729`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;

use cyrup_core::{ModelThinkingLevel, ProviderId};
use serde_json::json;

use super::fake::{FakeLlama, PLAIN_TEMPLATE, THINKING_TEMPLATE, model, model_with};
use super::fixture::{
    Fx, Opts, PROVIDER, assistant_error, fixture, listed_llama, refresh_llama, session,
    store_credential,
};

/// The key `/login llama.cpp` stored in every test below.
const KEY: &str = "sk-llama-fixture";

/// A router catalog with one model of every selectability class
/// (`provider.ts:49-56` `modelIsSelectable`).
async fn router() -> FakeLlama {
    let fake = FakeLlama::start(vec![
        // Loaded and thinking-capable: selectable, and its template is read.
        model_with("qwen3", "loaded", json!({ "meta": { "n_ctx": 8192 } })),
        // Loaded, no thinking template, and image input.
        model_with(
            "plain",
            "loaded",
            json!({
                "meta": { "n_ctx": 4096 },
                "architecture": { "input_modalities": ["text", "image"] },
            }),
        ),
        // An unloaded PRESET: selectable because the router autoloads on first use.
        model_with("gemma", "unloaded", json!({ "source": "preset" })),
        // Unloaded and not a preset: not routable, so not selectable.
        model("cold", "unloaded"),
        // An unloaded preset that failed to start: not selectable.
        model_with(
            "broken",
            "unloaded",
            json!({ "source": "preset", "status": { "value": "unloaded", "failed": true } }),
        ),
    ])
    .await;
    fake.set_chat_template("qwen3", THINKING_TEMPLATE);
    fake.set_chat_template("plain", PLAIN_TEMPLATE);
    fake
}

/// A session over a stored credential naming `fake`, with its catalog already refreshed.
async fn refreshed(fx: &Fx, fake: &FakeLlama) -> cyrup_session_svc::AgentSession {
    store_credential(fx, fake.url(), Some(KEY));
    let session = session(fx, &Opts::default()).await;
    let errors = refresh_llama(&session).await;
    assert!(errors.is_empty(), "the refresh failed: {errors:?}");
    session
}

/// Scenario 1, discovery: with the server configured the router's SELECTABLE models are listed,
/// each mapped the way `toPiModel` maps it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_configured_server_lists_exactly_the_selectable_models() {
    let fx = fixture();
    let fake = router().await;
    store_credential(&fx, fake.url(), Some(KEY));
    let session = session(&fx, &Opts::default()).await;

    // Building and binding a session contacts no server and lists nothing: pi's startup refresh is
    // cache-only (`agent-session-services.ts:190-206`), so the catalog arrives with a refresh.
    assert!(
        fake.requests().is_empty(),
        "assembling a session must not touch the server: {:?}",
        fake.requests()
    );
    assert!(listed_llama(&session).is_empty());

    let errors = refresh_llama(&session).await;
    assert!(errors.is_empty(), "the refresh failed: {errors:?}");

    // `loaded` and `sleeping` models, plus unloaded PRESETS that did not fail, because the router
    // reports `models_autoload` (`provider.ts:49-56`). `cold` and `broken` are filtered out.
    let mut ids = listed_llama(&session);
    ids.sort();
    assert_eq!(ids, ["gemma", "plain", "qwen3"]);

    let catalog = session.configured_model_catalog();
    let find = |id: &str| {
        catalog
            .iter()
            .find(|m| m.provider.as_str() == PROVIDER && m.id.as_str() == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} is not listed"))
    };
    let qwen = find("qwen3");
    // `baseUrl: llamaInferenceUrl(serverUrl)` (`provider.ts:99`), the stored credential's server.
    assert_eq!(qwen.base_url, format!("{}/v1", fake.url()));
    assert_eq!(qwen.api.as_str(), "openai-completions");
    // `meta.n_ctx` is the runtime context window (`provider.ts:73-76`).
    assert_eq!(qwen.context_window, 8192);
    assert_eq!(qwen.max_tokens, 8192, "`maxTokens: contextWindow` (`:107`)");
    assert!(
        qwen.reasoning,
        "a template mentioning `enable_thinking` is a reasoning model (`:97`)"
    );
    let plain = find("plain");
    assert_eq!(plain.context_window, 4096);
    assert!(!plain.reasoning, "a plain template is not (`:97`)");
    assert!(
        plain
            .input
            .iter()
            .any(|modality| matches!(modality, cyrup_provider::Modality::Image)),
        "`architecture.input_modalities` with `image` adds image input (`:101`): {:?}",
        plain.input
    );
    // No runtime, configured or trained context size: pi's 128000 default (`provider.ts:80`).
    assert_eq!(find("gemma").context_window, 128_000);

    // The wire, from the server's side: the catalog came from the stored credential's server with
    // its key, and ONLY loaded models were asked for their template (`provider.ts:236-239`):
    // asking an unloaded preset would load it, and asking a sleeping one would wake it.
    let listings = fake.requests_to("GET", "/models");
    assert!(
        !listings.is_empty(),
        "the catalog is read from `GET /models`"
    );
    for request in fake.requests() {
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {KEY}").as_str()),
            "every router request carries the stored key: {request:?}"
        );
    }
    for request in fake.requests_to("GET", "/props") {
        if request.query_param("model").is_some() {
            assert_eq!(
                request.query_param("autoload"),
                Some("false"),
                "a template read must not autoload the model it asks about (`client.ts:198`): {request:?}"
            );
        }
    }
    let mut templated: Vec<String> = fake
        .requests_to("GET", "/props")
        .iter()
        .filter_map(|r| r.query_param("model"))
        .map(str::to_string)
        .collect();
    templated.sort();
    assert_eq!(
        templated,
        ["plain", "qwen3"],
        "only the LOADED models are asked for their chat template: {:?}",
        fake.requests()
    );
}

/// The production session, with nothing attached by hand, refreshes llama.cpp from its stored
/// credential. This is the wiring every other test in this file stands on: the builder attaches the
/// refresh engine's credential store, models store, refresher and provider-auth source. Without
/// them the engine has no credential store to resolve the provider's auth against and silently
/// skips it (`models.ts:573-576`), and the extension's `provider_auth` and `refresh_provider` host
/// verbs answer "nothing" and "no refresh backend".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_production_session_refreshes_llama_from_its_stored_credential() {
    let fx = fixture();
    let fake = router().await;
    store_credential(&fx, fake.url(), Some(KEY));
    let session = session(&fx, &Opts::default()).await;

    let errors = refresh_llama(&session).await;

    assert!(errors.is_empty(), "the refresh failed: {errors:?}");
    assert!(
        !fake.requests_to("GET", "/models").is_empty(),
        "the catalog-refresh engine never resolved llama.cpp's stored credential, so it never \
         contacted the server: the builder must attach the credential store \
         (`GuestProviderRegistry::attach_refresh_auth`)"
    );
    let mut ids = listed_llama(&session);
    ids.sort();
    assert_eq!(ids, ["gemma", "plain", "qwen3"]);
}

/// The catalog a refresh persists to `<agent_dir>/models-store.json` is the one the binary restores
/// at its next start: the chat models AND their classifier twins (pi persists one array holding
/// both, `provider.ts:251-253` `persist: { models: [...refreshed, ...refreshedClassifiers] }`).
/// The builder backs the registry with `cyrup_config::models_store::FileModelsStore`, whose
/// `write_with_classifiers` writes both halves as one operation; a store that refused the
/// classifier models would fail the publication before its `update` ran, and the catalog would
/// never be installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_persists_chat_and_classifier_models_to_the_models_store_file() {
    let fx = fixture();
    let fake = router().await;
    store_credential(&fx, fake.url(), Some(KEY));
    let session = session(&fx, &Opts::default()).await;

    let errors = refresh_llama(&session).await;

    assert!(errors.is_empty(), "the refresh failed: {errors:?}");
    let mut ids = listed_llama(&session);
    ids.sort();
    assert_eq!(
        ids,
        ["gemma", "plain", "qwen3"],
        "a refresh over the on-disk store installs the catalog (the store refused the classifier \
         models, so the publication failed before its `update` ran)"
    );
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.agent_dir.join("models-store.json")).unwrap(),
    )
    .unwrap();
    let models = stored[PROVIDER]["models"].as_array().unwrap();
    let types = |wanted: &str| {
        models
            .iter()
            .filter(|m| m.get("type").and_then(|t| t.as_str()).unwrap_or("chat") == wanted)
            .count()
    };
    assert_eq!(types("chat"), 3, "{stored}");
    assert_eq!(types("classifier"), 3, "{stored}");
}

/// Scenario 1, the turn: a one-shot turn against a chosen llama.cpp model streams its reply through
/// the router's `/v1/chat/completions`, and `enable_thinking` rides in `chat_template_kwargs` when
/// the model's template supports it (`provider.ts:97-98`, `:119-125`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_streams_through_chat_completions_with_enable_thinking_in_the_template_kwargs() {
    let fx = fixture();
    let fake = router().await;
    fake.set_reply("Hello from llama.cpp");
    let session = refreshed(&fx, &fake).await;

    let picked = session.set_model("llama.cpp/qwen3").await.unwrap();
    assert_eq!(picked.provider.as_str(), PROVIDER);
    assert_eq!(picked.model.as_str(), "qwen3");
    // Medium is the one level the model maps on (`thinkingLevelMap: { off: "off", medium:
    // "medium", .. }`, `provider.ts:102-104`).
    assert_eq!(
        session
            .set_thinking_level(ModelThinkingLevel::Medium)
            .await
            .unwrap(),
        ModelThinkingLevel::Medium
    );

    let _events = session.prompt("say hi").await.expect("prompt accepted");
    session.wait_for_idle().await;

    // The reply came off the fake's SSE stream, reassembled from its two deltas.
    assert_eq!(
        session.last_assistant_text().await.as_deref(),
        Some("Hello from llama.cpp"),
        "the turn did not stream; its assistant message ended in {:?}. The stored credential must \
         reach the provider's own credential store: the native's controller reads \
         `HostServices::provider_credentials` once, when it is built at `init`, so the builder has \
         to attach the provider-auth source before init, or the stream resolves no credential and \
         answers \"not configured\"",
        assistant_error(&session).await
    );

    let calls = fake.requests_to("POST", "/v1/chat/completions");
    assert_eq!(
        calls.len(),
        1,
        "one request per turn: {:?}",
        fake.requests()
    );
    let request = &calls[0];
    let body = request.json();
    assert_eq!(body["model"], "qwen3");
    assert_eq!(body["stream"], true);
    assert_eq!(
        body["chat_template_kwargs"]["enable_thinking"], true,
        "`thinkingFormat: qwen-chat-template` sends the thinking switch as a template kwarg: {body}"
    );
    assert!(
        body["messages"].as_array().is_some_and(|messages| messages
            .iter()
            .any(|m| m["content"].to_string().contains("say hi"))),
        "the user's prompt is in the request: {body}"
    );
    // `supportsStore: false` and `supportsDeveloperRole: false` (`provider.ts:113-114`).
    assert!(body.get("store").is_none(), "no `store` member: {body}");
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {KEY}").as_str()),
        "the stored key authenticates the request"
    );

    // Turning thinking off flips the kwarg instead of dropping it (`off: "off"`).
    session
        .set_thinking_level(ModelThinkingLevel::Off)
        .await
        .unwrap();
    let _events = session.prompt("again").await.expect("prompt accepted");
    session.wait_for_idle().await;
    let calls = fake.requests_to("POST", "/v1/chat/completions");
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].json()["chat_template_kwargs"]["enable_thinking"],
        false,
        "thinking off is `enable_thinking: false`: {}",
        calls[1].body
    );
}

/// A model whose template does not mention `enable_thinking` is not a reasoning model, so its
/// requests carry no template kwargs at all (`provider.ts:97`, `:125`: `...(reasoning && {
/// thinkingFormat })`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_model_without_a_thinking_template_sends_no_template_kwargs() {
    let fx = fixture();
    let fake = router().await;
    let session = refreshed(&fx, &fake).await;

    session.set_model("llama.cpp/plain").await.unwrap();
    let _events = session.prompt("say hi").await.expect("prompt accepted");
    session.wait_for_idle().await;

    assert_eq!(
        session.last_assistant_text().await.as_deref(),
        Some("Hello from llama.cpp"),
        "the turn did not stream; its assistant message ended in {:?}. The stored credential never reaches \
         the provider's own credential store: the native's controller reads \
         `HostServices::provider_credentials` once, when it is built at `init`, and the builder \
         attaches the provider-auth source after init (or never), so the stream resolves no \
         credential and answers \"not configured\"",
        assistant_error(&session).await
    );
    let calls = fake.requests_to("POST", "/v1/chat/completions");
    assert_eq!(calls.len(), 1);
    let body = calls[0].json();
    assert_eq!(body["model"], "plain");
    assert!(
        body.get("chat_template_kwargs").is_none(),
        "no thinking template, no kwargs: {body}"
    );
}

/// Scenario 2: with no configuration at all llama.cpp is unavailable and nothing is listed. The
/// provider IS registered (pi registers it unconditionally, `index.ts:44`); what is absent is its
/// auth, so a refresh does nothing and the server is never contacted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_any_configuration_llama_is_unavailable_and_nothing_is_listed() {
    let fx = fixture();
    let fake = router().await;
    // The server exists and is reachable; nothing names it.
    let session = session(&fx, &Opts::default()).await;

    let provider = session
        .services()
        .guest_providers
        .provider(PROVIDER)
        .expect("the provider is registered whether or not a server is configured (`index.ts:44`)");
    let check = cyrup_config::login::live_provider_auth_check(&session.services().auth, &provider)
        .await
        .expect("the check itself does not fail");
    assert!(
        check.is_none(),
        "no stored credential and no `LLAMA_BASE_URL`: not available (`provider.ts:181-186`)"
    );

    assert!(
        refresh_llama(&session).await.is_empty(),
        "a refresh with nothing configured is a clean no-op"
    );
    assert!(listed_llama(&session).is_empty());
    assert!(
        fake.requests().is_empty(),
        "nothing names the server, so nothing contacts it: {:?}",
        fake.requests()
    );
    assert!(
        session.set_model("llama.cpp/qwen3").await.is_err(),
        "there is no such model to select"
    );
}

/// The environment variable alone configures llama.cpp: the provider is AVAILABLE (`check`'s second
/// source, `provider.ts:29-35`, `:181-186`, labelled `LLAMA_BASE_URL`) and a refresh reads the
/// catalog from that server, because the engine hands `refreshModels` the credential the auth
/// strategy resolves, whose env carries the server (`resolveRefreshCredential`, `models.ts:608-635`).
/// With no stored key the request key is `LLAMA_API_KEY`, else `local` (`provider.ts:190`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_environment_variable_alone_configures_llama_and_lists_models() {
    let fx = fixture();
    let fake = router().await;
    let opts = Opts {
        ambient_env: HashMap::from([
            ("LLAMA_BASE_URL".to_string(), fake.url().to_string()),
            ("LLAMA_API_KEY".to_string(), "sk-from-env".to_string()),
        ]),
        ..Opts::default()
    };
    let session = session(&fx, &opts).await;

    let provider = session
        .services()
        .guest_providers
        .provider(PROVIDER)
        .unwrap();
    let check = cyrup_config::login::live_provider_auth_check(&session.services().auth, &provider)
        .await
        .unwrap()
        .expect("`LLAMA_BASE_URL` configures the provider");
    assert_eq!(check.source.as_deref(), Some("LLAMA_BASE_URL"));
    assert!(
        fake.requests().is_empty(),
        "availability is decided without contacting the server"
    );

    assert!(refresh_llama(&session).await.is_empty());

    let mut ids = listed_llama(&session);
    ids.sort();
    assert_eq!(ids, ["gemma", "plain", "qwen3"]);
    let listings = fake.requests_to("GET", "/models");
    assert!(!listings.is_empty());
    assert_eq!(
        listings[0].header("authorization"),
        Some("Bearer sk-from-env"),
        "no stored key: the request key is `LLAMA_API_KEY`"
    );
}

/// Availability gates the listing, not just the catalog: a populated catalog disappears from the
/// listing when the credential goes (`/logout llama.cpp`), because `check` then reports no server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removing_the_credential_hides_a_populated_catalog() {
    let fx = fixture();
    let fake = router().await;
    let session = refreshed(&fx, &fake).await;
    assert_eq!(listed_llama(&session).len(), 3, "listed while configured");

    session
        .services()
        .auth
        .delete(&ProviderId::from(PROVIDER))
        .await
        .unwrap();

    assert!(
        listed_llama(&session).is_empty(),
        "the models are still in the provider's catalog but the provider is no longer available"
    );
}

/// Scenario 3: `--no-extensions` drops the built-in. The control half of the test is the same
/// fixture without the flag, so the absence is the flag's doing and not the fixture's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_extensions_drops_the_llama_built_in_and_its_provider() {
    let fx = fixture();
    let fake = router().await;
    store_credential(&fx, fake.url(), Some(KEY));

    // Control: loaded, registered and listing.
    let with = session(&fx, &Opts::default()).await;
    assert!(
        with.services()
            .ext_host
            .loaded_ids()
            .iter()
            .any(|id| id.as_str() == PROVIDER),
        "the control session loads the built-in"
    );
    assert!(refresh_llama(&with).await.is_empty());
    assert_eq!(listed_llama(&with).len(), 3);
    let contacted = fake.requests().len();
    assert!(contacted > 0);

    // `--no-extensions`: pi's `builtin:llama.cpp` is a path in the tier the flag collapses
    // (`package-manager.ts:972-974`, `resource-loader.ts:569-571`).
    let without = session(
        &fx,
        &Opts {
            no_extensions: true,
            ..Opts::default()
        },
    )
    .await;
    assert!(
        !without
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .any(|id| id.as_str() == PROVIDER),
        "`--no-extensions` unloads the built-in"
    );
    assert!(
        without
            .services()
            .guest_providers
            .provider(PROVIDER)
            .is_none(),
        "and no provider is registered"
    );
    assert!(refresh_llama(&without).await.is_empty());
    assert!(listed_llama(&without).is_empty());
    assert!(without.set_model("llama.cpp/qwen3").await.is_err());
    assert_eq!(
        fake.requests().len(),
        contacted,
        "a session without the extension never contacts the server"
    );
}

/// Scenario 4: the built-in is absent from the startup `[Extensions]` list (it is `hidden`,
/// `resource-loader.ts:729`) while it is loaded and its provider works. Both halves in one session,
/// so "hidden" cannot be satisfied by "not loaded".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn llama_is_absent_from_the_startup_extension_list_but_its_provider_works() {
    let fx = fixture();
    let fake = router().await;
    let session = refreshed(&fx, &fake).await;

    let listed = cyrup_tui::StartupReport::from_session(&session, false).extensions;
    assert!(
        !listed.iter().any(|id| id == PROVIDER),
        "the startup extension list never shows a built-in: {listed:?}"
    );
    assert!(
        session
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .any(|id| id.as_str() == PROVIDER),
        "it is loaded all the same"
    );
    assert_eq!(listed_llama(&session).len(), 3, "and its provider works");
}
