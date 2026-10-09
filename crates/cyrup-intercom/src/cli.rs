//! The scripting client for the local intercom broker, a port of pi-intercom's `cli.ts`
//! (`v0.14.0`, `41dc8f6`, #131). Two entry points run it: the `cyrup-intercom-cli` binary
//! (`src/bin/cyrup-intercom-cli.rs`) and `cyrup intercom …`, the subcommand of the `cyrup` binary
//! every install has — which is what the default `crossMachine.remoteCommand`
//! ([`crate::config::DEFAULT_REMOTE_COMMAND`]) runs on the remote host, so a cross-machine relay
//! needs nothing installed beyond `cyrup` itself.
//!
//! ```text
//! cyrup-intercom-cli list [--json]
//! cyrup-intercom-cli send --to worker --text "build failed" [--name <bridge-name>] [--json]
//! cyrup-intercom-cli send --to reviewer@workstation --text "ship it" [--name <bridge-name>] [--json]
//! cyrup-intercom-cli ask --to worker --text "status?" [--timeout-ms N] [--name <bridge-name>] [--json]
//! cyrup-intercom-cli relay --envelope-stdin [--json]
//! ```
//!
//! `send --to name@machine` is the explicit cross-machine SSH relay's sending half
//! (`v0.16.0 cli.ts:232-244`): the target is resolved through Herdr's saved machines and the
//! message is carried over `ssh` to the remote host's `relay` subcommand. `relay` is that
//! subcommand — the RECEIVING half (`cli.ts:181-230`): it reads one envelope from stdin, refuses
//! anything [`parse_relay_envelope`] refuses, and delivers `[Unverified cross-machine origin]` plus
//! the body to a LOCAL session with the SSH-asserted origin attached as `crossMachine` provenance.
//! `relay` is deliberately absent from the usage text (`cli.test.ts`: `doesNotMatch(CLI_USAGE,
//! /relay/)`): it is the protocol the sender's `ssh` speaks, not a command for people.
//!
//! The CLI registers as a regular session (`cli.ts:10-11`), so it shows up in the roster and replies
//! can be routed back to it while it stays connected (`ask`). It never starts a broker: it dials the
//! one a running cyrup session already owns, through the same [`IntercomClient::connect_target`]
//! every session uses — which is also what applies `CYRUP_INTERCOM_SCOPE_ID` to the registration.
//!
//! Exit codes (`cli.ts:13`): 0 ok | 1 usage, connection, or delivery failure | 2 ask timeout.
//!
//! Because it talks to the same-machine broker only, it can also be run over ssh on a remote machine
//! to bridge coordination without opening any network listener (`cli.ts:15-19`):
//!
//! ```text
//! ssh myserver 'cyrup-intercom-cli list'
//! ```
//!
//! # [CYRUP-DELTA]s
//!
//! * The program is a compiled binary, not a `tsx` script, so the usage line names the entry point
//!   as invoked — `cyrup intercom` or `cyrup-intercom-cli` — where upstream's names `cli.ts`, and
//!   the default session name / registered `model` is `cyrup-intercom-cli` (from either entry
//!   point) where upstream's is `pi-intercom-cli`. The connect-failure
//!   hint asks about "a cyrup session with cyrup-intercom loaded" — the crate-wide `pi` → `cyrup`
//!   rename the tool descriptions already carry.
//! * A `send` whose connection drops before the broker acks it reports
//!   `intercom request failed: …` exactly as upstream's rejected promise does. This crate's
//!   [`IntercomClient::send`] resolves that case to an `outcome_known: false` result instead of
//!   rejecting, so the CLI maps it back rather than calling it a `delivery failed`.
//! * `--timeout-ms` above `2^31 - 1` waits as long as it says. Node clamps such a `setTimeout` to
//!   1 ms (with a `TimeoutOverflowWarning`), so upstream's `ask --timeout-ms 3000000000` times out
//!   at once; the parser accepts the same values upstream's does, only the runtime quirk is not
//!   reproduced.

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use crate::config::{CrossMachineConfig, load_config};
use crate::cross_machine::{
    CrossMachineDeps, SpawnRunner, ValidatedRelayEnvelope, herdr_bin_from,
    parse_cross_machine_target, read_relay_envelope, relay_message, relay_sender_name,
    resolve_origin, send_cross_machine,
};
use crate::paths::{agent_dir_path, intercom_dir_path};
use crate::transport::client::{InboundEvent, IntercomClient, SendOptions};
use crate::transport::protocol::{SessionInfo, SessionRegistration, now_ms};
use crate::transport::target::broker_connect_target;
use tokio::sync::broadcast::error::RecvError;

