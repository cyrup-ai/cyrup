//! Runtime model-catalog refresh — the pi.dev overlay (1:1 port of Pi
//! `packages/coding-agent/src/core/remote-catalog-provider.ts`; DRIFT-007).
//!
//! # What this is
//!
//! The 31 catalogs under `providers/catalog/*.json` are compiled in and therefore freeze at build
//! time; upstream keeps them current by *overlaying* a catalog fetched from `https://pi.dev` roughly
//! every four hours, with `ETag` revalidation and an on-disk cache. Without that, every model
//! addition/repricing needs a hand-refresh of the embedded JSON (which is exactly what commit
//! `6d29542` had to do). This module is that overlay.
//!
//! # The floor invariant — read this before changing anything here
//!
//! **The embedded catalogs are the source of truth and the FLOOR.** The overlay may add a model or
//! replace one by id; it can never remove one, empty a provider, or make a refresh failure
//! observable as "fewer models". Every branch below is written to preserve that:
//!
//! - [`merge_models`] starts from the baseline and only replaces-by-id or pushes (Pi
//!   `mergeModels`, `remote-catalog-provider.ts:8-16`). It is the whole floor guarantee, in five
//!   lines, and it is exercised directly by `tests/remote_catalog.rs`.
//! - Every store read is `.ok().flatten()`: a corrupt or unreadable cache degrades to "no overlay",
//!   never to an error and never to a smaller catalog.
//! - A `304 Not Modified` can only be *asked for* when a cached body backs the validator, so it can
//!   never leave the overlay empty (Pi's own load-bearing comment, `:68-69`).
//! - A transport failure, a non-OK status, an unparseable body, a disabled/offline run and a
//!   missing agent dir all end with the built-in catalogs unchanged.
//!
//! # Where the merge happens (a deliberate divergence from Pi's shape)
//!
//! Pi wraps a provider so `getModels()` merges a mutable `dynamicModels` field on every call. Here
//! [`crate::provider::Provider::models`] returns a BORROWED slice (`provider.rs:21`), so a per-call
//! merge is not expressible without changing the trait. Instead the merge happens ONCE, at provider
//! construction, inside [`RemoteCatalogProvider`]. That is behaviourally equivalent because cyrup
//! keeps no long-lived registry: `full_model_registry()` rebuilds via `default_models(..)` on every
//! read and `compose_provider_registry` rebuilds `all_providers_with(..)` on every call, so a
//! background refresh that writes the store is picked up by the next registry read — with no live
//! mutation, no `set_provider` race, and no lock on the hot read path.
//!
//! # Cadence and trigger (Pi's own policy — do not invent a different one)
//!
//! - Freshness window: [`REMOTE_CATALOG_REFRESH_INTERVAL_MS`] = 4h (Pi `:6`).
//! - Network at construction time is OFF by default upstream (Pi commit `c889eb88` removed it from
//!   `ModelRuntime.create`); the running modes fire a fire-and-forget refresh afterwards
//!   (`main.ts:863-866`, `interactive-mode.ts:826-836`). [`RefreshOptions::CACHE_ONLY`] is the
//!   startup mode; [`RefreshOptions::network`] is the background one.
//! - `pi update --models` forces one (`allowNetwork: true, force: true`) under a 15s timeout, which
//!   is [`RemoteCatalog::with_request_timeout`]'s default.

