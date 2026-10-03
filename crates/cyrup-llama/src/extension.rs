//! the `NativeExtension`, `/llama` command flow and constructor (`index.ts`)
//!
//! Port of `packages/coding-agent/src/extensions/llama/index.ts` @v0.99.2-17 (EXT-027). Pi's
//! factory registers the llama.cpp provider and one command, `/llama` (`index.ts:42-44`, `:183`);
//! [`LlamaExtension`] does the same as a `cyrup_ext::native::NativeExtension`: one live provider at
//! `init` and the `llama` command.
//!
//! # Shape
//!
//! * [`LlamaExtension`] is the `NativeExtension`: registration, the mode / configuration gates of
//!   the command handler (`index.ts:185-191`) and the hand-off to the overlay
//!   ([`crate::ui::show_llama_ui`], `showLlamaUi`, `index.ts:192`).
//! * [`Flow`] is the body of that hand-off, `index.ts:192-227`: the catalog read with its
//!   Retry/Close dialog, the model list, and the load / unload / download actions with their
//!   catalog re-sync. It talks to the screen only through [`LlamaUi`], so it runs unchanged against
//!   the real overlay and against a scripted one.
//! * [`ContextRefreshHost`] connects [`crate::provider::LlamaProvider::refresh`] to the refresh the
//!   host is running (see its doc).
//!
//! # Mechanism differences
//!
//! * `[CYRUP-DELTA]` `index.ts:43` calls `createLlamaProvider()` inside the factory. A native is
//!   built before the host binds its services and registrar, and the controller needs both (the
//!   stored credential the provider streams with, and the way to replace the registered provider
//!   after a catalog change), so the controller is built at `init`, which the host runs after
//!   `set_host_services` / `set_late_registrar` (`native.rs`, "Called … BEFORE `init`"). Same
//!   provider, built later. Ledger: EXT-027.
//! * `[CYRUP-DELTA]` `ctx.modelRegistry.getProviderAuth` and `.refresh` (`index.ts:30`, `:54`) are
//!   reached through `HostServices::provider_auth` and `HostServices::refresh_provider`; a native's
//!   [`HostCtx`] carries no model registry. Same two calls, same arguments
//!   (`providers: [llama.cpp]`, `allowNetwork: true`, the 15 s signal). Ledger: EXT-027.
//! * `[CYRUP-DELTA]` `AbortSignal.timeout(15_000)` (`index.ts:51`) fails a pending `fetch` with the
//!   signal's own reason, a `TimeoutError` whose message contains `timeout`, which
//!   `isConnectionError` matches (`index.ts:14`). A `CancellationToken` carries no reason, so
//!   [`Flow::sync_catalog`] maps a cancelled list call to [`LlamaError::Timeout`] when its own
//!   timer fired. Ledger: EXT-027 (same mechanism as `ui.rs`'s `controller.abort(new Error(..))`).
//! * `pi.registerProvider(provider.provider)` (`index.ts:44`) becomes
//!   `InitApi::register_provider_live`, and every later catalog replacement goes through
//!   `LateRegistrar::register_provider_live` (see [`crate::provider`]'s "Catalog state").
//! * Upstream's handler is `async (_args, ctx)`; the arguments are ignored (`index.ts:185`), as
//!   here.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use cyrup_core::ExtensionId;
use cyrup_core::ProviderId;
use cyrup_ext::host::services::{
    HostProviderAuth, ModelsPersist, ModelsPublication, ProviderRefreshContext,
};
use cyrup_ext::registry::CommandDescriptor;
use cyrup_ext::{
    ExtError, ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, LateRegistrar,
    NativeExtension, NotifyKind,
};
use cyrup_provider::auth::{
    Credential, CredentialInfo, CredentialStore, InMemoryCredentialStore, ModifyFn,
};
use cyrup_provider::{
    AnyModel, AuthError, ClassifierModel, Model, ModelsStoreEntry, Provider, ProviderError,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::LLAMA_PROVIDER_ID;
use crate::client::{LlamaClient, LlamaModelInfo, LlamaModelStatus, LlamaProgress, format_bytes};
use crate::error::LlamaError;
use crate::huggingface::{
    HuggingFaceClient, find_huggingface_token, find_huggingface_token_with_home,
    process_environment,
};
use crate::provider::{
    CatalogEntry, CatalogPublication, CatalogPublisher, LlamaController, LlamaControllerOptions,
    LlamaRefreshHost, RegisterProviderFn, SetCatalogOptions,
};
use crate::ui::{
    ConnectionChoice, LlamaKeys, LlamaManagerAction, LlamaUi, ProgressOptions, ProgressOutcome,
    run_with_progress, search_fn, show_llama_ui,
};

/// The name of the command (`pi.registerCommand("llama", ...)`, `index.ts:183`).
const COMMAND: &str = "llama";

/// The command's description (`index.ts:184`).
const COMMAND_DESCRIPTION: &str = "Manage llama.cpp router models";

/// The extension's id: the name pi's built-in table gives it (`{ name: "llama.cpp", factory:
/// llamaExtension, builtin: true }`, `packages/coding-agent/src/extensions/index.ts`), which is also
/// the provider id.
const EXTENSION_ID: &str = LLAMA_PROVIDER_ID;

/// `AbortSignal.timeout(15_000)` (`index.ts:51`).
const SYNC_TIMEOUT: Duration = Duration::from_secs(15);

/// `LLAMA_BASE_URL` as the provider's auth environment overlay names it (`index.ts:35`).
const BASE_URL_ENV: &str = crate::provider::LLAMA_BASE_URL_ENV;

// =================================================================================================
// Errors
// =================================================================================================

/// What a failed step of the `/llama` flow looked like to upstream's `catch`: its message.
///
/// Upstream's `isConnectionError` reads `${error.name} ${error.message}` (`index.ts:12-13`). The
/// names Node gives the errors a `fetch` or an `AbortSignal` produces (`TypeError`, `TimeoutError`,
/// `AbortError`) add nothing the messages do not already carry: [`LlamaError`] documents that
/// each `Display` text is the message the JS runtime would put there (`fetch failed...`, `The
/// operation was aborted due to timeout`), and a plain `Error`'s name matches none of the three
/// words. So only the message is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowError {
    message: String,
}

