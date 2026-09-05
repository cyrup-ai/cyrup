//! The ACP front-end's **session lifecycle** over a live stdio connection.
//!
//! `ACP-057`, `ACP-062`, `ACP-069`, `ACP-072`, `ACP-077`, `ACP-120`, `ACP-121`, `ACP-122`,
//! `ACP-123`, `ACP-153`, `ACP-203`…`ACP-208`, `ACP-214`…`ACP-218`, `ACP-282`, `ACP-284`,
//! `ACP-285`, `ACP-293`.
//!
//! [`super::acp_transport`] covers the process seam with a **one-shot** pipe: write every frame,
//! close stdin, read what came back. That shape cannot express the assertions here, all of which
//! are about what happens *between* two frames — a `session/load` whose replay must be on the wire
//! before its own response, a `session/cancel` that must overtake the `session/prompt` it cancels,
//! a `config_option_update` that arrives unprompted after a setter. So this file holds stdin open
//! and reads incrementally, which is what an editor does.
//!
//! Everything is offline and credential-less. Two consequences shape the tests and are load-bearing
//! rather than incidental:
//!
//! * **No assistant message can ever be produced**, and `SessionManager::create`'s doc is explicit
//!   that the file is *"deferred until the first assistant message"* (pi's `openSync(file,"wx")`
//!   first-flush, `session-manager.ts:926-935`). A session created here therefore never reaches
//!   disk, so `session/list` and `session/load` are exercised against a **seeded** transcript
//!   written by [`seed_session`] — which is also the only way to assert on replay content.
//! * A `session/prompt` still starts and settles a real run, so `ACP-121`'s exactly-once settle and
//!   `ACP-123`'s cancel interleaving are observable; only the assistant text is absent.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

/// How long any single expectation waits. Generous, because a `session/new` builds a real session
/// (settings, resources, extension host) on a cold debug binary.
const WAIT: Duration = Duration::from_secs(45);

/// A live `cyrup --acp` with stdin held open.
struct Acp {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    /// Every frame read so far, in wire order. The ordering assertions read this.
    seen: Vec<Value>,
    _tmp: TempDir,
    project: PathBuf,
    home: PathBuf,
}

impl Acp {
    /// Spawn the binary against a scrubbed, empty `HOME`.
    ///
    /// The env scrub is [`super::acp_transport::run`]'s, for its stated reason: an ambient
    /// `CYRUP_INTERCOM=1` alone satisfies `is_installed()` and detaches an immortal broker per run.
    fn start() -> Self {
        Self::start_with(&[], &[])
    }

    /// A harness whose sessions have a **model**: the scripted offline `faux` provider.
    ///
    /// `Acp::start` is credential-less, so `AgentSession::prompt`'s preflight refuses with
    /// `NoModelSelected` and no run ever begins — which is right for most of this file and useless
    /// for the three assertions that are about what a *run* does. `--model faux/faux-1` reaches
    /// `cyrup_provider::faux::FauxProvider` through `crates/cyrup/src/provider.rs`'s
    /// `#[cfg(feature = "faux")]` arm (the feature `build.rs` requests for exactly this reason), so
    /// a whole turn happens offline, deterministically, with no network and no credential.
    ///
    /// With nothing queued the provider answers `StopReason::Error` /
    /// `"No more faux responses queued"` — see `crates/cyrup-provider/src/faux.rs` — which is not a
    /// limitation here but the instrument: it is a **terminal run failure**, the exact shape
    /// `ACP-022` is about, produced without a provider to fail against.
    fn start_scripted() -> Self {
        Self::start_with(&["--model", "faux/faux-1", "--offline"], &[])
    }

    /// A harness whose session has a **reasoning** model, so the whole thinking ladder exists.
    ///
    /// `faux/faux-1` is a non-reasoning model, so `ACP-062`'s ladder collapses to the one-entry
    /// `[off]` list `ACP-Q12` prescribes and `ACP-072`'s clamp has nothing to clamp; a session
    /// with no model at all does not even advertise the `model` option (`ACP-063`). Selecting a
    /// real model needs only a *present* credential — `has_configured_auth` is a config check, not
    /// a network one — and the two tests that use this never issue a `session/prompt`, so the
    /// placeholder key is never sent anywhere. `--offline` is belt: nothing here may reach the
    /// network.
    ///
    /// This is what the suite was accidentally relying on before: it passed only on a machine
    /// whose environment carried real credentials, and asserted nothing anywhere else.
    fn start_with_reasoning_model() -> Self {
        Self::start_with(
            &["--offline"],
            &[("ANTHROPIC_API_KEY", "cyrup-it-placeholder-never-sent")],
        )
    }

