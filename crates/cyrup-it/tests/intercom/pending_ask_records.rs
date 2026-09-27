//! ICOM-057 — the broker's on-disk pending-ask records (`pi-intercom` `d69854d`, v0.11.0, issue #104;
//! `v0.14.0 broker/broker.ts:31,103-111,167-203,225-226,709,743,798,852,879,897,1027,1043,1191-1255`).
//!
//! A delivered blocking ask leaves `<intercomDir>/pending-asks/<encodeURIComponent(id)>.json`
//! (`JSON.stringify(record, null, 2) + "\n"`, mode 0600 in a 0700 directory) until its ask edge goes
//! away. Every test here runs the real `cyrup-intercom-broker` binary over a real Unix socket and
//! reads the directory the way an outside observer would.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::{broker_bin, registration, within};
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::{IntercomClient, SendOptions};
use cyrup_intercom::transport::spawn::wait_for_broker;

struct TestBroker {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    records: PathBuf,
    child: tokio::process::Child,
}

impl Drop for TestBroker {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// A broker whose ask timeout is `ask_timeout_ms`, over an agent dir `seed` may pre-populate.
async fn start_broker(ask_timeout_ms: u64, seed: impl FnOnce(&Path)) -> TestBroker {
    let dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(dir.path());
    let records = intercom_dir.join("pending-asks");
    seed(&records);
    let socket = broker_socket_path(&intercom_dir);
    let child = tokio::process::Command::new(broker_bin())
        .env("CYRUP_CODING_AGENT_DIR", dir.path())
        .env("CYRUP_INTERCOM_ASK_TIMEOUT_MS", ask_timeout_ms.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn the real intercom broker");
    wait_for_broker(&socket, Duration::from_secs(20))
        .await
        .expect("broker up");
    TestBroker {
        _dir: dir,
        socket,
        records,
        child,
    }
}

async fn client(broker: &TestBroker, name: &str) -> IntercomClient {
    IntercomClient::connect(
        &broker.socket,
        registration(name),
        Some(format!("sess-{name}")),
    )
    .await
    .expect("client connects")
}

fn ask(id: &str, text: &str) -> SendOptions {
    SendOptions {
        text: text.to_string(),
        expects_reply: Some(true),
        message_id: Some(id.to_string()),
        ..SendOptions::default()
    }
}

fn read_record(path: &Path) -> (String, serde_json::Value) {
    let body = std::fs::read_to_string(path).expect("the record exists");
    let value = serde_json::from_str(&body).expect("the record is JSON");
    (body, value)
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// `writePendingAskRecord` on the delivered-ask arm, and `removePendingAskRecord` on the reply
/// (`:709`, `:743`). The file is upstream's exactly: field order, two-space indent, trailing
/// newline, `expiresAt = createdAt + askTimeoutMs`, 0600 in a 0700 directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivered_ask_is_recorded_until_it_is_answered() {
    let broker = start_broker(90_000, |_| {}).await;
    let asker = client(&broker, "asker").await;
    let target = client(&broker, "target").await;

    asker
        .send("target", ask("ask-1", "Which branch?"))
        .await
        .expect("the ask is delivered");
    let path = broker.records.join("ask-1.json");
    let (body, record) = read_record(&path);
    let created_at = record["createdAt"].as_u64().expect("createdAt");
    assert_eq!(
        record,
        serde_json::json!({
            "askId": "ask-1",
            "messageId": "ask-1",
            "asker": { "sessionId": "sess-asker", "name": "asker" },
            "target": { "sessionId": "sess-target", "name": "target" },
            "question": "Which branch?",
            "createdAt": created_at,
            "expiresAt": created_at + 90_000,
        })
    );
    assert!(
        body.starts_with("{\n  \"askId\": \"ask-1\",\n  \"messageId\""),
        "`JSON.stringify(record, null, 2)` key order and indent: {body:?}"
    );
    assert!(body.ends_with("}\n"), "trailing newline");
    #[cfg(unix)]
    {
        assert_eq!(mode(&path), 0o600, "INTERCOM_RUNTIME_FILE_MODE");
        assert_eq!(mode(&broker.records), 0o700, "INTERCOM_DIR_MODE");
    }

    // A plain (non-blocking) send leaves no record.
    asker
        .send(
            "target",
            SendOptions {
                text: "fyi".to_string(),
                message_id: Some("note-1".to_string()),
                ..SendOptions::default()
            },
        )
        .await
        .expect("delivered");
    assert!(!broker.records.join("note-1.json").exists());

    target
        .send(
            "asker",
            SendOptions {
                text: "main".to_string(),
                reply_to: Some("ask-1".to_string()),
                ..SendOptions::default()
            },
        )
        .await
        .expect("the reply is delivered");
    assert!(!path.exists(), "the reply removes the record");

    asker.disconnect();
    target.disconnect();
}

/// `cancel_ask` removes the record (`:897`), and the file name is `encodeURIComponent(messageId)` —
/// a `/` in a client-chosen id never becomes a path separator.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_ask_removes_its_record_and_the_id_is_uri_encoded() {
    let broker = start_broker(90_000, |_| {}).await;
    let asker = client(&broker, "asker").await;
    let target = client(&broker, "target").await;

    asker
        .send("target", ask("team/ask 2", "Ship it?"))
        .await
        .expect("delivered");
    let path = broker.records.join("team%2Fask%202.json");
    assert_eq!(read_record(&path).1["messageId"], "team/ask 2");

    asker.cancel_ask("team/ask 2");
    assert!(
        within(Duration::from_secs(5), || !path.exists()).await,
        "cancel_ask removes the record"
    );

    asker.disconnect();
    target.disconnect();
}

/// `pruneAskEdges` (`:1191-1199`) runs at the head of every send: once an ask has outlived the ask
/// timeout, the next send by anyone removes its record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_ask_record_is_pruned_by_the_next_send() {
    let broker = start_broker(300, |_| {}).await;
    let asker = client(&broker, "asker").await;
    let target = client(&broker, "target").await;

