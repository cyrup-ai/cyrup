//! SCOPE_10 SUBTASK3 — `view: "transcript"` over a run this process is driving in the FOREGROUND:
//! pi `formatLiveForegroundTranscript` (`runs/background/run-status.ts:247-292` `@7fe9dee1`), whose
//! own header sentence is *"Request-only snapshot of the same artifact used by Fleet; no session or
//! output fallback."*
//!
//! # Why the whole function, and not just its gate
//!
//! The task's subject is upstream's `:249` — the one session gate upstream implements as a THROW.
//! `:249` is the first statement of this function and guards nothing else; porting the predicate
//! alone would land a refusal with nothing to refuse, because cyrup's
//! [`crate::background::run_status::resolve_run_id`] scans only ASYNC run directories, so a
//! foreground id resolves to `Async run not found. Provide id or dir.` and never reaches a
//! transcript at all. The function is the gate's reason to exist, so it lands with it.
//!
//! # A `throw` is an `Err(String)`
//!
//! Upstream throws four times here and the host turns each into an error result. cyrup's control
//! surface has no `isError` flag; the crate's convention — stated at
//! [`crate::extension::executor::status`]'s control-action block — is that `Err` IS the
//! user-facing failure message the tool surface turns into a `ToolError`. That is what porting
//! "the shape" means. It is emphatically not `panic!`: `lib.rs:19-24` denies panicking crate-wide.
//!
//! # `[CYRUP-DELTA]`s
//!
//! * **`options.index` cannot be a non-integer or negative** (`:252-254`). cyrup's selector
//!   carries `Option<usize>` ([`crate::extension::executor::requests::StatusViewSelector`]), so
//!   upstream's `Number.isInteger(...) && >= 0` assertion is discharged at the PARSE boundary and
//!   the branch is unrepresentable rather than dropped.
//! * **`control.activeChildren` is not optional here.**
//!   [`ForegroundControlEntry::active_children`] is a `BTreeMap`, always present, possibly empty;
//!   upstream's field is an optional `Map`. An EMPTY map carries exactly the information
//!   upstream's ABSENT field does — "this entry has no per-child records" — so it takes upstream's
//!   `!control.activeChildren` branch and falls through to the `currentAgent` synthesis. Both of
//!   upstream's observable outcomes stay reachable (a synthesized single child; the
//!   `no active foreground child` sentence when there is no agent either); only the spelling of
//!   the input differs.
//! * **The 32 KiB tail is written locally, not with
//!   [`crate::workflows::truncate_to_bytes`].** That helper appends `"..."` and cuts from the
//!   FRONT of the budget; upstream keeps the TAIL and skips UTF-8 continuation bytes at the new
//!   start (`:283-287`). They are not interchangeable — one shows the beginning of a transcript,
//!   the other its most recent activity, which is the entire point of a live tail.
//! * **[`safe_display_text`] is the right sanitizer HERE**, and it is pi `safeTerminalText`. The
//!   OTHER one ([`crate::workflows::sanitize_display_text`], pi `sanitizeDisplayText`) strips
//!   rather than escapes and is what the async status SNAPSHOT uses;
//!   `workflows/display_text.rs:1-9` warns about exactly this confusion.

use std::path::PathBuf;

use crate::artifacts::ArtifactDirPreference;
use crate::background::delivery::SessionGate;
use crate::extension::executor::notices::ForegroundControlEntry;
use crate::identity::SessionId;
use crate::tui::fleet_transcript::{
    FleetTranscriptEvent, FleetTranscriptReadOptions, ToolStatus, read_fleet_transcript,
    safe_display_text,
};

/// pi `: 80` (`run-status.ts:268`) — this surface's own default, distinct from
/// [`crate::background::fleet_view::DEFAULT_TRANSCRIPT_LINES`] only in that it is stated inline
/// upstream rather than as a constant.
const DEFAULT_LIVE_TRANSCRIPT_LINES: i64 = 80;
/// pi `Math.min(500, …)` (`:268`).
const MAX_LIVE_TRANSCRIPT_LINES: i64 = 500;
/// pi `maxOutputBytes = 32 * 1024` (`:278`).
const MAX_OUTPUT_BYTES: usize = 32 * 1024;
/// pi `body.includes("… message truncated")` (`:277`) — the marker
/// [`crate::tui::fleet_transcript`]'s own message clipper writes (`:562`), re-detected here rather
/// than re-derived.
const MESSAGE_TRUNCATED_MARKER: &str = "… message truncated";

