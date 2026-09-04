//! Host-target unit tests for the ergonomic guest layer (arch-08 §3.6). These exercise the same
//! `ExtensionApi` routing the wasm `guest` glue uses — subscription bitset, typed event dispatch,
//! outcome lowering, and guest-tool execution — WITHOUT a wasm runtime (the `Ctx` capability calls
//! are inert stubs on the host target).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::RawOutcome;
use crate::prelude::*;
use serde_json::json;

#[test]
fn example_subscribes_to_tool_call_and_agent_start() {
    let api = crate::example::build();
    let kinds = api.subscription_kinds();
    assert!(kinds.contains(&0), "tool_call (kind 0) subscribed");
    assert!(kinds.contains(&7), "agent_start (kind 7) subscribed");
}

#[test]
fn tool_call_gate_blocks_bash_passes_others() {
    let mut api = ExtensionApi::new();
    api.on_tool_call(|ev, _ctx| {
        if ev.name == "bash" {
            Outcome::block("no bash")
        } else {
            Outcome::noop()
        }
    });
    let ctx = Ctx::new();

    let blocked = api.dispatch(0, &["tc1", "bash", "{}"], &ctx);
    // EXT-049: `block` carries pi's `ToolCallEventResult.terminate`; `Outcome::block`
    // is the non-terminating form (`extensions/types.ts:1072-1079` @v0.84.1).
    assert_eq!(blocked, RawOutcome::Block(Some("no bash".into()), false));

    let passed = api.dispatch(0, &["tc2", "read", "{}"], &ctx);
    assert_eq!(passed, RawOutcome::Noop);
}

#[test]
fn tool_call_mutate_lowers_to_json_patch() {
    let mut api = ExtensionApi::new();
    api.on_tool_call(|ev, _ctx| {
        let mut input = ev.input.clone();
        if let Some(obj) = input.as_object_mut() {
            obj.insert("added".into(), json!(true));
        }
        Outcome::replace_tool_input(input)
    });
    let out = api.dispatch(0, &["id", "x", "{\"a\":1}"], &Ctx::new());
    match out {
        RawOutcome::Mutate(s) => {
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["a"], json!(1));
            assert_eq!(v["added"], json!(true));
        }
        other => panic!("expected Mutate, got {other:?}"),
    }
}

#[test]
fn before_agent_start_dual_result_carries_both_fields() {
    let mut api = ExtensionApi::new();
    api.on_before_agent_start(|_ev, _ctx| {
        Outcome::before_agent_start(BeforeAgentStartResult {
            message: Some(json!({"role": "user", "content": "x"})),
            system_prompt: Some("new prompt".into()),
        })
    });
    let out = api.dispatch(4, &["prompt", "[]", "sys", "{}"], &Ctx::new());
    match out {
        RawOutcome::Mutate(s) => {
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["systemPrompt"], json!("new prompt"));
            assert!(v["message"].is_object());
        }
        other => panic!("expected Mutate, got {other:?}"),
    }
}

#[test]
fn unsubscribed_event_is_noop() {
    let api = ExtensionApi::new();
    assert_eq!(api.dispatch(2, &["[]"], &Ctx::new()), RawOutcome::Noop);
    assert!(api.subscription_kinds().is_empty());
}

#[test]
fn guest_tool_executes_and_streams() {
    let api = crate::example::build();
    let call = ToolCall::new("tc", json!({ "text": "hi" }));
    let out = api.execute_tool("demo_echo", call).expect("tool runs");
    assert!(!out.is_error);
    match &out.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "echo: hi"),
        other => panic!("unexpected content {other:?}"),
    }
    // an unknown tool errors instead of panicking.
    let err = api
        .execute_tool("nope", ToolCall::new("t", json!({})))
        .unwrap_err();
    assert!(err.contains("no such tool"));
}

