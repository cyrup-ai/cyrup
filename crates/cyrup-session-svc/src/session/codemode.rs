//! The session side of the `codemode` tool: what pi's `ExtensionToolContext` gives
//! `executeCodemode` — `ctx.tools`, `ctx.executeTool`, `ctx.sessionManager`, `ctx.modelRegistry`
//! and the extension's `appendEntry`, `getMode`, `getInlineBudget`
//! (`extensions/codemode/execute.ts:316-347`, `index.ts:31-45` @v1.0.1) — as one
//! [`cyrup_codemode_runtime::tool::CodemodeHost`].
//!
//! # Two stages
//!
//! The host exists from the moment the builder resolves its settings, because the tool's
//! `prepare_loadout` hook runs while the session is still being assembled and reads the mode and
//! budget then. The session itself exists only after [`AgentSession::into_shared`] mints its `Arc`,
//! which is when [`SessionCodemodeHost::attach`] hands the host a `Weak<AgentSession>`; everything
//! that needs the session (nested calls, the branch, the model registry) answers "no session" until
//! then, and a script cannot run before it.
//!
//! # Production call path
//!
//! [`crate::SessionBuilder::with_codemode`] (reached from the binary's `attach_native_extensions`)
//! makes the builder create one [`SessionCodemodeHost`] per built session and bind it into the
//! extension's slot; [`AgentSession::into_shared`] attaches the session.

use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use cyrup_agent::NestedToolCallOptions;
use cyrup_codemode_runtime::tool::{
    BranchCustomEntry, CODEMODE_STORE_ENTRY_TYPE, CodemodeHost, CodemodeModels,
    CodemodeStoreEntryData, NestedOutcome, StoreAppendFailed,
};
use cyrup_config::CodemodeMode;
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolResult};
use cyrup_provider::{CatalogOverlay, CreateModelsOptions, Models, Provider};
use cyrup_session::{Entry, KnownEntry};
use serde_json::Value;

use crate::event::AgentSessionEvent;

use super::AgentSession;

/// The `codemode.mode` / `codemode.inlineBudget` the session was built with.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CodemodeSettings {
    pub(crate) mode: CodemodeMode,
    pub(crate) inline_budget: f64,
}

/// The model registry scripts see, with what it was composed from: composition is pure, so the
/// registry is reused until the installed provider, the guest providers or the catalog overlay change
/// (the keys of the session's own registry snapshot, `model_runtime.rs`).
struct ComposedModels {
    provider: Arc<dyn Provider>,
    guest_generation: u64,
    overlay: Option<Arc<CatalogOverlay>>,
    models: Arc<Models>,
}

/// See the module docs.
pub struct SessionCodemodeHost {
    settings: CodemodeSettings,
    session: OnceLock<Weak<AgentSession>>,
    models: Mutex<Option<ComposedModels>>,
}

impl SessionCodemodeHost {
    pub(crate) fn new(settings: CodemodeSettings) -> Self {
        Self {
            settings,
            session: OnceLock::new(),
            models: Mutex::new(None),
        }
    }

    /// Bind the session this host acts for. Weak, so the host (which the extension's slot keeps
    /// alive for the process) never keeps a session alive.
    pub(crate) fn attach(&self, session: Weak<AgentSession>) {
        let _ = self.session.set(session);
    }

    fn session(&self) -> Option<Arc<AgentSession>> {
        self.session.get().and_then(Weak::upgrade)
    }
}

fn error_outcome(call_id: &ToolCallId, text: &str) -> NestedOutcome {
    NestedOutcome {
        call_id: call_id.clone(),
        result: ToolResult {
            content: vec![Content::text(text)],
            is_error: true,
            ..ToolResult::default()
        },
        is_error: true,
    }
}

#[async_trait::async_trait]
impl CodemodeHost for SessionCodemodeHost {
    fn mode(&self) -> CodemodeMode {
        self.settings.mode
    }

    fn inline_budget(&self) -> f64 {
        self.settings.inline_budget
    }

    fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.session()
            .map(|session| session.callable_tools())
            .unwrap_or_default()
    }

    async fn execute_nested(
        &self,
        caller: &ToolCallId,
        name: &str,
        args: Value,
        cancel: CancelToken,
    ) -> NestedOutcome {
        let Some(session) = self.session() else {
            return error_outcome(caller, "The session is gone");
        };
        let outcome = session
            .execute_nested_tool(
                caller,
                name,
                args,
                NestedToolCallOptions {
                    cancel: Some(cancel),
                    on_update: None,
                },
            )
            .await;
        NestedOutcome {
            call_id: outcome.tool_call.id.clone(),
            result: outcome.result,
            is_error: outcome.is_error,
        }
    }

    async fn branch_custom_entries(&self) -> Vec<BranchCustomEntry> {
        let Some(session) = self.session() else {
            return Vec::new();
        };
        let manager = session.manager.lock().await;
        manager
            .branch_path(None)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Known(KnownEntry::Custom {
                    custom_type, data, ..
                }) => Some(BranchCustomEntry {
                    custom_type: custom_type.clone(),
                    data: data.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    async fn append_store_entry(
        &self,
        data: CodemodeStoreEntryData,
    ) -> Result<(), StoreAppendFailed> {
        let session = self
            .session()
            .ok_or_else(|| StoreAppendFailed("the session is gone".to_owned()))?;
        let data = serde_json::to_value(&data).map_err(|e| StoreAppendFailed(e.to_string()))?;
        session
            .append_custom_entry(CODEMODE_STORE_ENTRY_TYPE, data)
            .await
            .map_err(|e| StoreAppendFailed(e.to_string()))
    }

    fn has_model_access(&self) -> bool {
        // The session always has a registry to compose, once it exists.
        true
    }

    fn models(&self) -> Option<Arc<dyn CodemodeModels>> {
        let session = self.session()?;
        let provider = session.provider.current();
        let guest_generation = session.services.guest_providers.generation();
        let overlay = session.services.catalog_overlay.load();
        let mut cached = self.models.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(hit) = cached.as_ref().filter(|composed| {
            Arc::ptr_eq(&composed.provider, &provider)
                && composed.guest_generation == guest_generation
                && match (&composed.overlay, &overlay) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                }
        }) {
            return Some(Arc::clone(&hit.models) as Arc<dyn CodemodeModels>);
        }
        let models = Arc::new(session.compose_codemode_models(&provider, overlay.clone()));
        *cached = Some(ComposedModels {
            provider,
            guest_generation,
            overlay,
            models: Arc::clone(&models),
        });
        Some(models as Arc<dyn CodemodeModels>)
    }
}

impl AgentSession {
    /// Append a custom (non-LLM) entry to the live session tree and announce it (pi
    /// `pi.appendEntry` → `sessionManager.appendCustomEntry` + `entry_appended`,
    /// `agent-session.ts:2265-2271`). The async twin of the guest-facing
    /// [`cyrup_ext::host::HostServices::append_entry`], which cannot wait for the manager and
    /// refuses when a turn holds it; a tool running mid-turn must wait instead.
    pub(crate) async fn append_custom_entry(
        &self,
        custom_type: &str,
        data: Value,
    ) -> Result<(), cyrup_session::SessionError> {
        let entry = {
            let mut manager = self.manager.lock().await;
            let id = manager.append_custom_entry(custom_type, Some(data))?;
            manager
                .entry(&id)
                .and_then(|entry| serde_json::to_value(entry).ok())
                .unwrap_or(Value::Null)
        };
        self.fanout_emit(AgentSessionEvent::EntryAppended { entry })
            .await;
        Ok(())
    }

    /// The registry the script `models` globals call: the composed built-in registry (the pi.dev
    /// overlay and `models.json` over the compiled-in catalogs) with the session's credentials,
    /// the guest-registered providers, and the session's own installed provider on top —
    /// pi's `ModelRuntime` registry (`rebuildProviders`, `model-runtime.ts:225-231`).
    fn compose_codemode_models(
        &self,
        installed: &Arc<dyn Provider>,
        overlay: Option<Arc<CatalogOverlay>>,
    ) -> Models {
        let (mut models, _composition_errors) = cyrup_config::compose_provider_registry(
            &self.services.model_config,
            CreateModelsOptions {
                credentials: Some(cyrup_config::login::runtime_credentials(
                    self.services.auth.clone(),
                )),
                auth_context: None,
                catalog_overlay: overlay,
            },
        );
        for id in self.services.guest_providers.ids() {
            if let Some(provider) = self.services.guest_providers.provider(&id) {
                models.set_provider(provider);
            }
        }
        models.set_provider(Arc::clone(installed));
        models
    }
}