impl FlowError {
    /// A plain `Error` (`new Error(message)`).
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The error's message (`error.message`).
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for FlowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FlowError {}

impl From<LlamaError> for FlowError {
    fn from(error: LlamaError) -> Self {
        Self::plain(error.to_string())
    }
}

/// A provider's refresh failure (`result.errors.get(LLAMA_PROVIDER_ID)`, `index.ts:61`). The host
/// wraps the provider's own error in [`ProviderError::ModelSource`]; upstream rethrows the
/// provider's error itself, so its message is the wrapper's source.
impl From<&ProviderError> for FlowError {
    fn from(error: &ProviderError) -> Self {
        match error {
            ProviderError::ModelSource(source) => Self::plain(source.to_string()),
            other => Self::plain(other.to_string()),
        }
    }
}

/// `isConnectionError` (`index.ts:11-15`): the error's message, lower-cased, mentions
/// `fetch failed`, `timeout` or `network`.
#[must_use]
pub fn is_connection_error(error: &FlowError) -> bool {
    let text = error.message.to_lowercase();
    text.contains("fetch failed") || text.contains("timeout") || text.contains("network")
}

/// `connectionErrorMessage` (`index.ts:17-20`): the text the Retry/Close dialog shows.
#[must_use]
pub fn connection_error_message(error: &FlowError) -> String {
    if is_connection_error(error) {
        "Could not connect to the server.".to_string()
    } else {
        error.message.clone()
    }
}

// =================================================================================================
// Pure helpers
// =================================================================================================

/// `modelIsLoaded` (`index.ts:7-9`): `loaded` and `sleeping` both count (a sleeping model wakes on
/// the next request).
#[must_use]
pub fn model_is_loaded(model: &LlamaModelInfo) -> bool {
    matches!(
        model.status.value,
        LlamaModelStatus::Loaded | LlamaModelStatus::Sleeping
    )
}

/// What `parseHuggingFaceModel` returns (`index.ts:22`): `{ repository, quantization? }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuggingFaceSelection {
    /// `owner/repository`.
    pub repository: String,
    /// The text after the `:` that follows the first `/`, which is empty for `owner/repo:`.
    pub quantization: Option<String>,
}

/// `parseHuggingFaceModel` (`index.ts:22-27`): `owner/repo[:quant]`. The colon searched for is the
/// first one after the first `/` (`value.indexOf(":", value.indexOf("/") + 1)`), so a colon in the
/// owner part is not a separator; with no `/` the search starts at the beginning.
#[must_use]
pub fn parse_huggingface_model(value: &str) -> HuggingFaceSelection {
    let from = value.find('/').map_or(0, |slash| slash + 1);
    let colon = value
        .get(from..)
        .and_then(|rest| rest.find(':'))
        .map(|offset| from + offset);
    match colon {
        Some(colon) => HuggingFaceSelection {
            repository: value.get(..colon).unwrap_or_default().to_string(),
            quantization: Some(value.get(colon + 1..).unwrap_or_default().to_string()),
        },
        None => HuggingFaceSelection {
            repository: value.to_string(),
            quantization: None,
        },
    }
}

