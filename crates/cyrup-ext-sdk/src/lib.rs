//! cyrup-ext-sdk — the guest SDK for authoring cyrup extensions in Rust (arch-08; binds ADR-0002).
//!
//! Compiled to `wasm32-wasip2` (cdylib) it implements the `cyrup:ext` WIT world and emits a loadable
//! COMPONENT; compiled for the host (rlib) the ergonomic layer (events, descriptors, outcomes,
//! dispatch) is unit-testable. An author builds an [`ExtensionApi`] in a factory, subscribing to any
//! of the 33 lifecycle events with typed `(event, &Ctx) -> Outcome` handlers and registering
//! tools/commands/shortcuts/flags/providers/renderers/autocomplete — the Rust analog of Pi's
//! `ExtensionAPI` (extensions/types.ts:1128-1356).
//!
//! ## Modules
//! - [`prelude`] — the author entry point: `use cyrup_ext_sdk::prelude::*;`.
//! - [`api`] — [`ExtensionApi`], [`Outcome`], tool execution ([`ToolExec`]/[`ToolOutput`]).
//! - [`events`] — the 30 typed event payloads + per-event result shapes (33 subscribable events;
//!   30 payload structs, because some events share one — [`SessionLifecycleEvent`] serves both
//!   `session_start` and `session_shutdown` — and `agent_start`/`agent_settled` carry none).
//! - [`ctx`] — [`Ctx`]/[`CommandCtx`]/[`Ui`]/[`Session`]/[`Models`] capability wrappers.
//! - [`descriptor`] — tool/command/flag/provider descriptors.
//! - [`tool_factory`] — [`define_tool`], plus the `bash`/`read`/`write` descriptor builders
//!   ([`tool_factory::bash_descriptor`]/[`read_descriptor`](tool_factory::read_descriptor)/[`write_descriptor`](tool_factory::write_descriptor)),
//!   which are NOT re-exported at the crate root.
//! - [`autocomplete`] — the stacked [`AutocompleteProvider`] chain behind
//!   [`ExtensionApi::add_autocomplete_provider`].
//! - [`provider`] — the guest-side half of [`ExtensionApi::register_provider_with_handlers`]:
//!   [`ProviderHandlers`], carrying the [`OAuthProvider`] closures
//!   (`login`/`refresh_token`/`get_api_key`/`modify_models`) and `stream_simple` — the callbacks
//!   that cannot cross the seam as serialized JSON the way [`ProviderConfig`] does.
//! - [`widget`] — constructors for the serialized widget tree a `render_call`/`render_result`
//!   renderer returns.
//! - [`macros`] — the authoring guide for [`export_extension!`](crate::export_extension); the macro
//!   itself is `#[macro_export]`ed, so it lives at the crate root, not in that module.
//! - [`example`] — a bundled reference extension (the live-E2E fixture).
//! - `guest` (wasm32) — the `wit-bindgen` glue implementing the world's exports.
//!
//! `unsafe` is confined to the `wit-bindgen`-generated component export ABI in `guest` (the
//! `#[export_name]` shims); all author-facing code is safe Rust.

// A doc link that names an item that does not exist, or a public doc that points at a private one,
// is the failure mode a reader cannot detect without grepping — deny it at build time rather than
// letting warnings accumulate. This covers only what rustdoc visits by default: the private
// `src/ctx/*` submodules need `cargo doc --document-private-items` (see `ctx`'s module doc).
#![deny(rustdoc::broken_intra_doc_links, rustdoc::private_intra_doc_links)]
// An undocumented public item draws no rustdoc warning at all, so without this lint the count grows
// unobserved, which is how this crate accumulated its backlog. Unlike the two rustdoc lints above,
// `missing_docs` is a RUSTC lint: `cargo check` enforces it over every module the compiler sees —
// the private `src/ctx/*` submodules included, with no `--document-private-items` needed. The one
// exemption is `guest::bindings`, whose public items are all emitted by `wit_bindgen::generate!`
// and so have no source line to hang a `///` on; it carries its own inner `allow`.
#![warn(missing_docs)]

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

#[cfg(test)]
mod tests;

