//! Cache retention (Pi resolveCacheRetention / getCacheControl)
//!
//! `resolveCacheRetention` (anthropic-messages.ts:46-54) lives in
//! `crate::utils::provider_plumbing`: openai-completions.ts:141-149 and openai-responses.ts:47-55
//! declare the identical three-line ladder, and this file carried one of three byte-identical ports.

use super::compat::get_anthropic_compat;
use crate::auth::ProviderEnv;
use crate::model::Model;
use crate::stream::CacheRetention;
use crate::utils::provider_plumbing::resolve_cache_retention;
use serde_json::{Map, Value, json};

/// The `cache_control` ephemeral marker for the resolved retention (Pi `getCacheControl`,
/// anthropic-messages.ts:56-70). `None` when retention is `none`.
pub(super) fn get_cache_control(
    model: &Model,
    cache_retention: Option<CacheRetention>,
    env: Option<&ProviderEnv>,
) -> Option<Value> {
    let retention = resolve_cache_retention(cache_retention, env);
    if retention == CacheRetention::None {
        return None;
    }
    let mut cc = Map::new();
    cc.insert("type".to_string(), json!("ephemeral"));
    if retention == CacheRetention::Long
        && get_anthropic_compat(model).supports_long_cache_retention
    {
        cc.insert("ttl".to_string(), json!("1h"));
    }
    Some(Value::Object(cc))
}
