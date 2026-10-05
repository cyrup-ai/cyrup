//! CODE-006, extension half — `ExtensionToolContext.executeTool` / `.tools` for BOTH extension
//! tiers (pi `core/extensions/types.ts:367-395`, wired at `agent-session.ts:3420-3421` @v1.0.1).
//!
//! The native tier reaches the context through [`ExtensionToolContext::current`] inside its tool's
//! `execute`; the WASM tier through the `host-tool.execute-tool` / `callable-tools` imports, driven
//! here by a hand-written component ([`super::wat_guest`]) so the production `LiveExtension::load`,
//! import lifting and `execute_tool` binding run without the `wasm32-wasip2` SDK toolchain. The
//! session end of each is `cyrup-session-svc`'s `nested_tool_context` tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use crate::wrapper::ActiveToolNames;
use crate::{
    ExecuteToolOptions, ExtMode, ExtensionHost, ExtensionToolContext, HookOutcome, HostConfig,
    HostCtx, HostEvent, InitApi, NativeExtension, NestedToolRunner,
};
use cyrup_agent::{NestedToolCallOptions, ToolCallOutcome};
use cyrup_core::{
    CancelToken, Content, ExtensionId, Tool, ToolCall, ToolCallId, ToolError, ToolResult,
    ToolUpdate, ToolUpdateSink,
};
use serde_json::{Value, json};

fn cfg() -> HostConfig {
    HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }
}

struct NoAgent;
impl ActiveToolNames for NoAgent {
    fn active_tool_names(&self) -> Option<Vec<String>> {
        None
    }
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a nested call asked the runner for.
#[derive(Clone, Debug)]
struct Asked {
    caller: String,
    name: String,
    args: Value,
    cancel: Option<CancelToken>,
}

/// A runner that records every nested call and answers it `ran <name>`, numbering the call ids
/// `<caller>/<n>` the way the session's runner does.
#[derive(Default)]
struct StubRunner {
    asked: Mutex<Vec<Asked>>,
    tools: Vec<Arc<dyn Tool>>,
    /// How long a nested call takes.
    delay: std::time::Duration,
}

impl StubRunner {
    fn outcome(
        caller: &ToolCallId,
        n: usize,
        name: &str,
        text: &str,
        is_error: bool,
    ) -> ToolCallOutcome {
        ToolCallOutcome {
            tool_call: ToolCall {
                id: ToolCallId::from(format!("{caller}/{n}")),
                name: name.to_string(),
                arguments: serde_json::Map::new().into(),
                thought_signature: None,
                namespace: None,
            },
            result: ToolResult {
                content: vec![Content::text(text)],
                ..ToolResult::default()
            },
            is_error,
        }
    }
}

#[async_trait::async_trait]
impl NestedToolRunner for StubRunner {
    async fn execute_nested_tool(
        &self,
        caller: &ToolCallId,
        name: &str,
        args: Value,
        mut options: NestedToolCallOptions,
    ) -> ToolCallOutcome {
        let n = {
            let mut g = self.asked.lock().unwrap();
            g.push(Asked {
                caller: caller.to_string(),
                name: name.to_string(),
                args,
                cancel: options.cancel.clone(),
            });
            g.len()
        };
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if let Some(sink) = options.on_update.as_mut() {
            sink(ToolUpdate {
                content: vec![Content::text("partial")],
                ..ToolUpdate::default()
            });
        }
        Self::outcome(caller, n, name, &format!("ran {name}"), false)
    }

    fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }
}

fn as_runner<R: NestedToolRunner + 'static>(r: &Arc<R>) -> Arc<dyn NestedToolRunner> {
    Arc::clone(r) as Arc<dyn NestedToolRunner>
}

// ------------------------------------------------------------------------------ native tier ----

/// A native extension whose one tool calls `read` through its context and reports what it saw.
struct CallerExt {
    tool_name: &'static str,
    /// Holds the first call open until both calls are inside `execute`, so they overlap.
    barrier: Option<Arc<tokio::sync::Barrier>>,
}

struct CallerTool {
    name: &'static str,
    barrier: Option<Arc<tokio::sync::Barrier>>,
    params: Value,
}

