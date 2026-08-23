//! `ask_user_question` — the native tool bridging code-puppy flux's structured mid-task
//! question tool onto `HostServices::select` under the [`cyrup_ext::HumanInteractionLock`] (port doc
//! §3.4.4 / §5.1). Closes the only real capability gap between code-puppy flux and cyrup flux:
//! everything else ports as a prompt template, but a mid-turn structured question needs a real
//! tool call.

use std::sync::{Arc, OnceLock};

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use cyrup_ext::host::{DialogOptions, HostServices};
use serde::Deserialize;

/// One `options[]` entry (schema: `{label: string, description?: string}`, `label` required).
#[derive(Debug, Deserialize)]
struct QuestionOption {
    label: String,
    #[serde(default)]
    description: Option<String>,
}

/// The tool's parameter shape (schema in [`AskUserQuestionTool::new`]'s doc comment).
#[derive(Debug, Deserialize)]
struct AskParams {
    question: String,
    #[serde(default)]
    header: Option<String>,
    options: Vec<QuestionOption>,
    #[serde(default)]
    multiple: bool,
}

const CANCELLED: &str = "(cancelled — no selection made)";
const DONE_ROW: &str = "\u{2714} Done";

/// The `ask_user_question` native tool.
pub struct AskUserQuestionTool {
    host: Arc<OnceLock<Arc<dyn HostServices>>>,
    params: serde_json::Value,
}

impl AskUserQuestionTool {
    /// `host` is the SAME `OnceLock` [`crate::extension::FluxExtension`] holds — cloned in here so
    /// `set_host_services` (`native.rs:683`) binds both the extension and this tool at once.
    #[must_use]
    pub fn new(host: Arc<OnceLock<Arc<dyn HostServices>>>) -> Self {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The question to ask the user" },
                "header":   { "type": "string", "description": "Short category label shown with the question" },
                "options":  {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label":       { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["label"]
                    },
                    "minItems": 2,
                    "maxItems": 4
                },
                "multiple": { "type": "boolean", "description": "Allow selecting several options (default false)" }
            },
            "required": ["question", "options"]
        });
        Self { host, params }
    }

    /// Project `options` into `(display_row, label)` pairs — the `oauth_select` CYRUP-DELTA
    /// pattern (`cyrup-session-svc/src/host_services.rs:1696-1731`): `select` takes a flat array
    /// of option STRINGS and replies with the chosen STRING, with no carrier for a per-option
    /// description, so the description is folded into the display row and mapped back afterward.
    fn rows(options: &[QuestionOption]) -> Vec<(String, String)> {
        options
            .iter()
            .map(|o| {
                let display = match &o.description {
                    Some(d) if !d.trim().is_empty() => format!("{} — {}", o.label, d),
                    _ => o.label.clone(),
                };
                (display, o.label.clone())
            })
            .collect()
    }

    /// Resolve one blocking `select` round trip. `question`/`header` build the prompt exactly as
    /// code-puppy's tool shows it; `rows` is the option set still offered (multi-select removes an
    /// already-chosen option). Returns the label the user picked, or `None` on cancel/no-host.
    async fn select_once(
        host: Arc<dyn HostServices>,
        prompt: String,
        rows: Vec<(String, String)>,
    ) -> Option<String> {
        // `HostServices::select` is sync and blocking (the trait doc: "all methods are sync — the
        // host runs them on its own executor"); hop it off the async executor so the tool's own
        // task doesn't block the runtime while a human thinks.
        tokio::task::spawn_blocking(move || {
            let labels: Vec<serde_json::Value> =
                rows.iter().map(|(display, _)| serde_json::Value::String(display.clone())).collect();
            let picked =
                host.select(&prompt, &serde_json::Value::Array(labels), &DialogOptions::default())?;
            // Map the chosen DISPLAY ROW back to its bare label; two options with identical labels
            // resolve to the first (the same documented caveat `oauth_select` carries). Fall back
            // to matching the reply against the labels directly, so a renderer that echoed a label
            // straight through (rather than the full display row) still resolves.
            rows.iter()
                .find(|(display, _)| *display == picked)
                .or_else(|| rows.iter().find(|(_, label)| *label == picked))
                .map(|(_, label)| label.clone())
        })
        .await
        .ok()
        .flatten()
    }

    fn build_prompt(question: &str, header: Option<&str>) -> String {
        match header {
            Some(h) if !h.trim().is_empty() => format!("{h}: {question}"),
            _ => question.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "ask_user_question"
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }

    fn description(&self) -> &str {
        "Ask the user a structured multiple-choice question mid-task and return their selection; \
         prefer this over plain-text questions when the options are known."
    }

    fn label(&self) -> Option<&str> {
        Some("Ask")
    }

    fn prompt_snippet(&self) -> Option<&str> {
        Some(
            "ask_user_question(question, options[2-4], header?, multiple?) — ask a structured, \
             multiple-choice question mid-task and get the user's selection back.",
        )
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let Some(host) = self.host.get().cloned() else {
            return Err(ToolError::new("ask_user_question: no interactive host"));
        };

        let parsed: AskParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("ask_user_question: invalid parameters: {e}")))?;
        if parsed.options.len() < 2 || parsed.options.len() > 4 {
            return Err(ToolError::new(format!(
                "ask_user_question: `options` must have 2-4 entries, got {}",
                parsed.options.len()
            )));
        }

        let Some(lock) = host.human_interaction_lock() else {
            return Err(ToolError::new("ask_user_question: interaction lock unavailable"));
        };
        // Hold the guard across the WHOLE dialog (including the multi-select loop below): a
        // second question, or a permission dialog, waits rather than opening an overlapping
        // prompt. Releases on drop, including on a panic unwind.
        let _guard = lock.acquire().await;

        let prompt = Self::build_prompt(&parsed.question, parsed.header.as_deref());
        let answer = if parsed.multiple {
            let mut remaining = parsed.options;
            let mut chosen: Vec<String> = Vec::new();
            loop {
                if cancel.is_cancelled() {
                    break CANCELLED.to_string();
                }
                if remaining.is_empty() {
                    break chosen.join(", ");
                }
                let mut rows = vec![(DONE_ROW.to_string(), DONE_ROW.to_string())];
                rows.extend(Self::rows(&remaining));
                let picked = tokio::select! {
                    biased;
                    () = cancel.cancelled() => None,
                    p = Self::select_once(Arc::clone(&host), prompt.clone(), rows) => p,
                };
                match picked {
                    None => break CANCELLED.to_string(),
                    Some(label) if label == DONE_ROW => break chosen.join(", "),
                    Some(label) => {
                        remaining.retain(|o| o.label != label);
                        chosen.push(label);
                    }
                }
            }
        } else {
            let rows = Self::rows(&parsed.options);
            let picked = tokio::select! {
                biased;
                () = cancel.cancelled() => None,
                p = Self::select_once(Arc::clone(&host), prompt, rows) => p,
            };
            picked.unwrap_or_else(|| CANCELLED.to_string())
        };

        Ok(ToolResult { content: vec![Content::text(answer)], ..Default::default() })
    }
}
