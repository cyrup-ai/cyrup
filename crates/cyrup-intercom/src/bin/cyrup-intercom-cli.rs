//! `cyrup-intercom-cli` — the standalone scripting client for the local intercom broker
//! (pi-intercom `cli.ts`). The whole client lives in [`cyrup_intercom::cli`]; this binary is its
//! entry point for scripts that want it under its own name. `cyrup intercom …` runs the same code
//! from the `cyrup` binary.

// The crate-root `#![deny(...)]` in `lib.rs` governs the LIBRARY root only; a bin target is its own
// crate root and inherits nothing from it. Restated here for the same reason as in
// `cyrup-intercom-broker.rs`.
#![deny(clippy::unreachable, clippy::todo, clippy::unimplemented)]

use std::process::ExitCode;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    cyrup_intercom::cli::run(
        &cyrup_intercom::cli::process_args(1),
        cyrup_intercom::cli::BIN_PROGRAM,
    )
    .await
}
