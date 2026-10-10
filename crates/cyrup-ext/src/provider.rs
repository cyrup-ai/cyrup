//! Custom-provider registration host-side (arch-08 §5.6; Pi `registerProvider`, types.ts:1337/1363;
//! `runner.ts:344` bindCore). A guest sends a typed [`ProviderConfig`] across the seam; the host
//! parses it, resolves the API key (literal / `$ENV`/`${ENV}` interpolation / leading `!command`,
//! Pi's resolution rules), and routes it to the [`ModelRegistrySink`] the session injects. Until the
//! sink is bound, registrations QUEUE; [`ProviderHub::bind`] flushes the pending set (Pi's
//! defer→`bindCore` lifecycle, A-08-7).
//!
//! # EXT-M07 — the dynamic half of a guest provider is declared everywhere and invoked nowhere
//!
//! OAuth + `streamSimple` cross this seam as opaque blocks because they are guest CLOSURES; the
//! guest exports that execute them do exist and are implemented end to end —
//! [`crate::host::LiveExtension::provider_login`], `provider_refresh_token`,
//! `provider_get_api_key`, `provider_modify_models`, `provider_stream_simple`, each backed by a WIT
//! export and a `cyrup-ext-sdk` wrapper. **None of the five has a production caller** (workspace
//! grep at HEAD: only `crates/cyrup-it/tests/ext/{wasm_provider,ergonomic}.rs`). The one consumer
//! of a registration is `cyrup_session_svc::GuestProviderRegistry::upsert_provider`, which calls
//! [`ProviderRegistration::build_provider`] and gets a `ConfigProvider` over `baseUrl` with the
//! api-key string resolved ONCE at registration time.
//!
//! So a guest-registered provider is selectable and streamable, but statically: its login flow
//! never runs, an expired token is never refreshed, `getApiKey` never resolves a per-request key,
//! `modifyModels` never reshapes the catalog, and a custom `streamSimple` transport is replaced by
//! the built-in HTTP one. Upstream all five are plain fields of the object pi's `ModelRegistry`
//! holds and calls (`extensions/types.ts:1363-1475`, `model-registry.ts:828-960` @v0.84.1), so
//! there is nothing to wire there and everything to wire here. **The fix site is
//! `crates/cyrup-session-svc/src/guest_providers.rs` (plus whatever credential store the refresh
//! writes back to) — not this crate**, which already exposes every call it needs.
//!
//! # Live providers
//!
//! A NATIVE extension is not limited to that static description: [`ProviderHub::register_live`]
//! (reached through `InitApi::register_provider_live` and, after `init`, `LateRegistrar::
//! register_provider_live`) stores the extension's own `Arc<dyn Provider>` and hands it to
//! [`ModelRegistrySink::upsert_live_provider`] as is — no `ConfigProvider` is rebuilt, so its
//! transport, catalog and (re-registered) replacement are the extension's own.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A custom LLM provider configuration (host mirror of the SDK `ProviderConfig`, Pi types.ts:1363).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Literal, `$ENV`/`${ENV}` interpolation, or leading `!command` (resolved by [`resolve_api_key`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub models: Vec<ProviderModelConfig>,
    /// OAuth metadata (`{name, isSubscription?}`): a static marker that this provider
    /// authenticates via OAuth. The dynamic callbacks (login/refreshToken/getApiKey/modifyModels)
    /// are guest closures invoked via the `provider-*` exports
    /// ([`crate::host::LiveExtension::provider_login`], etc.).
    ///
    /// Kept as an open `Value` because the callback keys are guest closures with no host mirror;
    /// [`Self::oauth_is_subscription`] is the typed read of the one metadata key that has meaning
    /// on this side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<Value>,
    /// Whether the guest supplied a custom `streamSimple` handler (pi `ProviderConfig.streamSimple`,
    /// `extensions/types.ts:1452-1457` @v0.84.1).
    ///
    /// **EXT-M07 — nothing reads this in production.** It is parsed here and exposed by
    /// [`Self::has_stream_simple`]/[`ProviderRegistration::has_stream_simple`], but a workspace-wide
    /// grep at HEAD finds the only readers are `crates/cyrup-it/tests/ext/`. The realized provider
    /// comes from [`ProviderRegistration::build_provider`], which unconditionally builds a
    /// `ConfigProvider` streaming over `baseUrl` — so a guest that registered a provider precisely
    /// to own the transport gets the built-in HTTP one instead, silently, and its
    /// `provider-stream-simple` export is never invoked. The same holds for the four OAuth
    /// callbacks; see this module's doc.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_stream_simple: bool,
}