    fn start_with(extra: &[&str], env: &[(&str, &str)]) -> Self {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();

        let mut child = Command::new(crate::support::bins::cyrup())
            .current_dir(&project)
            .env("HOME", &home)
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("TOGETHER_API_KEY")
            .env_remove("GEMINI_API_KEY")
            // …and the ambient AWS ones. A developer box with Bedrock credentials in its
            // environment made `session/prompt` resolve a real model and issue a real HTTPS
            // request from a suite whose own doc says it is offline and credential-less — the
            // observed failure was a live Bedrock 403 arriving where `NoModelSelected` was
            // expected. `AWS_PROFILE`/`AWS_REGION` alone are enough for the provider's auth
            // preflight to pass, so the whole family goes.
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env_remove("AWS_SESSION_TOKEN")
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_REGION")
            .env_remove("AWS_DEFAULT_REGION")
            .env_remove("AWS_BEARER_TOKEN_BEDROCK")
            .env_remove("AWS_CONTAINER_CREDENTIALS_FULL_URI")
            .env_remove("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
            .env_remove("AWS_WEB_IDENTITY_TOKEN_FILE")
            .env_remove("AZURE_API_KEY")
            .env_remove("GROQ_API_KEY")
            .env_remove("MISTRAL_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("XAI_API_KEY")
            .env_remove("GOOGLE_API_KEY")
            .env_remove("GOOGLE_GENERATIVE_AI_API_KEY")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("CYRUP_INTERCOM")
            .env_remove("CYRUP_SUBAGENTS")
            .env_remove("CYRUP_PERMISSION_SYSTEM")
            .arg("--acp")
            .args(extra)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn cyrup --acp");

        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });

        let mut acp = Self {
            stdin: child.stdin.take(),
            child,
            rx,
            seen: Vec::new(),
            _tmp: tmp,
            project,
            home,
        };
        // Every test needs the handshake, and `ClientView` is what gates the auth `_meta` shim and
        // the dialog capabilities, so it is sent once here rather than per test.
        acp.send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {"terminal": true}}
        }));
        acp.answer(1);
        acp
    }

    fn send(&mut self, frame: Value) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        writeln!(stdin, "{frame}").expect("write frame");
        stdin.flush().expect("flush frame");
    }

    /// Read one more frame, or fail the test.
    fn next_frame(&mut self, why: &str) -> Value {
        match self.rx.recv_timeout(WAIT) {
            Ok(line) => {
                let frame: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
                self.seen.push(frame.clone());
                frame
            }
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "timed out waiting for {why}; frames so far:\n{}",
                    self.dump()
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!(
                    "the agent closed stdout while waiting for {why}:\n{}",
                    self.dump()
                )
            }
        }
    }

    /// Read until the response to `id` arrives, and return it.
    fn answer(&mut self, id: u64) -> Value {
        loop {
            let frame = self.next_frame(&format!("the response to id {id}"));
            if frame.get("id").and_then(Value::as_u64) == Some(id) {
                return frame;
            }
        }
    }

    /// Read until an `agent_message_chunk` whose text starts with `prefix` arrives.
    ///
    /// Needed because `session/new`'s follow-ups now include the startup prelude (`ACP-066`), so
    /// the FIRST `agent_message_chunk` of a session is that inventory rather than whatever the
    /// test is about. Matching on the text keeps each assertion pointed at its own chunk.
    fn chunk_starting_with(&mut self, prefix: &str) -> Value {
        loop {
            let update = self.update("agent_message_chunk");
            if update["content"]["text"]
                .as_str()
                .is_some_and(|t| t.starts_with(prefix))
            {
                return update;
            }
        }
    }

    /// The index in `seen` of the `agent_message_chunk` whose text starts with `prefix`.
    fn position_of_chunk(&self, prefix: &str) -> usize {
        self.position(
            &format!("an agent_message_chunk starting {prefix:?}"),
            |f| {
                f["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                    && f["params"]["update"]["content"]["text"]
                        .as_str()
                        .is_some_and(|t| t.starts_with(prefix))
            },
        )
    }

    /// Read until a `session/update` of `kind` arrives, and return its `update` object.
    fn update(&mut self, kind: &str) -> Value {
        loop {
            let frame = self.next_frame(&format!("a `{kind}` session/update"));
            if frame["params"]["update"]["sessionUpdate"] == kind {
                return frame["params"]["update"].clone();
            }
        }
    }

    /// Drain whatever has already arrived, without waiting. Used to settle the tail of a turn
    /// before an ordering assertion so `seen` is complete.
    fn drain(&mut self, grace: Duration) {
        while let Ok(line) = self.rx.recv_timeout(grace) {
            if let Ok(frame) = serde_json::from_str::<Value>(&line) {
                self.seen.push(frame);
            }
        }
    }

    /// The index in `seen` of the first frame matching `pred`.
    fn position(&self, what: &str, pred: impl Fn(&Value) -> bool) -> usize {
        self.seen
            .iter()
            .position(|f| pred(f))
            .unwrap_or_else(|| panic!("no frame matching {what} in:\n{}", self.dump()))
    }

    fn position_of_response(&self, id: u64) -> usize {
        self.position(&format!("the response to id {id}"), |f| {
            f.get("id").and_then(Value::as_u64) == Some(id)
        })
    }

    fn position_of_update(&self, kind: &str) -> usize {
        self.position(&format!("a `{kind}` update"), |f| {
            f["params"]["update"]["sessionUpdate"] == kind
        })
    }

    fn dump(&self) -> String {
        self.seen
            .iter()
            .map(|f| {
                let s = f.to_string();
                if s.len() > 400 {
                    format!("{}…", &s[..400])
                } else {
                    s
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `session/new` for this run's project, returning the minted session id.
    fn new_session(&mut self, id: u64) -> String {
        self.send(json!({
            "jsonrpc": "2.0", "id": id, "method": "session/new",
            "params": {"cwd": self.project, "mcpServers": []}
        }));
        let response = self.answer(id);
        assert!(
            response.get("error").is_none(),
            "session/new failed: {response}"
        );
        response["result"]["sessionId"]
            .as_str()
            .expect("session/new must mint a sessionId")
            .to_owned()
    }

    fn shutdown(mut self) -> String {
        drop(self.stdin.take());
        let status = self.child.wait().expect("wait for cyrup");
        assert_eq!(
            status.code(),
            Some(0),
            "ACP-024: every stdin outcome exits 0.\nframes:\n{}",
            self.dump()
        );
        let mut stderr = String::new();
        if let Some(handle) = self.child.stderr.take() {
            let _ = BufReader::new(handle).read_line(&mut stderr);
        }
        stderr
    }
}

/// `<home>/.cyrup/agent/sessions/--<encoded cwd>--`, i.e. `cyrup_session::layout::encode_cwd`'s
/// default root, written out here because `cyrup-it` deliberately has no `[dependencies]`.
fn seeded_dir(home: &Path, project: &Path) -> PathBuf {
    let raw = project.to_string_lossy();
    let trimmed = raw.strip_prefix('/').unwrap_or(&raw);
    let encoded: String = trimmed
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    home.join(".cyrup/agent/sessions")
        .join(format!("--{encoded}--"))
}

/// Write a complete, loadable transcript: a named session, one user message and one assistant
/// reply.
///
/// **Why this is seeded rather than produced.** `SessionManager::create` defers the file *"until
/// the first assistant message"* (pi's first-flush, `session-manager.ts:926-935`), and this suite
/// is credential-less by design, so no run here can ever produce one. Seeding is the only way to
/// reach `ACP-203`…`ACP-208` and `ACP-214`…`ACP-218` offline — and it is a fair instrument, because
/// the bytes below are exactly what `cyrup_session`'s own writer emits (the shapes are lifted from
/// `crates/cyrup-session/src/tests/deferred_context.rs`).
fn seed_session(home: &Path, project: &Path, id: &str, name: &str) -> PathBuf {
    let dir = seeded_dir(home, project);
    std::fs::create_dir_all(&dir).unwrap();
    let ts = "2026-09-05T07:00:00.000Z";
    let usage = json!({
        "input": 10, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 12,
        "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
    });
    let lines = [
        json!({"type": "session", "version": 3, "id": id, "timestamp": ts, "cwd": project}),
        json!({"type": "session_info", "id": "aaaaaaa0", "parentId": Value::Null,
               "timestamp": ts, "name": name}),
        json!({"type": "message", "id": "aaaaaaa1", "parentId": Value::Null, "timestamp": ts,
               "message": {"role": "user", "content": [{"type": "text", "text": "what is 2+2"}],
                           "timestamp": 1_767_600_000_000u64}}),
        json!({"type": "message", "id": "aaaaaaa2", "parentId": "aaaaaaa1", "timestamp": ts,
               "message": {"role": "assistant", "content": [{"type": "text", "text": "4"}],
                           "api": "anthropic-messages", "provider": "anthropic", "model": "claude",
                           "usage": usage, "stopReason": "stop",
                           "timestamp": 1_767_600_000_000u64}}),
    ];
    let path = dir.join(format!("2026-09-05T07-00-00-000Z_{id}.jsonl"));
    let body = lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, body + "\n").unwrap();
    path
}

const SEEDED_ID: &str = "01a07000-0000-7000-8000-0000000000aa";

// ------------------------------------------------------------------------------------------
// session/new: the advertised surface, and the command menu that follows the response
// ------------------------------------------------------------------------------------------

/// **ACP-062 / ACP-064 / ACP-069 / ACP-293** — `session/new` answers with the mode list and the
/// config options from one view read, and the command menu arrives **after** the response.
///
/// The ordering is upstream's `setTimeout(fn, 0)` and its stated reason, in the upstream author's
/// own words: *"some clients (e.g. Zed) will ignore notifications for an unknown sessionId. So we
/// must send this after the session/new response has been delivered."* `crate::HandlerOutcome` is
/// what makes that structural, and this is the assertion that it held on the wire.
///
/// The command set is also the `ACP-069`/`ACP-293` assertion that `available_commands` — not
/// `merge_commands(Vec::new())` — is what both handlers call: the seven built-ins plus whatever the
/// project catalog contributes, never the built-ins alone by accident.
#[test]
fn session_new_advertises_its_surface_and_then_its_commands() {
    // `ACP-063`'s `model` option is only advertised for a session with a non-empty catalog, and
    // `ACP-062`'s ladder is model-derived. This test used to pass only on a developer box whose
    // environment happened to carry real credentials, which is why the harness now scrubs those
    // and this asks for a model explicitly.
    let mut acp = Acp::start_with_reasoning_model();
    let session = acp.new_session(2);
    let response = acp.seen[acp.position_of_response(2)].clone();
    let result = &response["result"];

    // ACP-062 — the mode list is model-derived and its current value is the session's.
    let modes = &result["modes"];
    let ids: Vec<&str> = modes["availableModes"]
        .as_array()
        .expect("availableModes")
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"off") && ids.contains(&"medium"),
        "ACP-062: the thinking ladder must be advertised: {modes}"
    );
    assert!(
        modes["currentModeId"].is_string(),
        "ACP-062: currentModeId must be the session's own level: {modes}"
    );

    // ACP-064 — two options, `model` then `thought_level`, in that order.
    let options = result["configOptions"].as_array().expect("configOptions");
    let option_ids: Vec<&str> = options.iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert_eq!(
        option_ids,
        vec!["model", "thought_level"],
        "ACP-064: the option order is load-bearing and is not enforced by the type"
    );
    // ACP-063 — `currentValue` is a MEMBER of `options`, never a `'default'` sentinel.
    let thinking = &options[1];
    let values: Vec<&str> = thinking["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_str().unwrap())
        .collect();
    assert!(
        values.contains(&thinking["currentValue"].as_str().unwrap()),
        "ACP-063: currentValue must be a member of options: {thinking}"
    );

    // ACP-069 / ACP-293 — the menu, AFTER the response.
    let commands = acp.update("available_commands_update");
    let names: Vec<&str> = commands["availableCommands"]
        .as_array()
        .expect("availableCommands")
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for builtin in [
        "compact",
        "autocompact",
        "export",
        "session",
        "name",
        "steering",
        "follow-up",
    ] {
        assert!(
            names.contains(&builtin),
            "ACP-272: `/{builtin}` must be advertised: {names:?}"
        );
    }
    assert!(
        !names.contains(&"changelog"),
        "ACP-070: `/changelog` is cut, and the cut is a decision with a test: {names:?}"
    );

    assert!(
        acp.position_of_update("available_commands_update") > acp.position_of_response(2),
        "ACP-069: the command menu must follow the response — Zed drops notifications for a \
         sessionId it has not been told about yet.\nframes:\n{}",
        acp.dump()
    );

    // The notification is addressed to the session the response minted (`SessionScoped`).
    let addressed =
        acp.seen[acp.position_of_update("available_commands_update")]["params"]["sessionId"]
            .as_str()
            .unwrap()
            .to_owned();
    assert_eq!(addressed, session);
    acp.shutdown();
}

// ------------------------------------------------------------------------------------------
// session/prompt: the built-in dispatcher, the turn, and the cancel interleaving
// ------------------------------------------------------------------------------------------

/// **ACP-282 / ACP-284** — a `/session` prompt is intercepted above the turn queue and answered as
/// command output plus `end_turn`; it never reaches the model.
///
/// `ACP-284` pins the five-line shape. Only the four lines a credential-less session can produce
/// are asserted here; the token/cost lines are present but their values are trivially zero.
#[test]
fn a_builtin_prompt_is_dispatched_above_the_turn_queue() {
    let mut acp = Acp::start();
    let session = acp.new_session(2);

    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "/session"}]}
    }));
    let prefix = format!("Session: {session}\n");
    let chunk = acp.chunk_starting_with(&prefix);
    let text = chunk["content"]["text"].as_str().expect("a text chunk");
    assert!(
        text.starts_with(&prefix),
        "ACP-284: the first line is `Session: <id>`: {text}"
    );
    assert!(text.contains("\nMessages: "), "ACP-284: {text}");
    assert!(
        text.contains("\nCost: $"),
        "ACP-Q43: `${{:.3}}`, not JS number formatting: {text}"
    );

    let response = acp.answer(3);
    assert_eq!(
        response["result"]["stopReason"], "end_turn",
        "ACP-282: every built-in ends the turn: {response}"
    );
    // ACP-122 — the chunk is on the wire before the response, never behind it.
    assert!(
        acp.position_of_chunk(&prefix) < acp.position_of_response(3),
        "ACP-122: a response must never overtake a notification.\nframes:\n{}",
        acp.dump()
    );
    acp.shutdown();
}

