use super::*;

/// The decision produced by feeding one input event to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppAction {
    /// The user submitted a prompt (already optimistically shown in the transcript).
    Submit(String),
    /// The user queued the editor text as a follow-up (Alt+Enter, `app.message.followUp`): the run
    /// loop delivers it via [`AgentSession::follow_up`] when a turn is streaming, or as a plain submit
    /// when idle (Pi `handleFollowUp`, interactive-mode.ts:3554-3585). Carries the trimmed text.
    FollowUp(String),
    /// The user asked to restore queued messages back into the editor (Alt+Up, `app.message.dequeue`):
    /// the run loop reads the session's steering + follow-up queues, clears them, and prepends their
    /// text to the current editor buffer (Pi `handleDequeue` → `restoreQueuedMessagesToEditor`,
    /// interactive-mode.ts:3587-3594,3852-3871). Needs the live session (async queue read/clear), so
    /// like [`Self::FollowUp`] it rides an `AppAction` the run loop resolves.
    Dequeue,
    /// A fullscreen text selection was completed and must be written to the system clipboard —
    /// pi's `copySelectionToClipboard` tail (`tui-alt-screen.ts:1113-1117`), ADR-0005 §B-8.
    ///
    /// It rides an `AppAction` for the same reason [`Self::FollowUp`] and [`Self::Dequeue`] do:
    /// [`crate::clipboard::copy_to_clipboard`] is `async` and `App::handle_input` is not. The run
    /// loop performs the write and flashes `Copied!` or `Copy failed` on the result (§B-11).
    CopySelection(String),
    /// The user requested an abort/interrupt of the in-flight run (Esc).
    Interrupt,
    /// Esc pressed **while a turn is streaming** (Pi `defaultEditor.onEscape` first branch,
    /// interactive-mode.ts:2636-2637 → `restoreQueuedMessagesToEditor({abort: true})`).
    ///
    /// Distinct from [`Self::Interrupt`] because pi does not merely abort here: it first take-alls
    /// BOTH pending queues and puts their text back into the editor, so steering / follow-up
    /// messages the user typed during the run survive the interrupt instead of being silently
    /// dropped. The run loop drains ([`AgentSession::drain_queue`]), hands the result to
    /// [`App::restore_queued_to_editor`], and only then aborts.
    InterruptRestoreQueued,
    /// Abort an in-flight COMPACTION — Pi rebinds `defaultEditor.onEscape` to
    /// `() => this.session.abortCompaction()` for the whole compaction window
    /// (`interactive-mode.ts:3080-3086` @v0.83.0, restored at `:3094-3097`), so Escape cancels the
    /// compaction and nothing else. Without the rebind an Escape mid-compaction reached the ordinary
    /// chain, where `isStreaming` is false (compaction ABORTS the active run and does not set the
    /// agent snapshot) — i.e. it fell through to the empty-editor branch and did nothing.
    AbortCompaction,
    /// Esc pressed **while a `/tree` branch summarization is in flight** — Pi's rebound
    /// `defaultEditor.onEscape = () => this.session.abortBranchSummary()`
    /// (`interactive-mode.ts:4792-4795`). Distinct from [`Self::Interrupt`] because it must NOT tear
    /// down streaming state or kill a bash child: the only effect is cancelling the summarization,
    /// which resolves the spawned navigation with `aborted: true` and re-shows the tree.
    AbortBranchSummary,
    /// `tui.select.cancel` pressed while `/share`'s [`crate::chrome::BorderedLoader`] is mounted —
    /// pi's `CancellableLoader.handleInput` → `abortController.abort(); onAbort?.()`
    /// (`cancellable-loader.ts:31-37`), whose `onAbort` kills the `gh` child, unmounts the loader
    /// and shows `Share cancelled` (`session-share.ts:157-161`).
    ///
    /// Distinct from [`Self::Interrupt`] because the loader is MODAL over the editor and has
    /// nothing to do with the agent: resolving the same Escape through the global keymap — which is
    /// what happened before, there being no loader branch in the routing chain — aborted the
    /// session's run and any live bash child while the share carried on regardless.
    CancelShare,
    /// The user requested to quit the session.
    Quit,
    /// The user requested to suspend the process to the background (Ctrl+Z / SIGTSTP). The run loop
    /// tears down raw mode, raises `SIGTSTP`, and re-enters raw mode on `SIGCONT`.
    Suspend,
    /// A `!`/`!!` bash invocation: the run loop spawns the shell command, streams its output into the
    /// live bash block, and (for `!`, not `!!`) feeds the result back into the session context
    /// (`bash-execution.ts`; interactive-mode.ts `!` handler).
    RunBash { command: String, excluded: bool },
    /// Open the editor buffer in `$VISUAL`/`$EDITOR` (Ctrl+G, `app.editor.external`): the run loop
    /// tears the terminal down, launches the editor on a temp file, then reloads the buffer
    /// (`openExternalEditor`, interactive-mode.ts:3611).
    OpenExternalEditor,
    /// `Ctrl+G` pressed inside an open extension `ui.editor` dialog (L4 review §3): same teardown as
    /// [`Self::OpenExternalEditor`], but seeded from — and written back into — the dialog's OWN
    /// buffer rather than [`AppState::editor`] (Pi `ExtensionEditorComponent.openExternalEditor`,
    /// `extension-editor.ts:119-157`).
    OpenExternalEditorForSelector,
    /// A recognized slash command whose effect lives at the session/data layer (`setupEditorSubmitHandler`,
    /// interactive-mode.ts:2549-2734). The run loop executes it against the [`AgentSession`] (open a
    /// data-bound selector after sourcing its rows, drive the session lifecycle, export, copy, …).
    Command(AppCommand),
    /// A registered extension keyboard shortcut fired (R-08-017; Pi `registerShortcut`). Carries the
    /// shortcut key-id; the run loop dispatches it to the session's extension host
    /// (`ExtensionHost::run_shortcut` → `LiveExtension::execute_shortcut`).
    ExtensionShortcut(String),
    /// State changed; the frame should be redrawn.
    Redraw,
    /// Nothing to do.
    None,
}

