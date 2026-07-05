//! The `contact_supervisor` tool (`index.ts:1164-1422`), registered ONLY when child-orchestrator
//! metadata is present. `need_decision`/`interview_request` block on a supervisor reply over the
//! broker (the single-slot outbound waiter); `progress_update` is fire-and-forget.

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

use crate::identity::{ChildMessageKind, ChildOrchestratorMetadata, format_child_orchestrator_message, preferred_supervisor_target};
use crate::session_state::SharedIntercomState;
use crate::transport::client::{IntercomClient, SendOptions};

use super::text_result;

/// The `contact_supervisor` tool.
pub struct ContactSupervisorTool {
    state: Arc<SharedIntercomState>,
    metadata: ChildOrchestratorMetadata,
    parameters: serde_json::Value,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContactParams {
    reason: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    interview: Option<serde_json::Value>,
}

impl ContactSupervisorTool {
    /// Build the tool over the shared state + the child's captured orchestrator metadata.
    #[must_use]
    pub fn new(state: Arc<SharedIntercomState>, metadata: ChildOrchestratorMetadata) -> Self {
        Self { state, metadata, parameters: parameters_schema() }
    }

    /// `resolveSupervisorTarget` (`index.ts:880-888`): prefer the stable orchestrator session id
    /// (resolved against the live list), else resolve the orchestrator target, else the raw target.
    async fn resolve_supervisor(&self, client: &Arc<IntercomClient>) -> String {
        if let Some(sid) = &self.metadata.orchestrator_session_id
            && let Ok(Some(target)) = self.state.resolve_target(client, sid).await
        {
            return target;
        }
        match self.state.resolve_target(client, &self.metadata.orchestrator_target).await {
            Ok(Some(target)) => target,
            _ => preferred_supervisor_target(&self.metadata),
        }
    }