/// **ACP-285 / ACP-077 / ACP-Q20** — `/name` mutates and emits nothing itself; the **event pump**
/// is the single emitter of `session_info_update`.
///
/// The assertion that matters is the count: exactly one update for one rename. A port that emits
/// from the setter *and* subscribes the pump produces two, and a port that emits from neither
/// produces a client whose title goes stale with no error anywhere.
#[test]
fn a_rename_produces_exactly_one_session_info_update_from_the_pump() {
    let mut acp = Acp::start();
    let session = acp.new_session(2);

    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "/name integration proof"}]}
    }));
    acp.answer(3);
    acp.drain(Duration::from_millis(1500));

    let chunk = acp.seen[acp.position_of_chunk("Session name set: ")]["params"]["update"].clone();
    assert_eq!(
        chunk["content"]["text"], "Session name set: integration proof",
        "ACP-285: the confirmation is byte-for-byte upstream's"
    );

    let titled_at = acp.position("a titled session_info_update", |f| {
        f["params"]["update"]["sessionUpdate"] == "session_info_update"
            && f["params"]["update"].get("title").is_some()
    });
    let info = acp.seen[titled_at]["params"]["update"].clone();
    assert_eq!(info["title"], "integration proof");

    // **ACP-122 / ACP-285** — and it is on the wire BEFORE the response to the prompt that caused
    // it. `ACP-Q20` routes every `session_info_update` through the event pump, which is a
    // different task from the one that answers a built-in above the turn queue (`ACP-282`), so
    // the notification lost that race every time: the observed order was
    // `agent_message_chunk` -> `{"stopReason":"end_turn"}` -> `session_info_update`, 9 runs of 9,
    // and a client that treats the prompt response as the end of the turn attributes the rename
    // to the next turn or drops it. `crate::commands::RenameEcho` is what closes it: the causer
    // emits the update in its own ordered output and the pump consumes the claim.
    assert!(
        titled_at < acp.position_of_response(3),
        "ACP-122: the rename must reach the client before the turn it belongs to is \
         answered.\nframes:\n{}",
        acp.dump()
    );
    assert!(
        titled_at < acp.position_of_chunk("Session name set: "),
        "ACP-285's verify order: exactly one `session_info_update`, THEN the confirmation \
         line.\nframes:\n{}",
        acp.dump()
    );
    let updated_at = info["updatedAt"]
        .as_str()
        .expect("ACP-204: an ISO-8601 updatedAt");
    assert!(
        updated_at.ends_with('Z') && updated_at.as_bytes()[10] == b'T',
        "ACP-204: `toISOString()` shape, not a hand-rolled calendar: {updated_at}"
    );

    let titled = acp
        .seen
        .iter()
        .filter(|f| {
            f["params"]["update"]["sessionUpdate"] == "session_info_update"
                && f["params"]["update"].get("title").is_some()
        })
        .count();
    assert_eq!(
        titled,
        1,
        "ACP-Q20: one rename, one notification — the setter emits nothing and the pump emits \
         once.\nframes:\n{}",
        acp.dump()
    );
    acp.shutdown();
}