use crate::auth::{EnvAuthContext, ProviderAuth};
use crate::classifier::{AnyModel, ClassifierModel, ImageModel, ModelType};
use crate::context::Context;
use crate::error::ProviderError;
use crate::images::{AssistantImages, ImagesContext, ImagesOptions};
use crate::model::Model;
use crate::models_store::{ModelsStore, ModelsStoreEntry};
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions};
use crate::utils::http_date::parse_http_date_ms;
use crate::utils::refresh::RefreshDedup;
use crate::utils::simple_options::SimpleStreamOptions;
use cyrup_core::{EventStream, ProviderId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Pi `DEFAULT_CATALOG_BASE_URL` (`remote-catalog-provider.ts:5`). Upstream exposes this as a
/// constructor OPTION only — there is no env var and no settings key for it anywhere in pi — and
/// cyrup keeps it that way: [`RemoteCatalog::with_base_url`] is both the production override and the
/// seam tests point at a loopback listener, since the workspace has no injectable HTTP transport.
pub const DEFAULT_CATALOG_BASE_URL: &str = "https://pi.dev";

/// Pi `REMOTE_CATALOG_REFRESH_INTERVAL_MS` (`remote-catalog-provider.ts:6`) — four hours.
pub const REMOTE_CATALOG_REFRESH_INTERVAL_MS: i64 = 4 * 60 * 60 * 1000;

/// The model types this client can consume, sent as `?types=` (1:1 port of pi
/// `REMOTE_CATALOG_MODEL_TYPES`, `remote-catalog-provider.ts:22`, new at v1.0.0 — PROV-128).
///
/// Upstream's own comment states the contract: "Sent as `?types=` so the catalog server returns the
/// full-type shard instead of the chat-only one served to clients that predate model types. A
/// server that ignores the parameter still returns the chat-only shard, which this client handles
/// unchanged." (`:17-21`.)
///
/// **The two shards are different JSON shapes, and that is the trap.** Measured against the live
/// endpoint on 2026-10-05: `/api/models/providers/openrouter` answers a JSON OBJECT keyed by model
/// id with 400 rows, all `chat`; the same path with `?types=chat,image,classifier` answers a JSON
/// ARRAY of 469 (400 chat, 59 image, 10 classifier). [`parse_catalog`] accepts both, because pi's
/// `parseCatalog` does (`:37-51`) and cyrup's port always has — see its own docs. Sending the
/// parameter without that is what breaks the parse on day one.
pub const REMOTE_CATALOG_MODEL_TYPES: [ModelType; 3] = ModelType::ALL;

/// `REMOTE_CATALOG_MODEL_TYPES.join(",")` (pi `:102`) — `chat,image,classifier`.
fn remote_catalog_types_param() -> String {
    REMOTE_CATALOG_MODEL_TYPES
        .iter()
        .map(|t| t.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Default per-request timeout. Pi has no timeout on the background refresh itself and instead
/// aborts at the caller (15s in `package-manager-cli.ts:397-420` and `model-selector.ts:162-185`);
/// cyrup applies the same budget per request so a hung origin can never pin a background task.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Milliseconds since the Unix epoch (Pi `Date.now()`). A clock before the epoch degrades to `0`,
/// which only costs one extra revalidation.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `cyrup/<version> (<os>; rust; <arch>)` — the shape of Pi `getPiUserAgent`
/// (`utils/pi-user-agent.ts`), rebranded like every other `PI_*`/`pi` surface.
pub fn cyrup_user_agent() -> String {
    format!(
        "cyrup/{} ({}; rust; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Percent-encode a path segment (Pi `encodeURIComponent(provider.id)`). Built-in provider ids are
/// already unreserved, so this is identity for every real input; it exists so a `models.json`-declared
/// id with a `/` can never escape the route.
fn encode_path_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(char::from(byte)),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

// ------------------------------------------------------------------------------- pure functions --

/// Overlay `dynamic` onto `baseline`, matching by model id (1:1 port of Pi `mergeModels`,
/// `remote-catalog-provider.ts:8-16`).
///
/// Order-preserving: a remote model with a known id REPLACES that entry in place; an unknown id is
/// appended. **A baseline model is never removed** — that is DRIFT-007's floor invariant, and it is
/// why an empty or garbage `dynamic` is indistinguishable from no overlay at all.
pub fn merge_models(baseline: &[Model], dynamic: &[Model]) -> Vec<Model> {
    let mut merged = baseline.to_vec();
    for model in dynamic {
        match merged.iter().position(|entry| entry.id == model.id) {
            Some(index) => {
                if let Some(slot) = merged.get_mut(index) {
                    *slot = model.clone();
                }
            }
            None => merged.push(model.clone()),
        }
    }
    merged
}

/// Parse a remote catalog body (1:1 port of Pi `parseCatalog`, `remote-catalog-provider.ts:37-51`
/// @v1.0.1).
///
/// Three shapes are accepted, exactly as upstream: a JSON array, `{"models": [...]}`, or a plain
/// object whose VALUES are models. All three are live: pi.dev serves the **model-id-keyed object**
/// to a client that sends no `?types=`, and a **JSON array** to one that does
/// ([`REMOTE_CATALOG_MODEL_TYPES`]). Anything else is an error carrying Pi's message verbatim.
///
/// `provider` is forced onto every entry so a mislabelled body can never inject models into another
/// provider's catalog.
///
/// Rows of every type come back (pi returns `AnyModel[]`, `:40`). PROV-128 — before the `?types=`
/// parameter this returned `Vec<Model>` and an image or classifier row was silently discarded as a
/// failed chat parse; the distinction only became observable once the rows started arriving.
///
/// A row whose `type` this build does not know is DROPPED, not an error — pi's
/// `.filter(isSupportedModelType)` (`:49`, defined `:24-30`), which also keeps a row with no
/// `type` at all (a legacy chat row). The drop has to happen before deserialization because
/// [`AnyModel`]'s own deserializer rejects an unknown `type` outright; upstream's filter sits in
/// the same position for the same reason.
///
/// **[CYRUP-DELTA]** Pi filters to `"id" in entry` and spreads whatever else is there, because its
/// models are structural. cyrup's are nominal, so an entry that carries an `id` but does not
/// deserialize is DROPPED rather than fabricated with invented defaults. Dropping is the
/// floor-preserving choice: the baseline entry for that id simply survives unmerged.
pub fn parse_catalog(
    provider_id: &str,
    value: &serde_json::Value,
) -> Result<Vec<AnyModel>, ProviderError> {
    let entries: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => match map.get("models") {
            Some(serde_json::Value::Array(items)) => items.iter().collect(),
            _ => map.values().collect(),
        },
        _ => {
            return Err(ProviderError::Decode(format!(
                "Invalid model catalog for provider \"{provider_id}\""
            )));
        }
    };

    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            object.get("id")?;
            if !is_supported_model_type(object) {
                return None;
            }
            let mut owned = object.clone();
            owned.insert(
                "provider".to_string(),
                serde_json::Value::String(provider_id.to_string()),
            );
            let parsed =
                serde_json::from_value::<AnyModel>(serde_json::Value::Object(owned)).ok()?;
            // SEAM-131 — the same load-time fill the EMBEDDED catalogs get
            // (`catalog::load_catalog`). An overlay row never passes through that function:
            // `merge_models` replaces a baseline entry wholesale by id, so a refreshed
            // `anthropic` row that omits `promptCache` would arrive with `prompt_cache: None`,
            // `prompt_cache_ttl_ms` would answer `None`, and every session on that model would
            // report `Inactive (cache lifetime unavailable)` — the whole feature off, with no
            // other symptom. The pass only ever fills a key the row left unset, so an overlay
            // that does carry `promptCache` is untouched, and it is gated on direct Anthropic
            // exactly as the embedded pass is, so no gateway row gains an invented lifetime.
            //
            // PROV-134 — `apply_image_input_metadata` is applied here for the same reason, and on
            // all THREE variants rather than just chat, because upstream's `inputLimits` is on
            // `BaseModel` and its generator stamps `AnyModel` (`generate-models.ts:3487` and
            // `:3505`). The live pi.dev body is already stamped — measured on
            // `tests/fixtures/pi-dev-openrouter-all-types.json`, every image-capable row carries
            // `inputLimits` (270/270 chat, 57/57 image) and no text-only row does — so this is a
            // fill-only no-op against today's endpoint and a floor if a future shard regresses.
            Some(match parsed {
                AnyModel::Chat(mut model) => {
                    crate::catalog::apply_prompt_cache_metadata(&mut model);
                    crate::catalog::apply_image_input_metadata(&mut model);
                    AnyModel::Chat(model)
                }
                AnyModel::Image(mut model) => {
                    crate::catalog::apply_image_input_metadata_to_image(&mut model);
                    AnyModel::Image(model)
                }
                AnyModel::Classifier(mut model) => {
                    crate::catalog::apply_image_input_metadata_to_classifier(&mut model);
                    AnyModel::Classifier(model)
                }
            })
        })
        .collect())
}

/// pi `isSupportedModelType` (`remote-catalog-provider.ts:24-30`): an absent `type` is a chat row
/// and is kept; a string `type` is kept only when [`REMOTE_CATALOG_MODEL_TYPES`] names it. A
/// `type` that is present but not a string is a malformed row and is dropped.
fn is_supported_model_type(entry: &serde_json::Map<String, serde_json::Value>) -> bool {
    match entry.get("type") {
        None => true,
        Some(serde_json::Value::String(kind)) => REMOTE_CATALOG_MODEL_TYPES
            .iter()
            .any(|known| known.as_str() == kind),
        Some(_) => false,
    }
}

/// Split a parsed catalog into its three typed lists, preserving each type's relative order.
///
/// This is where the port stops carrying `AnyModel` and starts carrying the three concrete types,
/// and it is deliberate rather than incidental: downstream of here a chat caller holds `&[Model]`
/// and CANNOT be handed an image row, so "chat only" is a fact the compiler keeps instead of a
/// `filter(isModelType(m, "chat"))` that a later edit can forget. pi needs that filter at
/// `models.ts:1084` and `remote-catalog-provider.ts:73` precisely because its one array is
/// structurally typed; see `docs/RUST-DESIGN-REVIEW.md` on explicit domain enums and on stating the
/// exact guarantee (what remains possible: a row can still be the WRONG chat model — splitting by
/// type says nothing about a row's contents).
pub fn split_by_type(models: Vec<AnyModel>) -> (Vec<Model>, Vec<ImageModel>, Vec<ClassifierModel>) {
    let mut chat = Vec::new();
    let mut images = Vec::new();
    let mut classifiers = Vec::new();
    for model in models {
        match model {
            AnyModel::Chat(m) => chat.push(m),
            AnyModel::Image(m) => images.push(m),
            AnyModel::Classifier(m) => classifiers.push(m),
        }
    }
    (chat, images, classifiers)
}

/// [`merge_models`] for image rows: pi keys its merge map by `${getModelType(model)}\0${model.id}`
/// (`remote-catalog-provider.ts:33-35`), so ids are matched WITHIN a type. Order-preserving and
/// never-shrinking, exactly like the chat merge.
pub fn merge_image_models(baseline: &[ImageModel], dynamic: &[ImageModel]) -> Vec<ImageModel> {
    let mut merged = baseline.to_vec();
    for model in dynamic {
        match merged.iter().position(|entry| entry.id == model.id) {
            Some(index) => {
                if let Some(slot) = merged.get_mut(index) {
                    *slot = model.clone();
                }
            }
            None => merged.push(model.clone()),
        }
    }
    merged
}

/// [`merge_models`] for classifier rows; see [`merge_image_models`].
pub fn merge_classifier_models(
    baseline: &[ClassifierModel],
    dynamic: &[ClassifierModel],
) -> Vec<ClassifierModel> {
    let mut merged = baseline.to_vec();
    for model in dynamic {
        match merged.iter().position(|entry| entry.id == model.id) {
            Some(index) => {
                if let Some(slot) = merged.get_mut(index) {
                    *slot = model.clone();
                }
            }
            None => merged.push(model.clone()),
        }
    }
    merged
}

/// The overlay a stored entry contributes, after the staleness guard (1:1 port of Pi `remoteModels`,
/// `remote-catalog-provider.ts:32-40`).
///
/// A persisted overlay older than (or equal in age to) the built-in catalogs is DISCARDED WHOLE.
/// This is the fix for "persisted remote model catalogs overriding newer bundled catalogs after an
/// upgrade" (pi #7016) — without it, upgrading cyrup would silently keep serving the pre-upgrade
/// pi.dev snapshot on top of freshly refreshed embedded data.
pub fn remote_models(
    entry: Option<&ModelsStoreEntry>,
    local_generated_at: Option<i64>,
) -> &[Model] {
    match entry.filter(|entry| stored_entry_is_newer(entry, local_generated_at)) {
        Some(entry) => &entry.models,
        None => &[],
    }
}

/// The staleness guard of [`remote_models`] as a predicate, so the image and classifier channels
/// of the same persisted entry are gated on the SAME decision rather than a second copy of it
/// (pi has one `remoteModels`, over one array).
pub fn stored_entry_is_newer(entry: &ModelsStoreEntry, local_generated_at: Option<i64>) -> bool {
    match local_generated_at {
        Some(local) => entry.last_modified.is_some_and(|remote| remote > local),
        None => true,
    }
}

// ------------------------------------------------------------------------------- the overlay ----

/// One provider's overlay rows, split by model type (PROV-128).
///
/// The split is the point: the chat accessor returns `&[Model]`, so no caller on a chat path can be
/// handed an image row even by mistake. pi keeps one `AnyModel[]` and re-filters it at every read
/// (`remote-catalog-provider.ts:73`, `models.ts:1084`); the filter is what this type makes
/// unnecessary.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderOverlay {
    chat: Vec<Model>,
    images: Vec<ImageModel>,
    classifiers: Vec<ClassifierModel>,
}

