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

/// A remote shell that cannot find `remoteCommand` (exit 127, nothing on stdout) gets its own
/// sentence naming the program and the PATH fix, not "needs upgrading" — which would send the
/// operator after a binary that is not there. Only that exact shape: a 127 that DID print a reply,
/// or any other exit with no JSON, is still the upgrade notice.
#[tokio::test]
async fn a_remote_command_the_shell_cannot_find_says_so() {
    let reply = |stdout: &str, code: i32| CommandResult {
        stdout: stdout.to_string(),
        stderr: "sh: 1: cyrup: not found\n".to_string(),
        code,
        timed_out: false,
    };
    let runner = ScriptedRunner::new(ok(MACHINES), ok(AGENTS), reply("", 127));
    let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup intercom");
    let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
        .await
        .expect_err("not found");
    assert_eq!(
        error,
        CrossMachineError::RemoteCommandNotFound {
            machine: "workstation".to_string(),
            command: "cyrup".to_string(),
        }
    );
    assert_eq!(
        error.to_string(),
        "Remote command \"cyrup\" was not found on \"workstation\". Install cyrup there, or set \
         crossMachine.remoteCommand to its absolute path (non-interactive ssh often lacks \
         ~/.cargo/bin on PATH)."
    );

    let upgrade = "Remote cyrup-intercom on \"workstation\" has no compatible relay support and needs upgrading.";
    for (stdout, code) in [("garbage", 127), ("", 1)] {
        let runner = ScriptedRunner::new(ok(MACHINES), ok(AGENTS), reply(stdout, code));
        let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup intercom");
        let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .expect_err(stdout);
        assert_eq!(error.to_string(), upgrade, "{stdout:?} / {code}");
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

// ---------------------------------------------------------------------------------------------
// `cross-machine-transport.test.ts`, `cross-machine-discovery.test.ts` at `v0.16.0`, case by case.
// ---------------------------------------------------------------------------------------------

/// A runner whose answers come from a function of the invocation, recording every call.
struct FnRunner<F> {
    answer: F,
    seen: Mutex<Vec<Invocation>>,
}

impl<F> FnRunner<F>
where
    F: Fn(&str, &[&str]) -> CommandResult + Send + Sync,
{
    fn new(answer: F) -> Self {
        Self {
            answer,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> std::sync::MutexGuard<'_, Vec<Invocation>> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait::async_trait]
impl<F> CommandRunner for FnRunner<F>
where
    F: Fn(&str, &[&str]) -> CommandResult + Send + Sync,
{
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
        Ok((self.answer)(command, args))
    }
}

const FAKE_SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";

fn agent_list(rows: &[(&str, Option<&str>)]) -> String {
    let agents: Vec<serde_json::Value> = rows
        .iter()
        .map(|(name, session_id)| {
            let mut row = serde_json::json!({ "agent": "cyrup", "name": name });
            if let Some(session_id) = session_id {
                row["agent_session"] = serde_json::json!({
                    "agent": "cyrup",
                    "kind": "path",
                    "source": "herdr:cyrup",
                    "value": format!("/home/user/.cyrup/sessions/session_{session_id}.jsonl"),
                });
            }
            row
        })
        .collect();
    serde_json::json!({ "id": "cli:agent:list", "result": { "agents": agents } }).to_string()
}

fn discovery_deps(run: &dyn CommandRunner) -> DiscoveryDeps<'_> {
    DiscoveryDeps {
        run,
        herdr_bin: "herdr",
        discovery_timeout: DISCOVERY_TIMEOUT,
    }
}

const THREE_MACHINES: &str = r#"[
    {"label":"laptop","target":"laptop.example","enabled":true},
    {"label":"Workstation","target":"workstation.example","enabled":true},
    {"label":"disabled","target":"disabled.example","enabled":false}
]"#;

/// `discovers only the selected machine and sends hostile message text only on stdin`
/// (`cross-machine-transport.test.ts:136-167`): the disabled and unselected machines are never
/// contacted, the remote command is ONE argv element that may contain spaces, the hostile body
/// appears nowhere in argv, and every `herdr` call runs under the discovery deadline.
#[tokio::test]
async fn hostile_text_travels_only_on_stdin_and_only_the_selected_machine_is_discovered() {
    let runner = FnRunner::new(|command, args| match (command, args.first().copied()) {
        ("herdr", Some("machine")) => ok(THREE_MACHINES),
        ("herdr", _) => {
            if args.get(1) == Some(&"Workstation") {
                ok(&agent_list(&[("reviewer", Some(FAKE_SESSION_ID))]))
            } else {
                ok(r#"{"result":{"agents":[]}}"#)
            }
        }
        _ => ok(r#"{"ok":true}"#),
    });
    let hostile = "hello; $(touch /tmp/nope)\n\"quoted\" && exit 9";
    let deps = CrossMachineDeps::new(
        &runner,
        "herdr",
        "/opt/cyrup tools/cyrup-intercom-cli --profile trusted",
    );
    let delivered = send_cross_machine("reviewer@workstation", hostile, origin(), &deps)
        .await
        .unwrap();
    assert_eq!(delivered.discovered.machine.label, "Workstation");

    let calls = runner.calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.args.iter().any(|a| a == "disabled" || a == "laptop")),
        "no other machine is contacted"
    );
    let ssh = calls.last().unwrap();
    assert_eq!(ssh.command, "ssh");
    assert_eq!(
        ssh.args,
        vec![
            "workstation.example".to_string(),
            "/opt/cyrup tools/cyrup-intercom-cli --profile trusted relay --envelope-stdin --json"
                .to_string()
        ]
    );
    assert!(!ssh.args.iter().any(|a| a.contains(hostile)));
    let envelope: serde_json::Value = serde_json::from_str(ssh.stdin.as_deref().unwrap()).unwrap();
    assert_eq!(envelope["text"], hostile);
    assert_eq!(ssh.timeout, Some(DELIVERY_TIMEOUT));
    assert!(
        calls
            .iter()
            .filter(|c| c.command == "herdr")
            .all(|c| c.timeout == Some(DISCOVERY_TIMEOUT))
    );
}

/// `rejects empty or control-character remote commands before invoking SSH`
/// (`cross-machine-transport.test.ts:169-182`) — upstream's whole corpus, NUL, tab and `\x1f`
/// included.
#[tokio::test]
async fn every_control_character_in_the_remote_command_is_refused_before_ssh() {
    for command in [
        "",
        "   ",
        "cyrup-intercom-cli\0--bad",
        "cyrup-intercom-cli\t--bad",
        "cyrup-intercom-cli\u{1f}--bad",
        "cyrup-intercom-cli\u{7f}--bad",
    ] {
        let runner = ScriptedRunner::new(ok(MACHINES), ok(AGENTS), ok(r#"{"ok":true}"#));
        let deps = CrossMachineDeps::new(&runner, "herdr", command);
        let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .expect_err(command);
        let sentence = error.to_string();
        assert!(
            sentence == "Remote command must not be empty."
                || sentence == "Remote command must not contain ASCII control characters.",
            "{command:?}: {sentence}"
        );
        assert!(
            !runner.calls().iter().any(|c| c.command == "ssh"),
            "{command:?}"
        );
    }
}

/// `runCommand returns an awaitable timeout result after terminating its child`
/// (`cross-machine-transport.test.ts:184-189`): code 124, `timedOut`, and reaped promptly — a
/// child that ignores `SIGTERM` cannot hold the caller past the deadline.
#[tokio::test]
async fn a_timed_out_child_that_ignores_sigterm_is_killed_and_reaped_promptly() {
    let started = std::time::Instant::now();
    let result = SpawnRunner
        .run(
            "sh",
            &["-c", "trap '' TERM; sleep 30"],
            None,
            Some(Duration::from_millis(30)),
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        CommandResult {
            stdout: String::new(),
            stderr: String::new(),
            code: 124,
            timed_out: true
        }
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timed-out child should be reaped promptly: {:?}",
        started.elapsed()
    );
}

/// A timed-out run hands back what the child had ALREADY written (`runCommand` accumulates
/// `stdout`/`stderr` in `data` handlers and returns them from `onClose` whichever way it closed,
/// `:36-37,:55,:61-62`). Observable through the caller: a relay that printed its refusal and then
/// hung is a REFUSAL, not a missing relay.
#[tokio::test]
async fn a_timed_out_run_returns_what_the_child_had_written() {
    let result = SpawnRunner
        .run(
            "sh",
            &["-c", "printf partial; printf oops >&2; sleep 30"],
            None,
            Some(Duration::from_millis(500)),
        )
        .await
        .unwrap();
    assert!(result.timed_out);
    assert_eq!(result.code, 124);
    assert_eq!(result.stdout, "partial");
    assert_eq!(result.stderr, "oops");

    // And through `sendCrossMachine`: `{ok:false,error}` on stdout then a hang is the relay's
    // refusal (`Remote intercom delivery … failed`), not `needs upgrading`.
    struct HangsAfterRefusing;
    #[async_trait::async_trait]
    impl CommandRunner for HangsAfterRefusing {
        async fn run(
            &self,
            command: &str,
            args: &[&str],
            stdin: Option<&str>,
            timeout: Option<Duration>,
        ) -> std::io::Result<CommandResult> {
            if command == "ssh" {
                return SpawnRunner
                    .run(
                        "sh",
                        &[
                            "-c",
                            r#"printf '{"ok":false,"error":"target is busy"}'; sleep 30"#,
                        ],
                        stdin,
                        Some(Duration::from_millis(500)),
                    )
                    .await;
            }
            ScriptedRunner::new(ok(MACHINES), ok(AGENTS), ok(""))
                .run(command, args, stdin, timeout)
                .await
        }
    }
    let deps = CrossMachineDeps::new(&HangsAfterRefusing, "herdr", "cyrup-intercom-cli");
    let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Remote intercom delivery via workstation failed: target is busy"
    );
}

/// An envelope bigger than the pipe buffer, to a child that never reads stdin, must hit the
/// DEADLINE: Node's `stdin.end(input)` only queues the bytes, so the write cannot be what holds a
/// caller past its timeout (`:65`).
#[tokio::test]
async fn an_unread_stdin_cannot_hold_the_run_past_its_deadline() {
    let started = std::time::Instant::now();
    let big = "x".repeat(1024 * 1024);
    let result = SpawnRunner
        .run(
            "sh",
            &["-c", "sleep 30"],
            Some(&big),
            Some(Duration::from_millis(300)),
        )
        .await
        .unwrap();
    assert!(result.timed_out);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// `runCommand rejects once when spawning fails` (`cross-machine-transport.test.ts:191-194`).
#[tokio::test]
async fn a_command_that_cannot_start_is_an_error_not_a_result() {
    let error = SpawnRunner
        .run(
            "/definitely/not/a/command",
            &[],
            None,
            Some(Duration::from_millis(100)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

/// `non-JSON and incompatible remote output report incompatible relay support`
/// (`cross-machine-transport.test.ts:196-204`) — upstream's three replies, with a failing exit and
/// `unknown command: relay` on stderr as a pre-relay host would produce, plus `version: 1.0`
/// (`1.0 !== 1` is `false` in JS) as the compatible shape.
#[tokio::test]
async fn pre_relay_hosts_and_incompatible_replies_say_needs_upgrading() {
    for stdout in ["", r#"{"ok":"yes"}"#, r#"{"version":2,"ok":true}"#] {
        let runner = ScriptedRunner::new(
            ok(MACHINES),
            ok(AGENTS),
            CommandResult {
                stdout: stdout.to_string(),
                stderr: "unknown command: relay".to_string(),
                code: 1,
                timed_out: false,
            },
        );
        let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
        let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            CrossMachineError::NoRelaySupport {
                machine: "workstation".to_string()
            },
            "{stdout:?}"
        );
        assert!(
            error
                .to_string()
                .contains("no compatible relay support and needs upgrading")
        );
    }
    let runner = ScriptedRunner::new(ok(MACHINES), ok(AGENTS), ok(r#"{"ok":true,"version":1.0}"#));
    let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
    assert!(
        send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
            .await
            .is_ok()
    );
}

/// `structured remote failure preserves the reported error`
/// (`cross-machine-transport.test.ts:206-216`).
#[tokio::test]
async fn a_structured_remote_failure_preserves_the_reported_error() {
    let runner = ScriptedRunner::new(
        ok(MACHINES),
        ok(AGENTS),
        CommandResult {
            stdout: r#"{"ok":false,"error":"target rejected the envelope version"}"#.to_string(),
            stderr: String::new(),
            code: 1,
            timed_out: false,
        },
    );
    let deps = CrossMachineDeps::new(&runner, "herdr", "cyrup-intercom-cli");
    let error = send_cross_machine("reviewer@workstation", "hi", origin(), &deps)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Remote intercom delivery via workstation failed: target rejected the envelope version"
    );
}

/// `parses the supported explicit remote address forms` (`cross-machine-discovery.test.ts:41-50`).
#[test]
fn both_supported_address_forms_parse() {
    assert_eq!(
        parse_cross_machine_target("reviewer@Workstation").unwrap(),
        ("reviewer".to_string(), "Workstation".to_string())
    );
    assert_eq!(
        parse_cross_machine_target(&format!("{FAKE_SESSION_ID}@workstation")).unwrap(),
        (FAKE_SESSION_ID.to_string(), "workstation".to_string())
    );
}

/// `parses current Herdr machine and agent list schemas` (`:52-59`), against Herdr's CLI
/// envelope (`{id, result: {agents}}`).
#[test]
fn the_current_herdr_envelope_parses() {
    let machines = parse_saved_machines(THREE_MACHINES).unwrap();
    assert_eq!(
        machines
            .iter()
            .map(|m| (m.label.as_str(), m.enabled))
            .collect::<Vec<_>>(),
        vec![("laptop", true), ("Workstation", true), ("disabled", false)]
    );
    assert_eq!(
        parse_remote_agents(&agent_list(&[("reviewer", Some(FAKE_SESSION_ID))])).unwrap(),
        vec![RemoteAgent {
            name: "reviewer".to_string(),
            session_id: Some(FAKE_SESSION_ID.to_string()),
            cwd: None,
            status: None,
        }]
    );
}

/// `listMachineAgents turns Herdr's debug error into a readable reason with an update hint`
/// (`:61-73`) — upstream's exact stderr and exact sentence.
#[tokio::test]
async fn herdrs_debug_error_becomes_a_readable_reason_with_the_update_hint() {
    let stderr = "Error: Custom { kind: Unsupported, error: \"machine 'workmac': remote Herdr does not support machine API forwarding; update Herdr on this machine\" }\n";
    let runner = FnRunner::new(|_, _| failed(stderr, 1));
    let machine = SavedMachine {
        label: "workmac".to_string(),
        target: "10.0.0.1".to_string(),
        enabled: true,
    };
    let error = list_machine_agents(&machine, &discovery_deps(&runner))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Saved Herdr machine \"workmac\" is unreachable: remote Herdr does not support machine API forwarding; update Herdr on this machine. Its running Herdr server is too old; update Herdr there, then run `herdr --remote 10.0.0.1` in a terminal to replace the server."
    );
}

/// The regex SEARCH semantics of the unwrap (`:96`): a first `error: "` that cannot close does not
/// stop a later, well-formed one from being read; an escaped quote and an escaped backslash
/// collapse; and `.` does not match a line terminator, so a backslash before one cannot be
/// consumed.
#[tokio::test]
async fn the_herdr_error_unwrap_searches_like_a_regex() {
    async fn detail_for(stderr: &str) -> String {
        let stderr = stderr.to_string();
        let runner = FnRunner::new(move |_, _| failed(&stderr, 1));
        let machine = SavedMachine {
            label: "m".to_string(),
            target: "t".to_string(),
            enabled: true,
        };
        list_machine_agents(&machine, &discovery_deps(&runner))
            .await
            .unwrap_err()
            .to_string()
    }
    assert_eq!(
        detail_for("error: \"a\\\nb\" then error: \"real\"").await,
        "Saved Herdr machine \"m\" is unreachable: real.",
        "the first candidate hits `\\` + newline and fails; the search moves on"
    );
    assert_eq!(
        detail_for(r#"Error: Custom { error: "say \"hi\" and \\ done" }"#).await,
        "Saved Herdr machine \"m\" is unreachable: say \"hi\" and \\ done."
    );
    // No candidate closes: the detail is `stderr.trim()`.
    assert_eq!(
        detail_for("error: \"a\\\nb\"").await,
        "Saved Herdr machine \"m\" is unreachable: error: \"a\\\nb\".",
    );
}

/// `listMachineAgents reports each Pi session's cwd and status` (`:75-88`): other agent kinds are
/// not relay targets.
#[tokio::test]
async fn only_this_products_agents_are_listed_with_their_cwd_and_status() {
    let runner = FnRunner::new(|_, _| {
        ok(r#"{"result":{"agents":[
            {"agent":"cyrup","name":"adapter","cwd":"/work/adapter","agent_status":"idle"},
            {"agent":"claude","name":"other","cwd":"/work/x","agent_status":"idle"}
        ]}}"#)
    });
    let machine = SavedMachine {
        label: "workstation".to_string(),
        target: "workstation.example".to_string(),
        enabled: true,
    };
    assert_eq!(
        list_machine_agents(&machine, &discovery_deps(&runner))
            .await
            .unwrap(),
        vec![RemoteAgent {
            name: "adapter".to_string(),
            session_id: None,
            cwd: Some("/work/adapter".to_string()),
            status: Some("idle".to_string()),
        }]
    );
}

/// `unnamed remote Pi sessions are listed and targetable by session id` (`:90-103`), with the
/// path form `…/2026_<uuid>.jsonl` and the regex's `i` flag on the extension.
#[tokio::test]
async fn unnamed_sessions_are_listed_and_targetable_by_session_id() {
    let unnamed = format!(
        r#"{{"result":{{"agents":[
            {{"agent":"cyrup","agent_session":{{"kind":"path","value":"/home/user/.cyrup/sessions/x/2026_{FAKE_SESSION_ID}.jsonl"}}}},
            {{"agent":"cyrup","agent_session":{{"kind":"path","value":"/s/y_{FAKE_SESSION_ID}.JSONL"}}}},
            {{"agent":"cyrup"}}
        ]}}}}"#
    );
    let parsed = parse_remote_agents(&unnamed).unwrap();
    assert_eq!(
        parsed.len(),
        2,
        "the row with no name and no session id has no address"
    );
    assert!(
        parsed
            .iter()
            .all(|a| a.name == FAKE_SESSION_ID && a.session_id.as_deref() == Some(FAKE_SESSION_ID))
    );

    let runner = FnRunner::new(move |_, args| {
        if args.first() == Some(&"machine") {
            ok(THREE_MACHINES)
        } else {
            ok(
                r#"{"result":{"agents":[{"agent":"cyrup","agent_session":{"kind":"path","value":"/s/x/2026_00000000-0000-4000-8000-000000000001.jsonl"}}]}}"#,
            )
        }
    });
    let found = discover_remote_agent(
        &format!("{FAKE_SESSION_ID}@workstation"),
        &discovery_deps(&runner),
    )
    .await
    .unwrap();
    assert_eq!(found.agent.session_id.as_deref(), Some(FAKE_SESSION_ID));
}