#[test]
fn guest_command_executes_and_completes() {
    let mut api = ExtensionApi::new();
    api.register_command_with_completions(
        "greet",
        CommandDescriptor::new("greet"),
        |args: &str, _ctx: &CommandCtx| Ok(Some(format!("hi {}", args.trim()))),
        |prefix: &str| {
            ["world", "team"]
                .iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| c.to_string())
                .collect()
        },
    );
    assert_eq!(
        api.execute_command("greet", "bob").unwrap(),
        Some("hi bob".to_string())
    );
    assert_eq!(
        api.argument_completions("greet", "te"),
        vec!["team".to_string()]
    );
    // an unknown command errors rather than panicking.
    assert!(
        api.execute_command("nope", "")
            .unwrap_err()
            .contains("no such command")
    );
}

#[test]
fn static_completions_fall_back_when_no_dynamic_completer() {
    let mut api = ExtensionApi::new();
    let mut desc = CommandDescriptor::new("x");
    desc.completions = vec!["alpha".into(), "beta".into()];
    api.register_command("x", desc, |_a: &str, _c: &CommandCtx| Ok(None));
    assert_eq!(
        api.argument_completions("x", "al"),
        vec!["alpha".to_string()]
    );
}

#[test]
fn message_renderer_renders_call_and_result() {
    struct R;
    impl MessageRenderer for R {
        fn render_call(
            &self,
            call: &serde_json::Value,
            _o: &crate::RenderOptions,
            _c: &Ctx,
        ) -> Option<serde_json::Value> {
            Some(json!({ "kind": "call", "echo": call.clone() }))
        }
        fn render_result(
            &self,
            _r: &serde_json::Value,
            _o: &crate::RenderOptions,
            _c: &Ctx,
        ) -> Option<serde_json::Value> {
            Some(json!({ "kind": "result" }))
        }
    }
    let mut api = ExtensionApi::new();
    api.register_message_renderer("demo", R);
    let opts = crate::RenderOptions::default();
    let call = api.render_call("demo", &json!({ "a": 1 }), &opts).unwrap();
    assert_eq!(call["kind"], json!("call"));
    assert_eq!(call["echo"]["a"], json!(1));
    assert_eq!(
        api.render_result("demo", &json!({}), &opts).unwrap()["kind"],
        json!("result")
    );
    // an unregistered type returns None (default renderer).
    assert!(api.render_call("other", &json!({}), &opts).is_none());
}

/// EXT-006 — the `(options, theme)` half of upstream's renderer signature reaches the renderer, and
/// a renderer that branches on it produces different output for different options. Upstream:
/// `MessageRenderer = (message, options, theme) => Component | undefined`
/// (`pi/packages/coding-agent/src/core/extensions/types.ts:1213-1217` @v0.84.4).
#[test]
fn a_renderer_sees_the_display_options_and_the_theme_name() {
    struct R;
    impl MessageRenderer for R {
        fn render_call(
            &self,
            _call: &serde_json::Value,
            opts: &crate::RenderOptions,
            _c: &Ctx,
        ) -> Option<serde_json::Value> {
            Some(json!({
                "expanded": opts.expanded,
                "outputPad": opts.output_pad,
                "isPartial": opts.is_partial,
                "theme": opts.theme.clone(),
            }))
        }
    }
    let mut api = ExtensionApi::new();
    api.register_message_renderer("demo", R);

    let collapsed = crate::RenderOptions::default();
    let out = api.render_call("demo", &json!({}), &collapsed).unwrap();
    assert_eq!(out["expanded"], json!(false));
    assert_eq!(out["theme"], json!(null));

    let expanded = crate::RenderOptions {
        expanded: true,
        output_pad: 2,
        is_partial: true,
        theme: Some("dark".to_string()),
    };
    let out = api.render_call("demo", &json!({}), &expanded).unwrap();
    assert_eq!(out["expanded"], json!(true));
    assert_eq!(out["outputPad"], json!(2));
    assert_eq!(out["isPartial"], json!(true));
    assert_eq!(out["theme"], json!("dark"));
}

