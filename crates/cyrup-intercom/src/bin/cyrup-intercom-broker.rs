//! A thin standalone entrypoint for the intercom broker process.
//!
//! In production the broker is dispatched from the main `cyrup` binary via the hidden
//! `cyrup __intercom-broker` subcommand (re-exec of `current_exe()`); this binary is the direct,
//! `CYRUP_INTERCOM_BROKER_BINARY`-override-shaped entrypoint (and the real-subprocess fixture the
//! `crates/cyrup-it/tests/intercom/broker_roundtrip.rs` integration proof launches). Both call the
//! same [`cyrup_intercom::broker::run`]. Argv is ignored (the presence of this binary IS the
//! broker), so it works whether invoked bare or with a trailing `__intercom-broker` token.

// The crate-root `#![deny(...)]` in `lib.rs` governs the LIBRARY root only; a bin target is its own
// crate root and inherits nothing from it. `[lints] workspace = true` covers the four workspace
// denies here, but `clippy::unreachable`/`todo`/`unimplemented` live in `lib.rs` alone, so without
// this the no-panic wall has a hole exactly the shape of the two bin targets. Restated locally
// rather than promoted to `[workspace.lints.clippy]`: 12 production call sites across other crates
// (tracker.rs:560, extension.rs:5755, main.rs:1190, sessions.rs:471, http.rs:547, wasm_host.rs:130
// and :239, and others) carry no allow, so promoting these three would break crates this change
// has no business touching.
#![deny(clippy::unreachable, clippy::todo, clippy::unimplemented)]

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::process::ExitCode {
    match cyrup_intercom::broker::run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cyrup-intercom-broker: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
