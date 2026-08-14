//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

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
mod untracked_misses;
