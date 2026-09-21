//! The FROZEN CONTRACT's trait half — the three injection seams that make this subsystem testable
//! on a box with no herdr, no ghostty and no macOS.
//!
//! Written once, by the orchestrator, before any implementation. **Implementations do not edit
//! this file.** A change here changes every consumer at once and is the orchestrator's call.
//!
//! Upstream: `src/inspectors/plugins.ts` (8 lines) and the `InspectorPlugin` shape in
//! `src/inspectors/types.ts` @v0.68.0.

use cyrup_core::{ToolError, ToolResult};

use super::types::{CommandOutput, InspectorContext, InspectorLaunch, InspectorParams};

/// One inspector backend — herdr or ghostty.
///
/// `status` and `close` return `Option<…>` because upstream's methods are OPTIONAL
/// (`status?`/`close?`). `None` is not a failure: it drives upstream's own
/// *"Inspector plugin '{name}' does not support status for async run {runId}."* /
/// *"… does not support close …"* (`actions.ts:141-147`, both `isError: true`, so both become
/// `Err(ToolError)` in cyrup — see [`super::types`]'s module doc for that mapping).
#[async_trait::async_trait]
pub trait InspectorPlugin: Send + Sync {
    /// The plugin's stable name, as it appears in upstream's messages.
    fn name(&self) -> &'static str;

    /// Whether this backend's host is present and usable RIGHT NOW. Consulted before `owns`.
    async fn available(&self, ctx: &InspectorContext) -> bool;

    /// Whether this backend already holds a binding for this target. Cheap and synchronous: it
    /// reads the binding file, it does not talk to the host.
    fn owns(&self, ctx: &InspectorContext) -> bool;

    /// Open (or focus, when already open) an inspector for this target.
    async fn open(
        &self,
        ctx: &InspectorContext,
        launch: &InspectorLaunch,
        params: &InspectorParams,
    ) -> Result<ToolResult, ToolError>;

    /// Report this target's inspector. `None` ⇒ this backend has no status method.
    async fn status(&self, ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>>;

    /// Close this target's inspector. `None` ⇒ this backend has no close method.
    async fn close(&self, ctx: &InspectorContext) -> Option<Result<ToolResult, ToolError>>;
}

/// herdr's CLI/socket seam, so `herdr/` is testable against a fake.
///
/// The real implementation delegates to `cyrup_herdr` — the workspace's ONE herdr client. An
/// implementation that opens its own socket or spawns its own `herdr` process is a defect.
#[async_trait::async_trait]
pub trait HerdrClient: Send + Sync {
    /// Run one herdr operation, returning its JSON payload.
    async fn run(&self, args: &[&str]) -> Result<serde_json::Value, super::types::HerdrErrorCode>;
}

/// ghostty's `osascript` seam, so `ghostty/` is testable off macOS.
#[async_trait::async_trait]
pub trait GhosttyRunner: Send + Sync {
    /// Run one `osascript` invocation.
    async fn run(&self, args: &[&str]) -> Result<CommandOutput, super::types::HerdrErrorCode>;
}

/// The built-in backends, in the order [`super::actions`] consults them.
///
/// Upstream's `createBuiltinInspectorPlugins()` (`plugins.ts`) — herdr first, then ghostty. The
/// ORDER is load-bearing: `launch_for` takes the first plugin that is both `available` and either
/// `owns` this target or is the first available backend.
#[must_use]
pub fn builtin_inspector_plugins() -> Vec<Box<dyn InspectorPlugin>> {
    vec![
        Box::new(super::herdr::plugin::HerdrInspectorPlugin::new()),
        Box::new(super::ghostty::plugin::GhosttyInspectorPlugin::new()),
    ]
}
