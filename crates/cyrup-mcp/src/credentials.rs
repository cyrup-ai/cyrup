//! MCP OAuth credential storage — `mcp-auth.ts` (1000 lines) and `mcp-keyring-helper.cjs`
//! (89 lines) at `pi-mcp-adapter` v2.25.0, plus `utils.ts`'s bearer-token resolution.
//! Gap-analysis `13f-mcp-credentials.md` (MCP-250…MCP-291) and `13b` MCP-084.
//!
//! # The one fact that shapes everything below
//!
//! **The OS keychain is the only store, and there is no plaintext fallback.** Upstream throws
//! `OAuth secure credential storage is unavailable. Configure the OS credential store and retry
//! authentication.` rather than degrading, and the `mcp-oauth/sha256-<hash>/tokens.json` path that
//! still appears throughout `mcp-auth.ts` is **import-only** — read once, written into the keychain,
//! then the file *and its directory* are removed. Any port that reintroduces a plaintext write is a
//! security regression against upstream (MCP-269, MCP-281).
//!
//! This is why [`crate::credentials`] does **not** reuse `cyrup_config::auth::AuthStore`: that store
//! is a plaintext-JSON `auth.json` at mode 0600, keyed by `ProviderId`, with no `serverUrl` slot, no
//! PKCE slot, no `configPreRegistered` flag and a *millisecond* `expires` field. Folding MCP
//! credentials into it would be a plaintext downgrade **and** a schema mismatch. What *is* reused is
//! its **shape**: `cyrup_provider::auth::store::CredentialStore::modify`'s "THE ONLY write path,
//! serialized read-modify-write per provider id" becomes [`McpAuthStore`]'s per-server-name
//! `tokio::sync::Mutex` (MCP-268) — the `FileLock` half is deliberately *not* copied, because the
//! store is a keychain and there is no file to lock.
//!
//! # Three layers, and collapsing them ships a bug
//!
//! 1. **Payload** ([`chunk_boundaries`], [`AuthEntryChunkManifest`]) — one JSON record split across
//!    N keychain accounts because Windows Credential Manager caps a value at
//!    `CRED_MAX_CREDENTIAL_BLOB_SIZE` = 2560 bytes stored as UTF-16, i.e. 1280 characters.
//! 2. **Backend** ([`AuthSecretStore`]) — the real keychain, the four fault-injection stores, and
//!    the `keyctl session -` recovery store.
//! 3. **Process-lifetime cache** ([`AuthStoreInner::cache`]) keyed by server name alone, whose
//!    invalidation points live in another section (`server-manager.ts`, `session-recovery.ts`).
//!
//! Get the layering wrong and a keychain write silently succeeds while reads keep serving a stale
//! token — the bug class that makes a 401 loop unkillable.
//!
//! # What rmcp owns, and what is left here
//!
//! `rmcp::transport::auth` owns the OAuth *protocol* — PRM/AS discovery, DCR, PKCE S256, exchange,
//! refresh, scope upgrade — and reaches persistence through exactly two object-safe traits,
//! [`rmcp::transport::auth::CredentialStore`] (`load`/`save`/`clear`, **no key**) and
//! [`rmcp::transport::auth::StateStore`] (keyed by CSRF token). This module implements both over
//! the keychain ([`McpCredentialStore`], [`McpStateStore`], MCP-291) and keeps everything upstream
//! wrapped around the same keychain that rmcp has no opinion about: account naming, the chunking
//! manifest, the read cache, the legacy plaintext import, the URL binding, and the Linux
//! revoked-keyring recovery hop.
//!
//! Three of upstream's five `AuthEntry` slots are now rmcp types, so `isTokenExpired`, the
//! MCP-SDK token conversion and the `codeVerifier`/`oauthState` accessors dissolve (MCP-267,
//! MCP-271, MCP-288). Absolute fractional `expiresAt` survives in exactly **one** place: the
//! legacy-import converter, [`legacy_credentials`].
//!
//! # Why `oauth2` is not a direct dependency of this crate
//!
//! [`rmcp::transport::auth::StoredCredentials::new`] takes an
//! [`rmcp::transport::auth::OAuthTokenResponse`] = `oauth2::StandardTokenResponse<…>`, and
//! `StoredAuthorizationState` is `#[non_exhaustive]` with `oauth2`-typed constructors — but both are
//! `Serialize + Deserialize`, and `StandardTokenResponse`'s serde shape **is** the RFC 6749 §5.1
//! wire shape. So the two constructors below ([`token_response_from_parts`],
//! [`authorization_state_from_parts`]) build them through `serde_json`, which needs no `oauth2`
//! line in `Cargo.toml` and cannot drift from the on-wire format. This is the legacy-import and
//! test path only; live tokens arrive already typed from `AuthorizationManager`.
//!
//! # Deferred, and exactly what is missing
//!
//! * `MCP-260` — **landed.** The hidden subcommand's *host* half is
//!   `crates/cyrup/src/mcp_keyring_helper_cmd.rs`: it exposes the
//!   `SUBCOMMAND` / `is_selected(argv)` / `dispatch()` triple beside `intercom_broker_cmd.rs`,
//!   `cyrup::predispatch::Internal::McpKeyringHelper` classifies the argv, and `cyrup`'s `main()`
//!   dispatches it above the bootstrap HTTP-proxy install and above the package/config and
//!   credential-print gates, so the helper answers before anything can initialise the cache, the
//!   config or tracing. The default re-exec target — `current_exe()` +
//!   [`KEYRING_HELPER_SUBCOMMAND`] — therefore resolves to a program that understands the token;
//!   before it landed, the recovery hop worked only through `…_KEYRING_RECOVERY_HELPER`, and the
//!   default arm re-exec'd the agent with an argument it did not recognize.
//! * `TODO(MCP-278)` / `TODO(MCP-283)` — the two acceptance suites. The in-process cases are ported
//!   below; the **two subprocess cases** (the fixture `keyctl` asserting `$1 == "session"`,
//!   `$2 == "-"`, exiting 64 otherwise, then `shift 2; exec "$@"`; and its negative twin where a
//!   generic failure must not spawn anything) need a `crates/cyrup-it` target, which sets
//!   `autotests = false` and gates every `[[test]]` behind `required-features = ["it"]`, so the MCP
//!   target must be declared by hand.
//! * `TODO(MCP-287)` — the three timeout/exit-code fixtures (a helper that sleeps 30 s ⇒ the rung-1
//!   message within ~10 s and no zombie; one printing `{"ok":false,…}` and exiting 1 ⇒ rung 2; the
//!   same exiting 0 ⇒ rung 5) belong to that same integration target. The six rungs themselves are
//!   implemented and ordered.
//! * `TODO(MCP-269)` — the repo-level assertion that no MCP credential material ever reaches
//!   `cyrup_config::env`'s auth path. The *design* satisfies it (nothing here touches `AuthStore`);
//!   the standing guard against a future regression does not exist yet.
//!
//! # No secret ever reaches a log, a `Debug` render or an error string (MCP-277)
//!
//! [`AuthEntry`] and [`StoredClientInfo`] have **hand-written** `Debug` impls; there is no
//! `#[derive(Debug)]` on anything that carries a secret. rmcp already redacts `token_response`,
//! `pkce_verifier` and `csrf_token`, so the composed record inherits redaction for three of its four
//! slots and only `StoredClientInfo::client_secret` needed new work. Error variants carry a server
//! name, an account name, a chunk account and a source label — never a payload. The pattern **not**
//! to copy is in tree: `cyrup_provider::auth::types::Credential` derives `Debug` over its `refresh`
//! and `access` fields, so a `{:?}` prints tokens.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use regex::Regex;
use rmcp::transport::auth::{
    AuthError, OAuthTokenResponse, StoredAuthorizationState, StoredCredentials,
};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::dirs::McpDirs;
use crate::errors::{McpError, McpResult};

// ---------------------------------------------------------------------------------------------
// §6.1 Constants — reproduced literally (MCP-282 for the env namespace)
// ---------------------------------------------------------------------------------------------

/// The keychain **service**: the durable identity of every stored credential, user-visible in
/// Keychain Access, seahorse and `keyctl show`.
///
/// Upstream is `pi-mcp-adapter.oauth`; the rename is **forced** (MCP-280). The port's payload is not
/// wire-compatible with upstream's — `StoredTokens{accessToken,…}` versus
/// `StoredCredentials{client_id, token_response{access_token,…}}` — so writing the new shape under
/// the old service would put an unreadable payload on the exact account a live `pi-mcp-adapter`
/// install reads, and its `parseAuthEntryPayload` would reject it, destroying that install's
/// credentials.
pub const AUTH_SECRET_SERVICE: &str = "cyrup.mcp.oauth";

/// `mcp-auth.ts`'s `AUTH_SECRET_SERVICE`. Read **once per server, read-only**, and never written or
/// deleted (MCP-280): a keychain entry is not a plaintext leak, and removing it would break a
/// co-installed `pi-mcp-adapter` for no security benefit. That asymmetry against the legacy *file*
/// importer — which deletes, mandatorily — is the thing to get right.
pub const LEGACY_AUTH_SECRET_SERVICE: &str = "pi-mcp-adapter.oauth";

/// `AUTH_SECRET_CHUNK_SIZE` — both the chunk width **and** the chunking threshold. A threshold
/// *above* [`AUTH_SECRET_VALUE_LIMIT`] is a pinned regression: oversized records would still fail to
/// persist on Windows.
pub const AUTH_SECRET_CHUNK_SIZE: usize = 1000;

/// `AUTH_SECRET_VALUE_LIMIT` — `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2560 bytes) ÷ 2, because Windows
/// Credential Manager stores the blob as UTF-16.
pub const AUTH_SECRET_VALUE_LIMIT: usize = 1280;

/// `AUTH_CHUNK_MANIFEST_KEY` — the discriminator that makes a payload a manifest rather than an
/// entry. The literal is kept verbatim: it is a **stored-format token**, not branding, and renaming
/// it would orphan every chunked credential written by an earlier build.
pub const AUTH_CHUNK_MANIFEST_KEY: &str = "__piMcpAdapterOAuthChunked";

/// Upper bound on a manifest's `chunkCount` (**cyrup addition**, MCP-286).
///
/// Upstream requires only "integer > 0" and then materialises exactly that many account strings, so
/// a corrupt or hostile base payload claiming `chunkCount: 1e9` drives an unbounded loop of keyring
/// reads. 64 covers a 64 KB credential with headroom; anything larger is treated as "not a manifest",
/// which is the degradation upstream's own validator already applies to every other malformed field.
/// The threat model is "an attacker who can already write your keychain", so this is belt-and-braces.
pub const AUTH_CHUNK_COUNT_LIMIT: usize = 64;

/// `KEYRING_RECOVERY_TIMEOUT_MS` — the wall-clock cap on the `keyctl` subprocess.
pub const KEYRING_RECOVERY_TIMEOUT_MS: u64 = 10_000;

/// The helper's stdin cap and the parent's `maxBuffer`, 1 MiB. Both sides cap **independently**.
pub const KEYRING_HELPER_MAX_BYTES: usize = 1024 * 1024;

/// The hidden subcommand the `keyctl` hop re-execs `current_exe()` under (MCP-260).
///
/// `crates/cyrup/src/mcp_keyring_helper_cmd.rs` owns the `is_selected(argv)` / `dispatch()` pair
/// beside `intercom_broker_cmd.rs` and `subagent_runner_cmd.rs`; its `dispatch` is a call to
/// [`run_keyring_helper`] and nothing else.
pub const KEYRING_HELPER_SUBCOMMAND: &str = "__mcp-keyring-helper";

/// `PI_MCP_ADAPTER_TEST_AUTH_STORE` — the backend override, matched by **exact** string equality
/// against `memory` | `sizelimited` | `unavailable` | `keyrevoked`.
pub const TEST_AUTH_STORE_ENV: [&str; 2] =
    ["CYRUP_MCP_TEST_AUTH_STORE", "PI_MCP_ADAPTER_TEST_AUTH_STORE"];

/// `PI_MCP_ADAPTER_DISABLE_AUTH_CACHE` — `== "1"` disables. Any other value (`"true"`, `"0"`, empty)
/// leaves the cache **enabled** (MCP-259).
pub const AUTH_CACHE_DISABLED_ENV: [&str; 2] = [
    "CYRUP_MCP_DISABLE_AUTH_CACHE",
    "PI_MCP_ADAPTER_DISABLE_AUTH_CACHE",
];

/// `PI_MCP_ADAPTER_DISABLE_KEYRING_RECOVERY` — `== "1"` disables recovery entirely.
pub const KEYRING_RECOVERY_DISABLED_ENV: [&str; 2] = [
    "CYRUP_MCP_DISABLE_KEYRING_RECOVERY",
    "PI_MCP_ADAPTER_DISABLE_KEYRING_RECOVERY",
];

/// `PI_MCP_ADAPTER_KEYRING_RECOVERY_KEYCTL` — overrides the `keyctl` program path (trimmed; blank ⇒
/// `"keyctl"`).
pub const KEYRING_RECOVERY_KEYCTL_ENV: [&str; 2] = [
    "CYRUP_MCP_KEYRING_RECOVERY_KEYCTL",
    "PI_MCP_ADAPTER_KEYRING_RECOVERY_KEYCTL",
];

/// `PI_MCP_ADAPTER_KEYRING_RECOVERY_HELPER` — overrides the helper. Upstream's default resolves
/// `./mcp-keyring-helper.cjs` against `import.meta.url`; the port's default is
/// `std::env::current_exe()`, so this now names a **program** rather than a script.
///
/// `PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE` deliberately **does not port**: it names a JavaScript
/// interpreter and there is none (13f *Out of scope*).
pub const KEYRING_RECOVERY_HELPER_ENV: [&str; 2] = [
    "CYRUP_MCP_KEYRING_RECOVERY_HELPER",
    "PI_MCP_ADAPTER_KEYRING_RECOVERY_HELPER",
];

/// `PI_MCP_ADAPTER_TEST_LINUX_KEYRING_RECOVERY` — `== "1"` forces the recovery path on non-Linux.
pub const TEST_LINUX_KEYRING_RECOVERY_ENV: [&str; 2] = [
    "CYRUP_MCP_TEST_LINUX_KEYRING_RECOVERY",
    "PI_MCP_ADAPTER_TEST_LINUX_KEYRING_RECOVERY",
];

/// The `source` label every store-side parse failure is reported against.
const STORE_SOURCE: &str = "OS secure credential store";
/// The `source` label a reassembled chunked payload is reported against.
const STORE_CHUNKS_SOURCE: &str = "OS secure credential store chunks";

/// An environment reader. Injected rather than read directly because edition 2024 makes
/// `std::env::set_var` `unsafe`, so a test that pinned a variable could not undo it — the same
/// convention [`crate::dirs::resolve_auth_base_dir`] already uses (MCP-068).
pub type EnvFn = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The production reader: `std::env::var(key).ok()`.
#[must_use]
pub fn process_env() -> EnvFn {
    Arc::new(|key: &str| std::env::var(key).ok())
}

/// Dual-read `CYRUP_MCP_<SUFFIX>` then `PI_MCP_ADAPTER_<SUFFIX>` (MCP-282) — the convention
/// `cyrup_config::env` already uses for `["CYRUP_AGENT_DIR", "PI_CODING_AGENT_DIR"]` and
/// `cyrup_provider::auth::oauth::callback` for `["CYRUP_OAUTH_CALLBACK_HOST", "PI_OAUTH_CALLBACK_HOST"]`.
/// `CYRUP_*` wins.
fn env_first(env: &EnvFn, names: &[&str; 2]) -> Option<String> {
    names.iter().find_map(|name| env(name))
}

/// `process.env[X] === '1'` — strict, so `"true"` and `"0"` do **not** trip it.
fn env_is_one(env: &EnvFn, names: &[&str; 2]) -> bool {
    env_first(env, names).as_deref() == Some("1")
}

/// Seconds since the Unix epoch. A clock before the epoch yields `0` rather than panicking.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------------------------------
// MCP-254 — the error taxonomy
// ---------------------------------------------------------------------------------------------

/// Which store operation failed. Upstream's `OAuthCredentialStoreError.operation` discriminant,
/// which `commands.ts` and the section-07 refresh driver both switch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOp {
    /// `store.read(account)`.
    Read,
    /// `store.write(account, payload)`.
    Write,
    /// `store.remove(account)`.
    Remove,
}

impl StoreOp {
    /// The verb as it appears in the message: `Failed to **read** OAuth credentials …`.
    const fn verb(self) -> &'static str {
        match self {
            StoreOp::Read => "read",
            StoreOp::Write => "write",
            StoreOp::Remove => "remove",
        }
    }

    /// The preposition: upstream writes `… from the OS secure credential store` for read and remove
    /// and `… to …` for write. Byte-exactness matters — `commands-panel-auth-storage.test.ts`
    /// matches on the sentence.
    const fn preposition(self) -> &'static str {
        match self {
            StoreOp::Write => "to",
            _ => "from",
        }
    }
}

impl fmt::Display for StoreOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.verb())
    }
}

/// What one backend call failed with — the innermost link of the chain MCP-262's predicate walks.
///
/// Kept separate from [`AuthStoreError`] so `keyring_core::Error`'s own `source()` (the platform
/// error carried by `PlatformFailure` / `NoStorageAccess` / `BadDataFormat`) stays reachable: the
/// revoked-keyring detection is a walk over that chain, not a match on the outermost message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthSecretStoreError {
    /// The credential store itself refused the operation.
    #[error(transparent)]
    Keyring(#[from] keyring::Error),

    /// `getKeyringEntry`'s catch: the store could not be constructed at all — a locked keychain, no
    /// D-Bus session, no default store. MCP-252's sentence, and the only place it is produced.
    #[error(
        "OAuth secure credential storage is unavailable. Configure the OS credential store and retry authentication."
    )]
    StoreUnavailable {
        /// The `keyring_core::Error` that `Entry::new` returned.
        #[source]
        source: keyring::Error,
    },

    /// The `keyctl session -` recovery hop failed. Carries one of MCP-287's six rung messages
    /// verbatim.
    #[error("{0}")]
    Recovery(String),

    /// A valid manifest named a chunk account that held nothing. Carries
    /// [`AuthStoreError::MissingChunk`]'s rendered message so the wrapping
    /// [`AuthStoreError::Unavailable`] keeps it in its `source()` chain, exactly as upstream nests
    /// the bare `Error` inside `OAuthCredentialStoreError`'s `cause`.
    #[error("{0}")]
    MissingChunk(String),
}

/// Every failure this module produces.
///
/// The class is contract, not decoration: `mcp-auth-flow.ts`'s refresh driver **rethrows**
/// [`AuthStoreError::Unavailable`] while swallowing every other refresh error into `null`, so a
/// broken keychain must stay distinguishable from an ordinary auth failure or it becomes an infinite
/// silent re-auth loop. At the rmcp boundary the same distinction is
/// [`AuthError::InternalError`] versus [`AuthError::AuthorizationRequired`] — see
/// [`store_error_to_auth_error`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuthStoreError {
    /// `OAuthCredentialStoreError` — the store was unreachable for `operation`.
    #[error(
        "Failed to {operation} OAuth credentials for {server} {} the OS secure credential store",
        operation.preposition()
    )]
    Unavailable {
        /// Which call failed.
        operation: StoreOp,
        /// The `mcpServers` key, as configured.
        server: String,
        /// The backend failure, whose `source()` chain the recovery predicate walks.
        #[source]
        source: AuthSecretStoreError,
    },

    /// A chunk account named by a valid manifest held nothing. Wrapped in
    /// [`AuthStoreError::Unavailable`] with [`StoreOp::Read`] by the caller, because a partially
    /// lost credential must surface as "unavailable", **never** as "absent" — the latter silently
    /// restarts an OAuth flow the user already completed.
    #[error("Missing OAuth credential chunk {chunk_account} for {server}")]
    MissingChunk {
        /// The `{account}.chunk.{digest}.{index}` account that was empty.
        chunk_account: String,
        /// The `mcpServers` key.
        server: String,
    },

    /// `parseJsonPayload`'s throw — the stored bytes are not JSON.
    #[error("Failed to parse OAuth credentials for {server} from {source_label}")]
    ParseJson {
        /// The `mcpServers` key.
        server: String,
        /// Where the bytes came from: the store, the store's chunks, or a legacy file path.
        source_label: String,
        /// The `serde_json` syntax error. Carries an offset, never a payload byte.
        #[source]
        source: serde_json::Error,
    },

    /// `parseAuthEntryPayload`'s second throw — valid JSON, wrong shape. A wrong-typed *optional*
    /// field poisons the whole entry, a missing *required* field does the same, and a JSON array is
    /// rejected exactly as a scalar is (MCP-250).
    #[error("Failed to parse OAuth credentials for {server} from {source_label}: invalid credential shape")]
    ParseShape {
        /// The `mcpServers` key.
        server: String,
        /// Where the bytes came from.
        source_label: String,
    },

    /// `removeLegacyAuthEntry`'s fatal half. Failing to delete the plaintext *secret* is fatal;
    /// failing to delete its *directory* is swallowed (§6.8).
    #[error("Failed to remove legacy plaintext OAuth credentials for {server} at {path}")]
    LegacyRemove {
        /// The `mcpServers` key.
        server: String,
        /// The `tokens.json` that survived.
        path: PathBuf,
        /// The `std::fs` failure.
        #[source]
        source: std::io::Error,
    },

    /// **No upstream twin.** JavaScript has no join handles; a `tokio::task::spawn_blocking` that
    /// the runtime cancelled or that panicked has to surface as *something*, and it must not be
    /// mistaken for "no credentials".
    #[error("MCP credential store task failed: {0}")]
    Internal(String),
}

impl AuthStoreError {
    /// Is this the class `inspectAuthForUrl` degrades to `unavailable` and the section-07 refresh
    /// driver rethrows? Only [`AuthStoreError::Unavailable`] qualifies — a parse failure on the
    /// stored payload deliberately **propagates** (MCP-284, MCP-265).
    #[must_use]
    pub fn is_store_unavailable(&self) -> bool {
        matches!(self, AuthStoreError::Unavailable { .. })
    }

    /// The `operation` discriminant, when there is one.
    #[must_use]
    pub fn operation(&self) -> Option<StoreOp> {
        match self {
            AuthStoreError::Unavailable { operation, .. } => Some(*operation),
            _ => None,
        }
    }
}

/// MCP-291's mapping, and the reason it is a named function rather than a `From`: **every**
/// `AuthStoreError` becomes [`AuthError::InternalError`], and none of them may become
/// [`AuthError::AuthorizationRequired`]. A store failure that reaches `AuthorizationManager` as
/// "authorization required" restarts an authorization the user already completed, forever.
#[must_use]
pub fn store_error_to_auth_error(error: &AuthStoreError) -> AuthError {
    AuthError::InternalError(error.to_string())
}

// ---------------------------------------------------------------------------------------------
// MCP-250 / MCP-277 / MCP-290 — the stored record
// ---------------------------------------------------------------------------------------------

/// The DCR fields `rmcp`'s [`StoredCredentials`] drops (MCP-290).
///
/// `StoredCredentials` persists **only** `client_id`, and `initialize_from_store` re-applies it with
/// `configure_client_id`. A **confidential** dynamically-registered client must survive a restart
/// with its secret, or the first refresh after a restart sends a `client_id` with no secret, draws
/// `invalid_client`, and wipes the credentials — so the DCR response is persisted here and re-applied
/// through [`Self::to_oauth_client_config`] *after* `initialize_from_store()` returns.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredClientInfo {
    /// `client_id` from the registration response, or from `oauth.clientId` in `mcp.json`.
    pub client_id: String,
    /// `client_secret` — a confidential client's half of the refresh credential. Redacted by
    /// [`fmt::Debug`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// `client_id_issued_at`, fractional Unix seconds as the server sent it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_issued_at: Option<f64>,
    /// `client_secret_expires_at`, fractional Unix seconds; `0` means "never expires" per RFC 7591.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<f64>,
    /// The redirect URIs the client registered. **The one field that degrades silently**: a mixed
    /// array yields `None` rather than rejecting the entry (MCP-250 rule 4).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_lenient_string_array"
    )]
    pub redirect_uris: Option<Vec<String>>,
    /// SEP-2352 authorization-server issuer binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// `true` when this entry is a secretless SEP-2352 issuer **stub** written by the
    /// config-`clientId` path of `saveClientInformation`. Section 07's `clientInformation()` refuses
    /// to return such a stub — serving it would send a refresh with a `client_id` and no secret,
    /// drawing `invalid_client` and wiping credentials. Storage must carry the flag through verbatim
    /// or that enforcement has nothing to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_pre_registered: Option<bool>,
}

