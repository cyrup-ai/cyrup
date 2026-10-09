//! The demo extension's EVENT PROBE: what this guest observed of the pi v1.1.0 event batch, kept in
//! guest memory and reported on demand, so a host-side test can assert on what actually crossed the
//! component boundary rather than on what the host meant to send.
//!
//! [`REPORT_COMMAND`] answers with one JSON object (also sent through `ui.notify`, prefixed
//! [`REPORT_PREFIX`]):
//!
//! - `toolEnds` — every `tool_execution_end` seen, as `{callId, toolName, isError, durationMs}`
//!   (`durationMs` is `null` for a call that did not run; pi `ToolExecutionEndEvent.durationMs?`,
//!   `core/extensions/types.ts` @v1.1.0).
//! - `compactFailed` — every `session_compact_failed` seen, as pi's own payload keys (SESS-050;
//!   `core/extensions/types.ts:795-807` @v1.1.0).
//! - `settled` — the `aborted` flag of every `agent_settled` seen (pi `AgentSettledEvent.aborted`,
//!   `core/extensions/types.ts:1005-1009` @v1.1.0).
//! - `promptOptions` — for every prompt starting with [`OPTIONS_PROMPT`], what this guest's second
//!   `before_agent_start` handler read after its first one added the [`GUEST_SECTION`] section
//!   through `systemPromptOptions` (EXT-084, pi `emitBeforeAgentStart`,
//!   `core/extensions/runner.ts:1420-1476` @v1.1.0): `{sections, eventHas, ctxHas}`, the option
//!   section names it was handed and which [`SECTION_MARKERS`] its `systemPrompt` and
//!   `ctx.system_prompt()` render. A prompt equal to [`FORCE_PROMPT`] also makes a third handler
//!   return [`FORCED_PROMPT`] as the prompt.
//! - `boundaries` — every `turn_end` / `agent_before_settle` boundary seen (EXT-078, pi
//!   `emitBoundary`, `core/extensions/runner.ts:1029-1080` @v1.1.0), as `{event, outcome,
//!   messageEntryId, toolResultEntryIds, canContinue}`. On a turn whose assistant text contains
//!   [`BOUNDARY_MARKER`], the first `turn_end` handler appends a [`BOUNDARY_NOTE`] custom message;
//!   the second reads it back through the preview the `preview-boundary` import rebuilt (recorded
//!   as `previewedNote`) and asks for one more provider request.

use std::cell::RefCell;

use serde_json::{Value, json};

use crate::{CommandDescriptor, ExtensionApi};

/// `/eventprobe` — report what this guest has observed, as JSON.
pub const REPORT_COMMAND: &str = "eventprobe";
/// The `ui.notify` prefix of the report.
pub const REPORT_PREFIX: &str = "eventprobe: ";

/// A prompt starting with this drives the `systemPromptOptions` probe.
pub const OPTIONS_PROMPT: &str = "options-probe";
/// The probe prompt that also forces the whole system prompt.
pub const FORCE_PROMPT: &str = "options-probe force";
/// What the forcing handler returns as the prompt.
pub const FORCED_PROMPT: &str = "FORCED BY THE GUEST";
/// The section the guest's first handler adds, and its content.
pub const GUEST_SECTION: (&str, &str) = ("guest_rule", "GUEST_RULE");
/// The rendered sections the reading handler looks for: the guest's own, and `team`, which a
/// host-side extension running before the guest may add with content `TEAM_RULE`.
pub const SECTION_MARKERS: [(&str, &str); 2] = [
    ("guest_rule", "<guest_rule>\nGUEST_RULE\n</guest_rule>"),
    ("team", "<team>\nTEAM_RULE\n</team>"),
];

/// An assistant text containing this makes the guest append [`BOUNDARY_NOTE`] and continue.
pub const BOUNDARY_MARKER: &str = "BOUNDARY-CONTINUE";
/// The custom message the boundary probe appends.
pub const BOUNDARY_NOTE: &str = "a note from the boundary";