/// `explicit machine label restricts discovery to the unique known machine` (`:105-121`): the
/// match is case-insensitive in both halves, the catalog's spelling wins, and the argv is exactly
/// the two forwarding calls.
#[tokio::test]
async fn an_explicit_label_selects_the_unique_machine_case_insensitively() {
    let runner = FnRunner::new(|_, args| {
        if args.first() == Some(&"machine") {
            ok(THREE_MACHINES)
        } else {
            ok(&agent_list(&[("reviewer", Some(FAKE_SESSION_ID))]))
        }
    });
    let found = discover_remote_agent("REVIEWER@workstation", &discovery_deps(&runner))
        .await
        .unwrap();
    assert_eq!(found.machine.label, "Workstation");
    let calls = runner.calls();
    assert_eq!(
        calls.iter().map(|c| c.args.clone()).collect::<Vec<_>>(),
        vec![
            vec!["machine", "list", "--json"],
            vec!["--machine", "Workstation", "agent", "list"],
        ]
    );
    assert!(calls.iter().all(|c| c.timeout == Some(DISCOVERY_TIMEOUT)));
}

/// `unknown and disabled machine labels fail before remote agent listing` (`:123-134`).
#[tokio::test]
async fn unknown_and_disabled_labels_fail_before_any_agent_listing() {
    for target in ["reviewer@unknown", "reviewer@disabled"] {
        let runner = FnRunner::new(|_, args| {
            if args.first() == Some(&"machine") {
                ok(THREE_MACHINES)
            } else {
                ok(&agent_list(&[("reviewer", None)]))
            }
        });
        let error = discover_remote_agent(target, &discovery_deps(&runner))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("is unknown or disabled"),
            "{target}"
        );
        assert_eq!(
            runner.calls().len(),
            1,
            "only `machine list` ran for {target}"
        );
    }
}

