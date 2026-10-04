//! ICOM-074 — the cross-machine send path, against `cross-machine-*.test.ts` at `v0.16.0`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Mutex;
use std::time::Duration;

use super::*;

/// One scripted process invocation.
struct Invocation {
    command: String,
    args: Vec<String>,
    stdin: Option<String>,
    timeout: Option<Duration>,
}

/// A [`CommandRunner`] that answers from a script keyed on the first argv token, and records what
/// it was asked to run. The production runner spawns processes; every assertion below is about the
/// ARGV and the envelope, which is exactly what upstream's own tests inject a `run` double for.
struct ScriptedRunner {
    machine_list: CommandResult,
    agent_list: CommandResult,
    ssh: CommandResult,
    seen: Mutex<Vec<Invocation>>,
}

fn ok(stdout: &str) -> CommandResult {
    CommandResult {
        stdout: stdout.to_string(),
        stderr: String::new(),
        code: 0,
        timed_out: false,
    }
}

fn failed(stderr: &str, code: i32) -> CommandResult {
    CommandResult {
        stdout: String::new(),
        stderr: stderr.to_string(),
        code,
        timed_out: false,
    }
}

impl ScriptedRunner {
    fn new(machine_list: CommandResult, agent_list: CommandResult, ssh: CommandResult) -> Self {
        Self {
            machine_list,
            agent_list,
            ssh,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> std::sync::MutexGuard<'_, Vec<Invocation>> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait::async_trait]
impl CommandRunner for ScriptedRunner {
    async fn run(
        &self,
        command: &str,
        args: &[&str],
        stdin: Option<&str>,
        timeout: Option<Duration>,
    ) -> std::io::Result<CommandResult> {
        self.calls().push(Invocation {
            command: command.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            stdin: stdin.map(str::to_string),
            timeout,
        });
        Ok(match (command, args.first().copied()) {
            ("ssh", _) => self.ssh.clone(),
            (_, Some("machine")) => self.machine_list.clone(),
            _ => self.agent_list.clone(),
        })
    }
}

const MACHINES: &str = r#"[{"id":"m1","label":"workstation","target":"user@ws","enabled":true}]"#;
const AGENTS: &str = r#"{"agents":[
  {"agent":"cyrup","name":"reviewer","cwd":"/repo","agent_status":"idle",
   "agent_session":{"kind":"path","value":"/h/.cyrup/sessions/s_6f1c2a7e-1111-4222-8333-444455556666.jsonl"}}
]}"#;
const RELAY_OK: &str =
    r#"{"ok":true,"delivered":true,"id":"m-9","origin":"alice@laptop","trust":"ssh-asserted"}"#;

fn origin() -> CrossMachineOrigin {
    CrossMachineOrigin {
        name: "alice".to_string(),
        session_id: "sess-1".to_string(),
        machine: "laptop".to_string(),
    }
}

/// `parseCrossMachineTarget` (`v0.16.0 cross-machine-discovery.ts:80-86`) — exactly two non-blank,
/// whitespace-free halves. A second `@` is a REFUSAL, not a last-`@` split, which is what keeps a
/// relay-of-a-relay unspellable.
#[test]
fn a_target_is_exactly_one_at_between_two_non_blank_halves() {
    assert_eq!(
        parse_cross_machine_target("reviewer@workstation").unwrap(),
        ("reviewer".to_string(), "workstation".to_string())
    );
    for bad in [
        "reviewer",
        "@workstation",
        "reviewer@",
        "a@b@c",
        "rev iewer@ws",
        "reviewer@w s",
        "@",
    ] {
        let error = parse_cross_machine_target(bad).expect_err(bad);
        assert_eq!(
            error.to_string(),
            format!(
                "Invalid remote target \"{bad}\"; expected name@machine or full-session-uuid@machine."
            )
        );
    }
}

/// `parseSavedMachines` (`:49-58`): a bare array or `{machines: […]}`, rows missing a string
/// `label`/`target` SKIPPED rather than fatal, and `enabled !== false`.
#[test]
fn the_machine_catalog_keys_on_label_and_defaults_enabled_true() {
    let parsed = parse_saved_machines(
        r#"{"machines":[
            {"label":"a","target":"t-a"},
            {"label":"b","target":"t-b","enabled":false},
            {"target":"no-label"},
            {"label":"c"},
            "junk"
        ]}"#,
    )
    .unwrap();
    assert_eq!(
        parsed,
        vec![
            SavedMachine {
                label: "a".to_string(),
                target: "t-a".to_string(),
                enabled: true
            },
            SavedMachine {
                label: "b".to_string(),
                target: "t-b".to_string(),
                enabled: false
            },
        ]
    );
    // The `result` unwrap (`parseJsonOutput`, `:40-47`) and the bare-array shape.
    assert_eq!(
        parse_saved_machines(r#"{"result":[{"label":"a","target":"t"}]}"#)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        parse_saved_machines("not json").unwrap_err().to_string(),
        "herdr machine list returned invalid JSON."
    );
}