impl ProviderOverlay {
    /// Split a parsed catalog (pi's `AnyModel[]`) into the three typed lists.
    pub fn from_any(models: Vec<AnyModel>) -> Self {
        let (chat, images, classifiers) = split_by_type(models);
        Self {
            chat,
            images,
            classifiers,
        }
    }

    pub fn from_chat(chat: Vec<Model>) -> Self {
        Self {
            chat,
            images: Vec::new(),
            classifiers: Vec::new(),
        }
    }

    pub fn from_parts(
        chat: Vec<Model>,
        images: Vec<ImageModel>,
        classifiers: Vec<ClassifierModel>,
    ) -> Self {
        Self {
            chat,
            images,
            classifiers,
        }
    }

    pub fn chat(&self) -> &[Model] {
        &self.chat
    }

    pub fn images(&self) -> &[ImageModel] {
        &self.images
    }

    pub fn classifiers(&self) -> &[ClassifierModel] {
        &self.classifiers
    }

    /// `true` when this provider's overlay contributes nothing of ANY type. An overlay that
    /// contributes nothing must be indistinguishable from no overlay.
    pub fn is_empty(&self) -> bool {
        self.chat.is_empty() && self.images.is_empty() && self.classifiers.is_empty()
    }

    /// Keep only the rows that label themselves as belonging to `provider_id` — pi's
    /// `.filter(model => model.provider === provider.id)` (`:59`): a body that mislabels its
    /// provider must not leak into another provider's catalog.
    #[must_use]
    pub fn retain_provider(mut self, provider_id: &str) -> Self {
        self.chat.retain(|m| m.provider.as_str() == provider_id);
        self.images.retain(|m| m.provider.as_str() == provider_id);
        self.classifiers
            .retain(|m| m.provider.as_str() == provider_id);
        self
    }
}

/// A loaded, already-staleness-checked overlay: provider id → the models that provider's remote
/// catalog contributes, split by type. Cheap to clone behind an `Arc` and immutable once built, so
/// the sync, hot registry reads never touch the disk or take a lock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogOverlay {
    by_provider: BTreeMap<String, ProviderOverlay>,
}

impl CatalogOverlay {
    /// Build from already-parsed per-provider CHAT model lists. Entries whose models are empty are
    /// dropped — an empty overlay and no overlay must be indistinguishable.
    pub fn from_entries(entries: impl IntoIterator<Item = (String, Vec<Model>)>) -> Self {
        Self::from_overlays(
            entries
                .into_iter()
                .map(|(id, models)| (id, ProviderOverlay::from_chat(models))),
        )
    }

