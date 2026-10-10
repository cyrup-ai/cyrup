//! WHEN cyrup reports program status over OSC 7501 (TUI-171) — a 1:1 port of
//! `packages/coding-agent/src/modes/interactive/program-status-reporter.ts` (111 lines, new in pi
//! `503c60552` / #10607, the same commit that added the byte layer in
//! [`crate::program_status`]).
//!
//! `working` during agent runs and compaction, `blocked` while a dialog waits for the user, then
//! `done`, `error` or `idle` once the run settles.
//!
//! ## Privacy, which is a design constraint and not a side effect
//!
//! Messages are limited to **the session name, dialog titles, and the first line of errors**
//! (upstream's own class doc, `:11-16`). Prompts and assistant output are never reported — the
//! report goes to the terminal emulator, which may put it in a window title, a taskbar tooltip or
//! a notification, i.e. somewhere a screenshot or a shoulder catches it.
//!
//! ## Why the write is deferred
//!
//! [`Self::take_pending`] exists for the reason [`crate::terminal_progress::TerminalProgress`]'s
//! `pending` does: pi's reporter calls `terminal.setProgramStatus` synchronously from inside its
//! event handler, while cyrup's session-event fold
//! ([`crate::App::ingest_event_rendered_owned`]) is a pure state transition that the run loop turns
//! into terminal output one step later. Draining is what guarantees one write per transition rather
//! than one per frame.

use crate::program_status::{BlockedKind, ProgramState, ProgramStatus};

/// What a blocked dialog is waiting for, and what to say about it (`BlockedStatus`,
/// `program-status-reporter.ts:5`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedStatus {
    /// `permission` for a confirm, `question` for a selection or an input, `auth` for a login
    /// (`interactive-mode.ts:2694`, `:2755-2758`, `:6321`).
    pub kind: BlockedKind,
    /// The dialog's TITLE — never its body, and never anything the user typed.
    pub message: String,
}

/// `firstLine(text)` — `program-status-reporter.ts:7-9`:
/// `text?.split(/\r?\n/, 1)[0]?.trim() || "Error"`.
///
/// Note the `||`: an empty or whitespace-only first line falls back to `"Error"`, which is why the
/// trim happens before the test and not after.
fn first_line(text: Option<&str>) -> String {
    let line = text
        .unwrap_or("")
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r')
        .trim();
    if line.is_empty() {
        "Error".to_string()
    } else {
        line.to_string()
    }
}

/// `ProgramStatusReporter` — `program-status-reporter.ts:17-111`.
///
/// Session-scoped state (one per bound session), while the support bit and the last-written status
/// are process-global in [`crate::program_status`]; see that module's docs for why that split is
/// forced rather than chosen.
#[derive(Clone, Debug, Default)]
pub struct ProgramStatusReporter {
    run_active: bool,
    compacting: bool,
    /// Outcome of the current run, reported once it settles (`:22-23`). Defaults to `done`.
    run_result: Option<ProgramStatus>,
    /// Status while no run is active (`:24-25`). Defaults to `idle`.
    resting_status: Option<ProgramStatus>,
    /// Open dialogs by source, in the order they opened; the most recent one is reported
    /// (`:26-27`). A `Vec` rather than a map because insertion order IS the semantics and
    /// `setBlocked` reopening a source must MOVE it to the end — see [`Self::set_blocked`].
    blocked: Vec<(String, BlockedStatus)>,
    /// `lastReport`, the dedup key (`:28`). Pi uses `JSON.stringify(status)`; the Rust equivalent
    /// of "the serialized status" is the status itself, since [`ProgramStatus`] is `PartialEq`.
    last_report: Option<ProgramStatus>,
    /// The report recorded but not yet written to the terminal, drained by
    /// [`Self::take_pending`]. See the module docs.
    pending: Option<ProgramStatus>,
}

impl ProgramStatusReporter {
    /// A fresh reporter: no run, nothing blocked, resting `idle`.
    pub fn new() -> Self {
        Self::default()
    }

