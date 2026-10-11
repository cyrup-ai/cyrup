//! The extension-facing half of nested tool calls — pi's `ExtensionToolContext.executeTool()` and
//! `.tools` (`core/extensions/types.ts:367-395` @v1.0.1).
//!
//! Pi hands every tool `execute` a context whose `executeTool` is bound to THAT call's id, so a
//! nested call is `<callId>/<n>` and its events carry `parentToolCallId`; `signal` is the default
//! cancellation, and `tools` is what a nested call can reach (`agent-session.ts:3420-3421`:
//! `executeTool: (callerId, …) => this._executeNestedToolCall(callerId, …)`,
//! `getCallableTools: () => this._getCallableTools()`).
//!
//! # How a Rust tool reaches it
//!
//! [`cyrup_core::Tool::execute`] has no context parameter, and widening it would touch every tool
//! in the workspace. Every tool the session runs is already wrapped by
//! [`crate::RegisteredTool`], which knows the call id and the cancel token, so the wrapper binds an
//! [`ExtensionToolContext`] for the duration of the call in a [`tokio::task_local`], and a tool
//! reads it with [`ExtensionToolContext::current`].
//!
//! A task-local is scoped to the one future the wrapper wraps: it is installed when that future is
//! polled and removed when the poll returns, so two calls running in parallel — in separate tasks
//! or interleaved by one `join_all` — each see their own id and never the other's. There is no
//! "current call" slot anywhere that outlives a call. What a task-local does not do is follow a
//! `tokio::spawn`: a tool that hands work to another task takes its context first
//! ([`ExtensionToolContext`] is `Clone` and `Send`) and moves the clone in.
//!
//! The session implements [`NestedToolRunner`]; `cyrup-ext` cannot depend on the session crate, so
//! the host holds the narrow trait ([`crate::ExtensionHost::set_nested_tool_runner`]).
//!
//! A WASM guest reaches the same context through the `host-tool.execute-tool` and
//! `host-tool.callable-tools` imports (`host::live`), bound by `LiveExtension::execute_tool` to
//! the call in flight on that instance.

use std::future::Future;
use std::sync::{Arc, RwLock, Weak};

use cyrup_agent::{NestedToolCallOptions, ToolCallOutcome};
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolUpdate, ToolUpdateSink};
use serde_json::{Value, json};

/// What the session offers an extension host for tools that call tools — the object-safe seam
/// over `AgentSession::execute_nested_tool` and `AgentSession::callable_tools` (pi
/// `_executeNestedToolCall`, `agent-session.ts:699-740`, and `_getCallableTools`, `:1515`).
#[async_trait::async_trait]
pub trait NestedToolRunner: Send + Sync {
    /// Run `name` on behalf of the tool call `caller`. The call gets the id `<caller>/<n>`.
    ///
    /// Never fails for tool failures: an unknown tool, a tool that is not callable, a validation
    /// error, a blocked call and a failing tool all come back as an outcome with `is_error` set.
    async fn execute_nested_tool(
        &self,
        caller: &ToolCallId,
        name: &str,
        args: Value,
        options: NestedToolCallOptions,
    ) -> ToolCallOutcome;

    /// The tools a nested call resolves against: the active `direct` tools and every registered
    /// `codemode` or `deferred` one.
    fn callable_tools(&self) -> Vec<Arc<dyn Tool>>;
}

/// The place a host keeps its [`NestedToolRunner`]. Shared by the host and every
/// [`crate::RegisteredTool`] it wraps, so a tool wrapped BEFORE the session existed still reaches
/// the runner once it is attached.
///
/// Holds a [`Weak`]: the session owns the runner and the host is reachable from the session, so a
/// strong reference here would keep the session's pieces alive after the session is dropped.
#[derive(Default)]
pub struct NestedRunnerSlot {
    runner: RwLock<Option<Weak<dyn NestedToolRunner>>>,
}

impl NestedRunnerSlot {
    /// Attach `runner`; the last one wins.
    pub fn set(&self, runner: &Arc<dyn NestedToolRunner>) {
        if let Ok(mut g) = self.runner.write() {
            *g = Some(Arc::downgrade(runner));
        }
    }

    /// The attached runner, or `None` before one is attached or after its session is gone.
    pub fn get(&self) -> Option<Arc<dyn NestedToolRunner>> {
        self.runner.read().ok()?.as_ref()?.upgrade()
    }
}

/// Options of one [`ExtensionToolContext::execute_tool`] (pi `ExecuteToolOptions`,
/// `extensions/types.ts:368-373`).
#[derive(Default)]
pub struct ExecuteToolOptions {
    /// Pi `signal`: *"Defaults to the calling tool's signal."* `None` here is that default.
    pub cancel: Option<CancelToken>,
    /// Pi `onUpdate`: receives partial results of the nested tool, in addition to the
    /// `tool_execution_update` events.
    pub on_update: Option<ToolUpdateSink>,
}

/// The context of the tool call in flight — pi `ExtensionToolContext`'s `executeTool` and `tools`
/// (`extensions/types.ts:383-395`), bound to one call.
#[derive(Clone)]
pub struct ExtensionToolContext {
    runner: Arc<dyn NestedToolRunner>,
    call_id: ToolCallId,
    cancel: CancelToken,
}

tokio::task_local! {
    static CURRENT: ExtensionToolContext;
}

