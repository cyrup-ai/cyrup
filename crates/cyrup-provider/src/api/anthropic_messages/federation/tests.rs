//! PROV-119 tests: the resolution order, the activation gate, and the exchange on the wire.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
//!
//! The wire tests run against a loopback listener rather than a mocked client, because the things
//! most likely to be wrong are the body field names, the URL and the header — none of which a
//! mock of our own construction would catch.

use super::*;
use crate::auth::AuthResult;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A unique temp directory, removed on drop. `tempfile` is not a dependency of this crate and two
/// tests do not justify adding one.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("cyrup-prov119-{tag}-{nanos}"));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(workspace: Option<&str>, token: IdentityToken) -> FederationConfig {
    FederationConfig {
        federation_rule_id: "fdrl_rule".to_string(),
        organization_id: "11111111-2222-3333-4444-555555555555".to_string(),
        service_account_id: "svac_account".to_string(),
        workspace_id: workspace.map(str::to_string),
        identity_token: token,
    }
}

/// A loopback server that answers every `POST /v1/oauth/token` from `replies`, recording the raw
/// request text of each call. Returns the base URL and the recorded requests.
struct FakeExchange {
    base_url: String,
    requests: Arc<tokio::sync::Mutex<Vec<String>>>,
    calls: Arc<AtomicUsize>,
}