    fn run_result(&self) -> ProgramStatus {
        self.run_result
            .clone()
            .unwrap_or_else(|| ProgramStatus::new(ProgramState::Done))
    }

    fn resting_status(&self) -> ProgramStatus {
        self.resting_status
            .clone()
            .unwrap_or_else(|| ProgramStatus::new(ProgramState::Idle))
    }

    /// `handleEvent(event)` — `program-status-reporter.ts:35-77`. `session_name` is upstream's
    /// `this.getSessionName()`, read at report time; cyrup passes it in because the fold has it to
    /// hand and the reporter has no session handle.
    ///
    /// Every arm not listed `return`s **without reporting** (`:73-74`), and `message_end` returns
    /// early for any role but `assistant` (`:43`). `session_info_changed` changes no state but
    /// FALLS THROUGH to [`Self::report`] (`:70-72`), so the name in the message refreshes.
    pub fn handle_event(
        &mut self,
        ev: &cyrup_session_svc::AgentSessionEvent,
        session_name: Option<&str>,
    ) {
        use cyrup_session_svc::AgentSessionEvent as E;
        // [CYRUP-DELTA, mechanism only] pi's `getSessionName()` is a closure over the live session,
        // and by the time `session_info_changed` reaches `handleEvent` the session already carries
        // the new name, so pi reports the NEW one (`program-status-reporter.ts:70-72` exists for
        // exactly that refresh). cyrup's `AppState::status.session_name` is set by the FOLD arm,
        // which runs after this call, so on that one event the name is taken from the event itself.
        // Same reported value; the difference is only where the string comes from.
        let session_name = match ev {
            E::SessionInfoChanged { name } => name.as_deref(),
            _ => session_name,
        };
        match ev {
            E::AgentStart => {
                self.run_active = true;
                self.run_result = Some(ProgramStatus::new(ProgramState::Done));
            }
            E::MessageEnd { .. } => {
                // `if (event.message.role !== "assistant") return;` (`:43`) — the LATEST assistant
                // response decides the outcome, so a retried error is replaced by its successful
                // retry. Read through the same serde projection every other `message_end` consumer
                // in this crate uses (`app/event_extract.rs`), because `AgentMessage` is not a
                // direct dependency.
                let Some(message) = crate::app::assistant_message_from_event(ev) else {
                    return;
                };
                self.run_result = Some(if message.stop_reason == cyrup_core::StopReason::Error {
                    ProgramStatus::with_message(
                        ProgramState::Error,
                        first_line(message.error_message.as_deref()),
                    )
                } else {
                    ProgramStatus::new(ProgramState::Done)
                });
            }
            E::CompactionStart { .. } => self.compacting = true,
            E::CompactionEnd {
                reason,
                aborted,
                error_message,
                ..
            } => {
                self.compacting = false;
                if self.run_active {
                    // A failed recovery compaction ends the run unless a later response succeeds
                    // (`:54-57`).
                    if *aborted {
                        self.run_result = Some(ProgramStatus::new(ProgramState::Idle));
                    } else if let Some(err) = error_message.as_deref() {
                        self.run_result = Some(ProgramStatus::with_message(
                            ProgramState::Error,
                            first_line(Some(err)),
                        ));
                    }
                } else if *aborted {
                    self.resting_status = Some(ProgramStatus::new(ProgramState::Idle));
                } else if *reason == cyrup_session_svc::CompactionReason::Manual {
                    // `:60-63`. A NON-manual compaction outside a run changes nothing at all.
                    self.resting_status = Some(match error_message.as_deref() {
                        Some(err) => {
                            ProgramStatus::with_message(ProgramState::Error, first_line(Some(err)))
                        }
                        None => ProgramStatus::new(ProgramState::Done),
                    });
                }
            }
            E::AgentSettled { aborted, .. } => {
                self.run_active = false;
                self.resting_status = Some(if *aborted {
                    ProgramStatus::new(ProgramState::Idle)
                } else {
                    self.run_result()
                });
            }
            // No state change; the fall-through to `report()` is the point (`:70-72`).
            E::SessionInfoChanged { .. } => {}
            _ => return,
        }
        self.report(session_name);
    }

