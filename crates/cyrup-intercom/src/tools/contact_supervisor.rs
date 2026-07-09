//! The `contact_supervisor` tool (`index.ts:1164-1422`), registered ONLY when child-orchestrator
//! metadata is present. `need_decision`/`interview_request` block on a supervisor reply over the
//! broker (the single-slot outbound waiter); `progress_update` is fire-and-forget.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};

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
                let interview_input = params
                    .interview
                    .ok_or_else(|| ToolError::new("interview_request requires an `interview` object"))?;
                let interview = validate_supervisor_interview_request(&interview_input)?;
                let body = format_supervisor_interview_request(&interview, params.message.as_deref());
                let text = format_child_orchestrator_message(ChildMessageKind::Interview, &self.metadata, &body);
                let question_id = uuid::Uuid::new_v4().to_string();
                let reply = self
                    .state
                    .ask_and_wait(&client, &supervisor, question_id, text, None, cancel)
                    .await
                    .map_err(to_tool_err)?;

                // `parseStructuredSupervisorReply` (index.ts:345-356) + the `details` projection at
                // index.ts:1368-1372: `None` (no JSON-looking candidate) -> `{}`, `Some(Ok(_))` ->
                // `{structuredReply: ...}`, `Some(Err(_))` -> `{structuredReplyParseError: ...}`.
                let details = match parse_structured_supervisor_reply(&reply, &interview) {
                    None => serde_json::json!({}),
                    Some(Ok(structured)) => serde_json::json!({ "structuredReply": structured }),
                    Some(Err(parse_error)) => serde_json::json!({ "structuredReplyParseError": parse_error }),
                };

                Ok(ToolResult {
                    content: vec![Content::text(reply)],
                    details: Some(details),
                    terminate: false,
                })
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

/// A validated interview question (`SupervisorInterviewQuestion`, `index.ts:48-53`). `options` is
/// reduced to option labels only: neither `formatSupervisorInterviewRequest` nor
/// `validateSupervisorInterviewReply` consult any option field besides the label
/// (`interviewOptionLabel`, `index.ts:213-215`), so preserving extra option-object fields would be
/// dead weight here.
#[derive(Debug, Clone)]
struct InterviewQuestion {
    id: String,
    r#type: String,
    question: String,
    context: Option<String>,
    options: Option<Vec<String>>,
}

/// A validated interview request (`SupervisorInterviewRequest`, `index.ts:55-59`).
#[derive(Debug, Clone)]
struct Interview {
    title: Option<String>,
    description: Option<String>,
    questions: Vec<InterviewQuestion>,
}

/// A single structured reply response (`SupervisorInterviewReply["responses"][number]`,
/// `index.ts:61-63`).
#[derive(Debug, Clone, serde::Serialize)]
struct StructuredReplyResponse {
    id: String,
    value: serde_json::Value,
}

/// A validated structured supervisor reply (`SupervisorInterviewReply`, `index.ts:61-63`).
#[derive(Debug, Clone, serde::Serialize)]
struct StructuredReply {
    responses: Vec<StructuredReplyResponse>,
}

