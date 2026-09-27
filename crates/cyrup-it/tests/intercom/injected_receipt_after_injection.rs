//! ICOM-069 — the `injected` receipt is emitted only once the host has TAKEN the message
//! (pi-intercom v0.14.0 `17699ba`, `index.ts:1230-1245`: `pi.sendMessage(…); emitMessageReceipt(
//! injectedMessage.id, "injected")`). A hand-off that fails emits nothing, so the sender's last
//! known state stays `acknowledged` — and that state is what an `ask` timeout quotes as "Last known
//! delivery state" (`latestDeliveryState`), so a supervisor is not told a message it should re-send
//! was injected.
//!
//! Real on both ends: a genuine broker, the receiver's production `spawn_inbound_loop`, and the
//! SENDER's production `spawn_inbound_loop` recording receipts into its own
//! `SharedIntercomState::latest_delivery_state`. Only the receiver's host is a double, because a
//! failing `inject_message` is the condition under test.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::common::{Broker, registration, within};
use cyrup_ext::HostServices;
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::inbound::spawn_inbound_loop;
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::transport::client::{IntercomClient, SendOptions};

/// An idle host (so every inbound message takes the trigger delivery) whose `inject_message` fails
/// while `fail` is set — the live host's `Err` when no session is wired or its injection pump is
/// gone.
struct FlakyHost {
    fail: AtomicBool,
    attempts: AtomicUsize,
}

impl HostServices for FlakyHost {
    fn is_idle(&self) -> bool {
        true
    }
    fn append_entry(
        &self,
        _custom_type: &str,
        _data: &serde_json::Value,
    ) -> Result<String, String> {
        Ok("entry-1".to_string())
    }
    fn inject_message(
        &self,
        _content: &str,
        _custom_type: Option<&str>,
        _display: bool,
        _details: Option<&serde_json::Value>,
        _trigger_turn: bool,
    ) -> Result<(), String> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            Err("session injection pump is no longer running".to_string())
        } else {
            Ok(())
        }
    }
}

async fn session(broker: &Broker, name: &str) -> (Arc<IntercomClient>, Arc<SharedIntercomState>) {
    let client = Arc::new(
        IntercomClient::connect(
            &broker.socket,
            registration(name),
            Some(format!("{name}-session")),
        )
        .await
        .expect("connects to the broker"),
    );
    let state = Arc::new(SharedIntercomState::new(
        IntercomConfig::default(),
        600_000,
        PathBuf::from("/tmp/work"),
    ));
    state.set_client(Some(client.clone()));
    state.set_has_ui(true);
    spawn_inbound_loop(state.clone(), client.clone());
    (client, state)
}

fn plain(id: &str, text: &str) -> SendOptions {
    SendOptions {
        text: text.to_string(),
        message_id: Some(id.to_string()),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_injection_leaves_the_senders_last_state_at_acknowledged() {
    let broker = Broker::start().await;
    let (receiver, receiver_state) = session(&broker, "worker").await;
    let host = Arc::new(FlakyHost {
        fail: AtomicBool::new(true),
        attempts: AtomicUsize::new(0),
    });
    receiver_state.set_host_services(host.clone());
    let (sender, sender_state) = session(&broker, "supervisor").await;
    let target = receiver.session_id().expect("registered");

    // The host refuses the hand-off.
    assert!(
        sender
            .send(&target, plain("lost", "please do X"))
            .await
            .unwrap()
            .delivered
    );
    assert!(
        within(Duration::from_secs(10), || host
            .attempts
            .load(Ordering::SeqCst)
            == 1)
        .await,
        "the receiver tried to hand the message to its host"
    );

    // The host recovers; the next message is taken. Receipts from one receiver arrive in the order
    // it emitted them, so once THIS `injected` is in, any `injected` for `lost` would be too.
    host.fail.store(false, Ordering::SeqCst);
    assert!(
        sender
            .send(&target, plain("taken", "then do Y"))
            .await
            .unwrap()
            .delivered
    );
    assert!(
        within(Duration::from_secs(10), || sender_state
            .latest_delivery_state(Some("taken"), "none")
            == "injected")
        .await,
        "a taken message is reported injected: {}",
        sender_state.latest_delivery_state(Some("taken"), "none")
    );
    assert_eq!(
        sender_state.latest_delivery_state(Some("lost"), "none"),
        "acknowledged",
        "a message the host never took is not reported injected"
    );
    assert_eq!(host.attempts.load(Ordering::SeqCst), 2);
    receiver.disconnect();
    sender.disconnect();
}