/// **ACP-072 / ACP-075 / ACP-077** — `session/set_mode` answers `{}` and the pump pushes the
/// **applied** level, plus the whole re-derived option set.
///
/// A test asserting only that `{}` came back passes the broken version — which is why the applied
/// level is read out of the pushed `current_mode_update` and cross-checked against the re-derived
/// `config_option_update.currentValue`, the two places a clamp would disagree.
#[test]
fn set_mode_answers_empty_and_the_pump_reports_the_applied_level() {
    // The thinking ladder is model-derived: a session with no model, or with the non-reasoning
    // `faux` one, advertises only `off` and there is no applied level to disagree about.
    let mut acp = Acp::start_with_reasoning_model();
    let session = acp.new_session(2);

    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/set_mode",
        "params": {"sessionId": session, "modeId": "high"}
    }));
    let response = acp.answer(3);
    assert_eq!(
        response["result"],
        json!({}),
        "ACP-072: the response is `{{}}`"
    );

    let mode = acp.update("current_mode_update");
    let applied = mode["currentModeId"]
        .as_str()
        .expect("currentModeId")
        .to_owned();

    let config = acp.update("config_option_update");
    let options = config["configOptions"].as_array().expect("configOptions");
    let thinking = options
        .iter()
        .find(|o| o["id"] == "thought_level")
        .expect("the thought_level option");
    assert_eq!(
        thinking["currentValue"].as_str(),
        Some(applied.as_str()),
        "ACP-075: the re-derived option set and the mode update must report the SAME applied \
         level — this is where a clamp would disagree with itself"
    );
    acp.shutdown();
}

