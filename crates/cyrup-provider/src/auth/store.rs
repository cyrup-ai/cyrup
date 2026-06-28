//! The credential store contract + an in-memory default (arch-01 §3.7 / func-01 §7.2).
//!
//! The file-backed (`auth.json`, cross-process-locked) implementation lives in `cyrup-config`
//! (arch-07) and implements this same trait; only the trait + an in-memory default belong here.

use super::types::Credential;
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