/// `malformed or non-explicit addresses fail closed without invoking Herdr` (`:136-155`) —
/// upstream's whole corpus, with JS's whitespace set (U+FEFF counts, U+0085 does not).
#[tokio::test]
async fn malformed_addresses_fail_closed_without_invoking_herdr() {
    for target in [
        "reviewer",
        "@workstation",
        "reviewer@",
        "reviewer@@workstation",
        " reviewer@workstation",
        "reviewer@workstation ",
        "review er@workstation",
        "reviewer@work\tstation",
        "reviewer@work\u{feff}station",
        "\u{a0}reviewer@workstation",
    ] {
        let runner = FnRunner::new(|_, _| ok(THREE_MACHINES));
        let error = discover_remote_agent(target, &discovery_deps(&runner))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("expected name@machine or full-session-uuid@machine"),
            "{target:?}"
        );
        assert!(runner.calls().is_empty(), "{target:?}");
    }
    // U+0085 is Unicode `White_Space` but NOT a JS `\s`, so `/\s/.test` passes it.
    assert!(parse_cross_machine_target("review\u{85}er@workstation").is_ok());
}

/// `duplicate exact agent names on the selected machine are ambiguous` (`:157-166`).
#[tokio::test]
async fn duplicate_agent_names_that_differ_only_in_case_are_ambiguous() {
    let runner = FnRunner::new(|_, args| {
        if args.first() == Some(&"machine") {
            ok(THREE_MACHINES)
        } else {
            ok(&agent_list(&[("reviewer", None), ("Reviewer", None)]))
        }
    });
    let error = discover_remote_agent("reviewer@workstation", &discovery_deps(&runner))
        .await
        .unwrap_err();
    assert!(matches!(error, DiscoveryError::AmbiguousAgent { .. }));
    assert!(error.to_string().contains("target is ambiguous"));
}

