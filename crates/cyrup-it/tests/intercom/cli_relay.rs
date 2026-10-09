//! ICOM-074 / ICOM-075 / ICOM-071 — the explicit cross-machine SSH relay, both halves, against
//! each other.
//!
//! Every test runs the BUILT `cyrup-intercom-cli` binary as a subprocess against a real
//! `cyrup-intercom-broker` process, with a real [`IntercomClient`] as the recipient — the
//! arrangement an `ssh host 'cyrup-intercom-cli relay --envelope-stdin --json'` has on the far
//! machine. Upstream's `cli.test.ts` drives `runCli` with a `FakeClient`; its relay cases are
//! proved here on the wire instead: the recipient's `Message` is what a real session reads.
//!
//! The two halves are proved against EACH OTHER, not against a hand-written reply:
//!
//! * [`send_cross_machine`] (the sender) is driven through a [`CommandRunner`] double that execs the
//!   real binary where `ssh` would go, so the JSON the CLI prints is exactly what the sender
//!   parses — including the default `crossMachine.remoteCommand`, which is the case that used to
//!   end in `NoRelaySupport` ("needs upgrading") because the remote had no `relay` subcommand.
//! * the CLI's own `send --to name@machine` is driven with fake `herdr` and `ssh` executables on
//!   `PATH`, the second of which runs the real `relay` against a SECOND broker: two hosts, two
//!   brokers, two real CLI processes, one message.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use cyrup_intercom::config::{DEFAULT_REMOTE_COMMAND, config_path};
use cyrup_intercom::cross_machine::{
    CommandResult, CommandRunner, CrossMachineDeps, CrossMachineError, CrossMachineOrigin,
    MAX_RELAY_ENVELOPE_BYTES, MAX_RELAY_TEXT_BYTES, RELAY_MESSAGE_PREFIX, send_cross_machine,
};
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient};
use cyrup_intercom::transport::protocol::{CrossMachineKind, Message, RelayTrust, SessionInfo};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::broadcast::Receiver;

use super::common::{Broker, registration};
use crate::support::scratch::Scratch;

const ORIGIN_SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";

fn bin_dir() -> PathBuf {
    crate::support::bins::intercom_cli()
        .parent()
        .expect("the CLI binary has a directory")
        .to_path_buf()
}

/// `PATH` with `front` ahead of the inherited one.
fn path_with(front: &Path) -> String {
    let inherited = std::env::var("PATH").unwrap_or_default();
    format!("{}:{inherited}", front.display())
}

/// The "remote" host's install: the directory of the `cyrup` binary at `cyrup` (the default
/// `remoteCommand`, `cyrup intercom`), then the CLI's (one and the same unless `CYRUP_IT_BIN_DIR`
/// splits them).
fn remote_path_dirs(cyrup: &Path) -> PathBuf {
    let cyrup_dir = cyrup.parent().expect("the cyrup binary has a directory");
    PathBuf::from(format!("{}:{}", cyrup_dir.display(), bin_dir().display()))
}

/// Run the CLI with `args`, `stdin` on its standard input, at `agent_dir`, plus `env`.
async fn run_cli(
    scratch: &Scratch,
    agent_dir: &Path,
    args: &[&str],
    stdin: &[u8],
    env: &[(&str, &str)],
) -> Output {
    let mut cmd =
        crate::support::env::hermetic(crate::support::bins::intercom_cli(), &scratch.home());
    cmd.current_dir(scratch.work())
        .env("CYRUP_CODING_AGENT_DIR", agent_dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().expect("spawn cyrup-intercom-cli");
    let mut pipe = child.stdin.take().expect("piped stdin");
    let input = stdin.to_vec();
    // Written on its own task: a refusal that exits before reading must not wedge the test.
    let writer = tokio::spawn(async move {
        let _ = pipe.write_all(&input).await;
        let _ = pipe.shutdown().await;
    });
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .unwrap_or_else(|_| panic!("cyrup-intercom-cli {args:?} did not exit within 30 s"))
        .expect("run cyrup-intercom-cli");
    let _ = writer.await;
    output
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("utf-8 stderr")
}

fn keys(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_object()
        .expect("a JSON object")
        .keys()
        .map(String::as_str)
        .collect()
}

/// The envelope a sender writes (`cross-machine-transport.ts:89`).
fn envelope(target: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "target": target,
        "text": text,
        "trust": "ssh-asserted",
        "origin": { "name": "worker", "sessionId": ORIGIN_SESSION_ID, "machine": "laptop" },
    })
}

