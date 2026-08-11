//! Credential store (`auth.json`) — the file-backed `CredentialStore` impl: serialized
//! read-modify-write, OAuth refresh under the lock, cross-process file lock, 0600 perms, plus the
//! request-time precedence helper (arch-07 §3.5/§6.3, R-07-014…R-07-017, R-07-030).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use cyrup_core::ProviderId;
use cyrup_provider::{CredentialInfo, CredentialType};
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
        Credential::ApiKey {
            key: Some(key.into()),
            env: None,
        }
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
        Arc::clone(
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
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
        self.runtime
            .read()
            .ok()
            .and_then(|g| g.get(provider.as_str()).cloned())
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

    /// Provider-scoped env of a stored `api_key` credential (Pi `AuthStorage.getProviderEnv`,
    /// auth-storage.ts:305-308). Returns `Some` (a copy of the credential's `env` map, possibly
    /// empty — mirroring Pi's truthy `cred.env` check on a present object) only for an `api_key`
    /// credential that carries an `env`; `None` for OAuth, a missing credential, or an `api_key`
    /// without `env`. The model-registry consumer (model-registry.ts:704,809) passes this map as
    /// the scoped `env` when resolving a `models.json` `apiKey`/`headers` config value via
    /// [`crate::config_value::resolve_config_value`].
    pub async fn get_provider_env(
        &self,
        provider: &ProviderId,
    ) -> Result<Option<HashMap<String, String>>, AuthError> {
        match self.read(provider).await? {
            Some(Credential::ApiKey { env: Some(env), .. }) => Ok(Some(env.into_iter().collect())),
            _ => Ok(None),
        }
    }

    /// All providers with a stored credential in `auth.json` (Pi `AuthStorage.list`,
    /// auth-storage.ts:329-331). Used by the interactive auth UI to enumerate configured providers
    /// (interactive-mode.ts:4671). Keys are returned in sorted order (the on-disk map is a
    /// `BTreeMap`), which is deterministic; Pi returns `Object.keys` insertion order.
    pub fn list(&self) -> Result<Vec<String>, AuthError> {
        Ok(self.read_file()?.into_keys().collect())
    }

    /// The COMPOSED credential enumeration `ModelRuntime.listCredentials()` returns
    /// (model-runtime.ts:424 → `RuntimeCredentials.list()`, runtime-credentials.ts:29-36): the
    /// `auth.json` entries **overlaid with the runtime `--api-key` providers**, each as
    /// `{ providerId, type }` and never a secret.
    ///
    /// [`Self::list`] is the FILE tier alone (Pi's inner `AuthStorage.list`). This is the outer
    /// tier: cyrup fuses Pi's `AuthStorage` and its `RuntimeCredentials` decorator into one
    /// [`AuthStore`], so the decorator's overlay has to be applied here or it is applied nowhere —
    /// which is what made a `--api-key`-supplied provider invisible to `/logout`, where Pi lists it
    /// (`getLogoutProviderOptions` → `listCredentials()`, interactive-mode.ts:4890).
    ///
    /// Reads the file ONCE (`list()` + a `read()` per provider was N+1 reads of the same file) and
    /// never resolves a `!command` / `$VAR` api-key value — Pi: "Implementations must not execute
    /// configured API-key commands while listing" (`ai/src/auth/types.ts:69-70`).
    pub fn list_credentials(&self) -> Result<Vec<CredentialInfo>, AuthError> {
        let mut out: Vec<CredentialInfo> = self
            .read_file()?
            .into_iter()
            .map(|(provider, cred)| CredentialInfo {
                provider: ProviderId::from(provider.as_str()),
                credential_type: match cred {
                    Credential::ApiKey { .. } => CredentialType::ApiKey,
                    Credential::Oauth { .. } => CredentialType::Oauth,
                },
            })
            .collect();
        // `for (const providerId of this.overrides.keys()) entries.set(providerId, {providerId,
        // type: "api_key"})` — a runtime key REPLACES the stored entry's type, so `--api-key` on a
        // provider whose stored credential is OAuth enumerates as `api_key`, exactly as upstream's
        // `Map.set` overwrite does.
        if let Ok(runtime) = self.runtime.read() {
            for provider in runtime.keys() {
                let id = ProviderId::from(provider.as_str());
                match out.iter_mut().find(|e| e.provider == id) {
                    Some(existing) => existing.credential_type = CredentialType::ApiKey,
                    None => out.push(CredentialInfo {
                        provider: id,
                        credential_type: CredentialType::ApiKey,
                    }),
                }
            }
        }
        Ok(out)
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
        let mut text =
            serde_json::to_string_pretty(&Value::Object(obj)).map_err(AuthError::Parse)?;
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

    /// Whether any form of auth is configured for a provider WITHOUT refreshing tokens (Pi
    /// `AuthStorage.hasAuth`, auth-storage.ts:344-349): runtime override, a stored credential, or a
    /// known env var. `env` is an optional scoped override map for the env tier.
    pub fn has_auth(&self, provider: &ProviderId, env: Option<&HashMap<String, String>>) -> bool {
        if self.runtime_api_key(provider).is_some() {
            return true;
        }
        if matches!(self.read_file(), Ok(map) if map.contains_key(provider.as_str())) {
            return true;
        }
        crate::env_keys::get_env_api_key(provider.as_str(), env).is_some()
    }

    /// Report auth status without exposing values or refreshing (Pi `getAuthStatus`,
    /// auth-storage.ts:354-369). Precedence: stored → runtime(`--api-key`) → environment.
    pub fn get_auth_status(
        &self,
        provider: &ProviderId,
        env: Option<&HashMap<String, String>>,
    ) -> AuthStatus {
        if matches!(self.read_file(), Ok(map) if map.contains_key(provider.as_str())) {
            return AuthStatus {
                configured: true,
                source: Some(AuthSource::Stored),
                label: None,
            };
        }
        if self.runtime_api_key(provider).is_some() {
            return AuthStatus {
                configured: false,
                source: Some(AuthSource::Runtime),
                label: Some("--api-key".to_string()),
            };
        }
        if let Some(keys) = crate::env_keys::find_env_keys(provider.as_str(), env)
            && let Some(first) = keys.into_iter().next()
        {
            return AuthStatus {
                configured: false,
                source: Some(AuthSource::Environment),
                label: Some(first),
            };
        }
        AuthStatus {
            configured: false,
            source: None,
            label: None,
        }
    }

    /// Resolve a usable API key for a provider (Pi `AuthStorage.getApiKey`, auth-storage.ts:462-520):
    /// runtime override → stored api_key (via `resolveConfigValue`) → OAuth access token (if not
    /// expired) → env fallback (unless `include_fallback` is false).
    ///
    /// NOTE: a stored OAuth credential whose access token has **expired** requires a network refresh
    /// against the provider's token endpoint. That refresh (Pi `getOAuthProvider`/`getOAuthApiKey`)
    /// is intentionally out of `cyrup-config` (DI-10: no network I/O); until the OAuth provider
    /// registry lands, an expired token resolves to `None` so model discovery skips the provider —
    /// matching Pi's refresh-failure end state (auth-storage.ts:503-505).
    pub async fn get_api_key(
        &self,
        provider: &ProviderId,
        include_fallback: bool,
        env: Option<&HashMap<String, String>>,
    ) -> Result<Option<String>, AuthError> {
        // 1. Runtime override (`--api-key`) wins.
        if let Some(k) = self.runtime_api_key(provider) {
            return Ok(Some(k));
        }

        match self.read(provider).await? {
            Some(Credential::ApiKey {
                key: Some(raw),
                env: cred_env,
            }) => {
                let scoped: Option<HashMap<String, String>> =
                    cred_env.map(|m| m.into_iter().collect());
                return Ok(crate::config_value::resolve_config_value(
                    &raw,
                    scoped.as_ref(),
                ));
            }
            Some(Credential::ApiKey { key: None, .. }) => { /* fall through to env */ }
            Some(Credential::Oauth {
                access, expires, ..
            }) => {
                let now = unix_millis();
                if now < expires {
                    return Ok(Some(access));
                }
                // Expired: refresh lives outside this crate (see method note).
                return Ok(None);
            }
            None => {}
        }

        if !include_fallback {
            return Ok(None);
        }
        Ok(crate::env_keys::get_env_api_key(provider.as_str(), env))
    }
}

/// Current unix time in milliseconds (OAuth `expires` is an epoch-ms timestamp, like `Date.now()`).
fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Where a resolved/queried credential came from (Pi `AuthStatus.source`, auth-storage.ts:38-42).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    Stored,
    Runtime,
    Environment,
    Fallback,
    ModelsJsonKey,
    ModelsJsonCommand,
}

