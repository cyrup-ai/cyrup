//! The `ExtensionHost` facade (arch-08 §3.1): the single entry point the session service wires in.
//! Holds the registry + dispatcher + native registry (+ the Wasmtime engine/pool when the
//! `wasm-host` feature is on). Exposes the two agent seams — [`ExtSubscriber`] (notify) and
//! [`ExtHooks`] (mutating) — plus the merged active tool set.

use crate::dispatch::Dispatcher;
use crate::error::ExtError;
use crate::hooks::ExtHooks;
use crate::native::{ExtMode, HostCtx, InitApi, NativeExtension, NativeHandle};
use crate::registry::ExtensionRegistry;
use crate::subscriber::ExtSubscriber;
use cyrup_agent::{EventSubscriber, Hooks};
use cyrup_core::{CancelToken, ExtensionId, Tool};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Configuration for the host (mode + cwd + UI availability drive the dispatch `HostCtx`).
#[derive(Clone, Debug)]
pub struct HostConfig {
    pub mode: ExtMode,
    pub has_ui: bool,
    pub cwd: PathBuf,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            mode: ExtMode::default(),
            has_ui: true,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

/// The extension host facade (arch-08 §3.1).
pub struct ExtensionHost {
    dispatcher: Arc<Dispatcher>,
    registry: Arc<ExtensionRegistry>,
    config: HostConfig,
    loaded: RwLock<Vec<ExtensionId>>,
    #[cfg(feature = "wasm-host")]
    wasm: Option<crate::host_runtime::WasmRuntime>,
}

impl Default for ExtensionHost {
    fn default() -> Self {
        Self::new(HostConfig::default())
    }
}

impl ExtensionHost {
    /// A native-only host foundation (no Wasmtime engine spun up). Sufficient for the full
    /// dispatch/registration/seam/containment surface (tested without wasm).
    pub fn new(config: HostConfig) -> Self {
        Self {
            dispatcher: Arc::new(Dispatcher::new()),
            registry: Arc::new(ExtensionRegistry::new()),
            config,
            loaded: RwLock::new(Vec::new()),
            #[cfg(feature = "wasm-host")]
            wasm: None,
        }
    }

    /// Load a compiled-in native extension (R-ARCH-EXT-003). Awaits `init` (R-08-001), registers its
    /// tools/commands, builds its subscription bitset, and wires it into the dispatcher in load order.
    pub async fn load_native(&self, ext: Arc<dyn NativeExtension>) -> Result<(), ExtError> {
        let id = ext.id();
        self.reserve_id(&id)?;

        let mut api = InitApi::new();
        ext.init(&mut api).await?;
        let (subs, tools, commands) = api.into_parts();

        for tool in tools {
            self.registry.register_tool(id.clone(), tool)?;
        }
        for (name, desc) in commands {
            self.registry.register_command(id.clone(), name, desc)?;
        }

        let ctx = HostCtx::event(self.config.mode, self.config.has_ui, self.config.cwd.clone());
        let handle = Arc::new(NativeHandle::new(ext, subs, ctx));
        self.dispatcher.add(handle)?;
        Ok(())
    }

    /// The ordered-awaited, notify-only subscriber handed to the agent (R-02-012/048).
    pub fn subscriber(&self, cancel: CancelToken) -> Arc<dyn EventSubscriber> {
        Arc::new(ExtSubscriber::new(self.dispatcher.clone(), cancel))
    }

    /// The mutating hooks adapter handed to the agent (arch-02 §3.3).
    pub fn hooks(&self) -> Arc<dyn Hooks> {
        Arc::new(ExtHooks::new(self.dispatcher.clone()))
    }

    /// The merged active tool set: built-ins overridden by extension tools (R-08-012/014).
    pub fn active_tools(&self, base: &[Arc<dyn Tool>]) -> Result<Vec<Arc<dyn Tool>>, ExtError> {
        self.registry.active_tools(base)
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    pub fn dispatcher(&self) -> &Dispatcher {
        &self.dispatcher
    }

    /// Ids of loaded extensions in load order.
    pub fn loaded_ids(&self) -> Vec<ExtensionId> {
        self.loaded.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Build a host with the Wasmtime runtime spun up (engine + instance pool + epoch driver).
    /// Must be called from within a tokio runtime. Behind the `wasm-host` feature (arch-08 §2).
    #[cfg(feature = "wasm-host")]
    pub fn with_wasm(config: HostConfig) -> Result<Self, ExtError> {
        let mut host = Self::new(config);
        host.wasm = Some(crate::host_runtime::WasmRuntime::new()?);
        Ok(host)
    }

    /// The Wasmtime runtime bundle, if this host was built with [`Self::with_wasm`].
    #[cfg(feature = "wasm-host")]
    pub fn wasm(&self) -> Option<&crate::host_runtime::WasmRuntime> {
        self.wasm.as_ref()
    }

    fn reserve_id(&self, id: &ExtensionId) -> Result<(), ExtError> {
        let mut g = self.loaded.write().map_err(|_| ExtError::Io("host lock poisoned".into()))?;
        if g.iter().any(|e| e == id) {
            return Err(ExtError::DuplicateId(id.to_string()));
        }
        g.push(id.clone());
        Ok(())
    }
}
