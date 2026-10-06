//! The built-in extension that registers the `codemode` tool (pi
//! `extensions/codemode/index.ts` @v1.0.1, with the table entry in `extensions/index.ts:9-14`).
//!
//! `codemode` is registered inactive. Activate it with `--tools`, the `defaultTools` setting
//! (`+codemode`), or `set_active_tools_by_name`.
//!
//! # Production call path
//!
//! The binary attaches [`CodemodeExtension`] as a native built-in (`cyrup::session_launch`,
//! `attach_native_extensions`) through `SessionFactory::with_codemode`, which also binds the
//! session into [`CodemodeExtension::host`] when each session is built. The session builder then
//! loads the extension like every other native, and its `init` registers the tool.

use std::sync::Arc;

use cyrup_config::CodemodeMode;
use cyrup_core::ExtensionId;
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};

use crate::tool::{CodemodeHostSlot, CodemodeTool, CodemodeToolOptions, SandboxFactory};

/// The id the extension loads under.
pub const EXTENSION_ID: &str = "codemode";

/// `createCodemodeExtension(options)` (`index.ts:31-45`).
#[derive(Clone)]
pub struct CodemodeExtension {
    options: CodemodeToolOptions,
}

impl CodemodeExtension {
    /// An extension whose scripts run in sandboxes from `sandboxes` and see the session bound into
    /// `host`.
    #[must_use]
    pub fn new(host: CodemodeHostSlot, sandboxes: Arc<dyn SandboxFactory>) -> Self {
        Self {
            options: CodemodeToolOptions::new(host, sandboxes),
        }
    }

    /// Overrides the `codemode.mode` setting (`CodemodeExtensionOptions.mode`).
    #[must_use]
    pub fn with_mode(mut self, mode: CodemodeMode) -> Self {
        self.options.mode = Some(mode);
        self
    }

    /// Overrides the `codemode.inlineBudget` setting (`CodemodeExtensionOptions.inlineBudget`).
    #[must_use]
    pub fn with_inline_budget(mut self, budget: f64) -> Self {
        self.options.inline_budget = Some(budget);
        self
    }

    /// Whether scripts get the `models` namespace when the session has model access. Default
    /// `true` (`CodemodeExtensionOptions.models`).
    #[must_use]
    pub fn with_models(mut self, models: bool) -> Self {
        self.options.models = models;
        self
    }

    /// Where the session this extension's tool runs in is bound.
    #[must_use]
    pub fn host(&self) -> &CodemodeHostSlot {
        &self.options.host
    }
}

#[async_trait::async_trait]
impl NativeExtension for CodemodeExtension {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(EXTENSION_ID)
    }

    /// Registers the tool (`pi.registerTool({ ...createCodemodeToolDefinition(...), defaultActive:
    /// false })`, `index.ts:36-43`). The tool declares `default_active() == false` itself, which is
    /// where cyrup reads that flag.
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(Arc::new(CodemodeTool::new(self.options.clone())));
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    /// Ambient: pi's `builtin:codemode` is a path in the tier `--no-extensions` collapses
    /// (`package-manager.ts:972-974`, `resource-loader.ts:706-730` @v1.0.1), so the flag drops it
    /// here too.
    fn is_ambient(&self) -> bool {
        true
    }

    /// Replaceable: pi's entry is `{ name: "codemode", factory, replaceable: true, builtin: true }`
    /// (`extensions/index.ts:11-12` @v1.0.1), so an extension that registers a tool, command or flag
    /// named like one of this extension's takes over instead of colliding with it
    /// (`omitReplacedExtensions`, `resource-loader.ts:116-153`).
    fn replaceable(&self) -> bool {
        true
    }

    /// Hidden from the startup `[Extensions]` listing, as pi marks every `builtin:` extension
    /// (`resource-loader.ts:729`; EXT-092).
    fn is_hidden(&self) -> bool {
        true
    }
}