async fn peer(broker: &Broker, name: &str) -> Arc<IntercomClient> {
    Arc::new(
        IntercomClient::connect(&broker.socket, registration(name), None)
            .await
            .expect("peer connects"),
    )
}

async fn next_message(rx: &mut Receiver<InboundEvent>) -> (SessionInfo, Message) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { from, message })) => return (from, *message),
            Ok(Ok(_other)) => continue,
            Ok(Err(e)) => panic!("event channel error: {e}"),
            Err(_) => panic!("timed out waiting for the relayed message"),
        }
    }
}

/// No message reaches `rx` within a short window.
async fn assert_no_message(rx: &mut Receiver<InboundEvent>, why: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { message, .. })) => {
                panic!("{why}: a message was delivered: {message:?}")
            }
            Ok(Ok(_other)) => continue,
            Ok(Err(_)) | Err(_) => return,
        }
    }
}

/// The ONE compact JSON line a `--json` run prints.
fn one_json_line(out: &Output) -> serde_json::Value {
    let text = stdout(out);
    assert!(
        text.ends_with('\n') && text.matches('\n').count() == 1,
        "{text:?}"
    );
    assert!(!text.contains("\n  "), "compact, not pretty: {text:?}");
    serde_json::from_str(&text).expect("stdout is JSON")
}

/// `runCli relay registers an ephemeral asserted sender and forwards the envelope`
/// (`cli.test.ts:271-291`), on the wire: the recipient reads the marker line then the body, the
/// `crossMachine` provenance, and a sender named `name@machine` that is a runtime fallback alias;
/// the CLI prints `{ ok, delivered, id, origin, trust }` as ONE compact line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_delivers_the_marked_body_with_crossmachine_provenance() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&broker, "reviewer").await;
    let mut rx = reviewer.subscribe();

    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &["relay", "--envelope-stdin", "--json"],
        envelope("reviewer", "hello").to_string().as_bytes(),
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");

    let (from, message) = next_message(&mut rx).await;
    assert_eq!(
        message.content.text, "[Unverified cross-machine origin]\nhello",
        "the prompt-injection marker, byte for byte"
    );
    assert_eq!(RELAY_MESSAGE_PREFIX, "[Unverified cross-machine origin]\n");
    let provenance = message.cross_machine.as_ref().expect("crossMachine is set");
    assert_eq!(provenance.kind, CrossMachineKind::SshRelay);
    assert_eq!(provenance.trust, RelayTrust::SshAsserted);
    assert_eq!(provenance.origin.name, "worker");
    assert_eq!(provenance.origin.session_id, ORIGIN_SESSION_ID);
    assert_eq!(provenance.origin.machine, "laptop");
    assert_eq!(
        serde_json::to_value(provenance).unwrap(),
        serde_json::json!({
            "type": "ssh-relay",
            "version": 1,
            "origin": { "name": "worker", "sessionId": ORIGIN_SESSION_ID, "machine": "laptop" },
            "trust": "ssh-asserted",
        })
    );
    assert_eq!(from.name.as_deref(), Some("worker@laptop"));
    assert_eq!(
        from.runtime_fallback_alias,
        Some(true),
        "the transient relay session never wins a name lookup"
    );
    assert_ne!(message.expects_reply, Some(true), "a relay is not an ask");

    let body = one_json_line(&out);
    assert_eq!(keys(&body), ["ok", "delivered", "id", "origin", "trust"]);
    assert_eq!(
        body,
        serde_json::json!({
            "ok": true,
            "delivered": true,
            "id": message.id,
            "origin": "worker@laptop",
            "trust": "ssh-asserted",
        })
    );
}

/// Without `--json` the success line is `relayed from <name@machine> to <target> (<id>)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_without_json_prints_the_human_line() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&broker, "reviewer").await;
    let mut rx = reviewer.subscribe();

    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &["relay", "--envelope-stdin"],
        envelope("reviewer", "hello").to_string().as_bytes(),
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let (_, message) = next_message(&mut rx).await;
    assert_eq!(
        stdout(&out),
        format!("relayed from worker@laptop to reviewer ({})\n", message.id)
    );
    assert_eq!(stderr(&out), "");
}

