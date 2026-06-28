//! Credential store (`auth.json`) — the file-backed `CredentialStore` impl: serialized
//! read-modify-write, OAuth refresh under the lock, cross-process file lock, 0600 perms, plus the
//! request-time precedence helper (arch-07 §3.5/§6.3, R-07-014…R-07-017, R-07-030).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use cyrup_core::ProviderId;
use serde_json::{Map, Value};

use crate::env::ConfigDirs;
use crate::error::AuthError;

/// A stored credential (R-07-016). `api_key` optionally carries provider-scoped `env`; `oauth`
/// carries refresh/access/expires plus any provider-specific extension fields.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Credential {
    ApiKey {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        env: Option<BTreeMap<String, String>>,
    },
    Oauth {
        refresh: String,
        access: String,
        expires: i64,
        #[serde(flatten)]
        ext: Map<String, Value>,
    },
}

impl Credential {
    pub fn api_key(key: impl Into<String>) -> Self {
        Credential::ApiKey { key: Some(key.into()), env: None }
    }

    /// The usable request key for an `api_key` credential (None for OAuth, handled separately).
    pub fn stored_api_key(&self) -> Option<&str> {
        match self {
            Credential::ApiKey { key, .. } => key.as_deref(),
            Credential::Oauth { .. } => None,
        }
    }
}

/// The on-disk shape: `{ "<provider>": <Credential> }`.
type AuthFile = BTreeMap<String, Credential>;

fn parse_auth(text: &str) -> Result<AuthFile, AuthError> {
    if text.trim().is_empty() {
        return Ok(AuthFile::new());
    }
    serde_json::from_str(text).map_err(AuthError::Parse)
}

/// File-backed credential store (arch-07 §3.5).
pub struct AuthStore {
    path: PathBuf,
    /// Per-provider in-process serialization so unrelated providers never contend (R-07-030).
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// `--api-key`: explicit per-run override (top precedence tier). Not persisted.
    runtime: RwLock<HashMap<String, String>>,
}

impl AuthStore {
    /// Open the store; does not read `auth.json` until the first credential op (lazy).
    pub fn open(dirs: &ConfigDirs) -> Self {
        Self::at(dirs.auth_path())
    }

    pub fn at(path: PathBuf) -> Self {
        Self {
            path,
            locks: Mutex::new(HashMap::new()),
            runtime: RwLock::new(HashMap::new()),
        }
    }

    fn provider_lock(&self, provider: &ProviderId) -> Arc<tokio::sync::Mutex<()>> {
        let key = provider.as_str().to_string();
        let mut guard = match self.locks.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        Arc::clone(guard.entry(key).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))))
    }

    /// `--api-key`: explicit per-run override (R-07-018).
    pub fn set_runtime_api_key(&self, provider: ProviderId, key: String) {
        if let Ok(mut g) = self.runtime.write() {
            g.insert(provider.as_str().to_string(), key);
        }
    }

    pub fn remove_runtime_api_key(&self, provider: &ProviderId) {
        if let Ok(mut g) = self.runtime.write() {
            g.remove(provider.as_str());
        }
    }

    pub fn runtime_api_key(&self, provider: &ProviderId) -> Option<String> {
        self.runtime.read().ok().and_then(|g| g.get(provider.as_str()).cloned())
    }

    fn read_file(&self) -> Result<AuthFile, AuthError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => parse_auth(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AuthFile::new()),
            Err(e) => Err(AuthError::Io(e)),
        }
    }

    /// Read a provider's credential (`None` if absent). A bad file surfaces `Parse` (R-01-017).
    pub async fn read(&self, provider: &ProviderId) -> Result<Option<Credential>, AuthError> {
        let map = self.read_file()?;
        Ok(map.get(provider.as_str()).cloned())
    }

    /// Serialized read-modify-write per provider — the ONLY write path. OAuth refresh MUST happen
    /// inside `f`; a failure there preserves the stored credential and never falls back to env
    /// (R-07-015/017, R-01-014). Returns the post-write credential.
    pub async fn modify<F, Fut>(
        &self,
        provider: &ProviderId,
        f: F,
    ) -> Result<Option<Credential>, AuthError>
    where
        F: FnOnce(Option<Credential>) -> Fut,
        Fut: Future<Output = Result<Option<Credential>, AuthError>>,
    {
        let provider_lock = self.provider_lock(provider);
        let _held = provider_lock.lock().await;

        // Cross-process critical section.
        let flock = crate::lock::FileLock::acquire(&self.path)
            .map_err(|e| AuthError::Lock(e.to_string()))?;

        let mut map = self.read_file()?;
        let current = map.get(provider.as_str()).cloned();

        // OAuth refresh / mutation happens here, under both locks.
        let next = f(current).await?;

        match next {
            Some(cred) => {
                map.insert(provider.as_str().to_string(), cred);
            }
            None => {
                map.remove(provider.as_str());
            }
        }

        let obj: Map<String, Value> = map
            .iter()
            .filter_map(|(k, v)| serde_json::to_value(v).ok().map(|v| (k.clone(), v)))
            .collect();
        let mut text = serde_json::to_string_pretty(&Value::Object(obj)).map_err(AuthError::Parse)?;
        text.push('\n');
        crate::lock::write_atomic(&self.path, text.as_bytes(), true)
            .map_err(|e| AuthError::Lock(e.to_string()))?;

        drop(flock);
        Ok(map.get(provider.as_str()).cloned())
    }

    /// Delete a provider's credential (falls back to env on next resolution).
    pub async fn delete(&self, provider: &ProviderId) -> Result<(), AuthError> {
        self.modify(provider, |_current| async { Ok(None) }).await?;
        Ok(())
    }
}

