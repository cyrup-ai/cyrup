//! The native EXTENSION registration seam, end to end through a real session build: a native that
//! calls `InitApi::register_virtual_model` during `init` must reach the session's virtual-model
//! registry, show up in the composed catalog and be selectable — and a registration the registry
//! refuses must become a startup diagnostic rather than a failed load.
//!
//! Upstream is pi @ **v1.0.4**: `pi.registerVirtualModel()` queues onto
//! `runtime.pendingVirtualModelRegistrations` (`core/extensions/loader.ts:226-228`), and
//! `createAgentSessionServices` drains the queue into `modelRuntime.registerVirtualModel` with a
//! per-item `try`/`catch` that pushes `Extension "{path}" error: {message}` and carries on
//! (`core/agent-session-services.ts:182-194`), then fires one
//! `refresh({ allowNetwork: false })` (`:194`).
//!
//! cyrup's equivalent is the builder's `bind_model_registry` flush: the extension host has loaded by
//! then (so every registration is queued) and the catalog snapshot the flush validates against is
//! built from the same three sources `compose_model_registry` unions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{ExtensionId, ModelThinkingLevel};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome, HostEvent};
use cyrup_provider::faux::{FauxConfig, FauxModelDefinition, FauxProvider};
use cyrup_provider::{
    ModelRoute, ModelRouteError, ModelRouteRequest, ModelRouter, Provider, VirtualModelDefinition,
    VirtualModelSpec,
};
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig};

struct NoRouter;

#[async_trait::async_trait]
impl ModelRouter for NoRouter {
    async fn route(&self, _request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        Err(ModelRouteError::new("not routed in this test"))
    }
}

/// A native that registers ONE virtual model through the extension seam, exactly as a bundled
/// router does.
struct RouterExt {
    provider: String,
    id: String,
}

#[async_trait::async_trait]
impl NativeExtension for RouterExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("builtin:test-router")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_virtual_model(VirtualModelDefinition::new(
            VirtualModelSpec {
                provider: self.provider.as_str().into(),
                id: self.id.as_str().into(),
                name: "Auto".to_string(),
                thinking_levels: Some(vec![ModelThinkingLevel::Off, ModelThinkingLevel::High]),
                context_window: None,
                max_tokens: None,
                input: None,
            },
            Arc::new(NoRouter),
        ));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

fn faux() -> Arc<FauxProvider> {
    Arc::new(FauxProvider::with_config(FauxConfig {
        models: ["small", "large"]
            .iter()
            .map(|id| {
                let mut d = FauxModelDefinition::new(*id);
                d.reasoning = true;
                d.context_window = 200_000;
                d
            })
            .collect(),
        ..FauxConfig::default()
    }))
}

async fn session_with(provider: &str, id: &str) -> (TempDir, AgentSession) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    for d in [&cwd, &agent_dir] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    // The extension host must be built, else nothing registers.
    cfg.no_extensions = false;
    cfg.session_dir = Some(tmp.path().join("sessions"));
    let session = SessionBuilder::new(faux() as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(RouterExt {
            provider: provider.to_string(),
            id: id.to_string(),
        }))
        .build()
        .await
        .expect("the session builds");
    (tmp, session)
}

/// A virtual model a native registered at `init` is in the catalog, is AVAILABLE under a provider
/// id nothing physical defines, and is selectable.
///
/// This is the whole feature's reachability proof: without it nothing in the shipped binary can put
/// a virtual model in front of a user.
///
/// RED-PROVE: remove the `guest_providers.attach_virtual_models(..)` call from `SessionBuilder`'s
/// bind block — the sink refuses every registration with "virtual-model registry is not bound to
/// this session", `router/auto` never appears in the catalog and all three assertions fail (the
/// refusal does surface as a startup diagnostic, which is how the removal would be noticed).
/// Separately, drop the `virtual_model_hub.bind(sink)` flush from
/// `ExtensionRegistry::bind_model_registry` — the same three fail, with no diagnostic at all.
#[tokio::test]
async fn an_extension_registered_virtual_model_is_in_the_catalog_and_selectable() {
    let (_tmp, session) = session_with("router", "auto").await;
    assert!(
        session.services().startup_diagnostics.extensions.is_empty(),
        "{:?}",
        session.services().startup_diagnostics.extensions
    );

    let catalog = session.full_model_catalog();
    let row = catalog
        .iter()
        .find(|m| m.provider.as_str() == "router" && m.id.as_str() == "auto")
        .expect("router/auto is in the composed catalog");
    assert!(cyrup_provider::is_virtual_model(row));

    // Available: pi marks a provider of only virtual models configured, because it needs no
    // credentials (`model-runtime.ts:962-968`).
    assert!(
        session
            .available_model_catalog()
            .iter()
            .any(|m| m.provider.as_str() == "router" && m.id.as_str() == "auto"),
        "a virtual-only provider's models must be available"
    );

    let selected = session.set_model("router/auto").await.expect("selectable");
    assert_eq!(
        (selected.provider.as_str(), selected.model.as_str()),
        ("router", "auto")
    );
    assert_eq!(
        selected.api.as_ref().map(cyrup_core::ApiId::as_str),
        Some(cyrup_core::VIRTUAL_MODEL_API)
    );
}

/// A registration whose id already names a PHYSICAL model of that provider is refused, and the
/// refusal is a startup diagnostic in pi's own wording — the extension still loads and the physical
/// model is untouched.
///
/// pi: `throw new Error(`Virtual model ${providerId}/${id} conflicts with a physical model.`)`
/// (`model-runtime.ts:958-960`), caught and pushed as `Extension "{path}" error: {message}`
/// (`agent-session-services.ts:185-191`).
///
/// RED-PROVE: `?`-propagate the refusal out of `VirtualModelHub::bind` instead of collecting it —
/// the session build fails outright and `expect("the session builds")` panics, so this test dies
/// rather than asserting. Separately, swallow the refusal without recording it — the diagnostic
/// assertion fails and an extension author gets no explanation at all.
#[tokio::test]
async fn a_conflicting_registration_becomes_a_startup_diagnostic() {
    let (_tmp, session) = session_with("faux", "small").await;
    let diags = &session.services().startup_diagnostics.extensions;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].path, PathBuf::from("builtin:test-router"));
    assert_eq!(
        diags[0].error,
        "Extension \"builtin:test-router\" error: Virtual model faux/small conflicts with a \
         physical model."
    );
    assert!(
        !diags[0].fatal,
        "a refused registration is not a load failure"
    );

    // The physical model is still physical, and still the only `faux/small`.
    let rows: Vec<_> = session
        .full_model_catalog()
        .iter()
        .filter(|m| m.provider.as_str() == "faux" && m.id.as_str() == "small")
        .cloned()
        .collect();
    assert_eq!(rows.len(), 1);
    assert!(!cyrup_provider::is_virtual_model(&rows[0]));
}
