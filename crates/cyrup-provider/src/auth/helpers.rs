//! Custom-provider auth helpers (arch-01 §3.7 / func-01 R-01-062/063).

use super::ApiKeyAuth;
use super::types::{AuthContext, AuthResult, Credential, ModelAuth};
use crate::error::AuthError;
use crate::model::Model;
use std::sync::Arc;

/// The standard env-key strategy (func-01 R-01-063): a stored/explicit credential wins; otherwise
/// the first present variable of `vars` is used. A `None` credential with no env var present
/// resolves to `None` (not configured).
pub fn env_key<I, S>(vars: I) -> Arc<dyn ApiKeyAuth>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    Arc::new(EnvKeyAuth {
        vars: vars.into_iter().map(Into::into).collect(),
    })
}

/// A keyless-local strategy (func-01 R-01-062): always resolves as "configured, no key" — for local
/// servers (Ollama, llama.cpp, vLLM) that need no credential.
pub fn keyless_local() -> Arc<dyn ApiKeyAuth> {
    Arc::new(KeylessLocalAuth)
}

struct EnvKeyAuth {
    vars: Vec<String>,
}

#[async_trait::async_trait]
impl ApiKeyAuth for EnvKeyAuth {
    fn name(&self) -> &str {
        "env-key"
    }

    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        // A stored/explicit credential owns the provider — env is NOT consulted (R-01-012).
        if let Some(cred) = cred {
            return Ok(match cred {
                Credential::ApiKey { key, env } => Some(AuthResult {
                    auth: ModelAuth {
                        api_key: key.clone(),
                        ..Default::default()
                    },
                    env: env.clone(),
                    source: Some("stored".to_string()),
                }),
                // OAuth credential is not an api-key strategy's concern.
                Credential::Oauth { .. } => None,
            });
        }

        // Ambient: first present env var of the list (R-01-063).
        for var in &self.vars {
            if let Some(val) = ctx.env(var).await
                && !val.is_empty()
            {
                return Ok(Some(AuthResult::from_key(val, "env")));
            }
        }
        Ok(None)
    }
}

struct KeylessLocalAuth;

#[async_trait::async_trait]
impl ApiKeyAuth for KeylessLocalAuth {
    fn name(&self) -> &str {
        "keyless-local"
    }

    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        // Always "configured, no key" (R-01-062), carrying any provider-scoped env overlay.
        let env = cred.and_then(|c| c.env().cloned());
        Ok(Some(AuthResult {
            auth: ModelAuth::default(),
            env,
            source: Some("keyless".to_string()),
        }))
    }
}
