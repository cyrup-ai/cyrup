//! Tool result finalization: normalise the executed outcome, run `after_tool_call`, and fold the
//! replace-not-merge table into a `Finalized`.

use super::Finalized;
use crate::agent::message::empty_details;
use crate::agent::run::RunCtx;
use crate::agent::util::now_millis;
use crate::event::{AgentMessage, ToolResultMessage};
use crate::hooks::{AfterOutcome, AfterToolCall, AgentContextView};
use cyrup_core::{AssistantMessage, Content, TerminateHint, ToolCall, ToolError, ToolResult};
use serde_json::Value;
use std::sync::Arc;

/// Pi `ExecutedToolCallOutcome { result, isError }` (`agent-loop.ts:569-572`): the tool's own
/// outcome after the throw→error-result conversion, BEFORE `after_tool_call` sees it.
pub(super) struct Executed {
    pub(super) result: ToolResult,
    pub(super) is_error: bool,
}

impl From<Result<ToolResult, ToolError>> for Executed {
    fn from(outcome: Result<ToolResult, ToolError>) -> Self {
        match outcome {
            // AGENT-009 — `terminate` is optional upstream (`AgentToolResult.terminate?`,
            // types.ts:354-368) and `TerminateHint` carries all three of its values through
            // unchanged: `Unspecified` puts no key on the wire, `Continue` puts an explicit `false`.
            Ok(result) => Self {
                result,
                is_error: false,
            },
            // A throwing TOOL yields `createErrorToolResult(...)` (`agent-loop.ts:700-703`
            // @v0.83.0), i.e. `details: {}` and no `terminate`.
            Err(e) => Self {
                result: ToolResult {
                    content: vec![Content::text(e.to_string())],
                    details: Some(empty_details()),
                    usage: None,
                    added_tool_names: Vec::new(),
                    terminate: TerminateHint::Unspecified,
                },
                is_error: true,
            },
        }
    }
}

impl RunCtx {
    /// Finalize one executed call: normalise → `after_tool_call` → fold. The two runtimes call
    /// this; it composes the three steps and does nothing else.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn finalize(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[Arc<AgentMessage>],
        call: &ToolCall,
        source_index: usize,
        args: Value,
        outcome: Result<ToolResult, ToolError>,
    ) -> Finalized {
        let executed = Executed::from(outcome);
        let hook = self
            .after_hook(assistant, ctx_messages, call, &args, &executed)
            .await;
        fold_tool_outcome(call, source_index, executed, hook)
    }

    /// The shell: the one await in finalization. Builds the read-side view of the executed
    /// result (Pi `AfterToolCallContext`, types.ts:100-113) and returns the hook's verdict
    /// UNINTERPRETED — [`AfterOutcome::Failed`] is a third outcome of the fold, not an absence
    /// (see [`fold_tool_outcome`]).
    pub(super) async fn after_hook(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[Arc<AgentMessage>],
        call: &ToolCall,
        args: &Value,
        executed: &Executed,
    ) -> AfterOutcome {
        let ctx = AfterToolCall {
            tool_name: &call.name,
            tool_call_id: &call.id,
            args,
            content: &executed.result.content,
            details: executed.result.details.as_ref(),
            usage: executed.result.usage.as_ref(),
            is_error: executed.is_error,
            terminate: executed.result.terminate,
            assistant_message: assistant,
            tool_call: call,
            context: AgentContextView {
                system_prompt: &self.system_prompt,
                messages: ctx_messages,
                tools: &self.tools,
            },
        };
        self.hooks.after_tool_call(ctx, self.cancel.child()).await
    }
}

