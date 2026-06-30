//! Bin-side startup-UI orchestration (Pi `cli/startup-ui.ts` + `cli/session-picker.ts` +
//! `cli/project-trust.ts`): the fixed, app-owned set of pre-launch selectors that run BEFORE the
//! agent runtime is built, over the existing cyrup-tui selectors (`SessionSelector`/`TrustSelector`)
//! driven by [`cyrup_tui::run_startup_selector`].
//!
//! Pi's pre-launch interactive surface lives in `createStartupTui` (startup-ui.ts:77): a bare,
//! short-lived TUI that mounts ONE selector, resolves a value, and tears down before
//! `createAgentSessionRuntime`. The two selectors the bin needs already exist and are public:
//! `SessionSelector` (the `--resume` picker, `selectSession`, session-picker.ts:15) and `TrustSelector`
//! (the project-trust prompt, `createProjectTrustContext`, project-trust.ts:7). What was missing — and
//! is built here — is the orchestration that sources their rows from the L5 seams, runs them, and feeds
//! the result back into session/trust resolution.
//!
//! The **pure** row/label/decision builders are unit-tested in this module; the actual
//! `CrosstermBackend` event loop ([`run_resume_picker`]/[`run_trust_prompt`]) needs a real terminal and
//! is exercised only from `main.rs` (like `run_interactive`).

use std::path::PathBuf;
use std::time::SystemTime;

use cyrup_config::trust::{
    decide_trust, has_trust_requiring_resources, TrustInputs, TrustOption, TrustOutcome, TrustStore,
};
use cyrup_session_svc::{
    AppMode, DefaultProjectTrust, SessionInfo, TrustDecision, TrustEntry,
};
use cyrup_tui::{
    run_startup_selector, ListSelector, SelectKeymap, SelectorOutcome, SessionRow, SessionSelector,
    SessionSelectorOutcome, TrustSelector, UiTheme,
};

// ---------------------------------------------------------------------------------------------
// `--resume` picker (#1) — Pi `selectSession` (session-picker.ts:15-55).
// ---------------------------------------------------------------------------------------------

/// Build the resume-picker rows from the persisted-session listing (Pi `SessionSelectorComponent`
/// rows over the `current`/`all` `SessionsLoader`s). 1:1 with the in-app `/resume` row build
/// (`app.rs::OpenSelector(Session)`): the display label is the name, else the first message, else the
/// id (truncated); the description is the message count with a `(current)` marker; the search text is
/// `{id} {name} {allMessagesText} {cwd}` (Pi `getSessionSearchText`); recency is the modified-time
/// nanos for the relevance tie-break. `current_id` marks the session in progress (none pre-launch).
pub fn session_rows(sessions: &[SessionInfo], current_id: Option<&str>) -> Vec<SessionRow> {
    sessions
        .iter()
        .map(|s| {
            let is_current = current_id.is_some_and(|c| c == s.id.to_string());
            let desc = format!(
                "{} msgs{}",
                s.message_count,
                if is_current { " (current)" } else { "" }
            );
            let search_text = format!(
                "{} {} {} {}",
                s.id,
                s.name.as_deref().unwrap_or(""),
                s.all_messages_text,
                s.cwd
            );
            SessionRow {
                path: s.path.display().to_string(),
                label: session_label(s),
                name: s.name.clone(),
                desc: Some(desc),
                search_text,
                recency: system_time_nanos(s.modified),
            }
        })
        .collect()
}

/// The resume-picker display label (Pi `formatSessionLabel`): name, else first message, else id,
/// flattened to one line and truncated to 80 chars.
fn session_label(info: &SessionInfo) -> String {
    let raw = match &info.name {
        Some(n) if !n.trim().is_empty() => n.clone(),
        _ if !info.first_message.trim().is_empty() => info.first_message.clone(),
        _ => info.id.to_string(),
    };
    truncate_summary(&raw)
}

