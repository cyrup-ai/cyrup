//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.
//!
//! One deliberate exception: `oauth_http_proxy.rs` (PROV-047) was moved back OUT to
//! `crates/cyrup-provider/tests/oauth_http_proxy.rs` as a real integration-test binary. It calls
//! `crate::stream::sse::configure_http_proxy`, which mutates a process-global static consulted by
//! every other test's proxy resolution fallback — consolidating it into this shared binary let it
//! silently reroute unrelated concurrently-running loopback tests (`remote_catalog`,
//! `github_copilot`) into `ECONNREFUSED`. A separate OS process per `cargo test` integration binary
//! makes that static invisible to everything else, with no per-test opt-in required anywhere. See
//! that file's module doc comment for the full account.

mod anthropic_sensitive_stop;
mod api_key_login;
mod available_of_type;
mod builtin_oauth;
mod catalog_data;
mod catalog_model_types;
mod catalog_refresh;
mod classifier_dispatch;
mod classifier_store;
mod classifier_support;
mod classifier_types;
mod llama_cpp_classify;
mod llama_cpp_classify_fake_server;
mod llama_cpp_classify_render;
mod llama_cpp_classify_wire;
mod openrouter_anthropic_route;
mod overflow_estimate_parity;
mod remote_catalog;
mod sampling_params;
mod thinking_max;
mod transform_headers_on_the_wire;
