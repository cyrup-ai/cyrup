//! cyrup-test-support — the workspace's shared deterministic test harness (arch-00 §11; func-00
//! R-00-011/012/013). 1:1 with Pi's test-support layer (spread across `pi/packages/ai`,
//! `pi/packages/coding-agent/test`, `pi/packages/agent/test/harness`, `pi/packages/tui/test`).
//!
//! Capabilities:
//! - **faux/scripted provider** — re-exported [`faux`] (the chunked, abort-aware, prompt-cache,
//!   factory-capable provider core, `ai/src/providers/faux.ts`) + the declarative [`response`]
//!   format and the cycling, context-capturing [`scripted`] stream fn (`coding-agent/test/test-harness.ts`).
//! - **session/agent harness** — [`harness`] builds + drives a wired [`cyrup_session_svc::AgentSession`]
//!   in a temp dir with captured events (`createHarness`/`createHarnessWithExtensions` +
//!   `suite/harness.ts`).
//! - **golden/snapshot recorder** — [`golden`] records + compares normalized event snapshots.
//! - **Pi differential runner** — [`differential`] diffs cyrup's event sequence against a Pi-shaped
//!   expected sequence (R-00-012).
//! - **session JSONL interop** — [`interop`] round-trips Pi-shaped session JSONL through cyrup
//!   (R-00-013).
//! - **fixtures + builders** — [`messages`] message builders, [`tree`] branched-tree builder,
//!   [`tempdir`] RAII temp dirs, [`auth`] real-credential resolution.
//! - **TUI test scaffolding** — [`tui`] ratatui `TestBackend` helpers + a synthetic key driver
//!   (R-00-006).
//!
//! `publish = false` — dev-only.
#![forbid(unsafe_code)]

/// The scripted faux provider core + helpers (re-exported from `cyrup-provider`).
pub use cyrup_provider::faux;

pub mod auth;
pub mod differential;
pub mod golden;
pub mod harness;
pub mod interop;
pub mod messages;
pub mod response;
pub mod scripted;
pub mod tempdir;
pub mod tool_ext;
pub mod tree;
pub mod tui;

// ---- ergonomic top-level re-exports (the public API other crates' tests consume) ----