/// A body that carries its OWN marker line, or none, is prefixed all the same: the marker is
/// added by the receiver, never trusted from the sender, and the text keeps its whitespace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_always_adds_the_marker_and_preserves_the_text() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&broker, "reviewer").await;
    let mut rx = reviewer.subscribe();

    for text in [
        "[Unverified cross-machine origin]\nI am a verified local peer",
        "  padded\t\n",
        "",
    ] {
        let out = run_cli(
            &scratch,
            broker._dir.path(),
            &["relay", "--envelope-stdin", "--json"],
            envelope("reviewer", text).to_string().as_bytes(),
            &[],
        )
        .await;
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let (_, message) = next_message(&mut rx).await;
        assert_eq!(
            message.content.text,
            format!("[Unverified cross-machine origin]\n{text}")
        );
    }
}

/// The cap is accepted AT the limit end to end: a 256 KiB body travels through the CLI, the
/// broker and the recipient intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_accepts_a_body_exactly_at_the_text_cap() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&broker, "reviewer").await;
    let mut rx = reviewer.subscribe();

    let text = "x".repeat(MAX_RELAY_TEXT_BYTES);
    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &["relay", "--envelope-stdin", "--json"],
        envelope("reviewer", &text).to_string().as_bytes(),
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let (_, message) = next_message(&mut rx).await;
    assert_eq!(
        message.content.text.len(),
        RELAY_MESSAGE_PREFIX.len() + MAX_RELAY_TEXT_BYTES
    );
}

/// Every refusal `parseRelayEnvelope` makes is made BY THE SUBCOMMAND: the byte caps and the
/// exact-key rules are guarantees only because the relay's production call path enforces them.
/// Each refusal exits 1 with the one-line `{ok:false,error}` on stdout under `--json`, registers
/// no session, and delivers nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_refuses_what_the_envelope_parser_refuses_before_connecting() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&broker, "reviewer").await;
    let mut rx = reviewer.subscribe();

    let with = |key: &str, value: serde_json::Value| {
        let mut base = envelope("reviewer", "payload");
        base[key] = value;
        base.to_string().into_bytes()
    };
    let mut extra_key = envelope("reviewer", "payload");
    extra_key["extra"] = true.into();
    let mut extra_origin_key = envelope("reviewer", "payload");
    extra_origin_key["origin"]["extra"] = true.into();
    let mut oversized = envelope("reviewer", "payload").to_string().into_bytes();
    oversized.resize(MAX_RELAY_ENVELOPE_BYTES + 1, b' ');

    let cases: Vec<(&str, Vec<u8>, String)> = vec![
        (
            "invalid JSON",
            b"{".to_vec(),
            "Invalid cross-machine relay envelope JSON.".to_string(),
        ),
        (
            "empty stdin",
            Vec::new(),
            "Invalid cross-machine relay envelope JSON.".to_string(),
        ),
        (
            "not an object",
            b"[]".to_vec(),
            "Invalid cross-machine relay envelope.".to_string(),
        ),
        (
            "version 2",
            with("version", 2.into()),
            "Unsupported cross-machine relay envelope version; upgrade cyrup-intercom on both machines."
                .to_string(),
        ),
        (
            "trust verified",
            with("trust", "verified".into()),
            "Unsupported cross-machine relay envelope trust.".to_string(),
        ),
        (
            "undocumented key",
            extra_key.to_string().into_bytes(),
            "Invalid cross-machine relay envelope.".to_string(),
        ),
        (
            "undocumented origin key",
            extra_origin_key.to_string().into_bytes(),
            "Invalid cross-machine relay envelope.".to_string(),
        ),
        (
            "blank target",
            with("target", " \t".into()),
            "Invalid cross-machine relay envelope.".to_string(),
        ),
        (
            "oversized target",
            with("target", "t".repeat(1025).into()),
            "Invalid cross-machine relay envelope.".to_string(),
        ),
        (
            "oversized text",
            with("text", "x".repeat(MAX_RELAY_TEXT_BYTES + 1).into()),
            "Invalid cross-machine relay envelope.".to_string(),
        ),
        (
            "blank origin name",
            with(
                "origin",
                serde_json::json!({ "name": " ", "sessionId": "s", "machine": "m" }),
            ),
            "Invalid cross-machine relay envelope.".to_string(),
        ),
        (
            "oversized envelope",
            oversized,
            format!("Cross-machine relay envelope exceeds {MAX_RELAY_ENVELOPE_BYTES} byte limit."),
        ),
        (
            "relay-of-a-relay",
            with("target", "reviewer@workstation".into()),
            "relay target must be a local name or session id".to_string(),
        ),
    ];
    for (label, input, expected) in cases {
        let out = run_cli(
            &scratch,
            broker._dir.path(),
            &["relay", "--envelope-stdin", "--json"],
            &input,
            &[],
        )
        .await;
        assert_eq!(out.status.code(), Some(1), "{label}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{label}: --json writes nothing to stderr");
        let body = one_json_line(&out);
        assert_eq!(
            body,
            serde_json::json!({ "ok": false, "error": expected }),
            "{label}"
        );
        assert_no_message(&mut rx, label).await;
    }

    // Without `--json` the same sentence goes to stderr and stdout stays empty.
    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &["relay", "--envelope-stdin"],
        &with("version", 2.into()),
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "Unsupported cross-machine relay envelope version; upgrade cyrup-intercom on both machines.\n"
    );
}

