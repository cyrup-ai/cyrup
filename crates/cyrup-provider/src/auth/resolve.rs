//! Auth resolution precedence engine (arch-01 §6.2 / func-01 R-01-011..013).
//!
//! Precedence (highest wins): explicit per-request → stored credential → ambient (env). A stored
//! credential owns its provider (env is not consulted, R-01-012); a failed OAuth refresh surfaces a
//! typed error and never falls back to an env key (R-01-013).

use super::store::CredentialStore;
use super::types::{AuthContext, AuthResult, Credential, ProviderEnv};
use super::{OAuthAuth, ProviderAuth};
use crate::error::AuthError;
use crate::model::Model;
use cyrup_core::ProviderId;
use std::sync::Arc;

/// Per-request overrides feeding the explicit (top) precedence tier (func-01 R-01-011 #1).
#[derive(Clone, Copy, Default)]
pub struct AuthOverrides<'a> {
    pub api_key: Option<&'a str>,
    pub env: Option<&'a ProviderEnv>,
}

/// Resolve auth for one request (func-01 R-01-011..013). Returns `Ok(None)` for an unconfigured
/// provider; `Err` carries the R-01-017 taxonomy.
pub async fn resolve_provider_auth(
    provider_id: &ProviderId,
    auth: &ProviderAuth,
    model: &Model,
    store: &dyn CredentialStore,
    ctx: &dyn AuthContext,
    overrides: AuthOverrides<'_>,
) -> Result<Option<AuthResult>, AuthError> {
    // 1. Explicit per-request key (highest precedence, R-01-011 #1).
    if let (Some(key), Some(api_key_auth)) = (overrides.api_key, auth.api_key.as_ref()) {
        let cred = Credential::ApiKey { key: Some(key.to_string()), env: overrides.env.cloned() };
        return api_key_auth.resolve(model, ctx, Some(&cred)).await;
    }

    // 2. Stored credential — a stored credential owns the provider (R-01-012).
    match store.read(provider_id).await {
        Err(e) => Err(e),
        Ok(Some(cred)) => {
            // OAuth credential: refresh-if-needed under lock; never falls back to env (R-01-013).
            if let Credential::Oauth { .. } = &cred
                && let Some(oauth) = auth.oauth.clone()
            {
                return resolve_stored_oauth(provider_id, oauth, store, cred).await;
            }
            // API-key credential: resolve with the stored credential (env not consulted, R-01-012).
            if let Credential::ApiKey { .. } = &cred
                && let Some(api_key_auth) = auth.api_key.as_ref()
            {
                return api_key_auth.resolve(model, ctx, Some(&cred)).await;
            }
            // Stored credential type with no matching handler → treat as not configured.
            Ok(None)
        }
        // 3. Ambient (env). Only reached when nothing is stored (R-01-011 #3 / R-01-012).
        Ok(None) => match auth.api_key.as_ref() {
            Some(api_key_auth) => api_key_auth.resolve(model, ctx, None).await,
            None => Ok(None),
        },
    }
}