/// **ACP-120 / ACP-210 / ACP-057** — a prompt for an id this connection never issued is
/// `Unknown sessionId: <id>` at `-32602`, byte-for-byte, **and the connection survives it**.
///
/// The survival half is `ACP-057`'s: `ConnectionTo::spawn`'s own doc is that *"if the spawned task
/// returns an error, the entire server will shut down"*, so a per-request failure that propagates
/// turns one bad request into a dead editor connection.
#[test]
fn an_unknown_session_is_invalid_params_and_the_connection_survives() {
    let mut acp = Acp::start();
    let session = acp.new_session(2);

    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": "01a07000-0000-7000-8000-00000000dead",
                   "prompt": [{"type": "text", "text": "x"}]}
    }));
    let response = acp.answer(3);
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(
        response["error"]["message"], "Unknown sessionId: 01a07000-0000-7000-8000-00000000dead",
        "ACP-210: built by hand so `From<ErrorCode>` cannot stamp `Invalid params` over it"
    );

    // ACP-291 — a malformed id is a DIFFERENT fact from an unknown one, and a client that cannot
    // tell them apart cannot tell its own bug from a stale history entry.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "session/delete",
        "params": {"sessionId": "../../etc/passwd"}
    }));
    let refused = acp.answer(4);
    assert_eq!(refused["error"]["code"], -32602);
    assert_ne!(
        refused["error"]["message"], "Unknown sessionId: ../../etc/passwd",
        "ACP-291: a hostile id is refused by the validator, not reported as a lookup miss"
    );

    // ACP-057 — and the connection still answers.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 5, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "/session"}]}
    }));
    let after = acp.answer(5);
    assert!(
        after.get("result").is_some(),
        "ACP-057: two bad requests must not cost the connection: {after}"
    );
    acp.shutdown();
}

/// **ACP-057** — `session/new` is built **off** the dispatch loop, so a request sent after it is
/// answered before it.
///
/// `ConnectionTo`'s own doc is unambiguous that the connection cannot process new messages while a
/// handler runs, and a session build is seconds of settings/resource/extension-host work. Awaiting
/// it inline would queue every later message behind it — including `session/cancel`, which is the
/// one message a user presses a button for.
///
/// [`super::acp_transport`] can only assert that the later request is answered *at all*, because
/// its pipe closes before the build finishes. Holding stdin open is what makes the ORDER visible.
#[test]
fn session_new_is_built_off_the_dispatch_loop() {
    let mut acp = Acp::start();
    let project = acp.project.clone();
    acp.send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "session/new",
        "params": {"cwd": project, "mcpServers": []}
    }));
    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "authenticate",
        "params": {"methodId": "cyrup_terminal_login"}
    }));

    let auth = acp.answer(3);
    assert!(
        auth.get("error").is_none(),
        "ACP-014: authenticate is a successful no-op; an error reads as a failed login: {auth}"
    );
    let created = acp.answer(2);
    assert!(
        created.get("result").is_some(),
        "session/new failed: {created}"
    );

    assert!(
        acp.position_of_response(3) < acp.position_of_response(2),
        "ACP-057: the later `authenticate` must be answered first — otherwise the build is \
         holding the dispatch loop and a `session/cancel` would queue behind it.\nframes:\n{}",
        acp.dump()
    );
    acp.shutdown();
}

