//! PROV-042 — `ModelsStreamTransforms.transformHeaders` must reach the wire on the path the agent
//! actually streams through, for **every** api impl.
//!
//! # What was wrong
//!
//! The seam existed as a field ([`crate::StreamOptions::transform_headers`], `stream.rs:216`) and
//! was applied in exactly one place: [`crate::collection::Models::apply_auth`] — pi's literal
//! position, `git -C tmp/pi show v0.84.4:packages/ai/src/models.ts` `:657`
//! (`if (options?.transformHeaders) headers = await options.transformHeaders(headers ?? {})`),
//! stripped at `:660`. That is correct *for pi*, where every request goes through
//! `Models.stream`/`streamSimple` (`:667-679`, `:688-694`).
//!
//! cyrup's agent loop does not. It streams `StreamFn` → [`crate::Provider::stream`] →
//! [`crate::wire::WireProvider`] (`wire.rs:149`) → `ApiImpl::run`, and
//! `rg '\.stream_simple\(|Models::stream' crates/` finds no production caller of the collection at
//! all. So a `transform_headers` closure installed by a caller of the agent was silently inert:
//! `before_provider_headers` could be subscribed to and would never fire, and an extension that
//! adds a corporate proxy header or strips an identifying one could not exist.
//!
//! # What these tests pin
//!
//! One case per registered api impl, driven through the real `ApiImpl::run` against a loopback
//! origin that records the request head — the established no-network technique in this crate
//! (`tests/anthropic_sensitive_stop.rs:19-23`). Each case proves all three clauses of the item's
//! Verify at once:
//!
//!  1. the closure **receives the fully-assembled set** — it asserts the impl's own auth header is
//!     present in what it is handed;
//!  2. its **return value wins** — the header it adds is on the wire;
//!  3. a **deletion takes effect** — the auth header it removed is not on the wire, which is the
//!     `x-api-key` suppression clause the item names.
//!
//! `every_registered_api_impl_has_a_case` fails if a new api is registered without one, so the
//! "every api impl, not just one" bar stays a property rather than a snapshot.
//!
//! Red before the fix: every case failed clause 2 (the marker never reached the wire), because no
//! `ApiImpl::run` consulted `opts.transform_headers`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crate::api::ApiRegistry;
use crate::{
    AuthResult, Context, HeaderMap, Modality, Model, ModelCost, ProviderEnv, StreamOptions, channel,
};
use cyrup_core::{ApiId, CancelToken, Content, Message};

/// The header the transform ADDS. Must not collide with anything an impl or reqwest sets.
const MARKER: &str = "x-cyrup-prov-042";

// ------------------------------------------------------------------------------ loopback origin --

/// `true` once `acc` holds a complete HTTP/1.1 request (head + declared `Content-Length` body).
/// Copied from `tests/anthropic_sensitive_stop.rs:46` — every impl here sends `Content-Length`.
fn request_is_complete(acc: &[u8]) -> bool {
    let Some(head_end) = acc.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head_end = head_end + 4;
    let head = String::from_utf8_lossy(&acc[..head_end]).to_lowercase();
    let len = head.lines().find_map(|line| {
        line.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    match len {
        Some(n) => acc.len() >= head_end + n,
        None => true,
    }
}

/// Record every request head (lower-cased) and answer each with an empty `text/event-stream`.
///
/// The body is deliberately empty: these tests assert on what went OUT, and every impl treats a
/// zero-frame stream as a (possibly degenerate) terminal rather than hanging.
fn spawn_capture_origin() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}");
    let heads: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = heads.clone();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(20)));
            let mut buf = [0u8; 8192];
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if request_is_complete(&acc) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            if let Some(end) = acc.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&acc[..end]).to_lowercase();
                if let Ok(mut g) = sink.lock() {
                    g.push(head);
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    (url, heads)
}

// ------------------------------------------------------------------------------------- fixtures --

fn model(api: &str, provider: &str, id: &str, base_url: String) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: api.into(),
        provider: provider.into(),
        base_url,
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost {
            input: 1.0,
            output: 1.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: None,
        },
        context_window: 100_000,
        max_tokens: 4_096,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// Pin proxy resolution off so a developer's ambient `HTTP_PROXY` cannot send the loopback request
/// off-box (the provider env overlay wins over the process env,
/// `utils/node_http_proxy.rs:38-58`), plus any per-api env the impl needs.
fn env_with(extra: &[(&str, &str)]) -> ProviderEnv {
    let mut env = ProviderEnv::new();
    env.insert("no_proxy".to_string(), "*".to_string());
    for (k, v) in extra {
        env.insert((*k).to_string(), (*v).to_string());
    }
    env
}

