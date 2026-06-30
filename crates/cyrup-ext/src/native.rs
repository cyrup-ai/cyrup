//! The native built-in extension path (arch-08 §3.2, R-ARCH-EXT-003). Compiled-in Rust extensions
//! implement [`NativeExtension`]; the same dispatch/registration machinery drives them WITHOUT any
//! wasm. A native built-in and a WASM component are interchangeable at the dispatch layer (both
//! become an [`Extension`] handle); only the call mechanism differs.

use crate::contract::HookOutcome;
use crate::error::ExtError;
use crate::event::{EventKind, HostEvent, Subscriptions};
use crate::extension::{ExtKind, Extension};
use crate::registry::CommandDescriptor;
use cyrup_core::{CancelToken, ExtensionId, Tool};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

/// Which context tier a handler runs in (arch-08 §6.3, the deadlock rule). Session-mutating control
/// ops are legal only from [`CtxTier::Command`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtxTier {
    Event,
    Command,
}

/// The runtime mode the host is in (arch-08 §6.3); UI degrades by mode (R-08-023).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExtMode {
    #[default]
    Tui,
    Rpc,
    Json,
    Print,
}

/// Context handed to an extension at dispatch (arch-08 §6.3). Event handlers get an `Event`-tier
/// ctx (no session mutation); command handlers get a `Command`-tier ctx. The host check is
/// authoritative even though the SDK also enforces it at the type level.
#[derive(Clone, Debug)]
pub struct HostCtx {
    pub mode: ExtMode,
    pub has_ui: bool,
    pub cwd: PathBuf,
    tier: CtxTier,
    /// Rich native-ctx fields (Pi `ExtensionContext`, types.ts:300-333). On the wasm path these are
    /// served by the `session`/`models`/`ui` capability imports; the native built-in path carries
    /// them inline so a built-in reaches the same surface without crossing a boundary (gap-08 #6).
    rich: HostCtxRich,
}

/// The richer fields a native built-in's [`HostCtx`] exposes (Pi `ExtensionContext`, types.ts:300-333):
/// the current model, idle/trust flags, the context-usage snapshot, and the active system prompt.
/// (`sessionManager`/`modelRegistry` remain seam-injected handles — arch-08 §5.6 — so they are not
/// inlined here; the data fields a built-in actually reads are.)
#[derive(Clone, Debug, Default)]
pub struct HostCtxRich {
    /// The current model ref (Pi `ctx.model`).
    pub model: Option<String>,
    /// Whether the agent is idle (Pi `ctx.isIdle`).
    pub is_idle: bool,
    /// Whether the project is trusted (Pi `ctx.isProjectTrusted`).
    pub is_project_trusted: bool,
    /// The context-usage snapshot (Pi `ctx.getContextUsage()`).
    pub context_usage: Option<serde_json::Value>,
    /// The active system prompt (Pi `ctx.getSystemPrompt()`).
    pub system_prompt: Option<String>,
}

impl HostCtx {
    /// An event-tier context (inside the agent/session flow): NO session mutation.
    pub fn event(mode: ExtMode, has_ui: bool, cwd: PathBuf) -> Self {
        Self { mode, has_ui, cwd, tier: CtxTier::Event, rich: HostCtxRich::default() }
    }

    /// A command-tier context (user-initiated, outside the loop): session mutation allowed.
    pub fn command(mode: ExtMode, has_ui: bool, cwd: PathBuf) -> Self {
        Self { mode, has_ui, cwd, tier: CtxTier::Command, rich: HostCtxRich::default() }
    }

    /// Attach the rich native-ctx fields (Pi `ExtensionContext`, gap-08 #6).
    pub fn with_rich(mut self, rich: HostCtxRich) -> Self {
        self.rich = rich;
        self
    }

    /// The rich native-ctx fields (model/idle/trust/usage/system-prompt).
    pub fn rich(&self) -> &HostCtxRich {
        &self.rich
    }

    /// The current model ref (Pi `ctx.model`).
    pub fn model(&self) -> Option<&str> {
        self.rich.model.as_deref()
    }
    /// Whether the agent is idle (Pi `ctx.isIdle`).
    pub fn is_idle(&self) -> bool {
        self.rich.is_idle
    }
    /// Whether the project is trusted (Pi `ctx.isProjectTrusted`).
    pub fn is_project_trusted(&self) -> bool {
        self.rich.is_project_trusted
    }
    /// The context-usage snapshot (Pi `ctx.getContextUsage()`).
    pub fn context_usage(&self) -> Option<&serde_json::Value> {
        self.rich.context_usage.as_ref()
    }
    /// The active system prompt (Pi `ctx.getSystemPrompt()`).
    pub fn system_prompt(&self) -> Option<&str> {
        self.rich.system_prompt.as_deref()
    }

