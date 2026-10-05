//! Test doubles for the `codemode` tool's two seams: a scripted [`ScriptSandbox`] and a recording
//! [`CodemodeHost`]. Behind the `testkit` feature; a test of this crate or of a session that hosts
//! the tool drives the real tool, extension and session through them without an engine.
//!
//! The scripted sandbox does not interpret JavaScript. A test supplies the "script" as a Rust
//! closure from the source text and a [`ScriptEnv`] — the tool callbacks and globals the tool
//! registered — to a [`CodemodeResult`], so what is under test is exactly what the tool does with
//! the sandbox's inputs and outputs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod models;

pub use models::FakeModels;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use cyrup_config::CodemodeMode;
use cyrup_core::{CancelToken, Tool, ToolCallId};
use futures::future::BoxFuture;
use serde_json::{Map, Value};

use crate::tool::host::{CodemodeHost, NestedOutcome, StoreAppendFailed};
use crate::tool::models::CodemodeModels;
use crate::tool::store::{BranchCustomEntry, CodemodeStoreEntryData};
use crate::tool::{SandboxFactory, SandboxUnavailable};
use crate::types::{
    CodemodeCall, CodemodeError, CodemodeResult, CodemodeStoreWrites, CodemodeToolContext,
    Deadline, ErrorKind, ExecuteOptions, SandboxClosed, SandboxOptions, ScriptSandbox,
    ToolCallback, ToolResult,
};
use cyrup_codemode::types::OutputItem;

/// What a scripted script sees: the tools and globals the tool registered, the store `load()`
/// reads, and the execution's cancel token.
#[derive(Clone)]
pub struct ScriptEnv {
    tools: Arc<BTreeMap<String, ToolCallback>>,
    globals: Arc<BTreeMap<String, ToolCallback>>,
    /// The values `load(key)` reads.
    pub store: Map<String, Value>,
    /// The token the caller passed to `execute`.
    pub cancel: CancelToken,
    calls: CancelToken,
}

impl ScriptEnv {
    /// `tools.<name>(args)`.
    pub async fn tool(&self, name: &str, args: Value) -> ToolResult {
        let callback = self
            .tools
            .get(name)
            .unwrap_or_else(|| panic!("the tool registered no sandbox tool `{name}`"));
        callback(
            Some(args),
            CodemodeToolContext {
                cancel: self.calls.child_token(),
            },
        )
        .await
    }

    /// `tools.<name>(args)` with the given per-call abort signal.
    pub async fn tool_with_cancel(
        &self,
        name: &str,
        args: Value,
        cancel: CancelToken,
    ) -> ToolResult {
        let callback = self
            .tools
            .get(name)
            .unwrap_or_else(|| panic!("the tool registered no sandbox tool `{name}`"));
        callback(Some(args), CodemodeToolContext { cancel }).await
    }

    /// A global called with `args`, as a spread global receives them.
    pub async fn global(&self, name: &str, args: Vec<Value>) -> ToolResult {
        let callback = self
            .globals
            .get(name)
            .unwrap_or_else(|| panic!("the tool registered no global `{name}`"));
        callback(
            Some(Value::Array(args)),
            CodemodeToolContext {
                cancel: self.calls.child_token(),
            },
        )
        .await
    }

    /// The cancel token of one tool call: cancelled when the execution ends.
    #[must_use]
    pub fn call_cancel(&self) -> CancelToken {
        self.calls.child_token()
    }
}

/// A completed result with no output.
#[must_use]
pub fn completed(output: Vec<OutputItem>, value: Option<Value>) -> CodemodeResult {
    CodemodeResult::Completed {
        value,
        output,
        calls: Vec::<CodemodeCall>::new(),
        store_writes: CodemodeStoreWrites::default(),
    }
}

/// A script failure that threw `name: message` at `stack`.
#[must_use]
pub fn failed(
    kind: ErrorKind,
    message: &str,
    stack: Option<&str>,
    output: Vec<OutputItem>,
) -> CodemodeResult {
    CodemodeResult::Failed {
        error: CodemodeError {
            kind,
            name: None,
            message: message.to_owned(),
            stack: stack.map(str::to_owned),
        },
        output,
        calls: Vec::new(),
    }
}

/// The "script" of a [`ScriptedSandboxFactory`]: source text and environment to result.
pub type Script =
    Arc<dyn Fn(String, ScriptEnv) -> BoxFuture<'static, CodemodeResult> + Send + Sync>;

/// What one scripted execution was asked to do.
#[derive(Clone, Debug)]
pub struct SeenRun {
    pub tool_names: Vec<String>,
    pub global_names: Vec<String>,
    pub deadline: Deadline,
    pub memory_limit_bytes: Option<u64>,
    pub code: String,
    pub store: Map<String, Value>,
    pub closed: bool,
}