/// Resolve a stored OAuth credential, refreshing under the store lock if expired (func-01
/// R-01-013/014/067). A refresh failure returns `AuthError::OAuth` with the stored credential left
/// intact for re-login; it never falls back to an env key.
async fn resolve_stored_oauth(
    provider_id: &ProviderId,
    oauth: Arc<dyn OAuthAuth>,
    store: &dyn CredentialStore,
    stored: Credential,
) -> Result<Option<AuthResult>, AuthError> {
    let expires = match &stored {
        Credential::Oauth { expires, .. } => *expires,
        _ => return Ok(None),
    };

    let cred = if now_secs() >= expires {
        // Refresh authoritatively, UNDER the per-provider lock (R-01-014/067). The closure is
        // double-checked so a concurrent refresh is not duplicated.
        let oauth_for = oauth.clone();
        let modified = store
            .modify(
                provider_id,
                Box::new(move |current| {
                    Box::pin(async move {
                        let current = match current {
                            Some(c) => c,
                            None => return Ok(None), // logged out meanwhile
                        };
                        match &current {
                            Credential::Oauth { expires, .. } => {
                                if now_secs() < *expires {
                                    // Another request/process already refreshed it.
                                    return Ok(None);
                                }
                            }
                            _ => return Ok(None),
                        }
                        // Network refresh inside the lock; failure surfaces as AuthError::OAuth and
                        // leaves the stored credential untouched (R-01-013).
                        let refreshed = oauth_for.refresh(&current).await?;
                        Ok(Some(refreshed))
                    })
                }),
            )
            .await?;
        match modified {
            Some(c) => c,
            None => return Ok(None),
        }
    } else {
        stored
    };

    let auth = oauth.to_auth(&cred).await?;
    Ok(Some(AuthResult { auth, env: cred.env().cloned(), source: Some("OAuth".to_string()) }))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth::helpers::env_key;
    use crate::auth::store::InMemoryCredentialStore;
    use crate::auth::types::{AuthContext, Credential, ModelAuth};
    use crate::auth::OAuthAuth;
    use crate::model::{Modality, Model, ModelCost};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_model() -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            api: "test".into(),
            provider: "p".into(),
            base_url: None,
            reasoning: false,
            input: vec![Modality::Text],
            output: Vec::new(),
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 100,
            thinking_level_map: None,
            compat: None,
        }
    }

    /// Map-backed ambient context for deterministic env precedence tests.
    struct MapCtx(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn stored_beats_env() {
        let store = InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("p"), Credential::api_key("stored-key"));
        let ctx = MapCtx(BTreeMap::from([("API_KEY".to_string(), "env-key".to_string())]));
        let auth = ProviderAuth::with_api_key(env_key(["API_KEY"]));
        let r = resolve_provider_auth(
            &ProviderId::from("p"),
            &auth,
            &test_model(),
            &store,
            &ctx,
            AuthOverrides::default(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(r.auth.api_key.as_deref(), Some("stored-key"));
        assert_eq!(r.source.as_deref(), Some("stored"));
    }

    #[tokio::test]
    async fn falls_back_to_env_when_nothing_stored() {
        let store = InMemoryCredentialStore::new();
        let ctx = MapCtx(BTreeMap::from([("API_KEY".to_string(), "env-key".to_string())]));
        let auth = ProviderAuth::with_api_key(env_key(["API_KEY"]));
        let r = resolve_provider_auth(
            &ProviderId::from("p"),
            &auth,
            &test_model(),
            &store,
            &ctx,
            AuthOverrides::default(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(r.auth.api_key.as_deref(), Some("env-key"));
        assert_eq!(r.source.as_deref(), Some("env"));
    }

    #[tokio::test]
    async fn explicit_override_beats_stored_and_env() {
        let store = InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("p"), Credential::api_key("stored-key"));
        let ctx = MapCtx(BTreeMap::from([("API_KEY".to_string(), "env-key".to_string())]));
        let auth = ProviderAuth::with_api_key(env_key(["API_KEY"]));
        let r = resolve_provider_auth(
            &ProviderId::from("p"),
            &auth,
            &test_model(),
            &store,
            &ctx,
            AuthOverrides { api_key: Some("explicit-key"), env: None },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(r.auth.api_key.as_deref(), Some("explicit-key"));
    }

    #[tokio::test]
    async fn unconfigured_resolves_none() {
        let store = InMemoryCredentialStore::new();
        let ctx = MapCtx(BTreeMap::new());
        let auth = ProviderAuth::with_api_key(env_key(["API_KEY"]));
        let r = resolve_provider_auth(
            &ProviderId::from("p"),
            &auth,
            &test_model(),
            &store,
            &ctx,
            AuthOverrides::default(),
        )
        .await
        .unwrap();
        assert!(r.is_none());
    }

    /// An OAuth strategy whose refresh always fails (to prove no env fallback).
    struct FailingOAuth {
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl OAuthAuth for FailingOAuth {
        fn name(&self) -> &str {
            "failing-oauth"
        }
        async fn refresh(&self, _cred: &Credential) -> Result<Credential, AuthError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(AuthError::oauth(ProviderId::from("p"), "refresh boom"))
        }
        async fn to_auth(&self, _cred: &Credential) -> Result<ModelAuth, AuthError> {
            Ok(ModelAuth::default())
        }
    }

    #[tokio::test]
    async fn oauth_refresh_failure_does_not_fall_back_to_env() {
        // Expired OAuth credential stored; an env key is ALSO present.
        let expired = Credential::Oauth {
            refresh: "r".into(),
            access: "a".into(),
            expires: 0, // far in the past → refresh required
            ext: serde_json::Map::new(),
        };
        let store = InMemoryCredentialStore::new().with_credential(ProviderId::from("p"), expired);
        let ctx = MapCtx(BTreeMap::from([("API_KEY".to_string(), "env-key".to_string())]));
        let oauth = Arc::new(FailingOAuth { calls: AtomicUsize::new(0) });
        // Provider supports BOTH oauth and an env key — but the failed refresh must NOT use the env.
        let auth = ProviderAuth {
            api_key: Some(env_key(["API_KEY"])),
            oauth: Some(oauth.clone()),
        };
        let err = resolve_provider_auth(
            &ProviderId::from("p"),
            &auth,
            &test_model(),
            &store,
            &ctx,
            AuthOverrides::default(),
        )
        .await
        .expect_err("oauth refresh failure should be a typed error");
        assert_eq!(err.code(), "oauth");
        assert_eq!(oauth.calls.load(Ordering::SeqCst), 1);

        // The stored credential is preserved for re-login (R-01-013).
        match store.read(&ProviderId::from("p")).await.unwrap() {
            Some(Credential::Oauth { .. }) => {}
            other => panic!("stored oauth credential should be preserved, got {other:?}"),
        }
    }
}