pub use auth::{
    api_key, get_real_auth_store, has_api_key, has_auth_for_provider, real_agent_dir,
    real_auth_path, resolve_api_key, resolve_api_key_refreshing, resolve_api_key_refreshing_in,
};
pub use differential::{
    agent_loop_kinds, assert_event_kinds, canonical_event, canonicalize_cross_impl,
    diff_normalized, diff_sequences, event_kind_sequence, pi_fixture_events, run_differential,
    stream_event_type_sequence, value_type_sequence,
};
pub use golden::{normalize_value, snapshot, verify as verify_golden, verify_snapshot};
pub use harness::{
    Harness, HarnessError, HarnessOptions, TestSession, TestSessionOptions, create_harness,
    create_harness_with_extensions, create_test_session, message_text,
};
pub use interop::{InteropError, assert_jsonl_roundtrip, import_export};
pub use messages::{assistant_msg, create_assistant_message, create_user_message, user_msg};
pub use response::{
    FauxResponse, FauxToolCall, ModelOverride, UsageOverride, build_assistant_message, build_usage,
    faux_model, faux_model_from_def, faux_model_with_context_window,
};
pub use scripted::{
    FauxStreamFnState, ScriptedProvider, create_faux_stream_fn, create_faux_stream_fn_queued,
    create_faux_stream_fn_with_model, create_faux_stream_fn_with_models,
};
// Re-export the declarative multi-model definition (Pi `FauxModelDefinition`) so harness callers can
// build `HarnessOptions::models` without reaching into `cyrup_provider::faux`.
pub use cyrup_provider::faux::FauxModelDefinition;
pub use tempdir::TestTempDir;
pub use tool_ext::{SyntheticCall, SyntheticTool, ToolExtension};
pub use tree::{TreeError, TreeMessage, TreeRole, TreeStructure, build_test_tree};

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod smoke {
    use super::*;
    use cyrup_provider::StreamEvent;

    /// The scripted provider streams a declarative response and captures the request context
    /// (`createFauxStreamFn` parity).
    #[tokio::test]
    async fn scripted_provider_streams_and_captures() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let (provider, state) = create_faux_stream_fn(vec![FauxResponse::text("hello")]);
        let model = response::faux_model();
        let stream = provider.stream(&model, &Context::default(), &StreamOptions::default());
        let msg = cyrup_provider::collect_message(stream).await;
        assert_eq!(msg.stop_reason, cyrup_core::StopReason::Stop);
        assert_eq!(msg.content, vec![cyrup_core::Content::text("hello")]);
        let st = state.lock().unwrap();
        assert_eq!(st.call_count, 1);
        assert_eq!(st.contexts.len(), 1);
    }

    /// The scripted provider cycles responses with wrap-around (Pi `callCount % len`).
    #[tokio::test]
    async fn scripted_provider_cycles() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let (provider, _state) =
            create_faux_stream_fn(vec![FauxResponse::text("a"), FauxResponse::text("b")]);
        let model = response::faux_model();
        let texts: Vec<String> = {
            let mut out = Vec::new();
            for _ in 0..3 {
                let s = provider.stream(&model, &Context::default(), &StreamOptions::default());
                let m = cyrup_provider::collect_message(s).await;
                out.push(message_text(&cyrup_core::Message::Assistant(m)));
            }
            out
        };
        assert_eq!(texts, vec!["a", "b", "a"]);
    }

    /// `Harness::run` returns only once the session is IDLE, not merely once the run stream has
    /// ended. On an unbound session the run-scoped stream is closed inside the agent's `agent_end`
    /// emit (`cyrup-session-svc/src/subscriber.rs` `end_run`), which is BEFORE the agent's own
    /// `SettlementGuard::drop` releases its `running` latch — so a caller that prompts again the
    /// instant the stream ends can be refused with `StreamingNeedsBehavior` (`session/run.rs`
    /// `prompt`, via `is_run_active`). pi's multi-turn tests never do that: every one of them
    /// does `await session.prompt(...)` and then `await session.waitForIdle()`
    /// (`core/agent-session.ts:1626` @v0.84.4). The window is nanoseconds unloaded and
    /// milliseconds under load (`cyrup-agent/src/tests/settlement_latch.rs`), which is exactly how
    /// it surfaced: one two-turn seam test lost it once in a loaded full-suite run and passed 3/3
    /// re-run alone. Many turns on the multi-thread runtime widen the odds; the assertion is the
    /// guarantee `run` now makes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn run_returns_only_once_the_session_is_idle() {
        let harness = create_harness(HarnessOptions::with_responses(vec![FauxResponse::text(
            "done",
        )]))
        .await
        .expect("build harness");
        for turn in 0..40 {
            harness.run(format!("turn {turn}")).await.expect("run");
            assert!(
                harness.session().is_idle(),
                "turn {turn}: `run` returned while the session still reported a run in flight"
            );
        }
    }

    /// The full session harness drives a turn and captures the ordered event sequence.
    #[tokio::test]
    async fn harness_runs_a_turn_and_captures_events() {
        let harness = create_harness(HarnessOptions::with_responses(vec![FauxResponse::text(
            "the answer is 42",
        )]))
        .await
        .expect("build harness");

        let events = harness.run("what is the answer?").await.expect("run");
        let kinds = event_kind_sequence(&events);
        assert_eq!(kinds.first().map(String::as_str), Some("agent_start"));
        // SEAM-005: the session-layer stream closes with `agent_settled` (the WHOLE run is done),
        // immediately after the run's last `agent_end`.
        assert_eq!(kinds.last().map(String::as_str), Some("agent_settled"));
        assert_eq!(
            kinds.iter().rev().nth(1).map(String::as_str),
            Some("agent_end")
        );
        assert!(kinds.iter().any(|k| k == "message_start"));
        assert!(kinds.iter().any(|k| k == "message_end"));

        // The faux provider saw exactly one call.
        assert_eq!(harness.faux().call_count, 1);

        // The assistant text reached the transcript.
        let texts = harness.assistant_texts().await;
        assert!(
            texts.iter().any(|t| t.contains("the answer is 42")),
            "got {texts:?}"
        );
    }

    /// Differential runner: the harness's emitted kinds match an expected Pi-shaped sequence.
    #[tokio::test]
    async fn differential_runner_matches_expected_kinds() {
        let harness = create_harness(HarnessOptions::with_responses(vec![FauxResponse::text(
            "hi",
        )]))
        .await
        .expect("build harness");
        let events = harness.run("hello").await.expect("run");
        let kinds = event_kind_sequence(&events);
        // Diff against itself ⇒ no diff.
        assert!(diff_sequences(&kinds, &kinds).is_none());
        // A perturbed expectation ⇒ a diff is reported.
        let mut wrong = kinds.clone();
        wrong.push("bogus".to_string());
        assert!(assert_event_kinds(&wrong, &kinds).is_err());
    }

    /// Golden recorder: first write seeds the golden, second compare matches.
    #[test]
    fn golden_records_then_matches() {
        let dir = TestTempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");
        let events = vec![
            StreamEvent::Start {
                partial: faux_partial(),
            },
            StreamEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
                partial: faux_partial(),
            },
        ];
        // Seed.
        verify_golden(&path, &snapshot(&events)).unwrap();
        // Re-verify ⇒ match.
        verify_golden(&path, &snapshot(&events)).unwrap();
        // A change ⇒ a unified diff error.
        let changed = vec![StreamEvent::Start {
            partial: faux_partial(),
        }];
        assert!(verify_golden(&path, &snapshot(&changed)).is_err());
    }

    /// JSONL interop: a Pi-shaped session round-trips through cyrup with entry equality.
    #[test]
    fn jsonl_interop_round_trips() {
        // Build a real session, export it, then assert it round-trips (a Pi-shaped fixture has the
        // same JSONL shape — header line + entry lines).
        use cyrup_session::manager::{NewSessionOpts, SessionManager};
        let dir = TestTempDir::new().unwrap();
        let mut mgr = SessionManager::in_memory(dir.path(), NewSessionOpts::default()).unwrap();
        mgr.append_message(user_msg("hello")).unwrap();
        mgr.append_message(assistant_msg("hi there")).unwrap();
        let mut buf = Vec::new();
        mgr.export_jsonl(&mut buf).unwrap();
        let jsonl = String::from_utf8(buf).unwrap();
        let exported = assert_jsonl_roundtrip(&jsonl).expect("round-trip");
        assert!(exported.contains("hello"));
    }

    /// Branched-tree builder produces a navigable tree with a text→id map.
    #[test]
    fn tree_builder_branches() {
        use cyrup_session::manager::{NewSessionOpts, SessionManager};
        let dir = TestTempDir::new().unwrap();
        let mut mgr = SessionManager::in_memory(dir.path(), NewSessionOpts::default()).unwrap();
        let structure = TreeStructure::new(vec![
            TreeMessage::user("u1"),
            TreeMessage::assistant("a1"),
            TreeMessage::user("u2"),
            // Branch back to a1 and append a divergent user turn.
            TreeMessage::user("u3").branch_from("a1"),
        ]);
        let ids = build_test_tree(&mut mgr, &structure).expect("build tree");
        assert_eq!(ids.len(), 4);
        assert!(ids.contains_key("a1"));
        // Branching from an unknown ref errors (Pi parity).
        let bad = TreeStructure::new(vec![TreeMessage::user("x").branch_from("nope")]);
        assert!(build_test_tree(&mut mgr, &bad).is_err());
    }

    /// TUI TestBackend renders a widget into a snapshot-able grid; key driver builds events.
    #[test]
    fn tui_backend_renders_and_keys() {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        use ratatui::widgets::Paragraph;
        let mut term = tui::TestTerminal::new(10, 1);
        term.draw(|frame| {
            frame.render_widget(Paragraph::new("hello"), frame.area());
        });
        let snap = term.snapshot();
        assert!(snap.starts_with("hello"), "got {snap:?}");

        let keys = tui::type_string("ab");
        assert_eq!(keys.len(), 2);
        let ctrl_c = tui::key_with(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(ctrl_c.modifiers, KeyModifiers::CONTROL);
    }

    /// `create_test_session` wires an AgentSession over a caller-supplied provider (Pi
    /// `createTestSession`). Driven here by the faux provider standing in for a real one.
    #[tokio::test]
    async fn create_test_session_builds_over_a_provider() {
        use std::sync::Arc;
        let provider = Arc::new(scripted::ScriptedProvider::new(vec![FauxResponse::text(
            "hi",
        )]));
        let ts = create_test_session(provider, TestSessionOptions::default())
            .await
            .expect("build test session");
        // The session resolved a model and can report its current address.
        assert_eq!(
            ts.session()
                .model()
                .expect("session must have a resolved model")
                .provider
                .as_str(),
            "faux"
        );
    }

    /// Harness threads a model `contextWindow` override (Pi `HarnessOptions.contextWindow`) onto the
    /// resolved session model — making compaction-threshold scenarios reproducible.
    #[tokio::test]
    async fn harness_threads_context_window_override() {
        let opts = HarnessOptions {
            context_window: Some(2_048),
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        assert_eq!(
            harness
                .session()
                .services()
                .model
                .as_ref()
                .expect("session must have a resolved model")
                .context_window,
            2_048
        );
        // Default (no override) keeps the faux model's 128000.
        let dflt = create_harness(HarnessOptions::default())
            .await
            .expect("build harness");
        assert_eq!(
            dflt.session()
                .services()
                .model
                .as_ref()
                .expect("session must have a resolved model")
                .context_window,
            128_000
        );
    }

    /// Harness threads CLI settings overrides (Pi `HarnessOptions.settings`: retry/compaction/etc.)
    /// through `SessionBuilder::cli_settings` into the effective settings.
    #[tokio::test]
    async fn harness_threads_settings_override() {
        let mut settings = cyrup_config::Settings::new();
        settings
            .set_field(
                "retry",
                serde_json::json!({ "enabled": true, "maxRetries": 7 }),
            )
            .expect("set retry");
        let opts = HarnessOptions {
            settings,
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        let retry = harness
            .session()
            .services()
            .settings
            .effective()
            .get("retry")
            .cloned()
            .expect("retry present");
        assert_eq!(retry["maxRetries"], serde_json::json!(7));
        assert_eq!(retry["enabled"], serde_json::json!(true));
    }

    /// Harness injects a custom tool (Pi `tools`/`baseToolsOverride`) via a synthetic native
    /// extension; the scripted model calls it and the tool records the invocation.
    #[tokio::test]
    async fn harness_injects_and_dispatches_a_custom_tool() {
        use std::sync::Arc;
        let tool = Arc::new(tool_ext::SyntheticTool::new("echo_tool", "tool-ran"));
        let calls = tool.calls_handle();
        let opts = HarnessOptions {
            // First turn: call the tool; second turn (after the tool result): a plain text answer.
            responses: vec![
                FauxResponse::tool_call("echo_tool", serde_json::json!({ "x": 1 })),
                FauxResponse::text("done"),
            ],
            tools: vec![tool],
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        let events = harness.run("use the tool").await.expect("run");
        let kinds = event_kind_sequence(&events);
        assert!(
            kinds.iter().any(|k| k == "tool_execution_start"),
            "got {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| k == "tool_execution_end"),
            "got {kinds:?}"
        );
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].params, serde_json::json!({ "x": 1 }));
    }

    /// Harness reproduces the unauthenticated path (Pi `withConfiguredAuth: false`): no runtime
    /// credential is wired, yet the scripted provider (which needs no auth) still drives a turn.
    #[tokio::test]
    async fn harness_unauthenticated_path_builds_and_runs() {
        let opts = HarnessOptions {
            with_configured_auth: false,
            responses: vec![FauxResponse::text("ok")],
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        // No stored/runtime credential for the faux provider.
        let provider = harness
            .session()
            .model()
            .expect("session must have a resolved model")
            .provider
            .clone();
        assert!(
            harness
                .session()
                .services()
                .auth
                .runtime_api_key(&provider)
                .is_none()
        );
        // The turn still completes (the faux provider requires no auth).
        let events = harness.run("hello").await.expect("run");
        assert_eq!(
            event_kind_sequence(&events).last().map(String::as_str),
            Some("agent_settled")
        );
    }

    /// `initialActiveToolNames` (Pi suite/harness.ts:68,184) seeds the visible/active tool set when
    /// no explicit allow/exclude is given.
    #[tokio::test]
    async fn harness_initial_active_tool_names_gate() {
        let opts = HarnessOptions {
            initial_active_tool_names: Some(vec!["read".to_string()]),
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        // The session builds with only the read tool active; a turn still completes.
        let events = harness.run("hi").await.expect("run");
        assert_eq!(
            event_kind_sequence(&events).first().map(String::as_str),
            Some("agent_start")
        );
    }

    /// Differential anchor (func-00 R-00-012): the harness's emitted session-event ordering for a
    /// plain text turn matches a sequence **captured from RUNNING Pi** — its own `agentLoop()`
    /// (`pi/packages/agent/src/agent-loop.ts`) driven by Pi's own faux core, recorded into
    /// `fixtures/pi/text-turn.pi-captured.events.jsonl` (see
    /// `spec/gap-analysis/fixtures-capture/capture-agentloop-textturn.ts`). The text turn streams
    /// text_start → text_delta → text_end, and the agent re-emits the refreshed partial on a
    /// *distinct* `message_update` per content-block event (Pi agent-loop.ts:319-366), so one text
    /// block yields three `message_update`s — an 11-event sequence Pi and cyrup emit identically.
    ///
    /// This is now a TRUE cross-impl anchor (R-00-012 "identical emitted event sequence: types +
    /// ordering"), not a self-golden. The self-recorded `text-turn.events.jsonl` remains as a
    /// field-level regression golden; this test anchors the *ordering* to real Pi data.
    #[tokio::test]
    async fn differential_matches_pi_captured_fixture() {
        let fixture = include_str!("../fixtures/pi/text-turn.pi-captured.events.jsonl");
        let pi_events = crate::differential::pi_fixture_events(fixture);
        let expected = crate::differential::value_type_sequence(&pi_events);
        assert_eq!(expected.len(), 11, "Pi-captured fixture event count");

        let harness = create_harness(HarnessOptions::with_responses(vec![FauxResponse::text(
            "hi",
        )]))
        .await
        .expect("build harness");
        let events = harness.run("hello").await.expect("run");
        // The fixture is a capture of Pi's own `agentLoop()` (see its `_note`), so it contains only
        // agent-loop events. cyrup's harness records SESSION events, whose superset includes
        // `agent_settled` (SEAM-005) — a session-layer event Pi emits from `AgentSession`
        // (agent-session.ts:581-588), never from the loop. Compare the agent-loop subset; padding
        // the EXPECTED side would falsely assert Pi's loop emits it.
        let actual = crate::differential::agent_loop_kinds(&event_kind_sequence(&events));
        assert_event_kinds(&expected, &actual).expect(
            "cyrup text-turn ordering diverges from the REAL Pi-captured agent-loop sequence",
        );
    }

    /// The faux core tool-call id matches Pi's `tool:<ts>:<rand>` shape (deterministic [CYRUP-DELTA]).
    #[test]
    fn faux_tool_call_id_matches_pi_shape() {
        use cyrup_core::Content;
        let tc = faux::faux_tool_call("echo", serde_json::json!({ "a": 1 }));
        match tc {
            Content::ToolCall(call) => {
                let id = call.id.as_str();
                assert!(id.starts_with("tool:"), "got {id}");
                assert_eq!(id.split(':').count(), 3, "tool:<ts>:<rand> shape, got {id}");
            }
            _ => panic!("expected tool call"),
        }
        // Explicit id is honored (Pi `options.id`).
        let tc2 =
            faux::faux_tool_call_with_id("echo", serde_json::json!({}), Some("custom-id".into()));
        match tc2 {
            Content::ToolCall(call) => assert_eq!(call.id.as_str(), "custom-id"),
            _ => panic!("expected tool call"),
        }
    }

    /// Message builders carry Pi's exact default usage/cost shapes.
    #[test]
    fn message_builders_match_pi_defaults() {
        match assistant_msg("x") {
            cyrup_core::Message::Assistant(a) => {
                assert_eq!(a.usage.input, 1);
                assert_eq!(a.usage.output, 1);
                assert_eq!(a.usage.total_tokens, 2);
            }
            _ => panic!("expected assistant"),
        }
        assert!(!create_user_message("hi").is_assistant());
    }

    /// Multi-model harness (Pi `models?: FauxModelDefinition[]` + `harness.models`/`getModel(id)`,
    /// suite/harness.ts:64,82-84,201-202): the scripted provider advertises >1 model, the first is
    /// the default, and the rest are reachable by id — and a turn still drives end-to-end.
    #[tokio::test]
    async fn harness_advertises_multiple_models_and_looks_up_by_id() {
        let opts = HarnessOptions {
            models: vec![
                FauxModelDefinition::new("m-a"),
                FauxModelDefinition::new("m-b"),
            ],
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        assert_eq!(harness.models().len(), 2);
        assert_eq!(harness.model().map(|m| m.id.as_str()), Some("m-a"));
        assert!(harness.get_model("m-b").is_some());
        assert!(harness.get_model("nope").is_none());
        // The session resolves the default (first) model, and a turn completes.
        assert_eq!(
            harness
                .session()
                .services()
                .model
                .as_ref()
                .expect("session must have a resolved model")
                .id
                .as_str(),
            "m-a"
        );
        let events = harness.run("hi").await.expect("run");
        assert_eq!(
            event_kind_sequence(&events).last().map(String::as_str),
            Some("agent_settled")
        );
    }

    /// Queue-consuming harness flavour (Pi `registerFauxProvider`/`suite/harness.ts`): responses are
    /// consumed in order, `appendResponses` extends the queue, `getPendingResponseCount` reports the
    /// remainder.
    #[tokio::test]
    async fn harness_queue_mode_consumes_and_appends() {
        let opts = HarnessOptions {
            queue_responses: true,
            responses: vec![FauxResponse::text("first")],
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        assert_eq!(harness.pending_count(), 1);
        harness.run("q1").await.expect("run");
        assert_eq!(harness.pending_count(), 0);
        harness.append_responses(vec![FauxResponse::text("second")]);
        assert_eq!(harness.pending_count(), 1);
        harness.run("q2").await.expect("run");
        let texts = harness.assistant_texts().await;
        assert!(texts.iter().any(|t| t.contains("first")), "got {texts:?}");
        assert!(texts.iter().any(|t| t.contains("second")), "got {texts:?}");
    }

    /// Queue exhaustion (Pi faux.ts:451-461): once the queue drains, further calls stream the
    /// `"No more faux responses queued"` error terminal.
    #[tokio::test]
    async fn scripted_queue_mode_exhaustion_errors() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let (provider, _state) =
            create_faux_stream_fn_queued(vec![FauxResponse::text("only")], vec![faux_model()]);
        assert_eq!(provider.pending_count(), 1);
        let model = faux_model();
        let m1 = cyrup_provider::collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(m1.content, vec![cyrup_core::Content::text("only")]);
        assert_eq!(provider.pending_count(), 0);
        let m2 = cyrup_provider::collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(m2.stop_reason, cyrup_core::StopReason::Error);
        assert_eq!(
            m2.error_message.as_deref(),
            Some("No more faux responses queued")
        );
    }

    /// Queue-exhaustion EVENT ORDERING — byte/behaviour-diff anchor against Pi faux.ts:451-461.
    ///
    /// Pi handles the no-step (empty-queue) case OUT-OF-BAND of `streamWithDeltas`:
    /// `outer.push({ type: "error", reason: "error", error: message }); outer.end(message); return;`
    /// — it pushes a SINGLE `error` event and never emits a leading `start`. So Pi's exhaustion
    /// event sequence is exactly `[error]`. The prior cyrup routed exhaustion through
    /// `faux_event_stream`, which unconditionally prepends `StreamEvent::Start` (faux.rs:408),
    /// yielding `[start, error]` — a silent divergence the `*_errors` test above missed (it only
    /// asserts the collected terminal message, which is identical either way). This pins the exact
    /// event-kind sequence so the divergence cannot regress.
    #[tokio::test]
    async fn scripted_queue_exhaustion_emits_error_only_no_start() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        // A queue that starts empty ⇒ the very first call is already the no-step path.
        let (provider, _state) = create_faux_stream_fn_queued(vec![], vec![faux_model()]);
        let events = drain(provider.stream(
            &faux_model(),
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        // Pi emits exactly one event for exhaustion (faux.ts:451-461): the `error` terminal. No
        // `start`. This is the load-bearing byte-diff: `events.len() == 1`, kind == error.
        assert_eq!(
            events.len(),
            1,
            "Pi exhaustion emits [error] only (faux.ts:451-461); got {events:?}"
        );
        match &events[0] {
            StreamEvent::Error { reason, error } => {
                assert_eq!(*reason, cyrup_provider::stream::ErrorReason::Error);
                assert_eq!(error.stop_reason, cyrup_core::StopReason::Error);
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("No more faux responses queued")
                );
            }
            other => panic!("expected a lone Error terminal (Pi faux.ts:451-461), got {other:?}"),
        }
        // Explicit guard against the [start, error] regression.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::Start { .. })),
            "exhaustion must not emit a leading `start` (Pi pushes only the `error` event)"
        );
    }

    /// Byte-diff vs Pi `withUsageEstimate` for the queue-exhaustion terminal (faux.ts:451-461 →
    /// 213-251). Pi stamps the error message (content `[]`) with `withUsageEstimate`, NOT the fixed
    /// `buildUsage` defaults. For a single-user-message context the expected usage is derived from
    /// `serializeContext` = `"user:hello"` (10 chars) ⇒ `input = ceil(10/4) = 3`, `output = 0`
    /// (`assistantContentToText([]) === ""`), `cacheRead = cacheWrite = 0`, `totalTokens = 3`. The old
    /// path stamped the fixed `input:100/output:50/total:150`.
    #[tokio::test]
    async fn scripted_queue_exhaustion_usage_matches_withusageestimate() {
        use crate::user_msg;
        use cyrup_core::{Cost, Usage};
        use cyrup_provider::{Context, Provider, StreamOptions};

        // Empty queue ⇒ the first call is the no-step exhaustion path.
        let (provider, _state) = create_faux_stream_fn_queued(vec![], vec![faux_model()]);
        let ctx = Context {
            messages: vec![user_msg("hello")],
            ..Default::default()
        };
        let m = cyrup_provider::collect_message(provider.stream(
            &faux_model(),
            &ctx,
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(m.stop_reason, cyrup_core::StopReason::Error);
        assert_eq!(
            m.error_message.as_deref(),
            Some("No more faux responses queued")
        );
        // Re-derived expected bytes from Pi's withUsageEstimate (faux.ts:235-247).
        let expected = Usage {
            input: 3,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 3,
            cost: Cost::default(),
        };
        assert_eq!(
            m.usage, expected,
            "exhaustion usage must match Pi withUsageEstimate"
        );
    }

    /// Arbitrary `Model` override (Pi `createHarness({ model })`, test-harness.ts:324,369-370): an
    /// injected model with non-default modalities/reasoning/window flows onto the resolved session
    /// model.
    #[tokio::test]
    async fn harness_injects_arbitrary_model_override() {
        use cyrup_provider::{Modality, Model, ModelCost};
        let custom = Model {
            id: "custom-1".into(),
            name: "Custom".into(),
            api: cyrup_core::ApiId::from("anthropic-messages"),
            provider: cyrup_core::ProviderId::from("faux"),
            base_url: "http://localhost:0".into(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 4_096,
            max_tokens: 1_024,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        };
        let opts = HarnessOptions {
            model: Some(custom),
            ..Default::default()
        };
        let harness = create_harness(opts).await.expect("build harness");
        assert_eq!(harness.model().map(|m| m.id.as_str()), Some("custom-1"));
        let resolved = harness
            .session()
            .services()
            .model
            .clone()
            .expect("session must have a resolved model");
        assert_eq!(resolved.id.as_str(), "custom-1");
        assert_eq!(resolved.context_window, 4_096);
        assert!(resolved.reasoning);
        assert_eq!(resolved.input, vec![Modality::Text]);
    }

    /// Faux-core async factory step (Pi async `FauxResponseFactory`, faux.ts:96-101,463-464): an
    /// `.await`-ing factory resolves lazily and sees the call state + resolved `StreamOptions`.
    #[tokio::test]
    async fn faux_async_factory_is_driven_through_reexport() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let provider = faux::FauxProvider::new();
        provider.set_response_steps(vec![faux::FauxResponseStep::async_factory(
            |_ctx, opts, state, _model| async move {
                tokio::task::yield_now().await;
                let sid = opts
                    .session_id
                    .as_ref()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default();
                faux::faux_assistant_message(
                    vec![faux::faux_text(format!("c{}-sid{sid}", state.call_count))],
                    cyrup_core::StopReason::Stop,
                )
            },
        )]);
        let model = provider.model().clone();
        let opts = StreamOptions {
            session_id: Some(cyrup_core::SessionId::from("z9")),
            ..Default::default()
        };
        let msg =
            cyrup_provider::collect_message(provider.stream(&model, &Context::default(), &opts))
                .await;
        assert_eq!(msg.content, vec![cyrup_core::Content::text("c1-sidz9")]);
    }

    /// `API_KEY` skip constant (Pi utilities.ts:26): `ANTHROPIC_OAUTH_TOKEN` is preferred over
    /// `ANTHROPIC_API_KEY`; `has_api_key` agrees with `api_key().is_some()`.
    #[test]
    fn api_key_skip_constant_matches_env_derivation() {
        let expected = std::env::var("ANTHROPIC_OAUTH_TOKEN")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("ANTHROPIC_API_KEY")
                    .ok()
                    .filter(|s| !s.is_empty())
            });
        assert_eq!(api_key(), expected);
        assert_eq!(has_api_key(), expected.is_some());
    }

    // ---- gap 9: committed field-level Pi-shaped stream-event goldens ----
    //
    // The `StreamEvent` serde shape is byte-1:1 with Pi's `AssistantMessageEvent` (stream.rs:144-151:
    // identical `type` tags + camelCase payload fields), so a normalized recording of the faux
    // provider's stream IS a Pi-shaped golden. Each fixture drives a *minimal, deterministic* content
    // shape (seeded chunk PRNG ⇒ stable across runs) so the golden is field-level stable, extending
    // the lone `text-turn` anchor to thinking / tool-call / error / multi-turn flows. On first run a
    // missing golden is written into the source tree (golden recorder); thereafter it is asserted.

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("pi")
            .join(name)
    }

    async fn drain(stream: cyrup_core::EventStream<StreamEvent>) -> Vec<StreamEvent> {
        use futures::StreamExt;
        let mut s = stream;
        let mut out = Vec::new();
        while let Some(ev) = s.next().await {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn pi_shaped_thinking_stream_golden() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let provider = faux::FauxProvider::new();
        provider.set_responses(vec![faux::faux_assistant_message(
            vec![faux::faux_thinking("ok")],
            cyrup_core::StopReason::Stop,
        )]);
        let model = provider.model().clone();
        let events =
            drain(provider.stream(&model, &Context::default(), &StreamOptions::default())).await;
        verify_golden(
            fixture_path("thinking-stream.events.jsonl"),
            &snapshot(&events),
        )
        .expect("thinking-stream golden");
        let types = stream_event_type_sequence(&events);
        assert_eq!(types.first().map(String::as_str), Some("start"));
        assert!(types.iter().any(|t| t == "thinking_start"));
        assert!(types.iter().any(|t| t == "thinking_delta"));
        assert!(types.iter().any(|t| t == "thinking_end"));
        assert_eq!(types.last().map(String::as_str), Some("done"));
    }

    /// TRUE cross-impl anchor (R-00-012): cyrup's live faux `thinking` stream is diffed against a
    /// fixture **captured from running Pi's own faux core** (node executing
    /// `pi/packages/ai/src/providers/faux.ts`; see `fixtures/pi/thinking-stream.pi-captured.events.jsonl`
    /// + `spec/gap-analysis/fixtures-capture/capture-faux-thinking.ts`). Asserts (1) identical emitted
    /// event **type + ordering** (R-00-012's exact contract) and (2) field-level equality of the
    /// terminal `done` message after folding the two documented Pi<->cyrup representation deltas
    /// (`role` type-encoding; integer-vs-`f64` numbers) via `canonical_event`. The intermediate
    /// `partial` snapshots intentionally diverge: Pi's shallow `{...partial}` clone aliases the nested
    /// `content` array (faux.ts:338,347) so its `thinking_start` partial shows the FINAL `"ok"`,
    /// whereas cyrup's value-semantics snapshot correctly shows `""` — a Pi quirk cyrup does not
    /// reproduce, so the field-level anchor is the unambiguous terminal message.
    #[tokio::test]
    async fn pi_captured_thinking_stream_is_true_cross_impl_anchor() {
        use cyrup_provider::{Context, Provider, StreamOptions};

        // The real-Pi capture (skips the `{"_note":…}` provenance line).
        let jsonl =
            std::fs::read_to_string(fixture_path("thinking-stream.pi-captured.events.jsonl"))
                .expect("read Pi-captured fixture");
        let pi_events = crate::differential::pi_fixture_events(&jsonl);
        assert!(
            !pi_events.is_empty(),
            "Pi-captured fixture has no typed events"
        );

        // cyrup's live faux stream for the identical scenario.
        let provider = faux::FauxProvider::new();
        provider.set_responses(vec![faux::faux_assistant_message(
            vec![faux::faux_thinking("ok")],
            cyrup_core::StopReason::Stop,
        )]);
        let model = provider.model().clone();
        let cy_events =
            drain(provider.stream(&model, &Context::default(), &StreamOptions::default())).await;

        // (1) type + ordering — R-00-012's exact contract, fully cross-impl.
        let pi_types = crate::differential::value_type_sequence(&pi_events);
        let cy_types = stream_event_type_sequence(&cy_events);
        assert_eq!(
            pi_types, cy_types,
            "Pi vs cyrup event type+ordering mismatch"
        );
        assert_eq!(pi_types.first().map(String::as_str), Some("start"));
        assert_eq!(pi_types.last().map(String::as_str), Some("done"));

        // (2) terminal `done` message — field-level, modulo the two documented deltas.
        let pi_done = pi_events
            .iter()
            .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("done"))
            .cloned()
            .expect("Pi capture has a done event");
        let cy_done = cy_events
            .iter()
            .find(|e| {
                serde_json::to_value(e)
                    .ok()
                    .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
                    .as_deref()
                    == Some("done")
            })
            .map(|e| serde_json::to_value(e).expect("serialize cyrup done"))
            .expect("cyrup has a done event");
        assert_eq!(
            crate::differential::canonical_event(pi_done),
            crate::differential::canonical_event(cy_done),
            "Pi vs cyrup terminal `done` message diverges beyond the documented role/number deltas",
        );
    }

    #[tokio::test]
    async fn pi_shaped_tool_call_stream_golden() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let provider = faux::FauxProvider::new();
        provider.set_responses(vec![faux::faux_assistant_message(
            vec![faux::faux_tool_call_with_id(
                "echo",
                serde_json::json!({ "a": 1 }),
                Some("tc-1".into()),
            )],
            cyrup_core::StopReason::ToolUse,
        )]);
        let model = provider.model().clone();
        let events =
            drain(provider.stream(&model, &Context::default(), &StreamOptions::default())).await;
        verify_golden(
            fixture_path("tool-call-stream.events.jsonl"),
            &snapshot(&events),
        )
        .expect("tool-call-stream golden");
        let types = stream_event_type_sequence(&events);
        assert_eq!(types.first().map(String::as_str), Some("start"));
        assert!(types.iter().any(|t| t == "toolcall_start"));
        assert!(types.iter().any(|t| t == "toolcall_end"));
        assert_eq!(types.last().map(String::as_str), Some("done"));
    }

    #[tokio::test]
    async fn pi_shaped_error_stream_golden() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let provider = faux::FauxProvider::new();
        provider.set_responses(vec![faux::faux_assistant_message_with(
            Vec::new(),
            cyrup_core::StopReason::Error,
            faux::FauxMessageOptions {
                error_message: Some("boom".into()),
                ..Default::default()
            },
        )]);
        let model = provider.model().clone();
        let events =
            drain(provider.stream(&model, &Context::default(), &StreamOptions::default())).await;
        verify_golden(
            fixture_path("error-stream.events.jsonl"),
            &snapshot(&events),
        )
        .expect("error-stream golden");
        let types = stream_event_type_sequence(&events);
        assert_eq!(types.first().map(String::as_str), Some("start"));
        assert_eq!(types.last().map(String::as_str), Some("error"));
    }

    #[tokio::test]
    async fn pi_shaped_multi_turn_stream_golden() {
        use cyrup_provider::{Context, Provider, StreamOptions};
        let provider = faux::FauxProvider::new();
        provider.set_responses(vec![
            faux::faux_assistant_message(vec![faux::faux_text("hi")], cyrup_core::StopReason::Stop),
            faux::faux_assistant_message(vec![faux::faux_text("yo")], cyrup_core::StopReason::Stop),
        ]);
        let model = provider.model().clone();
        let mut events =
            drain(provider.stream(&model, &Context::default(), &StreamOptions::default())).await;
        events.extend(
            drain(provider.stream(&model, &Context::default(), &StreamOptions::default())).await,
        );
        verify_golden(
            fixture_path("multi-turn-stream.events.jsonl"),
            &snapshot(&events),
        )
        .expect("multi-turn-stream golden");
        // Two complete turns: two `start` and two `done` terminals.
        let types = stream_event_type_sequence(&events);
        assert_eq!(types.iter().filter(|t| *t == "start").count(), 2);
        assert_eq!(types.iter().filter(|t| *t == "done").count(), 2);
    }

    fn faux_partial() -> std::sync::Arc<cyrup_core::AssistantMessage> {
        std::sync::Arc::new(cyrup_core::AssistantMessage {
            content: Vec::new(),
            provider: cyrup_core::ProviderId::from("faux"),
            model: "faux-1".into(),
            api: cyrup_core::ApiId::from("faux"),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: cyrup_core::Usage::default(),
            stop_reason: cyrup_core::StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        })
    }
}

#[cfg(test)]
mod tests;