/// One-line + 80-char truncation (port of cyrup-tui `truncate_summary`).
fn truncate_summary(s: &str) -> String {
    const MAX: usize = 80;
    let one_line = s.replace(['\n', '\r', '\t'], " ");
    if one_line.chars().count() <= MAX {
        one_line
    } else {
        let head: String = one_line.chars().take(MAX.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn system_time_nanos(t: SystemTime) -> u128 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// What the `--resume` picker resolved to (Pi `selectSession` returns the chosen path or `null`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeChoice {
    /// Resume the session at this file path (Pi `SessionManager.open(selectedPath)`).
    Selected(PathBuf),
    /// The picker was cancelled (Pi `null` ⇒ `console.log("No session selected"); process.exit(0)`).
    Cancelled,
}

/// Map a terminal selector outcome to a [`ResumeChoice`] (pure; the TTY loop is separate). A
/// `Confirm` carries the chosen session file path; a `Cancel` is the cancellation.
pub fn interpret_resume(outcome: &SelectorOutcome) -> ResumeChoice {
    match outcome {
        SelectorOutcome::Confirm(path) => ResumeChoice::Selected(PathBuf::from(path)),
        _ => ResumeChoice::Cancelled,
    }
}

/// Run the `--resume` picker over a real terminal (Pi `selectSession`): mount a [`SessionSelector`]
/// built from `sessions`, drive it to a confirm/cancel, and return the choice. Delete `Apply`
/// payloads remove the session file from disk (the selector already dropped the row); rename is a
/// no-op pre-launch (the picker runs before the session services that own the header rewrite —
/// renaming remains available from the in-app `/resume`). TTY-only; not unit-tested.
pub fn run_resume_picker(
    theme: &UiTheme,
    sessions: &[SessionInfo],
    current_id: Option<&str>,
) -> anyhow::Result<ResumeChoice> {
    let rows = session_rows(sessions, current_id);
    let mut selector = SessionSelector::new(rows);
    let keymap = SelectKeymap::default();
    let outcome = run_startup_selector(theme, &keymap, &mut selector, |payload| {
        // The session selector emits delete/rename via a tagged `Apply` payload; effect the delete on
        // disk so the picker stays consistent (Pi's `SessionSelectorComponent` deletes through the
        // loader). Rename is deferred to the in-app `/resume` (no header-rewrite seam pre-launch).
        if let Some(SessionSelectorOutcome::Delete(path)) =
            SessionSelectorOutcome::parse_apply(payload)
        {
            let _ = std::fs::remove_file(&path);
        }
    })?;
    Ok(interpret_resume(&outcome))
}

// ---------------------------------------------------------------------------------------------
// Project-trust prompt (#3) — Pi `createProjectTrustContext` (project-trust.ts:7-62).
// ---------------------------------------------------------------------------------------------

/// Whether the interactive project-trust prompt must run BEFORE the build (Pi
/// `shouldResolveProjectTrust`, main.ts:621): the resolved [`decide_trust`] outcome is
/// [`TrustOutcome::NeedsPrompt`] (i.e. there are trust-requiring project resources, no `--approve`/
/// `--no-approve` override, no saved decision for the folder or an ancestor, and the default policy is
/// `prompt`) AND the run is interactive. Pure over the inputs so it is unit-testable.
pub fn trust_needs_prompt(
    has_resources: bool,
    trust_override: Option<bool>,
    saved: Option<TrustDecision>,
    default_trust: DefaultProjectTrust,
    mode: AppMode,
) -> bool {
    if mode != AppMode::Interactive {
        return false;
    }
    let outcome = decide_trust(TrustInputs {
        has_resources,
        trust_override,
        saved,
        default_trust,
        mode,
        prompt_choice: None,
    });
    outcome == TrustOutcome::NeedsPrompt
}

/// The `saved decision` header line for the trust prompt (Pi `formatSavedTrust`): `none`, or
/// `trusted (<path>)` / `untrusted (<path>)`.
pub fn format_saved_trust(saved: &Option<TrustEntry>) -> String {
    match saved {
        None => "none".to_string(),
        Some(entry) => {
            let label = if entry.decision.is_trusted() { "trusted" } else { "untrusted" };
            format!("{label} ({})", entry.path.display())
        }
    }
}

/// The preselected option index for the trust prompt (Pi `app.rs::OpenSelector(Trust)` selection
/// logic): the option whose trust + saved path matches the nearest saved decision, else the first
/// option (`Trust`).
pub fn trust_selected_index(options: &[TrustOption], saved: &Option<TrustEntry>) -> usize {
    options
        .iter()
        .position(|o| {
            saved.as_ref().is_some_and(|s| {
                s.decision.is_trusted() == o.trusted
                    && o.saved_path.as_deref() == Some(s.path.as_path())
            })
        })
        .unwrap_or(0)
}

/// What the project-trust prompt resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustChoice {
    /// The user picked option `index`; the run trusts iff `trusted` and the option's `updates` are
    /// persisted to the trust store.
    Chosen { index: usize, trusted: bool },
    /// The prompt was cancelled (Pi `ui.select → undefined`): proceed untrusted, persist nothing.
    Cancelled,
}

/// Map a terminal `TrustSelector` outcome to a [`TrustChoice`] (pure). `Confirm` carries the chosen
/// option INDEX (the selector encodes it as a decimal string); any out-of-range / unparsable index or
/// a `Cancel` is a cancellation.
pub fn interpret_trust(outcome: &SelectorOutcome, options: &[TrustOption]) -> TrustChoice {
    match outcome {
        SelectorOutcome::Confirm(idx) => match idx.parse::<usize>() {
            Ok(i) if i < options.len() => {
                let trusted = options.get(i).map(|o| o.trusted).unwrap_or(false);
                TrustChoice::Chosen { index: i, trusted }
            }
            _ => TrustChoice::Cancelled,
        },
        _ => TrustChoice::Cancelled,
    }
}

/// Run the project-trust prompt over a real terminal (Pi `createProjectTrustContext`'s
/// `ui.select`/`showStartupSelector`): mount a [`TrustSelector`] over the `options`, drive it to a
/// confirm/cancel, persist the chosen option's `updates` to the `trust_store`, and return the
/// resolved trust decision (`Some(true/false)` ⇒ feed as the run's `trust_override`; `None` ⇒
/// cancelled, proceed untrusted). TTY-only; not unit-tested.
pub fn run_trust_prompt(
    theme: &UiTheme,
    cwd: &std::path::Path,
    options: &[TrustOption],
    saved: &Option<TrustEntry>,
    trust_store: &TrustStore,
) -> anyhow::Result<Option<bool>> {
    if options.is_empty() {
        return Ok(None);
    }
    let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();
    let selected = trust_selected_index(options, saved);
    let mut selector = TrustSelector::new(
        cwd.display().to_string(),
        format_saved_trust(saved),
        false,
        labels,
        selected,
    );
    let keymap = SelectKeymap::default();
    let outcome = run_startup_selector(theme, &keymap, &mut selector, |_| {})?;
    match interpret_trust(&outcome, options) {
        TrustChoice::Chosen { index, trusted } => {
            if let Some(option) = options.get(index) {
                // Persist the decision (Pi `write_project_trust(option.updates)`); a session-only
                // option has empty updates and writes nothing.
                trust_store
                    .set_many(&option.updates)
                    .map_err(|e| anyhow::anyhow!("writing project trust: {e}"))?;
            }
            Ok(Some(trusted))
        }
        TrustChoice::Cancelled => Ok(None),
    }
}

// ---------------------------------------------------------------------------------------------
// Missing-session-cwd prompt (#3) — Pi `promptForMissingSessionCwd` → `showStartupSelector`
// (main.ts:573-585; startup-ui.ts:134-163).
// ---------------------------------------------------------------------------------------------

/// The `continue` row's value sentinel for the missing-session-cwd prompt.
const MISSING_CWD_CONTINUE: &str = "continue";

/// What the missing-session-cwd prompt resolved to (Pi `promptForMissingSessionCwd` returns the
/// chosen cwd or `undefined`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MissingCwdChoice {
    /// Reopen the session against the fallback cwd (Pi `{ label: "Continue", value: fallbackCwd }`).
    Continue,
    /// Cancel → exit 0 (Pi `{ label: "Cancel", value: undefined }` ⇒ `process.exit(0)`).
    Cancel,
}

