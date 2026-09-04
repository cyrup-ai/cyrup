//! Provider selection seam (arch-11 §1, §3.6 note).
//!
//! Resolves a `(provider, model, apiKey)` triple to the owning [`Provider`] exactly as Pi's
//! `resolveCliModel` does (`packages/coding-agent/src/core/model-resolver.ts:385` @v0.83.0, called
//! from `packages/coding-agent/src/main.ts:423-429`): an explicit `--provider` wins, else the `provider/...`
//! prefix on `--model`, else — when nothing else in startup upgrades it — the zero-model
//! [`UnconfiguredProvider`]. A `--api-key` is installed as a runtime credential for the resolved
//! provider (Pi `options.apiKey`); any explicit provider is looked up in the registry. There is
//! intentionally NO silent fallback: a prefix that is not a known provider is a clear error listing
//! the providers that ARE available.
//!
//! **PROV-052.** This used to return the scripted `FauxProvider` for the no-flag case, so a bare
//! credential-less `cyrup -p hi` answered `No more faux responses queued`. pi does not have a
//! faux fallback at all: `faux.ts` is exported from the `pi-ai` package for tests, is absent from
//! `providers/all.ts`, is not a `KnownProvider`, and `git grep faux v0.83.0 --
//! packages/coding-agent/src/` matches zero files. With no credential pi's `getAvailable()` is
//! empty, `findInitialModel` returns `{model: undefined}` at its step 5
//! (`packages/coding-agent/src/core/model-resolver.ts:650-651`) and
//! `packages/coding-agent/src/main.ts:852-855` prints `formatNoModelsAvailableMessage()` to stderr
//! and exits 1 for every mode EXCEPT `interactive`. The empty catalog of [`UnconfiguredProvider`]
//! reproduces that, mode gating included: `resolve_model` turns it into `model: None` + the
//! `modelFallbackMessage` banner (`cyrup-session-svc/src/builder.rs:1601-1612`, pi
//! `sdk.ts:216-218`), and the hard stop lives one tier up on the built session — `main.rs`'s rpc
//! (`:741-745`) and print/json (`:862-866`) arms check `session.model().is_none()` and return
//! `no_models_available()`, while the interactive arm (`:552`) deliberately does not, so a
//! credential-less first run still gets a TUI to type `/login` into (SEAM-075). See
//! `cyrup_provider::unconfigured`'s module docs.
//!
//! **The registry every function here reads is the COMPOSED one** — the built-ins
//! (`cyrup_provider::all_providers`, the 1:1 port of Pi `providers/all.ts`) with
//! `<agent_dir>/models.json` layered over them. Pi has exactly one registry and it is the composed
//! one (`ModelRuntime.rebuildProviders`,
//! `packages/coding-agent/src/core/model-runtime.ts:226-231`); reaching for
//! `cyrup_provider::default_models` directly here would read a registry Pi does not have, and a
//! provider declared only in `models.json` would be unlaunchable, unlistable and unselectable.
//! Hence every entry point takes the loaded [`ModelFile`]; pass `&ModelFile::default()` when there
//! is deliberately no user config in play.

use std::sync::{Arc, RwLock};

use anyhow::bail;
use cyrup_config::ModelFile;
use cyrup_config::models_store::FileModelsStore;
use cyrup_config::policy::NetworkPolicy;
use cyrup_provider::unconfigured::UnconfiguredProvider;
use cyrup_provider::{
    CatalogOverlay, CreateModelsOptions, Credential, InMemoryCredentialStore, Models, ModelsStore,
    Provider, RefreshOptions, RemoteCatalog,
};
use cyrup_sdk::core::ProviderId;

/// The process-wide runtime model-catalog overlay (DRIFT-007).
///
/// `None` — the value at process start and after any failure — means "embedded catalogs only",
/// which is exactly the pre-DRIFT-007 behavior. A slot rather than a `OnceLock` because the
/// background refresh re-installs a newer overlay when one arrives; every registry build here is
/// already constructed fresh per call ([`composed_registry`]), so the next read simply picks it up
/// with no live mutation and no lock on the streaming path.
static CATALOG_OVERLAY: RwLock<Option<Arc<CatalogOverlay>>> = RwLock::new(None);

fn active_catalog_overlay() -> Option<Arc<CatalogOverlay>> {
    // A poisoned lock degrades to "no overlay" — never a panic, never fewer models than embedded.
    CATALOG_OVERLAY.read().ok().and_then(|g| g.clone())
}

fn install_catalog_overlay(overlay: Option<Arc<CatalogOverlay>>) {
    if let Ok(mut slot) = CATALOG_OVERLAY.write() {
        *slot = overlay;
    }
}

/// The disk-backed remote catalog: `<agent_dir>/models-store.json` behind the pi.dev fetcher, with
/// the built-in manifest stamp installed so the staleness guard can discard an overlay that is not
/// newer than the compiled-in catalogs.
fn remote_catalog(dirs: &cyrup_config::ConfigDirs) -> Arc<RemoteCatalog> {
    let store: Arc<dyn ModelsStore> = Arc::new(FileModelsStore::for_dirs(dirs));
    Arc::new(
        RemoteCatalog::new(store)
            .with_local_generated_at(cyrup_provider::builtin_model_data_generated_at()),
    )
}

fn all_provider_ids() -> Vec<String> {
    cyrup_provider::all_providers()
        .iter()
        .map(|p| p.id().as_str().to_string())
        .collect()
}