/// `parseRemoteAgents` (`:60-78`): the agent-kind filter, the `_<uuid>.jsonl` session-id
/// extraction `93c5a01` added, and the "unnamed session is addressed by its id" fallback.
#[test]
fn remote_agents_are_filtered_by_agent_kind_and_named_by_session_id_when_unnamed() {
    let parsed = parse_remote_agents(
        r#"{"agents":[
            {"agent":"other","name":"nope"},
            {"agent":"cyrup","name":"reviewer","cwd":"/repo","agent_status":"working",
             "agent_session":{"kind":"path","value":"/s/x_6f1c2a7e-1111-4222-8333-444455556666.jsonl"}},
            {"agent":"cyrup","name":"",
             "agent_session":{"kind":"path","value":"/s/y_aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.jsonl"}},
            {"agent":"cyrup","agent_session":{"kind":"id","value":"not-a-path"}},
            {"agent":"cyrup","name":"plain"}
        ]}"#,
    )
    .unwrap();
    assert_eq!(
        parsed,
        vec![
            RemoteAgent {
                name: "reviewer".to_string(),
                session_id: Some("6f1c2a7e-1111-4222-8333-444455556666".to_string()),
                cwd: Some("/repo".to_string()),
                status: Some("working".to_string()),
            },
            RemoteAgent {
                name: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
                session_id: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()),
                cwd: None,
                status: None,
            },
            RemoteAgent {
                name: "plain".to_string(),
                session_id: None,
                cwd: None,
                status: None,
            },
        ],
        "the non-cyrup row and the nameless row with no path-borne id have no address"
    );
    // A BARE array is not accepted for agents, unlike the machine catalog (`:62`).
    assert!(
        parse_remote_agents(r#"[{"agent":"cyrup","name":"x"}]"#)
            .unwrap()
            .is_empty()
    );
}

/// `discoverRemoteAgent` (`:119-145`) is `=== 1` at BOTH steps, with a distinct sentence for zero
/// and for many. `747326b` tightened this precisely so a relay never guesses.
#[tokio::test]
async fn discovery_refuses_zero_and_many_at_both_steps() {
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "missing machine",
            r#"[{"label":"other","target":"t","enabled":true}]"#,
            AGENTS,
            "Saved Herdr machine \"workstation\" is unknown or disabled.",
        ),
        (
            "disabled machine",
            r#"[{"label":"workstation","target":"t","enabled":false}]"#,
            AGENTS,
            "Saved Herdr machine \"workstation\" is unknown or disabled.",
        ),
        (
            "two enabled machines with the label",
            r#"[{"label":"workstation","target":"a","enabled":true},
                {"label":"WORKSTATION","target":"b","enabled":true}]"#,
            AGENTS,
            "Saved Herdr machine label \"workstation\" is ambiguous; expected exactly one enabled machine.",
        ),
        (
            "no agent matches",
            MACHINES,
            r#"{"agents":[{"agent":"cyrup","name":"someone-else"}]}"#,
            "No live cyrup agent on saved Herdr machine \"workstation\" exactly matches \"reviewer\".",
        ),
        (
            "two agents match",
            MACHINES,
            r#"{"agents":[{"agent":"cyrup","name":"reviewer"},{"agent":"cyrup","name":"Reviewer"}]}"#,
            "Multiple live cyrup agents on saved Herdr machine \"workstation\" exactly match \"reviewer\"; target is ambiguous.",
        ),
    ];
    for (label, machines, agents, expected) in cases {
        let runner = ScriptedRunner::new(ok(machines), ok(agents), ok(RELAY_OK));
        let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
        let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .expect_err(label);
        assert_eq!(error.to_string(), expected, "{label}");
        assert!(
            !runner.calls().iter().any(|call| call.command == "ssh"),
            "{label}: nothing is sent when the target does not resolve to exactly one agent"
        );
    }
}

