//! ICOM-067 — the scripting client `cyrup-intercom-cli`, the port of pi-intercom v0.14.0's `cli.ts`
//! (`41dc8f6`, #131).
//!
//! Every test runs the BUILT binary as a subprocess against a real `cyrup-intercom-broker` process,
//! with a real [`IntercomClient`] as the peer on the other end — the same arrangement a script on
//! the machine (or one reached over `ssh`) has. The upstream suite (`cli.test.ts`) drives `runCli`
//! with a `FakeClient`; its parse/registration cases live as unit tests beside the binary, and the
//! `runCli` cases are proved here against the real broker instead.
//!
//! The child is built by `support::env::hermetic`, so an ambient `CYRUP_INTERCOM_SCOPE_ID` or
//! `CYRUP_CODING_AGENT_DIR` on the developer's shell cannot route it anywhere but the test's own
//! broker; each test sets exactly the variables it means.

use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_intercom::transport::client::{
    InboundEvent, IntercomClient, LivenessConfig, SendOptions,
};
use cyrup_intercom::transport::protocol::{Message, ScopeId, SessionInfo};
use cyrup_intercom::transport::target::BrokerConnectTarget;
use tokio::sync::broadcast::Receiver;

use super::common::{Broker, registration};
use crate::support::scratch::Scratch;

/// `CLI_USAGE`'s first line, which every usage error ends with.
const USAGE_HEAD: &str = "usage: cyrup-intercom-cli <list|send|ask>";

/// Run the CLI with `args`, pointed at `agent_dir` (where the broker's socket lives) plus `env`.
async fn cli(scratch: &Scratch, agent_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd =
        crate::support::env::hermetic(crate::support::bins::intercom_cli(), &scratch.home());
    cmd.current_dir(scratch.work())
        .env("CYRUP_CODING_AGENT_DIR", agent_dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(20), cmd.output())
        .await
        .unwrap_or_else(|_| panic!("cyrup-intercom-cli {args:?} did not exit within 20 s"))
        .expect("run cyrup-intercom-cli")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("utf-8 stderr")
}

/// The one JSON line a `--json` failure prints (`cli.ts:139-141`): exactly one line, nothing on
/// stderr.
fn json_failure(out: &Output) -> serde_json::Value {
    assert_eq!(stderr(out), "", "a --json failure writes nothing to stderr");
    let text = stdout(out);
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    assert_eq!(lines.len(), 1, "exactly one JSON line on stdout: {text:?}");
    let body: serde_json::Value = serde_json::from_str(lines[0]).expect("stdout is JSON");
    assert_eq!(body["ok"], false, "{body}");
    body
}

/// The object's keys, in the order they were written.
fn keys(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_object()
        .expect("a JSON object")
        .keys()
        .map(String::as_str)
        .collect()
}

async fn peer(broker: &Broker, name: &str) -> Arc<IntercomClient> {
    Arc::new(
        IntercomClient::connect(&broker.socket, registration(name), None)
            .await
            .expect("peer connects"),
    )
}

/// The next routed message on `rx`, skipping roster events.
async fn next_message(rx: &mut Receiver<InboundEvent>) -> (SessionInfo, Message) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { from, message })) => return (from, *message),
            Ok(Ok(_other)) => continue,
            Ok(Err(e)) => panic!("event channel error: {e}"),
            Err(_) => panic!("timed out waiting for the CLI's message over the broker"),
        }
    }
}

