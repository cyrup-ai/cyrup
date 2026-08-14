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
    TrustInputs, TrustOption, TrustOutcome, TrustStore, decide_trust, has_trust_requiring_resources,
};
use cyrup_session_svc::{AppMode, DefaultProjectTrust, SessionInfo, TrustDecision, TrustEntry};
use cyrup_tui::{
    ListSelector, SelectKeymap, SelectorOutcome, SessionRow, SessionSelector,
    SessionSelectorOutcome, TrustSelector, UiTheme, run_startup_selector,
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
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
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
            let label = if entry.decision.is_trusted() {
                "trusted"
            } else {
                "untrusted"
            };
            format!("{label} ({})", entry.path.display())
        }
    }
}

/// The option matching the persisted decision, if any (Pi `isSavedOption`,
/// `trust-selector.ts:92-98`): its trust flag AND its saved path both equal the nearest saved
/// decision's. This is what drives the ` ✓` saved-decision marker (S20); it is deliberately
/// `Option`, because upstream distinguishes "no option matches" from "option 0 matches".
pub fn trust_saved_index(options: &[TrustOption], saved: &Option<TrustEntry>) -> Option<usize> {
    options.iter().position(|o| {
        saved.as_ref().is_some_and(|s| {
            s.decision.is_trusted() == o.trusted && o.saved_path.as_deref() == Some(s.path.as_path())
        })
    })
}