impl StoredClientInfo {
    /// A bare client record, the shape `saveClientInformation`'s config-`clientId` path writes when
    /// it also sets [`Self::config_pre_registered`].
    #[must_use]
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: None,
            client_id_issued_at: None,
            client_secret_expires_at: None,
            redirect_uris: None,
            issuer: None,
            config_pre_registered: None,
        }
    }

    /// `clientInformation()`'s stub test (§6.2), the *data* half — section 07 owns the refusal.
    ///
    /// A stub is detected either by the explicit [`Self::config_pre_registered`] marker or by the
    /// legacy shape `{clientId, issuer}` with no `clientSecret` / `clientIdIssuedAt` /
    /// `clientSecretExpiresAt` / `redirectUris`.
    #[must_use]
    pub fn is_pre_registered_stub(&self) -> bool {
        if self.config_pre_registered == Some(true) {
            return true;
        }
        self.client_secret.is_none()
            && self.client_id_issued_at.is_none()
            && self.client_secret_expires_at.is_none()
            && self.redirect_uris.is_none()
    }

    /// MCP-290's re-apply, ready for
    /// [`rmcp::transport::auth::AuthorizationManager::configure_client`]. Section 07 calls it
    /// **after** `initialize_from_store()` returns, because that call ends with
    /// `configure_client_id(&stored.client_id)` and would otherwise leave the secret unset.
    ///
    /// Returns `None` for a pre-registered stub: a stub is only usable paired with the config that
    /// supplies the secret, and handing it back as standalone client information is the
    /// `invalid_client` bug.
    #[must_use]
    pub fn to_oauth_client_config(
        &self,
        redirect_uri: &str,
    ) -> Option<rmcp::transport::auth::OAuthClientConfig> {
        if self.is_pre_registered_stub() {
            return None;
        }
        let mut config =
            rmcp::transport::auth::OAuthClientConfig::new(self.client_id.clone(), redirect_uri);
        if let Some(secret) = &self.client_secret {
            config = config.with_client_secret(secret.clone());
        }
        Some(config)
    }
}

impl fmt::Debug for StoredClientInfo {
    /// MCP-277: `client_secret` renders as `[REDACTED]`, never as its value. A credential printed
    /// into a transcript, a log file or a crash report grants whoever reads it the access the
    /// credential encodes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredClientInfo")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("client_id_issued_at", &self.client_id_issued_at)
            .field("client_secret_expires_at", &self.client_secret_expires_at)
            .field("redirect_uris", &self.redirect_uris)
            .field("issuer", &self.issuer)
            .field("config_pre_registered", &self.config_pre_registered)
            .finish()
    }
}

/// `stringArray(value)` (MCP-250 rule 4) — accepted only when it is an array of strings; a mixed
/// array yields "omit the field", **not** "reject the entry", because upstream's `stringArray` never
/// returns `null`. This one field degrades silently, and reproducing that is the whole reason for a
/// custom deserializer rather than a plain `Option<Vec<String>>`.
fn deserialize_lenient_string_array<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(serde_json::Value::Array(items)) = value else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            serde_json::Value::String(text) => out.push(text),
            // A non-string element makes the whole array absent, exactly as `every()` failing does.
            _ => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// The complete auth entry for one server — upstream's `AuthEntry`, with three of its five slots now
/// typed by rmcp.
///
/// | upstream | port |
/// |---|---|
/// | `tokens: StoredTokens` | [`Self::credentials`] — `rmcp::…::StoredCredentials` |
/// | `clientInfo: StoredClientInfo` | [`Self::client`] — the DCR fields rmcp drops (MCP-290) |
/// | `codeVerifier` + `oauthState` | [`Self::state`] — `rmcp::…::StoredAuthorizationState` |
/// | `serverUrl` | [`Self::server_url`] |
///
/// **Unknown keys are dropped, not rejected** — serde's default, and `deny_unknown_fields` must
/// **not** be added: a record written by a newer version has to round-trip harmlessly through an
/// older one.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthEntry {
    /// The tokens, as rmcp persists them. `Debug` renders `token_response` as `[REDACTED]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<StoredCredentials>,
    /// The DCR client record (MCP-290).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<StoredClientInfo>,
    /// One PKCE/CSRF slot per server. Keeping **one** slot and having
    /// [`McpStateStore::load`](rmcp::transport::auth::StateStore::load) return it only when
    /// `csrf_token` matches reproduces upstream's single `oauthState` slot exactly while satisfying
    /// rmcp's keyed trait. `Debug` redacts both secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StoredAuthorizationState>,
    /// The exact `serverUrl` string these credentials were issued for. An entry without one is
    /// **invalid** — it predates the binding (MCP-264).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
}

impl AuthEntry {
    /// Is every slot empty? `removeAuthEntry` is still the caller's decision; this only reports.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.credentials.is_none()
            && self.client.is_none()
            && self.state.is_none()
            && self.server_url.is_none()
    }

    /// `hasStoredTokens` — `!!entry?.tokens`, with **no** expiry consideration. The live expiry
    /// predicate is rmcp's `AuthorizationManager::get_access_token` (MCP-267).
    ///
    /// **No in-crate caller, and that is the correct shape.** Its only one used to be
    /// `McpAuthStore::has_stored_credentials`, a `auth_entry(name)? .is_some_and(has_credentials)`
    /// wrapper that was itself never called and has been deleted. Every in-crate question about
    /// usable credentials is asked at *token* granularity instead —
    /// [`crate::oauth::inspect_mcp_oauth_tokens_for_url`] — because a `start_auth` writes the
    /// dynamic-registration record before any browser round trip, so an entry exists for a server
    /// that has never completed a login and this entry-level predicate would answer `true` for it.
    /// Kept as a public accessor on a public type: an embedder holding an [`AuthEntry`] has no other
    /// way to ask, and it is the one slot test that is not a field read.
    #[must_use]
    pub fn has_credentials(&self) -> bool {
        self.credentials.is_some()
    }

    /// `getAuthForUrl`'s two rejections, factored out because [`McpAuthStore::inspect_auth_for_url`]
    /// needs the same test: an entry with **no** stored URL is invalid ("this is from an old version
    /// — consider it invalid"), and a stored URL that differs is invalid.
    ///
    /// Comparison is exact string equality — **no** URL normalization, no trailing-slash tolerance.
    /// Adding any would silently widen credential reuse: a credential minted for one authorization
    /// server would be presented to a different one.
    #[must_use]
    pub fn matches_url(&self, server_url: &str) -> bool {
        // `if (!entry.serverUrl)` is a JS truthiness test, so the empty string is *absent*.
        self.server_url
            .as_deref()
            .is_some_and(|stored| !stored.is_empty() && stored == server_url)
    }
}

impl fmt::Debug for AuthEntry {
    /// MCP-277. rmcp already redacts `token_response`, `pkce_verifier` and `csrf_token`, and
    /// [`StoredClientInfo`]'s own `Debug` redacts `client_secret`, so this impl exists to guarantee
    /// there is no `#[derive(Debug)]` that a later field addition could silently widen.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthEntry")
            .field("credentials", &self.credentials)
            .field("client", &self.client)
            .field("state", &self.state)
            .field("server_url", &self.server_url)
            .finish()
    }
}

/// Build an [`OAuthTokenResponse`] without a direct `oauth2` dependency.
///
/// `StandardTokenResponse`'s serde shape **is** RFC 6749 §5.1's wire shape — `access_token`,
/// `token_type` (deserialized case-insensitively), `expires_in`, `refresh_token`, and a
/// space-delimited `scope` — so going through `serde_json` is both dependency-free and incapable of
/// drifting from the format the authorization server actually sends.
fn token_response_from_parts(
    access_token: &str,
    refresh_token: Option<&str>,
    expires_in: Option<u64>,
    scope: Option<&str>,
) -> Result<OAuthTokenResponse, serde_json::Error> {
    let mut object = serde_json::Map::new();
    object.insert(
        "access_token".to_string(),
        serde_json::Value::String(access_token.to_string()),
    );
    object.insert(
        "token_type".to_string(),
        serde_json::Value::String("Bearer".to_string()),
    );
    if let Some(refresh) = refresh_token {
        object.insert(
            "refresh_token".to_string(),
            serde_json::Value::String(refresh.to_string()),
        );
    }
    if let Some(expires) = expires_in {
        object.insert(
            "expires_in".to_string(),
            serde_json::Value::Number(expires.into()),
        );
    }
    if let Some(scope) = scope {
        object.insert(
            "scope".to_string(),
            serde_json::Value::String(scope.to_string()),
        );
    }
    serde_json::from_value(serde_json::Value::Object(object))
}

/// Build a [`StoredAuthorizationState`] without a direct `oauth2` dependency.
///
/// `StoredAuthorizationState::new` takes `&PkceCodeVerifier` and `&CsrfToken`, both `oauth2` types;
/// the struct is `#[non_exhaustive]` so a literal is impossible from outside rmcp. It is
/// `Deserialize`, and the legacy importer is the only caller that has raw strings rather than typed
/// values, so serde is the constructor.
fn authorization_state_from_parts(
    pkce_verifier: &str,
    csrf_token: &str,
    created_at: u64,
) -> Result<StoredAuthorizationState, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "pkce_verifier": pkce_verifier,
        "csrf_token": csrf_token,
        "expected_issuer": serde_json::Value::Null,
        "require_issuer": false,
        "created_at": created_at,
        "requested_scopes": Vec::<String>::new(),
    }))
}

// ---------------------------------------------------------------------------------------------
// MCP-251 — key naming in the OS keychain
// ---------------------------------------------------------------------------------------------

/// `createHash("sha256").update(bytes).digest("hex")` — lowercase hex, the same formatting
/// `cyrup_ext_subagents::exec::mcp_direct_tools::compute_mcp_server_hash` and
/// [`crate::dirs::compute_server_hash`] already use.
fn hex_sha256(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a `String` is infallible; the result is discarded rather than unwrapped.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// `getAuthEntryAccount(serverName)` — `sha256-<64 hex>` of the **UTF-8 bytes of the server name**.
///
/// Two consequences a porter must not miss:
///
/// * **The account is derived from the server name only.** Not from the URL, not from `oauthDir`,
///   not from the config file the server came from. Two projects configuring a server named `github`
///   share one keychain entry, and writes through two different options objects land on the same
///   account — upstream pins this deliberately with *"does not use configured oauthDir values as
///   secure-store namespaces"*.
/// * **The empty string is a valid server name** with a valid account (`sha256` of zero bytes). Do
///   not special-case it.
///
/// The same token is the legacy directory name ([`McpAuthStore::auth_entry_file_path`]), which is what makes an
/// arbitrary configured name — `../escape`, `@scope/name`, `сервер` — path-safe. Upstream's
/// `typeof serverName !== 'string'` guards do not port (MCP-276): the parameter is `&str`, the
/// value's only origin is a `serde_json` object **key** from `mcp.json`, and the hazard the guard
/// exists for is fully discharged here.
#[must_use]
pub fn auth_entry_account(server_name: &str) -> String {
    format!("sha256-{}", hex_sha256(server_name.as_bytes()))
}

/// `getAuthEntryChunkAccount(account, manifest, index)`.
#[must_use]
pub fn auth_entry_chunk_account(account: &str, chunk_digest: &str, index: usize) -> String {
    format!("{account}.chunk.{chunk_digest}.{index}")
}

// ---------------------------------------------------------------------------------------------
// MCP-253 / MCP-286 — the chunk manifest
// ---------------------------------------------------------------------------------------------

/// The manifest a chunked credential leaves at the **base** account.
///
/// Field order is load-bearing only in the sense that it is the emitted order; serde emits
/// declaration order, which reproduces JS insertion order:
///
/// ```json
/// {"__piMcpAdapterOAuthChunked":1,"chunkCount":7,"chunkDigest":"a1b2c3d4e5f60718"}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthEntryChunkManifest {
    /// The discriminator. Strictly `1` — not "truthy" (`isAuthEntryChunkManifest` tests `=== 1`).
    #[serde(rename = "__piMcpAdapterOAuthChunked")]
    marker: u8,
    /// How many `.chunk.<digest>.<i>` accounts hold the payload.
    #[serde(rename = "chunkCount")]
    pub chunk_count: usize,
    /// The first 16 hex characters of `sha256(payload)`. Part of every chunk account name, which is
    /// what makes a rewrite with different content land on *different* accounts and a rewrite with
    /// identical content land on the same ones.
    #[serde(rename = "chunkDigest")]
    pub chunk_digest: String,
}

impl AuthEntryChunkManifest {
    /// `createChunkManifest(payload)` — `ceil(len / 1000)` chunks over the payload's **UTF-8 bytes**
    /// and the first 16 hex characters of its SHA-256.
    ///
    /// > **The one genuine Rust hazard, and why the port diverges here.** Upstream's
    /// > `payload.length` and `payload.slice()` are **UTF-16 code units** while `chunkDigest` hashes
    /// > the **UTF-8** bytes of the same payload. `JSON.stringify` does not escape non-ASCII BMP or
    /// > astral characters, so a `scope` string, an `issuer` host or a token with any non-ASCII
    /// > character reaches the chunker verbatim, and upstream's `slice` can split a surrogate pair
    /// > into a lone surrogate that Security.framework and gnome-keyring (both UTF-8) cannot
    /// > faithfully store. Because the port's service name **and** record shape both changed
    /// > (MCP-280), no JS writer ever reads these accounts, so byte-boundary chunking is free:
    /// > [`chunk_boundaries`] cuts at the largest char boundary ≤ `i * 1000` bytes, so a chunk is
    /// > ≤1000 bytes — well under the 1280 ceiling — and no code point is split. Self-consistency is
    /// > the entire contract; cross-implementation byte-compatibility is not one.
    #[must_use]
    pub fn for_payload(payload: &str) -> Self {
        let digest = hex_sha256(payload.as_bytes());
        Self {
            marker: 1,
            chunk_count: payload.len().div_ceil(AUTH_SECRET_CHUNK_SIZE),
            // 16 ASCII hex characters always exist in a 64-character digest; the fallback keeps the
            // slice total without an index panic.
            chunk_digest: digest.get(..16).unwrap_or(digest.as_str()).to_string(),
        }
    }

    /// `getAuthEntryChunkAccounts(account, manifest)`.
    #[must_use]
    pub fn chunk_accounts(&self, account: &str) -> Vec<String> {
        (0..self.chunk_count)
            .map(|index| auth_entry_chunk_account(account, &self.chunk_digest, index))
            .collect()
    }
}

/// `isAuthEntryChunkManifest(value)` — all four properties, plus MCP-286's upper bound.
///
/// A payload failing **any** check is treated as an ordinary entry, not as an error. That is the
/// degradation upstream's validator applies to every other malformed field, and it is what keeps a
/// hostile `chunkCount: 1e9` from becoming a billion keyring reads.
fn parse_chunk_manifest(value: &serde_json::Value) -> Option<AuthEntryChunkManifest> {
    let object = value.as_object()?;
    if object.get(AUTH_CHUNK_MANIFEST_KEY)?.as_u64()? != 1 {
        return None;
    }
    let count_value = object.get("chunkCount")?;
    // `Number.isInteger(chunkCount) && chunkCount > 0`: `as_u64` rejects a fractional or negative
    // number, and `> 0` rejects zero.
    let chunk_count = usize::try_from(count_value.as_u64()?).ok()?;
    if chunk_count == 0 || chunk_count > AUTH_CHUNK_COUNT_LIMIT {
        return None;
    }
    let chunk_digest = object.get("chunkDigest")?.as_str()?;
    // `/^[a-f0-9]{16}$/` — hand-coded rather than a `Regex`, because the pattern is trivial and the
    // constructor is fallible.
    if chunk_digest.len() != 16
        || !chunk_digest
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
    {
        return None;
    }
    Some(AuthEntryChunkManifest {
        marker: 1,
        chunk_count,
        chunk_digest: chunk_digest.to_string(),
    })
}

/// The largest char boundary at or below `index`.
///
/// `str::floor_char_boundary` is still unstable, and `&s[a..b]` is denied by
/// `clippy::indexing_slicing` *and* would panic off a boundary — so this is both the portable and
/// the panic-free way to say it.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut boundary = index;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

/// The `chunk_count + 1` cut points of a chunked payload, in order, starting at `0` and ending at
/// `payload.len()`.
///
/// Boundary `i` is `floor_char_boundary(payload, i * 1000)`, so every chunk is ≤1000 bytes and no
/// code point is split. Strictly increasing for any payload whose characters are ≤4 bytes — i.e.
/// every valid `str` — so no chunk is ever empty.
#[must_use]
pub fn chunk_boundaries(payload: &str, chunk_count: usize) -> Vec<usize> {
    (0..=chunk_count)
        .map(|index| floor_char_boundary(payload, index.saturating_mul(AUTH_SECRET_CHUNK_SIZE)))
        .collect()
}

/// Split `payload` into `manifest.chunk_count` pieces at [`chunk_boundaries`].
fn split_payload(payload: &str, chunk_count: usize) -> Vec<&str> {
    let bounds = chunk_boundaries(payload, chunk_count);
    bounds
        .windows(2)
        .filter_map(|pair| match (pair.first(), pair.last()) {
            (Some(&start), Some(&end)) => payload.get(start..end),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// MCP-252 / MCP-258 — the backend layer
// ---------------------------------------------------------------------------------------------

/// Upstream's `AuthSecretStore` interface: three synchronous calls against one keychain service.
///
/// Synchronous on purpose. Every function in `mcp-auth.ts` is synchronous, `keyring` calls are
/// blocking syscalls, and the recovery hop is a blocking `spawnSync`. The async half of the port
/// lives one level up, where [`McpAuthStore`] wraps each call in `tokio::task::spawn_blocking`
/// (MCP-291) and holds the per-server lock across it (MCP-268).
pub trait AuthSecretStore: Send + Sync {
    /// `getPassword() ?? undefined`. **Absence is `Ok(None)`, never an error** — mapping
    /// `keyring_core::Error::NoEntry` wrong makes every fresh server look like a store failure.
    fn read(&self, account: &str) -> Result<Option<String>, AuthSecretStoreError>;

    /// `setPassword(payload)`.
    fn write(&self, account: &str, payload: &str) -> Result<(), AuthSecretStoreError>;

    /// `deleteCredential()`. A credential that was already absent is **not** a failure: upstream's
    /// `deleteCredential()` returns `false` rather than throwing.
    fn remove(&self, account: &str) -> Result<(), AuthSecretStoreError>;

    /// The three inspection exports (`getTestAuthSecretStoreReadCount`,
    /// `getTestAuthSecretStoreEntries`, `removeTestAuthSecretStoreEntry`) reach inside the store
    /// through this. They are contract, not incidental — both upstream test files use them.
    ///
    /// **No production caller, deliberately:** this is a downcast to the fault-injection backend, so
    /// every caller is a test or an inspection export. Production code reaching for it would be
    /// branching on which backend it got, which is exactly what [`AuthSecretStore`] exists to
    /// prevent. Keep it on the trait rather than on [`MemorySecretStore`]: the store arrives as
    /// `dyn AuthSecretStore` (chosen by `…_TEST_AUTH_STORE`), so a concrete-type method would be
    /// unreachable from where the inspection actually happens.
    fn as_memory(&self) -> Option<&MemorySecretStore> {
        None
    }
}

/// The real backend: one `keyring::Entry` per call, against one service.
///
/// Upstream's `loadKeyringEntryClass` and its absolute-path native-binding fallback table across
/// `darwin-{arm64,x64}`, `win32-{arm64,x64,ia32}-msvc`, `linux-{arm64-gnu,arm64-musl,arm-gnueabihf,
/// riscv64-gnu,x64-gnu,x64-musl}` and `freebsd-x64` **vanish entirely** — the Rust backends are
/// linked at compile time, so there is no module load to fail and no path to fall back to. The
/// *error* the table guards against still exists (locked keychain, no D-Bus session, no default
/// store) and still produces the same sentence, through
/// [`AuthSecretStoreError::StoreUnavailable`].
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    /// A backend bound to `service`. [`AUTH_SECRET_SERVICE`] for the live store,
    /// [`LEGACY_AUTH_SECRET_SERVICE`] for MCP-280's one-time importer.
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// `getKeyringEntry(account)`. `keyring::Entry::new` is **fallible** in 4.x — unlike the 3.x API
    /// and unlike `@napi-rs/keyring` — and `keyring::Entry::store_status()` reports the one-time
    /// credential-store initialisation, which is the exact analogue of upstream's try/catch.
    fn entry(&self, account: &str) -> Result<keyring::Entry, AuthSecretStoreError> {
        keyring::Entry::new(&self.service, account)
            .map_err(|source| AuthSecretStoreError::StoreUnavailable { source })
    }
}

impl AuthSecretStore for KeyringSecretStore {
    fn read(&self, account: &str) -> Result<Option<String>, AuthSecretStoreError> {
        match self.entry(account)?.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(AuthSecretStoreError::Keyring(error)),
        }
    }

    fn write(&self, account: &str, payload: &str) -> Result<(), AuthSecretStoreError> {
        self.entry(account)?
            .set_password(payload)
            .map_err(AuthSecretStoreError::Keyring)
    }

    fn remove(&self, account: &str) -> Result<(), AuthSecretStoreError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(AuthSecretStoreError::Keyring(error)),
        }
    }
}

/// Which of upstream's four fault-injection stores a [`MemorySecretStore`] is playing.
///
/// The four hand-rolled backends collapse onto one type plus an injected `keyring_core::Error`,
/// because two of that enum's `Display` strings are **byte-identical** to the ones upstream's stores
/// fabricate — which is also the confirmation that `@napi-rs/keyring` binds this crate family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimulatedFault {
    /// `memoryAuthSecretStore` — a plain map.
    #[default]
    None,
    /// `sizeLimitedAuthSecretStore` — mimics the Windows Credential Manager per-value ceiling. Only
    /// `write` fails, and only past [`AUTH_SECRET_VALUE_LIMIT`].
    SizeLimited,
    /// `unavailableAuthSecretStore` — every op fails.
    Unavailable,
    /// `keyRevokedAuthSecretStore` — every op fails with a cause chain the recovery predicate
    /// matches.
    KeyRevoked,
}

impl SimulatedFault {
    /// The exact-equality dispatch of `getAuthSecretStore()`.
    fn from_env_value(value: &str) -> Option<Self> {
        match value {
            "memory" => Some(SimulatedFault::None),
            "sizelimited" => Some(SimulatedFault::SizeLimited),
            "unavailable" => Some(SimulatedFault::Unavailable),
            "keyrevoked" => Some(SimulatedFault::KeyRevoked),
            _ => None,
        }
    }

    /// The error this fault raises for an op that is not the size check.
    fn error(self) -> Option<AuthSecretStoreError> {
        match self {
            SimulatedFault::None | SimulatedFault::SizeLimited => None,
            // `Display` is `Platform failure: simulated secure credential store unavailable`.
            SimulatedFault::Unavailable => Some(AuthSecretStoreError::Keyring(
                keyring::Error::PlatformFailure(Box::new(std::io::Error::other(
                    "simulated secure credential store unavailable",
                ))),
            )),
            // `Display` is `Couldn't access platform storage: KeyRevoked`, byte-identical to the
            // string upstream's `createKeyRevokedTestError` fabricates, with the inner `KeyRevoked`
            // reachable through `source()` exactly as upstream's `{ cause: … }` is.
            SimulatedFault::KeyRevoked => Some(AuthSecretStoreError::Keyring(
                keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("KeyRevoked"))),
            )),
        }
    }
}

/// The in-process backend behind every unit test, and the read counter both upstream suites assert
/// on.
///
/// **Divergence, named:** upstream's `memory` and `sizelimited` stores share one module-global map
/// and one module-global counter; here each [`MemorySecretStore`] owns its own, because the port's
/// primary seam is constructor injection ([`McpAuthStore::with_backend`]) rather than a process
/// global. No upstream test uses both backends against one map, so nothing observable changes; what
/// *is* preserved is that both bump the counter and that a throwing read bumps it too.
pub struct MemorySecretStore {
    entries: StdMutex<IndexMap<String, String>>,
    read_count: AtomicU64,
    fault: SimulatedFault,
}

impl MemorySecretStore {
    /// A clean store with no injected fault.
    #[must_use]
    pub fn new() -> Self {
        Self::with_fault(SimulatedFault::None)
    }

    /// A store playing one of upstream's four backends.
    #[must_use]
    pub fn with_fault(fault: SimulatedFault) -> Self {
        Self {
            entries: StdMutex::new(IndexMap::new()),
            read_count: AtomicU64::new(0),
            fault,
        }
    }

    /// `getTestAuthSecretStoreReadCount()`.
    #[must_use]
    pub fn read_count(&self) -> u64 {
        self.read_count.load(Ordering::SeqCst)
    }

