//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

mod area03_repairs;
mod branch_provenance_and_export;
mod compaction;
mod compaction_nested_calls;
mod deferred_context;
mod durable_rewrite;
mod estimator_prefix_timestamp_parity;
mod listing_progress;
mod listing_unparseable_message;
mod parity;
mod sessions;
mod system_row_bytes;
mod tool_result_fields;
mod tool_state;
