//! Hermetic scaffolding shared by the llama.cpp tests: a temp home, the credential `/login
//! llama.cpp` stores, the catalog cache a completed refresh persists, and a real assembled
//! session built through the production seam.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cyrup_config::{AuthStore, CliConfigOverrides, ConfigDirs, EnvVars, ModelFile};
use cyrup_core::CancelToken;
use cyrup_ext::host::services::ProviderRefreshRequest;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionConfig};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The provider id pi registers (`provider.ts` `LLAMA_PROVIDER_ID`).
pub const PROVIDER: &str = "llama.cpp";

pub struct Fx {
    pub _tmp: TempDir,
    pub cwd: PathBuf,
    pub agent_dir: PathBuf,
    pub home: PathBuf,
}

/// A project, an agent dir and a home, all under one temp root.
pub fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let home = tmp.path().join("home");
    for dir in [&cwd, &agent_dir, &home] {
        std::fs::create_dir_all(dir).unwrap();
    }
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
        home,
    }
}

/// What `/login llama.cpp` stores for a server: `{ type: "api_key", key, env: { LLAMA_BASE_URL } }`
/// (`provider.ts:175-179`). The refresh engine hands `refreshModels` the credential its auth
/// strategy RESOLVES (`resolveRefreshCredential`, `ai/src/models.ts:608-635`), whose `env` carries
/// `LLAMA_BASE_URL` from this stored credential or from the ambient variable alike, so either
/// configures a catalog refresh (`credentialServerUrl`, `provider.ts:24-27`, `:228-230`).
pub fn store_credential(fx: &Fx, server_url: &str, key: Option<&str>) {
    let mut credential = json!({ "type": "api_key", "env": { "LLAMA_BASE_URL": server_url } });
    if let (Some(key), Some(object)) = (key, credential.as_object_mut()) {
        object.insert("key".to_string(), json!(key));
    }
    std::fs::write(
        fx.agent_dir.join("auth.json"),
        serde_json::to_string(&json!({ PROVIDER: credential })).unwrap(),
    )
    .unwrap();
}

/// The chat model JSON a refresh persists for a router model: `toPiModel`'s output
/// (`provider.ts:84-130`) as the models store writes it, a thinking-capable model carrying
/// `thinkingLevelMap` and `compat.thinkingFormat: "qwen-chat-template"`.
pub fn cached_chat_model(
    id: &str,
    server_url: &str,
    context_window: u64,
    reasoning: bool,
) -> Value {
    let mut compat = json!({
        "supportsStore": false,
        "supportsDeveloperRole": false,
        "supportsReasoningEffort": false,
        "supportsUsageInStreaming": true,
        "supportsStrictMode": false,
        "maxTokensField": "max_tokens",
    });
    let mut model = json!({
        "id": id,
        "name": id,
        "api": "openai-completions",
        "provider": PROVIDER,
        "baseUrl": format!("{server_url}/v1"),
        "reasoning": reasoning,
        "input": ["text"],
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": context_window,
        "maxTokens": context_window,
    });
    if reasoning {
        compat["thinkingFormat"] = json!("qwen-chat-template");
        model["thinkingLevelMap"] = json!({
            "off": "off", "minimal": null, "low": null, "medium": "medium", "high": null,
            "xhigh": null,
        });
    }
    model["compat"] = compat;
    model
}

/// Write `<agent_dir>/models-store.json` the way a completed refresh persists it
/// (`models-store.json` is `{ <provider>: { models, lastModified, checkedAt } }`).
pub fn seed_catalog_cache(fx: &Fx, models: Vec<Value>) {
    let now = 4_102_444_800_000_i64;
    std::fs::write(
        fx.agent_dir.join("models-store.json"),
        serde_json::to_string_pretty(&json!({
            PROVIDER: { "models": models, "lastModified": now, "checkedAt": now }
        }))
        .unwrap(),
    )
    .unwrap();
}

