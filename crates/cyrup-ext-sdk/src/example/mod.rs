//! A tiny reference extension authored with this SDK (arch-08 §11; the analog of Pi's
//! `examples/extensions/permission-gate.ts` + a dynamic tool). Building this crate to
//! `wasm32-wasip2` produces a loadable `cyrup:ext` COMPONENT whose `init` registers everything
//! below; the host loads it and dispatches real events to it (the arch-08b live E2E proof).
//!
//! [`build`] does nothing but call one `install` per concern, so a reader looking for a single seam
//! opens one module instead of scanning the whole demo:
//!
//! - `hooks` — the host-event subscriptions (including the `session_start` handler that registers
//!   the `demo_late` tool from a LIVE event).
//! - `tools` — the `demo_echo` and `signal_probe` guest tools.
//! - `commands_capability` — the commands driving the `exec`, `http-client`, `ext-fs` and `proc`
//!   capability grants.
//! - `commands_ui` — the `ui.*` dialog commands and the `ctrl+t` shortcut.
//! - `commands_session` — the commands reading or mutating session / agent state.
//! - `renderers` — the message and entry renderers, and the types behind them.
//! - `provider` — the `demo-oauth` provider and the global autocomplete provider.
//! - `virtual_model` — the `router/demo-auto` virtual model and the router behind it.
//! - `model_calls` — the commands that make model calls through the session's providers.
//! - `wiring` — the `demo-flag` CLI flag and the `demo:bus` event-bus subscription.

mod commands_capability;
mod commands_session;
mod commands_ui;
mod hooks;
pub mod model_calls;
mod provider;
mod renderers;
mod tools;
pub mod virtual_model;
mod wiring;

use crate::ExtensionApi;

/// The identifiers a host-side test needs to name this demo's virtual-model seam, so an assertion
/// cannot drift from the guest by holding its own copy of a literal.
pub use virtual_model::{
    CANCEL_OBSERVED, DEMO_ROUTER_ID, DEMO_ROUTER_PROVIDER, HOLD_COMMAND, HOLD_TOOL,
};

/// Build the demo extension's [`ExtensionApi`]. Pure ergonomic-layer code — also unit-testable on
/// the host target.
pub fn build() -> ExtensionApi {
    let mut api = ExtensionApi::new();
    hooks::install(&mut api);
    tools::install(&mut api);
    commands_capability::install(&mut api);
    commands_ui::install(&mut api);
    commands_session::install(&mut api);
    renderers::install(&mut api);
    provider::install(&mut api);
    virtual_model::install(&mut api);
    model_calls::install(&mut api);
    wiring::install(&mut api);
    api
}
