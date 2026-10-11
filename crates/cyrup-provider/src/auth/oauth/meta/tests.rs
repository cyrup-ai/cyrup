//! PROV-080 — the Meta Model API OAuth flow against a loopback stub of auth.meta.com and the
//! Muse Code key-mint endpoint. Each of pi's four `test/meta-oauth.test.ts` cases (f1b2e77f5) is
//! ported and named for the case it ports; the rest pin the branches pi's tests leave unasserted
//! (every poll outcome, `errorDetail`'s precedence, the 401/403 message, the untrusted-URI
//! fallback, cancellation, the wire shape of all three requests).
//!
//! pi drives the login with fake timers and `interval: 5`; Rust has no ambient clock to fake under
//! a real loopback socket (tokio's paused clock would also fire reqwest's 30 s timeout), so the
//! login test uses `interval: 1` in real time and checks `expires` as a window.
//!
//! All of these were red at the base: `auth::oauth::meta` did not exist.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use crate::auth::oauth::interaction::ScriptedInteraction;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const AUTHORIZE_PATH: &str = "/oidc/device/authorization/";
const TOKEN_PATH: &str = "/oidc/device/token/";
const MINT_PATH: &str = "/muse-code/key";

// ---------------------------------------------------------------------------- loopback harness

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    /// Lower-cased header names.
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

type Router = Arc<dyn Fn(&str, usize) -> (u16, String) + Send + Sync>;