    /// Build from per-provider lists of every type (PROV-128).
    pub fn from_overlays(entries: impl IntoIterator<Item = (String, ProviderOverlay)>) -> Self {
        Self {
            by_provider: entries
                .into_iter()
                .filter(|(_, overlay)| !overlay.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_provider.is_empty()
    }

    /// The provider ids this overlay contributes to.
    pub fn provider_ids(&self) -> impl Iterator<Item = &str> {
        self.by_provider.keys().map(String::as_str)
    }

    /// Everything this overlay contributes for one provider.
    pub fn overlay_for(&self, provider_id: &str) -> Option<&ProviderOverlay> {
        self.by_provider.get(provider_id)
    }

    /// The remote CHAT models for one provider (empty when this provider has no overlay).
    pub fn models_for(&self, provider_id: &str) -> &[Model] {
        self.by_provider
            .get(provider_id)
            .map_or(&[][..], ProviderOverlay::chat)
    }

    /// The remote image models for one provider (PROV-128).
    pub fn image_models_for(&self, provider_id: &str) -> &[ImageModel] {
        self.by_provider
            .get(provider_id)
            .map_or(&[][..], ProviderOverlay::images)
    }

    /// The remote classifier models for one provider (PROV-128).
    pub fn classifier_models_for(&self, provider_id: &str) -> &[ClassifierModel] {
        self.by_provider
            .get(provider_id)
            .map_or(&[][..], ProviderOverlay::classifiers)
    }

    /// `baseline` with this provider's overlay merged over it (see [`merge_models`] — never shrinks).
    pub fn merged_for(&self, provider_id: &str, baseline: &[Model]) -> Vec<Model> {
        merge_models(baseline, self.models_for(provider_id))
    }

    /// Wrap `provider` so its catalog reads come back merged (Pi `withRemoteCatalog`,
    /// `remote-catalog-provider.ts:61-158`). Returns `provider` UNCHANGED when this overlay has
    /// nothing for it, so the no-overlay path allocates nothing and behaves bit-identically to today.
    pub fn apply(&self, provider: Arc<dyn Provider>) -> Arc<dyn Provider> {
        match self.by_provider.get(provider.id().as_str()) {
            Some(overlay) if !overlay.is_empty() => {
                Arc::new(RemoteCatalogProvider::new(provider, overlay))
            }
            _ => provider,
        }
    }
}

/// A [`Provider`] decorator whose catalog is `inner`'s with a remote overlay merged in. Every other
/// behavior — id, display name, base URL, headers, credential filter, auth, streaming, dynamic
/// refresh — delegates untouched.
///
/// # PROV-M01 — why every surface method is named here, including the ones with trait defaults
///
/// Upstream `withRemoteCatalog` is an OBJECT SPREAD:
/// `return { ...provider, getModels: …, refreshModels: … }`
/// (`packages/coding-agent/src/core/remote-catalog-provider.ts:52-54` @v0.83.0). Every other member
/// of `Provider` — `name`, `baseUrl?`, `headers?`, `filterModels?`, `auth`, `stream`, `streamSimple`
/// (`packages/ai/src/models.ts:76-119` @v0.83.0) — survives BY CONSTRUCTION. Rust has no spread, so
/// a hand-written delegating impl forwards exactly the methods it names, and the four members whose
/// cyrup counterparts carry a TRAIT DEFAULT fail silently when omitted: the decorator inherits the
/// default and returns a plausible answer rather than the inner's.
///
/// That was not hypothetical here. Before this delegation was written out, `filter_models` was
/// dropped — and `github-copilot` is the one built-in that installs one
/// ([`crate::providers::github_copilot::filter_github_copilot_models`], via
/// `WireProvider::with_filter_models`, `providers/github_copilot.rs:178`). `all_providers_with_overlay`
/// maps EVERY built-in through [`CatalogOverlay::apply`] (`providers/all.rs:148-157`), so in the
/// overlay configuration `Models::get_available` (`collection.rs:419`) called `filter_models` on the
/// decorator, got the identity default back, and offered the user every Copilot model regardless of
/// what the OAuth credential's `availableModelIds` actually entitled. `name`/`base_url`/`headers`
/// were dropped the same way: a wrapped `WireProvider` overrides all three (`wire.rs:113-123`), and
/// the decorator reported the id as the display name and `None` for both provider-level defaults.
///
/// `get_model` is deliberately NOT delegated: its default derives from `models()`, which this type
/// overrides, so the default resolves against the MERGED catalog — which is what upstream's
/// `Models` does against the spread's `getModels`. Delegating it to `inner` would be the bug.
pub struct RemoteCatalogProvider {
    inner: Arc<dyn Provider>,
    models: Vec<Model>,
    /// The inner provider's non-chat rows with this overlay merged over them, computed once at
    /// construction for the same reason `models` is (see the module doc). PROV-128.
    image_models: Vec<ImageModel>,
    classifier_models: Vec<ClassifierModel>,
}

impl RemoteCatalogProvider {
    pub fn new(inner: Arc<dyn Provider>, overlay: &ProviderOverlay) -> Self {
        let models = merge_models(inner.models(), overlay.chat());
        // The baseline for the non-chat types is the inner provider's own `get_all_models()`, the
        // same way the chat baseline is its `models()` (pi merges over
        // `provider.getAllModels?.() ?? provider.getModels()`, `remote-catalog-provider.ts:75`).
        let inner_all = inner.get_all_models();
        let baseline_images: Vec<ImageModel> = inner_all
            .iter()
            .filter_map(AnyModel::as_image)
            .cloned()
            .collect();
        let baseline_classifiers: Vec<ClassifierModel> = inner_all
            .iter()
            .filter_map(AnyModel::as_classifier)
            .cloned()
            .collect();
        Self {
            inner,
            models,
            image_models: merge_image_models(&baseline_images, overlay.images()),
            classifier_models: merge_classifier_models(
                &baseline_classifiers,
                overlay.classifiers(),
            ),
        }
    }

    /// The undecorated provider (its catalog is the built-in floor).
    pub fn inner(&self) -> &Arc<dyn Provider> {
        &self.inner
    }
}

#[async_trait::async_trait]
impl Provider for RemoteCatalogProvider {
    fn id(&self) -> &ProviderId {
        self.inner.id()
    }

    /// PROV-M01 — carried by `...provider`. The trait default is `self.id().as_str()`, so omitting
    /// this silently renamed every overlaid provider to its machine id.
    fn name(&self) -> &str {
        self.inner.name()
    }

    /// PROV-M01 — carried by `...provider`. The trait default is `None`.
    fn base_url(&self) -> Option<&str> {
        self.inner.base_url()
    }

    /// PROV-M01 — carried by `...provider`. The trait default is `None`.
    fn headers(&self) -> Option<&crate::HeaderMap> {
        self.inner.headers()
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    /// PROV-M01 + PROV-128 — carried by `...provider` upstream, and the ONE member whose omission
    /// was a live capability loss rather than a latent one once a built-in gained image rows: the
    /// trait default rebuilds the list from `models()`, which is chat-only, so an overlaid
    /// `openrouter` would have reported its 55 image rows as absent for as long as an overlay
    /// existed — and `supports_image_generation`'s default, which reads this, would have gone with
    /// them. Upstream's own `getAllModels` merges over
    /// `provider.getAllModels?.() ?? provider.getModels()` (`remote-catalog-provider.ts:75`).
    fn get_all_models(&self) -> Vec<AnyModel> {
        let mut all: Vec<AnyModel> = Vec::with_capacity(
            self.models.len() + self.image_models.len() + self.classifier_models.len(),
        );
        all.extend(self.models.iter().cloned().map(AnyModel::Chat));
        all.extend(self.image_models.iter().cloned().map(AnyModel::Image));
        all.extend(
            self.classifier_models
                .iter()
                .cloned()
                .map(AnyModel::Classifier),
        );
        all
    }

    /// PROV-M01 — carried by `...provider`. The trait default is the ABSENT-member error envelope,
    /// so omitting this would make an overlaid provider report "does not support image generation"
    /// for a model its inner provider can generate. PROV-128.
    async fn generate_images(
        &self,
        model: &ImageModel,
        context: &ImagesContext,
        options: &ImagesOptions,
    ) -> AssistantImages {
        self.inner.generate_images(model, context, options).await
    }

    /// PROV-M01 — forwarded with [`Provider::generate_images`]: the default answers from
    /// `get_all_models()`, which for this decorator includes OVERLAY image rows the inner provider
    /// may have no implementation for. The inner provider's own answer is the truthful one (pi's
    /// `generateImages` presence travels through the spread with its implementation). PROV-128.
    fn supports_image_generation(&self) -> bool {
        self.inner.supports_image_generation()
    }

    /// PROV-M01 — carried by `...provider`. Same shape as [`Provider::generate_images`]: the trait
    /// default is the absent-member error result, so an overlaid provider with classifier models
    /// would have reported "does not support classification". Latent before PROV-128 (no built-in
    /// ships classifier rows and extension providers are not overlaid) and fixed here with the
    /// image leg, because both legs are lost by the same omission.
    async fn classify(
        &self,
        model: &crate::classifier::ClassifierModel,
        context: &crate::classifier::ClassifierContext,
        options: &crate::classifier::ClassifierOptions,
    ) -> crate::classifier::ClassifierResult {
        self.inner.classify(model, context, options).await
    }

    /// PROV-M01 — forwarded with [`Provider::classify`], for the reason
    /// [`Provider::supports_image_generation`]'s forward gives.
    fn supports_classification(&self) -> bool {
        self.inner.supports_classification()
    }

    /// PROV-M01 — carried by `...provider`. The trait default returns the catalog UNCHANGED, which
    /// is indistinguishable from a working filter for every provider that installs none — so the
    /// one provider that does install one (`github-copilot`) lost its credential-scoped narrowing
    /// with no test able to see it.
    ///
    /// The models handed on are the caller's slice, not `self.models`: `Models::get_available`
    /// passes the catalog it already resolved (`collection.rs:419`), exactly as pi passes
    /// `getModels()`' result to `filterModels` at `models.ts:407`.
    fn filter_models(
        &self,
        models: &[Model],
        credential: Option<&crate::auth::Credential>,
    ) -> Vec<Model> {
        self.inner.filter_models(models, credential)
    }

    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.inner.provider_auth()
    }

    /// SEAM-142 — forwarded with [`Provider::refresh_models`], so the wrapper is as dynamic as the
    /// provider it overlays (pi's spread carries `refreshModels` or its absence through).
    fn has_refresh_models(&self) -> bool {
        self.inner.has_refresh_models()
    }

    /// PROV-S05 — the context is forwarded UNCHANGED. pi's spread carries `refreshModels` through
    /// with its argument intact (`remote-catalog-provider.ts:54` @v0.83.0); dropping `allow_network`,
    /// `force` or the abort token here would make an overlaid provider silently un-cancellable.
    async fn refresh_models(
        &self,
        ctx: &crate::provider::RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        self.inner.refresh_models(ctx).await
    }

    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream(model, context, options)
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        self.inner.stream_simple(model, context, options)
    }
}

// ------------------------------------------------------------------------------- the refresher ---

/// One refresh's network posture (Pi `RefreshModelsContext.allowNetwork`/`force`,
/// `models.ts:34-44`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefreshOptions {
    /// `false` during offline / cache-only initialization. When false the store is still READ (so
    /// the persisted overlay is restored) but no request is ever issued.
    pub allow_network: bool,
    /// Bypass the 4h freshness window and fetch immediately (`cyrup update --models`).
    pub force: bool,
}