impl ProviderConfig {
    /// Whether access through this provider's OAuth is backed by a SUBSCRIPTION rather than a
    /// metered API key (EXT-051).
    ///
    /// pi added `isSubscription?: boolean` to the `oauth` block at
    /// `pi/packages/coding-agent/src/core/extensions/types.ts:1475` @v0.84.1 (absent at v0.83.0),
    /// documented "Whether access through this auth method is backed by a provider subscription."
    /// cyrup already models the concept for BUILT-IN providers — `AuthProvider::is_subscription`
    /// (`crates/cyrup-provider/src/auth/mod.rs:101`, overridden by anthropic / github_copilot /
    /// kimi_coding / openai_codex / xai) — but an extension-supplied provider's key arrived inside
    /// the opaque `oauth` blob with no reader, so a guest declaring `isSubscription` was still
    /// presented like a metered API-key provider.
    ///
    /// Absent or non-boolean reads as `false`, matching an omitted optional upstream.
    pub fn oauth_is_subscription(&self) -> bool {
        self.oauth
            .as_ref()
            .and_then(|o| o.get("isSubscription"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

/// One long-context pricing tier for a registered model (Pi `ModelCostTier`, ai/types.ts:750-753).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
    #[serde(default)]
    pub input_tokens_above: u64,
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

/// Per-token cost for a registered model (Pi `ProviderModelConfig.cost`, types.ts:1493 — "per-
/// million-token cost rates and optional request-wide input pricing tiers"). All four base rates
/// are required by Pi (they may be `0`); `tiers` is optional and, when present, replaces every base
/// rate for requests whose total input usage exceeds the highest matching threshold.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelCostTier>>,
}

/// Per-model config inside a [`ProviderConfig`] (Pi `ProviderModelConfig`, types.ts:1404-1429).
/// Carries the FULL Pi shape (gap-08 #14): a host mirror that kept only `id`/`name`/`contextWindow`/
/// `maxOutputTokens` silently dropped a registered provider's cost/reasoning/modality/api/baseUrl/
/// thinking-level map/headers/compat before they reached the model-registry sink. Open-shaped fields
/// (`thinkingLevelMap`, `compat`) cross as `serde_json::Value` (Pi `Model<Api>["…"]`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Per-model API family override (Pi `api`, types.ts:1410).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Per-model endpoint override (Pi `baseUrl`, types.ts:1412).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Whether the model supports extended thinking (Pi `reasoning`, types.ts:1414).
    #[serde(default)]
    pub reasoning: bool,
    /// Pi `thinkingLevelMap` (types.ts:1416): pi-level → provider value (`null` marks unsupported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<Value>,
    /// Supported input modalities (Pi `input: ("text"|"image")[]`, types.ts:1418).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    /// Provider input limits and the cache-safe image resize profile (Pi
    /// `inputLimits?: AnyModel["inputLimits"]`, `extensions/types.ts:1961` @v1.0.4, declared on
    /// `ProviderModelConfigBase` immediately after `input` and immediately BEFORE `promptCache`
    /// on the chat subtype — which is why it sits here rather than beside `prompt_cache`).
    ///
    /// Without it an extension-registered provider could not declare a resize profile at all, and
    /// the resize profile read off the model an extension's `before_agent_start` handler selects
    /// would have nothing to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_limits: Option<cyrup_provider::ModelInputLimits>,
    /// Per-token cost (Pi `cost`, types.ts:1422).
    #[serde(default)]
    pub cost: ModelCost,
    /// Max context window in tokens (Pi `contextWindow`, types.ts:1424).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Max output tokens (Pi `maxTokens`, types.ts:1426).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Default sampling parameters applied at every thinking level (Pi
    /// `ProviderChatModelConfig.samplingParams?: Model<Api>["samplingParams"]`,
    /// `core/provider-composer.ts:71` @f1b2e77f5). Typed, so a non-object value fails the
    /// registration rather than vanishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_params: Option<cyrup_provider::SamplingParams>,
    /// Per-thinking-level sampling overrides spread over [`Self::sampling_params`] for the
    /// request's effective level (Pi `ProviderChatModelConfig.samplingParamsByThinkingLevel?`,
    /// `core/provider-composer.ts:72` @f1b2e77f5, added `76dfb88f6`). CFG-104, extension path:
    /// without this field serde dropped the key with no diagnostic, so an extension provider's
    /// per-level sampling silently did nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_params_by_thinking_level: Option<cyrup_provider::SamplingParamsByThinkingLevel>,
    /// Per-model custom headers (Pi `headers`, types.ts:1428).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// OpenAI-compat settings (Pi `compat`, types.ts:1430): open-shaped `Model<Api>["compat"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<Value>,
    /// Prompt-cache lifetimes per retention tier, in SECONDS (Pi
    /// `promptCache?: Model<Api>["promptCache"]`, `extensions/types.ts:1978` @v1.0.4) — what
    /// `cyrup_provider::prompt_cache_ttl_ms` schedules cache warming against. Absent on every
    /// extension-declared model that does not say otherwise: `apply_prompt_cache_metadata` only
    /// stamps DIRECT Anthropic catalog rows, so an extension provider opts itself in by declaring
    /// this rather than by claiming an api family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache: Option<cyrup_provider::ModelPromptCache>,
}

/// A resolved provider registration: the parsed config, the resolved API key (if any), and whether
/// it carries an OAuth block (whose dynamic callbacks need guest exports — gap-08 #5).
#[derive(Clone, Debug)]
pub struct ProviderRegistration {
    pub id: String,
    pub config: ProviderConfig,
    pub resolved_api_key: Option<String>,
}

impl ProviderRegistration {
    /// Whether this provider authenticates via OAuth (vs. a static/resolved API key). The dynamic
    /// `login`/`refreshToken`/`getApiKey`/`modifyModels` callbacks are invoked across the
    /// `provider-*` exports (the host drives `/login` through [`crate::host::LiveExtension`]).
    pub fn has_oauth(&self) -> bool {
        self.config.oauth.is_some()
    }

    /// The OAuth display name (Pi `oauth.name`), if this provider carries an OAuth block.
    pub fn oauth_name(&self) -> Option<String> {
        self.config
            .oauth
            .as_ref()
            .and_then(|v| v.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_string)
    }

    /// Whether access through this provider's OAuth is backed by a SUBSCRIPTION rather than a
    /// metered API key (EXT-051) — pi's `oauth.isSubscription`
    /// (`extensions/types.ts:1475` @v0.84.1, absent at v0.83.0). The typed read of the key that
    /// used to reach the host inside the opaque `oauth` blob with nothing to read it, so an
    /// extension-supplied subscription provider was presented like a metered one. cyrup's
    /// BUILT-IN providers already model this as `AuthProvider::is_subscription`
    /// (`crates/cyrup-provider/src/auth/mod.rs:101`).
    pub fn oauth_is_subscription(&self) -> bool {
        self.config.oauth_is_subscription()
    }

    /// Whether the guest supplied a custom `streamSimple` handler (drives `provider-stream-simple`).
    pub fn has_stream_simple(&self) -> bool {
        self.config.has_stream_simple
    }

    /// Realize this registration as a concrete [`cyrup_provider::Provider`] (Pi
    /// `ModelRegistry.applyProviderConfig`, model-registry.ts:917-940): a [`cyrup_provider::ConfigProvider`]
    /// over the parsed model catalog + the resolved api key. The model registry (arch-08 §5.6) installs
    /// the returned provider so a guest `registerProvider` model becomes selectable AND streamable in
    /// the assembled run.
    ///
    /// Per-model wire fields are folded exactly as Pi does: `api = model.api || config.api`
    /// (model-registry.ts:923), `baseUrl = model.baseUrl ?? config.baseUrl` (:931), `contextWindow`
    /// defaults to 128000 and `maxTokens` to 16384 (Pi's `models.json` parse defaults,
    /// model-registry.ts:621-622). A model that resolves no `api` or `baseUrl` is skipped (Pi rejects
    /// such a registration at `validateProviderConfig`; here it is dropped rather than panicking).
    pub fn build_provider(&self) -> std::sync::Arc<dyn cyrup_provider::Provider> {
        let name = if self.config.name.is_empty() {
            self.id.clone()
        } else {
            self.config.name.clone()
        };
        cyrup_provider::ConfigProvider::new(
            self.id.clone(),
            name,
            self.resolved_api_key.clone(),
            self.build_models(),
        )
        .into_arc()
    }

    /// Parse this registration's model catalog into concrete [`cyrup_provider::Model`]s (Pi
    /// model-registry.ts:922-940). Open-shaped fields (`thinkingLevelMap`, `compat`) are deserialized
    /// from their carried JSON; a malformed block is dropped (never a panic) rather than failing the
    /// whole registration.
    pub fn build_models(&self) -> Vec<cyrup_provider::Model> {
        use cyrup_provider::{Modality, Model, ModelCost};

        let mut out: Vec<Model> = Vec::with_capacity(self.config.models.len());
        for m in &self.config.models {
            // `api = model.api || config.api` (Pi model-registry.ts:923); required.
            let Some(api) = m.api.clone().or_else(|| self.config.api.clone()) else {
                continue;
            };
            // `baseUrl = model.baseUrl ?? config.baseUrl` (Pi :931); required.
            let Some(base_url) = m.base_url.clone().or_else(|| self.config.base_url.clone()) else {
                continue;
            };
            let input: Vec<Modality> = if m.input.is_empty() {
                vec![Modality::Text]
            } else {
                m.input
                    .iter()
                    .filter_map(|s| match s.as_str() {
                        "image" => Some(Modality::Image),
                        "text" => Some(Modality::Text),
                        _ => None,
                    })
                    .collect()
            };
            let thinking_level_map = m
                .thinking_level_map
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let compat = m
                .compat
                .as_ref()
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            let headers = Self::merge_headers(&self.config.headers, &m.headers);

            out.push(Model {
                id: m.id.as_str().into(),
                name: m.name.clone().unwrap_or_else(|| m.id.clone()),
                api: api.into(),
                provider: self.id.as_str().into(),
                base_url,
                reasoning: m.reasoning,
                input,
                input_limits: m.input_limits.clone(),
                cost: ModelCost {
                    input: m.cost.input,
                    output: m.cost.output,
                    cache_read: m.cost.cache_read,
                    cache_write: m.cost.cache_write,
                    tiers: m.cost.tiers.as_ref().map(|tiers| {
                        tiers
                            .iter()
                            .map(|t| cyrup_provider::ModelCostTier {
                                input_tokens_above: t.input_tokens_above,
                                input: t.input,
                                output: t.output,
                                cache_read: t.cache_read,
                                cache_write: t.cache_write,
                            })
                            .collect()
                    }),
                },
                context_window: m.context_window.unwrap_or(128_000),
                max_tokens: m.max_tokens.unwrap_or(16_384),
                // AGENT-026 / CFG-104 — pi's `extensionModelFromDefinition` spreads the whole
                // definition onto the model (`{ ...definition, api, provider, baseUrl, headers:
                // undefined }`, `core/provider-composer.ts:297` @f1b2e77f5), so both sampling keys
                // reach `Model` unchanged; `resolve_sampling_params` merges them per request.
                sampling_params: cyrup_provider::ModelSamplingParams::new(
                    m.sampling_params.clone(),
                    m.sampling_params_by_thinking_level.clone(),
                ),
                prompt_cache: m.prompt_cache.clone(),
                thinking_level_map,
                compat,
                headers,
            });
        }
        out
    }

    /// Merge provider-level and per-model custom headers into a [`cyrup_provider::HeaderMap`] (per-model
    /// wins). Pi tracks these in `modelRequestHeaders` and injects them per request; cyrup carries them
    /// on `Model.headers` (the request header overlay), so the registered provider sends them too.
    /// Returns `None` when neither level supplies a header.
    fn merge_headers(
        provider_headers: &BTreeMap<String, String>,
        model_headers: &BTreeMap<String, String>,
    ) -> Option<cyrup_provider::HeaderMap> {
        if provider_headers.is_empty() && model_headers.is_empty() {
            return None;
        }
        let mut out: cyrup_provider::HeaderMap = std::collections::BTreeMap::new();
        for (k, v) in provider_headers.iter().chain(model_headers.iter()) {
            out.insert(k.clone(), Some(v.clone()));
        }
        Some(out)
    }
}

/// Resolve an API-key spec per Pi's rules (types.ts apiKey doc): a leading `!` runs a shell command
/// and uses its trimmed stdout; `$VAR`/`${VAR}` interpolate the environment; otherwise it is literal.
/// Returns `Ok(None)` for an absent spec. A failed `!command` surfaces an error (never a panic).
pub fn resolve_api_key(spec: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = spec else { return Ok(None) };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if let Some(cmd) = raw.strip_prefix('!') {
        // Run via the platform shell; trim the trailing newline (Pi `!command`).
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| format!("apiKey command failed to spawn: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("apiKey command exited non-zero: {}", stderr.trim()));
        }
        let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok(Some(key));
    }
    Ok(Some(interpolate_env(raw)))
}

/// Interpolate `$VAR` and `${VAR}` from the environment; unknown vars expand to empty (Pi behavior).
fn interpolate_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // `${VAR}` form.
        if chars.peek() == Some(&'{') {
            chars.next();
            let mut name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                name.push(c);
            }
            out.push_str(&std::env::var(&name).unwrap_or_default());
            continue;
        }
        // `$VAR` form (alnum + underscore).
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_alphanumeric() || c == '_' {
                name.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&std::env::var(&name).unwrap_or_default());
        }
    }
    out
}

