//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

mod anthropic_sensitive_stop;
mod api_key_login;
mod builtin_oauth;
mod catalog_data;
mod overflow_estimate_parity;
mod remote_catalog;
mod sampling_params;
mod thinking_max;
