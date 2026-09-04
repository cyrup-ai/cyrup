//! `ctx.mode` / `ctx.hasUI` reach a WASM guest (Pi `ExtensionContext.mode` / `.hasUI`,
//! extensions/types.ts:311,313; arch-08 §6.3).
//!
//! Pi puts both on the BASE context so every handler can guard terminal-only UI ("Use `tui` to
//! guard terminal-only UI such as custom components") and dialog-capable UI ("true in TUI and RPC
//! modes"). Before the fix the `ext-mode` enum was declared in the WIT world and used by ZERO
//! functions — `interface ctx-state` exposed only `is-idle`/`has-pending-messages`/
//! `is-project-trusted`/`get-system-prompt` — so a guest had no way to ask, while the native
//! built-in path had both off `HostCtx` (`native.rs:91-92`).
//!
//! This drives the REAL component: the `cyrup-ext-sdk` demo guest's `/hostmode` command calls
//! `ctx.mode()` / `ctx.has_ui()` and returns what it saw, and the assertions below compare that
//! against the [`HostConfig`] the host was built with — the same struct `cyrup-session-svc`'s
//! builder fills from the app mode (`builder.rs:739-740`). Two configurations are checked in
//! opposite directions, so neither can be satisfied by a hard-coded default.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cyrup_core::{CancelToken, ExtensionId};
use cyrup_ext::{CannedResponses, ExtMode, ExtensionHost, HostConfig, RecordingServices};
use std::sync::{Arc, OnceLock};

/// The built demo component. Same contract as before — honor a prebuilt artifact from the
/// environment, otherwise build the guest crate for `wasm32-wasip2` — except that both halves now
/// happen ONCE for the whole suite in `crates/cyrup-it/build.rs`, which resolves
/// `CYRUP_EXT_FIXTURE_COMPONENT` or runs the wasip2 build and hands the path over as
/// `CYRUP_IT_COMPONENT`. The `OnceLock` is kept because it is what makes the *read* happen at most
/// once per test binary, and three tests in this file share it.
fn component_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(crate::support::bins::component_bytes)
}

/// Load the demo component into a host built with `config`, run `/hostmode`, and return what the
/// guest reported. `load_wasm` is the same entry `discover_and_load` -> `load_discovered` funnels
/// every discovered extension through, so this is the production load path.
async fn hostmode_as_seen_by_the_guest(config: HostConfig) -> String {
    let host = ExtensionHost::with_wasm(config).expect("host with wasm");
    let services = Arc::new(RecordingServices::new(CannedResponses::default()));
    host.load_wasm(ExtensionId::from("demo"), component_bytes(), services)
        .await
        .expect("demo component loads");
    let cancel = CancelToken::new();
    host.run_command("hostmode", "", &cancel)
        .await
        .expect("the /hostmode command runs")
        .expect("it produces output")
}

/// A TUI host with dialogs available — Pi's `mode: "tui"`, `hasUI: true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_reads_the_tui_host_mode_and_ui_availability() {
    let cwd = std::env::temp_dir();
    let out = hostmode_as_seen_by_the_guest(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: cwd.clone(),
    })
    .await;
    assert_eq!(
        out, "mode=tui has_ui=true",
        "the guest read the host's configured mode/hasUI"
    );
}

/// A headless print-mode host — Pi's `mode: "print"`, `hasUI: false`. Both values differ from the
/// `HostConfig::default()` pair (`tui` + `true`) and from the WIT enum's first variant, so this
/// cannot pass on a default: the guest must be reading the host's ACTUAL configuration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_reads_a_headless_print_host_mode_and_ui_availability() {
    let cwd = std::env::temp_dir();
    let out = hostmode_as_seen_by_the_guest(HostConfig {
        mode: ExtMode::Print,
        has_ui: false,
        cwd: cwd.clone(),
    })
    .await;
    assert_eq!(
        out, "mode=print has_ui=false",
        "a headless host must not report itself to the guest as an interactive TUI"
    );
}

/// The `rpc` and `json` modes round-trip too — every `ext-mode` variant the WIT enum declares is
/// reachable, not just the two the other tests pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_ext_mode_variant_crosses_the_boundary() {
    for (mode, expected) in [
        (ExtMode::Rpc, "mode=rpc has_ui=true"),
        (ExtMode::Json, "mode=json has_ui=false"),
    ] {
        let has_ui = mode == ExtMode::Rpc; // Pi: dialogs exist in TUI and RPC modes.
        let out = hostmode_as_seen_by_the_guest(HostConfig {
            mode,
            has_ui,
            cwd: std::env::temp_dir(),
        })
        .await;
        assert_eq!(out, expected, "{mode:?} crossed the boundary intact");
    }
}
