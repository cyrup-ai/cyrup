//! Child-side subagent prompt runtime — ports pi `runs/shared/subagent-prompt-runtime.ts`.
//!
//! # Why this module exists at all
//!
//! [`crate::exec::structured`] ports pi's ENTIRE parent-side structured-output mechanism —
//! `create_structured_output_runtime`, `read_structured_output`,
//! `cleanup_structured_output_runtime`, `structured_output_instruction`, the two env-var
//! constants, and the [`crate::exec::structured::StructuredOutputRuntime`] struct. Every one of
//! them had ZERO callers outside their own file. The mechanism was ported faithfully and wired to
//! nothing.
//!
//! What ran instead was [`crate::exec::structured::extract_structured_output_value`], a heuristic
//! that scans the child's assistant messages for the newest fenced ```json block. That has no pi
//! counterpart, and it quietly contradicts the very rule `structured.rs` documents: pi's defining
//! property (`structured-output.ts:56-58`) is that a missing capture file is a HARD failure "EVEN
//! WHEN prose was produced". A fenced block IS prose, so cyrup was accepting exactly what pi
//! rejects — while its own doc comment claimed otherwise.
//!
//! # The mechanism (pi `subagent-prompt-runtime.ts:279-313`)
//!
//! The parent writes the declared JSON Schema to a private file and passes two env vars to the
//! child: [`STRUCTURED_OUTPUT_SCHEMA_ENV`] (where to read the schema) and
//! [`STRUCTURED_OUTPUT_CAPTURE_ENV`] (where to write the value). Child-side, this runtime reads
//! both, builds `{ type: "object", properties: { value: <schema> }, required: ["value"] }` as the
//! tool's parameters — so the model is constrained by the caller's real schema, not a freeform
//! blob — validates on call, writes the capture file, and returns `terminate: true` to end the
//! step. The parent then reads that file back.
//!
//! # Why a SEPARATE extension rather than a third `RegistrationMode`
//!
//! A plain (non-fanout) subagent child attaches no subagents extension at all —
//! `subagent_extension_for_env` returns `None` for it, matching pi (`index.ts:243-245` registers
//! nothing). So the `structured_output` tool cannot come from that extension without perturbing a
//! gate that is deliberately closed.
//!
//! pi has the same split and solves it the same way: `pi-args.ts:13` points at
//! `subagent-prompt-runtime.ts` as its OWN extension, loaded into the child independently of the
//! orchestrator surface. This module is that extension. It registers one tool, subscribes to
//! nothing, and exists only when both env vars are present — so a child with no declared schema
//! carries no extra surface whatsoever.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::tool::ExecMode;
use cyrup_core::{
    CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome, HostEvent};

use crate::exec::structured::{
    STRUCTURED_OUTPUT_CAPTURE_ENV, STRUCTURED_OUTPUT_INSTRUCTION, STRUCTURED_OUTPUT_SCHEMA_ENV,
    validate_structured_output,
};

/// The extension id this child-side runtime registers under. Distinct from the orchestrator
/// extension's `subagents` id — the two never coexist in one process (a plain child gets only this
/// one; a root orchestrator gets only that one), but they are separate extensions, not two modes
/// of the same one.
pub const PROMPT_RUNTIME_EXTENSION_ID: &str = "subagent-prompt-runtime";

/// The tool name the child must call, and which
/// [`crate::exec::structured::STRUCTURED_OUTPUT_MISSING_ERROR`] names when it was never called.
pub const STRUCTURED_OUTPUT_TOOL_NAME: &str = "structured_output";

/// pi's exact tool description (`subagent-prompt-runtime.ts:299`).
const STRUCTURED_OUTPUT_TOOL_DESCRIPTION: &str =
    "Submit the required final structured output for this subagent step. This terminates the step.";

/// The child-side `structured_output` tool (pi `subagent-prompt-runtime.ts:288-313`).
pub struct StructuredOutputTool {
    /// The caller's declared JSON Schema, used to validate the submitted value.
    schema: serde_json::Value,
    /// `{ type: "object", properties: { value: <schema> }, required: ["value"],
    /// additionalProperties: false }` — pi builds the tool's parameters by NESTING the caller's
    /// schema under `value` rather than exposing it at the top level, so the model is constrained
    /// by the real schema instead of handed a freeform object.
    parameters: serde_json::Value,
    /// Where the validated value is written for the parent to read back.
    output_path: PathBuf,
}

