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
}

impl ToolSearchExtension {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// Ambient: pi's `builtin:tool-search` is a path in the tier `--no-extensions` collapses
    /// (`package-manager.ts:972-974`, `resource-loader.ts:706-730` @v1.0.1), as `codemode`'s is.
    fn is_ambient(&self) -> bool {
        true
    }

    /// Hidden from the startup `[Extensions]` listing, as pi marks every `builtin:` extension
    /// (`resource-loader.ts:729`; EXT-092).
    fn is_hidden(&self) -> bool {
        true
    }
}