impl ExtensionToolContext {
    /// A context for the call `call_id`, whose default cancellation is `cancel`.
    pub fn new(
        runner: Arc<dyn NestedToolRunner>,
        call_id: ToolCallId,
        cancel: CancelToken,
    ) -> Self {
        Self {
            runner,
            call_id,
            cancel,
        }
    }

    /// The context of the tool call this future is running inside, or `None` when it is not inside
    /// a call a session runs (a tool called directly, a host with no session attached).
    ///
    /// Read it at the top of `execute` and move the clone into anything spawned: a task-local does
    /// not cross a `tokio::spawn`.
    pub fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok()
    }

    /// Run `fut` with this context as the current one. The binding exists for the duration of the
    /// future and for nothing else.
    pub async fn scope<F: Future>(self, fut: F) -> F::Output {
        CURRENT.scope(self, fut).await
    }

    /// The id of the call this context is bound to; a nested call made through it is
    /// `<call_id>/<n>`.
    pub fn call_id(&self) -> &ToolCallId {
        &self.call_id
    }

    /// The calling tool's cancel token — pi's `signal`, the default of
    /// [`ExecuteToolOptions::cancel`].
    pub fn cancel(&self) -> &CancelToken {
        &self.cancel
    }

    /// The tools [`Self::execute_tool`] can call — pi `ExtensionToolContext.tools`.
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.runner.callable_tools()
    }

    /// Run another tool — pi `ExtensionToolContext.executeTool(name, args, options?)`
    /// (`extensions/types.ts:394`). The call gets the id `<calling id>/<n>`; its `tool_call`,
    /// `tool_result` and `tool_execution_*` events carry the calling id as their parent; it does not
    /// appear in the transcript; and a bounded record of it is kept as `nestedCalls` on the calling
    /// tool's result message.
    ///
    /// Never fails for tool failures: unknown tools, validation errors, blocked calls and failing
    /// tools come back with `is_error` set.
    pub async fn execute_tool(
        &self,
        name: &str,
        args: Value,
        options: ExecuteToolOptions,
    ) -> ToolCallOutcome {
        let ExecuteToolOptions { cancel, on_update } = options;
        self.runner
            .execute_nested_tool(
                &self.call_id,
                name,
                args,
                NestedToolCallOptions {
                    cancel: Some(cancel.unwrap_or_else(|| self.cancel.clone())),
                    on_update,
                },
            )
            .await
    }
}

/// A streamed partial result as pi's `AgentToolResult` JSON, `{content, details?, terminate?}`
/// (`agent-loop.ts:681-691` @v0.83.0): an absent `details` or `terminate` is an absent key.
pub fn tool_update_json(update: &ToolUpdate) -> Value {
    let mut o = serde_json::Map::new();
    o.insert(
        "content".into(),
        serde_json::to_value(&update.content).unwrap_or(Value::Null),
    );
    if let Some(d) = &update.details {
        o.insert("details".into(), d.clone());
    }
    if let Some(t) = update.terminate.wire() {
        o.insert("terminate".into(), Value::Bool(t));
    }
    Value::Object(o)
}

/// One tool as the JSON a WASM guest reads it by: `name`, `label`, `description`, `parameters`,
/// `outputSchema`, `exposure` and `namespace` — what pi's `AgentTool` carries that a sandbox needs
/// to describe it.
fn tool_row(t: &dyn Tool) -> serde_json::Map<String, Value> {
    let mut o = serde_json::Map::new();
    o.insert("name".into(), json!(t.name()));
    if let Some(label) = t.label() {
        o.insert("label".into(), json!(label));
    }
    o.insert("description".into(), json!(t.description()));
    o.insert("parameters".into(), t.parameters().clone());
    if let Some(schema) = t.output_schema() {
        o.insert("outputSchema".into(), schema.clone());
    }
    o.insert("exposure".into(), json!(t.exposure().as_str()));
    if let Some(ns) = t.namespace() {
        o.insert(
            "namespace".into(),
            serde_json::to_value(ns).unwrap_or(Value::Null),
        );
    }
    o
}

/// The callable tools as the JSON a WASM guest reads from `host-tool.callable-tools`: one
/// [`tool_row`] per tool.
pub fn callable_tools_json(tools: &[Arc<dyn Tool>]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| Value::Object(tool_row(t.as_ref())))
            .collect(),
    )
}

/// The loadout a guest's `prepare-loadout` export is handed (pi `ToolLoadout`,
/// `core/extensions/types.ts:540-551` @v1.0.4): `{declared, callable, registered}`, each an array
/// of [`tool_row`]s plus the tool's normalized `promptGuidelines`. The rows also answer pi's
/// `getExposure(name)`, `getNamespace(name)` and `getPromptGuidelines(name)`, which a closure on
/// the object could not cross the boundary as.
pub fn loadout_json(view: &cyrup_core::LoadoutView<'_>) -> Value {
    let rows = |tools: &[Arc<dyn Tool>]| {
        Value::Array(
            tools
                .iter()
                .map(|t| {
                    let mut row = tool_row(t.as_ref());
                    row.insert(
                        "promptGuidelines".into(),
                        json!(cyrup_core::normalized_prompt_guidelines(t.as_ref())),
                    );
                    Value::Object(row)
                })
                .collect(),
        )
    };
    json!({
        "declared": rows(view.declared()),
        "callable": rows(view.callable()),
        "registered": rows(view.registered()),
    })
}
