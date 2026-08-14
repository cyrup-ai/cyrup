//! Custom-provider auth helpers (arch-01 §3.7 / func-01 R-01-062/063).

use super::ApiKeyAuth;
use super::oauth::{AuthInteraction, AuthPrompt, OAuthError};
use super::types::{AuthContext, AuthResult, Credential, ModelAuth};
use crate::error::AuthError;
use crate::model::Model;
use std::sync::Arc;

/// The standard env-key strategy (func-01 R-01-063): a stored/explicit credential wins; otherwise
/// the first present variable of `vars` is used. A `None` credential with no env var present
/// resolves to `None` (not configured).
///
/// `name` is upstream's first argument — a **user-facing display string**, not an id:
/// `envApiKeyAuth("OpenRouter API key", ["OPENROUTER_API_KEY"])` (`ai/src/auth/helpers.ts:9`,
/// `providers/openrouter.ts:13` @v0.83.0). It is what [`ApiKeyAuth::name`] reports, what `/login`
/// lists as the method, and what [`ApiKeyAuth::login`] interpolates into `Enter {name}`
/// (`helpers.ts:12`).
///
/// CFG-005 / ADR-0010 step 2: this parameter used to be dropped and `name()` hardcoded `"env-key"`,
/// which forced `cyrup-config` to (a) sniff `name()` to decide whether a strategy has a login and
/// (b) reconstruct the label as `"{provider name} API key"`. That reconstruction is wrong for every
/// provider whose upstream label is not that shape — `"GitHub Copilot token"`
/// (`providers/github-copilot.ts:15`), `"Hugging Face token"` (`providers/huggingface.ts:11`),
/// `"Gemini API key"` for the provider named *Google* (`providers/google.ts:11`), and
/// `"Moonshot AI API key"` for the provider named *Moonshot AI CN* (`providers/moonshotai-cn.ts:11`).
pub fn env_key<I, S>(name: impl Into<String>, vars: I) -> Arc<dyn ApiKeyAuth>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    Arc::new(EnvKeyAuth {
        name: name.into(),
        vars: vars.into_iter().map(Into::into).collect(),
    })
}

/// A keyless-local strategy (func-01 R-01-062): always resolves as "configured, no key" — for local
/// servers (Ollama, llama.cpp, vLLM) that need no credential.
pub fn keyless_local() -> Arc<dyn ApiKeyAuth> {
    Arc::new(KeylessLocalAuth)
}

struct EnvKeyAuth {
    name: String,
    vars: Vec<String>,
}

#[async_trait::async_trait]
impl ApiKeyAuth for EnvKeyAuth {
    fn name(&self) -> &str {
        &self.name
    }

    /// `envApiKeyAuth` always defines `login` (`ai/src/auth/helpers.ts:12-15` @v0.83.0), so the
    /// answer is unconditionally `true` — the option is offered for every env-key provider.
    fn supports_login(&self) -> bool {
        true
    }

    /// `envApiKeyAuth`'s `login` (`ai/src/auth/helpers.ts:12-15` @v0.83.0), verbatim:
    ///
    /// ```ts
    /// login: async (interaction) => {
    ///     const key = await interaction.prompt({ type: "secret", message: `Enter ${name}` });
    ///     return { type: "api_key", key };
    /// },
    /// ```
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        let key = interaction
            .prompt(AuthPrompt::secret(format!("Enter {}", self.name)))
            .await?;
        Ok(Credential::api_key(key))
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
