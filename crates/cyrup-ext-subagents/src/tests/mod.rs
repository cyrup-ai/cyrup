//! Crate-internal test modules.
//!
//! These files previously lived under `crates/cyrup-ext-subagents/tests/` as separate Cargo
//! integration-test binaries. None of them needs a process seam, a built artifact, or a
//! `CARGO_BIN_EXE_*` path — they only ever exercised this crate's own (public) API in-process, so
//! each one costs a whole extra link+process for nothing. They are relocated here verbatim; the
//! ONLY edit is the crate self-reference (`cyrup_ext_subagents::X` -> `crate::X`), which is
//! required now that they compile *inside* the library rather than against it.
//!
//! Files that mutate the process environment (`std::env::set_var`/`remove_var`) can NOT move here:
//! this crate is `#![forbid(unsafe_code)]` (src/lib.rs) and Rust 2024 requires `unsafe` for those
//! calls, and `forbid` cannot be locally overridden. Those files stay in `tests/`.
//!
//! Assertions are unchanged — several of these are the named `Verify` step of an open parity item.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod acceptance_policy_parity;
mod bundled_resources_registration_integration;
mod child_prompt_runtime_integration;
mod discovery_integration;
mod dynamic_collect_record_fidelity;
mod dynamic_group_acceptance_parity;
mod management_actions_integration;
mod read_only_agent_name_alternation;
mod spawn_temp_file_cleanup;
mod steer_delivery_integration;
mod verify_memo_and_redaction;
mod watchdog_wiring;
