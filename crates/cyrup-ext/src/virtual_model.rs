//! Virtual-model registration host-side — pi `pi.registerVirtualModel()` /
//! `pi.unregisterVirtualModel()` (`extensions/types.ts:1865-1876` @v1.0.4), the pre-bind
//! `pendingVirtualModelRegistrations` queue (`extensions/loader.ts:190`, `:226-232`, `:500-513`) and
//! the bind-time flush (`extensions/runner.ts:498-514`, `core/agent-session-services.ts:182-194`).
//!
//! A virtual model is a selectable catalog entry that routes each request to a physical model. The
//! definition — spec plus router — is [`cyrup_provider::VirtualModelDefinition`]; this module is
//! only the registration lifecycle, written as the sibling of [`crate::provider::ProviderHub`]
//! because upstream gives the two the SAME lifecycle: queue while the extension loads, flush when
//! the model registry binds, apply directly afterwards. The docs page says so outright —
//! "Registration follows the same queuing and reload rules as `pi.registerProvider()`".
//!
//! The registry sink is [`crate::provider::ModelRegistrySink`], extended rather than duplicated:
//! upstream carries all five provider actions in ONE `providerActions` bag handed to one `bindCore`
//! (`runner.ts:413-418`), so a second trait with a second bind would be cyrup's invention.
//!
//! # [CYRUP-DELTA] owner attribution
//!
//! pi keeps `virtualModels` in the `ModelRuntime` with NO owner attribution
//! (`core/model-runtime.ts:179`) and removes an entry only on an explicit `unregisterVirtualModel`
//! (`:976-982`). After a pi `/reload`, a dead extension's router therefore stays registered and its
//! captured `runtime.createContext()` (`loader.ts:504-508`) throws against the runtime the reload
//! invalidated (`:191-200`). cyrup attributes each registration to its owning extension and drops
//! it with the owner (see [`crate::registry::ExtensionRegistry::purge_owner`]), which is strictly
//! better behaviour and is recorded here so a later reader does not "repair" it toward upstream.
//!
//! What is NOT a delta, and must stay: `unregisterProvider` does not remove virtual models. The
//! docs page is explicit — "`pi.unregisterVirtualModel(provider, id)` removes it;
//! `pi.unregisterProvider()` does not."

use std::sync::Arc;

use cyrup_provider::VirtualModelDefinition;

use crate::provider::ModelRegistrySink;

/// A virtual model's identity: the provider id it is listed under and its model id. pi's two-level
/// `Map<providerId, Map<id, …>>` key (`model-runtime.ts:179`), flattened — the registration ORDER
/// is observable (upstream pins `getModels("faux") == ["small","large","auto","fast"]`), so this is
/// an ordered `Vec` of pairs rather than a sorted map.
type VirtualKey = (String, String);

/// One queued or registered virtual model, with the extension path a failure is attributed to.
struct Registered {
    key: VirtualKey,
    definition: VirtualModelDefinition,
    /// pi's `extensionPath` on each pending entry (`loader.ts:226-232`), used to build the
    /// `Extension "{path}" error: {message}` diagnostic at flush time
    /// (`agent-session-services.ts:185-191`).
    extension_path: String,
}

/// A virtual-model registration that the sink refused, in pi's diagnostic shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualModelFlushError {
    /// The extension the registration came from.
    pub extension_path: String,
    /// The provider id the virtual model was listed under.
    pub provider: String,
    /// The virtual model's id.
    pub id: String,
    /// The sink's refusal text — upstream's own throw text, since the sink is
    /// `VirtualModelRegistry::register`.
    pub error: String,
}

impl VirtualModelFlushError {
    /// pi's diagnostic line verbatim: `Extension "{path}" error: {message}`
    /// (`agent-session-services.ts:188-190`).
    #[must_use]
    pub fn diagnostic(&self) -> String {
        format!(
            "Extension \"{}\" error: {}",
            self.extension_path, self.error
        )
    }
}

/// The virtual-model registration hub: resolved registrations, the pre-bind pending queue, and the
/// injected [`ModelRegistrySink`]. The sibling of [`crate::provider::ProviderHub`], with the same
/// three-state lifecycle (queue → bind+flush → direct).
#[derive(Default)]
pub struct VirtualModelHub {
    /// Every registration, in registration order. Re-registering a `(provider, id)` REPLACES the
    /// entry IN PLACE — pi's `Map.set` on an existing key keeps its position, and the position is
    /// observable in the model listing.
    registrations: Vec<Registered>,
    /// Keys registered before a sink was bound — flushed in order at [`Self::bind`]. pi's
    /// `pendingVirtualModelRegistrations` (`loader.ts:190`).
    pending: Vec<VirtualKey>,
    sink: Option<Arc<dyn ModelRegistrySink>>,
}

