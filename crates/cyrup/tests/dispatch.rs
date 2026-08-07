//! Dispatch integration tests (arch-11 §11 A-11-2/A-11-3): build a real `AgentSession` over a
//! scripted faux provider in a tempdir and exercise the PRINT and JSON dispatchers into `Vec<u8>`
//! buffers — no TTY, no network. Asserts the final assistant text (PRINT) and the ordered JSONL
//! event sequence (JSON), plus follow-up replay (R-11-009).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup::input::Inputs;
use cyrup::run::{run_json_dispatch, run_print_dispatch};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_sdk::core::{AssistantMessage, StopReason};
use cyrup_session_svc::{
    AgentSessionRuntime, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
};
use tempfile::TempDir;

/// Build the RUNTIME host the one-shot modes drive, over a faux provider scripted with `responses`.
///
/// SEAM-006: print/json take an `AgentSessionRuntime`, not a bare `AgentSession` — Pi's entry point
/// is `runPrintMode(runtimeHost: AgentSessionRuntime, options)` (print-mode.ts:32). The tempdirs are
/// returned so they outlive the runtime.
async fn runtime_with(
    responses: Vec<AssistantMessage>,
) -> (Arc<AgentSessionRuntime>, TempDir, TempDir) {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;

    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false; // ephemeral, like one-shot PRINT/JSON (R-11-008)
    let target = config.target.clone();

    let factory = Arc::new(SessionFactory::new(provider, config));
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
    (runtime, cwd, agent_dir)
}

