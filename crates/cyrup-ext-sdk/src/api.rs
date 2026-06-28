//! The ergonomic guest API (arch-08 §3.6). An author registers tools/commands, subscribes to
//! events, and returns block/mutate/notify outcomes — all in safe Rust, without touching the raw
//! WIT bindings. The same `ExtensionApi` is unit-testable on the host target; the `wasm32-wasip2`
//! build wires it to the generated `cyrup:ext` bindings (see `crate::guest`).

use crate::descriptor::{CommandDescriptor, ToolDescriptor};
use serde_json::Value;

/// The block/mutate/notify contribution a handler returns (mirrors the host `HookOutcome`).
#[derive(Clone, Debug)]
pub enum Outcome {
    /// notify-only / no change.
    Noop,
    /// Short-circuit the action with an optional reason (first block wins host-side).
    Block(Option<String>),
    /// Replace the in-flight value with this JSON patch (event-specific shape).
    Mutate(Value),
    /// The extension fully serviced the action.
    Handled(Value),
}

/// Result of executing a guest-registered tool.
#[derive(Clone, Debug, Default)]
pub struct ToolOutput {
    pub content_text: String,
    pub details: Option<Value>,
    pub is_error: bool,
}

/// A tool implementation supplied by the guest author.
pub trait ToolExec: Send + 'static {
    fn execute(&self, input: Value) -> Result<ToolOutput, String>;
}

impl<F> ToolExec for F
where
    F: Fn(Value) -> Result<ToolOutput, String> + Send + 'static,
{
    fn execute(&self, input: Value) -> Result<ToolOutput, String> {
        (self)(input)
    }
}

/// A `tool_call` handler: inspect the call, return an outcome (block/mutate/noop).
pub type ToolCallHandler = Box<dyn Fn(&str, &str, &Value) -> Outcome + Send + 'static>;
/// A `tool_result` handler.
pub type ToolResultHandler = Box<dyn Fn(&str, &str, &Value, bool) -> Outcome + Send + 'static>;
/// A `context` handler (receives + returns the message list as JSON).
pub type ContextHandler = Box<dyn Fn(&Value) -> Outcome + Send + 'static>;

/// A registered tool: its descriptor + its executor.
pub struct RegisteredTool {
    pub descriptor: ToolDescriptor,
    pub exec: Box<dyn ToolExec>,
}

/// The collected registrations + handlers an extension declares in its `init` (arch-08 §3.6).
#[derive(Default)]
pub struct ExtensionApi {
    pub(crate) tools: Vec<RegisteredTool>,
    pub(crate) commands: Vec<(String, CommandDescriptor)>,
    pub(crate) on_tool_call: Option<ToolCallHandler>,
    pub(crate) on_tool_result: Option<ToolResultHandler>,
    pub(crate) on_context: Option<ContextHandler>,
}

impl ExtensionApi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool (overrides a built-in of the same name host-side, R-08-012).
    pub fn register_tool(&mut self, descriptor: ToolDescriptor, exec: impl ToolExec) {
        self.tools.push(RegisteredTool { descriptor, exec: Box::new(exec) });
    }

    /// Register a command (R-08-016).
    pub fn register_command(&mut self, name: impl Into<String>, desc: CommandDescriptor) {
        self.commands.push((name.into(), desc));
    }

    /// Subscribe to `tool_call` (the permission seam, R-08-010).
    pub fn on_tool_call(
        &mut self,
        f: impl Fn(&str, &str, &Value) -> Outcome + Send + 'static,
    ) {
        self.on_tool_call = Some(Box::new(f));
    }

    /// Subscribe to `tool_result` (patch chaining, R-08-011).
    pub fn on_tool_result(
        &mut self,
        f: impl Fn(&str, &str, &Value, bool) -> Outcome + Send + 'static,
    ) {
        self.on_tool_result = Some(Box::new(f));
    }

    /// Subscribe to `context` (filter/replace messages, R-08-028 subset).
    pub fn on_context(&mut self, f: impl Fn(&Value) -> Outcome + Send + 'static) {
        self.on_context = Some(Box::new(f));
    }

    /// The `u8` event-kind list this extension subscribed to, for the host `subscribe` import
    /// (R-ARCH-EXT-014). Kept in sync with the host `EventKind` discriminants.
    pub fn subscription_kinds(&self) -> Vec<u8> {
        let mut v = Vec::new();
        if self.on_tool_call.is_some() {
            v.push(0); // EventKind::ToolCall
        }
        if self.on_tool_result.is_some() {
            v.push(1); // EventKind::ToolResult
        }
        if self.on_context.is_some() {
            v.push(2); // EventKind::Context
        }
        v
    }
}