#[async_trait::async_trait]
impl Tool for CallerTool {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // Hold until both calls are inside `execute`, THEN read the context: a binding that outlived
        // its call, or that one call could overwrite for another, is read here.
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        let ctx = ExtensionToolContext::current()
            .ok_or_else(|| ToolError::new("no tool context is bound"))?;
        let outcome = ctx
            .execute_tool(
                "read",
                json!({ "path": call_id.as_str() }),
                ExecuteToolOptions::default(),
            )
            .await;
        Ok(ToolResult {
            content: vec![Content::text(format!(
                "ctx={} nested={} text={}",
                ctx.call_id(),
                outcome.tool_call.id,
                text_of(&outcome.result.content)
            ))],
            ..ToolResult::default()
        })
    }
}

#[async_trait::async_trait]
impl NativeExtension for CallerExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("native-caller")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
        api.register_tool(Arc::new(CallerTool {
            name: self.tool_name,
            barrier: self.barrier.clone(),
            params: json!({ "type": "object" }),
        }));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

async fn native_host(
    runner: Option<&Arc<StubRunner>>,
    barrier: Option<Arc<tokio::sync::Barrier>>,
) -> (ExtensionHost, Arc<dyn Tool>) {
    let host = ExtensionHost::new(cfg());
    host.set_active_tool_source(Arc::new(NoAgent));
    if let Some(runner) = runner {
        host.set_nested_tool_runner(&as_runner(runner));
    }
    host.load_native(Arc::new(CallerExt {
        tool_name: "native_caller",
        barrier,
    }))
    .await
    .unwrap();
    let tool = host
        .active_tools(&[])
        .unwrap()
        .into_iter()
        .find(|t| t.name() == "native_caller")
        .expect("the native tool is in the active set");
    (host, tool)
}

fn no_updates() -> ToolUpdateSink {
    Box::new(|_| {})
}

/// pi binds `executeTool: (callerId, …) => this._executeNestedToolCall(callerId, …)` per tool call
/// (`agent-session.ts:3420`), so the call a native tool makes is `<its call id>/<n>`, and `signal`
/// defaults to the call's own.
#[tokio::test]
async fn a_native_tool_reaches_execute_tool_bound_to_its_own_call_id() {
    let runner = Arc::new(StubRunner::default());
    let (_host, tool) = native_host(Some(&runner), None).await;
    let cancel = CancelToken::new();

    let result = tool
        .execute(
            ToolCallId::from("call-A"),
            json!({}),
            cancel.clone(),
            no_updates(),
        )
        .await
        .unwrap();

    assert_eq!(
        text_of(&result.content),
        "ctx=call-A nested=call-A/1 text=ran read"
    );
    let asked = runner.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].caller, "call-A");
    assert_eq!(asked[0].name, "read");
    assert_eq!(asked[0].args, json!({ "path": "call-A" }));
    // `signal` defaults to the calling tool's: cancelling the call's token cancels the nested one.
    let nested_cancel = asked[0].cancel.clone().expect("a cancel token is passed");
    assert!(!nested_cancel.is_cancelled());
    cancel.cancel();
    assert!(
        nested_cancel.is_cancelled(),
        "the nested call carries the calling tool's own cancellation"
    );
}

/// Two calls to the same tool in flight at once never see each other's id: the binding is scoped to
/// the one future that runs the call, not to a slot that outlives it.
#[tokio::test]
async fn two_simultaneous_native_calls_never_see_each_others_call_id() {
    let runner = Arc::new(StubRunner::default());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let (_host, tool) = native_host(Some(&runner), Some(barrier)).await;

    let a = tool.execute(
        ToolCallId::from("call-A"),
        json!({}),
        CancelToken::new(),
        no_updates(),
    );
    let b = tool.execute(
        ToolCallId::from("call-B"),
        json!({}),
        CancelToken::new(),
        no_updates(),
    );
    let (a, b) = tokio::join!(a, b);

    let (a, b) = (text_of(&a.unwrap().content), text_of(&b.unwrap().content));
    assert!(a.starts_with("ctx=call-A "), "{a}");
    assert!(b.starts_with("ctx=call-B "), "{b}");
    let mut callers: Vec<String> = runner
        .asked
        .lock()
        .unwrap()
        .iter()
        .map(|c| format!("{}:{}", c.caller, c.args["path"].as_str().unwrap()))
        .collect();
    callers.sort();
    assert_eq!(callers, ["call-A:call-A", "call-B:call-B"]);
}

