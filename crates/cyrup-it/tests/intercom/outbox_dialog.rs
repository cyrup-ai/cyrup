//! The extension outbox's `confirmSend` dialog (`pi-intercom` `v0.16.0 index.ts:1190-1206`), driven
//! through `handle_outbox_request` — the function the `intercom:outbox-request` bus event reaches —
//! against a real broker and a connected peer.
//!
//! Unlike the `intercom` tool's dialogs (title `Send message`), the outbox asks on behalf of ANOTHER
//! EXTENSION, and upstream's dialog says so: the title is `Send extension message` and the body
//! names the extension that is asking, by name and by id, before the target and the text.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::{DialogOptions, HostServices};
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::outbox::{INTERCOM_OUTBOX_RESULT_EVENT, handle_outbox_request};
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient};

use super::common::{Broker, registration, within};

/// Declines every dialog and records both the dialog and the outbox results the session emits.
struct Declining {
    confirms: Mutex<Vec<(String, String)>>,
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl HostServices for Declining {
    fn confirm(&self, prompt: &str, message: &str, _opts: &DialogOptions) -> bool {
        self.confirms
            .lock()
            .unwrap()
            .push((prompt.to_string(), message.to_string()));
        false
    }

    fn emit_event(&self, topic: &str, payload: &serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((topic.to_string(), payload.clone()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_outbox_asks_on_the_extensions_behalf_under_upstreams_title() {
    let broker = Broker::start().await;
    let me = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("alice"),
            Some("alice-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let peer = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration("reviewer"),
            Some("peer-session".to_string()),
        )
        .await
        .expect("connects"),
    );
    let mut peer_events = peer.subscribe();

    let state = Arc::new(SharedIntercomState::new(
        IntercomConfig {
            confirm_send: true,
            ..IntercomConfig::default()
        },
        600_000,
        std::path::PathBuf::from("/w"),
    ));
    state.set_client(Some(me.clone()));
    state.set_has_ui(true);
    let host = Arc::new(Declining {
        confirms: Mutex::new(Vec::new()),
        events: Mutex::new(Vec::new()),
    });
    state.set_host_services(host.clone());

    handle_outbox_request(
        state.clone(),
        serde_json::json!({
            "version": 1,
            "requestId": "req-1",
            "extensionId": "acme.notifier",
            "extensionName": "Acme Notifier",
            "to": "peer-session",
            "message": "build finished",
        }),
    );

    assert!(
        within(Duration::from_secs(5), || {
            !host.confirms.lock().unwrap().is_empty()
        })
        .await,
        "the outbox asked the human"
    );
    assert_eq!(
        host.confirms.lock().unwrap().as_slice(),
        [(
            "Send extension message".to_string(),
            "Allow Acme Notifier (acme.notifier) to send to \"reviewer\":\n\nbuild finished"
                .to_string()
        )],
        "upstream's title, and the body names WHICH extension is asking"
    );

    // A declined dialog settles the request and sends nothing.
    assert!(
        within(Duration::from_secs(5), || {
            host.events
                .lock()
                .unwrap()
                .iter()
                .any(|(topic, _)| topic == INTERCOM_OUTBOX_RESULT_EVENT)
        })
        .await,
        "the request was settled"
    );
    let events = host.events.lock().unwrap().clone();
    let (_, result) = events
        .iter()
        .find(|(topic, _)| topic == INTERCOM_OUTBOX_RESULT_EVENT)
        .expect("a result event");
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["code"], "user_cancelled");

    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, peer_events.recv()).await {
        assert!(
            !matches!(event, InboundEvent::Message { .. }),
            "a declined outbox request delivers nothing: {event:?}"
        );
    }
    me.disconnect();
    peer.disconnect();
}
