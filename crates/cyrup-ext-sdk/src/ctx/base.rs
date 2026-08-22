//! The base handler context: the `ctx-state` getters, the `bus` emit/unsubscribe pair and the two
//! `control` ops pi puts on the BASE `ExtensionContext` (`abort`/`shutdown`), plus [`ExtMode`] and
//! the [`Ctx`] type itself. The rest of `impl Ctx` lives beside the WIT interface each slice
//! fronts — `tools`, `exec`, `fs`, `http`, `proc`.
//!
//! [`Ctx::register_tool`] also lives here rather than in `tools`: it fronts no WIT import at all —
//! it hands a descriptor to the guest's own `register_tool_late` for the host to pick up at its
//! next tool refresh — so it belongs with the type rather than with the `ext-tools` introspection.

use serde::Serialize;

use super::{Models, Session, Ui};

/// The mode the host is running in (Pi `ExtensionMode`, types.ts:305 — `"tui" | "rpc" | "json" |
/// "print"`); the WIT `types.ext-mode` enum. Mirrored here rather than re-exported from the
/// generated bindings so the type also exists when the SDK is compiled for the host target (unit
/// tests), where the bindings module does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExtMode {
    #[default]
    Tui,
    Rpc,
    Json,
    Print,
}

impl ExtMode {
    /// The Pi wire spelling (`ExtensionMode`, types.ts:305).
    pub fn as_str(&self) -> &'static str {
        match self {
            ExtMode::Tui => "tui",
            ExtMode::Rpc => "rpc",
            ExtMode::Json => "json",
            ExtMode::Print => "print",
        }
    }
}

/// The capability context handed to every handler (event tier: no session mutation).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ctx;

impl Ctx {
    pub fn new() -> Self {
        Ctx
    }
    /// UI surface (R-08-022).
    pub fn ui(&self) -> Ui {
        Ui
    }
    /// Read-only session view + state persistence (R-08-026/027).
    pub fn session(&self) -> Session {
        Session
    }
    /// Model registry view. EXT-074: `set_model`/`set_thinking_level` are NOT command-only — this
    /// line said they were, and the host dropped that gate at GAP-11 (`cyrup-ext/src/host/live.rs`
    /// takes the ungated `guest_of` for both). pi binds both with only `assertActive`
    /// (`core/extensions/loader.ts:359-362` and `:369-372` @v0.83.0), so the ungated host is the
    /// parity-correct one and the comment was the stale half.
    pub fn models(&self) -> Models {
        Models
    }

    /// Convenience: post a transient notification.
    pub fn notify(&self, message: &str) {
        self.ui().notify(message);
    }

    /// Stop listening on an inter-extension bus topic (EXT-050) — the unsubscribe closure pi's
    /// `pi.events.on()` returns (`core/event-bus.ts:18-27` @v0.83.0), tracked by the loader since
    /// v0.84.1 (`extensions/loader.ts:413-421`). Before this a `subscribe` was permanent for the
    /// instance's life and a guest listening only while a mode was active had to filter by hand.
    pub fn unsubscribe(&self, topic: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::bus::unsubscribe(topic);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = topic;
    }