/// `validateSupervisorInterviewRequest` (`index.ts:121-211`): full 1:1 port — id uniqueness, the
/// per-type `options` requirement (single/multi) / prohibition (text/image/info), option-entry
/// shape validation (non-empty string, or object with a non-empty `label`), and trimming of
/// `id`/`question`/`title`/`description`.
fn validate_supervisor_interview_request(input: &serde_json::Value) -> Result<Interview, ToolError> {
    let obj = input
        .as_object()
        .ok_or_else(|| ToolError::new("interview must be an object with a questions array"))?;

    if let Some(title) = obj.get("title")
        && !title.is_string()
    {
        return Err(ToolError::new("interview.title must be a string when provided"));
    }
    if let Some(description) = obj.get("description")
        && !description.is_string()
    {
        return Err(ToolError::new("interview.description must be a string when provided"));
    }

    let raw_questions = match obj.get("questions").and_then(|q| q.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Err(ToolError::new("interview.questions must be a non-empty array")),
    };

    const VALID_TYPES: [&str; 5] = ["single", "multi", "text", "image", "info"];
    let mut ids: HashSet<String> = HashSet::new();
    let mut questions: Vec<InterviewQuestion> = Vec::with_capacity(raw_questions.len());

    for (index, question_input) in raw_questions.iter().enumerate() {
        let question = question_input
            .as_object()
            .ok_or_else(|| ToolError::new(format!("interview.questions[{index}] must be an object")))?;

        let id = match question.get("id").and_then(|v| v.as_str()).map(str::trim) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => return Err(ToolError::new(format!("interview.questions[{index}].id must be a non-empty string"))),
        };
        if !ids.insert(id.clone()) {
            return Err(ToolError::new(format!("interview question id must be unique: {id}")));
        }

        let qtype = match question.get("type").and_then(|v| v.as_str()) {
            Some(t) if VALID_TYPES.contains(&t) => t.to_string(),
            _ => {
                return Err(ToolError::new(format!(
                    "interview.questions[{index}].type must be one of: single, multi, text, image, info"
                )));
            }
        };

        let prompt = match question.get("question").and_then(|v| v.as_str()).map(str::trim) {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => return Err(ToolError::new(format!("interview.questions[{index}].question must be a non-empty string"))),
        };

        if let Some(context) = question.get("context")
            && !context.is_string()
        {
            return Err(ToolError::new(format!("interview.questions[{index}].context must be a string when provided")));
        }
        let context = question.get("context").and_then(|v| v.as_str()).map(str::to_string);

        let mut options: Option<Vec<String>> = None;
        if let Some(options_value) = question.get("options") {
            let arr = options_value.as_array().ok_or_else(|| {
                ToolError::new(format!("interview.questions[{index}].options must be an array when provided"))
            })?;
            let mut opts = Vec::with_capacity(arr.len());
            for (option_index, option) in arr.iter().enumerate() {
                if let Some(s) = option.as_str() {
                    let label = s.trim();
                    if label.is_empty() {
                        return Err(ToolError::new(format!(
                            "interview.questions[{index}].options[{option_index}] must not be empty"
                        )));
                    }
                    opts.push(label.to_string());
                } else {
                    let label = option
                        .as_object()
                        .and_then(|o| o.get("label"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|l| !l.is_empty());
                    match label {
                        Some(label) => opts.push(label.to_string()),
                        None => {
                            return Err(ToolError::new(format!(
                                "interview.questions[{index}].options[{option_index}] must be a non-empty string or an object with a non-empty label"
                            )));
                        }
                    }
                }
            }
            options = Some(opts);
        }

        let is_choice = qtype == "single" || qtype == "multi";
        if is_choice && options.as_ref().is_none_or(|o| o.is_empty()) {
            return Err(ToolError::new(format!(
                "interview.questions[{index}].options must be a non-empty array for {qtype} questions"
            )));
        }
        if !is_choice && options.is_some() {
            return Err(ToolError::new(format!(
                "interview.questions[{index}].options is only valid for single and multi questions"
            )));
        }

        questions.push(InterviewQuestion { id, r#type: qtype, question: prompt, context, options });
    }

    let title = obj.get("title").and_then(|v| v.as_str()).map(|s| s.trim().to_string());
    let description = obj.get("description").and_then(|v| v.as_str()).map(|s| s.trim().to_string());

    Ok(Interview { title, description, questions })
}

/// `interviewExampleValue` (`index.ts:217-228`).
fn interview_example_value(question: &InterviewQuestion) -> serde_json::Value {
    match question.r#type.as_str() {
        "multi" => {
            let labels: Vec<serde_json::Value> = question
                .options
                .as_ref()
                .map(|opts| opts.iter().take(2).map(|o| serde_json::Value::String(o.clone())).collect())
                .unwrap_or_default();
            serde_json::Value::Array(labels)
        }
        "single" => match question.options.as_ref().and_then(|opts| opts.first()) {
            Some(label) => serde_json::Value::String(label.clone()),
            None => serde_json::Value::String("option label".to_string()),
        },
        "image" => serde_json::Value::String("image/file reference or description".to_string()),
        _ => serde_json::Value::String("answer text".to_string()),
    }
}

/// `formatSupervisorInterviewRequest` (`index.ts:230-274`): the questions body PLUS the
/// machine-parseable JSON response-shape example and reply-format instructions, verbatim.
fn format_supervisor_interview_request(interview: &Interview, message: Option<&str>) -> String {
    let mut lines: Vec<String> = Vec::new();

    if let Some(title) = interview.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        lines.push(format!("Interview: {title}"));
    }
    if let Some(description) = interview.description.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        lines.push(description.to_string());
    }
    if let Some(note) = message.map(str::trim).filter(|m| !m.is_empty()) {
        lines.push(format!("Child note: {note}"));
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }

    lines.push("Questions:".to_string());
    for (index, question) in interview.questions.iter().enumerate() {
        lines.push(format!("{}. [{}] ({}) {}", index + 1, question.id, question.r#type, question.question));
        if let Some(context) = question.context.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            lines.push(format!("   Context: {context}"));
        }
        if let Some(options) = question.options.as_ref().filter(|o| !o.is_empty()) {
            lines.push("   Options:".to_string());
            for option in options {
                lines.push(format!("   - {option}"));
            }
        }
    }

    let response_example = serde_json::json!({
        "responses": interview
            .questions
            .iter()
            .filter(|q| q.r#type != "info")
            .map(|q| serde_json::json!({ "id": q.id, "value": interview_example_value(q) }))
            .collect::<Vec<_>>(),
    });

    lines.push(String::new());
    lines.push("Supervisor reply instructions:".to_string());
    lines.push(
        "Reply with plain JSON or a fenced ```json block using this stable shape. Use the question ids exactly. \
Info questions are context-only and do not need responses. For single questions, value is one option label. \
For multi questions, value is an array of option labels. For text/image questions, value is a string unless \
the question asks otherwise."
            .to_string(),
    );
    lines.push(String::new());
    lines.push("```json".to_string());
    lines.push(serde_json::to_string_pretty(&response_example).unwrap_or_default());
    lines.push("```".to_string());

    lines.join("\n")
}

/// `validateSupervisorInterviewReply` (`index.ts:276-343`): the parsed reply JSON must be an
/// object with a `responses` array; each response must reference a unique, non-info question id
/// present in `value`, with per-type shape rules (single: one option label; multi: array of
/// option labels; text/image: a raw string).
fn validate_supervisor_interview_reply(value: &serde_json::Value, interview: &Interview) -> Result<StructuredReply, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "reply JSON must be an object with a responses array".to_string())?;

    let responses_input = obj
        .get("responses")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "reply JSON must include a responses array".to_string())?;

    let question_by_id: std::collections::HashMap<&str, &InterviewQuestion> = interview
        .questions
        .iter()
        .filter(|q| q.r#type != "info")
        .map(|q| (q.id.as_str(), q))
        .collect();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut responses: Vec<StructuredReplyResponse> = Vec::with_capacity(responses_input.len());

    for (index, response) in responses_input.iter().enumerate() {
        let response_obj = response
            .as_object()
            .ok_or_else(|| format!("responses[{index}] must be an object"))?;

        let id = match response_obj.get("id").and_then(|v| v.as_str()).map(str::trim) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => return Err(format!("responses[{index}].id must be a non-empty string")),
        };
        let question = question_by_id
            .get(id.as_str())
            .ok_or_else(|| format!("responses[{index}].id must match a non-info interview question id"))?;
        if !seen_ids.insert(id.clone()) {
            return Err(format!("responses[{index}].id is duplicated: {id}"));
        }
        if !response_obj.contains_key("value") {
            return Err(format!("responses[{index}].value is required"));
        }
        let value = response_obj.get("value").cloned().unwrap_or(serde_json::Value::Null);

        match question.r#type.as_str() {
            "single" => {
                let s = value
                    .as_str()
                    .ok_or_else(|| format!("responses[{index}].value must be a string for single questions"))?;
                let trimmed = s.trim();
                let option_labels: HashSet<&str> = question.options.iter().flatten().map(String::as_str).collect();
                if !option_labels.contains(trimmed) {
                    return Err(format!("responses[{index}].value must match one of the question options"));
                }
                responses.push(StructuredReplyResponse { id, value: serde_json::Value::String(trimmed.to_string()) });
            }
            "multi" => {
                let arr = value
                    .as_array()
                    .ok_or_else(|| format!("responses[{index}].value must be an array of strings for multi questions"))?;
                let mut selected: Vec<String> = Vec::with_capacity(arr.len());
                for item in arr {
                    let s = item.as_str().ok_or_else(|| {
                        format!("responses[{index}].value must be an array of strings for multi questions")
                    })?;
                    selected.push(s.trim().to_string());
                }
                let option_labels: HashSet<&str> = question.options.iter().flatten().map(String::as_str).collect();
                if let Some(invalid) = selected.iter().find(|item| !option_labels.contains(item.as_str())) {
                    return Err(format!(
                        "responses[{index}].value contains an option that is not in the question options: {invalid}"
                    ));
                }
                responses.push(StructuredReplyResponse {
                    id,
                    value: serde_json::Value::Array(selected.into_iter().map(serde_json::Value::String).collect()),
                });
            }
            other => {
                let s = value
                    .as_str()
                    .ok_or_else(|| format!("responses[{index}].value must be a string for {other} questions"))?;
                responses.push(StructuredReplyResponse { id, value: serde_json::Value::String(s.to_string()) });
            }
        }
    }

    Ok(StructuredReply { responses })
}

