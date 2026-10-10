//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

mod bash_session_env;
mod builtin_exposure;
mod builtin_tool_order;
mod cross_registry_mutation_lock;
mod edit_preview_diff;
mod find_abort;
mod grep_context_zero_line_text;
#[cfg(feature = "inline-images")]
mod image_proc;
mod isolation;
mod mutation_lock_is_first_await;
mod no_inherited_harness_stdio;
mod pi_schema;
mod pi_tool_semantics;
mod read_access_errno;
#[cfg(feature = "inline-images")]
mod read_image_resize_profile;
mod read_limits;
mod read_model_vision;
mod structured_output;
mod tools;
mod walk_error_text;
mod write_semantics;