/// The direction the model-cycle keybindings move through the cycle set (`app.model.cycleForward` vs
/// `cycleBackward`, `core/keybindings.ts:76-83`; Pi `cycleModel(direction)`, interactive-mode.ts:3617).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CycleDirection {
    /// Next model (Ctrl+P).
    Forward,
    /// Previous model (Shift+Ctrl+P).
    Backward,
}

/// A slash command / keybinding whose execution the run loop performs against the session/resources
/// layer (the in-crate effects — `/hotkeys`, `/debug`, `/changelog`, `/quit` — are applied directly in
/// [`App::dispatch_submission`] and never become an `AppCommand`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppCommand {
    /// Open a data-bound selector; the run loop sources its rows from session-svc / resources.
    OpenSelector(SelectorKind),
    /// `/model [text]` (`handleModelCommand`, interactive-mode.ts:4175-4196): bare (`None`) opens the
    /// unfiltered picker; a `Some(text)` that EXACTLY matches a model sets it directly (no picker),
    /// otherwise opens the picker pre-filtered to `text`. The run loop resolves the exact-match against
    /// the live catalog (`findExactModelReferenceMatch`), so the search term rides the command.
    ModelCommand(Option<String>),
    /// `/login [provider]` (`handleLoginCommand`, interactive-mode.ts:4993-5026): bare (`None`)
    /// opens the auth-method choice; a `Some(ref)` that matches exactly one provider option starts
    /// that login immediately, one that matches several rows of the SAME provider opens the
    /// method choice for it, and anything else opens the full provider picker. Resolution needs
    /// the live registry + credential store, so — like [`Self::ModelCommand`] — the argument rides
    /// the command and the run loop resolves it.
    LoginCommand(Option<String>),
    /// Persist an entry's `/tree` label (Pi `onLabelChange` → `sessionManager.appendLabelChange`,
    /// interactive-mode.ts:4589-4591): set/replace when `label` is non-empty, remove when empty
    /// (`apply_label` drops empty labels). The run loop applies it via the session's `set_label` path.
    SetEntryLabel { entry_id: String, label: String },
    /// Cycle to the next/previous model in place (`app.model.cycleForward`/`cycleBackward`): the run
    /// loop reads the cycle set (the scoped models if any, else the available catalog), advances from
    /// the current model, and calls [`AgentSession::set_model`] (Pi `cycleModel`,
    /// interactive-mode.ts:3617-3632).
    CycleModel(CycleDirection),
    /// Cycle the reasoning level one step (`app.thinking.cycle`, Shift+Tab): the run loop calls
    /// [`AgentSession::cycle_thinking_level`], which is GATED on model support — a non-reasoning model
    /// returns `None` (nothing changes) and cycling otherwise walks the model's OWN supported levels
    /// (incl. `xhigh` where mapped), exactly like Pi's `cycleThinkingLevel` (agent-session.ts:1599 →
    /// interactive-mode.ts:3606-3614). Rides an `AppCommand` because the gate needs the live model.
    CycleThinking,
    /// Set the reasoning level to a specific value — the `/settings` → `Thinking level` submenu
    /// (TUI-032). Pi's `onThinkingLevelChange` calls `this.session.setThinkingLevel(level)`
    /// (`interactive-mode.ts:4222-4226`), which clamps to the model's capabilities and emits
    /// `ThinkingLevelChanged`; it is a session op, not a settings write, so it does not go through
    /// [`Self::ApplySetting`].
    ///
    /// That is true of the **`Enter` path only**, and stating it without that qualifier is what
    /// hid this gap: Pi's `setThinkingLevel` takes `ModelMutationOptions { persist?: boolean }`
    /// (`agent-session.ts:256`) and its `Ctrl+S` sibling passes `{ persist: true }`
    /// (`interactive-mode.ts:4813` → `:4788`). The persisting half is
    /// [`Self::ConfirmSelectionAsDefault`], not this variant — this one stays session-only, which
    /// is correct parity for `Enter`.
    SetThinking(String),
    /// `/thinking [level]` (Pi `handleThinkingCommand`, `interactive-mode.ts:4771-4784`).
    ///
    /// `None` opens the picker; `Some(level)` applies it **session-only** — pi's argument path is
    /// `selectThinkingLevel(level, false)` (`:4786`), i.e. `{ persist: false }`, the same as
    /// `Enter` in the picker. An unrecognised level produces pi's exact error rather than silently
    /// opening the picker.
    ThinkingCommand(Option<String>),
    /// `/settings` → "Default thinking level per model" step 2 (Pi's stepped-submenu `onComplete`,
    /// `settings-selector.ts:652-666`). `model` is `"provider/id"`; `level` is a level name or the
    /// [`crate::app::CLEAR_MODEL_THINKING`] sentinel, which REMOVES the override.
    SetModelThinkingLevel { model: String, level: String },
    /// Apply a confirmed data-bound selection (`{kind}` chose `{value}`): set the model, switch the
    /// branch, login/logout, etc.
    ConfirmSelection { kind: SelectorKind, value: String },
    /// `Ctrl+S` inside the model or thinking picker: apply the selection to the session **and**
    /// persist it as the global default (Pi `selectModel(m, true)` / `selectLevel(l, true)` →
    /// `{ persist: true }`, `interactive-mode.ts:4999`, `:4813`).
    ///
    /// Distinct from [`Self::ConfirmSelection`] because the persist is opt-in per keypress, not a
    /// property of the selection: plain `Enter` must leave the settings file untouched.
    /// The write is Global-scoped unconditionally — Pi's setters go straight to
    /// `this.globalSettings` (`settings-manager.ts:731-744`, `:786-790`) with no scope choice.
    ConfirmSelectionAsDefault { kind: SelectorKind, value: String },
    /// Persist a settings field changed **in place** in the `/settings` grid (Pi settings-selector
    /// `onChange` → `SettingsManager.setNested`). The slot stays open; the `/reload` re-reads it.
    ApplySetting { id: String, value: String },
    /// `/new` — start a fresh session (`handleClearCommand`).
    NewSession,
    /// `/compact [instructions]` — manually compact context (`handleCompactCommand`).
    Compact(Option<String>),
    /// `/clone` — duplicate the session at the current position (`handleCloneCommand`).
    Clone,
    /// `/reload` — rebuild keybindings/extensions/skills/prompts/themes (`handleReloadCommand`).
    Reload,
    /// `/export [path]` — export the session (`handleExportCommand`).
    Export(Option<String>),
    /// `/import <path>` — import + resume a JSONL session (`handleImportCommand`).
    Import(Option<String>),
    /// `/share` — publish the session as a secret gist (`handleShareCommand`).
    Share,
    /// `/copy` — copy the last assistant message to the clipboard (`handleCopyCommand`).
    Copy,
    /// `app.message.copy` pressed while `/tree` owns the input slot: copy the HIGHLIGHTED entry's
    /// full text to the clipboard (pi `TreeList.copySelected`, `tree-selector.ts:627-630`, wired to
    /// the consumer at `interactive-mode.ts:5297-5308`). Carries the entry id; the run loop resolves
    /// the text against the live session via [`AgentSession::entry_copy_text`], which is pi's
    /// `getEntryCopyText` (`:896-922`).
    ///
    /// The id — not the text — rides the command because the DAG the selector holds carries only
    /// clipped one-line row labels; materialising every entry's full body into the node list to
    /// serve one keystroke would put the whole transcript in the selector.
    CopyEntry(String),
    /// `/name <name>` — set the session display name (`handleNameCommand`).
    SetName(String),
    /// `/name` with NO argument — the GETTER half of `handleNameCommand`
    /// (`interactive-mode.ts:5632-5644` @v0.83.0): with a name set it reports it, and only with no
    /// name set does it warn about usage (TUI-080). Needs the run loop because the stored name lives
    /// on the session, which `run_command` cannot reach.
    ShowName,
    /// `/session` — show session info + stats (`handleSessionCommand`).
    SessionInfo,
    /// `/resume` in-list delete of a persisted session file (session-selector.ts:540 →
    /// `delete_session_file`). Carries the session path.
    DeleteSession(String),
    /// `/resume` in-list rename of a persisted session (session-selector.ts:585 →
    /// `rename_session_file`). Carries the session path + new name.
    RenameSession { path: String, name: String },
}