/// Non-secret auth status for a provider (Pi `AuthStatus`, auth-storage.ts:38-42). `configured` is
/// true only for a persisted (`stored`) credential; `label` carries the human-readable source hint
/// (e.g. the env-var name or `--api-key`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthStatus {
    pub configured: bool,
    pub source: Option<AuthSource>,
    pub label: Option<String>,
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
        return Ok(Some(ResolvedAuth {
            key: k.to_string(),
            source: CredentialSource::Explicit,
        }));
    }
    // 2. stored credential — suppresses env.
    match stored {
        Stored::ApiKey(k) => {
            return Ok(Some(ResolvedAuth {
                key: k.clone(),
                source: CredentialSource::Stored,
            }));
        }
        Stored::OAuth(t) => {
            return Ok(Some(ResolvedAuth {
                key: t.clone(),
                source: CredentialSource::Stored,
            }));
        }
        Stored::OAuthRefreshFailed => {
            // Stored credential present but refresh failed: no env fallback (A-07-5).
            return Err(AuthError::Oauth(
                "refresh failed; refusing env fallback".to_string(),
            ));
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
        return Ok(Some(ResolvedAuth {
            key: k.to_string(),
            source: CredentialSource::Env,
        }));
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

    /// The returned `TempDir` guard owns the directory's lifetime — callers MUST bind it
    /// (`let (s, _p, _dir) = store();`) or the tree is deleted before the test runs.
    fn store() -> (AuthStore, PathBuf, crate::test_util::TempDir) {
        let dir = crate::test_util::temp_dir();
        let path = dir.join("agent").join("auth.json");
        (AuthStore::at(path.clone()), path, dir)
    }

    #[tokio::test]
    async fn stored_over_env_then_delete_falls_back() {
        // A-07-5: stored credential used over env; delete → env.
        let (s, _p, _dir) = store();
        let provider = ProviderId::from("anthropic");
        s.modify(&provider, |_| async {
            Ok(Some(Credential::api_key("stored-key")))
        })
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
        let r = resolve_auth(None, &Stored::None, None, None, Some("cfg"))
            .unwrap()
            .unwrap();
        assert_eq!(r.source, CredentialSource::Configured);
        assert!(
            resolve_auth(None, &Stored::None, None, None, None)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn oauth_refresh_failure_preserves_credential() {
        // R-07-017: a failure in `f` does not write; the stored credential is preserved.
        let (s, _p, _dir) = store();
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
        let (s, _p, _dir) = store();
        let a = ProviderId::from("a");
        let b = ProviderId::from("b");
        s.modify(&a, |_| async { Ok(Some(Credential::api_key("ka"))) })
            .await
            .unwrap();
        s.modify(&b, |_| async { Ok(Some(Credential::api_key("kb"))) })
            .await
            .unwrap();
        assert_eq!(
            s.read(&a).await.unwrap().unwrap().stored_api_key(),
            Some("ka")
        );
        assert_eq!(
            s.read(&b).await.unwrap().unwrap().stored_api_key(),
            Some("kb")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owner_only_permissions() {
        // A-07-9 / R-07-014
        use std::os::unix::fs::PermissionsExt;
        let (s, path, _dir) = store();
        let provider = ProviderId::from("x");
        s.modify(&provider, |_| async { Ok(Some(Credential::api_key("k"))) })
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn malformed_auth_file_surfaces_parse_error() {
        let r = parse_auth("{ not json");
        assert!(matches!(r, Err(AuthError::Parse(_))));
    }

    #[tokio::test]
    async fn get_api_key_resolves_template_and_runtime_and_env() {
        let (s, _p, _dir) = store();
        let provider = ProviderId::from("acme-test-provider");
        // stored value is a `$VAR` template resolved via the credential's scoped env map.
        s.modify(&provider, |_| async {
            Ok(Some(Credential::ApiKey {
                key: Some("$ACME_KEY".to_string()),
                env: Some([("ACME_KEY".to_string(), "resolved-secret".to_string())].into()),
            }))
        })
        .await
        .unwrap();
        let key = s.get_api_key(&provider, true, None).await.unwrap();
        assert_eq!(key.as_deref(), Some("resolved-secret"));

        // runtime override wins over stored.
        s.set_runtime_api_key(provider.clone(), "cli-key".to_string());
        assert_eq!(
            s.get_api_key(&provider, true, None)
                .await
                .unwrap()
                .as_deref(),
            Some("cli-key")
        );
        s.remove_runtime_api_key(&provider);

        // no stored credential → env fallback via the provider→env map (openai).
        let openai = ProviderId::from("openai");
        let env: HashMap<String, String> =
            [("OPENAI_API_KEY".to_string(), "sk-env".to_string())].into();
        assert_eq!(
            s.get_api_key(&openai, true, Some(&env))
                .await
                .unwrap()
                .as_deref(),
            Some("sk-env")
        );
        // include_fallback = false suppresses env.
        assert_eq!(
            s.get_api_key(&openai, false, Some(&env)).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn get_api_key_oauth_expiry() {
        let (s, _p, _dir) = store();
        let provider = ProviderId::from("oauth-prov");
        // valid (future) access token is returned.
        let future = unix_millis() + 600_000;
        s.modify(&provider, |_| async move {
            Ok(Some(Credential::Oauth {
                refresh: "r".into(),
                access: "fresh-token".into(),
                expires: future,
                ext: Map::new(),
            }))
        })
        .await
        .unwrap();
        assert_eq!(
            s.get_api_key(&provider, true, None)
                .await
                .unwrap()
                .as_deref(),
            Some("fresh-token")
        );
        // expired token → None (refresh lives outside the crate; no env fallback for oauth).
        s.modify(&provider, |_| async {
            Ok(Some(Credential::Oauth {
                refresh: "r".into(),
                access: "stale".into(),
                expires: 1,
                ext: Map::new(),
            }))
        })
        .await
        .unwrap();
        assert_eq!(s.get_api_key(&provider, true, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn auth_status_sources() {
        let (s, _p, _dir) = store();
        let openai = ProviderId::from("openai");
        // environment source with label.
        let env: HashMap<String, String> =
            [("OPENAI_API_KEY".to_string(), "sk-env".to_string())].into();
        let st = s.get_auth_status(&openai, Some(&env));
        assert!(!st.configured);
        assert_eq!(st.source, Some(AuthSource::Environment));
        assert_eq!(st.label.as_deref(), Some("OPENAI_API_KEY"));
        // stored source.
        s.modify(&openai, |_| async { Ok(Some(Credential::api_key("k"))) })
            .await
            .unwrap();
        let st = s.get_auth_status(&openai, Some(&env));
        assert!(st.configured);
        assert_eq!(st.source, Some(AuthSource::Stored));
        assert!(s.has_auth(&openai, Some(&env)));
    }

    #[tokio::test]
    async fn get_provider_env_returns_scoped_env_for_api_key_only() {
        // Pi AuthStorage.getProviderEnv (auth-storage.ts:305-308).
        let (s, _p, _dir) = store();
        let prov = ProviderId::from("acme");
        // api_key with env → Some(copy).
        s.modify(&prov, |_| async {
            Ok(Some(Credential::ApiKey {
                key: Some("$ACME_KEY".to_string()),
                env: Some([("ACME_KEY".to_string(), "secret".to_string())].into()),
            }))
        })
        .await
        .unwrap();
        let env = s.get_provider_env(&prov).await.unwrap().unwrap();
        assert_eq!(env.get("ACME_KEY").map(String::as_str), Some("secret"));

        // api_key without env → None.
        let bare = ProviderId::from("bare");
        s.modify(&bare, |_| async { Ok(Some(Credential::api_key("k"))) })
            .await
            .unwrap();
        assert_eq!(s.get_provider_env(&bare).await.unwrap(), None);

        // OAuth credential → None.
        let oauth = ProviderId::from("oauthp");
        s.modify(&oauth, |_| async {
            Ok(Some(Credential::Oauth {
                refresh: "r".into(),
                access: "a".into(),
                expires: 0,
                ext: Map::new(),
            }))
        })
        .await
        .unwrap();
        assert_eq!(s.get_provider_env(&oauth).await.unwrap(), None);

        // missing provider → None.
        assert_eq!(
            s.get_provider_env(&ProviderId::from("nope")).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn list_enumerates_providers_with_credentials() {
        // Pi AuthStorage.list (auth-storage.ts:329-331).
        let (s, _p, _dir) = store();
        assert!(s.list().unwrap().is_empty());
        s.modify(&ProviderId::from("openai"), |_| async {
            Ok(Some(Credential::api_key("a")))
        })
        .await
        .unwrap();
        s.modify(&ProviderId::from("anthropic"), |_| async {
            Ok(Some(Credential::api_key("b")))
        })
        .await
        .unwrap();
        // BTreeMap → sorted.
        assert_eq!(
            s.list().unwrap(),
            vec!["anthropic".to_string(), "openai".to_string()]
        );
        // deleting a provider drops it from the list.
        s.delete(&ProviderId::from("openai")).await.unwrap();
        assert_eq!(s.list().unwrap(), vec!["anthropic".to_string()]);
    }

    #[test]
    fn pkce_challenge_is_stable() {
        // RFC 7636 appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce::challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