    pub fn tier(&self) -> CtxTier {
        self.tier
    }

    /// Deadlock guard (R-08-008): returns `Err(ExtError::Deadlock)` if a session-mutating control
    /// op is attempted from an event handler. Authoritative regardless of the guest SDK's types.
    pub fn require_command_tier(&self) -> Result<(), ExtError> {
        if self.tier == CtxTier::Command {
            Ok(())
        } else {
            Err(ExtError::Deadlock)
        }
    }
}

/// The decomposed result of [`InitApi`]: the declared subscriptions, registered tools, and
/// registered `(name, descriptor)` commands.
pub(crate) type InitParts = (Subscriptions, Vec<Arc<dyn Tool>>, Vec<(String, CommandDescriptor)>);

/// What a native extension declares during [`NativeExtension::init`]: its subscriptions plus any
/// tools/commands it registers (arch-08 §3.5). Mirrors the guest's registration imports.
#[derive(Default)]
pub struct InitApi {
    subs: Subscriptions,
    tools: Vec<Arc<dyn Tool>>,
    commands: Vec<(String, CommandDescriptor)>,
}

impl InitApi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the subscription bitset (R-ARCH-EXT-014): which §5 event kinds this extension handles.
    pub fn subscribe(&mut self, kinds: &[EventKind]) {
        for &k in kinds {
            self.subs.add(k);
        }
    }

    /// Register a tool. Overrides a built-in of the same name at the registry (R-08-012).
    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Register a command (runs with a command-tier ctx; may call session ops, R-08-016).
    pub fn register_command(&mut self, name: impl Into<String>, desc: CommandDescriptor) {
        self.commands.push((name.into(), desc));
    }

    pub fn subscriptions(&self) -> Subscriptions {
        self.subs
    }

    pub(crate) fn into_parts(self) -> InitParts {
        (self.subs, self.tools, self.commands)
    }
}

/// Native built-ins implement this directly (arch-08 §3.2). First-party / promoted extensions
/// (R-ARCH-EXT-003/006) live here — full speed, in-process, no serialization.
#[async_trait::async_trait]
pub trait NativeExtension: Send + Sync {
    fn id(&self) -> ExtensionId;
    /// Registers tools/commands + declares subscriptions. Awaited before the extension goes live
    /// (R-08-001).
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError>;
    /// Handle one event. Returns this extension's block/mutate/notify contribution.
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome;

    /// Execute a registered slash command this extension owns (Pi `command.handler(args, ctx)`,
    /// agent-session.ts:1159; R-08-016). `ctx` is **command-tier** (session mutation allowed). The
    /// optional `String` is the command's text output (Pi commands return `void`; cyrup mirrors the
    /// WASM `execute-command` shape so the two paths are interchangeable). The default rejects: a
    /// native built-in that registers a command via [`InitApi::register_command`] MUST override this
    /// to service it. Built-ins that only subscribe to events leave it unimplemented.
    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        Err(ExtError::Component(format!("native extension has no handler for command `{name}`")))
    }
}

/// Wraps a `NativeExtension` into the unified [`Extension`] handle, applying panic containment
/// (R-08-036): a panicking handler is caught and surfaced as `ExtError::Panicked`, never crashing
/// the host. The chain then skips it (arch-08 §6.1).
pub struct NativeHandle {
    id: ExtensionId,
    subs: Subscriptions,
    ctx: HostCtx,
    inner: Arc<dyn NativeExtension>,
}

impl NativeHandle {
    pub fn new(
        inner: Arc<dyn NativeExtension>,
        subs: Subscriptions,
        ctx: HostCtx,
    ) -> Self {
        let id = inner.id();
        Self { id, subs, ctx, inner }
    }
}

#[async_trait::async_trait]
impl Extension for NativeHandle {
    fn id(&self) -> &ExtensionId {
        &self.id
    }

    fn kind(&self) -> ExtKind {
        ExtKind::Native
    }

    fn subscriptions(&self) -> &Subscriptions {
        &self.subs
    }

    async fn invoke_event(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        // Containment: catch a panicking handler (R-08-036). `AssertUnwindSafe` is sound here — on
        // a caught unwind we discard the handler's state and surface an error; we never resume it.
        let fut = AssertUnwindSafe(self.inner.on_event(ev, &self.ctx));
        let raced = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ExtError::Cancelled),
            r = fut.catch_unwind() => r,
        };
        match raced {
            Ok(outcome) => Ok(outcome),
            Err(panic) => Err(ExtError::Panicked(panic_msg(panic))),
        }
    }
}

fn panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}
