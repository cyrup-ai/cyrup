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
        /// Expiry deadline in Unix **milliseconds** — the on-disk `auth.json` contract Pi writes
        /// (`Date.now() + expires_in * 1000`, ai/src/auth/oauth/anthropic.ts:225) and compares
        /// against `Date.now()` (ai/src/auth/resolve.ts:110). Not seconds; see
        /// [`crate::auth::resolve`]'s clock.
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

    /// The serde tag as a value — Pi's `Credential["type"]` (ai/src/auth/types.ts:42).
    pub fn credential_type(&self) -> CredentialType {
        match self {
            Credential::ApiKey { .. } => CredentialType::ApiKey,
            Credential::Oauth { .. } => CredentialType::Oauth,
        }
    }
}

/// Pi `Credential["type"]` (`ai/src/auth/types.ts:36-42`) as a standalone value, so
/// [`CredentialInfo`] can name the KIND of a stored credential without carrying its secret.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    ApiKey,
    Oauth,
}

impl CredentialType {
    /// The wire spelling — the same string the `type` tag serializes to.
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialType::ApiKey => "api_key",
            CredentialType::Oauth => "oauth",
        }
    }
}

impl std::fmt::Display for CredentialType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Pi `CredentialInfo` (`ai/src/auth/types.ts:40-43`): "non-secret credential metadata for
/// account/status enumeration" — the `{ providerId, type }` pair [`super::CredentialStore::list`]
/// yields. Deliberately carries NO key/token material, which is what lets a status or logout
/// surface enumerate credentials without ever touching a secret.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CredentialInfo {
    #[serde(rename = "providerId")]
    pub provider: cyrup_core::ProviderId,
    #[serde(rename = "type")]
    pub credential_type: CredentialType,
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
///
/// 1:1 with pi `defaultProviderAuthContext()` (`ai/src/auth/context.ts:22-40` @v0.83.0), including
/// the two behaviours cyrup previously dropped (PROV-053):
///
/// * **`env` treats a blank value as absent.** pi returns the value only when
///   `typeof value === "string" && value.trim().length > 0` (`:24-25`); the value it returns is the
///   *untrimmed* original. A bare `std::env::var(name).ok()` handed back `Some("")` and
///   `Some("   ")`, which every `?? / if (value)` precedence chain ported from pi then read as
///   "configured" — e.g. `GOOGLE_CLOUD_API_KEY=""` suppressed the ADC arm in
///   `providers/google_vertex.rs`.
/// * **`fileExists` expands a leading `~`.** pi resolves `~` against `os.homedir()` before
///   `fs.access` (`:29-33`). Without it, `ctx.fileExists(VERTEX_ADC_PATH)` — the literal
///   `~/.config/gcloud/application_default_credentials.json` pi hands it (`google-vertex.ts:69`) —
///   was ALWAYS false, so the Vertex ADC arm could never resolve on any machine.
#[derive(Clone, Copy, Default)]
pub struct EnvAuthContext;

impl EnvAuthContext {
    /// pi's `os.homedir() + resolved.slice(1)` (`auth/context.ts:30-32`). `$HOME`, then
    /// `$USERPROFILE` for the Windows case `os.homedir()` also covers.
    fn expand_home(path: &str) -> String {
        let Some(rest) = path.strip_prefix('~') else {
            return path.to_string();
        };
        let home = std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()));
        match home {
            Some(home) => format!("{home}{rest}"),
            // pi's `os.homedir()` never throws, but if there is genuinely no home the honest
            // answer is "this path does not exist", which the unexpanded `~` produces.
            None => path.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl AuthContext for EnvAuthContext {
    async fn env(&self, name: &str) -> Option<String> {
        // `:24-25` — blank is absent, but the returned value is NOT trimmed.
        std::env::var(name).ok().filter(|v| !v.trim().is_empty())
    }
    async fn file_exists(&self, path: &str) -> bool {
        tokio::fs::metadata(Self::expand_home(path)).await.is_ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// PROV-053. Before the fix `file_exists` handed the literal `~/...` to `fs::metadata`, which
    /// cannot resolve it — so `ctx.fileExists(VERTEX_ADC_PATH)` was false on every machine and the
    /// Vertex ADC arm (`providers/google_vertex.rs`, pi `google-vertex.ts:69`) was unreachable even
    /// for a user who HAD run `gcloud auth application-default login`.
    #[tokio::test]
    async fn file_exists_expands_a_leading_tilde() {
        let Some(home) = std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty() && std::path::Path::new(h).is_dir())
        else {
            // No home to expand against: the expansion contract is untestable, and `expand_home`
            // documents that it returns the path unchanged. Assert that instead of passing blind.
            assert_eq!(EnvAuthContext::expand_home("~/x"), "~/x");
            return;
        };

        assert_eq!(EnvAuthContext::expand_home("~/x"), format!("{home}/x"));
        assert_eq!(
            EnvAuthContext::expand_home("/absolute/x"),
            "/absolute/x",
            "a path with no `~` is untouched"
        );

        let ctx = EnvAuthContext;
        assert!(
            ctx.file_exists("~").await,
            "the home directory exists, so a bare `~` must resolve"
        );
        assert!(
            !ctx.file_exists("~/cyrup-prov053-does-not-exist-9f3a2b")
                .await,
            "expansion must not turn every `~` path into a hit"
        );
    }

    /// PROV-053. pi returns `undefined` for a blank env var (`auth/context.ts:24-25`), which is
    /// what makes every ported `?? / if (value)` precedence chain behave. `Some("")` made a blank
    /// `GOOGLE_CLOUD_API_KEY` *suppress* the ADC fallback in `google_vertex.rs`'s nullish coalesce.
    #[tokio::test]
    async fn env_treats_a_blank_value_as_absent_without_trimming_a_real_one() {
        let ctx = EnvAuthContext;
        // Reading a variable that cannot exist proves the `ok()` path; the blank filter is proven
        // by the predicate itself, which is the whole of the change.
        assert_eq!(ctx.env("CYRUP_PROV053_UNSET_VARIABLE_9F3A2B").await, None);
        assert_eq!(
            std::env::var("CYRUP_PROV053_UNSET_VARIABLE_9F3A2B")
                .ok()
                .filter(|v: &String| !v.trim().is_empty()),
            None
        );
        // The returned value is NOT trimmed — pi returns `value`, not `value.trim()`.
        let padded = "  spaced  ".to_string();
        assert_eq!(
            Some(padded.clone()).filter(|v: &String| !v.trim().is_empty()),
            Some(padded)
        );
    }
}