/// The host's `opts-json` parses into the guest mirror, and an absent or malformed bag takes the
/// defaults rather than skipping the render — see [`crate::RenderOptions::from_json`].
#[test]
fn the_hosts_opts_json_parses_into_the_guest_mirror() {
    let parsed = crate::RenderOptions::from_json(&json!({
        "expanded": true,
        "outputPad": 3,
        "isPartial": true,
        "theme": "solarized",
    }));
    assert_eq!(
        parsed,
        crate::RenderOptions {
            expanded: true,
            output_pad: 3,
            is_partial: true,
            theme: Some("solarized".to_string()),
        }
    );
    assert_eq!(
        crate::RenderOptions::from_json(&json!({})),
        crate::RenderOptions::default()
    );
    assert_eq!(
        crate::RenderOptions::from_json(&serde_json::Value::Null),
        crate::RenderOptions::default()
    );
}

#[test]
fn content_block_serializes_like_core_content() {
    let block = ContentBlock::Image {
        data: "AAAA".into(),
        mime_type: "image/png".into(),
    };
    let v = serde_json::to_value(block).unwrap();
    assert_eq!(v["type"], json!("image"));
    assert_eq!(v["mimeType"], json!("image/png"));
}

#[test]
fn provider_oauth_closures_dispatch() {
    use crate::{OAuthProvider, ProviderHandlers};
    let mut api = ExtensionApi::new();
    let oauth = OAuthProvider::new(
        "Demo",
        // host target: OAuthCallbacks.on_prompt returns Err (inert), so the login derives the
        // access from a constant rather than the prompt — still proves the closure dispatches.
        |_cb: &crate::OAuthCallbacks| Ok(json!({ "refresh": "r", "access": "a0", "expires": 0 })),
        |creds: serde_json::Value| {
            let r = creds.get("refresh").and_then(|v| v.as_str()).unwrap_or("r");
            Ok(json!({ "refresh": r, "access": "a1", "expires": 0 }))
        },
        |creds: &serde_json::Value| {
            Ok(creds
                .get("access")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string())
        },
    )
    .with_modify_models(|m: serde_json::Value, _c: &serde_json::Value| Ok(m));

    api.register_provider_with_handlers(
        "demo",
        ProviderConfig {
            name: "demo".into(),
            base_url: None,
            api: None,
            api_key: None,
            auth_header: None,
            headers: Default::default(),
            models: vec![],
            oauth: None,
            has_stream_simple: false,
        },
        ProviderHandlers::new().with_oauth(oauth),
    );

    // The static config auto-filled the oauth marker (so the host knows it has OAuth).
    assert_eq!(api.providers()[0].1.oauth, Some(json!({ "name": "Demo" })));

    let creds = api.provider_login("demo").unwrap();
    assert_eq!(creds["access"], json!("a0"));
    let refreshed = api.provider_refresh_token("demo", creds.clone()).unwrap();
    assert_eq!(refreshed["access"], json!("a1"));
    assert_eq!(api.provider_get_api_key("demo", &creds).unwrap(), "a0");
    let models = json!([{ "id": "m" }]);
    assert_eq!(
        api.provider_modify_models("demo", models.clone(), &creds)
            .unwrap(),
        models
    );
    // A provider without an OAuth handler surfaces an error rather than panicking.
    assert!(
        api.provider_login("missing")
            .unwrap_err()
            .contains("no OAuth handler")
    );
}

#[test]
fn provider_stream_simple_pushes_events() {
    use crate::{ProviderConfig, ProviderHandlers, ProviderStream};
    let mut api = ExtensionApi::new();
    let stream = |model: serde_json::Value,
                  _c: serde_json::Value,
                  _o: serde_json::Value,
                  out: &ProviderStream| {
        out.emit(json!({ "type": "text", "text": model["id"].clone() }));
        Ok(())
    };
    api.register_provider_with_handlers(
        "p",
        ProviderConfig {
            name: "p".into(),
            base_url: None,
            api: None,
            api_key: None,
            auth_header: None,
            headers: Default::default(),
            models: vec![],
            oauth: None,
            has_stream_simple: false,
        },
        ProviderHandlers::new().with_stream_simple(stream),
    );
    // has_stream_simple was auto-set from the handler.
    assert!(api.providers()[0].1.has_stream_simple);
    // On the host target `emit` is inert, but the handler still runs end-to-end (no panic / Ok).
    let st = ProviderStream::new("s1");
    api.provider_stream_simple("p", &st, json!({ "id": "m" }), json!({}), json!({}))
        .unwrap();
}