/// The program name the standalone binary (`src/bin/cyrup-intercom-cli.rs`) shows in its usage.
pub const BIN_PROGRAM: &str = "cyrup-intercom-cli";

/// The program name `cyrup intercom …` shows in its usage — the documented entry point, and what
/// the default `crossMachine.remoteCommand` ([`crate::config::DEFAULT_REMOTE_COMMAND`]) runs.
pub const SUBCOMMAND_PROGRAM: &str = "cyrup intercom";

/// `CLI_USAGE` (`cli.ts:26-27`), naming `program` — the entry point as invoked — where upstream's
/// names its script. The second line is indented under the first option, as upstream's is.
fn cli_usage(program: &str) -> String {
    let indent = " ".repeat("usage: ".len() + program.len() + 1);
    format!(
        "usage: {program} <list|send|ask> [--to <name|session-id>] [--text <message>]\n\
         {indent}[--timeout-ms <n>] [--name <session-name>] [--json]"
    )
}

/// `DEFAULT_ASK_TIMEOUT_MS` (`cli.ts:29`).
const DEFAULT_ASK_TIMEOUT_MS: u64 = 120_000;

/// The CLI's default session name and its registered `model` (`"pi-intercom-cli"`,
/// `cli.ts:49,116`; [CYRUP-DELTA] renamed with the binary). Fixed whichever entry point runs the
/// CLI, so a roster row means the same thing from either.
const CLI_SESSION_NAME: &str = BIN_PROGRAM;

/// `Number.MAX_SAFE_INTEGER` — the ceiling of `Number.isSafeInteger` (`cli.ts:76`).
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Exit code for an `ask` that got no reply in time (`cli.ts:13`).
const EXIT_ASK_TIMEOUT: u8 = 2;

/// How long `await deps.client.disconnect()` (`cli.ts:246`) can take: the client's own forced
/// teardown fires 2 s after `unregister` (`broker/client.ts:563-565`), plus slack for it to land.
const DISCONNECT_WAIT: Duration = Duration::from_millis(2500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    List,
    Send,
    Ask,
    Relay,
}

impl Command {
    fn as_str(self) -> &'static str {
        match self {
            Command::List => "list",
            Command::Send => "send",
            Command::Ask => "ask",
            Command::Relay => "relay",
        }
    }
}

/// `CliOptions` (`cli.ts:41-49`).
#[derive(Debug, PartialEq, Eq)]
struct CliOptions {
    command: Command,
    to: Option<String>,
    text: Option<String>,
    envelope_stdin: bool,
    timeout_ms: u64,
    name: String,
    json: bool,
}

/// The `relay` usage refusal, raised both before and after the option loop (`cli.ts:74,111`).
fn relay_usage(usage: &str) -> String {
    format!("relay requires only --envelope-stdin (and optional --json)\n{usage}")
}

/// `parseCliArgs` (`cli.ts:53-122`). The `Err` is the `CliUsageError` message, whose usage text
/// names `program`.
fn parse_cli_args(argv: &[String], program: &str) -> Result<CliOptions, String> {
    let usage = cli_usage(program);
    let (command, rest) = match argv.split_first() {
        Some((first, rest)) => (first.as_str(), rest),
        // `String(command)` of the missing first element.
        None => ("undefined", &[][..]),
    };
    let command = match command {
        "list" => Command::List,
        "send" => Command::Send,
        "ask" => Command::Ask,
        "relay" => Command::Relay,
        other => return Err(format!("unknown command: {other}\n{usage}")),
    };
    let mut opts = CliOptions {
        command,
        to: None,
        text: None,
        envelope_stdin: false,
        timeout_ms: DEFAULT_ASK_TIMEOUT_MS,
        name: CLI_SESSION_NAME.to_string(),
        json: false,
    };

    // `relay` takes `--envelope-stdin` exactly once, `--json` at most once, and nothing else at
    // all (`cli.ts:70-75`) — judged BEFORE the option loop, so `--text ""` is a usage error and not
    // a missing value.
    if command == Command::Relay
        && (rest.iter().filter(|a| *a == "--envelope-stdin").count() != 1
            || rest.iter().filter(|a| *a == "--json").count() > 1
            || rest
                .iter()
                .any(|a| a != "--envelope-stdin" && a != "--json"))
    {
        return Err(relay_usage(&usage));
    }

    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        if arg == "--json" {
            opts.json = true;
            continue;
        }
        if arg == "--envelope-stdin" {
            opts.envelope_stdin = true;
            continue;
        }
        let Some(value) = args.next() else {
            return Err(format!("missing value for {arg}\n{usage}"));
        };
        match arg.as_str() {
            "--to" => opts.to = Some(value.clone()),
            "--text" => opts.text = Some(value.clone()),
            "--name" => opts.name = value.clone(),
            "--timeout-ms" => opts.timeout_ms = parse_timeout_ms(value)?,
            _ => return Err(format!("unknown option: {arg}\n{usage}")),
        }
    }

    if opts.command == Command::Relay {
        // `!opts.envelopeStdin || opts.to || opts.text || opts.name !== "pi-intercom-cli"`
        // (`cli.ts:110`): unreachable after the pre-check above, kept as upstream keeps it.
        if !opts.envelope_stdin
            || opts.to.as_deref().is_some_and(|to| !to.is_empty())
            || opts.text.as_deref().is_some_and(|text| !text.is_empty())
            || opts.name != CLI_SESSION_NAME
        {
            return Err(relay_usage(&usage));
        }
    } else {
        if opts.envelope_stdin {
            return Err(format!("--envelope-stdin is only valid for relay\n{usage}"));
        }
        if opts.command != Command::List {
            // `!opts.to` / `!opts.text`: an empty string is as missing as an absent one.
            if opts.to.as_deref().is_none_or(str::is_empty) {
                return Err(format!(
                    "--to is required for {}\n{usage}",
                    opts.command.as_str()
                ));
            }
            if opts.text.as_deref().is_none_or(str::is_empty) {
                return Err(format!(
                    "--text is required for {}\n{usage}",
                    opts.command.as_str()
                ));
            }
        }
    }

    Ok(opts)
}