/// A timer that cancels its token after a delay, `AbortSignal.timeout` (`index.ts:51`). Dropping it
/// stops the timer AND cancels the token: whatever still holds the token (a refresh the host
/// detached, a request in flight) is working for a caller that is gone and has no deadline of its
/// own once the timer is.
pub(crate) struct TimeoutSignal {
    pub(crate) token: CancellationToken,
    timer: JoinHandle<()>,
}

impl TimeoutSignal {
    pub(crate) fn after(delay: Duration) -> Self {
        let token = CancellationToken::new();
        let fire = token.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            fire.cancel();
        });
        Self { token, timer }
    }
}

impl Drop for TimeoutSignal {
    fn drop(&mut self) {
        self.timer.abort();
        self.token.cancel();
    }
}

/// "No signal": a token nobody cancels.
fn never() -> CancellationToken {
    CancellationToken::new()
}

// =================================================================================================
// The refresh host
// =================================================================================================

/// Connects [`crate::provider::LlamaProvider::refresh`] (`refreshModels`, `provider.ts:201-259`) to
/// the refresh the host is running.
///
/// Pi hands `refreshModels` a `RefreshModelsContext` carrying the credential, the stored catalog
/// and `publish` (`models.ts:74-90`). A cyrup provider's `refresh_models` receives only
/// `allow_network`, `force` and the abort token, so the host publishes the rest as
/// [`ProviderRefreshContext::current`] for the duration of the call; this reads it back. Called
/// outside a host-run refresh there is no store, so `credential` and `stored` are `None` and
/// `publish` answers `false` (the refresh ends, as a superseded one does).
pub(crate) struct ContextRefreshHost;

/// A [`CatalogEntry`] as the host's models store holds it: the chat models in the entry and the
/// classifier models beside it (`ModelsPersist::Write`).
fn persist_of(entry: CatalogEntry) -> ModelsPersist {
    let mut models: Vec<Model> = Vec::new();
    let mut classifiers: Vec<ClassifierModel> = Vec::new();
    for model in entry.models {
        match model {
            AnyModel::Chat(chat) => models.push(chat),
            AnyModel::Classifier(classifier) => classifiers.push(classifier),
        }
    }
    ModelsPersist::Write {
        entry: ModelsStoreEntry {
            models,
            checked_at: Some(entry.checked_at),
            ..ModelsStoreEntry::default()
        },
        classifiers,
    }
}

#[async_trait]
impl CatalogPublisher for ContextRefreshHost {
    /// pi's `context.publish`, whose rejection fails the refresh (`models.ts:498-518`): the
    /// host's publisher is asked, and its error is the refresh's error.
    async fn publish(&self, publication: CatalogPublication) -> Result<bool, LlamaError> {
        let Some(context) = ProviderRefreshContext::current() else {
            return Ok(false);
        };
        let publication = ModelsPublication {
            persist: publication.persist.map(persist_of),
            update: publication.update,
        };
        context
            .publish(publication)
            .await
            .map_err(|error| match error {
                ProviderError::Aborted => LlamaError::Cancelled,
                other => LlamaError::Message(other.to_string()),
            })
    }
}

#[async_trait]
impl LlamaRefreshHost for ContextRefreshHost {
    async fn credential(&self) -> Option<Credential> {
        ProviderRefreshContext::current()?.credential
    }

    async fn stored(&self) -> Option<CatalogEntry> {
        let context = ProviderRefreshContext::current()?;
        let stored = context.stored?;
        let models = stored
            .models
            .into_iter()
            .map(AnyModel::Chat)
            .chain(
                context
                    .stored_classifiers
                    .into_iter()
                    .map(AnyModel::Classifier),
            )
            .collect();
        Some(CatalogEntry {
            models,
            checked_at: stored.checked_at.unwrap_or(0),
        })
    }
}

// =================================================================================================
// The command flow
// =================================================================================================

/// Where the clients the flow creates connect to. Production uses [`Endpoints::default`]: the
/// process's proxy-aware HTTP stack, `https://huggingface.co` and the process environment. Tests
/// and embedders with a proxy, a mirror or their own TLS configuration set the three seams, the
/// way upstream's `HuggingFaceClient` takes a `baseUrl` (`huggingface.ts:67`) and `process.env`
/// is a default argument (`huggingface.ts:46`).
#[derive(Clone, Default)]
pub struct Endpoints {
    /// The HTTP client both servers are reached through.
    pub http: Option<reqwest::Client>,
    /// The Hugging Face API root (`baseUrl`, `huggingface.ts:67`).
    pub hugging_face_url: Option<String>,
    /// The environment the Hugging Face token is looked up in (`env = process.env`,
    /// `huggingface.ts:46`); with one given no home directory is consulted.
    pub environment: Option<BTreeMap<String, String>>,
}