/// The model-registry the session injects (Pi `bindCore`). The hub upserts/removes providers here;
/// the concrete impl lives in cyrup-provider/cyrup-session and is wired at runtime (arch-08 §5.6).
pub trait ModelRegistrySink: Send + Sync {
    /// Register (or replace) a provider's models (Pi `registerProvider` → ModelRegistry).
    fn upsert_provider(&self, reg: &ProviderRegistration);
    /// Register (or replace) a LIVE provider: the sink stores `provider` as given instead of
    /// realizing a [`cyrup_provider::ConfigProvider`] from a [`ProviderRegistration`]. Replacing an
    /// existing id swaps the `Arc`, so the next catalog read sees the replacement's models.
    ///
    /// Pi's `registerProvider(provider: Provider)` overload takes a finished provider object
    /// (`extensions/types.ts:1803` @v0.99.2-17) and `ModelRegistry` holds it; cyrup's JSON
    /// registration can describe only the static half (`ProviderConfig`, `:1804`), so a native that
    /// owns its own transport and catalog hands the realized provider over here.
    fn upsert_live_provider(&self, id: &str, provider: Arc<dyn cyrup_provider::Provider>);
    /// Remove a provider's models, restoring any built-ins it overrode (Pi `unregisterProvider`).
    fn remove_provider(&self, id: &str);

