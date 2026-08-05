//! Auth/credential resolution for e2e tests (Pi `resolveApiKey`/`hasAuthForProvider`/
//! `getRealAuthStorage`, utilities.ts:75-125), reading the real cyrup credential store
//! (`~/.cyrup/agent/auth.json`). cyrup delegates OAuth refresh + write-back to
//! [`cyrup_config::AuthStore`]'s serialized modify path (the same one the provider auth flow uses),
//! so these helpers read the resolved credential without re-implementing refresh.

use std::path::PathBuf;

use cyrup_config::{AuthError, AuthStore, Credential};
use cyrup_core::ProviderId;
use cyrup_provider::OAuthAuth;

/// The credential for authenticated e2e tests: `ANTHROPIC_OAUTH_TOKEN`, else `ANTHROPIC_API_KEY`
/// (Pi `API_KEY`, utilities.ts:26). Tests that need a live key gate on this — `None` ⇒ skip (the
/// Rust analogue of Pi's `describe.skipIf(!API_KEY)`). An empty env var counts as absent.
pub fn api_key() -> Option<String> {
    std::env::var("ANTHROPIC_OAUTH_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.is_empty()))
}

/// Whether [`api_key`] is present — the predicate authenticated e2e tests gate on (Pi `!!API_KEY`,
/// the inverse of `describe.skipIf(!API_KEY)`, utilities.ts:24-26).
pub fn has_api_key() -> bool {
    api_key().is_some()
}

/// The real cyrup agent dir (`$CYRUP_AGENT_DIR`, else `$PI_CODING_AGENT_DIR`, else
/// `~/.cyrup/agent`) — Pi `PI_AGENT_DIR`, utilities.ts:117.
pub fn real_agent_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CYRUP_AGENT_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cyrup").join("agent")
}

/// The real `auth.json` path (Pi `AUTH_PATH`, utilities.ts:32).
pub fn real_auth_path() -> PathBuf {
    real_agent_dir().join("auth.json")
}

/// An [`AuthStore`] backed by the real `~/.cyrup/agent/auth.json` (Pi `getRealAuthStorage`,
/// utilities.ts:123-125).
pub fn get_real_auth_store() -> AuthStore {
    AuthStore::at(real_auth_path())
}

/// Whether a provider has a stored credential (Pi `hasAuthForProvider`, utilities.ts:111-114).
pub async fn has_auth_for_provider(provider: &ProviderId) -> bool {
    matches!(get_real_auth_store().read(provider).await, Ok(Some(_)))
}

/// Resolve a usable request key for a provider from the real store WITHOUT refreshing (Pi
/// `resolveApiKey` fast path, utilities.ts:75-83): an `api_key` credential returns its key; an
/// `oauth` credential returns its stored access token verbatim. Use [`resolve_api_key_refreshing`]
/// when an expired OAuth token must be refreshed + persisted. `None` when absent or keyless.
pub async fn resolve_api_key(provider: &ProviderId) -> Option<String> {
    match get_real_auth_store().read(provider).await.ok()? {
        Some(Credential::ApiKey { key, .. }) => key,
        Some(Credential::Oauth { access, .. }) => Some(access),
        None => None,
    }
}