/// `--timeout-ms` must match `^[0-9]+$` and be a positive safe integer (`cli.ts:74-79`). A digit
/// run too long for `u64` is past `MAX_SAFE_INTEGER` anyway.
fn parse_timeout_ms(value: &str) -> Result<u64, String> {
    let parsed = if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        value.parse::<u64>().ok()
    } else {
        None
    };
    match parsed {
        Some(ms) if ms > 0 && ms <= MAX_SAFE_INTEGER => Ok(ms),
        _ => Err(format!("invalid --timeout-ms value: {value}")),
    }
}

/// `buildCliRegistration` (`cli.ts:143-154`): the process's own cwd and pid, `model` fixed to the
/// CLI's name, `status: "idle"`.
///
/// `runtime_fallback_alias` is set for the relay's transient `name@machine` session (`cli.ts:195`,
/// `Boolean(relayEnvelope)`), so it never wins a name lookup: `worker@laptop` is a label for an
/// asserted origin, not a peer anyone should be able to address or reply to.
fn build_cli_registration(name: &str, runtime_fallback_alias: bool) -> SessionRegistration {
    let now = now_ms();
    SessionRegistration {
        name: Some(name.to_string()),
        runtime_fallback_alias: runtime_fallback_alias.then_some(true),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        model: CLI_SESSION_NAME.to_string(),
        pid: std::process::id().into(),
        started_at: now.into(),
        last_activity: now.into(),
        status: Some("idle".to_string()),
        tmux_pane: None,
        herdr_pane_id: None,
        herdr_session_path: None,
        extra: Default::default(),
    }
}

/// `sessionRow` (`cli.ts:125-133`).
fn session_row(session: &SessionInfo) -> serde_json::Value {
    serde_json::json!({
        "name": session.name.as_deref().unwrap_or("(unnamed)"),
        "id": session.id,
        "model": session.model,
        "status": session.status.as_deref().unwrap_or("?"),
        "cwd": session.cwd,
    })
}

/// How an `ask` settled (`AskOutcome`, `cli.ts:188-191`).
enum AskOutcome {
    Timeout,
    DeliveryFailure(String),
    /// `from` is already the label the JSON reply prints (`reply.from.name ?? reply.from.id`,
    /// `cli.ts:238`), the only part of the sender's `SessionInfo` the CLI reads.
    Reply {
        from: String,
        text: String,
    },
}

/// The CLI's two output streams. Writes are best-effort, like `process.stdout.write`: a closed pipe
/// must not turn a delivered message into a panic.
struct Streams {
    out: std::io::Stdout,
    err: std::io::Stderr,
}

impl Streams {
    fn out(&self, text: &str) {
        let mut out = self.out.lock();
        let _ = out.write_all(text.as_bytes());
        let _ = out.flush();
    }

    fn err(&self, text: &str) {
        let mut err = self.err.lock();
        let _ = err.write_all(text.as_bytes());
        let _ = err.flush();
    }
}