    /// Register (or replace) a VIRTUAL model — pi `registerVirtualModel`
    /// (`core/model-runtime.ts:947-974` @v1.0.4), reached through the same `providerActions` bag
    /// `bindCore` carries the three provider hooks in (`extensions/runner.ts:413-418`), which is
    /// why it lives on this trait rather than on one of its own.
    ///
    /// Fallible where the provider members are not, because upstream's `registerVirtualModel`
    /// THROWS: on an empty provider or id, and on an id that already names a physical model of that
    /// provider (`:955-961`). The refusal text is upstream's own, and
    /// [`crate::virtual_model::VirtualModelHub::bind`] contains it as a per-registration diagnostic
    /// exactly as both upstream flush loops do.
    ///
    /// # Errors
    ///
    /// The registry's refusal text, verbatim from upstream.
    fn upsert_virtual_model(
        &self,
        definition: &cyrup_provider::VirtualModelDefinition,
    ) -> Result<(), String>;

    /// Remove ONE virtual model — pi `unregisterVirtualModel(provider, id)`
    /// (`core/model-runtime.ts:976-982`). An absent pair is a no-op, as upstream's is, and
    /// `remove_provider` must NOT imply this one: the docs page states that
    /// `pi.unregisterProvider()` does not remove virtual models.
    fn remove_virtual_model(&self, provider: &str, id: &str);
}

