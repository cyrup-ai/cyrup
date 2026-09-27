//! `cyrup-intercom-cli` — the scripting client for the local intercom broker, a port of
//! pi-intercom's `cli.ts` (`v0.14.0`, `41dc8f6`, #131).
//!
//! ```text
//! cyrup-intercom-cli list [--json]
//! cyrup-intercom-cli send --to worker --text "build failed" [--name <bridge-name>] [--json]
//! cyrup-intercom-cli ask --to worker --text "status?" [--timeout-ms N] [--name <bridge-name>] [--json]
//! ```
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
//! * The program is a compiled binary, not a `tsx` script, so the usage line names
//!   `cyrup-intercom-cli` where upstream's names `cli.ts`, and the default session name / registered
//!   `model` is `cyrup-intercom-cli` where upstream's is `pi-intercom-cli`. The connect-failure
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

// The crate-root `#![deny(...)]` in `lib.rs` governs the LIBRARY root only; a bin target is its own
// crate root and inherits nothing from it. Restated here for the same reason as in
// `cyrup-intercom-broker.rs`.
#![deny(clippy::unreachable, clippy::todo, clippy::unimplemented)]

use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use cyrup_intercom::paths::agent_dir_path;
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient, SendOptions};
use cyrup_intercom::transport::protocol::{SessionInfo, SessionRegistration, now_ms};
use cyrup_intercom::transport::target::broker_connect_target;
use tokio::sync::broadcast::error::RecvError;

/// `CLI_USAGE` (`cli.ts:26-27`).
const CLI_USAGE: &str =
    "usage: cyrup-intercom-cli <list|send|ask> [--to <name|session-id>] [--text <message>]
                          [--timeout-ms <n>] [--name <session-name>] [--json]";

/// `DEFAULT_ASK_TIMEOUT_MS` (`cli.ts:29`).
const DEFAULT_ASK_TIMEOUT_MS: u64 = 120_000;

/// The CLI's default session name and its registered `model` (`"pi-intercom-cli"`,
/// `cli.ts:49,116`; [CYRUP-DELTA] renamed with the binary).
const CLI_SESSION_NAME: &str = "cyrup-intercom-cli";

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
}

impl Command {
    fn as_str(self) -> &'static str {
        match self {
            Command::List => "list",
            Command::Send => "send",
            Command::Ask => "ask",
        }
    }
}

/// `CliOptions` (`cli.ts:31-38`).
#[derive(Debug, PartialEq, Eq)]
struct CliOptions {
    command: Command,
    to: Option<String>,
    text: Option<String>,
    timeout_ms: u64,
    name: String,
    json: bool,
}