/// The providers whose catalogs are fetched from pi.dev — every configured built-in EXCEPT
/// `radius` (PROV-014).
///
/// Pi wraps each built-in in `withRemoteCatalog` with one exclusion:
/// `provider.id === "radius" ? provider : withRemoteCatalog(provider, …)`
/// (`packages/coding-agent/src/core/model-runtime.ts:183-189` @v0.84.4). Radius's store entry is
/// owned by its OWN refresh (the gateway's `/v1/config`, `cyrup_provider::RadiusProvider`), and
/// the pi.dev route has no catalog for it: the `404` branch would rewrite the entry's
/// `lastModified` to `0` and the overlay loader's staleness guard would then discard the gateway
/// catalog on the next start. The exclusion is applied to the FETCH list only; `load_overlay`
/// still reads the radius entry back, which is the restore half of `radius.ts:36-48`.
pub fn pi_dev_catalog_providers(configured: &[String]) -> Vec<String> {
    configured
        .iter()
        .filter(|id| id.as_str() != cyrup_provider::RADIUS_PROVIDER_ID)
        .cloned()
        .collect()
}

/// **Phase 1 of the runtime model catalog (DRIFT-007) — disk only, awaited, no network.**
///
/// `<agent_dir>/models-store.json` is loaded and installed, so the composed registry immediately
/// reflects the catalogs the last run fetched — including when this run is offline. This is Pi's
/// cache-only restore, `await modelRuntime.refresh({ allowNetwork: false })`
/// (`packages/coding-agent/src/core/agent-session-services.ts:180`), which runs inside
/// `createAgentSessionRuntime`.
///
/// **Call-site ordering is load-bearing.** Pi performs this restore during runtime CREATION
/// (`packages/coding-agent/src/main.ts:793`), which is upstream of the `--list-models` exit at
/// `main.ts:812-816`. So `pi --list-models` renders the persisted pi.dev overlay. cyrup must call
/// this before its own
/// `--list-models` early return for the same reason — that listing is the one entry point that
/// never reaches the rest of startup. It is awaited because it is a single small local file read;
/// it performs no network I/O of any kind, so awaiting it cannot block on a remote host.
pub async fn restore_model_catalog(dirs: &cyrup_config::ConfigDirs) {
    let catalog = remote_catalog(dirs);
    let all_ids = all_provider_ids();
    let refs: Vec<&str> = all_ids.iter().map(String::as_str).collect();
    let overlay = catalog.load_overlay(&refs).await;
    install_catalog_overlay((!overlay.is_empty()).then(|| Arc::new(overlay)));
}

/// **The modes that are allowed to touch the network for catalogs — and only those.**
///
/// Pi has exactly TWO catalog-refresh triggers and neither of them is a scripted run:
///
/// - RPC: `packages/coding-agent/src/main.ts:863-866` — `if (!offlineMode && appMode === "rpc") { void modelRuntime.refresh() }`,
///   guarded on the mode by name.
/// - Interactive: `interactive-mode.ts` `run()` — `if (!process.env.PI_OFFLINE) { void
///   this.session.modelRuntime.refresh()… }`, reached only from the interactive front-end.
///
/// Runtime CREATION never fetches: every `ModelRuntime.create` call in `coding-agent` passes
/// `allowModelNetwork: false` (`packages/coding-agent/src/main.ts:158`,
/// `packages/coding-agent/src/package-manager-cli.ts:401`) and
/// `packages/coding-agent/src/core/model-runtime.ts:163`
/// computes `refreshFromNetwork = runtime.modelNetworkEnabled && options.allowModelNetwork === true`.
/// So `pi -p "…"` and pi's JSON output mode issue no catalog request at all.
///
/// cyrup must land in the same place. A one-shot `cyrup -p "…"` or `--mode json` is the scripted /
/// CI path; silently adding an outbound `https://pi.dev` request to it would be a network trigger
/// upstream does not have. The catalog data those runs read is the persisted overlay restored by
/// [`restore_model_catalog`], which is disk-only.
pub fn mode_refreshes_catalogs(mode: cyrup_config::trust::AppMode) -> bool {
    use cyrup_config::trust::AppMode;
    matches!(mode, AppMode::Rpc | AppMode::Interactive)
}

/// **Phase 2 of the runtime model catalog (DRIFT-007) — fire and forget.**
///
/// When the mode is one Pi refreshes in ([`mode_refreshes_catalogs`]) AND
/// [`NetworkPolicy::allow_model_catalog_refresh`] permits it, a DETACHED task revalidates the
/// catalogs against pi.dev and re-installs the overlay if anything changed. This is Pi's post-init
/// trigger (`main.ts:863-866`, `interactive-mode.ts` `run()`), moved off `ModelRuntime.create` by pi
/// commit `c889eb88` precisely so that startup never blocks on it. Nothing awaits the task and every
/// error is swallowed: the worst case is the embedded catalogs.
///
/// Like Pi's, this is deliberately NOT reached by a `--list-models` run: upstream's trigger is
/// downstream of the `main.ts:812-816` listing exit, so a listing issues no request. Keeping this call
/// at its post-startup-UI site preserves that.
///
/// `configured_providers` restricts the fetch to providers the user has actually configured, exactly
/// as Pi's `Models.refresh` does by bailing when `resolveRefreshCredential` yields nothing
/// (`packages/ai/src/models.ts:296` — `if (!credential) return;`). Without it a bare `cyrup` would
/// fan out one request per built-in provider on
/// every start.
pub fn spawn_model_catalog_refresh(
    dirs: &cyrup_config::ConfigDirs,
    policy: NetworkPolicy,
    mode: cyrup_config::trust::AppMode,
    configured_providers: Vec<String>,
) {
    spawn_model_catalog_refresh_with(remote_catalog(dirs), policy, mode, configured_providers);
}

