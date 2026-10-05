//! Builders and readers shared by the sandbox tests.

use std::sync::Arc;
use std::time::Duration;

use cyrup_codemode::types::{OutputItem, ToolDeclaration};
use futures::future::BoxFuture;
use serde_json::Value;

use crate::sandbox::CodemodeSandbox;
use crate::types::{
    CallStatus, CodemodeCall, CodemodeError, CodemodeResult, CodemodeTool, CodemodeToolContext,
    Deadline, ExecuteOptions, SandboxOptions, ToolResult,
};

/// A sandbox that, when the test ends, waits for its isolate threads to wind down, as upstream's
/// `afterEach` closes every sandbox: a thread still unwinding when the test process exits makes
/// the process's exit slow enough for the runner to call the test leaky.
pub struct TestSandbox(CodemodeSandbox);

impl std::ops::Deref for TestSandbox {
    type Target = CodemodeSandbox;

    fn deref(&self) -> &CodemodeSandbox {
        &self.0
    }
}

impl Drop for TestSandbox {
    fn drop(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.0.live() != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A tool whose callback is `f`, which builds the future itself.
pub fn tool_with<F>(declaration: ToolDeclaration, f: F) -> CodemodeTool
where
    F: Fn(Option<Value>, CodemodeToolContext) -> BoxFuture<'static, ToolResult>
        + Send
        + Sync
        + 'static,
{
    CodemodeTool {
        declaration,
        execute: Arc::new(f),
    }
}

/// A tool whose callback is a plain function.
pub fn sync_tool<F>(name: &str, f: F) -> CodemodeTool
where
    F: Fn(Option<Value>) -> ToolResult + Send + Sync + 'static,
{
    tool_with(ToolDeclaration::new(name), move |args, _| {
        let result = f(args);
        Box::pin(async move { result })
    })
}

/// pi's `echo`: `{ name: "echo", execute: (args) => args }`.
pub fn echo() -> CodemodeTool {
    sync_tool("echo", Ok)
}

/// A sandbox with a generous deadline, as the upstream suite's `createSandbox`.
pub fn sandbox(tools: Vec<CodemodeTool>) -> TestSandbox {
    sandbox_with(SandboxOptions {
        tools,
        deadline: Deadline::After(Duration::from_secs(10)),
        ..SandboxOptions::default()
    })
}

pub fn sandbox_with(options: SandboxOptions) -> TestSandbox {
    TestSandbox(CodemodeSandbox::new(options).unwrap())
}

pub async fn run(sandbox: &CodemodeSandbox, code: &str) -> CodemodeResult {
    sandbox
        .execute(code, ExecuteOptions::default())
        .await
        .unwrap()
}

pub async fn run_with(
    sandbox: &CodemodeSandbox,
    code: &str,
    options: ExecuteOptions,
) -> CodemodeResult {
    sandbox.execute(code, options).await.unwrap()
}

pub fn deadline_ms(ms: u64) -> ExecuteOptions {
    ExecuteOptions {
        deadline: Some(Deadline::After(Duration::from_millis(ms))),
        ..ExecuteOptions::default()
    }
}

/// The return value of a completed execution (`None` is `undefined`); panics with the whole
/// result otherwise.
pub fn value(result: &CodemodeResult) -> Option<Value> {
    match result {
        CodemodeResult::Completed { value, .. } => value.clone(),
        CodemodeResult::Failed { .. } => panic!("expected a completed execution: {result:#?}"),
    }
}

/// The error of a failed execution; panics with the whole result otherwise.
pub fn error(result: &CodemodeResult) -> &CodemodeError {
    match result {
        CodemodeResult::Failed { error, .. } => error,
        CodemodeResult::Completed { .. } => panic!("expected a failed execution: {result:#?}"),
    }
}

pub fn output(result: &CodemodeResult) -> &[OutputItem] {
    match result {
        CodemodeResult::Completed { output, .. } | CodemodeResult::Failed { output, .. } => output,
    }
}

pub fn calls(result: &CodemodeResult) -> &[CodemodeCall] {
    match result {
        CodemodeResult::Completed { calls, .. } | CodemodeResult::Failed { calls, .. } => calls,
    }
}

pub fn call_summary(result: &CodemodeResult) -> Vec<(String, CallStatus)> {
    calls(result)
        .iter()
        .map(|call| (call.name.clone(), call.status))
        .collect()
}

/// Waits (bounded) for the sandbox's executions and isolate threads to wind down.
pub async fn wait_until_idle(sandbox: &CodemodeSandbox) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while sandbox.live() != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "{} executions or isolate threads are still alive",
            sandbox.live()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub fn text(text: &str) -> OutputItem {
    OutputItem::Text(text.to_owned())
}

pub fn image(data: &str, mime_type: &str) -> OutputItem {
    OutputItem::Image {
        data: data.to_owned(),
        mime_type: mime_type.to_owned(),
    }
}
