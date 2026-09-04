use super::*;

use crate::startup::armin_art;

impl<B: Backend> App<B> {
    /// Classify a submitted line via the [`CommandRegistry`] and route it (spec/tui/04 §2.3).
    ///
    /// A plain prompt is echoed into the transcript and returned as [`AppAction::Submit`] for the run
    /// loop to deliver to the runtime. A recognized slash command or a `!`/`!!` bash invocation is
    /// surfaced as a status line for now — opening the bound overlay / executing bash is wired as the
    /// selector + bash-execution subsystems land (tracked on the residual ledger). This keeps the
    /// editor → dispatch path real and faithful (commands never reach the agent as literal text).
    pub(crate) fn dispatch_submission(&mut self, text: &str) -> AppAction {
        let dispatch = self.state.commands.dispatch(text);
        // The startup hint bar is a first-run affordance; any real submission dismisses it
        // (Pi drops `compactInstructions` once the conversation begins).
        if !matches!(dispatch, Dispatch::Empty) {
            self.state.show_startup_hints = false;
        }
        match dispatch {
            Dispatch::Empty => AppAction::Redraw,
            // TUI-016 / TUI-052 — **no transcript echo here.** Pi's submit handler clears the editor
            // and calls `updatePendingMessagesDisplay()` (`interactive-mode.ts:2827-2833`); it never
            // writes the text into the chat container. The bubble is written when the session emits
            // `message_start` for the user message (`:2915-2918`), which for a message the session
            // QUEUES does not happen until the turn that carries it starts.
            //
            // cyrup used to `push_user` unconditionally right here, so a queued message was rendered
            // as a delivered one — and a message dequeued by Escape stayed in the transcript forever
            // as a phantom user turn that was never sent and is not in the session JSONL (TUI-052).
            Dispatch::Prompt(prompt) => AppAction::Submit(prompt),
            Dispatch::Command { name, arg } => self.run_command(&name, arg),
            Dispatch::Bash { command, excluded } => {
                // Open the live bash block (`bash-execution.ts`) and hand the spawn to the run loop.
                // Both labels are `keyText`-shaped upstream — `Running... (${keyText(…)} to cancel)`
                // (`bash-execution.ts:59`) and `keyHint("app.tools.expand", …)` (`:180`, `:184`) —
                // so they join every bound key (`keybinding-hints.ts:29-36`).
                let cancel = self.state.keymap.keys_label(Action::Interrupt);
                let expand = self.state.keymap.keys_label(Action::ToolsExpand);
                self.state
                    .transcript
                    .start_bash(command.clone(), excluded, cancel, expand);
                AppAction::RunBash { command, excluded }
            }
        }
    }