fn text(initial: &str, follow_ups: &[&str]) -> Inputs {
    Inputs {
        initial: initial.to_string(),
        images: Vec::new(),
        follow_ups: follow_ups.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn print_dispatch_writes_final_assistant_text() {
    let (runtime, _cwd, _agent) = runtime_with(vec![faux_assistant_message(
        vec![faux_text("hello world")],
        StopReason::Stop,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    let code = run_print_dispatch(&runtime, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    let printed = String::from_utf8(out).unwrap();
    assert_eq!(printed.trim(), "hello world");
    assert_eq!(code, 0, "a clean Stop terminal reason maps to exit 0");
}

#[tokio::test]
async fn print_dispatch_replays_follow_ups_in_order() {
    let (runtime, _cwd, _agent) = runtime_with(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ])
    .await;

    let mut out: Vec<u8> = Vec::new();
    run_print_dispatch(&runtime, &text("q1", &["q2"]), &mut out)
        .await
        .unwrap();

    // PRINT mode emits ONLY the final transcript message (Pi print-mode.ts:129-146; cyrup commit
    // a2c1bf5). The faux provider answers turns in script order, so the printed final is the
    // follow-up's response ("second answer") — proving the follow-up (q2) was replayed AFTER the
    // initial (q1) consumed "first answer" (R-11-009). The initial turn's text is NOT printed.
    let printed = String::from_utf8(out).unwrap();
    assert!(
        printed.contains("second answer"),
        "the follow-up's response is the final printed message, got: {printed:?}"
    );
    assert!(
        !printed.contains("first answer"),
        "only the final message is printed; the initial turn's text is suppressed, got: {printed:?}"
    );
}

#[tokio::test]
async fn json_dispatch_emits_ordered_event_stream() {
    let (runtime, _cwd, _agent) = runtime_with(vec![faux_assistant_message(
        vec![faux_text("hi there")],
        StopReason::Stop,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    let code = run_json_dispatch(&runtime, &text("hello", &[]), &mut out)
        .await
        .unwrap();

    let body = String::from_utf8(out).unwrap();
    let kinds: Vec<String> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("each JSONL line is a JSON object");
            value
                .get("type")
                .and_then(|t| t.as_str())
                .expect("every event carries a snake_case `type` tag")
                .to_string()
        })
        .collect();

    // JSON mode writes `sessionManager.getHeader()` as JSONL line 1 before the event stream (Pi
    // print-mode.ts:112-117; cyrup commit cbbde87), so the stream opens with the `session` header,
    // then `agent_start`, and closes with `agent_end`.
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("session"),
        "stream opens with the session header line"
    );
    assert_eq!(
        kinds.get(1).map(String::as_str),
        Some("agent_start"),
        "the first event after the header is agent_start"
    );
    // SEAM-005: the stream now closes with `agent_settled`. Pi's json mode writes EVERY subscribed
    // session event verbatim (`session.subscribe(event => writeRawStdout(JSON.stringify(event)))`,
    // print-mode.ts:103-108), and `agent_settled` is emitted last (agent-session.ts:585, from
    // `_runAgentPrompt`'s `finally`), so a pi json consumer sees it too. `agent_end` is now
    // second-to-last.
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("agent_settled"),
        "stream closes with agent_settled (the whole run, not just the last agent loop)"
    );
    assert_eq!(
        kinds.iter().rev().nth(1).map(String::as_str),
        Some("agent_end"),
        "…immediately preceded by the run's last agent_end"
    );
    assert_eq!(code, 0);
}

/// JSON mode ALWAYS returns exit 0, even when the final turn errored/aborted (Pi print-mode.ts:34,
/// 129-148: `exitCode` inits to 0 and is mutated only inside `if (mode === "text")`, so JSON never
/// leaves 0). The contrast below shows the SAME failed turn in PRINT/text mode DOES surface exit 1 —
/// so the divergence is JSON-mode-specific, not a session-wide change.
#[tokio::test]
async fn json_dispatch_always_exits_zero_even_on_failed_turn() {
    let (runtime, _cwd, _agent) = runtime_with(vec![faux_assistant_message(
        vec![faux_text("boom")],
        StopReason::Error,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    let code = run_json_dispatch(&runtime, &text("hi", &[]), &mut out)
        .await
        .unwrap();
    assert_eq!(
        code, 0,
        "JSON mode always returns 0 regardless of the terminal stop reason (Pi convention)"
    );

    // Contrast: the identical failed turn in PRINT/text mode surfaces exit 1 (print-mode.ts:135-137).
    let (runtime2, _cwd2, _agent2) = runtime_with(vec![faux_assistant_message(
        vec![faux_text("boom")],
        StopReason::Error,
    )])
    .await;
    let mut sink: Vec<u8> = Vec::new();
    let text_code = run_print_dispatch(&runtime2, &text("hi", &[]), &mut sink)
        .await
        .unwrap();
    assert_eq!(
        text_code, 1,
        "PRINT/text mode DOES surface a failed final turn as exit 1"
    );
}

/// `--session-id <fresh>` builds via the new `SessionTarget::CreateWithId` arm (Pi
/// `SessionManager.create(cwd, dir, { id })`, main.ts:349) — the session adopts the exact id and
/// persists. Previously a fresh id hit a literal `open` and failed; this exercises the create arm.
#[tokio::test]
async fn create_with_id_target_adopts_the_exact_id() {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());

    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = true;
    config.target = SessionTarget::CreateWithId("my-custom-id".to_string());

    let session = SessionBuilder::new(provider, config).build().await.unwrap();
    assert_eq!(session.session_id().as_str(), "my-custom-id");
}

/// `--fork <ref>` builds via the new `SessionTarget::Fork` arm (Pi `SessionManager.forkFrom`,
/// main.ts:251): the source session's history is copied into a fresh session that adopts the supplied
/// `--session-id`. Exercises the create-source → flush → fork-with-id path end-to-end.
#[tokio::test]
async fn fork_target_copies_history_into_a_new_id() {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();

    // 1. Build + run a persisted SOURCE session so a file with entries exists on disk.
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("source answer")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut src_cfg = SessionConfig::new(cwd.path(), agent_dir.path());
    src_cfg.persist = true;
    let src_target = src_cfg.target.clone();
    let src_factory = Arc::new(SessionFactory::new(provider, src_cfg));
    let source = AgentSessionRuntime::create(src_factory, src_target).await.unwrap();
    let mut sink: Vec<u8> = Vec::new();
    run_print_dispatch(&source, &text("seed", &[]), &mut sink)
        .await
        .unwrap();
    let source_file = source
        .session()
        .await
        .session_file()
        .await
        .expect("source session flushed to disk");

    // 2. Fork that file into a fresh session that adopts the explicit id.
    let provider2: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut fork_cfg = SessionConfig::new(cwd.path(), agent_dir.path());
    fork_cfg.persist = true;
    fork_cfg.target = SessionTarget::Fork {
        source: source_file,
        id: Some("forked-id".to_string()),
    };
    let forked = SessionBuilder::new(provider2, fork_cfg)
        .build()
        .await
        .unwrap();

    assert_eq!(
        forked.session_id().as_str(),
        "forked-id",
        "fork adopts --session-id"
    );
    assert!(
        !forked.entries_json().await.is_empty(),
        "fork copies the source history"
    );
}

// ----------------------------------------------------------------------------------------------
// SEAM-002 — every non-interactive host must tear the session down on a normal exit, emitting
// `session_shutdown{reason:"quit"}`. Pi reaches `AgentSessionRuntime.dispose()`
// (agent-session-runtime.ts:397-404) on EVERY exit: print-mode.ts's `finally { await
// disposeRuntime() }` (:152-157) and rpc-mode.ts's `shutdown()` (:723-739), triggered by stdin EOF
// (:801-803). Pre-fix cyrup's `dispose()` had zero production callers, so no extension and no
// subscriber ever observed a shutdown on a normal `cyrup -p …` / `--mode rpc` run.
//
// Note the ORDER pi uses — `unsubscribe()` then `dispose()` (rpc-mode.ts:731-733) — so the shutdown
// is NOT written to the mode's own output sink; it is observed by an independent subscriber (and by
// extensions), which is exactly what these tests assert.
// ----------------------------------------------------------------------------------------------

/// Drain `sub` for up to `budget` events and report whether a `session_shutdown` with
/// `reason == "quit"` came through.
async fn saw_quit_shutdown(
    sub: &mut cyrup_sdk::core::EventStream<cyrup_session_svc::AgentSessionEvent>,
) -> bool {
    use futures::StreamExt;
    for _ in 0..200 {
        match tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await {
            Ok(Some(cyrup_session_svc::AgentSessionEvent::SessionShutdown { reason })) => {
                assert_eq!(reason, "quit", "Pi disposes with reason `quit`");
                return true;
            }
            Ok(Some(_)) => {}
            _ => return false,
        }
    }
    false
}

#[tokio::test]
async fn print_dispatch_disposes_the_session_on_exit() {
    let (runtime, _cwd, _agent) = runtime_with(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )])
    .await;
    let mut sub = runtime.session().await.subscribe();

    let mut out: Vec<u8> = Vec::new();
    run_print_dispatch(&runtime, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    assert!(
        saw_quit_shutdown(&mut sub).await,
        "PRINT dispatch must emit session_shutdown{{reason:\"quit\"}} on exit (Pi print-mode.ts:152-157)"
    );
}

#[tokio::test]
async fn json_dispatch_disposes_the_session_on_exit() {
    let (runtime, _cwd, _agent) = runtime_with(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )])
    .await;
    let mut sub = runtime.session().await.subscribe();

    let mut out: Vec<u8> = Vec::new();
    run_json_dispatch(&runtime, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    assert!(
        saw_quit_shutdown(&mut sub).await,
        "JSON dispatch must emit session_shutdown{{reason:\"quit\"}} on exit (Pi print-mode.ts:152-157)"
    );
}

/// RPC: reader EOF is Pi's `process.stdin.on("end") → shutdown() → runtimeHost.dispose()`
/// (rpc-mode.ts:801-803 / :723-739).
#[tokio::test]
async fn rpc_dispatch_disposes_the_runtime_at_reader_eof() {
    use cyrup_session_svc::{AgentSessionRuntime, SessionFactory};

    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false;
    let target = config.target.clone();
    let factory = Arc::new(SessionFactory::new(provider, config));
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();

    let mut sub = runtime.session().await.subscribe();

    // A single command, then EOF.
    let reader =
        tokio::io::BufReader::new(std::io::Cursor::new(b"{\"type\":\"get_state\",\"id\":\"s\"}\n".to_vec()));
    let mut writer: Vec<u8> = Vec::new();
    cyrup::run::run_rpc_dispatch(&runtime, reader, &mut writer)
        .await
        .unwrap();

    assert!(
        saw_quit_shutdown(&mut sub).await,
        "RPC dispatch must dispose the runtime at reader EOF (Pi rpc-mode.ts:723-739/:801-803)"
    );
}

// ----------------------------------------------------------------------------------------------
// SEAM-001 — the mirror image of SEAM-002: every non-interactive host must ANNOUNCE the initial
// session with `session_start{reason:"startup"}` before it runs anything. Pi binds the extension
// host at print-mode.ts:73 / rpc-mode.ts:318, ahead of the send loop, and `bindExtensions` ends by
// emitting `_sessionStartEvent` (agent-session.ts:2250), which defaults to
// `{type:"session_start", reason:"startup"}` (agent-session.ts:389). Pre-fix cyrup emitted
// `session_start` ONLY from the runtime's replacement tail, so a one-shot `cyrup -p …` run — the
// same path a spawned subagent child re-execs into — never announced its one and only session.
// ----------------------------------------------------------------------------------------------

/// A native built-in that records the ORDER of the extension-visible lifecycle events. This is the
/// consumer Pi's `bindExtensions` actually serves (agent-session.ts:2250 emits `session_start` to
/// the extension runner), and the one SEAM-001/SEAM-002 exist for: the permission gate refreshing
/// its per-cwd policy, subagents resetting background-run tracking, intercom deregistering.
///
/// Asserting from HERE rather than from a session subscriber is what makes the test independent of
/// when the host happens to subscribe. Pi's own print/json subscriber is installed AFTER
/// `bindExtensions` (print-mode.ts:98→104), so it never sees `session_start` either — an assertion
/// on subscriber ordering would be pinning a cyrup-specific accident.
#[derive(Default)]
struct LifecycleProbe {
    seen: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for LifecycleProbe {
    fn id(&self) -> cyrup_sdk::core::ExtensionId {
        cyrup_sdk::core::ExtensionId::from("lifecycle-probe")
    }

    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[
            cyrup_ext::EventKind::SessionStart,
            cyrup_ext::EventKind::AgentStart,
            cyrup_ext::EventKind::SessionShutdown,
        ]);
        Ok(())
    }

    async fn on_event(
        &self,
        ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        let tag = match ev {
            cyrup_ext::HostEvent::SessionStart { .. } => Some("session_start"),
            cyrup_ext::HostEvent::AgentStart => Some("agent_start"),
            cyrup_ext::HostEvent::SessionShutdown { .. } => Some("session_shutdown"),
            _ => None,
        };
        if let Some(tag) = tag
            && let Ok(mut g) = self.seen.lock()
        {
            g.push(tag);
        }
        cyrup_ext::HookOutcome::Noop
    }
}

/// Build a runtime carrying the lifecycle probe; hands back the probe's shared log.
async fn runtime_with_probe(
    responses: Vec<AssistantMessage>,
) -> (
    Arc<AgentSessionRuntime>,
    Arc<std::sync::Mutex<Vec<&'static str>>>,
    TempDir,
    TempDir,
) {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;

    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false;
    let target = config.target.clone();

    let probe = Arc::new(LifecycleProbe::default());
    let seen = Arc::clone(&probe.seen);
    let factory = Arc::new(
        SessionFactory::new(provider, config)
            .with_native_extension(probe as Arc<dyn cyrup_ext::NativeExtension>),
    );
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
    (runtime, seen, cwd, agent_dir)
}

/// Assert `session_start` reached the extension exactly once, before the first run, and that
/// `session_shutdown` closed the sequence (SEAM-001 + SEAM-002 in one ordering).
fn assert_lifecycle(kinds: &[&str], mode: &str) {
    let starts = kinds.iter().filter(|k| **k == "session_start").count();
    assert_eq!(
        starts, 1,
        "{mode} dispatch must emit exactly one session_start (Pi agent-session.ts:2250); saw {kinds:?}"
    );
    let start_at = kinds.iter().position(|k| *k == "session_start").unwrap();
    let agent_at = kinds
        .iter()
        .position(|k| *k == "agent_start")
        .unwrap_or_else(|| panic!("{mode} dispatch must actually run a turn; saw {kinds:?}"));
    assert!(
        start_at < agent_at,
        "{mode} dispatch must announce the session BEFORE the first prompt \
         (Pi binds extensions at print-mode.ts:73, ahead of the send loop at :121); saw {kinds:?}"
    );
    assert_eq!(
        kinds.last().copied(),
        Some("session_shutdown"),
        "{mode} dispatch must tear the session down last (Pi print-mode.ts:152-157); saw {kinds:?}"
    );
}

#[tokio::test]
async fn print_dispatch_announces_session_start_before_the_run() {
    let (runtime, seen, _cwd, _agent) = runtime_with_probe(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    run_print_dispatch(&runtime, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    let kinds = seen.lock().unwrap().clone();
    assert_lifecycle(&kinds, "PRINT");
}

#[tokio::test]
async fn json_dispatch_announces_session_start_before_the_run() {
    let (runtime, seen, _cwd, _agent) = runtime_with_probe(vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )])
    .await;

    let mut out: Vec<u8> = Vec::new();
    run_json_dispatch(&runtime, &text("hi", &[]), &mut out)
        .await
        .unwrap();

    let kinds = seen.lock().unwrap().clone();
    assert_lifecycle(&kinds, "JSON");
}

/// Drain `sub` until the stream ends (or a per-event timeout elapses) and return the ordered event
/// kind strings.
async fn drain_kinds(
    sub: &mut cyrup_sdk::core::EventStream<cyrup_session_svc::AgentSessionEvent>,
) -> Vec<&'static str> {
    use futures::StreamExt;
    let mut kinds = Vec::new();
    for _ in 0..400 {
        match tokio::time::timeout(std::time::Duration::from_millis(500), sub.next()).await {
            Ok(Some(ev)) => kinds.push(ev.kind()),
            _ => break,
        }
    }
    kinds
}

/// SEAM-006: a loaded extension's `ctx.newSession()` must actually REPLACE the session under
/// `--mode print` / `--mode json`. Pi's print mode binds `commandContextActions.newSession` to
/// `runtimeHost.newSession` (print-mode.ts:76), so the op has a host. Before this, print/json ran
/// on a bare `AgentSession`, `apply_pending_control` found no `runtime_actions`, and the op died as
/// `SessionServiceError::NoRuntimeHost` behind a `tracing::warn!` the user never sees — for every
/// `cyrup -p` run AND every spawned subagent child, which re-execs into this same arm.
/// A native built-in whose `/swap` command queues a runtime-tier `ControlOp::NewSession` through the
/// same `HostServices::control` seam a wasm guest's `control.*` import reaches.
#[derive(Default)]
struct NewSessionExt {
    services: Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::HostServices>>>>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for NewSessionExt {
    fn id(&self) -> cyrup_sdk::core::ExtensionId {
        cyrup_sdk::core::ExtensionId::from("print-new-session")
    }
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.register_command(
            "swap",
            cyrup_ext::CommandDescriptor {
                description: "replace the active session".to_string(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }
    async fn on_event(
        &self,
        _ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        cyrup_ext::HookOutcome::Noop
    }
    async fn execute_command(
        &self,
        _name: &str,
        _args: &str,
        _ctx: &cyrup_ext::HostCtx,
    ) -> Result<Option<String>, cyrup_ext::ExtError> {
        let svc = self
            .services
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| cyrup_ext::ExtError::Component("no host services".into()))?;
        svc.control(cyrup_ext::ControlOp::NewSession { opts: serde_json::json!({}) })
            .map_err(cyrup_ext::ExtError::Component)?;
        Ok(Some(String::new()))
    }
}

/// Whether any assistant message in `ms` carries `needle` as text.
fn transcript_has(ms: &[cyrup_sdk::core::Message], needle: &str) -> bool {
    ms.iter().any(|m| match m {
        cyrup_sdk::core::Message::Assistant(a) => a.content.iter().any(|c| {
            matches!(c, cyrup_sdk::core::Content::Text { text, .. } if text.contains(needle))
        }),
        _ => false,
    })
}

#[tokio::test]
async fn print_dispatch_gives_extension_control_ops_a_runtime_host() {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("after the swap")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false;
    let target = config.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, config).with_native_extension(
            Arc::new(NewSessionExt::default()) as Arc<dyn cyrup_ext::NativeExtension>,
        ),
    );
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
    // Hold the ORIGINAL session so the test can prove the follow-up did NOT land on it. Asserting
    // only on stdout does not discriminate: the pre-fix, hoisted-session code prints "after the
    // swap" too — it just prints it out of the WRONG (replaced, disposed) session's transcript.
    let first = runtime.session().await;
    let first_id = first.session_id().to_string();

    let mut out: Vec<u8> = Vec::new();
    // `/swap` is the initial submission; the follow-up then runs on whatever session is active.
    run_print_dispatch(&runtime, &text("/swap", &["say something"]), &mut out)
        .await
        .unwrap();

    let active = runtime.session().await;
    assert_ne!(
        active.session_id().to_string(),
        first_id,
        "ctx.newSession() from a print-mode extension command must REPLACE the active session \
         (SEAM-006); it silently failed with NoRuntimeHost"
    );

    // The discriminating assertion: the follow-up ran on the REBOUND session, so its answer is in
    // the NEW session's transcript and absent from the replaced one. Pi's `rebindSession` does
    // `session = runtimeHost.session` (print-mode.ts:72), which is why cyrup's send loop re-reads
    // the active session per message instead of hoisting it once.
    assert!(
        transcript_has(&active.messages().await, "after the swap"),
        "the follow-up must run on the REBOUND session; the NEW session's transcript is {:?}",
        active.messages().await
    );
    assert!(
        !transcript_has(&first.messages().await, "after the swap"),
        "the follow-up must NOT be serviced by the replaced session (SEAM-006); it landed on {first_id}"
    );

    let printed = String::from_utf8(out).unwrap();
    assert!(
        printed.contains("after the swap"),
        "the follow-up must run on the REBOUND session and print its answer (Pi print-mode.ts:72); \
         got {printed:?}"
    );
}

/// The JSON-mode twin of the above. `runPrintMode` serves BOTH modes off the same `runtimeHost`
/// (print-mode.ts:32, the `mode === "json"` branch at :73), so fixing only one leaves the identical
/// defect in the other — which is exactly how this class of bug survives.
#[tokio::test]
async fn json_dispatch_gives_extension_control_ops_a_runtime_host() {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("after the swap")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false;
    let target = config.target.clone();
    let factory = Arc::new(
        SessionFactory::new(provider, config).with_native_extension(
            Arc::new(NewSessionExt::default()) as Arc<dyn cyrup_ext::NativeExtension>,
        ),
    );
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
    let first = runtime.session().await;
    let first_id = first.session_id().to_string();

    let mut out: Vec<u8> = Vec::new();
    run_json_dispatch(&runtime, &text("/swap", &["say something"]), &mut out)
        .await
        .unwrap();

    let active = runtime.session().await;
    assert_ne!(
        active.session_id().to_string(),
        first_id,
        "ctx.newSession() from a json-mode extension command must REPLACE the active session \
         (SEAM-006); it silently failed with NoRuntimeHost"
    );
    assert!(
        transcript_has(&active.messages().await, "after the swap"),
        "the follow-up must run on the REBOUND session; the NEW session's transcript is {:?}",
        active.messages().await
    );
    assert!(
        !transcript_has(&first.messages().await, "after the swap"),
        "the follow-up must NOT be serviced by the replaced session (SEAM-006); it landed on {first_id}"
    );
}

/// RPC: `AgentSessionRuntime::create` is the bind point for the persistent host (Pi rpc-mode.ts:318
/// `rebindSession()` invoked at :381), so the session handed out by the runtime is already
/// announced — and a later host bind must not repeat it.
#[tokio::test]
async fn rpc_runtime_announces_the_initial_session_at_startup() {
    use cyrup_session_svc::{AgentSessionRuntime, SessionFactory};

    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false;
    let target = config.target.clone();
    let factory = Arc::new(SessionFactory::new(provider, config));
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();

    let session = runtime.session().await;
    let mut sub = runtime.session().await.subscribe();
    // A host that binds after taking the runtime's session must NOT produce a second announcement.
    cyrup::run::announce_session_start(&session).await;

    let kinds = drain_kinds(&mut sub).await;
    assert!(
        !kinds.contains(&"session_start"),
        "the runtime already announced its initial session; a host bind must not repeat it, saw {kinds:?}"
    );
}

// ================================================================================================
// SEAM-033 — the initial `session_start` must be announced AFTER the host applies post-build CLI
// configuration, not at runtime-construction time.
//
// Pi ground truth, read from source: `main.ts:650` applies `--name`
// (`sessionManager.appendSessionInfo(name)`) and `main.ts:742-750` folds the resolved `--models`
// scope into `sessionOptions`, both strictly BEFORE `main.ts:793 createAgentSessionRuntime(...)`.
// `createAgentSessionRuntime` itself (agent-session-runtime.ts:414-432) deliberately never calls
// `bindExtensions`, so it emits nothing. The HOST announces, later still, from
// `rebindSession()` → `session.bindExtensions(...)` (print-mode.ts:119 → :73 →
// agent-session.ts:2250).
//
// Cyrup's analog of main.ts:650/742-750 is `main.rs`'s `apply_post_build`, which runs AFTER the
// runtime is built (the session it configures does not exist before). So the runtime constructor
// used by the print/json arm must NOT announce — `AgentSessionRuntime::create_unannounced` — and
// the announcement is the first thing `run_print`/`run_json` do. Announcing at construction time
// hands every `session_start` handler an unnamed, unscoped session; and since print/json is the arm
// a spawned subagent child re-execs into, every subagent run inherits it.
// ================================================================================================

/// Records, for each `session_start` it observes, what the session looked like AT THAT MOMENT.
///
/// The session handle is installed by the test after the runtime exists — which is exactly the
/// window under test. If the announcement has already happened by then, the probe records the
/// `announced-before-the-host-could-configure-anything` marker instead of a name, so a regression
/// reads as a description rather than an empty vector.
#[derive(Default)]
struct ConfigAtStartProbe {
    session: Arc<std::sync::OnceLock<Arc<cyrup_session_svc::AgentSession>>>,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for ConfigAtStartProbe {
    fn id(&self) -> cyrup_sdk::core::ExtensionId {
        cyrup_sdk::core::ExtensionId::from("config-at-start-probe")
    }

    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::SessionStart]);
        Ok(())
    }

    async fn on_event(
        &self,
        ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        if matches!(ev, cyrup_ext::HostEvent::SessionStart { .. }) {
            let observed = match self.session.get() {
                None => "announced-before-the-host-could-configure-anything".to_string(),
                Some(session) => format!(
                    "name={:?} scoped_models={}",
                    session.session_name().await,
                    session.scoped_models().len()
                ),
            };
            if let Ok(mut g) = self.seen.lock() {
                g.push(observed);
            }
        }
        cyrup_ext::HookOutcome::Noop
    }
}