/// `list` prints `name\tid[0..8]\tmodel\tstatus\tcwd` per session (`cli.ts:166-169`), including
/// the CLI's own registration (`cli.ts:113-122`: its cwd, `model` = its name, `status: "idle"`);
/// `list --json` prints the pretty `{ok, sessions}` object with `(unnamed)` for a nameless session
/// and the full id (`cli.ts:163-164`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn list_prints_the_roster_plain_and_as_json() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let worker = peer(&broker, "worker").await;
    let worker_id = worker.session_id().expect("registered");
    let mut nameless = registration("x");
    nameless.name = None;
    let _nameless = IntercomClient::connect(&broker.socket, nameless, None)
        .await
        .expect("nameless peer connects");

    let out = cli(&scratch, broker._dir.path(), &["list"], &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let text = stdout(&out);
    let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
    let worker_row = rows
        .iter()
        .find(|r| r[0] == "worker")
        .unwrap_or_else(|| panic!("no worker row in {text:?}"));
    assert_eq!(
        worker_row,
        &vec!["worker", &worker_id[..8], "test-model", "?", "/tmp/work"],
        "a status-less session prints `?`, the id is cut to 8 chars"
    );
    assert!(
        rows.iter().any(|r| r[0] == "(unnamed)"),
        "a nameless session prints `(unnamed)`: {text:?}"
    );
    let work = scratch.work().canonicalize().expect("canonical work dir");
    let own = rows
        .iter()
        .find(|r| r[0] == "cyrup-intercom-cli")
        .unwrap_or_else(|| panic!("the CLI registers as a regular session: {text:?}"));
    assert_eq!(own.len(), 5, "{own:?}");
    assert_eq!(
        own[2..],
        ["cyrup-intercom-cli", "idle", &*work.to_string_lossy()]
    );
    assert!(text.ends_with('\n'), "{text:?}");

    let out = cli(&scratch, broker._dir.path(), &["list", "--json"], &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let text = stdout(&out);
    assert!(
        text.starts_with("{\n  \"ok\": true,\n  \"sessions\": ["),
        "JSON.stringify(…, null, 2): {text:?}"
    );
    let body: serde_json::Value = serde_json::from_str(&text).expect("stdout is JSON");
    assert_eq!(keys(&body), ["ok", "sessions"]);
    let sessions = body["sessions"].as_array().expect("sessions array");
    let worker_json = sessions
        .iter()
        .find(|s| s["name"] == "worker")
        .unwrap_or_else(|| panic!("no worker in {body}"));
    assert_eq!(keys(worker_json), ["name", "id", "model", "status", "cwd"]);
    assert_eq!(
        worker_json,
        &serde_json::json!({
            "name": "worker",
            "id": worker_id,
            "model": "test-model",
            "status": "?",
            "cwd": "/tmp/work",
        })
    );
    assert!(sessions.iter().any(|s| s["name"] == "(unnamed)"), "{body}");
}

/// `send` delivers the text to the peer as a plain message (no `expectsReply`) under the CLI's
/// session name, and prints `delivered to <to> (<id>)` naming the id the peer received
/// (`cli.ts:174-183`). `--name` renames the CLI's session; `--json` prints `{ok, delivered, id}`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_delivers_text_and_reports_the_message_id() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let worker = peer(&broker, "worker").await;
    let mut rx = worker.subscribe();

    let out = cli(
        &scratch,
        broker._dir.path(),
        &["send", "--to", "worker", "--text", "build failed"],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let (from, message) = next_message(&mut rx).await;
    assert_eq!(message.content.text, "build failed");
    assert_ne!(message.expects_reply, Some(true), "send is not an ask");
    assert_eq!(from.name.as_deref(), Some("cyrup-intercom-cli"));
    assert_eq!(
        stdout(&out),
        format!("delivered to worker ({})\n", message.id)
    );

    let out = cli(
        &scratch,
        broker._dir.path(),
        &[
            "send", "--to", "worker", "--text", "again", "--name", "bridge", "--json",
        ],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let (from, message) = next_message(&mut rx).await;
    assert_eq!(message.content.text, "again");
    assert_eq!(
        from.name.as_deref(),
        Some("bridge"),
        "--name names the CLI's session"
    );
    let body: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("stdout is JSON");
    assert_eq!(keys(&body), ["ok", "delivered", "id"]);
    assert_eq!(
        body,
        serde_json::json!({ "ok": true, "delivered": true, "id": message.id })
    );
}

/// A send the broker refuses exits 1 with `delivery failed: <reason>` on stderr
/// (`cli.ts:176-178`), and under `--json` as the single failure line with no `reason` key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_to_an_unknown_target_exits_1_with_delivery_failed() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let _worker = peer(&broker, "worker").await;

    let out = cli(
        &scratch,
        broker._dir.path(),
        &["send", "--to", "ghost", "--text", "hi"],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(err.starts_with("delivery failed: "), "{err:?}");
    assert!(
        !err.contains("unknown reason"),
        "the broker's reason is carried: {err:?}"
    );

    let out = cli(
        &scratch,
        broker._dir.path(),
        &["send", "--to", "ghost", "--text", "hi", "--json"],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    let body = json_failure(&out);
    assert_eq!(keys(&body), ["ok", "error"]);
    assert_eq!(body["error"], err.trim_end_matches('\n'));
}

/// `ask` sends with `expectsReply` and prints the text of the message whose `replyTo` names it
/// (`cli.ts:187-242`). A message that reaches the CLI first WITHOUT `replyTo` — delivered, so the
/// CLI really did receive it — must not be taken for the answer. `--json` prints
/// `{ok, from, text}` with the replier's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_prints_the_reply_to_its_own_question() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let worker = peer(&broker, "worker").await;

    // Answers every ask, after first sending the asker an unrelated plain message.
    let answerer = {
        let worker = worker.clone();
        let mut rx = worker.subscribe();
        tokio::spawn(async move {
            let mut asks = Vec::new();
            for answer in ["all good", "yes"] {
                let (from, message) = next_message(&mut rx).await;
                assert_eq!(message.expects_reply, Some(true), "ask sets expectsReply");
                let noise = worker
                    .send(
                        &from.id,
                        SendOptions {
                            text: "not the answer".to_string(),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("noise send");
                assert!(noise.delivered, "the unrelated message reached the CLI");
                let reply = worker
                    .send(
                        &from.id,
                        SendOptions {
                            text: answer.to_string(),
                            reply_to: Some(message.id.clone()),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("reply send");
                assert!(reply.delivered, "the reply reached the CLI");
                asks.push(message.content.text);
            }
            asks
        })
    };

    let out = cli(
        &scratch,
        broker._dir.path(),
        &[
            "ask",
            "--to",
            "worker",
            "--text",
            "status?",
            "--timeout-ms",
            "15000",
        ],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    assert_eq!(stdout(&out), "all good\n");

    let out = cli(
        &scratch,
        broker._dir.path(),
        &["ask", "--to", "worker", "--text", "ready?", "--json"],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let body: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("stdout is JSON");
    assert_eq!(keys(&body), ["ok", "from", "text"]);
    assert_eq!(
        body,
        serde_json::json!({ "ok": true, "from": "worker", "text": "yes" })
    );

    let asks = tokio::time::timeout(Duration::from_secs(20), answerer)
        .await
        .expect("answerer finishes")
        .expect("answerer task");
    assert_eq!(asks, ["status?", "ready?"]);
}

/// An `ask` nobody answers exits 2 after `--timeout-ms` with the timeout text on stderr
/// (`cli.ts:231-233`); under `--json` the failure line carries `reason: "timeout"`. The peer did
/// receive the question, so this is the timer firing, not a delivery failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_without_a_reply_times_out_with_exit_2() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let worker = peer(&broker, "worker").await;
    let mut rx = worker.subscribe();

    let started = Instant::now();
    let out = cli(
        &scratch,
        broker._dir.path(),
        &[
            "ask",
            "--to",
            "worker",
            "--text",
            "?",
            "--timeout-ms",
            "200",
        ],
        &[],
    )
    .await;
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "ask timed out after 200 ms waiting for a reply from worker\n"
    );
    let (_, question) = next_message(&mut rx).await;
    assert_eq!(question.expects_reply, Some(true));
    assert_eq!(question.content.text, "?");

    let out = cli(
        &scratch,
        broker._dir.path(),
        &[
            "ask",
            "--to",
            "worker",
            "--text",
            "?",
            "--timeout-ms",
            "200",
            "--json",
        ],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(2));
    let body = json_failure(&out);
    assert_eq!(keys(&body), ["ok", "error", "reason"]);
    assert_eq!(
        body["error"],
        "ask timed out after 200 ms waiting for a reply from worker"
    );
    assert_eq!(body["reason"], "timeout");
}

/// With no broker listening the CLI exits 1 with the connect-failure text (`cli.ts:153-158`) —
/// and it only dials: it never starts a broker of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_broker_exits_1_with_the_connect_failure_text() {
    let scratch = Scratch::new();
    let agent_dir = scratch.agent_dir();

    let out = cli(&scratch, &agent_dir, &["list"], &[]).await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(
        err.starts_with("cannot reach the local intercom broker: "),
        "{err:?}"
    );
    assert!(
        err.ends_with(
            "\nis a cyrup session with cyrup-intercom loaded currently running on this machine?\n"
        ),
        "{err:?}"
    );
    assert!(
        !agent_dir.join("intercom").join("broker.sock").exists(),
        "the CLI must not spawn a broker"
    );

    let out = cli(&scratch, &agent_dir, &["list", "--json"], &[]).await;
    assert_eq!(out.status.code(), Some(1));
    let body = json_failure(&out);
    assert_eq!(body["error"], err.trim_end_matches('\n'));
    assert_eq!(body.get("reason"), None);
}

/// A usage error is reported before any connect attempt — no broker is running here, so a CLI that
/// dialled first would answer with the connect failure instead. Plain: the message and the usage on
/// stderr. `--json` (found anywhere in the raw argv, `cli.ts:139`): exactly one JSON line on
/// stdout, nothing on stderr.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn usage_errors_precede_connect_and_honour_json() {
    let scratch = Scratch::new();
    let agent_dir = scratch.agent_dir();

    let out = cli(&scratch, &agent_dir, &["send", "--json"], &[]).await;
    assert_eq!(out.status.code(), Some(1));
    let body = json_failure(&out);
    assert_eq!(keys(&body), ["ok", "error"]);
    let error = body["error"].as_str().expect("error string");
    assert!(
        error.starts_with("--to is required for send\n"),
        "{error:?}"
    );
    assert!(error.contains(USAGE_HEAD), "{error:?}");

    let out = cli(&scratch, &agent_dir, &["teleport"], &[]).await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(
        err.starts_with(&format!("unknown command: teleport\n{USAGE_HEAD}")),
        "{err:?}"
    );

    let out = cli(
        &scratch,
        &agent_dir,
        &["ask", "--to", "w", "--text", "?", "--timeout-ms", "1.5"],
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert_eq!(stderr(&out), "invalid --timeout-ms value: 1.5\n");
}

/// `CYRUP_INTERCOM_SCOPE_ID` puts the CLI's registration in that scope (`v0.13.0
/// broker/client.ts:286`), so its `list` sees only same-scope sessions — and an unscoped CLI does
/// not see the scoped one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scope_id_isolates_the_roster() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let _open = peer(&broker, "open-peer").await;
    let _scoped = IntercomClient::connect_target_with_liveness(
        &BrokerConnectTarget::Socket(broker.socket.clone()),
        registration("scoped-peer"),
        None,
        ScopeId::parse("team-a"),
        LivenessConfig::default(),
    )
    .await
    .expect("scoped peer connects");

    let names = |out: &Output| -> Vec<String> {
        assert_eq!(out.status.code(), Some(0), "{}", stderr(out));
        let body: serde_json::Value = serde_json::from_str(&stdout(out)).expect("stdout is JSON");
        body["sessions"]
            .as_array()
            .expect("sessions array")
            .iter()
            .map(|s| s["name"].as_str().expect("name").to_string())
            .collect()
    };

    let scoped = cli(
        &scratch,
        broker._dir.path(),
        &["list", "--json"],
        &[("CYRUP_INTERCOM_SCOPE_ID", "team-a")],
    )
    .await;
    let scoped = names(&scoped);
    assert!(scoped.iter().any(|n| n == "scoped-peer"), "{scoped:?}");
    assert!(!scoped.iter().any(|n| n == "open-peer"), "{scoped:?}");

    let open = names(&cli(&scratch, broker._dir.path(), &["list", "--json"], &[]).await);
    assert!(open.iter().any(|n| n == "open-peer"), "{open:?}");
    assert!(!open.iter().any(|n| n == "scoped-peer"), "{open:?}");
}