impl VirtualModelHub {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a virtual model — pi `registerVirtualModel` (`loader.ts:500-513` for
    /// the api, `:226-228` for the queue, `runner.ts:539-543` for the post-bind direct call).
    ///
    /// Returns the sink's refusal when a sink is bound and refuses (pi's `registerVirtualModel`
    /// throws for an empty provider/id and for an id that already names a PHYSICAL model); the
    /// registration is still recorded, because pi's own post-bind path propagates the throw to the
    /// caller and leaves the runtime untouched — and a recorded-but-unregistered entry is what
    /// makes a later [`Self::bind`] retry it, which is pi's behaviour when the first flush ran
    /// before the catalog existed.
    pub fn register(
        &mut self,
        definition: VirtualModelDefinition,
        extension_path: impl Into<String>,
    ) -> Result<(), String> {
        let key = (
            definition.spec.provider.as_str().to_string(),
            definition.spec.id.as_str().to_string(),
        );
        let entry = Registered {
            key: key.clone(),
            definition,
            extension_path: extension_path.into(),
        };
        let slot = match self.registrations.iter_mut().position(|r| r.key == key) {
            Some(at) => {
                if let Some(existing) = self.registrations.get_mut(at) {
                    *existing = entry;
                }
                at
            }
            None => {
                self.registrations.push(entry);
                self.registrations.len().saturating_sub(1)
            }
        };
        match (&self.sink, self.registrations.get(slot)) {
            // Hand the sink the STORED definition, not the argument: the stored one is what a later
            // re-bind would flush, so the sink and the hub can never disagree about a key.
            (Some(sink), Some(reg)) => sink.upsert_virtual_model(&reg.definition),
            (Some(_), None) => Ok(()),
            (None, _) => {
                self.pending.retain(|p| p != &key);
                self.pending.push(key);
                Ok(())
            }
        }
    }

    /// Remove ONE virtual model — pi `unregisterVirtualModel(provider, id)`
    /// (`model-runtime.ts:976-982`; the pre-bind form filters the queue by the same pair,
    /// `loader.ts:229-232`). Returns whether it was present, which is pi's no-op arm.
    ///
    /// Scoped to the pair: a sibling virtual model under the same provider id survives.
    pub fn unregister(&mut self, provider: &str, id: &str) -> bool {
        let key = (provider.to_string(), id.to_string());
        let had = self.registrations.iter().any(|r| r.key == key);
        self.registrations.retain(|r| r.key != key);
        self.pending.retain(|p| p != &key);
        if had && let Some(sink) = &self.sink {
            sink.remove_virtual_model(provider, id);
        }
        had
    }

    /// Bind the registry sink and FLUSH the pending registrations into it, in order — pi's
    /// `bindCore` loop (`runner.ts:502-514`) and the identical loop in
    /// `createAgentSessionServices` (`agent-session-services.ts:182-194`).
    ///
    /// A per-registration failure is CONTAINED and returned, never propagated: both upstream loops
    /// wrap each item in its own `try`/`catch`, emit a diagnostic and carry on, so one bad router
    /// does not cost the others their registration nor fail the extension load.
    pub fn bind(&mut self, sink: Arc<dyn ModelRegistrySink>) -> Vec<VirtualModelFlushError> {
        let mut errors = Vec::new();
        for key in std::mem::take(&mut self.pending) {
            let Some(reg) = self.registrations.iter().find(|r| r.key == key) else {
                continue;
            };
            if let Err(error) = sink.upsert_virtual_model(&reg.definition) {
                errors.push(VirtualModelFlushError {
                    extension_path: reg.extension_path.clone(),
                    provider: key.0,
                    id: key.1,
                    error,
                });
            }
        }
        self.sink = Some(sink);
        errors
    }

    /// Whether the registry sink has been bound (pi post-`bindCore`).
    #[must_use]
    pub fn is_bound(&self) -> bool {
        self.sink.is_some()
    }

    /// The `(provider, id)` pairs still queued for the next [`Self::bind`].
    #[must_use]
    pub fn pending_pairs(&self) -> Vec<(String, String)> {
        self.pending.clone()
    }

    /// Every registered `(provider, id)` pair, in registration order.
    #[must_use]
    pub fn keys(&self) -> Vec<(String, String)> {
        self.registrations.iter().map(|r| r.key.clone()).collect()
    }

    /// The definition registered under `(provider, id)`, if any.
    #[must_use]
    pub fn get(&self, provider: &str, id: &str) -> Option<&VirtualModelDefinition> {
        self.registrations
            .iter()
            .find(|r| r.key.0 == provider && r.key.1 == id)
            .map(|r| &r.definition)
    }
}