/// **ACP-121 / ACP-123 / ACP-153** — a `session/prompt` resolves exactly once, and a
/// `session/cancel` issued straight after it is *dispatched* before the prompt's response.
///
/// The dispatch-order half is the one that can only be seen here. `dispatch_prompt` returns
/// immediately and the turn owns the responder, so the cancel notification is processed while the
/// prompt is still in flight. If `session/prompt` were awaited inline in the handler, the cancel
/// would sit behind it in the dispatch loop and the user's stop button would do nothing until the
/// turn had already finished.
#[test]
fn a_prompt_settles_once_and_a_cancel_is_not_queued_behind_it() {
    let mut acp = Acp::start();
    let session = acp.new_session(2);

    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hello"}]}
    }));
    acp.send(json!({
        "jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}
    }));

    // `ACP-121` / `ACP-126` — ANSWERED, not left as a spinner. This harness is credential-less
    // and has no model, so `AgentSession::prompt`'s preflight refuses with `NoModelSelected` and
    // `AcpFailure::classify` turns that into `auth_required` (-32000): the client is told to
    // configure a key, which is the whole reason `--terminal-login` exists. The assertion is on
    // the exact code rather than on "there is a result", because the previous form —
    // `response.get("result").is_some()` — could not hold in this harness at all and had never
    // run: `Acp::start` removes every provider key, so the only reachable outcome is this error.
    let response = acp.answer(3);
    assert_eq!(
        response["error"]["code"].as_i64(),
        Some(-32000),
        "ACP-126: a preflight refusal is an auth_required ERROR, never a fabricated end_turn: \
         {response}"
    );

    // ACP-121 — exactly once. A second response frame for id 3 is the double-respond the consuming
    // `Turn::settle` exists to make unrepresentable.
    acp.drain(Duration::from_secs(1));
    let answers = acp
        .seen
        .iter()
        .filter(|f| f.get("id").and_then(Value::as_u64) == Some(3))
        .count();
    assert_eq!(
        answers,
        1,
        "ACP-121: a turn resolves EXACTLY once.\nframes:\n{}",
        acp.dump()
    );

    // ACP-159 — and cancelling again, with nothing running, is a legal no-op that answers nothing.
    acp.send(json!({
        "jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}
    }));
    acp.send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "/session"}]}
    }));
    assert!(
        acp.answer(4).get("result").is_some(),
        "ACP-159: an idle cancel must not poison the next turn"
    );
    acp.shutdown();
}

/// **ACP-022, at the wire.** A run that fails must NOT be answered `stopReason: "end_turn"`.
///
/// This is the defect the field report reproduced three ways (a real Bedrock 403, an injected 401,
/// an injected 500 after a tool result): the editor rendered a successful, empty turn while the
/// JSONL recorded `stopReason='error'`. `crates/cyrup-acp/src/turn.rs`'s unit tests drive the
/// `AgentEnd`/`AgentSettled` pair directly; only here is the whole path — a real
/// `AgentSession::prompt`, a real provider failure, a real `session/prompt` response frame —
/// under test.
///
/// The scripted `faux` provider with nothing queued answers `StopReason::Error` /
/// `"No more faux responses queued"`, which is a genuine terminal run failure produced offline.
#[test]
fn a_failing_run_is_an_error_response_not_an_empty_end_turn() {
    let mut acp = Acp::start_scripted();
    let session = acp.new_session(2);

    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hello"}]}
    }));
    let response = acp.answer(3);

    assert!(
        response.get("result").is_none(),
        "ACP-022: a run whose terminal message is `stopReason: error` must not resolve the \
         prompt successfully — an `end_turn` with no content is a turn the user reads as \
         complete and empty: {response}"
    );
    let error = &response["error"];
    assert_eq!(
        error["code"].as_i64(),
        Some(-32603),
        "a non-auth provider failure is Internal: {response}"
    );
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("No more faux responses queued")),
        "the provider's own sentence is what reaches the client: {response}"
    );

    // ACP-121 — still exactly once, on the failure path too.
    acp.drain(Duration::from_secs(1));
    let answers = acp
        .seen
        .iter()
        .filter(|f| f.get("id").and_then(Value::as_u64) == Some(3))
        .count();
    assert_eq!(answers, 1, "ACP-121:\nframes:\n{}", acp.dump());

    // ACP-057 — and the connection survives it: the next request is answered.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "/session"}]}
    }));
    assert!(
        acp.answer(4).get("result").is_some(),
        "ACP-057: a failed turn must not close the connection"
    );
    acp.shutdown();
}

/// **ACP-123 / ACP-153** — a `session/cancel` sent straight after a `session/prompt` is
/// *dispatched* while that prompt is still in flight.
///
/// This is the half that can only be seen against a real run: `dispatch_prompt` returns
/// immediately and the turn owns the responder, so the notification is processed before the
/// prompt's response is written. If `session/prompt` were awaited inline in the handler the cancel
/// would sit behind it in the dispatch loop and the user's stop button would do nothing until the
/// turn had already finished.
///
/// The assertion is on the *response order*, not on the stop reason: whether the cancel lands
/// before or after the (very fast) scripted run settles is a race, and pinning `cancelled` here
/// would make the test flaky. What is not a race is that the connection kept dispatching.
#[test]
fn a_cancel_is_dispatched_while_the_prompt_is_still_in_flight() {
    let mut acp = Acp::start_scripted();
    let session = acp.new_session(2);

    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hello"}]}
    }));
    acp.send(json!({
        "jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}
    }));
    // A request issued AFTER the cancel. If the dispatch loop were blocked inside the prompt,
    // neither this nor the cancel could be processed until the turn ended — and this response
    // would then arrive after the prompt's.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "session/list", "params": {}
    }));

    acp.answer(4);
    acp.answer(3);
    assert!(
        acp.position_of_response(4) < acp.position_of_response(3),
        "ACP-153: the later `session/list` must be answered first — otherwise the dispatch loop \
         is held by the prompt and a `session/cancel` cannot reach the turn.\nframes:\n{}",
        acp.dump()
    );

    acp.drain(Duration::from_secs(1));
    assert_eq!(
        acp.seen
            .iter()
            .filter(|f| f.get("id").and_then(Value::as_u64) == Some(3))
            .count(),
        1,
        "ACP-121: a cancelled turn still resolves EXACTLY once.\nframes:\n{}",
        acp.dump()
    );
    acp.shutdown();
}

