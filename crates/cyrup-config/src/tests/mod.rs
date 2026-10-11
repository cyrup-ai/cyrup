//! Crate-local test modules, relocated from `tests/` so the whole suite links
//! into ONE binary instead of one process per file. Assertions are unchanged.

mod composed_non_chat_models;
mod config_schemas;
mod installation_id;
mod live_provider_auth;
mod models_json_provider;
mod models_store_classifiers;
mod models_store_images;