/// [`spawn_model_catalog_refresh`] over an injected [`RemoteCatalog`].
///
/// The base URL is the only transport seam this subsystem has (Pi's `catalogBaseUrl` option), so
/// this is what lets the mode/offline gating be proven against a loopback listener instead of
/// `https://pi.dev`. Returns the [`JoinHandle`](tokio::task::JoinHandle) when a task was actually
/// spawned and `None` when the gate declined — a test can then await the task rather than sleep,
/// and can assert the gate fired without waiting on a socket that will never be opened.
pub fn spawn_model_catalog_refresh_with(
    catalog: Arc<RemoteCatalog>,
    policy: NetworkPolicy,
    mode: cyrup_config::trust::AppMode,
    configured_providers: Vec<String>,
) -> Option<tokio::task::JoinHandle<()>> {
    // Print/JSON stop here: Pi refreshes in rpc and interactive only, so a scripted run must issue
    // no request. Offline / `--offline` / `CYRUP_OFFLINE` / `PI_OFFLINE` stops here too. So does
    // having no configured provider: there is nothing whose catalog we are entitled to fetch.
    if !mode_refreshes_catalogs(mode)
        || !policy.allow_model_catalog_refresh()
        || configured_providers.is_empty()
    {
        return None;
    }
    let all_ids = all_provider_ids();
    let configured_providers = pi_dev_catalog_providers(&configured_providers);
    Some(tokio::spawn(async move {
        let refs: Vec<&str> = configured_providers.iter().map(String::as_str).collect();
        // Best-effort: per-provider errors are collected, not propagated, and are deliberately not
        // surfaced to the user — a catalog that could not be refreshed is simply the embedded one.
        let _errors = catalog
            .refresh_providers(&refs, RefreshOptions::network())
            .await;
        let all_refs: Vec<&str> = all_ids.iter().map(String::as_str).collect();
        let overlay = catalog.load_overlay(&all_refs).await;
        install_catalog_overlay((!overlay.is_empty()).then(|| Arc::new(overlay)));
    }))
}

/// The 15-second budget `cyrup update --models` gives the whole refresh (Pi
/// `refreshModelCatalogs`'s `setTimeout(() => controller.abort(), 15_000)`,
/// `packages/coding-agent/src/package-manager-cli.ts:404`).
const MODELS_REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// **`cyrup update --models`** — the FOREGROUND, forced catalog refresh (Pi `refreshModelCatalogs`,
/// `packages/coding-agent/src/package-manager-cli.ts:397-423` @v0.83.0, dispatched from `:726-735`).
///
/// This is the only entry point that is neither of the two background triggers
/// ([`restore_model_catalog`], [`spawn_model_catalog_refresh`]): the user asked for it, so it is
/// awaited, it bypasses the 4h freshness window (`force: true`, `:409` ⇒
/// [`RefreshOptions::forced`]), and — unlike the fire-and-forget path — every per-provider error is
/// REPORTED rather than swallowed (`:414-417`).
///
/// Three upstream details are load-bearing:
///
/// - **The whole refresh is bounded** by one 15s abort (`:403-404`), not a per-request timeout, and
///   an abort is an ERROR (`Model catalog refresh timed out.`, `:411-413`) rather than a silent
///   partial success.
/// - **Only providers with a resolvable credential are fetched.** Upstream never fans out over every
///   built-in: `Models.refresh` bails per provider at `packages/ai/src/models.ts:296` (`if
///   (!credential) return;`), so a fresh install with no `auth.json` issues zero requests and still
///   succeeds. Same predicate as `main.rs`'s background trigger — [`cyrup_config::AuthStore`].
/// - **No offline gate.** `refreshModelCatalogs` passes `allowNetwork: true` unconditionally
///   (`:407`) and `Models.refresh` has no `isOfflineModeEnabled()` check — that check exists only on
///   the PACKAGE update path (`package-manager.ts:1080`). An explicit `cyrup update --models` is a
///   request for the network, so [`NetworkPolicy`] is deliberately not consulted here.
///
/// The refreshed catalogs land in `<agent_dir>/models-store.json`, which is what
/// [`restore_model_catalog`] reads on every later run — including offline ones.
///
/// Returns `Err(message)` with the string the caller renders as pi's `Error: {message}`.
pub async fn refresh_model_catalogs(dirs: &cyrup_config::ConfigDirs) -> Result<(), String> {
    let auth = cyrup_config::AuthStore::at(dirs.agent_dir.join("auth.json"));
    let configured: Vec<String> = cyrup_provider::all_providers()
        .iter()
        .filter(|p| auth.has_auth(p.id(), None))
        .map(|p| p.id().as_str().to_string())
        .collect();
    refresh_model_catalogs_with(remote_catalog(dirs), configured).await
}

