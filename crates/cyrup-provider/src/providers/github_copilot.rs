//! The GitHub Copilot provider (arch-01 §5) — a 1:1 port of Pi v0.83.0
//! `packages/ai/src/providers/github-copilot.ts` plus the runtime half of
//! `packages/ai/src/auth/oauth/github-copilot.ts`.
//!
//! Copilot is the only built-in that speaks all three of cyrup's OpenAI/Anthropic wire protocols
//! from ONE catalog: `anthropic-messages` (the Claude models), `openai-completions` (Fable/Kimi) and
//! `openai-responses` (the GPT/Gemini/MAI families) — `github-copilot.ts:27-31`. It is also the only
//! one whose request base URL is *derived from the credential* rather than fixed: the Copilot token
//! embeds a `proxy-ep=` claim naming the account's edge, and every request must go there
//! (`auth/oauth/github-copilot.ts:64-85`). That derivation is why `to_auth` returns a `base_url`
//! override; [`crate::wire::WireProvider`] hands it to the api impl, which prefers it over
//! `model.base_url` (`api/anthropic_messages.rs:443-450`).
//!
//! # Catalog provenance — read before refreshing
//!
//! The 28 models in `catalog/github-copilot.json` are the verbatim contents of Pi
//! `packages/ai/src/providers/github-copilot.models.ts` at commit `b0c2a90e` (2026-07-17), the LAST
//! revision at which Pi tracks this catalog's literal data in git. One commit later (`a9f6a315`,
//! "feat(ai): separate generated model data") the data moved to
//! `packages/ai/src/providers/data/github-copilot.json`, which `.gitignore:11` excludes — so at the
//! ported tag `v0.83.0` the catalog is not obtainable from the repository at all, and `b0c2a90e` is
//! the closest knowable snapshot to it.
//!
//! **This note used to argue why THIS catalog was deliberately newer than its siblings' `91585d9a`.
//! That argument is settled and the exception is gone: since 2026-08-15 every embedded catalog is
//! generated from `b0c2a90e` by `cargo run -p xtask -- gen-catalogs` (PROV-018/PROV-060).** The
//! evidence it rested on is why: `91585d9a` was demonstrably stale against the ported tag on points
//! that change behaviour, not just pricing —
//!
//! - `mai-code-1-flash-picker` moved from `openai-completions` to `openai-responses` in pi
//!   `f7b78e2a` (2026-07-13, "route GitHub Copilot MAI-Code models through /responses endpoint").
//!   Shipping the `91585d9a` row would POST chat-completions bodies to a `/responses` model — the
//!   same defect PROV-054 later found on `xai/grok-4.5`, which nobody had made this argument for.
//! - `gemini-3-flash-preview`'s context window went 200k → 1M in `9eedaf8c`.
//! - The `gpt-5.6-{luna,sol,terra}` trio landed for Copilot after `91585d9a`.
//!
//! Generalising this one file's reasoning to all 35 is exactly what the regeneration did. Do not
//! hand-edit this file — `gen-catalogs --check` fails if you do.
//!
//! # Where the login lives
//!
//! `loginGitHubCopilot` (`auth/oauth/github-copilot.ts:331-357`) — the GitHub device-code flow, the
//! `enableAllGitHubCopilotModels` policy-acceptance pass and the interactive enterprise-domain
//! prompt — is ported in [`crate::auth::oauth::github_copilot::GitHubCopilotLogin`], beside the
//! other flows and on that module's device-code poller. [`GitHubCopilotOAuth`] below is the
//! *runtime half* of the same upstream object: the two [`crate::auth::OAuthAuth`] members a stored
//! credential needs, `refresh` and `to_auth`, which `GitHubCopilotLogin` delegates to.
//! [`github_copilot_auth`] wires the full flow, so `/login` reaches it (PROV-029).

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::types::{AuthContext, EnvAuthContext, ModelAuth};
use crate::auth::{
    Credential, CredentialStore, InMemoryCredentialStore, OAuthAuth, ProviderAuth, env_key,
};
use crate::error::AuthError;
use crate::model::Model;
use crate::wire::WireProvider;
use cyrup_core::ProviderId;
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

/// The provider id (Pi `github-copilot.ts:11`).
pub const GITHUB_COPILOT_PROVIDER_ID: &str = "github-copilot";

/// The default (individual-plan) API base URL. Used only when the access token carries no
/// `proxy-ep=` claim and no enterprise domain is configured (Pi `github-copilot.ts:13`,
/// `auth/oauth/github-copilot.ts:83`).
pub const GITHUB_COPILOT_BASE_URL: &str = "https://api.individual.githubcopilot.com";

/// The env var carrying a Copilot token (Pi `envApiKeyAuth("GitHub Copilot token",
/// ["COPILOT_GITHUB_TOKEN"])`, `github-copilot.ts:15`). Already mapped in
/// [`crate::env_api_keys`] (`env_api_keys.rs:36-37`).
pub const COPILOT_GITHUB_TOKEN_ENV: &str = "COPILOT_GITHUB_TOKEN";

/// The default GitHub domain when no enterprise domain is configured (Pi
/// `auth/oauth/github-copilot.ts:236`).
pub const DEFAULT_GITHUB_DOMAIN: &str = "github.com";

/// The editor-identity headers Copilot's edge requires on every call (Pi `COPILOT_HEADERS`,
/// `auth/oauth/github-copilot.ts:11-16`). They are also baked into every catalog row's
/// `model.headers`, which is how they reach *request* traffic; this constant is what the OAuth
/// token/model-listing calls send.
pub const COPILOT_HEADERS: [(&str, &str); 4] = [
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];

/// `X-GitHub-Api-Version` sent on the model-listing call (Pi `COPILOT_API_VERSION`,
/// `auth/oauth/github-copilot.ts:17`).
pub const COPILOT_API_VERSION: &str = "2026-06-01";

/// Deadline on the model-listing call (Pi `AbortSignal.timeout(5000)`,
/// `auth/oauth/github-copilot.ts:123`).
const MODELS_REQUEST_TIMEOUT: Duration = Duration::from_millis(5000);

/// Safety margin subtracted from the Copilot token's `expires_at` (Pi
/// `expiresAt * 1000 - 5 * 60 * 1000`, `auth/oauth/github-copilot.ts:260`). Note this is a *second*
/// margin on top of [`crate::auth::resolve`]'s five-minute refresh window — Pi stacks them the same
/// way, so a Copilot token is refreshed roughly ten minutes before its true expiry.
const COPILOT_TOKEN_EXPIRY_SKEW_MS: i64 = 5 * 60 * 1000;

/// The `auth.json` key holding the enterprise host (Pi `OAuthCredential.enterpriseUrl`,
/// `auth/oauth/github-copilot.ts:262`); flattened into [`Credential::Oauth`]'s `ext`.
const EXT_ENTERPRISE_URL: &str = "enterpriseUrl";

/// The `auth.json` key holding the account's selectable model ids (Pi
/// `OAuthCredential.availableModelIds`, `auth/oauth/github-copilot.ts:274`).
const EXT_AVAILABLE_MODEL_IDS: &str = "availableModelIds";

/// The verbatim catalog extracted from Pi's generated `github-copilot.models.ts` (see the
/// module-level provenance note).
const GITHUB_COPILOT_CATALOG_JSON: &str = include_str!("catalog/github-copilot.json");

/// A failure raised by the Copilot OAuth flow. Pi throws bare `Error(message)` strings
/// (`auth/oauth/github-copilot.ts:100/131/246/254`); this carries the SAME message text so the
/// operator-visible string is unchanged, wrapped as [`AuthError::OAuth`]'s `source` because cyrup's
/// introspection paths are typed rather than throwing (`error.rs:1-8`).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CopilotAuthError(pub String);

impl CopilotAuthError {
    fn new(msg: impl Into<String>) -> Self {
        CopilotAuthError(msg.into())
    }
}