/// A loopback HTTP server; `router(path, nth_hit_for_that_path)` picks the reply, so a poll
/// sequence is scriptable. Nothing here reaches a real host.
async fn spawn(router: Router) -> (String, Arc<Mutex<Vec<Recorded>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    tokio::spawn(async move {
        let hits: Arc<Mutex<BTreeMap<String, usize>>> = Arc::new(Mutex::new(BTreeMap::new()));
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let sink = Arc::clone(&sink);
            let router = Arc::clone(&router);
            let hits = Arc::clone(&hits);
            tokio::spawn(async move {
                let mut raw: Vec<u8> = Vec::new();
                let mut buf = [0u8; 1024];
                let head_end = loop {
                    match sock.read(&mut buf).await {
                        Ok(0) => break raw.len(),
                        Ok(n) => {
                            raw.extend_from_slice(&buf[..n]);
                            if let Some(idx) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                                break idx + 4;
                            }
                        }
                        Err(_) => return,
                    }
                };
                let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                let mut lines = head.lines();
                let path = lines
                    .next()
                    .and_then(|line| line.split(' ').nth(1))
                    .unwrap_or("/")
                    .to_string();
                let headers: Vec<(String, String)> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                    .collect();
                let content_length = headers
                    .iter()
                    .find(|(k, _)| k == "content-length")
                    .and_then(|(_, v)| v.parse::<usize>().ok())
                    .unwrap_or(0);
                while raw.len() < head_end + content_length {
                    match sock.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => raw.extend_from_slice(&buf[..n]),
                        Err(_) => return,
                    }
                }
                let body = String::from_utf8_lossy(&raw[head_end..]).to_string();
                let nth = {
                    let mut hits = hits.lock().unwrap();
                    let counter = hits.entry(path.clone()).or_insert(0);
                    let nth = *counter;
                    *counter += 1;
                    nth
                };
                sink.lock().unwrap().push(Recorded {
                    path: path.clone(),
                    headers,
                    body,
                });
                let (status, reply) = router(&path, nth);
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    (format!("http://{addr}"), log)
}

struct NoProxyEnv;
#[async_trait::async_trait]
impl AuthContext for NoProxyEnv {
    async fn env(&self, name: &str) -> Option<String> {
        // Pin proxy resolution off, so an ambient `HTTP_PROXY` cannot send loopback traffic away.
        name.eq_ignore_ascii_case("no_proxy")
            .then(|| "*".to_string())
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

fn flow_at(origin: &str) -> MetaOAuth {
    MetaOAuth::with_endpoints(
        format!("{origin}{AUTHORIZE_PATH}"),
        format!("{origin}{TOKEN_PATH}"),
        format!("{origin}{MINT_PATH}"),
        Arc::new(NoProxyEnv),
    )
}

fn device_body() -> String {
    json!({
        "device_code": "device-code-123",
        "user_code": "ABCD-1234",
        "verification_uri": "https://auth.meta.com/oauth/device/",
        "verification_uri_complete": "https://auth.meta.com/oauth/device/?code=ABCD-1234",
        "interval": 1,
        "expires_in": 600,
    })
    .to_string()
}

fn oauth_parts(credential: &Credential) -> (String, String, i64) {
    match credential {
        Credential::Oauth {
            refresh,
            access,
            expires,
            ..
        } => (refresh.clone(), access.clone(), *expires),
        other => panic!("expected an oauth credential, got {other:?}"),
    }
}

// ----------------------------------------------------------------------- pi's four cases

/// pi `meta-oauth.test.ts:39-103`, "logs in with the device flow and mints a Model API key": the
/// device-code event, one pending poll then the identity token, the progress note, and the minted
/// key stored as `access` with the identity token as `refresh` and a one-day lifetime. The three
/// requests' wire shape is asserted too (pi checks the same fields through its stubbed `fetch`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_in_with_the_device_flow_and_mints_a_model_api_key() {
    let (origin, log) = spawn(Arc::new(|path: &str, nth: usize| match (path, nth) {
        (AUTHORIZE_PATH, _) => (200, device_body()),
        (TOKEN_PATH, 0) => (400, json!({ "error": "authorization_pending" }).to_string()),
        (TOKEN_PATH, _) => (
            200,
            json!({ "access_token": "identity-token", "token_type": "Bearer" }).to_string(),
        ),
        (MINT_PATH, _) => (200, json!({ "api_key": "LLM|minted-key" }).to_string()),
        _ => (404, "{}".to_string()),
    }))
    .await;
    let interaction = ScriptedInteraction::new(Vec::new());

    let before = crate::auth::oauth::now_ms();
    let credential = flow_at(&origin)
        .login(&interaction, &LoginOptions::default())
        .await
        .unwrap();
    let after = crate::auth::oauth::now_ms();

    let (refresh, access, expires) = oauth_parts(&credential);
    assert_eq!(refresh, "identity-token");
    assert_eq!(access, "LLM|minted-key");
    assert!(
        (before + DAY_MS..=after + DAY_MS).contains(&expires),
        "expires {expires} not within one day of the login"
    );

    let events = interaction.events();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(
        events[0],
        AuthEvent::DeviceCode {
            user_code: "ABCD-1234".to_string(),
            verification_uri: "https://auth.meta.com/oauth/device/?code=ABCD-1234".to_string(),
            interval_seconds: Some(1.0),
            expires_in_seconds: Some(600.0),
        }
    );
    assert_eq!(
        events[1],
        AuthEvent::Progress {
            message: "Enabling Meta Model API access...".to_string(),
        }
    );

    let log = log.lock().unwrap().clone();
    let paths: Vec<&str> = log.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, [AUTHORIZE_PATH, TOKEN_PATH, TOKEN_PATH, MINT_PATH]);
    // `meta.ts:75-80`: a form POST of `client_id` alone.
    assert_eq!(log[0].body, "client_id=1031625952748946");
    assert_eq!(
        log[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(log[0].header("accept"), Some("application/json"));
    // `meta.ts:114-118`: grant type, device code, client id — in that order.
    assert_eq!(
        log[1].body,
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code\
         &device_code=device-code-123&client_id=1031625952748946"
    );
    // `meta.ts:148-156`: a JSON POST of `{}` with the identity token and the api version.
    let mint = &log[3];
    assert_eq!(mint.body, "{}");
    assert_eq!(mint.header("authorization"), Some("Bearer identity-token"));
    assert_eq!(mint.header("content-type"), Some("application/json"));
    assert_eq!(mint.header("accept"), Some("application/json"));
    assert_eq!(mint.header("x-api-version"), Some("1.0.0"));
}

/// pi `meta-oauth.test.ts:105-129`, "re-mints the API key from the stored identity token on
/// refresh": the refresh is a mint, not a call to auth.meta.com.
#[tokio::test]
async fn re_mints_the_api_key_from_the_stored_identity_token_on_refresh() {
    let (origin, log) = spawn(Arc::new(|path: &str, _| match path {
        MINT_PATH => (200, json!({ "api_key": "LLM|fresh-key" }).to_string()),
        _ => (404, "{}".to_string()),
    }))
    .await;
    let stored = crate::auth::oauth::oauth_credential("LLM|old-key", "identity-token", 1);

    let before = crate::auth::oauth::now_ms();
    let refreshed = flow_at(&origin).refresh(&stored).await.unwrap();
    let after = crate::auth::oauth::now_ms();

    let (refresh, access, expires) = oauth_parts(&refreshed);
    assert_eq!(refresh, "identity-token");
    assert_eq!(access, "LLM|fresh-key");
    assert!((before + DAY_MS..=after + DAY_MS).contains(&expires));
    let log = log.lock().unwrap().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].path, MINT_PATH);
    assert_eq!(
        log[0].header("authorization"),
        Some("Bearer identity-token")
    );
}

/// pi `meta-oauth.test.ts:131-142`, "reports the setup URL when Meta issues no key".
#[tokio::test]
async fn reports_the_setup_url_when_meta_issues_no_key() {
    let (origin, _) = spawn(Arc::new(|_: &str, _| {
        (
            200,
            json!({ "require_payment": true, "action_url": "https://dev.meta.ai/billing" })
                .to_string(),
        )
    }))
    .await;
    let stored = crate::auth::oauth::oauth_credential("", "identity-token", 1);
    let error = flow_at(&origin).refresh(&stored).await.unwrap_err();
    // The trait folds the flow's error into the crate taxonomy (`oauth refresh failed for meta`);
    // pi's rejection message is its cause.
    assert_eq!(error.code(), "oauth");
    let cause = std::error::Error::source(&error)
        .map(ToString::to_string)
        .unwrap_or_default();
    assert_eq!(
        cause,
        "Meta did not issue an API key. Complete setup at https://dev.meta.ai/billing"
    );
}

/// pi `meta-oauth.test.ts:144-148`, "uses the minted key as the request api key".
#[tokio::test]
async fn uses_the_minted_key_as_the_request_api_key() {
    let auth = MetaOAuth::new()
        .to_auth(&crate::auth::oauth::oauth_credential(
            "LLM|key",
            "identity-token",
            1,
        ))
        .await
        .unwrap();
    assert_eq!(auth.api_key.as_deref(), Some("LLM|key"));
    assert!(auth.headers.is_none());
    assert!(auth.base_url.is_none());
}

// ----------------------------------------------------------- branches pi's tests leave open

/// The strategy's identity (`meta.ts:196-200`) and the upstream constants (`:19-26`).
#[test]
fn constants_and_identity_are_upstream_verbatim() {
    assert_eq!(CLIENT_ID, "1031625952748946");
    assert_eq!(
        DEVICE_AUTHORIZATION_URL,
        "https://auth.meta.com/oidc/device/authorization/"
    );
    assert_eq!(DEVICE_TOKEN_URL, "https://auth.meta.com/oidc/device/token/");
    assert_eq!(API_KEY_MINT_URL, "https://api.meta.ai/muse-code/key");
    assert_eq!(API_KEY_LIFETIME_MS, 86_400_000);
    assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    let flow = MetaOAuth::new();
    assert_eq!(flow.name(), "Meta (Muse subscription)");
    assert!(flow.is_subscription());
    assert_eq!(flow.login_label(), Some("Sign in with Meta"));
}

/// `errorDetail` (`meta.ts:49-55`): first non-blank string of the four keys, in that order, trimmed.
#[test]
fn error_detail_takes_the_first_non_blank_string_in_upstream_order() {
    let detail = |value: Value| error_detail(Some(&value));
    assert_eq!(
        detail(json!({ "error": "e", "message": "m", "detail": "d", "error_description": "ed" })),
        ": ed"
    );
    assert_eq!(
        detail(json!({ "error": "e", "message": "m", "detail": "d" })),
        ": d"
    );
    assert_eq!(detail(json!({ "error": "e", "message": "  m  " })), ": m");
    // Blank and non-string values are skipped, not reported.
    assert_eq!(
        detail(json!({ "error_description": "   ", "detail": 5, "error": "e" })),
        ": e"
    );
    assert_eq!(detail(json!({ "status": "bad" })), "");
    assert_eq!(detail(json!(["error"])), "");
    assert_eq!(error_detail(None), "");
}

/// Every outcome of one device-token reply (`meta.ts:120-140`).
#[test]
fn every_token_reply_maps_as_upstream_does() {
    let reply =
        |ok: bool, status: u16, value: Value| classify_token_reply(ok, status, Some(&value));
    assert_eq!(
        reply(true, 200, json!({ "access_token": "id" })),
        DeviceCodePollResult::Complete("id".to_string())
    );
    assert_eq!(
        reply(false, 400, json!({ "error": "authorization_pending" })),
        DeviceCodePollResult::Pending
    );
    assert_eq!(
        reply(false, 400, json!({ "error": "slow_down", "interval": 7 })),
        DeviceCodePollResult::SlowDown {
            interval_seconds: Some(7.0)
        }
    );
    // `positiveNumber`: a zero, negative or non-numeric interval is no hint at all.
    assert_eq!(
        reply(false, 400, json!({ "error": "slow_down", "interval": 0 })),
        DeviceCodePollResult::SlowDown {
            interval_seconds: None
        }
    );
    assert_eq!(
        reply(false, 400, json!({ "error": "access_denied" })),
        DeviceCodePollResult::Failed {
            message: "Meta login was denied.".to_string()
        }
    );
    assert_eq!(
        reply(false, 400, json!({ "error": "expired_token" })),
        DeviceCodePollResult::Failed {
            message: "Meta device authorization expired. Please restart login.".to_string()
        }
    );
    assert_eq!(
        reply(
            false,
            500,
            json!({ "error": "server_error", "error_description": "down" })
        ),
        DeviceCodePollResult::Failed {
            message: "Meta device token request failed with status 500: down".to_string()
        }
    );
    // A success that carries no token is not a success: the `switch` default.
    assert_eq!(
        reply(true, 200, json!({ "access_token": "" })),
        DeviceCodePollResult::Failed {
            message: "Meta device token request failed with status 200".to_string()
        }
    );
    // An error code on a 200 still decides the outcome.
    assert_eq!(
        reply(true, 200, json!({ "error": "authorization_pending" })),
        DeviceCodePollResult::Pending
    );
    assert_eq!(
        classify_token_reply(false, 502, None),
        DeviceCodePollResult::Failed {
            message: "Meta device token request failed with status 502".to_string()
        }
    );
}

/// `startDeviceAuthorization` (`meta.ts:87-99`): an untrusted complete URI falls back to the plain
/// one; two untrusted URIs, or a missing code, is an invalid response that quotes the body; the
/// polling hints pass through `positiveNumber`, so a missing `expires_in` is NO deadline.
#[test]
fn device_authorization_parse_follows_upstream() {
    let parse = |value: Value| parse_device_authorization(Some(&value));
    let fallback = parse(json!({
        "device_code": "d",
        "user_code": "u",
        "verification_uri": "https://auth.meta.com/device",
        "verification_uri_complete": "javascript:alert(1)",
        "interval": 0,
    }))
    .unwrap();
    assert_eq!(fallback.verification_uri, "https://auth.meta.com/device");
    assert_eq!(fallback.interval_seconds, None);
    assert_eq!(fallback.expires_in_seconds, None);
    // http is trusted too (`url.protocol !== "https:" && url.protocol !== "http:"`).
    assert_eq!(
        parse(json!({ "device_code": "d", "user_code": "u", "verification_uri": "http://x.test" }))
            .unwrap()
            .verification_uri,
        "http://x.test/"
    );

    let error =
        parse(json!({ "device_code": "d", "user_code": "u", "verification_uri": "file:///x" }))
            .unwrap_err();
    assert_eq!(
        error.to_string(),
        r#"Invalid Meta device authorization response: {"device_code":"d","user_code":"u","verification_uri":"file:///x"}"#
    );
    let error =
        parse(json!({ "device_code": "", "user_code": "u", "verification_uri": "https://x" }))
            .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("Invalid Meta device authorization response: ")
    );
    assert_eq!(
        parse_device_authorization(None).unwrap_err().to_string(),
        "Invalid Meta device authorization response: null"
    );
}

/// `meta.ts:91`: the invalid-response message is `JSON.stringify(json)` of what `response.json()`
/// parsed, so it prints as JS prints: integer-like keys first and ascending, an integer past 2^53
/// as the nearest double, and a float with no fraction without one. The body goes through
/// [`read_json`] as the real reply does.
#[test]
fn the_invalid_response_message_prints_the_body_as_json_stringify_does() {
    let body = r#"{"b":1,"1":2,"n":9007199254740993,"f":1.0,"device_code":""}"#;
    assert_eq!(
        parse_device_authorization(read_json(body).as_ref())
            .unwrap_err()
            .to_string(),
        r#"Invalid Meta device authorization response: {"1":2,"b":1,"n":9007199254740992,"f":1,"device_code":""}"#
    );
}

/// `meta.ts:84-86`: a failed authorization names the status and the server's detail.
#[tokio::test]
async fn a_failed_device_authorization_names_the_status_and_detail() {
    let (origin, _) = spawn(Arc::new(|_: &str, _| {
        (
            429,
            json!({ "error": "rate_limited", "message": "too many" }).to_string(),
        )
    }))
    .await;
    let error = flow_at(&origin)
        .start_device_authorization(None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Meta device authorization failed with status 429: too many"
    );
}

/// `meta.ts:158-169`: 401/403 is a dead session (the detail lands after the full stop, as pi
/// writes it); any other failure names its status.
#[tokio::test]
async fn mint_failures_follow_upstream() {
    for (status, expected) in [
        (
            401,
            "Meta session expired (status 401). Run `/login meta` to sign in again.: token revoked",
        ),
        (
            403,
            "Meta session expired (status 403). Run `/login meta` to sign in again.: token revoked",
        ),
        (
            500,
            "Meta API key mint failed with status 500: token revoked",
        ),
    ] {
        let (origin, _) = spawn(Arc::new(move |_: &str, _| {
            (status, json!({ "detail": "token revoked" }).to_string())
        }))
        .await;
        let error = flow_at(&origin)
            .mint_api_key("identity-token", None)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
    // No key and no trusted setup URL: the sentence alone.
    let (origin, _) = spawn(Arc::new(|_: &str, _| {
        (
            200,
            json!({ "action_url": "javascript:void(0)" }).to_string(),
        )
    }))
    .await;
    let error = flow_at(&origin)
        .mint_api_key("identity-token", None)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Meta did not issue an API key.");
}

/// A cancel during the poll wait ends the login as `Login cancelled`: the token reaches
/// `poll_oauth_device_code_flow`'s `abortable_sleep`, and the mint is never called. The stub answers
/// `authorization_pending` forever; the cancel lands during the first wait.
///
/// NON-REGRESSION GUARD, not a red-proof of `run_login`'s port of `meta.ts:189-192` (`catch (e) {
/// if (interaction.signal.aborted) throw new Error("Login cancelled"); throw e; }`). With that
/// mapping removed this test still passes, because the poller and `send`'s biased `select!` already
/// answer `Cancelled` (mutation `080-cancel-map`, green). The mapping matters only for a cancel that
/// lands after a reply was read and before its error returns, a race no test here can stage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_login_reports_login_cancelled() {
    let (origin, log) = spawn(Arc::new(|path: &str, _| match path {
        AUTHORIZE_PATH => (200, device_body()),
        _ => (400, json!({ "error": "authorization_pending" }).to_string()),
    }))
    .await;
    let cancel = CancelToken::new();
    let interaction = ScriptedInteraction::new(Vec::new()).with_cancel(cancel.clone());
    let flow = flow_at(&origin);
    let options = LoginOptions::default();
    let login = flow.login(&interaction, &options);
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(login, canceller);
    assert_eq!(result.unwrap_err().to_string(), "Login cancelled");
    // The device code was shown; the flow never reached the mint.
    assert!(matches!(
        interaction.events().first(),
        Some(AuthEvent::DeviceCode { .. })
    ));
    assert!(log.lock().unwrap().iter().all(|r| r.path != MINT_PATH));
}

/// An API-key credential cannot be refreshed or turned into request auth by this strategy.
#[tokio::test]
async fn an_api_key_credential_is_refused() {
    let key = Credential::api_key("sk");
    let flow = MetaOAuth::new();
    assert!(flow.refresh(&key).await.is_err());
    assert!(flow.to_auth(&key).await.is_err());
}