/// `listMachineAgents` (`:94-111`) unwraps herdr's Rust debug error into a readable reason and
/// appends the "server is too old" hint for the one detail that has a remedy.
#[tokio::test]
async fn an_unreachable_machine_reports_herdrs_own_reason_and_the_upgrade_hint() {
    let runner = ScriptedRunner::new(
        ok(MACHINES),
        failed(
            r#"Error: Custom { kind: Other, error: "machine 'workstation': server does not support machine API forwarding" }"#,
            1,
        ),
        ok(RELAY_OK),
    );
    let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
    let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
        .await
        .expect_err("unreachable");
    assert_eq!(
        error.to_string(),
        "Saved Herdr machine \"workstation\" is unreachable: server does not support machine API \
         forwarding. Its running Herdr server is too old; update Herdr there, then run \
         `herdr --remote user@ws` in a terminal to replace the server."
    );

    // A failure with no envelope falls back to `stderr.trim() || exit <code>`, and gets NO hint.
    let plain = ScriptedRunner::new(ok(MACHINES), failed("boom.", 3), ok(RELAY_OK));
    let deps = CrossMachineDeps::new(&plain, "herdr", "cyrup-intercom-cli");
    assert_eq!(
        send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .expect_err("plain")
            .to_string(),
        "Saved Herdr machine \"workstation\" is unreachable: boom.",
        "exactly one sentence-final period: `detail.replace(/\\.$/, \"\")` then a literal `.`"
    );
}

/// `sendCrossMachine` (`:72-110`) — the happy path's ARGV and envelope, which is the whole wire
/// contract the remote relay parses.
#[tokio::test]
async fn a_resolved_target_is_sent_as_an_ssh_relay_envelope_on_stdin() {
    let runner = ScriptedRunner::new(ok(MACHINES), ok(AGENTS), ok(RELAY_OK));
    let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
    let delivered = send_cross_machine("reviewer@workstation", "ship it", origin(), &deps)
        .await
        .expect("delivered");
    assert_eq!(delivered.discovered.machine.label, "workstation");
    assert_eq!(delivered.discovered.agent.name, "reviewer");
    assert_eq!(delivered.stdout, RELAY_OK);

    let calls = runner.calls();
    assert_eq!(calls.len(), 3, "machine list, agent list, ssh");
    assert_eq!(calls[0].command, "herdr");
    assert_eq!(calls[0].args, vec!["machine", "list", "--json"]);
    assert_eq!(calls[0].timeout, Some(DISCOVERY_TIMEOUT));
    assert_eq!(calls[1].command, "herdr");
    assert_eq!(
        calls[1].args,
        vec!["--machine", "workstation", "agent", "list"],
        "`herdr --machine <label> agent list` — the forwarding form"
    );
    assert_eq!(calls[2].command, "ssh");
    assert_eq!(
        calls[2].args,
        vec![
            "user@ws",
            "cyrup-intercom-cli relay --envelope-stdin --json"
        ],
        "the ssh target is the machine's SSH target, and the remote argv is one string"
    );
    assert_eq!(calls[2].timeout, Some(DELIVERY_TIMEOUT));
    let stdin = calls[2]
        .stdin
        .as_deref()
        .expect("the envelope rides on stdin");
    assert!(stdin.ends_with('\n'), "`${{JSON.stringify(envelope)}}\\n`");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(stdin.trim()).unwrap(),
        serde_json::json!({
            "version": 1,
            // `match.agent.sessionId ?? match.agent.name` — the id wins when discovery found one.
            "target": "6f1c2a7e-1111-4222-8333-444455556666",
            "text": "ship it",
            "origin": { "name": "alice", "sessionId": "sess-1", "machine": "laptop" },
            "trust": "ssh-asserted",
        })
    );
}