/// `runCli relay cannot recurse to a remote address` (`cli.test.ts:293-304`): the refusal comes
/// BEFORE the broker is touched, so no `worker@laptop` session ever joins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_of_a_relay_never_registers_a_session() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let observer = peer(&broker, "observer").await;
    let mut rx = observer.subscribe();

    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &["relay", "--envelope-stdin", "--json"],
        envelope("reviewer@workstation", "hello")
            .to_string()
            .as_bytes(),
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    let deadline = tokio::time::Instant::now() + Duration::from_millis(600);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::SessionJoined(joined))) => {
                panic!("a session joined for a refused relay: {joined:?}")
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => break,
        }
    }
}

/// A target that is not connected is the broker's refusal, reported as `relay delivery failed:`
/// with the broker's reason (`cli.ts:226`) — the CLI exits 1 and the sender reads `ok: false`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_to_an_unknown_target_reports_relay_delivery_failed() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let _reviewer = peer(&broker, "reviewer").await;

    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &["relay", "--envelope-stdin", "--json"],
        envelope("ghost", "hello").to_string().as_bytes(),
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    let body = one_json_line(&out);
    assert_eq!(keys(&body), ["ok", "error"]);
    assert_eq!(body["ok"], false);
    let error = body["error"].as_str().expect("error string");
    assert!(error.starts_with("relay delivery failed: "), "{error:?}");
    assert!(!error.contains("unknown reason"), "{error:?}");
}

/// No broker running: `cannot reach the local intercom broker` — the failure a sender's `ssh`
/// reports as the relay's refusal, with the cyrup wording of upstream's hint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_without_a_broker_says_so() {
    let scratch = Scratch::new();
    let empty = tempfile::tempdir().expect("tempdir");
    let out = run_cli(
        &scratch,
        empty.path(),
        &["relay", "--envelope-stdin", "--json"],
        envelope("reviewer", "hello").to_string().as_bytes(),
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    let body = one_json_line(&out);
    assert_eq!(body["ok"], false);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .starts_with("cannot reach the local intercom broker: "),
        "{body}"
    );
}

/// `parseCliArgs keeps relay hidden and restricted to an stdin envelope`, as the process: a usage
/// error exits 1 with the usage text, and under `--json` as the one-line failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_usage_errors_exit_1_with_the_usage_text() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    for args in [
        &["relay"][..],
        &["relay", "--envelope-stdin", "--text", "hello"],
        &["relay", "--envelope-stdin", "--name", "cyrup-intercom-cli"],
        &["relay", "--envelope-stdin", "--envelope-stdin"],
    ] {
        let out = run_cli(&scratch, broker._dir.path(), args, b"", &[]).await;
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let err = stderr(&out);
        assert!(
            err.starts_with("relay requires only --envelope-stdin (and optional --json)\nusage: cyrup-intercom-cli <list|send|ask>"),
            "{args:?}: {err:?}"
        );
        let (_, usage) = err.split_once('\n').expect("a message line then the usage");
        assert!(
            !usage.contains("relay"),
            "relay stays out of the usage text: {usage:?}"
        );
    }
    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &["send", "--to", "x", "--text", "y", "--envelope-stdin"],
        b"",
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).starts_with("--envelope-stdin is only valid for relay\n"));
}

// ---------------------------------------------------------------------------------------------
// The two halves against each other.
// ---------------------------------------------------------------------------------------------

/// A [`CommandRunner`] that answers Herdr's two discovery calls from fixtures and, where
/// `send_cross_machine` would run `ssh`, EXECS THE REAL CLI instead: the remote command string
/// goes to `sh -c` exactly as sshd would run it, with `PATH` set so the default `cyrup intercom`
/// (the `cyrup` binary) and the standalone `cyrup-intercom-cli` both resolve, and the broker the
/// "remote" host owns.
struct ExecInsteadOfSsh {
    remote_agent_dir: PathBuf,
    scratch_home: PathBuf,
    machines: String,
    agents: String,
    /// The `ssh` argv and stdin of the last delivery, for assertions.
    ssh_seen: std::sync::Mutex<Option<(Vec<String>, String)>>,
}