/// The pure fold: pi `finalizeExecutedToolCall`'s three-way table (`agent-loop.ts:724-750`) as a
/// total function of its inputs, with no await and no `self`. Replace-not-merge per field
/// (R-02-025). `added_tool_names` rides through untouched on the override arm: pi spreads
/// `{...result}` before applying the hook's explicit fields (`:736-742`) and `addedToolNames` is
/// not one of them, so no hook can set or clear it — only the throw arm drops it.
pub(super) fn fold_tool_outcome(
    call: &ToolCall,
    source_index: usize,
    executed: Executed,
    hook: AfterOutcome,
) -> Finalized {
    let Executed {
        result,
        mut is_error,
    } = executed;
    // Exhaustive destructure: a field added to `ToolResult` must be placed in this table.
    let ToolResult {
        mut content,
        mut details,
        mut usage,
        mut added_tool_names,
        mut terminate,
    } = result;
    match hook {
        AfterOutcome::Override(ov) => {
            if let Some(c) = ov.content {
                content = c;
            }
            if let Some(d) = ov.details {
                details = Some(d);
            }
            // Replace-not-merge, the same rule as `content`/`details` (Pi
            // `usage: afterResult.usage ?? result.usage`, agent-loop.ts:738; types.ts:70-78:
            // "There is no deep merge for `content`, `details`, or `usage`").
            if let Some(u) = ov.usage {
                usage = Some(u);
            }
            if let Some(e) = ov.is_error {
                is_error = e;
            }
            // Pi `terminate: afterResult.terminate ?? result.terminate` (agent-loop.ts:739
            // @v0.83.0): an absent hook value keeps whatever the tool set.
            if let Some(t) = ov.terminate {
                terminate = t;
            }
        }
        AfterOutcome::Keep => {}
        AfterOutcome::Failed(e) => {
            // Pi discards the whole result for `createErrorToolResult(…)` when the hook throws
            // (agent-loop.ts:743-745), and that carries neither usage nor added tool names. The
            // replacement content is the thrown value's own text
            // (`error instanceof Error ? error.message : String(error)`, agent-loop.ts:744), so
            // the failing hook's reason — not a fixed label — is what the model reads back.
            content = vec![Content::text(e.to_string())];
            // AGENT-009 — `createErrorToolResult` sets `details: {}` (agent-loop.ts:756-761).
            details = Some(empty_details());
            usage = None;
            added_tool_names = Vec::new();
            is_error = true;
            terminate = TerminateHint::Unspecified;
        }
    }

    let message = ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content,
        details,
        usage,
        added_tool_names,
        is_error,
        // Pi `createToolResultMessage` stamps every tool result with `Date.now()`
        // (agent-loop.ts:741); this reaches the wire payload via `convert_to_llm`.
        timestamp: now_millis(),
    };
    Finalized::new(source_index, message, terminate)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::error::HookError;
    use crate::event::AgentEvent;
    use crate::hooks::AfterOverride;
    use cyrup_core::{ToolCallId, Usage};
    use serde_json::json;

    fn call() -> ToolCall {
        ToolCall {
            id: ToolCallId::from("call-1"),
            name: "echo".to_string(),
            arguments: serde_json::Map::new().into(),
            thought_signature: None,
        }
    }

    fn usage(input: u64) -> Usage {
        Usage {
            input,
            ..Usage::default()
        }
    }

    fn ok_result() -> ToolResult {
        ToolResult {
            content: vec![Content::text("tool said")],
            details: Some(json!({ "k": "v" })),
            usage: Some(usage(11)),
            added_tool_names: vec!["late".to_string()],
            terminate: TerminateHint::Terminate,
        }
    }

    fn text_of(c: &[Content]) -> Vec<String> {
        c.iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    /// A throwing tool becomes pi's `createErrorToolResult`: the error's own text, `details: {}`,
    /// and no usage, no added tools, no `terminate`.
    #[test]
    fn executed_from_err_is_pi_error_tool_result() {
        let ex = Executed::from(Err::<ToolResult, _>(ToolError::new("boom")));
        assert!(ex.is_error);
        assert_eq!(text_of(&ex.result.content), vec!["boom".to_string()]);
        assert_eq!(ex.result.details, Some(json!({})));
        assert!(ex.result.usage.is_none());
        assert!(ex.result.added_tool_names.is_empty());
        assert_eq!(ex.result.terminate, TerminateHint::Unspecified);

        let ok = Executed::from(Ok::<_, ToolError>(ok_result()));
        assert!(!ok.is_error);
        assert_eq!(ok.result.terminate, TerminateHint::Terminate);
    }

    /// `Keep`: the tool's result stands in full, the index rides through, and the derived
    /// `tool_execution_end.result` carries `terminate: true` and `addedToolNames`.
    #[test]
    fn fold_keep_passes_the_result_through() {
        let fin = fold_tool_outcome(
            &call(),
            3,
            Executed::from(Ok(ok_result())),
            AfterOutcome::Keep,
        );
        assert_eq!(fin.source_index(), 3);
        assert_eq!(fin.terminate(), TerminateHint::Terminate);
        let AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } = fin.end_event()
        else {
            panic!("end_event is ToolExecutionEnd");
        };
        assert_eq!(tool_call_id, ToolCallId::from("call-1"));
        assert_eq!(tool_name, "echo");
        assert!(!is_error);
        assert_eq!(result["terminate"], json!(true));
        assert_eq!(result["addedToolNames"], json!(["late"]));
        assert_eq!(result["usage"]["input"], json!(11));
        let msg = fin.into_message();
        assert_eq!(msg.usage, Some(usage(11)));
        assert_eq!(msg.added_tool_names, vec!["late".to_string()]);
        assert!(!msg.is_error);
    }

    /// `Override` is replace-not-merge PER FIELD: only the fields the hook set change; a
    /// `Some(Unspecified)` terminate CLEARS the tool's hint; `added_tool_names` cannot be touched.
    #[test]
    fn fold_override_replaces_only_the_given_fields() {
        let over = AfterOverride {
            content: Some(vec![Content::text("patched")]),
            usage: Some(usage(700)),
            terminate: Some(TerminateHint::Unspecified),
            ..AfterOverride::default()
        };
        let fin = fold_tool_outcome(
            &call(),
            0,
            Executed::from(Ok(ok_result())),
            AfterOutcome::Override(over),
        );
        assert_eq!(
            fin.terminate(),
            TerminateHint::Unspecified,
            "Some(Unspecified) clears the hint"
        );
        let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = fin.end_event()
        else {
            panic!("end_event is ToolExecutionEnd");
        };
        assert!(
            result.get("terminate").is_none(),
            "cleared hint puts no key on the wire"
        );
        assert!(!is_error, "is_error untouched when the hook left it None");
        let msg = fin.into_message();
        assert_eq!(text_of(&msg.content), vec!["patched".to_string()]);
        assert_eq!(msg.details, Some(json!({ "k": "v" })), "details kept");
        assert_eq!(msg.usage, Some(usage(700)), "usage replaced in full");
        assert_eq!(
            msg.added_tool_names,
            vec!["late".to_string()],
            "not a hook field"
        );

        let flip = AfterOverride {
            is_error: Some(true),
            ..AfterOverride::default()
        };
        let fin = fold_tool_outcome(
            &call(),
            0,
            Executed::from(Ok(ok_result())),
            AfterOutcome::Override(flip),
        );
        assert!(fin.into_message().is_error);
    }

    /// `Failed`: the WHOLE result becomes an error result carrying the hook's own text — usage and
    /// added tools dropped, `terminate` cleared — pi's `catch` in `finalizeExecutedToolCall`.
    #[test]
    fn fold_failed_replaces_the_whole_result() {
        let fin = fold_tool_outcome(
            &call(),
            1,
            Executed::from(Ok(ok_result())),
            AfterOutcome::Failed(HookError::new("redaction pass failed on block 3")),
        );
        assert_eq!(fin.terminate(), TerminateHint::Unspecified);
        let msg = fin.into_message();
        assert!(msg.is_error);
        assert_eq!(
            text_of(&msg.content),
            vec!["redaction pass failed on block 3".to_string()]
        );
        assert_eq!(msg.details, Some(json!({})));
        assert!(msg.usage.is_none());
        assert!(msg.added_tool_names.is_empty());
    }

    /// `Finalized::new` derives the event payload from the message: absent optionals produce NO
    /// key, and the event's id/name/is_error come from the message itself.
    #[test]
    fn finalized_new_derives_the_event_payload_from_the_message() {
        let message = ToolResultMessage {
            tool_call_id: ToolCallId::from("c9"),
            tool_name: "t".to_string(),
            content: vec![Content::text("x")],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            is_error: true,
            timestamp: now_millis(),
        };
        let fin = Finalized::new(7, message, TerminateHint::Continue);
        assert_eq!(fin.source_index(), 7);
        let AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } = fin.end_event()
        else {
            panic!("end_event is ToolExecutionEnd");
        };
        assert_eq!(tool_call_id, ToolCallId::from("c9"));
        assert_eq!(tool_name, "t");
        assert!(is_error);
        assert_eq!(
            result["terminate"],
            json!(false),
            "explicit Continue IS on the wire"
        );
        assert!(result.get("details").is_none());
        assert!(result.get("usage").is_none());
        assert!(result.get("addedToolNames").is_none());
    }
}
