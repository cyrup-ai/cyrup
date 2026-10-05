//! SUBA-150 — the `subagent` tool's ONE workflow field.
//!
//! pi `0538e14d` (#2588, `feat(workflows)!`, v0.74.0) is a declared breaking change to the tool
//! surface: `workflowScript` and `workflowScriptPath` left the tool and one `workflow` field took
//! their place (`extension/schemas.ts:223-226` @v0.75.0). The field carries three different kinds
//! of value, and upstream decides which at exactly one place — `normalizePublicSubagentExecution`
//! plus the resolution block at `subagent-executor.ts:7928-7958` @v0.75.0:
//!
//! * `true` — run the single ` ```js workflow ` fenced block written in the SAME assistant reply
//!   that issued this tool call. The string `"true"` means the same thing, because some MCP
//!   clients stringify the boolean branch of a union (`df3b6df1`, #2610, v0.75.0:
//!   `public-execution.ts:105`, `reply-workflow-script.ts:29`, `rpc.ts:527`, `index.ts:744`).
//! * a string containing `/` or `\` — a script FILE, read from the request cwd
//!   (`isWorkflowScriptPath`, `public-execution.ts:62-64`; `readWorkflowScriptFile`,
//!   `subagent-executor.ts:559-571`).
//! * any other non-blank string — a named workflow RESOURCE, resolved through the registry
//!   (`resolveWorkflowResource`, `subagent-executor.ts:7941`).
//!
//! # Why a domain enum rather than a `Value` re-inspected per use
//!
//! `docs/RUST-DESIGN-REVIEW.md`'s "Explicit domain enums": the three kinds are a closed domain
//! classification, decided by a `/`-or-`\` scan and a `true`/`"true"` test that must give the same
//! answer at every reader. Upstream re-derives it four times — `hasNamedWorkflow`
//! (`public-execution.ts:110`), `hasRawScript` (`:114`), the resource branch (`:7941`) and the
//! raw-script branch (`:7948`) — and that is precisely how a value named `"true"` once became a
//! resource lookup (#2600, the defect `df3b6df1` fixed). [`WorkflowSource::parse`] is the one
//! classification; downstream code matches on the variant and cannot ask the question again.
//!
//! **What becomes impossible:** a reader that treats `workflow: "true"` as a resource name, or a
//! path as a resource, or that forgets the blank-string refusal. **What still must be tested:**
//! that `parse` is called at the single boundary (`Tool::execute`) — the enum cannot force its own
//! construction — and that each variant's RESOLUTION reads the right thing, which is
//! [`crate::extension::tool::SubagentTool::lower_workflow_field`]'s job and is tested there.
//!
//! # The internal carrier
//!
//! Resolution rewrites the request's `workflow` into `workflowScript`, exactly as upstream does
//! (`publicParams = { ...withoutWorkflowSource, workflowScript }`, `:7957`). `workflowScript`
//! survives as the package's INTERNAL carrier upstream too (`disabled-features.ts:106-110` names
//! it as such), which is why the whole scripted-workflow engine, its `NESTED_WORKFLOW_REFUSAL`
//! and the scheduled-run target shape need no rename here.

use serde_json::{Map, Value};

/// pi `REMOVED_WORKFLOW_SCRIPT` (`public-execution.ts:56` @v0.75.0), verbatim.
pub(crate) const REMOVED_WORKFLOW_SCRIPT: &str = "workflowScript was removed; write the script in one ```js workflow block in this reply and call subagent({ workflow: true }), or pass a script file as workflow: \"./path/to/script.js\".";

/// pi `REMOVED_WORKFLOW_SCRIPT_PATH` (`public-execution.ts:57` @v0.75.0), verbatim.
pub(crate) const REMOVED_WORKFLOW_SCRIPT_PATH: &str = "workflowScriptPath was removed; pass the script file as workflow: \"./path/to/script.js\" (a workflow value containing '/' is a path).";

/// pi `scriptTextHint` (`subagent-executor.ts:7934` @v0.75.0), verbatim — appended to a script-FILE
/// read failure and to a RESOURCE refusal, and deliberately NOT to a reply-block refusal, whose own
/// text already explains the reply contract.
pub(crate) const SCRIPT_TEXT_HINT: &str = " A workflow string is a named workflow resource or a script file path. To run script text, write it in one ```js workflow block in the same reply and call subagent({ workflow: true }).";