#[async_trait::async_trait]
impl CommandRunner for ExecInsteadOfSsh {
    async fn run(
        &self,
        command: &str,
        args: &[&str],
        stdin: Option<&str>,
        timeout: Option<Duration>,
    ) -> std::io::Result<CommandResult> {
        if command == "herdr" {
            let stdout = if args.first() == Some(&"machine") {
                self.machines.clone()
            } else {
                self.agents.clone()
            };
            return Ok(CommandResult {
                stdout,
                stderr: String::new(),
                code: 0,
                timed_out: false,
            });
        }
        assert_eq!(command, "ssh", "only herdr and ssh are ever run");
        *self.ssh_seen.lock().unwrap() = Some((
            args.iter().map(|a| (*a).to_string()).collect(),
            stdin.unwrap_or_default().to_string(),
        ));
        let remote_command = args.get(1).copied().unwrap_or_default();
        let mut cmd = crate::support::env::hermetic("sh", &self.scratch_home);
        // `cyrup` only lends `PATH` its directory: the child is the hermetic `sh` above.
        let cyrup = crate::support::bins::cyrup();
        cmd.arg("-c")
            .arg(remote_command)
            .env("PATH", path_with(&remote_path_dirs(&cyrup)))
            .env("CYRUP_CODING_AGENT_DIR", &self.remote_agent_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut cmd = tokio::process::Command::from(cmd);
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let mut pipe = child.stdin.take().expect("piped stdin");
        let input = stdin.unwrap_or_default().to_string();
        tokio::spawn(async move {
            let _ = pipe.write_all(input.as_bytes()).await;
            let _ = pipe.shutdown().await;
        });
        let output = tokio::time::timeout(
            timeout.unwrap_or(Duration::from_secs(30)),
            child.wait_with_output(),
        )
        .await
        .expect("the remote relay finished within the delivery timeout")?;
        Ok(CommandResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(1),
            timed_out: false,
        })
    }
}

fn exec_runner(remote: &Broker, scratch: &Scratch, agents: &str) -> ExecInsteadOfSsh {
    ExecInsteadOfSsh {
        remote_agent_dir: remote._dir.path().to_path_buf(),
        scratch_home: scratch.home(),
        machines: r#"[{"label":"workstation","target":"user@ws","enabled":true}]"#.to_string(),
        agents: agents.to_string(),
        ssh_seen: std::sync::Mutex::new(None),
    }
}

fn sender_origin() -> CrossMachineOrigin {
    CrossMachineOrigin {
        name: "worker".to_string(),
        session_id: ORIGIN_SESSION_ID.to_string(),
        machine: "laptop".to_string(),
    }
}

const REVIEWER: &str = r#"{"result":{"agents":[{"agent":"cyrup","name":"reviewer"}]}}"#;

/// ICOM-074 proved against ICOM-075: the sender's `send_cross_machine`, with the DEFAULT
/// `remoteCommand` (`cyrup intercom`), delivers to a real recipient through the real `cyrup`
/// binary — the one every install has. Before the `relay` subcommand existed this ended in
/// `NoRelaySupport` ("needs upgrading"), because the remote answered `unknown command: relay` and
/// printed no JSON; and while the default named the standalone `cyrup-intercom-cli`, a host
/// installed the documented way (`cargo install … cyrup`) answered `sh: not found` (exit 127).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_senders_default_remote_command_delivers_through_the_real_binary() {
    let remote = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&remote, "reviewer").await;
    let mut rx = reviewer.subscribe();
    let runner = exec_runner(&remote, &scratch, REVIEWER);

    let deps = CrossMachineDeps::new(&runner, "herdr", DEFAULT_REMOTE_COMMAND);
    let delivered = send_cross_machine(
        "reviewer@workstation",
        "ship it; $(touch /tmp/nope)\n\"quoted\"",
        sender_origin(),
        &deps,
    )
    .await
    .expect("the default remote command reaches a relay-capable host");

    let (from, message) = next_message(&mut rx).await;
    assert_eq!(
        message.content.text,
        "[Unverified cross-machine origin]\nship it; $(touch /tmp/nope)\n\"quoted\""
    );
    assert_eq!(from.name.as_deref(), Some("worker@laptop"));
    let provenance = message.cross_machine.expect("crossMachine");
    assert_eq!(provenance.origin.session_id, ORIGIN_SESSION_ID);

    // The CLI's stdout is exactly what `sendCrossMachine` returned and parsed.
    let reply: serde_json::Value = serde_json::from_str(&delivered.stdout).expect("JSON");
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["delivered"], true);
    assert_eq!(reply["id"], message.id.as_str());
    assert_eq!(reply["origin"], "worker@laptop");
    assert_eq!(reply["trust"], "ssh-asserted");

    let (argv, stdin) = runner
        .ssh_seen
        .lock()
        .unwrap()
        .clone()
        .expect("ssh was run");
    assert_eq!(
        argv,
        vec![
            "user@ws".to_string(),
            "cyrup intercom relay --envelope-stdin --json".to_string()
        ]
    );
    assert!(stdin.ends_with('\n'));
}