impl Endpoints {
    async fn llama_client(
        &self,
        url: &str,
        api_key: Option<String>,
        env: Option<&BTreeMap<String, String>>,
    ) -> Result<LlamaClient, LlamaError> {
        match &self.http {
            Some(http) => LlamaClient::with_http_client(url, api_key, http.clone()),
            None => LlamaClient::new(url, api_key, env).await,
        }
    }

    async fn hugging_face_client(&self) -> Result<HuggingFaceClient, LlamaError> {
        let token = match &self.environment {
            Some(environment) => find_huggingface_token_with_home(environment, None).await,
            None => find_huggingface_token(&process_environment()).await,
        };
        match &self.http {
            Some(http) => Ok(HuggingFaceClient::with_http_client(
                token,
                self.hugging_face_url.as_deref(),
                http.clone(),
            )),
            None => HuggingFaceClient::new(token, self.hugging_face_url.as_deref(), None).await,
        }
    }
}

/// The body of `showLlamaUi(ctx, async (ui) => { ... })` (`index.ts:192-227`) with the helpers it
/// closes over (`syncCatalog`, `loadModel`, `unloadModel`, `downloadModel`, `index.ts:46-181`).
pub struct Flow {
    host: Arc<dyn HostServices>,
    controller: LlamaController,
    client: LlamaClient,
    endpoints: Endpoints,
    sync_timeout: Duration,
}

impl Flow {
    /// A flow against `client`, mirroring its catalog into `controller` and refreshing the host's
    /// model registry through `host`.
    #[must_use]
    pub fn new(
        host: Arc<dyn HostServices>,
        controller: LlamaController,
        client: LlamaClient,
        endpoints: Endpoints,
    ) -> Self {
        Self {
            host,
            controller,
            client,
            endpoints,
            sync_timeout: SYNC_TIMEOUT,
        }
    }

    /// Replace the 15 s limit of [`Self::sync_catalog`] (`index.ts:51`); chiefly for tests.
    #[must_use]
    pub fn with_sync_timeout(mut self, timeout: Duration) -> Self {
        self.sync_timeout = timeout;
        self
    }

    fn notify(&self, message: &str) {
        self.host.notify(message, NotifyKind::Info);
    }

    /// `syncCatalog` (`index.ts:46-64`): read the catalog (unless the caller has it), hand it to
    /// the provider, then have the host refresh the provider's stored catalog.
    ///
    /// The refresh is live even when the host is offline (`allowNetwork: true`, `index.ts:55-56`):
    /// the command has just contacted the configured server.
    ///
    /// # Errors
    ///
    /// The list call's error; `Model catalog refresh timed out.` when the 15 s signal fired during
    /// the refresh (`index.ts:60`); the provider's refresh error (`index.ts:61-62`); a refusal of
    /// the new provider by the host.
    pub async fn sync_catalog(
        &self,
        catalog: Option<Vec<LlamaModelInfo>>,
    ) -> Result<Vec<LlamaModelInfo>, FlowError> {
        let signal = TimeoutSignal::after(self.sync_timeout);
        let current = match catalog {
            Some(catalog) => catalog,
            None => self
                .client
                .list(false, &signal.token)
                .await
                .map_err(|error| match error {
                    LlamaError::Cancelled if signal.token.is_cancelled() => LlamaError::Timeout,
                    other => other,
                })?,
        };
        self.controller
            .set_catalog(
                &current,
                self.client.server_url(),
                SetCatalogOptions::default(),
            )
            .map_err(FlowError::from)?;
        let result = self
            .host
            .refresh_provider(LLAMA_PROVIDER_ID, true, signal.token.clone())
            .await;
        if result.aborted {
            return Err(FlowError::plain("Model catalog refresh timed out."));
        }
        if let Some(error) = result.errors.get(LLAMA_PROVIDER_ID) {
            return Err(FlowError::from(error));
        }
        Ok(current)
    }

    /// `readCatalog` (`index.ts:193-203`): sync, and on any failure offer Retry / Close.
    async fn read_catalog(&self, ui: &Arc<dyn LlamaUi>) -> Option<Vec<LlamaModelInfo>> {
        loop {
            match self.sync_catalog(None).await {
                Ok(catalog) => return Some(catalog),
                Err(error) => {
                    let choice = ui
                        .connection_error(
                            self.client.server_url(),
                            &connection_error_message(&error),
                        )
                        .await;
                    if choice == ConnectionChoice::Close {
                        return None;
                    }
                }
            }
        }
    }