/// [`refresh_model_catalogs`] over an injected [`RemoteCatalog`] and provider set — the same
/// transport seam [`spawn_model_catalog_refresh_with`] exposes, and for the same reason: the base URL
/// is the only place this subsystem can be redirected (Pi's `catalogBaseUrl`), so the force flag, the
/// 15s bound and the error report can be proven against a loopback listener instead of
/// `https://pi.dev`. The credential-gated provider list is resolved by the caller because it is the
/// half that reads the real `auth.json` and the process environment.
pub async fn refresh_model_catalogs_with(
    catalog: Arc<RemoteCatalog>,
    configured: Vec<String>,
) -> Result<(), String> {
    let errors = {
        let configured = pi_dev_catalog_providers(&configured);
        let refs: Vec<&str> = configured.iter().map(String::as_str).collect();
        match tokio::time::timeout(
            MODELS_REFRESH_TIMEOUT,
            catalog.refresh_providers(&refs, RefreshOptions::forced()),
        )
        .await
        {
            Ok(errors) => errors,
            // `if (result.aborted) throw new Error("Model catalog refresh timed out.")` (`:411-413`).
            Err(_elapsed) => return Err("Model catalog refresh timed out.".to_string()),
        }
    };

    if !errors.is_empty() {
        // `Could not refresh model catalogs: ${details}` where `details` joins `${provider}:
        // ${error.message}` with `"; "` (`:415-417`).
        let details = errors
            .iter()
            .map(|(provider, err)| format!("{provider}: {err}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("Could not refresh model catalogs: {details}"));
    }

    // The freshly persisted catalogs are the overlay this process would read from here on. Nothing
    // else runs in a `cyrup update --models` process, but installing it keeps the in-memory view and
    // the store from disagreeing for any caller that follows.
    let all_ids = all_provider_ids();
    let all_refs: Vec<&str> = all_ids.iter().map(String::as_str).collect();
    let overlay = catalog.load_overlay(&all_refs).await;
    install_catalog_overlay((!overlay.is_empty()).then(|| Arc::new(overlay)));
    Ok(())
}

/// The composed registry (built-ins + `models.json`) over an optional runtime `--api-key`
/// credential. Composition errors are the caller's to surface —
/// [`models_json_composition_errors`] is the once-at-startup view.
fn composed_registry(
    models_json: &ModelFile,
    api_key: Option<&str>,
    provider_id: Option<&str>,
) -> (Models, Vec<String>) {
    // Install the runtime `--api-key` as a credential for the resolved provider so the provider
    // streams with it (Pi threads `apiKey` into the auth context). Absent a key the env-backed
    // default auth resolves the key at stream time.
    let credentials = match (api_key, provider_id) {
        (Some(key), Some(id)) => {
            let store = InMemoryCredentialStore::new()
                .with_credential(ProviderId::from(id), Credential::api_key(key));
            Some(Arc::new(store) as Arc<dyn cyrup_provider::CredentialStore>)
        }
        _ => None,
    };
    cyrup_config::compose_provider_registry(
        models_json,
        CreateModelsOptions {
            credentials,
            auth_context: None,
            // The runtime pi.dev overlay (DRIFT-007), applied UNDER `models.json`: a user's explicit
            // config still wins, and the embedded catalogs still floor the result.
            catalog_overlay: active_catalog_overlay(),
        },
    )
}

/// The composed registry over an EXPLICIT credential store — the shape a caller that must resolve
/// request auth for a model needs (Pi `ModelRuntime.create({ allowModelNetwork: false })` followed
/// by `modelRuntime.getAuth(model, …)`, `packages/coding-agent/src/core/model-runtime.ts:376-384`).
///
/// [`composed_registry`] can only seed a single runtime `--api-key`; the credential-print surface
/// (`cyrup auth print-api-key`) must instead present every credential persisted in `auth.json` so
/// [`cyrup_provider::Models::get_auth_with`] resolves the same way a real request would. Composition
/// errors are dropped here for the same reason Pi's credential-print path drops them: they are a
/// once-at-startup concern ([`models_json_composition_errors`]), not part of the printed value.
pub fn registry_with_credentials(
    models_json: &ModelFile,
    credentials: Arc<dyn cyrup_provider::CredentialStore>,
) -> Models {
    let (models, _errors) = cyrup_config::compose_provider_registry(
        models_json,
        CreateModelsOptions {
            credentials: Some(credentials),
            auth_context: None,
            catalog_overlay: active_catalog_overlay(),
        },
    );
    models
}

/// Every model across ALL built-in providers — the faithful data source for `--list-models` (Pi
/// `modelRuntime.getAvailable()`, `packages/coding-agent/src/cli/list-models.ts:35`). Independent
/// of `--provider`/`--model`: Pi's `listModels` always enumerates the full multi-provider
/// registry (`providers/all.ts`), never just
/// the session's selected provider. Neither the scripted `faux` double nor the zero-model
/// [`UnconfiguredProvider`] appears here: pi's `faux.ts` is absent from `providers/all.ts`, and the
/// unconfigured stand-in is a cyrup run-time placeholder, not a catalog entry (PROV-052).
pub fn all_available_models(models_json: &ModelFile) -> Vec<cyrup_provider::Model> {
    let (models, _errors) = composed_registry(models_json, None, None);
    models.get_models(None)
}

/// The **auth-configured** subset of [`all_available_models`] — Pi `modelRuntime.getAvailable()`
/// (`packages/ai/src/models.ts:394-405` @v0.83.0, `:522-538` @v0.84.1), whose interface comment is
/// unambiguous (`models.ts:152-153`): *"Return models whose providers have complete auth
/// configuration."*
///
/// ```text
/// return checks.flatMap(({ provider, credential, auth }) => {
///     if (!auth) return [];
///     return provider.filterModels?.(models, credential) ?? models;
/// });
/// ```
///
/// This is what `--list-models` must enumerate (`cli/list-models.ts:35` @v0.83.0, `:39` @v0.84.1),
/// and what makes its empty branch — pi's `formatNoModelsAvailableMessage()` at `list-models.ts:
/// 37-40` — reachable on a fresh install. [`all_available_models`] is pi's `getModels()`, i.e. the
/// whole compiled catalog, which is a different question. SEAM-020.
///
/// `has_configured_auth` is the same predicate [`default_launch_model`] already consumes (pi step 4's
/// `hasConfiguredAuth`, `model-resolver.ts:633-648`), so the listing and the launch answer "which
/// provider can I actually use?" identically. The per-provider `filterModels` narrowing has no cyrup
/// counterpart yet and is a no-op here.
pub fn available_models(
    models_json: &ModelFile,
    has_configured_auth: &dyn Fn(&cyrup_provider::Model) -> bool,
) -> Vec<cyrup_provider::Model> {
    all_available_models(models_json)
        .into_iter()
        .filter(|m| has_configured_auth(m))
        .collect()
}

/// The one-shot composition report for `<agent_dir>/models.json`: the messages a caller should print
/// once at startup (Pi's `ModelRuntime.compositionErrors` —
/// `packages/coding-agent/src/core/model-runtime.ts:103` declares the map, `:220` fills it, and
/// `getError()` at `:336-344` renders it). Empty when the
/// file is absent or every provider block composes.
pub fn models_json_composition_errors(models_json: &ModelFile) -> Vec<String> {
    let (_models, errors) = composed_registry(models_json, None, None);
    errors
}

/// Resolve the launch `(provider_id, "provider/model_id")` for the **no-`--provider`/no-`--model`**
/// default path, by Pi's `findInitialModel` precedence
/// (`packages/coding-agent/src/core/model-resolver.ts:621-648` @v0.83.0, steps 3-4):
///
/// 1. the saved settings default `(defaultProvider, defaultModelId)` when it resolves in the full
///    registry (Pi step 3, `model-resolver.ts:621-631`, `modelRuntime.getModel` +
///    `hasConfiguredAuth`), else
/// 2. the first *configured* provider's curated default model (Pi step 4, `model-resolver.ts:633-648`:
///    `getAvailable()` scanned against `defaultModelPerProvider`), else the first available model.
///
/// Returns `Some((provider_id, pattern))` for a REAL configured provider so the caller launches on it
/// (footer shows e.g. `together/moonshotai/Kimi-K2.6`), or `None` when NOTHING is configured — in
/// which case the caller keeps the zero-model [`UnconfiguredProvider`], the session builds
/// modelless with pi's `modelFallbackMessage` banner, and the non-interactive modes then stop with
/// `formatNoModelsAvailableMessage()` (Pi `findInitialModel` step 5, model-resolver.ts:650-651 ⇒
/// the mode-gated `main.ts:852-855`). `has_configured_auth` mirrors Pi `modelRegistry`'s check (a
/// provider with a stored credential / known env var such as `TOGETHER_API_KEY` / runtime `--api-key`).
pub fn default_launch_model(
    default_provider: Option<&str>,
    default_model_id: Option<&str>,
    has_configured_auth: &dyn Fn(&cyrup_provider::Model) -> bool,
    models_json: &ModelFile,
) -> Option<(String, String)> {
    let all = all_available_models(models_json);
    let available: Vec<cyrup_provider::Model> = all
        .iter()
        .filter(|m| has_configured_auth(m))
        .cloned()
        .collect();
    // No `--provider`/`--model`, no `--models` scope, fresh (non-continuing) session: Pi's step-1/2
    // (CLI args / scoped) are inert, so this exercises steps 3-5 exactly.
    let result = cyrup_config::find_initial_model(
        None,
        None,
        &[],
        false,
        default_provider,
        default_model_id,
        None,
        &all,
        &available,
        has_configured_auth,
    );
    result.model.map(|m| {
        let provider = m.provider.as_str().to_string();
        let pattern = format!("{}/{}", provider, m.id.as_str());
        (provider, pattern)
    })
}

/// A [`cyrup_session_svc::ProviderResolver`] backed by [`select_provider`]: rebuilds the owning
/// built-in provider — installing its env-backed credentials — for a target provider id. Wired into
/// the session so a `/model` selection that targets a DIFFERENT provider than the current one swaps
/// the owning provider live (Pi model+provider switch —
/// `packages/coding-agent/src/modes/interactive/components/model-selector.ts:354-359`, whose
/// `handleSelect` calls `setDefaultModelAndProvider(model.provider, model.id)` then the session's
/// select callback). The provider's
/// key resolves at stream time from the environment (e.g. `TOGETHER_API_KEY`), matching Pi.
pub struct BuiltinProviderResolver {
    models_json: Arc<ModelFile>,
}

impl BuiltinProviderResolver {
    /// Bind the resolver to the session's loaded `models.json` so an in-session `/model` selection
    /// of a user-declared provider swaps onto the COMPOSED provider, not a built-in that does not
    /// exist (Pi resolves every `setModel` against the one composed registry).
    pub fn new(models_json: Arc<ModelFile>) -> Self {
        Self { models_json }
    }
}

impl cyrup_session_svc::ProviderResolver for BuiltinProviderResolver {
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        select_provider(Some(provider_id), None, None, &self.models_json).map_err(|e| e.to_string())
    }
}

/// The provider id a model pattern addresses, if it carries an explicit `provider/...` prefix.
fn provider_prefix(model_pattern: Option<&str>) -> Option<&str> {
    let pattern = model_pattern?;
    let (prefix, _) = pattern.split_once('/')?;
    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
}

/// The resolved provider id (Pi `resolveCliModel` precedence): explicit `--provider`, else the
/// `--model` prefix, else `None` (⇒ the zero-model [`UnconfiguredProvider`]).
fn resolve_provider_id<'a>(
    provider_override: Option<&'a str>,
    model_pattern: Option<&'a str>,
) -> Option<&'a str> {
    provider_override
        .filter(|s| !s.is_empty())
        .or_else(|| provider_prefix(model_pattern))
}

