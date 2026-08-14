//! Seam tests drained from **`crates/cyrup-intercom`** — 20 files.
//!
//! What makes a test belong here: it runs the real `cyrup-intercom-broker` process over a real
//! Unix socket. That includes the two hostile-`UnixListener` protocol files (number domain, array
//! payload, explicit null, forward compatibility), the lifecycle files (runtime claim, startup
//! fail-fast, reconnect, registration under session id), the shared-human-lock/surface files, and
//! the two that kill the broker outright.
//!
//! Migration notes:
//!
//! * `support::bins::intercom_broker()` and `support::bins::intercom_child_fixture()` replace
//!   `env!("CARGO_BIN_EXE_…")`, which does not cross a package boundary.
//! * The duplicated helpers collapse into [`mod common`] here (target-local, since they are
//!   meaningless outside this seam). Done as they landed, per this note — but only where the copies
//!   were verified byte-identical: `Broker` (6), `registration` (9), `spawn_broker` (6), `within`
//!   (5), `write_broker_command` (4). The six `RawClient`s and four `HostileBroker`s stayed put,
//!   because they are NOT copies — each has its own method set, so merging them would be rewriting
//!   behaviour rather than relocating it. `common.rs` carries the table.
//! * `child_bridge_activation` lost its `#![cfg(feature = "test-fixtures")]`; the reason that
//!   RESTORES the test rather than disabling it is written at the top of that file.
//! * `tool_actions` (9) and `compose_send_leg` (2) arrived LATER than the other 18, and from a
//!   different place: they were `#[cfg(test)]` modules inside `crates/cyrup-intercom/src/`, not
//!   `tests/` files, and they only ever passed because a sibling integration target in that
//!   package incidentally caused cargo to link `cyrup-intercom-broker` into `target/<profile>/`.
//!   Once the 18 above moved here, `cargo test -p cyrup-intercom --lib` stopped producing that
//!   binary and all 11 went red on the spawn. Each file's header records the one rewrite it needed
//!   to reach the crate from outside.
//! * Socket paths go under the test's own `TempDir` (§4 R1) and listeners bind `:0` (§4 R4) —
//!   both already true throughout this crate's tests; keep it that way.
//! * A detached `__intercom-broker` grandchild that inherits a harness pipe FD above 2 is the
//!   known deadlock in this suite: `wait_with_output()` reads to EOF, NOT to child exit, so it
//!   blocks forever. `.config/nextest.toml`'s `leak-timeout` turns that into a named LEAK-FAIL —
//!   it is a tripwire, not a fix.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

/// Helpers whose copies were byte-identical across the migrated files. See its module doc for the
/// table of what was collapsed and what deliberately was not.
mod common;

mod broker_extension_bus_miss_branches;
mod broker_roundtrip;
mod broker_runtime_claim;
mod broker_startup_fail_fast;
mod child_bridge_activation;
mod compose_send_leg;
mod dismiss_incoming_ask;
mod human_surface;
mod intercom_command_transcript;
mod intercom_id_command;
mod presence_context_usage;
mod protocol_array_payload_rejection;
mod protocol_explicit_null_rejection;
mod protocol_forward_compat;
mod protocol_number_domain;
mod reconnect;
mod registers_under_session_id;
mod session_info_context_fields;
mod shared_human_lock;
mod tool_actions;

/// §4 R5 layer 3. Cheap, and it names the leak at the top of the run instead of letting a broker
/// test quietly inherit an ambient `CYRUP_INTERCOM=1` — the variable that has already leaked 13
/// broker processes out of one run here.
#[test]
fn no_ambient_provider_credentials_or_feature_gates() {
    support::env::assert_no_ambient_provider_credentials();
    support::env::assert_no_ambient_feature_gates();
}