/// A tool run with no runner attached (a host before the session exists, a tool called directly) has
/// no context — pi's `ctx` is `undefined` there — and the binding is gone once the call has ended.
#[tokio::test]
async fn without_a_runner_or_after_the_call_there_is_no_context() {
    let (_host, tool) = native_host(None, None).await;
    let err = tool
        .execute(
            ToolCallId::from("call-A"),
            json!({}),
            CancelToken::new(),
            no_updates(),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no tool context is bound"),
        "{err}"
    );

    let runner = Arc::new(StubRunner::default());
    let (_host, tool) = native_host(Some(&runner), None).await;
    tool.execute(
        ToolCallId::from("call-A"),
        json!({}),
        CancelToken::new(),
        no_updates(),
    )
    .await
    .unwrap();
    assert!(
        ExtensionToolContext::current().is_none(),
        "the context does not outlive the call"
    );
}

/// pi `ctx.tools` is what `executeTool` can call.
#[tokio::test]
async fn the_context_lists_the_callable_tools() {
    let runner = Arc::new(StubRunner {
        tools: vec![Arc::new(CallerTool {
            name: "listed",
            barrier: None,
            params: json!({ "type": "object" }),
        })],
        ..StubRunner::default()
    });
    let ctx = ExtensionToolContext::new(
        as_runner(&runner),
        ToolCallId::from("c"),
        CancelToken::new(),
    );
    let names: Vec<String> = ctx.tools().iter().map(|t| t.name().to_string()).collect();
    assert_eq!(names, ["listed"]);
}

// -------------------------------------------------------------------------------- WASM tier ----

#[cfg(feature = "wasm-host")]
mod wasm {
    use super::*;
    use crate::manifest::Capabilities;
    use crate::tests::wat_guest::{
        Lowered, REGISTRATION_FLAG_AND_UNSUBSCRIBE, UI_STATUS_AND_SELECT, WatGuest, wat_str,
    };
    use std::time::Duration;

    const REGISTER_TOOL: Lowered = Lowered {
        core_name: "register_tool",
        component_func: "$register-tool",
        core_sig: "(param i32 i32)",
        needs_realloc: true,
    };
    const EXECUTE_TOOL: Lowered = Lowered {
        core_name: "execute_tool",
        component_func: "$execute-tool-import",
        core_sig: "(param i32 i32 i32 i32 i32 i32 i32 i32)",
        needs_realloc: true,
    };
    const CALLABLE_TOOLS: Lowered = Lowered {
        core_name: "callable_tools",
        component_func: "$callable-tools-import",
        core_sig: "(param i32 i32 i32)",
        needs_realloc: true,
    };
    const SUBSCRIBE: Lowered = Lowered {
        core_name: "subscribe",
        component_func: "$subscribe",
        core_sig: "(param i32 i32)",
        needs_realloc: false,
    };
    const SET_STATUS: Lowered = Lowered {
        core_name: "set_status",
        component_func: "$set-status",
        core_sig: "(param i32 i32 i32 i32 i32)",
        needs_realloc: false,
    };

    /// The `host-tool` import, declaring the two functions this change adds.
    const HOST_TOOL_IMPORT: &str = r#"  (import "cyrup:ext/host-tool@0.15.0" (instance $ht
    (export "execute-tool" (func (param "call-id" string) (param "name" string) (param "args-json" string) (param "collect-updates" bool) (result (result (tuple string (list string)) (error string)))))
    (export "callable-tools" (func (param "call-id" string) (result (result string (error string)))))))
  (alias export $ht "execute-tool" (func $execute-tool-import))
  (alias export $ht "callable-tools" (func $callable-tools-import))