/// **ACP-219 / ACP-224** — deleting the session the client is *currently in*.
///
/// The critical this unit is rated for is silent data loss: `DiskStore` holds an `O_APPEND` fd, so
/// unlinking the live session's file without disposing it first leaves the session running and
/// appending every later turn to an inode no listing, no `session/load` and no user can reach —
/// and nothing errors. `SessionManager::delete_session` takes the session out of the slot and
/// disposes it (through `LiveSession::dispose_and_take_path`, which is the type-level guarantee
/// that the two cannot be reordered) before anything touches the file.
///
/// The scripted provider is what makes this reachable offline: the file is deferred until the
/// first assistant message, so a credential-less harness never puts one on disk to delete.
///
/// `ACP-224`'s pinned consequence is asserted too — a following `session/prompt` gets
/// `Unknown sessionId` rather than resurrecting the file, which is where cyrup deliberately
/// differs from pi-acp.
#[test]
fn deleting_the_live_session_removes_its_file_and_leaves_no_stub() {
    let mut acp = Acp::start_scripted();
    let session = acp.new_session(2);

    // One turn, so the transcript is flushed. The run FAILS (nothing is queued in the faux
    // provider) and that is fine here — an errored assistant turn is still an assistant turn, and
    // it is what materialises the file.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hello"}]}
    }));
    acp.answer(3);

    let dir = seeded_dir(&acp.home, &acp.project);
    let file = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no session dir at {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(&session) && n.ends_with(".jsonl"))
        })
        .unwrap_or_else(|| panic!("no transcript for {session} in {}", dir.display()));

    acp.send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "session/delete",
        "params": {"sessionId": session}
    }));
    let deleted = acp.answer(4);
    assert_eq!(
        deleted["result"],
        json!({}),
        "ACP-218: the response is upstream's empty object: {deleted}"
    );
    assert!(
        !file.exists(),
        "ACP-219: the file is gone: {}",
        file.display()
    );

    // ACP-224 — a following prompt for the deleted id is `Unknown sessionId`, not a resurrection.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 5, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hello again"}]}
    }));
    let after = acp.answer(5);
    assert_eq!(after["error"]["code"].as_i64(), Some(-32602), "{after}");
    assert!(
        after["error"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("Unknown sessionId: ")),
        "ACP-210's message, byte-for-byte: {after}"
    );

    // ACP-219's own verify: nothing recreated the file behind the delete — not the disposed
    // session, not the refused prompt.
    acp.drain(Duration::from_millis(500));
    assert!(
        !file.exists(),
        "ACP-219: no headerless stub reappeared at {}",
        file.display()
    );

    // ACP-218 — and deleting it again is a legal, idempotent success.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 6, "method": "session/delete",
        "params": {"sessionId": session}
    }));
    assert_eq!(acp.answer(6)["result"], json!({}));
    acp.shutdown();
}

/// **ACP-066 / ACP-068 / ACP-081** — the startup prelude, and the frame order it must arrive in.
///
/// The ordering assertion is on **raw frame order**, which is the whole of `ACP-068`: upstream
/// needs a `setTimeout(…, 0)` because in TypeScript the response is emitted by returning, and the
/// cut of that timer is only correct because `Responder::respond` and
/// `ConnectionTo::send_notification` enqueue on the same channel from the same task. A refactor
/// that answers `session/new` from a different task breaks this silently, and this is what says so.
#[test]
fn the_startup_prelude_follows_the_session_new_response() {
    let mut acp = Acp::start();
    // Something to report. An empty project has no context, no skills, no prompts, no extensions
    // and no custom themes, and `ACP-081` then suppresses the prelude entirely — which is its own
    // assertion, below.
    std::fs::write(acp.project.join("AGENTS.md"), "# house rules\n").unwrap();

    let session = acp.new_session(2);
    assert!(!session.is_empty());

    let prelude = acp.chunk_starting_with("## ");
    let text = prelude["content"]["text"].as_str().expect("a text chunk");
    assert!(
        text.starts_with("## Context\n- AGENTS.md"),
        "ACP-066: the inventory is `## Heading` + `- item`, Context comes first (pi's order), and \
         a context file inside the cwd is named relative to it rather than by absolute path: \
         {text:?}"
    );
    assert!(
        !text.contains("## Themes"),
        "ACP-066: a section with nothing in it emits nothing at all, not a bare heading — this \
         hermetic HOME has no custom themes: {text:?}"
    );
    // The native built-ins are loaded in every run, so the Extensions block is always present and
    // always after Context. That pins pi's section ORDER, which the renderer must not reshuffle.
    let context_at = text.find("## Context").expect("a context block");
    let extensions_at = text
        .find("## Extensions")
        .unwrap_or_else(|| panic!("the native extensions are always loaded: {text:?}"));
    assert!(context_at < extensions_at, "pi's order: {text:?}");

    // ACP-069 — the command menu is the other follow-up, and it is also after the response.
    acp.update("available_commands_update");
    assert!(
        acp.position_of_response(2) < acp.position_of_chunk("## "),
        "ACP-068: the prelude must NOT overtake the response — some clients drop a notification \
         for a sessionId they have not been told about yet.\nframes:\n{}",
        acp.dump()
    );
    assert!(
        acp.position_of_response(2) < acp.position_of_update("available_commands_update"),
        "ACP-069: and neither may the command menu.\nframes:\n{}",
        acp.dump()
    );
    acp.shutdown();
}