    /// The command flow (`index.ts:204-227`): read the catalog, then loop on the model list until
    /// the user closes it. A failed action is reported after the catalog has been re-read, unless
    /// it was a connection error (that surfaces as the Retry/Close dialog of the re-read instead).
    ///
    /// # Errors
    ///
    /// Never in practice: every failing step is reported through the UI or `notify`, as upstream's
    /// flow catches them all; `show_llama_ui` still reports an `Err` as an error notification
    /// (`ui.ts:485-488`).
    pub async fn run(&self, ui: Arc<dyn LlamaUi>) -> Result<(), FlowError> {
        let Some(mut catalog) = self.read_catalog(&ui).await else {
            return Ok(());
        };
        loop {
            let action = ui.show_models(self.client.server_url(), &catalog).await;
            let outcome = match action {
                LlamaManagerAction::Close => return Ok(()),
                LlamaManagerAction::Download => self.download_model(&ui).await,
                LlamaManagerAction::Model(model) => {
                    if model_is_loaded(&model) {
                        self.unload_model(&ui, &model).await
                    } else if model.status.value == LlamaModelStatus::Unloaded {
                        self.load_model(&ui, &catalog, &model).await
                    } else {
                        self.host.notify(
                            &format!("{} is {}", model.id, model.status.value.as_str()),
                            NotifyKind::Warning,
                        );
                        Ok(())
                    }
                }
            };
            let Some(refreshed) = self.read_catalog(&ui).await else {
                return Ok(());
            };
            catalog = refreshed;
            if let Err(error) = outcome
                && !is_connection_error(&error)
            {
                self.host.notify(error.message(), NotifyKind::Error);
            }
        }
    }

    /// `loadModel` (`index.ts:66-123`).
    ///
    /// With other models loaded the user chooses to unload them first, keep them, or cancel. When
    /// they were unloaded and the load is cancelled or fails, they are loaded again; a failing
    /// restore never replaces the original error.
    pub(crate) async fn load_model(
        &self,
        ui: &Arc<dyn LlamaUi>,
        catalog: &[LlamaModelInfo],
        target: &LlamaModelInfo,
    ) -> Result<(), FlowError> {
        let loaded: Vec<&LlamaModelInfo> = catalog
            .iter()
            .filter(|model| model.id != target.id && model_is_loaded(model))
            .collect();
        let mut replace = false;
        if !loaded.is_empty() {
            let count = loaded.len();
            let title = format!(
                "{count} model{} loaded",
                if count == 1 { " is" } else { "s are" }
            );
            let options = [
                "Unload all and load".to_string(),
                "Keep loaded and load".to_string(),
                "Cancel".to_string(),
            ];
            let choice = ui.select(&title, &options).await;
            match choice.as_deref() {
                None | Some("Cancel") => return Ok(()),
                Some(choice) => replace = choice == "Unload all and load",
            }
        }

        if replace {
            for model in &loaded {
                self.client.unload_and_wait(&model.id, &never()).await?;
            }
        }

        let outcome: Result<(), FlowError> = async {
            let result = run_with_progress(
                Arc::clone(ui),
                ProgressOptions {
                    title: "Loading model".to_string(),
                    model: target.id.clone(),
                    initial_message: "Starting…".to_string(),
                    cancel_title: "Stop loading?".to_string(),
                    cancel_message: target.id.clone(),
                },
                |signal, update| async move {
                    self.client
                        .load_and_wait(&target.id, update.as_ref(), &signal)
                        .await
                        .map_err(FlowError::from)
                },
                || async {
                    self.client
                        .unload(&target.id, &never())
                        .await
                        .map_err(FlowError::from)
                },
            )
            .await?;
            if matches!(result, ProgressOutcome::Cancelled) {
                if replace {
                    self.restore_loaded(&loaded).await?;
                }
                return Ok(());
            }
            let refreshed = self.sync_catalog(None).await?;
            let loaded_now = refreshed
                .iter()
                .find(|model| model.id == target.id)
                .is_some_and(|model| model.status.value == LlamaModelStatus::Loaded);
            self.notify(&if loaded_now {
                format!("Loaded {}", target.id)
            } else {
                format!("Load started for {}", target.id)
            });
            Ok(())
        }
        .await;

        if let Err(error) = outcome {
            if replace {
                // Preserve the original load error (`index.ts:117-119`).
                let _ = self.restore_loaded(&loaded).await;
            }
            return Err(error);
        }
        Ok(())
    }