/// The `remoteCommand` guards (`:81-83`) run BEFORE discovery, so a malformed config costs no
/// `herdr` round trips — and a control character never reaches the string `ssh` interprets.
#[tokio::test]
async fn a_blank_or_control_bearing_remote_command_is_refused_before_any_process_runs() {
    for (command, expected) in [
        ("", "Remote command must not be empty."),
        ("   ", "Remote command must not be empty."),
        (
            "cyrup-intercom-cli\nrm -rf /",
            "Remote command must not contain ASCII control characters.",
        ),
        (
            "cyrup-intercom-cli\u{7f}",
            "Remote command must not contain ASCII control characters.",
        ),
    ] {
        let runner = ScriptedRunner::new(ok(MACHINES), ok(AGENTS), ok(RELAY_OK));
        let deps = CrossMachineDeps::new(&runner, "herdr", command);
        let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .expect_err(command);
        assert_eq!(error.to_string(), expected);
        assert!(
            runner.calls().is_empty(),
            "the guard runs before discovery, so nothing is spawned"
        );
    }
}

/// `relaySupportError` (`:68-70`) vs a real refusal (`:106-109`) — a non-JSON, non-record,
/// `ok`-less or version-mismatched reply means "upgrade", and only an `{ok:false, error}` object is
/// reported as a delivery failure.
#[tokio::test]
async fn an_unparseable_or_versioned_reply_is_an_upgrade_notice_not_a_delivery_failure() {
    let upgrade = "Remote cyrup-intercom on \"workstation\" has no compatible relay support and needs upgrading.";
    for (reply, code, expected) in [
        ("cyrup-intercom-cli: unknown command: relay", 1, upgrade),
        ("[]", 0, upgrade),
        (r#"{"delivered":true}"#, 0, upgrade),
        (r#"{"ok":true,"version":2}"#, 0, upgrade),
        // ok:false with no `error` string is still "upgrade": the relay did not speak the protocol.
        (r#"{"ok":false}"#, 1, upgrade),
        (
            r#"{"ok":false,"error":"Session \"x\" is not connected."}"#,
            1,
            "Remote intercom delivery via workstation failed: Session \"x\" is not connected.",
        ),
        // A ZERO exit with `ok: false` is still a refusal — upstream ORs the two (`:106`).
        (
            r#"{"ok":false,"error":"nope"}"#,
            0,
            "Remote intercom delivery via workstation failed: nope",
        ),
    ] {
        let runner = ScriptedRunner::new(
            ok(MACHINES),
            ok(AGENTS),
            CommandResult {
                stdout: reply.to_string(),
                stderr: String::new(),
                code,
                timed_out: false,
            },
        );
        let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
        let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .expect_err(reply);
        assert_eq!(error.to_string(), expected, "reply: {reply}");
    }
    // `version: 1` present is the v0.16.0 shape and must NOT be read as incompatible.
    let runner = ScriptedRunner::new(ok(MACHINES), ok(AGENTS), ok(r#"{"ok":true,"version":1}"#));
    let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
    assert!(
        send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .is_ok()
    );
}

/// `runCommand` (`:34-66`) as `53580c2` hardened it: a timed-out run reports code 124 with the
/// flag set, and the child is killed rather than waited on.
#[tokio::test]
async fn the_spawn_runner_reports_124_and_timed_out_when_the_deadline_elapses() {
    let runner = SpawnRunner;
    let slow = runner
        .run(
            "sh",
            &["-c", "sleep 30"],
            None,
            Some(Duration::from_millis(50)),
        )
        .await
        .expect("spawned");
    assert_eq!(slow.code, 124);
    assert!(slow.timed_out);

    // Stdin is written and the pipe CLOSED, or a remote relay reading it would never return.
    let echoed = runner
        .run(
            "cat",
            &[],
            Some("envelope\n"),
            Some(Duration::from_secs(10)),
        )
        .await
        .expect("spawned");
    assert_eq!(echoed.stdout, "envelope\n");
    assert_eq!(echoed.code, 0);
    assert!(!echoed.timed_out);

    // A non-zero exit is a RESULT, not an error: only a failure to start is an `Err`.
    let failing = runner
        .run("sh", &["-c", "echo oops >&2; exit 3"], None, None)
        .await
        .expect("spawned");
    assert_eq!(failing.code, 3);
    assert_eq!(failing.stderr.trim(), "oops");
    assert!(
        runner
            .run("cyrup-no-such-binary-icom074", &[], None, None)
            .await
            .is_err()
    );
}