/// The preselected option index for the trust prompt (Pi `app.rs::OpenSelector(Trust)` selection
/// logic): the option whose trust + saved path matches the nearest saved decision, else the first
/// option (`Trust`) — Pi's `Math.max(0, findIndex(isSavedOption))`, `trust-selector.ts:45-48`.
pub fn trust_selected_index(options: &[TrustOption], saved: &Option<TrustEntry>) -> usize {
    trust_saved_index(options, saved).unwrap_or(0)
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

/// Persist a chosen trust option's `updates` — a literal port of pi's
/// `saveProjectTrustPromptResult` (`packages/coding-agent/src/core/project-trust.ts:40-44`
/// @v0.83.0):
///
/// ```text
/// function saveProjectTrustPromptResult(trustStore: ProjectTrustStore, result: ProjectTrustOption): void {
///     if (result.updates.length > 0) {
///         trustStore.setMany(result.updates);
///     }
/// }
/// ```
///
/// The `updates.length > 0` guard is load-bearing, not an optimisation. Both session-only rows
/// (`trust_options(cwd, true)` indices 2 and 4) carry an EMPTY `updates`, and
/// [`TrustStore::set_many`] is unconditional once entered: it takes a `FileLock` (creating
/// `trust.json.lock`), re-reads and re-serialises the whole map, and `write_atomic`s the result
/// (`cyrup-config/src/trust.rs:160-186`) even for an empty slice. Calling it anyway is what made a
/// "…(this session only)" answer leave exactly the permanent trace the row exists to avoid: an
/// otherwise-untouched agent dir gained a `trust.json` (`{}`) plus a `trust.json.lock`, and a
/// pre-seeded store was rewritten byte-for-byte-differently. SEAM-064.
fn persist_trust_choice(trust_store: &TrustStore, option: &TrustOption) -> anyhow::Result<()> {
    if option.updates.is_empty() {
        return Ok(());
    }
    trust_store
        .set_many(&option.updates)
        .map_err(|e| anyhow::anyhow!("writing project trust: {e}"))
}

/// Run the project-trust prompt over a real terminal (Pi `createProjectTrustContext`'s
/// `ui.select`/`showStartupSelector`): mount a [`TrustSelector`] over the `options`, drive it to a
/// confirm/cancel, persist the chosen option's `updates` through pi's `updates.length > 0` guard
/// ([`persist_trust_choice`], project-trust.ts:40-44), and return the resolved trust decision
/// (`Some(true/false)` ⇒ feed as the run's `trust_override`; `None` ⇒ cancelled, proceed untrusted).
/// TTY-only; the persistence branch it runs is unit-tested through [`persist_trust_choice`].
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
    )
    // S20: the ` ✓` on the option the trust store already holds (`trust-selector.ts:109-110`).
    .with_saved_index(trust_saved_index(options, saved));
    let keymap = SelectKeymap::default();
    let outcome = run_startup_selector(theme, &keymap, &mut selector, |_| {})?;
    match interpret_trust(&outcome, options) {
        TrustChoice::Chosen { index, trusted } => {
            if let Some(option) = options.get(index) {
                // Persist the decision through pi's `saveProjectTrustPromptResult` guard
                // (project-trust.ts:40-44) — a session-only option has empty `updates` and must not
                // reach `set_many` at all. See [`persist_trust_choice`].
                persist_trust_choice(trust_store, option)?;
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
        SelectorOutcome::Confirm(value) if value == MISSING_CWD_CONTINUE => {
            MissingCwdChoice::Continue
        }
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
    let title = prompt_body
        .lines()
        .next()
        .unwrap_or(prompt_body)
        .to_string();
    let rows = vec![
        (
            MISSING_CWD_CONTINUE.to_string(),
            "Continue".to_string(),
            Some(format!(
                "continue in current cwd: {}",
                fallback_cwd.display()
            )),
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
pub fn has_trust_requiring_project_resources(
    cwd: &std::path::Path,
    home: &std::path::Path,
) -> bool {
    has_trust_requiring_resources(cwd, home)
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
        assert_eq!(
            interpret_resume(&SelectorOutcome::Cancel),
            ResumeChoice::Cancelled
        );
        // A stray Redraw/Ignored is treated as "no selection yet" → cancelled by the mapper.
        assert_eq!(
            interpret_resume(&SelectorOutcome::Redraw),
            ResumeChoice::Cancelled
        );
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
        assert!(!trust_needs_prompt(
            true,
            None,
            None,
            DefaultProjectTrust::Ask,
            AppMode::Print
        ));
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
        assert_eq!(
            interpret_missing_cwd(&SelectorOutcome::Cancel),
            MissingCwdChoice::Cancel
        );
        assert_eq!(
            interpret_missing_cwd(&SelectorOutcome::Redraw),
            MissingCwdChoice::Cancel
        );
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
            TrustChoice::Chosen {
                index: 0,
                trusted: true
            }
        );
        // An out-of-range index is treated as a cancellation (no write).
        assert_eq!(
            interpret_trust(&SelectorOutcome::Confirm("99".to_string()), &options),
            TrustChoice::Cancelled
        );
        assert_eq!(
            interpret_trust(&SelectorOutcome::Cancel, &options),
            TrustChoice::Cancelled
        );
    }

    /// Snapshot a directory's entry names, sorted — so a stray `trust.json` / `trust.json.lock`
    /// appearing is observable, not just a change to a file that already existed.
    fn dir_entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// SEAM-064 — the PRE-LAUNCH prompt (`main.rs`, pi `selectProjectTrustOption`,
    /// project-trust.ts:32) asks for `includeSessionOnly: true`, so it renders pi's FIVE rows in
    /// pi's order (`getProjectTrustOptions`, trust-manager.ts:65-95), and either session-only row
    /// carries an EMPTY `updates` — pi's `saveProjectTrustPromptResult` (project-trust.ts:40-44)
    /// only calls `setMany` when `updates.length > 0`, which is how an answer avoids recording a
    /// permanent verdict.
    ///
    /// The name's second clause is an observation, not a paraphrase of the options data. The first
    /// half of this test asserts the option SET; the second half drives the actual writer
    /// ([`persist_trust_choice`], the branch `run_trust_prompt` runs) and looks at the disk: a
    /// folder with no trust store must still have none afterwards — no `{}` `trust.json`, no
    /// `trust.json.lock` — and a pre-seeded store must come back byte-identical. Asserting only
    /// `option.updates.is_empty()` is what let the missing guard ship: it passed happily while
    /// `run_trust_prompt` called `set_many` unconditionally.
    #[test]
    fn pre_launch_trust_prompt_offers_pi_five_rows_and_session_only_writes_nothing() {
        let options = cyrup_config::trust::trust_options(Path::new("/work"), true);
        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Trust",
                "Trust parent folder (/)",
                "Trust (this session only)",
                "Do not trust",
                "Do not trust (this session only)",
            ]
        );
        // The two session-only rows keep their verdict for this run and persist NOTHING.
        for index in [2usize, 4] {
            let option = &options[index];
            assert!(
                option.updates.is_empty(),
                "row {index} ({}) must write nothing",
                option.label
            );
            assert!(option.saved_path.is_none());
            assert_eq!(
                interpret_trust(&SelectorOutcome::Confirm(index.to_string()), &options),
                TrustChoice::Chosen {
                    index,
                    trusted: index == 2
                }
            );
        }
        // …and the persisting rows still do persist, so this is not a blanket disarm.
        for index in [0usize, 1, 3] {
            assert!(!options[index].updates.is_empty());
        }

        // --------------------------------------------------------------------------------------
        // The above is options DATA. What "writes nothing" actually claims is about the WRITER, so
        // drive the persistence branch `run_trust_prompt` runs ([`persist_trust_choice`]) and
        // observe the disk. Without pi's `updates.length > 0` guard (project-trust.ts:40-44) every
        // one of these assertions fails: `TrustStore::set_many` takes a `FileLock`, re-serialises
        // the map and `write_atomic`s it even for an empty slice (cyrup-config/src/trust.rs:160-186).
        // --------------------------------------------------------------------------------------
        let tmp = tempfile::TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        let cwd = tmp.path().join("work");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let store_path = agent_dir.join("trust.json");
        let store = TrustStore::new(store_path.clone());
        let real = cyrup_config::trust::trust_options(&cwd, true);
        let session_only: Vec<usize> = real
            .iter()
            .enumerate()
            .filter(|(_, o)| o.label.contains("this session only"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(session_only.len(), 2, "both session-only rows must be present");

        // 1. A folder with NO trust store: a session-only answer must not CREATE one — no `{}`
        //    trust.json, and no `trust.json.lock` from the lock the writer would have taken.
        assert!(dir_entries(&agent_dir).is_empty());
        for &index in &session_only {
            persist_trust_choice(&store, &real[index]).unwrap();
            assert_eq!(
                dir_entries(&agent_dir),
                Vec::<String>::new(),
                "row {index} ({}) created files in a folder that had none: {:?}",
                real[index].label,
                dir_entries(&agent_dir)
            );
        }
        assert!(!store_path.exists(), "no trust.json may be created");

        // 2. A PRE-SEEDED store must come back BYTE-IDENTICAL. The seed is written as raw bytes in a
        //    compact shape deliberately, because that is what the live repro measured (28 bytes →
        //    34 bytes after a "Trust (this session only)") and because a store seeded through
        //    `set_many` itself would NOT prove anything here: re-serialising an unchanged
        //    `BTreeMap` is idempotent, so an unguarded `set_many(&[])` over canonical bytes lands
        //    the same bytes back. Against a hand-written store it does not — it normalises the file
        //    to sorted pretty JSON + trailing newline. Byte-identity is only an observation of the
        //    absent write when the on-disk form is one the writer would change.
        let key = real[0]
            .saved_path
            .as_deref()
            .expect("the `Trust` row persists the canonicalised cwd")
            .to_string_lossy()
            .into_owned();
        let seed = format!("{{{:?}:true}}", key).into_bytes();
        std::fs::write(&store_path, &seed).expect("seed the trust store by hand");
        let before = std::fs::read(&store_path).expect("seeded trust.json");
        let before_entries = dir_entries(&agent_dir);
        assert_eq!(before, seed);
        assert_eq!(
            before_entries,
            vec!["trust.json".to_string()],
            "the hand-written seed must not have produced a lock file"
        );
        assert!(
            store
                .nearest(&cwd)
                .unwrap()
                .is_some_and(|e| e.decision.is_trusted()),
            "the seed must be a store the reader actually accepts"
        );
        for &index in &session_only {
            persist_trust_choice(&store, &real[index]).unwrap();
            assert_eq!(
                std::fs::read(&store_path).expect("trust.json still readable"),
                before,
                "row {index} ({}) rewrote the trust store",
                real[index].label
            );
            assert_eq!(dir_entries(&agent_dir), before_entries);
        }

        // 3. The same helper on a PERSISTING row does write — so the assertions above are capable
        //    of observing a write, and the guard is not a blanket disarm.
        let do_not_trust = real
            .iter()
            .position(|o| o.label == "Do not trust")
            .expect("the `Do not trust` row");
        persist_trust_choice(&store, &real[do_not_trust]).unwrap();
        let after = std::fs::read(&store_path).expect("trust.json after a persisting row");
        assert_ne!(
            after, before,
            "a persisting row must still reach `set_many` (pi calls it when updates.length > 0)"
        );
        assert!(
            store
                .nearest(&cwd)
                .unwrap()
                .is_some_and(|e| !e.decision.is_trusted()),
            "the persisting row's verdict must be readable back from the store"
        );
    }
}
