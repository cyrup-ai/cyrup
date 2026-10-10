//! The built-in extension that registers `tool_search` (pi `extensions/tool-search/index.ts`
//! @v1.0.1, with the table entry `{ name: "tool-search", replaceable: true, builtin: true }` in
//! `extensions/index.ts:12`).
//!
//! `tool_search` is registered inactive. Activate it with `--tools`, the `defaultTools` setting
//! (`+tool_search`), or `set_active_tools_by_name`.
//!
//! # Production call path
//!
//! The binary attaches [`ToolSearchExtension`] as a native built-in (`cyrup::session_launch`,
//! `attach_native_extensions`). The session builder loads it with
//! `ExtensionHost::load_native_with_services`, which calls [`NativeExtension::set_host_services`]
//! and then [`NativeExtension::init`]; `init` registers a tool bound to that session.

use std::sync::Arc;
use std::sync::{Mutex, PoisonError};

use cyrup_core::ExtensionId;
use cyrup_ext::{
    ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};

use crate::tool::ToolSearchTool;

/// The id the extension loads under.
pub const EXTENSION_ID: &str = "tool-search";

/// `createToolSearchExtension()` (`index.ts:15-19`).
#[derive(Default)]
pub struct ToolSearchExtension {
    /// The session the next `init` registers its tool for. One extension value is loaded into every
    /// session a factory builds, and each load binds before it initialises, so `init` takes what
    /// this holds at that moment and a later session's bind does not reach an earlier tool.
    session: Mutex<Option<Arc<dyn HostServices>>>,
    /// Named on the command line by `-e builtin:tool-search` (EXT-094); see
    /// [`ToolSearchExtension::loaded_explicitly`].
    explicit: bool,
}

impl ToolSearchExtension {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The extension as `-e builtin:tool-search` loads it (EXT-094): an explicit `-e` path, which pi
    /// keeps under `--no-extensions` (`noExtensions ? cliEnabledExtensions : …`,
    /// `core/resource-loader.ts`), so it is not [ambient](NativeExtension::is_ambient).
    #[must_use]
    pub fn loaded_explicitly(mut self) -> Self {
        self.explicit = true;
        self
    }
}

#[async_trait::async_trait]
impl NativeExtension for ToolSearchExtension {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(EXTENSION_ID)
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = Some(services);
    }

    /// Registers the tool (`pi.registerTool({ ...createToolSearchToolDefinition({ tools: pi }),
    /// defaultActive: false })`, `index.ts:16-18`). The tool declares `default_active() == false`
    /// itself, which is where cyrup reads that flag.
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        let session = self
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        api.register_tool(Arc::new(ToolSearchTool::new(session)));
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    /// Ambient: pi's `builtin:tool-search` is a settings-resolved path, the tier `--no-extensions`
    /// collapses (the built-in loop in `resolve()`, `core/package-manager.ts:971-985`;
    /// `noExtensions ? cliEnabledExtensions : …`, `core/resource-loader.ts:578` @f1b2e77f5), as
    /// `codemode`'s is — unless `-e builtin:tool-search` named it, which puts it in
    /// `cliEnabledExtensions` ([`ToolSearchExtension::loaded_explicitly`]).
    fn is_ambient(&self) -> bool {
        !self.explicit
    }

    /// Hidden from the startup `[Extensions]` listing, as pi marks every `builtin:` extension
    /// (`resource-loader.ts:729`; EXT-092).
    fn is_hidden(&self) -> bool {
        true
    }
}
