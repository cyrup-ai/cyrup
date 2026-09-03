//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

// The ONE waiver for the four lints the workspace denies (Cargo.toml:97-101). Lint levels set on a
// parent module propagate into file-loaded submodules, so this covers every module declared below
// and no test file needs (or should carry) its own copy.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod support;

mod agent_loop;
mod area02_backlog;
mod agent_message_role_key;
mod hook_failure_text;
mod model_boundary;
mod pending_containment;
mod preflight_validation;
mod proxy_live_turn;
mod round2_parity;
mod settlement_latch;
mod tool_result_model;
mod turn_tool_refresh;
mod type_driven_core;
mod untracked_misses;