    /// Route a recognized slash command (`setupEditorSubmitHandler`, interactive-mode.ts:2554-2734).
    /// In-crate effects (info blocks, quit, easter eggs) are applied here directly and return
    /// [`AppAction::Redraw`]; session/data-bound effects return [`AppAction::Command`] for the run
    /// loop to execute against the [`AgentSession`]. Note `/theme`, `/think`, `/show-images` are NOT
    /// commands — Pi has no such builtins, so they fall through [`CommandRegistry::dispatch`] to
    /// [`Dispatch::Prompt`] and reach the agent as literal text (theme is reached via `/settings` →
    /// Theme, thinking via Shift+Tab; see [`Action::ThinkingCycle`]).
    pub(crate) fn run_command(&mut self, name: &str, arg: Option<String>) -> AppAction {
        use AppCommand as C;
        let cmd = |c| AppAction::Command(c);
        match name {
            // --- data-bound selectors (run loop sources rows) ---
            // `/model [text]` threads its argument (`handleModelCommand(searchTerm?)`,
            // interactive-mode.ts:4175): exact match → set directly; partial → pre-filtered picker.
            "model" => cmd(C::ModelCommand(arg)),
            // `/thinking [level]` (Pi `handleThinkingCommand`, `interactive-mode.ts:4771`),
            // dispatched with pi's `text === "/thinking" || startsWith("/thinking ")` guard
            // (`:2996`) — hence its entry in `ARGUMENT_DISPATCH_NAMES`. No argument opens the
            // picker; an argument applies SESSION-ONLY (`selectThinkingLevel(level, false)`,
            // `:4786`), which is why this is not the persisting command.
            "thinking" => cmd(C::ThinkingCommand(arg)),
            "settings" => cmd(C::OpenSelector(SelectorKind::Settings)),
            "scoped-models" => cmd(C::OpenSelector(SelectorKind::ScopedModels)),
            "tree" => cmd(C::OpenSelector(SelectorKind::Tree)),
            "resume" => cmd(C::OpenSelector(SelectorKind::Session)),
            "trust" => cmd(C::OpenSelector(SelectorKind::Trust)),
            "fork" => cmd(C::OpenSelector(SelectorKind::UserMessage)),
            // `/login [provider]` threads its argument the same way `/model` does
            // (`handleLoginCommand(providerRef?)`, interactive-mode.ts:2810).
            "login" => cmd(C::LoginCommand(arg)),
            "logout" => cmd(C::OpenSelector(SelectorKind::Logout)),
            // --- session lifecycle / IO (run loop) ---
            "new" => cmd(C::NewSession),
            "compact" => cmd(C::Compact(arg)),
            "clone" => cmd(C::Clone),
            "reload" => cmd(C::Reload),
            // TUI-079 — `/export` and `/import` take ONE quote-aware token, not the whole
            // remainder: pi runs both through `getPathCommandArgument`
            // (`interactive-mode.ts:5435`, `:5480` @v0.83.0). Without it
            // `/export "my session.html"` wrote a file whose name contained the quote characters,
            // `/export a.html junk` wrote to the path `a.html junk`, and an unterminated quote —
            // which upstream REFUSES — was accepted as a path. A refusal arrives here as `None`,
            // which is already each arm's no-argument branch: usage for `/import`, and the
            // session-directory default for `/export`, exactly as upstream's `undefined` does.
            "export" => cmd(C::Export(
                arg.as_deref()
                    .and_then(crate::commands::path_command_argument),
            )),
            "import" => cmd(C::Import(
                arg.as_deref()
                    .and_then(crate::commands::path_command_argument),
            )),
            "share" => cmd(C::Share),
            "copy" => cmd(C::Copy),
            // TUI-080 — `/name` with no argument is a GETTER upstream, not a usage error. This arm
            // used to print `usage: /name <session name>` unconditionally, so a user who typed
            // `/name` to CHECK the session's name was told they had used the command wrong, and the
            // only way to read the name was `/session`.
            "name" => match arg {
                Some(n) => cmd(C::SetName(n)),
                None => cmd(C::ShowName),
            },
            "session" => cmd(C::SessionInfo),
            // --- in-crate info blocks ---
            "hotkeys" => {
                // `handleHotkeysCommand` (interactive-mode.ts:6197-6203) appends to the TRANSCRIPT —
                // `chatContainer.addChild(Spacer(1) / DynamicBorder / Text(bold accent title,1,0) /
                // Spacer(1) / Markdown(body,1,1) / DynamicBorder)`. That is byte-for-byte the same
                // component stack `/changelog` builds at :6067-6072, i.e. [`Entry::Block`]; there is no
                // floating overlay anywhere in the command (`git grep showOverlay v0.84.1` finds it
                // only in `tui.ts` and the extension-UI path at :2719). The help therefore SCROLLS
                // WITH the conversation and stays in scrollback instead of being a modal that
                // captures keys and vanishes on Esc.
                let body = self.hotkeys_markdown();
                self.state.transcript.push_block("Keyboard Shortcuts", body);
                AppAction::Redraw
            }
            "changelog" => {
                self.state
                    .transcript
                    .push_block("What's New", "No changelog entries found.");
                AppAction::Redraw
            }
            "debug" => {
                let body = self.debug_markdown();
                self.state.transcript.push_block("Debug", body);
                AppAction::Redraw
            }
            "quit" => {
                self.state.should_quit = true;
                AppAction::Quit
            }
            // `/arminsayshi` (`armin.ts` `ArminComponent`): the 31×36 XBM bitmap rendered with
            // half-block glyphs (the random CRT/glitch animation effects are non-deterministic chrome
            // and omitted; the art itself is a real rich render, not a status line).
            "arminsayshi" => {
                self.state
                    .transcript
                    .push_block("Armin says hi!", armin_art());
                AppAction::Redraw
            }
            // `/dementedelves` (`daxnuts.ts`): a themed banner block (the model-triggered animation is
            // chrome; the announcement is a real rich block).
            "dementedelves" => {
                self.state.transcript.push_block(
                    "Demented Elves",
                    "🧝 The demented elves have entered the chat.",
                );
                AppAction::Redraw
            }
            // Any other unhandled recognized name: a status line.
            other => {
                self.state
                    .transcript
                    .push_status(format!("command: /{other}"));
                AppAction::Redraw
            }
        }
    }

