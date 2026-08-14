//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

mod bash_session_env;
mod builtin_tool_order;
mod cross_registry_mutation_lock;
mod edit_preview_diff;
mod grep_context_zero_line_text;
mod isolation;
mod pi_schema;
mod pi_tool_semantics;
mod read_access_errno;
mod read_model_vision;
mod tools;
mod write_semantics;
