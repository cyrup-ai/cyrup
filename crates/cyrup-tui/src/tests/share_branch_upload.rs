//! **DRIFT-055** — what `/share` uploads to Radius is pi's `exportSessionForShare`: the CURRENT
//! branch, linearised, under a header stamped at share time, then the `pi.share` entry
//! (`modes/interactive/session-share.ts:49-54` → `core/session-export.ts:9-29` @v0.87.1).
//!
//! cyrup uploaded the whole session tree with its original `parentId`s and the header the session
//! was created with, so a shared artifact carried every abandoned branch. This drives `/share`
//! end to end against a local stand-in for the Radius gateway and reads the body it received.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{NavigateTreeOptions, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;
use serde_json::Value;
use tempfile::TempDir;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{App, AppCommand, UiTheme};

/// Accept ONE request, return its body, answer 500 (the upload's outcome is not under test).
async fn capture_one_request(listener: tokio::net::TcpListener) -> Vec<u8> {
    let (mut sock, _) = listener.accept().await.unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let body_start = loop {
        let n = sock.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed before the headers ended");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..body_start]).to_ascii_lowercase();
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .map(|v| v.trim().parse().unwrap())
        .expect("a content-length header");
    while buf.len() < body_start + len {
        let n = sock.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed before the body ended");
        buf.extend_from_slice(&chunk[..n]);
    }
    sock.write_all(
        b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    buf[body_start..body_start + len].to_vec()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn share_uploads_the_current_branch_linearised_under_a_share_time_header() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    // A far-future radius OAuth credential, as `share_radius_route.rs` seeds it.
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{"radius":{"type":"oauth","refresh":"rt","access":"at-shared","expires":32503680000000}}"#,
    )
    .unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("answer one")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("abandoned answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("answer three")], StopReason::Stop),
    ]);
    let session = Arc::new(
        SessionBuilder::new(faux as Arc<dyn Provider>, config)
            .build()
            .await
            .unwrap(),
    );

    // Two branches: back up to before the second question and ask a different one.
    let _ = session.prompt("question one").await.unwrap();
    session.wait_for_idle().await;
    let _ = session.prompt("abandoned question").await.unwrap();
    session.wait_for_idle().await;
    let second_user = session.user_messages_for_forking().await[1]
        .entry_id
        .clone();
    session
        .navigate_tree(second_user, NavigateTreeOptions::default())
        .await
        .unwrap();
    let _ = session.prompt("question three").await.unwrap();
    session.wait_for_idle().await;
    let latest_entry = session
        .entries_json()
        .await
        .iter()
        .filter_map(|e| e["timestamp"].as_str())
        .map(|t| OffsetDateTime::parse(t, &Rfc3339).unwrap())
        .max()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(capture_one_request(listener));
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.set_radius_share_gateway(&gateway);
    app.execute_command(AppCommand::Share, &session, None).await;
    let body = String::from_utf8(server.await.unwrap()).unwrap();

    let lines: Vec<Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let header = &lines[0];
    assert_eq!(header["type"], "session", "{body}");
    let shared_at = OffsetDateTime::parse(header["timestamp"].as_str().unwrap(), &Rfc3339).unwrap();
    assert!(
        shared_at >= latest_entry,
        "the header is stamped when the share is made, not when the session began: {header}"
    );
    assert!(
        !body.contains("abandoned question") && !body.contains("abandoned answer"),
        "the abandoned branch must not be uploaded:\n{body}"
    );
    for text in [
        "question one",
        "answer one",
        "question three",
        "answer three",
    ] {
        assert!(body.contains(text), "{text} missing:\n{body}");
    }
    // One chain from `null`, ending in the `pi.share` entry stamped with the header's time.
    let mut previous = Value::Null;
    for entry in &lines[1..] {
        assert_eq!(entry["parentId"], previous, "{entry}");
        previous = entry["id"].clone();
    }
    let share = lines.last().unwrap();
    assert_eq!(share["customType"], "pi.share", "{share}");
    assert_eq!(share["timestamp"], header["timestamp"]);
}