/// Resolve a [`Provider`] for the requested `(provider, model, apiKey)` triple.
///
/// - No explicit provider/prefix ⇒ the zero-model [`UnconfiguredProvider`]. Startup may still
///   upgrade this to a real configured provider (`main.rs`'s `default_launch_model` block, Pi
///   `findInitialModel` steps 3-4); when it does not, the empty catalog resolves to `model: None`
///   plus Pi's `modelFallbackMessage` banner (`cyrup-session-svc/src/builder.rs:1601-1612`, pi
///   `sdk.ts:216-218`), and the MODE-gated stop in `main.rs` (rpc `:741-745`, print/json
///   `:862-866`) turns that into `formatNoModelsAvailableMessage()` + exit 1 for the
///   non-interactive modes only (`packages/coding-agent/src/main.ts:852-855`). PROV-052 — this arm
///   used to hand back the scripted `FauxProvider`.
/// - Any explicit provider ⇒ looked up in the built-in registry (Pi `providers/all.ts`), with
///   `api_key` installed as a runtime credential when present (Pi `options.apiKey`).
/// - A provider that is not a built-in ⇒ a clear error listing the available built-ins. `faux` is
///   deliberately NOT among them: Pi's own scripted provider is not in `providers/all.ts` either
///   and is unreachable from its CLI.
pub fn select_provider(
    provider_override: Option<&str>,
    model_pattern: Option<&str>,
    api_key: Option<&str>,
    models_json: &ModelFile,
) -> anyhow::Result<Arc<dyn Provider>> {
    match resolve_provider_id(provider_override, model_pattern) {
        None => Ok(Arc::new(UnconfiguredProvider::new())),
        // PROV-052 — TEST-ONLY, and compiled out of every normal build. `faux` is not a provider a
        // shipped binary can select: pi's own `faux.ts` is absent from `providers/all.ts`, is not a
        // `KnownProvider`, and is referenced by zero files under `packages/coding-agent/src/`
        // @v0.83.0. The feature is enabled solely by this package's self-dev-dependency (Cargo.toml),
        // so `cargo tree -p cyrup -e features --edges normal` carries no `faux` and this arm does not
        // exist in `cargo build`/`--release`/`cargo install`. It exists so the integration tests that
        // spawn `CARGO_BIN_EXE_cyrup` can still script an offline turn end-to-end.
        #[cfg(feature = "faux")]
        Some("faux") => Ok(Arc::new(cyrup_provider::faux::FauxProvider::new())),
        Some(id) => {
            let (models, _errors) = composed_registry(models_json, api_key, Some(id));
            match models.get_provider(id) {
                Some(provider) => Ok(provider),
                None => {
                    let mut available: Vec<String> = models
                        .get_providers()
                        .iter()
                        .map(|p| p.id().as_str().to_string())
                        .collect();
                    available.sort();
                    bail!(
                        "model targets provider '{id}', which is not a known provider. \
                         Available providers: {}. \
                         (Declare a custom one under \"providers\" in <agent-dir>/models.json; \
                         there is intentionally no silent fallback.)",
                        available.join(", ")
                    )
                }
            }
        }
    }
}