/// The relay's refusals reach the SENDER as a refusal, not as "needs upgrading": a relay that
/// answers `{ok:false,error}` is a v1 relay, whatever it refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_relays_refusals_reach_the_sender_as_remote_refusals() {
    let remote = Broker::start().await;
    let scratch = Scratch::new();
    let _reviewer = peer(&remote, "reviewer").await;

    // The agent list names an agent that is not actually connected to the remote broker.
    let runner = exec_runner(
        &remote,
        &scratch,
        r#"{"result":{"agents":[{"agent":"cyrup","name":"ghost"}]}}"#,
    );
    let deps = CrossMachineDeps::new(&runner, "herdr", DEFAULT_REMOTE_COMMAND);
    let error = send_cross_machine("ghost@workstation", "hi", sender_origin(), &deps)
        .await
        .expect_err("the remote broker has no ghost");
    match error {
        CrossMachineError::RemoteRefused { machine, error } => {
            assert_eq!(machine, "workstation");
            assert!(error.starts_with("relay delivery failed: "), "{error:?}");
        }
        other => panic!("expected the relay's own refusal, got {other:?}"),
    }

    // A sender whose origin the relay refuses (blank machine) gets the parser's sentence.
    let runner = exec_runner(&remote, &scratch, REVIEWER);
    let deps = CrossMachineDeps::new(&runner, "herdr", DEFAULT_REMOTE_COMMAND);
    let blank = CrossMachineOrigin {
        machine: " ".to_string(),
        ..sender_origin()
    };
    let error = send_cross_machine("reviewer@workstation", "hi", blank, &deps)
        .await
        .expect_err("a blank origin machine is refused by the receiving parser");
    assert_eq!(
        error,
        CrossMachineError::RemoteRefused {
            machine: "workstation".to_string(),
            error: "Invalid cross-machine relay envelope.".to_string(),
        }
    );
}

/// A host whose `remoteCommand` has no `relay` subcommand — a pre-relay build — IS the
/// "needs upgrading" case: the reply is not JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_command_with_no_relay_subcommand_still_says_needs_upgrading() {
    let remote = Broker::start().await;
    let scratch = Scratch::new();
    let runner = exec_runner(&remote, &scratch, REVIEWER);
    // `true` ignores its arguments and prints nothing: the shape of a build without `relay`.
    let deps = CrossMachineDeps::new(&runner, "herdr", "true");
    let error = send_cross_machine("reviewer@workstation", "hi", sender_origin(), &deps)
        .await
        .expect_err("not a relay");
    assert_eq!(
        error,
        CrossMachineError::NoRelaySupport {
            machine: "workstation".to_string()
        }
    );
}

/// A `remoteCommand` the remote SHELL cannot find — the shape of a host where `cyrup` is not on
/// the non-interactive ssh `PATH` — is named as such through a real `sh -c` (exit 127, nothing on
/// stdout), not reported as a relay that "needs upgrading".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_command_the_shell_cannot_find_is_named_not_upgrade() {
    let remote = Broker::start().await;
    let scratch = Scratch::new();
    let runner = exec_runner(&remote, &scratch, REVIEWER);
    let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-not-installed-here intercom");
    let error = send_cross_machine("reviewer@workstation", "hi", sender_origin(), &deps)
        .await
        .expect_err("nothing to run");
    assert_eq!(
        error,
        CrossMachineError::RemoteCommandNotFound {
            machine: "workstation".to_string(),
            command: "cyrup-not-installed-here".to_string(),
        }
    );
}

fn write_executable(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod script");
}