/// `parseCliArgs` (`cli.ts:42-96`). The `Err` is the `CliUsageError` message.
fn parse_cli_args(argv: &[String]) -> Result<CliOptions, String> {
    let (command, rest) = match argv.split_first() {
        Some((first, rest)) => (first.as_str(), rest),
        // `String(command)` of the missing first element.
        None => ("undefined", &[][..]),
    };
    let command = match command {
        "list" => Command::List,
        "send" => Command::Send,
        "ask" => Command::Ask,
        other => return Err(format!("unknown command: {other}\n{CLI_USAGE}")),
    };
    let mut opts = CliOptions {
        command,
        to: None,
        text: None,
        timeout_ms: DEFAULT_ASK_TIMEOUT_MS,
        name: CLI_SESSION_NAME.to_string(),
        json: false,
    };

    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        if arg == "--json" {
            opts.json = true;
            continue;
        }
        let Some(value) = args.next() else {
            return Err(format!("missing value for {arg}\n{CLI_USAGE}"));
        };
        match arg.as_str() {
            "--to" => opts.to = Some(value.clone()),
            "--text" => opts.text = Some(value.clone()),
            "--name" => opts.name = value.clone(),
            "--timeout-ms" => opts.timeout_ms = parse_timeout_ms(value)?,
            _ => return Err(format!("unknown option: {arg}\n{CLI_USAGE}")),
        }
    }

    if opts.command != Command::List {
        // `!opts.to` / `!opts.text`: an empty string is as missing as an absent one.
        if opts.to.as_deref().is_none_or(str::is_empty) {
            return Err(format!(
                "--to is required for {}\n{CLI_USAGE}",
                opts.command.as_str()
            ));
        }
        if opts.text.as_deref().is_none_or(str::is_empty) {
            return Err(format!(
                "--text is required for {}\n{CLI_USAGE}",
                opts.command.as_str()
            ));
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

/// `buildCliRegistration` (`cli.ts:113-123`): the process's own cwd and pid, `model` fixed to the
/// CLI's name, `status: "idle"`.
fn build_cli_registration(name: &str) -> SessionRegistration {
    let now = now_ms();
    SessionRegistration {
        name: Some(name.to_string()),
        runtime_fallback_alias: None,
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

/// `runCli` (`cli.ts:135-248`), returning the process exit code.
async fn run_cli(argv: &[String], io: &Streams) -> u8 {
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

    let opts = match parse_cli_args(argv) {
        Ok(opts) => opts,
        Err(message) => return report_failure(&message, 1, None),
    };

    let connected = match broker_connect_target(&agent_dir_path()) {
        Ok(target) => {
            IntercomClient::connect_target(&target, build_cli_registration(&opts.name), None).await
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

    let code = match run_command(&client, &opts, io).await {
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

/// The connected half of `runCli` (`cli.ts:160-244`).
async fn run_command(
    client: &IntercomClient,
    opts: &CliOptions,
    io: &Streams,
) -> Result<(), Failure> {
    let to = opts.to.as_deref().unwrap_or_default();
    let text = opts.text.clone().unwrap_or_default();
    let request_failed = |e: cyrup_intercom::IntercomError| -> Failure {
        (format!("intercom request failed: {e}"), 1, None)
    };

    match opts.command {
        Command::List => {
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
        Command::Send => {
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
        Command::Ask => match ask(client, to, text, opts.timeout_ms).await {
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

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    // `args()` panics on a non-UTF-8 argument; Node hands such bytes over lossily decoded.
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let io = Streams {
        out: std::io::stdout(),
        err: std::io::stderr(),
    };
    ExitCode::from(run_cli(&argv, &io).await)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// `parseCliArgs accepts list with defaults` (`cli.test.ts`).
    #[test]
    fn parse_accepts_list_with_defaults() {
        let opts = parse_cli_args(&args(&["list"])).unwrap();
        assert_eq!(opts.command, Command::List);
        assert!(!opts.json);
        assert_eq!(opts.timeout_ms, DEFAULT_ASK_TIMEOUT_MS);
        assert_eq!(opts.name, "cyrup-intercom-cli");
    }

    /// `parseCliArgs parses send options`.
    #[test]
    fn parse_send_options() {
        let opts = parse_cli_args(&args(&[
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
        let opts = parse_cli_args(&args(&[
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
        let err = |a: &[&str]| parse_cli_args(&args(a)).unwrap_err();
        assert!(err(&[]).starts_with("unknown command: undefined\n"));
        assert!(err(&["teleport"]).starts_with("unknown command: teleport\n"));
        assert!(
            err(&["send", "--carrier-pigeon", "x"])
                .starts_with("unknown option: --carrier-pigeon\n")
        );
        assert!(err(&["send", "--to"]).starts_with("missing value for --to\n"));
        assert!(err(&["send"]).ends_with(CLI_USAGE));
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
            let err = parse_cli_args(&args(&[
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
        let err = |a: &[&str]| parse_cli_args(&args(a)).unwrap_err();
        assert!(err(&["send", "--text", "hi"]).starts_with("--to is required for send\n"));
        assert!(err(&["send", "--to", "w"]).starts_with("--text is required for send\n"));
        assert!(err(&["ask", "--to", "w"]).starts_with("--text is required for ask\n"));
        assert!(err(&["ask", "--to", "", "--text", "?"]).starts_with("--to is required for ask\n"));
        assert!(parse_cli_args(&args(&["list", "--to", "w"])).is_ok());
    }

    /// `buildCliRegistration fills required session fields`.
    #[test]
    fn registration_fills_required_session_fields() {
        let registration = build_cli_registration("bridge");
        assert_eq!(registration.name.as_deref(), Some("bridge"));
        assert_eq!(registration.model, "cyrup-intercom-cli");
        assert_eq!(registration.status.as_deref(), Some("idle"));
        assert_eq!(registration.started_at, registration.last_activity);
        assert_eq!(
            registration.pid,
            serde_json::Number::from(std::process::id())
        );
    }
}