#[test]
fn autocomplete_provider_stacking_folds_over_base() {
    use crate::{AutocompleteItem, AutocompleteQuery, AutocompleteSuggestions};
    let mut api = ExtensionApi::new();
    api.add_autocomplete_provider(
        |q: &AutocompleteQuery, current: Option<&AutocompleteSuggestions>| {
            let mut items = current.map(|c| c.items.clone()).unwrap_or_default();
            items.push(AutocompleteItem::new("x:one"));
            Some(AutocompleteSuggestions {
                items,
                prefix: q.current_line().to_string(),
            })
        },
    );
    api.add_autocomplete_provider(
        |_q: &AutocompleteQuery, current: Option<&AutocompleteSuggestions>| {
            let mut items = current.map(|c| c.items.clone()).unwrap_or_default();
            items.push(AutocompleteItem::new("y:two"));
            current.map(|c| AutocompleteSuggestions {
                items,
                prefix: c.prefix.clone(),
            })
        },
    );
    assert_eq!(api.autocomplete_provider_count(), 2);

    let base = AutocompleteSuggestions {
        items: vec![AutocompleteItem::new("builtin")],
        prefix: String::new(),
    };
    let query = AutocompleteQuery {
        lines: vec!["he".into()],
        cursor_line: 0,
        cursor_col: 2,
        force: false,
    };
    let folded = api.autocomplete_suggest(Some(base), &query).unwrap();
    let values: Vec<&str> = folded.items.iter().map(|i| i.value.as_str()).collect();
    assert_eq!(
        values,
        vec!["builtin", "x:one", "y:two"],
        "providers stack in registration order"
    );
    assert_eq!(
        folded.prefix, "he",
        "first provider set the prefix from the cursor line"
    );

    // With no base and no providers, suggest yields None.
    let empty = ExtensionApi::new();
    assert!(empty.autocomplete_suggest(None, &query).is_none());
}

