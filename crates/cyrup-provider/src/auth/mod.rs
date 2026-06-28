//! Authentication: credential store, provider auth strategies, and the resolution precedence
//! engine (arch-01 §3.7 / func-01 §7).

pub mod helpers;
pub mod resolve;
pub mod store;
pub mod types;

use crate::error::AuthError;
use crate::model::Model;
use std::sync::Arc;

pub use helpers::{env_key, keyless_local};
pub use resolve::{resolve_provider_auth, AuthOverrides};
pub use store::{CredentialStore, InMemoryCredentialStore, ModifyFn};
pub use types::{
    AuthContext, AuthResult, Credential, EnvAuthContext, ModelAuth, ProviderEnv,
};

/// How a provider authenticates (func-01 §4.1: at least one of `api_key | oauth`).
#[derive(Clone, Default)]
pub struct ProviderAuth {
    pub api_key: Option<Arc<dyn ApiKeyAuth>>,
    pub oauth: Option<Arc<dyn OAuthAuth>>,
}

impl ProviderAuth {
    /// API-key-only auth.
    pub fn with_api_key(strategy: Arc<dyn ApiKeyAuth>) -> Self {
        Self { api_key: Some(strategy), oauth: None }
    }

    /// OAuth-only auth.
    pub fn with_oauth(strategy: Arc<dyn OAuthAuth>) -> Self {
        Self { api_key: None, oauth: Some(strategy) }
    }

    /// `true` if at least one strategy is configured (func-01 §4.1 invariant).
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some() || self.oauth.is_some()
    }
}

/// An API-key resolution strategy (env-var helper, keyless-local, custom). Merges a stored/explicit
/// credential with ambient sources; `None` means "not configured".
#[async_trait::async_trait]
pub trait ApiKeyAuth: Send + Sync {
    fn name(&self) -> &str;

    /// Resolve request auth. `cred` is the explicit/stored credential (when present); a `None` `cred`
    /// means the resolver may consult ambient sources (env vars) via `ctx` (func-01 R-01-011/012).
    async fn resolve(
        &self,
        model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError>;
}

/// An OAuth strategy. `refresh` runs UNDER the credential-store lock (func-01 R-01-014/067).
#[async_trait::async_trait]
pub trait OAuthAuth: Send + Sync {
    fn name(&self) -> &str;

    /// Network refresh of an expired credential. A failure surfaces as `AuthError::OAuth` and MUST
    /// NOT fall back to an env key (func-01 R-01-013).
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError>;

    /// Side-effect-free derivation of request auth from a valid credential.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError>;
}