/// Extracts the first fenced code-block body (optionally tagged ` ```json `), mirroring the JS
/// regex `` /```(?:json)?\s*([\s\S]*?)```/i `` (`index.ts:346`): find the first opening fence,
/// skip an optional case-insensitive `json` tag and following whitespace, then take everything up
/// to the next fence.
fn extract_fenced_block(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after_open = text.get(start + 3..)?;
    let looks_like_json_tag =
        after_open.len() >= 4 && after_open.as_bytes().get(..4).is_some_and(|b| b.eq_ignore_ascii_case(b"json"));
    let after_tag = if looks_like_json_tag { after_open.get(4..)? } else { after_open };
    let after_ws = after_tag.trim_start();
    let close = after_ws.find("```")?;
    Some(after_ws.get(..close)?.to_string())
}

/// `parseStructuredSupervisorReply` (`index.ts:345-356`): `None` when the reply text (or its first
/// fenced block) doesn't look like JSON (doesn't start with `{`/`[`); otherwise `Some(Ok(_))` on a
/// valid structured reply or `Some(Err(_))` carrying the JSON-parse or validation error message.
fn parse_structured_supervisor_reply(text: &str, interview: &Interview) -> Option<Result<StructuredReply, String>> {
    let candidate = extract_fenced_block(text).unwrap_or_else(|| text.to_string());
    let candidate = candidate.trim();
    if !candidate.starts_with('{') && !candidate.starts_with('[') {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(candidate) {
        Ok(value) => Some(validate_supervisor_interview_reply(&value, interview)),
        Err(e) => Some(Err(e.to_string())),
    }
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn sample_interview(extra_question: Option<serde_json::Value>) -> serde_json::Value {
        let mut questions = vec![serde_json::json!({
            "id": "q1",
            "type": "single",
            "question": "Pick one",
            "options": ["a", "b"],
        })];
        if let Some(q) = extra_question {
            questions.push(q);
        }
        serde_json::json!({ "title": "T", "questions": questions })
    }

    // --- Item 1: validateSupervisorInterviewRequest full rule set ---

    #[test]
    fn rejects_duplicate_question_ids() {
        let input = sample_interview(Some(serde_json::json!({
            "id": "q1",
            "type": "text",
            "question": "Another",
        })));
        let err = validate_supervisor_interview_request(&input).unwrap_err();
        assert!(err.message.contains("must be unique"), "{}", err.message);
    }

    #[test]
    fn rejects_single_question_without_options() {
        let input = serde_json::json!({
            "questions": [{ "id": "q1", "type": "single", "question": "Pick one" }]
        });
        let err = validate_supervisor_interview_request(&input).unwrap_err();
        assert!(err.message.contains("options must be a non-empty array"), "{}", err.message);
    }

    #[test]
    fn rejects_options_on_non_choice_question() {
        let input = serde_json::json!({
            "questions": [{ "id": "q1", "type": "text", "question": "Say something", "options": ["a"] }]
        });
        let err = validate_supervisor_interview_request(&input).unwrap_err();
        assert!(err.message.contains("only valid for single and multi"), "{}", err.message);
    }

    #[test]
    fn rejects_malformed_option_entry() {
        let input = serde_json::json!({
            "questions": [{ "id": "q1", "type": "single", "question": "Pick", "options": [42] }]
        });
        let err = validate_supervisor_interview_request(&input).unwrap_err();
        assert!(err.message.contains("non-empty string or an object with a non-empty label"), "{}", err.message);
    }

    #[test]
    fn accepts_valid_interview_and_trims_fields() {
        let input = serde_json::json!({
            "title": "  My Title  ",
            "questions": [{ "id": " q1 ", "type": "single", "question": " Pick one ", "options": [" a ", { "label": " b " }] }]
        });
        let interview = validate_supervisor_interview_request(&input).expect("should validate");
        assert_eq!(interview.title.as_deref(), Some("My Title"));
        assert_eq!(interview.questions[0].id, "q1");
        assert_eq!(interview.questions[0].question, "Pick one");
        assert_eq!(interview.questions[0].options.as_deref(), Some(&["a".to_string(), "b".to_string()][..]));
    }

    // --- Item 2: reply-instructions block + structured-reply parsing ---

    #[test]
    fn format_includes_json_reply_instructions_block() {
        let interview = validate_supervisor_interview_request(&sample_interview(None)).expect("valid");
        let body = format_supervisor_interview_request(&interview, None);
        assert!(body.contains("Supervisor reply instructions:"), "{body}");
        assert!(body.contains("```json"), "{body}");
        assert!(body.contains("\"responses\""), "{body}");
        assert!(body.contains("\"id\": \"q1\""), "{body}");
    }

    #[test]
    fn parses_fenced_structured_reply() {
        let interview = validate_supervisor_interview_request(&sample_interview(None)).expect("valid");
        let reply_text = "Sure, here you go:\n```json\n{\"responses\": [{\"id\": \"q1\", \"value\": \"a\"}]}\n```\n";
        let parsed = parse_structured_supervisor_reply(reply_text, &interview).expect("should attempt parse");
        let structured = parsed.expect("should be valid");
        assert_eq!(structured.responses.len(), 1);
        assert_eq!(structured.responses[0].id, "q1");
        assert_eq!(structured.responses[0].value, serde_json::Value::String("a".to_string()));
    }

    #[test]
    fn rejects_reply_value_not_in_options() {
        let interview = validate_supervisor_interview_request(&sample_interview(None)).expect("valid");
        let reply_text = "{\"responses\": [{\"id\": \"q1\", \"value\": \"nope\"}]}";
        let parsed = parse_structured_supervisor_reply(reply_text, &interview).expect("should attempt parse");
        let err = parsed.unwrap_err();
        assert!(err.contains("must match one of the question options"), "{err}");
    }

    #[test]
    fn non_json_reply_yields_no_structured_attempt() {
        let interview = validate_supervisor_interview_request(&sample_interview(None)).expect("valid");
        assert!(parse_structured_supervisor_reply("just a plain reply", &interview).is_none());
    }
}