    asker
        .send("target", ask("ask-3", "Still there?"))
        .await
        .expect("delivered");
    let path = broker.records.join("ask-3.json");
    assert!(path.exists());
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(path.exists(), "nothing prunes without a send");

    target
        .send(
            "asker",
            SendOptions {
                text: "unrelated".to_string(),
                ..SendOptions::default()
            },
        )
        .await
        .expect("delivered");
    assert!(
        !path.exists(),
        "the send's prune removed the expired record"
    );

    asker.disconnect();
    target.disconnect();
}

/// The constructor's `ensurePendingAskRecordDir(); this.prunePendingAskRecords();`
/// (`:225-226`, `:1236-1255`): at startup every `*.json` file that is unparseable, not a record, or
/// expired is removed; a live record and anything that is not `*.json` stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_prunes_stale_records_and_keeps_live_ones() {
    let far_future = 32_503_680_000_000_u64; // year 3000, in ms
    let live = serde_json::json!({
        "askId": "live", "messageId": "live",
        "asker": { "sessionId": "a", "name": null },
        "target": { "sessionId": "b", "name": "b" },
        "question": "q", "createdAt": 1, "expiresAt": far_future
    });
    let mut expired = live.clone();
    expired["expiresAt"] = serde_json::json!(2);
    let mut unnamed = live.clone();
    unnamed["target"].as_object_mut().unwrap().remove("name");

    let broker = start_broker(90_000, |dir| {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("live.json"), live.to_string()).unwrap();
        std::fs::write(dir.join("expired.json"), expired.to_string()).unwrap();
        std::fs::write(dir.join("unnamed.json"), unnamed.to_string()).unwrap();
        std::fs::write(dir.join("garbage.json"), "{not json").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a record").unwrap();
    })
    .await;

    let mut left: Vec<String> = std::fs::read_dir(&broker.records)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, vec!["live.json".to_string(), "notes.txt".to_string()]);
}