/// The subset of pi's `SubagentState` this renderer reads (`:265`): the parent session file, the
/// base cwd, the artifact-dir preference and the current session id.
///
/// A dedicated record rather than [`crate::tui::fleet_state::FleetState`]: building that state is
/// an `async` scan of the whole async root, and a transcript request for one live foreground run
/// has no business paying for it.
pub(crate) struct LiveForegroundTranscriptState {
    /// pi `state.currentSessionId` — the `:249` gate's left-hand side.
    pub(crate) current_session: Option<SessionId>,
    /// pi `state.parentSessionFile`.
    pub(crate) parent_session_file: Option<PathBuf>,
    /// pi `state.baseCwd` — the fallback when the control recorded no cwd of its own.
    pub(crate) base_cwd: PathBuf,
    /// pi `state.artifactDirPreference` (SUBA-048).
    pub(crate) artifact_dir_preference: ArtifactDirPreference,
}

/// One entry of the child list `:255-257` builds — either a real
/// [`crate::extension::executor::foreground_control::ForegroundChildEntry`] or the single child
/// `:257` synthesizes from the parent entry.
struct TranscriptChild {
    index: usize,
    agent: String,
    session_name: Option<String>,
}

/// pi `ToolStatus` as the word `:271` interpolates
/// (`tui/fleet_transcript.rs:231-240`'s serde vocabulary).
fn tool_status_word(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Running => "running",
        ToolStatus::Complete => "complete",
        ToolStatus::Error => "error",
    }
}

/// pi `:269-274`'s `flatMap` — one event rendered to text, then split on line boundaries.
fn event_text(event: &FleetTranscriptEvent) -> String {
    match event {
        FleetTranscriptEvent::Tool(tool) => {
            // `:271` — `[header, argsPayload ?? args, output ?? error].filter(Boolean).join("\n")`.
            let mut parts: Vec<&str> = Vec::new();
            let header = format!("Tool: {} ({})", tool.name, tool_status_word(tool.status));
            parts.push(header.as_str());
            if let Some(args) = tool
                .args_payload
                .as_deref()
                .or(tool.args.as_deref())
                .filter(|text| !text.is_empty())
            {
                parts.push(args);
            }
            if let Some(output) = tool
                .output
                .as_deref()
                .or(tool.error.as_deref())
                .filter(|text| !text.is_empty())
            {
                parts.push(output);
            }
            parts.join("\n")
        }
        FleetTranscriptEvent::Notice { text, .. } => text.clone(),
        FleetTranscriptEvent::Assistant { text, .. } => format!("Assistant: {text}"),
        // `:273` — a `user` record in a child transcript is the SUPERVISOR talking to the child.
        FleetTranscriptEvent::User { text, .. } => format!("Supervisor: {text}"),
    }
}