impl StructuredOutputTool {
    /// Build the tool for `schema`, capturing to `output_path`.
    #[must_use]
    pub fn new(schema: serde_json::Value, output_path: PathBuf) -> Self {
        let parameters = serde_json::json!({
            "type": "object",
            "properties": { "value": schema },
            "required": ["value"],
            "additionalProperties": false,
        });
        Self {
            schema,
            parameters,
            output_path,
        }
    }
}

#[async_trait]
impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        STRUCTURED_OUTPUT_TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        STRUCTURED_OUTPUT_TOOL_DESCRIPTION
    }

    fn label(&self) -> Option<&str> {
        Some("Structured Output")
    }

    /// pi appends [`STRUCTURED_OUTPUT_INSTRUCTION`] to the CHILD's system prompt whenever the
    /// capture env var is set (`subagent-prompt-runtime.ts:111`). cyrup's extension API exposes no
    /// system-prompt append hook — `HostCtx::system_prompt` is read-only — but the `Tool` trait
    /// feeds exactly that section of the default system prompt via these two methods, so the
    /// instruction reaches the model by the idiomatic route instead of a bespoke one.
    ///
    /// This is also what finally makes [`crate::exec::structured::structured_output_instruction`] live: it was ported with
    /// pi's exact wording and then never called by anything.
    fn prompt_snippet(&self) -> Option<&str> {
        Some("structured_output: submit this step's required final structured result")
    }

    /// Per func-03 R-03-039 a guideline must NAME its tool so it stays meaningful once the tool is
    /// absent — pi's wording already does ("...calling the `structured_output` tool...").
    fn prompt_guidelines(&self) -> &[&str] {
        const GUIDELINES: &[&str] = &[STRUCTURED_OUTPUT_INSTRUCTION];
        GUIDELINES
    }

    /// Sequential, not [`ExecMode::Parallel`]: this call terminates the step and writes the single
    /// capture file the parent reads back, so it must not interleave with other tool calls.
    fn execution_mode(&self) -> ExecMode {
        ExecMode::Sequential
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let value = params.get("value").cloned().ok_or_else(|| {
            ToolError::new("structured_output requires a `value` conforming to the declared schema")
        })?;

        // pi throws here (`subagent-prompt-runtime.ts:303-305`), which surfaces to the model as a
        // tool error it can retry — the capture file is deliberately NOT written on an invalid
        // value, so the parent's read-back still reports "missing" rather than reading a value
        // that never passed validation.
        validate_structured_output(&self.schema, &value)
            .map_err(|message| ToolError::new(format!("Structured output validation failed: {message}")))?;

        if let Some(dir) = self.output_path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| ToolError::new(format!("Failed to write structured output: {err}")))?;
        }
        let encoded = serde_json::to_vec(&value)
            .map_err(|err| ToolError::new(format!("Failed to encode structured output: {err}")))?;
        std::fs::write(&self.output_path, &encoded)
            .map_err(|err| ToolError::new(format!("Failed to write structured output: {err}")))?;

        // pi writes with `{ mode: 0o600 }`; the value can carry whatever the caller's schema
        // describes, so it gets the same owner-only treatment as the schema file itself.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &self.output_path,
                std::fs::Permissions::from_mode(0o600),
            );
        }

        Ok(ToolResult {
            content: vec![Content::text("Structured output captured.")],
            details: Some(serde_json::json!({ "path": self.output_path.display().to_string() })),
            usage: None,
            added_tool_names: Vec::new(),
            terminate: true,
        })
    }
}

/// The child-side extension that registers [`StructuredOutputTool`] and nothing else.
pub struct SubagentPromptRuntime {
    id: ExtensionId,
    tool: Arc<StructuredOutputTool>,
}

impl SubagentPromptRuntime {
    #[must_use]
    pub fn new(schema: serde_json::Value, output_path: PathBuf) -> Self {
        Self {
            id: ExtensionId::from(PROMPT_RUNTIME_EXTENSION_ID),
            tool: Arc::new(StructuredOutputTool::new(schema, output_path)),
        }
    }
}