impl RefreshOptions {
    /// Restore the persisted overlay without touching the network — the startup posture (Pi
    /// `agent-session-services.ts:180`, `refresh({ allowNetwork: false })`).
    pub const CACHE_ONLY: Self = Self {
        allow_network: false,
        force: false,
    };

    /// Background refresh: network allowed, 4h freshness window respected.
    pub const fn network() -> Self {
        Self {
            allow_network: true,
            force: false,
        }
    }

    /// Forced refresh: network allowed, freshness window bypassed.
    pub const fn forced() -> Self {
        Self {
            allow_network: true,
            force: true,
        }
    }
}

/// The remote-catalog client: fetches, revalidates and persists per-provider catalogs, and loads the
/// persisted result back as a [`CatalogOverlay`].
///
/// Concurrent refreshes of the SAME provider collapse onto one in-flight fetch via
/// [`RefreshDedup`] — Pi's `inflightRefresh ??= …` memo, cleared in its `finally`
/// (`remote-catalog-provider.ts:50,56,117` @v0.83.0; the declaration is `:50`, not `:47`).
pub struct RemoteCatalog {
    base_url: String,
    store: Arc<dyn ModelsStore>,
    local_generated_at: Option<i64>,
    /// Per-provider staleness floors (XAI_1), keyed by provider id. Consulted by [`load_overlay`]
    /// in preference to `local_generated_at` when a provider has an entry; empty by default, which
    /// is exactly [`with_local_generated_at`]'s pre-existing behavior for every provider.
    ///
    /// [`load_overlay`]: Self::load_overlay
    /// [`with_local_generated_at`]: Self::with_local_generated_at
    local_generated_at_by_provider: BTreeMap<String, i64>,
    request_timeout: Duration,
    auth_ctx: Arc<dyn crate::auth::AuthContext>,
    inflight: Mutex<BTreeMap<String, Arc<RefreshDedup>>>,
}

impl RemoteCatalog {
    pub fn new(store: Arc<dyn ModelsStore>) -> Self {
        Self {
            base_url: DEFAULT_CATALOG_BASE_URL.to_string(),
            store,
            local_generated_at: None,
            local_generated_at_by_provider: BTreeMap::new(),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            auth_ctx: Arc::new(EnvAuthContext),
            inflight: Mutex::new(BTreeMap::new()),
        }
    }

    /// Override the ambient environment used for `HTTP(S)_PROXY`/`NO_PROXY` resolution (same seam as
    /// [`crate::wire::WireProvider::with_auth_context`]). Production keeps the default
    /// [`EnvAuthContext`]; tests inject an empty context so an ambient proxy on the developer's
    /// machine cannot silently reroute a loopback request.
    #[must_use]
    pub fn with_auth_context(mut self, ctx: Arc<dyn crate::auth::AuthContext>) -> Self {
        self.auth_ctx = ctx;
        self
    }

    /// Point the client at a different origin (Pi's `catalogBaseUrl` option). This is the ONLY
    /// injection seam for the transport, which is why the tests drive it against a loopback
    /// listener rather than mocking `reqwest`.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The generation timestamp of the compiled-in catalogs, in epoch ms
    /// (`crate::providers::builtin_model_data_generated_at`). Feeds the staleness guard in
    /// [`remote_models`]; `None` disables the guard, exactly as Pi's `undefined` does.
    #[must_use]
    pub fn with_local_generated_at(mut self, generated_at_ms: Option<i64>) -> Self {
        self.local_generated_at = generated_at_ms;
        self
    }