/// Map a terminal selector outcome to a [`MissingCwdChoice`] (pure). Only the `Continue` row confirms
/// a continuation; a `Cancel` or any other row is a cancellation.
pub fn interpret_missing_cwd(outcome: &SelectorOutcome) -> MissingCwdChoice {
    match outcome {
        SelectorOutcome::Confirm(value) if value == MISSING_CWD_CONTINUE => MissingCwdChoice::Continue,
        _ => MissingCwdChoice::Cancel,
    }
}

/// Run the interactive missing-session-cwd Continue/Cancel prompt over a real terminal (Pi
/// `promptForMissingSessionCwd` → `showStartupSelector`, main.ts:454-462): mount a two-option
/// [`ListSelector`] titled with `prompt_body`, drive it to a confirm/cancel, and return the choice.
/// TTY-only; not unit-tested (the pure mapper [`interpret_missing_cwd`] is).
pub fn run_missing_cwd_prompt(
    theme: &UiTheme,
    prompt_body: &str,
    fallback_cwd: &std::path::Path,
) -> anyhow::Result<MissingCwdChoice> {
    // The title's first line (the `ListSelector` header is single-line); the full Pi body's cwd lines
    // are surfaced as the option descriptions.
    let title = prompt_body.lines().next().unwrap_or(prompt_body).to_string();
    let rows = vec![
        (
            MISSING_CWD_CONTINUE.to_string(),
            "Continue".to_string(),
            Some(format!("continue in current cwd: {}", fallback_cwd.display())),
        ),
        ("cancel".to_string(), "Cancel".to_string(), None),
    ];
    let mut selector = ListSelector::prompt(title, rows, 0);
    let keymap = SelectKeymap::default();
    let outcome = run_startup_selector(theme, &keymap, &mut selector, |_| {})?;
    Ok(interpret_missing_cwd(&outcome))
}

