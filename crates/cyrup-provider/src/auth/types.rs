//! Auth data model (arch-01 §3.7 / func-01 §7.2).

use crate::HeaderMap;
use std::collections::BTreeMap;

/// Provider-scoped environment / config overlay applied during requests (func-01 R-01-016).
pub type ProviderEnv = BTreeMap<String, String>;

/// A persisted credential (func-01 §7.2). serde tag `type = api_key | oauth`; the OAuth `ext` map
/// flattens provider-specific fields (Copilot endpoint, etc.) for Pi `auth.json` interop (R-00-013).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        env: Option<ProviderEnv>,
    },
    Oauth {
        refresh: String,
        access: String,
        /// Unix seconds.
        expires: i64,
        #[serde(flatten)]
        ext: serde_json::Map<String, serde_json::Value>,
    },
}

impl Credential {
    /// A bare API-key credential.
    pub fn api_key(key: impl Into<String>) -> Self {
        Credential::ApiKey {
            key: Some(key.into()),
            env: None,
        }
    }

    /// The provider-scoped env overlay carried by this credential, if any (R-01-016).
    pub fn env(&self) -> Option<&ProviderEnv> {
        match self {
            Credential::ApiKey { env, .. } => env.as_ref(),
            Credential::Oauth { .. } => None,
        }
    }
}

/// The resolved request auth an `ApiImpl` needs: key, header overlay, and an optional base-url
/// override (arch-01 §3.7).
#[derive(Clone, Debug, Default)]
pub struct ModelAuth {
    pub api_key: Option<String>,
    pub headers: Option<HeaderMap>,
    pub base_url: Option<String>,
}

/// The full result of auth resolution handed to the request path (arch-01 §3.7).
#[derive(Clone, Debug)]
pub struct AuthResult {
    pub auth: ModelAuth,
    /// Provider-scoped env overlay to apply for this request (R-01-016).
    pub env: Option<ProviderEnv>,
    /// Human-readable source label (e.g. `"stored"`, `"env"`, `"OAuth"`, `"keyless"`).
    pub source: Option<String>,
}

impl AuthResult {
    pub fn from_key(key: impl Into<String>, source: impl Into<String>) -> Self {
        AuthResult {
            auth: ModelAuth {
                api_key: Some(key.into()),
                ..Default::default()
            },
            env: None,
            source: Some(source.into()),
        }
    }
}

/// Ambient-source lookup (env vars / file presence) injected for testability (arch-01 §3.7).
#[async_trait::async_trait]
pub trait AuthContext: Send + Sync {
    async fn env(&self, name: &str) -> Option<String>;
    async fn file_exists(&self, path: &str) -> bool;
}

/// The default ambient context: real process env + filesystem.
#[derive(Clone, Copy, Default)]
pub struct EnvAuthContext;

#[async_trait::async_trait]
impl AuthContext for EnvAuthContext {
    async fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
    async fn file_exists(&self, path: &str) -> bool {
        tokio::fs::metadata(path).await.is_ok()
    }
}
