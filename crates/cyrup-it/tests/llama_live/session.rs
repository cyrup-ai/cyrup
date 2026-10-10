//! The user's path against a real router: a REAL `AgentSession` (built by
//! `cyrup::session_launch::build_factory`, which attaches the llama.cpp built-in exactly as the
//! binary does) with the credential `/login llama.cpp` stores, refreshed the way `/llama` refreshes
//! it, then a prompted turn on the router's text model.

use futures::StreamExt as _;

use cyrup_core::CancelToken;
use cyrup_llama::client::{LlamaClient, LlamaProgress};
use cyrup_provider::{AnyModel, ProviderEnv};

use super::fixture::{
    Opts, PROVIDER, assistant_error, fixture, listed_llama, refresh_llama, session,
    store_credential,
};
use super::router::{API_KEY, CHAT_MODEL, DECISION_MODEL, Router, setup};

/// `future`, or a panic naming `step` after `secs`: a real server that stops answering must fail
/// the test with a name, not hang it.
async fn step<T>(step: &str, secs: u64, future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(secs), future)
        .await
        .unwrap_or_else(|_| panic!("{step} did not finish within {secs} s"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_session_refreshes_from_and_streams_through_a_real_router() {
    let Some(setup) = setup("a_real_session_refreshes_from_and_streams_through_a_real_router")
    else {
        return;
    };
    let router = Router::start(&setup).await;
    let url = router.url().to_string();
    let cancel = CancelToken::new();
    let env = ProviderEnv::from([("no_proxy".to_string(), "*".to_string())]);
    let client = LlamaClient::new(&url, Some(API_KEY.to_string()), Some(&env))
        .await
        .unwrap();
    // Both loaded: an unloaded `models_dir` entry is not selectable, and EXT-110 is only a claim
    // about a decision model the router would otherwise offer.
    let quiet = |_: LlamaProgress| {};
    for model in [CHAT_MODEL, DECISION_MODEL] {
        client.load_and_wait(model, &quiet, &cancel).await.unwrap();
    }

    let fx = fixture();
    store_credential(&fx, &url, Some(API_KEY));
    let session = session(&fx, &Opts::default()).await;
    assert_eq!(
        step("the llama.cpp refresh", 60, refresh_llama(&session)).await,
        Vec::<String>::new()
    );
    assert_eq!(
        listed_llama(&session),
        [CHAT_MODEL],
        "the session offers the text model and NOT the decision model (EXT-110)"
    );
    let provider = session
        .services()
        .guest_providers
        .provider(PROVIDER)
        .unwrap();
    let mut classifiers: Vec<(String, String)> = provider
        .get_all_models()
        .iter()
        .filter_map(AnyModel::as_classifier)
        .map(|model| {
            (
                model.id.as_str().to_string(),
                model.api.as_str().to_string(),
            )
        })
        .collect();
    classifiers.sort();
    assert_eq!(
        classifiers,
        [
            (CHAT_MODEL.to_string(), "llama-cpp-classify".to_string()),
            (
                DECISION_MODEL.to_string(),
                "typesafe-system-one".to_string()
            ),
        ]
    );

    let picked = session
        .set_model(&format!("{PROVIDER}/{CHAT_MODEL}"))
        .await
        .unwrap();
    assert_eq!(picked.model.as_str(), CHAT_MODEL);
    let mut events = step("the prompt", 60, session.prompt("Once upon a time"))
        .await
        .expect("prompt accepted");
    // DRAIN the run's events: the stream is a bounded channel (1024, `cyrup-session-svc`'s
    // `subscriber.rs`), and a real model's unbounded reply is one event per token, so a held but
    // unread stream stalls the run once it fills. The fake-server seam tests never get there (two
    // deltas); stories260K answers ~2000 tokens, and the first live run hung exactly here.
    let mut seen = 0_usize;
    step("the turn", 120, async {
        while events.next().await.is_some() {
            seen += 1;
        }
    })
    .await;
    assert!(seen > 0, "the run emitted no event");
    step("settling", 30, session.wait_for_idle()).await;
    assert_eq!(assistant_error(&session).await, None);
    let text = session.last_assistant_text().await.unwrap_or_default();
    assert!(
        !text.is_empty(),
        "the turn produced no text from the real router"
    );
}
