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
//! orchestrator surface. This module is that extension.
//!
//! # The rest of the file (the child-side PROMPT runtime)
//!
//! `subagent-prompt-runtime.ts` is not only the structured-output tool. Its two other exports are
//! what make a child behave like a child at all, and both were unported until now:
//!
//! * **`before_agent_start` → [`rewrite_subagent_prompt`]** (`:97-113,323-341`). The parent writes
//!   the persona's `inheritProjectContext` / `inheritSkills` decision and the fanout grant into the
//!   child's env (`pi-args.ts:215-216,181`; cyrup `exec/mod.rs`'s
//!   [`INHERIT_PROJECT_CONTEXT_ENV`]/[`INHERIT_SKILLS_ENV`] + `child_role_env`). NOTHING read them
//!   child-side, so `inheritProjectContext: false` was a pure no-op: the child re-assembled its own
//!   system prompt from its own cwd and happily inherited every `AGENTS.md`/`CLAUDE.md` the persona
//!   had asked to be spared. And no child was ever TOLD it was a child, so a delegated worker that
//!   inherited orchestration history would cheerfully keep orchestrating — launching its own
//!   subagents, re-running the parent's fanout — because nothing in its prompt said not to.
//! * **`context` → [`strip_parent_only_subagent_messages`]** (`:141-159,317-321`). A forked child
//!   starts from the PARENT's conversation, which is full of parent-only orchestration bookkeeping:
//!   `subagent-notify` completions, slash-command results, control notices, and the parent's own
//!   `subagent` tool calls/results. Left in place, the child reads its own history as evidence that
//!   it is the orchestrator.
//!
//! [CYRUP-DELTA] Two section-boundary adaptations, both forced by cyrup's own system-prompt shape:
//!
//! 1. pi's `stripProjectContext`/`stripInheritedSkills` scan for markdown headers
//!    (`"\n\n# Project Context\n\n…"`, `"\n\nThe following skills provide…"`) and cut to whichever
//!    NEXT header appears first — a heuristic forced by pi's header-only sectioning. cyrup's
//!    assembler (`cyrup-session/src/prompt/{builder,skills_inject}.rs`) emits both sections with
//!    explicit CLOSING tags (`</project_context>`, `</available_skills>`), so the port cuts on the
//!    real delimiters instead of guessing where a section ends. Same intent, exact boundaries.
//! 2. pi's `stripSubagentOrchestrationSkill` (`:83-87`, called UNCONDITIONALLY at `:108`) deletes
//!    the `pi-subagents` skill entry from an inherited prompt. It matches two shapes: pi's
//!    attribute form `<skill name="pi-subagents" …>…</skill>`, and the nested form whose body
//!    contains `<name>pi-subagents</name>`. cyrup's assembler emits ONLY the nested form
//!    (`cyrup-session/src/prompt/skills_inject.rs:34-46`), so [`strip_subagent_orchestration_skill`]
//!    ports the second replace and omits the first — there is no attribute form to match.
//!
//!    An earlier revision of this comment claimed the port was dead code because "this crate has no
//!    `skills/` directory and registers no skill". That was FALSE on both counts even when it was
//!    written: `crates/cyrup-ext-subagents/resources/skills/pi-subagents/SKILL.md` is a 58 KB file
//!    that has always shipped here, and it is now registered through the extension's
//!    `resources_discover` contribution (`extension.rs`), so a parent session's
//!    `<available_skills>` block genuinely carries a `pi-subagents` entry and a forked child
//!    genuinely inherits it. Stripping it is exactly what stops a delegated worker from reading its
//!    own prompt as a licence to orchestrate.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cyrup_agent::AgentMessage;
use cyrup_core::tool::ExecMode;
use cyrup_core::{
    CancelToken, Content, ExtensionId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{EventKind, EventPatch, ExtError, HookOutcome, HostEvent};

use crate::exec::structured::{
    STRUCTURED_OUTPUT_CAPTURE_ENV, STRUCTURED_OUTPUT_INSTRUCTION, STRUCTURED_OUTPUT_SCHEMA_ENV,
    validate_structured_output,
};
use crate::spawn::nested_events::FANOUT_CHILD_ENV;

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

/// Child env flag: whether this subagent inherits the parent's project-context files
/// (`AGENTS.md`/`CLAUDE.md`) — pi `SUBAGENT_INHERIT_PROJECT_CONTEXT_ENV`
/// (`subagent-prompt-runtime.ts:11`), written parent-side by `exec/mod.rs`.
///
/// Declared HERE, next to the only code that reads it, and re-exported by the writer — a
/// write-only constant with no reader was exactly this item's defect.
pub const INHERIT_PROJECT_CONTEXT_ENV: &str = "CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT";

/// Child env flag: whether this subagent inherits the parent's skills — pi
/// `SUBAGENT_INHERIT_SKILLS_ENV` (`subagent-prompt-runtime.ts:12`).
///
/// The parent ALSO passes `--no-skills` when this is `0` (pi `pi-args.ts:155-157`, cyrup
/// `exec/mod.rs`), which stops the child DISCOVERING skills. This flag is the second half: a forked
/// child whose prompt already carries an inherited skills section still has to have it removed.
pub const INHERIT_SKILLS_ENV: &str = "CYRUP_SUBAGENT_INHERIT_SKILLS";

/// pi `CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS` (`subagent-prompt-runtime.ts:21-27`), verbatim.
pub const CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS: &str = concat!(
    "You are a child subagent, not the parent orchestrator.\n",
    "The parent session owns delegation, orchestration, review fanout, and follow-up worker launches.\n",
    "Ignore prior parent-only orchestration instructions in inherited conversation history.\n",
    "Do not propose or run subagents. Complete only your assigned role-specific task with the tools available to you.\n",
    "If you need to edit files, use the available editing tools. Do not print tool-call syntax, patches, or pseudo-tool calls as text.",
);

/// pi `CHILD_FANOUT_BOUNDARY_INSTRUCTIONS` (`subagent-prompt-runtime.ts:29-36`), verbatim. Used
/// instead of [`CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS`] for a child the parent DID authorize to fan
/// out ([`FANOUT_CHILD_ENV`] = `1`), so the grant is not contradicted by its own system prompt.
pub const CHILD_FANOUT_BOUNDARY_INSTRUCTIONS: &str = concat!(
    "You are a child subagent with explicit fanout responsibility for this assigned task.\n",
    "The parent session owns final orchestration, acceptance, and follow-up implementation launches.\n",
    "You may use the `subagent` tool only for the fanout work explicitly requested in this task.\n",
    "Do not broaden yourself into general parent orchestration. Do not launch follow-up workers unless the task explicitly asks for that.\n",
    "The maxSubagentDepth cap still applies and may block further fanout.\n",
    "If you need to edit files, use the available editing tools. Do not print tool-call syntax, patches, or pseudo-tool calls as text.",
);

/// pi `PARENT_ONLY_CUSTOM_MESSAGE_TYPES` (`subagent-prompt-runtime.ts:38-46`), verbatim. Every one
/// is orchestration bookkeeping the PARENT session produced about its children; a child that reads
/// them in its own history reads itself as the orchestrator.
///
/// Three of the seven are live cyrup producers today — `"subagent-notify"`
/// (`background/watch.rs`), `"subagent-slash-result"` (`registration/cost.rs`) and
/// `"subagent_control_notice"` (`tui/notices.rs`) — and the rest are kept because this is a POLICY
/// list, not an inventory: a forked child's inherited history can carry any customType a past
/// (or upstream-compatible) session wrote, and each of these is parent-only wherever it came from.
const PARENT_ONLY_CUSTOM_MESSAGE_TYPES: &[&str] = &[
    "subagent-orchestration-instructions",
    "subagent-slash-result",
    "subagent-slash-text-result",
    "subagent-notify",
    "subagent_control_notice",
    "subagent-control",
    "subagent-control-notice",
];

/// The orchestration tool a child must not read itself as having called
/// (`crate::extension::TOOL_NAME`; pi keys the same filters on the literal `"subagent"`,
/// `subagent-prompt-runtime.ts:124,129`).
const SUBAGENT_TOOL_NAME: &str = "subagent";

/// Opening delimiter of cyrup's project-context section (`cyrup-session/src/prompt/builder.rs`'s
/// `project_context_open`). See the module doc's [CYRUP-DELTA] 1.
const PROJECT_CONTEXT_OPEN: &str = "<project_context>";
/// Closing delimiter of cyrup's project-context section (`builder.rs`'s `project_context_close`).
const PROJECT_CONTEXT_CLOSE: &str = "</project_context>";
/// First line of cyrup's skills section (`cyrup-session/src/prompt/skills_inject.rs`'s
/// `SKILLS_PREAMBLE`) — the section starts at the preamble, NOT at the `<available_skills>` tag.
const SKILLS_OPEN: &str = "Available skills (open the SKILL.md with the read tool to use one):";
/// Closing delimiter of cyrup's skills section (`skills_inject.rs`).
const SKILLS_CLOSE: &str = "</available_skills>";

/// What [`rewrite_subagent_prompt`] was told about this child (pi's three `readBooleanEnv` results
/// plus the structured-output presence check, `subagent-prompt-runtime.ts:111,330-338`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptRewriteOptions {
    /// `false` strips the inherited project-context section (pi `inheritProjectContext ?? true`).
    pub inherit_project_context: bool,
    /// `false` strips the inherited skills section (pi `inheritSkills ?? true`).
    pub inherit_skills: bool,
    /// `true` selects [`CHILD_FANOUT_BOUNDARY_INSTRUCTIONS`] over
    /// [`CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS`] (pi `fanoutChild === true`).
    pub fanout_child: bool,
    /// Whether the structured-output capture var is set, which appends
    /// [`STRUCTURED_OUTPUT_INSTRUCTION`] under the boundary block (pi `:111`).
    pub structured_output: bool,
}