/// **ACP-066** — an empty project still reports the session's own inventory, and reports exactly
/// one prelude.
///
/// `ACP-081`'s suppression rule (an all-empty inventory emits no chunk at all, where upstream
/// emits one containing a single newline) is **not** assertable here and its unit test is
/// `cyrup_acp::startup::tests::a_bare_project_produces_no_prelude_at_all`: the native built-in
/// extensions are compiled into the binary, so `## Extensions` is never empty and the wire always
/// carries a prelude. What IS assertable here is the other half — that a bare project's prelude
/// contains the blocks it can fill and none of the ones it cannot, and that there is exactly one.
#[test]
fn a_bare_project_reports_only_the_blocks_it_can_fill() {
    let mut acp = Acp::start();
    acp.new_session(2);
    let text = acp.chunk_starting_with("## ")["content"]["text"]
        .as_str()
        .expect("a text chunk")
        .to_owned();
    assert!(
        text.contains("## Extensions"),
        "the native built-ins are always loaded: {text:?}"
    );
    assert!(
        !text.contains("## Context") && !text.contains("## Themes"),
        "ACP-066: an empty section emits nothing at all — this project has no AGENTS.md and this \
         HOME has no custom themes: {text:?}"
    );

    // Exactly one prelude: `HandlerOutcome::follow_up` is consumed once, so upstream's
    // `startupInfoSent` flag has no counterpart and cannot get out of step.
    acp.update("available_commands_update");
    acp.drain(Duration::from_millis(500));
    let preludes = acp
        .seen
        .iter()
        .filter(|f| {
            f["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
                && f["params"]["update"]["content"]["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("## "))
        })
        .count();
    assert_eq!(
        preludes,
        1,
        "one session, one prelude.\nframes:\n{}",
        acp.dump()
    );
    acp.shutdown();
}

// ------------------------------------------------------------------------------------------
// session/list, session/load, session/delete
// ------------------------------------------------------------------------------------------

/// **ACP-217** — the whole of `session/load`'s ordering, which is the reason it has its own driver
/// (`SessionManager::handle_load`) rather than going through `crate::respond_then_notify`.
///
/// Every replay notification must be on the wire **before** the response, and
/// `available_commands_update` **after** it. `respond_then_notify` writes the response first by
/// construction — correct for `session/new`, wrong here — and `LoadSessionResponse` names no
/// session, so `SessionScoped` yields `None` and a follow-up would not be addressable at all.
/// Both facts are `lib.rs`'s and neither is a defect there; this is the assertion that the load
/// path does not go through it.
///
/// Also **ACP-214** (user/assistant text replay, in transcript order) and **ACP-203**…**ACP-207**
/// (the listing projection) against the same seeded transcript.
#[test]
fn session_load_replays_before_its_response_and_advertises_after_it() {
    let mut acp = Acp::start();
    let (home, project) = (acp.home.clone(), acp.project.clone());
    let path = seed_session(&home, &project, SEEDED_ID, "seeded proof session");
    assert!(path.exists());

    // ACP-203 / ACP-205 / ACP-206 — the projection: id, absolute cwd, title, ISO-8601 updatedAt.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "session/list", "params": {"cwd": project}
    }));
    let listed = acp.answer(2);
    let rows = listed["result"]["sessions"].as_array().expect("sessions");
    assert_eq!(rows.len(), 1, "the seeded session must be found: {listed}");
    assert_eq!(rows[0]["sessionId"], SEEDED_ID);
    assert_eq!(rows[0]["cwd"], project.to_string_lossy().as_ref());
    assert_eq!(
        rows[0]["title"], "seeded proof session",
        "ACP-205: the `session_info` name wins over the first user message"
    );
    // ACP-208 — one page, so no cursor; `_meta` is still `{}` rather than absent.
    assert!(
        listed["result"].get("nextCursor").is_none(),
        "ACP-208: `None` is an ABSENT key under `skip_serializing_none`: {listed}"
    );
    assert_eq!(listed["result"]["_meta"], json!({}));

    // ACP-217 — the load itself.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 3, "method": "session/load",
        "params": {"sessionId": SEEDED_ID, "cwd": project, "mcpServers": []}
    }));
    let response = acp.answer(3);
    assert!(
        response.get("error").is_none(),
        "session/load failed: {response}"
    );
    // ACP-062 — a load advertises the same one-read surface a new session does.
    assert!(
        response["result"]["modes"]["availableModes"].is_array(),
        "ACP-062: `session/load` must carry the mode list too: {response}"
    );
    acp.update("available_commands_update");

    // ACP-214 — the transcript, in order, before the response.
    let user = acp.position_of_update("user_message_chunk");
    let agent = acp.position_of_update("agent_message_chunk");
    let settled = acp.position_of_response(3);
    let commands = acp.position_of_update("available_commands_update");
    assert!(
        user < agent,
        "ACP-214: replay is in transcript order.\nframes:\n{}",
        acp.dump()
    );
    assert!(
        agent < settled,
        "ACP-217: EVERY replay notification precedes the response.\nframes:\n{}",
        acp.dump()
    );
    assert!(
        settled < commands,
        "ACP-217: the command advertisement follows it.\nframes:\n{}",
        acp.dump()
    );
    assert_eq!(
        acp.seen[user]["params"]["update"]["content"]["text"],
        "what is 2+2"
    );
    assert_eq!(acp.seen[agent]["params"]["update"]["content"]["text"], "4");

    // ACP-218 — delete answers the literal `{}` and the session is gone from the listing.
    acp.send(json!({
        "jsonrpc": "2.0", "id": 4, "method": "session/delete", "params": {"sessionId": SEEDED_ID}
    }));
    assert_eq!(
        acp.answer(4)["result"],
        json!({}),
        "ACP-Q36: `{{}}`, with no `_meta` audit trail"
    );
    acp.send(json!({
        "jsonrpc": "2.0", "id": 5, "method": "session/delete", "params": {"sessionId": SEEDED_ID}
    }));
    assert_eq!(
        acp.answer(5)["result"],
        json!({}),
        "ACP-218: deleting an already-deleted session is success, not an error"
    );
    assert!(!path.exists(), "ACP-219: the file itself must be gone");

    acp.send(json!({
        "jsonrpc": "2.0", "id": 6, "method": "session/list", "params": {"cwd": project}
    }));
    assert_eq!(
        acp.answer(6)["result"]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "ACP-219: and no headerless stub reappeared under the live session"
    );
    acp.shutdown();
}