impl FakeExchange {
    async fn start(replies: Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::clone(&requests);
        let counter = Arc::clone(&calls);
        tokio::spawn(async move {
            for (status, body) in replies {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let read = socket.read(&mut buf).await.unwrap_or(0);
                recorded
                    .lock()
                    .await
                    .push(String::from_utf8_lossy(&buf[..read]).to_string());
                counter.fetch_add(1, Ordering::SeqCst);
                let reason = if status == 200 { "OK" } else { "Unauthorized" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{}", addr.port()),
            requests,
            calls,
        }
    }

    async fn request(&self, index: usize) -> String {
        self.requests.lock().await[index].clone()
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn ok_body(token: &str, expires_in: u64) -> String {
    format!(
        r#"{{"access_token":"{token}","token_type":"Bearer","expires_in":{expires_in},"scope":"workspace:inference"}}"#
    )
}

/// A unique base URL per test, so the process-wide token cache cannot leak between tests.
fn unique_base(base: &str, tag: &str) -> String {
    format!("{base}/{tag}")
}

// ---------------------------------------------------------------------------------------------
// The exchange, on the wire
// ---------------------------------------------------------------------------------------------

/// The request body carries the RFC 7523 grant and every required field under its documented name,
/// the assertion is the file's contents, and the token comes back usable.
///
/// Red before the feature: there is no exchange to make at all.
#[tokio::test]
async fn the_exchange_sends_the_documented_jwt_bearer_body() {
    let dir = TempDir::new("body");
    let token_path = dir.path().join("token");
    std::fs::write(&token_path, "  header.payload.signature\n").expect("write");

    let server = FakeExchange::start(vec![(200, ok_body("sk-ant-oat01-minted", 3600))]).await;
    let cfg = config(
        Some("wrkspc_one"),
        IdentityToken::File(token_path.to_string_lossy().to_string()),
    );

    let token = access_token(&server.base_url, &cfg)
        .await
        .expect("exchange");
    assert_eq!(token, "sk-ant-oat01-minted");

    let raw = server.request(0).await;
    assert!(
        raw.starts_with("POST /v1/oauth/token "),
        "the exchange must POST /v1/oauth/token, got:\n{raw}"
    );
    let body = raw.split("\r\n\r\n").nth(1).expect("body").to_string();
    let sent: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(
        sent["grant_type"], "urn:ietf:params:oauth:grant-type:jwt-bearer",
        "RFC 7523 jwt-bearer is the only accepted grant"
    );
    // The assertion is the file's contents, trimmed — a trailing newline in a projected token file
    // would otherwise travel into the JWT and fail verification.
    assert_eq!(sent["assertion"], "header.payload.signature");
    assert_eq!(sent["federation_rule_id"], "fdrl_rule");
    assert_eq!(
        sent["organization_id"],
        "11111111-2222-3333-4444-555555555555"
    );
    assert_eq!(sent["service_account_id"], "svac_account");
    assert_eq!(sent["workspace_id"], "wrkspc_one");
}

/// An absent workspace is OMITTED, not sent as `null`: the server then selects the rule's sole
/// enabled workspace, and `null` is not a documented value.
#[tokio::test]
async fn an_absent_workspace_id_is_omitted_from_the_body() {
    let server = FakeExchange::start(vec![(200, ok_body("sk-ant-oat01-a", 3600))]).await;
    let base = unique_base(&server.base_url, "no-workspace");
    let cfg = config(None, IdentityToken::Inline("j.w.t".to_string()));

    access_token(&base, &cfg).await.expect("exchange");

    let body = server
        .request(0)
        .await
        .split("\r\n\r\n")
        .nth(1)
        .unwrap()
        .to_string();
    let sent: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(
        sent.get("workspace_id").is_none(),
        "workspace_id must be absent, not null: {sent}"
    );
}

/// A second request inside the token's lifetime reuses the cached token and performs no second
/// exchange; this is what pi's one-client-per-config cache exists to provide.
#[tokio::test]
async fn a_token_is_reused_until_it_nears_expiry() {
    let server = FakeExchange::start(vec![
        (200, ok_body("sk-ant-oat01-first", 3600)),
        (200, ok_body("sk-ant-oat01-second", 3600)),
    ])
    .await;
    let base = unique_base(&server.base_url, "reuse");
    let cfg = config(None, IdentityToken::Inline("j.w.t".to_string()));

    let first = access_token(&base, &cfg).await.expect("first");
    let second = access_token(&base, &cfg).await.expect("second");

    assert_eq!(first, "sk-ant-oat01-first");
    assert_eq!(second, first, "the cached token must be reused");
    assert_eq!(
        server.call_count(),
        1,
        "exactly one exchange for two requests"
    );
}

/// A token already inside its refresh margin is re-exchanged rather than sent: `expires_in: 1`
/// leaves no usable window at all.
#[tokio::test]
async fn an_expiring_token_is_re_exchanged() {
    let server = FakeExchange::start(vec![
        (200, ok_body("sk-ant-oat01-stale", 1)),
        (200, ok_body("sk-ant-oat01-fresh", 3600)),
    ])
    .await;
    let base = unique_base(&server.base_url, "refresh");
    let cfg = config(None, IdentityToken::Inline("j.w.t".to_string()));

    let first = access_token(&base, &cfg).await.expect("first");
    // `expires_in: 1` with a margin of a tenth of the lifetime leaves under a second of validity.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = access_token(&base, &cfg).await.expect("second");

    assert_eq!(first, "sk-ant-oat01-stale");
    assert_eq!(
        second, "sk-ant-oat01-fresh",
        "an expired token must be re-exchanged"
    );
    assert_eq!(server.call_count(), 2);
}

/// The identity token file is re-read on every exchange, so a rotated projected token is picked up
/// and a `jti`-bearing assertion is never replayed.
#[tokio::test]
async fn the_identity_token_file_is_re_read_on_every_exchange() {
    let dir = TempDir::new("reread");
    let token_path = dir.path().join("token");
    std::fs::write(&token_path, "first.assertion").expect("write");

    let server = FakeExchange::start(vec![
        (200, ok_body("sk-ant-oat01-a", 1)),
        (200, ok_body("sk-ant-oat01-b", 3600)),
    ])
    .await;
    let base = unique_base(&server.base_url, "reread");
    let cfg = config(
        None,
        IdentityToken::File(token_path.to_string_lossy().to_string()),
    );

    access_token(&base, &cfg).await.expect("first");
    std::fs::write(&token_path, "rotated.assertion").expect("rewrite");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    access_token(&base, &cfg).await.expect("second");

    let first = server.request(0).await;
    let second = server.request(1).await;
    assert!(
        first.contains("first.assertion"),
        "first exchange sends the original"
    );
    assert!(
        second.contains("rotated.assertion"),
        "the second exchange must re-read the file, not replay the cached assertion:\n{second}"
    );
}

/// A denied exchange fails the request and the message points at where the real reason is
/// recorded, because the 401 body is always the opaque `Authentication failed`.
#[tokio::test]
async fn a_denied_exchange_names_the_authentication_history() {
    let server = FakeExchange::start(vec![(
        401,
        r#"{"type":"error","error":{"type":"authentication_error","message":"Authentication failed"}}"#
            .to_string(),
    )])
    .await;
    let base = unique_base(&server.base_url, "denied");
    let cfg = config(None, IdentityToken::Inline("j.w.t".to_string()));

    let err = access_token(&base, &cfg).await.expect_err("must fail");
    let text = err.to_string();
    assert!(
        text.contains("401"),
        "the status belongs in the message: {text}"
    );
    assert!(
        text.contains("authentication history"),
        "a 401 body is always opaque, so the message must say where the reason lives: {text}"
    );
}

/// A missing identity-token file is reported with the path, which is the single most common
/// misconfiguration (an unmounted projected-token volume).
#[tokio::test]
async fn a_missing_identity_token_file_names_the_path() {
    let cfg = config(
        None,
        IdentityToken::File("/nonexistent/anthropic/token".to_string()),
    );
    let err = access_token("http://127.0.0.1:1", &cfg)
        .await
        .expect_err("must fail");
    let text = err.to_string();
    assert!(
        text.contains("/nonexistent/anthropic/token"),
        "the path belongs in the message: {text}"
    );
}

/// A 2xx whose body carries no `access_token` is a malformed response, not a usable empty token.
#[test]
fn a_response_without_an_access_token_is_malformed() {
    assert!(matches!(
        parse_token_response(r#"{"token_type":"Bearer","expires_in":3600}"#),
        Err(FederationError::Malformed(_))
    ));
    assert!(matches!(
        parse_token_response(r#"{"access_token":"","expires_in":3600}"#),
        Err(FederationError::Malformed(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// The exchange URL
// ---------------------------------------------------------------------------------------------

/// The exchange path is appended to the API base, and a `base_url` that already names the messages
/// endpoint does not produce `…/v1/messages/v1/oauth/token`.
#[test]
fn the_token_url_is_derived_from_the_api_base() {
    assert_eq!(
        token_url("https://api.anthropic.com"),
        "https://api.anthropic.com/v1/oauth/token"
    );
    assert_eq!(
        token_url("https://api.anthropic.com/"),
        "https://api.anthropic.com/v1/oauth/token"
    );
    assert_eq!(
        token_url("https://api.anthropic.com/v1/messages"),
        "https://api.anthropic.com/v1/oauth/token"
    );
}

// ---------------------------------------------------------------------------------------------
// The activation gate
// ---------------------------------------------------------------------------------------------

fn model_on(provider: &str) -> Model {
    Model {
        id: "claude-opus-5-5".into(),
        name: "Claude Opus 5.5".into(),
        api: super::super::API_ID.into(),
        provider: provider.into(),
        base_url: "https://api.anthropic.com".to_string(),
        reasoning: true,
        input: vec![crate::model::Modality::Text],
        cost: crate::model::ModelCost {
            input: 4.0,
            output: 20.0,
            cache_read: 0.2,
            cache_write: 5.0,
            tiers: None,
        },
        input_limits: None,
        prompt_cache: None,
        context_window: 1_000_000,
        max_tokens: 128_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn env_of(pairs: &[(&str, &str)]) -> crate::auth::types::ProviderEnv {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// An `EnvSource` with an EXPLICIT empty ambient map, so no gate test can be flipped by the
/// federation variables happening to be exported in the shell that runs it.
fn source<'a>(
    overlay: &'a crate::auth::types::ProviderEnv,
    ambient: &'a crate::auth::types::ProviderEnv,
) -> EnvSource<'a> {
    EnvSource {
        overlay: Some(overlay),
        ambient: Some(ambient),
    }
}

fn complete_env() -> Vec<(&'static str, &'static str)> {
    vec![
        (ANTHROPIC_FEDERATION_RULE_ID_ENV, "fdrl_rule"),
        (ANTHROPIC_ORGANIZATION_ID_ENV, "org-uuid"),
        (ANTHROPIC_SERVICE_ACCOUNT_ID_ENV, "svac_account"),
        (ANTHROPIC_IDENTITY_TOKEN_FILE_ENV, "/var/run/token"),
    ]
}

/// The documented four activate federation, and the workspace id is read without gating it.
#[test]
fn the_four_documented_variables_activate_federation() {
    let auth = AuthResult {
        auth: ModelAuth::default(),
        env: None,
        source: None,
    };
    let empty = crate::auth::types::ProviderEnv::new();
    let env = env_of(&complete_env());
    let cfg = FederationConfig::from_env(&model_on("anthropic"), &auth, source(&env, &empty))
        .expect("federation applies");
    assert_eq!(cfg.federation_rule_id, "fdrl_rule");
    assert_eq!(cfg.service_account_id, "svac_account");
    assert_eq!(cfg.workspace_id, None, "workspace is optional");
    assert_eq!(
        cfg.identity_token,
        IdentityToken::File("/var/run/token".to_string())
    );
}

/// CYRUP-DELTA against pi: a missing `ANTHROPIC_SERVICE_ACCOUNT_ID` does NOT activate federation.
///
/// pi activates on three variables and leaves `service_account_id` undefined in the config it
/// hands the SDK; `service_account_id` is a required field of the exchange, so doing the same here
/// would activate and then fail with an opaque 401.
#[test]
fn a_missing_service_account_id_does_not_activate_federation() {
    let auth = AuthResult {
        auth: ModelAuth::default(),
        env: None,
        source: None,
    };
    let mut pairs = complete_env();
    pairs.retain(|(k, _)| *k != ANTHROPIC_SERVICE_ACCOUNT_ID_ENV);
    let empty = crate::auth::types::ProviderEnv::new();
    let env = env_of(&pairs);
    assert!(
        FederationConfig::from_env(&model_on("anthropic"), &auth, source(&env, &empty)).is_none(),
        "three of four variables must not activate federation"
    );
}

/// `ANTHROPIC_IDENTITY_TOKEN` is accepted in place of the file (CYRUP-DELTA: pi reads only the
/// file), and the file wins when both are set.
#[test]
fn either_identity_token_source_satisfies_the_gate() {
    let auth = AuthResult {
        auth: ModelAuth::default(),
        env: None,
        source: None,
    };
    let mut pairs = complete_env();
    pairs.retain(|(k, _)| *k != ANTHROPIC_IDENTITY_TOKEN_FILE_ENV);
    pairs.push((ANTHROPIC_IDENTITY_TOKEN_ENV, "inline.j.w.t"));
    let empty = crate::auth::types::ProviderEnv::new();
    let env = env_of(&pairs);
    let cfg = FederationConfig::from_env(&model_on("anthropic"), &auth, source(&env, &empty))
        .expect("the inline token satisfies the gate");
    assert_eq!(
        cfg.identity_token,
        IdentityToken::Inline("inline.j.w.t".to_string())
    );

    let mut both = complete_env();
    both.push((ANTHROPIC_IDENTITY_TOKEN_ENV, "inline.j.w.t"));
    let env = env_of(&both);
    let cfg = FederationConfig::from_env(&model_on("anthropic"), &auth, source(&env, &empty))
        .expect("both");
    assert_eq!(
        cfg.identity_token,
        IdentityToken::File("/var/run/token".to_string()),
        "a rotating projected file is the fresher source"
    );
}

/// A resolved key or auth header wins: federation is last in the credential chain.
#[test]
fn request_auth_suppresses_federation() {
    let empty = crate::auth::types::ProviderEnv::new();
    let env = env_of(&complete_env());
    let model = model_on("anthropic");

    let with_key = AuthResult {
        auth: ModelAuth {
            api_key: Some("sk-ant-api03-key".to_string()),
            ..Default::default()
        },
        env: None,
        source: None,
    };
    assert!(
        FederationConfig::from_env(&model, &with_key, source(&env, &empty)).is_none(),
        "a key wins over federation"
    );

    // Case-insensitively: a header overlay is a plain map and its casing must not decide whether a
    // workload federates.
    for name in ["Authorization", "authorization"] {
        let mut headers = crate::HeaderMap::new();
        headers.insert(name.to_string(), Some("Bearer t".to_string()));
        let with_header = AuthResult {
            auth: ModelAuth {
                api_key: None,
                headers: Some(headers),
                base_url: None,
            },
            env: None,
            source: None,
        };
        assert!(
            FederationConfig::from_env(&model, &with_header, source(&env, &empty)).is_none(),
            "an `{name}` header wins over federation"
        );
    }
}

/// Only the anthropic provider federates: the exchange is an Anthropic endpoint, so an
/// Anthropic-compatible gateway on another provider id must not be sent these ids.
#[test]
fn only_the_anthropic_provider_federates() {
    let auth = AuthResult {
        auth: ModelAuth::default(),
        env: None,
        source: None,
    };
    let empty = crate::auth::types::ProviderEnv::new();
    let env = env_of(&complete_env());
    for provider in ["opencode", "github-copilot", "openrouter"] {
        assert!(
            FederationConfig::from_env(&model_on(provider), &auth, source(&env, &empty)).is_none(),
            "{provider} must not federate"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The token never becomes an api key
// ---------------------------------------------------------------------------------------------

/// The minted token rides as `Authorization: Bearer` and `api_key` stays EMPTY — the whole reason
/// is that `is_oauth_token` is `api_key.contains("sk-ant-oat")` and a federated token is
/// `sk-ant-oat01-…`, so a token in the key slot would rewrite every tool name on the wire.
#[test]
fn the_minted_token_never_lands_in_the_api_key() {
    let base = AuthResult {
        auth: ModelAuth::default(),
        env: None,
        source: Some("workload identity federation".to_string()),
    };
    let applied = apply_to_auth(&base, "sk-ant-oat01-minted");

    assert_eq!(
        applied.auth.api_key, None,
        "a federated token in `api_key` would flip `is_oauth`"
    );
    assert_eq!(
        applied
            .auth
            .headers
            .as_ref()
            .and_then(|h| h.get("Authorization"))
            .and_then(|v| v.as_deref()),
        Some("Bearer sk-ant-oat01-minted")
    );
    // The guard that makes the above load-bearing: this is the predicate the wire depends on.
    assert!(
        super::super::claude_code::is_oauth_token("sk-ant-oat01-minted"),
        "a federated token DOES match the OAuth heuristic, which is why it must stay out of `api_key`"
    );
}
