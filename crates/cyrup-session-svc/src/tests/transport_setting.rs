//! The stream-transport settings must reach the provider: `transport` and the
//! `websocketConnectTimeoutMs` that qualifies it. Both are read off the same `StreamOptions` the
//! provider is handed, and both had the same defect shape — parsed and validated in the config
//! layer, threaded through `AgentBuilder`, and never assigned by `SessionBuilder`.
//!
//! `transport` — Pi `sdk.ts:357`
//! (`transport: settingsManager.getTransport()` in the `Agent` options), fed by
//! `settings-manager.ts:750-752` (`this.settings.transport ?? "auto"`).
//!
//! cyrup parsed `transport`, MIGRATED the legacy `websockets` boolean into it
//! (`cyrup-config/src/settings.rs:371-378`) and offered it as a `/settings` choice row, but the
//! Settings→Agent block in `builder.rs` never passed it on: `AgentBuilder::transport` had no
//! non-test caller workspace-wide, so every run streamed with the hardcoded `Transport::Auto` seeded
//! in `AgentBuilder::new` and the persisted value died in the config layer.
//!
//! The observable is `StreamOptions.transport` as the provider actually receives it — the same
//! struct an embedder-supplied `StreamFn` (`ProxyStreamFn`) and every wire API read. `FauxProvider`
//! is a real `Provider`, so this is the production stream path with an offline provider, not a stub
//! of the seam.
//!
//! CAVEAT (recorded deliberately): no cyrup wire API *consumes* `StreamOptions.transport` yet — Pi's
//! only consumer is `ai/src/api/openai-codex-responses.ts:300,1465`, which is one of the four
//! unported wire APIs. So this proves the setting now reaches the provider boundary; making a
//! provider ACT on it is provider-side work outside this crate.

use std::sync::{Arc, Mutex};

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider, FauxResponseStep};
use cyrup_provider::{Provider, Transport};
use super::common::{base_config, fixture};
use crate::{InputSource, SessionBuilder, SessionConfig, Settings, UserInput};

/// Run one prompt through a real session built with `cli` as its settings layer, and return every
/// `StreamOptions.transport` the provider was called with.
async fn transports_seen_with(cli: Settings) -> Vec<Option<Transport>> {
    let fx = fixture();
    let seen: Arc<Mutex<Vec<Option<Transport>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::factory(move |_ctx, opts, _s, _m| {
        sink.lock().unwrap().push(opts.transport);
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
    })]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    let session =
        SessionBuilder::new(provider, cfg).cli_settings(cli).build().await.unwrap();
    let _ = session.prompt("hello").await.unwrap();
    session.wait_for_idle().await;
    let out = seen.lock().unwrap().clone();
    assert!(!out.is_empty(), "the provider was never called — the run did not reach the stream seam");
    out
}

/// A persisted `"transport": "sse"` must arrive at the provider as `Transport::Sse`.
#[tokio::test]
async fn configured_transport_reaches_the_provider_stream_options() {
    let seen = transports_seen_with(Settings::parse(r#"{"transport":"sse"}"#).unwrap()).await;
    assert!(
        seen.iter().all(|t| *t == Some(Transport::Sse)),
        "`transport: sse` must reach StreamOptions.transport, saw {seen:?}"
    );
}

/// `websocket` is not special-cased into the same bucket — the whole `TransportSetting` union
/// round-trips (Pi's `Transport` is `"sse" | "websocket" | "websocket-cached" | "auto"`).
#[tokio::test]
async fn websocket_transport_round_trips_distinctly() {
    let seen =
        transports_seen_with(Settings::parse(r#"{"transport":"websocket"}"#).unwrap()).await;
    assert!(
        seen.iter().all(|t| *t == Some(Transport::Websocket)),
        "`transport: websocket` must reach StreamOptions.transport, saw {seen:?}"
    );
}

/// The legacy `websockets: false` boolean migrates to `transport: "sse"`
/// (`settings.rs:371-378`, Pi `settings-manager.ts` migration) — and that migrated value must now
/// travel the same wire, which is the half that was missing.
#[tokio::test]
async fn the_migrated_legacy_websockets_boolean_reaches_the_provider() {
    let seen = transports_seen_with(Settings::parse(r#"{"websockets":false}"#).unwrap()).await;
    assert!(
        seen.iter().all(|t| *t == Some(Transport::Sse)),
        "the `websockets` → `transport` migration must reach the provider, saw {seen:?}"
    );
}

/// With nothing configured the agent still streams with Pi's `"auto"` default, so the wiring is not
/// a behavior change for the unconfigured majority.
#[tokio::test]
async fn unset_transport_stays_auto() {
    let seen = transports_seen_with(Settings::new()).await;
    assert!(
        seen.iter().all(|t| *t == Some(Transport::Auto)),
        "an unset `transport` must remain `auto`, saw {seen:?}"
    );
}

/// CFG-006 / AGENT-031 — the `websocketConnectTimeoutMs` setting must reach the provider's
/// `StreamOptions`.
///
/// pi resolves it in the session `streamFn` as
/// `options?.websocketConnectTimeoutMs ?? settingsManager.getWebSocketConnectTimeoutMs()` and
/// spreads it onto every `streamSimple` call (`core/sdk.ts:310-311,314` @v0.83.0).
///
/// RED before this pass: BOTH halves existed and neither was connected —
/// `Settings::websocket_connect_timeout_ms` parsed and validated the key (`settings.rs:732`) and
/// `AgentBuilder::websocket_connect_timeout_ms` threaded it onto `StreamOptions`
/// (`cyrup-provider/src/stream.rs:201`), but nothing in `SessionBuilder` assigned it. A user who
/// set the key got no error and no effect, which is the AGENT-021 defect shape: a field documented
/// as live that silently sends nothing. The factory below would observe `None`.
#[tokio::test]
async fn websocket_connect_timeout_setting_reaches_the_providers_stream_options() {
    let fx = fixture();
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("websocketConnectTimeoutMs", serde_json::json!(7_500)).unwrap();

    let seen: Arc<std::sync::Mutex<Vec<Option<u64>>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::factory({
        let seen = Arc::clone(&seen);
        move |_ctx, options, _state, _model| {
            crate::sync::lock(&seen).push(options.websocket_connect_timeout_ms);
            faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)
        }
    })]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx))
        .cli_settings(cli)
        .build()
        .await
        .expect("build")
        .into_shared();

    let _ = session.prompt(UserInput::text("go", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;

    assert_eq!(
        crate::sync::lock(&seen).clone(),
        vec![Some(7_500)],
        "the resolved `websocketConnectTimeoutMs` must arrive on `StreamOptions` for every request"
    );
}
