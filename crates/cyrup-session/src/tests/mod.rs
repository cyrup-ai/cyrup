//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

mod compaction;
mod deferred_context;
mod estimator_prefix_timestamp_parity;
mod listing_unparseable_message;
mod parity;
mod sessions;
mod tool_result_fields;