    /// `setBlocked(source, status)` — `program-status-reporter.ts:80-84`: `this.blocked.delete(source)`
    /// then, when a status is given, `this.blocked.set(source, status)`.
    ///
    /// The delete-then-insert is load-bearing: reopening a source **moves it to the end** of the
    /// insertion order rather than stacking a second entry or keeping its old position, and
    /// [`Self::current_status`] reports the LAST one.
    pub fn set_blocked(
        &mut self,
        source: &str,
        status: Option<BlockedStatus>,
        session_name: Option<&str>,
    ) {
        self.blocked.retain(|(s, _)| s != source);
        if let Some(status) = status {
            self.blocked.push((source.to_string(), status));
        }
        self.report(session_name);
    }

    /// `reset()` — `program-status-reporter.ts:87-93`: forget the previous session's run, for
    /// example after switching sessions. Open dialogs are deliberately NOT cleared, as upstream.
    pub fn reset(&mut self, session_name: Option<&str>) {
        self.run_active = false;
        self.compacting = false;
        self.run_result = Some(ProgramStatus::new(ProgramState::Done));
        self.resting_status = Some(ProgramStatus::new(ProgramState::Idle));
        self.report(session_name);
    }

    /// `report()` — `program-status-reporter.ts:95-101`: stamp the app name on the current status,
    /// drop it if it is identical to the last one reported, then hand it to the terminal.
    ///
    /// "Hand it to the terminal" is where cyrup differs in mechanism only: the status is parked in
    /// [`Self::pending`] and the run loop writes it (see the module docs).
    pub fn report(&mut self, session_name: Option<&str>) {
        let status = ProgramStatus {
            app: Some(crate::resume_hint::APP_NAME.to_string()),
            ..self.current_status(session_name)
        };
        if self.last_report.as_ref() == Some(&status) {
            return;
        }
        self.last_report = Some(status.clone());
        self.pending = Some(status);
    }

    /// The report the run loop owes the terminal, if any.
    pub fn take_pending(&mut self) -> Option<ProgramStatus> {
        self.pending.take()
    }