/// The two hosts as processes. Host A runs `cyrup-intercom-cli send --to reviewer@workstation`;
/// its `herdr` and `ssh` are scripts on `PATH`, and the `ssh` script runs the remote command
/// string under `sh -c` against host B's broker. The recipient on B reads a message that names
/// A's session as the asserted origin, resolved from `CYRUP_SESSION_ID`, with the `machineName`
/// from A's `config.json`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_send_to_name_at_machine_reaches_a_recipient_on_another_broker() {
    let host_a = Broker::start().await;
    let host_b = Broker::start().await;
    let scratch = Scratch::new();
    let worker = peer(&host_a, "worker").await;
    let worker_id = worker.session_id().expect("registered");
    let reviewer = peer(&host_b, "reviewer").await;
    let mut rx = reviewer.subscribe();

    std::fs::write(
        config_path(&host_a._dir.path().join("intercom")),
        r#"{"crossMachine":{"machineName":"laptop"}}"#,
    )
    .expect("write config.json");

    let scripts = scratch.dir("scripts");
    write_executable(
        &scripts,
        "herdr",
        r#"#!/bin/sh
case "$1" in
  machine) echo '[{"label":"workstation","target":"user@ws","enabled":true}]' ;;
  --machine) echo '{"result":{"agents":[{"agent":"cyrup","name":"reviewer"}]}}' ;;
  *) echo "unexpected herdr call: $*" >&2; exit 2 ;;
esac
"#,
    );
    write_executable(
        &scripts,
        "ssh",
        r#"#!/bin/sh
# ssh <target> <remote command string>: run it as sshd would, on host B.
shift
exec env CYRUP_CODING_AGENT_DIR="$HOST_B_AGENT_DIR" sh -c "$1"
"#,
    );
    let path = path_with(&bin_dir());
    let path = format!("{}:{path}", scripts.display());
    let host_b_dir = host_b._dir.path().to_string_lossy().into_owned();

    let out = run_cli(
        &scratch,
        host_a._dir.path(),
        &[
            "send",
            "--to",
            "Reviewer@WORKSTATION",
            "--text",
            "ship it",
            "--json",
        ],
        b"",
        &[
            ("PATH", path.as_str()),
            ("HERDR_BIN_PATH", scripts.join("herdr").to_str().unwrap()),
            ("HOST_B_AGENT_DIR", host_b_dir.as_str()),
            ("CYRUP_SESSION_ID", worker_id.as_str()),
        ],
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "");
    let body = one_json_line(&out);
    assert_eq!(
        keys(&body),
        ["ok", "delivered", "crossMachine", "machine", "target"]
    );
    assert_eq!(
        body,
        serde_json::json!({
            "ok": true,
            "delivered": true,
            "crossMachine": true,
            "machine": "workstation",
            "target": "reviewer",
        })
    );

    let (from, message) = next_message(&mut rx).await;
    assert_eq!(
        message.content.text,
        "[Unverified cross-machine origin]\nship it"
    );
    assert_eq!(from.name.as_deref(), Some("worker@laptop"));
    let provenance = message.cross_machine.expect("crossMachine");
    assert_eq!(provenance.origin.name, "worker");
    assert_eq!(
        provenance.origin.session_id, worker_id,
        "resolved from CYRUP_SESSION_ID against host A's roster"
    );
    assert_eq!(provenance.origin.machine, "laptop");
}

/// The human line for a cross-machine send names the DISCOVERED name and label.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_cross_machine_send_prints_the_discovered_address_over_ssh() {
    let host_a = Broker::start().await;
    let host_b = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&host_b, "reviewer").await;
    let mut rx = reviewer.subscribe();
    let scripts = scratch.dir("scripts");
    write_executable(
        &scripts,
        "herdr",
        r#"#!/bin/sh
case "$1" in
  machine) echo '[{"label":"Workstation","target":"user@ws","enabled":true}]' ;;
  --machine) echo '{"result":{"agents":[{"agent":"cyrup","name":"Reviewer"}]}}' ;;
esac
"#,
    );
    write_executable(
        &scripts,
        "ssh",
        "#!/bin/sh\nshift\nexec env CYRUP_CODING_AGENT_DIR=\"$HOST_B_AGENT_DIR\" sh -c \"$1\"\n",
    );
    let path = format!("{}:{}", scripts.display(), path_with(&bin_dir()));
    let host_b_dir = host_b._dir.path().to_string_lossy().into_owned();
    let out = run_cli(
        &scratch,
        host_a._dir.path(),
        &["send", "--to", "reviewer@workstation", "--text", "hi"],
        b"",
        &[
            ("PATH", path.as_str()),
            ("HERDR_BIN_PATH", scripts.join("herdr").to_str().unwrap()),
            ("HOST_B_AGENT_DIR", host_b_dir.as_str()),
        ],
    )
    .await;
    // The remote broker's session is named `reviewer` (lower-case) but Herdr reports `Reviewer`:
    // the relay's target is Herdr's spelling, which the remote broker resolves case-insensitively.
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), "delivered to Reviewer@Workstation over SSH\n");
    // No `CYRUP_SESSION_ID`, no roster row for the CLI's own name (its transient registration is
    // excluded): the origin falls back to `--name`, the session id to `unknown`, and the machine
    // to this host's default name — never blank.
    let (from, message) = next_message(&mut rx).await;
    let name = from.name.expect("the relay session is named");
    let (cli_name, machine) = name.split_once('@').expect("name@machine");
    assert_eq!(cli_name, "cyrup-intercom-cli");
    assert!(
        !machine.is_empty() && machine == machine.to_lowercase() && !machine.contains('.'),
        "{machine:?}"
    );
    assert_eq!(
        message
            .cross_machine
            .expect("crossMachine")
            .origin
            .session_id,
        "unknown"
    );
}