/// pi's invalid-`workflow` refusal (`public-execution.ts:108` @v0.75.0), verbatim.
pub(crate) const INVALID_WORKFLOW_VALUE: &str = "workflow must be true (the ```js workflow block in this reply), a script path containing '/', or a named workflow resource.";

/// pi `"args requires workflow."` (`public-execution.ts:118` @v0.75.0), verbatim.
pub(crate) const ARGS_REQUIRES_WORKFLOW: &str = "args requires workflow.";

/// pi `"workflow cannot be combined with an internal workflowScript."` (`:112` @v0.75.0).
pub(crate) const WORKFLOW_WITH_INTERNAL_SCRIPT: &str =
    "workflow cannot be combined with an internal workflowScript.";

/// Which of the three kinds of value the `workflow` field carried — pi's `workflow === true` /
/// `isWorkflowScriptPath` / named-resource split, decided ONCE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowSource {
    /// `workflow: true`, or the string `"true"` an MCP client sent for it (#2610). The script is
    /// the one ` ```js workflow ` block in the assistant reply that issued this tool call.
    ReplyBlock,
    /// A script FILE path — the value contained `/` or `\`. Resolved against the request cwd.
    ScriptFile(String),
    /// A named, extension-owned workflow RESOURCE.
    Resource(String),
}

impl WorkflowSource {
    /// Classify a raw `workflow` value.
    ///
    /// The `"true"` coercion comes FIRST, before any branching, exactly as upstream placed it
    /// (`public-execution.ts:103-105`: *"Coerce it here so no path can resolve it as a named
    /// resource or script file called \"true\""*).
    ///
    /// # Errors
    ///
    /// [`INVALID_WORKFLOW_VALUE`] for `false`, a blank string, or any non-string non-`true` value —
    /// upstream's single message for the whole invalid set (`:107-109`).
    pub(crate) fn parse(value: &Value) -> Result<Self, &'static str> {
        // #2610 — the string "true" IS the boolean branch; never a resource and never a file.
        if value == &Value::Bool(true) || value.as_str() == Some("true") {
            return Ok(Self::ReplyBlock);
        }
        let Some(raw) = value.as_str().filter(|text| !text.trim().is_empty()) else {
            return Err(INVALID_WORKFLOW_VALUE);
        };
        if is_workflow_script_path(raw) {
            Ok(Self::ScriptFile(raw.to_string()))
        } else {
            Ok(Self::Resource(raw.to_string()))
        }
    }
}

/// pi `isWorkflowScriptPath` (`public-execution.ts:62-64` @v0.75.0): *"A workflow value containing
/// `/` or `\` is a script path; named workflow resource names contain neither."*
#[must_use]
pub(crate) fn is_workflow_script_path(workflow: &str) -> bool {
    workflow.contains('/') || workflow.contains('\\')
}

/// pi `removedModelWorkflowFieldError` (`public-execution.ts:70-74` @v0.75.0) — the two parameters
/// v0.74.0 deleted from the tool, refused by NAME with the sentence that names the replacement.
///
/// Key PRESENCE is the test, as upstream's `Object.hasOwn` is: a `workflowScript: null` is still a
/// caller reaching for the removed parameter.
#[must_use]
pub(crate) fn removed_model_workflow_field_error(
    request: &Map<String, Value>,
) -> Option<&'static str> {
    if request.contains_key("workflowScript") {
        return Some(REMOVED_WORKFLOW_SCRIPT);
    }
    if request.contains_key("workflowScriptPath") {
        return Some(REMOVED_WORKFLOW_SCRIPT_PATH);
    }
    None
}

/// How many `subagent` tool calls in one assistant reply asked for the reply block — pi
/// `scriptFromReply`'s `replyCalls` count (`reply-workflow-script.ts:29-30` @v0.75.0), which counts
/// `arguments.workflow === true || arguments.workflow === "true"` so the stringified form cannot
/// smuggle a second script past the one-per-reply rule.
///
/// Pure over the already-extracted `(tool name, workflow argument)` pairs so the grammar half and
/// the session-walking half stay separable — the same split upstream keeps and the same one
/// [`crate::extension::reply_workflow_script`] already observes.
#[must_use]
pub(crate) fn count_reply_workflow_calls<'a>(
    calls: impl IntoIterator<Item = (&'a str, Option<&'a Value>)>,
) -> usize {
    calls
        .into_iter()
        .filter(|(name, workflow)| {
            *name == crate::extension::TOOL_NAME
                && workflow.is_some_and(|value| {
                    value == &Value::Bool(true) || value.as_str() == Some("true")
                })
        })
        .count()
}

