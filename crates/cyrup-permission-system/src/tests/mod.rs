//! Crate-internal test modules.
//!
//! These were formerly `tests/*.rs` integration binaries. Cargo compiles every file under `tests/`
//! into its OWN binary and runs it as its OWN process; for assertions that touch no process-global
//! state that per-file process buys nothing, so they live here as ordinary `#[cfg(test)]` modules
//! of the library instead.
//!
//! The assertions are unchanged from their integration-test form — only the crate self-reference
//! (`cyrup_permission_system::…` → `crate::…`) was rewritten.
//!
//! # What may move here
//!
//! `cargo test` runs the whole crate's unit tests as parallel threads in ONE process, so this
//! directory used to be barred to any test that touched the process ENVIRONMENT: such a test was
//! isolated only while it owned a `tests/` binary of its own. That bar is gone. No test in this
//! crate mutates the process environment any more — every env read goes through [`crate::envx`],
//! whose `cfg(test)` build consults a THREAD-LOCAL overlay, and an override is installed with
//! `envx::pin`, which no other thread can observe.
//!
//! What a test moved here DOES have to respect is the one constraint that overlay carries: a pin is
//! thread-local, so the pinned body must stay on the pinning thread. Drive it synchronously, or on
//! a `new_current_thread` runtime (what `#[tokio::test]` builds by default) — never on a
//! multi-thread runtime, whose workers cannot see the pin.
//!
//! The modules below either construct their `ExtensionConfig` values directly and never resolve a
//! path from the environment, or pin what they resolve. (Three of them ASSERT, through
//! [`crate::envx::var`] so a pin would be seen, that `FORWARDING_AGENT_DIR_ENV` resolves to nothing
//! as a precondition; no test in this crate sets it.)

mod forwarded_prompt_fractional_timeout;
mod forwarding_audit_trail;
mod forwarding_has_ui_guard;
mod forwarding_persist;
mod forwarding_preserve_location;
mod forwarding_response_path_containment;
mod prompt_dedup;