    /// `getTestAuthSecretStoreEntries()` — insertion-ordered, so a test can assert "exactly one
    /// non-`.chunk.` entry at the base account" without sorting.
    #[must_use]
    pub fn entries(&self) -> Vec<(String, String)> {
        self.lock()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// `removeTestAuthSecretStoreEntry(account)` — reaches *behind* the store's own `remove`, which
    /// is how the "a deleted chunk surfaces as `unavailable`" case is set up.
    pub fn remove_entry(&self, account: &str) {
        self.lock().shift_remove(account);
    }

    /// Seed a value without bumping the read counter or tripping a fault — the
    /// `writeBehindTheCache` helper's backing primitive.
    pub fn seed(&self, account: &str, payload: &str) {
        self.lock().insert(account.to_string(), payload.to_string());
    }

    /// `resetTestAuthSecretStore()`'s store half: map **and** counter. The cache half is
    /// [`McpAuthStore::reset_cache`], and the two differ exactly as upstream pins —
    /// `resetAuthEntryCache` leaves the counter alone.
    pub fn reset(&self) {
        self.lock().clear();
        self.read_count.store(0, Ordering::SeqCst);
    }

    /// A poisoned lock still holds a usable map: the only panic that could poison it would come from
    /// the standard library, and losing every credential handle over it is worse than proceeding.
    fn lock(&self) -> std::sync::MutexGuard<'_, IndexMap<String, String>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for MemorySecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthSecretStore for MemorySecretStore {
    fn read(&self, account: &str) -> Result<Option<String>, AuthSecretStoreError> {
        // **All four** injected backends bump the counter on `read`, including the two that throw —
        // pinned by `mcp-auth-cache.test.ts`, which observes the throwing pair with recovery
        // disabled. The bump therefore precedes the fault check.
        self.read_count.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.fault.error() {
            return Err(error);
        }
        Ok(self.lock().get(account).cloned())
    }

    fn write(&self, account: &str, payload: &str) -> Result<(), AuthSecretStoreError> {
        if let Some(error) = self.fault.error() {
            return Err(error);
        }
        if self.fault == SimulatedFault::SizeLimited && payload.len() > AUTH_SECRET_VALUE_LIMIT {
            // `Display` is `Value of 'password encoded as UTF-16' is longer than the platform limit
            // of 2560 chars` — byte-identical to upstream's fabricated string, but here it is a
            // **typed** condition rather than a message match.
            return Err(AuthSecretStoreError::Keyring(keyring::Error::TooLong(
                "password encoded as UTF-16".to_string(),
                (AUTH_SECRET_VALUE_LIMIT * 2) as u32,
            )));
        }
        self.lock().insert(account.to_string(), payload.to_string());
        Ok(())
    }

    fn remove(&self, account: &str) -> Result<(), AuthSecretStoreError> {
        if let Some(error) = self.fault.error() {
            return Err(error);
        }
        self.lock().shift_remove(account);
        Ok(())
    }

    fn as_memory(&self) -> Option<&MemorySecretStore> {
        Some(self)
    }
}

/// A backend whose `remove` always fails, for MCP-255's and MCP-285's asymmetry tests.
///
/// No upstream twin: upstream tests the same asymmetry by mutating its module-global store. This is
/// the injectable equivalent.
pub struct FailingRemoveStore {
    inner: MemorySecretStore,
    /// `None` fails every removal; `Some(i)` fails only the chunk at index `i`.
    failing_suffix: Option<String>,
}

impl FailingRemoveStore {
    /// Fail every `remove`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: MemorySecretStore::new(),
            failing_suffix: None,
        }
    }

    /// Fail only removals of accounts ending in `suffix` — e.g. `".1"` for chunk index 1.
    #[must_use]
    pub fn failing_suffix(suffix: impl Into<String>) -> Self {
        Self {
            inner: MemorySecretStore::new(),
            failing_suffix: Some(suffix.into()),
        }
    }
}

impl Default for FailingRemoveStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthSecretStore for FailingRemoveStore {
    fn read(&self, account: &str) -> Result<Option<String>, AuthSecretStoreError> {
        self.inner.read(account)
    }

    fn write(&self, account: &str, payload: &str) -> Result<(), AuthSecretStoreError> {
        self.inner.write(account, payload)
    }

    fn remove(&self, account: &str) -> Result<(), AuthSecretStoreError> {
        let fails = self
            .failing_suffix
            .as_ref()
            .is_none_or(|suffix| account.ends_with(suffix.as_str()));
        if fails {
            return Err(AuthSecretStoreError::Keyring(
                keyring::Error::NotSupportedByStore("remove refused".to_string()),
            ));
        }
        self.inner.remove(account)
    }

    fn as_memory(&self) -> Option<&MemorySecretStore> {
        Some(&self.inner)
    }
}

/// `getAuthSecretStore()` — the env selector, matched by **exact** string equality.
///
/// Constructor injection ([`McpAuthStore::with_backend`]) is the primary seam; this exists for the
/// one end-to-end case that crosses a process boundary, where MCP-260's recovery test genuinely
/// needs it. `cyrup_ext::caps::proc::ProcCaps`'s `with_kill_grace` / `with_write_stdin_timeout` are
/// the in-tree precedent for exactly this shape.
fn select_backend(env: &EnvFn, service: &str) -> Arc<dyn AuthSecretStore> {
    match env_first(env, &TEST_AUTH_STORE_ENV)
        .as_deref()
        .and_then(SimulatedFault::from_env_value)
    {
        Some(fault) => Arc::new(MemorySecretStore::with_fault(fault)),
        None => Arc::new(KeyringSecretStore::new(service)),
    }
}

// ---------------------------------------------------------------------------------------------
// §6.7 / MCP-260 · MCP-261 · MCP-262 · MCP-287 — the Linux revoked-keyring recovery hop
// ---------------------------------------------------------------------------------------------

/// `causeChainContains(error, /key\s*(?:has been\s*)?revoked|keyrevoked/i)`'s pattern.
///
/// It matches, in practice: `KeyRevoked` (the `linux-keyutils` error name), `Key has been revoked`
/// (the `strerror` text for `EKEYREVOKED`), and `key revoked`.
///
/// `Regex::new` is fallible and `unwrap` is denied, so the `LazyLock` holds an `Option`; a `None`
/// (impossible for this literal) degrades to "never matches", which disables recovery rather than
/// spawning subprocesses on every keychain hiccup.
static KEY_REVOKED_PATTERN: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)key\s*(?:has been\s*)?revoked|keyrevoked").ok());

/// Walk `std::error::Error::source()` transitively, testing each link's `Display` against the
/// revoked-keyring pattern (MCP-262).
///
/// Upstream walks `error.cause` with a `Set` cycle guard and tests `name`, `message` and `code`;
/// Rust has one renderable string per link, and `keyring_core::Error::{PlatformFailure,
/// NoStorageAccess, BadDataFormat}` are the **only** variants that return a `source`, so the walk
/// terminates naturally. It is depth-capped anyway so a pathological `source()` impl cannot spin —
/// the same discipline [`crate::errors::McpError::is_cleanup_failure`] already applies.
#[must_use]
pub fn cause_chain_contains_key_revoked(error: &(dyn std::error::Error + 'static)) -> bool {
    let Some(pattern) = KEY_REVOKED_PATTERN.as_ref() else {
        return false;
    };
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    for _ in 0..32 {
        let Some(link) = current else { return false };
        if pattern.is_match(&link.to_string()) {
            return true;
        }
        current = link.source();
    }
    false
}

/// `isLinuxKeyringRecoveryEnabled()` — disabled by `…_DISABLE_KEYRING_RECOVERY == "1"`; otherwise
/// enabled on Linux, or anywhere when `…_TEST_LINUX_KEYRING_RECOVERY == "1"`.
fn is_linux_keyring_recovery_enabled(env: &EnvFn) -> bool {
    if env_is_one(env, &KEYRING_RECOVERY_DISABLED_ENV) {
        return false;
    }
    cfg!(target_os = "linux") || env_is_one(env, &TEST_LINUX_KEYRING_RECOVERY_ENV)
}

/// `shouldAttemptLinuxKeyringRecovery(error)` — **both** halves required.
///
/// The negative case matters as much as the positive one: upstream's own test sets a `keyctl` that
/// exits 99 and asserts the fake store file is never created, because a predicate that fires on a
/// generic failure spawns a subprocess on every keychain hiccup.
fn should_attempt_recovery(env: &EnvFn, error: &AuthStoreError) -> bool {
    is_linux_keyring_recovery_enabled(env) && cause_chain_contains_key_revoked(error)
}

/// Which keyring call the helper is being asked to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyringRecoveryOperation {
    /// `getPassword()`.
    Read,
    /// `setPassword(payload)`.
    Write,
    /// `deleteCredential()`.
    Remove,
}

/// The one-line JSON request the parent writes to the helper's stdin (MCP-261).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyringHelperRequest {
    /// `read` | `write` | `remove`.
    pub operation: KeyringRecoveryOperation,
    /// The keychain service. It travels the wire **even though it is a constant**, and the helper
    /// validates it, so the helper stays a general one-shot keyring tool rather than a hard-coded
    /// one.
    pub service: String,
    /// The keychain account.
    pub account: String,
    /// The value, for `write` only. **Omitted entirely** for `read`/`remove` — `JSON.stringify`
    /// drops `undefined`, so those requests carry no `payload` key at all, not `"payload": null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
}

/// The one-line JSON response the helper writes to stdout.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyringHelperResponse {
    /// Whether the operation succeeded.
    pub ok: bool,
    /// `read` only: whether a credential existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub found: Option<bool>,
    /// `read` only, and only when `found`: the stored value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Error text when `ok` is `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How the recovery hop is invoked. Every field is env-overridable and read **per call**, because
/// upstream reads `process.env` on every `runLinuxKeyringRecoveryOperation`.
struct RecoveryInvocation {
    keyctl: String,
    program: PathBuf,
    args: Vec<String>,
}

impl RecoveryInvocation {
    /// `keyctl` from `…_KEYRING_RECOVERY_KEYCTL` (trimmed; blank ⇒ `"keyctl"`), and the program from
    /// `…_KEYRING_RECOVERY_HELPER` (trimmed) or `current_exe()` + [`KEYRING_HELPER_SUBCOMMAND`].
    ///
    /// `PI_MCP_ADAPTER_KEYRING_RECOVERY_NODE` has no counterpart: after MCP-260 the re-execed
    /// program *is* the `cyrup` binary and there is no interpreter to name.
    fn resolve(env: &EnvFn) -> Result<Self, AuthSecretStoreError> {
        let keyctl = env_first(env, &KEYRING_RECOVERY_KEYCTL_ENV)
            .map(|raw| raw.trim().to_string())
            .filter(|trimmed| !trimmed.is_empty())
            .unwrap_or_else(|| "keyctl".to_string());

        match env_first(env, &KEYRING_RECOVERY_HELPER_ENV)
            .map(|raw| raw.trim().to_string())
            .filter(|trimmed| !trimmed.is_empty())
        {
            // An override names a **program**, not a script: it is exec'd directly, with no
            // subcommand token appended, so a fixture helper can be a plain shell script.
            Some(helper) => Ok(Self {
                keyctl,
                program: PathBuf::from(helper),
                args: Vec::new(),
            }),
            None => {
                let program = std::env::current_exe().map_err(|error| {
                    AuthSecretStoreError::Recovery(format!(
                        "Linux keyring recovery helper could not start: {error}"
                    ))
                })?;
                Ok(Self {
                    keyctl,
                    program,
                    args: vec![KEYRING_HELPER_SUBCOMMAND.to_string()],
                })
            }
        }
    }
}

/// `runLinuxKeyringRecoveryOperation(operation, account, payload)` — one `keyctl session -` round
/// trip.
///
/// ```text
/// argv     : <keyctl>  "session"  "-"  <program> [args…]
/// stdin    : {"operation":…,"service":…,"account":…[,"payload":…]} + "\n"
/// encoding : utf8      maxBuffer: 1 MiB      timeout: 10_000 ms
/// ```
///
/// **The argv shape is pinned by a fixture.** Upstream's test `keyctl` asserts
/// `$1 == "session" && $2 == "-"`, exits **64** otherwise, then `shift 2; exec "$@"` — so any extra
/// flag breaks the mechanism. `session -` creates an *anonymous* session keyring and execs its
/// remaining argv inside it, which is the only way a process attached to a revoked keyring can
/// perform a keyring call at all. That `keyring` links in-process in Rust changes nothing: an
/// in-process library is precisely what cannot recover.
fn run_recovery_operation(
    env: &EnvFn,
    service: &str,
    operation: KeyringRecoveryOperation,
    account: &str,
    payload: Option<&str>,
) -> Result<KeyringHelperResponse, AuthSecretStoreError> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let invocation = RecoveryInvocation::resolve(env)?;
    let request = KeyringHelperRequest {
        operation,
        service: service.to_string(),
        account: account.to_string(),
        payload: payload.map(str::to_string),
    };
    let body = serde_json::to_string(&request).map_err(|error| {
        AuthSecretStoreError::Recovery(format!(
            "Linux keyring recovery helper could not start: {error}"
        ))
    })?;

    let mut child = Command::new(&invocation.keyctl)
        .arg("session")
        .arg("-")
        .arg(&invocation.program)
        .args(&invocation.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            // Rung 1. `spawnSync`'s failure to launch lands here, and so does its `timeout`.
            AuthSecretStoreError::Recovery(format!(
                "Linux keyring recovery helper could not start: {error}"
            ))
        })?;

    // stdin is written from its own thread: the request can approach 1 MiB while a pipe buffer is
    // 64 KB, so writing inline would deadlock against a helper that had not started draining yet.
    let stdin_writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(format!("{body}\n").as_bytes());
            let _ = stdin.flush();
        })
    });

    // stdout is read from its own thread with the 1 MiB `maxBuffer` cap. Both sides cap
    // independently; exceeding it is rung 1, exactly as `spawnSync`'s `ENOBUFS` is.
    let stdout_reader = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 8192];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => return Ok(buffer),
                    Ok(read) => {
                        match chunk.get(..read) {
                            Some(slice) => buffer.extend_from_slice(slice),
                            None => return Ok(buffer),
                        }
                        if buffer.len() > KEYRING_HELPER_MAX_BYTES {
                            return Err("stdout maxBuffer exceeded".to_string());
                        }
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
        })
    });

    // `spawnSync`'s `timeout` has no `std` equivalent, so this is a bounded poll: on expiry the
    // child is killed and the **rung-1** variant is returned, not a timeout-specific one — because
    // upstream's timeout populates `result.error`, not `result.status`, and a Rust-only message
    // would silently change the sentence the user sees (MCP-287).
    let deadline = Instant::now() + Duration::from_millis(KEYRING_RECOVERY_TIMEOUT_MS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(handle) = stdin_writer {
                        let _ = handle.join();
                    }
                    if let Some(handle) = stdout_reader {
                        let _ = handle.join();
                    }
                    return Err(AuthSecretStoreError::Recovery(format!(
                        "Linux keyring recovery helper could not start: {} timed out after {} ms",
                        invocation.keyctl, KEYRING_RECOVERY_TIMEOUT_MS
                    )));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                return Err(AuthSecretStoreError::Recovery(format!(
                    "Linux keyring recovery helper could not start: {error}"
                )))
            }
        }
    };

    if let Some(handle) = stdin_writer {
        let _ = handle.join();
    }
    let stdout = match stdout_reader.map(std::thread::JoinHandle::join) {
        Some(Ok(Ok(bytes))) => bytes,
        Some(Ok(Err(message))) => {
            return Err(AuthSecretStoreError::Recovery(format!(
                "Linux keyring recovery helper could not start: {message}"
            )))
        }
        _ => Vec::new(),
    };

    // Rung 2. Note the ordering against rung 5: the real helper sets exit code 1 alongside every
    // `{ok:false}` reply, so this rung wins and the user sees
    // `… failed with exit code 1` rather than the helper's own error text.
    if !status.success() {
        let code = status
            .code()
            .map_or_else(|| "unknown".to_string(), |code| code.to_string());
        return Err(AuthSecretStoreError::Recovery(format!(
            "Linux keyring recovery helper failed with exit code {code}"
        )));
    }

    // Rung 3.
    let text = String::from_utf8_lossy(&stdout);
    let response: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| {
        AuthSecretStoreError::Recovery(
            "Linux keyring recovery helper returned invalid JSON".to_string(),
        )
    })?;

    // Rung 4 — `typeof response !== 'object' || response === null || typeof response.ok !== 'boolean'`.
    let ok = response
        .as_object()
        .and_then(|object| object.get("ok"))
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            AuthSecretStoreError::Recovery(
                "Linux keyring recovery helper returned an invalid response".to_string(),
            )
        })?;
    let typed: KeyringHelperResponse = serde_json::from_value(response).map_err(|_| {
        AuthSecretStoreError::Recovery(
            "Linux keyring recovery helper returned an invalid response".to_string(),
        )
    })?;

    // Rung 5 — **unreachable against the real helper** (see rung 2), kept because it is the
    // documented contract for any third-party helper substituted through `…_KEYRING_RECOVERY_HELPER`.
    if !ok {
        return Err(AuthSecretStoreError::Recovery(
            typed
                .error
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "Linux keyring recovery helper failed".to_string()),
        ));
    }

    // Rung 6.
    if operation == KeyringRecoveryOperation::Read
        && typed.found == Some(true)
        && typed.value.is_none()
    {
        return Err(AuthSecretStoreError::Recovery(
            "Linux keyring recovery helper returned an invalid read response".to_string(),
        ));
    }

    Ok(typed)
}

/// `linuxKeyringRecoveryAuthSecretStore` — each op is one `keyctl session -` subprocess round trip.
///
/// **Not selectable** by the env switch: it is entered *only* as a one-shot retry from
/// [`McpAuthStore`]'s three recovery sites, and a second failure propagates.
pub struct LinuxKeyringRecoveryStore {
    service: String,
    env: EnvFn,
}

impl LinuxKeyringRecoveryStore {
    /// A recovery store for one service.
    #[must_use]
    pub fn new(service: impl Into<String>, env: EnvFn) -> Self {
        Self {
            service: service.into(),
            env,
        }
    }
}

impl AuthSecretStore for LinuxKeyringRecoveryStore {
    fn read(&self, account: &str) -> Result<Option<String>, AuthSecretStoreError> {
        let response = run_recovery_operation(
            &self.env,
            &self.service,
            KeyringRecoveryOperation::Read,
            account,
            None,
        )?;
        // `response.value` only when `ok && found === true`, else `undefined`.
        Ok(if response.found == Some(true) {
            response.value
        } else {
            None
        })
    }

    fn write(&self, account: &str, payload: &str) -> Result<(), AuthSecretStoreError> {
        run_recovery_operation(
            &self.env,
            &self.service,
            KeyringRecoveryOperation::Write,
            account,
            Some(payload),
        )
        .map(|_| ())
    }

    fn remove(&self, account: &str) -> Result<(), AuthSecretStoreError> {
        run_recovery_operation(
            &self.env,
            &self.service,
            KeyringRecoveryOperation::Remove,
            account,
            None,
        )
        .map(|_| ())
    }
}

/// The helper process's whole body (MCP-261) — `mcp-keyring-helper.cjs` in Rust.
///
/// It performs exactly one keyring operation and exits. It never reads config, never touches the
/// network, never logs a secret, and its stdout is exactly one line of JSON. This is the one code
/// path in the crate that must not initialise the cache, the config or tracing.
///
/// `crates/cyrup/src/mcp_keyring_helper_cmd.rs`'s `dispatch()` is
/// `run_keyring_helper(&mut std::io::stdin(), &mut std::io::stdout())` and nothing else.
///
/// Returns the process exit code. **1 on every error reply**, matching the `.cjs`'s
/// `process.exitCode = 1`, so the parent's rung 2 keeps winning: a helper that exited 0 on error
/// would silently change the message the user sees.
pub fn run_keyring_helper<R: std::io::Read, W: std::io::Write>(stdin: &mut R, stdout: &mut W) -> i32 {
    match keyring_helper_exchange(stdin) {
        Ok(response) => {
            write_helper_response(stdout, &response);
            0
        }
        Err(message) => {
            write_helper_response(
                stdout,
                &KeyringHelperResponse {
                    ok: false,
                    error: Some(message),
                    ..KeyringHelperResponse::default()
                },
            );
            1
        }
    }
}

/// One line of JSON, newline-terminated. A write failure is unreportable — stdout is the only
/// channel — so it is discarded rather than escalated.
fn write_helper_response<W: std::io::Write>(stdout: &mut W, response: &KeyringHelperResponse) {
    if let Ok(body) = serde_json::to_string(response) {
        let _ = writeln!(stdout, "{body}");
        let _ = stdout.flush();
    }
}

/// Read, validate, perform. Validation order and messages are upstream's, exactly:
/// `request too large` (>1 MiB), `invalid request`, `invalid operation`, `invalid service`,
/// `invalid account`, `invalid payload`.
fn keyring_helper_exchange<R: std::io::Read>(stdin: &mut R) -> Result<KeyringHelperResponse, String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stdin.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        match chunk.get(..read) {
            Some(slice) => buffer.extend_from_slice(slice),
            None => break,
        }
        if buffer.len() > KEYRING_HELPER_MAX_BYTES {
            return Err("request too large".to_string());
        }
    }

    let text = String::from_utf8(buffer).map_err(|_| "invalid request".to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|_| "invalid request".to_string())?;
    let object = value.as_object().ok_or("invalid request")?;

    let operation = match object.get("operation").and_then(serde_json::Value::as_str) {
        Some("read") => KeyringRecoveryOperation::Read,
        Some("write") => KeyringRecoveryOperation::Write,
        Some("remove") => KeyringRecoveryOperation::Remove,
        _ => return Err("invalid operation".to_string()),
    };
    let service = object
        .get("service")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("invalid service")?;
    let account = object
        .get("account")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("invalid account")?;

    let store = KeyringSecretStore::new(service);
    match operation {
        KeyringRecoveryOperation::Read => {
            let value = store.read(account).map_err(|error| error.to_string())?;
            Ok(match value {
                Some(value) => KeyringHelperResponse {
                    ok: true,
                    found: Some(true),
                    value: Some(value),
                    error: None,
                },
                None => KeyringHelperResponse {
                    ok: true,
                    found: Some(false),
                    value: None,
                    error: None,
                },
            })
        }
        KeyringRecoveryOperation::Write => {
            let payload = object
                .get("payload")
                .and_then(serde_json::Value::as_str)
                .ok_or("invalid payload")?;
            store
                .write(account, payload)
                .map_err(|error| error.to_string())?;
            Ok(KeyringHelperResponse {
                ok: true,
                ..KeyringHelperResponse::default()
            })
        }
        KeyringRecoveryOperation::Remove => {
            store.remove(account).map_err(|error| error.to_string())?;
            Ok(KeyringHelperResponse {
                ok: true,
                ..KeyringHelperResponse::default()
            })
        }
    }
}

// ---------------------------------------------------------------------------------------------
// MCP-256 — the legacy plaintext import path, and the base dir that still partitions auth attempts
// ---------------------------------------------------------------------------------------------

/// `AuthStorageOptions` — **legacy plaintext import directory only**. Persistent secrets do not use
/// it as their store.
///
/// It is not dead once import is done: `mcp-auth-flow.ts` folds [`McpAuthStore::auth_base_dir`] into
/// two in-flight dedup keys — `` `${serverName}|${baseDir}` `` and
/// `` `${serverName}|${serverUrl}|${baseDir}` `` — so the base dir still partitions concurrent auth
/// attempts. Keep the accessor public.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthStorageOptions {
    /// `options.baseDir`: the resolved `settings.oauthDir`, or `None` for "use the agent dir".
    /// Absent is **not** the same as `<agent_dir>/mcp-oauth` — the precedence ladder in
    /// [`McpAuthStore::auth_base_dir`] is what turns absent into that.
    pub base_dir: Option<PathBuf>,
}

impl AuthStorageOptions {
    /// `getAuthStorageOptions(settings.oauthDir, cwd)` — `resolveConfiguredOAuthDir` then
    /// `baseDir ? { baseDir } : {}`.
    ///
    /// Upstream's `"settings.oauthDir must be a string"` throw is the deserialiser's job here
    /// (MCP-066), and `undefined` / `null` / blank all yield `None`.
    #[must_use]
    pub fn from_settings(oauth_dir: Option<&str>, cwd: &Path) -> Self {
        Self {
            base_dir: oauth_dir.and_then(|raw| crate::dirs::resolve_configured_oauth_dir(raw, cwd)),
        }
    }

    /// An explicit root, for a caller that already resolved one.
    ///
    /// **No production caller, deliberately:** production always arrives through
    /// [`Self::from_settings`], because the `settings.oauthDir` trim/blank/resolve rule
    /// ([`crate::dirs::resolve_configured_oauth_dir`]) is the *only* sanctioned way to turn config
    /// into a root, and a second production entry point that skips it would be a second copy of that
    /// rule. What is left is tests and any downstream embedder that resolved a root by other means;
    /// both want the raw constructor, neither wants the config grammar.
    #[must_use]
    pub fn with_base_dir(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base_dir.into()),
        }
    }
}

/// Upstream's `StoredTokens` — the v2.25.0 on-keychain and on-disk token shape, read **only** by the
/// two importers.
///
/// Nothing writes this type. It exists so a `tokens.json` left by an older adapter, and a keychain
/// entry left by a co-installed `pi-mcp-adapter`, can both be translated (MCP-256, MCP-280).
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyStoredTokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Fractional Unix **seconds**. Never milliseconds — `cyrup_provider::auth::types::Credential`'s
    /// `expires` is `i64` milliseconds and rmcp's `token_received_at` is integral seconds; never
    /// assign between them (MCP-267's unit trap).
    #[serde(default)]
    expires_at: Option<f64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    issuer: Option<String>,
}

