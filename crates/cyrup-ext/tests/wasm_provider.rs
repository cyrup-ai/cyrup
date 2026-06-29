//! LIVE guest COMPONENT — provider OAuth + custom `streamSimple` + autocomplete stacking +
//! active-tool restriction (host gap-08 #1/#3 + sdk gap #1/#2/#7), proven end-to-end. Builds the
//! `cyrup-ext-sdk` bundled demo to a `wasm32-wasip2` COMPONENT, loads it with a non-deny
//! [`RecordingServices`] backend, and drives the new `provider-*` / `autocomplete-suggest` exports
//! across the boundary into the running `.wasm`, asserting the guest closures ran.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::{
    CannedResponses, ExtMode, ExtensionHost, HostConfig, OAuthEvent, RecordingServices,
};
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

/// Build (or locate) the demo guest component (shared with the other live tests; cargo caches it).
fn fixture_component() -> PathBuf {
    if let Ok(p) = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(&cargo)
        .args(["build", "-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2"])
        .status()
        .expect("spawn cargo to build the wasm32-wasip2 fixture component");
    assert!(status.success(), "building cyrup-ext-sdk fixture component failed");
    let target_dir = std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")
    });
    let wasm = target_dir.join("wasm32-wasip2/debug/cyrup_ext_sdk.wasm");
    assert!(wasm.exists(), "fixture component not found at {}", wasm.display());
    wasm
}

fn cfg() -> HostConfig {
    HostConfig { mode: ExtMode::Tui, has_ui: true, cwd: PathBuf::from(".") }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_guest_provider_oauth_stream_autocomplete_activetools() {
    let bytes = std::fs::read(fixture_component()).expect("read fixture component bytes");

    // A non-deny backend so the guest `login` flow's `onPrompt` returns a (canned) callback code.
    let responses = CannedResponses { oauth_prompt: Some("code123".into()), ..Default::default() };
    let rec = Arc::new(RecordingServices::new(responses));

    let host = ExtensionHost::with_wasm(cfg()).expect("host with wasm runtime");
    let ext = host
        .load_wasm("demo".into(), &bytes, rec.clone())
        .await
        .expect("load + init the live wasm extension");

    // The static provider config crossed the seam: registered with an OAuth block + streamSimple.
    let reg = host
        .registry()
        .provider_registration("demo-oauth")
        .expect("registry read")
        .expect("demo-oauth registered");
    assert!(reg.has_oauth(), "provider carries an OAuth block");
    assert_eq!(reg.oauth_name().as_deref(), Some("Demo SSO"));
    assert!(reg.has_stream_simple(), "provider supplies a custom streamSimple");
    assert_eq!(reg.config.models.len(), 1);

    // 1) provider-login: runs the guest `login(callbacks)` flow across the boundary. The guest calls
    //    `onAuth` + `onPrompt`; the host backend answers the prompt with the canned code.
    let creds = ext.provider_login("demo-oauth").await.expect("login flow runs");
    assert_eq!(creds["access"], json!("a-code123"), "credentials carry the prompted code");
    assert_eq!(creds["refresh"], json!("r-demo"));

    // The guest drove the host OAuth callbacks — observable host-side.
    let events = ext.guest().oauth_events();
    assert!(
        events.iter().any(|e| matches!(e, OAuthEvent::Auth { .. })),
        "guest called onAuth: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, OAuthEvent::Prompt { .. })),
        "guest called onPrompt: {events:?}"
    );

    // 2) refreshToken: rotates the access token (Pi refreshToken).
    let refreshed = ext.provider_refresh_token("demo-oauth", &creds).await.expect("refresh");
    assert_eq!(refreshed["access"], json!("a-refreshed"));

    // 3) getApiKey: derives the key string from credentials.
    let key = ext.provider_get_api_key("demo-oauth", &creds).await.expect("get api key");
    assert_eq!(key, "a-code123");

    // 4) modifyModels: identity here, but proves the optional callback dispatches.
    let models = json!([{ "id": "demo-model" }]);
    let out = ext.provider_modify_models("demo-oauth", &models, &creds).await.expect("modify models");
    assert_eq!(out, models);

    // 5) streamSimple: the guest pushes two assistant-message events via the provider-stream import.
    ext.provider_stream_simple(
        "demo-oauth",
        "stream-1",
        &json!({ "id": "demo-model" }),
        &json!({ "messages": [] }),
        &json!({}),
    )
    .await
    .expect("streamSimple runs");
    let stream_events = ext.guest().stream_events();
    assert_eq!(stream_events.len(), 2, "guest streamed two events: {stream_events:?}");
    assert_eq!(stream_events[0].0, "stream-1");
    assert_eq!(stream_events[0].1["text"], json!("stream from demo-model"));
    assert_eq!(stream_events[1].1["type"], json!("done"));

    // 6) autocomplete-suggest: the guest's stacked provider augments the host's built-in base.
    let base = json!({ "items": [{ "value": "builtin", "label": "builtin" }], "prefix": "" });
    let query = json!({ "lines": ["dem"], "cursorLine": 0, "cursorCol": 3, "force": false });
    let suggestions = ext.autocomplete_suggest(Some(&base), &query).await.expect("autocomplete");
    let items = suggestions["items"].as_array().expect("items array");
    assert!(
        items.iter().any(|i| i["value"] == json!("builtin")),
        "the wrapped (current) provider's items survive: {items:?}"
    );
    assert!(
        items.iter().any(|i| i["value"] == json!("demo:run")),
        "the stacked guest provider added its item: {items:?}"
    );
    assert_eq!(suggestions["prefix"], json!("dem"), "guest used the cursor line as the prefix");

    // 7) the planmode command restricts the active tools via ext-tools.set-active-tools, then reads
    //    them back via get-active-tools — the plan-mode active-tool restriction, end to end.
    let cancel = cyrup_core::CancelToken::new();
    let out = host.run_command("planmode", "", &cancel).await.expect("planmode runs");
    assert_eq!(out.as_deref(), Some("active tools: read"));
    assert_eq!(ext.guest().active_tools_restriction().as_deref(), Some(&["read".to_string()][..]));
}