/// The Pi `resolveCliModel` **unknown-model diagnostic**
/// (`packages/coding-agent/src/core/model-resolver.ts:520-544` @v0.83.0 — the `if (provider)`
/// custom-id fallback block, whose warning string is composed at `:543-544`): when a
/// `--model` targets a *known* provider (explicit `--provider`, or a `provider/…` prefix that is a
/// built-in) but no catalog model matches the requested id, Pi still builds a custom-id model and
/// **warns** `Model "<pattern>" not found for provider "<provider>". Using custom model id.`. Returns
/// that warning string, or `None` when the model resolves (or the provider can't be determined — an
/// *unknown* provider is a hard error raised earlier by [`select_provider`], not a warning).
///
/// Matching is case-insensitive and lenient (exact id, full `provider/id`, or an id substring) so it
/// never false-warns on a resolvable model — e.g. `together/moonshotai/kimi-k2.6` matches the
/// catalog's `moonshotai/Kimi-K2.6`.
pub fn unknown_model_warning(
    provider_override: Option<&str>,
    model_pattern: Option<&str>,
    catalog: &[cyrup_provider::Model],
) -> Option<String> {
    let pattern = model_pattern?;
    let known = |name: &str| {
        catalog
            .iter()
            .find(|m| m.provider.as_str().eq_ignore_ascii_case(name))
            .map(|m| m.provider.as_str().to_string())
    };

    // Determine the (canonical) provider + the provider-stripped pattern, exactly as Pi does.
    let (provider, rest) = match provider_override.filter(|s| !s.is_empty()) {
        Some(p) => {
            let canonical = known(p)?; // an unknown explicit provider is select_provider's error
            let prefix = format!("{canonical}/");
            let rest = if pattern
                .to_ascii_lowercase()
                .starts_with(&prefix.to_ascii_lowercase())
            {
                pattern[prefix.len()..].to_string()
            } else {
                pattern.to_string()
            };
            (canonical, rest)
        }
        None => {
            // Infer from a `provider/…` prefix; a non-provider prefix has no provider to diagnose — no warn.
            let (prefix, after) = pattern.split_once('/')?;
            (known(prefix)?, after.to_string())
        }
    };

    // Drop a trailing `:level` so the displayed pattern matches Pi's `fallbackPattern`.
    let (base, _level) = crate::cli::split_model_level(&rest);
    if base.is_empty() {
        return None;
    }
    let base_lc = base.to_ascii_lowercase();
    let found = catalog.iter().any(|m| {
        m.provider.as_str().eq_ignore_ascii_case(&provider)
            && (m.id.as_str().eq_ignore_ascii_case(&base)
                || m.id.as_str().to_ascii_lowercase().contains(&base_lc)
                || format!("{}/{}", m.provider.as_str(), m.id.as_str())
                    .eq_ignore_ascii_case(pattern))
    });
    if found {
        None
    } else {
        Some(format!(
            "Model \"{base}\" not found for provider \"{provider}\". Using custom model id."
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn all_available_models_span_the_full_registry() {
        // The full multi-provider registry (Pi `getAvailable`), not just one provider's catalog.
        let models = all_available_models(&ModelFile::default());
        assert!(!models.is_empty());
        let providers: std::collections::BTreeSet<&str> =
            models.iter().map(|m| m.provider.as_str()).collect();
        // Several distinct built-in providers are represented (anthropic/openai/together at minimum).
        assert!(
            providers.len() > 1,
            "expected multiple providers, got {providers:?}"
        );
        assert!(providers.contains("anthropic"));
        assert!(providers.contains("openai"));
        assert!(providers.contains("together"));
        // Neither the scripted double nor the unconfigured stand-in is a catalog entry.
        assert!(!providers.contains("faux"));
    }

    /// PROV-052. Replaces `defaults_and_faux_resolve_to_faux`, which was a **test-defect**: it
    /// pinned behaviour pi does not have. pi's scripted provider (`packages/ai/src/providers/faux.ts`)
    /// is exported only from the `pi-ai` package for tests — it is absent from
    /// `packages/ai/src/providers/all.ts`, it is not a member of `KnownProvider`, and
    /// `git grep faux v0.83.0 -- packages/coding-agent/src/` matches **zero** files. Neither a bare
    /// invocation nor any `--provider`/`--model` spelling can reach it in pi. This test was RED
    /// before the fix (`select_provider(None, ..)` returned `FauxProvider`, id `"faux"`) and is
    /// GREEN after.
    #[test]
    fn no_flags_resolve_to_an_empty_catalog_never_to_a_test_double() {
        // The bare invocation: a zero-model provider, which `resolve_model` turns into
        // `model: None` + pi's `modelFallbackMessage` ⇒ the mode-gated stop in `main.rs` prints
        // `packages/coding-agent/src/main.ts:852-855`'s message and exits 1 in the non-interactive
        // modes, while interactive still opens a TUI to type `/login` into.
        let p = select_provider(None, None, None, &ModelFile::default()).unwrap();
        assert_eq!(p.id().as_str(), "unconfigured");
        assert!(
            p.models().is_empty(),
            "the no-credential fallback must expose no models — an available model here is what \
             silently launched the scripted double"
        );
        // A bare id with no `provider/` prefix is the same no-flag path.
        let p = select_provider(None, Some("faux-1"), None, &ModelFile::default()).unwrap();
        assert_eq!(p.id().as_str(), "unconfigured");
        assert!(p.models().is_empty());
        // An unrecognised prefix is the ordinary unknown-provider error, never a silent double.
        // (`Arc<dyn Provider>` is not `Debug`, so `expect_err` is unavailable — take the `Err` side.)
        let err = select_provider(None, Some("not-a-provider/x"), None, &ModelFile::default())
            .err()
            .expect("an unknown prefix is an error");
        assert!(err.to_string().contains("not a known provider"));
        // `faux` itself is selectable ONLY under this package's TEST-ONLY `faux` feature, which is
        // reached solely through the self-dev-dependency; the shipped binary has no such arm. That
        // half of the invariant is pinned by a Cargo-graph test that cannot be expressed here —
        // `crates/cyrup-provider/tests/faux_not_in_normal_build.rs`.
    }

    #[test]
    fn explicit_provider_override_wins_over_model_prefix() {
        // `--provider openai` with a bare model resolves to openai (Pi precedence).
        let p = select_provider(Some("openai"), Some("gpt-4o"), None, &ModelFile::default())
            .expect("openai built-in");
        assert_eq!(p.id().as_str(), "openai");
    }

    #[test]
    fn built_in_real_providers_resolve_from_the_registry() {
        let anthropic = select_provider(
            None,
            Some("anthropic/claude-opus"),
            None,
            &ModelFile::default(),
        )
        .expect("anthropic built-in");
        assert_eq!(anthropic.id().as_str(), "anthropic");
        let openai = select_provider(None, Some("openai/gpt-4o"), None, &ModelFile::default())
            .expect("openai built-in");
        assert_eq!(openai.id().as_str(), "openai");
    }

    #[test]
    fn api_key_is_accepted_for_a_real_provider() {
        let p = select_provider(
            Some("openai"),
            Some("openai/gpt-4o"),
            Some("sk-runtime"),
            &ModelFile::default(),
        )
        .expect("openai built-in with runtime key");
        assert_eq!(p.id().as_str(), "openai");
    }

    #[test]
    fn together_kimi_resolves_to_together_provider() {
        let together = select_provider(
            None,
            Some("together/moonshotai/Kimi-K2.6"),
            None,
            &ModelFile::default(),
        )
        .expect("together is built-in");
        assert_eq!(together.id().as_str(), "together");
        assert!(
            together
                .models()
                .iter()
                .any(|m| m.id.as_str() == "moonshotai/Kimi-K2.6")
        );
    }

    #[test]
    fn unknown_model_within_known_provider_warns_but_resolvable_does_not() {
        let catalog = all_available_models(&ModelFile::default());
        // A real catalog model on a known provider → NO warning (the live path must not false-warn).
        assert_eq!(
            unknown_model_warning(None, Some("together/moonshotai/Kimi-K2.6"), &catalog),
            None
        );
        // Case-insensitive: the lowercased live-path spelling still resolves → no warning.
        assert_eq!(
            unknown_model_warning(None, Some("together/moonshotai/kimi-k2.6"), &catalog),
            None
        );
        // A bogus model id on a KNOWN provider → Pi's "Using custom model id." warning.
        let warn = unknown_model_warning(None, Some("openai/totally-made-up-9000"), &catalog)
            .expect("a warning for an unknown model on a known provider");
        assert!(warn.contains("totally-made-up-9000"));
        assert!(warn.contains("not found for provider \"openai\""));
        assert!(warn.contains("Using custom model id."));
        // Explicit `--provider` + a bogus id (no prefix) → same warning.
        let warn2 = unknown_model_warning(Some("openai"), Some("nope-model"), &catalog)
            .expect("explicit-provider warning");
        assert!(warn2.contains("not found for provider \"openai\""));
        // No `provider/` prefix and no `--provider` → unconfigured (no provider to diagnose) → NO warning.
        assert_eq!(unknown_model_warning(None, Some("gpt-4o"), &catalog), None);
        // No `--model` at all → nothing to diagnose.
        assert_eq!(unknown_model_warning(Some("openai"), None, &catalog), None);
    }

    #[test]
    fn no_model_with_configured_provider_launches_that_provider_default() {
        // Given a configured provider (Pi `hasConfiguredAuth` true — e.g. `TOGETHER_API_KEY` set) and
        // no `--model`/`--provider`, the launch model is that provider's curated default, NOT the
        // unconfigured stand-in
        // (Pi `findInitialModel` step 4, packages/coding-agent/src/core/model-resolver.ts:633-648).
        let together_configured = |m: &cyrup_provider::Model| m.provider.as_str() == "together";
        let (provider, pattern) =
            default_launch_model(None, None, &together_configured, &ModelFile::default())
                .expect("a configured provider yields a real launch model");
        assert_eq!(provider, "together");
        assert_ne!(provider, "unconfigured");
        // Pi `defaultModelPerProvider.together` — `together: "moonshotai/Kimi-K2.6"`,
        // packages/coding-agent/src/core/model-resolver.ts:41 @v0.83.0.
        assert_eq!(pattern, "together/moonshotai/Kimi-K2.6");
    }

    #[test]
    fn no_model_and_nothing_configured_yields_no_launch_model() {
        // Nothing configured ⇒ `None` ⇒ the caller keeps the zero-model `UnconfiguredProvider`, the
        // session builds modelless with pi's `modelFallbackMessage`, and the non-interactive modes
        // then stop (Pi `findInitialModel` step 5,
        // packages/coding-agent/src/core/model-resolver.ts:650-651 — `model: undefined` ⇒ the
        // mode-gated `packages/coding-agent/src/main.ts:852-855`).
        let nothing_configured = |_: &cyrup_provider::Model| false;
        assert_eq!(
            default_launch_model(None, None, &nothing_configured, &ModelFile::default()),
            None
        );
    }

    #[test]
    fn saved_settings_default_wins_over_curated_provider_default() {
        // A saved settings default `(provider, model)` that resolves in the registry wins over the
        // configured-provider curated default (Pi `findInitialModel` step 3,
        // packages/coding-agent/src/core/model-resolver.ts:621-631).
        let together_configured = |m: &cyrup_provider::Model| m.provider.as_str() == "together";
        // The saved default names together's curated model explicitly; still resolves to together.
        let (provider, pattern) = default_launch_model(
            Some("together"),
            Some("moonshotai/Kimi-K2.6"),
            &together_configured,
            &ModelFile::default(),
        )
        .expect("saved settings default resolves");
        assert_eq!(provider, "together");
        assert_eq!(pattern, "together/moonshotai/Kimi-K2.6");
    }

    #[test]
    fn truly_unknown_provider_errors_clearly() {
        let err = match select_provider(
            None,
            Some("definitely-not-a-provider/whatever"),
            None,
            &ModelFile::default(),
        ) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error for an unknown provider"),
        };
        assert!(err.contains("definitely-not-a-provider"));
        assert!(err.contains("not a known provider"));
        assert!(err.contains("together"));
        assert!(err.contains("anthropic"));
    }
}