/// A [`SandboxFactory`] whose sandboxes run a Rust closure instead of JavaScript.
#[derive(Clone)]
pub struct ScriptedSandboxFactory {
    script: Script,
    seen: Arc<Mutex<Vec<SeenRun>>>,
}

impl ScriptedSandboxFactory {
    #[must_use]
    pub fn new(script: Script) -> Self {
        Self {
            script,
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Every execution so far, in order.
    #[must_use]
    pub fn runs(&self) -> Vec<SeenRun> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl SandboxFactory for ScriptedSandboxFactory {
    fn create(
        &self,
        options: SandboxOptions,
    ) -> Result<Arc<dyn ScriptSandbox>, SandboxUnavailable> {
        Ok(Arc::new(ScriptedSandbox {
            options,
            script: Arc::clone(&self.script),
            seen: Arc::clone(&self.seen),
            index: Mutex::new(None),
            closed: CancelToken::new(),
        }))
    }
}

struct ScriptedSandbox {
    options: SandboxOptions,
    script: Script,
    seen: Arc<Mutex<Vec<SeenRun>>>,
    index: Mutex<Option<usize>>,
    closed: CancelToken,
}

#[async_trait::async_trait]
impl ScriptSandbox for ScriptedSandbox {
    async fn execute(
        &self,
        code: &str,
        options: ExecuteOptions,
    ) -> Result<CodemodeResult, SandboxClosed> {
        if self.closed.is_cancelled() {
            return Err(SandboxClosed);
        }
        {
            let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
            seen.push(SeenRun {
                tool_names: self
                    .options
                    .tools
                    .iter()
                    .map(|tool| tool.declaration.name.clone())
                    .collect(),
                global_names: self
                    .options
                    .globals
                    .iter()
                    .map(|tool| tool.declaration.name.clone())
                    .collect(),
                deadline: options.deadline.unwrap_or(self.options.deadline),
                memory_limit_bytes: self.options.memory_limit_bytes,
                code: code.to_owned(),
                store: options.store.clone(),
                closed: false,
            });
            *self.index.lock().unwrap_or_else(PoisonError::into_inner) = Some(seen.len() - 1);
        }
        let calls = CancelToken::new();
        let env = ScriptEnv {
            tools: Arc::new(
                self.options
                    .tools
                    .iter()
                    .map(|tool| (tool.declaration.name.clone(), Arc::clone(&tool.execute)))
                    .collect(),
            ),
            globals: Arc::new(
                self.options
                    .globals
                    .iter()
                    .map(|tool| (tool.declaration.name.clone(), Arc::clone(&tool.execute)))
                    .collect(),
            ),
            store: options.store,
            cancel: options.cancel.unwrap_or_default(),
            calls: calls.clone(),
        };
        let result = (self.script)(code.to_owned(), env).await;
        // "Cancelled when the script finishes (including unawaited calls)".
        calls.cancel();
        Ok(result)
    }

    async fn close(&self) {
        self.closed.cancel();
        let index = *self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = index
            && let Some(run) = self
                .seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(index)
        {
            run.closed = true;
        }
    }
}

/// A tool with settable metadata that answers every call with its name as text.
pub struct StubTool {
    name: String,
    description: String,
    parameters: Value,
    output_schema: Option<Value>,
    exposure: cyrup_core::ToolExposure,
    namespace: Option<cyrup_core::ToolNamespace>,
    default_active: bool,
}

impl StubTool {
    #[must_use]
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string", "description": "Text to echo" } },
                "required": ["text"]
            }),
            output_schema: None,
            exposure: cyrup_core::ToolExposure::Direct,
            namespace: None,
            default_active: true,
        }
    }

    #[must_use]
    pub fn exposure(mut self, exposure: cyrup_core::ToolExposure) -> Self {
        self.exposure = exposure;
        self
    }

    #[must_use]
    pub fn output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    #[must_use]
    pub fn parameters(mut self, parameters: Value) -> Self {
        self.parameters = parameters;
        self
    }

    #[must_use]
    pub fn namespace(mut self, name: &str, description: Option<&str>) -> Self {
        self.namespace = Some(cyrup_core::ToolNamespace {
            name: name.to_owned(),
            description: description.map(str::to_owned),
            instructions: None,
        });
        self
    }

    /// Usage guidance on the namespace set by [`Self::namespace`].
    #[must_use]
    pub fn namespace_instructions(mut self, instructions: &str) -> Self {
        if let Some(namespace) = self.namespace.as_mut() {
            namespace.instructions = Some(instructions.to_owned());
        }
        self
    }

    #[must_use]
    pub fn inactive(mut self) -> Self {
        self.default_active = false;
        self
    }

    #[must_use]
    pub fn arc(self) -> Arc<dyn Tool> {
        Arc::new(self)
    }
}