/// `runCli treats name@machine as an explicit remote address before local delivery`,
/// `runCli and discovery share the explicit remote address corpus` and
/// `ask` refusing an `@` target (`cli.test.ts:231-269`): none of these registers a session or
/// touches the local broker, and a local failure never falls back to a cross-machine send.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_refuses_malformed_and_ask_targets_before_registering() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let observer = peer(&broker, "observer").await;
    let mut rx = observer.subscribe();

    let mut cases: Vec<(Vec<&str>, &str)> = Vec::new();
    for target in [
        "@workstation",
        "reviewer@",
        "reviewer@@workstation",
        " reviewer@workstation",
        "reviewer@workstation ",
        "review er@workstation",
        "reviewer@work\tstation",
    ] {
        cases.push((
            vec!["send", "--to", target, "--text", "hello", "--json"],
            "Invalid remote target",
        ));
    }
    cases.push((
        vec![
            "ask",
            "--to",
            "reviewer@workstation",
            "--text",
            "hello",
            "--json",
        ],
        "ask only supports local names or session ids",
    ));
    for (args, expected) in cases {
        let out = run_cli(&scratch, broker._dir.path(), &args, b"", &[]).await;
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let body = one_json_line(&out);
        assert_eq!(body["ok"], false, "{args:?}");
        assert!(
            body["error"].as_str().unwrap().contains(expected),
            "{args:?}: {body}"
        );
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::SessionJoined(joined))) => {
                panic!("a refused invocation registered a session: {joined:?}")
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => break,
        }
    }
}

/// A cross-machine send that cannot be resolved is `explicit cross-machine delivery failed:` and
/// never a silent local delivery — even though a LOCAL session named `reviewer` exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_cross_machine_send_never_falls_back_to_a_local_session() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let reviewer = peer(&broker, "reviewer").await;
    let mut rx = reviewer.subscribe();
    let scripts = scratch.dir("scripts");
    write_executable(&scripts, "herdr", "#!/bin/sh\necho '[]'\n");

    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &[
            "send",
            "--to",
            "reviewer@workstation",
            "--text",
            "hello",
            "--json",
        ],
        b"",
        &[("HERDR_BIN_PATH", scripts.join("herdr").to_str().unwrap())],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    let body = one_json_line(&out);
    assert_eq!(
        body,
        serde_json::json!({
            "ok": false,
            "error": "explicit cross-machine delivery failed: Saved Herdr machine \"workstation\" is unknown or disabled.",
        })
    );
    assert_no_message(&mut rx, "no local fallback").await;
}

/// A malformed `config.json` refuses a cross-machine send with its path (where the machine name
/// and remote command are read), and leaves every other command alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_config_refuses_only_the_cross_machine_send() {
    let broker = Broker::start().await;
    let scratch = Scratch::new();
    let _peer = peer(&broker, "reviewer").await;
    std::fs::write(
        config_path(&broker._dir.path().join("intercom")),
        r#"{"crossMachine":{"machineName":""}}"#,
    )
    .expect("write config.json");

    let out = run_cli(
        &scratch,
        broker._dir.path(),
        &[
            "send",
            "--to",
            "reviewer@workstation",
            "--text",
            "hello",
            "--json",
        ],
        b"",
        &[],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    let body = one_json_line(&out);
    let error = body["error"].as_str().unwrap();
    assert!(
        error.starts_with("Failed to load intercom config at "),
        "{error:?}"
    );
    assert!(
        error.ends_with("\"crossMachine.machineName\" must be a non-empty string"),
        "{error:?}"
    );

    let out = run_cli(&scratch, broker._dir.path(), &["list"], b"", &[]).await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}