/// An unsigned JWT whose namespaced claim carries a ChatGPT account id — what
/// `openai_codex_responses::headers::extract_account_id` (`headers.rs:15-32`) parses before any
/// request is made. Mirrors the shape `openai_codex_responses/tests/mod.rs:83` builds.
fn codex_token() -> String {
    use base64::Engine as _;
    let payload = serde_json::json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct_prov042" },
        "sub": "user_1",
    });
    // `ATOB` (`openai_codex_responses/mod.rs:114`) is the WHATWG forgiving-base64 decoder: the
    // STANDARD alphabet with optional padding, `-`/`_` REJECTED — so this must not be URL-safe.
    let body =
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&payload).unwrap());
    format!("eyJhbGciOiJub25lIn0.{body}.sig")
}

/// One case: the api to drive, a header its own `build_headers` installs (used both as the "did
/// the closure see the assembled set" probe and as the deletion target), and its auth/model
/// fixture. For every SSE impl that is the auth header, which is the `x-api-key` suppression clause
/// the item names; Bedrock is the documented exception (see its case).
struct Case {
    api: &'static str,
    probe_header: &'static str,
    model_id: &'static str,
    provider: &'static str,
    auth: AuthResult,
}

fn cases() -> Vec<Case> {
    let keyed = |extra: &[(&str, &str)]| {
        let mut a = AuthResult::from_key("test-key-not-a-real-credential", "test");
        a.env = Some(env_with(extra));
        a
    };
    vec![
        Case {
            api: crate::known_api::ANTHROPIC_MESSAGES,
            probe_header: "x-api-key",
            model_id: "claude-opus-4-5",
            provider: "anthropic",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::OPENAI_COMPLETIONS,
            probe_header: "authorization",
            model_id: "gpt-4o",
            provider: "openai",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::OPENAI_RESPONSES,
            probe_header: "authorization",
            model_id: "gpt-5",
            provider: "openai",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::AZURE_OPENAI_RESPONSES,
            probe_header: "api-key",
            model_id: "gpt-5",
            provider: "azure",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::GOOGLE_GENERATIVE_AI,
            probe_header: "x-goog-api-key",
            model_id: "gemini-3-pro",
            provider: "google",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::GOOGLE_VERTEX,
            probe_header: "x-goog-api-key",
            model_id: "gemini-3-pro",
            provider: "google-vertex",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::MISTRAL_CONVERSATIONS,
            probe_header: "authorization",
            model_id: "mistral-large",
            provider: "mistral",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::PI_MESSAGES,
            probe_header: "authorization",
            model_id: "pi-1",
            provider: "pi",
            auth: keyed(&[]),
        },
        Case {
            api: crate::known_api::OPENAI_CODEX_RESPONSES,
            probe_header: "authorization",
            model_id: "gpt-5-codex",
            provider: "openai-codex",
            auth: {
                let mut a = AuthResult::from_key(codex_token(), "oauth");
                a.env = Some(env_with(&[]));
                a
            },
        },
        Case {
            api: crate::known_api::BEDROCK_CONVERSE_STREAM,
            // Bedrock is the one impl whose transform position is BEFORE its auth header exists,
            // and deliberately so: SigV4 signs the header set (`authorize`,
            // `bedrock_converse_stream/headers.rs:50-73`), so a hook that ran after signing could
            // only add unsigned headers or invalidate the signature. It therefore sits at pi's own
            // pre-signing injection point for caller headers (`bedrock-converse-stream.ts:224-227`
            // @v0.84.4), and `content-type` is the assembled header it can see and suppress there.
            probe_header: "content-type",
            model_id: "anthropic.claude-sonnet-4-5-20250929-v1:0",
            provider: "amazon-bedrock",
            auth: AuthResult {
                auth: Default::default(),
                env: Some(env_with(&[
                    ("AWS_BEDROCK_SKIP_AUTH", "1"),
                    ("AWS_REGION", "us-east-1"),
                ])),
                source: None,
            },
        },
    ]
}