#[async_trait::async_trait]
impl Tool for StubTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.parameters
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn output_schema(&self) -> Option<&Value> {
        self.output_schema.as_ref()
    }
    fn exposure(&self) -> cyrup_core::ToolExposure {
        self.exposure
    }
    fn namespace(&self) -> Option<&cyrup_core::ToolNamespace> {
        self.namespace.as_ref()
    }
    fn default_active(&self) -> bool {
        self.default_active
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: cyrup_core::ToolUpdateSink,
    ) -> Result<cyrup_core::ToolResult, cyrup_core::ToolError> {
        Ok(cyrup_core::ToolResult {
            content: vec![cyrup_core::Content::text(format!("{} ran", self.name))],
            ..cyrup_core::ToolResult::default()
        })
    }
}

/// A [`CodemodeHost`] over fixed tools, a scripted nested-call pipeline and an in-memory branch.
pub struct RecordingHost {
    pub mode: Mutex<CodemodeMode>,
    pub inline_budget: Mutex<f64>,
    pub tools: Mutex<Vec<Arc<dyn Tool>>>,
    /// `(caller, name, args)` of every nested call.
    pub nested: Mutex<Vec<(ToolCallId, String, Value)>>,
    /// Answers a nested call; the default answers every call with `"ok"` text.
    #[allow(clippy::type_complexity)]
    pub on_nested: Mutex<Option<Arc<dyn Fn(&str, &Value) -> NestedOutcome + Send + Sync>>>,
    pub branch: Mutex<Vec<BranchCustomEntry>>,
    pub appended: Mutex<Vec<CodemodeStoreEntryData>>,
    pub fail_append: Mutex<Option<String>>,
    pub models: Mutex<Option<Arc<dyn CodemodeModels>>>,
}

impl Default for RecordingHost {
    fn default() -> Self {
        Self {
            mode: Mutex::new(CodemodeMode::On),
            inline_budget: Mutex::new(cyrup_config::DEFAULT_CODEMODE_INLINE_BUDGET),
            tools: Mutex::new(Vec::new()),
            nested: Mutex::new(Vec::new()),
            on_nested: Mutex::new(None),
            branch: Mutex::new(Vec::new()),
            appended: Mutex::new(Vec::new()),
            fail_append: Mutex::new(None),
            models: Mutex::new(None),
        }
    }
}

impl RecordingHost {
    #[must_use]
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        let host = Self::default();
        *host.tools.lock().unwrap() = tools;
        host
    }

    /// A text outcome.
    #[must_use]
    pub fn text_outcome(call_id: &str, text: &str, is_error: bool) -> NestedOutcome {
        NestedOutcome {
            call_id: ToolCallId::from(call_id),
            result: cyrup_core::ToolResult {
                content: vec![cyrup_core::Content::text(text)],
                is_error,
                ..cyrup_core::ToolResult::default()
            },
            is_error,
        }
    }
}

#[async_trait::async_trait]
impl CodemodeHost for RecordingHost {
    fn mode(&self) -> CodemodeMode {
        *self.mode.lock().unwrap()
    }

    fn inline_budget(&self) -> f64 {
        *self.inline_budget.lock().unwrap()
    }

    fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.lock().unwrap().clone()
    }

    async fn execute_nested(
        &self,
        caller: &ToolCallId,
        name: &str,
        args: Value,
        _cancel: CancelToken,
    ) -> NestedOutcome {
        let n = {
            let mut nested = self.nested.lock().unwrap();
            nested.push((caller.clone(), name.to_owned(), args.clone()));
            nested.len()
        };
        let on_nested = self.on_nested.lock().unwrap().clone();
        match on_nested {
            Some(answer) => answer(name, &args),
            None => Self::text_outcome(&format!("{caller}/{n}"), "ok", false),
        }
    }

    async fn branch_custom_entries(&self) -> Vec<BranchCustomEntry> {
        self.branch.lock().unwrap().clone()
    }

    async fn append_store_entry(
        &self,
        data: CodemodeStoreEntryData,
    ) -> Result<(), StoreAppendFailed> {
        if let Some(message) = self.fail_append.lock().unwrap().clone() {
            return Err(StoreAppendFailed(message));
        }
        self.appended.lock().unwrap().push(data);
        Ok(())
    }

    fn has_model_access(&self) -> bool {
        self.models.lock().unwrap().is_some()
    }

    fn models(&self) -> Option<Arc<dyn CodemodeModels>> {
        self.models.lock().unwrap().clone()
    }
}