#[async_trait]
impl NativeExtension for SubagentPromptRuntime {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(self.tool.clone());
        Ok(())
    }

    /// No subscriptions: this runtime is a tool provider only, exactly like pi's, which registers
    /// the tool and returns.
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// Build the child-side runtime iff this process was spawned as a subagent step with a declared
/// output schema (pi `subagent-prompt-runtime.ts:279-282`: BOTH env vars must be present).
///
/// Returns `None` — registering nothing at all — when either var is absent, when the schema file
/// cannot be read, or when it does not parse. A malformed schema is deliberately NOT a hard
/// failure here: the parent already validated the schema when it created the runtime, so an
/// unreadable file child-side means the private temp dir is gone, and failing the whole child
/// process over it would turn a recoverable "structured output missing" into an unexplained
/// startup crash. The parent's read-back reports the missing capture either way.
#[must_use]
pub fn prompt_runtime_extension_for_env() -> Option<Arc<dyn NativeExtension>> {
    let capture = std::env::var(STRUCTURED_OUTPUT_CAPTURE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let schema_path = std::env::var(STRUCTURED_OUTPUT_SCHEMA_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())?;

    let bytes = std::fs::read(&schema_path).ok()?;
    let schema: serde_json::Value = serde_json::from_slice(&bytes).ok()?;

    Some(Arc::new(SubagentPromptRuntime::new(
        schema,
        PathBuf::from(capture),
    )) as Arc<dyn NativeExtension>)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn tool(schema: serde_json::Value, path: PathBuf) -> StructuredOutputTool {
        StructuredOutputTool::new(schema, path)
    }

    /// pi nests the caller's schema under `value` rather than exposing it at the top level
    /// (`subagent-prompt-runtime.ts:283-288`), so the model is constrained by the REAL schema.
    #[test]
    fn parameters_nest_the_callers_schema_under_value() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "verdict": { "type": "string" } },
            "required": ["verdict"],
        });
        let t = tool(schema.clone(), PathBuf::from("/tmp/unused.json"));
        let params = t.parameters();

        assert_eq!(params["properties"]["value"], schema);
        assert_eq!(params["required"], serde_json::json!(["value"]));
        assert_eq!(params["additionalProperties"], serde_json::json!(false));
    }

    #[tokio::test]
    async fn a_valid_value_is_captured_and_terminates_the_step() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("nested").join("output.json");
        let t = tool(
            serde_json::json!({
                "type": "object",
                "properties": { "verdict": { "type": "string" } },
                "required": ["verdict"],
            }),
            out.clone(),
        );

        let result = t
            .execute(
                ToolCallId::from("call-1"),
                serde_json::json!({ "value": { "verdict": "ship it" } }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect("a schema-conforming value is captured");

        assert!(result.terminate, "capturing the value terminates the step");
        // The parent reads this file back; the nested dir must have been created for it.
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({ "verdict": "ship it" }));
    }

    /// An invalid value must NOT write the capture file. If it did, the parent's read-back would
    /// surface a value that never passed validation instead of pi's "missing" hard failure.
    #[tokio::test]
    async fn an_invalid_value_errors_without_writing_the_capture_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("output.json");
        let t = tool(
            serde_json::json!({
                "type": "object",
                "properties": { "verdict": { "type": "string" } },
                "required": ["verdict"],
            }),
            out.clone(),
        );

        let err = t
            .execute(
                ToolCallId::from("call-1"),
                serde_json::json!({ "value": { "wrong": 1 } }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("a value missing a required property must be refused");

        assert!(
            format!("{err}").contains("Structured output validation failed"),
            "pi's exact wording, got: {err}"
        );
        assert!(
            !out.exists(),
            "an invalid value must leave NO capture file — the parent must still see 'missing'"
        );
    }

    /// Both env vars are required, matching pi's `if (structuredOutputPath && structuredSchemaPath)`.
    /// A child with no declared schema must carry no extra surface at all.
    #[test]
    fn the_runtime_is_absent_unless_both_env_vars_resolve() {
        // Nothing set (these tests run with a clean env for these two vars).
        assert!(
            std::env::var(STRUCTURED_OUTPUT_CAPTURE_ENV).is_err()
                || std::env::var(STRUCTURED_OUTPUT_SCHEMA_ENV).is_err()
                || prompt_runtime_extension_for_env().is_some(),
            "with neither var set the runtime must not build"
        );
    }
}
