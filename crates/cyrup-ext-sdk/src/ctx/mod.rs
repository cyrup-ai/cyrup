//! The handler context wrappers (arch-08 §2.2/§6.3; Pi `ExtensionContext`/`ExtensionUIContext`/
//! `ExtensionCommandContext`, types.ts:131-404 @v0.83.0 — `ExtensionUIContext` `:131-282`,
//! `ExtensionContext` `:307-347`, `ExtensionCommandContext` `:353-387`, `ReplacedSessionContext`
//! `:394-404`; EXT-072: the `:124-390` this cited opens on `AutocompleteProviderFactory`). Every
//! event/tool handler receives a [`Ctx`], the
//! safe-Rust front for the `ui`/`session`/`models`/`exec`/`bus` capability imports. Command handlers
//! receive a [`CommandCtx`] which additionally exposes the COMMAND-only `control` ops — the
//! type-level half of the deadlock rule (the host check is authoritative, R-08-008).
//!
//! On `wasm32` each method calls the generated WIT import; on the host target (unit tests) the
//! methods return inert defaults so the ergonomic API is exercisable without a runtime.
//!
//! `needless_return` is allowed: the `#[cfg]`-split dual bodies use an early `return` in the wasm
//! arm so the host arm can be a distinct tail expression.
//!
//! ## Submodules
//! One per `cyrup:ext` WIT import interface — the axis every item in this module already sorts on.
//! All are private; the types are re-exported flat, so `cyrup_ext_sdk::ctx::Ctx` (and every other
//! path an author or the `guest` glue already uses) resolves exactly as it did when this was one
//! file.
//!
//! - `base` — `ctx-state`, `bus`, `control.abort`/`shutdown`: [`ExtMode`], [`Ctx`] and its state.
//! - `tools` — `ext-tools` + `registration`: active-tool / command / flag / provider introspection.
//! - `exec` — `exec`: [`Ctx::exec`] and [`ExecResult`].
//! - `fs` — `ext-fs`: [`Ctx::read_file`] / [`Ctx::write_file`].
//! - `http` — `http-client`: [`HttpRequest`], [`HttpResponse`], [`HttpStreamResponse`].
//! - `proc` — `proc`: [`ProcSpawnOptions`] and the spawn/poll bridge.
//! - `ui` — `ui`: [`NotifyKind`] and [`Ui`].
//! - `session` — `session`: [`Session`].
//! - `models` — `models`: [`Models`].
//! - `command` — `control`: [`CommandCtx`].
//! - `with_session` — the guest-side `withSession` callback registry + [`ReplacedSessionContext`].
//! - `tool_call` — `host-tool`: [`Signal`] and [`ToolCall`].
#![allow(clippy::needless_return)]

mod base;
mod command;
mod exec;
mod fs;
mod http;
mod models;
mod proc;
mod session;
mod tool_call;
mod tools;
mod ui;
mod with_session;

pub use base::{Ctx, ExtMode};
pub use command::CommandCtx;
pub use exec::ExecResult;
pub use http::{HttpRequest, HttpResponse, HttpStreamResponse};
pub use models::Models;
pub use proc::ProcSpawnOptions;
pub use session::Session;
pub use tool_call::{Signal, ToolCall};
pub use ui::{NotifyKind, Ui};
pub use with_session::{
    register_with_session, run_with_session, ReplacedSessionContext, WithSessionFn,
};

/// Parse a host JSON string; `Value::Null` on failure. Private to `ctx` — a child module reaches it
/// as `super::parse_json`, which is why it is not `pub(crate)`.
fn parse_json(s: String) -> serde_json::Value {
    serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
}