    async fn dispatch(&self, params: ContactParams, cancel: &CancelToken) -> Result<ToolResult, ToolError> {
        let client = self
            .state
            .client()
            .ok_or_else(|| ToolError::new("intercom is not connected to the broker"))?;

        let supervisor = self.resolve_supervisor(&client).await;
        if client.session_id().as_deref() == Some(supervisor.as_str()) {
            return Err(ToolError::new("Cannot contact the supervisor: it resolves to this session."));
        }

        match params.reason.as_str() {
            "progress_update" => {
                let message = require(params.message, "progress_update requires `message`")?;
                let text = format_child_orchestrator_message(ChildMessageKind::Update, &self.metadata, &message);
                // Fire-and-forget: send, do not block on a reply (index.ts:1263-1289).
                let result = client
                    .send(&supervisor, SendOptions { text, ..Default::default() })
                    .await
                    .map_err(to_tool_err)?;
                if result.delivered {
                    Ok(text_result(format!("Progress update sent to supervisor ({supervisor}).")))
                } else {
                    Err(ToolError::new(result.reason.unwrap_or_else(|| "update not delivered".to_string())))
                }
            }
            "need_decision" => {
                let message = require(params.message, "need_decision requires `message`")?;
                let text = format_child_orchestrator_message(ChildMessageKind::Ask, &self.metadata, &message);
                let question_id = uuid::Uuid::new_v4().to_string();
                let reply = self
                    .state
                    .ask_and_wait(&client, &supervisor, question_id, text, None, cancel)
                    .await
                    .map_err(to_tool_err)?;
                Ok(text_result(reply))
            }
            "interview_request" => {
                let interview = params
                    .interview
                    .ok_or_else(|| ToolError::new("interview_request requires an `interview` object"))?;
                let body = validate_and_format_interview(&interview, params.message.as_deref())?;
                let text = format_child_orchestrator_message(ChildMessageKind::Interview, &self.metadata, &body);
                let question_id = uuid::Uuid::new_v4().to_string();
                let reply = self
                    .state
                    .ask_and_wait(&client, &supervisor, question_id, text, None, cancel)
                    .await
                    .map_err(to_tool_err)?;
                // TODO(Phase 3 refinement, R-INT-013): parse the reply into `details.structuredReply`
                //   via `parseStructuredSupervisorReply` (index.ts:345-356). The blocking round trip
                //   + raw reply text is faithful today; the structured-reply projection is a
                //   presentation detail that lands with the human-surface phase (P-4) that renders it.
                Ok(text_result(reply))
            }
            other => Err(ToolError::new(format!(
                "unknown contact_supervisor reason \"{other}\" (expected need_decision/progress_update/interview_request)"
            ))),
        }
    }
}

fn require(value: Option<String>, msg: &str) -> Result<String, ToolError> {
    match value {
        Some(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(ToolError::new(msg.to_string())),
    }
}

fn to_tool_err(e: crate::error::IntercomError) -> ToolError {
    ToolError::new(e.to_string())
}

/// `validateSupervisorInterviewRequest` (`index.ts:121-211`, the shape-validation subset): the
/// `interview` must be an object with a non-empty `questions` array, each carrying `id`/`type`/
/// `question`. Returns a human-readable text rendering of the interview for the message body.
fn validate_and_format_interview(interview: &serde_json::Value, message: Option<&str>) -> Result<String, ToolError> {
    let obj = interview
        .as_object()
        .ok_or_else(|| ToolError::new("interview must be an object with a questions array"))?;
    if let Some(title) = obj.get("title")
        && !title.is_string()
    {
        return Err(ToolError::new("interview.title must be a string when provided"));
    }
    let questions = obj
        .get("questions")
        .and_then(|q| q.as_array())
        .ok_or_else(|| ToolError::new("interview.questions must be an array"))?;
    if questions.is_empty() {
        return Err(ToolError::new("interview.questions must not be empty"));
    }

    let mut lines: Vec<String> = Vec::new();
    if let Some(msg) = message.filter(|m| !m.trim().is_empty()) {
        lines.push(msg.to_string());
        lines.push(String::new());
    }
    if let Some(title) = obj.get("title").and_then(|t| t.as_str()) {
        lines.push(format!("Interview: {title}"));
    }
    if let Some(desc) = obj.get("description").and_then(|d| d.as_str()) {
        lines.push(desc.to_string());
    }
    for (i, q) in questions.iter().enumerate() {
        let q_obj = q
            .as_object()
            .ok_or_else(|| ToolError::new(format!("interview.questions[{i}] must be an object")))?;
        let id = q_obj.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::new(format!("interview.questions[{i}].id must be a string"))
        })?;
        let qtype = q_obj.get("type").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::new(format!("interview.questions[{i}].type must be a string"))
        })?;
        if !matches!(qtype, "single" | "multi" | "text" | "image" | "info") {
            return Err(ToolError::new(format!(
                "interview.questions[{i}].type must be one of single/multi/text/image/info"
            )));
        }
        let prompt = q_obj.get("question").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolError::new(format!("interview.questions[{i}].question must be a string"))
        })?;
        lines.push(format!("[{id}] ({qtype}) {prompt}"));
        if let Some(options) = q_obj.get("options").and_then(|v| v.as_array()) {
            let opts: Vec<String> = options.iter().filter_map(|o| o.as_str().map(str::to_string)).collect();
            if !opts.is_empty() {
                lines.push(format!("    options: {}", opts.join(", ")));
            }
        }
    }
    Ok(lines.join("\n"))
}

fn parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "reason": {
                "type": "string",
                "enum": ["need_decision", "progress_update", "interview_request"],
                "description": "Why you are contacting the supervisor."
            },
            "message": { "type": "string", "description": "The message to the supervisor (required for need_decision/progress_update)." },
            "interview": {
                "type": "object",
                "description": "A structured interview request (interview_request).",
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "type": { "type": "string", "enum": ["single", "multi", "text", "image", "info"] },
                                "question": { "type": "string" },
                                "options": { "type": "array", "items": { "type": "string" } },
                                "context": { "type": "string" }
                            },
                            "required": ["id", "type", "question"]
                        }
                    }
                },
                "required": ["questions"]
            }
        },
        "required": ["reason"]
    })
}

#[async_trait]
impl Tool for ContactSupervisorTool {
    fn name(&self) -> &str {
        "contact_supervisor"
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        "Contact your supervising orchestrator over the intercom: need_decision (blocks for an answer), progress_update (fire-and-forget), or interview_request (blocks for structured answers)."
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: ContactParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid contact_supervisor tool call: {e}")))?;
        self.dispatch(parsed, &cancel).await
    }
}