/// `full session UUID selects the exact remote agent` (`:168-184`): an agent whose NAME is the
/// UUID of another's session does not shadow the one whose session id it is.
#[tokio::test]
async fn a_full_session_uuid_matches_session_ids_not_names() {
    let other = "00000000-0000-4000-8000-000000000002";
    let runner = FnRunner::new(move |_, args| {
        if args.first() == Some(&"machine") {
            ok(THREE_MACHINES)
        } else {
            ok(&agent_list(&[
                (FAKE_SESSION_ID, Some(other)),
                ("reviewer", Some(FAKE_SESSION_ID)),
            ]))
        }
    });
    let found = discover_remote_agent(
        &format!("{FAKE_SESSION_ID}@workstation"),
        &discovery_deps(&runner),
    )
    .await
    .unwrap();
    assert_eq!(found.agent.name, "reviewer");
    assert_eq!(found.agent.session_id.as_deref(), Some(FAKE_SESSION_ID));
}

/// `timeout and command failures clearly identify the selected machine as unreachable`
/// (`:186-200`).
#[tokio::test]
async fn timeouts_and_command_failures_identify_the_machine_as_unreachable() {
    for (failure, detail) in [
        (
            CommandResult {
                stdout: String::new(),
                stderr: String::new(),
                code: 124,
                timed_out: true,
            },
            "timed out",
        ),
        (failed("connection refused", 255), "connection refused"),
    ] {
        let runner = FnRunner::new(move |_, args| {
            if args.first() == Some(&"machine") {
                ok(THREE_MACHINES)
            } else {
                failure.clone()
            }
        });
        let deps = DiscoveryDeps {
            run: &runner,
            herdr_bin: "herdr",
            discovery_timeout: Duration::from_millis(123),
        };
        let error = discover_remote_agent("reviewer@workstation", &deps)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Saved Herdr machine \"Workstation\" is unreachable: {detail}.")
        );
        assert!(
            runner
                .calls()
                .iter()
                .all(|c| c.timeout == Some(Duration::from_millis(123)))
        );
    }
}

/// `process.env.HERDR_BIN_PATH ?? "herdr"` (`v0.16.0 cross-machine-transport.ts:79`) — and a blank
/// value is an env var that was unset badly, not a request to exec the empty string.
#[test]
fn the_herdr_binary_comes_from_herdr_bin_path_only() {
    assert_eq!(herdr_bin_from(|_| None), "herdr");
    assert_eq!(herdr_bin_from(|_| Some("  ".to_string())), "herdr");
    assert_eq!(
        herdr_bin_from(|key| (key == HERDR_BIN_PATH).then(|| "/opt/herdr".to_string())),
        "/opt/herdr"
    );
    assert_eq!(
        herdr_bin_from(|key| (key == "HERDR_BIN").then(|| "/opt/other".to_string())),
        "herdr",
        "upstream reads HERDR_BIN_PATH here and does NOT fall back to HERDR_BIN"
    );
}
