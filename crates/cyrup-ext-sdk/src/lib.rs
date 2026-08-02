//! cyrup-ext-sdk — the guest SDK for authoring cyrup extensions in Rust (arch-08; binds ADR-0002).
//!
//! Compiled to `wasm32-wasip2` (cdylib) it implements the `cyrup:ext` WIT world and emits a loadable
//! COMPONENT; compiled for the host (rlib) the ergonomic layer (events, descriptors, outcomes,
//! dispatch) is unit-testable. An author builds an [`ExtensionApi`] in a factory, subscribing to any
//! of the 30 lifecycle events with typed `(event, &Ctx) -> Outcome` handlers and registering
//! tools/commands/shortcuts/flags/providers/renderers/autocomplete — the Rust analog of Pi's
//! `ExtensionAPI` (extensions/types.ts:1128-1356).
//!
//! ## Modules
//! - [`api`] — [`ExtensionApi`], [`Outcome`], tool execution ([`ToolExec`]/[`ToolOutput`]).
//! - [`events`] — the 30 typed event payloads + per-event result shapes.
//! - [`ctx`] — [`Ctx`]/[`CommandCtx`]/[`Ui`]/[`Session`]/[`Models`] capability wrappers.
//! - [`descriptor`] — tool/command/flag/provider descriptors.
//! - [`example`] — a bundled reference extension (the live-E2E fixture).
//! - `guest` (wasm32) — the `wit-bindgen` glue implementing the world's exports.
//!
//! `unsafe` is confined to the `wit-bindgen`-generated component export ABI in [`guest`] (the
//! `#[export_name]` shims); all author-facing code is safe Rust.

pub mod api;
pub mod autocomplete;
pub mod ctx;
pub mod descriptor;
pub mod events;
pub mod example;
pub mod macros;
pub mod provider;
pub mod tool_factory;
pub mod widget;

#[cfg(target_arch = "wasm32")]
pub mod guest;

pub use api::{
    CommandExec, ContentBlock, ExtensionApi, MessageRenderer, Outcome, RawOutcome,
    RegisteredCommand, RegisteredRenderer, RegisteredShortcut, RegisteredTool, ShortcutExec,
    ToolExec, ToolOutput,
};
pub use autocomplete::{
    AutocompleteItem, AutocompleteProvider, AutocompleteQuery, AutocompleteSuggestions,
};
pub use ctx::{
    CommandCtx, Ctx, ExecResult, HttpRequest, HttpResponse, HttpStreamResponse, Models, NotifyKind,
    ProcSpawnOptions, ReplacedSessionContext, Session, Signal, ToolCall, Ui,
};
pub use descriptor::{
    CommandDescriptor, DialogOptions, ExecMode, ExecOptions, FlagSpec, ForkOptions, ForkPosition,
    ModelCost, NavigateOptions, NewSessionOptions, ProviderConfig, ProviderModelConfig, RenderShell,
    SwitchSessionOptions, ToolDescriptor,
};
pub use events::*;
pub use provider::{
    OAuthCallbacks, OAuthCredentials, OAuthProvider, ProviderHandlers, ProviderStream, StreamSimple,
};
pub use tool_factory::define_tool;

/// The author-facing import surface: `use cyrup_ext_sdk::prelude::*;`.
pub mod prelude {
    pub use crate::api::{
        CommandExec, ContentBlock, ExtensionApi, MessageRenderer, Outcome, ShortcutExec, ToolExec,
        ToolOutput,
    };
    pub use crate::autocomplete::{
        AutocompleteItem, AutocompleteProvider, AutocompleteQuery, AutocompleteSuggestions,
    };
    pub use crate::ctx::{
        CommandCtx, Ctx, HttpRequest, HttpResponse, HttpStreamResponse, Models, NotifyKind,
        ProcSpawnOptions, ReplacedSessionContext, Session, Signal, ToolCall, Ui,
    };
    pub use crate::descriptor::{
        CommandDescriptor, DialogOptions, ExecMode, FlagSpec, ForkOptions, ForkPosition,
        NavigateOptions, NewSessionOptions, ProviderConfig, ProviderModelConfig,
        SwitchSessionOptions, ToolDescriptor,
    };
    pub use crate::events::*;
    pub use crate::provider::{
        OAuthCallbacks, OAuthCredentials, OAuthProvider, ProviderHandlers, ProviderStream,
        StreamSimple,
    };
    /// The serialized widget tree a renderer returns (EXT-006). Exported as a MODULE, not flat
    /// names, so `widget::text(..)` reads at the call site and cannot collide with the `text`
    /// field/method names an extension author already has in scope.
    pub use crate::widget;
    pub use crate::tool_factory::define_tool;
}

/// The factory the wasm guest `init` calls to build the extension (arch-08 §3.6). This crate ships
/// the bundled [`example`] demo; an external author replaces it via [`export_extension!`].
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn extension_factory() -> ExtensionApi {
    example::build()
}

// Export the bundled demo as THIS crate's `cyrup:ext` component (wasm32 only) through the public
// `export_extension!` macro — so the live end-to-end test loads a component produced by the very
// macro a third-party author would use, not a hand-written `guest.rs` copy (closes sdk gap #1).
#[cfg(target_arch = "wasm32")]
crate::export_extension!(crate::extension_factory);