    /// `restoreLoaded` (`index.ts:85-89`).
    pub(crate) async fn restore_loaded(&self, loaded: &[&LlamaModelInfo]) -> Result<(), FlowError> {
        self.notify("Restoring previously loaded models");
        let silent = |_: LlamaProgress| {};
        for model in loaded {
            self.client
                .load_and_wait(&model.id, &silent, &never())
                .await?;
        }
        self.sync_catalog(None).await?;
        Ok(())
    }

    /// `unloadModel` (`index.ts:125-135`).
    pub(crate) async fn unload_model(
        &self,
        ui: &Arc<dyn LlamaUi>,
        model: &LlamaModelInfo,
    ) -> Result<(), FlowError> {
        if !ui.confirm("Unload model?", &model.id).await {
            return Ok(());
        }
        self.client.unload_and_wait(&model.id, &never()).await?;
        self.sync_catalog(None).await?;
        self.notify(&format!("Unloaded {}", model.id));
        Ok(())
    }

    /// `downloadModel` (`index.ts:137-181`): search Hugging Face, read the repository's details,
    /// ask for the quantization, and have the router download it.
    pub(crate) async fn download_model(&self, ui: &Arc<dyn LlamaUi>) -> Result<(), FlowError> {
        let hugging_face = Arc::new(self.endpoints.hugging_face_client().await?);
        let search_client = Arc::clone(&hugging_face);
        let selected = ui
            .search_models(search_fn(move |query, signal| {
                let hugging_face = Arc::clone(&search_client);
                async move { hugging_face.search(&query, &signal).await }
            }))
            .await;
        let Some(selected) = selected else {
            return Ok(());
        };
        let parsed = parse_huggingface_model(&selected);
        ui.show_status("Loading model details", &parsed.repository);
        let details = hugging_face.details(&parsed.repository, &never()).await?;
        if details.gated.is_gated() {
            let approval = if details.gated == crate::huggingface::HuggingFaceGated::Manual {
                "Manual approval is required"
            } else {
                "Accept the access terms"
            };
            let title = format!(
                "Hugging Face access required\n{id}\n\n{approval} at:\nhttps://huggingface.co/{id}\n\nThe llama.cpp server needs HF_TOKEN with access.",
                id = details.id
            );
            let options = ["Continue".to_string(), "Back".to_string()];
            if ui.select(&title, &options).await.as_deref() != Some("Continue") {
                return Ok(());
            }
        }
        let mut quantization = parsed
            .quantization
            .filter(|quantization| !quantization.is_empty());
        if quantization.is_none() && !details.quantizations.is_empty() {
            let options: Vec<String> = details
                .quantizations
                .iter()
                .map(|entry| {
                    let detail = [
                        entry.size.map(format_bytes),
                        (entry.name == "Q4_K_M").then(|| "recommended".to_string()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    if detail.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{} · {detail}", entry.name)
                    }
                })
                .collect();
            let title = format!("Select quantization\n{}", details.id);
            let Some(choice) = ui.select(&title, &options).await else {
                return Ok(());
            };
            quantization = options
                .iter()
                .position(|option| *option == choice)
                .and_then(|index| details.quantizations.get(index))
                .map(|entry| entry.name.clone())
                .filter(|name| !name.is_empty());
            if quantization.is_none() {
                return Ok(());
            }
        }
        let model = match &quantization {
            Some(quantization) => format!("{}:{quantization}", details.id),
            None => details.id.clone(),
        };
        let result = run_with_progress(
            Arc::clone(ui),
            ProgressOptions {
                title: "Downloading model".to_string(),
                model: model.clone(),
                initial_message: "Starting…".to_string(),
                cancel_title: "Stop download?".to_string(),
                cancel_message: model.clone(),
            },
            |signal, update| {
                let model = model.clone();
                async move {
                    self.client
                        .download_and_wait(&model, update.as_ref(), &signal)
                        .await
                        .map_err(FlowError::from)
                }
            },
            || async {
                self.client
                    .unload(&model, &never())
                    .await
                    .map_err(FlowError::from)
            },
        )
        .await?;
        let ProgressOutcome::Completed(catalog) = result else {
            return Ok(());
        };
        self.sync_catalog(Some(catalog)).await?;
        self.notify(&format!("Downloaded {model}"));
        Ok(())
    }
}

// =================================================================================================
// The extension
// =================================================================================================

/// The credential store the provider streams with: the host's, reached through
/// [`HostServices::provider_credentials`] at the moment of each call.
///
/// `[CYRUP-DELTA]` `createLlamaProvider()` (`index.ts:43`) is handed pi's one `AuthStorage` by the
/// runtime that builds the extension, so it is there from the first call. A native is built before
/// the host has a credential store to give it (`LiveHostServices::attach_provider_auth` runs once
/// the session's provider registry exists, after the natives' `init`), so the store is looked up
/// per call: the provider sees whatever store the host holds NOW, a login stored later included.
/// Same credential, resolved later. Ledger: EXT-027.
///
/// A host with no credential store gets a process-local one, so the provider is registered and
/// simply has no credential until the host has one.
pub(crate) struct HostCredentials {
    services: Arc<OnceLock<Arc<dyn HostServices>>>,
    fallback: InMemoryCredentialStore,
}

impl HostCredentials {
    /// The store over the services cell the extension fills in `set_host_services`.
    pub(crate) fn new(services: Arc<OnceLock<Arc<dyn HostServices>>>) -> Self {
        Self {
            services,
            fallback: InMemoryCredentialStore::new(),
        }
    }