thread_local! {
    static BOUNDARIES: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    static PROMPT_OPTIONS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    /// The guest is single-threaded; one instance's memory is its own.
    static TOOL_ENDS: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    static SETTLED: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    static COMPACT_FAILED: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

/// Install the probe's subscriptions and its report command.
pub fn install(api: &mut ExtensionApi) {
    api.on_tool_exec_end(|ev, _ctx| {
        TOOL_ENDS.with(|ends| {
            ends.borrow_mut().push(json!({
                "callId": ev.call_id,
                "toolName": ev.name,
                "isError": ev.is_error,
                "durationMs": ev.duration_ms,
            }));
        });
    });

    api.on_agent_settled(|ev, _ctx| {
        SETTLED.with(|settled| settled.borrow_mut().push(ev.aborted));
    });

    api.on_session_compact_failed(|ev, _ctx| {
        COMPACT_FAILED.with(|seen| {
            seen.borrow_mut().push(json!({
                "reason": ev.reason,
                "errorMessage": ev.error_message,
                "aborted": ev.aborted,
                "willRetry": ev.will_retry,
                "fromExtension": ev.from_extension,
            }));
        });
    });

    // EXT-084: three `before_agent_start` handlers over ONE `systemPromptOptions` object. The
    // first adds a section; the second reads what the first left — in `event.systemPrompt`,
    // re-rendered by the host's `render-system-prompt` import, and in `ctx.system_prompt()`; the
    // third forces the prompt.
    api.on_before_agent_start(|ev, _ctx| {
        if !ev.prompt.starts_with(OPTIONS_PROMPT) {
            return crate::Outcome::noop();
        }
        let mut options = ev.options.clone();
        if let Some(fields) = options.as_object_mut()
            && let Some(sections) = fields
                .entry("sections")
                .or_insert_with(|| json!({}))
                .as_object_mut()
        {
            sections.insert(GUEST_SECTION.0.to_string(), json!(GUEST_SECTION.1));
        }
        crate::Outcome::before_agent_start(crate::BeforeAgentStartResult {
            system_prompt_options: Some(options),
            ..Default::default()
        })
    });
    api.on_before_agent_start(|ev, ctx| {
        if ev.prompt.starts_with(OPTIONS_PROMPT) {
            let ctx_prompt = ctx.system_prompt();
            let has = |text: &str| -> Vec<&str> {
                SECTION_MARKERS
                    .iter()
                    .filter(|(_, marker)| text.contains(marker))
                    .map(|(name, _)| *name)
                    .collect()
            };
            let sections: Vec<String> = ev
                .options
                .get("sections")
                .and_then(Value::as_object)
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            PROMPT_OPTIONS.with(|seen| {
                seen.borrow_mut().push(json!({
                    "sections": sections,
                    "eventHas": has(&ev.system_prompt),
                    "ctxHas": has(&ctx_prompt),
                }));
            });
        }
        crate::Outcome::noop()
    });
    api.on_before_agent_start(|ev, _ctx| {
        if ev.prompt != FORCE_PROMPT {
            return crate::Outcome::noop();
        }
        crate::Outcome::before_agent_start(crate::BeforeAgentStartResult {
            system_prompt: Some(FORCED_PROMPT.to_string()),
            ..Default::default()
        })
    });

    // EXT-078: two `turn_end` handlers over one boundary, and an `agent_before_settle` reader.
    api.on_turn_end(|ev, _ctx| {
        BOUNDARIES.with(|seen| {
            seen.borrow_mut().push(json!({
                "event": "turn_end",
                "outcome": ev.boundary.outcome,
                "messageEntryId": ev.message_entry_id,
                "toolResultEntryIds": ev.tool_result_entry_ids,
                "canContinue": ev.boundary.context.get("canContinue"),
            }));
        });
        if !ev.message.to_string().contains(BOUNDARY_MARKER) {
            return crate::Outcome::noop();
        }
        crate::Outcome::boundary(crate::BoundaryResult {
            entries: Some(json!([{
                "type": "custom_message",
                "customType": "boundary-probe",
                "content": BOUNDARY_NOTE,
                "display": true,
            }])),
            continue_: None,
        })
    });
    api.on_turn_end(|ev, _ctx| {
        if !ev.message.to_string().contains(BOUNDARY_MARKER) {
            return crate::Outcome::noop();
        }
        let previewed = ev
            .boundary
            .context
            .get("contextMessages")
            .map(Value::to_string)
            .unwrap_or_default()
            .contains(BOUNDARY_NOTE);
        BOUNDARIES.with(|seen| {
            seen.borrow_mut().push(json!({
                "event": "turn_end",
                "previewedNote": previewed,
                "canContinue": ev.boundary.context.get("canContinue"),
            }));
        });
        crate::Outcome::boundary(crate::BoundaryResult {
            entries: None,
            continue_: Some(true),
        })
    });
    api.on_agent_before_settle(|ev, _ctx| {
        BOUNDARIES.with(|seen| {
            seen.borrow_mut().push(json!({
                "event": "agent_before_settle",
                "outcome": ev.boundary.outcome,
                "canContinue": ev.boundary.context.get("canContinue"),
            }));
        });
        crate::Outcome::noop()
    });

    api.register_command(
        REPORT_COMMAND,
        CommandDescriptor::new("Report the events this extension observed (demo)."),
        |_args: &str, ctx: &crate::CommandCtx| {
            let report = json!({
                "toolEnds": TOOL_ENDS.with(|ends| ends.borrow().clone()),
                "settled": SETTLED.with(|settled| settled.borrow().clone()),
                "compactFailed": COMPACT_FAILED.with(|seen| seen.borrow().clone()),
                "promptOptions": PROMPT_OPTIONS.with(|seen| seen.borrow().clone()),
                "boundaries": BOUNDARIES.with(|seen| seen.borrow().clone()),
            })
            .to_string();
            ctx.ui().notify(&format!("{REPORT_PREFIX}{report}"));
            Ok(Some(report))
        },
    );
}