/// The full Copilot catalog (1:1 with Pi `GITHUB_COPILOT_MODELS`). A parse failure yields an empty
/// catalog (surfaced loudly by the count test) rather than a panic (NO-PANIC policy).
pub fn github_copilot_models() -> Vec<Model> {
    serde_json::from_str(GITHUB_COPILOT_CATALOG_JSON).unwrap_or_default()
}

/// The Copilot [`ProviderAuth`] (Pi `github-copilot.ts:16-19`): an API key from
/// `$COPILOT_GITHUB_TOKEN`, **plus** the OAuth strategy — Copilot is one of the few providers Pi
/// gives both.
/// PROV-029: the strategy is the FULL flow
/// ([`crate::auth::oauth::github_copilot::GitHubCopilotLogin`]) — `login` + `refresh` + `to_auth` —
/// not the runtime half alone. Wiring [`GitHubCopilotOAuth`] here left `login` on the
/// [`OAuthAuth`] trait default, so `/login github-copilot` reported `LoginUnsupported` even though
/// the whole device-code flow was ported and tested.
///
/// `[CYRUP-DELTA]` pi wraps this in `lazyOAuth({ name, load: loadGitHubCopilotOAuth })`
/// (`github-copilot.ts:16`) so a bundler cannot follow a *variable* dynamic `import()` into
/// Node-only flow code (`auth/oauth/load.ts:9-12`). Rust links statically: the flow module is in
/// the binary either way, there is no import to defer, and `auth/oauth/load.rs`'s own note records
/// that the bundled-registration path is the only path here. Naming the flow directly is the same
/// object pi's `load` resolves to, minus an indirection that in Rust can only *fail* — an
/// unregistered loader yields `FlowUnavailable` where pi would always resolve.
pub fn github_copilot_auth() -> ProviderAuth {
    ProviderAuth {
        api_key: Some(env_key("GitHub Copilot token", [COPILOT_GITHUB_TOKEN_ENV])),
        oauth: Some(Arc::new(
            crate::auth::oauth::github_copilot::GitHubCopilotLogin::new(),
        )),
    }
}