/// Build the print/json arm the way `main.rs` does: `create_unannounced`, then post-build
/// configuration, then dispatch. Returns the probe's shared handles so the test can install the
/// session and read what the extension saw.
#[allow(clippy::type_complexity)]
async fn unannounced_runtime_with_probe(
    responses: Vec<AssistantMessage>,
) -> (
    Arc<AgentSessionRuntime>,
    Arc<std::sync::OnceLock<Arc<cyrup_session_svc::AgentSession>>>,
    Arc<std::sync::Mutex<Vec<String>>>,
    TempDir,
    TempDir,
) {
    let cwd = tempfile::tempdir().unwrap();
    let agent_dir = tempfile::tempdir().unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;

    let mut config = SessionConfig::new(cwd.path(), agent_dir.path());
    config.persist = false;
    let target = config.target.clone();

    let probe = Arc::new(ConfigAtStartProbe::default());
    let session_slot = Arc::clone(&probe.session);
    let seen = Arc::clone(&probe.seen);
    let factory = Arc::new(
        SessionFactory::new(provider, config)
            .with_native_extension(probe as Arc<dyn cyrup_ext::NativeExtension>),
    );
    let runtime = AgentSessionRuntime::create_unannounced(factory, target).await.unwrap();
    (runtime, session_slot, seen, cwd, agent_dir)
}