/// What the boundary made of the request's `workflow` field.
///
/// A one-field struct rather than a bare `Option<WorkflowSource>` so the classification the
/// boundary performed is NAMED at the two places that consult it — the `args requires workflow.`
/// refusal and the resolution match — and so the advertise-vs-dispatch guard
/// (`schema::tests::every_advertised_schema_property_is_read_outside_provided_keys`) sees a real
/// read of the advertised `workflow` property rather than only a string key lookup.
pub(crate) struct ClassifiedWorkflowRequest {
    /// The classified `workflow` value; `None` when the request carried no `workflow` at all.
    pub(crate) workflow: Option<WorkflowSource>,
}

/// pi `readWorkflowScriptFile` (`runs/foreground/subagent-executor.ts:559-571` @v0.75.0) — a
/// `workflow` value containing a separator is a script FILE, read from the already-resolved
/// request cwd before any sandbox.
///
/// Both error sentences are upstream's, and both name the RESOLVED path rather than the value the
/// caller typed, because a relative path that resolved somewhere unexpected is the usual cause.
///
/// # Errors
///
/// The read failure, or the empty-file refusal.
pub(crate) fn read_workflow_script_file(
    requested: &str,
    cwd: &std::path::Path,
) -> Result<String, String> {
    let script_path = cwd.join(requested);
    let display = script_path.display();
    let script = std::fs::read_to_string(&script_path)
        .map_err(|error| format!("Failed to read workflow script '{display}': {error}"))?;
    if script.trim().is_empty() {
        return Err(format!("Workflow script file '{display}' is empty."));
    }
    Ok(script)
}

/// pi `readReplyWorkflowScript`'s branch walk (`extension/reply-workflow-script.ts:14-26`
/// @v0.75.0), as a pure function of the branch.
///
/// Walks BACKWARDS for the assistant message carrying `call_id` — upstream's own direction, and
/// load-bearing: a resumed or re-forked session can hold an older assistant message whose reply
/// also contained a workflow block, and the newest match is the reply that issued THIS call.
///
/// `cyrup_session`'s own entry types are the data: an [`cyrup_session::Entry::Unknown`] and every
/// non-assistant message are skipped exactly as upstream's `entry.type !== "message"` /
/// `message?.role !== "assistant"` guards skip them.
///
/// # Errors
///
/// [`crate::extension::reply_workflow_script::NOT_A_MODEL_TOOL_CALL_REFUSAL`] when no assistant
/// message on the branch carries this tool call id; otherwise whichever of
/// [`crate::extension::reply_workflow_script::script_from_reply`]'s refusals applies.
pub(crate) fn script_from_branch(
    branch: &[&cyrup_session::Entry],
    call_id: &cyrup_core::ToolCallId,
) -> Result<String, String> {
    let Some(content) = branch
        .iter()
        .rev()
        .filter_map(|entry| assistant_content_of(entry))
        .find(|content| {
            content.iter().any(
                |block| matches!(block, cyrup_core::Content::ToolCall(call) if &call.id == call_id),
            )
        })
    else {
        return Err(
            crate::extension::reply_workflow_script::NOT_A_MODEL_TOOL_CALL_REFUSAL.to_string(),
        );
    };
    // pi `scriptFromReply` (`:28-39`): the one-per-reply count over this reply's own `subagent`
    // calls, then the fence grammar over its joined text blocks.
    let calls = count_reply_workflow_calls(content.iter().filter_map(|block| match block {
        cyrup_core::Content::ToolCall(call) => {
            Some((call.name.as_str(), call.arguments.get("workflow")))
        }
        _ => None,
    }));
    let text = content
        .iter()
        .filter_map(|block| match block {
            cyrup_core::Content::Text { text, .. } => Some(text.as_ref()),
            _ => None,
        })
        .collect::<Vec<&str>>()
        .join("\n");
    crate::extension::reply_workflow_script::script_from_reply(&text, calls)
}