/// What the credential store yields for request-time resolution (arch-07 §6.3).
#[derive(Clone, Debug, PartialEq)]
pub enum Stored {
    /// No stored credential — env/configured fallbacks apply.
    None,
    /// A stored api_key (suppresses env).
    ApiKey(String),
    /// A valid OAuth access token (suppresses env).
    OAuth(String),
    /// An OAuth credential whose refresh FAILED — must NOT fall back to env (R-07-017 / A-07-5).
    OAuthRefreshFailed,
}

/// The precedence tier a resolved key came from (R-07-017).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    /// explicit per-request, incl. `--api-key`.
    Explicit,
    Stored,
    /// programmatic SDK-set default (ranked below stored).
    RuntimeAssigned,
    Env,
    Configured,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAuth {
    pub key: String,
    pub source: CredentialSource,
}

/// Request-time auth resolution precedence (R-07-017 / R-01-011):
/// `explicit > stored > runtime-assigned > env > configured`. A stored credential suppresses env
/// (R-01-012); an OAuth refresh failure does NOT fall back to env (R-01-013 / A-07-5).
pub fn resolve_auth(
    explicit: Option<&str>,
    stored: &Stored,
    runtime_assigned: Option<&str>,
    env: Option<&str>,
    configured: Option<&str>,
) -> Result<Option<ResolvedAuth>, AuthError> {
    // 1. explicit per-request (incl. --api-key) — top tier.
    if let Some(k) = explicit {
        return Ok(Some(ResolvedAuth { key: k.to_string(), source: CredentialSource::Explicit }));
    }
    // 2. stored credential — suppresses env.
    match stored {
        Stored::ApiKey(k) => {
            return Ok(Some(ResolvedAuth { key: k.clone(), source: CredentialSource::Stored }));
        }
        Stored::OAuth(t) => {
            return Ok(Some(ResolvedAuth { key: t.clone(), source: CredentialSource::Stored }));
        }
        Stored::OAuthRefreshFailed => {
            // Stored credential present but refresh failed: no env fallback (A-07-5).
            return Err(AuthError::Oauth("refresh failed; refusing env fallback".to_string()));
        }
        Stored::None => {}
    }
    // 3. runtime-assigned programmatic default.
    if let Some(k) = runtime_assigned {
        return Ok(Some(ResolvedAuth {
            key: k.to_string(),
            source: CredentialSource::RuntimeAssigned,
        }));
    }
    // 4. environment variables.
    if let Some(k) = env {
        return Ok(Some(ResolvedAuth { key: k.to_string(), source: CredentialSource::Env }));
    }
    // 5. model/provider-configured key/command.
    if let Some(k) = configured {
        return Ok(Some(ResolvedAuth {
            key: k.to_string(),
            source: CredentialSource::Configured,
        }));
    }
    Ok(None)
}

/// OAuth PKCE (S256) helpers (R-07-016). Correct, pure transforms; callers supply randomness.
pub mod pkce {
    use base64::Engine;
    use sha2::{Digest, Sha256};

    /// Encode raw random bytes as a PKCE `code_verifier` (RFC 7636 base64url, no padding).
    pub fn verifier_from_bytes(bytes: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Compute the S256 `code_challenge` for a `code_verifier`.
    pub fn challenge(verifier: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let digest = hasher.finalize();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn store() -> (AuthStore, PathBuf) {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("agent").join("auth.json");
        (AuthStore::at(path.clone()), path)
    }

    #[tokio::test]
    async fn stored_over_env_then_delete_falls_back() {
        // A-07-5: stored credential used over env; delete → env.
        let (s, _p) = store();
        let provider = ProviderId::from("anthropic");
        s.modify(&provider, |_| async { Ok(Some(Credential::api_key("stored-key"))) })
            .await
            .unwrap();

        let stored = s.read(&provider).await.unwrap();
        let stored_kind = match stored {
            Some(Credential::ApiKey { key: Some(k), .. }) => Stored::ApiKey(k),
            _ => Stored::None,
        };
        let resolved = resolve_auth(None, &stored_kind, None, Some("env-key"), None)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.key, "stored-key");
        assert_eq!(resolved.source, CredentialSource::Stored);

        // delete → env fallback
        s.delete(&provider).await.unwrap();
        assert!(s.read(&provider).await.unwrap().is_none());
        let resolved = resolve_auth(None, &Stored::None, None, Some("env-key"), None)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.key, "env-key");
        assert_eq!(resolved.source, CredentialSource::Env);
    }