    /// Emit on the inter-extension event bus (R-08-029).
    pub fn emit(&self, topic: &str, payload: impl Serialize) {
        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::bus::emit(topic, &payload);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (topic, payload);
        }
    }

    /// Register a tool from inside a LIVE handler, after `init` (Pi `api.registerTool()` called at
    /// runtime — `examples/extensions/dynamic-tools.ts` registers from a `session_start` handler;
    /// `extensions/loader.ts:249-256` follows every registration with `runtime.refreshTools()`).
    /// The host re-materializes it into an executable handle at its next tool refresh, so the tool
    /// is model-visible on the following turn.
    pub fn register_tool(
        &self,
        descriptor: crate::descriptor::ToolDescriptor,
        exec: impl crate::api::ToolExec,
    ) {
        let tool = crate::api::RegisteredTool { descriptor, exec: Box::new(exec) };
        #[cfg(target_arch = "wasm32")]
        crate::guest::register_tool_late(tool);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = tool;
    }

    // --- base-context state + lifecycle (pi `ExtensionContext`, types.ts:307-347 @v0.83.0;
    // EXT-072: `:305` is `ExtensionMode`). Pi puts ALL of
    // these on the base context — "Available in all contexts" — so they live on `Ctx`, not on
    // `CommandCtx`, and the host does not tier-gate them (EXT-005). ---

    /// The mode the host is running in (Pi `ctx.mode`, types.ts:311). Pi's guidance: "Use `tui` to
    /// guard terminal-only UI such as custom components" — a widget or a custom renderer is
    /// meaningless under `json`/`print`, so branch on this before registering one.
    pub fn mode(&self) -> ExtMode {
        #[cfg(target_arch = "wasm32")]
        {
            use crate::guest::bindings::cyrup::ext::types::ExtMode as Wit;
            return match crate::guest::bindings::cyrup::ext::ctx_state::get_mode() {
                Wit::Tui => ExtMode::Tui,
                Wit::Rpc => ExtMode::Rpc,
                Wit::Json => ExtMode::Json,
                Wit::Print => ExtMode::Print,
            };
        }
        #[cfg(not(target_arch = "wasm32"))]
        ExtMode::Tui
    }

    /// Whether dialog-capable UI is available (Pi `ctx.hasUI`, types.ts:313 — "true in TUI and RPC
    /// modes"). Check this before [`Ui::confirm`]/[`Ui::input`]/[`Ui::select`]: with no UI those
    /// answer with their inert default rather than reaching a human.
    pub fn has_ui(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::has_ui();
        }
        #[cfg(not(target_arch = "wasm32"))]
        true
    }

    /// Whether no agent run is in flight (pi `ctx.isIdle()`, types.ts:330 @v0.83.0; EXT-072: `:333`
    /// is `signal`'s doc line).
    pub fn is_idle(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::is_idle();
        }
        #[cfg(not(target_arch = "wasm32"))]
        true
    }

    /// Whether user messages are queued for the next turn (Pi `ctx.hasPendingMessages()`,
    /// types.ts:338 @v0.83.0; EXT-072: `:341` is `getContextUsage`'s doc line).
    pub fn has_pending_messages(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::has_pending_messages();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }

    /// Whether the project is trusted (pi `ctx.isProjectTrusted()`, types.ts:332 @v0.83.0;
    /// EXT-072: `:335` is `abort`'s doc line).
    pub fn is_project_trusted(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::is_project_trusted();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }

    /// The host's current working directory — pi `ctx.cwd` (`extensions/types.ts:315` @v0.83.0),
    /// on the BASE `ExtensionContext` beside `mode` (:311) and `hasUI` (:313), so it is available
    /// to every handler and every tool `execute`, not just command handlers.
    ///
    /// EXT-044: without this a guest could resolve no relative path at all — it could not tell
    /// which project it was in, scope a cache, interpret a path in a tool argument, or compose one
    /// for its `ext-fs`/`exec` grant. cyrup's own native tier had always exposed it as
    /// `HostCtx.cwd`, so this was a divergence between cyrup's two tiers as well as against pi.
    pub fn cwd(&self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::get_cwd();
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Whether the RUN this handler is executing inside has been cancelled (EXT-045).
    ///
    /// pi exposes this as `signal: AbortSignal | undefined` on the base `ExtensionContext`
    /// (`extensions/types.ts:334` @v0.83.0, "The current abort signal, or undefined when the agent
    /// is not streaming"). CYRUP-DELTA: an `AbortSignal` is an event target, and a component-model
    /// value cannot be a callback target — so the guest POLLS rather than being woken. Check it
    /// between units of work in a long non-tool handler; a guest TOOL should keep using the
    /// per-call [`Signal::is_aborted`](crate::ctx::Signal::is_aborted) poll, which is the closer
    /// analog of upstream's `execute(…, signal, …)` parameter.
    pub fn is_run_cancelled(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::is_run_cancelled();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }

    /// The active system prompt (Pi `ctx.getSystemPrompt()`, types.ts:346); empty when no session
    /// backend is attached.
    pub fn system_prompt(&self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ctx_state::get_system_prompt();
        }
        #[cfg(not(target_arch = "wasm32"))]
        String::new()
    }

    /// Abort the in-flight agent run (Pi `ctx.abort()`, types.ts:336 @v0.83.0, doc "Abort the
    /// current agent operation" at `:335`; legal from every tier). EXT-073: the `:339 — available in
    /// all contexts` this line used to carry belongs to `shutdown`, not `abort`.
    pub fn abort(&self) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::control::abort();
        }
        #[cfg(not(target_arch = "wasm32"))]
        Ok(())
    }

    /// Request a graceful host shutdown (Pi `ctx.shutdown()`, types.ts:340 @v0.83.0, doc
    /// "Gracefully shutdown pi and exit. Available in all contexts." at `:339`). The host exits at
    /// its next settle point. EXT-073: `:344` is `compact`.
    pub fn shutdown(&self) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::control::shutdown();
        }
        #[cfg(not(target_arch = "wasm32"))]
        Ok(())
    }
}