/// Upstream's `AuthEntry` — the v2.25.0 record, read only by the two importers.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyAuthEntry {
    #[serde(default)]
    tokens: Option<LegacyStoredTokens>,
    #[serde(default)]
    client_info: Option<StoredClientInfo>,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    oauth_state: Option<String>,
    #[serde(default)]
    server_url: Option<String>,
}

/// `{accessToken, refreshToken, expiresAt, scope, issuer}` + `clientInfo.clientId` →
/// [`StoredCredentials`] (MCP-256, MCP-267, MCP-288).
///
/// `token_received_at = now` and `expires_in = max(0, floor(expiresAt - now))`, which is precisely
/// `oauth-handler.ts`'s `getStoredTokens` arithmetic.
///
/// **The `expiresAt = 0` case resolves to `expires_in = 0` — already expired.** Upstream reads the
/// same field with three different zero-semantics (`isTokenExpired`'s `!expiresAt` says "no
/// expiry"; `getStoredTokens`'s `expiresAt !== undefined && expiresAt < now` says "expired";
/// `tokens()`'s `expiresAt ? … : undefined` omits `expires_in`), and collapsing them silently
/// changes behaviour at whichever site loses. This site takes `getStoredTokens`'s semantic. Do not
/// "restore" `isTokenExpired`'s falsy rule here.
///
/// **Named behaviour delta:** [`StoredCredentials::client_id`] is required and non-`Option`, so a
/// legacy entry carrying `tokens` but **no** `clientInfo.clientId` cannot be translated. The caller
/// imports the `clientInfo`/`serverUrl` if present, drops the tokens, deletes the file, and lets the
/// next call re-authenticate — never fabricating a client id.
fn legacy_credentials(
    tokens: &LegacyStoredTokens,
    client_id: &str,
    now: f64,
) -> Option<StoredCredentials> {
    let expires_in = tokens.expires_at.map(|expires_at| {
        let remaining = expires_at - now;
        if remaining <= 0.0 {
            0
        } else {
            // `Math.max(0, Math.floor(expiresAt - now))`, saturating rather than wrapping.
            remaining.floor().min(u64::MAX as f64) as u64
        }
    });
    let token_response = token_response_from_parts(
        &tokens.access_token,
        tokens.refresh_token.as_deref(),
        expires_in,
        tokens.scope.as_deref(),
    )
    .ok()?;
    let granted_scopes = tokens
        .scope
        .as_deref()
        .map(|scope| {
            scope
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(
        StoredCredentials::new(
            client_id.to_string(),
            Some(token_response),
            granted_scopes,
            Some(now as u64),
        )
        .with_issuer(tokens.issuer.clone()),
    )
}

/// The whole v2.25.0 → port translation, used by both importers.
///
/// Returns `None` only when the record has nothing worth keeping, so a translation failure never
/// half-writes: MCP-280 requires that a legacy keychain entry which fails translation leaves **both**
/// services untouched.
fn translate_legacy_entry(legacy: LegacyAuthEntry, now: f64) -> Option<AuthEntry> {
    let credentials = match (&legacy.tokens, &legacy.client_info) {
        (Some(tokens), Some(client)) => legacy_credentials(tokens, &client.client_id, now),
        // Tokens with no client id are dropped, not fabricated (the named delta above).
        _ => None,
    };
    let state = match (&legacy.code_verifier, &legacy.oauth_state) {
        (Some(verifier), Some(csrf)) => {
            authorization_state_from_parts(verifier, csrf, now as u64).ok()
        }
        // Upstream stores the two independently, but rmcp's `StoredAuthorizationState` needs both to
        // be meaningful: a verifier with no CSRF token can never be matched by `StateStore::load`,
        // and a CSRF token with no verifier cannot complete an exchange. Half a PKCE context is
        // dropped rather than persisted in a shape that would fail later, at the worst moment.
        _ => None,
    };
    let entry = AuthEntry {
        credentials,
        client: legacy.client_info,
        state,
        server_url: legacy.server_url,
    };
    if entry.is_empty() {
        None
    } else {
        Some(entry)
    }
}

// ---------------------------------------------------------------------------------------------
// §6.9 / MCP-265 — the three-state status accessor
// ---------------------------------------------------------------------------------------------

/// `OAuthCredentialStatus` — the **status** accessor's three states, and the only three-state one.
///
/// Status UI distinguishes "no credentials" from "the store is broken"; authentication paths do not
/// — they use [`McpAuthStore::auth_for_url`] and stay **fail-closed**. A broken keychain degrades
/// `/mcp` status output; it never silently grants access and never silently restarts auth.
#[derive(Debug)]
// `Present` carries a whole `AuthEntry` while the other two carry little. Boxing it would trade a
// stack-size warning for a heap allocation on the *common* arm, and the `/mcp` panel builds one of
// these per configured server on every render — the same trade `crate::config::OAuthSetting`
// documents.
#[allow(clippy::large_enum_variant)]
pub enum OAuthCredentialStatus {
    /// A credential exists and is bound to the requested URL.
    Present(AuthEntry),
    /// No entry, no stored `serverUrl`, or a URL mismatch.
    Absent,
    /// The store itself is unreachable. Carries
    /// [`format_oauth_credential_store_unavailable`]'s sentence.
    Unavailable {
        /// The actionable sentence, ready for the panel.
        message: String,
    },
}

/// `formatOAuthCredentialStoreUnavailable(error)` (MCP-263) — two literals, verbatim.
///
/// The word "Pi" is host branding and becomes the cyrup app name ([`crate::dirs::APP_NAME`]); that
/// is the one intentional edit and it is the only one.
#[must_use]
pub fn format_oauth_credential_store_unavailable(error: &AuthStoreError) -> String {
    if cfg!(target_os = "linux") && cause_chain_contains_key_revoked(error) {
        return format!(
            "OAuth credential store unavailable: the Linux session keyring may be revoked. \
             Start {} from a fresh login/keyring session and retry.",
            crate::dirs::APP_NAME
        );
    }
    "OAuth credential store unavailable. Configure or unlock the OS credential store and retry."
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// MCP-266 — `McpAuthStore`: the accessor surface section 07 consumes
// ---------------------------------------------------------------------------------------------

/// Whether a read may migrate a legacy plaintext entry — and therefore whether it may touch the
/// cache at all.
///
/// *"Status-only reads deliberately bypass the cache because they do not migrate legacy entries"*:
/// a status read must never seed the cache with a value an ordinary read would have migrated and
/// normalized differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadBehavior {
    /// `behavior.migrateLegacy` absent or `true` — the ordinary read.
    Migrate,
    /// `behavior.migrateLegacy === false` — [`McpAuthStore::inspect_auth_for_url`]'s read.
    StatusOnly,
}

impl ReadBehavior {
    const fn migrates(self) -> bool {
        matches!(self, ReadBehavior::Migrate)
    }
}

/// The state one [`McpAuthStore`] owns. Behind an `Arc` so a `spawn_blocking` closure can hold it.
struct AuthStoreInner {
    /// The live backend, bound to [`AUTH_SECRET_SERVICE`].
    backend: Arc<dyn AuthSecretStore>,
    /// MCP-280's read-only importer, bound to [`LEGACY_AUTH_SECRET_SERVICE`].
    legacy_backend: Arc<dyn AuthSecretStore>,
    /// The `<agent_dir>` chain, for the default legacy base dir.
    dirs: McpDirs,
    /// The configured `settings.oauthDir`, already resolved against `cwd`.
    options: AuthStorageOptions,
    /// `authEntryCache: Map<serverName, AuthEntry | undefined>`. **Absence is cached** as an
    /// explicit `None`, distinguished from "not cached" by `contains_key`.
    cache: RwLock<HashMap<String, Option<AuthEntry>>>,
    /// Servers whose legacy-*service* import has already been attempted, so the second read does not
    /// touch [`LEGACY_AUTH_SECRET_SERVICE`] (MCP-280's "one-time").
    legacy_service_seen: RwLock<HashSet<String>>,
    /// MCP-268's per-server-name serialization.
    locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Read **per call**, never captured once: the suite's write-behind-the-cache helper depends on
    /// exactly that.
    env: EnvFn,
}

/// The credential vault: one object owning the backend, the cache and the legacy-import options.
///
/// Upstream's module-level `authEntryCache` and `memoryAuthEntries` amount to process-global state;
/// the port is **inherent methods on one struct**, not free functions over globals, so two sessions
/// in one process cannot collide and a test needs no reset hook it forgot to call.
///
/// `Clone` is cheap and shares state — every clone is the same vault.
#[derive(Clone)]
pub struct McpAuthStore {
    inner: Arc<AuthStoreInner>,
}

impl McpAuthStore {
    /// The production store: the real keychain under both services, `std::env` for the switches.
    #[must_use]
    pub fn new(dirs: McpDirs, options: AuthStorageOptions) -> Self {
        Self::from_env(dirs, options, process_env())
    }

    /// The production store with an injected environment — `getAuthSecretStore()`'s selector still
    /// applies, so `…_TEST_AUTH_STORE=memory` swaps both backends for in-memory ones.
    #[must_use]
    pub fn from_env(dirs: McpDirs, options: AuthStorageOptions, env: EnvFn) -> Self {
        let backend = select_backend(&env, AUTH_SECRET_SERVICE);
        let legacy_backend = select_backend(&env, LEGACY_AUTH_SECRET_SERVICE);
        Self::with_backends(backend, legacy_backend, dirs, options, env)
    }

    /// **The primary test seam** (MCP-258): constructor injection of the backend, with an empty
    /// legacy service.
    #[must_use]
    pub fn with_backend(
        backend: Arc<dyn AuthSecretStore>,
        dirs: McpDirs,
        options: AuthStorageOptions,
    ) -> Self {
        Self::with_backends(
            backend,
            Arc::new(MemorySecretStore::new()),
            dirs,
            options,
            process_env(),
        )
    }

    /// Both backends and the environment, for MCP-280's importer test and the recovery fixtures.
    #[must_use]
    pub fn with_backends(
        backend: Arc<dyn AuthSecretStore>,
        legacy_backend: Arc<dyn AuthSecretStore>,
        dirs: McpDirs,
        options: AuthStorageOptions,
        env: EnvFn,
    ) -> Self {
        Self {
            inner: Arc::new(AuthStoreInner {
                backend,
                legacy_backend,
                dirs,
                options,
                cache: RwLock::new(HashMap::new()),
                legacy_service_seen: RwLock::new(HashSet::new()),
                locks: StdMutex::new(HashMap::new()),
                env,
            }),
        }
    }

    /// The live backend, for the inspection hooks.
    #[must_use]
    pub fn backend(&self) -> &Arc<dyn AuthSecretStore> {
        &self.inner.backend
    }

    /// The read-only legacy-service backend (MCP-280).
    #[must_use]
    pub fn legacy_backend(&self) -> &Arc<dyn AuthSecretStore> {
        &self.inner.legacy_backend
    }

    /// `getAuthBaseDir(options)` — `$MCP_OAUTH_DIR` (trimmed, non-empty) outranks the configured
    /// dir, which outranks `<agent_dir>/mcp-oauth`.
    ///
    /// Public because `mcp-auth-flow.ts` folds it into two in-flight dedup keys; it is not dead once
    /// import is done.
    #[must_use]
    pub fn auth_base_dir(&self) -> PathBuf {
        crate::dirs::resolve_auth_base_dir(
            &self.inner.dirs,
            self.inner.options.base_dir.as_deref(),
            &|key| (self.inner.env)(key),
        )
    }

    /// `getAuthEntryFilePath(serverName, options)` — `<baseDir>/sha256-<hex>/tokens.json`.
    ///
    /// Public because two upstream test suites assert on it. The `sha256-<hex>` component is what
    /// makes an arbitrary configured name path-safe: for `"../escape"` the relative path is still
    /// `sha256-<64 hex>/tokens.json`, never `../escape/tokens.json`.
    #[must_use]
    pub fn auth_entry_file_path(&self, server_name: &str) -> PathBuf {
        self.auth_base_dir()
            .join(auth_entry_account(server_name))
            .join("tokens.json")
    }

    /// `getServerDir(serverName, options)`.
    fn server_dir(&self, server_name: &str) -> PathBuf {
        self.auth_base_dir().join(auth_entry_account(server_name))
    }

    /// `isAuthEntryCacheEnabled()` — enabled unless the switch is exactly `"1"`. Read **per call**
    /// (MCP-259).
    #[must_use]
    pub fn is_cache_enabled(&self) -> bool {
        !env_is_one(&self.inner.env, &AUTH_CACHE_DISABLED_ENV)
    }

    /// `resetAuthEntryCache()` — clears the cache **only**, leaving the backend read counter alone.
    /// `resetTestAuthSecretStore()` is this plus [`MemorySecretStore::reset`], and the two differing
    /// is pinned upstream.
    ///
    /// **No production caller, deliberately:** upstream exports `resetAuthEntryCache` from its test
    /// surface only. The production invalidation points are per-server and targeted —
    /// [`Self::invalidate_cache`], called from the connect loop's 401 handling and from session
    /// recovery — because a blanket clear would evict entries for servers that never saw a 401 and
    /// turn one server's auth failure into a keychain read storm across every other one. Keep this
    /// for the between-cases reset that a shared process-lifetime cache otherwise leaks across.
    pub fn reset_cache(&self) {
        self.write_cache().clear();
        self.inner
            .legacy_service_seen
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// `invalidateAuthEntryCache(serverName)` — forget one entry so the next ordinary read reloads
    /// secure storage.
    ///
    /// Called from `server-manager.ts` twice (connection setup got a 401 on an OAuth-capable server;
    /// the connect loop got a 401) and from `session-recovery.ts` once (an in-flight call got HTTP
    /// 401). All three gate on `supportsOAuth(definition)`, and the two `server-manager` sites guard
    /// with an `invalidated` boolean so one connect attempt invalidates at most once — that
    /// **policy** belongs to the server-manager section; this owns only the eviction primitive.
    /// Harmless while the cache is disabled.
    pub fn invalidate_cache(&self, server_name: &str) {
        self.write_cache().remove(server_name);
    }

    fn read_cache(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, Option<AuthEntry>>> {
        self.inner
            .cache
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_cache(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, Option<AuthEntry>>> {
        self.inner
            .cache
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The per-server-name `tokio::sync::Mutex` (MCP-268), created on first use.
    ///
    /// Upstream's mutators are read → mutate → write with **no** lock: within one process the JS
    /// event loop makes the sequence atomic because every function in `mcp-auth.ts` is synchronous.
    /// That guarantee does not survive the port — `keyring` calls are blocking syscalls issued from
    /// a multi-threaded tokio runtime and rmcp's `CredentialStore` is `async_trait` with `&self`, so
    /// `AuthorizationManager` calls `save` from whichever task refreshed. This restores exactly what
    /// the language previously supplied and claims nothing upstream did not have. Deliberately
    /// **not** a cross-process lock: the store is a keychain, there is no file to lock, and
    /// inventing a lock file would add an on-disk artifact upstream has no counterpart for.
    fn server_lock(&self, server_name: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .inner
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            locks
                .entry(server_name.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }
}

// ---------------------------------------------------------------------------------------------
// §6.4 / §6.8 — read, write, remove: the full call sequences
// ---------------------------------------------------------------------------------------------

/// `parseJsonPayload(serverName, payload, source)`.
fn parse_json_payload(
    server: &str,
    payload: &str,
    source_label: &str,
) -> Result<serde_json::Value, AuthStoreError> {
    serde_json::from_str(payload).map_err(|source| AuthStoreError::ParseJson {
        server: server.to_string(),
        source_label: source_label.to_string(),
        source,
    })
}

/// `parseAuthEntryPayload`'s second half — `toAuthEntry(parsed)`.
///
/// serde's typed deserialization gives all four normalization rules for free:
/// unknown keys are dropped (no `deny_unknown_fields`); a wrong-typed *optional* field poisons the
/// whole entry; a missing *required* field does the same; and a JSON array is rejected exactly as a
/// scalar is. The one exception, `redirectUris`, is [`deserialize_lenient_string_array`].
fn auth_entry_from_value(
    server: &str,
    value: serde_json::Value,
    source_label: &str,
) -> Result<AuthEntry, AuthStoreError> {
    serde_json::from_value(value).map_err(|_| AuthStoreError::ParseShape {
        server: server.to_string(),
        source_label: source_label.to_string(),
    })
}

impl McpAuthStore {
    /// `readExistingChunkManifest(store, serverName, account)` — a **swallow-all** read: any error,
    /// and any payload that is not a manifest, yields `None`.
    fn existing_chunk_manifest(
        store: &dyn AuthSecretStore,
        account: &str,
    ) -> Option<AuthEntryChunkManifest> {
        let payload = store.read(account).ok()??;
        let value = serde_json::from_str::<serde_json::Value>(&payload).ok()?;
        parse_chunk_manifest(&value)
    }

    /// `removeChunkPayloads` — the **non**-swallowing variant, used only by the remove path
    /// (MCP-285). Deliberately not shared with [`Self::try_remove_chunk_payloads`].
    fn remove_chunk_payloads(
        store: &dyn AuthSecretStore,
        account: &str,
        manifest: &AuthEntryChunkManifest,
    ) -> Result<(), AuthSecretStoreError> {
        for chunk_account in manifest.chunk_accounts(account) {
            store.remove(&chunk_account)?;
        }
        Ok(())
    }

    /// `tryRemoveChunkPayloads` — the swallowing variant, used only by the write path:
    /// *"Stale chunk cleanup must not hide a successful credential write."*
    fn try_remove_chunk_payloads(
        store: &dyn AuthSecretStore,
        account: &str,
        manifest: Option<&AuthEntryChunkManifest>,
    ) {
        let Some(manifest) = manifest else { return };
        let _ = Self::remove_chunk_payloads(store, account, manifest);
    }

    /// `writeSecureAuthEntryToStore(store, serverName, entry)` — the write algorithm, in order.
    ///
    /// 1. `account = getAuthEntryAccount(serverName)`.
    /// 2. `payload = JSON.stringify(entry)` — **compact**, no whitespace (MCP-275). The comment
    ///    states why: *"Compact: multiline secrets corrupt gnome-keyring plaintext (GKeyFile)
    ///    collections."* On that backend a multi-line value corrupts the collection file, losing
    ///    every credential in it — not just this one. Note the contrasting in-tree convention:
    ///    `cyrup_config::auth::AuthStore` deliberately uses `to_string_pretty` for `auth.json`.
    ///    Copying that habit here is the failure mode.
    /// 3. read the previous manifest (swallow-all).
    /// 4. decide whether to chunk: `payload.len() > 1000`.
    /// 5. if chunking, write the chunks **first** and the manifest **last**. *Order is load-bearing:*
    ///    a crash between them leaves orphan chunks while the base account still holds the previous
    ///    good value, so reads stay consistent.
    /// 6. else write the payload at the base account.
    /// 7. on a digest change, remove the *previous* chunks, best-effort. When the digest is
    ///    unchanged the old chunk accounts **are** the new ones, so skipping cleanup is correct, not
    ///    lazy. Both-`None` is a no-op.
    /// 8. on any failure in 5–7, remove the **new** chunks and return
    ///    [`AuthStoreError::Unavailable`] with [`StoreOp::Write`].
    /// 9. on success only, publish to the cache — this sits *outside* the fallible region, so a
    ///    failed write never publishes.
    fn write_secure_auth_entry_to_store(
        &self,
        store: &dyn AuthSecretStore,
        server_name: &str,
        entry: &AuthEntry,
    ) -> Result<(), AuthStoreError> {
        let account = auth_entry_account(server_name);
        let payload =
            serde_json::to_string(entry).map_err(|source| AuthStoreError::ParseJson {
                server: server_name.to_string(),
                source_label: STORE_SOURCE.to_string(),
                source,
            })?;
        debug_assert!(
            !payload.contains('\n'),
            "a stored secret must never contain a newline (MCP-275)"
        );

        let previous = Self::existing_chunk_manifest(store, &account);
        let manifest = (payload.len() > AUTH_SECRET_CHUNK_SIZE)
            .then(|| AuthEntryChunkManifest::for_payload(&payload));

        let attempt = || -> Result<(), AuthSecretStoreError> {
            match &manifest {
                Some(manifest) => {
                    for (index, chunk) in split_payload(&payload, manifest.chunk_count)
                        .into_iter()
                        .enumerate()
                    {
                        store.write(
                            &auth_entry_chunk_account(&account, &manifest.chunk_digest, index),
                            chunk,
                        )?;
                    }
                    let body = serde_json::to_string(manifest).unwrap_or_default();
                    store.write(&account, &body)?;
                }
                None => store.write(&account, &payload)?,
            }
            let previous_digest = previous.as_ref().map(|m| m.chunk_digest.as_str());
            let next_digest = manifest.as_ref().map(|m| m.chunk_digest.as_str());
            if previous_digest != next_digest {
                Self::try_remove_chunk_payloads(store, &account, previous.as_ref());
            }
            Ok(())
        };

        if let Err(source) = attempt() {
            Self::try_remove_chunk_payloads(store, &account, manifest.as_ref());
            return Err(AuthStoreError::Unavailable {
                operation: StoreOp::Write,
                server: server_name.to_string(),
                source,
            });
        }

        self.publish_to_cache(server_name, &payload);
        Ok(())
    }

    /// `publishAuthEntryToCache(serverName, payload)` — the payload just written is **re-parsed and
    /// re-normalized**, so the cache holds *the shape a fresh store read would return*, not the
    /// caller's object. If normalization fails the entry is **deleted**, not set.
    ///
    /// Gated on the enable flag, which is what makes the suite's write-behind-the-cache helper work.
    fn publish_to_cache(&self, server_name: &str, payload: &str) {
        if !self.is_cache_enabled() {
            return;
        }
        match serde_json::from_str::<AuthEntry>(payload) {
            Ok(normalized) => {
                self.write_cache()
                    .insert(server_name.to_string(), Some(normalized));
            }
            Err(_) => {
                self.write_cache().remove(server_name);
            }
        }
    }

    /// `readChunkedAuthEntry` — read each chunk account in index order, join, parse.
    ///
    /// A chunk that is absent throws `Missing OAuth credential chunk … for …` **wrapped** in
    /// [`StoreOp::Read`]'s `Unavailable`: the upstream test pins that a deleted chunk yields
    /// `inspectAuthForUrl(...).status === "unavailable"`, **not** `"absent"` — a partially lost
    /// credential must never look like "no credential".
    ///
    /// The final parse is deliberately **outside** the wrapping (MCP-284).
    fn read_chunked_auth_entry(
        store: &dyn AuthSecretStore,
        server_name: &str,
        account: &str,
        manifest: &AuthEntryChunkManifest,
    ) -> Result<AuthEntry, AuthStoreError> {
        let mut joined = String::new();
        for chunk_account in manifest.chunk_accounts(account) {
            let wrap = |source: AuthSecretStoreError| AuthStoreError::Unavailable {
                operation: StoreOp::Read,
                server: server_name.to_string(),
                source,
            };
            let chunk = store.read(&chunk_account).map_err(wrap)?.ok_or_else(|| {
                AuthStoreError::Unavailable {
                    operation: StoreOp::Read,
                    server: server_name.to_string(),
                    source: AuthSecretStoreError::MissingChunk(
                        AuthStoreError::MissingChunk {
                            chunk_account: chunk_account.clone(),
                            server: server_name.to_string(),
                        }
                        .to_string(),
                    ),
                }
            })?;
            joined.push_str(&chunk);
        }
        let value = parse_json_payload(server_name, &joined, STORE_CHUNKS_SOURCE)?;
        auth_entry_from_value(server_name, value, STORE_CHUNKS_SOURCE)
    }

    /// `readLegacyAuthEntry(serverName, options)` — `existsSync` → `readFileSync` → strict parse with
    /// the **file path** as the `source` label.
    fn read_legacy_auth_entry(
        &self,
        server_name: &str,
    ) -> Result<Option<AuthEntry>, AuthStoreError> {
        let path = self.auth_entry_file_path(server_name);
        if !path.exists() {
            return Ok(None);
        }
        let label = path.display().to_string();
        let data = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            // `readFileSync` throwing after `existsSync` passed is a race; upstream would propagate
            // a bare Error. A file that vanished between the two is "absent", which is the only
            // reading that does not turn a benign race into a fatal.
            Err(_) => return Ok(None),
        };
        let value = parse_json_payload(server_name, &data, &label)?;
        let legacy: LegacyAuthEntry =
            serde_json::from_value(value).map_err(|_| AuthStoreError::ParseShape {
                server: server_name.to_string(),
                source_label: label.clone(),
            })?;
        Ok(translate_legacy_entry(legacy, now_secs() as f64))
    }

    /// `removeLegacyAuthEntry(serverName, options)` — the deletion asymmetry is **mandatory**.
    ///
    /// Failing to delete the plaintext *secret* is fatal; failing to delete its *directory* is
    /// swallowed, because *"Directory may contain future non-secret metadata; the plaintext file was
    /// already removed."*
    fn remove_legacy_auth_entry(&self, server_name: &str) -> Result<(), AuthStoreError> {
        let path = self.auth_entry_file_path(server_name);
        if !path.exists() {
            return Ok(());
        }
        if let Err(source) = std::fs::remove_file(&path) {
            // `rmSync(file, { force: true })` treats "already gone" as success.
            if source.kind() != std::io::ErrorKind::NotFound {
                return Err(AuthStoreError::LegacyRemove {
                    server: server_name.to_string(),
                    path,
                    source,
                });
            }
        }
        let _ = std::fs::remove_dir_all(self.server_dir(server_name));
        Ok(())
    }

    /// MCP-280's one-time, read-only import from a co-installed `pi-mcp-adapter`.
    ///
    /// On a cold read the store looks at the **same account** under
    /// [`LEGACY_AUTH_SECRET_SERVICE`], translates the v2.25.0 record, writes the result under
    /// [`AUTH_SECRET_SERVICE`] — and, **unlike the legacy file case, does not delete the source**. A
    /// keychain entry is not a plaintext leak, and deleting it would break the co-installed install
    /// for no security benefit.
    ///
    /// Every failure is swallowed: a legacy entry that fails translation must leave **both** services
    /// untouched rather than half-written. The attempt is recorded either way, so the second read
    /// does not touch the legacy service.
    fn import_from_legacy_service(&self, server_name: &str) -> Option<AuthEntry> {
        {
            let seen = self
                .inner
                .legacy_service_seen
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if seen.contains(server_name) {
                return None;
            }
        }
        self.inner
            .legacy_service_seen
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(server_name.to_string());

        // Every failure below is `.ok()?` — a co-installed upstream record that is unreadable,
        // chunked or wrong-shaped degrades to **absent**, never to an error. That is the whole
        // point of the migration path: a broken *legacy* record must not be able to block a login
        // under the new service. It is also why there is no `"legacy OS secure credential store"`
        // source label in [`AuthStoreError`] — this reader never produces one.
        let account = auth_entry_account(server_name);
        let payload = self.inner.legacy_backend.read(&account).ok()??;
        let value = serde_json::from_str::<serde_json::Value>(&payload).ok()?;
        // A chunked legacy entry is out of scope for the importer: upstream's chunk accounts carry
        // upstream's record, and reassembling them here would mean re-implementing the old reader.
        // Leaving it alone is the "both services untouched" outcome.
        if parse_chunk_manifest(&value).is_some() {
            return None;
        }
        let legacy: LegacyAuthEntry = serde_json::from_value(value).ok()?;
        let entry = translate_legacy_entry(legacy, now_secs() as f64)?;
        // Write-through under the new service. A failure leaves the legacy service untouched, which
        // is the required outcome; the entry is still returned so this session works.
        let _ = self.write_secure_auth_entry_to_store(&*self.inner.backend, server_name, &entry);
        let _ = self.remove_legacy_auth_entry(server_name);
        Some(entry)
    }
}

impl McpAuthStore {
    /// `readAuthEntryFromStore(store, serverName, options, behavior)` — §6.8's five steps.
    ///
    /// Note step 2b: **`removeLegacyAuthEntry` runs even on a pure read, and even under
    /// `migrateLegacy: false`.** A plaintext file must not survive once the keychain holds the
    /// record. That is why inspection is non-destructive only for a server that has *no* keychain
    /// entry yet.
    ///
    /// Note also which throws are wrapped: **only** the backend `read` is inside
    /// [`StoreOp::Read`]'s `Unavailable`. The subsequent manifest parse and entry parse run outside
    /// it, so a corrupt base payload surfaces as [`AuthStoreError::ParseJson`] /
    /// [`AuthStoreError::ParseShape`] and `inspect_auth_for_url` **rethrows** rather than degrading
    /// to `Unavailable` (MCP-284). The remove path wraps the same parse, deliberately.
    fn read_auth_entry_from_store(
        &self,
        store: &dyn AuthSecretStore,
        server_name: &str,
        behavior: ReadBehavior,
    ) -> Result<Option<AuthEntry>, AuthStoreError> {
        let account = auth_entry_account(server_name);
        let payload =
            store
                .read(&account)
                .map_err(|source| AuthStoreError::Unavailable {
                    operation: StoreOp::Read,
                    server: server_name.to_string(),
                    source,
                })?;

        if let Some(payload) = payload {
            let value = parse_json_payload(server_name, &payload, STORE_SOURCE)?;
            let entry = match parse_chunk_manifest(&value) {
                Some(manifest) => {
                    Self::read_chunked_auth_entry(store, server_name, &account, &manifest)?
                }
                None => auth_entry_from_value(server_name, value, STORE_SOURCE)?,
            };
            self.remove_legacy_auth_entry(server_name)?;
            return Ok(Some(entry));
        }

        // MCP-280 — the co-installed upstream service, once per server, read-only.
        if let Some(entry) = self.import_from_legacy_service(server_name) {
            return Ok(Some(entry));
        }

        let Some(legacy_entry) = self.read_legacy_auth_entry(server_name)? else {
            return Ok(None);
        };
        if !behavior.migrates() {
            // What makes status inspection non-destructive *for a server that has no keychain entry
            // yet*: return the legacy entry without writing or deleting.
            return Ok(Some(legacy_entry));
        }
        self.write_secure_auth_entry_to_store(store, server_name, &legacy_entry)?;
        self.remove_legacy_auth_entry(server_name)?;
        Ok(Some(legacy_entry))
    }

    /// `readAuthEntry(serverName, options, behavior)` — the cache and the one recovery retry.
    ///
    /// Cacheable **only** when the read migrates *and* the cache is enabled. Store failures are
    /// never cached: two consecutive throwing reads both throw and leave no entry, so a later working
    /// read returns the true value rather than a poisoned one.
    fn read_auth_entry(
        &self,
        server_name: &str,
        behavior: ReadBehavior,
    ) -> Result<Option<AuthEntry>, AuthStoreError> {
        let cacheable = behavior.migrates() && self.is_cache_enabled();
        if cacheable {
            // `.clone()` on the way out and on the way in: an owned `AuthEntry` returned **by value**
            // gives upstream's `structuredClone` isolation for free. The hazard reappears the moment
            // this hands out `&AuthEntry`, `Arc<AuthEntry>` or a `Cow`, at nested-field granularity
            // and in both directions.
            if let Some(cached) = self.read_cache().get(server_name) {
                return Ok(cached.clone());
            }
        }

        let entry = match self.read_auth_entry_from_store_dispatch(server_name, behavior, false) {
            Ok(entry) => entry,
            Err(error) => {
                if !should_attempt_recovery(&self.inner.env, &error) {
                    return Err(error);
                }
                self.read_auth_entry_from_store_dispatch(server_name, behavior, true)?
            }
        };

        if cacheable {
            self.write_cache()
                .insert(server_name.to_string(), entry.clone());
        }
        Ok(entry)
    }

    /// Dispatch a read at either the ordinary backend or the recovery store.
    fn read_auth_entry_from_store_dispatch(
        &self,
        server_name: &str,
        behavior: ReadBehavior,
        recovery: bool,
    ) -> Result<Option<AuthEntry>, AuthStoreError> {
        if recovery {
            let store = LinuxKeyringRecoveryStore::new(
                AUTH_SECRET_SERVICE,
                Arc::clone(&self.inner.env),
            );
            self.read_auth_entry_from_store(&store, server_name, behavior)
        } else {
            self.read_auth_entry_from_store(&*self.inner.backend, server_name, behavior)
        }
    }

    /// `writeSecureAuthEntry(serverName, entry)` — the ordinary store, then one recovery retry.
    fn write_secure_auth_entry(
        &self,
        server_name: &str,
        entry: &AuthEntry,
    ) -> Result<(), AuthStoreError> {
        match self.write_secure_auth_entry_to_store(&*self.inner.backend, server_name, entry) {
            Ok(()) => Ok(()),
            Err(error) => {
                if !should_attempt_recovery(&self.inner.env, &error) {
                    return Err(error);
                }
                let store =
                    LinuxKeyringRecoveryStore::new(AUTH_SECRET_SERVICE, Arc::clone(&self.inner.env));
                self.write_secure_auth_entry_to_store(&store, server_name, entry)
            }
        }
    }

    /// `removeAuthEntryFromStore(store, serverName)` — read the base account; if it parses as a
    /// manifest, remove **all** chunk accounts with the *non*-best-effort variant, then remove the
    /// base account.
    ///
    /// The whole body is wrapped, so identical corruption that the read path reports as a bare parse
    /// error becomes `Unavailable { operation: Remove }` here (MCP-284).
    ///
    /// The asymmetry against the write path is deliberate (MCP-285): a single failing chunk delete
    /// aborts before the base account is removed, so the base keeps its manifest while some chunks
    /// are gone. That leaves a credential that reads as `unavailable` and cannot be cleared by
    /// retrying if the same chunk keeps failing — and the alternative, deleting the base first,
    /// would orphan chunks holding a live token.
    fn remove_auth_entry_from_store(
        &self,
        store: &dyn AuthSecretStore,
        server_name: &str,
    ) -> Result<(), AuthStoreError> {
        let account = auth_entry_account(server_name);
        let wrap = |source: AuthSecretStoreError| AuthStoreError::Unavailable {
            operation: StoreOp::Remove,
            server: server_name.to_string(),
            source,
        };
        let attempt = || -> Result<(), AuthSecretStoreError> {
            let payload = store.read(&account)?;
            if let Some(payload) = payload {
                let manifest = serde_json::from_str::<serde_json::Value>(&payload)
                    .ok()
                    .as_ref()
                    .and_then(parse_chunk_manifest);
                if let Some(manifest) = manifest {
                    Self::remove_chunk_payloads(store, &account, &manifest)?;
                }
            }
            store.remove(&account)
        };
        attempt().map_err(wrap)
    }

    // ---------------------------------------------------------------------------------------
    // The production accessor surface (§6.9)
    // ---------------------------------------------------------------------------------------

    /// `getAuthEntry(serverName, options)` — the **unvalidated-for-URL** read. Every mutator bases
    /// itself on this, not on [`Self::auth_for_url`], so the URL comparison is against whatever URL
    /// is stored.
    pub fn auth_entry(&self, server_name: &str) -> Result<Option<AuthEntry>, AuthStoreError> {
        self.read_auth_entry(server_name, ReadBehavior::Migrate)
    }

    /// `getAuthForUrl(serverName, serverUrl, options)` — the **fail-closed** accessor.
    ///
    /// `None` when there is no entry, when `entry.serverUrl` is absent (*"If no serverUrl is stored,
    /// this is from an old version - consider it invalid"*), or when it differs. String equality,
    /// **no URL normalization** — a trailing-slash change invalidates the credential, and widening
    /// that would present a credential minted for one authorization server to a different one.
    pub fn auth_for_url(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> Result<Option<AuthEntry>, AuthStoreError> {
        Ok(self
            .auth_entry(server_name)?
            .filter(|entry| entry.matches_url(server_url)))
    }

    /// `inspectAuthForUrl(serverName, serverUrl, options)` — the status accessor (MCP-265).
    ///
    /// Reads with `migrateLegacy: false` (so: no cache read, no cache write, no migration write),
    /// maps "no entry / no `serverUrl` / URL mismatch" to [`OAuthCredentialStatus::Absent`], and
    /// converts an [`AuthStoreError::Unavailable`] — and **only** that class — into
    /// [`OAuthCredentialStatus::Unavailable`]. Everything else propagates, including a parse failure
    /// on the stored payload (MCP-284).
    /// **No production caller.** `inspect_auth_for_url_async`, the wrapper that had one, was
    /// deleted in the uncalled-machinery sweep; this synchronous half survives it deliberately.
    /// Three other declarations explain themselves in terms of it — [`AuthEntry`]'s rejection pair
    /// and the `migrateLegacy` read — so deleting it would cost those explanations their referent,
    /// and it is the only entry point that surfaces a store read WITHOUT the refresh side effects
    /// [`crate::oauth::get_valid_token`] carries. Wire it, do not re-derive it, if a caller needs a
    /// plain read.
    pub fn inspect_auth_for_url(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> Result<OAuthCredentialStatus, AuthStoreError> {
        match self.read_auth_entry(server_name, ReadBehavior::StatusOnly) {
            Ok(Some(entry)) if entry.matches_url(server_url) => {
                Ok(OAuthCredentialStatus::Present(entry))
            }
            Ok(_) => Ok(OAuthCredentialStatus::Absent),
            Err(error) if error.is_store_unavailable() => Ok(OAuthCredentialStatus::Unavailable {
                message: format_oauth_credential_store_unavailable(&error),
            }),
            Err(error) => Err(error),
        }
    }

    /// `saveAuthEntry(serverName, entry, serverUrl?, options?)`.
    ///
    /// `serverUrl` is set as a **side effect on the caller's object** when supplied, which is why the
    /// entry is `&mut` — upstream mutates the caller's record and later reads depend on it. The
    /// truthiness test is upstream's: an empty string does not rewrite the binding.
    pub fn save_auth_entry(
        &self,
        server_name: &str,
        entry: &mut AuthEntry,
        server_url: Option<&str>,
    ) -> Result<(), AuthStoreError> {
        if let Some(url) = server_url.filter(|url| !url.is_empty()) {
            entry.server_url = Some(url.to_string());
        }
        self.write_secure_auth_entry(server_name, entry)?;
        self.remove_legacy_auth_entry(server_name)
    }

    /// `removeAuthEntry(serverName, options)` — store, then cache, then legacy file.
    ///
    /// **The ordering is upstream's and is not a bug to fix silently**: the cache is purged *after*
    /// the store op, so a throwing remove leaves the cache intact and the next read still serves the
    /// old value. The eviction itself is **not** behind the cache-enable flag — a removal performed
    /// while the cache is off still evicts.
    pub fn remove_auth_entry(&self, server_name: &str) -> Result<(), AuthStoreError> {
        match self.remove_auth_entry_from_store(&*self.inner.backend, server_name) {
            Ok(()) => Ok(()),
            Err(error) => {
                if !should_attempt_recovery(&self.inner.env, &error) {
                    return Err(error);
                }
                let store =
                    LinuxKeyringRecoveryStore::new(AUTH_SECRET_SERVICE, Arc::clone(&self.inner.env));
                self.remove_auth_entry_from_store(&store, server_name)
            }
        }?;
        self.write_cache().remove(server_name);
        self.remove_legacy_auth_entry(server_name)
    }
}

// ---------------------------------------------------------------------------------------------
// MCP-264 (critical) — the mutators' sibling-purge rule
// ---------------------------------------------------------------------------------------------

/// Which slot a mutator is writing. The purge set is always "all the others".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Credentials,
    Client,
    State,
}

impl McpAuthStore {
    /// **The four mutators share one algorithm** — read, conditionally purge siblings, set one
    /// field, save.
    ///
    /// | mutator | sets | deletes when `serverUrl && entry.serverUrl !== serverUrl` |
    /// |---|---|---|
    /// | `updateTokens` | `tokens` | `clientInfo`, `codeVerifier`, `oauthState` |
    /// | `updateClientInfo` | `clientInfo` | `tokens`, `codeVerifier`, `oauthState` |
    /// | `updateCodeVerifier` / `updateOAuthState` | the PKCE slot | `tokens`, `clientInfo` |
    ///
    /// In every case the deleted set is "all fields except the one being written". **The defence:** a
    /// server whose URL changed is a different authorization context, so no artifact from the old one
    /// may survive alongside a new one. Getting this wrong presents a credential minted for one
    /// authorization server to a different one, and re-uses a PKCE verifier across authorization
    /// contexts — a permission bypass and a credential disclosure, silently.
    ///
    /// Note this is **orthogonal** to rmcp's own defence:
    /// `AuthorizationManager::initialize_from_store` fences on the **issuer** changing (clearing the
    /// store, or keeping a portable CIMD client id and discarding tokens); this fences on the **MCP
    /// server URL** changing, which rmcp cannot see. Both are required and neither subsumes the
    /// other.
    ///
    /// The base is the *unvalidated* [`Self::auth_entry`] (upstream's `getAuthEntry(...) ?? {}`), so
    /// the comparison is against whatever URL is stored.
    fn mutate(
        &self,
        server_name: &str,
        server_url: Option<&str>,
        slot: Slot,
        apply: impl FnOnce(&mut AuthEntry),
    ) -> Result<(), AuthStoreError> {
        let mut entry = self.auth_entry(server_name)?.unwrap_or_default();
        let rebinding = server_url
            .filter(|url| !url.is_empty())
            .is_some_and(|url| entry.server_url.as_deref() != Some(url));
        if rebinding {
            if slot != Slot::Credentials {
                entry.credentials = None;
            }
            if slot != Slot::Client {
                entry.client = None;
            }
            if slot != Slot::State {
                entry.state = None;
            }
        }
        apply(&mut entry);
        self.save_auth_entry(server_name, &mut entry, server_url)
    }

    /// A clearer: read, drop one slot, save with **`serverUrl: undefined`** so a clear never
    /// rewrites the stored URL. No-ops when there is no entry.
    fn clear_slot(
        &self,
        server_name: &str,
        apply: impl FnOnce(&mut AuthEntry),
    ) -> Result<(), AuthStoreError> {
        let Some(mut entry) = self.auth_entry(server_name)? else {
            return Ok(());
        };
        apply(&mut entry);
        self.save_auth_entry(server_name, &mut entry, None)
    }

    /// `updateTokens(serverName, tokens, serverUrl?, options?)`, in the port's record shape.
    pub fn update_credentials(
        &self,
        server_name: &str,
        credentials: StoredCredentials,
        server_url: Option<&str>,
    ) -> Result<(), AuthStoreError> {
        self.mutate(server_name, server_url, Slot::Credentials, move |entry| {
            entry.credentials = Some(credentials);
        })
    }

    /// `updateClientInfo(serverName, clientInfo, serverUrl?, options?)`.
    pub fn update_client_info(
        &self,
        server_name: &str,
        client: StoredClientInfo,
        server_url: Option<&str>,
    ) -> Result<(), AuthStoreError> {
        self.mutate(server_name, server_url, Slot::Client, move |entry| {
            entry.client = Some(client);
        })
    }

    /// `updateCodeVerifier` + `updateOAuthState`, collapsed: rmcp keeps the PKCE verifier and the
    /// CSRF token in **one** [`StoredAuthorizationState`], so upstream's two mutators — which always
    /// ran back to back against the same authorization round — become one.
    pub fn update_state(
        &self,
        server_name: &str,
        state: StoredAuthorizationState,
        server_url: Option<&str>,
    ) -> Result<(), AuthStoreError> {
        self.mutate(server_name, server_url, Slot::State, move |entry| {
            entry.state = Some(state);
        })
    }

    /// `clearTokens(serverName, options)`.
    pub fn clear_credentials(&self, server_name: &str) -> Result<(), AuthStoreError> {
        self.clear_slot(server_name, |entry| entry.credentials = None)
    }

    /// `clearClientInfo(serverName, options)`.
    pub fn clear_client_info(&self, server_name: &str) -> Result<(), AuthStoreError> {
        self.clear_slot(server_name, |entry| entry.client = None)
    }

    /// `clearCodeVerifier` + `clearOAuthState`, collapsed with [`Self::update_state`].
    pub fn clear_state(&self, server_name: &str) -> Result<(), AuthStoreError> {
        self.clear_slot(server_name, |entry| entry.state = None)
    }

    /// Drop **every** issuer-bound artifact — credentials, DCR client record and PKCE state — while
    /// keeping the `serverUrl` binding.
    ///
    /// This is what rmcp's [`rmcp::transport::auth::CredentialStore::clear`] means: *this
    /// authorization context is invalid*. Clearing only the tokens would leave a `client` registered
    /// at the **old** issuer usable against a new one, which is the same hazard MCP-264 guards on
    /// the URL axis.
    pub fn clear_authorization_context(&self, server_name: &str) -> Result<(), AuthStoreError> {
        self.clear_slot(server_name, |entry| {
            entry.credentials = None;
            entry.client = None;
            entry.state = None;
        })
    }
}

// ---------------------------------------------------------------------------------------------
// MCP-268 — the serialized, off-reactor half of the surface
// ---------------------------------------------------------------------------------------------

impl McpAuthStore {
    /// Run `op` under this server's lock, on the blocking pool.
    ///
    /// Two things at once, and both are required. The lock is MCP-268: two concurrent refreshes of
    /// one server must not lose a rotated refresh token, and rmcp calls `save` from whichever task
    /// performed the refresh. The `spawn_blocking` is MCP-291: `keyring` calls are blocking syscalls
    /// and the recovery hop is a subprocess wait, neither of which may run on a reactor thread.
    async fn locked<T, F>(&self, server_name: &str, op: F) -> Result<T, AuthStoreError>
    where
        F: FnOnce(McpAuthStore) -> Result<T, AuthStoreError> + Send + 'static,
        T: Send + 'static,
    {
        let lock = self.server_lock(server_name);
        let _guard = lock.lock().await;
        let store = self.clone();
        tokio::task::spawn_blocking(move || op(store))
            .await
            .map_err(|error| AuthStoreError::Internal(error.to_string()))?
    }

    /// [`Self::auth_entry`], serialized and off the reactor.
    pub async fn auth_entry_async(
        &self,
        server_name: &str,
    ) -> Result<Option<AuthEntry>, AuthStoreError> {
        let name = server_name.to_string();
        self.locked(server_name, move |store| store.auth_entry(&name))
            .await
    }

    /// [`Self::auth_for_url`], serialized and off the reactor. The fail-closed accessor every
    /// authentication path uses.
    pub async fn auth_for_url_async(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> Result<Option<AuthEntry>, AuthStoreError> {
        let name = server_name.to_string();
        let url = server_url.to_string();
        self.locked(server_name, move |store| store.auth_for_url(&name, &url))
            .await
    }

    /// [`Self::update_credentials`], serialized: the whole read-modify-write is inside the lock.
    pub async fn update_credentials_async(
        &self,
        server_name: &str,
        credentials: StoredCredentials,
        server_url: Option<&str>,
    ) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        let url = server_url.map(str::to_string);
        self.locked(server_name, move |store| {
            store.update_credentials(&name, credentials, url.as_deref())
        })
        .await
    }

    /// [`Self::update_client_info`], serialized.
    pub async fn update_client_info_async(
        &self,
        server_name: &str,
        client: StoredClientInfo,
        server_url: Option<&str>,
    ) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        let url = server_url.map(str::to_string);
        self.locked(server_name, move |store| {
            store.update_client_info(&name, client, url.as_deref())
        })
        .await
    }

    /// [`Self::update_state`], serialized.
    pub async fn update_state_async(
        &self,
        server_name: &str,
        state: StoredAuthorizationState,
        server_url: Option<&str>,
    ) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        let url = server_url.map(str::to_string);
        self.locked(server_name, move |store| {
            store.update_state(&name, state, url.as_deref())
        })
        .await
    }

    /// [`Self::clear_authorization_context`], serialized.
    pub async fn clear_authorization_context_async(
        &self,
        server_name: &str,
    ) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        self.locked(server_name, move |store| {
            store.clear_authorization_context(&name)
        })
        .await
    }

    /// [`Self::clear_state`], serialized.
    pub async fn clear_state_async(&self, server_name: &str) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        self.locked(server_name, move |store| store.clear_state(&name))
            .await
    }

    /// [`Self::remove_auth_entry`], serialized.
    pub async fn remove_auth_entry_async(&self, server_name: &str) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        self.locked(server_name, move |store| store.remove_auth_entry(&name))
            .await
    }
}

// ---------------------------------------------------------------------------------------------
// MCP-291 — `rmcp::transport::auth::{CredentialStore, StateStore}` over the keychain
// ---------------------------------------------------------------------------------------------

/// rmcp's credential store for **one** MCP server.
///
/// [`rmcp::transport::auth::CredentialStore`] takes **no key**, so `cyrup-mcp` instantiates one
/// store per server, bound to that server's account. That is the natural shape, not a workaround: it
/// is what makes [`McpAuthStore`]'s server-name keying line up with rmcp's keyless trait.
///
/// Handing it over is `AuthorizationManager::set_credential_store`; rmcp's
/// `InMemoryCredentialStore` remains the default for tests that do not want a keychain.
///
/// **No installer, deliberately — and this is not a missing call site.** Nothing in the crate calls
/// `set_credential_store`, because production HTTP auth does not run through rmcp's
/// `AuthorizationManager` at all: [`crate::runtime`] builds a
/// [`crate::runtime::HttpAuthProvider`] — `StoredCredentialAuth`, `runtime.rs:235-245` — and hands
/// *that* to the connection builder, so the token is read from [`McpAuthStore`] directly and this
/// adapter is never consulted. The `AuthorizationManager` path is section 05's `AuthClient` work
/// (`runtime.rs:1620`, `runtime.rs:1975`); MCP-291 built the two stores ahead of it precisely so
/// that landing `AuthClient` is a `set_credential_store` / `set_state_store` pair and not a new
/// persistence layer. Wire this only as part of that unit — installing it beside
/// `StoredCredentialAuth` would give one server two independent readers of the same keychain slot.
pub struct McpCredentialStore {
    store: McpAuthStore,
    server_name: String,
    server_url: Option<String>,
}

impl McpCredentialStore {
    /// A store bound to one server. Pass `server_url` to keep the fail-closed URL binding
    /// (MCP-264) on rmcp's path too; `None` reads unvalidated, which is only correct before the
    /// URL is known.
    #[must_use]
    pub fn new(
        store: McpAuthStore,
        server_name: impl Into<String>,
        server_url: Option<String>,
    ) -> Self {
        Self {
            store,
            server_name: server_name.into(),
            server_url,
        }
    }
}

#[async_trait::async_trait]
impl rmcp::transport::auth::CredentialStore for McpCredentialStore {
    /// A store failure arrives as [`AuthError::InternalError`], **never**
    /// [`AuthError::AuthorizationRequired`] — the latter would make `AuthorizationManager` restart
    /// an authorization the user already completed, forever, which is exactly the failure mode
    /// §6.9's error-class contract exists to prevent.
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let entry = match &self.server_url {
            Some(url) => self
                .store
                .auth_for_url_async(&self.server_name, url)
                .await
                .map_err(|error| store_error_to_auth_error(&error))?,
            None => self
                .store
                .auth_entry_async(&self.server_name)
                .await
                .map_err(|error| store_error_to_auth_error(&error))?,
        };
        Ok(entry.and_then(|entry| entry.credentials))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.store
            .update_credentials_async(
                &self.server_name,
                credentials,
                self.server_url.as_deref(),
            )
            .await
            .map_err(|error| store_error_to_auth_error(&error))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store
            .clear_authorization_context_async(&self.server_name)
            .await
            .map_err(|error| store_error_to_auth_error(&error))
    }
}

/// rmcp's PKCE/CSRF state store for **one** MCP server.
///
/// [`rmcp::transport::auth::StateStore`] is keyed by CSRF token; keeping **one** `state` slot in the
/// [`AuthEntry`] and returning it only when `state.csrf_token == csrf` reproduces upstream's single
/// `oauthState` slot exactly while satisfying the keyed trait.
///
/// **No installer, deliberately:** the same reason as [`McpCredentialStore`] — production HTTP auth
/// runs through [`crate::runtime::HttpAuthProvider`] / `StoredCredentialAuth` (`runtime.rs:235-245`),
/// not rmcp's `AuthorizationManager`, so `set_state_store` has no caller until section 05's
/// `AuthClient` lands (`runtime.rs:1620`, `runtime.rs:1975`). The PKCE/CSRF material production
/// *does* use today is written and read through [`McpAuthStore`]'s `state` slot directly.
pub struct McpStateStore {
    store: McpAuthStore,
    server_name: String,
    server_url: Option<String>,
}

impl McpStateStore {
    /// A state store bound to one server.
    #[must_use]
    pub fn new(
        store: McpAuthStore,
        server_name: impl Into<String>,
        server_url: Option<String>,
    ) -> Self {
        Self {
            store,
            server_name: server_name.into(),
            server_url,
        }
    }
}

#[async_trait::async_trait]
impl rmcp::transport::auth::StateStore for McpStateStore {
    async fn save(
        &self,
        _csrf_token: &str,
        state: StoredAuthorizationState,
    ) -> Result<(), AuthError> {
        // The CSRF token is inside `state`; one slot per server means a second authorization round
        // overwrites the first, which is upstream's behaviour for `oauthState`.
        self.store
            .update_state_async(&self.server_name, state, self.server_url.as_deref())
            .await
            .map_err(|error| store_error_to_auth_error(&error))
    }

    /// Returns the stored state **only** when its `csrf_token` matches. A non-matching token yields
    /// `None` rather than the stored state — a callback carrying someone else's CSRF token must not
    /// be handed this server's PKCE verifier.
    async fn load(&self, csrf_token: &str) -> Result<Option<StoredAuthorizationState>, AuthError> {
        let entry = self
            .store
            .auth_entry_async(&self.server_name)
            .await
            .map_err(|error| store_error_to_auth_error(&error))?;
        Ok(entry
            .and_then(|entry| entry.state)
            .filter(|state| state.csrf_token == csrf_token))
    }

    /// Clears the slot **only** when it is the one being deleted, so a stale delete for an abandoned
    /// flow cannot wipe an in-flight one.
    async fn delete(&self, csrf_token: &str) -> Result<(), AuthError> {
        let entry = self
            .store
            .auth_entry_async(&self.server_name)
            .await
            .map_err(|error| store_error_to_auth_error(&error))?;
        let matches = entry
            .and_then(|entry| entry.state)
            .is_some_and(|state| state.csrf_token == csrf_token);
        if !matches {
            return Ok(());
        }
        self.store
            .clear_state_async(&self.server_name)
            .await
            .map_err(|error| store_error_to_auth_error(&error))
    }
}

// ---------------------------------------------------------------------------------------------
// MCP-270 — the embedder façade's one piece of real logic
// ---------------------------------------------------------------------------------------------

/// `McpOAuthTokenStatus` (`oauth.ts:16`) — a **different** three-state enum from
/// [`OAuthCredentialStatus`]: its `Present` variant carries tokens, not the whole entry.
///
/// **De-duplicated at integration.** `oauth.rs` had landed a second copy of this shape under the
/// name `OAuthCredentialStatus`; 13f's plan text fixes *that* name to the entry-carrying enum above
/// and MCP-270 fixes this one to the token-carrying enum, so the two are now one type with
/// upstream's payload — `{ status: "present"; tokens: McpOAuthTokens }`, i.e.
/// [`crate::oauth::McpTokens`], the projected wire shape rather than the stored record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpOAuthTokenStatus {
    /// A credential with tokens, projected to `McpOAuthTokens`.
    Present(crate::oauth::McpTokens),
    /// No credential, or a credential holding only a client record or a PKCE verifier.
    Absent,
    /// The store is unreachable; the message passes through unchanged.
    Unavailable {
        /// [`format_oauth_credential_store_unavailable`]'s sentence.
        message: String,
    },
}

impl From<OAuthCredentialStatus> for McpOAuthTokenStatus {
    /// `inspectMcpOAuthTokensForUrl`'s mapping (`oauth.ts:29-39`) — **the one piece of real logic in
    /// the façade**: a `present` status whose entry has no tokens collapses to `absent`, so an entry
    /// holding only a `clientInfo` or a PKCE verifier is not reported as authenticated.
    ///
    /// A credential whose `token_response` cannot be projected collapses the same way, which is
    /// upstream's `status.entry.tokens ? … : { status: "absent" }` over an absent `tokens` member.
    fn from(status: OAuthCredentialStatus) -> Self {
        match status {
            OAuthCredentialStatus::Present(entry) => entry
                .credentials
                .as_ref()
                .and_then(crate::oauth::project_tokens)
                .map_or(McpOAuthTokenStatus::Absent, McpOAuthTokenStatus::Present),
            OAuthCredentialStatus::Absent => McpOAuthTokenStatus::Absent,
            OAuthCredentialStatus::Unavailable { message } => {
                McpOAuthTokenStatus::Unavailable { message }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// MCP-084 (13b) — static bearer tokens
// ---------------------------------------------------------------------------------------------

/// `interpolateEnvVars(value)` (`utils.ts:74`) — the three syntaxes `${VAR}`, `$env:VAR` and
/// `{env:VAR}`, each replaced with the variable's value or the **empty string** when unset.
///
/// A missing variable interpolating to `""` rather than erroring is upstream's behaviour everywhere
/// except `resolveServerUrl`, which pre-checks with `getMissingEnvVars` and throws.
///
/// **This is the crate's single implementation** (MCP-082, MCP-342). `oauth.rs` had landed a second
/// copy; its one-argument process-env form survives as a delegation to this one.
#[must_use]
pub fn interpolate_env_vars(value: &str, env: &EnvFn) -> String {
    interpolate_env_vars_with(value, |name| env(name))
}

/// [`interpolate_env_vars`] against any lookup — the engine, and the testable half.
///
/// **Three chained passes, in upstream's order, not one alternation.** `utils.ts:74-79` is
///
/// ```text
/// value.replace(/\$\{(\w+)\}/g, …).replace(/\$env:(\w+)/g, …).replace(/\{env:(\w+)\}/g, …)
/// ```
///
/// and chaining is observable: a `${A}` whose value is `"$env:B"` expands again on pass 2, where a
/// single alternation pass would leave `$env:B` literal. (The single-alternation regex belongs to
/// `getMissingEnvVars` at `utils.ts:83`, which scans rather than substitutes — an easy pair to
/// transpose, and a defect this port carried until integration.)
#[must_use]
pub fn interpolate_env_vars_with<F>(value: &str, lookup: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    // The class is spelled out rather than written `\w` — see this module's `ENV_NAME` note. Rust's
    // `regex` makes `\w` Unicode-aware, JavaScript's does not, and a `${café}` would interpolate
    // here and stay literal upstream.
    static PATTERNS: LazyLock<[Option<Regex>; 3]> = LazyLock::new(|| {
        [
            Regex::new(r"\$\{([A-Za-z0-9_]+)\}").ok(),
            Regex::new(r"\$env:([A-Za-z0-9_]+)").ok(),
            Regex::new(r"\{env:([A-Za-z0-9_]+)\}").ok(),
        ]
    });
    let mut out = std::borrow::Cow::Borrowed(value);
    for pattern in PATTERNS.iter().flatten() {
        out = std::borrow::Cow::Owned(
            pattern
                .replace_all(&out, |captures: &regex::Captures<'_>| {
                    captures
                        .get(1)
                        .and_then(|name| lookup(name.as_str()))
                        .unwrap_or_default()
                })
                .into_owned(),
        );
    }
    out.into_owned()
}

/// `interpolateSecretExpression(value)` (`utils.ts`) — the `!` / `!!` command-marker grammar.
///
/// * `!!x` — an **escaped** literal `!`: the leading `!` is dropped and the rest is interpolated.
/// * `!x` — a command marker, returned **verbatim** so a later `resolveCommandSecret` can execute it.
/// * anything else — interpolated.
///
/// This is the divergence MCP-084 names: `cyrup_ext_subagents::exec::mcp_direct_tools`'s
/// `resolve_bearer_token` calls plain interpolation here, so `bearerToken: "!!x"` resolves to
/// `"!!x"` instead of `"!x"`. The adapter's grammar is the correct one.
#[must_use]
pub fn interpolate_secret_expression(value: &str, env: &EnvFn) -> String {
    if value.starts_with("!!") {
        // `interpolateEnvVars(value.slice(1))` drops exactly **one** `!`, so `!!x` yields the
        // literal `!x`. Stripping both — the obvious Rust `strip_prefix("!!")` — silently deletes
        // the escaped marker and turns an escaped literal into a bare value.
        return interpolate_env_vars(value.get(1..).unwrap_or_default(), env);
    }
    if value.starts_with('!') {
        return value.to_string();
    }
    interpolate_env_vars(value, env)
}

/// `resolveBearerToken(definition)` (`utils.ts`) — `bearerToken` wins over `bearerTokenEnv`.
///
/// `bearerToken` present ⇒ [`interpolate_secret_expression`]; else `process.env[bearerTokenEnv]`
/// when `bearerTokenEnv` is **truthy** (an empty string is not), else `None`.
///
/// A leading-`!` result is a *command* marker that `connectHttpClient` resolves through
/// `resolveCommandSecret` before setting `Authorization: Bearer …` — that execution belongs to the
/// transport section (13c), not here; this function reproduces upstream's resolution and leaves the
/// marker intact for it.
#[must_use]
pub fn resolve_bearer_token(
    bearer_token: Option<&str>,
    bearer_token_env: Option<&str>,
    env: &EnvFn,
) -> Option<String> {
    if let Some(token) = bearer_token {
        return Some(interpolate_secret_expression(token, env));
    }
    bearer_token_env
        .filter(|name| !name.is_empty())
        .and_then(|name| env(name))
}

/// Whether `resolve_bearer_token`'s result is an unresolved command marker (`!cmd`, but not `!!lit`)
/// — `connectHttpClient`'s `commandBearer` test.
#[must_use]
pub fn is_command_secret(value: &str) -> bool {
    value.starts_with('!') && !value.starts_with("!!")
}

/// `getMissingEnvVars(value)` (`utils.ts:83`) — every placeholder name in `value` whose variable is
/// **unset**, in first-occurrence order, deduplicated.
///
/// **One alternation, not three passes**, and that asymmetry with
/// [`interpolate_env_vars_with`] is upstream's: `getMissingEnvVars` *scans* where
/// `interpolateEnvVars` *substitutes*, so it must not see a later pass's output. `match[1] ?? match[2]
/// ?? match[3]` picks whichever of the three alternatives fired, and the `Set` is what makes
/// `https://x/${A}/${A}` report `A` once. Upstream's `[...set]` is insertion order, so the
/// `IndexSet`-shaped `Vec` + `contains` below reproduces the *order* the error message prints in,
/// which a `BTreeSet` would silently re-sort.
///
/// `undefined`-vs-empty matters: upstream tests `process.env[name] === undefined`, so a variable set
/// to the empty string is **present** and not reported — which is why the lookup's `Option` is
/// tested rather than its contents.
///
/// The character class is spelled `[A-Za-z0-9_]` rather than `\w` for the reason
/// [`interpolate_env_vars_with`] gives: Rust's `regex` makes `\w` Unicode-aware and JavaScript's
/// `\w` is ASCII-only, so `${café}` interpolates in one engine and stays literal in the other. Run
/// on node 22 against v2.26.1: `interpolateEnvVars("${café}")` is `"${café}"` and
/// `interpolateEnvVars("$env:café")` is `"é"` — the ASCII prefix `caf` is the name.
#[must_use]
pub fn missing_env_vars(value: &str, env: &EnvFn) -> Vec<String> {
    static PATTERN: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(r"\$\{([A-Za-z0-9_]+)\}|\$env:([A-Za-z0-9_]+)|\{env:([A-Za-z0-9_]+)\}").ok()
    });
    let Some(pattern) = PATTERN.as_ref() else {
        return Vec::new();
    };
    let mut missing: Vec<String> = Vec::new();
    for captures in pattern.captures_iter(value) {
        let Some(name) = (1..=3).find_map(|group| captures.get(group)).map(|m| m.as_str()) else {
            continue;
        };
        if env(name).is_none() && !missing.iter().any(|seen| seen == name) {
            missing.push(name.to_string());
        }
    }
    missing
}

/// `resolveServerUrl(definition)` (`utils.ts:167`) — the identity field that can **throw**, and the
/// only reason [`crate::dirs::compute_server_hash`] is fallible at all (MCP-084, MCP-141, MCP-145).
///
/// Four arms, in upstream's order:
///
/// 1. `definition.url == null` (JS loose equality, so `null` *and* `undefined`) ⇒ `Ok(None)`.
/// 2. a non-string `url` ⇒ `MCP server URL must be a string`. **Absorbed by the type system here**:
///    `ServerEntry::url` is `Option<String>`, so a non-string is rejected by the deserialiser
///    (MCP-066) and this arm has no Rust counterpart. Named so the missing string is accounted for
///    rather than looking like an omission.
/// 3. any placeholder naming an **unset** variable ⇒
///    `Missing environment variable{s} in MCP server URL: {names}` — singular/plural on the count,
///    names joined with `", "` in first-occurrence order.
/// 4. a resolved string `new URL()` rejects ⇒
///    `Invalid MCP server URL after environment interpolation: {resolved}`.
///
/// All three message forms were produced by **running upstream on node 22** (`utils.ts` @ v2.26.1,
/// `fafae21`), not transcribed: `"https://x/${NOPE}"` gives `Missing environment variable in MCP
/// server URL: NOPE`, `"https://x/${NOPE}/${ALSONOPE}"` gives `Missing environment variables in MCP
/// server URL: NOPE, ALSONOPE`, and `"not a url"` gives `Invalid MCP server URL after environment
/// interpolation: not a url`.
///
/// # Why the throw matters more than the message
///
/// `computeServerHash` calls this **inside** `isServerCacheValid`'s `try`, so a URL server whose
/// `${VAR}` is unset is never cache-valid — that is the sole mechanism keeping such a server out of
/// the cold-start direct-tool surface, where it would otherwise register tools it can never call
/// (MCP-145).
///
/// `url::Url::parse` is the WHATWG URL parser, which is what `new URL()` is; the two agree on the
/// cases that matter here — a scheme-relative `//x/y` and a bare path `/abs/path` are rejected by
/// both, and `unix:///tmp/s.sock`, `x:y` and `mailto:a@b` are accepted by both (measured on node 22).
pub fn resolve_server_url(url: Option<&str>, env: &EnvFn) -> McpResult<Option<String>> {
    let Some(url) = url else {
        return Ok(None);
    };
    let missing = missing_env_vars(url, env);
    if !missing.is_empty() {
        let plural = if missing.len() == 1 { "" } else { "s" };
        return Err(McpError::Config(format!(
            "Missing environment variable{plural} in MCP server URL: {}",
            missing.join(", ")
        )));
    }
    let resolved = interpolate_env_vars(url, env);
    if url::Url::parse(&resolved).is_err() {
        return Err(McpError::Config(format!(
            "Invalid MCP server URL after environment interpolation: {resolved}"
        )));
    }
    Ok(Some(resolved))
}

// ---------------------------------------------------------------------------------------------
// The seam against section 13g's OAuth flow (`crate::oauth`)
// ---------------------------------------------------------------------------------------------

impl McpAuthStore {
    /// [`Self::clear_credentials`], serialized.
    pub async fn clear_credentials_async(&self, server_name: &str) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        self.locked(server_name, move |store| store.clear_credentials(&name))
            .await
    }

    /// [`Self::clear_client_info`], serialized.
    pub async fn clear_client_info_async(&self, server_name: &str) -> Result<(), AuthStoreError> {
        let name = server_name.to_string();
        self.locked(server_name, move |store| store.clear_client_info(&name))
            .await
    }
}

/// `impl crate::oauth::McpOAuthStorage for McpAuthStore` — the production storage the OAuth flow
/// runs against (MCP-321), replacing section 13g's interim `InMemoryOAuthStorage`.
///
/// Every method is a one-line delegation to the serialized `*_async` wrapper, because 13f already
/// owns the hard half: the keychain, the chunking manifest, the process-lifetime cache, the
/// `keyctl` recovery, and — the part the flow depends on and must not re-derive — **MCP-264's URL
/// binding and sibling purge**.
///
/// | trait method | `McpAuthStore` | upstream |
/// |---|---|---|
/// | `load` | [`McpAuthStore::auth_entry_async`] | `getAuthEntry(serverName, options)` |
/// | `get_auth_for_url` | [`McpAuthStore::auth_for_url_async`] | `getAuthForUrl` |
/// | `save_credentials(_, url, Some)` | [`McpAuthStore::update_credentials_async`] | `updateTokens(name, tokens, serverUrl, options)` |
/// | `save_credentials(_, _, None)` | [`McpAuthStore::clear_credentials_async`] | `clearTokens(name, options)` |
/// | `save_client(_, url, Some)` | [`McpAuthStore::update_client_info_async`] | `updateClientInfo(name, info, serverUrl, options)` |
/// | `save_client(_, _, None)` | [`McpAuthStore::clear_client_info_async`] | `clearClientInfo(name, options)` |
/// | `clear_all` | [`McpAuthStore::remove_auth_entry_async`] | `clearAllCredentials` / `removeAuthEntry` |
/// | `base_dir` | [`McpAuthStore::auth_base_dir`] | `getAuthBaseDir(options)` |
///
/// **The `None` arms take no `server_url`, and that is upstream, not an omission.**
/// `mcp-auth.ts:983` `clearClientInfo` and `mcp-auth.ts:994` `clearTokens` both call
/// `saveAuthEntry(serverName, entry, undefined, options)` — the URL argument is literally
/// `undefined`, so the binding-changed purge (`mcp-auth.ts:868`, `:887`) cannot fire on a clear.
/// Threading the trait's `server_url` into the clear would purge the *sibling* slot on a
/// URL change, destroying a client registration the flow is about to reuse.
///
/// **The error arm is the load-bearing part.** Every failure crosses as
/// [`McpError::CredentialStore`], never [`McpError::Other`], so
/// [`AuthStoreError::is_store_unavailable`] stays reachable through
/// [`McpError::is_credential_store_failure`]. Section 07's refresh driver rethrows the store class
/// and swallows every other refresh error into `None`; a broken keychain arriving as an ordinary
/// failure is an infinite silent re-auth loop.
#[async_trait::async_trait]
impl crate::oauth::McpOAuthStorage for McpAuthStore {
    async fn load(&self, server_name: &str) -> crate::errors::McpResult<Option<AuthEntry>> {
        Ok(self.auth_entry_async(server_name).await?)
    }

    async fn get_auth_for_url(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> crate::errors::McpResult<Option<AuthEntry>> {
        Ok(self.auth_for_url_async(server_name, server_url).await?)
    }

    async fn save_credentials(
        &self,
        server_name: &str,
        server_url: &str,
        credentials: Option<StoredCredentials>,
    ) -> crate::errors::McpResult<()> {
        match credentials {
            Some(credentials) => self
                .update_credentials_async(server_name, credentials, Some(server_url))
                .await
                .map_err(crate::errors::McpError::from),
            // `clearTokens(serverName, options)` — no URL, see the table above.
            None => self
                .clear_credentials_async(server_name)
                .await
                .map_err(crate::errors::McpError::from),
        }
    }

    async fn save_client(
        &self,
        server_name: &str,
        server_url: &str,
        client: Option<StoredClientInfo>,
    ) -> crate::errors::McpResult<()> {
        match client {
            Some(client) => self
                .update_client_info_async(server_name, client, Some(server_url))
                .await
                .map_err(crate::errors::McpError::from),
            // `clearClientInfo(serverName, options)` — no URL, see the table above.
            None => self
                .clear_client_info_async(server_name)
                .await
                .map_err(crate::errors::McpError::from),
        }
    }

    async fn clear_all(&self, server_name: &str) -> crate::errors::McpResult<()> {
        self.remove_auth_entry_async(server_name)
            .await
            .map_err(crate::errors::McpError::from)
    }

    async fn oauth_state(&self, server_name: &str) -> crate::errors::McpResult<Option<String>> {
        Ok(self
            .auth_entry_async(server_name)
            .await?
            .and_then(|entry| entry.state)
            .map(|state| state.csrf_token.clone()))
    }

    async fn clear_oauth_state(&self, server_name: &str) -> crate::errors::McpResult<()> {
        self.clear_state_async(server_name)
            .await
            .map_err(crate::errors::McpError::from)
    }

    fn base_dir(&self) -> PathBuf {
        self.auth_base_dir()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// A store over an injectable memory backend, rooted in a scratch dir so the legacy-file paths
    /// are real.
    fn test_store(fault: SimulatedFault) -> (McpAuthStore, Arc<MemorySecretStore>, tempfile::TempDir)
    {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemorySecretStore::with_fault(fault));
        let dirs = McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf());
        let store = McpAuthStore::with_backends(
            backend.clone(),
            Arc::new(MemorySecretStore::new()),
            dirs,
            AuthStorageOptions::default(),
            Arc::new(|_| None),
        );
        (store, backend, dir)
    }

    fn credentials(access: &str) -> StoredCredentials {
        let response = token_response_from_parts(access, Some("refresh"), Some(3600), Some("a b"))
            .expect("token response");
        StoredCredentials::new(
            "client-1".to_string(),
            Some(response),
            vec!["a".to_string(), "b".to_string()],
            Some(1_700_000_000),
        )
        .with_issuer(Some("https://issuer.example".to_string()))
    }

    fn state(csrf: &str) -> StoredAuthorizationState {
        authorization_state_from_parts("verifier-secret", csrf, 1_700_000_000).expect("state")
    }

    // -- MCP-251 -------------------------------------------------------------------------------

    #[test]
    fn hostile_server_names_stay_inside_the_base_dir() {
        let (store, _backend, dir) = test_store(SimulatedFault::None);
        for name in ["Cloudflare Workers", "сервер", "../escape", "@scope/name", ""] {
            let path = store.auth_entry_file_path(name);
            let relative = path.strip_prefix(store.auth_base_dir()).unwrap();
            let text = relative.to_string_lossy().replace('\\', "/");
            let (head, tail) = text.split_once('/').unwrap();
            assert_eq!(tail, "tokens.json", "{name}");
            let hex = head.strip_prefix("sha256-").unwrap();
            assert_eq!(hex.len(), 64, "{name}");
            assert!(hex.chars().all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f')));
            assert!(!text.starts_with(".."), "{name}");
            assert!(!Path::new(&text).is_absolute(), "{name}");
            // `<authDir>/../escape/tokens.json` never exists.
            assert!(path.starts_with(dir.path()), "{name}");
        }
        // The empty string is a valid server name with a valid account: sha256 of zero bytes.
        assert_eq!(
            auth_entry_account(""),
            "sha256-e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_account_is_derived_from_the_server_name_alone() {
        let dir = tempfile::tempdir().unwrap();
        let one = McpAuthStore::with_backend(
            Arc::new(MemorySecretStore::new()),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::with_base_dir(dir.path().join("one")),
        );
        let two = McpAuthStore::with_backend(
            Arc::new(MemorySecretStore::new()),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::with_base_dir(dir.path().join("two")),
        );
        // Different configured oauthDirs, same keychain account: two projects configuring a server
        // named `github` share one entry.
        assert_ne!(one.auth_entry_file_path("github"), two.auth_entry_file_path("github"));
        assert_eq!(auth_entry_account("github"), auth_entry_account("github"));
    }

    // -- MCP-250 -------------------------------------------------------------------------------

    #[test]
    fn unknown_keys_are_dropped_not_rejected() {
        let value: serde_json::Value = serde_json::json!({
            "client": {"clientId": "c"},
            "serverUrl": "https://x.example/mcp",
            "futureKey": 9
        });
        let entry = auth_entry_from_value("s", value, "test").expect("entry");
        assert_eq!(entry.client.as_ref().unwrap().client_id, "c");
        assert_eq!(entry.server_url.as_deref(), Some("https://x.example/mcp"));
        let round_trip = serde_json::to_string(&entry).unwrap();
        assert!(!round_trip.contains("futureKey"));
    }

    #[test]
    fn malformed_payloads_produce_the_invalid_shape_error() {
        for payload in [
            r#"{"client":{}}"#,
            r#"{"client":{"clientId":1}}"#,
            r#"{"serverUrl":5}"#,
            r#"{"client":{"clientId":"c","clientSecret":true}}"#,
            "[1,2]",
            "5",
        ] {
            let value = serde_json::from_str::<serde_json::Value>(payload).unwrap();
            let error = auth_entry_from_value("srv", value, "src").unwrap_err();
            assert_eq!(
                error.to_string(),
                "Failed to parse OAuth credentials for srv from src: invalid credential shape",
                "{payload}"
            );
        }
    }

    #[test]
    fn redirect_uris_is_the_one_field_that_degrades_silently() {
        let value = serde_json::json!({"client":{"clientId":"c","redirectUris":["a", 2]}});
        let entry = auth_entry_from_value("s", value, "test").expect("entry survives");
        assert!(entry.client.as_ref().unwrap().redirect_uris.is_none());

        let value = serde_json::json!({"client":{"clientId":"c","redirectUris":["a","b"]}});
        let entry = auth_entry_from_value("s", value, "test").expect("entry");
        assert_eq!(
            entry.client.unwrap().redirect_uris.as_deref(),
            Some(["a".to_string(), "b".to_string()].as_slice())
        );
    }

    #[test]
    fn invalid_json_reports_without_the_shape_suffix() {
        let error = parse_json_payload("srv", "{not json", "src").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Failed to parse OAuth credentials for srv from src"
        );
    }

    // -- MCP-277 (critical) --------------------------------------------------------------------

    #[test]
    fn debug_never_renders_a_secret() {
        let mut client = StoredClientInfo::new("client-1");
        client.client_secret = Some("SUPER-SECRET-VALUE".to_string());
        let entry = AuthEntry {
            credentials: Some(credentials("ACCESS-TOKEN-VALUE")),
            client: Some(client),
            state: Some(state("CSRF-TOKEN-VALUE")),
            server_url: Some("https://x.example/mcp".to_string()),
        };
        let rendered = format!("{entry:?}");
        for secret in [
            "SUPER-SECRET-VALUE",
            "ACCESS-TOKEN-VALUE",
            "CSRF-TOKEN-VALUE",
            "verifier-secret",
            "refresh",
        ] {
            assert!(!rendered.contains(secret), "{secret} leaked into {rendered}");
        }
        assert!(rendered.contains("[REDACTED]"));
        // The server URL is not a secret and stays legible — it is what makes a Debug useful.
        assert!(rendered.contains("https://x.example/mcp"));
    }

    #[test]
    fn error_messages_never_interpolate_a_payload() {
        let error = AuthStoreError::Unavailable {
            operation: StoreOp::Write,
            server: "srv".to_string(),
            source: AuthSecretStoreError::Recovery("boom".to_string()),
        };
        assert_eq!(
            error.to_string(),
            "Failed to write OAuth credentials for srv to the OS secure credential store"
        );
        assert_eq!(
            AuthStoreError::Unavailable {
                operation: StoreOp::Read,
                server: "srv".to_string(),
                source: AuthSecretStoreError::Recovery("boom".to_string()),
            }
            .to_string(),
            "Failed to read OAuth credentials for srv from the OS secure credential store"
        );
        assert_eq!(
            AuthStoreError::MissingChunk {
                chunk_account: "sha256-aa.chunk.dd.3".to_string(),
                server: "srv".to_string(),
            }
            .to_string(),
            "Missing OAuth credential chunk sha256-aa.chunk.dd.3 for srv"
        );
    }

    // -- MCP-253 / MCP-275 / MCP-286 -----------------------------------------------------------

    #[test]
    fn a_large_credential_round_trips_through_chunks() {
        let (store, backend, _dir) = test_store(SimulatedFault::SizeLimited);
        let mut entry = AuthEntry {
            credentials: Some(credentials(&"t".repeat(5000))),
            client: None,
            state: None,
            server_url: Some("https://x.example/mcp".to_string()),
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example/mcp")).unwrap();

        let account = auth_entry_account("srv");
        let entries = backend.entries();
        // Exactly one non-`.chunk.` entry, at the base account.
        let base: Vec<_> = entries.iter().filter(|(k, _)| !k.contains(".chunk.")).collect();
        assert_eq!(base.len(), 1);
        assert_eq!(base[0].0, account);
        // Every stored value is under the Windows ceiling, and none contains a newline (MCP-275).
        for (key, value) in &entries {
            assert!(value.len() <= AUTH_SECRET_VALUE_LIMIT, "{key} is {} bytes", value.len());
            assert!(!value.contains('\n'), "{key} contains a newline");
        }
        // The manifest's key order is the emitted order.
        assert!(base[0].1.starts_with(r#"{"__piMcpAdapterOAuthChunked":1,"chunkCount":"#));

        let read = store.auth_entry("srv").unwrap().unwrap();
        assert!(read.credentials.is_some());
        assert_eq!(read.server_url.as_deref(), Some("https://x.example/mcp"));
    }

    #[test]
    fn chunking_never_splits_a_code_point() {
        // Non-ASCII with no upstream twin: upstream slices UTF-16 units while hashing UTF-8 bytes.
        let payload = "é".repeat(2000);
        let manifest = AuthEntryChunkManifest::for_payload(&payload);
        assert_eq!(manifest.chunk_count, payload.len().div_ceil(AUTH_SECRET_CHUNK_SIZE));
        let chunks = split_payload(&payload, manifest.chunk_count);
        assert_eq!(chunks.len(), manifest.chunk_count);
        for chunk in &chunks {
            assert!(chunk.len() <= AUTH_SECRET_CHUNK_SIZE);
            assert!(!chunk.is_empty());
        }
        assert_eq!(chunks.concat(), payload);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn the_chunking_threshold_is_at_or_below_the_value_limit() {
        // Deliberately an assertion on constants: this *is* the pinned regression. A threshold
        // above the ceiling means oversized records still fail to persist on Windows, and the two
        // constants drifting apart is the only way that happens.
        assert!(AUTH_SECRET_CHUNK_SIZE <= AUTH_SECRET_VALUE_LIMIT);
    }

    #[test]
    fn a_hostile_chunk_count_is_not_a_manifest() {
        let value = serde_json::json!({
            AUTH_CHUNK_MANIFEST_KEY: 1, "chunkCount": 100_000, "chunkDigest": "0123456789abcdef"
        });
        assert!(parse_chunk_manifest(&value).is_none());
        // Every other malformed field degrades the same way.
        for bad in [
            serde_json::json!({AUTH_CHUNK_MANIFEST_KEY: 2, "chunkCount": 1, "chunkDigest": "0123456789abcdef"}),
            serde_json::json!({AUTH_CHUNK_MANIFEST_KEY: 1, "chunkCount": 0, "chunkDigest": "0123456789abcdef"}),
            serde_json::json!({AUTH_CHUNK_MANIFEST_KEY: 1, "chunkCount": 1.5, "chunkDigest": "0123456789abcdef"}),
            serde_json::json!({AUTH_CHUNK_MANIFEST_KEY: 1, "chunkCount": 1, "chunkDigest": "XYZ"}),
            serde_json::json!({AUTH_CHUNK_MANIFEST_KEY: 1, "chunkCount": 1}),
        ] {
            assert!(parse_chunk_manifest(&bad).is_none(), "{bad}");
        }
        let good = serde_json::json!({
            AUTH_CHUNK_MANIFEST_KEY: 1, "chunkCount": 7, "chunkDigest": "a1b2c3d4e5f60718"
        });
        assert_eq!(parse_chunk_manifest(&good).unwrap().chunk_count, 7);
    }

    // -- MCP-254 / MCP-265 ---------------------------------------------------------------------

    #[test]
    fn a_deleted_chunk_is_unavailable_never_absent() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        let mut entry = AuthEntry {
            credentials: Some(credentials(&"t".repeat(5000))),
            client: None,
            state: None,
            server_url: Some("https://x.example/mcp".to_string()),
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example/mcp")).unwrap();
        store.reset_cache();

        let victim = backend
            .entries()
            .into_iter()
            .find(|(k, _)| k.ends_with(".1"))
            .expect("a chunk at index 1");
        backend.remove_entry(&victim.0);

        match store.inspect_auth_for_url("srv", "https://x.example/mcp").unwrap() {
            OAuthCredentialStatus::Unavailable { message } => {
                assert!(message.starts_with("OAuth credential store unavailable"));
            }
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    // -- MCP-284 -------------------------------------------------------------------------------

    #[test]
    fn a_corrupt_base_payload_propagates_on_read_and_wraps_on_remove() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        backend.seed(&auth_entry_account("srv"), "{not json");

        // Read path: the parse runs *outside* the wrapping, so inspect propagates.
        let error = store.inspect_auth_for_url("srv", "https://x.example").unwrap_err();
        assert!(!error.is_store_unavailable(), "{error}");
        assert_eq!(
            error.to_string(),
            "Failed to parse OAuth credentials for srv from OS secure credential store"
        );

        // Remove path: the same parse is *inside* the wrapping — except that a base payload which
        // is not JSON is simply "not a manifest", so the removal succeeds. What the remove path
        // must not do is surface the read failure as a parse error.
        store.remove_auth_entry("srv").unwrap();
        assert!(backend.entries().is_empty());
    }

    // -- MCP-255 / MCP-285 ---------------------------------------------------------------------

    #[test]
    fn a_failing_cleanup_never_fails_a_successful_write() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FailingRemoveStore::new());
        let store = McpAuthStore::with_backends(
            backend.clone(),
            Arc::new(MemorySecretStore::new()),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::default(),
            Arc::new(|_| None),
        );
        let mut entry = AuthEntry {
            credentials: Some(credentials(&"a".repeat(3000))),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example")).unwrap();
        // A rewrite with different content changes the digest, so cleanup of the previous chunks is
        // attempted — and refused. The write still returns Ok.
        entry.credentials = Some(credentials(&"b".repeat(3000)));
        store.save_auth_entry("srv", &mut entry, Some("https://x.example")).unwrap();
    }

    #[test]
    fn a_failing_chunk_delete_aborts_the_remove_before_the_base_account() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FailingRemoveStore::failing_suffix(".1"));
        let store = McpAuthStore::with_backends(
            backend.clone(),
            Arc::new(MemorySecretStore::new()),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::default(),
            Arc::new(|_| None),
        );
        let mut entry = AuthEntry {
            credentials: Some(credentials(&"a".repeat(5000))),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example")).unwrap();

        let error = store.remove_auth_entry("srv").unwrap_err();
        assert_eq!(error.operation(), Some(StoreOp::Remove));
        assert!(error.is_store_unavailable());
        let account = auth_entry_account("srv");
        let entries = backend.as_memory().unwrap().entries();
        // The base still holds its manifest; chunk 0 is gone.
        assert!(entries.iter().any(|(k, _)| *k == account));
        assert!(!entries.iter().any(|(k, _)| k.ends_with(".0")));
        assert!(entries.iter().any(|(k, _)| k.ends_with(".1")));
    }

    // -- MCP-264 (critical) --------------------------------------------------------------------

    #[test]
    fn a_url_change_wipes_exactly_the_other_slots() {
        let (store, _backend, _dir) = test_store(SimulatedFault::None);
        let seed = |store: &McpAuthStore| {
            let mut entry = AuthEntry {
                credentials: Some(credentials("old")),
                client: Some(StoredClientInfo::new("old-client")),
                state: Some(state("old-csrf")),
                server_url: Some("https://old.example/mcp".to_string()),
            };
            store.save_auth_entry("srv", &mut entry, Some("https://old.example/mcp")).unwrap();
        };

        seed(&store);
        store.update_credentials("srv", credentials("new"), Some("https://new.example/mcp")).unwrap();
        let entry = store.auth_entry("srv").unwrap().unwrap();
        assert!(entry.credentials.is_some());
        assert!(entry.client.is_none(), "a stale DCR record must not survive a rebinding");
        assert!(entry.state.is_none(), "a PKCE verifier must never cross authorization contexts");
        assert_eq!(entry.server_url.as_deref(), Some("https://new.example/mcp"));

        store.remove_auth_entry("srv").unwrap();
        seed(&store);
        store.update_client_info("srv", StoredClientInfo::new("new-client"), Some("https://new.example/mcp")).unwrap();
        let entry = store.auth_entry("srv").unwrap().unwrap();
        assert!(entry.credentials.is_none());
        assert_eq!(entry.client.unwrap().client_id, "new-client");
        assert!(entry.state.is_none());

        store.remove_auth_entry("srv").unwrap();
        seed(&store);
        store.update_state("srv", state("new-csrf"), Some("https://new.example/mcp")).unwrap();
        let entry = store.auth_entry("srv").unwrap().unwrap();
        assert!(entry.credentials.is_none());
        assert!(entry.client.is_none());
        assert_eq!(entry.state.unwrap().csrf_token, "new-csrf");
    }

    #[test]
    fn the_same_url_purges_nothing() {
        let (store, _backend, _dir) = test_store(SimulatedFault::None);
        let mut entry = AuthEntry {
            credentials: Some(credentials("old")),
            client: Some(StoredClientInfo::new("client")),
            state: Some(state("csrf")),
            server_url: Some("https://x.example/mcp".to_string()),
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example/mcp")).unwrap();
        store.update_credentials("srv", credentials("new"), Some("https://x.example/mcp")).unwrap();
        let entry = store.auth_entry("srv").unwrap().unwrap();
        assert!(entry.client.is_some());
        assert!(entry.state.is_some());
    }

    #[test]
    fn url_binding_is_exact_and_fail_closed() {
        let (store, _backend, _dir) = test_store(SimulatedFault::None);
        let mut entry = AuthEntry {
            credentials: Some(credentials("t")),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example/mcp")).unwrap();
        assert!(store.auth_for_url("srv", "https://x.example/mcp").unwrap().is_some());
        // A trailing-slash change invalidates the credential: no normalization, ever.
        assert!(store.auth_for_url("srv", "https://x.example/mcp/").unwrap().is_none());

        // An entry with no stored URL predates the binding and is invalid.
        let mut bare = AuthEntry {
            credentials: Some(credentials("t")),
            ..AuthEntry::default()
        };
        store.save_auth_entry("other", &mut bare, None).unwrap();
        assert!(store.auth_entry("other").unwrap().is_some());
        assert!(store.auth_for_url("other", "https://x.example/mcp").unwrap().is_none());
    }

    #[test]
    fn a_clearer_never_rewrites_the_stored_url() {
        let (store, _backend, _dir) = test_store(SimulatedFault::None);
        let mut entry = AuthEntry {
            credentials: Some(credentials("t")),
            client: Some(StoredClientInfo::new("client")),
            state: Some(state("csrf")),
            server_url: Some("https://x.example/mcp".to_string()),
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example/mcp")).unwrap();
        store.clear_credentials("srv").unwrap();
        let entry = store.auth_entry("srv").unwrap().unwrap();
        assert!(entry.credentials.is_none());
        assert!(entry.client.is_some());
        assert_eq!(entry.server_url.as_deref(), Some("https://x.example/mcp"));

        store.clear_state("srv").unwrap();
        assert!(store.auth_entry("srv").unwrap().unwrap().state.is_none());
        store.clear_client_info("srv").unwrap();
        let entry = store.auth_entry("srv").unwrap().unwrap();
        assert!(entry.client.is_none());
        assert_eq!(entry.server_url.as_deref(), Some("https://x.example/mcp"));

        // A clearer is a no-op when there is no entry.
        store.clear_credentials("absent").unwrap();
        assert!(store.auth_entry("absent").unwrap().is_none());
    }

    // -- MCP-257 / MCP-259 ---------------------------------------------------------------------

    fn store_with_env(env: EnvFn) -> (McpAuthStore, Arc<MemorySecretStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemorySecretStore::new());
        let store = McpAuthStore::with_backends(
            backend.clone(),
            Arc::new(MemorySecretStore::new()),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::default(),
            env,
        );
        (store, backend, dir)
    }

    #[test]
    fn two_reads_cost_one_backend_read_for_present_and_absent_alike() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        // Absent is cached as an explicit `None`, distinguished from "not cached".
        assert!(store.auth_entry("srv").unwrap().is_none());
        assert!(store.auth_entry("srv").unwrap().is_none());
        assert_eq!(backend.read_count(), 1);

        let mut entry = AuthEntry {
            credentials: Some(credentials("t")),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example")).unwrap();
        let before = backend.read_count();
        // A write publishes, so the next read serves the written value with zero backend reads.
        assert!(store.auth_entry("srv").unwrap().is_some());
        assert!(store.auth_entry("srv").unwrap().is_some());
        assert_eq!(backend.read_count(), before);
    }

    #[test]
    fn inspection_never_populates_or_consumes_the_cache() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        let mut entry = AuthEntry {
            credentials: Some(credentials("t")),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example")).unwrap();

        let before = backend.read_count();
        let _ = store.inspect_auth_for_url("srv", "https://x.example").unwrap();
        let _ = store.inspect_auth_for_url("srv", "https://x.example").unwrap();
        // Two inspections cost two backend reads even though an ordinary read warmed the cache.
        assert_eq!(backend.read_count(), before + 2);
        // …and an ordinary read still costs nothing.
        assert!(store.auth_entry("srv").unwrap().is_some());
        assert_eq!(backend.read_count(), before + 2);
    }

    #[test]
    fn invalidation_reloads_and_evicts_only_its_target() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        let mut entry = AuthEntry {
            credentials: Some(credentials("t")),
            ..AuthEntry::default()
        };
        store.save_auth_entry("a", &mut entry, Some("https://a.example")).unwrap();
        store.save_auth_entry("b", &mut entry, Some("https://b.example")).unwrap();
        let _ = store.auth_entry("a").unwrap();
        let before = backend.read_count();

        store.invalidate_cache("a");
        let _ = store.auth_entry("a").unwrap();
        assert_eq!(backend.read_count(), before + 1);
        let _ = store.auth_entry("b").unwrap();
        assert_eq!(backend.read_count(), before + 1, "b was not evicted");
    }

    #[test]
    fn a_returned_entry_is_isolated_from_the_cached_one() {
        let (store, _backend, _dir) = test_store(SimulatedFault::None);
        let mut client = StoredClientInfo::new("client");
        client.redirect_uris = Some(vec!["https://one.example".to_string()]);
        let mut entry = AuthEntry {
            client: Some(client),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example")).unwrap();

        let mut first = store.auth_entry("srv").unwrap().unwrap();
        first.client.as_mut().unwrap().issuer = Some("mutated".to_string());
        first.client.as_mut().unwrap().redirect_uris.as_mut().unwrap().push("x".to_string());
        first.credentials = None;

        let second = store.auth_entry("srv").unwrap().unwrap();
        let client = second.client.unwrap();
        assert!(client.issuer.is_none(), "nested mutation reached the cache");
        assert_eq!(client.redirect_uris.unwrap().len(), 1);
    }

    #[test]
    fn store_failures_are_never_cached() {
        let (store, backend, _dir) = test_store(SimulatedFault::Unavailable);
        assert!(store.auth_entry("srv").is_err());
        assert!(store.auth_entry("srv").is_err());
        // Both reads reached the backend — a throw must not poison the cache.
        assert_eq!(backend.read_count(), 2);
    }

    #[test]
    fn the_disable_switch_is_read_per_call_and_only_honours_the_literal_one() {
        for (value, expected_reads) in [("1", 2_u64), ("true", 1), ("0", 1), ("", 1)] {
            let owned = value.to_string();
            let (store, backend, _dir) =
                store_with_env(Arc::new(move |key: &str| {
                    (key == AUTH_CACHE_DISABLED_ENV[1]).then(|| owned.clone())
                }));
            assert!(store.auth_entry("srv").unwrap().is_none());
            assert!(store.auth_entry("srv").unwrap().is_none());
            assert_eq!(backend.read_count(), expected_reads, "value = {value:?}");
        }
    }

    #[test]
    fn cyrup_prefixed_names_win_the_dual_read() {
        let env: EnvFn = Arc::new(|key: &str| match key {
            "CYRUP_MCP_DISABLE_AUTH_CACHE" => Some("1".to_string()),
            "PI_MCP_ADAPTER_DISABLE_AUTH_CACHE" => Some("0".to_string()),
            _ => None,
        });
        let (store, _backend, _dir) = store_with_env(env);
        assert!(!store.is_cache_enabled());
    }

    #[test]
    fn remove_evicts_even_when_the_cache_is_disabled() {
        let (store, backend, _dir) = store_with_env(Arc::new(|_| None));
        let mut entry = AuthEntry {
            credentials: Some(credentials("t")),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut entry, Some("https://x.example")).unwrap();
        assert!(store.auth_entry("srv").unwrap().is_some());
        store.remove_auth_entry("srv").unwrap();
        let before = backend.read_count();
        assert!(store.auth_entry("srv").unwrap().is_none());
        assert_eq!(backend.read_count(), before + 1, "the read must reach the backend");
    }

    #[test]
    fn reset_cache_leaves_the_read_counter_alone() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        let _ = store.auth_entry("srv").unwrap();
        assert_eq!(backend.read_count(), 1);
        store.reset_cache();
        assert_eq!(backend.read_count(), 1, "resetAuthEntryCache must not reset the counter");
        backend.reset();
        assert_eq!(backend.read_count(), 0, "resetTestAuthSecretStore resets both");
    }

    #[test]
    fn every_injected_backend_bumps_the_read_counter_including_the_throwing_pair() {
        for fault in [
            SimulatedFault::None,
            SimulatedFault::SizeLimited,
            SimulatedFault::Unavailable,
            SimulatedFault::KeyRevoked,
        ] {
            let backend = MemorySecretStore::with_fault(fault);
            let _ = backend.read("account");
            assert_eq!(backend.read_count(), 1, "{fault:?}");
        }
    }

    // -- MCP-262 / MCP-263 ---------------------------------------------------------------------

    #[test]
    fn the_revoked_predicate_walks_the_whole_chain_and_only_matches_revocation() {
        let revoked = AuthStoreError::Unavailable {
            operation: StoreOp::Read,
            server: "srv".to_string(),
            source: AuthSecretStoreError::Keyring(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::other("KeyRevoked"),
            ))),
        };
        assert!(cause_chain_contains_key_revoked(&revoked));

        for text in ["Key has been revoked", "key revoked", "KEYREVOKED"] {
            let error = AuthSecretStoreError::Keyring(keyring::Error::PlatformFailure(Box::new(
                std::io::Error::other(text),
            )));
            assert!(cause_chain_contains_key_revoked(&error), "{text}");
        }

        let generic = AuthStoreError::Unavailable {
            operation: StoreOp::Read,
            server: "srv".to_string(),
            source: AuthSecretStoreError::Keyring(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::other("permission denied"),
            ))),
        };
        assert!(!cause_chain_contains_key_revoked(&generic));
        // `NoEntry` has no source at all — the walk terminates immediately.
        assert!(!cause_chain_contains_key_revoked(&AuthSecretStoreError::Keyring(
            keyring::Error::NoEntry
        )));
    }

    #[test]
    fn recovery_requires_both_halves_of_the_predicate() {
        let revoked = AuthStoreError::Unavailable {
            operation: StoreOp::Read,
            server: "srv".to_string(),
            source: AuthSecretStoreError::Keyring(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::other("KeyRevoked"),
            ))),
        };
        let forced: EnvFn = Arc::new(|key: &str| {
            (key == TEST_LINUX_KEYRING_RECOVERY_ENV[0]).then(|| "1".to_string())
        });
        assert!(should_attempt_recovery(&forced, &revoked));

        let disabled: EnvFn = Arc::new(|key: &str| match key {
            "CYRUP_MCP_TEST_LINUX_KEYRING_RECOVERY" => Some("1".to_string()),
            "CYRUP_MCP_DISABLE_KEYRING_RECOVERY" => Some("1".to_string()),
            _ => None,
        });
        assert!(!should_attempt_recovery(&disabled, &revoked));

        let generic = AuthStoreError::Unavailable {
            operation: StoreOp::Read,
            server: "srv".to_string(),
            source: AuthSecretStoreError::Recovery("boom".to_string()),
        };
        assert!(
            !should_attempt_recovery(&forced, &generic),
            "a generic failure must never spawn a subprocess"
        );
    }

    #[test]
    fn the_unavailable_sentences_are_verbatim() {
        let generic = AuthStoreError::Unavailable {
            operation: StoreOp::Read,
            server: "srv".to_string(),
            source: AuthSecretStoreError::Recovery("boom".to_string()),
        };
        let message = format_oauth_credential_store_unavailable(&generic);
        assert_eq!(
            message,
            "OAuth credential store unavailable. Configure or unlock the OS credential store and retry."
        );

        let revoked = AuthStoreError::Unavailable {
            operation: StoreOp::Read,
            server: "srv".to_string(),
            source: AuthSecretStoreError::Keyring(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::other("KeyRevoked"),
            ))),
        };
        let message = format_oauth_credential_store_unavailable(&revoked);
        if cfg!(target_os = "linux") {
            assert!(message.contains("session keyring may be revoked"));
            assert!(message.contains("fresh login/keyring session"));
        } else {
            assert_eq!(
                message,
                "OAuth credential store unavailable. Configure or unlock the OS credential store and retry."
            );
        }
    }

    #[test]
    fn the_store_unavailable_sentence_is_verbatim() {
        let error = AuthSecretStoreError::StoreUnavailable {
            source: keyring::Error::NoDefaultStore,
        };
        assert_eq!(
            error.to_string(),
            "OAuth secure credential storage is unavailable. Configure the OS credential store and retry authentication."
        );
    }

    // -- MCP-261 -------------------------------------------------------------------------------

    #[test]
    fn read_and_remove_requests_carry_no_payload_key() {
        let request = KeyringHelperRequest {
            operation: KeyringRecoveryOperation::Read,
            service: AUTH_SECRET_SERVICE.to_string(),
            account: "sha256-aa".to_string(),
            payload: None,
        };
        let body = serde_json::to_string(&request).unwrap();
        assert!(!body.contains("payload"), "{body}");
        assert!(body.contains(r#""operation":"read""#));

        let write = KeyringHelperRequest {
            operation: KeyringRecoveryOperation::Write,
            service: AUTH_SECRET_SERVICE.to_string(),
            account: "sha256-aa".to_string(),
            payload: Some("value".to_string()),
        };
        assert!(serde_json::to_string(&write).unwrap().contains(r#""payload":"value""#));
    }

    #[test]
    fn the_helper_rejects_each_malformed_request_with_its_exact_message() {
        for (request, message) in [
            ("[1,2]", "invalid request"),
            ("{not json", "invalid request"),
            (r#"{"operation":"nope","service":"s","account":"a"}"#, "invalid operation"),
            (r#"{"operation":"read","service":"","account":"a"}"#, "invalid service"),
            (r#"{"operation":"read","service":"s","account":""}"#, "invalid account"),
        ] {
            let mut stdin = std::io::Cursor::new(request.as_bytes().to_vec());
            let mut stdout = Vec::new();
            let code = run_keyring_helper(&mut stdin, &mut stdout);
            assert_eq!(code, 1, "{request}");
            let response: KeyringHelperResponse =
                serde_json::from_slice(stdout.trim_ascii_end()).unwrap();
            assert!(!response.ok);
            assert_eq!(response.error.as_deref(), Some(message), "{request}");
        }
    }

    #[test]
    fn the_helper_rejects_an_oversized_request() {
        let body = vec![b'x'; KEYRING_HELPER_MAX_BYTES + 16];
        let mut stdin = std::io::Cursor::new(body);
        let mut stdout = Vec::new();
        assert_eq!(run_keyring_helper(&mut stdin, &mut stdout), 1);
        let response: KeyringHelperResponse =
            serde_json::from_slice(stdout.trim_ascii_end()).unwrap();
        assert_eq!(response.error.as_deref(), Some("request too large"));
    }

    #[test]
    fn helper_responses_have_the_documented_shapes() {
        let found = KeyringHelperResponse {
            ok: true,
            found: Some(true),
            value: Some("v".to_string()),
            error: None,
        };
        assert_eq!(
            serde_json::to_string(&found).unwrap(),
            r#"{"ok":true,"found":true,"value":"v"}"#
        );
        let missing = KeyringHelperResponse {
            ok: true,
            found: Some(false),
            ..KeyringHelperResponse::default()
        };
        assert_eq!(serde_json::to_string(&missing).unwrap(), r#"{"ok":true,"found":false}"#);
        let done = KeyringHelperResponse {
            ok: true,
            ..KeyringHelperResponse::default()
        };
        assert_eq!(serde_json::to_string(&done).unwrap(), r#"{"ok":true}"#);
    }

    // -- MCP-256 / MCP-267 / MCP-288 -----------------------------------------------------------

    #[test]
    fn the_legacy_expiry_converter_table() {
        let now = 1_700_000_000.0_f64;
        let base = |expires_at: Option<f64>| LegacyStoredTokens {
            access_token: "a".to_string(),
            refresh_token: None,
            expires_at,
            scope: Some("read write".to_string()),
            issuer: None,
        };
        let expires_in = |tokens: &LegacyStoredTokens| -> Option<u64> {
            let credentials = legacy_credentials(tokens, "client", now).expect("credentials");
            let json = serde_json::to_value(credentials.token_response.as_ref().unwrap()).unwrap();
            json.get("expires_in").and_then(serde_json::Value::as_u64)
        };
        assert_eq!(expires_in(&base(None)), None, "absent stays absent");
        // `expiresAt = 0` is `getStoredTokens`'s semantic — already expired — NOT `isTokenExpired`'s
        // "no expiry". Do not "restore" the falsy rule here.
        assert_eq!(expires_in(&base(Some(0.0))), Some(0));
        assert_eq!(expires_in(&base(Some(now - 1.0))), Some(0));
        assert_eq!(expires_in(&base(Some(now + 60.0))), Some(60));

        let credentials = legacy_credentials(&base(Some(now + 60.0)), "client", now).unwrap();
        assert_eq!(credentials.client_id, "client");
        assert_eq!(credentials.granted_scopes, vec!["read".to_string(), "write".to_string()]);
        assert_eq!(credentials.token_received_at, Some(now as u64));
    }

    #[test]
    fn a_legacy_entry_with_no_client_id_drops_its_tokens_rather_than_fabricating_one() {
        let legacy: LegacyAuthEntry = serde_json::from_value(serde_json::json!({
            "tokens": {"accessToken": "a", "expiresAt": 4_000_000_000_i64},
            "serverUrl": "https://x.example/mcp"
        }))
        .unwrap();
        let entry = translate_legacy_entry(legacy, 1_700_000_000.0).expect("entry");
        assert!(entry.credentials.is_none(), "no synthetic client id");
        assert_eq!(entry.server_url.as_deref(), Some("https://x.example/mcp"));
    }

    #[test]
    fn a_legacy_plaintext_file_is_imported_then_deleted() {
        let (store, backend, dir) = test_store(SimulatedFault::None);
        let path = store.auth_entry_file_path("srv");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "tokens": {"accessToken": "a", "expiresAt": 4_000_000_000_i64, "scope": "x"},
                "clientInfo": {"clientId": "c", "clientSecret": "s"},
                "serverUrl": "https://x.example/mcp"
            })
            .to_string(),
        )
        .unwrap();
        assert!(path.exists());

        let entry = store.auth_entry("srv").unwrap().expect("imported");
        assert!(entry.credentials.is_some());
        assert_eq!(entry.client.as_ref().unwrap().client_secret.as_deref(), Some("s"));
        assert!(!path.exists(), "the plaintext file must not survive a successful import");
        assert!(!path.parent().unwrap().exists(), "its directory goes too");
        assert!(!backend.entries().is_empty(), "the record is in the keychain");
        drop(dir);
    }

    #[test]
    fn inspection_does_not_migrate() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        let path = store.auth_entry_file_path("srv");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "clientInfo": {"clientId": "c"},
                "serverUrl": "https://x.example/mcp"
            })
            .to_string(),
        )
        .unwrap();

        let status = store.inspect_auth_for_url("srv", "https://x.example/mcp").unwrap();
        assert!(matches!(status, OAuthCredentialStatus::Present(_)));
        assert!(path.exists(), "a status read must not migrate or delete");
        assert!(backend.entries().is_empty(), "and must not write");
    }

    // -- MCP-280 -------------------------------------------------------------------------------

    #[test]
    fn the_co_installed_upstream_service_is_imported_once_and_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemorySecretStore::new());
        let legacy = Arc::new(MemorySecretStore::new());
        legacy.seed(
            &auth_entry_account("srv"),
            &serde_json::json!({
                "tokens": {"accessToken": "a", "expiresAt": 4_000_000_000_i64},
                "clientInfo": {"clientId": "c"},
                "serverUrl": "https://x.example/mcp"
            })
            .to_string(),
        );
        let store = McpAuthStore::with_backends(
            backend.clone(),
            legacy.clone(),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::default(),
            Arc::new(|_| None),
        );

        let entry = store.auth_entry("srv").unwrap().expect("translated");
        assert!(entry.credentials.is_some());
        assert_eq!(entry.server_url.as_deref(), Some("https://x.example/mcp"));
        // The record now lives under the new service…
        assert!(!backend.entries().is_empty());
        // …and the legacy entry is still there, untouched.
        assert_eq!(legacy.entries().len(), 1);

        // The second read does not touch the legacy service.
        let before = legacy.read_count();
        store.reset_cache();
        let _ = store.auth_entry("srv").unwrap();
        assert_eq!(legacy.read_count(), before);
    }

    #[test]
    fn a_legacy_service_entry_that_fails_translation_leaves_both_services_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemorySecretStore::new());
        let legacy = Arc::new(MemorySecretStore::new());
        legacy.seed(&auth_entry_account("srv"), "{not json");
        let store = McpAuthStore::with_backends(
            backend.clone(),
            legacy.clone(),
            McpDirs::new(dir.path().to_path_buf(), dir.path().to_path_buf()),
            AuthStorageOptions::default(),
            Arc::new(|_| None),
        );
        assert!(store.auth_entry("srv").unwrap().is_none());
        assert!(backend.entries().is_empty(), "nothing half-written");
        assert_eq!(legacy.entries().len(), 1, "the source survives");
    }

    // -- MCP-268 -------------------------------------------------------------------------------

    #[tokio::test]
    async fn concurrent_updates_leave_one_write_intact_and_no_orphan_chunks() {
        let (store, backend, _dir) = test_store(SimulatedFault::None);
        let mut seed = AuthEntry {
            server_url: Some("https://x.example/mcp".to_string()),
            ..AuthEntry::default()
        };
        store.save_auth_entry("srv", &mut seed, Some("https://x.example/mcp")).unwrap();

        let mut tasks = Vec::new();
        for index in 0..8 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .update_credentials_async(
                        "srv",
                        credentials(&format!("token-{index}")),
                        Some("https://x.example/mcp"),
                    )
                    .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        let entry = store.auth_entry_async("srv").await.unwrap().unwrap();
        assert!(entry.credentials.is_some(), "the final entry is one write, intact");
        let account = auth_entry_account("srv");
        let bases: Vec<_> = backend
            .entries()
            .into_iter()
            .filter(|(k, _)| !k.contains(".chunk."))
            .collect();
        assert_eq!(bases.len(), 1);
        assert_eq!(bases[0].0, account);
    }

    // -- MCP-291 -------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_rmcp_stores_round_trip_and_never_report_authorization_required() {
        use rmcp::transport::auth::{CredentialStore as _, StateStore as _};

        let (store, _backend, _dir) = test_store(SimulatedFault::None);
        let url = "https://x.example/mcp".to_string();
        let credential_store =
            McpCredentialStore::new(store.clone(), "srv", Some(url.clone()));
        let state_store = McpStateStore::new(store.clone(), "srv", Some(url.clone()));

        assert!(credential_store.load().await.unwrap().is_none());
        credential_store.save(credentials("live")).await.unwrap();
        assert!(credential_store.load().await.unwrap().is_some());

        state_store.save("csrf-1", state("csrf-1")).await.unwrap();
        assert!(state_store.load("csrf-1").await.unwrap().is_some());
        // A non-matching CSRF token gets `None`, never the stored state.
        assert!(state_store.load("csrf-2").await.unwrap().is_none());
        // …and a stale delete cannot wipe an in-flight flow.
        state_store.delete("csrf-2").await.unwrap();
        assert!(state_store.load("csrf-1").await.unwrap().is_some());
        state_store.delete("csrf-1").await.unwrap();
        assert!(state_store.load("csrf-1").await.unwrap().is_none());

        credential_store.clear().await.unwrap();
        assert!(credential_store.load().await.unwrap().is_none());

        // A store failure arrives as `InternalError`, not `AuthorizationRequired`.
        let (broken, _backend, _dir) = test_store(SimulatedFault::Unavailable);
        let broken_store = McpCredentialStore::new(broken, "srv", Some(url));
        match broken_store.load().await {
            Err(AuthError::InternalError(message)) => {
                assert!(message.contains("Failed to read OAuth credentials"));
            }
            other => panic!("expected InternalError, got {other:?}"),
        }
    }

    // -- MCP-270 / MCP-290 / MCP-084 -----------------------------------------------------------

    #[test]
    fn an_entry_with_a_client_but_no_credentials_inspects_as_absent() {
        let present = OAuthCredentialStatus::Present(AuthEntry {
            client: Some(StoredClientInfo::new("c")),
            server_url: Some("https://x.example".to_string()),
            ..AuthEntry::default()
        });
        assert!(matches!(
            McpOAuthTokenStatus::from(present),
            McpOAuthTokenStatus::Absent
        ));

        let with_tokens = OAuthCredentialStatus::Present(AuthEntry {
            credentials: Some(credentials("t")),
            ..AuthEntry::default()
        });
        assert!(matches!(
            McpOAuthTokenStatus::from(with_tokens),
            McpOAuthTokenStatus::Present(_)
        ));

        let unavailable = OAuthCredentialStatus::Unavailable {
            message: "boom".to_string(),
        };
        match McpOAuthTokenStatus::from(unavailable) {
            McpOAuthTokenStatus::Unavailable { message } => assert_eq!(message, "boom"),
            other => panic!("expected passthrough, got {other:?}"),
        }
    }

    #[test]
    fn a_pre_registered_stub_is_stored_but_never_handed_back_as_client_information() {
        let mut stub = StoredClientInfo::new("configured");
        stub.issuer = Some("https://issuer.example".to_string());
        stub.config_pre_registered = Some(true);
        assert!(stub.is_pre_registered_stub());
        assert!(stub.to_oauth_client_config("http://127.0.0.1/callback").is_none());

        // The legacy shape `{clientId, issuer}` with no secret is a stub too.
        let mut legacy = StoredClientInfo::new("configured");
        legacy.issuer = Some("https://issuer.example".to_string());
        assert!(legacy.is_pre_registered_stub());

        let mut confidential = StoredClientInfo::new("dcr");
        confidential.client_secret = Some("secret".to_string());
        confidential.client_id_issued_at = Some(1.0);
        assert!(!confidential.is_pre_registered_stub());
        let config = confidential
            .to_oauth_client_config("http://127.0.0.1/callback")
            .expect("usable client");
        assert_eq!(config.client_id, "dcr");
        assert_eq!(config.client_secret.as_deref(), Some("secret"));

        // The flag round-trips verbatim through storage.
        let entry = AuthEntry {
            client: Some(stub),
            ..AuthEntry::default()
        };
        let body = serde_json::to_string(&entry).unwrap();
        assert!(body.contains(r#""configPreRegistered":true"#));
        let back: AuthEntry = serde_json::from_str(&body).unwrap();
        assert_eq!(back.client.unwrap().config_pre_registered, Some(true));
    }

    #[test]
    fn bearer_token_resolution_follows_the_secret_expression_grammar() {
        let env: EnvFn = Arc::new(|key: &str| match key {
            "TOKEN" => Some("from-env".to_string()),
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        });
        // `bearerToken` wins over `bearerTokenEnv`.
        assert_eq!(
            resolve_bearer_token(Some("literal"), Some("TOKEN"), &env).as_deref(),
            Some("literal")
        );
        // `!!` is an escaped literal `!`: the leading `!` is dropped and the rest interpolated.
        assert_eq!(
            resolve_bearer_token(Some("!!${HOME}"), None, &env).as_deref(),
            Some("!/home/u")
        );
        // A single `!` is a command marker and survives verbatim for the transport to execute.
        assert_eq!(
            resolve_bearer_token(Some("!op read x"), None, &env).as_deref(),
            Some("!op read x")
        );
        assert!(is_command_secret("!op read x"));
        assert!(!is_command_secret("!!literal"));
        // The three interpolation syntaxes, and a missing variable becoming the empty string.
        assert_eq!(
            resolve_bearer_token(Some("${TOKEN}/$env:TOKEN/{env:TOKEN}/${NOPE}"), None, &env)
                .as_deref(),
            Some("from-env/from-env/from-env/")
        );
        // `bearerTokenEnv` falls back correctly, and an empty name is not truthy.
        assert_eq!(resolve_bearer_token(None, Some("TOKEN"), &env).as_deref(), Some("from-env"));
        assert_eq!(resolve_bearer_token(None, Some(""), &env), None);
        assert_eq!(resolve_bearer_token(None, Some("MISSING"), &env), None);
        assert_eq!(resolve_bearer_token(None, None, &env), None);
    }

    // -- MCP-082 / MCP-342: the interpolation engine, de-duplicated at integration --------------

    #[test]
    fn interpolation_is_three_chained_passes_not_one_alternation() {
        let env: EnvFn = Arc::new(|key: &str| match key {
            // `A` expands to something that is *itself* a later-pass placeholder.
            "A" => Some("$env:B".to_string()),
            "B" => Some("resolved".to_string()),
            // `C` expands to an *earlier*-pass placeholder, which must NOT re-expand.
            "C" => Some("${B}".to_string()),
            _ => None,
        });

        // `utils.ts:74-79` chains three `.replace()` calls, so pass 1's output is pass 2's input.
        // A single alternation pass would leave `$env:B` literal here.
        assert_eq!(interpolate_env_vars("${A}", &env), "resolved");
        // ...and the chaining is one-directional: `{env:C}` runs on pass 3, after `${…}` is done,
        // so the `${B}` it produces is never expanded. This asymmetry is the reason the pass
        // structure has to be ported rather than approximated.
        assert_eq!(interpolate_env_vars("{env:C}", &env), "${B}");
        // All three forms in one string, and a missing variable becoming `""`.
        assert_eq!(
            interpolate_env_vars("[${B}][$env:B][{env:B}][${NOPE}]", &env),
            "[resolved][resolved][resolved][]"
        );
    }

    /// JavaScript's `\w` is **ASCII-only** and Rust's `regex` makes `\w` Unicode-aware, so the
    /// engine spells the class out as `[A-Za-z0-9_]`. Written `\w`, `${café}` interpolated here and
    /// stayed literal upstream — a silent digest divergence on any config with a non-ASCII variable
    /// name (MCP-082/MCP-143).
    ///
    /// Both expectations are node 22 output from upstream's own `interpolateEnvVars` (`utils.ts:74`
    /// @ v2.26.1) with `café` set: `"${café}"` comes back untouched, and `"$env:café"` becomes
    /// `"é"` — the ASCII run `caf` is the name, the rest is literal text.
    #[test]
    fn the_placeholder_name_class_is_ascii_exactly_as_javascripts_is() {
        let env: EnvFn = Arc::new(|key: &str| match key {
            "café" => Some("unicode".to_string()),
            "caf" => None,
            _ => None,
        });
        assert_eq!(interpolate_env_vars("${café}", &env), "${café}");
        assert_eq!(interpolate_env_vars("$env:café", &env), "é");
        assert_eq!(interpolate_env_vars("{env:café}", &env), "{env:café}");
    }

    // -- MCP-084: `resolveServerUrl`, the one identity resolver that throws ----------------------

    /// `getMissingEnvVars` + `resolveServerUrl` (`utils.ts:83`, `:167`).
    ///
    /// Every expectation is node 22 output from upstream at v2.26.1, including the three message
    /// forms. The two properties worth naming: the scan is **one alternation over all three
    /// syntaxes**, so a `{env:MISSING}` throws exactly as a `${MISSING}` does; and it reports names
    /// in **first-occurrence order**, deduplicated, which is the order the message prints.
    #[test]
    fn resolve_server_url_interpolates_and_throws_the_way_upstream_does() {
        let env: EnvFn = Arc::new(|key: &str| match key {
            "HOST" => Some("a.example".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        });

        assert_eq!(resolve_server_url(None, &env).expect("absent"), None);
        assert_eq!(
            resolve_server_url(Some("https://api.example.com/mcp"), &env)
                .expect("already absolute")
                .as_deref(),
            Some("https://api.example.com/mcp")
        );
        assert_eq!(
            resolve_server_url(Some("https://${HOST}/mcp"), &env)
                .expect("resolves")
                .as_deref(),
            Some("https://a.example/mcp")
        );
        // Set-but-empty is PRESENT: upstream tests `=== undefined`, not truthiness.
        assert_eq!(
            resolve_server_url(Some("https://x.example/${EMPTY}a"), &env)
                .expect("EMPTY is set")
                .as_deref(),
            Some("https://x.example/a")
        );

        for (url, message) in [
            (
                "https://x.example/${NOPE}",
                "Missing environment variable in MCP server URL: NOPE",
            ),
            (
                "https://x.example/$env:NOPE",
                "Missing environment variable in MCP server URL: NOPE",
            ),
            (
                "https://x.example/{env:NOPE}",
                "Missing environment variable in MCP server URL: NOPE",
            ),
            (
                "https://x.example/${NOPE}/${ALSONOPE}",
                "Missing environment variables in MCP server URL: NOPE, ALSONOPE",
            ),
            (
                "https://x.example/${NOPE}/${NOPE}",
                "Missing environment variable in MCP server URL: NOPE",
            ),
            (
                "not a url",
                "Invalid MCP server URL after environment interpolation: not a url",
            ),
            (
                "${HOST}",
                "Invalid MCP server URL after environment interpolation: a.example",
            ),
        ] {
            assert_eq!(
                resolve_server_url(Some(url), &env).expect_err(url).to_string(),
                message,
                "{url}"
            );
        }

        // The parser is WHATWG on both sides: measured against node's `new URL`, these four are
        // accepted and these two are rejected.
        for accepted in ["unix:///tmp/s.sock", "x:y", "mailto:a@b", "ws://x"] {
            assert!(resolve_server_url(Some(accepted), &env).is_ok(), "{accepted}");
        }
        for rejected in ["//x/y", "/abs/path"] {
            assert!(resolve_server_url(Some(rejected), &env).is_err(), "{rejected}");
        }

        assert_eq!(missing_env_vars("no placeholders", &env), Vec::<String>::new());
        assert_eq!(
            missing_env_vars("{env:B}/${A}", &env),
            vec!["B".to_string(), "A".to_string()],
            "first-occurrence order across the three forms"
        );
    }

    // -- MCP-321: `McpAuthStore` IS the OAuth flow's storage ------------------------------------

    #[tokio::test]
    async fn the_keyring_store_satisfies_the_oauth_storage_seam() {
        use crate::oauth::McpOAuthStorage as _;

        let (store, _backend, _dir) = test_store(SimulatedFault::None);
        let url = "https://x.example/mcp";

        assert!(store.load("srv").await.unwrap().is_none());
        assert!(store.get_auth_for_url("srv", url).await.unwrap().is_none());

        store
            .save_credentials("srv", url, Some(credentials("access-1")))
            .await
            .unwrap();
        let entry = store.get_auth_for_url("srv", url).await.unwrap().unwrap();
        assert!(entry.credentials.is_some());
        // The binding is exact-string: a different URL reads as absent, never as present.
        assert!(store.get_auth_for_url("srv", "https://y.example/mcp").await.unwrap().is_none());

        // `clearTokens` takes no URL (`mcp-auth.ts:994`), so the sibling client record survives.
        store
            .save_client(
                "srv",
                url,
                Some(StoredClientInfo {
                    client_id: "cid".to_string(),
                    client_secret: None,
                    client_id_issued_at: None,
                    client_secret_expires_at: None,
                    redirect_uris: None,
                    issuer: None,
                    config_pre_registered: Some(true),
                }),
            )
            .await
            .unwrap();
        store.save_credentials("srv", url, None).await.unwrap();
        let entry = store.load("srv").await.unwrap().unwrap();
        assert!(entry.credentials.is_none());
        assert!(entry.client.is_some(), "clearTokens must not purge the client record");

        assert_eq!(store.base_dir(), store.auth_base_dir());
        store.clear_all("srv").await.unwrap();
        assert!(store.load("srv").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_broken_store_crosses_the_seam_as_credential_store_never_as_other() {
        use crate::oauth::McpOAuthStorage as _;

        let (store, _backend, _dir) = test_store(SimulatedFault::Unavailable);
        let error = store.load("srv").await.unwrap_err();
        // Section 07's refresh driver rethrows the store class and swallows everything else into
        // `None`; misclassifying here is an infinite silent re-auth loop.
        assert!(error.is_credential_store_failure(), "{error}");
        assert!(matches!(&error, crate::errors::McpError::CredentialStore(inner)
            if inner.is_store_unavailable()));
    }
}