    #[test]
    fn api_key_override_beats_stored_and_env() {
        // A-07-5: --api-key overrides both.
        let r = resolve_auth(
            Some("cli-key"),
            &Stored::ApiKey("stored".into()),
            None,
            Some("env"),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(r.key, "cli-key");
        assert_eq!(r.source, CredentialSource::Explicit);
    }

    #[test]
    fn oauth_refresh_failure_no_env_fallback() {
        // A-07-5
        let r = resolve_auth(None, &Stored::OAuthRefreshFailed, None, Some("env"), None);
        assert!(matches!(r, Err(AuthError::Oauth(_))));
    }

    #[test]
    fn precedence_runtime_then_env_then_configured() {
        let r = resolve_auth(None, &Stored::None, Some("rt"), Some("env"), Some("cfg"))
            .unwrap()
            .unwrap();
        assert_eq!(r.source, CredentialSource::RuntimeAssigned);
        let r = resolve_auth(None, &Stored::None, None, Some("env"), Some("cfg"))
            .unwrap()
            .unwrap();
        assert_eq!(r.source, CredentialSource::Env);
        let r = resolve_auth(None, &Stored::None, None, None, Some("cfg")).unwrap().unwrap();
        assert_eq!(r.source, CredentialSource::Configured);
        assert!(resolve_auth(None, &Stored::None, None, None, None).unwrap().is_none());
    }

    #[tokio::test]
    async fn oauth_refresh_failure_preserves_credential() {
        // R-07-017: a failure in `f` does not write; the stored credential is preserved.
        let (s, _p) = store();
        let provider = ProviderId::from("claude");
        let original = Credential::Oauth {
            refresh: "r".into(),
            access: "a".into(),
            expires: 0,
            ext: Map::new(),
        };
        s.modify(&provider, {
            let original = original.clone();
            |_| async move { Ok(Some(original)) }
        })
        .await
        .unwrap();

        let err = s
            .modify(&provider, |_| async {
                Err::<Option<Credential>, _>(AuthError::Oauth("boom".into()))
            })
            .await;
        assert!(matches!(err, Err(AuthError::Oauth(_))));
        // credential preserved
        assert_eq!(s.read(&provider).await.unwrap(), Some(original));
    }

    #[tokio::test]
    async fn concurrent_modify_same_provider_serializes() {
        // A-07-9 / R-07-030: no interleaving (no double refresh) on the same provider.
        let dir = crate::test_util::temp_dir();
        let path = dir.join("auth.json");
        let s = Arc::new(AuthStore::at(path));
        let provider = ProviderId::from("p");
        let inflight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for i in 0..8u32 {
            let s = Arc::clone(&s);
            let provider = provider.clone();
            let inflight = Arc::clone(&inflight);
            let max_seen = Arc::clone(&max_seen);
            handles.push(tokio::spawn(async move {
                s.modify(&provider, |_cur| async move {
                    let n = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(n, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                    inflight.fetch_sub(1, Ordering::SeqCst);
                    Ok(Some(Credential::api_key(format!("k{i}"))))
                })
                .await
                .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        // Never more than one critical section in flight at a time.
        assert_eq!(max_seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_providers_proceed() {
        let (s, _p) = store();
        let a = ProviderId::from("a");
        let b = ProviderId::from("b");
        s.modify(&a, |_| async { Ok(Some(Credential::api_key("ka"))) }).await.unwrap();
        s.modify(&b, |_| async { Ok(Some(Credential::api_key("kb"))) }).await.unwrap();
        assert_eq!(s.read(&a).await.unwrap().unwrap().stored_api_key(), Some("ka"));
        assert_eq!(s.read(&b).await.unwrap().unwrap().stored_api_key(), Some("kb"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owner_only_permissions() {
        // A-07-9 / R-07-014
        use std::os::unix::fs::PermissionsExt;
        let (s, path) = store();
        let provider = ProviderId::from("x");
        s.modify(&provider, |_| async { Ok(Some(Credential::api_key("k"))) }).await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode =
            std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn malformed_auth_file_surfaces_parse_error() {
        let r = parse_auth("{ not json");
        assert!(matches!(r, Err(AuthError::Parse(_))));
    }

    #[test]
    fn pkce_challenge_is_stable() {
        // RFC 7636 appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(pkce::challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
}