"#;

    /// The `registration` import declaring `register-tool` AND `subscribe`: one instance import per
    /// interface, so a guest that uses both cannot take the two single-purpose fragments.
    fn registration_with_subscribe() -> String {
        let tool_only = crate::tests::tool_exposure::guest_registration_tool_import();
        let with_subscribe = tool_only.replace(
            r#"(export "register-tool" (func (param "t" $td-x) (result (result (error string)))))))"#,
            r#"(export "register-tool" (func (param "t" $td-x) (result (result (error string)))))
    (export "subscribe" (func (param "event-kinds" (list u8))))))"#,
        );
        assert_ne!(
            with_subscribe, tool_only,
            "the fragment shape the test patches moved"
        );
        format!("{with_subscribe}  (alias export $reg \"subscribe\" (func $subscribe))\n")
    }

    fn le(v: u32) -> String {
        v.to_le_bytes()
            .iter()
            .map(|b| format!("\\{b:02x}"))
            .collect()
    }

    /// The canonical-ABI `tool-descriptor` (136 bytes) for a tool named `name` with `{}` parameters
    /// and every optional member absent.
    fn descriptor(name_len: u32) -> String {
        let mut words = [0u32; 34];
        words[0] = 17200; // name ptr
        words[1] = name_len;
        words[6] = 17220; // parameters-json ptr
        words[7] = 2;
        words.iter().map(|w| le(*w)).collect()
    }

    const REG_RET: u32 = 16512;

    /// `init` registers the tool `nester`.
    fn init_registering() -> String {
        format!(
            "    (func (export \"init\") (result i32) \
             (call $register_tool (i32.const 17000) (i32.const {REG_RET})) i32.const {REG_RET})"
        )
    }

    /// `execute-tool` calls the `host-tool.execute-tool` import for `read` with `call-id` taken
    /// from the call itself or from the string at 17440 (`other`), and returns what came back as
    /// its own result: the outcome JSON as `content-json` on `ok`, the refusal as the error string
    /// on `err` (the two arms have the same layout, so the tag and two words are copied across). The
    /// first partial result the call streamed, when there is one, becomes its `details-json`.
    fn execute_tool_calling_import(own_call_id: bool) -> String {
        let call_id = if own_call_id {
            "(local.get 2) (local.get 3)"
        } else {
            "(i32.const 17440) (i32.const 5)"
        };
        format!(
            "    (func (export \"execute-tool\") (param i32 i32 i32 i32 i32 i32) (result i32) \
             (call $execute_tool {call_id} (i32.const 17400) (i32.const 4) (i32.const 17410) \
             (i32.const 16) (i32.const 1) (i32.const 18000)) \
             (i32.store (i32.const 18100) (i32.load (i32.const 18000))) \
             (i32.store (i32.const 18104) (i32.load (i32.const 18004))) \
             (i32.store (i32.const 18108) (i32.load (i32.const 18008))) \
             (i32.store (i32.const 18112) (i32.const 1)) \
             (i32.store (i32.const 18116) (i32.load (i32.load (i32.const 18012)))) \
             (i32.store (i32.const 18120) (i32.load (i32.add (i32.load (i32.const 18012)) (i32.const 4)))) \
             (i32.const 18100))"
        )
    }

    fn caller_component(own_call_id: bool) -> Vec<u8> {
        WatGuest {
            component: format!(
                "{}{HOST_TOOL_IMPORT}",
                crate::tests::tool_exposure::guest_registration_tool_import()
            ),
            lowered: vec![REGISTER_TOOL, EXECUTE_TOOL, CALLABLE_TOOLS],
            overrides: vec![
                ("init", init_registering()),
                ("execute-tool", execute_tool_calling_import(own_call_id)),
            ],
            data: vec![
                (17000, descriptor(6)),
                (17200, "nester".into()),
                (17220, "{}".into()),
                (17400, "read".into()),
                (17410, wat_str(r#"{"path":"a.txt"}"#)),
                (17440, "other".into()),
            ],
        }
        .build()
    }

    /// `execute-tool` calls `host-tool.callable-tools` for its own call and returns the JSON array
    /// as its `content-json` (`ok` and `err` share a layout, as in [`execute_tool_calling_import`]).
    fn lister_component() -> Vec<u8> {
        WatGuest {
            component: format!(
                "{}{HOST_TOOL_IMPORT}",
                crate::tests::tool_exposure::guest_registration_tool_import()
            ),
            lowered: vec![REGISTER_TOOL, EXECUTE_TOOL, CALLABLE_TOOLS],
            overrides: vec![
                ("init", init_registering()),
                (
                    "execute-tool",
                    "    (func (export \"execute-tool\") (param i32 i32 i32 i32 i32 i32) (result i32) \
                     (call $callable_tools (local.get 2) (local.get 3) (i32.const 18000)) \
                     (i32.store (i32.const 18100) (i32.load (i32.const 18000))) \
                     (i32.store (i32.const 18104) (i32.load (i32.const 18004))) \
                     (i32.store (i32.const 18108) (i32.load (i32.const 18008))) \
                     (i32.const 18100))"
                        .to_string(),
                ),
            ],
            data: vec![
                (17000, descriptor(6)),
                (17200, "nester".into()),
                (17220, "{}".into()),
            ],
        }
        .build()
    }

    /// pi `ctx.tools`: the guest reads the runner's callable tools through
    /// `host-tool.callable-tools`, with the fields a sandbox describes a tool by.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_guest_tool_reads_the_callable_tools_through_the_import() {
        let runner = Arc::new(StubRunner {
            tools: vec![Arc::new(CallerTool {
                name: "listed",
                barrier: None,
                params: json!({ "type": "object" }),
            })],
            ..StubRunner::default()
        });
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.set_active_tool_source(Arc::new(NoAgent));
        host.set_nested_tool_runner(&as_runner(&runner));
        host.load_wasm_with_caps(
            "lister-guest".into(),
            &lister_component(),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
        let tool = host
            .active_tools(&[])
            .unwrap()
            .into_iter()
            .find(|t| t.name() == "nester")
            .unwrap();

        let result = tool
            .execute(
                ToolCallId::from("guest-1"),
                json!({}),
                CancelToken::new(),
                no_updates(),
            )
            .await
            .unwrap();

        let listed: Value = serde_json::from_str(&text_of(&result.content)).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["name"], "listed");
        assert_eq!(listed[0]["exposure"], "direct");
        assert_eq!(listed[0]["parameters"], json!({ "type": "object" }));
    }

    async fn guest_host(
        runner: &Arc<impl NestedToolRunner + 'static>,
        own_call_id: bool,
    ) -> (
        ExtensionHost,
        Arc<crate::host::LiveExtension>,
        Arc<dyn Tool>,
    ) {
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.set_active_tool_source(Arc::new(NoAgent));
        host.set_nested_tool_runner(&as_runner(runner));
        let live = host
            .load_wasm_with_caps(
                "nester-guest".into(),
                &caller_component(own_call_id),
                Arc::new(crate::DenyServices),
                &Capabilities::host_granted(),
            )
            .await
            .expect("the component loads against the 0.15 world");
        let tool = host
            .active_tools(&[])
            .unwrap()
            .into_iter()
            .find(|t| t.name() == "nester")
            .expect("the guest's tool is in the active set");
        (host, live, tool)
    }

    /// A guest tool's `execute` calls `host-tool.execute-tool` and the call goes through the runner
    /// bound to the guest call that is executing — pi `ctx.executeTool` for a guest.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_guest_tool_reaches_execute_tool_bound_to_its_own_call_id() {
        let runner = Arc::new(StubRunner::default());
        let (_host, _live, tool) = guest_host(&runner, true).await;

        let result = tool
            .execute(
                ToolCallId::from("guest-1"),
                json!({}),
                CancelToken::new(),
                no_updates(),
            )
            .await
            .expect("the guest returns the outcome");

        let outcome: Value = serde_json::from_str(&text_of(&result.content))
            .expect("the guest handed back the outcome JSON the host returned");
        assert_eq!(outcome["toolCall"]["id"], "guest-1/1");
        assert_eq!(outcome["toolCall"]["name"], "read");
        assert_eq!(outcome["isError"], false);
        assert_eq!(outcome["result"]["content"][0]["text"], "ran read");
        let asked = runner.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].caller, "guest-1");
        assert_eq!(asked[0].args, json!({ "path": "a.txt" }));
        assert!(
            asked[0].cancel.is_some(),
            "the calling tool's cancellation rides along"
        );
        // `collect-updates` was set, so the partial result the nested tool streamed came back beside
        // the outcome (the guest here surfaces the first one as its `details`).
        assert_eq!(
            result.details,
            Some(json!({ "content": [{ "type": "text", "text": "partial" }] }))
        );
    }

    /// The wait for a nested call is host time, not guest CPU: a call that takes longer than the
    /// per-dispatch epoch budget (about five seconds) must not trap the guest on its way back in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_nested_call_longer_than_the_epoch_budget_does_not_trap_the_guest() {
        let runner = Arc::new(StubRunner {
            delay: Duration::from_secs(7),
            ..StubRunner::default()
        });
        let (_host, _live, tool) = guest_host(&runner, true).await;

        let result = tool
            .execute(
                ToolCallId::from("guest-1"),
                json!({}),
                CancelToken::new(),
                no_updates(),
            )
            .await
            .expect("the guest resumes after a nested call that outlived the epoch budget");

        let outcome: Value = serde_json::from_str(&text_of(&result.content)).unwrap();
        assert_eq!(outcome["result"]["content"][0]["text"], "ran read");
    }

    /// The call in flight is bound for the call and for nothing after it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_in_flight_call_is_unbound_when_the_call_ends() {
        let runner = Arc::new(StubRunner::default());
        let (_host, live, tool) = guest_host(&runner, true).await;
        assert!(live.guest().tool_call_in_flight().is_none());

        tool.execute(
            ToolCallId::from("guest-1"),
            json!({}),
            CancelToken::new(),
            no_updates(),
        )
        .await
        .unwrap();

        assert!(
            live.guest().tool_call_in_flight().is_none(),
            "the finished call is still bound"
        );
        assert!(
            live.guest().nested_context_for("guest-1").is_err(),
            "a finished call can still reach the runner"
        );
    }

    /// A guest can only call tools on behalf of the call it is executing: any other `call-id` is a
    /// refusal naming it, and the runner is never reached.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_guest_cannot_call_tools_on_behalf_of_another_call() {
        let runner = Arc::new(StubRunner::default());
        let (_host, _live, tool) = guest_host(&runner, false).await;

        let err = tool
            .execute(
                ToolCallId::from("guest-1"),
                json!({}),
                CancelToken::new(),
                no_updates(),
            )
            .await
            .expect_err("the import refuses a call-id that is not in flight");

        assert!(
            err.to_string()
                .contains("call-id `other` is not the tool call in flight on this extension"),
            "{err}"
        );
        assert!(runner.asked.lock().unwrap().is_empty());
    }

    /// With no runner attached the tool has no `ctx` (pi: `ctx` is `undefined` for a tool run
    /// outside a session), so the import refuses by name rather than inventing an outcome.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_guest_tool_run_without_a_session_is_refused_by_name() {
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        host.set_active_tool_source(Arc::new(NoAgent));
        host.load_wasm_with_caps(
            "nester-guest".into(),
            &caller_component(true),
            Arc::new(crate::DenyServices),
            &Capabilities::host_granted(),
        )
        .await
        .unwrap();
        let tool = host
            .active_tools(&[])
            .unwrap()
            .into_iter()
            .find(|t| t.name() == "nester")
            .unwrap();

        let err = tool
            .execute(
                ToolCallId::from("guest-1"),
                json!({}),
                CancelToken::new(),
                no_updates(),
            )
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("no session is attached, so a tool cannot call tools"),
            "{err}"
        );
    }

    /// A runner that makes the nested call go to ANOTHER tool of the calling guest — the call that
    /// would wait for the instance its caller holds.
    struct ReenteringRunner {
        tool: std::sync::OnceLock<Arc<dyn Tool>>,
    }

    #[async_trait::async_trait]
    impl NestedToolRunner for ReenteringRunner {
        async fn execute_nested_tool(
            &self,
            caller: &ToolCallId,
            name: &str,
            _args: Value,
            _options: NestedToolCallOptions,
        ) -> ToolCallOutcome {
            let tool = self.tool.get().expect("set before the call");
            let nested = ToolCallId::from(format!("{caller}/1"));
            let r = tool
                .execute(nested, json!({}), CancelToken::new(), no_updates())
                .await;
            let (text, is_error) = match r {
                Ok(r) => (text_of(&r.content), false),
                Err(e) => (e.to_string(), true),
            };
            StubRunner::outcome(caller, 1, name, &text, is_error)
        }
        fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
            Vec::new()
        }
    }

    /// A nested call that re-enters the SAME guest instance must not deadlock on the lock its own
    /// caller holds: it is refused with [`crate::host::GuestReentry`] and comes back as an error
    /// outcome, so the calling tool can carry on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_nested_call_into_the_same_guest_is_refused_not_deadlocked() {
        let runner = Arc::new(ReenteringRunner {
            tool: std::sync::OnceLock::new(),
        });
        let (_host, _live, tool) = guest_host(&runner, true).await;
        runner.tool.set(tool.clone()).ok().unwrap();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            tool.execute(
                ToolCallId::from("guest-1"),
                json!({}),
                CancelToken::new(),
                no_updates(),
            ),
        )
        .await
        .expect("the nested call returns instead of waiting on its own caller")
        .expect("the outer call completes");

        let outcome: Value = serde_json::from_str(&text_of(&result.content)).unwrap();
        assert_eq!(outcome["isError"], true);
        let text = outcome["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("tool call `guest-1/1` was made by `guest-1`")
                && text.contains("a tool cannot call a tool of its own extension"),
            "{text}"
        );
    }

    // ---------------------------------------------------------------- parentToolCallId on events ----

    /// `on-tool-call` echoes its trailing `parent-tool-call-id` into a status segment, so the test
    /// reads exactly what crossed the boundary.
    fn parent_echo_component() -> Vec<u8> {
        WatGuest {
            component: format!("{REGISTRATION_FLAG_AND_UNSUBSCRIBE}{UI_STATUS_AND_SELECT}"),
            lowered: vec![SUBSCRIBE, SET_STATUS],
            overrides: vec![
                (
                    "init",
                    "    (func (export \"init\") (result i32) \
                     (call $subscribe (i32.const 16384) (i32.const 1)) i32.const 16)"
                        .to_string(),
                ),
                (
                    "on-tool-call",
                    "    (func (export \"on-tool-call\") (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32) \
                     (call $set_status (i32.const 16400) (i32.const 6) (local.get 6) (local.get 7) (local.get 8)) \
                     i32.const 16)"
                        .to_string(),
                ),
            ],
            data: vec![
                (16384, format!("\\{:02x}", crate::EventKind::ToolCall as u8)),
                (16400, "parent".to_string()),
            ],
        }
        .build()
    }

    fn tool_call_event(id: &str) -> HostEvent {
        HostEvent::ToolCall {
            call_id: ToolCallId::from(id),
            name: "read".into(),
            input: json!({}),
        }
    }

    /// pi's `parentToolCallId?: string` on `tool_call` (`extensions/types.ts:1155-1161` @v1.0.1)
    /// reaches a guest as the trailing `option<string>`: the id for a nested call, `none` for a call
    /// the model issued.
    #[tokio::test]
    async fn a_guest_sees_the_parent_of_a_nested_tool_call_and_none_for_a_model_call() {
        let host = ExtensionHost::with_wasm(cfg()).unwrap();
        let live = host
            .load_wasm_with_caps(
                "echo-guest".into(),
                &parent_echo_component(),
                Arc::new(crate::DenyServices),
                &Capabilities::host_granted(),
            )
            .await
            .unwrap();
        let cancel = CancelToken::new();

        host.dispatcher()
            .dispatch_block_mutate(tool_call_event("call-1"), &cancel)
            .await;
        host.dispatcher()
            .dispatch_block_mutate_nested(
                tool_call_event("call-1/1"),
                &ToolCallId::from("call-1"),
                &cancel,
            )
            .await;

        assert_eq!(
            live.guest().statuses(),
            vec![
                ("parent".to_string(), None),
                ("parent".to_string(), Some("call-1".to_string())),
            ]
        );
    }

    /// The nested-call events of a tool that is executing in THIS instance are not delivered to it
    /// (its lock is held by the call that is waiting for them), and the call does not hang. Every
    /// other instance receives them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nested_call_events_skip_the_instance_whose_tool_is_making_the_call() {
        /// A runner whose nested call dispatches `tool_call` for the nested call through the host,
        /// the way the session's hooks do.
        struct DispatchingRunner {
            host: std::sync::OnceLock<Arc<ExtensionHost>>,
            /// Whether the dispatch passed, and how long it took.
            dispatched: Arc<Mutex<Option<(bool, Duration)>>>,
        }
        #[async_trait::async_trait]
        impl NestedToolRunner for DispatchingRunner {
            async fn execute_nested_tool(
                &self,
                caller: &ToolCallId,
                name: &str,
                _args: Value,
                _options: NestedToolCallOptions,
            ) -> ToolCallOutcome {
                let host = self.host.get().unwrap();
                let started = std::time::Instant::now();
                let reduced = host
                    .dispatcher()
                    .dispatch_block_mutate_nested(
                        tool_call_event(&format!("{caller}/1")),
                        caller,
                        &CancelToken::new(),
                    )
                    .await;
                *self.dispatched.lock().unwrap() = Some((
                    matches!(reduced, crate::Reduced::Pass(_)),
                    started.elapsed(),
                ));
                StubRunner::outcome(caller, 1, name, "dispatched", false)
            }
            fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
                Vec::new()
            }
        }

        // A native that watches `tool_call`: the instance that IS NOT making the call hears it.
        struct Watcher(Arc<Mutex<Vec<Option<String>>>>);
        #[async_trait::async_trait]
        impl NativeExtension for Watcher {
            fn id(&self) -> ExtensionId {
                ExtensionId::from("watcher")
            }
            async fn init(&self, api: &mut InitApi) -> Result<(), crate::ExtError> {
                api.subscribe(&[crate::EventKind::ToolCall]);
                Ok(())
            }
            async fn on_event(&self, _ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
                self.0
                    .lock()
                    .unwrap()
                    .push(ctx.parent_tool_call_id().map(ToString::to_string));
                HookOutcome::Noop
            }
        }

        // One guest that both owns the tool and subscribes to `tool_call`.
        let component = WatGuest {
            component: format!(
                "{}{HOST_TOOL_IMPORT}{UI_STATUS_AND_SELECT}",
                registration_with_subscribe()
            ),
            lowered: vec![REGISTER_TOOL, EXECUTE_TOOL, CALLABLE_TOOLS, SUBSCRIBE, SET_STATUS],
            overrides: vec![
                (
                    "init",
                    format!(
                        "    (func (export \"init\") (result i32) \
                         (call $subscribe (i32.const 16384) (i32.const 1)) \
                         (call $register_tool (i32.const 17000) (i32.const {REG_RET})) \
                         i32.const {REG_RET})"
                    ),
                ),
                ("execute-tool", execute_tool_calling_import(true)),
                (
                    "on-tool-call",
                    "    (func (export \"on-tool-call\") (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32) \
                     (call $set_status (i32.const 16400) (i32.const 6) (local.get 6) (local.get 7) (local.get 8)) \
                     i32.const 16)"
                        .to_string(),
                ),
            ],
            data: vec![
                (16384, format!("\\{:02x}", crate::EventKind::ToolCall as u8)),
                (16400, "parent".to_string()),
                (17000, descriptor(6)),
                (17200, "nester".into()),
                (17220, "{}".into()),
                (17400, "read".into()),
                (17410, wat_str(r#"{"path":"a.txt"}"#)),
            ],
        }
        .build();

        let dispatched = Arc::new(Mutex::new(None));
        let runner = Arc::new(DispatchingRunner {
            host: std::sync::OnceLock::new(),
            dispatched: Arc::clone(&dispatched),
        });
        let host = Arc::new(ExtensionHost::with_wasm(cfg()).unwrap());
        host.set_active_tool_source(Arc::new(NoAgent));
        host.set_nested_tool_runner(&as_runner(&runner));
        runner.host.set(Arc::clone(&host)).ok().unwrap();
        let watched = Arc::new(Mutex::new(Vec::new()));
        host.load_native(Arc::new(Watcher(Arc::clone(&watched))))
            .await
            .unwrap();
        let live = host
            .load_wasm_with_caps(
                "self-gating".into(),
                &component,
                Arc::new(crate::DenyServices),
                &Capabilities::host_granted(),
            )
            .await
            .unwrap();
        let tool = host
            .active_tools(&[])
            .unwrap()
            .into_iter()
            .find(|t| t.name() == "nester")
            .unwrap();

        tokio::time::timeout(
            Duration::from_secs(10),
            tool.execute(
                ToolCallId::from("guest-1"),
                json!({}),
                CancelToken::new(),
                no_updates(),
            ),
        )
        .await
        .expect("the nested dispatch does not wait on the instance its caller holds")
        .unwrap();

        let (passed, took) = dispatched
            .lock()
            .unwrap()
            .expect("the nested call ran to completion");
        assert!(
            passed,
            "the dispatch was blocked: the calling instance was waited on, not skipped"
        );
        assert!(
            took < Duration::from_secs(2),
            "the dispatch waited {took:?} on the instance its caller holds"
        );
        assert_eq!(
            *watched.lock().unwrap(),
            vec![Some("guest-1".to_string())],
            "every other extension receives the nested call's event, with its parent"
        );
        assert!(
            live.guest().statuses().is_empty(),
            "the instance whose tool made the call was not asked: {:?}",
            live.guest().statuses()
        );
    }
}