#[test]
fn define_tool_factory_bundles_descriptor_and_exec() {
    use crate::tool_factory::{bash_descriptor, define_tool};
    let mut api = ExtensionApi::new();
    let tool = define_tool(bash_descriptor("/work"), |call: ToolCall| {
        let cmd = call
            .params
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(ToolOutput::text(format!("ran: {cmd}")))
    });
    api.register_tool_def(tool);
    let out = api
        .execute_tool("bash", ToolCall::new("t", json!({ "command": "ls" })))
        .unwrap();
    match &out.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "ran: ls"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn dialog_options_and_typed_command_options_serialize() {
    use crate::{DialogOptions, ForkOptions, ForkPosition, NavigateOptions, NewSessionOptions};
    // Dialog option bag (Pi `ExtensionUIDialogOptions { signal?, timeout? }`).
    //
    // EXT-048: the wire key is `timeout`, NOT `timeoutMs` — `timeout?: number` upstream, with no
    // `timeoutMs` spelling anywhere in `packages/coding-agent/src` @v0.83.0. This assertion still
    // pinned the pre-EXT-048 key and was RED at HEAD; it went unnoticed because `cyrup-it` is
    // `required-features = ["it"]` and the merge gate never builds it (residual ledger, structural
    // defect J). Production (`cyrup-ext-sdk/src/descriptor.rs:180`) is the correct side.
    let to = serde_json::to_value(DialogOptions::timeout(5000)).unwrap();
    assert_eq!(
        to["timeout"],
        json!(5000),
        "EXT-048: the wire key is `timeout`"
    );
    assert!(
        to.get("timeoutMs").is_none(),
        "the pre-EXT-048 `timeoutMs` key must NOT be emitted: {to}"
    );
    // The alias survives for bags cyrup's own SDK already wrote (`descriptor.rs:180`), so a stored
    // `timeoutMs` still round-trips into the canonical field.
    let legacy: DialogOptions = serde_json::from_value(json!({ "timeoutMs": 5000 })).unwrap();
    assert_eq!(
        legacy.timeout_ms,
        Some(5000),
        "the `timeoutMs` alias still deserializes"
    );

    let sig = serde_json::to_value(DialogOptions::signal("abort-1")).unwrap();
    assert_eq!(sig["signalId"], json!("abort-1"));

    // Typed command-tier options serialize camelCase (cross the control opts-json seam).
    let fork = serde_json::to_value(ForkOptions {
        position: Some(ForkPosition::Before),
        with_session: true,
    })
    .unwrap();
    assert_eq!(fork["position"], json!("before"));
    assert_eq!(fork["withSession"], json!(true));

    let nav = serde_json::to_value(NavigateOptions {
        summarize: true,
        custom_instructions: Some("focus".into()),
        replace_instructions: false,
        label: Some("checkpoint".into()),
    })
    .unwrap();
    assert_eq!(nav["summarize"], json!(true));
    assert_eq!(nav["customInstructions"], json!("focus"));
    assert_eq!(nav["label"], json!("checkpoint"));

    let ns = serde_json::to_value(NewSessionOptions {
        parent_session: Some("parent-1".into()),
        with_session: true,
    })
    .unwrap();
    assert_eq!(ns["parentSession"], json!("parent-1"));

    // The CommandCtx typed methods are inert on the host target but must dispatch without panic.
    let ctx = CommandCtx::new();
    ctx.fork_with(
        "e1",
        &ForkOptions {
            position: Some(ForkPosition::At),
            with_session: false,
        },
    )
    .unwrap();
    ctx.navigate_with("t1", &NavigateOptions::default())
        .unwrap();
    ctx.new_session_with(&NewSessionOptions::default()).unwrap();
}

#[test]
fn example_registers_oauth_provider_and_autocomplete() {
    let api = crate::example::build();
    // The bundled demo now registers the demo-oauth provider with OAuth + streamSimple.
    let creds = api.provider_login("demo-oauth").err();
    // On host target on_prompt errors (inert), so login surfaces that error — proving the closure ran.
    assert!(
        creds.is_some(),
        "login closure dispatched (host-inert prompt errors)"
    );
    assert!(
        api.autocomplete_provider_count() >= 1,
        "demo stacks an autocomplete provider"
    );
}

#[test]
fn turn_start_decodes_index_and_timestamp() {
    // The host's `on-turn-start(turn-index, timestamp)` export lowers to ordered string args
    // `[turn_index, timestamp]` (Pi `TurnStartEvent`, types.ts:688-693). Prove the guest decodes
    // BOTH — the `timestamp` (Pi `Date.now()`) is a real second field, not dropped.
    use std::cell::Cell;
    use std::rc::Rc;
    let seen: Rc<Cell<(u32, u64)>> = Rc::new(Cell::new((0, 0)));
    let sink = seen.clone();
    let mut api = ExtensionApi::new();
    api.on_turn_start(move |ev, _| sink.set((ev.turn_index, ev.timestamp)));
    api.dispatch(9, &["4", "1700000000000"], &Ctx::new());
    assert_eq!(seen.get(), (4, 1_700_000_000_000));
}

#[test]
fn all_33_event_kinds_are_registerable() {
    // Register one handler per event kind and assert the bitset reports all 33 discriminants
    // (`mod kind`, api.rs:21-64, defines 33).
    let mut api = ExtensionApi::new();
    api.on_tool_call(|_, _| Outcome::noop());
    api.on_tool_result(|_, _| Outcome::noop());
    api.on_context(|_, _| Outcome::noop());
    api.on_message_end(|_, _| Outcome::noop());
    api.on_before_agent_start(|_, _| Outcome::noop());
    api.on_resources_discover(|_, _| Outcome::noop());
    api.on_project_trust(|_, _| Outcome::noop());
    api.on_agent_start(|_| {});
    api.on_agent_end(|_, _| {});
    api.on_turn_start(|_, _| {});
    api.on_turn_end(|_, _| {});
    api.on_message_start(|_, _| {});
    api.on_message_update(|_, _| {});
    api.on_tool_exec_start(|_, _| {});
    api.on_tool_exec_update(|_, _| {});
    api.on_tool_exec_end(|_, _| {});
    api.on_session_start(|_, _| {});
    api.on_session_shutdown(|_, _| {});
    api.on_input(|_, _| Outcome::noop());
    api.on_user_bash(|_, _| Outcome::noop());
    api.on_before_provider_request(|_, _| Outcome::noop());
    api.on_after_provider_response(|_, _| {});
    api.on_model_select(|_, _| {});
    api.on_thinking_level_select(|_, _| {});
    api.on_session_before_switch(|_, _| Outcome::noop());
    api.on_session_before_fork(|_, _| Outcome::noop());
    api.on_session_before_compact(|_, _| Outcome::noop());
    api.on_session_compact(|_, _| {});
    api.on_session_before_tree(|_, _| Outcome::noop());
    api.on_session_tree(|_, _| {});
    // Kinds 30-32, added after this test was written: `agent_settled`
    // (pi `AgentSettledEvent`), `before_provider_headers` (`extensions/types.ts:686-689`
    // @v0.83.0, EXT-009) and `session_info_changed` (`extensions/types.ts:571-575`, EXT-011).
    api.on_agent_settled(|_| {});
    api.on_before_provider_headers(|_, _| Outcome::noop());
    api.on_session_info_changed(|_, _| {});

    let kinds = api.subscription_kinds();
    assert_eq!(
        kinds.len(),
        33,
        "all 33 Pi events registerable, got {kinds:?}"
    );
    assert_eq!(kinds.first(), Some(&0));
    assert_eq!(kinds.last(), Some(&32));
}

#[test]
fn registered_shortcut_handler_actually_runs() {
    // registerShortcut is no longer structurally inert (Pi types.ts:1198-1205): the handler is
    // stored and runs via `execute_shortcut` (the `execute-shortcut` export's routing target).
    use std::cell::Cell;
    use std::rc::Rc;
    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();
    let mut api = ExtensionApi::new();
    api.register_shortcut("ctrl+g", "do the thing", move |_ctx: &Ctx| {
        f.set(true);
        Ok(())
    });

    let ctx = Ctx::new();
    assert!(api.execute_shortcut("ctrl+g", &ctx).is_ok());
    assert!(fired.get(), "the registered shortcut handler ran");
    // An unknown key surfaces an error, never a panic.
    assert!(api.execute_shortcut("ctrl+z", &ctx).is_err());
}

#[test]
fn tool_call_carries_a_cancellation_signal() {
    // The tool `execute` call now bundles a `signal` (Pi `ToolDefinition.execute` signal param,
    // sdk gap #1). On the host target the poll is inert (false); the live wasm E2E proves the real
    // host-backed poll across the boundary.
    let call = ToolCall::new("c1", json!({ "text": "hi" }));
    assert_eq!(call.call_id, "c1");
    assert!(
        !call.signal().is_aborted(),
        "host-target signal poll is inert"
    );
}

#[test]
fn with_session_closure_registers_runs_once_and_is_consumed() {
    use std::cell::Cell;
    use std::rc::Rc;

    // The `withSession` re-binding plumbing (Pi `ReplacedSessionContext`, sdk gap #3): a closure is
    // stored under an id (embedded in the `control.*` opts) and the host runs it via `with-session`.
    let ran = Rc::new(Cell::new(0u32));
    let r2 = ran.clone();
    let id = crate::ctx::register_with_session(Box::new(move |_rsc: &ReplacedSessionContext| {
        r2.set(r2.get() + 1);
        Ok(())
    }));

    // An unknown id is a no-op (never an error).
    assert!(crate::ctx::run_with_session("no-such-id").is_ok());
    assert_eq!(ran.get(), 0);

    // The registered id runs the closure exactly once...
    crate::ctx::run_with_session(&id).expect("withSession closure runs");
    assert_eq!(ran.get(), 1);

    // ...and is consumed: a second invocation is a no-op (the closure does not re-run).
    crate::ctx::run_with_session(&id).expect("consumed id is a no-op");
    assert_eq!(ran.get(), 1);
}

#[test]
fn new_session_with_callback_is_command_tier_and_ok_on_host() {
    // The closure-accepting session ops compile + return Ok on the host target (the control op is
    // inert here; the live E2E proves the host schedules + invokes the `with-session` export).
    let ctx = CommandCtx::new();
    let out = ctx.new_session_with_callback(&NewSessionOptions::default(), |_rsc| Ok(()));
    assert!(out.is_ok());
    let out = ctx.fork_with_callback("e1", &ForkOptions::default(), |_rsc| Ok(()));
    assert!(out.is_ok());
    let out =
        ctx.switch_session_with_callback("s1", &SwitchSessionOptions::default(), |_rsc| Ok(()));
    assert!(out.is_ok());
}

/// AGENT-005 guest half: the host's `on-tool-result(..., details-json, usage-json)` export lowers to
/// ordered string args, and the guest must decode arg 6 into `ToolResultEvent.usage` (Pi
/// `ToolResultEventBase.usage?: Usage`, types.ts:919-921). Before this the parameter did not exist
/// on the WIT function at all, so a guest could not observe tool-reported usage.
#[test]
fn tool_result_decodes_the_usage_argument() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let seen: Rc<RefCell<Option<serde_json::Value>>> = Rc::new(RefCell::new(None));
    let sink = seen.clone();
    let mut api = ExtensionApi::new();
    api.on_tool_result(move |ev, _| {
        *sink.borrow_mut() = ev.usage.clone();
        Outcome::noop()
    });
    api.dispatch(
        1,
        &[
            "c1",
            "bash",
            "{}",
            "[]",
            "false",
            "",
            r#"{"input":11,"output":22}"#,
        ],
        &Ctx::new(),
    );
    assert_eq!(
        seen.borrow().clone(),
        Some(serde_json::json!({ "input": 11, "output": 22 })),
        "the guest decoded the usage argument"
    );
}

