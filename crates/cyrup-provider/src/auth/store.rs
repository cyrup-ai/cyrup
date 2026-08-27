//! The credential store contract + an in-memory default (arch-01 §3.7 / func-01 §7.2).
//!
//! The file-backed (`auth.json`, cross-process-locked) implementation lives in `cyrup-config`
//! (arch-07) and implements this same trait; only the trait + an in-memory default belong here.

use super::types::{Credential, CredentialInfo};
use crate::error::AuthError;
use cyrup_core::ProviderId;
use dashmap::DashMap;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio::sync::Mutex;

/// The read-modify-write closure for [`CredentialStore::modify`]. It receives the current credential
/// and returns the new one (`Some`) or `None` to leave it unchanged. Runs UNDER the per-provider
/// lock (func-01 R-01-014) so an OAuth refresh inside it cannot race a concurrent caller.
pub type ModifyFn = Box<
    dyn FnOnce(Option<Credential>) -> BoxFuture<'static, Result<Option<Credential>, AuthError>>
        + Send,
>;

/// Persistence behind stored API keys / OAuth tokens (func-01 §7.2).
#[async_trait::async_trait]
pub trait CredentialStore: Send + Sync {
    /// `None` if absent; `Err` only on storage failure.
    async fn read(&self, provider: &ProviderId) -> Result<Option<Credential>, AuthError>;

    /// "List stored credential metadata without resolving or exposing secrets. Implementations must
    /// not execute configured API-key commands while listing." — Pi `CredentialStore.list`
    /// (`ai/src/auth/types.ts:71`, present since v0.83.0).
    ///
    /// This is the enumeration half of the contract: `read` answers for ONE provider, `list` says
    /// which providers have an entry at all, and it answers with [`CredentialInfo`] rather than
    /// [`Credential`] precisely so a status/logout surface never handles a secret. Upstream's own
    /// consumers are `ModelRuntime.listCredentials()` (model-runtime.ts:424) feeding
    /// `getLogoutProviderOptions` (interactive-mode.ts:4889-4898) and `resolveCredentialForPrint`
    /// (credential-print.ts:93), plus the `storedProviders` snapshot behind
    /// `getProviderAuthStatus` (model-runtime.ts:254, 429-436).
    ///
    /// Order is the store's own. `Err` only on storage failure.
    async fn list(&self) -> Result<Vec<CredentialInfo>, AuthError>;

    /// THE ONLY write path. Serialized read-modify-write per provider id (func-01 R-01-014). The
    /// OAuth refresh MUST happen inside `f` so concurrent requests/processes cannot double-refresh a
    /// rotated token. Unrelated providers MUST NOT contend (R-01-067).
    async fn modify(
        &self,
        provider: &ProviderId,
        f: ModifyFn,
    ) -> Result<Option<Credential>, AuthError>;

    /// Serialized against `modify`.
    async fn delete(&self, provider: &ProviderId) -> Result<(), AuthError>;
}

/// The default in-memory store. Uses a per-provider `tokio::sync::Mutex` so `modify` on provider A
/// never contends with B (func-01 R-01-067).
#[derive(Default)]
pub struct InMemoryCredentialStore {
    creds: DashMap<ProviderId, Credential>,
    locks: DashMap<ProviderId, Arc<Mutex<()>>>,
}