    /// The host's store for the llama.cpp provider, when it has one.
    fn host_store(&self) -> Option<Arc<dyn CredentialStore>> {
        self.services
            .get()
            .and_then(|services| services.provider_credentials(LLAMA_PROVIDER_ID))
    }
}

#[async_trait]
impl CredentialStore for HostCredentials {
    async fn read(&self, provider: &ProviderId) -> Result<Option<Credential>, AuthError> {
        match self.host_store() {
            Some(store) => store.read(provider).await,
            None => self.fallback.read(provider).await,
        }
    }

    async fn list(&self) -> Result<Vec<CredentialInfo>, AuthError> {
        match self.host_store() {
            Some(store) => store.list().await,
            None => self.fallback.list().await,
        }
    }

    async fn modify(
        &self,
        provider: &ProviderId,
        f: ModifyFn,
    ) -> Result<Option<Credential>, AuthError> {
        match self.host_store() {
            Some(store) => store.modify(provider, f).await,
            None => self.fallback.modify(provider, f).await,
        }
    }

    async fn delete(&self, provider: &ProviderId) -> Result<(), AuthError> {
        match self.host_store() {
            Some(store) => store.delete(provider).await,
            None => self.fallback.delete(provider).await,
        }
    }
}

/// The llama.cpp native extension: the `llama.cpp` provider and the `/llama` command.
pub struct LlamaExtension {
    id: ExtensionId,
    agent_dir: PathBuf,
    endpoints: Endpoints,
    sync_timeout: Duration,
    /// Late-bound by the host before `init` (`NativeExtension::set_host_services`), shared with the
    /// refresh host and the registration callback so they see it whenever it arrives.
    host_services: Arc<OnceLock<Arc<dyn HostServices>>>,
    /// Late-bound by the host before `init` (`NativeExtension::set_late_registrar`).
    registrar: Arc<OnceLock<Arc<dyn LateRegistrar>>>,
    controller: OnceLock<LlamaController>,
}

impl LlamaExtension {
    /// The extension for the agent directory `agent_dir` (where `keybindings.json` lives, which
    /// the overlay's keys are read from on every `/llama`).
    #[must_use]
    pub fn new(agent_dir: PathBuf) -> Self {
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            agent_dir,
            endpoints: Endpoints::default(),
            sync_timeout: SYNC_TIMEOUT,
            host_services: Arc::new(OnceLock::new()),
            registrar: Arc::new(OnceLock::new()),
            controller: OnceLock::new(),
        }
    }

    /// Connect the clients the command creates through `endpoints` instead of the defaults.
    #[must_use]
    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Self {
        self.endpoints = endpoints;
        self
    }

    /// Replace the 15 s limit of the command's catalog sync (`index.ts:51`); chiefly for tests.
    #[must_use]
    pub fn with_sync_timeout(mut self, timeout: Duration) -> Self {
        self.sync_timeout = timeout;
        self
    }

    /// The provider controller (`createLlamaProvider()`, `index.ts:43`), built on first use.
    ///
    /// The stored credential the provider streams with comes from the host
    /// (`HostServices::provider_credentials`), asked on EVERY call ([`HostCredentials`]) and not
    /// once here: the host attaches the credential store after `init` has run, so a store captured
    /// here could only ever be the empty one.
    fn controller(&self) -> &LlamaController {
        self.controller.get_or_init(|| {
            let credentials: Arc<dyn CredentialStore> =
                Arc::new(HostCredentials::new(Arc::clone(&self.host_services)));
            let registrar = Arc::clone(&self.registrar);
            let register: RegisterProviderFn = Arc::new(move |provider: Arc<dyn Provider>| {
                let registrar = registrar
                    .get()
                    .ok_or_else(|| "the host bound no provider registrar".to_string())?;
                registrar
                    .register_provider_live(LLAMA_PROVIDER_ID.to_string(), provider)
                    .map_err(|error| error.to_string())
            });
            LlamaController::new(
                LlamaControllerOptions::new(credentials, register)
                    .with_refresh_host(Arc::new(ContextRefreshHost)),
            )
        })
    }