    /// Per-provider staleness floors (XAI_1): `xai`'s embedded rows are fetched live at generation
    /// time, so its floor is NEWER than the global [`with_local_generated_at`] stamp — without
    /// this, [`load_overlay`] would accept a persisted overlay older than the embedded rows and let
    /// it shadow them (the exact failure pi #7016 fixed, reintroduced here specifically for xai).
    /// Defaults to empty, which is exactly [`with_local_generated_at`]'s pre-existing behavior for
    /// every provider that has no entry in `map`.
    ///
    /// [`with_local_generated_at`]: Self::with_local_generated_at
    /// [`load_overlay`]: Self::load_overlay
    #[must_use]
    pub fn with_local_generated_at_by_provider(mut self, map: BTreeMap<String, i64>) -> Self {
        self.local_generated_at_by_provider = map;
        self
    }

    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Load the persisted overlay for `provider_ids` WITHOUT any network access.
    ///
    /// Infallible by construction: an unreadable store, a stale entry and a provider-mismatched body
    /// all contribute nothing. This is the call that belongs on the startup path — it is a bounded
    /// number of small reads from one already-open JSON file, never a request.
    pub async fn load_overlay(&self, provider_ids: &[&str]) -> CatalogOverlay {
        let mut entries: Vec<(String, ProviderOverlay)> = Vec::new();
        for id in provider_ids {
            let stored = self.store.read(id, None).await.ok().flatten();
            // Per-provider floor when one exists (XAI_1), else the global stamp — never the other
            // way round, so a provider with no per-provider entry keeps today's behavior exactly.
            let floor = self
                .local_generated_at_by_provider
                .get(*id)
                .copied()
                .or(self.local_generated_at);
            let chat: Vec<Model> = remote_models(stored.as_ref(), floor).to_vec();
            // PROV-128 — the image and classifier halves of the same persisted entry, gated on the
            // SAME staleness decision (`stored_entry_is_newer`) so a stale entry cannot contribute
            // non-chat rows after its chat rows were discarded. Both reads degrade to empty: a
            // store that does not persist them is pi's pre-v1.0.0 file, not an error.
            let fresh = stored
                .as_ref()
                .is_some_and(|entry| stored_entry_is_newer(entry, floor));
            let (images, classifiers) = if fresh {
                (
                    self.store
                        .read_image_models(id, None)
                        .await
                        .unwrap_or_default(),
                    self.store
                        .read_classifier_models(id, None)
                        .await
                        .unwrap_or_default(),
                )
            } else {
                (Vec::new(), Vec::new())
            };
            // Pi filters the overlay to `model.provider === provider.id` (`:59`): a body that
            // mislabels its provider must not leak into another provider's catalog.
            let overlay =
                ProviderOverlay::from_parts(chat, images, classifiers).retain_provider(id);
            if !overlay.is_empty() {
                entries.push(((*id).to_string(), overlay));
            }
        }
        CatalogOverlay::from_overlays(entries)
    }

    /// Refresh one provider's catalog, deduplicated against any concurrent call for the same id.
    ///
    /// Returns `Ok(())` for every outcome that leaves a usable overlay — including "offline",
    /// "still fresh", `304`, and the `404`/`501` route-unimplemented case. Only a transport failure,
    /// an unexpected non-OK status or an unparseable body is an `Err`, and in each of those the
    /// persisted body is left intact so the built-in floor plus the previous overlay both survive.
    pub async fn refresh_provider(
        self: &Arc<Self>,
        provider_id: &str,
        options: RefreshOptions,
    ) -> Result<(), ProviderError> {
        let dedup = self.dedup_for(provider_id);
        let this = Arc::clone(self);
        let id = provider_id.to_string();
        dedup
            .run(move || async move { this.refresh_once(&id, options).await })
            .await
    }

