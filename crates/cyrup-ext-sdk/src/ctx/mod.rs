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
//! ## The host-target arm rule
//! "Inert default" is three-way, and a method added here picks its arm by this rule rather than by
//! taste:
//!
//! - A fire-and-forget op — `Result<(), _>` with no data to hand back — returns `Ok(())`
//!   (`base::Ctx::abort`/`shutdown`, `models::Models::set_thinking_level`/`set_model`,
//!   `ui::Ui::set_theme`, and the `command` module's `control` bridge).
//! - Anything that would have to FABRICATE host data returns
//!   `Err("<op> unavailable on host target")` instead of a plausible-looking success, so a
//!   host-target test that actually depends on host state fails loudly rather than asserting
//!   nothing (`exec`, `fs`, `http`, `proc`, `session::Session::append_entry`,
//!   `command::CommandCtx::system_prompt_options`, and `crate::provider`).
//! - A getter with no `Result` to fail through returns the empty value of its own type — `false`
//!   for `bool`, `String::new()` for `String`, `None` for `Option` — and, for a
//!   `serde_json::Value`, the shape the HOST's own no-session fallback produces, so guest and host
//!   arms agree on shape and an `as_array()` / `if let Some(..)` body is exercised on both:
//!   `Value::Array(vec![])` for the collection getters in `models`/`tools`/`ui`, and per-variant
//!   for `session`'s `session_call` — `"[]"` for entries and branch, `"null"` for tree, mirroring
//!   `crates/cyrup-ext/src/host/live.rs:519`/`:522`/`:525`.
//!
//! No host arm reads the runner's environment: `Ctx::cwd` returns `String::new()` on the host
//! target rather than the process working directory, so a host-target result never depends on
//! where the test binary happened to be launched.
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
//! - `bash_call` — `host-bash`: [`BashCommand`], the one command a guest bash backend runs.
//!
//! Because those modules are private, rustdoc does not visit them on a default
//! `cargo doc -p cyrup-ext-sdk --no-deps` run, so the crate-root
//! `#![deny(rustdoc::broken_intra_doc_links, rustdoc::private_intra_doc_links)]` (`src/lib.rs`)
//! checks nothing inside this directory on that invocation. When editing a file here, run
//! `cargo doc -p cyrup-ext-sdk --no-deps --document-private-items` — that is the invocation under
//! which these doc links are actually resolved.
//!
//! The crate-root `#![warn(missing_docs)]` is the opposite case and needs no special invocation: it
//! is a rustc lint, not a rustdoc one, so a plain `cargo check -p cyrup-ext-sdk` reports an
//! undocumented public item in these private submodules exactly as it would in `api.rs`.
#![allow(clippy::needless_return)]

mod base;
mod bash_call;
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
pub use bash_call::BashCommand;
pub use command::CommandCtx;
pub use exec::ExecResult;
pub use http::{HttpRequest, HttpResponse, HttpStreamResponse};
pub use models::Models;
pub use proc::ProcSpawnOptions;
pub use session::Session;
pub use tool_call::{Signal, ToolCall};
pub use ui::{NotifyKind, Ui};
pub use with_session::{
    ReplacedSessionContext, WithSessionFn, register_with_session, run_with_session,
};

/// Parse a host JSON string; `Value::Null` on failure. Private to `ctx` — a child module reaches it
/// as `super::parse_json`, which is why it is not `pub(crate)`.
fn parse_json(s: String) -> serde_json::Value {
    serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
}