/// pi `formatLiveForegroundTranscript` (`run-status.ts:247-292`).
///
/// `control_run_id` is the `foreground_controls` MAP KEY: [`ForegroundControlEntry`] carries no
/// run id of its own (pi's `ForegroundRunControl.runId` is a field), the same reason
/// `workflow_steering.rs:55-58` carries one alongside its cloned entry.
///
/// # Errors
///
/// Every `Err` is one of upstream's four verbatim `throw` messages.
pub(crate) fn format_live_foreground_transcript(
    control: &ForegroundControlEntry,
    control_run_id: &str,
    state: &LiveForegroundTranscriptState,
    index: Option<usize>,
    lines: Option<i64>,
) -> Result<String, String> {
    // `:249-251` — STRICT. `SessionGate::Strict` names this very line in its own doc
    // (`background/delivery/gate.rs:29-35`) and is the correct class here: upstream refuses when
    // there is NO current session (`!state.currentSessionId`) just as firmly as when the sessions
    // differ, which is precisely `Strict`'s `None => false` arm and precisely what distinguishes
    // it from `Permissive`. (Contrast S6 in `background/run_status.rs`, where neither gate class
    // is right — the difference is upstream's, not cyrup's.)
    if !SessionGate::Strict.admits(state.current_session.as_ref(), control.session_id.as_ref()) {
        return Err(format!(
            "Foreground run '{control_run_id}' is not owned by the current session."
        ));
    }

    // `:255-257` — the real per-child records, else the single child synthesized from the parent
    // entry. See this module's delta on `activeChildren`'s optionality.
    let children: Vec<TranscriptChild> = if control.active_children.is_empty() {
        control
            .current_agent
            .as_ref()
            .map(|agent| TranscriptChild {
                index: control.current_index.unwrap_or(0),
                agent: agent.clone(),
                session_name: control.session_name.clone(),
            })
            .into_iter()
            .collect()
    } else {
        control
            .active_children
            .values()
            .map(|child| TranscriptChild {
                index: child.index,
                agent: child.agent.clone(),
                session_name: child.session_name.clone(),
            })
            .collect()
    };

    let mut header: Vec<String> = vec![
        format!("Run: {control_run_id}"),
        "State: live foreground".to_string(),
    ];
    // `:259` — note the trailing sentence is NOT sanitized upstream; only the header lines are.
    if children.is_empty() {
        let head = header
            .iter()
            .map(|line| safe_display_text(line))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(format!(
            "{head}\nTranscript unavailable: no active foreground child."
        ));
    }
    // `:260-262`.
    if index.is_none() && children.len() > 1 {
        let indexes = children
            .iter()
            .map(|child| child.index.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "Transcript view requires index for foreground run '{control_run_id}'. Active child \
             indexes: {indexes}."
        ));
    }
    // `:263-264`.
    let child = match index {
        None => children.first(),
        Some(wanted) => children.iter().find(|child| child.index == wanted),
    };
    let Some(child) = child else {
        // Unreachable with `index == None` (the list is non-empty above), which is why upstream's
        // message interpolates the index unconditionally.
        let wanted = index.map_or_else(String::new, |i| i.to_string());
        return Err(format!(
            "Transcript index {wanted} is not an active foreground child of '{control_run_id}'."
        ));
    };

    // `:265` — pi `getArtifactsDir(state.parentSessionFile ?? null, control.cwd ?? state.baseCwd,
    // state.artifactDirPreference)`. cyrup's resolver takes the temp-root cwd as a fourth
    // argument that upstream derives internally; the SAME cwd feeds both, so the `temp`
    // preference lands where upstream's does.
    let cwd = control
        .cwd
        .clone()
        .unwrap_or_else(|| state.base_cwd.clone());
    let root = crate::artifacts::resolve_artifacts_dir(
        state.parent_session_file.as_deref(),
        Some(&cwd),
        &cwd,
        state.artifact_dir_preference,
    );
    // `:266`.
    let transcript_path =
        crate::artifacts::artifact_paths(&root, control_run_id, &child.agent, Some(child.index))
            .transcript_path;
    // `:267` — pi `{ trustedRoots: [root] }`. ⚠ an EMPTY root list refuses the read outright
    // (`tui/fleet_transcript.rs:348-350`); there is no "no roots means anything goes" mode, which
    // is why the root is passed rather than defaulted.
    let transcript = read_fleet_transcript(
        &transcript_path,
        &FleetTranscriptReadOptions {
            trusted_roots: vec![root],
            ..FleetTranscriptReadOptions::default()
        },
    );
    // `:268`.
    let line_limit = usize::try_from(
        lines
            .unwrap_or(DEFAULT_LIVE_TRANSCRIPT_LINES)
            .clamp(1, MAX_LIVE_TRANSCRIPT_LINES),
    )
    .unwrap_or(DEFAULT_LIVE_TRANSCRIPT_LINES as usize);

    // `:269-274`.
    let all_lines: Vec<String> = transcript
        .events
        .iter()
        .flat_map(|event| {
            event_text(event)
                .split('\n')
                .map(|line| line.trim_end_matches('\r').to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    // `:275` — `allLines.slice(-lineLimit)`.
    let tail_start = all_lines.len().saturating_sub(line_limit);
    let mut body = all_lines.get(tail_start..).unwrap_or_default().join("\n");
    // `:276-277`.
    let mut truncated = transcript.truncated
        || all_lines.len() > line_limit
        || transcript.events.iter().any(
            |event| matches!(event, FleetTranscriptEvent::Tool(tool) if tool.output_truncated),
        )
        || body.contains(MESSAGE_TRUNCATED_MARKER);
    // `:279-287` — keep the TAIL, then walk forward off any UTF-8 continuation byte the cut
    // landed inside. `is_char_boundary` is the same test upstream spells as `(b & 0xc0) == 0x80`,
    // expressed on the safe side of the `str` API.
    if body.len() > MAX_OUTPUT_BYTES {
        let mut start = body.len() - MAX_OUTPUT_BYTES;
        while start < body.len() && !body.is_char_boundary(start) {
            start += 1;
        }
        body = body.get(start..).unwrap_or_default().to_string();
        truncated = true;
    }

    // `:288-291`.
    header.push(format!(
        "Child: {} ({})",
        child.index,
        child
            .session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(child.agent.as_str())
    ));
    header.push(format!("Transcript: {}", transcript_path.display()));
    if let Some(warning) = transcript.warning.as_deref() {
        header.push(format!("Transcript warning: {warning}"));
    }
    header.push(format!(
        "Live transcript tail{}:",
        if truncated { " (tail truncated)" } else { "" }
    ));
    if body.is_empty() {
        header.push(
            "Transcript unavailable: no readable activity in the bounded artifact tail yet."
                .to_string(),
        );
    }
    // `:292` — `[...header.map(safeTerminalText), body].filter(Boolean).join("\n")`: the BODY is
    // not re-sanitized (its events already are, `fleet-transcript.ts`'s parser does it), and an
    // empty body is dropped rather than leaving a trailing blank line.
    let mut out: Vec<String> = header.iter().map(|line| safe_display_text(line)).collect();
    if !body.is_empty() {
        out.push(body);
    }
    Ok(out
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}