    /// `configuredClient` (`index.ts:29-40`): the client for the server the provider's auth names,
    /// or `None` after telling the user how to configure one.
    async fn configured_client(
        &self,
        host: &Arc<dyn HostServices>,
    ) -> Result<Option<LlamaClient>, FlowError> {
        let auth = host
            .provider_auth(LLAMA_PROVIDER_ID)
            .await
            .map_err(FlowError::plain)?;
        let Some(auth) = auth else {
            host.notify(
                &format!("Configure llama.cpp with /login {LLAMA_PROVIDER_ID}"),
                NotifyKind::Warning,
            );
            return Ok(None);
        };
        Ok(Some(self.client_for(&auth).await?))
    }

    /// The URL (`LLAMA_BASE_URL` of the auth environment, else `auth.baseUrl`, else empty, which
    /// does not normalise) and key of `index.ts:35-39`.
    async fn client_for(&self, auth: &HostProviderAuth) -> Result<LlamaClient, LlamaError> {
        let url = match auth.env.get(BASE_URL_ENV) {
            Some(configured) if !configured.is_empty() => configured.as_str(),
            _ => auth.base_url.as_deref().unwrap_or_default(),
        };
        self.endpoints
            .llama_client(url, auth.api_key.clone(), Some(&auth.env))
            .await
    }
}

#[async_trait]
impl NativeExtension for LlamaExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Registers the provider (`pi.registerProvider(provider.provider)`, `index.ts:44`) and the
    /// command (`index.ts:183-184`). Never fails: nothing here can.
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_provider_live(LLAMA_PROVIDER_ID, self.controller().provider());
        api.register_command(
            COMMAND,
            CommandDescriptor {
                description: COMMAND_DESCRIPTION.into(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    /// Ambient: pi's `builtin:llama.cpp` extension is a path in the tier `--no-extensions`
    /// collapses (`package-manager.ts:972-974`, `resource-loader.ts:569-571` @v0.99.2-17), so the
    /// flag drops it here too.
    fn is_ambient(&self) -> bool {
        true
    }

    /// Hidden from the startup `[Extensions]` listing, as pi marks every `builtin:` extension: the
    /// table entry carries only `builtin: true` (`extensions/index.ts`); the loader sets
    /// `extension.hidden = true` (`core/resource-loader.ts:729`).
    fn is_hidden(&self) -> bool {
        true
    }

    // `decides_project_trust` stays `false`: pi's built-ins load after trust is decided, in the
    // second pass only (`resource-loader.ts:378-399`, `:706-730`).

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.host_services.set(services);
    }

    fn set_late_registrar(&self, registrar: Arc<dyn LateRegistrar>) {
        let _ = self.registrar.set(registrar);
    }

    /// The `/llama` handler (`index.ts:185-228`).
    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        if name != COMMAND {
            return Err(ExtError::Component(format!(
                "the llama.cpp extension has no command `{name}`"
            )));
        }
        ctx.require_command_tier()?;
        let Some(host) = self.host_services.get().cloned() else {
            return Err(ExtError::Component(
                "the llama.cpp extension was given no host services".to_string(),
            ));
        };
        if ctx.mode != ExtMode::Tui {
            host.notify(
                "/llama is available in interactive mode",
                NotifyKind::Warning,
            );
            return Ok(None);
        }
        // A person answers the dialogs: the dispatcher's budget must not cut the command short.
        let _human_wait = ctx.begin_human_wait();
        let client = match self.configured_client(&host).await {
            Ok(Some(client)) => client,
            Ok(None) => return Ok(None),
            // pi lets this reject out of the handler (`index.ts:30-40`); the command runner shows
            // the message as `command:llama: <message>`.
            Err(error) => return Err(ExtError::CommandFailed(error.message().to_string())),
        };
        let flow = Flow::new(
            Arc::clone(&host),
            self.controller().clone(),
            client,
            self.endpoints.clone(),
        )
        .with_sync_timeout(self.sync_timeout);
        // `showLlamaUi` (`index.ts:192`). With no interactive surface the flow never starts, which
        // is what `ctx.ui.custom` does without a UI.
        let _ = show_llama_ui(
            host,
            LlamaKeys::from_agent_dir(&self.agent_dir),
            |ui| async move { flow.run(ui).await },
        )
        .await;
        Ok(None)
    }
}