/// Wall clock in Unix **milliseconds** — the unit `Credential::Oauth.expires` is stored in on disk
/// (Pi writes `Date.now() + expires_in * 1000`, ai/src/auth/oauth/anthropic.ts:225, and compares
/// against `Date.now()`, ai/src/auth/resolve.ts:110).
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Convert between the config and provider `Credential` shapes (identical serde shape: tagged
/// `api_key | oauth`) via a JSON round-trip.
fn to_provider_credential(c: &Credential) -> Result<cyrup_provider::Credential, AuthError> {
    let v = serde_json::to_value(c).map_err(AuthError::Parse)?;
    serde_json::from_value(v).map_err(AuthError::Parse)
}

fn to_config_credential(c: &cyrup_provider::Credential) -> Result<Credential, AuthError> {
    let v = serde_json::to_value(c).map_err(AuthError::Parse)?;
    serde_json::from_value(v).map_err(AuthError::Parse)
}

fn access_of(c: Credential) -> Option<String> {
    match c {
        Credential::ApiKey { key, .. } => key,
        Credential::Oauth { access, .. } => Some(access),
    }
}

/// Resolve a usable request key, refreshing an expired OAuth credential with `oauth` and persisting
/// the refreshed credential back to `auth.json` (1:1 with Pi `resolveApiKey`'s OAuth branch +
/// `getOAuthApiKey`, utilities.ts:85-103 / oauth/index.ts:131-156):
/// - `api_key` ⇒ its key;
/// - `oauth`, not expired ⇒ the stored access token;
/// - `oauth`, expired ⇒ `oauth.refresh(...)` under the per-provider store lock, written back atomically
///   at mode 0600 (via [`AuthStore::modify`], which owns the permission/atomic-write contract), then
///   the freshly-refreshed access token.
///
/// A refresh failure surfaces as [`AuthError::Oauth`] and leaves the stored credential intact for
/// re-login — it never falls back to env (R-01-013 parity). The double-checked closure makes a
/// concurrent refresh idempotent. `None` when absent or keyless.
pub async fn resolve_api_key_refreshing(
    provider: &ProviderId,
    oauth: &dyn OAuthAuth,
) -> Result<Option<String>, AuthError> {
    resolve_api_key_refreshing_in(&get_real_auth_store(), provider, oauth).await
}

/// [`resolve_api_key_refreshing`] against a caller-supplied `store` (for tests / non-default stores).
pub async fn resolve_api_key_refreshing_in(
    store: &AuthStore,
    provider: &ProviderId,
    oauth: &dyn OAuthAuth,
) -> Result<Option<String>, AuthError> {
    let current = match store.read(provider).await? {
        Some(c) => c,
        None => return Ok(None),
    };

    match &current {
        Credential::ApiKey { key, .. } => return Ok(key.clone()),
        Credential::Oauth { access, expires, .. } => {
            if now_millis() < *expires {
                return Ok(Some(access.clone()));
            }
        }
    }

    // Expired: refresh + write-back under the per-provider lock (Pi `saveAuthStorage` 0600).
    let refreshed = store
        .modify(provider, |cur| async move {
            let cur = match cur {
                Some(c) => c,
                None => return Ok(None), // logged out meanwhile
            };
            // Double-check: another request/process may have refreshed it already.
            if let Credential::Oauth { expires, .. } = &cur {
                if now_millis() < *expires {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
            let prov_cred = to_provider_credential(&cur)?;
            let new_prov = oauth
                .refresh(&prov_cred)
                .await
                .map_err(|e| AuthError::Oauth(e.to_string()))?;
            Ok(Some(to_config_credential(&new_prov)?))
        })
        .await?;

    // `modify` returns the post-write credential; `None` means the double-check won — re-read it.
    let resolved = match refreshed {
        Some(c) => c,
        None => match store.read(provider).await? {
            Some(c) => c,
            None => return Ok(None),
        },
    };
    Ok(access_of(resolved))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::tempdir::TestTempDir;
    use cyrup_provider::{Credential as PCred, ModelAuth};

    /// A refresh strategy that mints a fresh, far-future access token (Pi `getOAuthApiKey`'s
    /// `provider.refreshToken`).
    struct FreshOAuth {
        new_access: String,
    }

    #[async_trait::async_trait]
    impl OAuthAuth for FreshOAuth {
        fn name(&self) -> &str {
            "fresh-oauth"
        }
        async fn refresh(&self, _cred: &PCred) -> Result<PCred, cyrup_provider::AuthError> {
            Ok(PCred::Oauth {
                refresh: "new-refresh".into(),
                access: self.new_access.clone(),
                expires: now_millis() + 3_600_000,
                ext: serde_json::Map::new(),
            })
        }
        async fn to_auth(&self, _cred: &PCred) -> Result<ModelAuth, cyrup_provider::AuthError> {
            Ok(ModelAuth::default())
        }
    }

    fn store() -> (AuthStore, TestTempDir) {
        let dir = TestTempDir::new().unwrap();
        (AuthStore::at(dir.path().join("auth.json")), dir)
    }

    #[tokio::test]
    async fn expired_oauth_is_refreshed_and_written_back() {
        let (s, _dir) = store();
        let provider = ProviderId::from("anthropic");
        // Seed an EXPIRED oauth credential.
        s.modify(&provider, |_| async {
            Ok(Some(Credential::Oauth {
                refresh: "r".into(),
                access: "stale-token".into(),
                expires: 0,
                ext: serde_json::Map::new(),
            }))
        })
        .await
        .unwrap();

        let oauth = FreshOAuth { new_access: "fresh-token".into() };
        let key = resolve_api_key_refreshing_in(&s, &provider, &oauth).await.unwrap();
        assert_eq!(key.as_deref(), Some("fresh-token"));

        // The refreshed credential is PERSISTED (Pi `saveAuthStorage`), not just returned.
        match s.read(&provider).await.unwrap() {
            Some(Credential::Oauth { access, expires, .. }) => {
                assert_eq!(access, "fresh-token");
                assert!(expires > now_millis());
            }
            other => panic!("expected refreshed oauth, got {other:?}"),
        }
    }

    /// `expires` is Unix MILLISECONDS (Pi `Date.now() + expires_in * 1000`,
    /// ai/src/auth/oauth/anthropic.ts:225). Regression guard for CFG-011: a seconds clock is ~1000x
    /// smaller than a real millisecond deadline, so a token that expired a minute ago was reported
    /// as still valid and handed back verbatim instead of being refreshed.
    #[tokio::test]
    async fn recently_expired_millisecond_deadline_is_refreshed() {
        let (s, _dir) = store();
        let provider = ProviderId::from("anthropic");
        s.modify(&provider, |_| async {
            Ok(Some(Credential::Oauth {
                refresh: "r".into(),
                access: "stale-token".into(),
                // Expired one minute ago, in the unit the file actually stores.
                expires: now_millis() - 60_000,
                ext: serde_json::Map::new(),
            }))
        })
        .await
        .unwrap();

        let oauth = FreshOAuth { new_access: "fresh-token".into() };
        let key = resolve_api_key_refreshing_in(&s, &provider, &oauth).await.unwrap();
        assert_eq!(
            key.as_deref(),
            Some("fresh-token"),
            "a millisecond deadline one minute in the past must be treated as expired"
        );
    }

    #[tokio::test]
    async fn valid_oauth_returns_verbatim_without_refresh() {
        let (s, _dir) = store();
        let provider = ProviderId::from("anthropic");
        s.modify(&provider, |_| async {
            Ok(Some(Credential::Oauth {
                refresh: "r".into(),
                access: "live-token".into(),
                expires: now_millis() + 3_600_000,
                ext: serde_json::Map::new(),
            }))
        })
        .await
        .unwrap();

        let oauth = FreshOAuth { new_access: "should-not-be-used".into() };
        let key = resolve_api_key_refreshing_in(&s, &provider, &oauth).await.unwrap();
        assert_eq!(key.as_deref(), Some("live-token"));
    }

    #[tokio::test]
    async fn api_key_credential_returns_key() {
        let (s, _dir) = store();
        let provider = ProviderId::from("anthropic");
        s.modify(&provider, |_| async { Ok(Some(Credential::api_key("ak"))) }).await.unwrap();
        let oauth = FreshOAuth { new_access: "x".into() };
        let key = resolve_api_key_refreshing_in(&s, &provider, &oauth).await.unwrap();
        assert_eq!(key.as_deref(), Some("ak"));
    }

    #[tokio::test]
    async fn absent_credential_is_none() {
        let (s, _dir) = store();
        let oauth = FreshOAuth { new_access: "x".into() };
        let key = resolve_api_key_refreshing_in(&s, &ProviderId::from("none"), &oauth).await.unwrap();
        assert!(key.is_none());
    }
}