    /// `currentStatus()` — `program-status-reporter.ts:103-110`. Precedence, highest first:
    ///
    /// 1. the most-recently-opened blocked dialog (`[...blocked.values()].at(-1)`);
    /// 2. `compacting` ⇒ `working` with the literal message `Compacting context`;
    /// 3. `runActive` ⇒ `working`, else the resting status.
    ///
    /// …and for `working` or `done` ONLY, the message becomes the session name — so `idle` and
    /// `error` carry no name (and `error` keeps the error's first line it was built with).
    fn current_status(&self, session_name: Option<&str>) -> ProgramStatus {
        if let Some((_, blocked)) = self.blocked.last() {
            return ProgramStatus {
                state: ProgramState::Blocked,
                app: None,
                kind: Some(blocked.kind),
                message: Some(blocked.message.clone()),
            };
        }
        if self.compacting {
            return ProgramStatus::with_message(ProgramState::Working, "Compacting context");
        }
        let status = if self.run_active {
            ProgramStatus::new(ProgramState::Working)
        } else {
            self.resting_status()
        };
        if matches!(status.state, ProgramState::Working | ProgramState::Done) {
            return ProgramStatus {
                message: session_name.map(str::to_string),
                ..status
            };
        }
        status
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;
    use cyrup_agent::AgentMessage;
    use cyrup_core::{ApiId, AssistantMessage, Content, ProviderId, StopReason};
    use cyrup_session_svc::{AgentSessionEvent as E, CompactionReason};

    /// Upstream's `setup(sessionName)` (`program-status-reporter.test.ts:7-23`): a reporter plus the
    /// log of everything it reported. cyrup's reporter parks its report in `pending` instead of
    /// calling `terminal.setProgramStatus`, so draining that is the log.
    struct Harness {
        reporter: ProgramStatusReporter,
        reports: Vec<ProgramStatus>,
        name: Option<String>,
    }

    impl Harness {
        fn new(name: Option<&str>) -> Self {
            Self {
                reporter: ProgramStatusReporter::new(),
                reports: Vec::new(),
                name: name.map(str::to_string),
            }
        }

        fn drain(&mut self) {
            if let Some(status) = self.reporter.take_pending() {
                self.reports.push(status);
            }
        }

        fn send(&mut self, events: &[E]) {
            for ev in events {
                self.reporter.handle_event(ev, self.name.as_deref());
                self.drain();
            }
        }

        fn report(&mut self) {
            let name = self.name.clone();
            self.reporter.report(name.as_deref());
            self.drain();
        }

        fn set_blocked(&mut self, source: &str, status: Option<BlockedStatus>) {
            let name = self.name.clone();
            self.reporter.set_blocked(source, status, name.as_deref());
            self.drain();
        }

        fn reset(&mut self) {
            let name = self.name.clone();
            self.reporter.reset(name.as_deref());
            self.drain();
        }

        /// Upstream's `last()` — the newest report with `app` stripped, since every report carries
        /// it and comparing it in every case would just repeat the constant.
        fn last(&self) -> ProgramStatus {
            let mut status = self.reports.last().cloned().expect("a report");
            assert_eq!(
                status.app.as_deref(),
                Some(crate::resume_hint::APP_NAME),
                "every report carries the app name (program-status-reporter.ts:96)"
            );
            status.app = None;
            status
        }
    }

    /// Upstream's `assistantEnd(stopReason, errorMessage)`
    /// (`program-status-reporter.test.ts:25-33`) — note the content: `"secret assistant output"`,
    /// there so the privacy assertion has something to find.
    fn assistant_end(stop_reason: StopReason, error_message: Option<&str>) -> E {
        let mut msg = AssistantMessage::errored(
            ProviderId::from("anthropic"),
            "claude-opus-4",
            Some(ApiId::from("anthropic-messages")),
            stop_reason,
            String::new(),
        );
        msg.error_message = error_message.map(str::to_string);
        msg.content = vec![Content::Text {
            text: "secret assistant output".into(),
            text_signature: None,
        }];
        E::MessageEnd {
            message: AgentMessage::Assistant(msg.into()),
        }
    }

    const fn settled() -> E {
        E::AgentSettled { aborted: false }
    }

    const fn aborted_settle() -> E {
        E::AgentSettled { aborted: true }
    }

    /// Upstream's `compactionEnd(reason, options)` (`:37-47`).
    fn compaction_end(reason: CompactionReason, aborted: bool, error_message: Option<&str>) -> E {
        E::CompactionEnd {
            reason,
            result: None,
            aborted,
            will_retry: false,
            error_message: error_message.map(str::to_string),
        }
    }

    fn working(message: Option<&str>) -> ProgramStatus {
        ProgramStatus {
            state: ProgramState::Working,
            app: None,
            kind: None,
            message: message.map(str::to_string),
        }
    }

    fn done(message: Option<&str>) -> ProgramStatus {
        ProgramStatus {
            state: ProgramState::Done,
            app: None,
            kind: None,
            message: message.map(str::to_string),
        }
    }

    /// Upstream's first case (`program-status-reporter.test.ts:51-65`), privacy assertion included.
    #[test]
    fn a_run_reports_working_then_done_and_never_leaks_output() {
        let mut h = Harness::new(Some("Fix login"));
        h.report();
        assert_eq!(h.last(), ProgramStatus::new(ProgramState::Idle));

        h.send(&[E::AgentStart]);
        assert_eq!(h.last(), working(Some("Fix login")));

        h.send(&[
            assistant_end(StopReason::ToolUse, None),
            assistant_end(StopReason::Stop, None),
        ]);
        assert_eq!(
            h.last(),
            working(Some("Fix login")),
            "a finished assistant message does not end the run"
        );

        h.send(&[settled()]);
        assert_eq!(h.last(), done(Some("Fix login")));

        // PRIVACY: prompts and assistant output are never reported — only the session name, dialog
        // titles and the first line of errors (`program-status-reporter.ts:11-16`).
        let log = format!("{:?}", h.reports);
        assert!(
            !log.contains("secret assistant output"),
            "assistant output reached a report:\n{log}"
        );
    }

    /// Upstream's second case (`:67-81`), all four sub-cases.
    #[test]
    fn only_the_outcome_of_the_run_is_reported() {
        let mut h = Harness::new(None);

        // A retried error is replaced by its successful retry.
        h.send(&[
            E::AgentStart,
            assistant_end(StopReason::Error, Some("overloaded")),
            assistant_end(StopReason::Stop, None),
            settled(),
        ]);
        assert_eq!(h.last(), done(None));

        // A final error reports its FIRST LINE only.
        h.send(&[
            E::AgentStart,
            assistant_end(StopReason::Error, Some("Invalid API key\n{details}")),
            settled(),
        ]);
        assert_eq!(
            h.last(),
            ProgramStatus::with_message(ProgramState::Error, "Invalid API key")
        );

        // An aborted settle is `idle`, whatever the run produced…
        h.send(&[
            E::AgentStart,
            assistant_end(StopReason::Aborted, None),
            aborted_settle(),
        ]);
        assert_eq!(h.last(), ProgramStatus::new(ProgramState::Idle));

        // …including after a SUCCESSFUL response (an `agent_before_settle` hook, or a retry delay).
        h.send(&[
            E::AgentStart,
            assistant_end(StopReason::Stop, None),
            aborted_settle(),
        ]);
        assert_eq!(h.last(), ProgramStatus::new(ProgramState::Idle));
    }

    /// Upstream's third case (`:83-102`), both halves.
    #[test]
    fn a_failed_recovery_compaction_is_the_runs_error_unless_a_later_response_succeeds() {
        let mut h = Harness::new(None);
        h.send(&[
            E::AgentStart,
            assistant_end(StopReason::Length, None),
            E::CompactionStart {
                reason: CompactionReason::Overflow,
            },
            compaction_end(
                CompactionReason::Overflow,
                false,
                Some("Compaction failed\nstack"),
            ),
            settled(),
        ]);
        assert_eq!(
            h.last(),
            ProgramStatus::with_message(ProgramState::Error, "Compaction failed")
        );

        h.send(&[
            E::AgentStart,
            E::CompactionStart {
                reason: CompactionReason::Threshold,
            },
            compaction_end(
                CompactionReason::Threshold,
                false,
                Some("Compaction failed"),
            ),
            assistant_end(StopReason::Stop, None),
            settled(),
        ]);
        assert_eq!(
            h.last(),
            done(None),
            "a later successful response replaces the compaction's error"
        );
    }

    /// Upstream's fourth case (`:104-118`).
    #[test]
    fn compaction_inside_a_run_and_the_result_of_a_manual_compaction() {
        let mut h = Harness::new(Some("Session"));
        h.send(&[
            E::AgentStart,
            E::CompactionStart {
                reason: CompactionReason::Threshold,
            },
        ]);
        assert_eq!(
            h.last(),
            working(Some("Compacting context")),
            "compaction outranks the run and names itself, not the session"
        );
        h.send(&[compaction_end(CompactionReason::Threshold, false, None)]);
        assert_eq!(h.last(), working(Some("Session")));
        h.send(&[assistant_end(StopReason::Stop, None), settled()]);

        // Outside a run, only a MANUAL compaction sets the resting status.
        h.send(&[
            E::CompactionStart {
                reason: CompactionReason::Manual,
            },
            compaction_end(CompactionReason::Manual, false, None),
        ]);
        assert_eq!(h.last(), done(Some("Session")));
        h.send(&[
            E::CompactionStart {
                reason: CompactionReason::Manual,
            },
            compaction_end(CompactionReason::Manual, false, Some("No model")),
        ]);
        assert_eq!(
            h.last(),
            ProgramStatus::with_message(ProgramState::Error, "No model")
        );
        h.send(&[
            E::CompactionStart {
                reason: CompactionReason::Manual,
            },
            compaction_end(CompactionReason::Manual, true, None),
        ]);
        assert_eq!(h.last(), ProgramStatus::new(ProgramState::Idle));
    }

    /// Upstream's fifth case (`:120-137`): precedence, non-stacking, and the run settling
    /// underneath an open dialog.
    #[test]
    fn the_most_recent_open_dialog_wins_and_the_underlying_state_returns() {
        let mut h = Harness::new(None);
        h.send(&[E::AgentStart]);
        h.set_blocked(
            "extension-selector",
            Some(BlockedStatus {
                kind: BlockedKind::Permission,
                message: "Allow bash?".to_string(),
            }),
        );
        h.set_blocked(
            "login",
            Some(BlockedStatus {
                kind: BlockedKind::Auth,
                message: "Log in to Anthropic".to_string(),
            }),
        );
        assert_eq!(
            h.last(),
            ProgramStatus {
                state: ProgramState::Blocked,
                app: None,
                kind: Some(BlockedKind::Auth),
                message: Some("Log in to Anthropic".to_string()),
            },
            "the most recently opened dialog is reported"
        );

        // The run settles while the selector is still open: the dialog still outranks it.
        h.set_blocked("login", None);
        h.send(&[assistant_end(StopReason::Stop, None), settled()]);
        assert_eq!(
            h.last(),
            ProgramStatus {
                state: ProgramState::Blocked,
                app: None,
                kind: Some(BlockedKind::Permission),
                message: Some("Allow bash?".to_string()),
            }
        );

        // Reopening a source REPLACES its dialog and moves it to the end rather than stacking.
        h.set_blocked(
            "extension-selector",
            Some(BlockedStatus {
                kind: BlockedKind::Question,
                message: "Pick one".to_string(),
            }),
        );
        assert_eq!(
            h.last(),
            ProgramStatus {
                state: ProgramState::Blocked,
                app: None,
                kind: Some(BlockedKind::Question),
                message: Some("Pick one".to_string()),
            }
        );
        h.set_blocked("extension-selector", None);
        assert_eq!(
            h.last(),
            done(None),
            "with nothing blocked the settled run surfaces"
        );
    }

    /// Upstream's sixth case (`:139-148`): dedup on the whole status, and the session-name refresh
    /// that `session_info_changed` exists to produce.
    #[test]
    fn each_status_is_sent_once_and_the_session_name_is_followed() {
        let mut h = Harness::new(Some("Old"));
        h.send(&[
            E::AgentStart,
            E::TurnStart,
            assistant_end(StopReason::ToolUse, None),
        ]);
        h.report();
        assert_eq!(
            h.reports.len(),
            1,
            "`turn_start` reports nothing, and three identical statuses are one write \
             (program-status-reporter.ts:97-99)"
        );

        h.name = Some("New".to_string());
        h.send(&[E::SessionInfoChanged {
            name: Some("New".to_string()),
        }]);
        assert_eq!(h.last(), working(Some("New")));
    }

    /// Upstream's seventh case (`:150-155`): `reset()` after a completed run reports `idle`.
    #[test]
    fn a_session_rebind_returns_to_idle() {
        let mut h = Harness::new(None);
        h.send(&[
            E::AgentStart,
            assistant_end(StopReason::Stop, None),
            settled(),
        ]);
        assert_eq!(h.last(), done(None));
        h.reset();
        assert_eq!(h.last(), ProgramStatus::new(ProgramState::Idle));
    }
}