/// The constructor `main.rs`'s print/json arm uses must be silent — pi's `createAgentSessionRuntime`
/// returns without ever calling `bindExtensions` (agent-session-runtime.ts:414-432).
#[tokio::test]
async fn create_unannounced_leaves_the_announcement_to_the_host() {
    let (runtime, slot, seen, _cwd, _agent) = unannounced_runtime_with_probe(vec![]).await;
    let session = runtime.session().await;
    let _ = slot.set(Arc::clone(&session));

    assert_eq!(
        seen.lock().unwrap().clone(),
        Vec::<String>::new(),
        "create_unannounced must not announce; the host does (Pi createAgentSessionRuntime never \
         calls bindExtensions, agent-session-runtime.ts:414-432)"
    );

    // …and the announcement is still reachable, once, when the host does bind.
    session.bind_extensions().await;
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the host bind must produce exactly one session_start (agent-session.ts:2250)"
    );
}

/// PRINT: `--name` and `--models` (cyrup's `apply_post_build`, pi main.ts:650 + :742-750) are
/// applied between building the runtime and running the mode, so the extension's `session_start`
/// handler must see a session that already carries them.
#[tokio::test]
async fn print_dispatch_announces_only_after_post_build_configuration() {
    let (runtime, slot, seen, _cwd, _agent) = unannounced_runtime_with_probe(vec![
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ])
    .await;

    // Exactly what `main.rs` does between `create_unannounced` and the dispatch call.
    let session = runtime.session().await;
    let _ = slot.set(Arc::clone(&session));
    session.set_session_name("configured-by-cli").await.unwrap();
    session.set_scoped_models(
        session
            .model_catalog()
            .into_iter()
            .take(1)
            .map(|model| cyrup_session_svc::ScopedModel { model, thinking_level: None })
            .collect(),
    );
    let scoped = session.scoped_models().len();

    let mut buf: Vec<u8> = Vec::new();
    run_print_dispatch(&runtime, &text("hi", &[]), &mut buf).await.unwrap();

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec![format!("name={:?} scoped_models={scoped}", Some("configured-by-cli".to_string()))],
        "PRINT must announce the session only after --name/--models are applied \
         (Pi main.ts:650/:742-750 precede main.ts:793, and the announcement is later still at \
         print-mode.ts:119)"
    );
}