    /// Refresh several providers concurrently, best-effort (Pi `Models.refresh`'s `Promise.all` +
    /// per-provider error map, `models.ts:276-327`). Never returns `Err`: failures are COLLECTED and
    /// reported to the caller, which is what lets the trigger sites be fire-and-forget.
    pub async fn refresh_providers(
        self: &Arc<Self>,
        provider_ids: &[&str],
        options: RefreshOptions,
    ) -> Vec<(String, ProviderError)> {
        let futures = provider_ids.iter().map(|id| {
            let this = Arc::clone(self);
            let id = (*id).to_string();
            async move {
                let result = this.refresh_provider(&id, options).await;
                result.err().map(|e| (id, e))
            }
        });
        futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    fn dedup_for(&self, provider_id: &str) -> Arc<RefreshDedup> {
        // A poisoned mutex simply yields a fresh, unshared memo: the fetch still runs, just
        // un-deduplicated (never a panic).
        match self.inflight.lock() {
            Ok(mut map) => Arc::clone(
                map.entry(provider_id.to_string())
                    .or_insert_with(|| Arc::new(RefreshDedup::new())),
            ),
            Err(_) => Arc::new(RefreshDedup::new()),
        }
    }

    /// The refresh body, 1:1 with Pi `refreshModels` (`remote-catalog-provider.ts:55-118`).
    async fn refresh_once(
        &self,
        provider_id: &str,
        options: RefreshOptions,
    ) -> Result<(), ProviderError> {
        // Pi reads the store FIRST, before the network gate (`:58-60`). cyrup's overlay is
        // materialized by `load_overlay`, so the read here exists for the freshness window and the
        // validator — but the ordering matters for the same reason: nothing below can make an
        // offline run lose its persisted overlay.
        let stored = self.store.read(provider_id, None).await.ok().flatten();

        if !options.allow_network {
            return Ok(());
        }

        // `!force && stored.checkedAt !== undefined && stored.lastModified !== undefined &&
        //  Date.now() - stored.checkedAt < REMOTE_CATALOG_REFRESH_INTERVAL_MS` (`:61-67`).
        if !options.force
            && let Some(entry) = &stored
            && let (Some(checked_at), Some(_)) = (entry.checked_at, entry.last_modified)
            && now_ms().saturating_sub(checked_at) < REMOTE_CATALOG_REFRESH_INTERVAL_MS
        {
            return Ok(());
        }

        // "Only revalidate when a cached body backs the validator, so a 304 can never leave the
        // overlay empty." (Pi's own comment, `:68-69`.)
        let validator = stored
            .as_ref()
            .filter(|entry| !entry.models.is_empty())
            .and_then(|entry| entry.etag.clone());

        // `url.searchParams.set("types", REMOTE_CATALOG_MODEL_TYPES.join(","))`
        // (`remote-catalog-provider.ts:101-102`) — PROV-128. Without it pi.dev serves the
        // chat-only shard and no image or classifier row ever arrives; with it the response is a
        // different top-level JSON shape, which [`parse_catalog`] handles. The value is
        // `chat,image,classifier`; `,` is a legal sub-delim in a query value, and upstream's
        // `URLSearchParams` does not escape it either.
        let url = format!(
            "{}/api/models/providers/{}?types={}",
            self.base_url.trim_end_matches('/'),
            encode_path_segment(provider_id),
            remote_catalog_types_param()
        );

        // Reuse the provider-traffic client builder so the catalog fetch honours the same
        // HTTP(S)_PROXY / ALL_PROXY / NO_PROXY resolution as every other outbound request.
        // `None` takes the process-global HTTP idle timeout on top of this request's own total
        // deadline below — Pi's global undici dispatcher covers catalog fetches too.
        let client =
            crate::stream::sse::build_client_for_target(&url, self.auth_ctx.as_ref(), None, None)
                .await
                .map_err(|e| ProviderError::Transport(Box::new(e)))?;
        let mut request = client
            .get(&url)
            .timeout(self.request_timeout)
            .header("accept", "application/json")
            .header("user-agent", cyrup_user_agent());
        if let Some(validator) = &validator {
            request = request.header("if-none-match", validator);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::Transport(Box::new(e)))?;

        let checked_at = now_ms();
        let status = response.status().as_u16();

        // 304: the cached body is still current, so ONLY the freshness window moves (`:83-88`).
        if status == 304
            && let Some(entry) = stored.clone()
        {
            let _ = self
                .store
                .write(
                    provider_id,
                    ModelsStoreEntry {
                        checked_at: Some(checked_at),
                        ..entry
                    },
                    None,
                )
                .await;
            return Ok(());
        }

        // 404/501: the route is unimplemented, so the overlay is unavailable — clear the validators
        // and the last-modified stamp, but never error (`:89-97`).
        if status == 404 || status == 501 {
            let _ = self
                .store
                .write(
                    provider_id,
                    ModelsStoreEntry {
                        checked_at: Some(checked_at),
                        last_modified: Some(0),
                        etag: None,
                        ..stored.unwrap_or_default()
                    },
                    None,
                )
                .await;
            return Ok(());
        }

        if !(200..300).contains(&status) {
            // Transient failure: the cached body AND its validator stay valid, so the etag is KEPT
            // and only `checkedAt` moves — the next attempt revalidates instead of re-downloading
            // (`:98-104`).
            let _ = self
                .store
                .write(
                    provider_id,
                    ModelsStoreEntry {
                        checked_at: Some(checked_at),
                        ..stored.unwrap_or_default()
                    },
                    None,
                )
                .await;
            return Err(ProviderError::Http {
                status,
                message: format!("Model catalog request failed for {provider_id}: {status}"),
            });
        }

        // Headers must be read before the body is consumed.
        let last_modified = response
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_http_date_ms)
            .unwrap_or(0);
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        let body = response
            .text()
            .await
            .map_err(|e| ProviderError::Transport(Box::new(e)))?;
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            ProviderError::Decode(format!(
                "Invalid model catalog for provider \"{provider_id}\": {e}"
            ))
        })?;
        let refreshed = parse_catalog(provider_id, &value)?;
        // PROV-128 — the body now carries every type, so all three halves are persisted, as ONE
        // store operation: pi writes one `models: AnyModel[]` array (`:143`), and a reader must
        // never see new chat rows beside the previous refresh's image rows. cyrup's entry carries
        // chat rows only (the EXT-027 seam), so `write_all_types` is what makes the three halves
        // one write.
        let (chat, images, classifiers) = split_by_type(refreshed);
        let _ = self
            .store
            .write_all_types(
                provider_id,
                ModelsStoreEntry {
                    models: chat,
                    checked_at: Some(checked_at),
                    last_modified: Some(last_modified),
                    etag,
                },
                images,
                classifiers,
                None,
            )
            .await;
        Ok(())
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
    use crate::model::{Modality, ModelCost};
    use crate::models_store::InMemoryModelsStore;

    fn model(provider: &str, id: &str, context_window: u64) -> Model {
        Model {
            id: id.into(),
            name: id.to_string(),
            api: "openai-completions".into(),
            provider: provider.into(),
            base_url: "https://example.invalid/v1".to_string(),
            reasoning: false,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            input_limits: None,
            prompt_cache: None,
            context_window,
            max_tokens: 4096,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    /// PROV-M01 fixture — every method the decorator must forward carries a **distinct
    /// non-default** value, so a deleted delegation cannot pass by agreeing with the trait default.
    ///
    /// The trait defaults this deliberately contradicts (`provider.rs:23-51`): `name` → the id,
    /// `base_url` → `None`, `headers` → `None`, `filter_models` → the catalog unchanged.
    struct Decorated {
        id: ProviderId,
        models: Vec<Model>,
        headers: crate::HeaderMap,
    }

    #[async_trait::async_trait]
    impl Provider for Decorated {
        fn id(&self) -> &ProviderId {
            &self.id
        }
        fn name(&self) -> &str {
            "Decorated Display Name"
        }
        fn base_url(&self) -> Option<&str> {
            Some("https://inner.invalid/v1")
        }
        fn headers(&self) -> Option<&crate::HeaderMap> {
            Some(&self.headers)
        }
        fn models(&self) -> &[Model] {
            &self.models
        }
        /// A REAL narrowing, not the identity default: it keeps only ids starting with `keep`.
        fn filter_models(
            &self,
            models: &[Model],
            _credential: Option<&crate::auth::Credential>,
        ) -> Vec<Model> {
            models
                .iter()
                .filter(|m| m.id.as_str().starts_with("keep"))
                .cloned()
                .collect()
        }
        fn stream(
            &self,
            _model: &Model,
            _context: &Context,
            _options: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            Box::pin(tokio_stream::empty())
        }
    }

    /// PROV-M01 — upstream `withRemoteCatalog` SPREADS the provider
    /// (`remote-catalog-provider.ts:52` @v0.83.0), so `name`/`baseUrl`/`headers`/`filterModels`
    /// survive by construction. In Rust they survive only because the delegation is written out,
    /// and the failure of forgetting is SILENT — the trait default answers instead.
    #[test]
    fn the_decorator_forwards_every_surface_method_the_spread_carries() {
        let mut headers = crate::HeaderMap::new();
        headers.insert("x-inner".to_string(), Some("inner-value".to_string()));
        let inner: Arc<dyn Provider> = Arc::new(Decorated {
            id: "decorated".into(),
            models: vec![
                model("decorated", "keep-a", 1),
                model("decorated", "drop-b", 2),
            ],
            headers,
        });
        // Assert PRESENCE on the inner first: a fixture that ever loses a declaration must fail
        // loudly rather than make the comparisons below vacuous.
        assert_ne!(
            inner.name(),
            inner.id().as_str(),
            "fixture must not agree with the default"
        );
        assert!(
            inner.base_url().is_some(),
            "fixture must declare a base_url"
        );
        assert!(inner.headers().is_some(), "fixture must declare headers");

        let overlay = ProviderOverlay::from_chat(vec![model("decorated", "keep-c", 3)]);
        let w = RemoteCatalogProvider::new(inner.clone(), &overlay);

        assert_eq!(w.id(), inner.id());
        assert_eq!(w.name(), "Decorated Display Name");
        assert_eq!(w.base_url(), Some("https://inner.invalid/v1"));
        assert_eq!(
            w.headers().and_then(|h| h.get("x-inner")),
            Some(&Some("inner-value".to_string()))
        );

        // The overlay half still works: the merged catalog is the floor plus the overlay.
        let ids: Vec<&str> = w.models().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["keep-a", "drop-b", "keep-c"]);
        // `get_model` resolves against the MERGED catalog (it must NOT be delegated to the inner).
        assert!(
            w.get_model("keep-c").is_some(),
            "get_model must see the overlay"
        );

        // The credential filter is the one whose loss was a live defect: the identity default would
        // return all three.
        let filtered: Vec<String> = w
            .filter_models(w.models(), None)
            .into_iter()
            .map(|m| m.id.to_string())
            .collect();
        assert_eq!(filtered, ["keep-a", "keep-c"]);
    }

    /// PROV-M01, the production path: `all_providers_with_overlay` maps EVERY built-in through
    /// [`CatalogOverlay::apply`] (`providers/all.rs:148-157`), and `github-copilot` is the one
    /// built-in that installs a `filter_models` (`providers/github_copilot.rs:178`). Before the
    /// delegation landed, wrapping it discarded the credential-scoped narrowing entirely.
    #[test]
    fn overlaying_github_copilot_keeps_its_credential_filter() {
        use crate::auth::Credential;
        let providers = crate::all_providers();
        let copilot = providers
            .iter()
            .find(|p| p.id().as_str() == "github-copilot")
            .expect("github-copilot is a built-in")
            .clone();
        let catalog = copilot.models().to_vec();
        assert!(
            catalog.len() > 2,
            "the embedded copilot catalog must be non-trivial"
        );

        // An OAuth credential entitled to exactly one of the catalog's ids.
        let entitled = catalog[0].id.to_string();
        let cred = Credential::Oauth {
            access: "a".into(),
            refresh: "r".into(),
            expires: i64::MAX,
            ext: [(
                "availableModelIds".to_string(),
                serde_json::json!([entitled.clone()]),
            )]
            .into_iter()
            .collect(),
        };
        // Presence first: the UNWRAPPED provider narrows.
        let bare = copilot.filter_models(&catalog, Some(&cred));
        assert_eq!(
            bare.len(),
            1,
            "the bare provider must narrow to the entitled id"
        );

        let overlay = CatalogOverlay::from_entries(vec![(
            "github-copilot".to_string(),
            vec![model("github-copilot", "overlay-only", 7)],
        )]);
        let wrapped = overlay.apply(copilot);
        assert!(
            wrapped.get_model("overlay-only").is_some(),
            "the overlay must actually have wrapped the provider"
        );
        let narrowed = wrapped.filter_models(wrapped.models(), Some(&cred));
        assert_eq!(
            narrowed
                .iter()
                .map(|m| m.id.to_string())
                .collect::<Vec<_>>(),
            vec![entitled],
            "the decorator must forward filter_models, not answer with the identity default"
        );
    }

    #[test]
    fn merge_replaces_by_id_appends_unknown_and_never_removes() {
        let baseline = vec![model("groq", "a", 1), model("groq", "b", 2)];
        let dynamic = vec![model("groq", "b", 999), model("groq", "c", 3)];
        let merged = merge_models(&baseline, &dynamic);
        // Order preserved, `b` replaced in place, `c` appended, `a` untouched.
        let ids: Vec<&str> = merged.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(merged[1].context_window, 999);
        // The floor: an empty overlay is exactly the baseline.
        assert_eq!(merge_models(&baseline, &[]), baseline);
    }

    #[test]
    fn parse_catalog_accepts_all_three_shapes_and_forces_provider() {
        let one = serde_json::json!({
            "id": "m1", "name": "M1", "api": "openai-completions", "provider": "WRONG",
            "baseUrl": "https://x.invalid", "reasoning": false, "input": ["text"],
            "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
            "contextWindow": 100, "maxTokens": 10
        });
        let array = serde_json::Value::Array(vec![one.clone()]);
        let wrapped = serde_json::json!({ "models": [one.clone()] });
        let keyed = serde_json::json!({ "m1": one });
        for value in [array, wrapped, keyed] {
            let parsed = parse_catalog("groq", &value).unwrap();
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0].provider().as_str(), "groq");
            assert_eq!(parsed[0].model_type(), ModelType::Chat);
        }
        // Non-object/array bodies carry Pi's message verbatim.
        let err = parse_catalog("groq", &serde_json::json!(42)).unwrap_err();
        assert!(
            err.to_string()
                .contains("Invalid model catalog for provider \"groq\""),
            "{err}"
        );
        // Entries without an id, and entries that cannot become a Model, are dropped (not fatal).
        assert!(
            parse_catalog(
                "groq",
                &serde_json::json!([{"name": "no id"}, {"id": "partial"}])
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn staleness_guard_discards_an_overlay_not_newer_than_the_builtins() {
        let entry = ModelsStoreEntry {
            models: vec![model("groq", "a", 1)],
            last_modified: Some(1_000),
            checked_at: Some(1_000),
            etag: None,
        };
        // No local stamp → guard disabled (Pi's `undefined`).
        assert_eq!(remote_models(Some(&entry), None).len(), 1);
        // Strictly newer than the builtins → kept.
        assert_eq!(remote_models(Some(&entry), Some(999)).len(), 1);
        // Equal or older → discarded WHOLE (pi #7016).
        assert!(remote_models(Some(&entry), Some(1_000)).is_empty());
        assert!(remote_models(Some(&entry), Some(1_001)).is_empty());
        // A stored entry with no `lastModified` cannot prove it is newer → discarded.
        let undated = ModelsStoreEntry {
            last_modified: None,
            ..entry
        };
        assert!(remote_models(Some(&undated), Some(1)).is_empty());
        assert!(remote_models(None, None).is_empty());
    }

    #[tokio::test]
    async fn load_overlay_drops_provider_mismatched_and_empty_entries() {
        let store = Arc::new(InMemoryModelsStore::new());
        store
            .write(
                "groq",
                ModelsStoreEntry {
                    // A body that mislabels its provider must not leak across providers.
                    models: vec![model("xai", "leak", 1), model("groq", "ok", 1)],
                    last_modified: Some(2),
                    checked_at: Some(2),
                    etag: None,
                },
                None,
            )
            .await
            .unwrap();
        let catalog = RemoteCatalog::new(store).with_local_generated_at(Some(1));
        let overlay = catalog.load_overlay(&["groq", "xai"]).await;
        assert_eq!(overlay.models_for("groq").len(), 1);
        assert_eq!(overlay.models_for("groq")[0].id.as_str(), "ok");
        assert!(overlay.models_for("xai").is_empty());
        assert!(!overlay.is_empty());
    }

    #[test]
    fn encode_path_segment_cannot_escape_the_route() {
        assert_eq!(
            encode_path_segment("openai-completions"),
            "openai-completions"
        );
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(encode_path_segment("a b"), "a%20b");
    }

    #[test]
    fn user_agent_has_pis_shape() {
        let ua = cyrup_user_agent();
        assert!(ua.starts_with("cyrup/"), "{ua}");
        assert!(ua.contains(std::env::consts::ARCH), "{ua}");
    }
}