/// What the pre-connect checks of `runCli` decided to do (`cli.ts:177-191`).
///
/// Everything that can be refused WITHOUT the broker is refused here, so a bad relay envelope or a
/// malformed `name@machine` never registers a session; and the variants carry exactly what their
/// command needs, so a `Relay` without a validated envelope, or a cross-machine `Send` without the
/// `crossMachine` config, cannot be built.
enum Plan {
    List,
    /// A local `send`.
    Send,
    /// `send --to name@machine`: carried over SSH, never to the local broker (`cli.ts:233-244`).
    SendCrossMachine(CrossMachineConfig),
    Ask,
    /// `relay --envelope-stdin`: an envelope that already passed [`read_relay_envelope`].
    Relay(ValidatedRelayEnvelope),
}

/// `await readProcessStdin()` then `parseRelayEnvelope` (`cli.ts:182`), on a blocking thread
/// because a pipe read is.
async fn read_relay_stdin() -> Result<ValidatedRelayEnvelope, String> {
    tokio::task::spawn_blocking(|| read_relay_envelope(std::io::stdin().lock()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// The pre-connect half of `runCli` (`cli.ts:179-191`). The `Err` is the message `reportFailure`
/// prints.
async fn plan(opts: &CliOptions) -> Result<Plan, String> {
    let to = opts.to.as_deref().unwrap_or_default();
    match opts.command {
        Command::List => Ok(Plan::List),
        Command::Relay => {
            let envelope = read_relay_stdin().await?;
            // No relay-of-a-relay (`cli.ts:183`): the remote half of a target is the sender's
            // business, never the relay's.
            if envelope.target().as_str().contains('@') {
                return Err("relay target must be a local name or session id".to_string());
            }
            Ok(Plan::Relay(envelope))
        }
        Command::Ask if to.contains('@') => {
            Err("ask only supports local names or session ids".to_string())
        }
        Command::Ask => Ok(Plan::Ask),
        Command::Send if to.contains('@') => {
            // `parseCrossMachineTarget(opts.to)` (`cli.ts:187`) — the same corpus discovery uses,
            // judged before any registration.
            parse_cross_machine_target(to).map_err(|e| e.to_string())?;
            // `runMain`'s `loadConfig()` (`cli.ts:328`), but only where the config is read: the
            // origin's machine name and the remote command. A malformed file is refused with its
            // path, as everywhere else this crate loads it.
            let config = load_config(&intercom_dir_path(&agent_dir_path()))?;
            Ok(Plan::SendCrossMachine(config.cross_machine))
        }
        Command::Send => Ok(Plan::Send),
    }
}

/// `runCli` (`cli.ts:166-318`), returning the process exit code.
async fn run_cli(argv: &[String], program: &str, io: &Streams) -> u8 {
    // Keyed on the RAW argv, not the parsed options, so a usage error still answers in JSON
    // (`argv.includes("--json")`, `cli.ts:139`).
    let json_failures = argv.iter().any(|a| a == "--json");
    let report_failure = |message: &str, code: u8, reason: Option<&str>| -> u8 {
        if json_failures {
            let mut body = serde_json::json!({ "ok": false, "error": message });
            if let (Some(reason), Some(map)) = (reason, body.as_object_mut()) {
                map.insert("reason".to_string(), reason.into());
            }
            io.out(&format!("{body}\n"));
        } else {
            io.err(&format!("{message}\n"));
        }
        code
    };

    let opts = match parse_cli_args(argv, program) {
        Ok(opts) => opts,
        Err(message) => return report_failure(&message, 1, None),
    };

    let plan = match plan(&opts).await {
        Ok(plan) => plan,
        Err(message) => return report_failure(&message, 1, None),
    };

    // A relay registers under the ASSERTED origin, `name@machine`, as a runtime fallback alias
    // (`cli.ts:193-195`); everything else under `--name`.
    let (registration_name, is_relay) = match &plan {
        Plan::Relay(envelope) => (envelope.sender_name(), true),
        _ => (opts.name.clone(), false),
    };
    let connected = match broker_connect_target(&agent_dir_path()) {
        Ok(target) => {
            IntercomClient::connect_target(
                &target,
                build_cli_registration(&registration_name, is_relay),
                None,
            )
            .await
        }
        Err(e) => Err(e),
    };
    let client = match connected {
        Ok(client) => client,
        Err(e) => {
            return report_failure(
                &format!(
                    "cannot reach the local intercom broker: {e}\nis a cyrup session with \
                     cyrup-intercom loaded currently running on this machine?"
                ),
                1,
                None,
            );
        }
    };

    let code = match run_command(&client, &opts, plan, io).await {
        Ok(()) => 0,
        Err((message, code, reason)) => report_failure(&message, code, reason),
    };
    disconnect(&client).await;
    code
}

/// `await deps.client.disconnect().catch(() => {})` (`cli.ts:246`). [`IntercomClient::disconnect`]
/// only queues `unregister` + close and returns; returning from `main` straight after would drop the
/// runtime with those still unwritten. So wait for the connection's own `Disconnected` — the
/// broker closing its end, or the client's 2 s forced teardown — bounded either way.
async fn disconnect(client: &IntercomClient) {
    let mut events = client.subscribe();
    client.disconnect();
    let closed = async {
        loop {
            match events.recv().await {
                Ok(InboundEvent::Disconnected(_)) | Err(RecvError::Closed) => return,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
            }
        }
    };
    let _ = tokio::time::timeout(DISCONNECT_WAIT, closed).await;
}

/// A failure `runCli` reports: its message, exit code and optional JSON `reason`.
type Failure = (String, u8, Option<&'static str>);

/// The connected half of `runCli` (`cli.ts:200-316`).
async fn run_command(
    client: &IntercomClient,
    opts: &CliOptions,
    plan: Plan,
    io: &Streams,
) -> Result<(), Failure> {
    let to = opts.to.as_deref().unwrap_or_default();
    let text = opts.text.clone().unwrap_or_default();
    let request_failed =
        |e: crate::IntercomError| -> Failure { (format!("intercom request failed: {e}"), 1, None) };

    match plan {
        Plan::List => {
            let sessions = client.list_sessions().await.map_err(request_failed)?;
            if opts.json {
                let rows: Vec<_> = sessions.iter().map(session_row).collect();
                let body = serde_json::json!({ "ok": true, "sessions": rows });
                io.out(&format!("{}\n", pretty(&body)));
            } else {
                for session in &sessions {
                    let id: String = session.id.chars().take(8).collect();
                    io.out(&format!(
                        "{}\t{id}\t{}\t{}\t{}\n",
                        session.name.as_deref().unwrap_or("(unnamed)"),
                        session.model,
                        session.status.as_deref().unwrap_or("?"),
                        session.cwd,
                    ));
                }
            }
            Ok(())
        }
        Plan::Relay(envelope) => {
            // `cli.ts:215-230` — the validated body, the validated target, and the asserted origin
            // as provenance. The recipient gets `[Unverified cross-machine origin]\n` first.
            let result = client
                .send_relayed(
                    envelope.target().as_str(),
                    SendOptions {
                        text: relay_message(&envelope),
                        ..Default::default()
                    },
                    envelope.provenance(),
                )
                .await
                .map_err(request_failed)?;
            if !result.outcome_known {
                let reason = result.reason.unwrap_or_default();
                return Err((format!("intercom request failed: {reason}"), 1, None));
            }
            if !result.delivered {
                return Err((
                    format!(
                        "relay delivery failed: {}",
                        result
                            .reason
                            .unwrap_or_else(|| "unknown reason".to_string())
                    ),
                    1,
                    None,
                ));
            }
            if opts.json {
                // The exact object `sendCrossMachine` parses on the other end of the SSH
                // connection (`cross-machine-transport.ts:96-108`); one compact line.
                let body = serde_json::json!({
                    "ok": true,
                    "delivered": true,
                    "id": result.id,
                    "origin": envelope.sender_name(),
                    "trust": "ssh-asserted",
                });
                io.out(&format!("{body}\n"));
            } else {
                io.out(&format!(
                    "relayed from {} to {} ({})\n",
                    envelope.sender_name(),
                    envelope.target().as_str(),
                    result.id
                ));
            }
            Ok(())
        }
        Plan::SendCrossMachine(config) => {
            // `cli.ts:233-244`. No fallback to a local send on any failure: a `@` target that
            // cannot be relayed is an error, never a silently-local delivery.
            let failed = |e: String| -> Failure {
                (
                    format!("explicit cross-machine delivery failed: {e}"),
                    1,
                    None,
                )
            };
            let sessions = client
                .list_sessions()
                .await
                .map_err(|e| failed(e.to_string()))?;
            let origin = resolve_origin(
                &sessions,
                &opts.name,
                &config.machine_name,
                client.session_id().as_deref(),
                |key| std::env::var(key).ok(),
            );
            let herdr_bin = herdr_bin_from(|key| std::env::var(key).ok());
            let deps = CrossMachineDeps::new(&SpawnRunner, &herdr_bin, &config.remote_command);
            let remote = send_cross_machine(to, &text, origin, &deps)
                .await
                .map_err(|e| failed(e.to_string()))?;
            if opts.json {
                let body = serde_json::json!({
                    "ok": true,
                    "delivered": true,
                    "crossMachine": true,
                    "machine": remote.discovered.machine.label,
                    "target": remote.discovered.agent.name,
                });
                io.out(&format!("{body}\n"));
            } else {
                io.out(&format!(
                    "delivered to {} over SSH\n",
                    relay_sender_name(
                        &remote.discovered.agent.name,
                        &remote.discovered.machine.label
                    )
                ));
            }
            Ok(())
        }
        Plan::Send => {
            let result = client
                .send(
                    to,
                    SendOptions {
                        text,
                        ..Default::default()
                    },
                )
                .await
                .map_err(request_failed)?;
            // The connection dropped with the send in flight: upstream's promise REJECTS here
            // (`onClose`, `broker/client.ts:217`), which lands in the outer `catch` (`cli.ts:243`),
            // not in the `!result.delivered` branch.
            if !result.outcome_known {
                let reason = result.reason.unwrap_or_default();
                return Err((format!("intercom request failed: {reason}"), 1, None));
            }
            if !result.delivered {
                return Err(delivery_failed(result.reason));
            }
            if opts.json {
                let body = serde_json::json!({ "ok": true, "delivered": true, "id": result.id });
                io.out(&format!("{}\n", pretty(&body)));
            } else {
                io.out(&format!("delivered to {to} ({})\n", result.id));
            }
            Ok(())
        }
        Plan::Ask => match ask(client, to, text, opts.timeout_ms).await {
            AskOutcome::Timeout => Err((
                format!(
                    "ask timed out after {} ms waiting for a reply from {to}",
                    opts.timeout_ms
                ),
                EXIT_ASK_TIMEOUT,
                Some("timeout"),
            )),
            AskOutcome::DeliveryFailure(reason) => Err(delivery_failed(Some(reason))),
            AskOutcome::Reply { from, text } => {
                if opts.json {
                    let body = serde_json::json!({
                        "ok": true,
                        "from": from,
                        "text": text,
                    });
                    io.out(&format!("{}\n", pretty(&body)));
                } else {
                    io.out(&format!("{text}\n"));
                }
                Ok(())
            }
        },
    }
}

/// `delivery failed: ${reason ?? "unknown reason"}` (`cli.ts:177,218,235`).
fn delivery_failed(reason: Option<String>) -> Failure {
    let reason = reason.unwrap_or_else(|| "unknown reason".to_string());
    (format!("delivery failed: {reason}"), 1, None)
}

/// `JSON.stringify(value, null, 2)`.
fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// The `ask` wait (`cli.ts:187-229`): send with `expectsReply`, then settle on the first message
/// whose `replyTo` names the sent id, a delivery failure, or the timeout — whichever comes first.
///
/// The timer covers the send as well as the wait, as upstream's is armed before `send()` is
/// called. A reply that lands before `send()` resolves its id is not lost: the subscription is
/// taken first, so it sits in the receiver until the id is known — the role upstream's
/// `earlyReplies` map plays. Replies to other ids, and messages that reply to nothing, are ignored.
async fn ask(client: &IntercomClient, to: &str, text: String, timeout_ms: u64) -> AskOutcome {
    let mut events = client.subscribe();
    let settle = async {
        let options = SendOptions {
            text,
            expects_reply: Some(true),
            ..Default::default()
        };
        let sent_id = match client.send(to, options).await {
            Ok(result) if result.delivered => result.id,
            Ok(result) => {
                return AskOutcome::DeliveryFailure(
                    result
                        .reason
                        .unwrap_or_else(|| "unknown reason".to_string()),
                );
            }
            Err(e) => return AskOutcome::DeliveryFailure(e.to_string()),
        };
        loop {
            match events.recv().await {
                Ok(InboundEvent::Message { from, message })
                    if message.reply_to.as_deref() == Some(sent_id.as_str()) =>
                {
                    return AskOutcome::Reply {
                        from: from.name.unwrap_or(from.id),
                        text: message.content.text,
                    };
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                // Upstream listens on an emitter that simply goes quiet when the connection
                // drops; only the timer settles it then.
                Err(RecvError::Closed) => std::future::pending::<()>().await,
            }
        }
    };
    tokio::time::timeout(Duration::from_millis(timeout_ms), settle)
        .await
        .unwrap_or(AskOutcome::Timeout)
}

/// Run the CLI over `argv` (the arguments AFTER the program / subcommand name) against the
/// process's real stdin / stdout / stderr, and return the exit code (`cli.ts:13`). Both entry points
/// call this: the `cyrup-intercom-cli` binary ([`BIN_PROGRAM`]) and the `cyrup intercom`
/// subcommand ([`SUBCOMMAND_PROGRAM`]); `program` is the name usage errors show.
pub async fn run(argv: &[String], program: &str) -> ExitCode {
    ExitCode::from(run_status(argv, program).await)
}

/// [`run`], returning the exit status as a number — for a host binary (`cyrup intercom`) whose
/// own dispatch reports exit codes as integers.
pub async fn run_status(argv: &[String], program: &str) -> u8 {
    let io = Streams {
        out: std::io::stdout(),
        err: std::io::stderr(),
    };
    run_cli(argv, program, &io).await
}

/// The process's own arguments after `skip` leading ones, decoded lossily. `args()` panics on a
/// non-UTF-8 argument; Node hands such bytes over lossily decoded.
#[must_use]
pub fn process_args(skip: usize) -> Vec<String> {
    std::env::args_os()
        .skip(skip)
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    /// [`parse_cli_args`] as the standalone binary runs it.
    fn parse(argv: &[String]) -> Result<CliOptions, String> {
        parse_cli_args(argv, BIN_PROGRAM)
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// `parseCliArgs accepts list with defaults` (`cli.test.ts`).
    #[test]
    fn parse_accepts_list_with_defaults() {
        let opts = parse(&args(&["list"])).unwrap();
        assert_eq!(opts.command, Command::List);
        assert!(!opts.json);
        assert_eq!(opts.timeout_ms, DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(opts.name, "cyrup-intercom-cli");
    }

    /// `parseCliArgs parses send options`.
    #[test]
    fn parse_send_options() {
        let opts = parse(&args(&[
            "send", "--to", "worker", "--text", "hello", "--name", "bridge", "--json",
        ]))
        .unwrap();
        assert_eq!(opts.command, Command::Send);
        assert_eq!(opts.to.as_deref(), Some("worker"));
        assert_eq!(opts.text.as_deref(), Some("hello"));
        assert_eq!(opts.name, "bridge");
        assert!(opts.json);
    }

    /// `parseCliArgs parses ask timeout`.
    #[test]
    fn parse_ask_timeout() {
        let opts = parse(&args(&[
            "ask",
            "--to",
            "worker",
            "--text",
            "?",
            "--timeout-ms",
            "5000",
        ]))
        .unwrap();
        assert_eq!(opts.timeout_ms, 5000);
    }

    /// `parseCliArgs rejects unknown commands and options`.
    #[test]
    fn parse_rejects_unknown_commands_and_options() {
        let err = |a: &[&str]| parse(&args(a)).unwrap_err();
        assert!(err(&[]).starts_with("unknown command: undefined\n"));
        assert!(err(&["teleport"]).starts_with("unknown command: teleport\n"));
        assert!(
            err(&["send", "--carrier-pigeon", "x"])
                .starts_with("unknown option: --carrier-pigeon\n")
        );
        assert!(err(&["send", "--to"]).starts_with("missing value for --to\n"));
        assert!(err(&["send"]).ends_with(&cli_usage(BIN_PROGRAM)));
    }

    /// `parseCliArgs rejects invalid timeout values`.
    #[test]
    fn parse_rejects_invalid_timeout_values() {
        for value in [
            "0",
            "soon",
            "50garbage",
            "1.5",
            "-1",
            "9007199254740992",
            "",
        ] {
            let err = parse(&args(&[
                "ask",
                "--to",
                "w",
                "--text",
                "?",
                "--timeout-ms",
                value,
            ]))
            .unwrap_err();
            assert_eq!(err, format!("invalid --timeout-ms value: {value}"));
        }
        assert_eq!(parse_timeout_ms("9007199254740991"), Ok(MAX_SAFE_INTEGER));
    }

    /// `parseCliArgs requires --to and --text for send/ask`, plus the falsy empty string.
    #[test]
    fn parse_requires_to_and_text_for_send_and_ask() {
        let err = |a: &[&str]| parse(&args(a)).unwrap_err();
        assert!(err(&["send", "--text", "hi"]).starts_with("--to is required for send\n"));
        assert!(err(&["send", "--to", "w"]).starts_with("--text is required for send\n"));
        assert!(err(&["ask", "--to", "w"]).starts_with("--text is required for ask\n"));
        assert!(err(&["ask", "--to", "", "--text", "?"]).starts_with("--to is required for ask\n"));
        assert!(parse(&args(&["list", "--to", "w"])).is_ok());
        // The unknown option is judged before the missing `--to`, as upstream's loop precedes it.
        assert!(
            parse(&args(&["send", "--to", "w", "--text-stdin", "ignored"]))
                .unwrap_err()
                .starts_with("unknown option: --text-stdin\n")
        );
    }

    /// `parseCliArgs keeps relay hidden and restricted to an stdin envelope` (`cli.test.ts`).
    #[test]
    fn relay_is_hidden_from_usage_and_restricted_to_an_stdin_envelope() {
        let usage = cli_usage(BIN_PROGRAM);
        assert!(!usage.contains("relay"));
        let opts = parse(&args(&["relay", "--envelope-stdin", "--json"])).unwrap();
        assert_eq!(opts.command, Command::Relay);
        assert!(opts.envelope_stdin && opts.json);
        assert!(
            parse(&args(&["relay", "--envelope-stdin"]))
                .unwrap()
                .envelope_stdin
        );
        let expected =
            format!("relay requires only --envelope-stdin (and optional --json)\n{usage}");
        for argv in [
            &["relay"][..],
            &["relay", "--json"],
            &["relay", "--envelope-stdin", "--text", "hello"],
            &["relay", "--envelope-stdin", "--text", ""],
            &["relay", "--envelope-stdin", "--name", "cyrup-intercom-cli"],
            &["relay", "--envelope-stdin", "--to", "reviewer"],
            &["relay", "--envelope-stdin", "--timeout-ms", "5"],
            &["relay", "--envelope-stdin", "--envelope-stdin"],
            &["relay", "--envelope-stdin", "--json", "--json"],
            &["relay", "--envelope-stdin", "extra"],
        ] {
            assert_eq!(parse(&args(argv)).unwrap_err(), expected, "{argv:?}");
        }
    }

    /// The usage text names the entry point as invoked: `cyrup intercom` (the documented one, and
    /// the default `crossMachine.remoteCommand`) or the standalone `cyrup-intercom-cli`. Either
    /// way the second line stays aligned under the first option, as upstream's `CLI_USAGE` is.
    #[test]
    fn usage_names_the_program_as_invoked() {
        let sub = parse_cli_args(&args(&["teleport"]), SUBCOMMAND_PROGRAM).unwrap_err();
        assert_eq!(
            sub,
            "unknown command: teleport\n\
             usage: cyrup intercom <list|send|ask> [--to <name|session-id>] [--text <message>]\n\
             \x20                     [--timeout-ms <n>] [--name <session-name>] [--json]"
        );
        assert!(!sub.contains("cyrup-intercom-cli"), "{sub}");
        assert_eq!(
            cli_usage(BIN_PROGRAM),
            "usage: cyrup-intercom-cli <list|send|ask> [--to <name|session-id>] [--text <message>]\n\
             \x20                         [--timeout-ms <n>] [--name <session-name>] [--json]"
        );
        // The registered session name does not follow the program name.
        assert_eq!(
            parse_cli_args(&args(&["list"]), SUBCOMMAND_PROGRAM)
                .unwrap()
                .name,
            "cyrup-intercom-cli"
        );
    }

    /// `--envelope-stdin` belongs to `relay` alone (`cli.ts:114`), and is judged before the
    /// missing-`--to` rule.
    #[test]
    fn the_stdin_envelope_flag_is_refused_outside_relay() {
        let usage = cli_usage(BIN_PROGRAM);
        for argv in [
            &["list", "--envelope-stdin"][..],
            &["send", "--envelope-stdin"],
            &["ask", "--to", "w", "--text", "?", "--envelope-stdin"],
        ] {
            assert_eq!(
                parse(&args(argv)).unwrap_err(),
                format!("--envelope-stdin is only valid for relay\n{usage}"),
                "{argv:?}"
            );
        }
    }

    /// `buildCliRegistration fills required session fields`, and the relay's alias flag
    /// (`cli.ts:151`: `...(runtimeFallbackAlias ? { runtimeFallbackAlias: true } : {})`).
    #[test]
    fn registration_fills_required_session_fields() {
        let registration = build_cli_registration("bridge", false);
        assert_eq!(registration.name.as_deref(), Some("bridge"));
        assert_eq!(registration.model, "cyrup-intercom-cli");
        assert_eq!(registration.status.as_deref(), Some("idle"));
        assert_eq!(registration.started_at, registration.last_activity);
        assert_eq!(
            registration.pid,
            serde_json::Number::from(std::process::id())
        );
        assert_eq!(registration.runtime_fallback_alias, None);
        assert_eq!(
            build_cli_registration("worker@laptop", true).runtime_fallback_alias,
            Some(true)
        );
    }
}