fn user_ctx() -> Context {
    Context {
        system_prompt: Some("be brief".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text("hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    }
}

// ------------------------------------------------------------------------------------- the test --

/// Drive one api impl end-to-end with a transform installed; return `(seen_by_closure, wire_head)`.
async fn run_case(case: &Case) -> (HeaderMap, String) {
    let (base_url, heads) = spawn_capture_origin();
    let registry = crate::api::builtin_registry();
    let api = registry
        .get(&ApiId::from(case.api))
        .unwrap_or_else(|| panic!("api '{}' is registered", case.api));

    let seen: Arc<Mutex<Option<HeaderMap>>> = Arc::new(Mutex::new(None));
    let recorder = seen.clone();
    let target = case.probe_header.to_string();
    let opts = StreamOptions {
        transform_headers: Some(Arc::new(move |headers: HeaderMap| {
            let recorder = recorder.clone();
            let target = target.clone();
            Box::pin(async move {
                if let Ok(mut g) = recorder.lock() {
                    *g = Some(headers.clone());
                }
                let mut out = headers;
                // Clause 3: a deletion must take effect on the wire.
                out.retain(|k, _| !k.eq_ignore_ascii_case(&target));
                // Clause 2: the return value wins.
                out.insert(MARKER.to_string(), Some("1".to_string()));
                out
            })
        })),
        ..Default::default()
    };

    let m = model(case.api, case.provider, case.model_id, base_url);
    let ctx = user_ctx();
    let auth = case.auth.clone();
    let (sink, mut rx) = channel(64);
    let task = tokio::spawn(async move {
        api.run(&m, &ctx, &auth, &opts, CancelToken::new(), sink)
            .await;
    });
    while rx.recv().await.is_some() {}
    task.await.expect("api task");

    let head = heads
        .lock()
        .ok()
        .and_then(|g| g.first().cloned())
        .unwrap_or_default();
    let observed = seen.lock().ok().and_then(|g| g.clone()).unwrap_or_default();
    (observed, head)
}

#[tokio::test]
async fn transform_headers_reaches_the_wire_for_every_api_impl() {
    for case in cases() {
        let (seen, head) = run_case(&case).await;

        assert!(
            !head.is_empty(),
            "[{}] no request reached the loopback origin — the impl failed before dispatch, so \
             this case proves nothing",
            case.api
        );
        // Clause 1 — the closure is handed the FULLY-ASSEMBLED set, including the auth header the
        // impl itself installs. pi's own transform runs before the api impl adds this (it sits in
        // `Models.applyAuth`, models.ts:657), so cyrup's position is strictly more capable — which
        // is what makes the item's `x-api-key` suppression clause reachable at all.
        assert!(
            seen.keys()
                .any(|k| k.eq_ignore_ascii_case(case.probe_header)),
            "[{}] the transform was handed {:?}, which does not contain the assembled auth header \
             '{}' — it ran too early",
            case.api,
            seen.keys().collect::<Vec<_>>(),
            case.probe_header
        );
        // Clause 2 — the return value wins.
        assert!(
            head.contains(MARKER),
            "[{}] the header the transform added never reached the wire. Request head was:\n{}",
            case.api,
            head
        );
        // Clause 3 — a deletion takes effect.
        assert!(
            !head.contains(&format!("\n{}:", case.probe_header))
                && !head.starts_with(&format!("{}:", case.probe_header)),
            "[{}] the transform removed '{}' and it was sent anyway. Request head was:\n{}",
            case.api,
            case.probe_header,
            head
        );
    }
}

/// The "every api impl, not just one" bar, as a property: a newly registered api with no case here
/// fails this test rather than silently escaping the seam.
#[test]
fn every_registered_api_impl_has_a_case() {
    let mut reg = ApiRegistry::new();
    crate::api::register_builtins(&mut reg);
    let covered: Vec<&str> = cases().iter().map(|c| c.api).collect();
    for id in reg.ids() {
        assert!(
            covered.contains(&id.as_str()),
            "api '{id}' is registered but has no PROV-042 wire case; add one to `cases()` (and \
             make its `run` call `crate::stream::apply_transform_headers`)"
        );
    }
}

/// `apply_transform_headers` runs the hook exactly once and is an identity when none is
/// installed.
///
/// Scope note (batch-4 review): an earlier version of this test was named
/// `the_models_level_seam_still_strips_the_field_it_applied` and its doc claimed to pin "that the
/// two applications never stack". It did not — it only calls this helper and never constructs a
/// `Models` or exercises `apply_auth`. That claim IS pinned, by
/// `collection.rs::transform_headers_runs_last_and_is_stripped_from_provider_options`, which
/// asserts `apply_auth` applies the hook last and then clears the field
/// (`collection.rs:733-737`, pi `models.ts:657`/`:660` @v0.84.4) so a `Models`-routed request
/// cannot transform twice. This test asserts only the call-count and identity properties of the
/// per-impl helper.
#[tokio::test]
async fn the_helper_runs_the_hook_once_and_is_an_identity_without_one() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = calls.clone();
    let opts = StreamOptions {
        transform_headers: Some(Arc::new(move |h: HeaderMap| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move { h })
        })),
        ..Default::default()
    };
    let out = crate::stream::apply_transform_headers(&opts, HeaderMap::new()).await;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(out.is_empty());

    let none = StreamOptions::default();
    assert!(
        crate::stream::apply_transform_headers(&none, HeaderMap::new())
            .await
            .is_empty(),
        "no hook installed must be an identity pass-through"
    );
}
