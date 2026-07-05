//! A thin standalone entrypoint for the intercom broker process.
//!
//! In production the broker is dispatched from the main `cyrup` binary via the hidden
//! `cyrup __intercom-broker` subcommand (re-exec of `current_exe()`); this binary is the direct,
//! `CYRUP_INTERCOM_BROKER_BINARY`-override-shaped entrypoint (and the real-subprocess fixture the
//! `tests/broker_roundtrip.rs` integration proof launches). Both call the same
//! [`cyrup_intercom::broker::run`]. Argv is ignored (the presence of this binary IS the broker), so
//! it works whether invoked bare or with a trailing `__intercom-broker` token.

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