pub use api::{
    ArgCompleter, BashOperations, CommandExec, ContentBlock, ExtensionApi,
    MarkdownTransformContext, MarkdownTransformer, MessageRenderer, Outcome, RawOutcome,
    RegisteredCommand, RegisteredRenderer, RegisteredShortcut, RegisteredTool, RenderOptions,
    ShortcutExec, TerminalInputHandler, TerminalInputResult, ToolExec, ToolOutput,
};
pub use autocomplete::{
    AutocompleteItem, AutocompleteProvider, AutocompleteQuery, AutocompleteSuggestions,
};
pub use ctx::{
    BashCommand, CommandCtx, Ctx, ExecResult, ExtMode, HttpRequest, HttpResponse,
    HttpStreamResponse, Models, NotifyKind, ProcSpawnOptions, ReplacedSessionContext, Session,
    Signal, ToolCall, Ui,
};
pub use descriptor::{
    CommandDescriptor, CompactOptions, ConstrainedSampling, ConstrainedSamplingConfig,
    DialogOptions, ExecMode, ExecOptions, FlagSpec, ForkOptions, ForkPosition, GrammarVariants,
    ModelCost, ModelCostTier, NavigateOptions, NewSessionOptions, ProviderConfig,
    ProviderModelConfig, RenderShell, StrictSampling, SwitchSessionOptions, ToolDescriptor,
};
pub use events::*;
pub use provider::{
    OAuthCallbacks, OAuthCredentials, OAuthProvider, ProviderHandlers, ProviderStream, StreamSimple,
};
pub use tool_factory::define_tool;
pub use widget::WidgetPlacement;

/// The author-facing import surface: `use cyrup_ext_sdk::prelude::*;`.
///
/// This list is the set-equal twin of the crate-root flat re-exports above — the same names, from
/// the same modules — with one deliberate difference: the root names [`crate::widget`] through its
/// `pub mod widget;` declaration rather than a `pub use`. `tests::prelude_export_parity` pins that
/// equality, so a name added to one list and forgotten in the other breaks this crate's build
/// instead of a downstream author's.
pub mod prelude {
    pub use crate::api::{
        ArgCompleter, BashOperations, CommandExec, ContentBlock, ExtensionApi,
        MarkdownTransformContext, MarkdownTransformer, MessageRenderer, Outcome, RawOutcome,
        RegisteredCommand, RegisteredRenderer, RegisteredShortcut, RegisteredTool, RenderOptions,
        ShortcutExec, TerminalInputHandler, TerminalInputResult, ToolExec, ToolOutput,
    };
    pub use crate::autocomplete::{
        AutocompleteItem, AutocompleteProvider, AutocompleteQuery, AutocompleteSuggestions,
    };
    pub use crate::ctx::{
        BashCommand, CommandCtx, Ctx, ExecResult, ExtMode, HttpRequest, HttpResponse,
        HttpStreamResponse, Models, NotifyKind, ProcSpawnOptions, ReplacedSessionContext, Session,
        Signal, ToolCall, Ui,
    };
    pub use crate::descriptor::{
        CommandDescriptor, CompactOptions, ConstrainedSampling, ConstrainedSamplingConfig,
        DialogOptions, ExecMode, ExecOptions, FlagSpec, ForkOptions, ForkPosition, GrammarVariants,
        ModelCost, ModelCostTier, NavigateOptions, NewSessionOptions, ProviderConfig,
        ProviderModelConfig, RenderShell, StrictSampling, SwitchSessionOptions, ToolDescriptor,
    };
    pub use crate::events::*;
    pub use crate::provider::{
        OAuthCallbacks, OAuthCredentials, OAuthProvider, ProviderHandlers, ProviderStream,
        StreamSimple,
    };
    pub use crate::tool_factory::define_tool;
    /// The serialized widget tree a renderer returns (EXT-006). Exported as a MODULE, not flat
    /// names, so `widget::text(..)` reads at the call site and cannot collide with the `text`
    /// field/method names an extension author already has in scope. The crate root needs no
    /// `pub use` twin for this one: `pub mod widget;` already makes `cyrup_ext_sdk::widget`
    /// nameable there.
    pub use crate::widget;
    pub use crate::widget::WidgetPlacement;
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