    /// Build the `/debug` info block (`handleDebugCommand`, interactive-mode.ts:5526): terminal size,
    /// active theme + generation, thinking level, and selector/stream state.
    fn debug_markdown(&self) -> String {
        let size = self.terminal.backend().size().ok();
        let (w, h) = size.map(|s| (s.width, s.height)).unwrap_or((0, 0));
        format!(
            "| Field | Value |\n|-------|-------|\n\
             | terminal | {w}×{h} |\n\
             | theme | {} (gen {}) |\n\
             | thinking | {} |\n\
             | show images | {} |\n\
             | streaming | {} |\n",
            self.state.theme.name,
            self.state.theme.generation,
            self.state.thinking_level,
            self.state.show_images,
            self.state.status.streaming,
        )
    }

    /// Rebuild the pending-messages region from BOTH queue sources — Pi's `getAllQueuedMessages`
    /// (`interactive-mode.ts:3942-3953` @v0.83.0), which concatenates the session's steering /
    /// follow-up lists with the `compactionQueuedMessages` of the matching mode, in that order.
    /// TUI-031.
    pub(crate) fn rebuild_pending_messages(&mut self) {
        let mut steering = self.state.session_queue.0.clone();
        steering.extend(
            self.state
                .compaction_queue
                .iter()
                .filter(|m| !m.follow_up)
                .map(|m| m.text.clone()),
        );
        let mut follow_up = self.state.session_queue.1.clone();
        follow_up.extend(
            self.state
                .compaction_queue
                .iter()
                .filter(|m| m.follow_up)
                .map(|m| m.text.clone()),
        );
        self.state.pending_messages.set(steering, follow_up);
    }

    /// Pi's `queueCompactionMessage(text, mode)` (`interactive-mode.ts:4014-4020` @v0.83.0): push
    /// onto the compaction queue, refresh the pending-messages display, and show
    /// `Queued message for after compaction`. The editor was already cleared by the submit path, and
    /// history was already recorded, so only the queue + the two surfaces are left. TUI-031.
    pub(crate) fn queue_compaction_message(&mut self, text: String, follow_up: bool) {
        self.state
            .compaction_queue
            .push(CompactionQueued { text, follow_up });
        self.rebuild_pending_messages();
        self.state
            .transcript
            .push_status("Queued message for after compaction");
    }

    /// Take the whole compaction queue — Pi's `flushCompactionQueue` opens with
    /// `const queuedMessages = [...this.compactionQueuedMessages]; this.compactionQueuedMessages =
    /// []; this.updatePendingMessagesDisplay();` (`interactive-mode.ts:4038-4041`), and
    /// `clearAllQueues` (`:3959-3971`) drains the same list for the Escape restore. TUI-031.
    pub(crate) fn take_compaction_queue(&mut self) -> Vec<CompactionQueued> {
        let taken = std::mem::take(&mut self.state.compaction_queue);
        if !taken.is_empty() {
            self.rebuild_pending_messages();
        }
        taken
    }

    /// Pi's `setToolsExpanded(expanded)` (`interactive-mode.ts:4032-4048` @v0.84.1) — the single
    /// entry point BOTH `Ctrl+O` and an extension's `ui.setToolsExpanded` go through, so the two
    /// cannot drift (TUI-010 / TUI-038).
    ///
    /// Early-returns when the value is unchanged (`:4033`), then sets the one flag, fans it out to
    /// every expandable surface — the tool blocks and the live bash block are cyrup's two — and
    /// echoes `Tool output: expanded|collapsed`.
    pub(crate) fn set_tools_expanded(&mut self, expanded: bool) {
        if !self.state.transcript.set_tool_expanded(expanded) {
            return;
        }
        self.state.transcript.set_bash_expanded(expanded);
        self.state.transcript.push_status(format!(
            "Tool output: {}",
            if expanded { "expanded" } else { "collapsed" }
        ));
    }
}