/// The content blocks of an entry that is an assistant message, or `None` for anything else — pi's
/// two `continue` guards (`reply-workflow-script.ts:19-21`).
fn assistant_content_of(entry: &cyrup_session::Entry) -> Option<&[cyrup_core::Content]> {
    match entry {
        cyrup_session::Entry::Known(cyrup_session::KnownEntry::Message {
            message: cyrup_session::AgentMessage::Core(cyrup_core::Message::Assistant(assistant)),
            ..
        }) => Some(&assistant.content),
        _ => None,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use serde_json::json;

    /// SUBA-150 — the three kinds, classified once. THE USER ACTION: a model calls
    /// `subagent({ workflow: … })` with each of upstream's three documented forms.
    #[test]
    fn the_three_workflow_forms_classify_as_upstream_classifies_them() {
        assert_eq!(
            WorkflowSource::parse(&json!(true)).unwrap(),
            WorkflowSource::ReplyBlock
        );
        assert_eq!(
            WorkflowSource::parse(&json!("./scripts/plan.js")).unwrap(),
            WorkflowSource::ScriptFile("./scripts/plan.js".to_string())
        );
        // A Windows separator is a path too (`public-execution.ts:63`).
        assert_eq!(
            WorkflowSource::parse(&json!("scripts\\plan.js")).unwrap(),
            WorkflowSource::ScriptFile("scripts\\plan.js".to_string())
        );
        assert_eq!(
            WorkflowSource::parse(&json!("review")).unwrap(),
            WorkflowSource::Resource("review".to_string())
        );
    }

    /// SUBA-150 fold-in (a) / pi #2610 (`df3b6df1`) — the string `"true"` IS the boolean branch.
    ///
    /// THE USER ACTION: an MCP client (upstream names `pi-claude-bridge`) stringifies the boolean
    /// arm of the `workflow` union. Before the fix the tool looked up a workflow resource named
    /// `true` and answered with an error advising `workflow: true` — which the caller had already
    /// sent, so no retry could ever succeed. Mutation killed: dropping the `"true"` arm, which
    /// makes this a `Resource("true")`.
    #[test]
    fn the_string_true_is_the_boolean_branch_and_never_a_resource_or_a_file() {
        assert_eq!(
            WorkflowSource::parse(&json!("true")).unwrap(),
            WorkflowSource::ReplyBlock,
            "a stringified boolean must not become a resource named 'true'"
        );
        assert_eq!(
            WorkflowSource::parse(&json!(true)).unwrap(),
            WorkflowSource::parse(&json!("true")).unwrap(),
            "the two spellings are one source"
        );
    }

    /// `false` is invalid, not "no workflow" — upstream's schema says *"false invalid"* and the
    /// boundary answers with one sentence for the whole invalid set.
    #[test]
    fn false_a_blank_string_and_a_non_string_are_refused_with_upstreams_sentence() {
        for value in [json!(false), json!(""), json!("   "), json!(7), json!([])] {
            assert_eq!(
                WorkflowSource::parse(&value).unwrap_err(),
                INVALID_WORKFLOW_VALUE,
                "{value} must be refused"
            );
        }
    }

    /// The two removed parameters are refused BY NAME, whatever their value.
    #[test]
    fn the_removed_parameters_are_refused_by_name_on_presence_alone() {
        let script = json!({ "workflowScript": "return 1;" });
        assert_eq!(
            removed_model_workflow_field_error(script.as_object().unwrap()),
            Some(REMOVED_WORKFLOW_SCRIPT)
        );
        // Presence, not truthiness: `null` is still a caller reaching for the removed parameter.
        let null_path = json!({ "workflowScriptPath": serde_json::Value::Null });
        assert_eq!(
            removed_model_workflow_field_error(null_path.as_object().unwrap()),
            Some(REMOVED_WORKFLOW_SCRIPT_PATH)
        );
        let clean = json!({ "workflow": true });
        assert_eq!(
            removed_model_workflow_field_error(clean.as_object().unwrap()),
            None
        );
    }

    /// The one-per-reply count includes the stringified form, and ignores other tools' calls.
    #[test]
    fn the_reply_call_count_counts_both_spellings_and_only_subagent_calls() {
        let yes = json!(true);
        let stringified = json!("true");
        let path = json!("./a.js");
        assert_eq!(
            count_reply_workflow_calls([
                ("subagent", Some(&yes)),
                ("subagent", Some(&stringified)),
                ("subagent", Some(&path)),
                ("bash", Some(&yes)),
                ("subagent", None),
            ]),
            2,
            "both spellings count; a path, another tool and a non-workflow call do not"
        );
    }
}