impl InMemoryCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a credential (builder-style, for tests / programmatic config).
    #[must_use]
    pub fn with_credential(self, provider: ProviderId, cred: Credential) -> Self {
        self.creds.insert(provider, cred);
        self
    }

    /// Synchronously seed/replace a credential.
    pub fn insert(&self, provider: ProviderId, cred: Credential) {
        self.creds.insert(provider, cred);
    }

    fn lock_for(&self, provider: &ProviderId) -> Arc<Mutex<()>> {
        self.locks
            .entry(provider.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[async_trait::async_trait]
impl CredentialStore for InMemoryCredentialStore {
    async fn read(&self, provider: &ProviderId) -> Result<Option<Credential>, AuthError> {
        Ok(self.creds.get(provider).map(|c| c.clone()))
    }

    async fn list(&self) -> Result<Vec<CredentialInfo>, AuthError> {
        // Metadata only — the key/token never leaves the map, matching Pi's "without resolving or
        // exposing secrets" (auth/types.ts:68-71). Sorted so enumeration is deterministic; Pi's
        // `AuthStorage.list` walks `Object.entries` of a JSON object, which is likewise stable.
        let mut out: Vec<CredentialInfo> = self
            .creds
            .iter()
            .map(|e| CredentialInfo {
                provider: e.key().clone(),
                credential_type: e.value().credential_type(),
            })
            .collect();
        out.sort_by(|a, b| a.provider.as_str().cmp(b.provider.as_str()));
        Ok(out)
    }

    async fn modify(
        &self,
        provider: &ProviderId,
        f: ModifyFn,
    ) -> Result<Option<Credential>, AuthError> {
        let lock = self.lock_for(provider);
        let _guard = lock.lock().await;
        // Snapshot the current value (no DashMap ref held across the await).
        let current = self.creds.get(provider).map(|c| c.clone());
        match f(current).await? {
            Some(new) => {
                self.creds.insert(provider.clone(), new.clone());
                Ok(Some(new))
            }
            // Unchanged: return the still-current value (e.g. another task already refreshed it).
            None => Ok(self.creds.get(provider).map(|c| c.clone())),
        }
    }

    async fn delete(&self, provider: &ProviderId) -> Result<(), AuthError> {
        let lock = self.lock_for(provider);
        let _guard = lock.lock().await;
        self.creds.remove(provider);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth::types::CredentialType;

    #[tokio::test]
    async fn read_modify_delete_roundtrip() {
        let store = InMemoryCredentialStore::new();
        let p = ProviderId::from("anthropic");
        assert!(store.read(&p).await.unwrap().is_none());

        let out = store
            .modify(
                &p,
                Box::new(|_cur| Box::pin(async { Ok(Some(Credential::api_key("sk-1"))) })),
            )
            .await
            .unwrap();
        assert!(matches!(out, Some(Credential::ApiKey { .. })));
        assert!(store.read(&p).await.unwrap().is_some());

        store.delete(&p).await.unwrap();
        assert!(store.read(&p).await.unwrap().is_none());
    }

    /// `CredentialStore.list()` (`ai/src/auth/types.ts:71`, present at v0.83.0): typed metadata for
    /// every stored provider, no secret, and it tracks `modify`/`delete`.
    #[tokio::test]
    async fn list_enumerates_typed_metadata_without_secrets() {
        let store = InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("openai"), Credential::api_key("sk-secret"))
            .with_credential(
                ProviderId::from("anthropic"),
                Credential::Oauth {
                    refresh: "r-secret".into(),
                    access: "a-secret".into(),
                    expires: 0,
                    ext: Default::default(),
                },
            );

        let listed = store.list().await.unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|i| (i.provider.as_str(), i.credential_type))
                .collect::<Vec<_>>(),
            vec![
                ("anthropic", CredentialType::Oauth),
                ("openai", CredentialType::ApiKey),
            ]
        );
        // "without resolving or exposing secrets" — the metadata carries no key material at all.
        let rendered = format!("{listed:?}");
        assert!(
            !rendered.contains("sk-secret")
                && !rendered.contains("a-secret")
                && !rendered.contains("r-secret"),
            "CredentialInfo leaked a secret: {rendered}"
        );

        store.delete(&ProviderId::from("openai")).await.unwrap();
        assert_eq!(
            store
                .list()
                .await
                .unwrap()
                .iter()
                .map(|i| i.provider.as_str().to_string())
                .collect::<Vec<_>>(),
            vec!["anthropic".to_string()],
            "delete drops the entry from the enumeration"
        );

        store
            .modify(
                &ProviderId::from("groq"),
                Box::new(|_| Box::pin(async { Ok(Some(Credential::api_key("k"))) })),
            )
            .await
            .unwrap();
        assert_eq!(store.list().await.unwrap().len(), 2, "modify adds an entry");
    }

    #[tokio::test]
    async fn modify_none_leaves_value_unchanged() {
        let store = InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("p"), Credential::api_key("keep"));
        let p = ProviderId::from("p");
        let out = store
            .modify(&p, Box::new(|_cur| Box::pin(async { Ok(None) })))
            .await
            .unwrap();
        match out {
            Some(Credential::ApiKey { key, .. }) => assert_eq!(key.as_deref(), Some("keep")),
            _ => panic!("expected unchanged api key"),
        }
    }
}
