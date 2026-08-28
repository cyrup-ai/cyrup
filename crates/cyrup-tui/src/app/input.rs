use super::*;

impl<B: Backend> App<B> {
    /// Map one input event to an [`AppAction`], mutating editor/transcript as needed. Global keys
    /// (resolved via the configurable [`Keymap`], R-10-018) take precedence over editing keys.
    pub fn handle_input(&mut self, ev: &InputEvent) -> AppAction {
        match ev {
            InputEvent::Key(key) => {
                if matches!(key.kind, KeyEventKind::Release) {
                    return AppAction::None;
                }
                // TUI-S10 — the global debug chord, checked BEFORE any focus routing. Pi tests it
                // inside `handleTerminalInput` and ahead of the dispatch to the focused component:
                // `if (matchesKey(data, "shift+ctrl+d") && this.onDebug) { this.onDebug(); return; }`
                // (`packages/tui/src/tui.ts:850` @v0.83.0), wired at
                // `interactive-mode.ts:2803` `this.ui.onDebug = () => this.handleDebugCommand();`
                // with the comment "works regardless of focus". It is deliberately NOT a
                // configurable id — upstream hardcodes it — which is why it sits outside the
                // `Keymap` rather than in `Action::from_id`. Without it `/debug` was reachable only
                // by typing it into the editor, i.e. never while a selector, dialog or overlay had
                // focus, which is exactly when a diagnostic dump is wanted.
                if key.code == KeyCode::Char('d')
                    && key
                        .modifiers
                        .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                {
                    return self.run_command("debug", None);
                }
                // A floating overlay (hotkeys/help popup) captures input first (spec/tui/05 §2
                // routing step 2): the topmost overlay handles the key; `Close` pops it, an unhandled
                // key is swallowed so it never leaks to the editor/agent beneath the modal.
                if !self.state.overlays.is_empty() {
                    return self.handle_overlay_key(key);
                }
                // ADR-0005 §B-9 — the alternate screen's viewport bindings, and their deliberate
                // shadowing of the unmodified editor bindings in fullscreen mode
                // (`keybindings.ts:159`, which is a comment on a *behaviour*, not documentation).
                //
                // Upstream implements the rule by POSITION, not by a flag: `TuiAltScreen` registers
                // `handleViewportInput` as an input listener (`tui-alt-screen.ts:227`), and the
                // listener loop in `handleTerminalInput` runs to completion before the focused
                // component is ever offered the key (`tui.ts:834-848` against `tui.ts:892-897`). A
                // listener that answers `{consume: true}` ends the dispatch; one that answers
                // `undefined` lets the key through unchanged. `false` from `handle_key` IS that
                // `undefined`, and the fall-through below — selector, global `Keymap`, extension
                // shortcuts, editor — is what keeps every one of them alive in fullscreen.
                //
                // Placed AFTER the overlay block and BEFORE the selector block for the same reason
                // upstream tests `shouldDeferViewportInputToOverlay()` immediately ahead of its page
                // arms (`:538-540`, `:599`): a focused overlay keeps its own `pageUp`, but the
                // editor — which is what cyrup's selector slot swaps in place, and what upstream
                // calls the focused component — does not.
                //
                // The rule's second half is resolution-time and needs nothing here: the table is
                // consulted through `AltScreenKeymap::action_in_mode`, which answers `None` for
                // every event under `TuiRenderMode::Regular` — so an inline `pageUp` reaches
                // `tui.editor.pageUp` and `app.pageUp` exactly as it did before ADR-0005, and would
                // still do so even if this block were reached with no alternate screen live.
                if self.altscreen.is_some() {
                    let App { altscreen, state, alt_keymap, .. } = self;
                    // `document()` is passed rather than held: §B-10's prompt walk is its only
                    // reader, and the renderer owns no copy of the transcript (`altscreen/mod.rs`,
                    // rule 2).
                    if let Some(alt) = altscreen.as_mut()
                        && alt.handle_key(key, alt_keymap, state.transcript.document())
                    {
                        return AppAction::Redraw;
                    }
                }
                // A focused selector captures input first (spec/tui/05 §2 routing step 2): its
                // navigation/confirm/cancel keys are handled before the global keymap, so `Esc`/`Ctrl+C`
                // dismiss the selector rather than interrupting the agent. Unbound keys fall through.
                if self.state.selector.is_some() {
                    return self.handle_selector_key(key);
                }
                // Routing chain: overlay > completion > editor > app (spec/tui/07 §2). A global key is
                // resolved here, but two context guards defer it to the editor so the chain holds
                // (audit #4 — the previous unconditional global resolution made Ctrl+D quit and Esc
                // abort an open popup):
                //   • Esc with the completion popup open dismisses the popup, never aborts the run
                //     (spec/tui/04 §5, spec/tui/07 §7).
                //   • Ctrl+D on a non-empty buffer is forward-delete; it only exits on empty
                //     (spec/tui/03 §6, spec/tui/07 §3.3).
                if let Some(action) = self.state.keymap.action_for(key) {
                    // `app.clipboard.pasteImage` (Ctrl+V): pi `handleClipboardPaste`
                    // (`interactive-mode.ts:2870-2892` @v0.84.2) reads an IMAGE first — inserting
                    // its temp-file PATH as text at the editor cursor — and, when there is none,
                    // reads TEXT and inserts that (DRIFT-045; the text half used to be missing, so
                    // Ctrl+V over a text clipboard did nothing). When the clipboard holds neither,
                    // the key is NOT swallowed: it falls through to the editor below, so a terminal
                    // that maps Ctrl+V to a bracketed paste still works.
                    if action == Action::ClipboardPasteImage {
                        if self.try_paste_clipboard_image_path() {
                            return AppAction::Redraw;
                        }
                        // Nothing on the clipboard: fall through to the editor handling below.
                    } else {
                        let defer_to_editor = match action {
                            Action::Interrupt => self.state.editor.autocomplete_open(),
                            Action::Quit => !self.state.editor.is_empty(),
                            // `PageUp`/`PageDown` are EDITOR bindings upstream and only editor
                            // bindings — pi defines no `app.pageUp`/`app.pageDown` at v0.83.0 or
                            // v0.84.1, and `tui.editor.pageUp` (`tui/src/keybindings.ts:89-90`)
                            // pages the CARET (`editor.ts:855-862` → `pageScroll`). cyrup resolved
                            // them globally and always scrolled the transcript, so the key never
                            // reached a focused multi-line editor at all. Defer to the editor
                            // whenever the buffer spans more than one visual line — i.e. whenever
                            // there is something in it to page — and otherwise fall through to
                            // cyrup's active-region transcript scroll, which has no pi analogue
                            // (pi pages committed history with the terminal's own scrollback).
                            Action::PageUp | Action::PageDown => {
                                self.state.editor.is_multi_visual_line()
                            }
                            _ => false,
                        };
                        if !defer_to_editor {
                            return self.apply_action(action);
                        }
                    }
                }
                // An extension-registered keyboard shortcut (R-08-017; Pi `registerShortcut`) fires at
                // the global-keymap tier — after the built-in bindings (so an extension can't shadow
                // `Ctrl+D`/`Esc`) but before the editor (so the key never leaks in as text). The run
                // loop dispatches the matched key-id to the session's extension host.
                if let Some((_, spec)) =
                    self.state.extension_shortcuts.iter().find(|(k, _)| k.matches(key))
                {
                    return AppAction::ExtensionShortcut(spec.id.clone());
                }
                match self.state.editor.handle_key(key) {
                    EditorOutcome::Submit(text) => self.dispatch_submission(&text),
                    EditorOutcome::Edited => AppAction::Redraw,
                    EditorOutcome::Ignored => AppAction::None,
                }
            }
            InputEvent::Paste(s) => {
                // A selector owns the slot: offer the paste to its embedded `Input` first — pi's
                // `Input.handleInput` bracketed-paste branch (`input.ts:54-84`) → `handlePaste`
                // (`:362-372`). A pure-list selector owns no input and answers `Ignored`, which
                // `handle_selector_paste` maps back to `AppAction::None`.
                if self.state.selector.is_some() {
                    return self.handle_selector_paste(s);
                }
                // Route bracketed paste through `handle_paste` so large pastes collapse to an atomic
                // `[paste #N …]` marker (spec/tui/03 §5.5); small pastes insert verbatim.
                self.state.editor.handle_paste(s);
                AppAction::Redraw
            }
            InputEvent::Resize(_, _) => AppAction::Redraw,
            // ADR-0005 §B-6/§B-7/§B-8 — the alternate screen's pointer surface. Reports only reach
            // here while it is live (`altscreen::mouse::map_reader_event`'s gate), so the `None`
            // arm is every regular-mode session and costs one `Option` test.
            //
            // Capturing the mouse takes the terminal's own selection away from the user, which is
            // why a renderer that captures it owes them a replacement — the outcomes below ARE
            // that replacement.
            InputEvent::Mouse(m) => {
                let area = match self.terminal.size() {
                    Ok(s) => ratatui::layout::Rect { x: 0, y: 0, width: s.width, height: s.height },
                    Err(_) => return AppAction::None,
                };
                let Some(alt) = self.altscreen.as_mut() else {
                    return AppAction::None;
                };
                match alt.handle_mouse(m, area) {
                    crate::altscreen::PointerOutcome::Ignored => AppAction::None,
                    crate::altscreen::PointerOutcome::Handled => AppAction::Redraw,
                    // The write is async, so it rides the action out to the run loop.
                    crate::altscreen::PointerOutcome::Copy(text) => AppAction::CopySelection(text),
                    // pi's `onRightClickPaste()` (`tui-alt-screen.ts:711`) inserts into the editor,
                    // through the same bracketed-paste path a keyboard paste takes so a large
                    // right-click paste collapses to the `[paste #N …]` marker identically.
                    crate::altscreen::PointerOutcome::Paste(text) => {
                        self.state.editor.handle_paste(&text);
                        AppAction::Redraw
                    }
                }
            }
            InputEvent::FocusGained => {
                self.state.editor.set_focused(true);
                AppAction::Redraw
            }
            InputEvent::FocusLost => {
                // ADR-0005 §B-4/§B-8 — the reason the alternate screen asks the terminal for
                // `?1004h` at all: `FOCUS_OUT` cancels a live scrollbar grab and an IN-FLIGHT
                // selection (`tui-alt-screen.ts:543-559`). Without it a drag that leaves the window
                // never ends, because the release is delivered to whatever took the focus. A
                // completed selection deliberately survives — `selection::focus_lost` clears only
                // the in-flight state, which is a different clear from `selection::cancel`.
                //
                // A no-op inline, where neither a thumb grab nor a selection exists.
                if let Some(alt) = self.altscreen.as_mut() {
                    alt.handle_focus_lost();
                }
                self.state.editor.set_focused(false);
                AppAction::Redraw
            }
        }
    }

