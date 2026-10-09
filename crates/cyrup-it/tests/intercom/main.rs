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

// ICOM-061: `/alias` renames the session and pushes the name to peers without the name poll.
mod alias_command;
mod broker_extension_bus_miss_branches;
mod broker_roundtrip;
mod broker_runtime_claim;
mod broker_startup_fail_fast;
mod child_bridge_activation;
// ICOM-067: the `cyrup-intercom-cli` scripting client (`cli.ts`), run as a subprocess against a
// real broker and a real peer.
mod cli_client;
// ICOM-074 / ICOM-075 / ICOM-071: the explicit cross-machine SSH relay — the `relay` subcommand
// and `send --to name@machine`, with both halves proved against each other.
mod cli_relay;
mod compose_send_leg;
mod dismiss_incoming_ask;
// ICOM-077: `intercom({ action: "handover" })` through the tool, over a real broker and peer.
mod handover_action;
// ICOM-078 / ICOM-085: `/handover`, the handover picker, and the live `/intercom` list with `h` and
// `alt+m`, through the production command/shortcut entry points and a driven overlay host.
mod handover_command;
// ICOM-065: the broker-side Herdr location join, against a fake Herdr socket.
mod herdr_location;
mod human_surface;
mod injected_receipt_after_injection;
// Written here, not drained: the live-session proofs for ICOM-035 / ICOM-062 / ICOM-068 — the
// production `IntercomExtension` inside a real `AgentSession`, against a real broker.
mod inbound_live_session;
mod intercom_command_transcript;
mod intercom_id_command;
// ICOM-066: `details.roster` and the collapsed one-line render.
mod list_roster;
// ICOM-077: the outbox's confirm dialog — title `Send extension message`, body names the extension.
mod outbox_dialog;
// ICOM-057: the broker's `pending-asks/*.json` records — write, remove, prune.
mod pending_ask_records;
mod presence_context_usage;
mod protocol_array_payload_rejection;
mod protocol_explicit_null_rejection;
mod protocol_forward_compat;
mod protocol_number_domain;
mod reconnect;
mod registers_under_session_id;
mod session_info_context_fields;
// ICOM-064: an extension claims the session's intercom id over the bus.
mod session_identity_claim;
mod shared_human_lock;
mod tool_actions;

// §4 R5 layer 3 — the ambient-credential and feature-gate guards (an ambient `CYRUP_INTERCOM=1`
// has already leaked 13 broker processes out of one run here) now live as `#[test]`s inside
// `support::env` itself, so every target that declares `mod support` runs them without per-target
// wiring. Nothing to declare here.
