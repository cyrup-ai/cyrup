//! Author-facing tool factories (Pi `defineTool` + the re-exported `createBashTool`/`createReadTool`/…
//! tool factories, sdk.ts:111-123; sdk gap #6). [`define_tool`] bundles a [`ToolDescriptor`] with its
//! executor into a [`RegisteredTool`] (the analog of Pi's identity `defineTool`, types.ts:493), and
//! the [`crate::tool_factory`] descriptor builders reproduce the shapes of Pi's built-in tools so an
//! author can compose / override them with a custom `cwd` or guidelines.

use crate::api::{RegisteredTool, ToolExec};
use crate::descriptor::{ExecMode, ToolDescriptor};
use serde_json::json;

/// Bundle a descriptor + executor into a [`RegisteredTool`] (Pi `defineTool`, types.ts:493). Pass
/// the result to [`crate::ExtensionApi::register_tool_def`].
pub fn define_tool(descriptor: ToolDescriptor, exec: impl ToolExec) -> RegisteredTool {
    RegisteredTool { descriptor, exec: Box::new(exec) }
}

/// A `bash` tool descriptor scoped to `cwd` (Pi `createBashTool(cwd)`, bash.ts:451). The author
/// supplies the executor (the guest runs `ctx.exec(...)` against its granted exec capability).
pub fn bash_descriptor(cwd: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        "bash",
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to run." },
                "cwd": { "type": "string", "default": cwd }
            },
            "required": ["command"]
        }),
    )
    .label("Bash")
    .description("Run a shell command in the project working directory.")
    .execution_mode(ExecMode::Sequential)
}

/// A `read` tool descriptor (Pi `createReadTool`, sdk.ts:118).
pub fn read_descriptor() -> ToolDescriptor {
    ToolDescriptor::new(
        "read",
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    )
    .label("Read")
    .description("Read a file from the project.")
}

/// A `write` tool descriptor (Pi `createWriteTool`, sdk.ts:120).
pub fn write_descriptor() -> ToolDescriptor {
    ToolDescriptor::new(
        "write",
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
            "required": ["path", "content"]
        }),
    )
    .label("Write")
    .description("Write a file in the project.")
    .execution_mode(ExecMode::Sequential)
}