/// The provider registration hub (arch-08 §5.6). Holds resolved registrations and an optional
/// injected [`ModelRegistrySink`]; registrations made before `bind` are queued and flushed at bind.
#[derive(Default)]
pub struct ProviderHub {
    registrations: Vec<ProviderRegistration>,
    /// LIVE providers (a native's own `Arc<dyn Provider>`, see [`Self::register_live`]), keyed by
    /// id. An id lives in at most one of this map and `registrations`: registering through either
    /// door replaces whatever the other held under that id (Pi "replaces all models").
    live: BTreeMap<String, Arc<dyn cyrup_provider::Provider>>,
    /// Ids registered before a sink was bound — flushed in order at [`Self::bind`].
    pending: Vec<String>,
    sink: Option<Arc<dyn ModelRegistrySink>>,
}

impl ProviderHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider from the guest's JSON config: parse → resolve api key → store, and either
    /// upsert into the bound sink or queue for the next [`Self::bind`] (Pi defer→bindCore).
    pub fn register(&mut self, id: String, config_json: &Value) -> Result<(), String> {
        let mut config: ProviderConfig =
            serde_json::from_value(config_json.clone()).map_err(|e| e.to_string())?;
        if config.name.is_empty() {
            config.name = id.clone();
        }
        let resolved_api_key = resolve_api_key(config.api_key.as_deref())?;
        let reg = ProviderRegistration {
            id: id.clone(),
            config,
            resolved_api_key,
        };

        // Replace an existing registration with the same id (Pi "replaces all models"), whichever
        // door it came through.
        self.live.remove(&id);
        self.registrations.retain(|r| r.id != id);
        self.registrations.push(reg.clone());

        match &self.sink {
            Some(sink) => sink.upsert_provider(&reg),
            None => {
                self.pending.retain(|p| p != &id);
                self.pending.push(id);
            }
        }
        Ok(())
    }

    /// Register a LIVE provider under `id`: store the `Arc` as given and either hand it to the
    /// bound sink now or queue it for the next [`Self::bind`], exactly like [`Self::register`].
    /// Re-registering an id (live or JSON) REPLACES it.
    ///
    /// Refuses a provider whose own [`cyrup_provider::Provider::id`] is not `id`: the session finds
    /// the provider that owns a model by `model.provider`, so a registry key that disagreed with the
    /// provider's identity would list models nothing could stream.
    pub fn register_live(
        &mut self,
        id: String,
        provider: Arc<dyn cyrup_provider::Provider>,
    ) -> Result<(), String> {
        if provider.id().as_str() != id {
            return Err(format!(
                "live provider registered as `{id}` identifies itself as `{}`",
                provider.id().as_str()
            ));
        }
        self.registrations.retain(|r| r.id != id);
        self.live.insert(id.clone(), Arc::clone(&provider));
        match &self.sink {
            Some(sink) => sink.upsert_live_provider(&id, provider),
            None => {
                self.pending.retain(|p| p != &id);
                self.pending.push(id);
            }
        }
        Ok(())
    }

    /// Unregister a provider (Pi `unregisterProvider`): drop it + notify the sink. Returns whether it
    /// was present.
    pub fn unregister(&mut self, id: &str) -> bool {
        let had = self.registrations.iter().any(|r| r.id == id) || self.live.contains_key(id);
        self.registrations.retain(|r| r.id != id);
        self.live.remove(id);
        self.pending.retain(|p| p != id);
        if had && let Some(sink) = &self.sink {
            sink.remove_provider(id);
        }
        had
    }

    /// Bind the model-registry sink and FLUSH the pending registrations into it (Pi `bindCore`).
    pub fn bind(&mut self, sink: Arc<dyn ModelRegistrySink>) {
        for id in std::mem::take(&mut self.pending) {
            if let Some(reg) = self.registrations.iter().find(|r| r.id == id) {
                sink.upsert_provider(reg);
            } else if let Some(provider) = self.live.get(&id) {
                sink.upsert_live_provider(&id, Arc::clone(provider));
            }
        }
        self.sink = Some(sink);
    }

    /// Whether the registry sink has been bound (Pi post-`bindCore`).
    pub fn is_bound(&self) -> bool {
        self.sink.is_some()
    }

    /// Ids still queued for the next bind (not yet flushed).
    pub fn pending_ids(&self) -> &[String] {
        &self.pending
    }

    /// All registered provider ids (Pi `getRegisteredProviders`).
    pub fn ids(&self) -> Vec<String> {
        self.registrations
            .iter()
            .map(|r| r.id.clone())
            .chain(self.live.keys().cloned())
            .collect()
    }

    /// The live provider registered under `id` ([`Self::register_live`]), if any.
    pub fn live_provider(&self, id: &str) -> Option<Arc<dyn cyrup_provider::Provider>> {
        self.live.get(id).cloned()
    }

    /// Look up a resolved registration by id.
    pub fn get(&self, id: &str) -> Option<&ProviderRegistration> {
        self.registrations.iter().find(|r| r.id == id)
    }
}