/// The absent case (Pi `undefined`): the host lowers `None` to an empty string, which must decode to
/// `None`, not to `Value::Null` or a zeroed object.
#[test]
fn tool_result_decodes_an_absent_usage_argument_as_none() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let seen: Rc<RefCell<Option<serde_json::Value>>> =
        Rc::new(RefCell::new(Some(serde_json::json!("sentinel"))));
    let sink = seen.clone();
    let mut api = ExtensionApi::new();
    api.on_tool_result(move |ev, _| {
        *sink.borrow_mut() = ev.usage.clone();
        Outcome::noop()
    });
    api.dispatch(
        1,
        &["c1", "write", "{}", "[]", "false", "", ""],
        &Ctx::new(),
    );
    assert_eq!(seen.borrow().clone(), None, "empty arg = Pi `undefined`");
}

/// The WRITE direction across the same seam: a guest's `ToolResultPatch` serializes `usage` so the
/// host's `decode_patch` can read it back off the mutate JSON (Pi `ToolResultEventResult.usage`,
/// types.ts:1085-1090). An omitted `usage` must not appear as a `null` key — that would read as an
/// explicit clear on the host side.
#[test]
fn tool_result_patch_carries_usage_and_omits_it_when_absent() {
    let patch = crate::events::ToolResultPatch {
        usage: Some(serde_json::json!({ "input": 5 })),
        ..Default::default()
    };
    let v = serde_json::to_value(&patch).unwrap();
    assert_eq!(v, serde_json::json!({ "usage": { "input": 5 } }));

    let empty = crate::events::ToolResultPatch::default();
    let v = serde_json::to_value(&empty).unwrap();
    assert!(!v.as_object().unwrap().contains_key("usage"));
}