/// Construct the GitHub Copilot provider over the given credential store + shared api registry. The
/// registry MUST provide `anthropic-messages`, `openai-completions` and `openai-responses` (use
/// [`builtin_registry`]) — Pi `github-copilot.ts:27-31`.
pub fn github_copilot_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        GITHUB_COPILOT_PROVIDER_ID,
        "GitHub Copilot",
        github_copilot_models(),
        github_copilot_auth(),
        store,
        registry,
    )
    // Pi `createProvider({ …, filterModels })` (`providers/github-copilot.ts:19-27` @v0.83.0).
    .with_filter_models(filter_github_copilot_models)
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn github_copilot_provider() -> WireProvider {
    github_copilot_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

// ---------------------------------------------------------------------------------------------
// Host / base-URL derivation (Pi `auth/oauth/github-copilot.ts:39-85`)
// ---------------------------------------------------------------------------------------------

/// Normalize a user-supplied GitHub Enterprise URL or bare domain to its host (Pi `normalizeDomain`,
/// `auth/oauth/github-copilot.ts:39-49`: trim, empty → null, prepend `https://` when there is no
/// scheme, then return `URL.hostname`; a parse failure → null).
///
/// `[CYRUP-DELTA]` Pi calls the ambient WHATWG `new URL()`. cyrup-provider has no `url` crate in its
/// dependency list (`Cargo.toml:22-36`) and this module may not add one, so the host is extracted
/// directly: optional `scheme://`, optional `userinfo@`, then the host up to the first `:` (port),
/// `/`, `?` or `#`, with `[…]` kept whole for IPv6 literals and ASCII lowercased as WHATWG does.
/// That covers every form this flow accepts (a bare domain, or a URL with scheme/userinfo/port/path).
/// Pi can also return the empty string here (a non-special scheme with an empty host); this returns
/// `None` instead, which is behaviourally identical because both call sites treat `""` as absent —
/// `if (trimmed && !enterpriseDomain) throw` (`:341`) and `enterpriseDomain || "github.com"`
/// (`:236`) are both falsy on `""`.
pub fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // `trimmed.includes("://") ? trimmed : "https://" + trimmed` (`:43`). When a scheme is present it
    // must be a syntactically valid one, or `new URL` throws.
    let after_scheme = match trimmed.find("://") {
        None => trimmed,
        Some(idx) => {
            let scheme = trimmed.get(..idx).unwrap_or("");
            let mut chars = scheme.chars();
            let valid = match chars.next() {
                Some(c) if c.is_ascii_alphabetic() => {
                    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                }
                _ => false,
            };
            if !valid {
                return None;
            }
            trimmed.get(idx + "://".len()..).unwrap_or("")
        }
    };

    // Authority ends at the first `/`, `?` or `#`.
    let authority = match after_scheme.find(['/', '?', '#']) {
        Some(idx) => after_scheme.get(..idx).unwrap_or(""),
        None => after_scheme,
    };
    // `userinfo@host` — the LAST `@` separates them.
    let host_port = match authority.rfind('@') {
        Some(idx) => authority.get(idx + 1..).unwrap_or(""),
        None => authority,
    };
    // IPv6 literals keep their brackets; otherwise the host ends at the port separator.
    let host = if host_port.starts_with('[') {
        // An unterminated literal is a parse failure — `new URL` throws, so this returns `None`.
        let close = host_port.find(']')?;
        host_port.get(..=close).unwrap_or("")
    } else {
        match host_port.find(':') {
            Some(idx) => host_port.get(..idx).unwrap_or(""),
            None => host_port,
        }
    };

    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Parse the `proxy-ep=` claim out of a Copilot token and turn it into an API base URL (Pi
/// `getBaseUrlFromToken`, `auth/oauth/github-copilot.ts:64-73`).
///
/// Token shape: `tid=…;exp=…;proxy-ep=proxy.individual.githubcopilot.com;…`. The `proxy.` prefix
/// becomes `api.`; a host without that prefix is used as-is.
///
/// `[CYRUP-DELTA]` Pi uses `/proxy-ep=([^;]+)/`; this scans for the same literal and takes the run up
/// to the next `;`, which is the identical match set (`[^;]+` is greedy but cannot cross a `;`) —
/// and, like the regex, keeps scanning when a candidate's value is empty.
pub fn base_url_from_token(token: &str) -> Option<String> {
    const MARKER: &str = "proxy-ep=";
    for (idx, _) in token.match_indices(MARKER) {
        let rest = token.get(idx + MARKER.len()..).unwrap_or("");
        let proxy_host = rest.split(';').next().unwrap_or("");
        if proxy_host.is_empty() {
            continue; // `[^;]+` requires at least one character; the regex would try further on
        }
        let api_host = match proxy_host.strip_prefix("proxy.") {
            Some(rest) => format!("api.{rest}"),
            None => proxy_host.to_string(),
        };
        return Some(format!("https://{api_host}"));
    }
    None
}

/// The API base URL for a request (Pi `getGitHubCopilotBaseUrl`,
/// `auth/oauth/github-copilot.ts:75-85`): the token's `proxy-ep=` wins, then the enterprise
/// `copilot-api.<domain>` host, then the individual-plan default.
///
/// Empty strings are treated as absent, matching JS truthiness at `:77` and `:82`.
pub fn github_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
    if let Some(token) = token.filter(|t| !t.is_empty())
        && let Some(url) = base_url_from_token(token)
    {
        return url;
    }
    if let Some(domain) = enterprise_domain.filter(|d| !d.is_empty()) {
        return format!("https://copilot-api.{domain}");
    }
    GITHUB_COPILOT_BASE_URL.to_string()
}

/// The enterprise host recorded on a credential, re-normalized (Pi `copilotEnterpriseDomain`,
/// `auth/oauth/github-copilot.ts:359-363`): a non-string or empty `enterpriseUrl` → `None`.
fn copilot_enterprise_domain(cred: &Credential) -> Option<String> {
    let ext = match cred {
        Credential::Oauth { ext, .. } => ext,
        Credential::ApiKey { .. } => return None,
    };
    let url = ext.get(EXT_ENTERPRISE_URL)?.as_str()?;
    if url.is_empty() {
        return None;
    }
    normalize_domain(url)
}

// ---------------------------------------------------------------------------------------------
// Model availability (Pi `github-copilot.ts:20-26` + `auth/oauth/github-copilot.ts:87-113`)
// ---------------------------------------------------------------------------------------------

/// Whether a `/models` entry is selectable (Pi `isSelectableCopilotModel`,
/// `auth/oauth/github-copilot.ts:91-96`): `model_picker_enabled === true`, `policy.state !==
/// "disabled"`, `capabilities.supports.tool_calls !== false`. Note the asymmetry — the picker flag
/// must be exactly `true`, while the other two only exclude their explicit negative.
fn is_selectable_copilot_model(item: &Map<String, Value>) -> bool {
    let picker_enabled = item.get("model_picker_enabled") == Some(&Value::Bool(true));
    let policy_disabled = item
        .get("policy")
        .and_then(Value::as_object)
        .and_then(|p| p.get("state"))
        .and_then(Value::as_str)
        == Some("disabled");
    let tool_calls_false = item
        .get("capabilities")
        .and_then(Value::as_object)
        .and_then(|c| c.get("supports"))
        .and_then(Value::as_object)
        .and_then(|s| s.get("tool_calls"))
        == Some(&Value::Bool(false));
    picker_enabled && !policy_disabled && !tool_calls_false
}

/// Extract the selectable model ids from a Copilot `/models` body (Pi
/// `parseAvailableCopilotModelIds`, `auth/oauth/github-copilot.ts:98-113`). A body whose `data` is
/// not an array is the error `"Invalid Copilot models response"`, verbatim.
pub fn parse_available_copilot_model_ids(raw: &Value) -> Result<Vec<String>, CopilotAuthError> {
    let data = raw
        .as_object()
        .and_then(|o| o.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| CopilotAuthError::new("Invalid Copilot models response"))?;

    let mut ids = Vec::new();
    for raw_item in data {
        let Some(item) = raw_item.as_object() else {
            continue; // `asRecord` → undefined, so `item?.id` is undefined and the row is skipped
        };
        if let Some(id) = item.get("id").and_then(Value::as_str)
            && is_selectable_copilot_model(item)
        {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}

/// Restrict the catalog to what the credential says the account can select (Pi
/// `Provider.filterModels`, `github-copilot.ts:20-26`).
///
/// Returns the catalog unchanged unless the credential is an OAuth one carrying an
/// `availableModelIds` array of strings — a non-OAuth credential, a missing key, a non-array, or an
/// array with any non-string element all fall through untouched (`:22-25`).
///
/// Installed on the provider through `WireProvider::with_filter_models` — pi's
/// `createProvider({ filterModels })` transport (`models.ts:545`/`:618`) — and applied by
/// [`crate::collection::Models::get_available`] at pi's exact position, `models.ts:407`
/// (PROV-032). `models()` still returns the complete 28-row catalog, as pi's `getModels()` does.
pub fn filter_github_copilot_models(
    models: &[Model],
    credential: Option<&Credential>,
) -> Vec<Model> {
    let ext = match credential {
        Some(Credential::Oauth { ext, .. }) => ext,
        // `credential?.type !== "oauth"` — undefined or api_key: unfiltered (`:22`).
        _ => return models.to_vec(),
    };
    let Some(raw) = ext.get(EXT_AVAILABLE_MODEL_IDS).and_then(Value::as_array) else {
        return models.to_vec(); // `!Array.isArray(availableModelIds)` (`:24`)
    };
    // `availableModelIds.every((id) => typeof id === "string")` (`:24`) — one non-string voids the
    // whole filter rather than dropping that entry.
    let mut available: Vec<&str> = Vec::with_capacity(raw.len());
    for id in raw {
        match id.as_str() {
            Some(s) => available.push(s),
            None => return models.to_vec(),
        }
    }
    models
        .iter()
        .filter(|m| available.contains(&m.id.as_str()))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The OAuth strategy (Pi `auth/oauth/github-copilot.ts:365-378`, minus `login`)
// ---------------------------------------------------------------------------------------------

/// Origin overrides used by this module's loopback tests so nothing in the suite can reach a real
/// GitHub host. Production always leaves this `None` and derives both origins from the domain / the
/// token's `proxy-ep=` claim.
#[derive(Clone, Debug)]
struct OriginOverride {
    /// Replaces `https://api.<domain>` for the `copilot_internal/v2/token` exchange.
    token_origin: String,
    /// Replaces [`github_copilot_base_url`] for the `/models` listing.
    api_origin: String,
}

/// The **runtime half** of GitHub Copilot's OAuth strategy: exchange the stored GitHub token for a
/// short-lived Copilot token, and derive each request's endpoint from that token (Pi
/// `githubCopilotOAuth`, `auth/oauth/github-copilot.ts:365-378`).
///
/// **Not the wired strategy.** [`github_copilot_auth`] carries
/// [`crate::auth::oauth::github_copilot::GitHubCopilotLogin`], which owns the full upstream object
/// — `login` (the RFC 8628 device grant) plus `refresh`/`to_auth`. This type is retained because
/// `GitHubCopilotLogin` **delegates** `refresh` and `to_auth` to it (it holds one as its `runtime`
/// field, `auth/oauth/github_copilot.rs:398`), so those two methods have exactly one
/// implementation. Nothing reaches this type's own [`OAuthAuth`] impl from `/login`; before
/// PROV-029 this WAS the wired strategy, and because it leaves `login` on the trait default,
/// `/login github-copilot` reported `LoginUnsupported` against a fully ported flow.
///
/// `[CYRUP-DELTA]` Pi wraps this in `lazyOAuth` (`auth/helpers.ts:38-56`) so the Node-only flow code
/// stays out of browser bundles. Rust links statically and has no bundle to split, so the strategy
/// is constructed directly; the observable behavior of `refresh`/`to_auth` is unchanged.
pub struct GitHubCopilotOAuth {
    /// Ambient context for HTTP(S)_PROXY / NO_PROXY resolution on the two OAuth calls, so they honor
    /// the same proxy policy as provider traffic (`stream/sse.rs:181-192`).
    auth_ctx: Arc<dyn AuthContext>,
    origin_override: Option<OriginOverride>,
}

impl Default for GitHubCopilotOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubCopilotOAuth {
    pub fn new() -> Self {
        Self {
            auth_ctx: Arc::new(EnvAuthContext),
            origin_override: None,
        }
    }

    /// Override the ambient auth context (for tests / custom env sources), mirroring
    /// [`WireProvider::with_auth_context`].
    #[must_use]
    pub fn with_auth_context(mut self, ctx: Arc<dyn AuthContext>) -> Self {
        self.auth_ctx = ctx;
        self
    }

    /// Point both GitHub origins at a loopback listener. Test-only seam (R-06 "never hit the
    /// network in a test"); it does not exist in release builds.
    #[cfg(test)]
    fn with_origin_override(mut self, token_origin: &str, api_origin: &str) -> Self {
        self.origin_override = Some(OriginOverride {
            token_origin: token_origin.to_string(),
            api_origin: api_origin.to_string(),
        });
        self
    }

    /// `https://api.<domain>/copilot_internal/v2/token` (Pi `getUrls().copilotTokenUrl`,
    /// `auth/oauth/github-copilot.ts:56`).
    fn copilot_token_url(&self, domain: &str) -> String {
        match &self.origin_override {
            Some(o) => format!("{}/copilot_internal/v2/token", o.token_origin),
            None => format!("https://api.{domain}/copilot_internal/v2/token"),
        }
    }

    /// The `/models` origin (Pi `fetchAvailableGitHubCopilotModelIds`,
    /// `auth/oauth/github-copilot.ts:116-117`).
    fn models_origin(&self, copilot_token: &str, enterprise_domain: Option<&str>) -> String {
        match &self.origin_override {
            Some(o) => o.api_origin.clone(),
            None => github_copilot_base_url(Some(copilot_token), enterprise_domain),
        }
    }

    /// `fetchJson` (Pi `auth/oauth/github-copilot.ts:126-133`): GET, non-2xx →
    /// `"<status> <statusText>: <body>"`.
    async fn fetch_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        timeout: Option<Duration>,
    ) -> Result<Value, CopilotAuthError> {
        let client =
            crate::stream::sse::build_client_for_target(url, self.auth_ctx.as_ref(), None, None)
                .await
                .map_err(|e| CopilotAuthError::new(e.to_string()))?;
        let mut request = client.get(url);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| CopilotAuthError::new(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            // `${response.status} ${response.statusText}: ${text}` (`:130`). reqwest exposes the
            // canonical reason phrase rather than the server's literal one; they agree for every
            // standard code.
            let text = response.text().await.unwrap_or_default();
            return Err(CopilotAuthError::new(format!(
                "{} {}: {text}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )));
        }
        response
            .json::<Value>()
            .await
            .map_err(|e| CopilotAuthError::new(e.to_string()))
    }

    /// Exchange a GitHub token for a Copilot token (Pi `refreshGitHubCopilotAccessToken`,
    /// `auth/oauth/github-copilot.ts:232-266`).
    async fn refresh_access_token(
        &self,
        refresh_token: &str,
        enterprise_domain: Option<&str>,
    ) -> Result<Credential, CopilotAuthError> {
        let domain = enterprise_domain
            .filter(|d| !d.is_empty())
            .unwrap_or(DEFAULT_GITHUB_DOMAIN);
        let url = self.copilot_token_url(domain);

        let mut headers: Vec<(&str, String)> = vec![
            ("Accept", "application/json".to_string()),
            ("Authorization", format!("Bearer {refresh_token}")),
        ];
        headers.extend(COPILOT_HEADERS.iter().map(|(k, v)| (*k, (*v).to_string())));

        let raw = self.fetch_json(&url, &headers, None).await?;

        let obj = raw
            .as_object()
            .ok_or_else(|| CopilotAuthError::new("Invalid Copilot token response"))?;
        // `typeof token !== "string" || typeof expiresAt !== "number"` (`:254`).
        let token = obj
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| CopilotAuthError::new("Invalid Copilot token response fields"))?;
        let expires_at = obj
            .get("expires_at")
            .and_then(Value::as_f64)
            .ok_or_else(|| CopilotAuthError::new("Invalid Copilot token response fields"))?;

        let mut ext = Map::new();
        // JS spreads `enterpriseUrl: enterpriseDomain`; `undefined` is dropped by JSON
        // serialization, so the key is written only when a domain is configured (`:262`).
        if let Some(domain) = enterprise_domain.filter(|d| !d.is_empty()) {
            ext.insert(
                EXT_ENTERPRISE_URL.to_string(),
                Value::String(domain.to_string()),
            );
        }
        Ok(Credential::Oauth {
            refresh: refresh_token.to_string(),
            access: token.to_string(),
            expires: (expires_at * 1000.0) as i64 - COPILOT_TOKEN_EXPIRY_SKEW_MS,
            ext,
        })
    }

    /// List the account's selectable model ids (Pi `fetchAvailableGitHubCopilotModelIds`,
    /// `auth/oauth/github-copilot.ts:115-125`).
    async fn fetch_available_model_ids(
        &self,
        copilot_token: &str,
        enterprise_domain: Option<&str>,
    ) -> Result<Vec<String>, CopilotAuthError> {
        let base_url = self.models_origin(copilot_token, enterprise_domain);
        let url = format!("{base_url}/models");

        let mut headers: Vec<(&str, String)> = vec![
            ("Accept", "application/json".to_string()),
            ("Authorization", format!("Bearer {copilot_token}")),
        ];
        headers.extend(COPILOT_HEADERS.iter().map(|(k, v)| (*k, (*v).to_string())));
        headers.push(("X-GitHub-Api-Version", COPILOT_API_VERSION.to_string()));

        let raw = self
            .fetch_json(&url, &headers, Some(MODELS_REQUEST_TIMEOUT))
            .await?;
        parse_available_copilot_model_ids(&raw)
    }
}

#[async_trait::async_trait]
impl OAuthAuth for GitHubCopilotOAuth {
    /// Pi `githubCopilotOAuth.name` (`auth/oauth/github-copilot.ts:366`).
    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    /// `isSubscription: true` — set on both the flow (pi v0.84.1
    /// `auth/oauth/github-copilot.ts:402`) and the provider's `lazyOAuth` wrapper
    /// (`providers/github-copilot.ts:16`).
    ///
    /// The strategy [`github_copilot_auth`] actually carries is
    /// [`crate::auth::oauth::github_copilot::GitHubCopilotLogin`], whose own `is_subscription`
    /// (`auth/oauth/github_copilot.rs:813`) is the answer `/login` observes; this one must agree
    /// with it because both stand for the same upstream object. Changing one without the other is
    /// the bug this doc used to invite by claiming *this* impl was the wired one.
    fn is_subscription(&self) -> bool {
        true
    }

    /// Pi `refreshGitHubCopilotToken` (`auth/oauth/github-copilot.ts:271-277`): mint a fresh Copilot
    /// token, then re-list the account's models onto it. A failure of EITHER call fails the refresh —
    /// Pi awaits the listing inside the returned object literal, so it is not best-effort.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        let provider = ProviderId::from(GITHUB_COPILOT_PROVIDER_ID);
        let refresh_token = match cred {
            Credential::Oauth { refresh, .. } => refresh.clone(),
            Credential::ApiKey { .. } => {
                return Err(AuthError::oauth(
                    provider,
                    CopilotAuthError::new("Invalid Copilot token response"),
                ));
            }
        };
        let enterprise = copilot_enterprise_domain(cred);

        let refreshed = self
            .refresh_access_token(&refresh_token, enterprise.as_deref())
            .await
            .map_err(|e| AuthError::oauth(provider.clone(), e))?;

        let (access, expires, mut ext) = match refreshed {
            Credential::Oauth {
                access,
                expires,
                ext,
                ..
            } => (access, expires, ext),
            Credential::ApiKey { .. } => {
                return Err(AuthError::oauth(
                    provider,
                    CopilotAuthError::new("Invalid Copilot token response"),
                ));
            }
        };

        let ids = self
            .fetch_available_model_ids(&access, enterprise.as_deref())
            .await
            .map_err(|e| AuthError::oauth(provider, e))?;
        ext.insert(
            EXT_AVAILABLE_MODEL_IDS.to_string(),
            Value::Array(ids.into_iter().map(Value::String).collect()),
        );

        Ok(Credential::Oauth {
            refresh: refresh_token,
            access,
            expires,
            ext,
        })
    }

    /// Pi `githubCopilotOAuth.toAuth` (`auth/oauth/github-copilot.ts:370-377`): the Copilot token is
    /// the bearer key and the endpoint is derived from it per credential. No headers are added here —
    /// the editor-identity set rides on each catalog row's `model.headers`.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        let access = match cred {
            Credential::Oauth { access, .. } => access.clone(),
            Credential::ApiKey { .. } => {
                return Err(AuthError::oauth(
                    ProviderId::from(GITHUB_COPILOT_PROVIDER_ID),
                    CopilotAuthError::new("Invalid Copilot token response"),
                ));
            }
        };
        let enterprise = copilot_enterprise_domain(cred);
        Ok(ModelAuth {
            api_key: Some(access.clone()),
            headers: None,
            base_url: Some(github_copilot_base_url(
                Some(&access),
                enterprise.as_deref(),
            )),
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::provider::Provider;
    use crate::stream::{StreamOptions, collect_message};
    use cyrup_core::StopReason;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// An [`AuthContext`] over a fixed map — also the way the OAuth tests keep proxy resolution
    /// deterministic (an ambient `HTTPS_PROXY` must not reroute a loopback request).
    struct MapEnv(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    fn empty_env() -> Arc<dyn AuthContext> {
        Arc::new(MapEnv(BTreeMap::new()))
    }

    // ------------------------------------------------------------------ catalog

    /// The catalog is Pi's `GITHUB_COPILOT_MODELS` verbatim at `b0c2a90e` — 28 models across the
    /// three wire APIs Pi declares at `github-copilot.ts:27-31`.
    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = github_copilot_models();
        assert_eq!(models.len(), 28);
        assert!(
            models
                .iter()
                .all(|m| m.provider.as_str() == GITHUB_COPILOT_PROVIDER_ID)
        );
        assert!(models.iter().all(|m| m.base_url == GITHUB_COPILOT_BASE_URL));

        let count = |api: &str| models.iter().filter(|m| m.api.as_str() == api).count();
        assert_eq!(count("anthropic-messages"), 9);
        assert_eq!(count("openai-completions"), 7);
        assert_eq!(count("openai-responses"), 12);
        // Nothing outside the three APIs Pi's `createProvider` maps.
        assert_eq!(
            count("anthropic-messages") + count("openai-completions") + count("openai-responses"),
            models.len()
        );
    }

    /// Every catalog row carries the four editor-identity headers (Pi's generator bakes
    /// `COPILOT_HEADERS` into each model). Copilot's edge rejects requests without them, and
    /// `model.headers` is the ONLY path by which they reach request traffic — `to_auth` adds none.
    #[test]
    fn every_catalog_row_carries_the_copilot_editor_headers() {
        let expected: crate::HeaderMap = COPILOT_HEADERS
            .iter()
            .map(|(k, v)| ((*k).to_string(), Some((*v).to_string())))
            .collect();
        for m in github_copilot_models() {
            assert_eq!(
                m.headers.as_ref(),
                Some(&expected),
                "{} lost its copilot headers",
                m.id.as_str()
            );
        }
    }

    /// Pi `f7b78e2a` ("route GitHub Copilot MAI-Code models through /responses endpoint", 2026-07-13)
    /// moved MAI-Code off chat-completions. MIRROR: `kimi-k2.7-code`, untouched by that commit, is
    /// still `openai-completions` — so this asserts the fix, not "whatever the file says".
    #[test]
    fn mai_code_routes_through_openai_responses() {
        let models = github_copilot_models();
        let api_of = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing from catalog"))
                .api
                .as_str()
                .to_string()
        };
        assert_eq!(api_of("mai-code-1-flash-picker"), "openai-responses");
        assert_eq!(api_of("kimi-k2.7-code"), "openai-completions");
    }

    /// Spot-check a row against Pi's literal source: `claude-fable-5` (`github-copilot.models.ts`
    /// @`b0c2a90e`) — 1M window, the three `openai-completions` compat negatives, and the
    /// `off: null` thinking level that makes "off" unsupported.
    #[test]
    fn claude_fable_5_matches_the_upstream_row() {
        let models = github_copilot_models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "claude-fable-5")
            .expect("claude-fable-5");
        assert_eq!(m.name, "Claude Fable 5");
        assert_eq!(m.api.as_str(), "openai-completions");
        assert_eq!(m.context_window, 1_000_000);
        assert_eq!(m.max_tokens, 128_000);
        assert_eq!(m.cost.input, 10.0);
        assert_eq!(m.cost.output, 50.0);
        assert_eq!(m.cost.cache_read, 1.0);
        assert_eq!(m.cost.cache_write, 12.5);
        let compat = m.compat.as_ref().expect("compat");
        assert_eq!(compat.supports_store, Some(false));
        assert_eq!(compat.supports_developer_role, Some(false));
        assert_eq!(compat.supports_reasoning_effort, Some(false));
        let map = m.thinking_level_map.as_ref().expect("thinkingLevelMap");
        assert_eq!(map.get("off"), Some(&None));
        assert_eq!(map.get("xhigh"), Some(&Some("xhigh".to_string())));
        assert_eq!(map.get("max"), Some(&Some("max".to_string())));
    }

    /// `claude-haiku-4.5` is the anthropic-messages side of the same file: it carries the
    /// `supportsEagerToolInputStreaming: false` override, which flips the request onto the legacy
    /// fine-grained-tool-streaming beta.
    #[test]
    fn claude_haiku_carries_the_anthropic_compat_override() {
        let models = github_copilot_models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "claude-haiku-4.5")
            .expect("claude-haiku-4.5");
        assert_eq!(m.api.as_str(), "anthropic-messages");
        assert_eq!(
            m.compat
                .as_ref()
                .and_then(|c| c.supports_eager_tool_input_streaming),
            Some(false)
        );
    }

    // ------------------------------------------------------------------ base-url derivation

    /// Pi `getBaseUrlFromToken` (`:64-73`): `proxy.<host>` → `api.<host>`.
    #[test]
    fn base_url_from_token_rewrites_the_proxy_prefix() {
        assert_eq!(
            base_url_from_token(
                "tid=abc;exp=123;proxy-ep=proxy.individual.githubcopilot.com;st=dotcom"
            ),
            Some("https://api.individual.githubcopilot.com".to_string())
        );
        // A host without the `proxy.` prefix is used verbatim (`:71-72` only strips `^proxy\.`).
        assert_eq!(
            base_url_from_token("proxy-ep=copilot-proxy.example.com"),
            Some("https://copilot-proxy.example.com".to_string())
        );
        // MIRROR: no claim at all → no derived URL.
        assert_eq!(base_url_from_token("tid=abc;exp=123"), None);
        // `[^;]+` needs at least one char, and the regex keeps scanning past a failed position.
        assert_eq!(
            base_url_from_token("proxy-ep=;proxy-ep=proxy.two.example;"),
            Some("https://api.two.example".to_string())
        );
    }

    /// Pi `getGitHubCopilotBaseUrl` (`:75-85`): token claim, then enterprise host, then the
    /// individual-plan default.
    #[test]
    fn base_url_precedence_matches_upstream() {
        assert_eq!(
            github_copilot_base_url(
                Some("proxy-ep=proxy.acme.githubcopilot.com;"),
                Some("acme.com")
            ),
            "https://api.acme.githubcopilot.com",
            "the token claim outranks the enterprise domain"
        );
        assert_eq!(
            github_copilot_base_url(Some("tid=no-claim"), Some("company.ghe.com")),
            "https://copilot-api.company.ghe.com"
        );
        assert_eq!(github_copilot_base_url(None, None), GITHUB_COPILOT_BASE_URL);
        // JS truthiness: an empty token/domain is "absent" (`:77`, `:82`).
        assert_eq!(
            github_copilot_base_url(Some(""), Some("")),
            GITHUB_COPILOT_BASE_URL
        );
    }

    /// Pi `normalizeDomain` (`:39-49`): trim → prepend `https://` when schemeless → `URL.hostname`.
    #[test]
    fn normalize_domain_extracts_the_host() {
        assert_eq!(
            normalize_domain("  company.ghe.com "),
            Some("company.ghe.com".to_string())
        );
        assert_eq!(
            normalize_domain("https://company.ghe.com/enterprises/x?q=1"),
            Some("company.ghe.com".to_string())
        );
        assert_eq!(
            normalize_domain("https://COMPANY.GHE.com"),
            Some("company.ghe.com".to_string())
        );
        assert_eq!(
            normalize_domain("https://user:pw@company.ghe.com:8443/"),
            Some("company.ghe.com".to_string())
        );
        assert_eq!(
            normalize_domain("https://[::1]:8443/x"),
            Some("[::1]".to_string())
        );
        // Empty / unparseable → null (`:41`, `:47`).
        assert_eq!(normalize_domain("   "), None);
        assert_eq!(normalize_domain("://nope"), None);
        assert_eq!(normalize_domain("https:///path-only"), None);
    }

    // ------------------------------------------------------------------ /models parsing

    fn models_body(entries: Value) -> Value {
        json!({ "data": entries })
    }

    /// Pi `isSelectableCopilotModel` (`:91-96`) — each exclusion, plus the MIRROR row that survives
    /// every one of them.
    #[test]
    fn only_selectable_models_are_listed() {
        let raw = models_body(json!([
            // MIRROR: minimal selectable row (no policy, no capabilities).
            { "id": "keep-minimal", "model_picker_enabled": true },
            // MIRROR: explicit positives everywhere.
            {
                "id": "keep-full",
                "model_picker_enabled": true,
                "policy": { "state": "enabled" },
                "capabilities": { "supports": { "tool_calls": true } }
            },
            // `model_picker_enabled === true` is strict: absent, false, or truthy-non-true all drop.
            { "id": "drop-no-picker" },
            { "id": "drop-picker-false", "model_picker_enabled": false },
            { "id": "drop-picker-truthy", "model_picker_enabled": "yes" },
            // `policy?.state !== "disabled"`.
            {
                "id": "drop-policy-disabled",
                "model_picker_enabled": true,
                "policy": { "state": "disabled" }
            },
            // `supports?.tool_calls !== false` — only an explicit `false` drops.
            {
                "id": "drop-no-tool-calls",
                "model_picker_enabled": true,
                "capabilities": { "supports": { "tool_calls": false } }
            },
            // A non-string id is skipped (`typeof id === "string"`, `:107`).
            { "id": 7, "model_picker_enabled": true },
            // A non-object row is skipped (`asRecord` → undefined, `:105`).
            "not-an-object"
        ]));
        assert_eq!(
            parse_available_copilot_model_ids(&raw).expect("parses"),
            vec!["keep-minimal".to_string(), "keep-full".to_string()]
        );
    }

    /// Pi `:101-103`: a body whose `data` is not an array throws this exact string.
    #[test]
    fn models_response_without_a_data_array_is_the_upstream_error() {
        for body in [json!({}), json!({ "data": {} }), json!([]), json!("nope")] {
            let err = parse_available_copilot_model_ids(&body).expect_err("must reject");
            assert_eq!(err.to_string(), "Invalid Copilot models response");
        }
    }

    // ------------------------------------------------------------------ filterModels

    fn oauth_cred(ext: Map<String, Value>) -> Credential {
        Credential::Oauth {
            refresh: "gho_refresh".to_string(),
            access: "tid=a;proxy-ep=proxy.individual.githubcopilot.com;".to_string(),
            expires: 0,
            ext,
        }
    }

    fn ext_with(key: &str, value: Value) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(key.to_string(), value);
        m
    }

    /// Pi `filterModels` (`github-copilot.ts:20-26`): an OAuth credential listing ids narrows the
    /// catalog to exactly those.
    #[test]
    fn filter_models_narrows_to_the_credentials_available_ids() {
        let models = github_copilot_models();
        let cred = oauth_cred(ext_with(
            EXT_AVAILABLE_MODEL_IDS,
            json!(["gpt-5.4", "claude-sonnet-4.5", "not-in-catalog"]),
        ));
        let filtered = filter_github_copilot_models(&models, Some(&cred));
        let ids: Vec<&str> = filtered.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-sonnet-4.5", "gpt-5.4"]);
    }

    /// MIRROR cases: every fall-through in `:21-25` returns the FULL catalog.
    #[test]
    fn filter_models_falls_through_to_the_full_catalog() {
        let models = github_copilot_models();
        let full = models.len();
        // `credential?.type !== "oauth"` — no credential at all.
        assert_eq!(filter_github_copilot_models(&models, None).len(), full);
        // ...and an api-key credential.
        let api_key = Credential::api_key("ghu_x");
        assert_eq!(
            filter_github_copilot_models(&models, Some(&api_key)).len(),
            full
        );
        // OAuth without the key.
        assert_eq!(
            filter_github_copilot_models(&models, Some(&oauth_cred(Map::new()))).len(),
            full
        );
        // `!Array.isArray(...)`.
        let not_array = oauth_cred(ext_with(EXT_AVAILABLE_MODEL_IDS, json!("gpt-5.4")));
        assert_eq!(
            filter_github_copilot_models(&models, Some(&not_array)).len(),
            full
        );
        // `.every(id => typeof id === "string")` — ONE non-string voids the whole filter rather than
        // dropping just that entry.
        let mixed = oauth_cred(ext_with(EXT_AVAILABLE_MODEL_IDS, json!(["gpt-5.4", 7])));
        assert_eq!(
            filter_github_copilot_models(&models, Some(&mixed)).len(),
            full
        );
        // An empty list is a valid all-strings array, so it filters to nothing.
        let empty = oauth_cred(ext_with(EXT_AVAILABLE_MODEL_IDS, json!([])));
        assert!(filter_github_copilot_models(&models, Some(&empty)).is_empty());
    }

    // ------------------------------------------------------------------ toAuth

    /// Pi `toAuth` (`:370-377`): the Copilot token is the bearer key AND the source of the endpoint.
    #[tokio::test]
    async fn to_auth_derives_the_endpoint_from_the_token() {
        let oauth = GitHubCopilotOAuth::new();
        let cred = oauth_cred(Map::new());
        let auth = oauth.to_auth(&cred).await.expect("to_auth");
        assert_eq!(
            auth.api_key.as_deref(),
            Some("tid=a;proxy-ep=proxy.individual.githubcopilot.com;")
        );
        assert_eq!(
            auth.base_url.as_deref(),
            Some("https://api.individual.githubcopilot.com")
        );
        // No headers here — the editor identity rides on `model.headers`.
        assert!(auth.headers.is_none());
    }

    /// MIRROR: an enterprise credential whose token has no `proxy-ep=` falls back to
    /// `copilot-api.<domain>` (`:82`), and `enterpriseUrl` is re-normalized on the way (`:359-363`).
    #[tokio::test]
    async fn to_auth_falls_back_to_the_enterprise_host() {
        let mut ext = Map::new();
        ext.insert(
            EXT_ENTERPRISE_URL.to_string(),
            json!("https://COMPANY.ghe.com/setup"),
        );
        let cred = Credential::Oauth {
            refresh: "gho_refresh".to_string(),
            access: "tid=a;exp=1".to_string(),
            expires: 0,
            ext,
        };
        let auth = GitHubCopilotOAuth::new()
            .to_auth(&cred)
            .await
            .expect("to_auth");
        assert_eq!(
            auth.base_url.as_deref(),
            Some("https://copilot-api.company.ghe.com")
        );
    }

    // ------------------------------------------------------------------ refresh (loopback only)

    /// A loopback HTTP server that answers `GET /copilot_internal/v2/token` and `GET /models` from
    /// canned bodies and records every request head it saw. Nothing here can reach a real host.
    async fn spawn_github(
        token_response: &'static str,
        models_response: &'static str,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let sink = sink.clone();
                tokio::spawn(async move {
                    // Read the request head (GET: no body follows the blank line).
                    let mut head = Vec::new();
                    let mut buf = [0u8; 512];
                    loop {
                        match sock.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                head.extend_from_slice(buf.get(..n).unwrap_or(&[]));
                                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => return,
                        }
                    }
                    let head = String::from_utf8_lossy(&head).to_string();
                    let body = if head.contains("/copilot_internal/v2/token") {
                        token_response
                    } else {
                        models_response
                    };
                    if let Ok(mut g) = sink.lock() {
                        g.push(head);
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn stored_github_token() -> Credential {
        Credential::Oauth {
            refresh: "gho_stored_github_token".to_string(),
            access: "stale-copilot-token".to_string(),
            expires: 0,
            ext: Map::new(),
        }
    }

    /// The whole of Pi `refreshGitHubCopilotToken` (`:271-277`): exchange the GitHub token at
    /// `copilot_internal/v2/token`, apply the five-minute expiry skew, then list the account's
    /// selectable models onto the credential. Also pins the two request shapes — bearer + the four
    /// `COPILOT_HEADERS`, and `X-GitHub-Api-Version` on the listing only.
    #[tokio::test]
    async fn refresh_exchanges_the_github_token_and_lists_models() {
        let (origin, seen) = spawn_github(
            r#"{"token":"tid=t1;exp=99;proxy-ep=proxy.individual.githubcopilot.com;","expires_at":1800000000}"#,
            r#"{"data":[{"id":"gpt-5.4","model_picker_enabled":true},{"id":"hidden"}]}"#,
        )
        .await;
        let oauth = GitHubCopilotOAuth::new()
            .with_auth_context(empty_env())
            .with_origin_override(&origin, &origin);

        let refreshed = oauth
            .refresh(&stored_github_token())
            .await
            .expect("refresh");
        let Credential::Oauth {
            refresh,
            access,
            expires,
            ext,
        } = refreshed
        else {
            panic!("refresh must yield an oauth credential");
        };

        // The GitHub token is carried forward unchanged; the Copilot token replaces `access`.
        assert_eq!(refresh, "gho_stored_github_token");
        assert_eq!(
            access,
            "tid=t1;exp=99;proxy-ep=proxy.individual.githubcopilot.com;"
        );
        // `expiresAt * 1000 - 5 * 60 * 1000` (`:260`).
        assert_eq!(expires, 1_800_000_000_i64 * 1000 - 5 * 60 * 1000);
        // Only the selectable model is listed.
        assert_eq!(ext.get(EXT_AVAILABLE_MODEL_IDS), Some(&json!(["gpt-5.4"])));
        // No enterprise domain was configured, so the key is absent (JS drops `undefined`, `:262`).
        assert!(!ext.contains_key(EXT_ENTERPRISE_URL));

        let heads = seen.lock().unwrap().clone();
        assert_eq!(
            heads.len(),
            2,
            "expected the token call then the models call"
        );
        let token_head = heads
            .iter()
            .find(|h| h.contains("/copilot_internal/v2/token"))
            .expect("token request");
        let models_head = heads
            .iter()
            .find(|h| h.contains("GET /models"))
            .expect("models request");
        assert!(token_head.contains("authorization: Bearer gho_stored_github_token"));
        // The Copilot token — not the GitHub one — authenticates the listing (`:120`).
        assert!(models_head.contains("authorization: Bearer tid=t1;"));
        for (name, value) in COPILOT_HEADERS {
            let expected = format!("{}: {value}", name.to_ascii_lowercase());
            assert!(token_head.contains(&expected), "token call missing {name}");
            assert!(
                models_head.contains(&expected),
                "models call missing {name}"
            );
        }
        // `X-GitHub-Api-Version` is on the listing only (`:122`); the token call never sends it.
        assert!(models_head.contains(&format!("x-github-api-version: {COPILOT_API_VERSION}")));
        assert!(
            !token_head
                .to_ascii_lowercase()
                .contains("x-github-api-version")
        );
    }

    /// Pi `:254`: a token response missing `token`/`expires_at` (or with the wrong types) fails with
    /// this exact message, and the stored credential is left for re-login (`AuthError::OAuth`).
    #[tokio::test]
    async fn refresh_rejects_a_malformed_token_response() {
        for body in [
            r#"{"expires_at":1800000000}"#,
            r#"{"token":"t","expires_at":"1800000000"}"#,
            r#"{"token":42,"expires_at":1800000000}"#,
        ] {
            let (origin, _) = spawn_github(body, r#"{"data":[]}"#).await;
            let oauth = GitHubCopilotOAuth::new()
                .with_auth_context(empty_env())
                .with_origin_override(&origin, &origin);
            let err = oauth
                .refresh(&stored_github_token())
                .await
                .expect_err("must reject");
            assert_eq!(err.code(), "oauth");
            let cause = std::error::Error::source(&err).expect("cause");
            assert_eq!(cause.to_string(), "Invalid Copilot token response fields");
        }
    }

    /// A non-object token body is Pi's other guard (`:246`).
    #[tokio::test]
    async fn refresh_rejects_a_non_object_token_response() {
        let (origin, _) = spawn_github("[]", r#"{"data":[]}"#).await;
        let oauth = GitHubCopilotOAuth::new()
            .with_auth_context(empty_env())
            .with_origin_override(&origin, &origin);
        let err = oauth
            .refresh(&stored_github_token())
            .await
            .expect_err("must reject");
        let cause = std::error::Error::source(&err).expect("cause");
        assert_eq!(cause.to_string(), "Invalid Copilot token response");
    }

    /// The listing is NOT best-effort: Pi awaits it inside the returned literal (`:274`), so a bad
    /// `/models` body fails the whole refresh rather than yielding a credential with no id list.
    #[tokio::test]
    async fn refresh_fails_when_the_models_listing_is_invalid() {
        let (origin, _) = spawn_github(
            r#"{"token":"tid=t1;","expires_at":1800000000}"#,
            r#"{"models":[]}"#,
        )
        .await;
        let oauth = GitHubCopilotOAuth::new()
            .with_auth_context(empty_env())
            .with_origin_override(&origin, &origin);
        let err = oauth
            .refresh(&stored_github_token())
            .await
            .expect_err("must reject");
        let cause = std::error::Error::source(&err).expect("cause");
        assert_eq!(cause.to_string(), "Invalid Copilot models response");
    }

    // ------------------------------------------------------------------ provider wiring

    /// Identity + the api-key half of `auth` (Pi `github-copilot.ts:11-17`). The env var is the one
    /// `env_api_keys.rs:36-37` already publishes for this provider id.
    #[test]
    fn provider_identity_and_env_mapping() {
        let p = github_copilot_provider();
        assert_eq!(p.id().as_str(), GITHUB_COPILOT_PROVIDER_ID);
        assert!(p.get_model("claude-sonnet-4.5").is_some());
        assert!(p.get_model("gpt-5.4").is_some());
        let vars =
            crate::env_api_keys::api_key_env_vars(GITHUB_COPILOT_PROVIDER_ID).expect("env mapping");
        assert!(vars.contains(&COPILOT_GITHUB_TOKEN_ENV));
        // Pi gives Copilot BOTH strategies (`github-copilot.ts:16-19`).
        let auth = p.provider_auth().expect("provider auth");
        assert!(auth.api_key.is_some());
        assert_eq!(
            auth.oauth.as_ref().map(|o| o.name()),
            Some("GitHub Copilot")
        );
    }

    /// With no stored credential and no `COPILOT_GITHUB_TOKEN`, a request is "not configured" —
    /// delivered as a terminal error, never thrown (R-01-009/018).
    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        let provider = github_copilot_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(empty_env());
        let model = provider.get_model("gpt-5.4").unwrap().clone();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    /// MIRROR of the above: `COPILOT_GITHUB_TOKEN` alone configures the provider, so the request gets
    /// past auth and dies at the transport instead.
    #[tokio::test]
    async fn env_token_resolves_auth_then_fails_at_transport() {
        let env = MapEnv(BTreeMap::from([(
            COPILOT_GITHUB_TOKEN_ENV.to_string(),
            "ghu_copilot_test".to_string(),
        )]));
        let provider = github_copilot_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider.get_model("gpt-5.4").unwrap().clone();
        model.base_url = "http://127.0.0.1:1/v1".to_string();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(
            !err.contains("not configured"),
            "auth should have resolved, got: {err}"
        );
        assert!(
            err.contains("transport"),
            "expected transport error, got: {err}"
        );
    }

    /// A loopback listener that records the first bytes of every connection it accepts and answers
    /// HTTP 500. Used to observe WHICH host a request went to.
    async fn spawn_recorder() -> (std::net::SocketAddr, Arc<Mutex<Vec<Vec<u8>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let sink = sink.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if let Ok(mut g) = sink.lock() {
                        g.push(buf.get(..n).unwrap_or(&[]).to_vec());
                    }
                    let _ = sock
                        .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    let _ = sock.flush().await;
                });
            }
        });
        (addr, seen)
    }

    /// End-to-end through [`crate::wire::WireProvider`]: a stored Copilot credential's `proxy-ep=`
    /// claim REDIRECTS the request away from `model.base_url` (Pi `toAuth`, `:370-377`, threaded by
    /// `api/anthropic_messages.rs:443-450`).
    ///
    /// Two loopback recorders make this a positive discrimination rather than an inverted assertion:
    /// `catalog` stands in for `model.base_url` and `edge` for the credential's endpoint. Only one of
    /// them may receive a TCP connection. (The derived endpoint is always `https://…`, so `edge` sees
    /// a TLS ClientHello rather than a request line — being *contacted at all* is the signal.)
    #[tokio::test]
    async fn stored_oauth_credential_redirects_the_request_to_its_proxy_endpoint() {
        let (catalog, catalog_hits) = spawn_recorder().await;
        let (edge, edge_hits) = spawn_recorder().await;

        let store = InMemoryCredentialStore::new().with_credential(
            ProviderId::from(GITHUB_COPILOT_PROVIDER_ID),
            Credential::Oauth {
                refresh: "gho_stored".to_string(),
                // No `proxy.` prefix to strip, so the endpoint is `https://<edge>` verbatim.
                access: format!("tid=a;proxy-ep={edge};"),
                // Far future: `resolve_stored_oauth` must NOT refresh (that would need the network),
                // so this exercises `to_auth` alone.
                expires: i64::MAX,
                ext: Map::new(),
            },
        );

        let provider = github_copilot_provider_with(Arc::new(store), Arc::new(builtin_registry()))
            .with_auth_context(empty_env());
        let mut model = provider.get_model("claude-sonnet-4.5").unwrap().clone();
        model.base_url = format!("http://{catalog}");
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;

        assert_eq!(
            edge_hits.lock().unwrap().len(),
            1,
            "the credential's endpoint was not used"
        );
        assert!(
            catalog_hits.lock().unwrap().is_empty(),
            "model.base_url was used despite the credential's endpoint override"
        );
        assert_eq!(msg.stop_reason, StopReason::Error); // TLS against a plain recorder
        assert!(
            !msg.error_message
                .unwrap_or_default()
                .contains("not configured")
        );
    }

    /// MIRROR of the above: an env API key resolves through `envApiKeyAuth`, which supplies NO
    /// base-url override, so the same request goes to `model.base_url` — and carries the catalog's
    /// editor-identity headers plus the bearer token that make Copilot's edge accept it.
    #[tokio::test]
    async fn env_key_leaves_the_request_on_the_catalog_base_url() {
        let (catalog, catalog_hits) = spawn_recorder().await;
        let (edge, edge_hits) = spawn_recorder().await;

        let env = MapEnv(BTreeMap::from([(
            COPILOT_GITHUB_TOKEN_ENV.to_string(),
            "ghu_copilot_test".to_string(),
        )]));
        let provider = github_copilot_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));
        let mut model = provider.get_model("claude-sonnet-4.5").unwrap().clone();
        model.base_url = format!("http://{catalog}");
        let _ = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;

        assert!(
            edge_hits.lock().unwrap().is_empty(),
            "nothing should reach the unused edge"
        );
        let hits = catalog_hits.lock().unwrap().clone();
        assert_eq!(hits.len(), 1, "model.base_url was not used");
        let head = String::from_utf8_lossy(hits.first().map(Vec::as_slice).unwrap_or(&[]))
            .to_ascii_lowercase();
        assert!(head.contains("ghu_copilot_test"), "head: {head}");
        // The catalog's editor headers ride along on every request (`model.headers`).
        for (name, value) in COPILOT_HEADERS {
            assert!(
                head.contains(&format!(
                    "{}: {}",
                    name.to_ascii_lowercase(),
                    value.to_ascii_lowercase()
                )),
                "request is missing {name}; head: {head}"
            );
        }
        // `edge` exists only to prove the negative half of the discrimination.
        let _ = edge;
    }

    /// PROV-032/PROV-031. The filter now has a PRODUCTION caller: it is installed on the provider
    /// through pi's `createProvider({ filterModels })` transport and applied by
    /// `Models::get_available` at pi's exact position (`models.ts:407` @v0.83.0).
    ///
    /// Red before the fix: `get_available` did not exist and `filter_github_copilot_models`' only
    /// callers were the tests above, so a Business/Enterprise account whose token authorises 3 of
    /// the 28 rows was offered all 28 in `/model`.
    #[tokio::test]
    async fn get_available_applies_the_copilot_filter_but_get_models_does_not() {
        use crate::auth::InMemoryCredentialStore;
        use crate::collection::{CreateModelsOptions, create_models};

        let catalog = github_copilot_models();
        let full = catalog.len();
        let three: Vec<Value> = catalog
            .iter()
            .take(3)
            .map(|m| json!(m.id.as_str()))
            .collect();

        let store = Arc::new(InMemoryCredentialStore::new().with_credential(
            GITHUB_COPILOT_PROVIDER_ID.into(),
            oauth_cred(ext_with(EXT_AVAILABLE_MODEL_IDS, Value::Array(three))),
        ));
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(store.clone()),
            ..Default::default()
        });
        models.set_provider(Arc::new(github_copilot_provider_with(
            store,
            Arc::new(builtin_registry()),
        )));

        assert_eq!(
            models.get_models(Some(GITHUB_COPILOT_PROVIDER_ID)).len(),
            full,
            "getModels() stays the complete synchronous catalog (models.ts:105-110)"
        );
        assert_eq!(
            models
                .get_available(Some(GITHUB_COPILOT_PROVIDER_ID))
                .await
                .len(),
            3,
            "getAvailable() applies filterModels after the auth check"
        );
    }

    /// PROV-031. An unconfigured provider contributes nothing to `get_available`, while
    /// `get_models` still returns its whole catalog.
    #[tokio::test]
    async fn get_available_skips_a_provider_with_no_credential() {
        use crate::auth::InMemoryCredentialStore;
        use crate::collection::{CreateModelsOptions, create_models};

        let store = Arc::new(InMemoryCredentialStore::new());
        let mut models = create_models(CreateModelsOptions {
            credentials: Some(store.clone()),
            // An auth context with NO variables, so `COPILOT_GITHUB_TOKEN` in the developer's real
            // environment cannot make this provider look configured.
            auth_context: Some(Arc::new(EmptyEnv)),
            ..Default::default()
        });
        models.set_provider(Arc::new(github_copilot_provider_with(
            store,
            Arc::new(builtin_registry()),
        )));
        assert!(
            models
                .check_auth(GITHUB_COPILOT_PROVIDER_ID)
                .await
                .is_none()
        );
        assert!(models.get_available(None).await.is_empty());
        assert!(!models.get_models(None).is_empty());
    }

    struct EmptyEnv;
    #[async_trait::async_trait]
    impl crate::auth::AuthContext for EmptyEnv {
        async fn env(&self, _name: &str) -> Option<String> {
            None
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    /// PROV-017. `Provider::name()` reaches the display name `WireProvider` already held.
    #[test]
    fn provider_exposes_its_display_name() {
        use crate::provider::Provider;
        let p = github_copilot_provider();
        assert_eq!(p.name(), "GitHub Copilot");
        assert_ne!(p.name(), p.id().as_str());
    }
}