    /// Resolve a global keymap action (R-10-024 Ctrl+C, R-10-030 abort).
    fn apply_action(&mut self, action: Action) -> AppAction {
        match action {
            Action::Quit => {
                self.state.should_quit = true;
                AppAction::Quit
            }
            Action::Interrupt => {
                // Pi REBINDS `defaultEditor.onEscape` to `() => this.session.abortBranchSummary()`
                // for the duration of a `/tree` branch summarization (`interactive-mode.ts:4792-4795`,
                // restored in the `finally` at `:4832`), so Escape cancels the summarization and
                // nothing else — no stream teardown, no bash kill. Checked FIRST for the same reason
                // Pi's rebind shadows the default handler.
                if self.state.branch_summary_in_flight {
                    return AppAction::AbortBranchSummary;
                }
                // A compaction rebinds Escape the same way a branch summarization does — Pi's
                // `case "compaction_start"` sets `this.defaultEditor.onEscape = () => {
                // this.session.abortCompaction(); }` (`interactive-mode.ts:3080-3086` @v0.83.0) and
                // `compaction_end` restores it (`:3094-3097`). Checked here for the same reason:
                // the rebind SHADOWS the default chain for the whole window.
                if self.state.compacting {
                    return AppAction::AbortCompaction;
                }
                // TUI-005 / TUI-009 — Pi's `defaultEditor.onEscape` is a chain of **four mutually
                // exclusive** `else if` branches (`interactive-mode.ts:2569-2595` @v0.83.0):
                //
                //   if      (isStreaming)   restoreQueuedMessagesToEditor({ abort: true })
                //   else if (isBashRunning) abortBash()
                //   else if (isBashMode)    editor.setText(""); isBashMode = false
                //   else if (editor empty)  the 500 ms double-Escape window
                //
                // cyrup ran the bash-child cancel as a plain `if` ahead of the streaming read, so an
                // Escape during a turn that also had a `!`-child killed the child as collateral —
                // upstream never touches a bash child while streaming, precisely because the arms
                // are exclusive. The third and fourth branches did not exist at all.
                //
                // 1. Streaming: restore the queued steering/follow-up text to the editor and THEN
                //    abort, so nothing typed during the run is lost.
                if self.state.status.streaming {
                    self.state.transcript.discard_streaming();
                    self.state.transcript.commit_tools();
                    self.state.status.set_streaming(false);
                    self.state.indicator.idle();
                    return AppAction::InterruptRestoreQueued;
                }
                // 2. A running `!`/`!!` bash block — cancel it (the run loop kills the child).
                if self.state.transcript.bash_running() {
                    self.state.transcript.bash_complete_simple(None, true);
                    self.state.transcript.commit_bash();
                    self.state.transcript.discard_streaming();
                    self.state.transcript.commit_tools();
                    self.state.indicator.idle();
                    return AppAction::Interrupt;
                }
                // 3. Bash MODE — a typed-but-unsent `!cmd` in the editor. Pi clears the buffer and
                //    leaves bash mode (`:2575-2578`); cyrup derives the mode from the buffer the way
                //    Pi's `onChange` does (`:2621-2622`, `text.trimStart().startsWith("!")`), so
                //    clearing the buffer *is* leaving the mode.
                if self.state.editor.text().trim_start().starts_with('!') {
                    self.state.editor.clear();
                    return AppAction::Redraw;
                }
                // 4. Empty editor — the double-Escape window (`:2579-2594`). `doubleEscapeAction`
                //    is a live, persisted `/settings` row that had no consumer at all; `tree` opens
                //    the session tree, `fork` opens the user-message selector (Pi's
                //    `showUserMessageSelector`), `none` does nothing. The window is 500 ms and a
                //    fire resets the stamp to zero so a third press starts a new pair.
                if self.state.editor.text().trim().is_empty() {
                    let action = self.state.double_escape_action.clone();
                    if action != "none" {
                        let now = std::time::Instant::now();
                        let within = self.state.last_escape.is_some_and(|t| {
                            now.duration_since(t) < std::time::Duration::from_millis(500)
                        });
                        if within {
                            self.state.last_escape = None;
                            let kind = if action == "tree" {
                                SelectorKind::Tree
                            } else {
                                SelectorKind::UserMessage
                            };
                            return AppAction::Command(AppCommand::OpenSelector(kind));
                        }
                        self.state.last_escape = Some(now);
                    }
                    return AppAction::Redraw;
                }
                // Nothing streaming, no bash, a non-`!` non-empty buffer: Pi's chain falls off the
                // end and does nothing.
                AppAction::Redraw
            }
            // `app.clear` (Ctrl+C, Pi `handleCtrlC` interactive-mode.ts:3797-3805): a second Ctrl+C
            // within 500 ms of the previous one EXITS — there is NO emptiness gate (Pi does not require
            // the editor to be empty; that is `Ctrl+D`'s rule, not `Ctrl+C`'s). Otherwise clear the
            // editor buffer and record the press time.
            Action::Clear => {
                let now = std::time::Instant::now();
                let double_tap = self
                    .state
                    .last_sigint
                    .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_millis(500));
                if double_tap {
                    self.state.should_quit = true;
                    AppAction::Quit
                } else {
                    self.state.editor.clear();
                    self.state.last_sigint = Some(now);
                    AppAction::Redraw
                }
            }
            // `app.tools.expand` (Ctrl+O) toggles tool-output expansion in-crate (`tool-execution.ts`
            // expand); the live tools re-render expanded/collapsed on the next frame.
            Action::ToolsExpand => {
                // TUI-038 / TUI-010 — this was an if/else: while ANY `!cmd` block was present the
                // tool-expansion flag could not be moved at all, and afterwards the two flags were
                // out of sync with each other and with what the user last asked for. Upstream is a
                // FAN-OUT: `setToolsExpanded` sets one `toolOutputExpanded` and then broadcasts it
                // to the active header and to every `isExpandable` child of
                // `loadedResourcesContainer` and `chatContainer`
                // (`interactive-mode.ts:4032-4048` @v0.84.1), of which the bash component is one
                // (`components/bash-execution.ts:29`, `setExpanded` at `:70`). It also ends in
                // `showStatus("Tool output: …")` (`:4047`) — the SAME line the extension path
                // already pushed, so the identical user-visible action produced a status when an
                // extension triggered it and none when a keystroke did.
                let expanded = !self.state.transcript.tool_expanded();
                self.set_tools_expanded(expanded);
                AppAction::Redraw
            }
            // `app.suspend` (Ctrl+Z) is surfaced to the run loop, which tears down raw mode, raises
            // SIGTSTP, and re-enters on SIGCONT (the raise lives in an isolated allow-unsafe shim).
            Action::Suspend => AppAction::Suspend,
            // `app.editor.external` (Ctrl+G): surfaced to the run loop, which restores the terminal,
            // launches `$VISUAL`/`$EDITOR` on the buffer, and reloads it.
            Action::ExternalEditor => AppAction::OpenExternalEditor,
            // Page scroll over the **active region** (spec/tui/07): committed history lives in the
            // terminal's native scrollback (paged with the terminal's own scroll, ADR-0001), but the
            // in-flight streaming/tool/bash output can exceed the viewport — `PageUp`/`PageDown` page
            // it without losing the live tail. The page size is one screenful, resolved at render time
            // against the live viewport; a fixed conservative page keeps this input-thread pure.
            Action::PageUp => {
                self.state.transcript.page_up(PAGE_SCROLL_LINES);
                AppAction::Redraw
            }
            Action::PageDown => {
                self.state.transcript.page_down(PAGE_SCROLL_LINES);
                AppAction::Redraw
            }
            // `app.thinking.cycle` (Shift+Tab): advance the reasoning level in place — no picker. The
            // cycle is GATED on the live model supporting thinking and walks the model's OWN supported
            // levels, so it rides an `AppCommand` the run loop resolves against the session (Pi
            // `cycleThinkingLevel` calls `session.cycleThinkingLevel()`, interactive-mode.ts:3606-3614;
            // agent-session.ts:1599). The footer + editor rule re-color off the emitted
            // `ThinkingLevelChanged` event, exactly as in Pi's event handler (interactive-mode.ts:2804).
            Action::ThinkingCycle => AppAction::Command(AppCommand::CycleThinking),
            // `app.model.cycleForward` / `cycleBackward` (Ctrl+P / Shift+Ctrl+P): the model swap needs
            // the live catalog + `set_model` at the session layer, so it rides an `AppCommand` the run
            // loop applies (Pi `cycleModel`, interactive-mode.ts:3617-3632).
            Action::ModelCycleForward => {
                AppAction::Command(AppCommand::CycleModel(CycleDirection::Forward))
            }
            Action::ModelCycleBackward => {
                AppAction::Command(AppCommand::CycleModel(CycleDirection::Backward))
            }
            // `app.message.followUp` (Alt+Enter): queue the editor text as a follow-up delivered after
            // the turn goes idle (Pi `handleFollowUp`, interactive-mode.ts:3554-3585). Empty input is a
            // no-op (Pi's `if (!text) return`); the streaming-vs-idle decision + delivery is async, so
            // it rides an `AppAction` the run loop resolves against the live session.
            Action::FollowUp => {
                let text = self.state.editor.text();
                if text.trim().is_empty() {
                    AppAction::Redraw
                } else {
                    AppAction::FollowUp(text)
                }
            }
            // `app.message.dequeue` (Alt+Up): restore queued messages to the editor. The queue read +
            // clear are on the live session, so it rides an `AppAction` the run loop resolves (Pi
            // `handleDequeue`, interactive-mode.ts:3587-3594).
            Action::Dequeue => AppAction::Dequeue,
            // `app.clipboard.pasteImage` (Ctrl+V) is resolved earlier in `handle_input` — it must be
            // able to fall through to the editor when the clipboard holds no image, which this arm
            // cannot do — so this arm is normally unreachable. It exists only to keep the match
            // exhaustive; it pastes the path best-effort and redraws (no panic, per the no-panic policy).
            Action::ClipboardPasteImage => {
                self.try_paste_clipboard_image_path();
                AppAction::Redraw
            }
            // TUI-008 — the seven ids `interactive-mode.ts:2608-2618` wires. Every destination
            // already existed and had no key routed to it; the ids were simply unrecognized, so a
            // `keybindings.json` naming them did nothing and the documented default chords were
            // dead keys.
            //
            // `app.model.select` (Ctrl+L): `showModelSelector()` (`:2608`) is the UNFILTERED picker
            // — the same thing a bare `/model` opens, which is `ModelCommand(None)`
            // (`handleModelCommand(undefined)`, `:4175`).
            Action::ModelSelect => AppAction::Command(AppCommand::ModelCommand(None)),
            // `app.thinking.toggle` (Ctrl+T): `toggleThinkingBlockVisibility` (`:3834-3850`) flips
            // `hideThinkingBlock`, PERSISTS it via `settingsManager.setHideThinkingBlock` (`:3836`)
            // and ends in `showStatus(\`Thinking blocks: ${… ? "hidden" : "visible"}\`)` (`:3849`).
            // The persist is what makes this different from a view-only flag, so it rides
            // `ApplySetting` — the same command the `/settings` row uses, whose handler also applies
            // the flip live to the transcript (one write path, not two).
            //
            // **[CYRUP-DELTA]** — pi additionally rebuilds the whole chat container from the session
            // messages (`:3838-3840`), so ALREADY-SHOWN assistant messages change form. cyrup's
            // committed rows have left the render tree for the terminal's native scrollback
            // (`flush_committed` → `insert_before`, ADR-0001), so they keep the form they committed
            // with; only the in-flight block and everything after the flip change. That residual is
            // `TUI-N06`, which owns it — it is not introduced here.
            Action::ThinkingToggle => {
                let hidden = !self.state.transcript.hide_thinking_block();
                // Pi flips its own field FIRST (`:3835`) and only then persists (`:3836`), so the
                // rebuild two lines later already sees the new value. Applying it here rather than
                // relying solely on the `ApplySetting` round-trip is not belt-and-braces: the
                // command is resolved by the run loop against the live session, so a press while
                // the settings write is unavailable (or simply before the loop turns) would
                // otherwise leave the view unchanged AND compute the same `hidden` again on the
                // next press — a key that toggles nothing, twice.
                self.state.transcript.set_hide_thinking_block(hidden);
                self.state
                    .transcript
                    .push_status(if hidden {
                        "Thinking blocks: hidden"
                    } else {
                        "Thinking blocks: visible"
                    });
                AppAction::Command(AppCommand::ApplySetting {
                    id: "hideThinkingBlock".to_string(),
                    value: hidden.to_string(),
                })
            }
            // `app.message.copy` (Ctrl+X): `void this.handleCopyCommand()` (`:2612`) — the identical
            // handler `/copy` runs, so it is the identical command.
            Action::MessageCopy => AppAction::Command(AppCommand::Copy),
            // `app.session.new/tree/fork/resume` (`:2615-2618`) → `handleClearCommand`,
            // `showTreeSelector`, `showUserMessageSelector`, `showSessionSelector` — i.e. exactly
            // what `/new`, `/tree`, `/fork` and `/resume` dispatch to in `run_command`. All four
            // ship with `defaultKeys: []` (`core/keybindings.ts:115-118`): reachable only from a
            // user's `keybindings.json`, which is precisely the case that used to be silent.
            Action::SessionNew => AppAction::Command(AppCommand::NewSession),
            Action::SessionTree => {
                AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree))
            }
            Action::SessionFork => {
                AppAction::Command(AppCommand::OpenSelector(SelectorKind::UserMessage))
            }
            Action::SessionResume => {
                AppAction::Command(AppCommand::OpenSelector(SelectorKind::Session))
            }
        }
    }

    /// Route one key to the topmost floating overlay (spec/tui/05 §2 step 2). `Close` pops it; any
    /// other outcome stays open and redraws. A no-op when the stack is empty.
    fn handle_overlay_key(&mut self, key: &event::KeyEvent) -> AppAction {
        let Some(top) = self.state.overlays.last_mut() else { return AppAction::None };
        match top.handle(key) {
            OverlayOutcome::Close => {
                self.state.overlays.pop();
                AppAction::Redraw
            }
            OverlayOutcome::Redraw | OverlayOutcome::Ignored => AppAction::Redraw,
        }
    }

    /// Whether a floating overlay is currently open (test/inspection access).
    pub fn overlay_open(&self) -> bool {
        !self.state.overlays.is_empty()
    }
}