/// JSON: same ordering, and the announcement still lands after the JSONL header — pi writes the
/// header at print-mode.ts:112-118 and only then `await rebindSession()` at :119.
#[tokio::test]
async fn json_dispatch_announces_after_the_header_and_after_post_build_configuration() {
    let (runtime, slot, seen, _cwd, _agent) = unannounced_runtime_with_probe(vec![
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ])
    .await;

    let session = runtime.session().await;
    let _ = slot.set(Arc::clone(&session));
    session.set_session_name("configured-by-cli").await.unwrap();
    session.set_scoped_models(
        session
            .model_catalog()
            .into_iter()
            .take(1)
            .map(|model| cyrup_session_svc::ScopedModel { model, thinking_level: None })
            .collect(),
    );
    let scoped = session.scoped_models().len();

    let mut buf: Vec<u8> = Vec::new();
    run_json_dispatch(&runtime, &text("hi", &[]), &mut buf).await.unwrap();

    assert_eq!(
        seen.lock().unwrap().clone(),
        vec![format!("name={:?} scoped_models={scoped}", Some("configured-by-cli".to_string()))],
        "JSON must announce the session only after --name/--models are applied"
    );

    let text_out = String::from_utf8(buf).unwrap();
    let first = text_out.lines().next().unwrap_or_default();
    let header: serde_json::Value = serde_json::from_str(first).unwrap();
    assert_eq!(
        header.get("type").and_then(serde_json::Value::as_str),
        Some("session"),
        "the JSONL header is still written FIRST, ahead of the bind (Pi print-mode.ts:112-118 → :119); \
         first line was {first}"
    );
}
