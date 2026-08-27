//! Request encoding: the `POST` target.

use crate::auth::AuthResult;
use crate::model::Model;

/// Resolve the `POST` target: an auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/responses` (the OpenAI SDK's `client.responses.create` path).
pub(super) fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/responses") {
        Some(trimmed.to_string())
    } else {
        Some(format!("{trimmed}/responses"))
    }
}