/// How a session is assembled.
#[derive(Default, Clone)]
pub struct Opts {
    /// `--no-extensions`.
    pub no_extensions: bool,
    /// The fixed ambient environment the credential store reads (`LLAMA_BASE_URL`,
    /// `LLAMA_API_KEY`). Empty by default: nothing ambient reaches the session.
    pub ambient_env: HashMap<String, String>,
}

/// A REAL session, assembled through `cyrup::session_launch::build_factory` - the one function
/// every mode arm of the binary builds its factory with, which is what attaches the llama.cpp
/// built-in (`attach_native_extensions`). Nothing here constructs the extension by hand, and
/// nothing here attaches the refresh engine's pieces by hand either.
///
/// The injected provider is the faux double, so a model the test selects afterwards is an
/// EXTENSION-registered one the session has to install itself.
pub async fn session(fx: &Fx, opts: &Opts) -> AgentSession {
    let env = EnvVars {
        home: Some(fx.home.clone()),
        ..EnvVars::default()
    };
    let overrides = CliConfigOverrides {
        agent_dir: Some(fx.agent_dir.clone()),
        cwd: Some(fx.cwd.clone()),
        ..Default::default()
    };
    let dirs = ConfigDirs::resolve(&overrides, &env).unwrap();

    let mut config = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    config.persist = false;
    config.trust_override = Some(true);
    config.no_extensions = opts.no_extensions;
    let target = config.target.clone();

    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let auth = Arc::new(
        AuthStore::at(fx.agent_dir.join("auth.json")).with_ambient_env(opts.ambient_env.clone()),
    );
    let factory = cyrup::session_launch::build_factory(
        provider,
        config,
        cyrup::file_settings_store(&dirs),
        auth,
        &dirs,
        Arc::new(ModelFile::default()),
        None,
    )
    .unwrap();
    // The session is exactly what the production builder returns: its models store
    // (`<agent_dir>/models-store.json`), the refresh engine's credential store and the host's
    // refresher and provider-auth verbs are attached by `SessionBuilder::build` itself, which is
    // what `a_production_session_refreshes_llama_from_its_stored_credential` and the rest of these
    // tests stand on.
    let session = factory.build(target, None).await.unwrap();
    session.bind_extensions().await;
    session
}

/// The error text of the latest assistant message of the session's branch, when it ended in an
/// error: the one observable of a turn that never streamed.
pub async fn assistant_error(session: &AgentSession) -> Option<String> {
    session
        .messages()
        .await
        .into_iter()
        .rev()
        .find_map(|message| match message {
            cyrup_core::Message::Assistant(assistant) => assistant.error_message,
            _ => None,
        })
}

/// `ctx.modelRegistry.refresh({ providers: ["llama.cpp"], allowNetwork: true })` - the call `/llama`
/// makes after every catalog change (`index.ts:54-60`), run on the session's own catalog-refresh
/// engine. Returns the engine's per-provider error text, empty when the refresh was clean.
pub async fn refresh_llama(session: &AgentSession) -> Vec<String> {
    let result = session
        .services()
        .guest_providers
        .refresh(ProviderRefreshRequest {
            providers: Some(vec![PROVIDER.to_string()]),
            allow_network: Some(true),
            force: false,
            cancel: CancelToken::new(),
        })
        .await;
    assert!(!result.aborted, "the refresh was not cancelled");
    result
        .errors
        .iter()
        .map(|(id, error)| format!("{id}: {error}"))
        .collect()
}

/// The `(provider, id)` pairs of the models the session lists as selectable and configured -
/// `--list-models`' source (`AgentSession::configured_model_catalog`).
pub fn listed(session: &AgentSession) -> Vec<(String, String)> {
    session
        .configured_model_catalog()
        .iter()
        .map(|model| {
            (
                model.provider.as_str().to_string(),
                model.id.as_str().to_string(),
            )
        })
        .collect()
}

/// The ids the session lists under the llama.cpp provider.
pub fn listed_llama(session: &AgentSession) -> Vec<String> {
    listed(session)
        .into_iter()
        .filter(|(provider, _)| provider == PROVIDER)
        .map(|(_, id)| id)
        .collect()
}