impl Default for PromptRewriteOptions {
    /// pi's own defaults for a var that is present-but-unreadable: inherit everything, plain child.
    fn default() -> Self {
        Self {
            inherit_project_context: true,
            inherit_skills: true,
            fanout_child: false,
            structured_output: false,
        }
    }
}

/// pi `readBooleanEnv` (`subagent-prompt-runtime.ts:52-56`): an ABSENT var is `None` (the caller's
/// default applies); a present var is `false` only for the exact string `"0"`, true otherwise.
fn read_boolean_env(get: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<bool> {
    get(name).map(|value| value != "0")
}

/// Excise `open …  close` (inclusive of both delimiters, and of the blank line the assembler emits
/// before the section) from `prompt`. Returns the input unchanged when either delimiter is absent
/// or they appear out of order — a prompt that does not contain the section is already correct.
fn strip_delimited_section(prompt: &str, open: &str, close: &str) -> String {
    let Some(start) = prompt.find(open) else {
        return prompt.to_string();
    };
    let Some(head) = prompt.get(..start) else {
        return prompt.to_string();
    };
    let Some(rest) = prompt.get(start..) else {
        return prompt.to_string();
    };
    let Some(close_at) = rest.find(close) else {
        return prompt.to_string();
    };
    let end = start.saturating_add(close_at).saturating_add(close.len());
    let Some(tail) = prompt.get(end..) else {
        return prompt.to_string();
    };
    // The assembler separates sections with a blank line; leaving it behind would accumulate
    // stray whitespace exactly where pi's header-anchored slice leaves none.
    let head = head.trim_end_matches(['\n', '\r', ' ', '\t']);
    let mut out = String::with_capacity(head.len().saturating_add(tail.len()));
    out.push_str(head);
    out.push_str(tail);
    out
}

/// pi `stripProjectContext` (`subagent-prompt-runtime.ts:69-74`), on cyrup's delimiters.
#[must_use]
pub fn strip_project_context(prompt: &str) -> String {
    strip_delimited_section(prompt, PROJECT_CONTEXT_OPEN, PROJECT_CONTEXT_CLOSE)
}

/// pi `stripInheritedSkills` (`subagent-prompt-runtime.ts:76-81`), on cyrup's delimiters.
#[must_use]
pub fn strip_inherited_skills(prompt: &str) -> String {
    strip_delimited_section(prompt, SKILLS_OPEN, SKILLS_CLOSE)
}

/// The orchestration skill's name, as it appears inside a `<name>` element of cyrup's
/// `<available_skills>` block. Deliberately the SAME constant
/// [`crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL`] carries — one name, two enforcement
/// points (that module refuses to RESOLVE it for a child; this one removes it from a prompt the
/// child INHERITED), and they must never drift.
const SUBAGENT_ORCHESTRATION_SKILL: &str = crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL;

/// pi `stripSubagentOrchestrationSkill` (`subagent-prompt-runtime.ts:83-87`): remove the
/// `pi-subagents` entry from an inherited `<available_skills>` block, leaving every other skill in
/// place.
///
/// Ports upstream's SECOND replace — the nested `<skill>…<name>pi-subagents</name>…</skill>` form,
/// which is the only shape cyrup's assembler emits (`skills_inject.rs:34-46`). Upstream's first
/// replace targets an attribute form (`<skill name="pi-subagents">`) cyrup never produces.
///
/// Unlike [`strip_inherited_skills`], this runs for EVERY child — including one that inherits
/// skills — because the orchestration skill is parent-only regardless of the inherit flag (pi calls
/// it unconditionally at `:108`, outside both `if` guards).
#[must_use]
pub fn strip_subagent_orchestration_skill(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(open_at) = rest.find(SKILL_OPEN) {
        let Some(head) = rest.get(..open_at) else { break };
        let Some(from_open) = rest.get(open_at..) else { break };
        let Some(close_rel) = from_open.find(SKILL_CLOSE) else {
            // An unterminated `<skill>` is not a block; emit the remainder verbatim.
            break;
        };
        let end = close_rel.saturating_add(SKILL_CLOSE.len());
        let Some(block) = from_open.get(..end) else { break };
        out.push_str(head);
        if !block_names_orchestration_skill(block) {
            out.push_str(block);
        } else {
            // Upstream's replacement is the empty string AND its pattern consumes the block's
            // trailing whitespace (`<\/skill>\s*`), so removing the entry leaves no blank line
            // where it used to be.
            rest = from_open.get(end..).unwrap_or("");
            let trimmed = rest.trim_start_matches([' ', '\t', '\r', '\n']);
            // Keep the indentation-free remainder, but never swallow the block's own closing
            // `</available_skills>` line separator entirely: re-emit a single newline so the
            // following element still starts on its own line.
            if !trimmed.is_empty() && rest.len() != trimmed.len() {
                out.push('\n');
            }
            rest = trimmed;
            continue;
        }
        rest = from_open.get(end..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Opening tag of one entry in cyrup's `<available_skills>` block (`skills_inject.rs:34`).
const SKILL_OPEN: &str = "<skill>";
/// Closing tag of one entry (`skills_inject.rs:46`).
const SKILL_CLOSE: &str = "</skill>";

/// pi `SUBAGENT_ORCHESTRATION_SKILL_NAME_PATTERN` (`:47`, `/<name>\s*pi-subagents\s*<\/name>/`):
/// does this `<skill>` block name the orchestration skill?
fn block_names_orchestration_skill(block: &str) -> bool {
    let mut rest = block;
    while let Some(at) = rest.find("<name>") {
        let Some(after) = rest.get(at.saturating_add("<name>".len())..) else {
            return false;
        };
        let Some(close) = after.find("</name>") else {
            return false;
        };
        if after.get(..close).unwrap_or("").trim() == SUBAGENT_ORCHESTRATION_SKILL {
            return true;
        }
        rest = after.get(close..).unwrap_or("");
    }
    false
}

/// pi `stripChildBoundaryInstructions` (`subagent-prompt-runtime.ts:89-95`): remove any boundary
/// block already present, then drop the leading blank lines that removal leaves.
///
/// Load-bearing for IDEMPOTENCE, not cosmetics: a child whose persona body was appended to a prompt
/// that already carried a boundary block (a fork, a resumed session, a re-entrant
/// `before_agent_start`) must end up with exactly ONE boundary block, and it must be the one this
/// run's flags select — not a stale `fanout` block from a run that was granted fanout.
fn strip_child_boundary_instructions(prompt: &str) -> String {
    let mut rewritten = prompt.to_string();
    for boundary in [CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS, CHILD_FANOUT_BOUNDARY_INSTRUCTIONS] {
        rewritten = rewritten.replace(boundary, "");
    }
    trim_leading_blank_lines(&rewritten).to_string()
}

/// pi's `.replace(/^(?:[ \t]*\r?\n)+/, "")` (`subagent-prompt-runtime.ts:94`): drop whole leading
/// BLANK lines only. A leading space on a non-blank first line is preserved, exactly as the regex
/// requires — the alternative (`trim_start`) would silently reflow an indented prompt body.
fn trim_leading_blank_lines(text: &str) -> &str {
    let mut rest = text;
    while let Some(line_end) = rest.find('\n') {
        let Some(first_line) = rest.get(..line_end) else { break };
        if !first_line.chars().all(|c| c == ' ' || c == '\t' || c == '\r') {
            break;
        }
        match rest.get(line_end.saturating_add(1)..) {
            Some(next) => rest = next,
            None => break,
        }
    }
    rest
}

/// pi `rewriteSubagentPrompt` (`subagent-prompt-runtime.ts:97-113`): strip what this child was told
/// not to inherit, remove any pre-existing boundary block, then PREFIX the boundary block this run
/// selects (plus the structured-output instruction when a schema was declared).
#[must_use]
pub fn rewrite_subagent_prompt(prompt: &str, opts: &PromptRewriteOptions) -> String {
    let mut rewritten = prompt.to_string();
    if !opts.inherit_project_context {
        rewritten = strip_project_context(&rewritten);
    }
    if !opts.inherit_skills {
        rewritten = strip_inherited_skills(&rewritten);
    }
    // pi `:108` — UNCONDITIONAL, outside both `if` guards above: even a child that inherits every
    // other skill must not inherit the parent's orchestration skill.
    rewritten = strip_subagent_orchestration_skill(&rewritten);
    rewritten = strip_child_boundary_instructions(&rewritten);
    let boundary = if opts.fanout_child {
        CHILD_FANOUT_BOUNDARY_INSTRUCTIONS
    } else {
        CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS
    };
    let structured = if opts.structured_output {
        format!("\n\n{STRUCTURED_OUTPUT_INSTRUCTION}")
    } else {
        String::new()
    };
    format!("{boundary}{structured}\n\n{rewritten}")
}

/// pi `isParentOnlySubagentMessage` (`:115-120`): a `custom` message whose type is parent-only.
fn is_parent_only_custom(message: &AgentMessage) -> bool {
    match message {
        AgentMessage::Custom { kind, .. } => PARENT_ONLY_CUSTOM_MESSAGE_TYPES.contains(&kind.as_str()),
        _ => false,
    }
}

/// pi `stripParentOnlySubagentMessages` (`subagent-prompt-runtime.ts:141-159`): drop the parent's
/// orchestration bookkeeping from the context a CHILD sends to the model.
///
/// Returns `None` when nothing changed, so the caller can leave the in-flight list untouched
/// (pi's `if (messages === event.messages) return undefined;`, `:319`) rather than handing back an
/// identical copy the dispatcher would treat as a mutation.
///
/// `preserve_fanout_tool_history` is pi's `SUBAGENT_FANOUT_CHILD_ENV === "1"` check (`:142`): a
/// child that IS authorized to fan out keeps its own `subagent` calls and results, because for it
/// they are its own work rather than the parent's. A plain child keeps neither.
#[must_use]
pub fn strip_parent_only_subagent_messages(
    messages: &[AgentMessage],
    preserve_fanout_tool_history: bool,
) -> Option<Vec<AgentMessage>> {
    let mut changed = false;
    let mut filtered: Vec<AgentMessage> = Vec::with_capacity(messages.len());
    for message in messages {
        let drop_subagent_tool_result = !preserve_fanout_tool_history
            && matches!(message, AgentMessage::ToolResult(tr) if tr.tool_name == SUBAGENT_TOOL_NAME);
        if is_parent_only_custom(message) || drop_subagent_tool_result {
            changed = true;
            continue;
        }
        if preserve_fanout_tool_history {
            filtered.push(message.clone());
            continue;
        }
        match strip_assistant_subagent_tool_calls(message) {
            // pi returns `undefined` for an assistant message left with NO content at all — the
            // message existed only to make the call, so it is dropped rather than sent empty.
            None => changed = true,
            Some(stripped) => {
                if stripped != *message {
                    changed = true;
                }
                filtered.push(stripped);
            }
        }
    }
    changed.then_some(filtered)
}

/// pi `stripAssistantSubagentToolCallBlocks` (`:132-139`): remove `subagent` tool-call blocks from
/// an assistant message; `None` means the message became empty and must be dropped entirely.
/// Any non-assistant message passes through untouched.
fn strip_assistant_subagent_tool_calls(message: &AgentMessage) -> Option<AgentMessage> {
    let AgentMessage::Assistant(assistant) = message else {
        return Some(message.clone());
    };
    let kept: Vec<Content> = assistant
        .content
        .iter()
        .filter(|block| !matches!(block, Content::ToolCall(tc) if tc.name == SUBAGENT_TOOL_NAME))
        .cloned()
        .collect();
    if kept.len() == assistant.content.len() {
        return Some(message.clone());
    }
    if kept.is_empty() {
        return None;
    }
    let mut assistant = assistant.clone();
    assistant.content = kept;
    Some(AgentMessage::Assistant(assistant))
}

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

/// The child-side runtime extension: the optional `structured_output` tool, the
/// `before_agent_start` prompt rewrite, and the `context` history filter.
pub struct SubagentPromptRuntime {
    id: ExtensionId,
    /// `Some` only when this step declared an `outputSchema` (both structured env vars resolved).
    tool: Option<Arc<StructuredOutputTool>>,
    /// `None` reproduces pi's early return at `subagent-prompt-runtime.ts:333` — when NONE of the
    /// three child flags is defined the prompt is left exactly as assembled. In practice a real
    /// spawn always defines all three (`exec/mod.rs` writes both inherit flags and `child_role_env`
    /// writes the fanout flag), so this is the "not actually a subagent child" case.
    rewrite: Option<PromptRewriteOptions>,
    /// pi's `preserveCurrentFanoutToolHistory` (`:142`) — see
    /// [`strip_parent_only_subagent_messages`].
    preserve_fanout_tool_history: bool,
}

impl SubagentPromptRuntime {
    /// The structured-output-only form (no prompt rewrite, no fanout grant).
    #[must_use]
    pub fn new(schema: serde_json::Value, output_path: PathBuf) -> Self {
        Self {
            id: ExtensionId::from(PROMPT_RUNTIME_EXTENSION_ID),
            tool: Some(Arc::new(StructuredOutputTool::new(schema, output_path))),
            rewrite: None,
            preserve_fanout_tool_history: false,
        }
    }

    /// Build from already-resolved parts. Kept env-free so callers (and tests) construct the exact
    /// runtime under test without touching process-global environment state.
    #[must_use]
    pub fn from_parts(
        tool: Option<Arc<StructuredOutputTool>>,
        rewrite: Option<PromptRewriteOptions>,
        preserve_fanout_tool_history: bool,
    ) -> Self {
        Self {
            id: ExtensionId::from(PROMPT_RUNTIME_EXTENSION_ID),
            tool,
            rewrite,
            preserve_fanout_tool_history,
        }
    }
}

#[async_trait]
impl NativeExtension for SubagentPromptRuntime {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Registers the tool when one exists and declares the two mutating seams pi's runtime hooks
    /// (`onRuntimeEvent("context", …)` `:317` and `onRuntimeEvent("before_agent_start", …)` `:323`).
    ///
    /// The subscription is not decoration: `Dispatcher::no_subscribers` short-circuits an event
    /// with no declared listener, so an unsubscribed handler is never called at all.
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        if let Some(tool) = &self.tool {
            api.register_tool(tool.clone());
        }
        // `context` is subscribed unconditionally, exactly as pi registers its handler
        // unconditionally: this extension exists ONLY inside a subagent child, and every subagent
        // child must have the parent's orchestration bookkeeping filtered out of its context.
        let mut kinds = vec![EventKind::Context];
        if self.rewrite.is_some() {
            kinds.push(EventKind::BeforeAgentStart);
        }
        api.subscribe(&kinds);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            // pi `:323-341`.
            HostEvent::BeforeAgentStart { system_prompt, .. } => {
                let Some(opts) = &self.rewrite else {
                    return HookOutcome::Noop;
                };
                let rewritten = rewrite_subagent_prompt(system_prompt, opts);
                // pi `:339`: an unchanged prompt returns nothing rather than a no-op mutation.
                if rewritten == *system_prompt {
                    HookOutcome::Noop
                } else {
                    HookOutcome::Mutate(EventPatch::SystemPromptAndInject {
                        system: Some(rewritten),
                        inject: None,
                    })
                }
            }
            // pi `:317-321`.
            HostEvent::Context { messages } => {
                match strip_parent_only_subagent_messages(messages, self.preserve_fanout_tool_history)
                {
                    Some(messages) => HookOutcome::Mutate(EventPatch::Context { messages }),
                    None => HookOutcome::Noop,
                }
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// Build the child-side runtime from this process's environment, or `None` when this process is not
/// a subagent child at all.
///
/// Two independent halves, matching pi — which loads `subagent-prompt-runtime.ts` into EVERY
/// subagent child (`pi-args.ts:141-143`) and then gates each half on its own vars:
///
/// * the `structured_output` tool needs BOTH structured vars (`:281`), plus a schema file that
///   reads and parses;
/// * the prompt rewrite needs at least ONE of the three child flags to be DEFINED (`:333`).
///
/// A process with neither gets `None` and carries no extra surface whatsoever. A malformed schema
/// is deliberately not a hard failure: the parent already validated it, so an unreadable file
/// child-side means the private temp dir is gone, and failing the child over it would turn a
/// recoverable "structured output missing" into an unexplained startup crash.
#[must_use]
pub fn prompt_runtime_extension_for_env() -> Option<Arc<dyn NativeExtension>> {
    prompt_runtime_extension_from(&|key| std::env::var(key).ok())
}

/// The env-injected form of [`prompt_runtime_extension_for_env`] — the whole decision as a pure
/// function of a lookup, so it is testable without mutating process-global environment state.
#[must_use]
pub fn prompt_runtime_extension_from(
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<Arc<dyn NativeExtension>> {
    let non_empty = |key: &str| get(key).filter(|value| !value.trim().is_empty());

    let capture = non_empty(STRUCTURED_OUTPUT_CAPTURE_ENV);
    let tool = match (&capture, non_empty(STRUCTURED_OUTPUT_SCHEMA_ENV)) {
        (Some(capture), Some(schema_path)) => std::fs::read(&schema_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .map(|schema| Arc::new(StructuredOutputTool::new(schema, PathBuf::from(capture)))),
        _ => None,
    };

    let inherit_project_context = read_boolean_env(get, INHERIT_PROJECT_CONTEXT_ENV);
    let inherit_skills = read_boolean_env(get, INHERIT_SKILLS_ENV);
    let fanout_child = read_boolean_env(get, FANOUT_CHILD_ENV);
    // pi `:333`: all three undefined => no rewrite at all.
    let rewrite = (inherit_project_context.is_some()
        || inherit_skills.is_some()
        || fanout_child.is_some())
    .then(|| PromptRewriteOptions {
        inherit_project_context: inherit_project_context.unwrap_or(true),
        inherit_skills: inherit_skills.unwrap_or(true),
        fanout_child: fanout_child == Some(true),
        // pi `:111` gates the appended instruction on the CAPTURE var alone.
        structured_output: capture.is_some(),
    });

    if tool.is_none() && rewrite.is_none() {
        return None;
    }
    Some(Arc::new(SubagentPromptRuntime::from_parts(
        tool,
        rewrite,
        fanout_child == Some(true),
    )) as Arc<dyn NativeExtension>)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

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

    /// A process that is not a subagent child at all — no structured vars, none of the three child
    /// flags — must build NOTHING. This is every top-level `cyrup` session.
    #[test]
    fn a_non_child_process_builds_no_runtime_at_all() {
        assert!(
            prompt_runtime_extension_from(&|_| None).is_none(),
            "an empty environment must not attach the child runtime"
        );
    }

    /// The rewrite half is independent of the structured-output half: a plain child (no declared
    /// schema) still gets the runtime, because it still needs its prompt and context shaped.
    #[test]
    fn the_inherit_flags_alone_build_the_runtime() {
        let env = |key: &str| match key {
            INHERIT_PROJECT_CONTEXT_ENV => Some("0".to_string()),
            INHERIT_SKILLS_ENV => Some("1".to_string()),
            FANOUT_CHILD_ENV => Some("0".to_string()),
            _ => None,
        };
        assert!(
            prompt_runtime_extension_from(&env).is_some(),
            "a child with inherit flags but no schema still needs the prompt/context runtime"
        );
    }

    /// pi `readBooleanEnv` (`:52-56`): absent => `None`; `"0"` => `false`; anything else => `true`.
    #[test]
    fn boolean_env_reads_match_pi_exactly() {
        let val = |v: Option<&str>| {
            let owned = v.map(str::to_string);
            read_boolean_env(&move |_| owned.clone(), "X")
        };
        assert_eq!(val(None), None);
        assert_eq!(val(Some("0")), Some(false));
        assert_eq!(val(Some("1")), Some(true));
        assert_eq!(val(Some("")), Some(true), "only the exact \"0\" is false");
        assert_eq!(val(Some("false")), Some(true), "pi does not parse words");
    }

    fn opts(inherit_project_context: bool, inherit_skills: bool, fanout_child: bool) -> PromptRewriteOptions {
        PromptRewriteOptions {
            inherit_project_context,
            inherit_skills,
            fanout_child,
            structured_output: false,
        }
    }

    /// A prompt shaped like the real assembler's output (`cyrup-session/src/prompt/builder.rs`
    /// order: body, project context, skills, footer).
    fn assembled_prompt() -> String {
        [
            "You are a coding assistant operating inside cyrup.",
            "",
            "<project_context>",
            "",
            "Project-specific instructions follow.",
            "",
            "<project_instructions path=\"/repo/AGENTS.md\">",
            "NEVER commit to main.",
            "</project_instructions>",
            "",
            "</project_context>",
            "",
            SKILLS_OPEN,
            "<available_skills>",
            "  <skill>",
            "    <name>deploy</name>",
            "  </skill>",
            "</available_skills>",
            "",
            "Current date: 2026-08-07",
        ]
        .join("\n")
    }

    #[test]
    fn inherit_project_context_false_removes_the_project_context_section() {
        let out = rewrite_subagent_prompt(&assembled_prompt(), &opts(false, true, false));
        assert!(!out.contains("NEVER commit to main."), "inherited AGENTS.md content must be gone");
        assert!(!out.contains(PROJECT_CONTEXT_OPEN));
        assert!(!out.contains(PROJECT_CONTEXT_CLOSE));
        // Everything AROUND the section survives — this is a cut, not a truncation.
        assert!(out.contains("You are a coding assistant operating inside cyrup."));
        assert!(out.contains("Current date: 2026-08-07"));
        assert!(out.contains("<name>deploy</name>"), "skills were inherited and must remain");
    }

    #[test]
    fn inherit_skills_false_removes_only_the_skills_section() {
        let out = rewrite_subagent_prompt(&assembled_prompt(), &opts(true, false, false));
        assert!(!out.contains("<name>deploy</name>"));
        assert!(!out.contains(SKILLS_OPEN));
        assert!(!out.contains(SKILLS_CLOSE));
        assert!(out.contains("NEVER commit to main."), "project context was inherited");
        assert!(out.contains("Current date: 2026-08-07"));
    }

    #[test]
    fn inheriting_everything_still_prefixes_the_child_boundary() {
        let out = rewrite_subagent_prompt(&assembled_prompt(), &opts(true, true, false));
        assert!(out.starts_with(CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS));
        assert!(out.contains("NEVER commit to main."));
        assert!(out.contains("<name>deploy</name>"));
    }

    /// A fanout-authorized child gets the fanout boundary, never the "do not run subagents" one —
    /// the grant and the prompt must not contradict each other.
    #[test]
    fn a_fanout_child_gets_the_fanout_boundary_only() {
        let out = rewrite_subagent_prompt("BODY", &opts(true, true, true));
        assert!(out.starts_with(CHILD_FANOUT_BOUNDARY_INSTRUCTIONS));
        assert!(!out.contains("Do not propose or run subagents."));
        assert!(out.ends_with("\n\nBODY"));
    }

    /// Re-running the rewrite must not stack boundary blocks, and the SECOND run's flags win.
    #[test]
    fn the_rewrite_is_idempotent_and_the_latest_flags_win() {
        let once = rewrite_subagent_prompt("BODY", &opts(true, true, true));
        let twice = rewrite_subagent_prompt(&once, &opts(true, true, false));
        assert!(twice.starts_with(CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS));
        assert!(
            !twice.contains(CHILD_FANOUT_BOUNDARY_INSTRUCTIONS),
            "a stale fanout grant must not survive a re-run: {twice}"
        );
        assert_eq!(twice.matches("You are a child subagent, not the parent orchestrator.").count(), 1);
        assert!(twice.ends_with("\n\nBODY"));
    }

    #[test]
    fn a_declared_schema_appends_the_structured_output_instruction_under_the_boundary() {
        let out = rewrite_subagent_prompt(
            "BODY",
            &PromptRewriteOptions { structured_output: true, ..opts(true, true, false) },
        );
        assert!(out.starts_with(CHILD_SUBAGENT_BOUNDARY_INSTRUCTIONS));
        assert!(out.contains(STRUCTURED_OUTPUT_INSTRUCTION));
        let without = rewrite_subagent_prompt("BODY", &opts(true, true, false));
        assert!(!without.contains(STRUCTURED_OUTPUT_INSTRUCTION));
    }

    /// A prompt with neither section is returned as-is by the strips (only the boundary is added).
    /// A prompt shaped like the real assembler's output when the subagents extension's
    /// `resources_discover` contribution HAS registered (`extension.rs`): the `pi-subagents`
    /// operational skill sits alongside a normal project skill.
    fn assembled_prompt_with_orchestration_skill() -> String {
        [
            "You are a coding assistant operating inside cyrup.",
            "",
            SKILLS_OPEN,
            "<available_skills>",
            "  <skill>",
            "    <name>deploy</name>",
            "    <description>Ship a release</description>",
            "    <location>/repo/.cyrup/skills/deploy/SKILL.md</location>",
            "  </skill>",
            "  <skill>",
            "    <name>pi-subagents</name>",
            "    <description>Orchestrate subagents</description>",
            "    <location>/pkg/resources/skills/pi-subagents/SKILL.md</location>",
            "  </skill>",
            "</available_skills>",
            "",
            "Current date: 2026-08-09",
        ]
        .join("\n")
    }

    /// The orchestration skill is removed and EVERY other skill survives, byte for byte.
    #[test]
    fn the_orchestration_skill_entry_is_removed_and_others_survive() {
        let out = strip_subagent_orchestration_skill(&assembled_prompt_with_orchestration_skill());
        assert!(!out.contains("pi-subagents"), "the orchestration entry must be gone: {out}");
        assert!(!out.contains("Orchestrate subagents"));
        assert!(out.contains("<name>deploy</name>"), "the unrelated skill survives: {out}");
        assert!(out.contains("Ship a release"));
        assert!(out.contains("<available_skills>") && out.contains("</available_skills>"));
        assert_eq!(out.matches("<skill>").count(), 1, "exactly one entry left: {out}");
        assert!(out.contains("Current date: 2026-08-09"));
    }

    /// pi calls it UNCONDITIONALLY (`:108`), outside both inherit guards — so a child that inherits
    /// every OTHER skill still loses this one.
    #[test]
    fn a_child_that_inherits_skills_still_loses_the_orchestration_skill() {
        let out = rewrite_subagent_prompt(
            &assembled_prompt_with_orchestration_skill(),
            &opts(true, true, false),
        );
        assert!(out.contains("<name>deploy</name>"), "skills were inherited: {out}");
        assert!(
            !out.contains("pi-subagents"),
            "the parent's orchestration skill must never survive into a child: {out}"
        );
    }

    /// A prompt with no orchestration entry is returned unchanged — no stray whitespace edits.
    #[test]
    fn stripping_the_orchestration_skill_is_a_no_op_when_absent() {
        let input = assembled_prompt();
        assert_eq!(strip_subagent_orchestration_skill(&input), input);
        assert_eq!(strip_subagent_orchestration_skill("no skills here"), "no skills here");
    }

    /// Only an exact `<name>pi-subagents</name>` matches — a skill that merely MENTIONS the name in
    /// its description is left alone (pi's pattern anchors on the `<name>` element, `:47`).
    #[test]
    fn a_skill_that_only_mentions_the_name_is_not_removed() {
        let input = [
            "<available_skills>",
            "  <skill>",
            "    <name>delegation-guide</name>",
            "    <description>How to use pi-subagents effectively</description>",
            "  </skill>",
            "</available_skills>",
        ]
        .join("\n");
        assert_eq!(strip_subagent_orchestration_skill(&input), input);
    }

    #[test]
    fn stripping_a_missing_section_is_a_no_op() {
        assert_eq!(strip_project_context("no sections here"), "no sections here");
        assert_eq!(strip_inherited_skills("no sections here"), "no sections here");
    }

    fn custom(kind: &str) -> AgentMessage {
        AgentMessage::Custom {
            kind: kind.to_string(),
            payload: serde_json::json!({}),
            timestamp: None,
        }
    }

    fn tool_result(tool_name: &str) -> AgentMessage {
        AgentMessage::ToolResult(cyrup_agent::ToolResultMessage {
            tool_call_id: ToolCallId::from("tc-1"),
            tool_name: tool_name.to_string(),
            content: vec![Content::text("done")],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            is_error: false,
            timestamp: 0,
        })
    }

    fn assistant(blocks: Vec<Content>) -> AgentMessage {
        let mut msg = cyrup_core::AssistantMessage::errored(
            "faux".into(),
            "m",
            Some("faux".into()),
            cyrup_core::StopReason::Stop,
            "x",
        );
        msg.content = blocks;
        AgentMessage::Assistant(msg)
    }

    fn tool_call_block(name: &str) -> Content {
        Content::ToolCall(cyrup_core::ToolCall {
            id: ToolCallId::from("tc-1"),
            name: name.to_string(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        })
    }

    #[test]
    fn every_parent_only_custom_type_is_dropped_from_a_childs_context() {
        for kind in PARENT_ONLY_CUSTOM_MESSAGE_TYPES {
            let messages = vec![AgentMessage::user_text("task"), custom(kind)];
            let out = strip_parent_only_subagent_messages(&messages, false)
                .unwrap_or_else(|| panic!("{kind} must be stripped"));
            assert_eq!(out.len(), 1, "{kind} must be dropped");
        }
    }

    #[test]
    fn a_plain_child_loses_the_parents_subagent_calls_and_results() {
        let messages = vec![
            AgentMessage::user_text("task"),
            assistant(vec![Content::text("delegating"), tool_call_block("subagent")]),
            tool_result("subagent"),
            tool_result("bash"),
        ];
        let out = strip_parent_only_subagent_messages(&messages, false).expect("changed");
        assert_eq!(out.len(), 3, "only the subagent toolResult is dropped");
        match &out.get(1) {
            Some(AgentMessage::Assistant(a)) => {
                assert_eq!(a.content.len(), 1, "the subagent toolCall block is gone");
                assert!(matches!(a.content.first(), Some(Content::Text { .. })));
            }
            other => panic!("expected an assistant message, got {other:?}"),
        }
        assert!(
            matches!(out.get(2), Some(AgentMessage::ToolResult(tr)) if tr.tool_name == "bash"),
            "an unrelated tool result must survive"
        );
    }

    /// An assistant message that was ONLY a `subagent` call has nothing left to say and is dropped
    /// (pi returns `undefined` for it, `:137`).
    #[test]
    fn an_assistant_message_that_was_only_a_subagent_call_is_dropped() {
        let messages = vec![assistant(vec![tool_call_block("subagent")])];
        let out = strip_parent_only_subagent_messages(&messages, false).expect("changed");
        assert!(out.is_empty());
    }

    /// A fanout-authorized child keeps its OWN delegation history — those calls are its work.
    #[test]
    fn a_fanout_child_keeps_its_own_subagent_history_but_still_loses_parent_notices() {
        let messages = vec![
            assistant(vec![tool_call_block("subagent")]),
            tool_result("subagent"),
            custom("subagent-notify"),
        ];
        let out = strip_parent_only_subagent_messages(&messages, true).expect("the notice changed it");
        assert_eq!(out.len(), 2, "both subagent tool messages survive");
        assert!(out.iter().all(|m| !matches!(m, AgentMessage::Custom { .. })));
    }

    /// Nothing to strip must report NO change, so the dispatcher leaves the list untouched rather
    /// than treating an identical copy as a mutation.
    #[test]
    fn a_clean_context_reports_no_change() {
        let messages = vec![AgentMessage::user_text("task"), tool_result("bash")];
        assert!(strip_parent_only_subagent_messages(&messages, false).is_none());
    }
}
