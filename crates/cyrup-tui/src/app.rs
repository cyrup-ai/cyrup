//! The app shell + event loop (arch-10 §3.3 `Tui`, §5 concurrency; R-ARCH-TUI-003/004).
//!
//! [`App`] is generic over `ratatui::backend::Backend` so the same render/ingest/input logic runs
//! against a real terminal (`CrosstermBackend`) and a headless `TestBackend` (R-10-010 /
//! R-ARCH-TUI-010). The interactive layout is an **inline viewport** (NOT the alternate screen,
//! R-ARCH-TUI-003): the live region holds only the *active* region — the in-flight streaming turn,
//! the editor, and the status line. Each committed conversation entry is flushed exactly once to the
//! terminal's native scrollback via `Terminal::insert_before` ([`App::draw`] →
//! `flush_committed`), so completed history scrolls natively and is never re-rendered in the viewport.
//!
//! `render` is pure (`state -> frame`): [`render`] takes `&mut AppState` and a `Frame` and never
//! touches real I/O, so tests draw into a `TestBackend` buffer and assert on cells.

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cyrup_core::{CancelToken, EventStream, ModelThinkingLevel};
// The extension-facing session backend trait: brings `LiveHostServices::set_label` (the live
// label-append the `/tree` `e` rename persists through — the SAME path a guest's `setLabel` uses,
// host_services.rs:866) into scope.
use cyrup_ext::host::HostServices;
use cyrup_config::login::{
    AuthType, LoginCommand, LoginProviderOption, LoginStep, ProviderLoginInput,
};
use cyrup_provider::auth::oauth::OAuthError;
use cyrup_provider::StreamEvent;
use cyrup_resources::theme::ThemeData;
use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, CompactionReason, InputSource, SummarizationRetrySource,
    UserInput,
};
use cyrup_session_svc::{
    AgentSessionRuntime, ForkPosition, NavigateTreeOptions, NavigateTreeOutcome, SessionDagKind,
    SessionDagNode,
};
use cyrup_session_svc::{NotifyKind, UiEffect, UiKind, UiReply, UiRequest};
use futures::StreamExt;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::terminal::{
    enable_raw_mode, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate,
};
use ratatui::crossterm::{execute, queue, ExecutableCommand};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::commands::{CommandRegistry, Dispatch};
use crate::component::{Component, InputEvent};
use crate::editor::{EditorOutcome, InputEditor};
use crate::error::TuiError;
use crate::extension_editor::ExtensionEditorSelector;
use crate::image::{ImageBlock, ImageRenderer, TerminalCapabilities};
use crate::keymap::{
    Action, EditorAction, Key, Keymap, ModelsKeymap, SelectAction, SelectKeymap, SessionKeymap,
    TreeKeymap,
};
use crate::login_dialog::{
    notify_auth_dialog, show_auth_prompt, LoginDialog, LoginFinished, LoginUiMsg,
    TuiAuthInteraction,
};
use crate::model_selector::{ModelEntry, ModelSelector};
use crate::overlay::{ExtensionOverlay, Overlay, OverlayOutcome};
use crate::selector::{
    CheckboxSelector, ListSelector, Selector, SelectorKind, SelectorOutcome,
};
use crate::session_selector::{SessionRow, SessionSelector, SessionSelectorOutcome};
use crate::settings_selector::{SettingRow, SettingsSelector, TrustSelector};
use crate::status::StatusLine;
use crate::status_indicator::{IndicatorKind, StatusIndicator, SPINNER_INTERVAL};
use crate::stray_reply::StrayReplyFilter;
use crate::terminal_title::session_terminal_title;
use crate::text_input::TextInputSelector;
use crate::theme::{ColorMode, ThemeController, UiTheme};
use crate::transcript::{content_text, entry_lines, thinking_text, TranscriptView};
use crate::tree_selector::{TreeKind, TreeNode, TreeSelector};

/// The number of visual lines a `PageUp`/`PageDown` scrolls the active region by (a conservative
/// screenful; spec/tui/07 page-scroll). Resolved on the pure input thread without the live viewport
/// height, then clamped against the real content at render time.
const PAGE_SCROLL_LINES: usize = 10;

/// How often a running `bash` call's `Elapsed …` figure is repainted — Pi's
/// `setInterval(() => context.invalidate(), 1000)` (bash.ts:471-473), armed only while a bash result
/// is still partial. See [`TranscriptView::has_running_elapsed_tool`].
pub const ELAPSED_TICK_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Esc pressed **while a `/tree` branch summarization is in flight** — Pi's rebound
    /// `defaultEditor.onEscape = () => this.session.abortBranchSummary()`
    /// (`interactive-mode.ts:4792-4795`). Distinct from [`Self::Interrupt`] because it must NOT tear
    /// down streaming state or kill a bash child: the only effect is cancelling the summarization,
    /// which resolves the spawned navigation with `aborted: true` and re-shows the tree.
    AbortBranchSummary,
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
    /// Apply a confirmed data-bound selection (`{kind}` chose `{value}`): set the model, switch the
    /// branch, login/logout, etc.
    ConfirmSelection { kind: SelectorKind, value: String },
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
    /// `/name <name>` — set the session display name (`handleNameCommand`).
    SetName(String),
    /// `/session` — show session info + stats (`handleSessionCommand`).
    SessionInfo,
    /// `/resume` in-list delete of a persisted session file (session-selector.ts:540 →
    /// `delete_session_file`). Carries the session path.
    DeleteSession(String),
    /// `/resume` in-list rename of a persisted session (session-selector.ts:585 →
    /// `rename_session_file`). Carries the session path + new name.
    RenameSession { path: String, name: String },
}

/// All retained UI state (the data half of the `state -> frame` split).
pub struct AppState {
    pub transcript: TranscriptView,
    pub editor: InputEditor,
    pub status: StatusLine,
    /// The working/idle status band (spec/tui/01 §6) — a 2-row spinner+message while a turn/retry/
    /// compaction runs, blank when idle. Driven by `AgentSessionEvent`s in [`App::ingest_event`].
    pub indicator: StatusIndicator,
    pub theme: UiTheme,
    /// The terminal color depth every theme is projected into (feature #3/#4): boot resolves it from
    /// the terminal (`ColorMode::detect` / the `ThemeController`); a live `/theme` switch re-projects
    /// the new theme through it so 256-color terminals keep indexed colors after a switch.
    pub color_mode: ColorMode,
    pub keymap: Keymap,
    /// The selector binding table (`tui.select.*`, spec/tui/05 §10) consulted while a selector owns
    /// the input slot.
    pub select_keymap: SelectKeymap,
    /// The `/tree` bespoke binding table (`app.tree.*`, spec/tui/05 §6.1) handed to each opened
    /// [`TreeSelector`] so JSON rebinds of fold/unfold/label flow through (R-10-018).
    pub tree_keymap: TreeKeymap,
    /// The `/resume` bespoke binding table (`app.session.*`, `core/keybindings.ts:91-94,135-154`)
    /// handed to each opened [`SessionSelector`], so a JSON rebind of sort/named/delete/path/rename
    /// reaches BOTH the handler and the header's hint rows (`session-selector.ts:171-179`).
    pub session_keymap: SessionKeymap,
    /// The `/scoped-models` bespoke binding table (`app.models.*`, `core/keybindings.ts:150-175`)
    /// handed to each opened [`CheckboxSelector`], so a JSON rebind of reorder/all/clear/provider/
    /// save reaches both the handler and the footer row (`scoped-models-selector.ts:199-204`).
    pub models_keymap: ModelsKeymap,
    /// The slash-command registry driving dispatch + autocomplete (rebuilt on `/reload`).
    pub commands: CommandRegistry,
    /// The active editor-swap selector, if any (spec/tui/05 §1.1): when `Some`, it replaces the
    /// editor in the bottom inline region and captures input until it confirms/cancels.
    pub selector: Option<ActiveSelector>,
    /// The floating overlay z-stack (spec/tui/05 §2): hotkeys/help popup (and, later, extension UI).
    /// The topmost overlay captures input; rendered over the live region bottom→top.
    pub overlays: Vec<Box<dyn Overlay>>,
    /// The current reasoning level (`off`…`xhigh`), preselected by the thinking selector and updated
    /// on confirm. The authoritative level lives on the agent/session at the L7 layer.
    pub thinking_level: String,
    /// Whether inline images are shown (vs. a text placeholder), toggled by the show-images selector.
    pub show_images: bool,
    /// The terminal image-protocol renderer (spec/tui/06 §6; `terminal-image.ts`). Defaults to the
    /// portable half-block raster; the production binary upgrades it to the real protocol via
    /// [`App::detect_image_support`]. Drives the inline render of [`AppState::pending_images`].
    pub image_renderer: ImageRenderer,
    /// Images attached to the next prompt (the `@`-mention of an image file / a paste), rendered
    /// inline above the editor in the live region (`components/image.ts`), honoring `show_images`.
    pub pending_images: Vec<ImageBlock>,
    /// Reserve the 2-row status band even when idle (spec/tui/01 §6.3). Default `false` (Pi's
    /// non-`clearOnShrink` behavior) so the editor/footer never reflow on idle viewports.
    pub reserve_status_rows: bool,
    /// The host TERMINAL's row count — Pi's `this.tui.terminal.rows` (`editor.ts:500`). Refreshed
    /// every [`App::draw`] from the backend; `24` until the first draw, matching the `?? 24` default
    /// pi itself uses when a terminal height is unavailable (`config-selector.ts:264-266`).
    ///
    /// **Not** the live-region height. The editor's row budget is `max(5, floor(terminalRows * 0.3))`
    /// (E3), which must be answered against the SCREEN; `region_constraints` is called once with the
    /// terminal height (from [`live_region_height`]) and once with the resulting viewport height
    /// (from [`render`]), and deriving the budget from its `avail` argument would make those two
    /// calls disagree and the split non-idempotent.
    pub term_rows: u16,
    /// Whether the compact startup keybinding-hints bar is shown (Pi `compactInstructions`,
    /// interactive-mode.ts:697-703): a one-line `interrupt · clear/exit · / commands · ! bash · more`
    /// affordance bar rendered just above the editor at startup, dismissed on the first submission.
    pub show_startup_hints: bool,
    /// A `DynamicBorder` loader occupying the editor slot during a long inline op (Pi
    /// `BorderedLoader`, bordered-loader.ts): `/share`'s gist creation and any extension-UI long op.
    /// When `Some`, it replaces the editor in the live region (the selector still wins if both are set,
    /// which never happens). Cleared when the op completes.
    pub loader: Option<crate::chrome::BorderedLoader>,
    /// The 80 ms phase index for the active [`Self::loader`] / status spinner (advanced by the run-loop
    /// tick). Drives the loader's animated glyph without a timer thread.
    pub loader_tick: usize,
    /// Set when the user requested quit; the run loop observes it.
    pub should_quit: bool,
    /// Timestamp of the last `Ctrl+C` press, for the double-tap-to-exit gate (Pi `handleCtrlC`,
    /// interactive-mode.ts:3361-3369): a second `Ctrl+C` within 500 ms exits; otherwise it clears the
    /// editor and records the press time. `None` until the first press.
    last_sigint: Option<std::time::Instant>,
    /// A status line to show **after** the next runtime session-swap re-binds the UI (the swap
    /// resets the transcript, so a pre-swap status would be wiped). Set by the session-lifecycle
    /// command handlers (`/new`/`/resume`/`/fork`/`/reload`/`/import`); consumed by
    /// [`App::rebind_session`] once the generation bump fires and the new session is installed.
    pub pending_swap_status: Option<String>,
    /// Committed lines already emitted to native scrollback via `Terminal::insert_before`
    /// (R-ARCH-TUI-003). Kept as a test-visible accumulator mirroring exactly what was handed to
    /// `insert_before`; never re-rendered inside the inline viewport.
    pub scrollback: Vec<Line<'static>>,
    /// Extension-registered keyboard shortcuts (R-08-017; Pi `registerShortcut`): each parsed
    /// [`Key`] spec paired with the [`ShortcutSpec`] the host routes on. Sourced from
    /// `ExtensionHost::shortcut_keys()` at boot and refreshed on session swap; a matching key press
    /// (checked at the global-keymap tier, after built-in bindings) becomes an
    /// [`AppAction::ExtensionShortcut`]. Empty when no extension registered a shortcut.
    ///
    /// This is also the registry `/hotkeys` reads for its **Extensions** table — upstream's
    /// `extensionRunner.getShortcuts(...)` (`interactive-mode.ts:6187-6196`) is the same set from
    /// the same source, so [`App::hotkeys_markdown`] iterates it rather than omitting the section.
    pub extension_shortcuts: Vec<(Key, ShortcutSpec)>,
    /// The env-sniffed terminal capabilities (feature #7/#8; Pi `getCapabilities`): image protocol +
    /// truecolor + OSC-8-hyperlink forwarding. Boot default is conservative (half-block, no
    /// hyperlinks); the binary refines it via [`App::detect_image_support`]. The `hyperlinks` flag
    /// gates OSC-8 emission in rendered links (`osc::hyperlink`).
    pub capabilities: TerminalCapabilities,
    /// The REPLY half of the extension-UI dialog currently occupying [`Self::selector`] (`kind ==
    /// SelectorKind::Extension{Confirm,Select,Input}`), if any (L4 review §2.1). A loaded guest's
    /// synchronous `ui.{confirm,select,input}` call blocks its own tokio task on this one-shot
    /// (`LiveHostServices::ui_roundtrip`) until the selector confirms or cancels; `App::run`'s `ui_rx`
    /// arm sets it when it opens the dialog, and [`App::confirm_selector`] /
    /// [`App::handle_selector_key`]'s `Cancel` arm take + resolve it. `None` whenever no extension
    /// dialog is open (including every ordinary first-party selector).
    pending_ui_reply: Option<PendingUiReply>,
    /// The `/tree` target the user confirmed, held while the "Summarize branch?" prompt (and, on its
    /// third option, the custom-instructions editor) is open — Pi keeps the same values in the
    /// `entryId` / `wantsSummary` / `customInstructions` locals of its `while (true)` prompt loop
    /// (`interactive-mode.ts:4749-4779`). Cleared the moment the navigation is dispatched or the
    /// prompt is escaped back to the tree.
    pending_tree_nav: Option<PendingTreeNav>,
    /// The window title currently asked for — either by an extension (Pi `setTitle` →
    /// `ui.terminal.setTitle`, `interactive-mode.ts:2238` → `terminal.ts:504-507`) or by the
    /// automatic session/cwd title ([`App::update_terminal_title`], Pi `updateTerminalTitle`,
    /// `interactive-mode.ts:818-826`). Retained so the value is observable in tests and after a
    /// redraw; the crossterm run loop is what actually writes the OSC 0 sequence.
    pub terminal_title: Option<String>,
    /// The OSC 9;4 taskbar progress indicator — Pi's `terminal.showTerminalProgress` gate plus the
    /// armed bit ([`crate::TerminalProgress`], `tui/src/terminal.ts:509-523`). Held here for the
    /// same reason as [`AppState::terminal_title`] directly above: the session-event fold records
    /// the transition and the crossterm run loop is what writes the escape sequence.
    pub terminal_progress: crate::TerminalProgress,
    /// Pi `this.streamingComponent` (`interactive-mode.ts:435`): the assistant message currently
    /// streaming, as a plain "is one open?" bit — cyrup's transcript owns the buffers, so only the
    /// lifetime matters here.
    ///
    /// Set on `message_start` for an `assistant` message (`:3129-3141`) and cleared the moment that
    /// message is finalized (`this.streamingComponent = undefined`, `:3213`). It is the guard Pi's
    /// `message_end` arm opens with (`if (this.streamingComponent && event.message.role ===
    /// "assistant")`, `:3182`), and it is what keeps a defensively-handled terminal
    /// `StreamEvent::Done` inside `message_update` from committing the same message twice.
    pub streaming_assistant: bool,
    /// The working directory whose basename goes into the automatic terminal title — Pi
    /// `sessionManager.getCwd()` (`interactive-mode.ts:819`). Seeded from the process cwd and
    /// re-pointed at the live session's cwd by [`App::run`] (and on every session swap), since a
    /// `/resume` of a session recorded elsewhere moves it.
    pub title_cwd: PathBuf,
    /// The custom header content an extension published (Pi `setHeader`, `interactive-mode.ts:2237`).
    /// Delivered (no longer dropped) and retained here — cyrup's TUI has no extension chrome slot to
    /// render it in yet, which is TUI-014's remaining half.
    pub extension_header: Option<String>,
    /// The custom footer content an extension published (Pi `setFooter`, `interactive-mode.ts:2236`).
    /// Same status as [`Self::extension_header`].
    pub extension_footer: Option<String>,
    /// The most recent extension widget payload (Pi `setWidget`, `interactive-mode.ts:2235`). Cyrup's
    /// WIT collapses Pi's `{key, content, options}` into one opaque JSON blob, so there is no key to
    /// map by; the latest payload wins. Same "delivered, not rendered" status as the header/footer —
    /// this is exactly TUI-014, which the sink wiring alone does NOT close.
    pub extension_widget: Option<serde_json::Value>,
    /// Whether a branch summarization spawned by [`App::begin_tree_navigation`] is still in flight.
    /// While set, `Esc` routes to `AgentSession::abort_branch_summary` instead of the turn abort —
    /// Pi's `defaultEditor.onEscape = () => this.session.abortBranchSummary()`
    /// (`interactive-mode.ts:4792-4795`), restored in its `finally`.
    branch_summary_in_flight: bool,
    /// The footer's git-branch source (Pi's `FooterDataProvider`, `footer-data-provider.ts`), which
    /// is what fills [`StatusLine::branch`]. Boots as "no repo" and is pointed at the session cwd by
    /// [`App::set_footer_git_cwd`]; the run loop re-polls it so a `git checkout` elsewhere repaints.
    pub git_branch: crate::footer_data::FooterGitBranch,
    /// The `AuthSelectorProvider[]` backing the open `/login` picker — Pi's `providerOptions` local
    /// (`showLoginProviderSelector`, `interactive-mode.ts:5086-5148`). Confirming carries the row
    /// INDEX into this vector, because one provider can contribute two rows (oauth + api key) and
    /// the provider id alone cannot disambiguate them.
    login_options: Vec<cyrup_config::login::LoginProviderOption>,
    /// The `/logout` twin of [`Self::login_options`] (`getLogoutProviderOptions`,
    /// `interactive-mode.ts:4970-4979`). Carries each row's `authType`, which picks between Pi's two
    /// logout status messages (`interactive-mode.ts:5159-5162`).
    logout_options: Vec<cyrup_config::login::LoginProviderOption>,
    /// The provider options an open [`SelectorKind::LoginAuthType`] selector is choosing BETWEEN —
    /// Pi's `providerOptions?` argument to `showLoginAuthTypeSelector`
    /// (`interactive-mode.ts:5028`). `None` for a bare `/login` (the method choice then opens the
    /// provider picker filtered to it, `:5063-5070`); `Some` when `/login <provider>` already
    /// pinned one provider that offers both methods (`:4998-5009`).
    login_auth_type_options: Option<Vec<cyrup_config::login::LoginProviderOption>>,
    /// The REPLY half of the login prompt the flow is currently blocked on — the login twin of
    /// [`Self::pending_ui_reply`]. The spawned login task's `AuthInteraction::prompt` awaits this
    /// one-shot (`login_dialog::TuiAuthInteraction::prompt`, Pi's `inputResolver`/`inputRejecter`
    /// pair, `login-dialog.ts:16-17`); [`App::confirm_selector`] resolves it with the typed answer
    /// and [`App::handle_selector_key`]'s `Cancel` arm rejects it with `"Login cancelled"`.
    pending_login_prompt: Option<tokio::sync::oneshot::Sender<Result<String, OAuthError>>>,
    /// The dialog's `AbortController` (`login-dialog.ts:15`, `:73-75`) for the flow currently on
    /// screen: `cancel()` fires it so a flow blocked on something other than a prompt (a callback
    /// server, a device-code poll) also unwinds. `None` whenever no login is in flight.
    login_cancel: Option<CancelToken>,
    /// Provider ids whose STORED credential is an OAuth one — cyrup's standing copy of the half of
    /// pi's `modelRuntime.snapshot.auth` that `isUsingOAuth` reads
    /// (`model-runtime.ts:458-460`, pi v0.84.1: `this.snapshot.auth.get(providerId)?.type ===
    /// "oauth"`).
    ///
    /// Pi can answer that question synchronously at footer-render time because the snapshot is an
    /// in-memory map the runtime keeps warm; cyrup's equivalent read
    /// ([`cyrup_config::login::stored_credentials`]) parses `auth.json` and is `async`, while the
    /// footer is folded from a **sync** `&mut self` (`ingest_event_rendered`). So the map is cached
    /// here and refreshed at exactly the points pi's own snapshot moves: session bind/swap and a
    /// settled `/login` or `/logout` (each of which ends in `footer.invalidate()`,
    /// `interactive-mode.ts:5449`, `:5475`). See [`App::refresh_auth_snapshot`].
    oauth_credential_providers: std::collections::BTreeSet<String>,
}

/// Row values of the "Summarize branch?" prompt. Pi compares the returned LABELS
/// (`summaryChoice !== "No summary"`, `=== "Summarize with custom prompt"`,
/// `interactive-mode.ts:4767,4769`); cyrup's [`ListSelector`] carries a separate value column, so
/// the labels stay Pi-exact for display while the routing keys stay stable.
/// The one provider id pi's footer treats as subscription-backed regardless of how it authenticates
/// — *"Kimi Coding is subscription-backed despite using API-key authentication"*
/// (pi v0.84.1 `coding-agent/src/modes/interactive/components/footer.ts:138-140`).
const KIMI_CODING_PROVIDER_ID: &str = "kimi-coding";

const BRANCH_SUMMARY_NONE: &str = "none";
const BRANCH_SUMMARY_YES: &str = "summarize";
const BRANCH_SUMMARY_CUSTOM: &str = "custom";

/// The `/tree` navigation awaiting the "Summarize branch?" answer (see
/// [`AppState::pending_tree_nav`]).
#[derive(Clone, Debug)]
struct PendingTreeNav {
    /// The confirmed tree row's entry id.
    target: String,
}

impl AppState {
    /// Fresh state with the given theme.
    pub fn new(theme: UiTheme) -> Self {
        // `if (areExperimentalFeaturesEnabled()) statsParts.push(… "xp" …)` (`footer.ts:162-164`).
        // Upstream re-reads `process.env.PI_EXPERIMENTAL` inside `render()`; cyrup reads it once
        // here, which is the only production writer of the flag — `set_experimental` had no caller
        // outside a test, so the `• xp` marker was unreachable however the user launched.
        let mut status = StatusLine::default();
        status.set_experimental(crate::status::experimental_features_enabled());
        AppState {
            transcript: TranscriptView::new(),
            editor: InputEditor::new(),
            status,
            indicator: StatusIndicator::new(),
            color_mode: theme.color_mode,
            theme,
            keymap: Keymap::default(),
            select_keymap: SelectKeymap::default(),
            tree_keymap: TreeKeymap::default(),
            session_keymap: SessionKeymap::default(),
            models_keymap: ModelsKeymap::default(),
            commands: CommandRegistry::new(),
            selector: None,
            overlays: Vec::new(),
            thinking_level: "medium".to_string(),
            show_images: true,
            image_renderer: ImageRenderer::default(),
            pending_images: Vec::new(),
            reserve_status_rows: false,
            term_rows: 24,
            show_startup_hints: true,
            loader: None,
            loader_tick: 0,
            should_quit: false,
            last_sigint: None,
            pending_swap_status: None,
            scrollback: Vec::new(),
            extension_shortcuts: Vec::new(),
            capabilities: TerminalCapabilities {
                images: None,
                true_color: true,
                hyperlinks: false,
            },
            pending_ui_reply: None,
            pending_tree_nav: None,
            terminal_title: None,
            // Off until a session binds and `terminal.showTerminalProgress` is read ([`App::run`]).
            // Pi has no seed at all — it re-reads the setting at each of its five call sites.
            terminal_progress: crate::TerminalProgress::default(),
            streaming_assistant: false,
            // Pi reads `sessionManager.getCwd()` at title time; the process cwd is the same value
            // until a session with a recorded cwd is bound, which re-points it ([`App::run`]).
            title_cwd: std::env::current_dir().unwrap_or_default(),
            extension_header: None,
            extension_footer: None,
            extension_widget: None,
            branch_summary_in_flight: false,
            // Pi constructs its `FooterDataProvider` from the session cwd; the binary points this at
            // the runtime's cwd via [`App::set_footer_git_cwd`] before the first frame. Booting as
            // "no repo" keeps a backend-only `AppState` free of any filesystem probe.
            git_branch: crate::footer_data::FooterGitBranch::none(),
            login_options: Vec::new(),
            logout_options: Vec::new(),
            login_auth_type_options: None,
            pending_login_prompt: None,
            login_cancel: None,
            oauth_credential_providers: std::collections::BTreeSet::new(),
        }
    }

    /// Whether a `/tree` branch summarization is still running (test/inspection access; drives the
    /// `Esc`→`abort_branch_summary` routing).
    pub fn branch_summary_in_flight(&self) -> bool {
        self.branch_summary_in_flight
    }

    /// Install the extension-registered keyboard shortcuts (R-08-017): each raw key-id is parsed to a
    /// [`Key`] spec (unparseable ids are dropped, never panicking) and retained with its id so a
    /// matching press routes to the owning extension. Called by the binary at boot and after a
    /// session swap, so a `/reload` that changes the registered set takes effect.
    ///
    /// Accepts either a bare key-id (`ExtensionHost::shortcut_keys()`'s `Vec<String>`) or an
    /// `(id, description)` pair — see [`ShortcutSpec`] for why both forms exist.
    pub fn set_extension_shortcuts(&mut self, specs: impl IntoIterator<Item = impl Into<ShortcutSpec>>) {
        self.extension_shortcuts = specs
            .into_iter()
            .map(Into::into)
            .filter_map(|spec| Key::parse(&spec.id).ok().map(|k| (k, spec)))
            .collect();
    }
}

/// One extension-registered keyboard shortcut as the TUI holds it — the display-side half of
/// upstream's `ExtensionShortcut` record (`coding-agent/src/core/extensions/types.ts:1547-1552`:
/// `shortcut: KeyId`, `description?: string`, `handler`, `extensionPath: string`). The handler lives
/// on the guest side of the WASM boundary and never crosses into the TUI; the id and the label are
/// what `/hotkeys` and the dispatcher need.
///
/// Both `From` impls exist because the two callers differ: the binary installs
/// `ExtensionHost::shortcut_keys()` (`crates/cyrup/src/main.rs:1634`), a `Vec<String>` of bare ids,
/// while a host that also carries the registered description installs `(id, description)` pairs.
/// A bare id therefore keeps working unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutSpec {
    /// `shortcut: KeyId` (`types.ts:1548`) — the raw id the host routes a fired press back on.
    pub id: String,
    /// `description ?? extensionPath` (`interactive-mode.ts:6193`) — the `/hotkeys` Action cell.
    /// `None` when the registering host supplied neither.
    pub description: Option<String>,
}

impl From<String> for ShortcutSpec {
    fn from(id: String) -> Self {
        ShortcutSpec { id, description: None }
    }
}

impl From<&str> for ShortcutSpec {
    fn from(id: &str) -> Self {
        ShortcutSpec { id: id.to_string(), description: None }
    }
}

impl From<(String, String)> for ShortcutSpec {
    fn from((id, description): (String, String)) -> Self {
        ShortcutSpec { id, description: Some(description) }
    }
}

impl From<(String, Option<String>)> for ShortcutSpec {
    fn from((id, description): (String, Option<String>)) -> Self {
        ShortcutSpec { id, description }
    }
}

/// The active editor-swap selector plus the state needed to restore the editor on close (spec/tui/05
/// §7 `ActiveSelector`). Pi snapshots the editor text on open (`interactive-mode.ts:2371`) and, for
/// the theme picker, restores the prior theme on cancel (`theme-selector.ts` caller responsibility).
pub struct ActiveSelector {
    kind: SelectorKind,
    inner: Box<dyn Selector>,
    /// Editor text snapshotted on open, re-applied when the slot closes.
    saved_editor: String,
    /// Theme to restore if a previewing selector is cancelled (theme picker only).
    restore_theme: Option<UiTheme>,
}

/// The REQUEST/REPLY pairing an open extension-UI dialog (`SelectorKind::Extension{Confirm,Select,
/// Input}`) resolves against (L4 review §2.1): `kind` is retained so a `Cancel` can resolve to the
/// correct per-kind deny default ([`default_ui_reply`]) without re-deriving it from the selector kind
/// at the call site.
struct PendingUiReply {
    kind: UiKind,
    reply: tokio::sync::oneshot::Sender<UiReply>,
    /// The dialog's title WITHOUT any countdown suffix, so each tick recomputes `"{base_title}
    /// ({s}s)"` fresh off the current remaining time rather than accumulating appended text.
    base_title: String,
    /// When this dialog auto-resolves to its per-kind deny default if the user hasn't answered by
    /// then — Pi's `CountdownTimer` (`countdown-timer.ts:7-38`), armed from the guest's
    /// `opts.timeout_ms` exactly like `LiveHostServices::ui_roundtrip`'s OWN independent host-side
    /// timeout race (`host_services.rs`) arms from the SAME value; the two are deliberately separate
    /// clocks (mirroring Pi's `createDialogPromise`'s host-armed `setTimeout` vs. the renderer's own
    /// `CountdownTimer`, `rpc-mode.ts:114-119`) — whichever fires first wins the reply, and the loser
    /// finds it a harmless no-op. `None` when the guest set no timeout (dialog waits indefinitely for
    /// a key, matching `ui_roundtrip`'s own `None` branch).
    deadline: Option<tokio::time::Instant>,
}

/// The per-kind deny default a dialog resolves to when the user cancels it (`Esc`) rather than
/// answering — Pi's `noOpUIContext` shape (`runner.ts:230-261`), the same mapping
/// `crates/cyrup-modes/src/rpc.rs`'s `default_ui_reply` uses for a timed-out/force-resolved RPC dialog.
fn default_ui_reply(kind: UiKind) -> UiReply {
    match kind {
        UiKind::Confirm => UiReply::Confirm(false),
        UiKind::Input | UiKind::Editor | UiKind::Select => UiReply::Text(None),
    }
}

/// Format `base` with a live "(Ns)" countdown suffix — Pi's `CountdownTimer`'s exact title format
/// (`` `${this.baseTitle} (${s}s)` ``, `countdown-timer.ts:14,23,55`). Rounds UP (Pi's own
/// `Math.ceil(timeoutMs / 1000)`, `countdown-timer.ts:18`) so e.g. 4500ms remaining reads "5s", not
/// "4s"; a `deadline` already in the past reads "0s" (the tick loop closes the dialog that same
/// pass, so this is never rendered for more than one frame).
fn countdown_title(base: &str, deadline: tokio::time::Instant) -> String {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let secs = remaining.as_millis().div_ceil(1000);
    format!("{base} ({secs}s)")
}

/// A backend that can rebuild a fresh handle over the **same** underlying terminal, so the inline
/// viewport can be re-sized to the live region's content height (ratatui's `Viewport::Inline` height
/// is fixed at construction; ADR-0001 commitment #1 / audit #1 require a content-sized region). The
/// rebuilt backend must preserve the cursor anchor used to place the inline viewport: `TestBackend`
/// copies its grid cursor; `CrosstermBackend` re-wraps `stdout`, where the real terminal cursor is
/// already authoritative.
pub trait RebuildBackend: Backend + Sized {
    /// A fresh backend over the same terminal, preserving the inline-viewport cursor anchor.
    fn rebuild(&self) -> Self;

    /// Prepare the **current** backend for a reconstruction at `new_height` (called by
    /// [`App::draw`] → `resize_viewport` immediately before [`rebuild`](Self::rebuild)): erase the
    /// current inline region and re-anchor the cursor so the rebuilt viewport leaves **no residual
    /// chrome** on a real terminal — the fix for the inline-viewport STACKING bug. See
    /// [`reanchor_inline_region`] for the geometry.
    ///
    /// The default is a **no-op**, which is correct for fresh-grid backends whose `rebuild` starts
    /// from a blank buffer and re-anchors itself (e.g. `TestBackend`): they can never stack, so there
    /// is nothing to erase. Only a persistent real-screen backend (`CrosstermBackend`) needs it.
    fn reanchor_inline(&mut self, _term_height: u16, _old_height: u16, _new_height: u16) {}
}

/// Erase the current inline region and re-anchor the cursor so that reconstructing the inline
/// viewport at `new_height` leaves **no residual chrome** (the inline-viewport STACKING fix).
///
/// ratatui's `Viewport::Inline(height)` is immutable after construction and `Terminal::resize` cannot
/// change it (it treats its argument as the *terminal* size), so a content-sized live region must be
/// reconstructed whenever its height changes. Reconstruction reserves the new rows by calling
/// `Backend::append_lines`, which — at the bottom of a real terminal — SCROLLS: without this step the
/// prior frame's hint bar / editor rules / footer scroll up into native scrollback and stay visible,
/// so a streaming turn that grows over several frames leaves a stack of duplicated chrome.
///
/// Emitting `MoveTo(top-of-current-region)` + `Clear(FromCursorDown)` first means that scroll carries
/// **blanks**, not chrome. The returned row is where the cursor is left; the reconstruction's
/// `compute_inline_size` reads it via `get_cursor_position`. The anchor is bottom-aligned with the
/// **minimal** scroll — `term_height - min(old, new)`: on growth this is the old region's top (append
/// scrolls exactly the delta); on shrink it sits lower so the shorter region stays pinned to the
/// bottom with no scroll. Both cases leave the region bottom-anchored, for growth **and** shrink.
pub fn reanchor_inline_region<W: io::Write>(
    w: &mut W,
    term_height: u16,
    old_height: u16,
    new_height: u16,
) -> u16 {
    let term_h = term_height.max(1);
    let last_row = term_h.saturating_sub(1);
    let anchor_y = term_h.saturating_sub(old_height.min(new_height)).min(last_row);
    if old_height > 0 {
        // Erase the whole current inline region (its top row down to the bottom of the screen).
        let erase_top = term_h.saturating_sub(old_height).min(last_row);
        let _ = queue!(w, MoveTo(0, erase_top), Clear(ClearType::FromCursorDown));
    }
    let _ = queue!(w, MoveTo(0, anchor_y));
    let _ = w.flush();
    anchor_y
}

impl RebuildBackend for ratatui::backend::TestBackend {
    fn rebuild(&self) -> Self {
        let area = self.buffer().area;
        let mut next = ratatui::backend::TestBackend::new(area.width, area.height);
        // Anchor the inline viewport at the **bottom** of the screen, exactly as a real terminal does
        // at launch (the cursor sits after the shell prompt). This makes `insert_before` scroll
        // committed history up off the top into native scrollback (out of the visible buffer) instead
        // of leaving it on-screen above a top-anchored viewport (ADR-0001; audit #1).
        let bottom = ratatui::layout::Position { x: 0, y: area.height.saturating_sub(1) };
        let _ = Backend::set_cursor_position(&mut next, bottom);
        next
    }
}

impl RebuildBackend for CrosstermBackend<Stdout> {
    fn rebuild(&self) -> Self {
        // The real terminal cursor is authoritative; a fresh wrapper over stdout re-reads it.
        CrosstermBackend::new(io::stdout())
    }

    fn reanchor_inline(&mut self, term_height: u16, old_height: u16, new_height: u16) {
        // `CrosstermBackend<W>: io::Write` — emit the erase + re-anchor straight to stdout, so the
        // next `rebuild()` + `Terminal::with_options` reserves the new rows without stacking chrome.
        let _ = reanchor_inline_region(self, term_height, old_height, new_height);
    }
}

/// The interactive front-end over an injectable backend.
pub struct App<B: Backend> {
    terminal: Terminal<B>,
    state: AppState,
    /// The current inline-viewport height (the live region's content height). Recomputed each
    /// [`draw`](Self::draw); the viewport is rebuilt only when it changes (audit #1).
    viewport_height: u16,
    /// Grow-only high-water mark for the live-region height WHILE a turn is active (streaming or a
    /// live `!` bash block). During a turn the viewport pins at this floor and stops tracking
    /// per-tool content churn, so the terminal is reconstructed (`resize_viewport` → `reanchor_inline`)
    /// only on GENUINE geometry changes (terminal resize, editor multi-line growth, selector/overlay/
    /// band) and the two idle↔active transitions per turn — never per completed tool. That stable
    /// height is what lets ratatui cell-diff the message churn inside a fixed viewport with no full
    /// repaint, eliminating the per-tool-call FLICKER. Reset to `0` the instant the turn goes idle so
    /// the region collapses back to the compact editor/footer (the void-fix is preserved).
    live_floor: u16,
    /// Where a spawned `/tree` navigation posts its outcome back to the run loop. Installed by
    /// [`App::install_tree_nav_channel`], which [`App::run`] calls once at startup. `None` when no
    /// run loop is present (an embedder or a test driving `execute_command` directly), in which case
    /// [`App::begin_tree_navigation`] falls back to awaiting the navigation inline — correct for a
    /// non-summarizing navigation (no model call, so no abort to deliver and nothing to keep the
    /// loop free for) and the only thing a caller without a loop can do.
    tree_nav_tx: Option<tokio::sync::mpsc::UnboundedSender<TreeNavMsg>>,
    /// Where the detached startup package-update check posts its answer — Pi's
    /// `this.checkForPackageUpdates().then((u) => u.length > 0 && this.showPackageUpdateNotification(u))`
    /// (`interactive-mode.ts:850-856`). Installed by [`App::set_package_update_channel`] before
    /// [`App::run`]; `None` (no channel wired, or the network policy declined) means the run loop
    /// grows no arm for it at all. The producer is `cyrup::update_check::spawn_package_update_check`.
    package_update_rx: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<String>>>,
    /// Where the spawned `/login` flow posts prompts, progress events and its final outcome —
    /// installed by [`App::install_login_channel`], which [`App::run`] calls once at startup (the
    /// same shape as [`Self::tree_nav_tx`]).
    ///
    /// `None` means no run loop is servicing the channel, and [`App::begin_provider_login`] then
    /// refuses to start a flow rather than spawning a task whose first `prompt` would block
    /// forever. There is no inline fallback here, unlike `/tree`'s: EVERY login flow is interactive
    /// by construction (that is what `AuthInteraction` is for), so an unattended one cannot
    /// complete.
    login_tx: Option<tokio::sync::mpsc::UnboundedSender<LoginUiMsg>>,
    /// Where [`App::login_provider_inputs`] sources the provider registry Pi reads off
    /// `modelRuntime` (`getLoginProviderOptions`, `interactive-mode.ts:4939`).
    ///
    /// Defaults to `cyrup_provider::all_providers()` — the compiled-in built-ins, which is where
    /// all 11 ported OAuth flows and every `env_key` strategy live. Overridable via
    /// [`App::set_login_provider_source`] so a test can drive the whole `/login` path against a
    /// stub provider WITHOUT reaching a real endpoint (see `tests/login_flow.rs`).
    login_providers: Option<LoginProviderSource>,
}

/// Where `/login` reads the provider registry from — Pi's `modelRuntime.getProviders()`
/// (`interactive-mode.ts:4943`). See [`App::set_login_provider_source`].
pub type LoginProviderSource =
    Arc<dyn Fn() -> Vec<Arc<dyn cyrup_provider::Provider>> + Send + Sync>;

/// A spawned `/tree` navigation's outcome, posted back to [`App::run`]'s `select!` so the summarize
/// leg never runs on the loop task (the `bash_rx` / `shortcut_status_rx` channel-back pattern).
/// Keeping it off-task is what makes Pi's Escape→`abortBranchSummary` binding deliverable at all:
/// awaited inline, the loop would service no key events for the whole provider round-trip.
#[derive(Debug)]
pub struct TreeNavMsg {
    /// The navigated-to entry id, so an aborted summarization can re-show the tree there.
    target: String,
    outcome: Result<NavigateTreeOutcome, String>,
}

impl TreeNavMsg {
    /// Pair a settled navigation with the entry it targeted. `pub` so `tests/*.rs` can hand
    /// [`App::apply_tree_nav_outcome`] a synthetic outcome (notably the abort case, which is
    /// otherwise a race to provoke) — the crate's established run-loop-only testing seam.
    pub fn new(target: impl Into<String>, outcome: Result<NavigateTreeOutcome, String>) -> Self {
        TreeNavMsg { target: target.into(), outcome }
    }
}

impl<B: Backend> App<B> {
    /// Build an app over `backend` using a **content-sized inline viewport** (R-ARCH-TUI-003,
    /// ADR-0001 #1): the live region holds only the active turn + status band + editor/selector +
    /// footer, so finished history flushes to native scrollback (`insert_before`) instead of the
    /// inline region swallowing the whole screen. No alternate screen is entered.
    pub fn new(backend: B, theme: UiTheme) -> Result<Self, TuiError> {
        let size = backend.size().map_err(|e| TuiError::Backend(e.to_string()))?;
        let state = AppState::new(theme);
        let height = live_region_height(&state, size.width, size.height.max(1));
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions { viewport: Viewport::Inline(height.max(1)) },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        // Seed `0` so the first `draw` always rebuilds the viewport bottom-anchored (the constructed
        // `Terminal` is top-anchored at the backend's initial cursor; the rebuild fixes the anchor).
        Ok(App {
            terminal,
            state,
            viewport_height: 0,
            live_floor: 0,
            tree_nav_tx: None,
            package_update_rx: None,
            login_tx: None,
            login_providers: None,
        })
    }

    /// Restore the terminal: pop keyboard flags, disable bracketed paste, leave raw mode, show
    /// cursor. Total and idempotent so an error path always leaves a usable terminal.
    ///
    /// The escape sequence itself lives in [`crate::panic_hook::restore_terminal_best_effort`] and
    /// this method is a thin delegation to it, deliberately: the panic hook runs the *same*
    /// teardown, and two hand-maintained copies would silently drift the first time
    /// [`App::into_stdout`] learned to enable a fourth mode — a drift only ever discovered by a
    /// user whose terminal was already broken. Note the release profile sets `panic = "abort"`, so
    /// no `Drop` guard can stand in for the hook (`Cargo.toml:215`).
    ///
    /// Generic over the backend rather than confined to the crossterm one it is *used* from: nothing
    /// in it is crossterm-specific (the escapes go straight to stdout; `show_cursor` is a `Backend`
    /// method), and a `CrosstermBackend<Stdout>` cannot be constructed in a test without a
    /// controlling terminal — which would leave the pairing below with no way to assert itself.
    pub fn restore(&mut self) -> Result<(), TuiError> {
        crate::panic_hook::restore_terminal_best_effort();
        // Not a second `Show`-for-its-own-sake: ratatui's `Terminal` tracks `hidden_cursor` itself
        // and its `Drop` re-emits `Show` when that flag is still set, so the flag is synced through
        // the API rather than left stale by the raw-stdout write above.
        let _ = self.terminal.show_cursor();
        Ok(())
    }

    /// The **exit** teardown: drain stdin, then [`Self::restore`] — Pi's `shutdown()`, which runs
    /// `await this.ui.terminal.drainInput(1000)` immediately before `this.stop()`
    /// (`interactive-mode.ts:3578`/`:3589` then `:3591`, both the signal and the interactive-quit
    /// branch). `crates/cyrup/src/main.rs` calls it at the single exit from the interactive loop.
    ///
    /// This is a distinct method rather than a change to [`Self::restore`] because the drain is only
    /// correct on the way out. `restore` also runs on [`App::suspend`] (Ctrl+Z) and around the
    /// external editor, where the terminal is handed to someone else and taken back — anything the
    /// user types there is theirs to keep, and discarding it would be a new bug. Pi draws the line in
    /// exactly the same place: `handleCtrlZ` calls a bare `ui.stop()` (`:3722`) and never `drainInput`.
    ///
    /// See [`crate::drain`] for what the drain protects against (buffered Kitty key-release reports
    /// and the quit keystroke itself leaking to the parent shell once raw mode is off).
    pub fn drain_and_restore(&mut self) -> Result<(), TuiError> {
        // Pi's `stop()` clears the OSC 9;4 indicator first (`interactive-mode.ts:6041-6043`), before
        // `ui.stop()` tears the terminal down. Doing it here as well as inside
        // [`crate::panic_hook::restore_terminal_best_effort`] is Pi's own two-level structure: the
        // interactive mode clears its indicator, and `ProcessTerminal.stop()` clears whatever is
        // still armed. Both are idempotent; this one additionally drops the session's own armed bit
        // so the keepalive cannot re-arm on the way out.
        self.clear_terminal_progress_on_exit();
        let _ = crate::drain::drain_stdin_before_exit();
        self.restore()
    }

    /// Write the parked OSC 9;4 transition, if any — the second half of Pi's
    /// `ui.terminal.setProgress` (`tui/src/terminal.ts:509-523`).
    pub fn flush_terminal_progress(&mut self) {
        if let Some(active) = self.state.terminal_progress.take_pending() {
            crate::write_terminal_progress(active);
        }
    }

    /// Re-send the active sequence — Pi's `setInterval(..., TERMINAL_PROGRESS_KEEPALIVE_MS)`
    /// (`terminal.ts:514-516`). Driven from the run loop's 1 s ticker, gated on
    /// [`crate::TerminalProgress::keepalive`] so an idle session never writes.
    ///
    /// Also the resume path: a Ctrl+Z suspend runs [`Self::restore`], which clears the terminal's
    /// indicator, and the next tick after `fg` puts it back for a turn that is still running.
    pub fn tick_terminal_progress_keepalive(&mut self) {
        if self.state.terminal_progress.keepalive() {
            crate::write_terminal_progress(true);
        }
    }

    /// The exit clear — Pi `stop()` (`interactive-mode.ts:6041-6043`) and `ProcessTerminal.stop()`
    /// (`terminal.ts:407-409`). Answers from the TERMINAL's armed bit, so an indicator this process
    /// lit is always taken back down even if the setting was turned off in between.
    pub fn clear_terminal_progress_on_exit(&mut self) {
        if self.state.terminal_progress.shutdown() {
            crate::write_terminal_progress(false);
        }
    }

    /// Immutable state access.
    pub fn state(&self) -> &AppState {
        &self.state
    }
    /// Mutable state access (drive the transcript/editor/status directly).
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }
    /// Install the extension-registered keyboard shortcuts (R-08-017; delegates to
    /// [`AppState::set_extension_shortcuts`]). The binary calls this at boot from
    /// `ExtensionHost::shortcut_keys()`.
    pub fn set_extension_shortcuts(
        &mut self,
        specs: impl IntoIterator<Item = impl Into<ShortcutSpec>>,
    ) {
        self.state.set_extension_shortcuts(specs);
    }

    /// Plumb the `autocompleteMaxVisible` setting (Pi, item #6) into the editor's autocomplete popup
    /// (clamped 3–20). The binary calls this from `settings.autocompleteMaxVisible` at boot.
    pub fn set_autocomplete_max_visible(&mut self, n: u16) {
        self.state.editor.set_autocomplete_max_visible(n);
    }

    /// Whether the idle 2-row status band is reserved (kept present) to avoid an editor/footer reflow
    /// when a spinner appears (item #9). Plumbed from Pi's `terminal.clearOnShrink` setting
    /// (interactive-mode.ts:1638-1642: an idle status container is cleared only when clearOnShrink is
    /// off — so `reserve_status_rows == clearOnShrink`). Default `false` matches Pi's default.
    pub fn set_reserve_status_rows(&mut self, reserve: bool) {
        self.state.reserve_status_rows = reserve;
    }

    /// Load a user `keybindings.json` document and merge it into every live keymap (R-10-018; Pi
    /// `KeybindingsManager.create`, keybindings.ts:348-352). Each map's `merge_json` applies only the
    /// ids in its own namespace (`app.*` / `editor.*` / `tui.select.*` / `app.tree.*`) and ignores the
    /// rest, so one document configures the global, editor, selector and tree maps in a single pass.
    /// A malformed document or key spec is surfaced as a typed error (the binary logs + continues with
    /// the defaults) — never a panic.
    pub fn load_keybindings_json(&mut self, json: &str) -> Result<(), TuiError> {
        self.state.keymap.merge_json(json)?;
        // X9 — every `… to expand` hint resolves its key label through the LIVE keymap upstream
        // (`keyText("app.tools.expand")`, `keybinding-hints.ts:34-36`). The transcript holds no
        // keymap, so the resolved label is pushed to it whenever bindings change.
        let expand = self.state.keymap.keys_label(Action::ToolsExpand);
        self.state.transcript.set_expand_hint(expand);
        self.state.select_keymap.merge_json(json)?;
        self.state.tree_keymap.merge_json(json)?;
        self.state.session_keymap.merge_json(json)?;
        self.state.models_keymap.merge_json(json)?;
        self.state.editor.merge_keybindings_json(json)?;
        Ok(())
    }
    /// The transcript view.
    pub fn transcript_mut(&mut self) -> &mut TranscriptView {
        &mut self.state.transcript
    }
    /// The input editor.
    pub fn editor_mut(&mut self) -> &mut InputEditor {
        &mut self.state.editor
    }
    /// The status line.
    pub fn status_mut(&mut self) -> &mut StatusLine {
        &mut self.state.status
    }

    /// Point the footer's git-branch source at `cwd` and publish the branch it finds — Pi's
    /// `new FooterDataProvider(cwd)` followed by the footer's `getGitBranch()`
    /// (`footer-data-provider.ts`, consumed at `footer.ts:116-120`).
    ///
    /// This is the ONLY producer of [`StatusLine::branch`] in the binary: without it the `(branch)`
    /// segment of the location line can never appear, because nothing else resolves a git HEAD.
    /// Called once from the bin's footer seeding, before the first frame.
    pub fn set_footer_git_cwd(&mut self, cwd: &std::path::Path) {
        self.state.git_branch = crate::footer_data::FooterGitBranch::discover(cwd);
        let branch = self.state.git_branch.branch().map(str::to_string);
        self.state.status.set_branch(branch);
    }

    /// Install the channel the detached startup package-update check answers on — Pi fires that
    /// check from `run()` and shows the notification whenever it settles
    /// (`interactive-mode.ts:850-861`, `:3920-3936`).
    ///
    /// Must be called before [`App::run`]; the binary passes the receiver
    /// `cyrup::update_check::spawn_package_update_check` returns, which is `None` when the
    /// [`NetworkPolicy`](cyrup_config::policy::NetworkPolicy) declined — and then no arm exists.
    pub fn set_package_update_channel(
        &mut self,
        rx: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<String>>>,
    ) {
        self.package_update_rx = rx;
    }

    /// Re-check the git refs and republish the branch when it moved — Pi's watch-driven
    /// `refreshGitBranchAsync` → `notifyBranchChange` (`footer-data-provider.ts`), driven here by
    /// [`App::run`]'s poll tick. Returns `true` when the footer needs a repaint.
    pub fn poll_footer_git_branch(&mut self) -> bool {
        if !self.state.git_branch.poll() {
            return false;
        }
        let branch = self.state.git_branch.branch().map(str::to_string);
        self.state.status.set_branch(branch);
        true
    }
    /// The terminal (test access to the rendered buffer via `terminal.backend()`).
    pub fn terminal(&self) -> &Terminal<B> {
        &self.terminal
    }

    /// The committed scrollback lines already emitted via `insert_before` (test/inspection access).
    pub fn scrollback_lines(&self) -> &[Line<'static>] {
        &self.state.scrollback
    }

    /// The current inline-viewport (live-region) height in rows — the bottom band of the screen the
    /// app repaints each frame (ADR-0001 #1). Committed history scrolls *above* this band into native
    /// scrollback; tests use it to read only the live region (the bottom `viewport_height` rows).
    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    /// The committed scrollback content as text, one entry per line (test/inspection access). This is
    /// the exact payload `Terminal::insert_before` received, so tests can assert finalized turns
    /// reached native scrollback without driving a real terminal.
    pub fn scrollback_text(&self) -> String {
        self.state.scrollback.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    /// Attach a decoded image to the next prompt (rendered inline above the editor, spec/tui/06 §6;
    /// `components/image.ts`). The `@`-mention of an image file and clipboard-image paste both land here.
    pub fn attach_image(&mut self, image: ImageBlock) {
        self.state.pending_images.push(image);
    }

    /// Attach an image file by path (the `@`-mention image source); a no-op (returns `false`) when the
    /// path is not a decodable image, so a stray mention never disrupts the prompt.
    pub fn attach_image_path(&mut self, path: &std::path::Path) -> bool {
        match ImageBlock::from_path(path) {
            Some(block) => {
                self.state.pending_images.push(block);
                true
            }
            None => false,
        }
    }

    /// Insert the temp-file PATH of a pasted clipboard image at the editor cursor as ordinary text —
    /// Pi's literal mechanism (`this.editor.insertTextAtCursor(filePath)`,
    /// interactive-mode.ts:2552). The bare path becomes editable text and, on submit, rides the
    /// outgoing user message AS TEXT (no image content block): the agent loads the raster on demand
    /// via a file-read tool, so a potentially huge image never floods context — Pi's deliberate
    /// context-economy choice, which the former `pending_images` embed here violated. Kept separate
    /// from the clipboard read so the path→editor step is unit-testable without a live system
    /// clipboard (`try_paste_clipboard_image_path` supplies the path in the binary).
    fn insert_clipboard_image_path(&mut self, path: &std::path::Path) {
        self.state.editor.insert_str(&path.to_string_lossy());
    }

    /// Read a system-clipboard image, materialize it to a `cyrup-clipboard-<uuid>.png` temp file, and
    /// insert its PATH as text at the editor cursor (Pi `handleClipboardImagePaste`,
    /// interactive-mode.ts:2537-2557). Returns `true` when an image was found and its path pasted;
    /// `false` when the clipboard holds no image (Pi `clipboard.hasImage()` gate) or on any
    /// clipboard/encode/IO error — so the caller lets Ctrl+V fall through to the editor, preserving
    /// normal text-paste behavior.
    fn try_paste_clipboard_image_path(&mut self) -> bool {
        match read_clipboard_image_to_temp() {
            Some(path) => {
                self.insert_clipboard_image_path(&path);
                true
            }
            None => false,
        }
    }

    /// Clear all attached images (after the prompt is sent, or on `Esc`).
    pub fn clear_images(&mut self) {
        self.state.pending_images.clear();
    }

    /// The images attached to the next prompt (test/inspection access).
    pub fn pending_images(&self) -> &[ImageBlock] {
        &self.state.pending_images
    }

    /// Env-sniff the controlling terminal's capabilities (feature #7; Pi `detectCapabilities`) and
    /// upgrade the portable half-block default to the negotiated image protocol (Kitty/iTerm2), while
    /// caching the resolved [`TerminalCapabilities`] so the OSC-8 hyperlink gate (feature #8) can read
    /// them. Called by the binary at startup; tests keep the half-block default (the inline path still
    /// renders to `TestBackend`).
    pub fn detect_image_support(&mut self) {
        let caps = crate::image::detect_capabilities();
        self.state.capabilities = caps;
        // Seed the process-wide OSC-8 answer the markdown renderer reads (Pi's cached
        // `getCapabilities()`, terminal-image.ts:138-143) so the link gate at `markdown.ts:692`
        // sees the same detection this call already paid for.
        crate::image::seed_hyperlink_support(caps.hyperlinks);
        // …and, when the terminal HAS an image protocol, measure its font cell instead of guessing
        // it (Pi `queryCellSize`, `tui.ts:647`/`:679-686`, gated on `getCapabilities().images` at
        // `:681`). Without this every inline image is laid out against `ratatui-image`'s `10x20`
        // placeholder cell, so a Kitty/iTerm2 image that is not width-clamped reserves the wrong
        // number of rows and is drawn at the wrong scale.
        //
        // Called by the binary from the SAME pre-reader-thread window as the theme probe (see
        // `crate::terminal_query`'s module docs for the timeout / input-safety contract); off a real
        // terminal `stdin_is_queryable` short-circuits it to `None` in microseconds, which is what
        // keeps this callable from tests.
        let cell_size = if caps.images.is_some() {
            use crate::terminal_query::TerminalProbe as _;
            crate::terminal_query::StdinTerminalProbe
                .query_cell_size(crate::terminal_query::CELL_SIZE_TIMEOUT)
        } else {
            None
        };
        self.state.image_renderer = ImageRenderer::from_capabilities_with_cell_size(caps, cell_size);
    }

    /// Apply a new theme, bumping its generation so caches invalidate (R-10-026). The theme is
    /// re-projected through the app's live [`ColorMode`] (feature #3/#4) so a `/theme` switch or hot
    /// reload on a 256-color terminal keeps indexed colors (`with_color_mode` is idempotent for an
    /// already-projected theme).
    pub fn set_theme(&mut self, theme: UiTheme) {
        let mut theme = theme.with_color_mode(self.state.color_mode);
        theme.generation = self.state.theme.generation.saturating_add(1);
        self.state.theme = theme;
    }

    /// Boot the render theme from a [`ThemeController`] (feature #4): adopt the controller's resolved
    /// color mode and set the projected theme. This is the seam the binary uses to honor
    /// `settings.theme` + the terminal background at startup instead of the hardwired dark boot.
    pub fn apply_theme_controller(&mut self, controller: &ThemeController) {
        self.state.color_mode = controller.color_mode();
        let mut theme = controller.theme();
        theme.generation = self.state.theme.generation.saturating_add(1);
        self.state.theme = theme;
    }

    /// The app's active color mode (test/inspection).
    pub fn color_mode(&self) -> ColorMode {
        self.state.color_mode
    }

    /// Point the automatic terminal title at the live session's working directory — Pi's
    /// `sessionManager.getCwd()` (`interactive-mode.ts:819`). Does NOT write anything on its own;
    /// [`Self::update_terminal_title`] is what recomputes the title.
    pub fn set_title_cwd(&mut self, cwd: PathBuf) {
        // X7 — the same value Pi hands the tool renderers as `ToolRenderContext.cwd`
        // (`tool-execution.ts:126`), which `read`'s compact classification resolves against.
        self.state.transcript.set_cwd(Some(cwd.clone()));
        self.state.title_cwd = cwd;
    }

    /// Recompute the automatic window title from the session name + cwd — Pi `updateTerminalTitle`
    /// (`interactive-mode.ts:818-826`) — and store it on [`AppState::terminal_title`].
    ///
    /// Returns the new title **only when it changed**, so a caller writes the OSC 0 sequence no more
    /// often than Pi calls `setTitle`. Pi's four call sites are startup (`:860`), a session
    /// (re-)bind (`:1761`), unbinding the extension set (`:1995`) and `session_info_changed`
    /// (`:2901`); [`App::run`] drives the first, second and fourth — the third has no cyrup
    /// counterpart, since extension chrome here is not torn down per session. Never per stream
    /// event. The write itself is the crossterm run loop's job
    /// ([`write_terminal_title`]), for the same reason the extension `SetTitle` effect is written
    /// there: a `TestBackend` app must not emit escape sequences onto the real stdout.
    ///
    /// The session name is read from the footer's [`StatusLine::session_name`], which is where the
    /// live value already lands (Pi reads the same value the footer does, `footer.ts:116-130`).
    pub fn update_terminal_title(&mut self) -> Option<String> {
        let title = session_terminal_title(
            self.state.status.session_name.as_deref(),
            &self.state.title_cwd,
        );
        if self.state.terminal_title.as_deref() == Some(title.as_str()) {
            return None;
        }
        self.state.terminal_title = Some(title.clone());
        Some(title)
    }

    /// Re-bind the UI to a freshly-installed runtime session (arch-11 §3.4 replacement; Pi's
    /// interactive session-swap). Called by the run loop on a generation bump (a `/new`/`/resume`/
    /// `/fork`/`/reload`/`/import` op or a runtime-side `SessionReplaced`): the run loop has already
    /// dropped the stale subscription and re-subscribed the new session's `AgentSessionEvent` stream;
    /// here we reset the per-session UI state (the transcript, the streaming/indicator status, any
    /// open selector/overlay) for the new session and surface the swap status line. Committed
    /// scrollback already lives in the terminal's native history (`insert_before`) and is preserved.
    /// Tear down every EXTENSION-owned UI surface before the old session is invalidated — pi
    /// `resetExtensionUI` (`interactive-mode.ts:1974-2003`), registered on the runtime via
    /// `setBeforeSessionInvalidate` (`:452`).
    ///
    /// The ordering is the point, and it is why this cannot be folded into
    /// [`Self::rebind_session`]. pi's runtime fires `abort → session_shutdown →
    /// beforeSessionInvalidate → dispose` (`agent-session-runtime.ts:167-177`), so this runs while
    /// the OLD session is still alive and its extension host still answerable. `rebind_session`
    /// runs AFTER the swap and resets session-owned surfaces (transcript, selector, overlays,
    /// status flags); an extension header, footer, widget, status row or shortcut binding left
    /// behind by the outgoing session's extensions would otherwise survive into the new one and
    /// keep rendering, owned by a host that no longer exists.
    pub fn reset_extension_ui(&mut self) {
        self.state.extension_header = None;
        self.state.extension_footer = None;
        self.state.extension_widget = None;
        self.state.extension_shortcuts.clear();
        self.state.status.extension_statuses.clear();
        // An extension dialog/editor overlay belongs to the outgoing host; leaving it up would
        // present a prompt whose reply channel is about to be dropped.
        self.state.overlays.clear();
        self.state.selector = None;
    }

    pub fn rebind_session(&mut self) {
        // Extension-owned surfaces first (pi `resetExtensionUI`, `interactive-mode.ts:1974-2003`).
        //
        // pi registers this on the runtime's `beforeSessionInvalidate` so it runs while the OLD
        // session is still alive. cyrup calls it here instead, and the difference is safe for a
        // specific reason: pi's hook is positioned early because a JS closure over `this` can also
        // reach into `oldSession.extensionRunner` (its own ordering test asserts exactly that),
        // whereas this function touches NOTHING but local UI state. There is no old-host resource
        // to race. The Rust hook cannot capture `&mut App` in an `Arc<dyn Fn()>` anyway — see
        // `AgentSessionRuntime::set_before_session_invalidate`, which exists as a library surface
        // for embedders that need the earlier position.
        self.reset_extension_ui();
        self.state.transcript = TranscriptView::new();
        self.state.selector = None;
        self.state.overlays.clear();
        self.state.status.set_streaming(false);
        self.state.status.set_queued(0);
        self.state.indicator.idle();
        // The new session starts idle, so drop the prior turn's grow-only height floor; the next
        // `draw` collapses the viewport to the compact idle region (void-fix).
        self.live_floor = 0;
        let msg = self.state.pending_swap_status.take().unwrap_or_else(|| "session replaced".into());
        self.state.transcript.push_status(msg);
    }

    /// Seed the transcript from a session's persisted conversation — Pi's `renderInitialMessages()`
    /// → `renderSessionEntries(buildContextEntries(), {updateFooter, populateHistory})`
    /// (interactive-mode.ts:3548-3562) and the `rebuildChatFromMessages()` used after a compaction
    /// or a tree/fork navigation (`:3599-3601`, `:1737-1742`).
    ///
    /// Without this a `/resume`, `/fork`, `/import`, `--resume` or `--continue` shows an EMPTY view
    /// even though the session file holds the whole conversation, because
    /// [`rebind_session`](Self::rebind_session) starts the new session from a fresh
    /// [`TranscriptView`].
    ///
    /// **Feed it [`AgentSession::raw_context_messages`], never `AgentSession::messages()`.** The
    /// latter is the LLM boundary (`convertToLlm`, `messages.ts:148-195`): it has already rendered a
    /// compaction summary, a branch summary, an extension `custom` message and a `!` bash execution
    /// down to `user` messages carrying wrapper prose ("The conversation history before this point
    /// was compacted into the following summary: …"), which would replay as the *user* having typed
    /// that text — and would seed it into the editor's Up-arrow history. Pi feeds the RAW projection
    /// for exactly this reason: `renderSessionEntries` maps entries through
    /// `sessionEntryToContextMessages` (interactive-mode.ts:3506-3516) whose roles are still
    /// `compactionSummary`/`branchSummary`/`custom`/`bashExecution`, and `addMessageToChat`
    /// (`:3308-3350`) routes each to its own component.
    ///
    /// The port follows Pi's `renderSessionItems` walk (`:3415-3497`) + `addMessageToChat`
    /// (`:3308-3413`):
    /// * `user` → the user block (a `<skill …>` submission still splits into its `[skill]`
    ///   invocation + the trailing message, via [`TranscriptView::push_user`]) and, like Pi's
    ///   `populateHistory`, the prompt is pushed into the editor's Up-arrow history;
    /// * `assistant` → the reasoning section, the answer markdown, a live tool block per `toolCall`
    ///   content, and the not-finished-cleanly notice ([`stop_reason_notice`]);
    /// * `toolResult` → attached to the matching open tool block by tool name, then the finished
    ///   leading run is committed so tools land between the assistant turns that bracket them
    ///   rather than all at the end;
    /// * `bashExecution` → a committed bash block (`BashExecutionComponent`, `:3310-3322`),
    ///   dim-bordered for a `!!` (`excludeFromContext`) run;
    /// * `custom` → the labeled extension block, **only when `display`** (`:3323-3336`);
    /// * `compactionSummary` / `branchSummary` → their own summary blocks (`:3337-3350`).
    ///
    /// **Divergence from pi — UNPORTED (the `ADR-0001` it once cited does not exist; see CLAUDE.md)**: Pi calls `chatContainer.clear()` before replaying, which
    /// wipes the previous session off the screen. cyrup's committed entries live in the terminal's
    /// native scrollback (`insert_before`) and cannot be erased, so after a mid-session `/resume`
    /// the previous conversation stays visible ABOVE the replayed one. The replay itself needs no
    /// re-render: it starts from an empty transcript and flushes forward normally.
    ///
    /// X11 — this is the NO-EXTENSIONS shorthand. Pi resolves an extension's registered message
    /// renderer on the replay walk too (`const renderer = this.session.extensionRunner
    /// .getMessageRenderer(message.customType)`, `interactive-mode.ts:3471`, inside the same
    /// `case "custom"` the `display` gate at `:3470` guards), exactly as it does on the live
    /// `addMessageToChat` path. Call [`Self::replay_session_with_extensions`] wherever a host is in
    /// hand — every production `/resume`, `/fork`, `/import` and `--continue` does — or a resumed
    /// session silently loses extension rendering that the live session had.
    pub fn replay_session(&mut self, messages: &[cyrup_session_svc::agent_message::AgentMessage]) {
        self.replay_session_rendered(messages, &std::collections::HashMap::new());
    }

    /// [`Self::replay_session`], first resolving each displayed `custom` message's registered
    /// extension renderer (EXT-006; Pi `getMessageRenderer(message.customType)`,
    /// `interactive-mode.ts:3471`).
    ///
    /// The renderer lookup is an async guest call with a timeout while the replay walk is sync, so
    /// — exactly like [`Self::ingest_event_with_extensions`] on the live path — every renderer runs
    /// FIRST and its text rides into the walk, keyed by the message's index.
    pub async fn replay_session_with_extensions(
        &mut self,
        messages: &[cyrup_session_svc::agent_message::AgentMessage],
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        use cyrup_session_svc::agent_message::AgentMessage;
        let mut rendered: std::collections::HashMap<usize, crate::transcript::Rendered> =
            std::collections::HashMap::new();
        for (i, message) in messages.iter().enumerate() {
            // `if (message.display)` (`:3470`) gates the whole arm, lookup included.
            let AgentMessage::Custom(c) = message else { continue };
            if !c.display {
                continue;
            }
            let payload = serde_json::to_value(message).unwrap_or(serde_json::Value::Null);
            if let Some(text) =
                extension_render_message(ext_host, &c.custom_type, &payload).await
            {
                rendered.insert(i, crate::transcript::Rendered::Text(text));
            }
        }
        self.replay_session_rendered(messages, &rendered);
    }

    /// The replay walk itself. `rendered` maps a message INDEX to the text an extension's registered
    /// renderer produced for it (X11); an absent entry draws the built-in framing, which is Pi's
    /// `getMessageRenderer(...) === undefined` outcome.
    fn replay_session_rendered(
        &mut self,
        messages: &[cyrup_session_svc::agent_message::AgentMessage],
        rendered: &std::collections::HashMap<usize, crate::transcript::Rendered>,
    ) {
        use cyrup_core::{Content, Message};
        use cyrup_session_svc::agent_message::AgentMessage;
        use serde_json::Value;
        for (index, message) in messages.iter().enumerate() {
            match message {
                AgentMessage::Core(Message::User { content, .. }) => {
                    let text = content_text(content);
                    if text.trim().is_empty() {
                        continue;
                    }
                    self.state.transcript.push_user(text.clone());
                    // Pi `populateHistory` (interactive-mode.ts:3387): replayed prompts are
                    // recallable with Up, so a resumed session can re-run its own last message.
                    self.state.editor.push_history(&text);
                }
                AgentMessage::Core(Message::Assistant(m)) => {
                    let thinking = thinking_text(&m.content);
                    if !thinking.is_empty() {
                        self.state.transcript.commit_thinking(Some(thinking));
                    }
                    let text = content_text(&m.content);
                    if !text.trim().is_empty() {
                        self.state.transcript.commit_assistant(Some(text));
                    }
                    for call in m.content.iter().filter_map(|c| match c {
                        Content::ToolCall(call) => Some(call),
                        _ => None,
                    }) {
                        // Pi files each replayed call component under `content.id`
                        // (`renderedPendingTools.set(content.id, component)`,
                        // interactive-mode.ts:3473) so the `toolResult` below resolves to the exact
                        // call that produced it — two `read`s in one turn are indistinguishable by
                        // name.
                        self.state.transcript.push_tool_start_rendered(
                            call.name.clone(),
                            Some(call.id.as_str().to_string()),
                            Value::Object(call.arguments.clone()),
                            None,
                        );
                    }
                    if let Some(notice) = stop_reason_notice(m) {
                        self.state.transcript.push_error(notice);
                    }
                }
                AgentMessage::Core(Message::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    is_error,
                    details,
                    ..
                }) => {
                    // The shape every per-tool `renderResult` reads (`{content, details}`).
                    let mut result = serde_json::Map::new();
                    result.insert(
                        "content".to_string(),
                        serde_json::to_value(content).unwrap_or(Value::Null),
                    );
                    if let Some(d) = details {
                        result.insert("details".to_string(), d.clone());
                    }
                    // `renderedPendingTools.get(message.toolCallId)` (`:3483`) — an exact id lookup,
                    // never a name scan.
                    self.state.transcript.push_tool_end_rendered(
                        tool_name.clone(),
                        Some(tool_call_id.as_str()),
                        *is_error,
                        Some(Value::Object(result)),
                        None,
                    );
                    // Keep call order in scrollback: commit the finished leading run now instead of
                    // deferring every tool of the whole replay to the end.
                    self.state.transcript.commit_finished_leading_tools();
                }
                AgentMessage::BashExecution(b) => {
                    self.state.transcript.push_bash_execution(
                        b.command.clone(),
                        b.exclude_from_context.unwrap_or(false),
                        &b.output,
                        b.exit_code.and_then(|c| i32::try_from(c).ok()),
                        b.cancelled,
                        // X13 — upstream replays both (`interactive-mode.ts:3460-3465`
                        // `message.truncated ? {truncated:true} : undefined, message.fullOutputPath`),
                        // which is what puts the `Output truncated. Full output: …` row back on a
                        // resumed session's `!` block.
                        b.truncated,
                        b.full_output_path.clone(),
                    );
                }
                AgentMessage::Custom(c) => {
                    // Pi renders a custom message only when it opted into display
                    // (`if (message.display)`, interactive-mode.ts:3470).
                    if c.display {
                        // X11 — `const renderer = this.session.extensionRunner.getMessageRenderer(
                        // message.customType); new CustomMessageComponent(message, renderer, …)`
                        // (`:3471-3477`). The replay arm is NOT a thinner variant of the live one:
                        // it performs the identical lookup, so a resumed session keeps the
                        // extension rendering the live session had. Absent an entry the built-in
                        // `[type] body` framing draws — `getMessageRenderer` returning `undefined`.
                        let rendered = rendered.get(&index).cloned().unwrap_or_default();
                        self.state.transcript.push_custom_message_rendered(
                            c.custom_type.clone(),
                            custom_message_text(&c.content),
                            rendered,
                        );
                    }
                }
                AgentMessage::BranchSummary(b) => {
                    self.state.transcript.push_branch_summary(b.summary.clone());
                }
                AgentMessage::CompactionSummary(c) => {
                    self.state
                        .transcript
                        .push_compaction_summary(c.tokens_before, c.summary.clone());
                }
            }
        }
        // A tool call whose result never persisted (an interrupted turn) still commits, as-is.
        self.state.transcript.commit_tools();
    }

    /// Emit the startup loaded-resources / diagnostics panel (Pi `showLoadedResources`,
    /// interactive-mode.ts:1480-1690, called with `{force: false, showDiagnosticsWhenQuiet: true}`
    /// at `:1769`).
    ///
    /// TUI-006: without this, extension load failures, shadowed skills and missing prompt paths were
    /// entirely invisible in cyrup — the data existed (`AgentSessionServices::startup_diagnostics`)
    /// but nothing rendered it. Push it before the first draw so it lands at the top of scrollback,
    /// ahead of the conversation.
    pub fn push_loaded_resources(&mut self, report: &crate::startup::StartupReport) {
        self.state.transcript.push_loaded_resources(crate::startup::build_startup_lines(report));
    }

    /// Put already-queued steering/follow-up text back into the editor — the buffer half of Pi's
    /// `restoreQueuedMessagesToEditor` (interactive-mode.ts:4064-4083). `queued` is
    /// `[...steering, ...followUp]` **already drained** from the session (Pi's `clearAllQueues()`
    /// at `:4065`, here [`AgentSession::drain_queue`]); this half is pure, so the run loop owns the
    /// async drain and the abort and the App owns what the user sees.
    ///
    /// The queued messages join with a blank line and are PREPENDED to whatever is already typed,
    /// with empty parts dropped (`:4074-4077` — `[queuedText, currentText].filter(t => t.trim())`).
    /// An empty queue leaves the editor untouched and returns `0`, which is how
    /// [`AppAction::Dequeue`] decides between Pi's two `handleDequeue` statuses (`:3834-3841`).
    /// The Esc path (`{abort: true}`) shows no status at all — Pi's escape branch never calls
    /// `showStatus`.
    pub fn restore_queued_to_editor(&mut self, queued: &[String]) -> usize {
        if queued.is_empty() {
            return 0;
        }
        let queued_text = queued.join("\n\n");
        let current = self.state.editor.text();
        let combined = [queued_text, current]
            .into_iter()
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.state.editor.set_text(&combined);
        queued.len()
    }

    /// Render one frame: first flush newly-committed entries to native scrollback (R-ARCH-TUI-003),
    /// then draw the active region into the inline viewport (pure: `state -> frame`).
    pub fn draw(&mut self) -> Result<(), TuiError>
    where
        B: RebuildBackend,
    {
        // Content-size the inline viewport to the live region (active turn + band + slot + footer),
        // recomputed every frame as content grows/shrinks (ADR-0001 #1, audit #1). The viewport is
        // rebuilt only when its height actually changes so steady-state frames keep their cell-diff.
        // Resize **before** flushing so the committed `insert_before` lines scroll above the
        // correctly-anchored viewport (the active turn's height is unaffected by the flush).
        let size = self.terminal.backend().size().ok();
        let term_h = size.map(|s| s.height).unwrap_or(self.viewport_height).max(1);
        let term_w = size.map(|s| s.width).unwrap_or(80);
        // Publish the SCREEN height before anything measures: the editor's row budget is
        // `max(5, floor(terminalRows * 0.3))` against the terminal, not the live region
        // (`editor.ts:499-501`; see [`AppState::term_rows`]). A selector that windows its own body
        // gets the same number through `Selector::set_terminal_height`, which is documented as
        // "called before `desired_height` on every frame" and, until now, was called only by the
        // standalone `startup_selector` loop — so the in-app `/config` grid and the `ui.editor`
        // dialog (E12) both sized themselves against a default they were never told to update.
        self.state.term_rows = term_h;
        // E17: the editor caps ITSELF at `max(5, floor(terminalRows * 0.3))` from inside
        // `render` (`editor.ts:499-501`), so it needs the screen height too — `region_constraints`
        // reserving the right number of rows is not the same thing as the component knowing its own
        // budget.
        self.state.editor.set_terminal_height(term_h);
        if let Some(active) = self.state.selector.as_mut() {
            active.inner.set_terminal_height(term_h);
        }
        let raw = live_region_height(&self.state, term_w, term_h);
        // Grow-only hysteresis GATED on the turn being active. `status.streaming` is set on
        // `AgentStart` and cleared on `AgentEnd`, so it spans the WHOLE multi-step turn including the
        // gaps between tools (it is NOT `transcript.has_active()`, which flickers false between tools
        // and would re-trigger per-tool reconstruction); `has_bash()` covers a live `!`/`!!` run.
        // While active, the viewport pins at its high-water (capped to the terminal height so a
        // resize-shrink still reduces it) and stops tracking per-tool content churn — so
        // `resize_viewport`/`reanchor_inline` fire only on genuine geometry changes, killing the
        // per-tool FLICKER. Idle: drop the floor and size to the live content so the region collapses
        // to the compact editor/footer (void-fix).
        let turn_active = self.state.status.streaming || self.state.transcript.has_bash();
        let desired = if turn_active {
            self.live_floor = self.live_floor.max(raw).min(term_h);
            self.live_floor
        } else {
            self.live_floor = 0;
            raw
        };
        if desired != self.viewport_height {
            self.resize_viewport(desired)?;
            self.viewport_height = desired;
        }
        self.flush_committed()?;
        let App { terminal, state, .. } = self;
        terminal
            .draw(|frame| render(frame, state))
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }

    /// Rebuild the terminal with a new inline-viewport `height` over a fresh handle to the same
    /// backend (ratatui's inline height is immutable after construction; audit #1). The cursor anchor
    /// is preserved by [`RebuildBackend::rebuild`], so the re-placed viewport stays where it was.
    fn resize_viewport(&mut self, height: u16) -> Result<(), TuiError>
    where
        B: RebuildBackend,
    {
        // Erase the CURRENT inline region and re-anchor the cursor BEFORE reconstructing at the new
        // height, so the reservation's `append_lines` scrolls BLANKS rather than the prior frame's
        // chrome. On a real terminal this is the whole difference between a clean regrow and the
        // hint-bar/editor-rule/footer STACKING the audit hit; a no-op on fresh-grid backends
        // (`TestBackend`), which start each `rebuild` from a blank buffer and can never stack.
        let size = self.terminal.backend().size().ok();
        let term_h = size.map(|s| s.height).unwrap_or(height).max(1);
        let old_h = self.viewport_height;
        self.terminal.backend_mut().reanchor_inline(term_h, old_h, height);

        let backend = self.terminal.backend().rebuild();
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions { viewport: Viewport::Inline(height.max(1)) },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        self.terminal = terminal;
        Ok(())
    }

    /// Move every newly-committed transcript entry into native scrollback via `Terminal::insert_before`
    /// **exactly once** (R-ARCH-TUI-003 / R-10-002), recording the same lines in the test-visible
    /// `scrollback` accumulator. After this the inline viewport only renders the active streaming turn,
    /// the editor, and the status line. A no-op when nothing was committed since the last flush.
    fn flush_committed(&mut self) -> Result<(), TuiError> {
        let committed = self.state.transcript.drain_committed();
        if committed.is_empty() {
            return Ok(());
        }
        // Content width for markdown wrapping: the live terminal width (R-ARCH-TUI-005), fallback 80.
        let width = self.terminal.backend().size().map(|s| s.width as usize).unwrap_or(80);
        let output_pad = self.state.transcript.output_pad();
        // Committed tool-result images keep rendering — a half-block raster is ordinary cells, so it
        // survives `insert_before` into native scrollback (see `ImageBlock::halfblock_lines`).
        let images = crate::transcript::ImageOpts {
            show: self.state.transcript.show_images(),
            width_cells: self.state.transcript.image_width_cells(),
            // X9/X7 — the same live `app.tools.expand` label and session cwd the in-viewport render
            // uses, so a committed block's hints and compact `read` header do not disagree with the
            // live one they were just scrolled up from.
            expand_key: self.state.transcript.expand_key(),
            cwd: self.state.transcript.cwd(),
            // X14 — the LIVE `this.toolOutputExpanded`. Upstream never freezes an expansion onto a
            // message: `setToolsExpanded` walks `chatContainer.children` and re-broadcasts to every
            // expandable child (`interactive-mode.ts:4032-4046`), so a branch/compaction summary
            // pushed while collapsed still opens when `Ctrl+O` is pressed before it paints.
            tools_expanded: self.state.transcript.tool_expanded(),
        };
        let lines: Vec<Line<'static>> = committed
            .iter()
            .flat_map(|e| entry_lines(e, &self.state.theme, width, output_pad, images))
            .collect();
        self.state.scrollback.extend(lines.iter().cloned());
        let style = self.state.theme.base_style();
        // Size the scrollback slot to the WRAPPED display-row count (not `lines.len()`) and render
        // WITH `.wrap()`: `entry_lines` emits one un-wrapped `Line` per prose paragraph, so a long
        // committed answer must wrap to width and reserve its wrapped height — otherwise
        // `insert_before` clips it to a single row and the full text is lost from native scrollback
        // (the PROSE-WRAP truncation; R-ARCH-TUI-003/-005, spec/tui/01 §3 overflow).
        let height = crate::transcript::wrapped_height(&lines, width).min(u16::MAX as usize) as u16;
        self.terminal
            .insert_before(height, move |buf| {
                Paragraph::new(lines).style(style).wrap(Wrap { trim: false }).render(buf.area, buf);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        Ok(())
    }

    /// Map one input event to an [`AppAction`], mutating editor/transcript as needed. Global keys
    /// (resolved via the configurable [`Keymap`], R-10-018) take precedence over editing keys.
    pub fn handle_input(&mut self, ev: &InputEvent) -> AppAction {
        match ev {
            InputEvent::Key(key) => {
                if matches!(key.kind, KeyEventKind::Release) {
                    return AppAction::None;
                }
                // A floating overlay (hotkeys/help popup) captures input first (spec/tui/05 §2
                // routing step 2): the topmost overlay handles the key; `Close` pops it, an unhandled
                // key is swallowed so it never leaks to the editor/agent beneath the modal.
                if !self.state.overlays.is_empty() {
                    return self.handle_overlay_key(key);
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
                    // `app.clipboard.pasteImage` (Ctrl+V): read a system-clipboard image and insert its
                    // temp-file PATH as text at the editor cursor (Pi `handleClipboardImagePaste` →
                    // `insertTextAtCursor(filePath)`, interactive-mode.ts:2537-2557). Gated on an image
                    // actually being present (Pi `clipboard.hasImage()`): when the clipboard holds no
                    // image the key is NOT swallowed — it falls through to the editor below so normal
                    // Ctrl+V behavior is preserved (do not break text paste).
                    if action == Action::ClipboardPasteImage {
                        if self.try_paste_clipboard_image_path() {
                            return AppAction::Redraw;
                        }
                        // No image on the clipboard: fall through to the editor (text) handling below.
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
                // A selector owns the slot: pure-list selectors ignore pastes (no embedded Input yet).
                if self.state.selector.is_some() {
                    return AppAction::None;
                }
                // Route bracketed paste through `handle_paste` so large pastes collapse to an atomic
                // `[paste #N …]` marker (spec/tui/03 §5.5); small pastes insert verbatim.
                self.state.editor.handle_paste(s);
                AppAction::Redraw
            }
            InputEvent::Resize(_, _) => AppAction::Redraw,
            InputEvent::FocusGained => {
                self.state.editor.set_focused(true);
                AppAction::Redraw
            }
            InputEvent::FocusLost => {
                self.state.editor.set_focused(false);
                AppAction::Redraw
            }
        }
    }

    /// Classify a submitted line via the [`CommandRegistry`] and route it (spec/tui/04 §2.3).
    ///
    /// A plain prompt is echoed into the transcript and returned as [`AppAction::Submit`] for the run
    /// loop to deliver to the runtime. A recognized slash command or a `!`/`!!` bash invocation is
    /// surfaced as a status line for now — opening the bound overlay / executing bash is wired as the
    /// selector + bash-execution subsystems land (tracked on the residual ledger). This keeps the
    /// editor → dispatch path real and faithful (commands never reach the agent as literal text).
    fn dispatch_submission(&mut self, text: &str) -> AppAction {
        let dispatch = self.state.commands.dispatch(text);
        // The startup hint bar is a first-run affordance; any real submission dismisses it
        // (Pi drops `compactInstructions` once the conversation begins).
        if !matches!(dispatch, Dispatch::Empty) {
            self.state.show_startup_hints = false;
        }
        match dispatch {
            Dispatch::Empty => AppAction::Redraw,
            Dispatch::Prompt(prompt) => {
                self.state.transcript.push_user(prompt.clone());
                AppAction::Submit(prompt)
            }
            Dispatch::Command { name, arg } => self.run_command(&name, arg),
            Dispatch::Bash { command, excluded } => {
                // Open the live bash block (`bash-execution.ts`) and hand the spawn to the run loop.
                // Both labels are `keyText`-shaped upstream — `Running... (${keyText(…)} to cancel)`
                // (`bash-execution.ts:59`) and `keyHint("app.tools.expand", …)` (`:180`, `:184`) —
                // so they join every bound key (`keybinding-hints.ts:29-36`).
                let cancel = self.state.keymap.keys_label(Action::Interrupt);
                let expand = self.state.keymap.keys_label(Action::ToolsExpand);
                self.state.transcript.start_bash(command.clone(), excluded, cancel, expand);
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
    fn run_command(&mut self, name: &str, arg: Option<String>) -> AppAction {
        use AppCommand as C;
        let cmd = |c| AppAction::Command(c);
        match name {
            // --- data-bound selectors (run loop sources rows) ---
            // `/model [text]` threads its argument (`handleModelCommand(searchTerm?)`,
            // interactive-mode.ts:4175): exact match → set directly; partial → pre-filtered picker.
            "model" => cmd(C::ModelCommand(arg)),
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
            "export" => cmd(C::Export(arg)),
            "import" => cmd(C::Import(arg)),
            "share" => cmd(C::Share),
            "copy" => cmd(C::Copy),
            "name" => match arg {
                Some(n) => cmd(C::SetName(n)),
                None => {
                    self.state.transcript.push_status("usage: /name <session name>");
                    AppAction::Redraw
                }
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
                self.state.transcript.push_block("What's New", "No changelog entries found.");
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
                self.state.transcript.push_block("Armin says hi!", armin_art());
                AppAction::Redraw
            }
            // `/dementedelves` (`daxnuts.ts`): a themed banner block (the model-triggered animation is
            // chrome; the announcement is a real rich block).
            "dementedelves" => {
                self.state
                    .transcript
                    .push_block("Demented Elves", "🧝 The demented elves have entered the chat.");
                AppAction::Redraw
            }
            // Any other unhandled recognized name: a status line.
            other => {
                self.state.transcript.push_status(format!("command: /{other}"));
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
                // A running `!`/`!!` bash block is cancelled first (the run loop kills the child),
                // mirroring Pi's `tui.select.cancel` on the bash component.
                if self.state.transcript.bash_running() {
                    self.state.transcript.bash_complete_simple(None, true);
                    self.state.transcript.commit_bash();
                }
                // Pi branches on `this.session.isStreaming` FIRST (interactive-mode.ts:2636): an Esc
                // that lands mid-turn restores the queued steering/follow-up text to the editor and
                // THEN aborts, so nothing the user typed during the run is lost. Read the flag
                // before the local teardown below clears it.
                let streaming = self.state.status.streaming;
                self.state.transcript.discard_streaming();
                self.state.transcript.commit_tools();
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
                if streaming {
                    AppAction::InterruptRestoreQueued
                } else {
                    AppAction::Interrupt
                }
            }
            // `app.clear` (Ctrl+C, Pi `handleCtrlC` interactive-mode.ts:3361-3369): a second Ctrl+C
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
                // `Ctrl+O` toggles the live bash block when one is present (`bash-execution.ts`
                // `setExpanded`), else the tool-output expansion.
                if self.state.transcript.has_bash() {
                    self.state.transcript.toggle_bash_expanded();
                } else {
                    self.state.transcript.toggle_tool_expanded();
                }
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

    /// The `/hotkeys` body — Pi `handleHotkeysCommand` (interactive-mode.ts:6090-6205), verbatim: three
    /// `**Section**` headings each over a `| Key | Action |` GFM table, keys backticked and joined with
    /// ` / ` where a row names two bindings.
    ///
    /// Every cell is `keyDisplayText(id)` = `formatKeys(getKeys(id), { capitalize: true })`
    /// (`keybinding-hints.ts:29-39`), i.e. **all** keys bound to the id joined with `/` and each chord
    /// part title-cased — not just the first key, so a rebind that binds two keys shows both. Unbound
    /// ids render as the empty string exactly as upstream's `keys.length === 0 → ""` does (:30).
    ///
    /// `[CYRUP-DELTA]` — three of upstream's `**Other**` rows are omitted because the binding itself is
    /// unported, not merely unbound: `app.model.select` ("Open model selector"), `app.thinking.toggle`
    /// ("Toggle thinking block visibility") and `app.message.copy` ("Copy last assistant message") have
    /// no [`Action`] variant (`core/keybindings.ts:21,23,26` vs `keymap.rs`'s enum). Printing them with
    /// an empty key cell would advertise a shortcut no key reaches.
    ///
    /// The trailing **Extensions** table (`:6186-6197`) IS built. It is gated on
    /// `if (shortcuts.size > 0)` (`:6189`) — no registered shortcut, no section, never an empty
    /// table — and each row is
    /// ``| `${formatKeyText(key, { capitalize: true })}` | ${shortcut.description ?? shortcut.extensionPath} |``
    /// (`:6193-6197`). cyrup's registry is [`AppState::extension_shortcuts`], the very set the input
    /// router already matches presses against (`:1501`), fed from `ExtensionHost::shortcut_keys()`
    /// — so the section is a read of live state, not a fabricated list.
    ///
    /// The `newLine` row's `" (Ctrl+Enter on Windows Terminal)"` suffix (:6151) is gated on
    /// `process.platform === "win32"`; it is emitted here under the same `cfg(windows)` condition.
    fn hotkeys_markdown(&self) -> String {
        let ek = self.state.editor.keymap_ref();
        let km = &self.state.keymap;
        // `keyDisplayText` — every bound key, `/`-joined, each part capitalized.
        let e = |a: EditorAction| {
            crate::chrome::format_key_text(&ek.keys_label(a).unwrap_or_default(), true)
        };
        let g =
            |a: Action| crate::chrome::format_key_text(&km.keys_label(a).unwrap_or_default(), true);
        let win_note = if cfg!(windows) { " (Ctrl+Enter on Windows Terminal)" } else { "" };
        let mut out = format!(
            "**Navigation**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{cursor_up}` / `{cursor_down}` / `{cursor_left}` / `{cursor_right}` | Move cursor / browse history |\n\
             | `{word_left}` / `{word_right}` | Move by word |\n\
             | `{line_start}` | Start of line |\n\
             | `{line_end}` | End of line |\n\
             | `{jump_fwd}` | Jump forward to character |\n\
             | `{jump_back}` | Jump backward to character |\n\
             | `{page_up}` / `{page_down}` | Scroll by page |\n\
             \n\
             **Editing**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{submit}` | Send message |\n\
             | `{new_line}` | New line{win_note} |\n\
             | `{del_word_back}` | Delete word backwards |\n\
             | `{del_word_fwd}` | Delete word forwards |\n\
             | `{del_line_start}` | Delete to start of line |\n\
             | `{del_line_end}` | Delete to end of line |\n\
             | `{yank}` | Paste the most-recently-deleted text |\n\
             | `{yank_pop}` | Cycle through the deleted text after pasting |\n\
             | `{undo}` | Undo |\n\
             \n\
             **Other**\n\
             | Key | Action |\n\
             |-----|--------|\n\
             | `{tab}` | Path completion / accept autocomplete |\n\
             | `{interrupt}` | Cancel autocomplete / abort streaming |\n\
             | `{clear}` | Clear editor (first) / exit (second) |\n\
             | `{exit}` | Exit (when editor is empty) |\n\
             | `{suspend}` | Suspend to background |\n\
             | `{thinking_cycle}` | Cycle thinking level |\n\
             | `{model_fwd}` / `{model_back}` | Cycle models |\n\
             | `{expand_tools}` | Toggle tool output expansion |\n\
             | `{external_editor}` | Edit message in external editor |\n\
             | `{follow_up}` | Queue follow-up message |\n\
             | `{dequeue}` | Restore queued messages |\n\
             | `{paste_image}` | Paste image or text from clipboard |\n\
             | `/` | Slash commands |\n\
             | `!` | Run bash command |\n\
             | `!!` | Run bash command (excluded from context) |",
            cursor_up = e(EditorAction::CursorUp),
            cursor_down = e(EditorAction::CursorDown),
            cursor_left = e(EditorAction::CursorLeft),
            cursor_right = e(EditorAction::CursorRight),
            word_left = e(EditorAction::CursorWordLeft),
            word_right = e(EditorAction::CursorWordRight),
            line_start = e(EditorAction::CursorLineStart),
            line_end = e(EditorAction::CursorLineEnd),
            jump_fwd = e(EditorAction::JumpForward),
            jump_back = e(EditorAction::JumpBackward),
            // Upstream reads these off the EDITOR map — `getEditorKeyDisplay("tui.editor.pageUp")`
            // (`interactive-mode.ts:5766-5767`, rendered at `:5808`) — not an app binding.
            page_up = e(EditorAction::PageUp),
            page_down = e(EditorAction::PageDown),
            submit = e(EditorAction::Submit),
            new_line = e(EditorAction::NewLine),
            del_word_back = e(EditorAction::DeleteWordBackward),
            del_word_fwd = e(EditorAction::DeleteWordForward),
            del_line_start = e(EditorAction::DeleteToLineStart),
            del_line_end = e(EditorAction::DeleteToLineEnd),
            yank = e(EditorAction::Yank),
            yank_pop = e(EditorAction::YankPop),
            undo = e(EditorAction::Undo),
            tab = e(EditorAction::Tab),
            interrupt = g(Action::Interrupt),
            clear = g(Action::Clear),
            exit = g(Action::Quit),
            suspend = g(Action::Suspend),
            thinking_cycle = g(Action::ThinkingCycle),
            model_fwd = g(Action::ModelCycleForward),
            model_back = g(Action::ModelCycleBackward),
            expand_tools = g(Action::ToolsExpand),
            external_editor = g(Action::ExternalEditor),
            follow_up = g(Action::FollowUp),
            dequeue = g(Action::Dequeue),
            paste_image = g(Action::ClipboardPasteImage),
        );
        // `if (shortcuts.size > 0) { hotkeys += "\n**Extensions**\n| Key | Action |\n|-----|--------|\n" }`
        // then one `| \`key\` | description |` row per entry (`interactive-mode.ts:6189-6197`).
        // The key cell is `formatKeyText(key, { capitalize: true })` over the REGISTERED id, not a
        // keymap lookup — an extension shortcut is not a rebindable `Keybinding`, so there is
        // nothing to resolve it against.
        if !self.state.extension_shortcuts.is_empty() {
            out.push_str("\n\n**Extensions**\n| Key | Action |\n|-----|--------|");
            for (_, spec) in &self.state.extension_shortcuts {
                let key_display = crate::chrome::format_key_text(&spec.id, true);
                // `shortcut.description ?? shortcut.extensionPath` (`:6192`). cyrup's
                // `ExtensionHost::shortcut_keys()` currently surfaces neither field — the guest's
                // `register_shortcut(key, desc)` drops `desc` at `cyrup-ext/src/host/live.rs:98`
                // and the registry keys on `ExtensionId` — so with nothing registered the raw
                // key-id stands in. It identifies the shortcut truthfully; a fabricated label
                // would not.
                let label = spec.description.as_deref().unwrap_or(spec.id.as_str());
                out.push_str(&format!("\n| `{key_display}` | {label} |"));
            }
        }
        out
    }

    /// Open an editor-swap selector (spec/tui/05 §1.1 `showSelector`): snapshot the editor text, build
    /// the selector for `kind`, and put it in the input slot. The theme picker also stashes the live
    /// theme so a cancel can restore it. Idempotent-ish: opening replaces any already-open selector.
    pub fn open_selector(&mut self, kind: SelectorKind) {
        let saved_editor = self.state.editor.text();
        // `with_upstream_chrome` applies the hint row / one-column inset ONLY for the kinds whose
        // pi component builds them (`SelectorKind::draws_hint_row` / `insets_rows`). Thinking,
        // show-images and theme are `DynamicBorder` + `SelectList` + `DynamicBorder` upstream and
        // get neither.
        let (inner, restore_theme): (Box<dyn Selector>, Option<UiTheme>) = match kind {
            SelectorKind::Thinking => (
                Box::new(ListSelector::thinking(&self.state.thinking_level).with_upstream_chrome(
                    kind,
                    &self.state.select_keymap,
                )),
                None,
            ),
            SelectorKind::ShowImages => (
                Box::new(ListSelector::show_images(self.state.show_images).with_upstream_chrome(
                    kind,
                    &self.state.select_keymap,
                )),
                None,
            ),
            SelectorKind::Theme => (
                Box::new(
                    ListSelector::theme(&self.state.theme.name)
                        .with_upstream_chrome(kind, &self.state.select_keymap),
                ),
                Some(self.state.theme.clone()),
            ),
            // Data-bound selectors must be opened via `open_data_selector` (they need L5 rows);
            // opening one with no data yields an empty-state list rather than a panic.
            other => (
                Box::new(
                    ListSelector::data(other, Vec::new(), 0)
                        .with_upstream_chrome(other, &self.state.select_keymap),
                ),
                None,
            ),
        };
        self.state.selector = Some(ActiveSelector { kind, inner, saved_editor, restore_theme });
    }

    /// Open a **data-bound** selector (`/model`, `/resume`, `/tree`, …) over rows the run loop sourced
    /// from session-svc / resources (spec/tui/05 §6, §8 late-data population). `rows` are
    /// `(value, label, description)`; `selected` preselects a row. Confirming hands the chosen `value`
    /// back to the run loop as [`AppCommand::ConfirmSelection`].
    pub fn open_data_selector(
        &mut self,
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
    ) {
        let saved_editor = self.state.editor.text();
        let inner: Box<dyn Selector> = Box::new(
            ListSelector::data(kind, rows, selected)
                .with_upstream_chrome(kind, &self.state.select_keymap),
        );
        self.state.selector =
            Some(ActiveSelector { kind, inner, saved_editor, restore_theme: None });
    }

    /// Open the bespoke scoped-models checkbox+reorder selector (`scoped-models-selector.ts`,
    /// spec/tui/05 §6) over the full `catalog` `(id, label, provider, desc)` with the current scope
    /// (`None` = all enabled). Confirming (Ctrl+S) yields an [`AppCommand::ConfirmSelection`] the run
    /// loop applies via `set_scoped_models`.
    pub fn open_checkbox_selector(
        &mut self,
        catalog: Vec<(String, String, String, Option<String>)>,
        enabled: Option<Vec<String>>,
    ) {
        let saved_editor = self.state.editor.text();
        let mut selector = CheckboxSelector::scoped_models(catalog, enabled);
        // `getFooterText` resolves the toggle key through `keyText("tui.select.confirm")`
        // (`scoped-models-selector.ts:198`), so the footer has to read the app's merged table, not
        // the stock one. Same for the bespoke `app.models.*` keys the rest of the row names
        // (`:199-204`), which is what `set_models_keymap` is for.
        selector.set_select_keymap(self.state.select_keymap.clone());
        selector.set_models_keymap(self.state.models_keymap.clone());
        let inner: Box<dyn Selector> = Box::new(selector);
        self.state.selector = Some(ActiveSelector {
            kind: SelectorKind::ScopedModels,
            inner,
            saved_editor,
            restore_theme: None,
        });
    }

    /// Open the `/model` selector (feature #1): the full [`ModelSelector`] with fuzzy search, the
    /// `all | scoped` scope toggle, `[provider]` badges, and a `✓` on the active model — over the live
    /// model catalog `(id, name, provider, current, scoped)`. Replaces the bare titled list the audit
    /// flagged. Snapshots the editor like every editor-swap selector. When `search` is `Some`, the
    /// picker opens **pre-filtered** to it (Pi `showModelSelector(initialSearchInput)`,
    /// interactive-mode.ts:4307,4333).
    pub fn open_model_selector(&mut self, models: Vec<ModelEntry>, search: Option<String>) {
        let saved_editor = self.state.editor.text();
        let mut selector = ModelSelector::new(models);
        // `getScopeHintText` is `keyHint("tui.input.tab", "scope") + …` (`model-selector.ts:229`),
        // resolved through the live table; cyrup's editor tier owns that binding.
        selector.set_editor_keymap(self.state.editor.keymap_ref());
        if let Some(term) = search {
            selector.set_search(term);
        }
        let inner: Box<dyn Selector> = Box::new(selector);
        self.state.selector = Some(ActiveSelector {
            kind: SelectorKind::Model,
            inner,
            saved_editor,
            restore_theme: None,
        });
    }

    /// Handle `/model [text]` (Pi `handleModelCommand`, interactive-mode.ts:4175-4196): with no term the
    /// unfiltered picker opens; with a term, an EXACT catalog match sets the model directly (no picker,
    /// `findExactModelReferenceMatch` → `session.setModel`), while a partial opens the picker
    /// pre-filtered to it. The catalog is the live available multi-provider catalog the picker itself
    /// sources (`model_entries`).
    async fn handle_model_command(&mut self, session: &Arc<AgentSession>, search: Option<String>) {
        let models = model_entries(session);
        if models.is_empty() {
            self.state.transcript.push_status("no models available (configure providers)");
            return;
        }
        if let Some(term) = search.as_deref()
            && let Some(model) = crate::model_selector::find_exact_model_reference_match(&models, term)
        {
            // Exact match → set the fully-qualified `provider/id` directly (mirrors the confirm path),
            // no picker (`handleModelCommand` early-returns after `setModel`).
            let id = format!("{}/{}", model.provider, model.id);
            match session.set_model(&id).await {
                Ok(_) => self.state.transcript.push_status(format!("model → {id}")),
                Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
            }
            return;
        }
        // No term, or a partial with no exact match → the picker, pre-filtered to the term if any.
        self.open_model_selector(models, search);
    }

    /// Open an arbitrary boxed [`Selector`] in the input slot under `kind` (the seam for the bespoke
    /// non-list selectors — `/tree`'s [`TreeSelector`] — that are not a plain [`ListSelector`] yet need
    /// the same editor-swap lifecycle as the data selectors). Snapshots the editor like the others.
    pub fn open_boxed_selector(&mut self, kind: SelectorKind, inner: Box<dyn Selector>) {
        let saved_editor = self.state.editor.text();
        self.state.selector =
            Some(ActiveSelector { kind, inner, saved_editor, restore_theme: None });
    }

    /// The kind of the currently-open selector, if any (test/inspection access).
    pub fn active_selector_kind(&self) -> Option<SelectorKind> {
        self.state.selector.as_ref().map(|s| s.kind)
    }

    /// Install the off-task `/tree` navigation channel and hand back its receiver.
    ///
    /// [`App::run`] calls this once at startup; without it [`Self::begin_tree_navigation`] falls
    /// back to awaiting the navigation inline, which is only ever correct for a NON-summarizing
    /// navigation. `pub` so `tests/*.rs` can exercise the spawned path (and therefore the
    /// Escape→abort routing and the live `IndicatorKind::BranchSummary` indicator) without standing
    /// up a whole run loop.
    pub fn install_tree_nav_channel(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<TreeNavMsg> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<TreeNavMsg>();
        self.tree_nav_tx = Some(tx);
        rx
    }

    // ========================================================================
    // `/login` + `/logout` (Pi `interactive-mode.ts:4941-5051`, `:5229-5403`)
    // ========================================================================

    /// Install the off-task `/login` channel and hand back its receiver.
    ///
    /// [`App::run`] calls this once at startup, exactly like
    /// [`Self::install_tree_nav_channel`]. Without it [`Self::begin_provider_login`] refuses to
    /// start a flow — see [`Self::login_tx`] for why there is no inline fallback.
    ///
    /// `pub` so `tests/*.rs` can drive a whole login without standing up a run loop (the crate's
    /// established run-loop-only testing seam, same as [`Self::open_extension_dialog`]).
    pub fn install_login_channel(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<LoginUiMsg>();
        self.login_tx = Some(tx);
        rx
    }

    /// Override where `/login` sources its provider registry (default:
    /// `cyrup_provider::all_providers()`).
    ///
    /// This is the offline-test seam mandated by the "tests must never hit real provider APIs"
    /// convention: a test injects a provider whose `OAuthAuth::login` is a pure in-process function,
    /// so the full `/login` path — picker → dialog → `AuthInteraction` → `cyrup_config::login::login`
    /// → credential store — runs end to end with no socket opened. Production never calls it.
    pub fn set_login_provider_source(&mut self, source: LoginProviderSource) {
        self.login_providers = Some(source);
    }

    /// `this.session.modelRuntime.getProviders()` + `getProviderAuthStatus` + `isUsingOAuth`, the
    /// three registry reads `getLoginProviderOptions` folds together
    /// (`interactive-mode.ts:4943-4947`).
    ///
    /// Pi's `Provider` interface carries `name`; cyrup's does not (the display name lives on the
    /// concrete `WireProvider`), so the name comes from [`crate::provider_display_name`] — the same
    /// `getProviderDisplayName` fallback the picker already used for its labels.
    async fn login_provider_inputs(&self, session: &Arc<AgentSession>) -> Vec<ProviderLoginInput> {
        Self::build_login_inputs(session, self.login_providers.as_deref()).await
    }

    /// The `&self`-free form of [`Self::login_provider_inputs`], so the spawned login task can
    /// rebuild the inputs itself — `ProviderLoginInput` is not `Clone`, so the vector cannot be
    /// handed across.
    ///
    /// The stored-credential kinds are read ONCE (`listCredentials()`, `auth-storage.ts:252-254`)
    /// rather than per provider: pi answers `isUsingOAuth` off a single in-memory
    /// `snapshot.auth` map (`model-runtime.ts:368`), and a read-per-provider would be ~31
    /// lock-and-parse round trips through `auth.json` every time `/login` opens.
    async fn build_login_inputs(
        session: &Arc<AgentSession>,
        source: Option<&(dyn Fn() -> Vec<Arc<dyn cyrup_provider::Provider>> + Send + Sync)>,
    ) -> Vec<ProviderLoginInput> {
        let store = &session.services().auth;
        let stored = cyrup_config::login::stored_credentials(store)
            .await
            .unwrap_or_default();
        let providers = match source {
            Some(source) => source(),
            None => cyrup_provider::all_providers(),
        };
        let mut out = Vec::with_capacity(providers.len());
        for provider in providers {
            // `provider.auth` — a provider with no auth strategy at all contributes no row
            // (`:4948`/`:4957` both test a member of it).
            let Some(auth) = provider.provider_auth().cloned() else {
                continue;
            };
            let id = provider.id().clone();
            // `isUsingOAuth(id)`: `snapshot.auth.get(id)?.type === "oauth"` — the STORED
            // credential's kind, not the provider's capability.
            let using_oauth = stored
                .iter()
                .any(|(p, t)| p.as_str() == id.as_str() && *t == AuthType::Oauth);
            out.push(ProviderLoginInput {
                name: crate::provider_display_name(id.as_str()),
                status: cyrup_config::login::provider_auth_status(store, &id, None),
                id,
                auth,
                using_oauth,
            });
        }
        out
    }

    /// Refresh the cached stored-credential kinds ([`AppState::oauth_credential_providers`]) from
    /// the session's `AuthStore`, then recompute the footer's ` (sub)` marker.
    ///
    /// This is cyrup's stand-in for pi keeping `modelRuntime.snapshot.auth` warm: pi's footer reads
    /// the map synchronously on every repaint (`isUsingOAuth`, `model-runtime.ts:458-460`), cyrup
    /// reads `auth.json` once per credential-changing event and answers from the cache.
    ///
    /// A read failure leaves the previous snapshot alone rather than clearing it — an unreadable
    /// `auth.json` is not evidence that the user logged out, and blanking the set would make the
    /// marker flicker off on a transient error.
    pub async fn refresh_auth_snapshot(&mut self, session: &Arc<AgentSession>) {
        if let Ok(stored) = cyrup_config::login::stored_credentials(&session.services().auth).await
        {
            self.state.oauth_credential_providers = stored
                .into_iter()
                .filter(|(_, kind)| *kind == AuthType::Oauth)
                .map(|(id, _)| id.as_str().to_string())
                .collect();
        }
        self.refresh_subscription_marker();
    }

    /// The provider registry the subscription predicate reads — pi's `this.models.getProvider(id)`
    /// (`model-runtime.ts:463`). Same source [`Self::build_login_inputs`] uses, so a test that
    /// substitutes the registry through [`Self::set_login_provider_source`] substitutes it here too.
    fn provider_oauth_strategy(
        &self,
        provider_id: &str,
    ) -> Option<Arc<dyn cyrup_provider::auth::OAuthAuth>> {
        let providers = match self.login_providers.as_deref() {
            Some(source) => source(),
            None => cyrup_provider::all_providers(),
        };
        providers
            .iter()
            .find(|p| p.id().as_str() == provider_id)
            .and_then(|p| p.provider_auth())
            .and_then(|auth| auth.oauth.clone())
    }

    /// The footer's `usingSubscription` predicate, verbatim from pi v0.84.1
    /// `coding-agent/src/modes/interactive/components/footer.ts:138-141`:
    ///
    /// ```text
    /// // Kimi Coding is subscription-backed despite using API-key authentication.
    /// const usingSubscription = state.model
    ///     ? state.model.provider === "kimi-coding" || this.session.modelRuntime.isUsingSubscription(state.model.provider)
    ///     : false;
    /// ```
    ///
    /// with `isUsingSubscription` expanded from `model-runtime.ts:462-464`:
    ///
    /// ```text
    /// isUsingSubscription(providerId) {
    ///     return this.isUsingOAuth(providerId) && this.models.getProvider(providerId)?.auth.oauth?.isSubscription === true;
    /// }
    /// ```
    ///
    /// **Both conjuncts are load-bearing.** `isUsingOAuth` alone — which is what pi itself called
    /// here until v0.84.0 (`v0.83.0:footer.ts:140`) — prints ` (sub)` for a metered OAuth sign-in
    /// such as OpenRouter; pi's v0.84.0 changelog records fixing exactly that (*"Fixed the footer
    /// showing `(sub)` for generic OAuth/OpenID sign-ins without a known subscription"*,
    /// `coding-agent/CHANGELOG.md:155`). And `isSubscription` alone would print ` (sub)` for an
    /// Anthropic user paying with `ANTHROPIC_API_KEY`, since that provider carries a subscription
    /// OAuth *strategy* whether or not the user signed in with it.
    ///
    /// The `kimi-coding` short-circuit is upstream's, not cyrup's: that provider is
    /// subscription-backed while authenticating with an API key, so neither conjunct can see it.
    fn provider_uses_subscription(&self, provider_id: &str) -> bool {
        if provider_id == KIMI_CODING_PROVIDER_ID {
            return true;
        }
        self.state.oauth_credential_providers.contains(provider_id)
            && self
                .provider_oauth_strategy(provider_id)
                .is_some_and(|oauth| oauth.is_subscription())
    }

    /// Recompute the footer's ` (sub)` marker for the currently-active provider. pi has no such
    /// method because its footer recomputes the flag on every repaint; cyrup's [`StatusLine`] is a
    /// value struct, so the flag is pushed whenever either of its two inputs moves — the active
    /// provider (`ModelChanged`) or the stored credentials ([`Self::refresh_auth_snapshot`]).
    ///
    /// No active provider ⇒ `false`, which is pi's `state.model ? … : false` (`footer.ts:139-141`).
    fn refresh_subscription_marker(&mut self) {
        let sub = self
            .state
            .status
            .provider
            .clone()
            .is_some_and(|p| self.provider_uses_subscription(&p));
        self.state.status.set_using_subscription(sub);
    }

    /// The accumulated body text of the open `/login` dialog (`None` when no dialog is open) —
    /// test/inspection access to what the flow has drawn so far, the same role
    /// [`Self::active_selector_kind`] plays for the slot itself.
    pub fn login_dialog_body(&mut self) -> Option<String> {
        self.login_dialog_mut().map(|d| d.body_text())
    }

    /// The open `/login` dialog's title (`` `Login to ${providerName}` ``), for the same reason.
    pub fn login_dialog_title(&mut self) -> Option<String> {
        self.login_dialog_mut().map(|d| d.title().to_string())
    }

    /// The `/login` dialog currently in the input slot, if any.
    fn login_dialog_mut(&mut self) -> Option<&mut LoginDialog> {
        self.state
            .selector
            .as_mut()
            .filter(|s| s.kind == SelectorKind::LoginDialog)
            .and_then(|s| s.inner.as_login_dialog())
    }

    /// `handleLoginCommand(providerRef?)` (`interactive-mode.ts:4994-5026`), routed through the
    /// ported [`cyrup_config::login::resolve_login_command`].
    async fn handle_login_command(&mut self, session: &Arc<AgentSession>, arg: Option<String>) {
        let inputs = self.login_provider_inputs(session).await;
        let options = cyrup_config::login::login_provider_options(&inputs, None);
        match cyrup_config::login::resolve_login_command(arg.as_deref(), &options) {
            // `startProviderLogin(providerOptions[0])` (`:5000-5003`).
            LoginCommand::Start(option) => self.begin_provider_login(session, *option),
            // `showLoginAuthTypeSelector(providerOptions?)` (`:4997`, `:5010`).
            LoginCommand::AuthTypeSelector { options } => {
                self.open_login_auth_type_selector(session, options)
            }
            // `showLoginProviderSelector(undefined, providerRef)` (`:5013`).
            LoginCommand::ProviderSelector {
                auth_type,
                initial_search,
            } => self.open_login_provider_selector(&inputs, auth_type, initial_search),
        }
    }

    /// `showLoginAuthTypeSelector(providerOptions?)` (`interactive-mode.ts:5028-5051`), routed
    /// through the ported [`cyrup_config::login::resolve_auth_type_selector`].
    fn open_login_auth_type_selector(
        &mut self,
        session: &Arc<AgentSession>,
        options: Option<Vec<LoginProviderOption>>,
    ) {
        match cyrup_config::login::resolve_auth_type_selector(options.as_deref()) {
            // `showStatus("No login methods available.")` (`:5046`).
            cyrup_config::login::AuthTypeSelector::Unavailable => {
                self.state
                    .transcript
                    .push_status(cyrup_config::login::NO_LOGIN_METHODS);
            }
            // One provider, one method: the selector is skipped entirely (`:5049-5055`).
            cyrup_config::login::AuthTypeSelector::Start(option) => {
                self.state.login_auth_type_options = None;
                self.begin_provider_login(session, *option);
            }
            cyrup_config::login::AuthTypeSelector::Choose {
                title,
                subscription_label,
                api_key_label,
            } => {
                // `options` in Pi's order: the subscription label first (`:5036-5041`).
                let mut rows: Vec<(String, String, Option<String>)> = Vec::new();
                if let Some(label) = subscription_label {
                    rows.push((AuthType::Oauth.as_str().to_string(), label, None));
                }
                if let Some(label) = api_key_label {
                    rows.push((AuthType::ApiKey.as_str().to_string(), label, None));
                }
                self.state.login_auth_type_options = options;
                self.open_data_selector(SelectorKind::LoginAuthType, rows, 0);
                if let Some(active) = self.state.selector.as_mut() {
                    active.inner.set_title(title);
                }
            }
        }
    }

    /// `showLoginProviderSelector(authType?, initialSearchInput?)`
    /// (`interactive-mode.ts:5085-5124`): the options narrowed to `auth_type`, or the empty-state
    /// status when nothing qualifies.
    fn open_login_provider_selector(
        &mut self,
        inputs: &[ProviderLoginInput],
        auth_type: Option<AuthType>,
        initial_search: Option<String>,
    ) {
        let options = cyrup_config::login::login_provider_options(inputs, auth_type);
        if options.is_empty() {
            self.state
                .transcript
                .push_status(cyrup_config::login::provider_selector_empty_message(auth_type));
            return;
        }
        // S5/S21: the real `OAuthSelectorComponent` (`oauth-selector.ts`) — search `Input`, fuzzy
        // filter, coloured status runs — in place of the bare `ListSelector`. `initialSearchInput`
        // (`:5124`) now lands where upstream puts it: seeded into the search box (`:99`), not
        // reported as a status line.
        let selector =
            crate::OAuthSelector::new(crate::OAuthMode::Login, &options, initial_search);
        self.state.login_options = options;
        self.open_boxed_selector(SelectorKind::Login, Box::new(selector));
    }

    /// `startProviderLogin(providerOption)` (`interactive-mode.ts:5017-5025`), routed through the
    /// ported [`cyrup_config::login::start_provider_login`].
    ///
    /// The OAuth and API-key legs are the SAME code here: both open the dialog and spawn
    /// `cyrup_config::login::login` with the matching [`AuthType`]. Upstream splits them into
    /// `showLoginDialog` / `showApiKeyLoginDialog` only because of two cosmetic differences — the
    /// amazon-bedrock `showDetails` block (`:5266-5272`; that provider is unported, see
    /// `providers/all.rs`) and the failure-message wording, which [`LoginFinished::oauth`] carries.
    fn begin_provider_login(&mut self, session: &Arc<AgentSession>, option: LoginProviderOption) {
        match cyrup_config::login::start_provider_login(&option) {
            // `showAmbientAuthDialog(providerOption)` (`:5023`, `:5229-5250`): a dialog with a
            // single info line and a close hint. Nothing to run, so no task is spawned.
            LoginStep::Ambient {
                title, message, ..
            } => {
                self.open_login_dialog(title);
                if let Some(dialog) = self.login_dialog_mut() {
                    dialog.show_info(&message, &[], true);
                }
            }
            LoginStep::Oauth { id, name } | LoginStep::ApiKey { id, name } => {
                let oauth = option.auth_type == AuthType::Oauth;
                let Some(tx) = self.login_tx.clone() else {
                    // No run loop is servicing the channel — refuse rather than spawn a task whose
                    // first prompt can never be answered.
                    self.state
                        .transcript
                        .push_status("login unavailable: no interactive session");
                    return;
                };
                // `new LoginDialogComponent(ui, providerId, …, providerName)` → title
                // `` `Login to ${providerName}` `` (`login-dialog.ts:41`).
                self.open_login_dialog(format!("Login to {name}"));
                // `dialog.signal` — the dialog's own AbortController (`login-dialog.ts:73-75`).
                let cancel = CancelToken::new();
                self.state.login_cancel = Some(cancel.clone());
                let auth_type = option.auth_type;
                let store = Arc::clone(&session.services().auth);
                // `getAuthPath()` (`env.rs:236-238`): the path the success status names.
                let auth_path = session.services().agent_dir.join("auth.json");
                let session = Arc::clone(session);
                let login_providers = self.login_providers.clone();
                tokio::spawn(async move {
                    let inputs =
                        Self::build_login_inputs(&session, login_providers.as_deref()).await;
                    let interaction = TuiAuthInteraction::new(tx.clone(), cancel);
                    // `await this.session.modelRuntime.login(providerId, method, {…})`
                    // (`interactive-mode.ts:5368`) — `Models.login` persists into the credential
                    // store itself, so there is no separate write here.
                    let result = cyrup_config::login::login(
                        &*store,
                        &inputs,
                        &id,
                        auth_type,
                        &interaction,
                    )
                    .await;
                    let finished = match result {
                        Ok(_) => LoginFinished {
                            provider_id: id.as_str().to_string(),
                            provider_name: name,
                            oauth,
                            result: Ok(()),
                            cancelled: false,
                            auth_path,
                        },
                        Err(e) => LoginFinished {
                            provider_id: id.as_str().to_string(),
                            provider_name: name,
                            oauth,
                            cancelled: e.is_cancelled(),
                            result: Err(e.to_string()),
                            auth_path,
                        },
                    };
                    let _ = tx.send(LoginUiMsg::Finished(Box::new(finished)));
                });
            }
        }
    }

    /// Put a fresh [`LoginDialog`] in the input slot (`editorContainer.clear(); addChild(dialog);
    /// setFocus(dialog)`, `interactive-mode.ts:5273-5276`). The hint text is taken from the LIVE
    /// `tui.select.*` bindings, matching Pi's `keyHint` (`login-dialog.ts:141`, `:164`).
    fn open_login_dialog(&mut self, title: impl Into<String>) {
        let dialog = LoginDialog::new(title, &self.state.select_keymap);
        self.open_boxed_selector(SelectorKind::LoginDialog, Box::new(dialog));
    }

    /// Apply one message from the spawned login flow (`notifyAuthDialog` / `showAuthPrompt` /
    /// the `try`/`catch` around `loginProvider`, `interactive-mode.ts:5285-5296`, `:5327-5360`,
    /// `:5392-5403`).
    ///
    /// `pub` for the same reason as [`Self::apply_tree_nav_outcome`]: `tests/*.rs` drives the
    /// settle half without a live run loop.
    pub fn apply_login_msg(&mut self, msg: LoginUiMsg) {
        match msg {
            LoginUiMsg::Notify(event) => {
                if let Some(dialog) = self.login_dialog_mut() {
                    notify_auth_dialog(dialog, *event);
                }
            }
            LoginUiMsg::Prompt { prompt, reply } => {
                let Some(dialog) = self.login_dialog_mut() else {
                    // The dialog is already gone (cancelled, or the flow raced the teardown):
                    // reject exactly as `cancel()` does (`login-dialog.ts:82-88`).
                    let _ = reply.send(Err(OAuthError::Cancelled));
                    return;
                };
                show_auth_prompt(dialog, &prompt);
                // A previous prompt still pending would be a flow bug, but resolving it as
                // cancelled is strictly better than leaking the sender (which would hang the flow).
                if let Some(stale) = self.state.pending_login_prompt.replace(reply) {
                    let _ = stale.send(Err(OAuthError::Cancelled));
                }
            }
            LoginUiMsg::Finished(finished) => self.finish_login(*finished),
        }
    }

    /// The `try`/`catch` tail of `showLoginDialog` / `showApiKeyLoginDialog`
    /// (`interactive-mode.ts:5285-5296`, `:5392-5403`): restore the editor, then either the
    /// success status (`completeProviderAuthentication`, `:5176-5227`) or the error banner — and
    /// NOTHING at all when the user cancelled, which is what `errorMsg !== "Login cancelled"`
    /// buys (`:5294`, `:5401`).
    fn finish_login(&mut self, finished: LoginFinished) {
        // `restoreEditor()` (`:5276-5281`).
        if self.active_selector_kind() == Some(SelectorKind::LoginDialog) {
            self.close_selector(true);
        }
        if let Some(reply) = self.state.pending_login_prompt.take() {
            let _ = reply.send(Err(OAuthError::Cancelled));
        }
        self.state.login_cancel = None;
        let name = &finished.provider_name;
        match &finished.result {
            Ok(()) => {
                // The credential the flow just persisted IS the auth snapshot change pi's
                // `completeProviderAuthentication` follows with `this.footer.invalidate()`
                // (`interactive-mode.ts:5448-5449`), which re-answers `usingSubscription` off the
                // now-current `snapshot.auth`. Apply the same delta to the cached map and repaint
                // the marker, so signing in to a Pro/Max plan lights ` (sub)` on the very next
                // frame instead of only after a restart.
                if finished.oauth {
                    self.state
                        .oauth_credential_providers
                        .insert(finished.provider_id.clone());
                } else {
                    // An API-key login REPLACES any stored OAuth credential for that provider
                    // (`auth.json` holds one credential per provider), so the OAuth half of the
                    // snapshot must drop it — otherwise switching Anthropic from Pro/Max to a
                    // metered key would keep the ` (sub)` marker on a metered account.
                    self.state
                        .oauth_credential_providers
                        .remove(&finished.provider_id);
                }
                self.refresh_subscription_marker();
                // `actionLabel` (`:5183`) + `` `${actionLabel}. Credentials saved to ${getAuthPath()}` ``
                // (`:5219`). `getAuthPath()` is `<agent_dir>/auth.json` (`env.rs:236`).
                let action = if finished.oauth {
                    format!("Logged in to {name}")
                } else {
                    format!("Saved API key for {name}")
                };
                let path = finished.auth_path.display();
                self.state
                    .transcript
                    .push_status(format!("{action}. Credentials saved to {path}"));
            }
            // `if (errorMsg !== "Login cancelled")` (`:5294`, `:5401`) — a cancel is silent.
            Err(_) if finished.cancelled => {}
            Err(message) => {
                let banner = if finished.oauth {
                    format!("Failed to login to {name}: {message}")
                } else {
                    format!("Failed to save API key for {name}: {message}")
                };
                self.state.transcript.push_error(banner);
            }
        }
    }

    /// `dialog.cancel()` (`login-dialog.ts:82-88`): abort the flow's signal AND reject the prompt it
    /// is blocked on with `"Login cancelled"`. Called from the selector `Cancel` arm.
    fn cancel_login(&mut self) {
        if let Some(reply) = self.state.pending_login_prompt.take() {
            let _ = reply.send(Err(OAuthError::Cancelled));
        }
        if let Some(cancel) = self.state.login_cancel.take() {
            cancel.cancel();
        }
    }

    /// Open Pi's three-option "Summarize branch?" prompt (`interactive-mode.ts:4755-4760`). Pi uses
    /// its generic `showExtensionSelector`; cyrup renders the same three options through a
    /// first-party [`ListSelector`] so the answer arrives as an ordinary
    /// [`AppCommand::ConfirmSelection`] rather than occupying the single extension-dialog reply slot.
    pub fn open_branch_summary_prompt(&mut self) {
        let rows = vec![
            (BRANCH_SUMMARY_NONE.to_string(), "No summary".to_string(), None),
            (BRANCH_SUMMARY_YES.to_string(), "Summarize".to_string(), None),
            (
                BRANCH_SUMMARY_CUSTOM.to_string(),
                "Summarize with custom prompt".to_string(),
                None,
            ),
        ];
        let title = SelectorKind::BranchSummary.title().to_string();
        self.open_boxed_selector(
            SelectorKind::BranchSummary,
            Box::new(ListSelector::prompt(title, rows, 0).with_upstream_chrome(
                SelectorKind::BranchSummary,
                &self.state.select_keymap,
            )),
        );
    }

    /// Open the custom-instructions editor (Pi `showExtensionEditor("Custom summarization
    /// instructions")`, `interactive-mode.ts:4769`) — the same INLINE editor component Pi's default
    /// `ExtensionEditorComponent` provides, never a teardown to `$EDITOR`.
    fn open_branch_summary_instructions(&mut self) {
        let title = SelectorKind::BranchSummaryInstructions.title().to_string();
        self.open_boxed_selector(
            SelectorKind::BranchSummaryInstructions,
            Box::new(
                ExtensionEditorSelector::new(title, "")
                    .with_keymaps(&self.state.select_keymap, &self.state.keymap),
            ),
        );
    }

    /// Dispatch the `/tree` navigation the user committed to (Pi `interactive-mode.ts:4781-4820`).
    ///
    /// Pi aborts an in-flight response FIRST — "the user committed to navigating: stop the active
    /// response" (`:4781-4785`), restoring the queued messages to the editor on the way — then runs
    /// `navigateTree`. cyrup did neither before SESS-023.
    ///
    /// The navigation itself is SPAWNED whenever a run loop is present, never awaited on the loop
    /// task. A summarizing navigation is a provider round-trip plus retry backoff; awaited inline in
    /// `App::run`'s `select!` it would freeze the loop for the whole call, so no keystroke could
    /// reach `abort_branch_summary` and no `IndicatorKind::BranchSummary` frame could ever render —
    /// exactly the residual `execute_command`'s own doc comment flags. The outcome comes back over
    /// [`Self::tree_nav_tx`] and is applied by [`Self::apply_tree_nav_outcome`].
    async fn begin_tree_navigation(
        &mut self,
        session: &Arc<AgentSession>,
        target: String,
        summarize: bool,
        custom_instructions: Option<String>,
    ) {
        // Pi `:4781-4785` — `restoreQueuedMessagesToEditor()` then `session.abort()`.
        if session.is_streaming().await {
            let (steering, follow_up) = session.drain_queue().await;
            let queued: Vec<String> = steering.into_iter().chain(follow_up).collect();
            self.restore_queued_to_editor(&queued);
            session.abort();
        }
        let opts = NavigateTreeOptions {
            summarize,
            custom_instructions,
            ..NavigateTreeOptions::default()
        };
        let entry = cyrup_core::EntryId::from(target.as_str());
        let Some(tx) = self.tree_nav_tx.clone() else {
            // No run loop (unit/embedder driving `execute_command` directly): await inline. Safe
            // for the non-summarizing path, which makes no model call.
            let outcome = session.navigate_tree(entry, opts).await.map_err(|e| e.to_string());
            self.apply_tree_nav_outcome(TreeNavMsg { target, outcome });
            return;
        };
        if summarize {
            // Pi shows the `BranchSummaryStatusIndicator` and rebinds Escape for the duration
            // (`:4796-4799`, `:4792-4795`); both are torn down in `apply_tree_nav_outcome`.
            self.state.branch_summary_in_flight = true;
            self.state
                .indicator
                .set(IndicatorKind::BranchSummary, Some("Summarizing branch...".to_string()));
        }
        let session = session.clone();
        tokio::spawn(async move {
            let outcome = session.navigate_tree(entry, opts).await.map_err(|e| e.to_string());
            let _ = tx.send(TreeNavMsg { target, outcome });
        });
    }

    /// Apply a settled `/tree` navigation (Pi `interactive-mode.ts:4805-4820`).
    ///
    /// The arm ORDER is load-bearing and was wrong before SESS-023: cyrup returns
    /// `{cancelled: true, aborted: true}` on an aborted summarization (matching Pi
    /// `agent-session.ts:3000-3001`), and the old code tested `cancelled` first — so aborting a
    /// summarization printed "tree navigation cancelled" and silently swallowed the tree. Pi tests
    /// `result.aborted` first (`:4805`) and re-shows the tree at the same entry, then `cancelled`
    /// (`:4809`).
    ///
    /// `pub` so `tests/*.rs` can drive the settle half without a live run loop, the same reason
    /// [`Self::open_extension_dialog`] is public.
    pub fn apply_tree_nav_outcome(&mut self, msg: TreeNavMsg) -> Option<AppCommand> {
        let TreeNavMsg { target, outcome } = msg;
        // Pi's `finally` (`:4830-4833`): clear the indicator and restore the Escape binding
        // regardless of how the navigation ended.
        if self.state.branch_summary_in_flight {
            self.state.branch_summary_in_flight = false;
            if self.state.indicator.kind() == IndicatorKind::BranchSummary
                || self.state.indicator.kind() == IndicatorKind::Retry
            {
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
            }
        }
        match outcome {
            Ok(o) if o.aborted => {
                // Pi `:4805-4808` — status, then re-show the tree at the same entry.
                self.state.transcript.push_status("Branch summarization cancelled");
                self.state.pending_tree_nav = Some(PendingTreeNav { target });
                return Some(AppCommand::OpenSelector(SelectorKind::Tree));
            }
            Ok(o) if o.cancelled => {
                self.state.transcript.push_status("Navigation cancelled");
            }
            Ok(o) => {
                if let Some(text) = o.editor_text {
                    self.state.editor.set_text(&text);
                }
                // A summarized branch navigation records a branch-summary message
                // (`branch-summary-message.ts`) into the transcript.
                if let Some(entry) = o.summary_entry {
                    self.state.transcript.push_branch_summary(entry.summary);
                }
                self.state.transcript.push_status("navigated session tree");
            }
            Err(e) => self.state.transcript.push_status(format!("tree error: {e}")),
        }
        None
    }

    /// Render a loaded extension's `ui.{confirm,select,input}` dialog request in the input slot (L4
    /// review §2.1; `ui.editor` is handled synchronously by the caller, never reaching here — see
    /// [`App::run`]'s `ui_rx` arm). Mirrors Pi's `createExtensionUIContext`
    /// (`interactive-mode.ts:2060-2111`): `confirm` opens a Yes/No [`ListSelector`] exactly like Pi's
    /// confirm-as-select (`:2172-2179`); `select` opens a [`ListSelector`] over the guest's option
    /// strings; `input` opens a [`TextInputSelector`]. The dialog's `reply` one-shot is stashed on
    /// [`AppState::pending_ui_reply`]; [`App::confirm_selector`] and the selector-cancel arm of
    /// [`App::handle_selector_key`] take + resolve it when the user answers.
    ///
    /// The input slot holds at most one occupant: if a selector (first-party or extension) or a
    /// floating overlay is already open when a guest dialog arrives, it is denied immediately with its
    /// per-kind deny default (there is nowhere to render it) rather than queued or silently dropped —
    /// the guest's `ui_roundtrip` never blocks past this call regardless.
    ///
    /// `pub` (not called outside [`App::run`]'s `ui_rx` arm in production) so `tests/*.rs` can drive
    /// it directly with a synthetic [`UiRequest`] — the crate's established pattern for exercising
    /// run-loop-only logic (mirrors [`Self::open_boxed_selector`]/[`Self::active_selector_kind`]).
    pub fn open_extension_dialog(&mut self, req: UiRequest) {
        if self.state.selector.is_some() || !self.state.overlays.is_empty() {
            self.state.transcript.push_status(format!(
                "extension {:?} dialog: another dialog/selector is already open, denied",
                req.kind
            ));
            let _ = req.reply.send(default_ui_reply(req.kind));
            return;
        }
        let UiRequest { kind, prompt, options, message, placeholder, opts, reply } = req;
        let (selector_kind, base_title, mut inner): (SelectorKind, String, Box<dyn Selector>) = match kind
        {
            UiKind::Confirm => {
                // Pi's EXACT join (`showExtensionConfirm`, `interactive-mode.ts:2177`):
                // `` `${title}\n${message}` `` — a real newline, not an em-dash. The title area
                // now auto-sizes + word-wraps (`ListSelector::desired_height`/`render`,
                // `title_wrapped_height`) so a long title and/or multi-line message both render in
                // full instead of being clipped to one row (L4 review §2.6).
                let title = if message.is_empty() { prompt } else { format!("{prompt}\n{message}") };
                let rows = vec![
                    ("yes".to_string(), "Yes".to_string(), None),
                    ("no".to_string(), "No".to_string(), None),
                ];
                (
                    SelectorKind::ExtensionConfirm,
                    title.clone(),
                    Box::new(ListSelector::prompt(title, rows, 0).with_upstream_chrome(
                        SelectorKind::ExtensionConfirm,
                        &self.state.select_keymap,
                    )),
                )
            }
            UiKind::Select => {
                // L4 review §4: an empty `options` list must still OPEN the dialog (Pi's
                // `ExtensionSelectorComponent`, `extension-selector.ts:101-103`, renders whatever
                // it's given including `[]`; Enter is a no-op with nothing selected, and resolution
                // only ever happens via Esc/timeout/signal — same as any other select), not
                // short-circuit to `None` before the guest's dialog is ever shown. `cyrup`'s RPC path
                // (`rpc.rs`) already forwards `options: []` verbatim with no such short-circuit;
                // `ListSelector`/`SelectList` already render an empty list safely (`"No matches"`,
                // `current_value()` never panics) — no special-casing needed here beyond NOT
                // early-returning.
                let picked: Vec<String> = options
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let rows: Vec<(String, String, Option<String>)> =
                    picked.into_iter().map(|o| (o.clone(), o, None)).collect();
                (
                    SelectorKind::ExtensionSelect,
                    prompt.clone(),
                    Box::new(ListSelector::prompt(prompt, rows, 0).with_upstream_chrome(
                        SelectorKind::ExtensionSelect,
                        &self.state.select_keymap,
                    )),
                )
            }
            UiKind::Input => (
                SelectorKind::ExtensionInput,
                prompt.clone(),
                // E6: the hint row is built from the LIVE `tui.select.*` table, so the first paint
                // already names the user's own submit/cancel keys — upstream re-resolves `keyHint`
                // on every render (`keybinding-hints.ts:34-44`) and so never shows stock defaults.
                Box::new(
                    TextInputSelector::new(prompt, placeholder)
                        .with_keymap(&self.state.select_keymap),
                ),
            ),
            // L4 review §3: the DEFAULT is an inline dialog (Pi's `ExtensionEditorComponent`,
            // `extension-editor.ts`), not a teardown to `$EDITOR` — `title` on `prompt`, the seed
            // text (Pi `prefill`) on `message` (L4 review §2's `editor(title, initial)` fix).
            UiKind::Editor => (
                SelectorKind::ExtensionEditor,
                prompt.clone(),
                // E9: the hint row is built from the LIVE `tui.select.*` + app tables, so the first
                // paint already names the user's own keys (upstream re-resolves every `keyHint` on
                // each render, `keybinding-hints.ts:34-44`).
                Box::new(
                    ExtensionEditorSelector::new(prompt, &message)
                        .with_keymaps(&self.state.select_keymap, &self.state.keymap),
                ),
            ),
        };
        // Pi's `CountdownTimer` (`countdown-timer.ts:7-38`, wired by `ExtensionSelectorComponent`/
        // `ExtensionInputComponent`): a guest-set `opts.timeout_ms > 0` arms a live 1s-cadence
        // countdown, shown in the title from the INSTANT the dialog opens (Pi calls `onTick`
        // synchronously in its constructor, `countdown-timer.ts:19`) and ticked forward by
        // [`App::tick_extension_dialog_countdown`] — closing the gap where the dialog otherwise never
        // showed the deadline `LiveHostServices::ui_roundtrip` already enforces host-side, and stayed
        // open on screen (stale) after that host-side timeout had already resolved the guest's call.
        let deadline = opts
            .timeout_ms
            .filter(|&ms| ms > 0)
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        if let Some(deadline) = deadline {
            inner.set_title(countdown_title(&base_title, deadline));
        }
        self.open_boxed_selector(selector_kind, inner);
        self.state.pending_ui_reply = Some(PendingUiReply { kind, reply, base_title, deadline });
    }

    /// Bind BOTH extension-UI seams of a session's host services to this run loop — the single place
    /// [`App::run`] and its session-swap arm attach the TUI, mirroring `cyrup-modes`' `run_rpc` /
    /// `rebind_session`, which install the same pair for RPC mode.
    ///
    /// The pair is not optional: [`UiSink`] carries the request/reply dialogs
    /// (`ui.{confirm,input,select,editor}`) and [`UiEffectSink`] carries the fire-and-forget mutators
    /// (`ui.{notify,set-status,set-widget,set-header,set-footer,set-title,set-editor-text,
    /// paste-editor-text,set-tools-expanded}`). `LiveHostServices` drops an effect outright when the
    /// effect sink is `None` — its headless (print/json) policy, Pi's `noOpUIContext`
    /// (`extensions/runner.ts:230-265`). Interactive is not headless in Pi: it passes a real
    /// `uiContext` (`interactive-mode.ts:2223-2268`), so installing only the dialog half made every
    /// fire-and-forget extension UI call vanish in the DEFAULT mode while working over RPC (TUI-S01).
    ///
    /// Must be re-run against every swapped-in session (`/new`, `/resume`, `/fork`, `/reload`,
    /// `/import`, or a runtime-side `SessionReplaced`): a replacement session brings a fresh
    /// `LiveHostServices` whose sinks are both `None`.
    pub fn install_ui_sinks(
        services: &cyrup_session_svc::LiveHostServices,
        ui: cyrup_session_svc::UiSink,
        effects: cyrup_session_svc::UiEffectSink,
    ) {
        services.set_ui_sink(ui);
        services.set_ui_effect_sink(effects);
    }

    /// Bind the INTERACTIVE-OVERLAY seam — Pi's `ctx.ui.custom(factory, { overlay: true, … })`
    /// (`interactive-mode.ts:2719`, the only `showOverlay` consumer upstream has).
    ///
    /// Separate from [`Self::install_ui_sinks`] because only THIS mode can service it. A `UiSink`
    /// dialog is one question and one answer, which RPC can carry over its
    /// `extension_ui_request`/`extension_ui_response` pair; an overlay is a component the renderer
    /// must DRIVE — paint it, feed it every keystroke, repaint on its own cadence — which needs a
    /// terminal. So `cyrup-modes`' `run_rpc` deliberately installs nothing here, leaving
    /// `LiveHostServices::open_overlay` to answer `false` so the extension falls back to its own
    /// non-interactive rendering (Pi's `!ctx.hasUI` branch) instead of blocking on a modal nobody
    /// can close.
    ///
    /// Must be re-run against every swapped-in session, for exactly the reason
    /// [`Self::install_ui_sinks`] must.
    pub fn install_overlay_sink(
        services: &cyrup_session_svc::LiveHostServices,
        overlays: cyrup_session_svc::OverlaySink,
    ) {
        services.set_overlay_sink(overlays);
    }

    /// Bind the THIRD extension seam of a session — the contained-fault listener Pi's interactive
    /// mode passes as `bindExtensions({ … onError })` (`interactive-mode.ts:1700-1701`:
    /// `onError: (error) => { this.showExtensionError(error.extensionPath, error.error,
    /// error.stack); }`).
    ///
    /// A guest handler fault is CONTAINED by the dispatcher (R-08-036) — the handler is skipped
    /// (fail open) or the action is blocked (fail closed) and the host survives either way — and is
    /// then reported to every registered listener. `cyrup-modes`' `run_rpc` registers one
    /// (`rpc.rs`'s `error_listener`, emitting an `extension_error` line) and its `rebind_session`
    /// re-registers it on every swap; the interactive TUI registered NONE, so with no listener
    /// `Dispatcher::report` degraded to a `tracing::warn!` that no TUI user ever sees. A broken
    /// extension therefore silently ate its own hook — or silently DENIED a tool — in the DEFAULT
    /// mode while an RPC client on the same session saw the fault (TUI-S02).
    ///
    /// The listener is invoked SYNCHRONOUSLY from whatever worker thread the faulting dispatch ran
    /// on, so it only forwards onto an unbounded channel the run loop drains; the drain arm calls
    /// [`Self::show_extension_error`].
    ///
    /// Must be re-run against every swapped-in session for the same reason
    /// [`Self::install_ui_sinks`] must: a replacement session brings a fresh `ExtensionHost` with an
    /// empty listener list (Pi re-binds `onError` from `rebindSession` too).
    pub fn install_error_listener(
        ext_host: &cyrup_ext::ExtensionHost,
        errors: tokio::sync::mpsc::UnboundedSender<cyrup_ext::ExtensionError>,
    ) {
        ext_host.add_error_listener(std::sync::Arc::new(
            move |err: &cyrup_ext::ExtensionError| {
                let _ = errors.send(err.clone());
            },
        ));
    }

    /// Render one contained extension fault into the transcript — Pi `showExtensionError`
    /// (`interactive-mode.ts:2545-2560`), whose copy is
    /// `Extension "${extensionPath}" error: ${error}` in the `error` colour.
    ///
    /// Pi appends a dimmed, indented stack trace when the thrown value carried one; cyrup's
    /// [`cyrup_ext::ExtensionError`] has no `stack` field (a contained fault is an `ExtError`
    /// string, not a JS `Error` object), so only the message line is emitted.
    ///
    /// `pub` for the same reason [`Self::apply_ui_effect`] is: `tests/*.rs` drive the run loop's
    /// drain arm directly, since `App::run` needs a real terminal event source.
    pub fn show_extension_error(&mut self, err: &cyrup_ext::ExtensionError) {
        self.state
            .transcript
            .push_error(format!("Extension \"{}\" error: {}", err.extension.as_str(), err.error));
    }

    /// Apply one fire-and-forget extension UI effect — the interactive-TUI half of the
    /// [`UiEffectSink`] seam `cyrup-modes`' `run_rpc` already drives for RPC mode.
    ///
    /// Pi builds a real `uiContext` for interactive mode (`interactive-mode.ts:2223-2268`) whose
    /// mutators land on concrete TUI state; only headless modes get `noOpUIContext`
    /// (`extensions/runner.ts:230-265`). Cyrup installed the request/reply [`UiSink`] here but never
    /// the effect sink, so every `notify`/`setStatus`/`setTitle`/`setEditorText`/`pasteToEditor`/
    /// `setToolsExpanded`/`setWidget`/`setHeader`/`setFooter` call was dropped by
    /// `LiveHostServices::emit_ui_effect` in the DEFAULT mode while working over RPC.
    ///
    /// Per-variant mapping (each cites the Pi interactive handler it ports):
    /// * `Notify` → `showExtensionNotify` (`:2518-2526`): `error` → `showError`, `warning` →
    ///   `showWarning`, otherwise `showStatus`.
    /// * `SetStatus` → `setExtensionStatus` (`:1920-1923`) → the footer's extension-status line.
    /// * `SetEditorText` → `this.editor.setText(text)` (`:2241`); `is_paste` (`pasteToEditor`,
    ///   `:2240`, which wraps the text in bracketed-paste markers and re-feeds the editor) → the
    ///   editor's real paste path, so the same sanitization applies.
    /// * `SetToolsExpanded` → `setToolsExpanded` (`:3887-3903`), including its no-op early-out and
    ///   its `Tool output: expanded|collapsed` status echo.
    /// * `SetTitle` → retained on [`AppState::terminal_title`]; the crossterm run loop writes the
    ///   OSC 0 sequence (`terminal.ts:504-507`), which a `TestBackend` app must not do.
    /// * `SetWidget`/`SetHeader`/`SetFooter` → retained on [`AppState`]. These now ARRIVE (they used
    ///   to be discarded before leaving `LiveHostServices`) but cyrup's TUI has no extension chrome
    ///   slot to draw them in, so TUI-014 stays open — see those fields' docs.
    ///
    /// `pub` for the same reason [`Self::open_extension_dialog`] is: `tests/*.rs` drive it directly.
    pub fn apply_ui_effect(&mut self, effect: UiEffect) {
        match effect {
            UiEffect::Notify { message, kind } => match kind {
                NotifyKind::Error => {
                    // Pi `showError` prefixes the copy (`interactive-mode.ts:3952`).
                    self.state.transcript.push_error(format!("Error: {message}"));
                }
                NotifyKind::Warning => {
                    self.state.transcript.push_warning(format!("Warning: {message}"));
                }
                NotifyKind::Info => self.state.transcript.push_status(message),
            },
            UiEffect::SetStatus { key, text } => {
                // `text: None` clears the key — `StatusLine::set_extension_status` already treats an
                // empty value as a removal (Pi `footer.ts:233`).
                self.state.status.set_extension_status(key, text.unwrap_or_default());
            }
            UiEffect::SetEditorText { text, is_paste } => {
                if is_paste {
                    self.state.editor.handle_paste(&text);
                } else {
                    self.state.editor.set_text(&text);
                }
            }
            UiEffect::SetToolsExpanded { expanded } => {
                if self.state.transcript.set_tool_expanded(expanded) {
                    self.state.transcript.push_status(format!(
                        "Tool output: {}",
                        if expanded { "expanded" } else { "collapsed" }
                    ));
                }
            }
            UiEffect::SetTitle { title } => self.state.terminal_title = Some(title),
            UiEffect::SetHeader { content } => self.state.extension_header = Some(content),
            UiEffect::SetFooter { content } => self.state.extension_footer = Some(content),
            UiEffect::SetWidget { widget } => self.state.extension_widget = Some(widget),
        }
    }

    /// Advance the open extension-UI dialog's countdown by one tick (Pi's `CountdownTimer`'s 1s
    /// `setInterval`, `countdown-timer.ts:21-30`): live-updates the selector's title with the
    /// remaining seconds, or — once the deadline has passed — auto-resolves the dialog to its
    /// per-kind deny default and closes the slot (Pi's `onExpire` → `onCancelCallback`,
    /// `extension-selector.ts:56`/`extension-input.ts:59`), exactly like an `Esc` cancel
    /// ([`App::handle_selector_key`]'s `Cancel` arm). A stale reply send (the host's OWN independent
    /// `ui_roundtrip` timeout already won the race) is a harmless no-op, same as every other reply
    /// site in this module. A no-op when no extension dialog is open or it has no timeout armed —
    /// callers gate the driving interval on this same condition so it costs nothing otherwise.
    ///
    /// `pub` for the same reason as [`Self::open_extension_dialog`]: `tests/*.rs` calls it directly
    /// to simulate the run loop's 1s tick without needing a real `tokio::time::sleep`.
    pub fn tick_extension_dialog_countdown(&mut self) {
        let Some((base_title, deadline)) = self
            .state
            .pending_ui_reply
            .as_ref()
            .and_then(|p| p.deadline.map(|d| (p.base_title.clone(), d)))
        else {
            return;
        };
        if tokio::time::Instant::now() >= deadline {
            if let Some(pending) = self.state.pending_ui_reply.take() {
                let _ = pending.reply.send(default_ui_reply(pending.kind));
            }
            self.close_selector(true);
        } else if let Some(active) = self.state.selector.as_mut() {
            active.inner.set_title(countdown_title(&base_title, deadline));
        }
    }

    /// Route one key to the active selector and act on the outcome (spec/tui/05 §3.1). `Confirm`
    /// applies the selection by kind and closes the slot; `Cancel` restores the prior theme (if any)
    /// and closes; `Preview` re-themes live without closing. A no-op if no selector is open.
    fn handle_selector_key(&mut self, key: &event::KeyEvent) -> AppAction {
        let Some(active) = self.state.selector.as_mut() else { return AppAction::None };
        let outcome = active.inner.handle(key, &self.state.select_keymap);
        let kind = active.kind;
        match outcome {
            SelectorOutcome::Ignored => AppAction::None,
            SelectorOutcome::Redraw => AppAction::Redraw,
            SelectorOutcome::Preview(value) => {
                // Theme live preview: re-theme the whole UI as the highlight moves
                // (`theme-selector.ts:54-56`). Other kinds never emit `Preview`.
                if kind == SelectorKind::Theme {
                    self.set_theme(UiTheme::builtin(&value));
                }
                AppAction::Redraw
            }
            SelectorOutcome::Confirm(value) => {
                // The login dialog is the one selector that does NOT close on confirm: submitting
                // answers the flow's in-flight `AuthInteraction::prompt` and the flow runs on —
                // Pi's `input.onSubmit` resolves `inputResolver` and leaves `editorContainer`
                // alone (`login-dialog.ts:56-64`), so the URL/device code stays on screen and a
                // second prompt can follow. The dialog is torn down by `finish_login` (the login
                // settled) or by the `Cancel` arm below.
                if kind == SelectorKind::LoginDialog {
                    if let Some(reply) = self.state.pending_login_prompt.take() {
                        let _ = reply.send(Ok(value));
                    }
                    return AppAction::Redraw;
                }
                let command = self.confirm_selector(kind, &value);
                self.close_selector(false);
                match command {
                    Some(c) => AppAction::Command(c),
                    None => AppAction::Redraw,
                }
            }
            SelectorOutcome::Apply(payload) => {
                // A `/tree` label save (`e` → `LabelInput` submit, tree_selector.rs) rides an
                // `"{entry_id}\u{1f}{label}"` `Apply` payload; the entry id is a UUID (never contains
                // the separator) so the split is unambiguous. Persist it via the session `set_label`
                // path and keep the slot open (the tree already refreshed its own `has_label` star).
                if kind == SelectorKind::Tree {
                    return match payload.split_once(crate::FIELD_SEP) {
                        Some((entry_id, label)) => AppAction::Command(AppCommand::SetEntryLabel {
                            entry_id: entry_id.to_string(),
                            label: label.to_string(),
                        }),
                        None => AppAction::Redraw,
                    };
                }
                // A `/resume` in-list delete/rename rides a unit-separator-*tagged* `Apply` payload
                // (`session_selector.rs`); decode it first so it never mis-routes to the settings
                // handler. The slot stays open (the selector already mutated its own row list).
                if let Some(action) = SessionSelectorOutcome::parse_apply(&payload) {
                    return match action {
                        SessionSelectorOutcome::Delete(path) => {
                            AppAction::Command(AppCommand::DeleteSession(path))
                        }
                        SessionSelectorOutcome::Rename { path, name } => {
                            AppAction::Command(AppCommand::RenameSession { path, name })
                        }
                        // `Resume` never arrives via `Apply` (it is a `Confirm`); ignore defensively.
                        SessionSelectorOutcome::Resume(_) => AppAction::Redraw,
                    };
                }
                // Otherwise a `/settings` row cycled in place: persist it live, keep the slot open
                // (Pi's settings selector applies on each `onChange`). The payload is `"id\u{1f}value"`.
                match payload.split_once(crate::FIELD_SEP) {
                    Some((id, value)) => AppAction::Command(AppCommand::ApplySetting {
                        id: id.to_string(),
                        value: value.to_string(),
                    }),
                    None => AppAction::Redraw,
                }
            }
            SelectorOutcome::Cancel => {
                // A cancelled extension-UI dialog resolves to its per-kind deny default (Pi's
                // `Esc`-cancelled select yields `undefined`, which `confirm`'s `result === Yes` then
                // reads as `false` — `interactive-mode.ts:2172-2179`) rather than hanging the
                // wasm-suspended guest until `ui_roundtrip`'s timeout (or forever, with none set).
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(default_ui_reply(pending.kind));
                }
                // `LoginDialogComponent.cancel()` (`login-dialog.ts:82-88`): abort the flow's signal
                // AND reject whatever prompt it is blocked on with `"Login cancelled"`. Without the
                // signal half, a flow parked on a callback server or a device-code poll (neither of
                // which is a prompt) would keep running with no dialog to talk to.
                if kind == SelectorKind::LoginDialog {
                    self.cancel_login();
                }
                self.close_selector(true);
                // The two `/tree` summarization prompts each have their OWN Escape destination in Pi
                // (`interactive-mode.ts:4761-4765`, `:4770-4773`), not a plain dismiss:
                match kind {
                    // "Summarize branch?" → back to the tree selector, same selection (`:4763`).
                    // `pending_tree_nav` is deliberately LEFT SET: the tree-open arm consumes it as
                    // the initial selection, which is what `showTreeSelector(entryId)` means.
                    SelectorKind::BranchSummary => {
                        return AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree));
                    }
                    // The custom-instructions editor → back to the prompt (Pi's `continue`, `:4772`),
                    // NOT out of the flow: the pending target is deliberately kept.
                    SelectorKind::BranchSummaryInstructions => {
                        self.open_branch_summary_prompt();
                        return AppAction::Redraw;
                    }
                    _ => {}
                }
                AppAction::Redraw
            }
            // A `/settings` submenu row (Pi `SettingItem.submenu`, settings-selector.ts:603-610):
            // replace the settings selector with the nested picker. Only `"theme"` exists today — the
            // theme picker with live preview (`ThemeSubmenu`); an unknown id is a defensive no-op.
            SelectorOutcome::OpenSubmenu(id) => {
                if id == "theme" {
                    self.open_selector(SelectorKind::Theme);
                }
                AppAction::Redraw
            }
            // `Ctrl+G` inside the extension `ui.editor` dialog (L4 review §3) — the actual
            // teardown+spawn+restore needs `&mut self: &mut App` (terminal access), which
            // `Selector::handle` doesn't have; bubble it up as an `AppAction` the run loop's
            // fallible `match` dispatches (mirrors the plain `Ctrl+G`/`AppAction::OpenExternalEditor`
            // arm right next to it).
            SelectorOutcome::OpenExternalEditor => AppAction::OpenExternalEditorForSelector,
        }
    }

    /// Apply a confirmed selection. The three dependency-free selectors (theme/thinking/show-images)
    /// are applied fully in-crate and return `None`; the data-bound selectors return an
    /// [`AppCommand::ConfirmSelection`] so the run loop applies the effect at the session layer (set
    /// model, switch branch, login…).
    fn confirm_selector(&mut self, kind: SelectorKind, value: &str) -> Option<AppCommand> {
        match kind {
            SelectorKind::Theme => {
                self.set_theme(UiTheme::builtin(value));
                self.state.transcript.push_status(format!("theme → {value}"));
                None
            }
            SelectorKind::Thinking => {
                self.state.thinking_level = value.to_string();
                self.state.status.set_thinking_level(value);
                // The editor's rule color is the always-visible thinking-level signal (spec/tui/03
                // §3.3) — keep it in lockstep with the selected level.
                self.state.editor.set_thinking_level(value);
                self.state.transcript.push_status(format!("thinking → {value}"));
                None
            }
            SelectorKind::ShowImages => {
                self.state.show_images = value == "yes";
                // TUI-007: the toggle governs TOOL-RESULT images too (Pi passes `showImages` into
                // every `ToolExecutionComponent`, interactive-mode.ts:3449), not just the editor's
                // attachment strip. Off ⇒ Pi's `[Image: …]` text stand-in.
                self.state.transcript.set_show_images(self.state.show_images);
                let label = if self.state.show_images { "inline" } else { "placeholder" };
                self.state.transcript.push_status(format!("images → {label}"));
                None
            }
            SelectorKind::ExtensionConfirm => {
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(UiReply::Confirm(value == "yes"));
                }
                None
            }
            SelectorKind::ExtensionSelect
            | SelectorKind::ExtensionInput
            | SelectorKind::ExtensionEditor => {
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(UiReply::Text(Some(value.to_string())));
                }
                None
            }
            // Unreachable: [`Self::handle_selector_key`] intercepts a login-dialog confirm before
            // it gets here (the dialog must NOT close on submit). Explicit rather than falling into
            // the `other` arm, which would emit a bogus `ConfirmSelection` command.
            SelectorKind::LoginDialog => {
                if let Some(reply) = self.state.pending_login_prompt.take() {
                    let _ = reply.send(Ok(value.to_string()));
                }
                None
            }
            other => Some(AppCommand::ConfirmSelection { kind: other, value: value.to_string() }),
        }
    }

    /// Execute a session/data-bound [`AppCommand`] against the live [`AgentSession`]
    /// (`setupEditorSubmitHandler` command handlers, interactive-mode.ts:2554-2734). Data-bound
    /// selectors source their rows here (spec/tui/05 §8 late-data population) and open via
    /// [`open_data_selector`](Self::open_data_selector); lifecycle/IO commands call the matching
    /// session method and surface a status line / info block. Errors degrade to a status line.
    ///
    /// KNOWN RESIDUAL (L4 review §2.1): this is still awaited **inline** in `App::run`'s `select!`
    /// loop, unlike [`AppAction::ExtensionShortcut`] (spawned — see that arm's comment for the
    /// deadlock this avoids). None of the match arms below call a guest capability directly, but a
    /// FEW `.await` a session-lifecycle op (`Reload`/`NewSession`/`Import`/the `Session`/`UserMessage`
    /// `ConfirmSelection` switch+fork paths/`Compact`) that dispatches `HostEvent::Session{Start,
    /// Shutdown,BeforeSwitch,BeforeFork,Compact}` to every live extension's hook
    /// (`session.rs` `dispatch_notify`/`vetoed`), and a guest SDK hook handler is handed the SAME
    /// `Ctx` a tool/shortcut handler gets (`cyrup-ext-sdk/src/ctx.rs`), so it COULD call
    /// `ctx.ui().*` mid-hook. If one ever does, this call site would deadlock exactly like the
    /// shortcut path did before the fix above — closing that residual needs restructuring
    /// `execute_command`'s state-mutating match arms so only the actual session/runtime `.await` runs
    /// off-task (mirroring the `bash_rx`/`shortcut_status_rx` channel-back pattern), which is a much
    /// larger refactor deliberately out of scope here. No Pi extension in the wild is known to call a
    /// UI dialog from a session-lifecycle hook (tool/shortcut/command handlers are the realistic
    /// corpus, both of which are deadlock-safe today), so this is a documented, narrow, follow-up-
    /// tracked gap rather than a silently-dropped one.
    pub async fn execute_command(
        &mut self,
        cmd: AppCommand,
        session: &Arc<AgentSession>,
        runtime: Option<&Arc<AgentSessionRuntime>>,
    ) {
        use AppCommand as C;
        match cmd {
            C::OpenSelector(SelectorKind::Model) => {
                // The bare `app.model.select` entry point (no search term) — same as `/model` with no
                // argument: the unfiltered picker.
                self.handle_model_command(session, None).await;
            }
            C::OpenSelector(SelectorKind::ScopedModels) => {
                // The scoped-models picker is the bespoke checkbox+reorder selector over the FULL
                // catalog (`scoped-models-selector.ts`): the catalog is every available model; the
                // current scope is `scoped_models()` (empty ⇒ "all enabled", Pi's `enabledIds = null`).
                let catalog: Vec<(String, String, String, Option<String>)> = session
                    .model_catalog()
                    .iter()
                    .map(|m| {
                        (m.id.to_string(), m.name.clone(), m.provider.to_string(), Some(m.provider.to_string()))
                    })
                    .collect();
                if catalog.is_empty() {
                    self.state.transcript.push_status("no models available (configure providers)");
                } else {
                    let scoped: Vec<String> =
                        session.scoped_models().into_iter().map(|sm| sm.model.id.to_string()).collect();
                    // Empty scope ⇒ "all enabled" (None); otherwise the explicit ordered set.
                    let enabled = if scoped.is_empty() { None } else { Some(scoped) };
                    self.open_checkbox_selector(catalog, enabled);
                }
            }
            C::OpenSelector(SelectorKind::UserMessage) => {
                // S22: the real `UserMessageSelectorComponent` (`user-message-selector.ts`) —
                // three lines per entry (message / `Message i of N` / blank) under a header that
                // sits ABOVE the top rule. The `Some(format!("message {}", i + 1))` description
                // this used to build is gone: the metadata line is the component's own, and its
                // text is `  Message ${position} of ${this.messages.length}` (`:66`).
                let rows: Vec<crate::UserMessageRow> = session
                    .user_messages_for_forking()
                    .await
                    .into_iter()
                    .map(|a| crate::UserMessageRow {
                        id: a.entry_id.to_string(),
                        text: a.text.clone(),
                    })
                    .collect();
                if rows.is_empty() {
                    self.state.transcript.push_status("no user messages to fork from");
                } else {
                    // `initialSelectedId` is unset here, so the constructor preselects the most
                    // recent message (`:24-26`) — the same row the old `last` index picked.
                    let selector = crate::UserMessageSelector::new(rows, None);
                    self.open_boxed_selector(SelectorKind::UserMessage, Box::new(selector));
                }
            }
            C::OpenSelector(SelectorKind::Tree) => {
                // `/tree` (tree-selector.ts): the real session DAG flattened via the new
                // `AgentSession::session_dag` getter (feature #2) — nodes with parent/depth/label/kind/
                // fold/leaf/label/timestamp — feeding the connector/fold/filter engine in
                // `tree_selector.rs`. This replaces the flat user-message spine the audit flagged as
                // "data-starved" so the selector renders the actual branch tree.
                let dag = session.session_dag().await;
                if dag.is_empty() {
                    self.state.transcript.push_status("no session history to navigate");
                } else {
                    let nodes: Vec<TreeNode> =
                        dag.iter().map(tree_node_from_dag).collect();
                    let mut tree = TreeSelector::new(nodes);
                    tree.set_keymap(self.state.tree_keymap.clone());
                    // `treeFilterMode` — the filter `/tree` OPENS with (Pi reads
                    // `settingsManager.getTreeFilterMode()` into `initialFilterMode` at
                    // `interactive-mode.ts:4644` and hands it to `TreeSelectorComponent`, which seeds
                    // `this.filterMode` at `tree-selector.ts:137`). Read per open, not cached, so a
                    // `/settings` change takes effect on the next `/tree` exactly as it does in Pi.
                    tree.set_filter(crate::tree_selector::FilterMode::from_setting(
                        &session.services().settings.effective().tree_filter_mode(),
                    ));
                    // Pi re-shows the tree AT THE SAME ENTRY after an escaped summarize prompt or an
                    // aborted summarization (`showTreeSelector(entryId)`,
                    // `interactive-mode.ts:4763,4807`); both paths park the id here.
                    if let Some(pending) = self.state.pending_tree_nav.take() {
                        tree.select_id(&pending.target);
                    }
                    self.open_boxed_selector(SelectorKind::Tree, Box::new(tree));
                }
            }
            C::LoginCommand(arg) => self.handle_login_command(session, arg).await,
            C::OpenSelector(SelectorKind::Login) => {
                // `showOAuthSelector("login")` → `showLoginAuthTypeSelector()`
                // (`interactive-mode.ts:5127-5130`), i.e. exactly a bare `/login`.
                self.handle_login_command(session, None).await;
            }
            C::OpenSelector(SelectorKind::Logout) => {
                // `showOAuthSelector("logout")` (`interactive-mode.ts:5132-5175`) →
                // `getLogoutProviderOptions()`: only providers with a STORED credential are listed,
                // each carrying its credential's `authType` (which picks the confirm message).
                let inputs = self.login_provider_inputs(session).await;
                let stored =
                    match cyrup_config::login::stored_credentials(&session.services().auth).await {
                        Ok(stored) => stored,
                        Err(e) => {
                            self.state.transcript.push_status(format!("logout error: {e}"));
                            return;
                        }
                    };
                let options = cyrup_config::login::logout_provider_options(&stored, &inputs);
                if options.is_empty() {
                    // Pi's verbatim copy (`interactive-mode.ts:5136-5138`).
                    self.state
                        .transcript
                        .push_status(cyrup_config::login::NO_STORED_CREDENTIALS);
                    return;
                }
                // S5/S21 — same component as `/login`, in `logout` mode (`:52`), which changes the
                // title (`:72`) and the empty-catalog copy (`:155-158`).
                let selector =
                    crate::OAuthSelector::new(crate::OAuthMode::Logout, &options, None);
                self.state.logout_options = options;
                self.open_boxed_selector(SelectorKind::Logout, Box::new(selector));
            }
            C::OpenSelector(SelectorKind::Settings) => {
                // `/settings` (settings-selector.ts): the curated toggle/choice grid sourced from the
                // live effective settings. Each row cycles in place on `Enter` and persists via
                // `ApplySetting` (Pi's settings selector applies on `onChange`).
                let rows = settings_rows(
                    session.services().settings.effective(),
                    &self.state.theme.name,
                    &self.state.keymap,
                );
                let inner: Box<dyn Selector> = Box::new(SettingsSelector::new("Settings", rows));
                self.open_boxed_selector(SelectorKind::Settings, inner);
            }
            C::OpenSelector(SelectorKind::Trust) => {
                // `/trust` (trust-selector.ts): the yes/parent/no option list under a cwd + saved-
                // decision header. Confirming writes the trust store (`write_project_trust`).
                let options = session.project_trust_options();
                let cwd = session.services().cwd.display().to_string();
                let saved = session.saved_trust_decision();
                let saved_label = format_saved_trust(&saved);
                // Pi `isSavedOption` (`trust-selector.ts:92-98`): the option whose trust flag AND
                // saved path both match the persisted decision. `selectedIndex` falls back to 0
                // when there is none (`Math.max(0, findIndex(...))`, `:45-48`), but the ` ✓`
                // marker is driven by the predicate itself (`:109-110`), so keep both (S20).
                let saved_index = options.iter().position(|o| {
                    saved.as_ref().is_some_and(|s| {
                        s.decision.is_trusted() == o.trusted
                            && o.saved_path.as_deref() == Some(s.path.as_path())
                    })
                });
                let selected = saved_index.unwrap_or(0);
                let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();
                let inner: Box<dyn Selector> = Box::new(
                    TrustSelector::new(
                        cwd,
                        saved_label,
                        session.services().project_trusted,
                        labels,
                        selected,
                    )
                    .with_saved_index(saved_index)
                    // `keyHint("tui.select.confirm", "save")` / `…cancel` (`trust-selector.ts:
                    // 78-82`) read `getKeybindings()`, so the row must be built from the app's
                    // merged table — `handle` only adopts it once a key has already been pressed,
                    // which is one paint too late.
                    .with_hints(&self.state.select_keymap),
                );
                self.open_boxed_selector(SelectorKind::Trust, inner);
            }
            C::OpenSelector(SelectorKind::Session) => {
                // `/resume` (session-selector.ts): the persisted-session list for this cwd, newest
                // first, sourced via the additive `list_sessions` seam. Confirming carries the chosen
                // session file path; the actual runtime swap is driven by the L7 `SessionRuntime`
                // (`switch_session`) once the runtime is threaded into the run loop (residual gap #3).
                let sessions = session.list_sessions();
                if sessions.is_empty() {
                    self.state.transcript.push_status("no saved sessions to resume");
                } else {
                    let current = session.session_id().to_string();
                    let rows: Vec<SessionRow> = sessions
                        .iter()
                        .map(|s| {
                            let label = session_label(s);
                            let is_current = s.id.to_string() == current;
                            let desc = format!(
                                "{} msgs{}",
                                s.message_count,
                                if is_current { " (current)" } else { "" }
                            );
                            // The query-DSL search text (`getSessionSearchText`,
                            // session-selector-search.ts:26): `{id} {name} {allMessagesText} {cwd}`.
                            let search_text = format!(
                                "{} {} {} {}",
                                s.id,
                                s.name.as_deref().unwrap_or(""),
                                s.all_messages_text,
                                s.cwd
                            );
                            SessionRow {
                                path: s.path.display().to_string(),
                                label,
                                name: s.name.clone(),
                                desc: Some(desc),
                                search_text,
                                recency: system_time_nanos(s.modified),
                            }
                        })
                        .collect();
                    // `new SessionSelectorComponent(..., { keybindings }, currentSessionFilePath)`
                    // (`interactive-mode.ts:4867-4884`): the picker is handed the live keybindings
                    // AND the running session's file path, and each `SessionInfo` carries its
                    // `parentSessionPath` (`session-manager.ts` → `session-selector.ts:222`).
                    // Without those three the threaded view has no edges to draw, the row you are
                    // sitting in is not accented, and the hint rows name stock keys.
                    let mut selector = SessionSelector::new(rows)
                        .with_keymaps(&self.state.session_keymap, self.state.editor.keymap_ref());
                    selector.set_parent_paths(sessions.iter().filter_map(|s| {
                        s.parent_session_path
                            .as_ref()
                            .map(|p| (s.path.display().to_string(), p.display().to_string()))
                    }));
                    // `options?.showRenameHint ?? this.canRename` (`session-selector.ts:772`):
                    // upstream's host declares the capability by passing a `renameSession`
                    // callback. cyrup's is the `SessionSelectorOutcome::Rename` arm below, which
                    // lands in `session.rename_session_file` — so the capability is present and the
                    // hint is on. Stated here rather than defaulted in the component, because the
                    // component cannot know whether its host wired the apply path.
                    selector.set_show_rename_hint(true);
                    // `currentSessionFilePath` — resolved from the listing rather than the manager
                    // so it is the SAME string the rows carry (a canonicalization mismatch would
                    // silently never match).
                    selector.set_current_session_path(
                        sessions
                            .iter()
                            .find(|s| s.id.to_string() == current)
                            .map(|s| s.path.display().to_string()),
                    );
                    let inner: Box<dyn Selector> = Box::new(selector);
                    self.open_boxed_selector(SelectorKind::Session, inner);
                }
            }
            C::OpenSelector(other) => {
                // Any remaining kind has no in-crate sourcing yet; surface the request (no silent drop).
                self.state.transcript.push_status(format!("{} selector unavailable", other.title()));
            }
            C::ConfirmSelection { kind: SelectorKind::Tree, value } => {
                // Confirming a tree row ASKS ABOUT SUMMARIZATION FIRST (Pi
                // `interactive-mode.ts:4744-4779`), then navigates. Before SESS-023 this arm called
                // `navigate_tree(.., NavigateTreeOptions::default())` — `summarize` hard-false — so
                // the entire branch-summary stack was unreachable from the shipped binary and
                // `branchSummary.skipPrompt` was a no-op.
                //
                // Pi's `getBranchSummarySkipPrompt()` gate (`:4753`) is a FRONT-END decision: when
                // set, skip the prompt entirely and navigate with `wantsSummary = false`.
                if session.services().settings.effective().branch_summary_skip_prompt() {
                    self.begin_tree_navigation(session, value, false, None).await;
                } else {
                    self.state.pending_tree_nav = Some(PendingTreeNav { target: value });
                    self.open_branch_summary_prompt();
                }
            }
            C::ConfirmSelection { kind: SelectorKind::BranchSummary, value } => {
                // The three-option answer (Pi `:4755-4777`). `custom` opens the instructions editor
                // and keeps the pending target; the other two dispatch the navigation directly.
                // `wantsSummary = summaryChoice !== "No summary"` (`:4767`).
                if value == BRANCH_SUMMARY_CUSTOM {
                    self.open_branch_summary_instructions();
                } else {
                    let Some(pending) = self.state.pending_tree_nav.take() else { return };
                    let summarize = value != BRANCH_SUMMARY_NONE;
                    self.begin_tree_navigation(session, pending.target, summarize, None).await;
                }
            }
            C::ConfirmSelection { kind: SelectorKind::BranchSummaryInstructions, value } => {
                // Pi `showExtensionEditor` returned a string (`:4769`): a complete choice, so the
                // prompt loop breaks and the navigation runs with `summarize: true`. An EMPTY string
                // is still a value (only `undefined`/Escape loops back), so it is forwarded as
                // `None` custom instructions rather than an empty override.
                let Some(pending) = self.state.pending_tree_nav.take() else { return };
                let instructions = (!value.trim().is_empty()).then_some(value);
                self.begin_tree_navigation(session, pending.target, true, instructions).await;
            }
            C::ConfirmSelection { kind: SelectorKind::Model, value } => {
                match session.set_model(&value).await {
                    Ok(_) => self.state.transcript.push_status(format!("model → {value}")),
                    Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
                }
            }
            C::SetEntryLabel { entry_id, label } => {
                // Persist the `/tree` label edit via the SAME live `set_label` path a loaded
                // extension's `setLabel` uses (`LiveHostServices::set_label` → `manager.append_label`,
                // host_services.rs:866). An empty label removes it (`apply_label` drops empty labels),
                // matching Pi's `value || undefined`. Silently degrades (unknown id / busy), like Pi.
                session.services().host_services.set_label(&entry_id, &label);
                let msg = if label.is_empty() {
                    "label removed".to_string()
                } else {
                    format!("label → {label}")
                };
                self.state.transcript.push_status(msg);
            }
            C::ModelCommand(search) => {
                // `/model [text]` (`handleModelCommand`, interactive-mode.ts:4175-4196): exact match
                // sets directly; a partial (or bare) opens the picker pre-filtered to `search`.
                self.handle_model_command(session, search).await;
            }
            C::CycleModel(direction) => {
                // The cycle set is the scoped models when a scope is active, else the full available
                // catalog (Pi `cycleModel`: `scopedModels.length > 0 ? scoped : available`). Cycling by
                // model id mirrors the `/model` confirm path (`set_model(&value)` above); the footer
                // re-reads the new model off the `ModelChanged` event this triggers.
                let scoped = session.scoped_models();
                let scoped_active = !scoped.is_empty();
                let cycle: Vec<(String, String, String)> = if scoped_active {
                    scoped
                        .iter()
                        .map(|sm| {
                            (sm.model.id.to_string(), sm.model.provider.to_string(), sm.model.name.clone())
                        })
                        .collect()
                } else {
                    session
                        .available_model_catalog()
                        .iter()
                        .map(|m| (m.id.to_string(), m.provider.to_string(), m.name.clone()))
                        .collect()
                };
                if cycle.len() <= 1 {
                    let msg =
                        if scoped_active { "Only one model in scope" } else { "Only one model available" };
                    self.state.transcript.push_status(msg);
                } else {
                    let current = session.model();
                    let n = cycle.len();
                    // `session.model()` is `Option` (pi `AgentSession.model: Model | undefined`,
                    // agent-session.ts:866-868): a modelless session matches nothing, so the cycle
                    // starts at the head exactly as pi's `findIndex === -1 ⇒ 0` does
                    // (agent-session.ts:1650-1653).
                    let cur = cycle.iter().position(|(id, prov, _)| {
                        current.as_ref().is_some_and(|c| {
                            id == c.model.as_str() && prov == c.provider.as_str()
                        })
                    });
                    let next = match direction {
                        CycleDirection::Forward => cur.map_or(0, |i| (i + 1) % n),
                        CycleDirection::Backward => cur.map_or(0, |i| (i + n - 1) % n),
                    };
                    if let Some((id, _prov, name)) = cycle.get(next) {
                        match session.set_model(id).await {
                            Ok(_) => self.state.transcript.push_status(format!("Switched to {name}")),
                            Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
                        }
                    }
                }
            }
            C::CycleThinking => {
                // Pi `cycleThinkingLevel` (interactive-mode.ts:3606-3614): call the session's cycle,
                // which is gated on `supportsThinking()` (agent-session.ts:1599-1608) and walks the
                // model's OWN supported levels (`getSupportedThinkingLevels`, incl. `xhigh` where
                // mapped). `Ok(None)` ⇒ the model does not reason: show the exact Pi status and change
                // NOTHING. `Ok(Some(level))` ⇒ `set_thinking_level` already emitted
                // `ThinkingLevelChanged`, so the footer + editor rule re-color off that event
                // (mirroring Pi's `footer.invalidate()` + `updateEditorBorderColor()`); here we only
                // surface Pi's `Thinking level: {level}` status line.
                match session.cycle_thinking_level().await {
                    Ok(None) => {
                        self.state.transcript.push_status("Current model does not support thinking");
                    }
                    Ok(Some(level)) => {
                        let label = match level {
                            ModelThinkingLevel::Off => "off",
                            ModelThinkingLevel::Minimal => "minimal",
                            ModelThinkingLevel::Low => "low",
                            ModelThinkingLevel::Medium => "medium",
                            ModelThinkingLevel::High => "high",
                            ModelThinkingLevel::Xhigh => "xhigh",
                            ModelThinkingLevel::Max => "max",
                        };
                        self.state.transcript.push_status(format!("Thinking level: {label}"));
                    }
                    Err(e) => self.state.transcript.push_status(format!("thinking error: {e}")),
                }
            }
            C::ConfirmSelection { kind: SelectorKind::ScopedModels, value } => {
                // The checkbox selector confirms with the ordered enabled ids (`\n`-joined), or the
                // `SCOPED_MODELS_ALL` sentinel for "all enabled". Rebuild the scoped set from the
                // catalog and persist via `set_scoped_models` (`scoped-models-selector.ts onPersist`).
                let catalog = session.model_catalog();
                let ordered_ids: Vec<String> = if value == crate::SCOPED_MODELS_ALL {
                    catalog.iter().map(|m| m.id.to_string()).collect()
                } else {
                    value.split('\n').filter(|s| !s.is_empty()).map(str::to_string).collect()
                };
                let scoped: Vec<cyrup_session_svc::ScopedModel> = ordered_ids
                    .iter()
                    .filter_map(|id| catalog.iter().find(|m| m.id.to_string() == *id))
                    .map(|m| cyrup_session_svc::ScopedModel {
                        model: m.clone(),
                        thinking_level: None,
                    })
                    .collect();
                let n = scoped.len();
                session.set_scoped_models(scoped);
                self.state.transcript.push_status(format!("scoped models → {n} enabled"));
            }
            C::ConfirmSelection { kind: SelectorKind::UserMessage, value } => {
                // `/fork` (user-message-selector.ts): fork at the chosen entry. With the runtime
                // threaded in, drive `AgentSessionRuntime::fork` so the runtime swaps to the new
                // branched session and the UI re-binds on the generation bump (Pi `fork`,
                // agent-session-runtime.ts:259); `position:"before"` re-seeds the editor with the
                // anchor text. Without a runtime (SDK/embedder), fall back to the in-place
                // `fork_at_entry` (no swap).
                let entry = cyrup_core::EntryId::from(value.as_str());
                match runtime {
                    Some(rt) => match rt.fork(entry, ForkPosition::Before).await {
                        Ok(r) if r.cancelled => {
                            self.state.transcript.push_status("fork cancelled")
                        }
                        Ok(r) => {
                            if let Some(text) = r.selected_text {
                                self.state.editor.set_text(&text);
                            }
                            self.state.pending_swap_status = Some("forked from message".into());
                        }
                        Err(e) => self.state.transcript.push_status(format!("fork error: {e}")),
                    },
                    None => match session.fork_at_entry(&entry, ForkPosition::Before).await {
                        Ok(_) => self.state.transcript.push_status("forked from message"),
                        Err(e) => self.state.transcript.push_status(format!("fork error: {e}")),
                    },
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Logout, value } => {
                // `/logout` onSelect (`interactive-mode.ts:5149-5166`): `modelRuntime.logout(id)` —
                // the ported `cyrup_config::login::logout`, which wraps a store failure as
                // `Credential store delete failed for …` (`ai/src/models.ts:446-452`). Env vars and
                // `models.json` are untouched, which is what the second message spells out.
                let Some(option) = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| self.state.logout_options.get(i))
                    .cloned()
                else {
                    // `if (!providerOption) return;` (`:5151-5153`).
                    return;
                };
                match cyrup_config::login::logout(&*session.services().auth, &option.id).await {
                    Ok(()) => {
                        // The credential is gone, so the auth snapshot pi's footer reads has lost
                        // this provider (`modelRuntime.logout` updates it before the repaint at
                        // `interactive-mode.ts:5388-5394`). Drop it here too, or ` (sub)` would
                        // survive the logout that removed the subscription credential.
                        self.state
                            .oauth_credential_providers
                            .remove(option.id.as_str());
                        self.refresh_subscription_marker();
                        let name = &option.name;
                        // Pi's two verbatim messages (`:5157-5161`).
                        let message = if option.auth_type == AuthType::Oauth {
                            format!("Logged out of {name}")
                        } else {
                            format!(
                                "Removed stored API key for {name}. Environment variables and \
                                 models.json config are unchanged."
                            )
                        };
                        self.state.transcript.push_status(message);
                    }
                    // `showError(\`Logout failed: ${message}\`)` (`:5163-5165`).
                    Err(e) => self
                        .state
                        .transcript
                        .push_error(format!("Logout failed: {e}")),
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Login, value } => {
                // `OAuthSelectorComponent`'s onSelect (`interactive-mode.ts:5106-5117`): re-find the
                // chosen option and `startProviderLogin(providerOption)`. The value is the row INDEX
                // (see `SelectorKind::Login`), which is what `(providerId, authType)` collapses to.
                let Some(option) = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| self.state.login_options.get(i))
                    .cloned()
                else {
                    // `if (!providerOption) return;` (`:5113-5115`).
                    return;
                };
                self.begin_provider_login(session, option);
            }
            C::ConfirmSelection { kind: SelectorKind::LoginAuthType, value } => {
                // `showLoginAuthTypeSelector`'s onSelect (`interactive-mode.ts:5063-5073`): with a
                // pinned provider, start ITS option of the chosen kind; otherwise open the provider
                // picker filtered to that kind.
                let auth_type = if value == AuthType::Oauth.as_str() {
                    AuthType::Oauth
                } else {
                    AuthType::ApiKey
                };
                match self.state.login_auth_type_options.take() {
                    Some(options) => {
                        // `providerOptions.find(p => p.authType === authType)` (`:5066`).
                        if let Some(option) =
                            options.iter().find(|o| o.auth_type == auth_type).cloned()
                        {
                            self.begin_provider_login(session, option);
                        }
                    }
                    // `this.showLoginProviderSelector(authType)` (`:5071`).
                    None => {
                        let inputs = self.login_provider_inputs(session).await;
                        self.open_login_provider_selector(&inputs, Some(auth_type), None);
                    }
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Trust, value } => {
                // The trust selector confirms with the chosen option INDEX; re-derive the options and
                // persist that option's store updates (Pi `/trust` `onSelect` → trust-store write).
                let options = session.project_trust_options();
                match value.parse::<usize>().ok().and_then(|i| options.get(i)) {
                    Some(opt) => match session.write_project_trust(&opt.updates) {
                        Ok(()) => {
                            let label = if opt.trusted { "trusted" } else { "untrusted" };
                            self.state.transcript.push_status(format!(
                                "project trust → {label} (/reload to apply to this session)"
                            ));
                        }
                        Err(e) => self.state.transcript.push_status(format!("trust error: {e}")),
                    },
                    None => self.state.transcript.push_status("trust selection cancelled"),
                }
            }
            C::ConfirmSelection { kind: SelectorKind::Session, value } => {
                // `/resume` swap (handleResumeSession, interactive-mode.ts): switch the runtime to the
                // chosen session file (Pi `switchSession`, agent-session-runtime.ts:193). The runtime
                // asserts the resumed cwd still exists, rebuilds cwd-bound services, and bumps the
                // generation; the UI re-binds on the bump. Without a runtime, surface the path.
                match runtime {
                    Some(rt) => match rt.switch_session(value.clone()).await {
                        Ok(r) if r.cancelled => {
                            self.state.transcript.push_status("resume cancelled")
                        }
                        Ok(_) => {
                            self.state.pending_swap_status = Some(format!("resumed {value}"));
                        }
                        Err(e) => self.state.transcript.push_status(format!("resume error: {e}")),
                    },
                    None => self
                        .state
                        .transcript
                        .push_status(format!("resume {value} (/reload to switch)")),
                }
            }
            C::DeleteSession(path) => {
                // `/resume` in-list delete (`onDeleteSession`): remove the persisted JSONL via the
                // additive `delete_session_file` seam (refuses the active session).
                match session.delete_session_file(std::path::Path::new(&path)) {
                    Ok(()) => self.state.transcript.push_status(format!("deleted session {path}")),
                    Err(e) => self.state.transcript.push_status(format!("delete error: {e}")),
                }
            }
            C::RenameSession { path, name } => {
                // `/resume` in-list rename (`onRenameSession`): persist a `session_info` name on the
                // target file via the additive `rename_session_file` seam.
                match session.rename_session_file(std::path::Path::new(&path), &name).await {
                    Ok(()) => self.state.transcript.push_status(format!("renamed session → {name}")),
                    Err(e) => self.state.transcript.push_status(format!("rename error: {e}")),
                }
            }
            C::ConfirmSelection { kind, value } => {
                self.state.transcript.push_status(format!("{} → {value}", kind.title()));
            }
            C::ApplySetting { id, value } => {
                // Persist a `/settings` toggle/choice live (Global scope; Pi's settings selector
                // writes the global layer). The `/reload` re-reads the effective view.
                let json = parse_setting_value(&value);
                // `outputPad` also takes effect ON SCREEN immediately (Pi `onOutputPadChange` →
                // `this.outputPad = padding` + re-render, interactive-mode.ts:4127-4136), unlike the
                // settings that only rebind on `/reload`: push the new pad into the live transcript so
                // the chat horizontal padding changes the moment the row is cycled.
                if id == "outputPad" {
                    let pad = if value == "0" { 0 } else { 1 };
                    self.state.transcript.set_output_pad(pad);
                }
                // `hideThinkingBlock` likewise takes effect live (Pi `setHideThinkingBlock`,
                // assistant-message.ts:57-62) — on the in-flight reasoning block and on every entry
                // committed after the flip. Pi additionally re-renders the ALREADY-shown assistant
                // messages; cyrup's committed rows have left the render tree for native scrollback
                // (`flush_committed` → `insert_before`), so history keeps the form it committed with.
                if id == "hideThinkingBlock" {
                    self.state.transcript.set_hide_thinking_block(value == "true");
                }
                // The image rows are live too (Pi re-reads them per `ToolExecutionComponent`).
                if id == "terminal.showImages" {
                    self.state.show_images = value == "true";
                    self.state.transcript.set_show_images(self.state.show_images);
                }
                // `terminal.showTerminalProgress` is live in Pi by construction — its gate is
                // `getShowTerminalProgress()` re-read at every call site, so a flip takes effect on
                // the next transition with no handler doing anything but persisting
                // (`onShowTerminalProgressChange`, `interactive-mode.ts:4311-4313`). cyrup caches
                // the gate on `AppState`, so the flip has to be pushed into it here or the row would
                // not take effect until the next session bind. Turning the row OFF while an
                // indicator is lit also parks a clear — see the `[CYRUP-DELTA]` on
                // `TerminalProgress::set_enabled`.
                if id == "terminal.showTerminalProgress" {
                    self.state.terminal_progress.set_enabled(value == "true");
                }
                if id == "terminal.imageWidthCells"
                    && let Ok(cells) = value.parse::<u16>()
                {
                    self.state.transcript.set_image_width_cells(cells);
                }
                // `editorPaddingX` is live in Pi too — `onEditorPaddingChange` writes the setting and
                // then calls `setPaddingX` on the live editor (`settings-selector.ts:687-689` →
                // `interactive-mode.ts:5393-5399`), so the rules re-inset on the very next frame.
                if id == "editorPaddingX"
                    && let Ok(pad) = value.parse::<i64>()
                {
                    self.state.editor.set_padding_x(pad);
                }
                // Same for `showHardwareCursor` (Pi `onShowHardwareCursorChange` →
                // `ui.setShowHardwareCursor(enabled)`, `tui.ts:346-352`, which hides the cursor
                // immediately when turned off rather than waiting for a rebind).
                if id == "showHardwareCursor" {
                    self.state.editor.set_show_hardware_cursor(value == "true");
                }
                // `enableSkillCommands` gates the `skill:<name>` half of the `/` menu
                // (`interactive-mode.ts:613`); Pi rebuilds the autocomplete provider on the change,
                // so rebuild the registry from the SAME catalog with the new gate.
                if id == "enableSkillCommands" {
                    self.state
                        .editor
                        .set_registry(crate::commands::CommandRegistry::with_dynamic(
                            crate::commands::dynamic_commands_from_catalog_gated(
                                &session.slash_command_catalog(),
                                value == "true",
                            ),
                        ));
                }
                // `transport` is live in Pi too, and it is the ONLY row whose live half touches the
                // agent rather than the UI: `onTransportChange` persists the setting AND assigns
                // `this.session.agent.transport = transport` (`interactive-mode.ts:4213-4216`), so
                // the very next request streams with the chosen transport. cyrup persisted only,
                // which left `AgentBuilder::transport`'s build-time seed in force until restart.
                if id == "transport" {
                    session.set_transport(&value).await;
                }
                match session.persist_setting(cyrup_session_svc::SettingsScope::Global, &id, json) {
                    Ok(()) => self.state.transcript.push_status(format!("{id} → {value}")),
                    Err(e) => self.state.transcript.push_status(format!("settings error: {e}")),
                }
            }
            C::Compact(arg) => match session.compact(arg).await {
                // Render the compaction-summary message (`compaction-summary-message.ts`): the
                // `[compaction]` label + `**Compacted from N tokens**` markdown body produced by the
                // op (Pi appends a `CompactionSummaryMessage` after a manual `/compact`).
                Ok(result) => {
                    self.state
                        .transcript
                        .push_compaction_summary(result.tokens_before, result.summary);
                }
                // A refusal (nothing to compact / already compacted / an extension veto) is now an
                // `Err` carrying Pi's reason string, so the status line names WHY instead of the old
                // undifferentiated "nothing to compact".
                Err(e) => self.state.transcript.push_status(format!("compact error: {e}")),
            },
            C::Clone => match session.clone_at(None).await {
                Ok(id) => self.state.transcript.push_status(format!("cloned session → {id}")),
                Err(e) => self.state.transcript.push_status(format!("clone error: {e}")),
            },
            C::Export(arg) => {
                // Format chosen **by extension**, matching Pi (`handleExportCommand`,
                // interactive-mode.ts:5106-5112): a `.jsonl` target writes the raw transcript;
                // every other target (including no path) writes a styled HTML document — HTML is the
                // default. cyrup renders the HTML body in-crate (`export::session_jsonl_to_html`) over
                // the session's own JSONL; the rich tool-card renderer is the L5 residual.
                let is_jsonl =
                    arg.as_deref().is_some_and(|p| p.trim_end().to_ascii_lowercase().ends_with(".jsonl"));
                if is_jsonl {
                    let path = arg.as_deref().map(std::path::Path::new);
                    match session.export_to_jsonl(path).await {
                        Ok(_) => {
                            let label = arg.unwrap_or_default();
                            self.state.transcript.push_status(format!("exported session → {label}"));
                        }
                        Err(e) => self.state.transcript.push_status(format!("export error: {e}")),
                    }
                } else {
                    // Pull the transcript as JSONL (no path ⇒ returned as text), render to HTML, write.
                    match session.export_to_jsonl(None).await {
                        Ok(Some(jsonl)) => {
                            let html = crate::export::session_jsonl_to_html(&jsonl);
                            match &arg {
                                Some(path) => match std::fs::write(path, &html) {
                                    Ok(()) => self
                                        .state
                                        .transcript
                                        .push_status(format!("exported session → {path}")),
                                    Err(e) => self
                                        .state
                                        .transcript
                                        .push_status(format!("export error: {e}")),
                                },
                                None => self.state.transcript.push_block("Session (HTML)", html),
                            }
                        }
                        Ok(None) => self.state.transcript.push_status("exported session"),
                        Err(e) => self.state.transcript.push_status(format!("export error: {e}")),
                    }
                }
            }
            C::SetName(name) => match session.set_session_name(&name).await {
                Ok(()) => self.state.transcript.push_status(format!("session name → {name}")),
                Err(e) => self.state.transcript.push_status(format!("name error: {e}")),
            },
            C::Copy => match session.last_assistant_text().await {
                Some(text) => {
                    let n = text.chars().count();
                    copy_to_clipboard(&text);
                    self.state.transcript.push_status(format!("copied last message ({n} chars)"));
                }
                None => self.state.transcript.push_status("no assistant message to copy"),
            },
            C::SessionInfo => {
                // Pi's `/session` renderer (interactive-mode.ts:5724-5763) reads exactly these
                // fields off `getSessionStats()`; cyrup renders them as its own markdown table.
                let stats = session.session_stats().await;
                let body = format!(
                    "| Field | Value |\n|-------|-------|\n\
                     | file | {} |\n| id | {} |\n\
                     | messages | {} |\n| user | {} |\n| assistant | {} |\n\
                     | tool calls | {} |\n| tool results | {} |\n\
                     | input tokens | {} |\n| output tokens | {} |\n\
                     | cache read | {} |\n| cache write | {} |\n| total tokens | {} |\n\
                     | cost | ${:.3} |\n",
                    stats.session_file.as_deref().unwrap_or("In-memory"),
                    stats.session_id,
                    stats.total_messages,
                    stats.user_messages,
                    stats.assistant_messages,
                    stats.tool_calls,
                    stats.tool_results,
                    stats.tokens.input,
                    stats.tokens.output,
                    stats.tokens.cache_read,
                    stats.tokens.cache_write,
                    stats.tokens.total,
                    stats.cost,
                );
                self.state.transcript.push_block("Session", body);
            }
            // Session-lifecycle ops drive the `AgentSessionRuntime` (arch-11 §3.4): the op rebuilds
            // the active session + bumps the generation, and the run loop's generation-watch arm
            // re-binds the UI (re-subscribe + reset transcript) → `pending_swap_status`. Without a
            // runtime (SDK/embedder), surface the request so the path is real (no silent drop).
            C::NewSession => match runtime {
                // `/new` (handleClearCommand): start a fresh session in the same cwd (Pi `newSession`).
                Some(rt) => match rt.new_session().await {
                    Ok(r) if r.cancelled => {
                        self.state.transcript.push_status("new session cancelled")
                    }
                    Ok(_) => self.state.pending_swap_status = Some("started a new session".into()),
                    Err(e) => self.state.transcript.push_status(format!("new session error: {e}")),
                },
                None => self.state.transcript.push_status("starting new session…"),
            },
            C::Reload => match runtime {
                // `/reload` (handleReloadCommand): rebuild the active session in place (Pi `reload`,
                // agent-session.ts:2451) — re-reads settings/resources/keybindings, resets the
                // provider, preserves the persisted transcript.
                Some(rt) => match rt.reload(None).await {
                    Ok(()) => self.state.pending_swap_status = Some("reloaded resources".into()),
                    Err(e) => self.state.transcript.push_status(format!("reload error: {e}")),
                },
                None => self.state.transcript.push_status("reloading resources…"),
            },
            C::Import(p) => match (runtime, p) {
                // `/import <path>` (handleImportCommand): copy + resume a JSONL session (Pi
                // `importFromJsonl`, agent-session-runtime.ts:353).
                (Some(rt), Some(path)) => match rt.import_from_jsonl(path.clone(), None).await {
                    Ok(r) if r.cancelled => self.state.transcript.push_status("import cancelled"),
                    Ok(_) => {
                        self.state.pending_swap_status = Some(format!("imported session {path}"))
                    }
                    Err(e) => self.state.transcript.push_status(format!("import error: {e}")),
                },
                (Some(_), None) => self.state.transcript.push_status("usage: /import <path>"),
                (None, p) => self
                    .state
                    .transcript
                    .push_status(format!("importing session {}", p.unwrap_or_default())),
            },
            C::Share => self.share_session(session).await,
        }
    }

    /// `/share` (`handleShareCommand`, interactive-mode.ts:5191): export the session to HTML, write a
    /// temp file, then shell `gh gist create --public=false <file>` behind a [`BorderedLoader`] and
    /// surface the resulting gist URL. `gh` missing / logged-out / failing degrades to a status line
    /// (Pi's `showError` paths). Fully in-crate (the HTML body is rendered by [`crate::export`]).
    async fn share_session(&mut self, session: &Arc<AgentSession>) {
        use tokio::process::Command;
        // Render the session HTML over its own JSONL (the same body `/export` writes).
        let html = match session.export_to_jsonl(None).await {
            Ok(Some(jsonl)) => crate::export::session_jsonl_to_html(&jsonl),
            Ok(None) => {
                self.state.transcript.push_status("nothing to share (empty session)");
                return;
            }
            Err(e) => {
                self.state.transcript.push_status(format!("share export error: {e}"));
                return;
            }
        };
        let tmp = std::env::temp_dir().join(format!("cyrup-session-{}.html", session.session_id()));
        if let Err(e) = std::fs::write(&tmp, html.as_bytes()) {
            self.state.transcript.push_status(format!("share write error: {e}"));
            return;
        }
        // Show the bordered loader in the editor slot while gh runs (Pi's `BorderedLoader`).
        // `keyHint("tui.select.cancel", "cancel")` (`bordered-loader.ts:36`) — the SELECT-tier
        // action, and `keyText` joins every bound key with `/` (`keybinding-hints.ts:29-36`), so the
        // stock hint reads `escape/ctrl+c cancel`. This used `Keymap::key_label(Action::Interrupt)`,
        // a different action resolved to its FIRST key only, which both named the wrong binding and
        // silently hid the second key the user can actually press.
        self.state.loader = Some(crate::chrome::BorderedLoader::cancellable(
            "Creating gist...",
            self.state
                .select_keymap
                .keys_label(SelectAction::Cancel)
                .unwrap_or_else(|| "escape/ctrl+c".into()),
        ));
        let result = Command::new("gh")
            .args(["gist", "create", "--public=false"])
            .arg(&tmp)
            .output()
            .await;
        self.state.loader = None;
        let _ = std::fs::remove_file(&tmp);
        match result {
            Ok(out) if out.status.success() => {
                let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if url.is_empty() {
                    self.state.transcript.push_status("gist created (no URL returned by gh)");
                } else {
                    self.state.transcript.push_block("Shared session", url);
                }
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let msg = err.trim();
                let detail = if msg.is_empty() { "gh gist create failed" } else { msg };
                self.state.transcript.push_status(format!("share error: {detail}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self
                .state
                .transcript
                .push_status("GitHub CLI (gh) is not installed — see https://cli.github.com/"),
            Err(e) => self.state.transcript.push_status(format!("share error: {e}")),
        }
    }

    /// Close the selector slot and restore the editor (spec/tui/05 §7 `done()`). When `cancelled` and
    /// a theme was being previewed, the prior theme is restored first.
    fn close_selector(&mut self, cancelled: bool) {
        if let Some(active) = self.state.selector.take() {
            if cancelled && let Some(theme) = active.restore_theme {
                self.set_theme(theme);
            }
            self.state.editor.set_text(&active.saved_editor);
        }
    }

    /// Fold an `AgentSessionEvent` into the UI state.
    ///
    /// Decodes tool names + error flag, model changes, queue depth, compaction, the live streaming
    /// **delta** text (`MessageUpdate` → [`Self::ingest_stream_event`] →
    /// [`TranscriptView::push_assistant_delta`](crate::transcript::TranscriptView::push_assistant_delta)),
    /// and the **terminal** assistant message (recovered via `StreamEvent::terminal_message()`, which
    /// yields a `&cyrup_core::AssistantMessage`). `cyrup-provider` is a direct dependency, so the
    /// token-by-token render (gap 1) is live, not deferred.
    pub fn ingest_event(&mut self, ev: &AgentSessionEvent) {
        self.ingest_event_rendered(ev, None, crate::transcript::Rendered::None);
    }

    /// [`Self::ingest_event`], first giving the loaded extensions a chance to RENDER the event
    /// (EXT-006). This is the production fold the interactive run loop uses; the sync
    /// [`Self::ingest_event`] is the no-extensions shorthand.
    ///
    /// Pi resolves a renderer at the point of display — `extensionRunner.getMessageRenderer(...)`
    /// for a custom message (interactive-mode.ts:3324-3336) and the per-tool `renderCall`/
    /// `renderResult` for a tool row (components/tool-execution.ts:81-112). cyrup's fold is sync
    /// (it mutates `&mut self` from a `select!` arm) while a guest renderer is an async wasm call,
    /// so the renderer runs FIRST and its text rides into the fold.
    pub async fn ingest_event_with_extensions(
        &mut self,
        ev: &AgentSessionEvent,
        ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ) {
        let rendered = extension_render(ext_host, ev).await;
        // X15 — the custom-ENTRY renderer is a SECOND, disjoint lookup (Pi keeps
        // `messageRenderers` and `entryRenderers` as separate maps, types.ts:1703-1704, and
        // `addCustomEntryToChat` resolves the entry one at `interactive-mode.ts:3432`). It rides in
        // the same way and for the same reason: the fold is sync, the guest call is async.
        let entry = match ev {
            AgentSessionEvent::EntryAppended { entry } => {
                let custom_type = custom_entry_type(entry);
                extension_render_entry(ext_host, &custom_type, entry).await
            }
            _ => crate::transcript::Rendered::None,
        };
        self.ingest_event_rendered(ev, rendered, entry);
    }

    /// The run loop's per-event fold: [`Self::ingest_event_with_extensions`], then the footer's
    /// session-derived context segment ([`Self::refresh_context_usage`]).
    ///
    /// This is the whole of what pi's `render()` does for the footer for free — it calls
    /// `this.session.getContextUsage()` on every frame (`footer.ts:108`). cyrup's fold is sync and
    /// cannot `await` the session, so the refresh is hoisted here, to the one place that both holds
    /// the session and already runs per event.
    pub async fn ingest_session_event(
        &mut self,
        ev: &AgentSessionEvent,
        session: &Arc<AgentSession>,
    ) {
        let ext_host = session.services().ext_host.clone();
        self.ingest_event_with_extensions(ev, &ext_host).await;
        // `autoCompactionEnabled` is a plain `bool` read with no session walk behind it, and
        // upstream's THIRD `setAutoCompactEnabled` call site is a settings toggle rather than a turn
        // event (`interactive-mode.ts:4417-4419`), so it must not ride the six-event predicate that
        // gates the (much more expensive) context recompute below.
        self.refresh_auto_compact(session);
        if context_usage_may_have_moved(ev) {
            self.refresh_context_usage(session).await;
        }
    }

    /// Re-read `getContextUsage()` off the live session into the footer (`footer.ts:106-111`), plus
    /// [`Self::refresh_auto_compact`].
    ///
    /// The three answers map straight onto [`StatusLine::set_context_usage`]; `percent` is a 0-100
    /// percentage session-side and a fraction footer-side, hence the `/ 100.0`.
    ///
    /// **The ` (auto)` suffix does not belong to this method alone.** Upstream sets it from three
    /// places — construction (`interactive-mode.ts:572`), a runtime-settings reapply (`:1902`) and
    /// the `/settings` auto-compaction toggle's `onAutoCompactChange` callback (`:4417-4419`) — and
    /// only the first two are turn-shaped. cyrup has no auto-compaction row in its settings selector
    /// today (`AgentSession::set_auto_compaction_enabled` is reached only from the RPC mode and the
    /// `SessionCommand` seam), so there is no toggle site to wire; when one is added it must call
    /// [`Self::refresh_auto_compact`] directly, exactly as `onAutoCompactChange` does. Until then the
    /// per-event refresh in [`Self::ingest_session_event`] picks up any out-of-band change.
    pub async fn refresh_context_usage(&mut self, session: &Arc<AgentSession>) {
        self.refresh_auto_compact(session);
        match session.stats_context_usage().await {
            Some(usage) => self.state.status.set_context_usage(
                usage.percent.map(|p| p / 100.0),
                Some(usage.context_window),
                session.auto_compaction_enabled(),
            ),
            None => self.state.status.set_context_usage(
                None,
                None,
                session.auto_compaction_enabled(),
            ),
        }
    }

    /// `this.footer.setAutoCompactEnabled(this.session.autoCompactionEnabled)` — the ` (auto)`
    /// suffix on the footer's context segment, on its own (`interactive-mode.ts:572`, `:1902`,
    /// `:4418`).
    ///
    /// Sync and cheap: `auto_compaction_enabled()` is an override-or-default `bool` read, no session
    /// walk. Any future auto-compaction toggle in cyrup's settings selector calls THIS.
    pub fn refresh_auto_compact(&mut self, session: &Arc<AgentSession>) {
        self.state.status.set_auto_compact(session.auto_compaction_enabled());
    }

    /// `rendered` is what a custom-MESSAGE / tool-row renderer produced (already collapsed to
    /// `Option<String>`, since both surfaces swallow a renderer throw upstream). `entry` is the
    /// three-state custom-ENTRY outcome, which does NOT collapse — see [`extension_render_entry`].
    fn ingest_event_rendered(
        &mut self,
        ev: &AgentSessionEvent,
        rendered: Option<String>,
        entry_rendered: crate::transcript::Rendered,
    ) {
        match ev {
            AgentSessionEvent::AgentStart => {
                // Pi `case "agent_start"` (`interactive-mode.ts:2865-2867`): the FIRST statement of
                // the arm, before the retry-handler restore and the working indicator, is
                // `if (getShowTerminalProgress()) this.ui.terminal.setProgress(true)`. The OSC write
                // is the run loop's (`flush_terminal_progress`), as for the OSC 0 title.
                self.state.terminal_progress.set(true);
                self.state.status.set_streaming(true);
                self.state.indicator.working();
            }
            AgentSessionEvent::AgentEnd { .. } => {
                // Pi `case "agent_end"` (`interactive-mode.ts:3057-3059`), again the arm's first
                // statement: `setProgress(false)`. `agent_end` — not `agent_settled` — is where Pi
                // clears, so a turn that goes on to auto-retry or run a queued continuation drops
                // the indicator and the next `agent_start` puts it back.
                self.state.terminal_progress.set(false);
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
                // Reasoning commits BEFORE the answer text so the scrollback order matches Pi's
                // content walk (thinking section, then the assistant markdown).
                self.state.transcript.commit_thinking(None);
                self.state.transcript.commit_assistant(None);
                // `if (this.streamingComponent) { … this.streamingComponent = undefined; }`
                // (`interactive-mode.ts:3271-3275`) — a turn that ended without a `message_end`
                // (an abort mid-stream) must not leave the slot open for the next turn.
                self.state.streaming_assistant = false;
                // Commit the turn's live tool executions into scrollback (`tool-execution.ts` tools
                // persist through the turn, then scroll up as committed history).
                self.state.transcript.commit_tools();
            }
            // SEAM-005 `agent_settled` (Pi interactive-mode.ts:3137): the run has fully settled —
            // no retry, post-run compaction or queued continuation will follow. Pi's interactive
            // mode does exactly ONE thing here, `await this.checkShutdownRequested()`; the visual
            // teardown already happened on `agent_end` above. That shutdown check lives in the
            // async event-loop arm (`run`, the `events.next()` branch) rather than in this sync
            // fold, which cannot `await` or return control to the caller — so this arm is a
            // deliberate no-op, NOT a missing case.
            AgentSessionEvent::AgentSettled => {}
            AgentSessionEvent::TurnStart | AgentSessionEvent::TurnEnd { .. } => {}
            // Pi `case "message_start"` (`interactive-mode.ts:3121-3143`): an `assistant` message
            // opens a fresh `AssistantMessageComponent` and files it in `this.streamingComponent`
            // (`:3130-3139`). cyrup's transcript already owns the streaming buffers, so the only
            // thing this arm has to reproduce is the LIFETIME — the bit `message_end` reads to know
            // an assistant message is open and unfinalized (`:3182`).
            AgentSessionEvent::MessageStart { .. } => {
                if message_role_from_event(ev).as_deref() == Some("assistant") {
                    self.state.streaming_assistant = true;
                }
            }
            // Pi `case "message_end"` (`interactive-mode.ts:3180-3216`). This is where an assistant
            // message is FINALIZED — `this.streamingComponent.updateContent(this.streamingMessage,
            // false)` at `:3193`, then `this.streamingComponent = undefined` at `:3213`.
            //
            // It is not optional bookkeeping: it is what makes a turn INTERLEAVE. Each finished
            // assistant text commits here, before the tool calls it requested start; each
            // `ToolExecutionComponent` is then appended after it (`:3166`/`:3240`) and the next
            // step's text after those. Committing assistant text only at `agent_end` instead —
            // which is what cyrup did while this arm was empty — concatenated every step's text
            // into one block and pushed the whole turn's tools below it, because
            // `commit_finished_leading_tools` refuses to commit a tool ahead of uncommitted
            // assistant text (`transcript.rs:865-868`) and so never fired at all.
            AgentSessionEvent::MessageEnd { .. } => {
                // A tool that reported usage for its own execution spends real tokens, so the
                // cumulative footer totals must include it (`footer.ts:99-101`). This is the
                // `toolResult` branch and, like upstream, must NOT restate the `CH` segment —
                // assistant usage goes through `add_usage` in [`Self::finalize_assistant_message`].
                if let Some(u) = tool_result_usage_from_event(ev) {
                    self.state.status.add_usage_totals(&u);
                }
                // `if (event.message.role === "user") break;` (`:3181`) plus the
                // `this.streamingComponent &&` guard (`:3182`): only an OPEN assistant message
                // finalizes here. The open bit is cleared by whichever path finalizes first, so a
                // producer that does deliver a terminal `StreamEvent::Done` inside `message_update`
                // cannot commit the same text twice.
                if self.state.streaming_assistant
                    && let Some(message) = assistant_message_from_event(ev)
                {
                    self.finalize_assistant_message(&message);
                }
                // The `AgentMessage` type lives in `cyrup-agent` (a dev-dep here, not a direct dep), so
                // the `Custom` arm is detected via its serde projection (`tag = "role"`,
                // `rename_all_fields = camelCase`) rather than a direct match — no dependency ripple.
                if let Some((kind, body)) = custom_message_from_event(ev) {
                    // EXT-006: `rendered` is the text the extension that registered a renderer for
                    // this custom type produced; absent one it is `None` and the default
                    // `[kind] body` framing draws (Pi `CustomMessageComponent`).
                    // `Rendered::None` is "no renderer claimed this type" — Pi's
                    // `getMessageRenderer(...) === undefined` (`interactive-mode.ts:3326`), which
                    // draws the default box. A renderer that FAULTED also lands here, matching
                    // `custom-message.ts:82-84`'s `catch { /* Fall through to default rendering */ }`.
                    let rendered = rendered
                        .map(crate::transcript::Rendered::Text)
                        .unwrap_or(crate::transcript::Rendered::None);
                    self.state.transcript.push_custom_message_rendered(kind, body, rendered);
                }
            }
            AgentSessionEvent::MessageUpdate { assistant_message_event, .. } => {
                self.ingest_stream_event(assistant_message_event);
            }
            AgentSessionEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                // Hand the raw call args to the transcript so each built-in renders its Pi-specific
                // `renderCall` header (path+range / `$ command` / `/pattern/` / …), not a generic
                // one-liner (transcript.rs `tool_lines` dispatch).
                // The `toolCallId` is what the matching `ToolExecutionEnd` is paired back by — Pi
                // files the component under it (`pendingTools.set(event.toolCallId, component)`,
                // interactive-mode.ts:3096). A turn that batches two `read`s cannot be resolved by
                // tool name.
                // EXT-006: an extension that declared a renderer for THIS tool supplies the call
                // header; `None` keeps the built-in per-tool dispatch.
                self.state.transcript.push_tool_start_rendered(
                    tool_name.clone(),
                    Some(tool_call_id.as_str().to_string()),
                    args.clone(),
                    rendered,
                );
                // Pi's `edit` renderCall fires `computeEditsDiff` the moment the streamed arguments
                // are complete (edit.ts:377-386) so the diff is on screen while the call is still
                // pending. `ToolExecutionStart` IS that moment here: cyrup emits it with the full
                // arguments and BEFORE `prepare`, i.e. before the `before_tool_call` permission gate
                // (`cyrup-agent/src/agent.rs:1181/1334`), so the preview is up for the whole time an
                // approval prompt is waiting — and nothing has been written yet.
                if tool_name == "edit" {
                    let cwd = self.state.title_cwd.clone();
                    if let Some(preview) = edit_preview(args, &cwd) {
                        self.state
                            .transcript
                            .set_edit_preview(Some(tool_call_id.as_str()), preview);
                    }
                }
            }
            AgentSessionEvent::ToolExecutionUpdate { tool_call_id, partial_result, .. } => {
                // Pi: `this.pendingTools.get(event.toolCallId)` (interactive-mode.ts:3104).
                self.state
                    .transcript
                    .push_tool_update(Some(tool_call_id.as_str()), Some(partial_result.clone()));
            }
            AgentSessionEvent::ToolExecutionEnd { tool_call_id, tool_name, is_error, result } => {
                // The full `{content, details, terminate}` result flows through so `renderResult` can
                // reach each tool's `details` (edit `diff`, bash/read truncation, …), and the
                // `toolCallId` routes it to the run that made THIS call (`:3113`).
                self.state.transcript.push_tool_end_rendered(
                    tool_name.clone(),
                    Some(tool_call_id.as_str()),
                    *is_error,
                    Some(result.clone()),
                    rendered,
                );
                // Progressively flush finished tools to native scrollback mid-turn so the inline
                // viewport holds only the running tail, not the whole turn's tool stack (the
                // SCREEN-FILL disaster). The finished tool leaves `active_tools` here; the
                // draw-after-every-event (`flush_committed` → `insert_before`) lands it above the
                // viewport on the very next frame, and `terminal.draw` renders the tail without it —
                // an atomic handoff, no duplicate/flash. Mirrors Pi's completed `tool-execution.ts`
                // components scrolling up into native history as the turn proceeds.
                self.state.transcript.commit_finished_leading_tools();
            }
            AgentSessionEvent::QueueUpdate { steering, follow_up } => {
                self.state.status.set_queued(steering.len().saturating_add(follow_up.len()));
            }
            AgentSessionEvent::CompactionStart { reason } => {
                // Pi `case "compaction_start"` (`interactive-mode.ts:3076-3078`): compaction is
                // also work the user waits on, so it arms the same indicator — including a manual
                // `/compact` outside any turn, which is the one progress window with no
                // `agent_start` around it.
                self.state.terminal_progress.set(true);
                // Pi's exact status copy (status-indicator.ts:80-82): a MANUAL `/compact` reads
                // "Compacting context…"; an automatic compaction reads "Auto-compacting…", prefixed
                // "Context overflow detected, " when the overflow path triggered it (item #9). The
                // ` (<key> to cancel)` suffix is appended by the band from the live keymap.
                let msg = match reason {
                    CompactionReason::Manual => "Compacting context...".to_string(),
                    CompactionReason::Overflow => {
                        "Context overflow detected, Auto-compacting...".to_string()
                    }
                    CompactionReason::Threshold => "Auto-compacting...".to_string(),
                };
                // X18 — the indicator is a BAND, not a message. `interactive-mode.ts:3286-3298`
                // `case "compaction_start"` calls `showStatusIndicator(new
                // CompactionStatusIndicator(...))` and nothing else; `StatusIndicator` extends
                // `Loader` (`status-indicator.ts:9-27`) and is mounted in the fixed status slot, so
                // it disappears the moment `clearStatusIndicator` runs. cyrup was ALSO pushing the
                // identical string into the transcript, which `insert_before` then froze into
                // scrollback as a permanent dim `• Compacting context...` row upstream never writes.
                self.state.indicator.set(IndicatorKind::Compaction, Some(msg));
            }
            AgentSessionEvent::CompactionEnd { .. } => {
                // Pi `case "compaction_end"` (`interactive-mode.ts:3090-3092`): clears
                // unconditionally, even when this was an AUTO-compaction inside a still-streaming
                // turn. Pi's own `agent_end` then re-clears; the visible effect is a brief gap in
                // the taskbar pulse, and matching it is why `TerminalProgress::set` does not
                // deduplicate repeated transitions.
                self.state.terminal_progress.set(false);
                // Back to working if the turn is still streaming, else idle.
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
                self.state.transcript.push_status("compaction complete");
            }
            AgentSessionEvent::AutoRetryStart { attempt, max_attempts, delay_ms, .. } => {
                // Pi's exact retry copy (status-indicator.ts:46-47): `Retrying (a/max) in Ns...`,
                // where N starts at `Math.ceil(delayMs / 1000)` and is then re-set every second by a
                // `CountdownTimer` (`:55-64`, `countdown-timer.ts:21-30`). `set_retry` owns that
                // countdown; formatting the message here would freeze N for the whole backoff. The
                // ` (<key> to cancel)` suffix is appended by the band from the live keymap.
                // X18 — band only, exactly as `interactive-mode.ts:3339-3347` `case
                // "auto_retry_start"`: `showStatusIndicator(new RetryStatusIndicator(...))`, no
                // chat write. The mirrored `• Retrying (1/3) in 30s...` row was cyrup-only, and
                // being a snapshot of a ticking countdown it froze at whatever second it was
                // pushed.
                self.state.indicator.set_retry(*attempt, *max_attempts, *delay_ms);
            }
            AgentSessionEvent::SummarizationRetryScheduled {
                attempt,
                max_attempts,
                delay_ms,
                error_message,
            } => {
                // Pi `interactive-mode.ts:3222-3229`: surface the transient error, then swap the
                // compaction/branch indicator for the same `RetryStatusIndicator` the turn-level
                // auto-retry uses, so a compacting session shows a countdown rather than hanging.
                // `showError(event.errorMessage)` then `showStatusIndicator(new
                // RetryStatusIndicator(...))` (`interactive-mode.ts:3367-3374`) — the error goes to
                // the chat, the countdown stays in the band (X18).
                self.state.transcript.push_error(error_message.clone());
                self.state.indicator.set_retry(*attempt, *max_attempts, *delay_ms);
            }
            AgentSessionEvent::SummarizationRetryAttemptStart { source } => {
                // Pi `interactive-mode.ts:3231-3240`: clear the retry indicator and RECREATE the
                // underlying one from `source` — that is the only reason the event carries it.
                let (kind, msg) = match source {
                    SummarizationRetrySource::BranchSummary => {
                        (IndicatorKind::BranchSummary, "Summarizing branch...".to_string())
                    }
                    SummarizationRetrySource::Compaction { reason } => (
                        IndicatorKind::Compaction,
                        match reason {
                            CompactionReason::Manual => "Compacting context...".to_string(),
                            CompactionReason::Overflow => {
                                "Context overflow detected, Auto-compacting...".to_string()
                            }
                            CompactionReason::Threshold => "Auto-compacting...".to_string(),
                        },
                    ),
                };
                self.state.indicator.set(kind, Some(msg));
            }
            AgentSessionEvent::SummarizationRetryFinished => {
                // Pi `interactive-mode.ts:3242-3245`: `clearStatusIndicator("retry")` only — it is
                // a no-op unless the retry indicator is the live one, which it is exactly when the
                // loop ended DURING a backoff (exhausted / aborted). A loop that ended on a
                // successful retried call already restored its own indicator in the arm above.
                if self.state.indicator.kind() == IndicatorKind::Retry {
                    if self.state.status.streaming {
                        self.state.indicator.working();
                    } else {
                        self.state.indicator.idle();
                    }
                }
            }
            // Pi renders bash output from the execution callback, not from this event
            // (`interactive-mode.ts:3075-3077`: "The bash execution callback handles TUI output
            // rendering."). Kept as an explicit no-op so the parity is visible.
            AgentSessionEvent::BashExecutionUpdate { .. } => {}
            AgentSessionEvent::AutoRetryEnd { success, .. } => {
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
                self.state
                    .transcript
                    .push_status(if *success { "retry succeeded" } else { "retry ended" });
            }
            AgentSessionEvent::ModelChanged { provider, model } => {
                let label = format!("{provider}/{model}");
                self.state.status.set_model(label.clone());
                // Feed the provider into the footer right cluster (`(provider)` prefix, footer.ts:191).
                self.state.status.set_provider(Some(provider.clone()));
                // …and re-answer `usingSubscription` for the NEW provider (`footer.ts:139-141`).
                // pi gets this for free — `model_changed` calls `footer.invalidate()`
                // (`interactive-mode.ts:3070`) and the flag is recomputed inside `render()`. cyrup
                // must push it, or a `/model` switch from a subscription provider to a metered one
                // would keep printing ` (sub)` (and vice versa).
                self.refresh_subscription_marker();
                self.state.transcript.push_status(format!("model → {label}"));
            }
            AgentSessionEvent::ThinkingLevelChanged { level } => {
                // Pi's `thinking_level_changed` handler (interactive-mode.ts:2804-2807) only
                // `footer.invalidate()` + `updateEditorBorderColor()` — NO status line (the acting
                // command, e.g. Shift+Tab's `C::CycleThinking`, owns the status). Mirror the level into
                // the footer right cluster (`• {level}`, footer.ts:186-188), the editor's rule color
                // (spec/tui/03 §3.3), and the TUI's cached level so `/debug` reflects the authoritative
                // session state.
                self.state.thinking_level = level.clone();
                self.state.status.set_thinking_level(level.clone());
                self.state.editor.set_thinking_level(level);
            }
            AgentSessionEvent::SessionInfoChanged { name } => {
                // Pi `interactive-mode.ts:2784` mirrors the renamed session into the header/status.
                let label = name.clone().unwrap_or_default();
                self.state.transcript.push_status(format!("session renamed → {label}"));
                // Pi's `session_info_changed` arm (`interactive-mode.ts:2900-2903`) is
                // `updateTerminalTitle()` + `footer.invalidate()`: the new name reaches BOTH the
                // footer's location line (` • {name}`, footer.ts:116-130) and the window title. The
                // recomputed title is written by the crossterm run loop (see
                // [`Self::update_terminal_title`]).
                self.state.status.set_session_name(name.clone());
                let _ = self.update_terminal_title();
            }
            AgentSessionEvent::EntryAppended { entry } => {
                // A loaded extension appended a custom (non-LLM) entry to the tree (Pi
                // `entry_appended`, agent-session.ts:140 → `addCustomEntryToChat(event.entry)`,
                // interactive-mode.ts:3105/3431-3450).
                let ty = custom_entry_type(entry);
                // X15 — `addCustomEntryToChat` is entirely a renderer question:
                //
                // ```ts
                // const renderer = this.session.extensionRunner.getEntryRenderer(entry.customType);
                // if (!renderer) { return; }                      // :3433-3435 — draws NOTHING
                // const component = new CustomEntryComponent(entry, renderer);
                // component.setExpanded(this.toolOutputExpanded);
                // if (!component.hasContent()) { return; }        // :3438-3440 — also nothing
                // ```
                //
                // …and `CustomEntryComponent` is where a THROW becomes the failure box
                // (`custom-entry.ts:47-52`) rather than being dropped. `entry_rendered` carries
                // that three-state answer here.
                if entry_rendered.has_content() {
                    self.state.transcript.push_custom_message_rendered(
                        ty,
                        String::new(),
                        entry_rendered,
                    );
                } else {
                    // CYRUP-DELTA: with no renderer claiming the type upstream shows nothing at
                    // all, which leaves a `/statedemo`-style entry invisible. cyrup keeps its
                    // pre-existing one-line receipt for that case only — strictly additive over
                    // "nothing", and it never competes with a renderer, because a renderer that
                    // produced output (or faulted) took the branch above.
                    self.state.transcript.push_status(format!("entry appended → {ty}"));
                }
            }
            // Session lifecycle (runtime replacement, arch-11 §3.4): surface a status line. The TUI
            // re-subscribes to the runtime's new generation on `SessionReplaced` (R-11-021); the
            // re-subscription itself is driven by the app's runtime watch, not this transcript hook.
            AgentSessionEvent::SessionStart { reason, .. } => {
                self.state.transcript.push_status(format!("session started ({reason})"));
            }
            AgentSessionEvent::SessionShutdown { reason } => {
                self.state.transcript.push_status(format!("session shutdown ({reason})"));
            }
            AgentSessionEvent::SessionReplaced { .. } => {
                self.state.status.set_streaming(false);
                self.state.indicator.idle();
            }
        }
    }

    /// Fold one streaming `StreamEvent` (the `assistantMessageEvent` payload of a `MessageUpdate`,
    /// session-svc `event.rs:111`) into the transcript — the live token-by-token render (gap 1).
    ///
    /// `TextDelta { delta, .. }` (provider `stream.rs:306`) is appended to the in-flight streaming
    /// buffer via [`TranscriptView::push_assistant_delta`], so the viewport grows a character at a
    /// time exactly like Pi's interactive stream. A terminal event (`Done`/`Error`, recoverable via
    /// [`StreamEvent::terminal_message`]) replaces the partial with the authoritative
    /// `AssistantMessage` text and records its token usage in the footer.
    ///
    /// `ThinkingDelta { delta, .. }` (provider `stream.rs:413`) grows the separate live *reasoning*
    /// block via [`TranscriptView::push_thinking_delta`]; the terminal event commits the message's
    /// authoritative `thinking` blocks ([`thinking_text`]) ahead of the answer text, matching Pi's
    /// in-order content walk (`assistant-message.ts:115-166`). The remaining non-text frames
    /// (start/text-start/text-end/thinking-start/thinking-end/toolcall*) carry only the running
    /// `partial`, whose content already reaches us via the deltas + the terminal, so nothing is
    /// rendered for them.
    ///
    /// A terminal whose `stop_reason` is not a clean stop also appends Pi's error-styled
    /// incomplete/failed-turn notice ([`stop_reason_notice`], `assistant-message.ts:175-201`).
    fn ingest_stream_event(&mut self, ev: &StreamEvent) {
        match ev {
            StreamEvent::TextDelta { delta, .. } => {
                if !delta.is_empty() {
                    self.state.transcript.push_assistant_delta(delta);
                }
            }
            // Reasoning deltas (provider `stream.rs:413`) grow their own live block above the
            // answer text, exactly as Pi renders the turn's `thinking` content
            // (`assistant-message.ts:115-166`). `ThinkingStart`/`ThinkingEnd` carry no incremental
            // text of their own — the authoritative blocks arrive with the terminal message below.
            StreamEvent::ThinkingDelta { delta, .. } => {
                if !delta.is_empty() {
                    self.state.transcript.push_thinking_delta(delta);
                }
            }
            // DEFENSIVE, not the live path. `cyrup-agent` `break 'consume`s the moment the stream
            // yields its terminal (`agent.rs:813-820`), so a terminal event is never re-emitted as
            // a `MessageUpdate` and this arm does not fire for a real turn — `MessageEnd` is where
            // an assistant message finalizes. It stays for any producer (an embedder, a replayed
            // transport) that does forward the terminal, and clears the open bit so `MessageEnd`
            // will not then commit the same text a second time. The `streaming_assistant` guard is
            // Pi's `if (this.streamingComponent && ...)` on `message_update`
            // (`interactive-mode.ts:3146`).
            StreamEvent::Done { message, .. } | StreamEvent::Error { error: message, .. }
                if self.state.streaming_assistant =>
            {
                self.finalize_assistant_message(message);
            }
            _ => {}
        }
    }

    /// Pi `message_end`'s finalization of an assistant message (`interactive-mode.ts:3183-3214`):
    /// the authoritative message replaces whatever streamed, and the streaming slot closes.
    ///
    /// Commits the reasoning FIRST — Pi walks the message content in order and `thinking` precedes
    /// the answer (`assistant-message.ts:115-166`) — preferring the final message's blocks over the
    /// streamed ones, since a redacted/summarised block only ever arrives terminally.
    fn finalize_assistant_message(&mut self, message: &cyrup_core::AssistantMessage) {
        // `this.streamingComponent = undefined` (`:3213`).
        self.state.streaming_assistant = false;
        let thinking = thinking_text(&message.content);
        if thinking.is_empty() {
            self.state.transcript.commit_thinking(None);
        } else {
            self.state.transcript.commit_thinking(Some(thinking));
        }
        let text = content_text(&message.content);
        if text.is_empty() {
            // Pure tool-use / empty terminal: keep any streamed partial; `AgentEnd` commits it.
            self.state.transcript.commit_assistant(None);
        } else {
            self.state.transcript.commit_assistant(Some(text));
        }
        let tokens = message.usage.total_tokens;
        if tokens > 0 {
            self.state.status.set_tokens(tokens);
        }
        // Accumulate the turn into the cumulative session footer totals (footer.ts:86-107).
        self.state.status.add_usage(&message.usage);
        // A turn that did not finish cleanly gets Pi's error-styled footer notice
        // (assistant-message.ts:175-201) — otherwise a 5xx, an abort or a max-token
        // truncation would end the turn with no explanation at all.
        if let Some(notice) = stop_reason_notice(message) {
            self.state.transcript.push_error(notice);
        }
    }
}

/// Whether `ev` can have moved `getContextUsage()`'s answer, and so needs a footer refresh.
///
/// Upstream recomputes the segment on every frame, so this predicate exists only to keep cyrup from
/// walking the session's entries on events that provably cannot change it (a keystroke echo, a
/// status line). The answer is a function of the branch's last assistant `usage`, the latest
/// compaction on that branch, and the model's context window — so: a finished message, the end of a
/// turn, a compaction, a model swap, and a session replacement.
fn context_usage_may_have_moved(ev: &AgentSessionEvent) -> bool {
    matches!(
        ev,
        AgentSessionEvent::MessageEnd { .. }
            | AgentSessionEvent::AgentEnd { .. }
            | AgentSessionEvent::CompactionEnd { .. }
            | AgentSessionEvent::ModelChanged { .. }
            | AgentSessionEvent::SessionStart { .. }
            | AgentSessionEvent::SessionReplaced { .. }
    )
}

/// The notice shown under an assistant turn that stopped on `length`, verbatim from Pi v0.84.1
/// `coding-agent/src/modes/interactive/components/assistant-message.ts:180`.
///
/// **Version lag, not a port bug.** Through v0.83.0 (`:153-161`) this read
/// `"Error: Model stopped because it reached the maximum output token limit. The response may be
/// incomplete."`. Upstream shortened it in `32850ef7c` ("fix(coding-agent): resume after
/// context-limited length stops", #7540), whose commit message gives the reason: a `length` stop is
/// no longer necessarily a max-output-token stop — it may be a context overflow that pi then
/// compacts and retries — so the TUI moved to "neutral truncation wording" that does not assert a
/// cause. Note the loss of the `Error: ` prefix is part of that change and is deliberate upstream:
/// only the `error` arm (`:193`) still prefixes.
pub(crate) const LENGTH_STOP_NOTICE: &str = "Response was truncated before completion.";

/// The `error`-styled notice Pi appends after an assistant turn that did not finish cleanly
/// (v0.84.1 `coding-agent/src/modes/interactive/components/assistant-message.ts:174-195`), or `None`
/// for a clean turn.
///
/// * `length` → [`LENGTH_STOP_NOTICE`], emitted **unconditionally**: a length stop can land before a
///   tool call is complete, so it is surfaced even on a tool turn (`:177`).
/// * `aborted` / `error` → emitted only when the message carries NO `toolCall` content (`:182`),
///   because for those the tool-execution component already reports the failure.
/// * `aborted` shows `errorMessage` unless it is the internal `Request was aborted` sentinel, in
///   which case the user-facing wording is `Operation aborted` (`:183-189`).
/// * `error` shows `Error: {errorMessage || "Unknown error"}` (`:190-193`).
fn stop_reason_notice(message: &cyrup_core::AssistantMessage) -> Option<String> {
    use cyrup_core::StopReason;
    if message.stop_reason == StopReason::Length {
        return Some(LENGTH_STOP_NOTICE.to_string());
    }
    let has_tool_calls =
        message.content.iter().any(|c| matches!(c, cyrup_core::Content::ToolCall(_)));
    if has_tool_calls {
        return None;
    }
    match message.stop_reason {
        StopReason::Aborted => Some(match message.error_message.as_deref() {
            Some(m) if !m.is_empty() && m != "Request was aborted" => m.to_string(),
            _ => "Operation aborted".to_string(),
        }),
        StopReason::Error => Some(format!(
            "Error: {}",
            match message.error_message.as_deref() {
                Some(m) if !m.is_empty() => m,
                _ => "Unknown error",
            }
        )),
        // `Pending` is the in-flight sentinel, so it must render like Pi's: Pi's chain is
        // `if (stopReason === "length") … else if (!hasToolCalls) { if ("aborted") … else if
        // ("error") … }` (assistant-message.ts:177-201), and `"pending"` matches none of them —
        // no notice. Grouped explicitly rather than via a `_ =>` so a future variant still breaks
        // this match, which is how this arm got written in the first place.
        //
        // `Deferred` joins it for the same reason, verified the same way: `deferred` appears
        // NOWHERE in `v0.84.1 coding-agent/src/modes/interactive/components/assistant-message.ts`,
        // so Pi's chain falls through it too and renders no notice.
        StopReason::Pending
        | StopReason::Deferred
        | StopReason::Stop
        | StopReason::Length
        | StopReason::ToolUse => None,
    }
}

/// Flatten a styled [`Line`] into its plain text (concatenated span content).
fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Copy `text` to the system clipboard best-effort via the platform CLI (`pbcopy` on macOS, `xclip`/
/// `wl-copy` on Linux). No new dependency and no unsafe — a missing tool is silently ignored
/// (`handleCopyCommand` clipboard write, interactive-mode.ts:5285). Unix-only.
#[cfg(unix)]
fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    let candidates: &[(&str, &[&str])] =
        &[("pbcopy", &[]), ("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])];
    for (bin, args) in candidates {
        let child = std::process::Command::new(bin)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }
}

/// No-op clipboard write on non-unix targets (the platform CLI tools are unix-only here).
#[cfg(not(unix))]
fn copy_to_clipboard(_text: &str) {}

/// Read a system-clipboard image and materialize it as a PNG temp file, returning its path (Pi
/// `readClipboardImage` + the temp-file write of `handleClipboardImagePaste`,
/// interactive-mode.ts:2544-2549 / `utils/clipboard-image.ts`). `arboard` is the faithful Rust analog
/// of Pi's native clipboard module (`utils/clipboard-native.ts`): NSPasteboard on macOS, X11/Wayland on
/// Linux. `get_image` hands back an RGBA8 raster (`arboard::ImageData`), which is re-encoded to PNG with
/// the in-tree `image` crate and written to `cyrup-clipboard-<uuid>.png` in the OS temp dir (Pi's
/// `pi-clipboard-<randomUUID>.<ext>`, always PNG here since the raster is already decoded).
///
/// Returns `None` when the clipboard holds no image (Pi's `clipboard.hasImage()` gate) or on ANY
/// clipboard/decode/encode/IO error — mirroring Pi's `catch {}` silent-ignore (no clipboard access, a
/// headless/permission-denied session, a zero-area raster, …) — so a bare Ctrl+V never disrupts the
/// editor and simply falls through to normal text handling.
fn read_clipboard_image_to_temp() -> Option<std::path::PathBuf> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let img = clipboard.get_image().ok()?;
    let width = u32::try_from(img.width).ok()?;
    let height = u32::try_from(img.height).ok()?;
    // `arboard::ImageData::bytes` is an RGBA8 raster; `from_raw` returns `None` if the buffer length
    // does not match `width * height * 4`, guarding a malformed clipboard payload without panicking.
    let raster = image::RgbaImage::from_raw(width, height, img.bytes.into_owned())?;
    let path =
        std::env::temp_dir().join(format!("cyrup-clipboard-{}.png", uuid::Uuid::now_v7()));
    raster.save_with_format(&path, image::ImageFormat::Png).ok()?;
    Some(path)
}

/// Truncate a one-line summary to a sane length (avoid overrunning the marker line).
/// Detect a `Custom`-role [`cyrup_agent::AgentMessage`] from its serde projection and return its
/// `(kind, body)` for [`TranscriptView::push_custom_message`](crate::transcript::TranscriptView::push_custom_message).
/// `AgentMessage` is only a dev-dependency here, so the message is inspected through `serde_json`
/// (`{"role":"custom","kind":…,"payload":…}`) instead of a direct pattern match — no dep ripple.
/// Returns `None` for any non-custom (core user/assistant/toolResult) message.
/// Whether the interactive host should exit NOW because a loaded extension called `ctx.shutdown()`
/// (EXT-005 / SEAM-005).
///
/// Pi checks a pending shutdown at exactly two moments, and cyrup's run loop calls this at both:
///
/// * `at_settle` — the `agent_settled` arm, `case "agent_settled": await
///   this.checkShutdownRequested()` (interactive-mode.ts:3137-3138). A settle means the whole run
///   is over (no retry, no post-run compaction, no queued continuation), so no idle re-check is
///   needed or wanted;
/// * otherwise — the `shutdownHandler` Pi binds in `bindExtensions`,
///   `this.shutdownRequested = true; if (this.session.isIdle) { void this.shutdown(); }`
///   (interactive-mode.ts:1753-1757). This is what makes Pi's own canonical example,
///   `examples/extensions/shutdown-command.ts` — a `/quit` COMMAND that never starts a run — exit
///   at all; gating solely on a settle would strand it forever.
///
/// Kept as a named predicate rather than an inline condition so it is testable without driving a
/// real terminal event loop.
pub fn should_honor_extension_shutdown(
    session: &cyrup_session_svc::AgentSession,
    at_settle: bool,
) -> bool {
    session.shutdown_requested() && (at_settle || session.is_idle())
}

/// How long the fold waits for an extension renderer before falling back to the built-in framing.
///
/// A renderer is a presentation concern on the interactive event path; it must never be able to
/// wedge the frame. The call runs on its OWN task (not inline in the `select!` arm) for the same
/// reason `AppAction::ExtensionShortcut` spawns: a guest handler may synchronously block on a
/// `ui.{confirm,input,…}` capability whose reply only THIS loop can deliver, and awaiting it inline
/// would be a genuine self-deadlock. Spawn + bounded wait makes the worst case "the block draws
/// with its built-in renderer", never a hang.
const EXTENSION_RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Ask the loaded extensions to render this event, if any registered a renderer for it (EXT-006).
///
/// * a custom message → the extension that registered `custom_type` (Pi `getMessageRenderer`,
///   runner.ts:579-587, resolved at interactive-mode.ts:3326);
/// * a tool start/end → the extension that declared a renderer for that TOOL NAME (Pi's per-tool
///   `renderCall`/`renderResult`, tool-execution.ts:81-112).
///
/// `None` for every other event, and for any event whose key has no registered renderer — the cheap
/// SYNC `has_*_renderer` pre-check runs first so the common path pays nothing.
pub async fn extension_render(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    ev: &AgentSessionEvent,
) -> Option<String> {
    let which = match ev {
        AgentSessionEvent::MessageEnd { .. } => {
            let (kind, _) = custom_message_from_event(ev)?;
            if !ext_host.has_message_renderer(&kind) {
                return None;
            }
            let message = serde_json::to_value(ev).ok()?.get("message")?.clone();
            Which::Message(kind, message)
        }
        AgentSessionEvent::ToolExecutionStart { tool_name, args, .. } => {
            if !ext_host.has_tool_renderer(tool_name) {
                return None;
            }
            Which::ToolCall(tool_name.clone(), args.clone())
        }
        AgentSessionEvent::ToolExecutionEnd { tool_name, result, .. } => {
            if !ext_host.has_tool_renderer(tool_name) {
                return None;
            }
            Which::ToolResult(tool_name.clone(), result.clone())
        }
        _ => return None,
    };
    // A FAULTING renderer collapses to `None` here on purpose: both of this function's surfaces
    // swallow the throw upstream — a custom message falls through to its default `[type] body` box
    // (`custom-message.ts:82-84`, `catch { /* Fall through to default rendering */ }`) and a tool
    // row keeps its built-in shell. The distinction is preserved by the host
    // ([`cyrup_ext::RenderOutcome`]) for the ENTRY surface, which does NOT swallow it — see
    // [`extension_render_entry`].
    run_renderer(ext_host, which).await.into_text()
}

/// Ask the loaded extensions to render an appended custom ENTRY (X15; Pi `addCustomEntryToChat`,
/// `interactive-mode.ts:3431-3436`, resolving `extensionRunner.getEntryRenderer(entry.customType)`
/// at `runner.ts:593-600`).
///
/// This is the ONE surface where the renderer's fault is user-visible, so it is the one that must
/// NOT collapse the three-state [`cyrup_ext::RenderOutcome`] the way [`extension_render`] does:
///
/// * no renderer, or a renderer that drew nothing (`:3433-3435` / `:3438-3440`) →
///   [`Rendered::None`], and the caller draws NOTHING;
/// * a rendered component (`custom-entry.ts:58-60`) → [`Rendered::Text`];
/// * a renderer that THREW (`custom-entry.ts:47-52`) → [`Rendered::Failed`], the failure box.
///
/// Same cheap sync pre-check (`if (!renderer) return;`) and the same spawn + bounded wait as
/// [`extension_render`].
pub async fn extension_render_entry(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    custom_type: &str,
    entry: &serde_json::Value,
) -> crate::transcript::Rendered {
    if !ext_host.has_entry_renderer(custom_type) {
        return crate::transcript::Rendered::None;
    }
    run_renderer(ext_host, Which::Entry(custom_type.to_string(), entry.clone())).await
}

/// The `customType` of a serialized session entry — the key an entry renderer is registered under
/// (Pi `entry.customType`, `session-manager.ts` `CustomEntry`; read by `addCustomEntryToChat`,
/// `interactive-mode.ts:3432`).
///
/// Both spellings are accepted because the persisted entry carries `customType` while the event
/// envelope's discriminator is `type`; `"custom"` is the last-resort label for an entry that
/// carries neither, and no renderer will ever claim it.
pub(crate) fn custom_entry_type(entry: &serde_json::Value) -> String {
    entry
        .get("customType")
        .or_else(|| entry.get("custom_type"))
        .or_else(|| entry.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("custom")
        .to_string()
}

/// Which registered renderer to invoke, and with what payload.
enum Which {
    Message(String, serde_json::Value),
    ToolCall(String, serde_json::Value),
    ToolResult(String, serde_json::Value),
    Entry(String, serde_json::Value),
}

/// Resolve an extension's registered MESSAGE renderer for `custom_type` and run it against
/// `payload` (Pi `getMessageRenderer(customType)`, `extensions/runner.ts:579-587`).
///
/// X11 — the REPLAY walk needs this without an `AgentSessionEvent` to hand: a `/resume` reads
/// persisted `AgentMessage`s, not events, and Pi performs the identical lookup there
/// (`interactive-mode.ts:3471`) as on the live path. Same cheap sync `has_message_renderer`
/// pre-check and the same spawn + bounded wait as [`extension_render`], so a wedged guest degrades
/// a replayed block to its built-in framing instead of stalling the resume.
pub async fn extension_render_message(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    custom_type: &str,
    payload: &serde_json::Value,
) -> Option<String> {
    if !ext_host.has_message_renderer(custom_type) {
        return None;
    }
    // Same collapse as [`extension_render`]: `custom-message.ts:82-84` swallows the throw.
    run_renderer(ext_host, Which::Message(custom_type.to_string(), payload.clone()))
        .await
        .into_text()
}

/// Run one renderer on its OWN task under [`EXTENSION_RENDER_TIMEOUT`] — see that constant's doc for
/// why the call must never be awaited inline on the event path.
///
/// X15 — the host now reports three outcomes ([`cyrup_ext::RenderOutcome`]) and this carries all
/// three back; [`extension_render`]/[`extension_render_message`] are what collapse `Failed` into
/// the default framing for their surfaces, because upstream does
/// (`custom-message.ts:82-84` catches and falls through).
async fn run_renderer(
    ext_host: &Arc<cyrup_ext::ExtensionHost>,
    which: Which,
) -> crate::transcript::Rendered {
    use crate::transcript::Rendered;
    let host = ext_host.clone();
    let task = tokio::spawn(async move {
        match which {
            Which::Message(key, payload) => host.render_message_call_outcome(&key, &payload).await,
            Which::ToolCall(key, payload) => host.render_tool_call_outcome(&key, &payload).await,
            Which::ToolResult(key, payload) => host.render_tool_result_outcome(&key, &payload).await,
            Which::Entry(key, payload) => host.render_entry(&key, &payload).await,
        }
    });
    let abort = task.abort_handle();
    match tokio::time::timeout(EXTENSION_RENDER_TIMEOUT, task).await {
        Ok(Ok(cyrup_ext::RenderOutcome::Rendered(v))) => Rendered::Text(rendered_text(&v)),
        // The renderer FAULTED. `cyrup-ext` already contained the fault (native panic caught,
        // guest trap mapped) and kept its message; upstream's `catch` binding is the same value.
        Ok(Ok(cyrup_ext::RenderOutcome::Failed(message))) => Rendered::Failed(message),
        Ok(Ok(cyrup_ext::RenderOutcome::None)) => Rendered::None,
        // The renderer TASK itself panicked — outside the host's `catch_unwind`, so no message
        // survived the unwind. Report it as a fault anyway: something threw, and reporting `None`
        // here is precisely the conflation X15 is about.
        Ok(Err(_)) => Rendered::Failed("renderer task panicked".to_string()),
        Err(_) => {
            // Cancel the wedged call rather than detaching it: dropping a `JoinHandle` only
            // detaches, and a renderer that blocks once will block again on the next event, so
            // detached tasks would pile up behind the instance's store lock.
            abort.abort();
            // NOT a fault: upstream renderers are synchronous and cannot time out, so there is no
            // `catch` to model. A wedged renderer degrades to the built-in framing (and, for an
            // entry, to nothing at all) rather than accusing the extension of throwing.
            Rendered::None
        }
    }
}

/// How deep a widget tree may nest before the flattener gives up (a guest can hand the host any
/// JSON, including a pathologically deep one; the flattener must terminate on adversarial input).
const MAX_WIDGET_DEPTH: usize = 16;

/// Flatten a renderer's returned JSON — a SERIALIZED WIDGET TREE — into the display text the
/// transcript draws.
///
/// This is the host half of the `render-call`/`render-result` contract documented in
/// `cyrup-ext/wit/world.wit`. Pi's renderers return an in-process `pi-tui` `Component` which the
/// interactive mode adds as a child of `CustomMessageComponent`/`ToolExecutionComponent`
/// (`components/custom-message.ts:66-81`, `components/tool-execution.ts:81-112`); nothing is ever
/// stringified. A WASM guest cannot hand back a live object, so cyrup's wire analog is the
/// component tree SERIALIZED, and the host is what turns it back into rows — the exact step that
/// was missing: every non-string return used to be pretty-printed, so a guest following cyrup's own
/// SDK example (`{"widget":"text","text":…}`) drew a raw JSON blob where Pi draws the component.
///
/// The vocabulary mirrors the `pi-tui` components a renderer actually returns (`packages/tui/src/
/// index.ts:13-32`); it is duplicated verbatim in both WIT world copies and constructed by
/// `cyrup_ext_sdk::widget` on the guest side:
///
/// | JSON                                              | Pi component      |
/// |---------------------------------------------------|-------------------|
/// | `"…"` (a bare string)                              | `Text` (degenerate) |
/// | `{"widget":"text","text":"…"}`                     | `Text` — the dominant shape |
/// | `{"widget":"markdown","text":"…"}`                 | `Markdown`        |
/// | `{"widget":"truncated-text","text":"…"}`           | `TruncatedText`   |
/// | `{"widget":"spacer","lines":n}` (default 1)        | `Spacer`          |
/// | `{"widget":"box"\|"container","children":[…]}`     | `Box` / `Container` — stacked |
/// | `{"widget":"hstack","children":[…]}`               | `HStack` — joined on one row |
/// | `[…]` (a bare array)                               | shorthand for a stack |
///
/// Anything the vocabulary does not cover — an unknown `widget` tag, a missing tag, a tree deeper
/// than [`MAX_WIDGET_DEPTH`] — falls back to the pretty-printed JSON rather than being dropped, so
/// an author who mistypes a node SEES the node instead of a blank row. The fallback applies to the
/// WHOLE tree, not the offending node, so the JSON on screen is the one the guest actually returned.
fn rendered_text(v: &serde_json::Value) -> String {
    flatten_widget(v, 0)
        .unwrap_or_else(|| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
}

/// One node of [`rendered_text`]'s widget tree. `None` = "not a widget I know", which the caller
/// turns into the whole-tree JSON fallback.
fn flatten_widget(v: &serde_json::Value, depth: usize) -> Option<String> {
    use serde_json::Value;
    if depth > MAX_WIDGET_DEPTH {
        return None;
    }
    match v {
        // A bare string is the degenerate `Text` node: a renderer that just wants to hand back the
        // lines it wants drawn should not have to wrap them.
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => flatten_children(items, depth, "\n"),
        Value::Object(o) => {
            let text = || o.get("text").and_then(Value::as_str).unwrap_or("").to_string();
            let children = |sep: &str| match o.get("children") {
                Some(Value::Array(items)) => flatten_children(items, depth, sep),
                // A container with no children is an empty row, not an error (Pi's `Container`
                // renders nothing until something is added to it).
                None => Some(String::new()),
                Some(_) => None,
            };
            match o.get("widget").and_then(Value::as_str)? {
                "text" | "markdown" | "truncated-text" => Some(text()),
                "spacer" => {
                    // `n` blank rows = a string of `n - 1` newlines (one empty row needs no
                    // separator). Clamped so a guest cannot ask the transcript for a million rows.
                    let n = o.get("lines").and_then(Value::as_u64).unwrap_or(1).min(64) as usize;
                    Some("\n".repeat(n.saturating_sub(1)))
                }
                "box" | "container" => children("\n"),
                "hstack" => children(""),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Flatten every child, joining with `sep`. One unrecognized child fails the WHOLE tree so the
/// caller's JSON fallback shows the guest's actual return rather than a half-rendered tree with a
/// silently missing row.
fn flatten_children(items: &[serde_json::Value], depth: usize, sep: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        out.push(flatten_widget(item, depth.saturating_add(1))?);
    }
    Some(out.join(sep))
}

/// The largest `edit` target that gets a synchronous pre-execution preview.
///
/// Pi's `computeEditsDiff` is `async` and its result lands via `context.invalidate()`, so an
/// enormous file only costs it a late repaint. cyrup's fold ([`App::ingest_event_rendered`]) is
/// synchronous — it mutates `&mut self` from a `select!` arm — so the read+diff happens on the UI
/// thread and an unbounded one would stall the frame. Source files an `edit` targets are orders of
/// magnitude under this; above it the preview is simply skipped and the post-write `details.diff`
/// renders as before, which is the pre-preview behaviour, not a regression.
const MAX_EDIT_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;

/// Pi `getRenderablePreviewInput` (edit.ts:170-192) + the `computeEditsDiff` call its `renderCall`
/// makes (`:377-386`), as one synchronous step.
///
/// The arguments are the RAW ones off the wire, before the agent preflight runs the tool's
/// `prepare_arguments` shim, so both shapes Pi accepts are handled here too: `edits[]` of
/// `{oldText, newText}` string pairs, and the legacy top-level `{oldText, newText}` single edit.
/// The path may arrive as `path` or `file_path`. Anything else yields `None` — no preview, no
/// change in behaviour.
fn edit_preview(
    args: &serde_json::Value,
    cwd: &std::path::Path,
) -> Option<Result<String, String>> {
    let obj = args.as_object()?;
    let path = obj
        .get("path")
        .or_else(|| obj.get("file_path"))
        .and_then(serde_json::Value::as_str)?;

    let str_field = |v: &serde_json::Value, k: &str| {
        v.get(k).and_then(serde_json::Value::as_str).map(str::to_string)
    };
    let edits: Vec<(String, String)> = match obj.get("edits").and_then(serde_json::Value::as_array) {
        // `args.edits.every(edit => typeof edit?.oldText === "string" && ...)` — one malformed
        // entry rejects the whole preview (edit.ts:180-186).
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|e| Some((str_field(e, "oldText")?, str_field(e, "newText")?)))
            .collect::<Option<Vec<_>>>()?,
        // `if (typeof args.oldText === "string" && typeof args.newText === "string")` (`:188-190`).
        _ => vec![(str_field(args, "oldText")?, str_field(args, "newText")?)],
    };

    let absolute = cyrup_tools::path::resolve_to_cwd(path, cwd);
    if std::fs::metadata(&absolute).is_ok_and(|m| m.len() > MAX_EDIT_PREVIEW_BYTES) {
        return None;
    }
    Some(
        cyrup_tools::tools::edit_diff::compute_edits_diff(path, &edits, cwd)
            .map(|p| p.diff)
            .map_err(|e| e.to_string()),
    )
}

/// The `usage` a `toolResult` message carries, if any (Pi `entry.message.role === "toolResult" &&
/// entry.message.usage`, `footer.ts:99-101`). Read through the same serde projection
/// [`custom_message_from_event`] uses, for the same reason: the `AgentMessage` type lives in
/// `cyrup-agent`, which is only a dev-dependency here.
fn tool_result_usage_from_event(ev: &AgentSessionEvent) -> Option<cyrup_core::Usage> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("toolResult") {
        return None;
    }
    serde_json::from_value(message.get("usage")?.clone()).ok()
}

/// The `role` discriminant of the message an event carries (`"user"`/`"assistant"`/`"toolResult"`/
/// `"custom"`), read through the same serde projection [`custom_message_from_event`] uses and for
/// the same reason: `AgentMessage` lives in `cyrup-agent`, a dev-dependency here.
///
/// This is Pi's `event.message.role` test (`interactive-mode.ts:3122`, `:3181`).
fn message_role_from_event(ev: &AgentSessionEvent) -> Option<String> {
    let value = serde_json::to_value(ev).ok()?;
    Some(value.get("message")?.get("role")?.as_str()?.to_string())
}

/// The authoritative [`AssistantMessage`](cyrup_core::AssistantMessage) a `message_end` carries, via
/// the same projection. `AgentMessage::Assistant` is an internally-tagged newtype variant, so the
/// serialized object is the assistant message's own fields plus `role` — which deserializes
/// straight back into `AssistantMessage`.
fn assistant_message_from_event(ev: &AgentSessionEvent) -> Option<cyrup_core::AssistantMessage> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return None;
    }
    serde_json::from_value(message.clone()).ok()
}

fn custom_message_from_event(ev: &AgentSessionEvent) -> Option<(String, String)> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("custom") {
        return None;
    }
    let kind =
        message.get("kind").and_then(serde_json::Value::as_str).unwrap_or("custom").to_string();
    let body = message.get("payload").map(custom_message_text).unwrap_or_default();
    Some((kind, body))
}

/// Extract display text from a `Custom` message payload (`string | (Text|Image)[]`, mirroring Pi's
/// `getCustomMessageText`): a JSON string is used verbatim; an array joins its `{text}` parts; any
/// other shape yields the empty string (rendered as a bare label).
fn custom_message_text(payload: &serde_json::Value) -> String {
    match payload {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Collapse a multi-line string to a single space-joined line, truncated to 80 graphemes with an
/// ellipsis (`truncateSummary` — selector descriptions, tree previews).
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

/// Project a flattened [`SessionDagNode`] (feature #2) into the tree selector's [`TreeNode`]: map the
/// UI-agnostic [`SessionDagKind`] to the render [`TreeKind`] glyph, carry depth/label/fold/leaf/label,
/// and use the leaf marker (`◀`) as the time label so the active branch tip is visible in the row.
/// Build the `/model` picker rows from the live session (feature #1): the FULL multi-provider registry
/// filtered to CONFIGURED providers (Pi `modelRegistry.getAvailable()`, model-selector.ts:152 +
/// model-registry.ts:644), each row tagged with its provider, whether it is the active model, and
/// whether it is in the scoped set (drives the `⇥` scope filter). `together` appears once
/// `TOGETHER_API_KEY` is set; the offline faux default stays selectable. Shared by the bare picker and
/// the `/model <text>` exact-match/pre-filter path so both see the identical catalog.
fn model_entries(session: &AgentSession) -> Vec<ModelEntry> {
    let current = session.model();
    let scoped: std::collections::HashSet<String> =
        session.scoped_models().into_iter().map(|sm| sm.model.id.to_string()).collect();
    session
        .available_model_catalog()
        .iter()
        .map(|m| ModelEntry {
            id: m.id.to_string(),
            name: m.name.clone(),
            provider: m.provider.to_string(),
            // No model selected ⇒ no row is marked current (pi renders the `/model` list against
            // the optional `session.model`).
            current: current.as_ref().is_some_and(|c| {
                m.id.as_str() == c.model.as_str() && m.provider.as_str() == c.provider.as_str()
            }),
            scoped: scoped.contains(m.id.as_str()),
        })
        .collect()
}

/// Resolve the external-editor command for the live session honoring Pi's precedence — settings
/// `externalEditor` → `$VISUAL` → `$EDITOR` → platform default (F14; Pi `getExternalEditorCommand`,
/// settings-manager.ts:846-848, consulted by `openExternalEditor` extension-editor.ts:117). Delegates
/// to the settings-tested [`cyrup_config::EffectiveSettings::external_editor`] (re-exported as
/// [`cyrup_session_svc::EffectiveSettings`]) so a configured editor is honored over the environment,
/// instead of the old inline `$VISUAL`/`$EDITOR`-only chain that ignored it.
fn resolve_external_editor(session: &AgentSession) -> String {
    session
        .services()
        .settings
        .effective()
        .external_editor(&cyrup_session_svc::EnvVars::from_process())
}

/// Spawn `editor_cmd path` (inheriting stdio) and, on a clean exit, return the file's contents with a
/// single trailing newline stripped (Pi's "reload the edited text"); `None` on a non-zero exit / spawn
/// failure (Pi's "no change"). `editor_cmd` is split on whitespace so `code --wait`-style commands work,
/// with `path` appended as the final argument. Pure (no terminal teardown, no `self`) so the resolved
/// command that actually runs can be exercised directly by a test — the terminal suspend/restore is the
/// caller's ([`App::edit_in_external_editor`]) responsibility.
fn run_editor_over_file(editor_cmd: &str, path: &std::path::Path) -> Option<String> {
    let mut parts = editor_cmd.split_whitespace();
    let status = parts
        .next()
        .map(|bin| std::process::Command::new(bin).args(parts).arg(path).status());
    if let Some(Ok(s)) = status
        && s.success()
        && let Ok(new_text) = std::fs::read_to_string(path)
    {
        let trimmed = new_text.strip_suffix('\n').unwrap_or(&new_text);
        return Some(trimmed.to_string());
    }
    None
}

/// Project one flattened [`SessionDagNode`] into the `/tree` selector's [`TreeNode`].
///
/// `pub` so the projection can be driven directly from a test with a hand-built `SessionDagNode`:
/// it is the production converter (`App::run`'s `/tree` arm maps the whole `session_dag()` through
/// it, `:2338`), and the alternative — standing a real multi-branch `AgentSession` up inside a TUI
/// test — would exercise the session layer rather than this mapping.
pub fn tree_node_from_dag(n: &SessionDagNode) -> TreeNode {
    let kind = match n.kind {
        SessionDagKind::Message | SessionDagKind::Other => TreeKind::Message,
        SessionDagKind::Tool => TreeKind::ToolGroup,
        SessionDagKind::ModelChange => TreeKind::ModelChange,
        SessionDagKind::ThinkingChange => TreeKind::ThinkingChange,
        SessionDagKind::Compaction => TreeKind::Compaction,
    };
    TreeNode {
        id: n.entry_id.to_string(),
        depth: n.depth,
        label: truncate_summary(&n.label),
        kind,
        foldable: n.foldable,
        folded: false,
        has_label: n.has_label,
        // Pi's column here is `labelTimestamp` — WHEN the entry's label was set — and the row it
        // decorates is a labeled one (`tree-selector.ts:741-743`). It was previously fed the literal
        // string `"current"` on the branch tip, which is neither: the `t` toggle
        // (`app.tree.toggleLabelTimestamp`) rendered the word "current" where Pi renders a clock
        // time, and did so on an unlabeled row, in a column Pi leaves off by default.
        //
        // Pi does mark the active path, just not here: `pathMarker` is a `•` prefix ahead of the
        // entry text, driven by an `activePathIds` SET covering the whole root→tip path
        // (`tree-selector.ts:736-738`). `SessionDagNode` carries only `is_leaf`, so that marker is
        // not portable from here either; it is not a substitute this column can hold.
        //
        // Set to `None` until the value exists to put here. It is dropped one and two layers down,
        // not in this crate: `cyrup_session::TreeNode` (manager.rs:29-34) has no timestamp field
        // even though `SessionManager::labels` already holds `(label, label-change timestamp)`
        // (manager.rs:43-44), so `SessionDagNode` (cyrup-session-svc session.rs:136-155) has nothing
        // to carry — its `timestamp` is the ENTRY's, a different quantity. Threading the label
        // timestamp through those two crates is the remaining half of this fix; the render side
        // (Pi's gate, Pi's default, Pi's `[+label time]` marker) is done and will display it the
        // moment a producer sets it.
        time_label: None,
    }
}

/// Build the `/settings` grid rows from the live effective settings (Pi `settings-selector.ts`
/// `SettingsConfig` → `SettingItem`s, :479-712). Each row's `id` is the dotted settings key persisted
/// on cycle; toggles cycle `true`/`false`, choices cycle their fixed sets. Read straight off
/// [`cyrup_session_svc::EffectiveSettings`] so the displayed value matches the merged config.
fn settings_rows(
    eff: &cyrup_session_svc::EffectiveSettings,
    current_theme: &str,
    keymap: &Keymap,
) -> Vec<SettingRow> {
    let choices = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // `const followUpKey = keyDisplayText("app.message.followUp")` (`settings-selector.ts:491`),
    // interpolated into the follow-up row's description at `:513`. `keyDisplayText` is `keyText`
    // with `{ capitalize: true }` (`keybinding-hints.ts:37-39`), i.e. `Alt+Enter`, and it reads the
    // LIVE table — a rebind changes the sentence.
    let follow_up_key = keymap
        .keys_label(Action::FollowUp)
        .map(|k| crate::chrome::format_key_text(&k, true))
        .unwrap_or_default();
    vec![
        // The "Theme" row opens the theme picker (Pi `SettingItem.submenu` → `ThemeSubmenu`,
        // settings-selector.ts:603-610) — the one in-app path Pi reaches theme switching through.
        SettingRow::submenu("theme", "Theme", current_theme.to_string(), "theme")
            .with_description("Color theme for the interface"),
        SettingRow::toggle("compaction.enabled", "Auto-compact", eff.compaction_enabled())
            .with_description("Automatically compact context when it gets too large"),
        SettingRow::toggle("terminal.showImages", "Show images", eff.show_images())
            .with_description("Render images inline in terminal"),
        SettingRow::choice(
            "terminal.imageWidthCells",
            "Image width",
            eff.image_width_cells().to_string(),
            choices(&["60", "80", "120"]),
        )
        .with_description("Preferred inline image width in terminal cells"),
        SettingRow::toggle("images.autoResize", "Auto-resize images", eff.image_auto_resize())
            .with_description("Resize large images to 2000x2000 max for better model compatibility"),
        SettingRow::toggle("images.blockImages", "Block images", eff.block_images())
            .with_description("Prevent images from being sent to LLM providers"),
        SettingRow::toggle("enableSkillCommands", "Skill commands", eff.enable_skill_commands())
            .with_description("Register skills as /skill:name commands"),
        // `showHardwareCursor` / `terminal.clearOnShrink` — the effective getters need the env surface;
        // a default `EnvVars` yields the persisted setting (else `false`), which is what the grid edits.
        SettingRow::toggle(
            "showHardwareCursor",
            "Show hardware cursor",
            eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::default()),
        )
        .with_description("Show the terminal cursor while still positioning it for IME support"),
        SettingRow::toggle(
            "terminal.clearOnShrink",
            "Clear on shrink",
            eff.clear_on_shrink(&cyrup_session_svc::EnvVars::default()),
        )
        .with_description("Clear empty rows when content shrinks (may cause flicker)"),
        SettingRow::choice(
            "editorPaddingX",
            "Editor padding",
            eff.editor_padding_x().to_string(),
            choices(&["0", "1", "2", "3"]),
        )
        .with_description("Horizontal padding for input editor (0-3)"),
        // Inserted right after editor-padding, matching Pi (`settings-selector.ts:681-689` splices the
        // "Output padding" row after "editor-padding"). Cycles 0|1; honored live by the transcript.
        SettingRow::choice(
            "outputPad",
            "Output padding",
            eff.output_pad().to_string(),
            choices(&["0", "1"]),
        )
        .with_description(
            "Horizontal padding for user messages, assistant messages, and thinking",
        ),
        SettingRow::choice(
            "autocompleteMaxVisible",
            "Autocomplete max items",
            eff.autocomplete_max_visible().to_string(),
            choices(&["3", "5", "7", "10", "15", "20"]),
        )
        .with_description("Max visible items in autocomplete dropdown (3-20)"),
        // `httpIdleTimeoutMs` — cycle the raw millisecond presets (Pi shows human labels; the persisted
        // value is the same ms number). `disabled` = 0 (`HTTP_IDLE_TIMEOUT_CHOICES`, http-dispatcher.ts:5).
        SettingRow::choice(
            "httpIdleTimeoutMs",
            "HTTP idle timeout (ms)",
            eff.http_idle_timeout_ms().unwrap_or(300_000).to_string(),
            choices(&["30000", "60000", "120000", "300000", "0"]),
        )
        .with_description(
            "Maximum idle gap while waiting for HTTP headers or body chunks. Disable for local \
             models that pause longer than five minutes.",
        ),
        SettingRow::toggle("hideThinkingBlock", "Hide thinking", eff.hide_thinking_block())
            .with_description("Hide thinking blocks in assistant responses"),
        SettingRow::toggle("collapseChangelog", "Collapse changelog", eff.collapse_changelog())
            .with_description("Show condensed changelog after updates"),
        SettingRow::toggle("quietStartup", "Quiet startup", eff.quiet_startup())
            .with_description("Disable verbose printing at startup"),
        SettingRow::toggle(
            "enableInstallTelemetry",
            "Install telemetry",
            eff.enable_install_telemetry(),
        )
        .with_description(
            "Send an anonymous version/update ping after changelog-detected updates",
        ),
        SettingRow::toggle(
            "terminal.showTerminalProgress",
            "Terminal progress",
            eff.show_terminal_progress(),
        )
        .with_description("Show OSC 9;4 progress indicators in the terminal tab bar"),
        SettingRow::choice(
            "steeringMode",
            "Steering mode",
            eff.steering_mode(),
            choices(&["all", "one-at-a-time"]),
        )
        .with_description(
            "Enter while streaming queues steering messages. 'one-at-a-time': deliver one, wait \
             for response. 'all': deliver all at once.",
        ),
        SettingRow::choice(
            "followUpMode",
            "Follow-up mode",
            eff.follow_up_mode(),
            choices(&["all", "one-at-a-time"]),
        )
        .with_description(format!(
            "{follow_up_key} queues follow-up messages until agent stops. 'one-at-a-time': \
             deliver one, wait for response. 'all': deliver all at once."
        )),
        SettingRow::choice(
            "transport",
            "Transport",
            eff.transport(),
            // Pi's four `TransportSetting` values in Pi's own cycle order (`settings-selector.ts:
            // 505-510`: `["sse", "websocket", "websocket-cached", "auto"]`). `websocket-cached` was
            // missing here, so a value the settings parser and `parse_transport` both accept was
            // unreachable from `/settings` and cycling past `sse` could never select it.
            choices(&["sse", "websocket", "websocket-cached", "auto"]),
        )
        .with_description(
            "Preferred transport for providers that support multiple transports",
        ),
        SettingRow::choice(
            "doubleEscapeAction",
            "Double-escape action",
            eff.double_escape_action(),
            choices(&["fork", "tree", "none"]),
        )
        .with_description("Action when pressing Escape twice with empty editor"),
        SettingRow::choice(
            "treeFilterMode",
            "Tree filter mode",
            eff.tree_filter_mode(),
            choices(&["default", "no-tools", "user-only", "labeled-only", "all"]),
        )
        .with_description("Default filter when opening /tree"),
        SettingRow::choice(
            "defaultProjectTrust",
            "Default project trust",
            default_trust_label(eff.default_project_trust()),
            choices(&["ask", "always", "never"]),
        )
        .with_description(
            "Fallback behavior when no extension or saved trust decision decides project trust",
        ),
    ]
}

/// The settings string for a [`cyrup_session_svc::DefaultProjectTrust`] (Pi serializes it as the
/// lowercase enum value `ask`/`always`/`never`).
fn default_trust_label(trust: cyrup_session_svc::DefaultProjectTrust) -> String {
    use cyrup_session_svc::DefaultProjectTrust as D;
    match trust {
        D::Ask => "ask",
        D::Always => "always",
        D::Never => "never",
    }
    .to_string()
}

/// Coerce a cycled `/settings` value string back into JSON for persistence: `true`/`false` → bool, an
/// integer → number, else a string (Pi's settings each have a typed `onChange`; the grid cycles the
/// display string, so we re-type it here).
fn parse_setting_value(value: &str) -> serde_json::Value {
    match value {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        other => match other.parse::<i64>() {
            Ok(n) => serde_json::Value::from(n),
            Err(_) => serde_json::Value::String(other.to_string()),
        },
    }
}

/// Format the saved trust-decision header line for the `/trust` selector (Pi `formatDecision`,
/// trust-selector.ts:23-31): `none`, or `trusted (path)` / `untrusted (path)`.
fn format_saved_trust(saved: &Option<cyrup_session_svc::TrustEntry>) -> String {
    match saved {
        None => "none".to_string(),
        Some(entry) => {
            let label = if entry.decision.is_trusted() { "trusted" } else { "untrusted" };
            format!("{label} ({})", entry.path.display())
        }
    }
}

/// The `/resume` row label for a session (Pi `session-selector.ts` row): its name (or first message),
/// trimmed to one line.
fn session_label(info: &cyrup_session_svc::SessionInfo) -> String {
    let raw = match &info.name {
        Some(n) if !n.trim().is_empty() => n.clone(),
        _ if !info.first_message.trim().is_empty() => info.first_message.clone(),
        _ => info.id.to_string(),
    };
    truncate_summary(&raw)
}

/// A monotonic recency key for a session's `modified` time (nanoseconds since the Unix epoch; `0`
/// before the epoch / on a clock fault). Drives the `Relevance` sort tie-break (newest first).
fn system_time_nanos(t: std::time::SystemTime) -> u128 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Render the `/arminsayshi` XBM bitmap as half-block art (`armin.ts`: 31×36, LSB-first, `1` =
/// background, `0` = foreground; two vertical pixels packed per cell into `█`/`▀`/`▄`/space). A pure,
/// deterministic transcript block (the animation effects are omitted as non-testable chrome).
fn armin_art() -> String {
    const WIDTH: usize = 31;
    const HEIGHT: usize = 36;
    const BYTES_PER_ROW: usize = WIDTH.div_ceil(8);
    const BITS: [u8; 144] = [
        255, 255, 255, 127, 255, 240, 255, 127, 255, 237, 255, 127, 255, 219, 255, 127, 255, 183,
        255, 127, 255, 119, 254, 127, 63, 248, 254, 127, 223, 255, 254, 127, 223, 63, 252, 127,
        159, 195, 251, 127, 111, 252, 244, 127, 247, 15, 247, 127, 247, 255, 247, 127, 247, 255,
        227, 127, 247, 7, 232, 127, 239, 248, 103, 112, 15, 255, 187, 111, 241, 0, 208, 91, 253,
        63, 236, 83, 193, 255, 239, 87, 159, 253, 238, 95, 159, 252, 174, 95, 31, 120, 172, 95, 63,
        0, 80, 108, 127, 0, 220, 119, 255, 192, 63, 120, 255, 1, 248, 127, 255, 3, 156, 120, 255,
        7, 140, 124, 255, 15, 206, 120, 255, 255, 207, 127, 255, 255, 207, 120, 255, 255, 223, 120,
        255, 255, 223, 125, 255, 255, 63, 126, 255, 255, 255, 127,
    ];
    // `1` (background) → false; `0` (foreground) → true. Out-of-range rows are background.
    let pixel = |x: usize, y: usize| -> bool {
        if y >= HEIGHT {
            return false;
        }
        let byte_index = y * BYTES_PER_ROW + x / 8;
        match BITS.get(byte_index) {
            Some(byte) => ((byte >> (x % 8)) & 1) == 0,
            None => false,
        }
    };
    let mut out = String::new();
    let rows = HEIGHT.div_ceil(2);
    for row in 0..rows {
        for x in 0..WIDTH {
            let upper = pixel(x, row * 2);
            let lower = pixel(x, row * 2 + 1);
            out.push(match (upper, lower) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push('\n');
    }
    out
}

/// The six live-region row heights `[msg, band, images, slot, popup, footer]` for a viewport of
/// `avail` rows (audit #1). Filled by priority **from the bottom** — footer, then the editor/selector
/// slot (+completion popup), then the status band, then the inline image strip — and the message
/// region takes the remainder, **capped to the active turn's content height** so an empty turn never
/// balloons into a void (the old `Constraint::Min(1)` flex). The function is idempotent: feeding back
/// its own sum reproduces the same split, so [`render`] (called with the viewport height) and
/// [`live_region_height`] (called with the terminal height) never disagree on row counts.
/// The editor's visible text-row budget on a `term_rows`-row terminal — Pi `editor.ts:499-501`:
///
/// ```text
/// // Calculate max visible lines: 30% of terminal height, minimum 5 lines
/// const terminalRows = this.tui.terminal.rows;
/// const maxVisibleLines = Math.max(5, Math.floor(terminalRows * 0.3));
/// ```
///
/// `floor(rows * 0.3)` is computed in integers as `rows * 3 / 10` (identical for every `u16`, and
/// free of the float rounding that would make e.g. `rows = 10` ambiguous). Rules are NOT counted:
/// the editor draws `1 + min(layoutLines, maxVisibleLines) + 1` rows.
///
/// Shared by [`region_constraints`] (which reserves the slot) and
/// [`crate::extension_editor::ExtensionEditorSelector`] (E12 — the `ui.editor` dialog embeds the
/// same `Editor`, `extension-editor.ts:70`, so the same cap applies to its body).
pub(crate) fn max_visible_editor_lines(term_rows: u16) -> u16 {
    ((u32::from(term_rows) * 3 / 10).min(u32::from(u16::MAX)) as u16).max(5)
}

fn region_constraints(state: &AppState, width: u16, avail: u16) -> [u16; 6] {
    let avail = avail.max(1);
    let max_editor = avail.saturating_sub(2).max(3);
    // A selector owns the slot at its desired height; otherwise the editor sizes to its line count +
    // the two rule rows (spec/tui/05 §1.1, spec/tui/03 §3.1).
    let want_slot = match state.selector.as_ref() {
        Some(active) => active.inner.desired_height(width).clamp(3, max_editor),
        // Size from the VISUAL (wrapped) line count, windowed and measured exactly as Pi's
        // `Editor.render` does:
        //
        // * **E15 — measure at the width it renders at.** Pi derives ONE `layoutWidth` and feeds it
        //   to both `this.lastWidth` and `layoutText()` (`editor.ts:489-497`). cyrup measured at a
        //   hardcoded `width - 1` while [`crate::editor::InputEditor`]'s render wraps at
        //   `layout_width(width)` = `width - 2 * paddingX` when `paddingX > 0`, so any
        //   `editorPaddingX` made the render wrap NARROWER than the measurement and produced rows
        //   the slot had no space for — clipped, caret row included.
        // * **E3 — the window is capped at 30% of the terminal.** `maxVisibleLines = Math.max(5,
        //   Math.floor(terminalRows * 0.3))` (`editor.ts:499-501`), then `layoutLines.slice(...)`
        //   (`:519`). The old cap was `avail - 2`, so a long paste grew the editor until it owned the
        //   terminal minus two rows and the transcript collapsed: on a 40-row terminal pi shows 12
        //   text rows and scrolls, cyrup showed 38.
        //
        // The `+2` is the two rule rows; `clamp(3, max_editor)` stays as the viewport backstop.
        None => (state
            .editor
            .visual_line_count(usize::from(state.editor.layout_width(width)))
            .min(usize::from(max_visible_editor_lines(state.term_rows)))
            .min(u16::MAX as usize) as u16)
            .saturating_add(2)
            .clamp(3, max_editor),
    };
    // The completion popup is appended below the editor's bottom rule (spec/tui/04 §7); suppressed
    // while a selector owns the slot.
    let want_popup = if state.selector.is_some() {
        0
    } else {
        state.editor.autocomplete().map(|ac| ac.list.rendered_height()).unwrap_or(0)
    };
    let footer_max: u16 = if state.status.has_extension_statuses() { 3 } else { 2 };
    let want_status = state.indicator.is_active() || state.reserve_status_rows;
    let want_images: u16 = if state.selector.is_some() || state.pending_images.is_empty() {
        0
    } else {
        state
            .pending_images
            .iter()
            .map(|b| state.image_renderer.cell_size(b, width).1)
            .fold(0u16, |a, h| a.saturating_add(h))
    };

    // L7 — the editor's MINIMUM HEIGHT. Pi docks the two bottom regions with explicit floors
    // (`interactive-mode.ts:876-883`):
    //
    // ```ts
    // const dock = new TuiLayouts.VStack([
    //     { component: this.pendingMessagesContainer, shrink: 1, minSize: 0 },
    //     { component: this.statusContainer,          shrink: 1, minSize: 0 },
    //     { component: this.widgetContainerAbove,     shrink: 1, minSize: 0 },
    //     { component: this.editorContainer,          shrink: 1, minSize: 3 },
    //     { component: this.widgetContainerBelow,     shrink: 1, minSize: 0 },
    //     { component: this.footerContainer,          shrink: 1, minSize: 1 },
    // ]);
    // ```
    //
    // and `allocateStackSizes`' shrink pass only ever takes rows from an entry while
    // `sizes[index] > (entry.minSize ?? 0)` — `capacity = sizes[index] - minSize`
    // (`tui/src/components/stack.ts:109,124`). So pi's editor never goes below 3 rows. When even
    // the floors do not fit, `candidates` empties and the pass returns (`:111`) with the stack
    // OVERFLOWING its box; the children past the box's clip rect are the ones that vanish, and the
    // editor — laid out before the footer — is not one of them.
    //
    // cyrup allocated the footer first and then took `want_slot.min(remaining)` with no floor at
    // all, so on a viewport of 3-4 rows the editor was squeezed to 1-2 rows: its own top/bottom
    // rules do not fit, let alone a line of text. Both floors are now reserved up front, editor
    // first, and only the surplus is handed out — which reproduces pi's answer on a very short
    // terminal (editor 3, footer 1 at 4 rows; editor 3 and no footer at 3) and is bit-identical to
    // the old split at every height where the old one was not already squeezing.
    const EDITOR_MIN_ROWS: u16 = 3;
    let mut remaining = avail;
    let slot_floor = want_slot.min(EDITOR_MIN_ROWS).min(remaining);
    remaining = remaining.saturating_sub(slot_floor);
    let footer_floor = 1u16.min(remaining);
    remaining = remaining.saturating_sub(footer_floor);
    // Surplus, in the old order: the footer fills out to `footer_max`, then the slot to `want_slot`.
    let footer_extra = footer_max.saturating_sub(footer_floor).min(remaining);
    let footer = footer_floor.saturating_add(footer_extra);
    remaining = remaining.saturating_sub(footer_extra);
    let slot_extra = want_slot.saturating_sub(slot_floor).min(remaining);
    let slot = slot_floor.saturating_add(slot_extra);
    remaining = remaining.saturating_sub(slot_extra);
    let popup = want_popup.min(remaining);
    remaining = remaining.saturating_sub(popup);
    let band = if want_status { 2u16.min(remaining) } else { 0 };
    remaining = remaining.saturating_sub(band);
    let images = want_images.min(remaining);
    remaining = remaining.saturating_sub(images);
    // The message region = the active turn's content, plus the startup-hint block at idle, capped
    // to whatever rows remain (so the inline viewport stays content-sized, not full-screen).
    let active = state.transcript.content_height(width as usize, &state.theme).min(u16::MAX as usize)
        as u16;
    // …at the block's WRAPPED height (`Text.render` wraps at `contentWidth = width - paddingX * 2`,
    // `tui/src/components/text.ts:64-67`), so a narrow terminal reserves the extra rows the block
    // grows into instead of clipping them off.
    let hint = if state.show_startup_hints
        && state.selector.is_none()
        && !state.transcript.has_active()
    {
        crate::chrome::compact_hint_height(&state.theme, &state.keymap, width)
    } else {
        0
    };
    let msg = active.max(hint).min(remaining);
    [msg, band, images, slot, popup, footer]
}

/// The inline-viewport height = the sum of the live-region rows (audit #1). Driven by
/// [`region_constraints`] against the **terminal** height so the content-sized viewport never
/// exceeds the screen.
fn live_region_height(state: &AppState, width: u16, term_height: u16) -> u16 {
    // A floating overlay (hotkeys/help; spec/tui/05 §2) is a modal that draws *over* the whole live
    // region — it needs the full screen to center its box, so the inline viewport expands to the
    // terminal height while one is open (the editor/footer still render behind it).
    if !state.overlays.is_empty() {
        return term_height.max(1);
    }
    region_constraints(state, width, term_height).iter().copied().fold(0u16, u16::saturating_add)
}

/// Pure render: lay out conversation / editor / status and render each component (`state -> frame`).
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    let [msg_h, band_h, images_h, slot_h, popup_h, footer_h] =
        region_constraints(state, area.width, area.height);
    let _ = msg_h; // the message region absorbs the remainder via `Min(0)` below.
    let [msg_area, band_area, images_area, slot_area, popup_area, status_area] = Layout::vertical([
        // `Min(0)` (not the old `Min(1)`): the empty turn must not balloon the viewport (audit #1).
        Constraint::Min(0),
        Constraint::Length(band_h),
        Constraint::Length(images_h),
        Constraint::Length(slot_h),
        Constraint::Length(popup_h),
        Constraint::Length(footer_h),
    ])
    .areas(area);
    state.transcript.render(frame, msg_area, &state.theme);
    // The compact startup-help block (`compactInstructions` + `compactOnboarding` + `onboarding`,
    // the startup `ExpandableText`'s collapsed body at interactive-mode.ts:936-957, framed by
    // `Spacer(1)` at `:960-962`) occupies the bottom rows of the otherwise-empty message area at
    // startup — just above the editor — sourced from the live keymap so rebinds reflect. It is
    // suppressed once a submission lands (`show_startup_hints` cleared) and while a selector owns
    // the slot, so it never shifts the editor/footer geometry. `render_compact_hints` degrades the
    // block from its edges inward when `rows` is short of the block's wrapped height, so the hint
    // bar itself survives down to a single row.
    if state.show_startup_hints
        && state.selector.is_none()
        && !state.transcript.has_active()
        && msg_area.height >= 1
    {
        let rows =
            crate::chrome::compact_hint_height(&state.theme, &state.keymap, msg_area.width)
                .min(msg_area.height);
        let hint_row = ratatui::layout::Rect {
            x: msg_area.x,
            y: msg_area.y.saturating_add(msg_area.height - rows),
            width: msg_area.width,
            height: rows,
        };
        crate::chrome::render_compact_hints(frame, hint_row, &state.theme, &state.keymap);
    }
    if images_h > 0 {
        render_images(frame, images_area, state);
    }
    if band_h > 0 {
        // `(${keyText("app.interrupt")} to cancel)` (`status-indicator.ts:47,78,100`) — `keyText`,
        // so ALL bound keys joined with `/` (`keybinding-hints.ts:29-36`), not just the first.
        let cancel = state.keymap.keys_label(Action::Interrupt);
        state.indicator.render(frame, band_area, &state.theme, cancel.as_deref());
    }
    if let Some(active) = state.selector.as_mut() {
        active.inner.render(frame, slot_area, &state.theme);
    } else if let Some(loader) = state.loader.as_ref() {
        // A long inline op (e.g. `/share`'s gist creation) owns the slot with a `BorderedLoader`.
        loader.render(frame, slot_area, &state.theme, state.loader_tick);
    } else {
        state.editor.render(frame, slot_area, &state.theme);
        if let Some(ac) = state.editor.autocomplete() {
            // E14: the popup lives INSIDE the editor's padding frame. Pi renders it at
            // `contentWidth` (= `width - paddingX * 2`) and prefixes the same `leftPadding` every
            // text row gets (`editor.ts:591-597`), so with `editorPaddingX` 1–3 — the values
            // `/settings` cycles — the completions line up with the text they complete. cyrup drew
            // them into `popup_area` at full frame width, flush at column 0. No effect at the
            // default padding of 0, which is why it went unnoticed.
            let pad = state.editor.effective_padding(popup_area.width);
            let inner = ratatui::layout::Rect {
                x: popup_area.x.saturating_add(pad),
                y: popup_area.y,
                width: popup_area.width.saturating_sub(pad.saturating_mul(2)),
                height: popup_area.height,
            };
            let lines = ac.list.lines(inner.width, &state.theme);
            frame.render_widget(Paragraph::new(lines).style(state.theme.base_style()), inner);
        }
    }
    state.status.render(frame, status_area, &state.theme);
    // Floating overlays draw last, on top of the live region, bottom→top (spec/tui/05 §2; arch-10
    // §6.4): each clears its own `Rect` then renders its box.
    for overlay in state.overlays.iter_mut() {
        overlay.render(frame, area, &state.theme);
    }
}

/// Render the attached-image strip inline above the editor (`components/image.ts`): stack each
/// [`ImageBlock`] at its natural cell height, drawing the real protocol when `show_images` is on and a
/// text placeholder when off (spec/tui/06 §6). Honors the live image protocol negotiated at startup.
fn render_images(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    for block in &state.pending_images {
        if y >= bottom {
            break;
        }
        let want = state.image_renderer.cell_size(block, area.width).1.max(1);
        let h = want.min(bottom.saturating_sub(y));
        let cell = ratatui::layout::Rect { x: area.x, y, width: area.width, height: h };
        state.image_renderer.render(frame, cell, block, &state.theme, state.show_images);
        y = y.saturating_add(h);
    }
}

// ----------------------------------------------------------------- crossterm wiring ----

impl App<CrosstermBackend<Stdout>> {
    /// Build the production app: raw mode on, bracketed paste + Kitty keyboard flags enabled
    /// (best-effort, with graceful fallback, R-ARCH-TUI-008), inline viewport on stdout.
    ///
    /// The panic hook goes in FIRST, before a single terminal mode is touched, so the window it
    /// covers is a superset of the window that can leave the terminal broken — a panic between
    /// `enable_raw_mode` and the return of this function is exactly as fatal to the user's shell as
    /// one during the event loop. Ports pi's `uncaughtCrash` install
    /// (`interactive-mode.ts:3684-3686`, handler at `:3622-3638`).
    pub fn into_stdout(theme: UiTheme) -> Result<Self, TuiError> {
        crate::panic_hook::install_panic_hook();
        enable_raw_mode()?;
        let mut out = io::stdout();
        out.execute(ratatui::crossterm::event::EnableBracketedPaste)?;
        // Kitty keyboard protocol where supported; ignore failure (legacy terminals).
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        // …and then ASK whether the push took, instead of assuming it did (Pi
        // `queryAndEnableKittyProtocol`, `tui/src/terminal.ts:213-226`). The query has to follow the
        // push — `CSI ? u` reports the top of the terminal's flag stack — and it has to run HERE:
        // this is the one window where raw mode is on and no crossterm reader thread is competing
        // for the reply (see `crate::keyboard_protocol`'s module docs, and
        // `crate::terminal_query`'s for the read's timeout/input-safety contract). The recorded
        // outcome is what the re-entry paths below re-apply and what the startup diagnostics read.
        let _ = crate::keyboard_protocol::negotiate();
        App::new(CrosstermBackend::new(out), theme)
    }

    /// Draw one frame wrapped in synchronized-output markers (CSI 2026, R-10-002 / R-ARCH-TUI-004).
    pub fn draw_synchronized(&mut self) -> Result<(), TuiError> {
        // The OSC 9;4 write for a progress transition the session-event fold (or a `/settings` flip)
        // recorded. Pi writes it synchronously inside the event handler
        // (`interactive-mode.ts:2865-2867` → `terminal.ts:509-523`); cyrup's fold is a pure state
        // transition, so the write happens here — one call site, ahead of the frame, reached by
        // EVERY run-loop arm that can have changed the state. Draining makes it once-per-transition
        // rather than once-per-frame.
        self.flush_terminal_progress();
        let mut out = io::stdout();
        let _ = out.execute(BeginSynchronizedUpdate);
        let res = self.draw();
        let _ = out.execute(EndSynchronizedUpdate);
        res
    }

    /// Suspend the process to the background (Ctrl+Z / `app.suspend`, `core/keybindings.ts`): tear the
    /// terminal back down to a usable cooked state, raise `SIGTSTP` on our own process group so the
    /// shell regains control, then — when the user `fg`s us and the kernel delivers `SIGCONT` — restore
    /// raw mode + the inline viewport and redraw. The signal is raised by shelling out to `kill -s
    /// TSTP <pid>` so the crate stays `#![forbid(unsafe_code)]` with **no** new dependency (a libc
    /// `raise` would need an unsafe shim + a new dep; the `kill` path needs neither). Unix-only; on
    /// other platforms it degrades to a redraw.
    pub fn suspend(&mut self) -> Result<(), TuiError> {
        self.restore()?;
        #[cfg(unix)]
        {
            // Stop our own process group; `kill` exits before the stop takes effect, and we resume on
            // SIGCONT (shell `fg`) at the next statement.
            let pid = std::process::id().to_string();
            let _ = std::process::Command::new("kill").args(["-s", "TSTP", &pid]).status();
        }
        // Resumed (or non-unix): re-enter raw mode + flags, then redraw the live region. The flags
        // are re-pushed unconditionally, exactly as Pi's `start()` does (`terminal.ts:164-166`) —
        // NOT re-negotiated: the crossterm reader thread is live by now, so a `CSI ? u` reply would
        // race it (`crate::keyboard_protocol` module docs). The startup decision still stands.
        enable_raw_mode()?;
        let mut out = io::stdout();
        let _ = out.execute(ratatui::crossterm::event::EnableBracketedPaste);
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let _ = self.terminal.clear();
        self.draw_synchronized()
    }

    /// Open the editor buffer in an external editor (Ctrl+G / `app.editor.external`,
    /// `openExternalEditor` interactive-mode.ts:3611): run the caller-resolved `editor_cmd` (the
    /// settings `externalEditor` → `$VISUAL` → `$EDITOR` → default chain, see [`App::run`]), write the
    /// buffer to a temp `*.pi.md`, tear the TUI down to release the terminal, run the editor (inheriting
    /// stdio), and — on a clean exit — reload the edited text (trailing newline stripped). The terminal
    /// is always restored, even on error. No `unsafe`, no new dependency (`std::process` + `std::fs`).
    pub fn open_external_editor(&mut self, editor_cmd: &str) -> Result<(), TuiError> {
        let current = self.state.editor.text();
        if let Some(new_text) = self.edit_in_external_editor(&current, editor_cmd)? {
            self.state.editor.set_text(&new_text);
        }
        self.draw_synchronized()
    }

    /// `Ctrl+G` pressed inside the extension `ui.editor` dialog (L4 review §3;
    /// [`AppAction::OpenExternalEditorForSelector`]): seed `$VISUAL`/`$EDITOR` with the OPEN
    /// dialog's own buffer (never [`AppState::editor`], the live prompt draft — unrelated) and, on a
    /// clean exit, write the result back into the SAME dialog buffer via
    /// [`Selector::apply_external_edit`] — the dialog stays open (Pi never resolves it from this
    /// path, `extension-editor.ts:119-157`); only `Enter`/`Esc` close it. A no-op if no selector is
    /// open or the open one doesn't support external editing (`external_edit_text` returns `None`).
    fn open_external_editor_for_selector(&mut self, editor_cmd: &str) -> Result<(), TuiError> {
        let Some(current) = self.state.selector.as_ref().and_then(|a| a.inner.external_edit_text())
        else {
            return Ok(());
        };
        if let Some(new_text) = self.edit_in_external_editor(&current, editor_cmd)?
            && let Some(active) = self.state.selector.as_mut()
        {
            active.inner.apply_external_edit(&new_text);
        }
        self.draw_synchronized()
    }

    /// Run the resolved `editor_cmd` over `initial` text and return the edited result on a clean exit
    /// (`Ok(None)` on a non-zero exit / spawn failure / unwritable temp file — Pi's "no change"). Tears
    /// the TUI down for the duration and always restores it before returning, even on failure — the
    /// caller is left with a usable terminal either way.
    ///
    /// `editor_cmd` is resolved by the caller (`App::run`) through the SAME precedence Pi uses —
    /// settings `externalEditor` → `$VISUAL` → `$EDITOR` → platform default
    /// ([`cyrup_config::EffectiveSettings::external_editor`], settings-manager.ts:846,
    /// extension-editor.ts:117) — rather than the old inline `$VISUAL`/`$EDITOR`-only chain that
    /// silently ignored a configured `externalEditor` (F14). This method just SPAWNS the resolved
    /// command via [`run_editor_over_file`].
    ///
    /// The synchronous, TUI-suspending core both [`Self::open_external_editor`] (Ctrl+G on the live
    /// input buffer) and [`Self::open_external_editor_for_selector`] (Ctrl+G inside the extension
    /// `ui.editor` dialog, L4 review §3) share.
    ///
    /// This runs entirely synchronously on the caller's task (no `.await`) — reused directly inside
    /// `App::run`'s `select!` loop is safe (nothing here can deadlock against a concurrently-blocked
    /// guest, unlike the `execute_command`/`run_shortcut` paths, which must never await guest-reentrant
    /// work inline for exactly that reason; see `App::run`'s `AppAction::ExtensionShortcut` arm).
    fn edit_in_external_editor(
        &mut self,
        initial: &str,
        editor_cmd: &str,
    ) -> Result<Option<String>, TuiError> {
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("cyrup-editor-{}.pi.md", std::process::id()));
        if std::fs::write(&tmp, initial).is_err() {
            self.state.transcript.push_status("external editor: could not write temp file");
            return Ok(None);
        }

        // Release the terminal (cooked mode, no inline viewport) so the editor owns the screen.
        self.restore()?;
        let result = run_editor_over_file(editor_cmd, &tmp);
        let _ = std::fs::remove_file(&tmp);

        // Re-enter raw mode + bracketed paste + Kitty flags; the caller redraws. Re-pushed, never
        // re-negotiated — same reason as `suspend` above.
        enable_raw_mode()?;
        let mut out = io::stdout();
        let _ = out.execute(ratatui::crossterm::event::EnableBracketedPaste);
        let _ = execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        let _ = self.terminal.clear();
        Ok(result)
    }

    /// The interactive event loop: `select!` over terminal input, the agent event stream, theme
    /// hot-reload, and cancellation (arch-10 §5). Renders with synchronized output. Submissions are
    /// routed to `session` (steer while streaming, else a fresh prompt; R-10-030).
    pub async fn run(
        &mut self,
        mut input: EventStream<InputEvent>,
        mut events: EventStream<AgentSessionEvent>,
        session: Arc<AgentSession>,
        runtime: Option<Arc<AgentSessionRuntime>>,
        mut theme_rx: Option<tokio::sync::watch::Receiver<Arc<ThemeData>>>,
        cancel: CancelToken,
    ) -> Result<(), TuiError> {
        // The active session + its event subscription are re-bound on every runtime replacement
        // (arch-11 §3.4): a session-swap command (or a runtime-side `SessionReplaced`) bumps the
        // runtime's generation `watch`, the loop drops the stale subscription, subscribes the new
        // session, and re-binds the UI ([`App::rebind_session`]). Without a runtime they are fixed.
        let mut session = session;
        let mut gen_rx = runtime.as_ref().map(|r| r.watch_generation());
        // The synchronous extension-dialog sink (L4 review §2.1): a loaded guest's `ui.{confirm,input,
        // select,editor}` capability blocks its OWN tokio task on a one-shot
        // (`LiveHostServices::ui_roundtrip`) while this loop's `ui_rx` arm renders the matching dialog
        // and replies once the user answers — the interactive-TUI mirror of `crates/cyrup-modes/src/
        // rpc.rs`'s `run_rpc`, which wires the SAME `UiSink` mechanism for RPC mode. Installed here
        // (only when a TUI is present — `App::run` is never invoked headless) and re-installed on every
        // session swap below, since a replacement session brings a fresh `LiveHostServices`.
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        // The FIRE-AND-FORGET sibling of `ui_tx` (TUI-S01). `LiveHostServices::emit_ui_effect` drops
        // every `ui.{notify,set-status,set-widget,set-header,set-footer,set-title,set-editor-text,
        // paste-editor-text,set-tools-expanded}` call when this sink is unset, which is exactly Pi's
        // headless `noOpUIContext` policy (`extensions/runner.ts:230-265`) — but interactive is NOT
        // headless in Pi: it passes a real `uiContext` (`interactive-mode.ts:2223-2268`). Cyrup's RPC
        // mode already installs this (`crates/cyrup-modes/src/rpc.rs`'s `run_rpc`); without the same
        // install here every fire-and-forget extension UI call vanished in the DEFAULT mode. Also
        // re-installed on session swap below, for the same reason `ui_tx` is.
        let (ui_effect_tx, mut ui_effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
        // The THIRD extension seam (TUI-S02): the contained-fault listener Pi's interactive mode
        // passes as `bindExtensions({ … onError })` (`interactive-mode.ts:1700-1701`). Every guest
        // handler fault the dispatcher contains + skips — or contains and turns into a BLOCK — is
        // reported here and drawn into the transcript by the `ext_error_rx` arm below
        // (`show_extension_error`). RPC mode has had this since `run_rpc` was written; interactive
        // had nothing, so `Dispatcher::report` degraded to a `tracing::warn!` and the fault was
        // invisible in the DEFAULT mode. Re-installed on session swap below, for the same reason
        // `ui_tx` is: a replacement session brings a fresh `ExtensionHost`.
        let (ext_error_tx, mut ext_error_rx) =
            tokio::sync::mpsc::unbounded_channel::<cyrup_ext::ExtensionError>();
        // The FOURTH extension seam: an interactive modal an extension owns the state of and this
        // loop owns the terminal for (Pi `ctx.ui.custom(factory, { overlay: true, … })`,
        // `interactive-mode.ts:2719`). `LiveHostServices::open_overlay` blocks the extension's OWN
        // (always spawned) task on a one-shot while this loop pushes the component onto the
        // `state.overlays` z-stack, routes every keystroke to it through the existing
        // `handle_overlay_key` chain, and ticks it at its own cadence. Dropping the overlay — a
        // `Close` outcome, a session swap, a quit — fires the one-shot and releases that task.
        // Re-installed on session swap below, for the same reason `ui_tx` is.
        let (overlay_tx, mut overlay_rx) =
            tokio::sync::mpsc::unbounded_channel::<cyrup_session_svc::OverlayRequest>();
        Self::install_ui_sinks(
            &session.services().host_services,
            ui_tx.clone(),
            ui_effect_tx.clone(),
        );
        Self::install_overlay_sink(&session.services().host_services, overlay_tx.clone());
        // The open overlay's self-refresh timer, armed from its own `refresh_ms` when it arrives and
        // dropped when the stack empties. `None` means "no ticking overlay is open", which the
        // `select!` arm below expresses as a `pending()` future rather than a spinning interval.
        let mut overlay_tick: Option<tokio::time::Interval> = None;
        Self::install_error_listener(&session.services().ext_host, ext_error_tx.clone());
        // The `/` menu's dynamic half (pi `interactive-mode.ts:1240-1300`). `slash_command_catalog()`
        // already merges registered extension commands, prompt templates and skills — it was just
        // never consumed outside RPC mode, so the interactive `/` list showed builtins only while an
        // RPC client saw everything from the SAME session. Re-installed on session swap below, for
        // the same reason the sinks are: a replacement session brings different extensions.
        // …gated by `enableSkillCommands`, which Pi applies at exactly this seam
        // (`interactive-mode.ts:613`) and nowhere else.
        self.state
            .editor
            .set_registry(crate::commands::CommandRegistry::with_dynamic(
                crate::commands::dynamic_commands_from_catalog_gated(
                    &session.slash_command_catalog(),
                    session.services().settings.effective().enable_skill_commands(),
                ),
            ));
        // `editorPaddingX` + `showHardwareCursor` — Pi seeds both while CONSTRUCTING the editor and
        // the TUI (`interactive-mode.ts:459` `new TUI(terminal, getShowHardwareCursor(), …)` and
        // `:470-474` `new CustomEditor(…, { paddingX: getEditorPaddingX(), … })`), so the very first
        // frame must already honour them. Re-applied on `/settings` cycle and on session swap below.
        {
            let eff = session.services().settings.effective();
            self.state.editor.set_padding_x(eff.editor_padding_x());
            self.state
                .editor
                .set_show_hardware_cursor(
                    eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::from_process()),
                );
        }
        // Honor the persisted `outputPad` at boot (Pi seeds `this.outputPad = getOutputPad()`,
        // interactive-mode.ts:440): the transcript defaults to Pi's `1`, but a configured `0` must take
        // effect on the first frame. Re-read after each session swap below (a swap resets the transcript).
        self.state
            .transcript
            .set_output_pad(session.services().settings.effective().output_pad().max(0) as usize);
        // Same for `hideThinkingBlock` (Pi seeds `this.hideThinkingBlock = getHideThinkingBlock()`
        // before constructing any `AssistantMessageComponent`): the very first reasoning block must
        // already honour the persisted setting.
        self.state
            .transcript
            .set_hide_thinking_block(session.services().settings.effective().hide_thinking_block());
        // `terminal.showImages` / `terminal.imageWidthCells` govern how a tool result's `image`
        // content blocks render (TUI-007) — seed both before the first frame.
        let eff = session.services().settings.effective();
        self.state.show_images = eff.show_images();
        self.state.transcript.set_show_images(self.state.show_images);
        self.state
            .transcript
            .set_image_width_cells(eff.image_width_cells().clamp(1, u16::MAX as i64) as u16);
        // `terminal.showTerminalProgress` — the gate on the OSC 9;4 taskbar indicator. Pi re-reads
        // it at each of its five call sites (`interactive-mode.ts:2865`/`:3057`/`:3076`/`:3090`/
        // `:6041`); cyrup caches it here and re-seeds it on a `/settings` flip and on a session swap,
        // which is the same liveness. Seeding only — never arms, since Pi arms only from an
        // `agent_start`/`compaction_start`.
        self.state.terminal_progress = crate::TerminalProgress::with_enabled(
            eff.show_terminal_progress(),
        );
        // The automatic window title (Pi `updateTerminalTitle`, interactive-mode.ts:818-826, called
        // at `:860` right after `init()`): `cyrup - <session name> - <cwd basename>`. Both inputs are
        // read from the LIVE session here — the name Pi reads via `sessionManager.getSessionName()`
        // and the cwd via `getCwd()` (the runtime's, which a `/resume` of a session recorded
        // elsewhere moves; the process cwd is the fallback seeded in `AppState::new`). Refreshed on
        // `session_info_changed` (`ingest_event`) and on every session swap (the `session_swapped`
        // arm below), which is exactly Pi's `:2901` / `:1761` call sites.
        // X7(b) — through [`Self::set_title_cwd`], NOT a bare `state.title_cwd = …`. That funnel is
        // what also lands the value on the transcript as Pi's `ToolRenderContext.cwd`
        // (`tool-execution.ts:126`), which `read`'s compact classification resolves its path against
        // (`read.ts:336`, `resolveToCwd(rawPath, cwd)`). Assigning the field directly left
        // `transcript.cwd()` at `None`, so the classification silently fell back to the PROCESS cwd
        // — which is exactly what the paragraph above says can differ after a `/resume`.
        if let Some(rt) = runtime.as_ref() {
            self.set_title_cwd(rt.cwd().to_path_buf());
        }
        self.state.status.set_session_name(session.session_name().await);
        if let Some(title) = self.update_terminal_title() {
            write_terminal_title(&title);
        }
        // The footer's ` (sub)` marker (`footer.ts:138-145`). pi answers it per repaint from
        // `modelRuntime.snapshot.auth`, which the runtime has already loaded by the time the first
        // frame draws; cyrup reads `auth.json` once, here, so the very FIRST frame of a session
        // started with a stored Pro/Max credential already shows the marker. Refreshed again on
        // every credential change (`finish_login`, the `/logout` arm) and on session swap below.
        self.refresh_auth_snapshot(&session).await;
        // …and, for the same reason, the context segment: upstream's `render()` reads
        // `getContextUsage()` per frame (`footer.ts:108`), so a `/resume`d session shows its
        // occupancy on the very first frame rather than only after the next assistant message.
        self.refresh_context_usage(&session).await;
        self.draw_synchronized()?;
        // The spinner tick (spec/tui/01 §6.2 / §10): an 80 ms redraw used **only while** a status
        // indicator is active, so the Braille frame advances without a timer thread and an idle
        // session never busy-loops (the branch is `if`-gated on `indicator.is_active()`).
        let mut spinner = tokio::time::interval(SPINNER_INTERVAL);
        spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The extension-UI dialog countdown tick (Pi's `CountdownTimer`, `countdown-timer.ts:21-30`):
        // a 1s redraw used **only while** an open `ui.{confirm,select,input}` dialog has a
        // guest-set `opts.timeout_ms` armed, so an idle session (or a dialog with no timeout) never
        // pays for it — mirrors the spinner's own `if`-gated pattern immediately above.
        let mut dialog_countdown = tokio::time::interval(Duration::from_secs(1));
        dialog_countdown.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The OSC 9;4 keepalive (Pi's `setInterval(..., TERMINAL_PROGRESS_KEEPALIVE_MS)`,
        // `tui/src/terminal.ts:514-516`): re-send the active sequence once a second for as long as a
        // turn or a compaction is running, because several terminals expire an indeterminate
        // progress state that is not refreshed. Same `if`-gated shape as the spinner above, so an
        // idle session — or any session with the setting off — never writes.
        let mut progress_keepalive = tokio::time::interval(crate::TERMINAL_PROGRESS_KEEPALIVE);
        progress_keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The footer's git-branch refresh (Pi watches `.git/HEAD` with `fs.watch` + a 500 ms debounce,
        // `footer-data-provider.ts`). cyrup polls the same 500 ms instead of holding an inotify
        // watch, and the branch is `if`-gated on actually being inside a repo — outside one this arm
        // never runs at all, and inside one a tick costs a `stat` and repaints only on a real change.
        let mut git_branch_poll = tokio::time::interval(crate::footer_data::POLL_INTERVAL);
        git_branch_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The running-`bash` elapsed tick — Pi's `setInterval(() => context.invalidate(), 1000)`,
        // armed by bash's own `renderResult` while its result is still partial and cleared on the
        // final one (bash.ts:471-479). Without it the `Elapsed …` figure would only advance when
        // some OTHER event happened to redraw. Same `if`-gated shape as the spinner above: an idle
        // session, and any turn not running a bash call, never ticks.
        let mut elapsed_tick = tokio::time::interval(ELAPSED_TICK_INTERVAL);
        elapsed_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // A live `!`/`!!` bash run: the receiver its deltas + terminal result arrive on. Kept as a
        // run-loop local (not on `self`) so the `select!` borrow does not collide with the
        // input-arm `&mut self`. X13 — cancellation is NOT a local token any more: the run goes
        // through `session.execute_bash*`, which owns the child's token (`_bashAbortController`,
        // agent-session.ts:2660), so `Esc` is `session.abort_bash()` — Pi's `abortBash()`.
        let mut bash_rx: Option<tokio::sync::mpsc::UnboundedReceiver<BashMsg>> = None;
        // A fired extension shortcut is spawned onto its own tokio task (see the
        // `AppAction::ExtensionShortcut` arm below for why); this channel carries its status/error
        // line back to the transcript once it settles, mirroring the `bash_rx` pattern above.
        let (shortcut_status_tx, mut shortcut_status_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        // The tmux keyboard-setup diagnostic (Pi `checkTmuxKeyboardSetup`, interactive-mode.ts:940-988,
        // wired at `:865-869`). Spawned, never awaited: Pi starts it alongside the version/package
        // checks and shows the warning whenever it settles, so a wedged `tmux show` (bounded at 2 s)
        // delays no frame. The sender is kept alive HERE, as a run-loop local, for the same reason
        // `shortcut_status_tx` is: a closed channel would make its `select!` arm's `Some(..)` pattern
        // fail on every iteration.
        let (tmux_warning_tx, mut tmux_warning_rx) =
            tokio::sync::mpsc::unbounded_channel::<&'static str>();
        {
            let tx = tmux_warning_tx.clone();
            tokio::spawn(async move {
                if let Some(warning) = crate::tmux::check_keyboard_setup().await {
                    let _ = tx.send(warning);
                }
            });
        }
        // A `/tree` navigation runs on its OWN task (see `App::begin_tree_navigation`) and posts its
        // outcome back here, so a branch summarization's provider round-trip never blocks this loop
        // — the same channel-back shape as `bash_rx` / `shortcut_status_rx`. Installing the sender is
        // what makes the spawned path (and therefore Escape→abort and the live
        // `IndicatorKind::BranchSummary` spinner) reachable at all.
        let mut tree_nav_rx = self.install_tree_nav_channel();
        // The `/login` channel (`login_dialog::LoginUiMsg`): the spawned flow's prompts, progress
        // events and final outcome. Installed for the same reason `tree_nav_rx` is — the flow must
        // not run on this task, or no keystroke could ever answer its prompts.
        let mut login_rx = self.install_login_channel();
        // The startup package-update check's answer channel, moved out of `self` so the `select!`
        // arm's borrow does not collide with the `&mut self` the other arms take — the same
        // run-loop-local shape as `bash_rx` / `tree_nav_rx`. `None` when the binary wired no channel
        // (offline / `--offline` / `CYRUP_SKIP_VERSION_CHECK`), in which case the arm never resolves.
        let mut package_update_rx = self.package_update_rx.take();
        loop {
            let theme_changed = async {
                match theme_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            // The open overlay's own cadence (Pi arms it inside the component's constructor —
            // `pi-subagents/src/tui/fleet.ts:516-521` `setInterval(… , options.refreshMs ?? 750)`).
            // No ticking overlay ⇒ a future that never resolves, so the arm costs nothing when the
            // z-stack is empty or the open modal is static.
            let overlay_ticked = async {
                match overlay_tick.as_mut() {
                    Some(t) => {
                        t.tick().await;
                    }
                    None => std::future::pending().await,
                }
            };
            let bash_next = async {
                match bash_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            // Resolve to `true` when the runtime swaps the active session (generation bump). When no
            // runtime is threaded in, never resolves (single fixed session).
            let package_updates = async {
                match package_update_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            let session_swapped = async {
                match gen_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => break,
                // The live `!`/`!!` block owns a `Loader` of its own (`bash-execution.ts:55-61`)
                // with its own `setInterval` (`loader.ts:77-80`), so its spinner animates whether or
                // not a turn is streaming — hence the second condition (X4).
                _ = spinner.tick(),
                    if self.state.indicator.is_active()
                        || self.state.transcript.bash_running() =>
                {
                    self.draw_synchronized()?;
                }
                _ = dialog_countdown.tick(),
                    if self.state.pending_ui_reply.as_ref().is_some_and(|p| p.deadline.is_some()) =>
                {
                    self.tick_extension_dialog_countdown();
                    self.draw_synchronized()?;
                }
                _ = progress_keepalive.tick(), if self.state.terminal_progress.keepalive() => {
                    // Pure terminal output, no UI state — Pi's interval writes the escape and
                    // nothing else, so this arm deliberately does NOT redraw.
                    self.tick_terminal_progress_keepalive();
                }
                _ = elapsed_tick.tick(), if self.state.transcript.has_running_elapsed_tool() => {
                    // Pi's `context.invalidate()` → `ui.requestRender()`: nothing to mutate, the
                    // `Elapsed` figure is computed from `started_at` at render time.
                    self.draw_synchronized()?;
                }
                _ = git_branch_poll.tick(), if self.state.git_branch.in_repo() => {
                    // Pi repaints only when the branch actually CHANGED (`notifyBranchChange` fires
                    // inside `if (this.cachedBranch !== nextBranch)`); an unchanged `stat` draws
                    // nothing.
                    if self.poll_footer_git_branch() {
                        self.draw_synchronized()?;
                    }
                }
                maybe_in = input.next() => {
                    let Some(ev) = maybe_in else { break };
                    match self.handle_input(&ev) {
                        AppAction::Quit => break,
                        AppAction::Suspend => self.suspend()?,
                        AppAction::OpenExternalEditor => {
                            let editor_cmd = resolve_external_editor(&session);
                            self.open_external_editor(&editor_cmd)?;
                        }
                        AppAction::OpenExternalEditorForSelector => {
                            let editor_cmd = resolve_external_editor(&session);
                            self.open_external_editor_for_selector(&editor_cmd)?;
                        }
                        AppAction::Interrupt => {
                            session.abort();
                            // Also kill a running bash child (the block was already marked cancelled
                            // in `apply_action`); the reader task's terminal `Done` clears `bash_rx`.
                            session.abort_bash();
                        }
                        AppAction::InterruptRestoreQueued => {
                            // Pi `onEscape` while streaming (interactive-mode.ts:2636-2637):
                            // `restoreQueuedMessagesToEditor({abort: true})` — take-all BOTH queues,
                            // put their text back in the editor, and only then abort. Without the
                            // restore, an Esc during a turn silently discards every steering /
                            // follow-up message the user typed while it ran.
                            let (steering, follow_up) = session.drain_queue().await;
                            let queued: Vec<String> =
                                steering.into_iter().chain(follow_up).collect();
                            self.restore_queued_to_editor(&queued);
                            session.abort();
                            session.abort_bash();
                        }
                        AppAction::AbortBranchSummary => {
                            // Pi `:4793` — cancel the summarization only. The spawned navigation
                            // resolves with `{cancelled: true, aborted: true}`, and the `tree_nav_rx`
                            // arm re-shows the tree; the indicator/Escape rebind are torn down there.
                            session.abort_branch_summary();
                        }
                        AppAction::RunBash { command, excluded } => {
                            // Replace any prior job (Pi keeps one `bashComponent`; a second `!` while
                            // the first still runs supersedes it).
                            session.abort_bash();
                            bash_rx = Some(spawn_session_bash(session.clone(), command, excluded));
                        }
                        AppAction::Submit(text) => {
                            // Spawned, not awaited inline (L4 review §2.1 — the SAME deadlock reason
                            // as `ExtensionShortcut` below): `prompt_accepted`/`steer` run Pi's
                            // pre-send extension-command dispatch + `input`-hook fan-out INLINE, before
                            // the run itself is spawned (`session.rs` `prepare` →
                            // `try_execute_extension_command` / `emit_input_event`), and either can
                            // call a guest's synchronous `ui.*` capability — this is in fact the MOST
                            // common guest-reentrant path (an extension's own `/command` handler, or an
                            // `on_input` hook, prompting for confirmation). This arm never touches
                            // `self.state` — the optimistic transcript echo already happened
                            // synchronously in `dispatch_submission` — so no channel-back is needed.
                            let session = session.clone();
                            tokio::spawn(async move {
                                let ui = UserInput::text(text, InputSource::Tui);
                                if session.is_streaming().await {
                                    let _ = session.steer(ui).await;
                                } else {
                                    let _ = session.prompt_accepted(ui).await;
                                }
                            });
                        }
                        AppAction::FollowUp(text) => {
                            // Pi `handleFollowUp` (interactive-mode.ts:3554-3585): while a turn is
                            // streaming, queue the text as a follow-up (delivered once the agent goes
                            // idle — a SEPARATE queue from `steer`); when idle, Alt+Enter behaves like a
                            // plain Enter submit. The editor is cleared here (Pi's `setText("")` in both
                            // branches) since `apply_action` deferred the mutation until this async
                            // streaming check. Spawned, not awaited, for the same guest-reentrancy reason
                            // as `Submit`.
                            self.state.editor.clear();
                            let streaming = session.is_streaming().await;
                            if !streaming {
                                // Idle → behaves like a normal prompt: echo optimistically before send.
                                self.state.transcript.push_user(text.clone());
                            }
                            let session = session.clone();
                            tokio::spawn(async move {
                                let ui = UserInput::text(text, InputSource::Tui);
                                if streaming {
                                    let _ = session.follow_up(ui).await;
                                } else {
                                    let _ = session.prompt_accepted(ui).await;
                                }
                            });
                        }
                        AppAction::Dequeue => {
                            // Pi `handleDequeue` → `restoreQueuedMessagesToEditor`
                            // (interactive-mode.ts:3587-3594,3852-3871): drain BOTH the steering and
                            // follow-up queues (steering first, then follow-up — Pi's
                            // `[...steering, ...followUp]` order), join their text by blank lines, and
                            // prepend it to the current editor buffer. When nothing is queued, show
                            // Pi's exact `No queued messages to restore` status and leave the editor
                            // untouched.
                            // One atomic take-all (Pi's `clearAllQueues()` returns what it drained),
                            // not a read-then-clear pair — the split form loses any message queued
                            // between the two calls.
                            let (steering, follow_up) = session.drain_queue().await;
                            let queued: Vec<String> =
                                steering.into_iter().chain(follow_up).collect();
                            match self.restore_queued_to_editor(&queued) {
                                0 => self
                                    .state
                                    .transcript
                                    .push_status("No queued messages to restore"),
                                n => self.state.transcript.push_status(format!(
                                    "Restored {n} queued message{} to editor",
                                    if n > 1 { "s" } else { "" }
                                )),
                            }
                        }
                        AppAction::Command(cmd) => {
                            self.execute_command(cmd, &session, runtime.as_ref()).await;
                            if should_honor_extension_shutdown(&session, false) {
                                return Ok(());
                            }
                        }
                        AppAction::ExtensionShortcut(key) => {
                            // Route the fired shortcut to the owning live extension (R-08-017; Pi
                            // `registerShortcut` handler) — SPAWNED, not awaited inline (L4 review
                            // §2.1). The shortcut handler may itself call a synchronous
                            // `ui.{confirm,input,select,editor}` capability, which blocks ITS calling
                            // tokio task on `ui_roundtrip`'s one-shot reply until this very `select!`
                            // loop services `ui_rx` and answers it. Awaiting `run_shortcut` inline HERE
                            // would make that blocked task and the loop that must unblock it the SAME
                            // task — a single task's `poll()` can never reach a sibling `select!` arm
                            // while it is synchronously blocked deeper in its own call stack (tokio's
                            // `block_in_place` frees a WORKER THREAD for other tasks, not this task's
                            // own other branches) — a genuine self-deadlock. Spawning it as its own task
                            // keeps the main loop free to poll `ui_rx` concurrently, exactly why
                            // `SessionManager::spawn_run` already spawns agent-turn tool execution
                            // (session.rs `drive_run`) instead of awaiting it inline. A guest fault
                            // (or, now, a spawn-side error) is surfaced as a status block via
                            // `shortcut_status_tx`, never a panic; the run loop keeps going regardless.
                            let ext_host = session.services().ext_host.clone();
                            let shortcut_cancel = cancel.clone();
                            let status_tx = shortcut_status_tx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = ext_host.run_shortcut(&key, &shortcut_cancel).await {
                                    let _ = status_tx.send(format!("shortcut {key}: {e}"));
                                }
                            });
                        }
                        AppAction::Redraw | AppAction::None => {}
                    }
                    self.draw_synchronized()?;
                }
                Some(msg) = bash_next => {
                    match msg {
                        BashMsg::Chunk(chunk) => self.state.transcript.bash_append(&chunk),
                        BashMsg::Done { exit_code, cancelled, truncated, full_output_path } => {
                            // X13 — Pi's completion arm verbatim (`interactive-mode.ts:6347-6353`):
                            //   this.bashComponent.setComplete(
                            //       result.exitCode, result.cancelled,
                            //       result.truncated ? {truncated:true, content:result.output} : undefined,
                            //       result.fullOutputPath);
                            // All FOUR fields, so `Output truncated. Full output: …`
                            // (`bash-execution.ts:195-199`) is reachable in a LIVE session and not
                            // only on replay. Recording into the session is NOT done here: it is
                            // `executeBash`'s own `recordBashResult` (agent-session.ts:2628-2643),
                            // which `AgentSession::execute_bash` already performs — with the
                            // `truncated`/`fullOutputPath` fields intact, which is what puts the
                            // warning back on the block after a `/resume`.
                            self.state.transcript.bash_complete(
                                exit_code,
                                cancelled,
                                truncated,
                                full_output_path,
                            );
                            self.state.transcript.commit_bash();
                            bash_rx = None;
                        }
                    }
                    self.draw_synchronized()?;
                }
                () = overlay_ticked, if !self.state.overlays.is_empty() => {
                    // Pi's `setInterval(() => { this.invalidate(); this.tui.requestRender(); })`
                    // (`fleet.ts:516-520`): let the component re-collect, and repaint only when it
                    // says the frame actually changed — the same "no-op edge costs no draw" rule the
                    // git-branch poll arm above follows.
                    let mut changed = false;
                    for overlay in self.state.overlays.iter_mut() {
                        changed |= overlay.tick();
                    }
                    if changed {
                        self.draw_synchronized()?;
                    }
                }
                Some(req) = overlay_rx.recv() => {
                    // An extension handed over an interactive modal (Pi `ctx.ui.custom(factory,
                    // { overlay: true, … })`). Its calling task is BLOCKED on the one-shot inside
                    // `req.done` until the `ExtensionOverlay` we build here is dropped, which
                    // happens on `Close` (`handle_overlay_key` pops it), on a session swap
                    // (`rebind_session` clears the stack) or on quit.
                    let cyrup_session_svc::OverlayRequest { overlay, done } = req;
                    let adapter = ExtensionOverlay::new(overlay, done);
                    // Arm the shared tick at THIS overlay's cadence before pushing, so the very
                    // first refresh lands one interval from now rather than immediately.
                    let refresh_ms = Overlay::refresh_ms(&adapter);
                    overlay_tick = (refresh_ms > 0).then(|| {
                        let period = std::time::Duration::from_millis(refresh_ms);
                        // `interval_at`, not `interval`: the latter's first tick resolves
                        // IMMEDIATELY, which would re-collect and repaint the frame we are about to
                        // draw below for no reason.
                        let mut interval =
                            tokio::time::interval_at(tokio::time::Instant::now() + period, period);
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        interval
                    });
                    self.state.overlays.push(Box::new(adapter));
                    self.draw_synchronized()?;
                }
                Some(req) = ui_rx.recv() => {
                    // A loaded guest opened a `ui.*` dialog (L4 review §2.1). EVERY kind, including
                    // `editor` (L4 review §3 — an INLINE dialog is now the default, matching Pi's
                    // `ExtensionEditorComponent`; `$VISUAL`/`$EDITOR` is reachable only via the
                    // dialog's own `Ctrl+G`, `AppAction::OpenExternalEditorForSelector`, above), opens
                    // the matching input-slot selector via `open_extension_dialog` and waits for a
                    // future key event to confirm/cancel it (`AppState::pending_ui_reply`).
                    self.open_extension_dialog(req);
                    self.draw_synchronized()?;
                }
                Some(effect) = ui_effect_rx.recv() => {
                    // The fire-and-forget counterpart of the `ui_rx` arm above: a loaded guest pushed
                    // a `ui.*` mutator and did NOT block on a reply, so there is nothing to answer —
                    // just apply it and redraw (Pi's mutators end in `this.ui.requestRender()`).
                    if let UiEffect::SetTitle { title } = &effect {
                        // Pi `setTitle` reaches the terminal, not a component
                        // (`interactive-mode.ts:2238` → `terminal.ts:504-507`), so it is written here
                        // on the crossterm path rather than inside the backend-generic
                        // `apply_ui_effect`.
                        write_terminal_title(title);
                    }
                    self.apply_ui_effect(effect);
                    self.draw_synchronized()?;
                }
                Some(err) = ext_error_rx.recv() => {
                    // A guest handler faulted and the dispatcher CONTAINED it (R-08-036). Pi shows
                    // it: `onError: (error) => this.showExtensionError(...)`
                    // (`interactive-mode.ts:1700-1701`). Without this arm the fault reached only
                    // `tracing`, so a broken extension silently ate its hook — or silently denied a
                    // tool — with nothing on screen (TUI-S02).
                    self.show_extension_error(&err);
                    self.draw_synchronized()?;
                }
                Some(msg) = shortcut_status_rx.recv() => {
                    self.state.transcript.push_status(msg);
                    self.draw_synchronized()?;
                }
                maybe_updates = package_updates => {
                    // Pi `:851-855` — `if (updates.length > 0) this.showPackageUpdateNotification(updates)`.
                    // The producer only ever sends a non-empty list and then drops its sender, so the
                    // receiver is retired here and the arm goes permanently pending: exactly one
                    // notification per session, as upstream's single `.then()` gives.
                    package_update_rx = None;
                    if let Some(packages) = maybe_updates {
                        self.state.transcript.push_package_updates(&packages);
                        self.draw_synchronized()?;
                    }
                }
                Some(warning) = tmux_warning_rx.recv() => {
                    // Pi `:866-868` — `showWarning`, whose copy is `Warning: {message}`
                    // (`interactive-mode.ts:3885-3889`), the same framing the extension `notify`
                    // path uses in `apply_ui_effect`.
                    self.state.transcript.push_warning(format!("Warning: {warning}"));
                    self.draw_synchronized()?;
                }
                Some(msg) = login_rx.recv() => {
                    // The spawned `/login` flow wants something: a prompt rendered, progress shown,
                    // or the whole login settled (Pi's `prompt`/`notify` callbacks +
                    // the `try`/`catch` around `loginProvider`, `interactive-mode.ts:5367-5374`,
                    // `:5285-5296`). Answers travel back over the one-shot the message carried.
                    self.apply_login_msg(msg);
                    self.draw_synchronized()?;
                }
                Some(msg) = tree_nav_rx.recv() => {
                    // A spawned `/tree` navigation settled (Pi `interactive-mode.ts:4805-4820`). An
                    // ABORTED summarization asks for the tree to be re-shown at the same entry, which
                    // needs the session (`session_dag`), so it comes back as a follow-up command.
                    if let Some(cmd) = self.apply_tree_nav_outcome(msg) {
                        self.execute_command(cmd, &session, runtime.as_ref()).await;
                    }
                    self.draw_synchronized()?;
                }
                maybe_ev = events.next() => {
                    let Some(ev) = maybe_ev else { continue };
                    // EXT-006: fold through the extension-aware path so a registered renderer
                    // actually draws the block (a custom message / a tool row). No renderer for the
                    // event's key ⇒ a sync pre-check short-circuits and this is the old behavior.
                    // `ingest_session_event` adds the footer's context-usage refresh, which needs the
                    // session this arm holds (`footer.ts:108`).
                    self.ingest_session_event(&ev, &session).await;
                    // A rename recomputed the window title inside `ingest_event`; the OSC 0 write is
                    // this loop's (Pi `session_info_changed` → `updateTerminalTitle`, `:2900-2903`).
                    // Gated on the event kind so no other event pays for a title recomputation.
                    if matches!(ev, AgentSessionEvent::SessionInfoChanged { .. })
                        && let Some(title) = self.state.terminal_title.clone()
                    {
                        write_terminal_title(&title);
                    }
                    // SEAM-005 / EXT-005: a guest's `ctx.shutdown()` is honored at the settle point
                    // (Pi interactive-mode.ts:3137-3138 `case "agent_settled": await
                    // this.checkShutdownRequested()`), and only there — `agent_end` cannot tell us
                    // whether a retry or a queued continuation is still coming.
                    if should_honor_extension_shutdown(
                        &session,
                        matches!(ev, AgentSessionEvent::AgentSettled),
                    ) {
                        return Ok(());
                    }
                    self.draw_synchronized()?;
                }
                ok = theme_changed => {
                    if ok && let Some(rx) = theme_rx.as_ref() {
                        let data = rx.borrow().clone();
                        self.set_theme(UiTheme::from_theme_data(&data, 0));
                        self.draw_synchronized()?;
                    }
                }
                swapped = session_swapped => {
                    // A runtime replacement (a `/new`/`/resume`/`/fork`/`/reload`/`/import` op, or a
                    // runtime-side `SessionReplaced`, R-11-021) installed a new active session: drop
                    // the stale subscription, subscribe the NEW session's event stream, and re-bind
                    // the UI. Honors a runtime-driven swap identically to a UI-driven one.
                    if swapped && let Some(rt) = runtime.as_ref() {
                        let new_session = rt.session().await;
                        events = new_session.subscribe();
                        session = new_session;
                        self.rebind_session();
                        // Pi re-titles the window from the newly bound session (`bindSession` →
                        // `updateTerminalTitle`, interactive-mode.ts:1761): a `/new`, `/resume` or
                        // `/fork` almost always changes the name, and a swap must never leave the
                        // previous session's name in the tab. The cwd is the runtime's factory base
                        // and does not move with the swap, so only the name is re-read.
                        self.state.status.set_session_name(session.session_name().await);
                        if let Some(title) = self.update_terminal_title() {
                            write_terminal_title(&title);
                        }
                        // The replacement session brings its own `AuthStore` (a `/resume` of a
                        // session recorded under a different agent dir reads a different
                        // `auth.json`), so the cached snapshot the ` (sub)` marker answers from is
                        // re-read here for the same reason the ui sinks are re-installed.
                        self.refresh_auth_snapshot(&session).await;
                        // …and the context segment, which is a property of the NEW branch's entries
                        // and its model's window (`footer.ts:108-111`).
                        self.refresh_context_usage(&session).await;
                        // The swapped-in session owns a fresh `LiveHostServices`; re-install the ui
                        // sink so a post-swap guest dialog still reaches this loop (L4 review §2.1,
                        // same re-install this run loop's `AppAction::Command` rebind mirrors from
                        // `crates/cyrup-modes/src/rpc.rs`'s `run_rpc`).
                        Self::install_ui_sinks(
                            &session.services().host_services,
                            ui_tx.clone(),
                            ui_effect_tx.clone(),
                        );
                        Self::install_overlay_sink(
                            &session.services().host_services,
                            overlay_tx.clone(),
                        );
                        // ...and the fault listener, whose `ExtensionHost` is likewise brand new on
                        // the swapped-in session (Pi re-binds `onError` from `rebindSession`, and
                        // `crates/cyrup-modes/src/rpc.rs`'s `rebind_session` does the same).
                        Self::install_error_listener(
                            &session.services().ext_host,
                            ext_error_tx.clone(),
                        );
                        // ...and the same for the `/` menu: a replacement session can load a
                        // DIFFERENT extension set (`/reload` exists precisely to change it), so a
                        // registry built from the previous session's catalog would be stale.
                        self.state.editor.set_registry(
                            crate::commands::CommandRegistry::with_dynamic(
                                crate::commands::dynamic_commands_from_catalog_gated(
                                    &session.slash_command_catalog(),
                                    session
                                        .services()
                                        .settings
                                        .effective()
                                        .enable_skill_commands(),
                                ),
                            ),
                        );
                        // `rebind_session` reset the transcript to Pi's default pad; re-read the
                        // swapped-in session's `outputPad` so a configured value survives the swap.
                        self.state.transcript.set_output_pad(
                            session.services().settings.effective().output_pad().max(0) as usize,
                        );
                        self.state.transcript.set_hide_thinking_block(
                            session.services().settings.effective().hide_thinking_block(),
                        );
                        let eff = session.services().settings.effective();
                        self.state.show_images = eff.show_images();
                        self.state.transcript.set_show_images(self.state.show_images);
                        // Re-read the progress gate for the swapped-in session's settings, for the
                        // same reason as the image rows beside it. Any indicator the OUTGOING
                        // session lit is dropped with its state; the swap arrives between turns.
                        self.state.terminal_progress =
                            crate::TerminalProgress::with_enabled(eff.show_terminal_progress());
                        self.state.transcript.set_image_width_cells(
                            eff.image_width_cells().clamp(1, u16::MAX as i64) as u16,
                        );
                        // `editorPaddingX` / `showHardwareCursor` are per-settings-layer, and a swap
                        // can move the project scope (`/resume` of a session recorded elsewhere), so
                        // re-apply both — Pi does exactly this from `rebindSession`
                        // (`interactive-mode.ts:1721-1732`: `ui.setShowHardwareCursor(...)` then
                        // `defaultEditor.setPaddingX(getEditorPaddingX())`).
                        self.state.editor.set_padding_x(eff.editor_padding_x());
                        self.state.editor.set_show_hardware_cursor(
                            eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::from_process()),
                        );
                                        // TUI-003: seed the view from the swapped-in session's conversation (Pi
                        // re-runs `renderInitialMessages()` after a tree/fork navigation,
                        // interactive-mode.ts:1737-1742). Without this a `/resume`, `/fork` or
                        // `/import` leaves the user staring at an empty transcript while the
                        // session file holds the whole history. `raw_context_messages` (NOT
                        // `messages()`) is Pi's `buildContextEntries()` projection: roles intact,
                        // so a compaction/branch summary, a `custom` message and a `!` run each
                        // reach their own component instead of replaying as user prose.
                        let restored = session.raw_context_messages().await;
                        if !restored.is_empty() {
                            // X11 — with extensions: the swapped-in session brings its own host,
                            // and Pi resolves `getMessageRenderer` on the replay walk too
                            // (`interactive-mode.ts:3471`).
                            let ext_host = session.services().ext_host.clone();
                            self.replay_session_with_extensions(&restored, &ext_host).await;
                        }
                        // The swapped-in session owns a fresh extension host; re-source its
                        // registered shortcut key-ids (R-08-017) so a post-swap press still routes.
                        let shortcuts = session.services().ext_host.shortcut_keys();
                        self.state.set_extension_shortcuts(shortcuts);
                        self.draw_synchronized()?;
                    }
                }
            }
            if self.state.should_quit {
                break;
            }
        }
        // pi `interactive-mode.ts:3589-3591`: drain, THEN stop. The drain MUST happen here, before
        // this function's own restore, not at the caller — `run` disables raw mode on the way out,
        // so a drain after it returns is a guaranteed no-op on the exact path it exists for, and
        // whatever is still queued (a late Kitty key-release report, or the Ctrl+D that asked for
        // this quit) has already been handed to the parent shell.
        self.drain_and_restore()
    }
}

/// A streamed message from a running `!`/`!!` bash execution (`bash-execution.ts` output pump).
#[derive(Clone, Debug)]
enum BashMsg {
    /// A sanitized stdout/stderr delta (Pi's `onChunk`, `interactive-mode.ts:6338-6343`).
    Chunk(String),
    /// The run finished — the four `setComplete(exitCode, cancelled, truncationResult,
    /// fullOutputPath)` arguments (`bash-execution.ts:98-103`, fed from `BashResult` at
    /// `interactive-mode.ts:6348-6353`).
    Done {
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
    },
}

/// Run a `!`/`!!` command through the session's own bash seam — Pi's `handleBashCommand`
/// (`interactive-mode.ts:6279-6364`), whose executor line is `await this.session.executeBash(command,
/// (chunk) => this.bashComponent.appendOutput(chunk), { excludeFromContext, operations })`
/// (`:6336-6345`) — streaming its deltas and terminal [`BashResult`] over the returned channel.
///
/// **X13.** This replaced a local `sh -c` pump that reported only an exit code, so
/// `truncated`/`fullOutputPath` were hard-coded `false, None` at the call to `setComplete` and the
/// `Output truncated. Full output: …` row (`bash-execution.ts:195-199`) could never appear in a live
/// session — only on the replay path, which reads the fields back off a persisted
/// `bashExecution` message. The seam that produces them already existed:
/// `AgentSession::execute_bash` → `run_bash` → `BashOutputBuffer` (`cyrup-session-svc/src/bash.rs`,
/// a port of `bash-executor.ts:57-124`), which spills to `cyrup-bash-<id>.log` once the raw stream
/// passes `DEFAULT_MAX_BYTES` and tail-truncates the preview. Nothing in the TUI reached it.
///
/// Routing through the session rather than spawning locally also picks up the rest of Pi's
/// `executeBash` contract that the local pump silently skipped: the `user_bash` extension event and
/// its `result` override ([`AgentSession::execute_bash_with_user_event`], Pi's per-front-end
/// `emitUserBash`, `interactive-mode.ts:6283-6288`), the `shellCommandPrefix`/`shellPath` settings,
/// the managed `bin` dir on `PATH`, ANSI/binary sanitization of every chunk, the
/// `bash_execution_update` event fan-out, and `recordBashResult` (`agent-session.ts:2628`) — which
/// is why the caller no longer appends its own `bashExecution` message.
///
/// Cancellation is the session's (`abortBash`, `agent-session.ts:2660`), not a token handed back
/// here: `execute_bash` installs its own child token and `AgentSession::abort_bash` fires it.
fn spawn_session_bash(
    session: Arc<AgentSession>,
    command: String,
    excluded: bool,
) -> tokio::sync::mpsc::UnboundedReceiver<BashMsg> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<BashMsg>();
    tokio::spawn(async move {
        let chunk_tx = tx.clone();
        // Pi's `(chunk) => { this.bashComponent.appendOutput(chunk); this.ui.requestRender(); }`
        // (`interactive-mode.ts:6338-6343`) — the redraw is the run loop's, so the sink only posts.
        let sink: cyrup_session_svc::BashChunkSink = Some(Box::new(move |delta: &str| {
            let _ = chunk_tx.send(BashMsg::Chunk(delta.to_string()));
        }));
        let options =
            cyrup_session_svc::BashOptions { exclude_from_context: excluded, id: None };
        let done = match session.execute_bash_with_user_event(&command, options, sink).await {
            Ok(result) => BashMsg::Done {
                exit_code: result.exit_code,
                cancelled: result.cancelled,
                truncated: result.truncated,
                full_output_path: result.full_output_path,
            },
            // A genuine backend failure (spawn error, missing shell, …). Pi's `catch`
            // (`interactive-mode.ts:6355-6360`) shows the message and calls
            // `setComplete(undefined, false)` — no exit code, not cancelled, no truncation report.
            Err(e) => {
                let _ = tx.send(BashMsg::Chunk(format!("Bash command failed: {e}\n")));
                BashMsg::Done {
                    exit_code: None,
                    cancelled: false,
                    truncated: false,
                    full_output_path: None,
                }
            }
        };
        let _ = tx.send(done);
    });
    rx
}

/// Write the OSC 0 window-title sequence — Pi `ProcessTerminal.setTitle`
/// (`pi/packages/tui/src/terminal.ts:504-507`, `\x1b]0;${title}\x07`).
///
/// `[CYRUP-DELTA]`: control characters are stripped first. Pi interpolates the extension-supplied
/// string verbatim, so a title containing `BEL`/`ESC` would close the OSC early and let the rest of
/// the string be interpreted as terminal commands. Stripping keeps an extension from driving the
/// terminal through a title.
pub fn write_terminal_title(title: &str) {
    use std::io::Write;
    let safe: String = title.chars().filter(|c| !c.is_control()).collect();
    let mut out = io::stdout();
    let _ = out.write_all(format!("\x1b]0;{safe}\x07").as_bytes());
    let _ = out.flush();
}

/// How long the reader thread idles between `event::poll` rounds when nothing is held.
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The much shorter poll used while [`StrayReplyFilter`] is holding events. A held opener (a bare
/// `Esc`, or `Alt+]`) is released after at most this long, so a real `Escape` press costs one
/// imperceptible tick rather than a full [`INPUT_POLL_INTERVAL`] — the standard escape-timeout
/// trade every terminal app makes to tell `ESC` from an escape *sequence*.
const HELD_FLUSH_INTERVAL: Duration = Duration::from_millis(20);

/// A terminal input stream backed by a blocking `event::read()` reader thread (the async crossterm
/// `EventStream` feature is not enabled in this build; arch-10 §5 fallback). Maps `crossterm::Event`
/// to [`InputEvent`] and forwards over an unbounded channel; stops when `cancel` fires.
///
/// Every event first passes through [`StrayReplyFilter`], the port of Pi's
/// `consumeOsc11BackgroundResponse` guard (`tui/src/tui.ts:788-794`): a terminal that answers the
/// boot-time OSC 11 probe *after* [`crate::terminal_query`]'s deadline would otherwise have its
/// reply decoded by crossterm into keystrokes and typed into the prompt. The filter only ever
/// removes a complete, terminated OSC 11 frame; anything it holds is replayed the moment the match
/// fails or the input goes idle — see that module's safety contract.
pub fn crossterm_input_stream(cancel: CancelToken) -> EventStream<InputEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    std::thread::spawn(move || {
        let mut filter = StrayReplyFilter::new();
        let mut released: Vec<Event> = Vec::new();
        'reader: while !cancel.is_cancelled() {
            let wait =
                if filter.is_holding() { HELD_FLUSH_INTERVAL } else { INPUT_POLL_INTERVAL };
            match event::poll(wait) {
                Ok(true) => match event::read() {
                    Ok(ev) => filter.push(ev, &mut released),
                    Err(_) => break,
                },
                // Idle: nothing more is coming, so release whatever the filter is holding.
                Ok(false) => filter.flush(&mut released),
                Err(_) => break,
            }
            for ev in released.drain(..) {
                if let Some(mapped) = map_event(ev)
                    && tx.send(mapped).is_err()
                {
                    break 'reader;
                }
            }
        }
    });
    Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
}

/// Map a crossterm event to our [`InputEvent`] (filtering non-press key kinds).
///
/// Key presses first go through [`rescue_native_shift_enter_live`] — upstream's
/// `ProcessTerminal.forwardInputSequence` normalization (v0.83.0 `tui/src/terminal.ts:305-312`).
/// On Apple Terminal (and, since v0.84.1, the Windows console) a bare `\r` is all the terminal
/// sends for BOTH `Enter` and `Shift+Enter`, so the modifier is recovered from the live keyboard
/// state instead of the byte stream. Everywhere else the event passes through untouched.
fn map_event(ev: Event) -> Option<InputEvent> {
    // `TERM_PROGRAM` is read only for the one key that can need it (a bare `Enter`), so no other
    // keystroke pays for a `getenv`.
    let term_program = match &ev {
        Event::Key(k)
            if k.code == ratatui::crossterm::event::KeyCode::Enter
                && k.modifiers == ratatui::crossterm::event::KeyModifiers::NONE =>
        {
            std::env::var("TERM_PROGRAM").ok()
        }
        _ => None,
    };
    map_event_on(ev, crate::native_modifiers::host_platform(), term_program.as_deref(), |k| {
        crate::native_modifiers::is_native_modifier_pressed(k)
    })
}

/// [`map_event`] with `process.platform`, `process.env.TERM_PROGRAM` and the native modifier helper
/// lifted into parameters, so the Apple-Terminal / Windows-console branch of the Shift+Enter rescue
/// is reachable from a test on any host (the same pattern as
/// [`crate::image::detect_capabilities_on_platform`]).
fn map_event_on(
    ev: Event,
    platform: &str,
    term_program: Option<&str>,
    probe: impl Fn(crate::native_modifiers::ModifierKey) -> bool,
) -> Option<InputEvent> {
    match ev {
        Event::Key(k) if !matches!(k.kind, KeyEventKind::Release) => {
            Some(InputEvent::Key(crate::native_modifiers::rescue_native_shift_enter(
                k,
                platform,
                term_program,
                probe,
            )))
        }
        Event::Key(_) => None,
        Event::Paste(s) => Some(InputEvent::Paste(s)),
        Event::Resize(w, h) => Some(InputEvent::Resize(w, h)),
        Event::FocusGained => Some(InputEvent::FocusGained),
        Event::FocusLost => Some(InputEvent::FocusLost),
        Event::Mouse(_) => None,
    }
}

/// The production input pipeline end-to-end: what [`crossterm_input_stream`]'s reader thread does
/// to a burst of raw crossterm events, i.e. [`StrayReplyFilter`] followed by [`map_event`], with the
/// idle flush at the end of the burst.
#[cfg(test)]
fn input_pipeline(raw: Vec<Event>) -> Vec<InputEvent> {
    let mut filter = StrayReplyFilter::new();
    let mut released: Vec<Event> = Vec::new();
    let mut out: Vec<InputEvent> = Vec::new();
    for ev in raw {
        filter.push(ev, &mut released);
        out.extend(released.drain(..).filter_map(map_event));
    }
    // Input has gone quiet: the reader thread's `Ok(false)` poll arm.
    filter.flush(&mut released);
    out.extend(released.drain(..).filter_map(map_event));
    out
}

/// The Shift+Enter rescue inside the real reader-thread mapping (G63; `terminal.ts:316-327`).
///
/// `map_event_on` is where the input thread actually normalizes a key event, so these drive THAT
/// with a synthesized `process.platform` / `TERM_PROGRAM` / native helper. The macOS and Windows
/// probe bodies are unimplemented and unexercised — see `tests/native_shift_enter.rs`.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod native_shift_enter_mapping_tests {
    use super::*;
    use crate::native_modifiers::ModifierKey;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    fn mapped_key(ev: Option<InputEvent>) -> KeyEvent {
        match ev {
            Some(InputEvent::Key(k)) => k,
            other => panic!("expected a key event, got {other:?}"),
        }
    }

    #[test]
    fn the_reader_rewrites_a_bare_enter_on_apple_terminal_when_shift_is_held() {
        let mapped = map_event_on(enter(), "darwin", Some("Apple_Terminal"), |k| {
            k == ModifierKey::Shift
        });
        assert_eq!(mapped_key(mapped), KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    }

    #[test]
    fn the_reader_leaves_a_plain_enter_alone_when_shift_is_up() {
        let mapped = map_event_on(enter(), "darwin", Some("Apple_Terminal"), |_| false);
        assert_eq!(mapped_key(mapped), KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    #[test]
    fn the_reader_never_probes_on_a_platform_that_encodes_modifiers() {
        let mapped =
            map_event_on(enter(), "linux", None, |_| panic!("the probe must not run on linux"));
        assert_eq!(mapped_key(mapped), KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    #[test]
    fn non_key_events_are_unaffected() {
        assert!(matches!(
            map_event_on(Event::Resize(10, 20), "darwin", Some("Apple_Terminal"), |_| true),
            Some(InputEvent::Resize(10, 20))
        ));
        assert!(
            map_event_on(
                Event::Key(KeyEvent {
                    kind: ratatui::crossterm::event::KeyEventKind::Release,
                    ..KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
                }),
                "win32",
                None,
                |_| true
            )
            .is_none(),
            "releases are still filtered out"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod stray_reply_pipeline_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn ch(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    /// A user launched cyrup and got `11;rgb:0c0c/0b0b/1313` typed into their prompt: the terminal
    /// answered the boot OSC 11 probe after `terminal_query`'s 100 ms deadline, so the reply reached
    /// the crossterm reader and was shredded into keys. Drive the exact shredded burst through the
    /// real reader-thread pipeline and then through the real editor, and assert the prompt is empty.
    #[test]
    fn a_late_osc11_reply_never_reaches_the_editor() {
        let mut raw = vec![Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT))];
        raw.extend("11;rgb:0c0c/0b0b/1313".chars().map(ch));
        // BEL (0x07) reaches crossterm's C0 arm as Ctrl+G.
        raw.push(Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)));

        let delivered = input_pipeline(raw);
        assert!(delivered.is_empty(), "no input event may survive the frame, got {delivered:?}");

        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        for ev in &delivered {
            app.handle_input(ev);
        }
        assert_eq!(app.state().editor.text(), "", "the prompt must be untouched");
    }

    /// The safety half: the same pipeline must deliver ordinary typing byte-for-byte, including the
    /// two keys the filter is allowed to hold (`Escape` and `Alt+]`).
    #[test]
    fn ordinary_typing_survives_the_pipeline_intact() {
        let mut raw: Vec<Event> = "hello 11; world".chars().map(ch).collect();
        raw.push(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        raw.push(Event::Key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)));

        let delivered = input_pipeline(raw.clone());
        assert_eq!(delivered.len(), raw.len(), "every key must be delivered: {delivered:?}");
        for (i, (got, want)) in delivered.iter().zip(raw.iter()).enumerate() {
            match (got, want) {
                (InputEvent::Key(a), Event::Key(b)) => assert_eq!(a, b, "event {i} differs"),
                other => panic!("event {i} changed shape: {other:?}"),
            }
        }

        // And it lands in the editor as the literal text the user typed.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        for ev in &delivered {
            app.handle_input(ev);
        }
        assert_eq!(app.state().editor.text(), "hello 11; world");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod clipboard_paste_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Pi's clipboard paste inserts the materialized temp-file PATH into the editor as ordinary text
    /// (`this.editor.insertTextAtCursor(filePath)`, interactive-mode.ts:2552) — NOT an inline image.
    /// This drives the exact `insert_clipboard_image_path` step the Ctrl+V handler calls, then the
    /// real Enter-submit path, then the `UserInput` the run loop builds from that text — proving the
    /// OUTGOING message the LLM receives is text carrying the path with NO image content block
    /// (`AppAction::Submit` → `UserInput::text`, app.rs:3158; Pi `userContent = [{type:text}]` +
    /// (empty) images, agent-session.ts:1117).
    #[test]
    fn clipboard_paste_inserts_path_as_text_with_no_image_block() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();

        // The path a clipboard image is materialized to (mirrors the real `cyrup-clipboard-<uuid>.png`
        // under the OS temp dir; it need not exist on disk — insertion is a pure text edit). On macOS
        // this is `/var/folders/…/T/…`, a leading-slash path — the case that must NOT be mistaken for
        // a slash command on submit.
        let path = std::env::temp_dir().join("cyrup-clipboard-0198f000-test.png");
        let path_str = path.to_string_lossy().to_string();

        app.insert_clipboard_image_path(&path);

        // Pi mechanism: the bare path is now editable text in the buffer …
        assert_eq!(app.state().editor.text(), path_str, "path must land in the editor as text");
        // … and is NOT embedded as an inline image (the former `pending_images` embed is gone), so the
        // potentially-huge raster never floods context.
        assert!(
            app.pending_images().is_empty(),
            "clipboard paste must not embed an image block into pending_images"
        );

        // Real submit path: plain Enter routes editor text → `dispatch_submission` → `AppAction::Submit`
        // (a leading-slash temp path fuzzy-matches no command, so it dispatches as a text prompt).
        let enter = InputEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let action = app.handle_input(&enter);
        assert_eq!(
            action,
            AppAction::Submit(path_str.clone()),
            "the pasted path must submit as a text prompt, not a slash command"
        );

        // The run loop turns that submitted text into the outgoing user message via `UserInput::text`
        // (app.rs:3158): the LLM receives the path AS TEXT with an EMPTY image set. `into_agent_message`
        // then yields a single text content block (Pi's `[{type:text,text}]` + no images).
        let outgoing = UserInput::text(path_str.clone(), InputSource::Tui);
        assert_eq!(outgoing.text, path_str, "outgoing message text is the pasted path");
        assert!(
            outgoing.images.is_empty(),
            "outgoing user message must carry no image content block"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod ctrl_c_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn ctrl_c() -> InputEvent {
        InputEvent::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
    }

    /// F10 — Pi `handleCtrlC` (interactive-mode.ts:3361-3369): a second Ctrl+C within 500 ms exits,
    /// with NO emptiness gate (the first press clears the editor and records the time even when the
    /// buffer is non-empty; only the timing — not emptiness — gates the exit).
    #[test]
    fn double_ctrl_c_within_500ms_exits_regardless_of_editor_contents() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        // A NON-empty editor: the first Ctrl+C still only clears (no exit) — disproving the old
        // "exit when already empty" mis-statement by showing the inverse: contents don't force a clear
        // vs exit choice, timing does.
        app.editor_mut().set_text("some draft text");
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Redraw, "first Ctrl+C clears, never exits");
        assert_eq!(app.state().editor.text(), "", "first Ctrl+C cleared the buffer");
        assert!(!app.state().should_quit, "one press must not exit");
        // Immediate second press (well within 500 ms) → exit.
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Quit, "second Ctrl+C within 500 ms exits");
        assert!(app.state().should_quit, "double-tap sets the quit flag");
    }

    /// A lone Ctrl+C on an EMPTY editor does NOT exit (there is no emptiness gate), and a press that
    /// lands MORE than 500 ms after the previous one re-clears + re-arms rather than exiting.
    #[test]
    fn single_or_stale_ctrl_c_does_not_exit() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        // Empty editor, single press → clear/record, NOT exit (the mis-stated rule would have exited).
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Redraw, "empty single Ctrl+C must not exit");
        assert!(!app.state().should_quit);
        // Age the recorded press beyond the 500 ms window; the next press is a fresh first tap.
        app.state_mut().last_sigint =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(600));
        assert_eq!(app.handle_input(&ctrl_c()), AppAction::Redraw, "a >500 ms-later Ctrl+C re-arms");
        assert!(!app.state().should_quit, "outside the window is not a double-tap");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod live_floor_tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// The FLICKER fix's height logic (a unit guard — the definitive check is the pty drive): while a
    /// turn is active the live-region height uses a grow-only floor so it does NOT track per-tool
    /// content churn (which is what forced a `resize_viewport`/`reanchor_inline` reconstruction — the
    /// flicker source — on essentially every tool event). The instant the turn goes idle the floor
    /// resets so the region collapses back to the compact editor/footer (void-fix).
    #[test]
    fn live_floor_grows_then_holds_during_a_turn_and_resets_when_idle() {
        let mut app = App::new(TestBackend::new(80, 30), UiTheme::dark()).unwrap();
        app.status_mut().set_model("anthropic/claude-opus-4-8");
        app.draw().unwrap();
        let idle = app.viewport_height();

        // Turn goes active (AgentStart sets `status.streaming`); a burst of finished tools grows the
        // live tail before it is committed.
        app.status_mut().set_streaming(true);
        for i in 0..8u32 {
            let name = format!("read_{i}");
            app.transcript_mut()
                .push_tool_start(name.clone(), serde_json::json!({ "path": format!("file_{i}.md") }));
            app.transcript_mut().push_tool_end(
                name,
                false,
                Some(serde_json::json!({ "content": [{ "type": "text", "text": format!("body {i}") }] })),
            );
        }
        app.draw().unwrap();
        let grown = app.viewport_height();
        assert!(grown > idle, "viewport should grow for the live tool tail ({grown} vs idle {idle})");

        // The finished tools commit to native scrollback mid-turn (SCREEN-FILL fix): the live content
        // collapses, but the floor HOLDS the viewport height so no reconstruction is triggered.
        app.transcript_mut().commit_finished_leading_tools();
        assert_eq!(app.state().transcript.active_tools().len(), 0, "finished tools left the tail");
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            grown,
            "grow-only floor must hold the height across the mid-turn commit (no per-tool reconstruct)"
        );

        // Turn goes idle (AgentEnd clears `status.streaming`): the floor resets and the region
        // collapses back to the compact idle height (void-fix preserved).
        app.status_mut().set_streaming(false);
        app.draw().unwrap();
        assert_eq!(
            app.viewport_height(),
            idle,
            "idle viewport must collapse back to the compact region after the turn"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod external_editor_tests {
    use super::*;

    /// Write a shell script into `dir` and return the multi-token editor command that runs it.
    ///
    /// The command is `"/bin/sh <script>"`, NOT the script path alone, and the script is
    /// deliberately left non-executable. Exec'ing a file this process itself just wrote is racy in
    /// a multi-threaded test binary: `std::fs::write` opens the file for writing, and any OTHER
    /// thread that forks (every `Command::spawn` in this binary) during that window hands its child
    /// an inherited write-fd, which makes the later `execve` of that same path fail with `ETXTBSY`.
    /// That surfaced as `run_editor_over_file` returning `None` about 9% of the time
    /// (`Os { code: 26, kind: ExecutableFileBusy }`, observed by instrumenting the spawn). Handing
    /// the script to `/bin/sh` as an ARGUMENT means only `/bin/sh` is exec'd and the script is
    /// merely opened for reading, so there is no window at all.
    ///
    /// It also exercises `run_editor_over_file`'s `split_whitespace` on a genuinely multi-token
    /// command (`sh`, the script, then the appended file), which is the realistic `$EDITOR` shape.
    #[cfg(unix)]
    fn sh_editor(dir: &std::path::Path, name: &str, body: &str) -> String {
        let script = dir.join(name);
        std::fs::write(&script, body).unwrap();
        format!("/bin/sh {}", script.display())
    }

    /// F14: the RESOLVED editor command is exactly what runs over the temp file — proving
    /// `edit_in_external_editor` spawns the command it is handed (which `App::run` resolves via
    /// `resolve_external_editor` → `EffectiveSettings::external_editor`, honoring settings
    /// `externalEditor` over `$VISUAL`/`$EDITOR`) rather than an inline env-only chain. The script
    /// rewrites the file; the reloaded text is the script's output.
    #[test]
    #[cfg(unix)]
    fn resolved_editor_command_is_the_one_that_runs() {
        let dir = tempfile::tempdir().unwrap();
        let editor = sh_editor(
            dir.path(),
            "fake-editor.sh",
            "printf 'REWRITTEN BY EDITOR' > \"$1\"\n",
        );

        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "original text").unwrap();

        let out = run_editor_over_file(&editor, &file);
        assert_eq!(
            out.as_deref(),
            Some("REWRITTEN BY EDITOR"),
            "the resolved editor's edit is reloaded"
        );
    }

    /// A non-zero editor exit yields `None` — Pi's "no change" (`false` exits 1 without editing).
    #[test]
    #[cfg(unix)]
    fn nonzero_editor_exit_is_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "keep me").unwrap();
        assert_eq!(run_editor_over_file("false", &file), None);
    }

    /// A trailing newline the editor leaves is stripped once (Pi's `.replace(/\n$/, "")`).
    #[test]
    #[cfg(unix)]
    fn trailing_newline_is_stripped_once() {
        let dir = tempfile::tempdir().unwrap();
        let editor = sh_editor(dir.path(), "nl-editor.sh", "printf 'line one\\n' > \"$1\"\n");
        let file = dir.path().join("buffer.md");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(run_editor_over_file(&editor, &file).as_deref(), Some("line one"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod x13_live_bash_tests {
    use super::*;
    use cyrup_provider::faux::FauxProvider;
    use cyrup_provider::Provider;
    use cyrup_session_svc::{SessionBuilder, SessionConfig};
    use ratatui::backend::TestBackend;

    /// ~3000 lines x ~40 bytes ≈ 120 KB — comfortably past `truncate.ts:11-12`'s 2000-line / 50 KB
    /// pair, so `bash-executor.ts`'s `ensureTempFile` spill and `truncateTail` both fire.
    const BIG: &str =
        "for i in $(seq 1 3000); do echo \"line-number-$i-padding-xxxxxxxxxx\"; done";

    async fn session(dir: &std::path::Path) -> Arc<AgentSession> {
        let cwd = dir.join("project");
        let agent_dir = dir.join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let mut cfg = SessionConfig::new(cwd, agent_dir);
        cfg.trust_override = Some(true);
        Arc::new(SessionBuilder::new(faux, cfg).build().await.unwrap())
    }

    /// Drive `spawn_session_bash` with EXACTLY the two `select!` arms the run loop uses, so the
    /// assertion covers the real wiring and not a re-implementation of it.
    async fn run_block(app: &mut App<TestBackend>, session: Arc<AgentSession>, command: &str) {
        app.state_mut().transcript.start_bash(command.to_string(), false, None, None);
        let mut rx = spawn_session_bash(session, command.to_string(), false);
        while let Some(msg) = rx.recv().await {
            match msg {
                BashMsg::Chunk(chunk) => app.state_mut().transcript.bash_append(&chunk),
                BashMsg::Done { exit_code, cancelled, truncated, full_output_path } => {
                    app.state_mut().transcript.bash_complete(
                        exit_code,
                        cancelled,
                        truncated,
                        full_output_path,
                    );
                    app.state_mut().transcript.commit_bash();
                    break;
                }
            }
        }
        app.draw().unwrap();
    }

    /// **X13 — a LIVE `!` run names its spool file.**
    ///
    /// `bash-execution.ts:195-199`:
    /// ```ts
    /// const wasTruncated = this.truncationResult?.truncated || contextTruncation.truncated;
    /// if (wasTruncated && this.fullOutputPath) {
    ///     statusParts.push(theme.fg("warning", `Output truncated. Full output: ${this.fullOutputPath}`));
    /// }
    /// ```
    /// `fullOutputPath` is `setComplete`'s FOURTH argument, which `handleBashCommand` passes from
    /// `result.fullOutputPath` (`interactive-mode.ts:6352`). The old local `sh -c` pump had no
    /// spool at all and always passed `false, None`, so the row was unreachable outside replay —
    /// even though `contextTruncation.truncated` was already true here (120 KB / 3000 lines), which
    /// is exactly why the `&& this.fullOutputPath` leg is what this test turns on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_live_bash_run_names_its_spool_file() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path()).await;
        let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
        run_block(&mut app, session, BIG).await;

        let out = app.scrollback_text();
        // TUI-N13: pi emits the whole status block as `new Text(`\n${statusParts.join("\n")}`, 1, 0)`
        // (`bash-execution.ts:201` @v0.83.0) — padding-left 1, WORD-WRAPPED to the terminal width —
        // and cyrup renders it the same way, so a status part longer than the width legitimately
        // occupies two visual lines in BOTH. The spool path comes from `std::env::temp_dir()`
        // (`cyrup-session-svc/src/bash.rs:258`), which on macOS is `/var/folders/<2>/<30>/T/`: at
        // that length ` Output truncated. Full output: <path>` is exactly 120 columns and the path
        // wraps onto the next line, while on Linux (`TMPDIR` unset -> `/tmp`) it does not. Reading a
        // single `.lines()` entry therefore asserted the length of the ambient TMPDIR rather than the
        // wiring under test. Flatten the wrap first; the assertion itself is unchanged and still
        // requires the RENDERED scrollback to name the executor's own spool file.
        let flat = out.split_whitespace().collect::<Vec<_>>().join(" ");
        let path = flat
            .split_once("Output truncated. Full output: ")
            .unwrap_or_else(|| panic!("no truncation row in a 120 KB live run:\n{out}"))
            .1
            .split_whitespace()
            .next()
            .unwrap_or_else(|| panic!("the truncation row named no file:\n{out}"))
            .to_string();
        assert!(path.contains("cyrup-bash-"), "the spool file is the executor's: {path}");

        // The named file really holds the FULL output — the row is not a decorative string.
        let spooled = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("the spool file must be readable at {path}: {e}"));
        assert!(spooled.contains("line-number-1-padding"), "nothing dropped from the front");
        assert!(spooled.contains("line-number-3000-padding"), "nor from the tail");
        let _ = std::fs::remove_file(&path);
    }

    /// MIRROR — a SMALL live run spools nothing, so the row must not appear. Proves the wiring
    /// forwards the executor's real report rather than hard-coding the other constant.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_small_live_bash_run_has_no_truncation_row() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path()).await;
        let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
        run_block(&mut app, session, "echo hello-small").await;

        let out = app.scrollback_text();
        assert!(out.contains("hello-small"), "the output rendered:\n{out}");
        assert!(!out.contains("Output truncated"), "and nothing was spooled:\n{out}");
    }

    /// The run is recorded through the session's own `recordBashResult` (`agent-session.ts:2628`)
    /// — WITH the truncation fields, which is what makes the replay arm able to restore the row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_live_run_records_a_bash_execution_message_carrying_the_spool_path() {
        let dir = tempfile::tempdir().unwrap();
        let session = session(dir.path()).await;
        let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
        run_block(&mut app, session.clone(), BIG).await;

        let msgs = session.agent_messages().await;
        let payload = msgs
            .iter()
            .find_map(|m| match m {
                cyrup_agent::AgentMessage::Custom { kind, payload, .. }
                    if kind == "bashExecution" =>
                {
                    Some(payload.clone())
                }
                _ => None,
            })
            .expect("`executeBash` records the run itself — the caller must not append its own");
        assert_eq!(payload["truncated"], true);
        let path = payload["fullOutputPath"].as_str().expect("the spool path persisted");
        assert!(path.contains("cyrup-bash-"));
        let _ = std::fs::remove_file(path);
    }
}