/// Whether the cwd has trust-requiring project resources (Pi `hasTrustRequiringProjectResources`).
/// Thin re-export so `main.rs` reads the predicate without a direct `cyrup-config` trust import.
pub fn has_trust_requiring_project_resources(cwd: &std::path::Path, home: &std::path::Path) -> bool {
    has_trust_requiring_resources(cwd, home)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use cyrup_sdk::core::SessionId;
    use std::path::Path;

    fn info(id: &str, name: Option<&str>, first: &str, msgs: usize) -> SessionInfo {
        SessionInfo {
            path: PathBuf::from(format!("/s/{id}.jsonl")),
            id: SessionId::from(id),
            cwd: "/work".to_string(),
            name: name.map(str::to_string),
            parent_session_path: None,
            created: SystemTime::UNIX_EPOCH,
            modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(msgs as u64),
            message_count: msgs,
            first_message: first.to_string(),
            all_messages_text: format!("{first} more text"),
        }
    }

    #[test]
    fn session_rows_label_desc_and_search_match_pi() {
        let sessions = vec![
            info("aaa", Some("My Session"), "hello world", 3),
            info("bbb", None, "first message here", 1),
            info("ccc", None, "", 0),
        ];
        let rows = session_rows(&sessions, Some("bbb"));
        // Named session → name is the label.
        assert_eq!(rows[0].label, "My Session");
        assert_eq!(rows[0].desc.as_deref(), Some("3 msgs"));
        // Unnamed → first message is the label; marked current via `current_id`.
        assert_eq!(rows[1].label, "first message here");
        assert_eq!(rows[1].desc.as_deref(), Some("1 msgs (current)"));
        // No name + no message → id is the label.
        assert_eq!(rows[2].label, "ccc");
        // Search text is `{id} {name} {allMessagesText} {cwd}`.
        assert!(rows[0].search_text.contains("aaa"));
        assert!(rows[0].search_text.contains("My Session"));
        assert!(rows[0].search_text.contains("/work"));
        // Recency strictly increases with the modified time so the relevance tie-break is stable.
        assert!(rows[0].recency > rows[2].recency);
    }

    #[test]
    fn interpret_resume_maps_confirm_and_cancel() {
        assert_eq!(
            interpret_resume(&SelectorOutcome::Confirm("/s/aaa.jsonl".to_string())),
            ResumeChoice::Selected(PathBuf::from("/s/aaa.jsonl"))
        );
        assert_eq!(interpret_resume(&SelectorOutcome::Cancel), ResumeChoice::Cancelled);
        // A stray Redraw/Ignored is treated as "no selection yet" → cancelled by the mapper.
        assert_eq!(interpret_resume(&SelectorOutcome::Redraw), ResumeChoice::Cancelled);
    }

    #[test]
    fn trust_needs_prompt_only_when_decide_trust_says_so_and_interactive() {
        // Trust-requiring resources, no override, no saved, default=prompt, interactive → NeedsPrompt.
        assert!(trust_needs_prompt(
            true,
            None,
            None,
            DefaultProjectTrust::Ask,
            AppMode::Interactive
        ));
        // Non-interactive never prompts (the non-interactive policy resolves it).
        assert!(!trust_needs_prompt(true, None, None, DefaultProjectTrust::Ask, AppMode::Print));
        // An explicit override resolves without a prompt.
        assert!(!trust_needs_prompt(
            true,
            Some(true),
            None,
            DefaultProjectTrust::Ask,
            AppMode::Interactive
        ));
        // No trust-requiring resources → nothing to gate.
        assert!(!trust_needs_prompt(
            false,
            None,
            None,
            DefaultProjectTrust::Ask,
            AppMode::Interactive
        ));
        // A saved decision resolves it (no prompt).
        assert!(!trust_needs_prompt(
            true,
            None,
            Some(TrustDecision::Trusted),
            DefaultProjectTrust::Ask,
            AppMode::Interactive
        ));
        // Default `always`/`never` resolve without a prompt.
        assert!(!trust_needs_prompt(
            true,
            None,
            None,
            DefaultProjectTrust::Always,
            AppMode::Interactive
        ));
    }

    #[test]
    fn interpret_missing_cwd_maps_continue_and_cancel() {
        assert_eq!(
            interpret_missing_cwd(&SelectorOutcome::Confirm("continue".to_string())),
            MissingCwdChoice::Continue
        );
        // Any other confirm value (e.g. the Cancel row) is a cancellation.
        assert_eq!(
            interpret_missing_cwd(&SelectorOutcome::Confirm("cancel".to_string())),
            MissingCwdChoice::Cancel
        );
        assert_eq!(interpret_missing_cwd(&SelectorOutcome::Cancel), MissingCwdChoice::Cancel);
        assert_eq!(interpret_missing_cwd(&SelectorOutcome::Redraw), MissingCwdChoice::Cancel);
    }

    #[test]
    fn format_saved_trust_matches_pi() {
        assert_eq!(format_saved_trust(&None), "none");
        let entry = TrustEntry {
            path: PathBuf::from("/work"),
            decision: TrustDecision::Trusted,
        };
        assert_eq!(format_saved_trust(&Some(entry)), "trusted (/work)");
        let entry2 = TrustEntry {
            path: PathBuf::from("/work"),
            decision: TrustDecision::Untrusted,
        };
        assert_eq!(format_saved_trust(&Some(entry2)), "untrusted (/work)");
    }

    #[test]
    fn trust_selected_index_prefers_the_saved_option() {
        let options = cyrup_config::trust::trust_options(Path::new("/work"), false);
        // No saved decision → preselect the first option (Trust).
        assert_eq!(trust_selected_index(&options, &None), 0);
        // A saved "trusted" at the cwd preselects the matching option.
        let saved = Some(TrustEntry {
            path: PathBuf::from("/work"),
            decision: TrustDecision::Trusted,
        });
        let idx = trust_selected_index(&options, &saved);
        assert!(options.get(idx).is_some_and(|o| o.trusted));
    }

    #[test]
    fn interpret_trust_maps_index_and_cancel() {
        let options = cyrup_config::trust::trust_options(Path::new("/work"), false);
        // Confirm "0" → the first option (Trust → trusted=true).
        assert_eq!(
            interpret_trust(&SelectorOutcome::Confirm("0".to_string()), &options),
            TrustChoice::Chosen { index: 0, trusted: true }
        );
        // An out-of-range index is treated as a cancellation (no write).
        assert_eq!(
            interpret_trust(&SelectorOutcome::Confirm("99".to_string()), &options),
            TrustChoice::Cancelled
        );
        assert_eq!(interpret_trust(&SelectorOutcome::Cancel, &options), TrustChoice::Cancelled);
    }
}
