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
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::terminal::{
    enable_raw_mode, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate,
};
use ratatui::crossterm::{execute, queue, ExecutableCommand};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::commands::{CommandRegistry, Dispatch};
use crate::component::{Component, InputEvent};
use crate::editor::{EditorOutcome, InputEditor};
use crate::error::TuiError;
use crate::extension_editor::ExtensionEditorSelector;
use crate::image::{ImageBlock, ImageRenderer, TerminalCapabilities};
use crate::keymap::{
    Action, EditorAction, Key, KeybindingIssue, Keymap, ModelsKeymap, SelectAction, SelectKeymap,
    SessionKeymap, TreeKeymap,
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
use crate::status_indicator::{IndicatorKind, StatusIndicator, WorkingIndicator, SPINNER_INTERVAL};
use crate::escape_reassembly::EscapeReassembler;
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

/// One entry of Pi's `compactionQueuedMessages` (`interactive-mode.ts:401`, the
/// `CompactionQueuedMessage` record `{ text, mode: "steer" | "followUp" }`). TUI-031.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionQueued {
    /// The submitted text, verbatim.
    pub text: String,
    /// `mode === "followUp"` — Alt+Enter's queue rather than Enter's steering queue.
    pub follow_up: bool,
}

/// One mounted extension widget — Pi's entry in `extensionWidgetsAbove` / `extensionWidgetsBelow`
/// (`interactive-mode.ts:1920-1960` @v0.83.0). TUI-014.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionWidget {
    /// Pi's `key`: the identity a re-emit replaces on.
    pub key: String,
    /// The rendered rows, already capped at [`ExtensionWidget::MAX_WIDGET_LINES`] with pi's
    /// truncation marker appended when the content was longer.
    pub lines: Vec<String>,
    /// `options.placement === "belowEditor"` (`:1925`, `:1957`); the default is `"aboveEditor"`.
    pub below: bool,
}

impl ExtensionWidget {
    /// `InteractiveMode.MAX_WIDGET_LINES` (`interactive-mode.ts:2008`).
    pub const MAX_WIDGET_LINES: usize = 10;

    /// Pi's truncation row, appended verbatim when the content exceeded the cap (`:1948-1950`,
    /// `theme.fg("muted", "... (widget truncated)")`).
    pub const TRUNCATED: &'static str = "... (widget truncated)";

    /// Read Pi's three `setWidget` arguments off the [`UiEffect::SetWidget`] carrier.
    ///
    /// SEAM-011/EXT-047: `set-widget` carries pi's `key`, `lines` and `placement` separately now
    /// (`wit/world.wit`, `HostServices::set_widget`), and `LiveHostServices` re-packs exactly those
    /// three under pi's own names for this in-process channel (`host_services.rs:724-737`) — so this
    /// reads `{"key": …, "lines": [...], "placement": "aboveEditor" | "belowEditor"}` and nothing
    /// else. It used to read a cyrup-invented `{"content": …, "options": {"placement": …}}` blob;
    /// after the seam widened, that spelling stopped arriving and every widget was dropped.
    ///
    /// `lines` is Pi's `content: string[]` arm (`:1942-1951`); `null`/absent is Pi's
    /// `content === undefined`, which REMOVES the key (`:1935-1938`) and is read here as an empty
    /// line list. A payload that is not an object at all has no structure to recover, so it renders
    /// as its JSON text under an empty key.
    pub fn from_json(v: &serde_json::Value) -> Self {
        let obj = v.as_object();
        let key = obj
            .and_then(|o| o.get("key"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        // `options?.placement ?? "aboveEditor"` (`interactive-mode.ts:1925` @v0.83.0) — the WIT
        // resolves the default host-side, so the carrier always spells the placement out.
        let placement = obj
            .and_then(|o| o.get("placement"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("aboveEditor");
        let below = placement == "belowEditor";
        let content = obj.and_then(|o| o.get("lines"));
        let mut lines: Vec<String> = match content {
            Some(serde_json::Value::String(text)) => {
                text.lines().map(str::to_string).collect()
            }
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .map(|i| {
                    i.as_str().map(str::to_string).unwrap_or_else(|| i.to_string())
                })
                .collect(),
            // `content === undefined` removes the widget (`:1935-1938`) — an empty line list is
            // what the caller reads as "remove".
            Some(serde_json::Value::Null) | None if obj.is_some() => Vec::new(),
            Some(other) => vec![other.to_string()],
            None => vec![v.to_string()],
        };
        if lines.len() > Self::MAX_WIDGET_LINES {
            lines.truncate(Self::MAX_WIDGET_LINES);
            lines.push(Self::TRUNCATED.to_string());
        }
        ExtensionWidget { key, lines, below }
    }
}

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
    SetThinking(String),
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
    /// Messages the session is HOLDING because a turn is streaming — Pi's
    /// `pendingMessagesContainer` (`interactive-mode.ts:328`, filled by
    /// `updatePendingMessagesDisplay` at `:3974-3991`), docked directly above the status band.
    /// Fed from `queue_update`; see [`crate::pending_messages`] for why it exists and what it
    /// replaced (TUI-016 / TUI-052).
    pub pending_messages: crate::pending_messages::PendingMessages,
    /// The last `queue_update` snapshot from the SESSION's own two queues, kept so the pending
    /// region can be rebuilt whenever either source changes — Pi's `getAllQueuedMessages`
    /// (`interactive-mode.ts:3942-3953`) folds `session.getSteeringMessages()` /
    /// `getFollowUpMessages()` together with `compactionQueuedMessages` every time it renders.
    /// TUI-031.
    pub session_queue: (Vec<String>, Vec<String>),
    /// Raised by the sync `compaction_end` arm and consumed by [`App::ingest_session_event`], which
    /// has the session needed to actually deliver the queue. TUI-031.
    pub compaction_flush_pending: bool,
    /// Whether a compaction is currently running — set by `compaction_start` and cleared by
    /// `compaction_end`, the window in which Pi's Escape handler is rebound to `abortCompaction`.
    pub compacting: bool,
    /// Pi's `compactionQueuedMessages` (`interactive-mode.ts:401`) — prompts submitted WHILE a
    /// compaction is running. The session layer has no compaction guard of its own, so without this
    /// a message typed mid-compaction was dispatched as a fresh turn assembled from a context the
    /// compaction was in the middle of rewriting. TUI-031.
    pub compaction_queue: Vec<CompactionQueued>,
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
    /// Timestamp of the last Escape on an EMPTY editor, for Pi's 500 ms double-Escape window
    /// (`interactive-mode.ts:2579-2594`, `private lastEscapeTime = 0` at `:355`). `None` until the
    /// first press, and reset to `None` when a pair fires so a third press starts a new pair.
    /// TUI-009.
    last_escape: Option<std::time::Instant>,
    /// The persisted `doubleEscapeAction` setting (`tree` / `fork` / `none`), cached here because
    /// [`App::apply_action`] resolves keys without a session in hand. Seeded at boot and re-seeded
    /// on every session swap alongside the other per-session settings. TUI-009.
    pub double_escape_action: String,
    /// The persisted `warnings.anthropicExtraUsage` value, cached for the `/settings` → `Warnings`
    /// submenu, which is opened from a selector outcome with no session in hand. Pi's default is
    /// `true` (`settings-selector.ts:134` `(this.state.anthropicExtraUsage ?? true)`). TUI-032.
    pub warn_anthropic_extra_usage: bool,
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
    /// The extension-visible mirror of the editor buffer (SEAM-T02) — the cell backing
    /// `HostServices::editor_text`, i.e. pi's `getEditorText: () =>
    /// this.editor.getExpandedText?.() ?? this.editor.getText()` (`interactive-mode.ts:2393`
    /// @v0.84.2). Republished by [`App::publish_extension_readbacks`] on every frame; handed to the
    /// session's `LiveHostServices` by [`App::install_extension_readbacks`], without which
    /// `editor_text` keeps the trait default `""` — the shape the read half shipped in, while its
    /// WRITE half (`set_editor_text`) worked, so a guest's read-modify-write silently discarded its
    /// own edit. Always present here (an unattached mirror is simply never read).
    editor_mirror: cyrup_session_svc::EditorTextMirror,
    /// The live theme seam handed to the session's `LiveHostServices` (SEAM-T01) — pi's four
    /// `createExtensionUIContext` theme bindings (`interactive-mode.ts:2401-2417` @v0.84.2). `None`
    /// until a session binds ([`App::install_extension_readbacks`]), and rebuilt on every session
    /// swap because it holds that session's resource snapshot. Kept here so
    /// [`App::publish_extension_readbacks`] can republish the active theme name each frame.
    theme_access: Option<Arc<crate::theme_access::TuiThemeAccess>>,
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
    /// The custom header content an extension published — Pi `setHeader(factory)` →
    /// `setExtensionHeader` (`interactive-mode.ts:2262-2290` @v0.83.0), which splices the custom
    /// header into `headerContainer` in place of `builtInHeader` and restores the built-in when the
    /// factory is `undefined`. TUI-033: rendered as the first rows of the message region.
    pub extension_header: Option<String>,
    /// The custom footer content an extension published — Pi `setFooter(factory)` →
    /// `setExtensionFooter` (`:2235-2257`), which clears `footerContainer` and swaps the extension
    /// component in for the built-in footer. TUI-033: rendered in place of the [`StatusLine`] rows.
    pub extension_footer: Option<String>,
    /// The extension widgets currently mounted, keyed by Pi's `key` — `setExtensionWidget`
    /// (`interactive-mode.ts:1920-1960` @v0.83.0) keeps two maps, `extensionWidgetsAbove` and
    /// `extensionWidgetsBelow`, removes the key from BOTH before re-inserting, and drops it entirely
    /// when `content` is `undefined`. TUI-014.
    ///
    /// Pi's three `setWidget(key, content, options)` arguments arrive separately since SEAM-011
    /// widened the WIT; the in-process [`UiEffect::SetWidget`] carrier re-packs them under pi's own
    /// `key`/`lines`/`placement` names (`host_services.rs:150-161`), which
    /// [`ExtensionWidget::from_json`] reads back field by field.
    pub extension_widgets: Vec<ExtensionWidget>,
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
            pending_messages: crate::pending_messages::PendingMessages::default(),
            session_queue: (Vec::new(), Vec::new()),
            compaction_flush_pending: false,
            compacting: false,
            compaction_queue: Vec::new(),
            reserve_status_rows: false,
            term_rows: 24,
            show_startup_hints: true,
            loader: None,
            loader_tick: 0,
            should_quit: false,
            last_sigint: None,
            last_escape: None,
            // Pi's own default is `"tree"` (`settings.ts` `getDoubleEscapeAction`); the real value
            // is seeded from the session's effective settings before the first frame.
            double_escape_action: "tree".to_string(),
            // Pi's `?? true` default (`settings-selector.ts:134`); re-seeded from the session's
            // effective settings before the first frame.
            warn_anthropic_extra_usage: true,
            pending_swap_status: None,
            scrollback: Vec::new(),
            extension_shortcuts: Vec::new(),
            capabilities: TerminalCapabilities {
                images: None,
                true_color: true,
                hyperlinks: false,
            },
            pending_ui_reply: None,
            editor_mirror: cyrup_session_svc::EditorTextMirror::new(),
            theme_access: None,
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
            extension_widgets: Vec::new(),
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
///
/// `now` is the instant of the tick being rendered — Pi's `CountdownTimer` decrements
/// `remainingSeconds` inside the `setInterval` callback (`countdown-timer.ts:22-24`), i.e. the
/// displayed value belongs to the tick, not to whenever the string happens to be formatted. Reading
/// the clock in here instead left [`App::tick_extension_dialog_countdown_at`]'s injected instant
/// governing only the expiry branch, so a ticked-forward countdown still printed its opening value.
fn countdown_title(base: &str, deadline: tokio::time::Instant, now: tokio::time::Instant) -> String {
    let remaining = deadline.saturating_duration_since(now);
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
    /// Where a spawned `/compact` posts its outcome back to the run loop — installed by
    /// [`App::install_compact_channel`], the same shape as [`Self::tree_nav_tx`].
    ///
    /// **TUI-055.** `session.compact(...)` is a 10–20 s provider call. Awaited inline in the run
    /// loop's `AppAction::Command` arm — which is what cyrup did — that single task cannot reach any
    /// other `select!` arm for the whole operation: the `compaction_start` event sits unread in
    /// `events`, `IndicatorKind::Compaction` is never armed, and the 80 ms spinner arm never fires.
    /// Measured live on 2026-08-13, sampled every 200 ms across a 10.5 s compaction: the status band
    /// was empty in **every** sample. Spawning it and answering over this channel is the same
    /// channel-back shape `/tree` and `/login` already use, and it is what lets the band Pi shows
    /// for the whole operation (`interactive-mode.ts:3075-3087`) actually reach the screen.
    ///
    /// `None` (an embedder or a test driving `execute_command` directly) falls back to awaiting
    /// inline, exactly as `/tree` does — correct, just without a live band, because there is no loop
    /// to paint one.
    compact_tx: Option<tokio::sync::mpsc::UnboundedSender<CompactOutcome>>,
    /// Where a spawned [`AgentSession::drain_queue`](cyrup_session_svc::AgentSession::drain_queue)
    /// hands its take-all back to the run loop (TUI-092 §5b.1).
    ///
    /// `drain_queue` ends in `emit_queue_update().await` (`cyrup-session-svc/src/session.rs:1495`),
    /// which fans the `QueueUpdate` out through `Fanout::emit` (`subscriber.rs:64-76`) — an
    /// **awaited send into every live subscription**, and those channels are
    /// `mpsc::channel(CHANNEL_CAPACITY)` with `CHANNEL_CAPACITY = 1024` (`subscriber.rs:23`), i.e.
    /// BOUNDED. One of those subscriptions is `App::run`'s own `events` stream
    /// (`AgentSession::subscribe` → `subscribe_persistent`). So awaiting `drain_queue` **on the run
    /// loop's task** closes a cycle: the loop blocks inside a send into the very channel that only
    /// the loop drains. With the channel full it never returns — and `Fanout::emit` discards the
    /// send result (`let _ = …`), so nothing is logged when it happens. `Escape` during a streaming
    /// turn and `Alt+Up` both reached it.
    ///
    /// Fixed in the TUI, not the session layer: `Fanout::emit`'s awaited send IS its contract
    /// ("backpressure → slows the agent, never drops", `subscriber.rs:63`), and spawning it there
    /// would reorder `QueueUpdate` and drop that backpressure for RPC mode and every SDK observer
    /// too. The defect is that the TUI awaited a session call on the one task that must stay free to
    /// drain the session's events.
    ///
    /// `None` (an embedder or a test driving `execute_command` directly) falls back to awaiting
    /// inline, exactly as `/tree` and `/compact` do — correct, because without a run loop there is
    /// no `events` subscription for the fan-out to block on.
    queue_drain_tx: Option<tokio::sync::mpsc::UnboundedSender<QueueDrain>>,
    /// Where a spawned session-lifecycle op (`/new`, `/reload`, `/import`, `/resume`, `/fork`) hands
    /// its outcome back to the run loop (TUI-092 §5b.2).
    ///
    /// These five `execute_command` arms each `.await` an `AgentSessionRuntime` op that dispatches
    /// `HostEvent::Session{Start,Shutdown,BeforeSwitch,BeforeFork}` to every live extension's hook,
    /// and a guest hook handler is handed the SAME `Ctx` a tool/shortcut handler gets — so it CAN
    /// call `ctx.ui().*`, which parks its calling task in `LiveHostServices`'
    /// `block_in_place` + `block_on` (`cyrup-session-svc/src/host_services.rs`) until THIS loop
    /// answers `ui_rx`. Awaited inline, that made the blocked task and the loop that must unblock it
    /// the same task: `block_in_place` frees a worker THREAD for other tasks, never this task's own
    /// other `select!` branches. A genuine, permanent self-deadlock — and `tokio::time::timeout`
    /// cannot rescue it, because a parked `poll()` is never polled again (proven in-repo:
    /// `cyrup-ext/src/dispatch.rs:499` wraps the same call in a budget that still cannot fire).
    ///
    /// This is the residual `execute_command`'s own doc comment used to flag and defer. It is closed
    /// the way that comment prescribed — the runtime `.await` runs off-task and only the
    /// `self.state` mutation comes back here — which is the same shape `C::Compact` and `/tree`
    /// already use.
    ///
    /// `None` (an embedder or a test driving `execute_command` directly) falls back to awaiting
    /// inline: correct there, because with no run loop there is no `ui_rx` arm for a guest dialog to
    /// be waiting on in the first place.
    lifecycle_tx: Option<tokio::sync::mpsc::UnboundedSender<LifecycleOutcome>>,
}

/// The `&mut self` work a settled session-lifecycle op still owes the run loop (TUI-092 §5b.2).
///
/// Deliberately tiny: `pending_swap_status` is set OPTIMISTICALLY before the spawn (see
/// [`App::dispatch_lifecycle`]), so a successful op needs nothing from here in the common case.
#[derive(Debug, Default)]
pub struct LifecycleEffects {
    /// `/fork` with `position: "before"` re-seeds the editor with the anchor text
    /// (`RuntimeForkResult::selected_text`).
    pub selected_text: Option<String>,
    /// `/reload` rebuilds the keymaps from this agent dir. Runs AFTER the session reload, which is
    /// Pi's order (`interactive-mode.ts:5386`, session reload then `this.keybindings.reload()`).
    pub reload_keybindings_in: Option<PathBuf>,
}

/// What a spawned session-lifecycle op hands back (TUI-092 §5b.2).
///
/// `Err` carries an ALREADY-RENDERED status line — the per-command cancellation string or error
/// wording Pi uses — so the run loop needs no context to display it, and the optimistic
/// `pending_swap_status` is cleared with it.
#[derive(Debug)]
pub struct LifecycleOutcome(pub Result<LifecycleEffects, String>);

/// Why a [`QueueDrain`] was requested — it decides what the run loop does with the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueDrainReason {
    /// `Escape` while streaming (`AppAction::InterruptRestoreQueued`, Pi `onEscape`
    /// interactive-mode.ts:2636-2637): restore to the editor, then abort the run and any bash child.
    Interrupt,
    /// `Alt+Up` (`AppAction::Dequeue`, Pi `handleDequeue` interactive-mode.ts:3587-3594): restore to
    /// the editor and report how many came back. No abort.
    Dequeue,
    /// The `/tree` pre-step (Pi `:4781-4785`). The abort is issued by the spawning task itself,
    /// immediately after the drain and before `navigate_tree`, so only the editor restore is left
    /// for the loop.
    TreeNav,
}

/// What a spawned [`AgentSession::drain_queue`](cyrup_session_svc::AgentSession::drain_queue) hands
/// back — Pi's `(steering, followUp)` pair plus the reason, so
/// [`App::apply_queue_drain`] can finish the job on the loop task (TUI-092 §5b.1).
#[derive(Clone, Debug)]
pub struct QueueDrain {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
    pub reason: QueueDrainReason,
}

/// What a spawned `/compact` hands back to the run loop — the `Ok`/`Err` of
/// [`AgentSession::compact`](cyrup_session_svc::AgentSession::compact), with the error already
/// rendered to a string so the message needs no session to interpret.
pub type CompactOutcome = Result<cyrup_session_svc::CompactionResult, String>;

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
            compact_tx: None,
            queue_drain_tx: None,
            lifecycle_tx: None,
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
    /// A malformed DOCUMENT (unparseable JSON, or a non-object top level) is surfaced as a typed
    /// error and nothing is applied — Pi's `loadRawConfig` returning `undefined`
    /// (`core/keybindings.ts:328-336` @v0.83.0). An individual bad ENTRY is not an error: it comes
    /// back in the returned [`KeybindingIssue`] list and every other entry still applies, so the
    /// binary can name the offending ids instead of claiming it ignored a file it half-applied
    /// (CFG-038). Never a panic.
    ///
    /// The issue lists of all six maps are concatenated rather than short-circuited, for the same
    /// reason: `?` between the maps used to leave the global keymap applied and the editor keymap
    /// untouched whenever a later map rejected something.
    pub fn load_keybindings_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        let mut issues = self.state.keymap.merge_json(json)?;
        // X9 — every `… to expand` hint resolves its key label through the LIVE keymap upstream
        // (`keyText("app.tools.expand")`, `keybinding-hints.ts:34-36`). The transcript holds no
        // keymap, so the resolved label is pushed to it whenever bindings change.
        let expand = self.state.keymap.keys_label(Action::ToolsExpand);
        self.state.transcript.set_expand_hint(expand);
        issues.extend(self.state.select_keymap.merge_json(json)?);
        issues.extend(self.state.tree_keymap.merge_json(json)?);
        issues.extend(self.state.session_keymap.merge_json(json)?);
        issues.extend(self.state.models_keymap.merge_json(json)?);
        issues.extend(self.state.editor.merge_keybindings_json(json)?);
        Ok(issues)
    }

    /// TUI-051 — re-read `<agent_dir>/keybindings.json` and re-apply it to every live map.
    ///
    /// Pi calls `this.keybindings.reload()` inside `handleReloadCommand`, immediately after
    /// `await this.session.reload(...)` (`interactive-mode.ts:5386` @v0.83.0) →
    /// `core/keybindings.ts:354-357` `setUserBindings(KeybindingsManager.loadFromFile(configPath))`
    /// → `loadFromFile` (`:363-367`) re-reads the file, re-runs `migrateKeybindingsConfig` and hands
    /// the result to `packages/tui/src/keybindings.ts:167-192` `rebuild()`.
    ///
    /// cyrup's `/reload` never touched the file — while both the command's help string
    /// (`commands.rs`) and the handler's own comment claimed it did — so the single documented way
    /// to apply an edited `keybindings.json` was a process restart, which nothing told the user.
    ///
    /// **Reset-then-merge, not merge**: `rebuild()` REPLACES (`keybindings.ts:187-191`), so an entry
    /// the user deleted must go back to its default. A missing file is not an error — it means "no
    /// user bindings", i.e. every default (Pi's `loadFromFile` returns `{}` for one).
    ///
    /// Returns the entries the reloaded document could not use (CFG-038), so `/reload` can name
    /// them the same way startup does.
    pub fn reload_keybindings_from(
        &mut self,
        agent_dir: &std::path::Path,
    ) -> Result<Vec<KeybindingIssue>, TuiError> {
        let path = agent_dir.join("keybindings.json");
        let json = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            // No file ⇒ defaults only, which the reset below already produces.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::from("{}"),
            Err(e) => return Err(TuiError::Backend(e.to_string())),
        };
        self.state.keymap = Keymap::default();
        self.state.select_keymap = crate::keymap::SelectKeymap::default();
        self.state.tree_keymap = crate::keymap::TreeKeymap::default();
        self.state.session_keymap = crate::keymap::SessionKeymap::default();
        self.state.models_keymap = crate::keymap::ModelsKeymap::default();
        self.state.editor.reset_keybindings_to_defaults();
        self.load_keybindings_json(&json)
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

    /// Pi `handleClipboardPaste` (`interactive-mode.ts:2870-2892` @v0.84.2): read an **image**
    /// first and, only when there is none, read **text** — both inserted at the editor cursor with
    /// `insertTextAtCursor`. Returns whether anything was pasted.
    ///
    /// The two clipboard reads are passed as closures rather than performed here so the ORDER is a
    /// unit-testable fact without a live system clipboard: pi's text read is lazy — it never runs
    /// when an image was found (`:2882` returns before `:2884`) — and a version that read both up
    /// front would pass an equality assertion while diverging on a clipboard holding both.
    fn paste_from_clipboard(
        &mut self,
        image: impl FnOnce() -> Option<std::path::PathBuf>,
        text: impl FnOnce() -> Option<String>,
    ) -> bool {
        // `const image = await readClipboardImage(); if (image) { … return; }` (`:2872-2882`).
        if let Some(path) = image() {
            self.insert_clipboard_image_path(&path);
            return true;
        }
        // `const text = await readClipboardText(); if (text) { this.editor.insertTextAtCursor(text) }`
        // (`:2884-2888`). DRIFT-045: this branch did not exist, so a Ctrl+V over a clipboard
        // holding text inserted nothing at all — against a help table that advertises
        // "Paste image or text from clipboard" (`:2101`).
        if let Some(text) = text().filter(|t| !t.is_empty()) {
            self.state.editor.insert_str(&text);
            return true;
        }
        false
    }

    /// Read a system-clipboard image, materialize it to a `cyrup-clipboard-<uuid>.png` temp file, and
    /// insert its PATH as text at the editor cursor; failing that, insert the clipboard's TEXT
    /// (Pi `handleClipboardPaste`, interactive-mode.ts:2870-2892). Returns `true` when something
    /// was pasted; `false` when the clipboard holds neither an image nor text, or on any
    /// clipboard/encode/IO error — so the caller still lets Ctrl+V fall through to the editor.
    fn try_paste_clipboard_image_path(&mut self) -> bool {
        self.paste_from_clipboard(read_clipboard_image_to_temp, crate::clipboard::read_clipboard_text)
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
        // TUI-N12 — seed the WHOLE record, not just `hyperlinks`: the cache now carries `images`
        // and `true_color` too, and this call site already holds all three.
        crate::image::seed_capabilities(caps);
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
        // TUI-N01 / TUI-036 — publish the capability where the two consumers can reach it: the
        // transcript's tool-result image gate (Pi `tool-execution.ts:331`) and the `/settings` grid
        // builder, which must not offer image rows on a terminal with no protocol
        // (`settings-selector.ts:654-671`). `AppState::image_renderer` is not reachable from either.
        self.state
            .transcript
            .set_graphical_images(self.state.image_renderer.is_graphical());
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
        self.state.extension_widgets.clear();
        self.state.extension_shortcuts.clear();
        self.state.status.extension_statuses.clear();
        // An extension dialog/editor overlay belongs to the outgoing host; leaving it up would
        // present a prompt whose reply channel is about to be dropped.
        self.state.overlays.clear();
        self.state.selector = None;
        // TUI-030 — the working-indicator family, which upstream resets in the SAME function:
        // `this.workingMessage = undefined; this.workingVisible = true; this.setWorkingIndicator();`
        // then `this.setHiddenThinkingLabel()` (`interactive-mode.ts:2210-2218` @v0.84.2). Without
        // this, an extension that hid the working band — or renamed it — would leave the NEXT
        // session with a band owned by a host that no longer exists: the same class of leak the
        // header/footer/widget clears above fix.
        //
        // Upstream's one extra step is the live band's copy: when the band is currently the working
        // one it is re-messaged to `"${defaultWorkingMessage} (${keyText("app.interrupt")} to
        // interrupt)"` (`:2213-2217`) — the ONLY place upstream ever suffixes a working message, and
        // it says "to interrupt", not the "to cancel" the other three kinds bake in.
        let interrupt = self.state.keymap.keys_label(Action::Interrupt);
        self.state.indicator.reset_extension_working_state(interrupt.as_deref());
        self.state.transcript.set_hidden_thinking_label(None);
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
        // The queue belongs to the OUTGOING session: its steering/follow-up lists were emitted by a
        // `queue_update` from a session that is gone, and its compaction queue would be delivered
        // into the new one. Clearing them clears the rendered region, which is the whole point —
        // this used to be `status.set_queued(0)`, which zeroed a counter with no render site and
        // left `pending_messages` drawing the previous session's `Steering: …` rows above the
        // editor for the rest of the process (TUI-016 / ADR-0009 item 3).
        self.state.session_queue = (Vec::new(), Vec::new());
        self.state.compaction_queue.clear();
        self.rebuild_pending_messages();
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

    /// TUI-N04 — the second statement of Pi's `renderInitialMessages()`, immediately after the
    /// replay (`interactive-mode.ts:3485`), body at `:3496-3514` @v0.83.0:
    ///
    /// ```ts
    /// private renderProjectTrustWarningIfNeeded(): void {
    ///     if (this.settingsManager.isProjectTrusted() || !hasTrustRequiringProjectResources(this.sessionManager.getCwd())) {
    ///         return;
    ///     }
    ///     if (this.chatContainer.children.length > 0) this.chatContainer.addChild(new Spacer(1));
    ///     this.chatContainer.addChild(new Text(theme.fg("warning",
    ///         `This project is not trusted. Project ${CONFIG_DIR_NAME} resources and packages are ignored. Use /trust to save a trust decision, then restart pi.`), 1, 0));
    /// }
    /// ```
    ///
    /// Both halves of the predicate already existed in cyrup and neither had a reader on this path:
    /// `AgentSessionServices::project_trusted` (`services.rs:104`, the same field the `/trust`
    /// dialog reads at [`Self::open_selector`]) and
    /// [`cyrup_config::trust::has_trust_requiring_resources`] (`trust.rs:201`, the same scan
    /// `AgentSessionBuilder` runs at `builder.rs:597` to decide whether trust is even in question).
    ///
    /// **The string is rebranded, not reworded**: `.cyrup` for pi's `CONFIG_DIR_NAME` (the directory
    /// `has_trust_requiring_resources` actually probes, `trust.rs:211`) and `cyrup` for `pi`.
    ///
    /// **[CYRUP-DELTA]** — pi gates its leading `Spacer(1)` on `chatContainer.children.length > 0`
    /// (`:3502`), so on a *completely* empty transcript the warning is the first row with no blank
    /// above it. `Entry::Warning` emits its leading blank unconditionally
    /// (`transcript.rs`'s `Entry::Warning` arm, matching `showWarning`), so cyrup shows one extra
    /// blank line in that one case. Reproducing the gate would mean a second warning entry kind
    /// whose only difference is a blank line; recorded rather than taken.
    pub fn render_project_trust_warning_if_needed(&mut self, session: &Arc<AgentSession>) {
        let services = session.services();
        if services.project_trusted
            || !cyrup_config::trust::has_trust_requiring_resources(&services.cwd, &services.home)
        {
            return;
        }
        // No `Warning: ` prefix: pi's trust banner is a RAW `Text` in the warning colour (`:3505`),
        // not a `showWarning` call, so — unlike `interactive-mode.ts:3884-3888`'s
        // `Warning: ${warningMessage}` — there is no prefix to carry (TUI-062).
        self.state.transcript.push_warning(PROJECT_UNTRUSTED_WARNING);
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
        // SEAM-T01/T02 — republish the extension-visible editor buffer and active theme name before
        // anything is painted, so a guest reading either sees the state this frame is about to
        // show. One call site because `draw` is the one every run-loop arm that can have changed
        // them passes through (see [`Self::publish_extension_readbacks`]).
        self.publish_extension_readbacks();
        // Content-size the inline viewport to the live region (active turn + band + slot + footer),
        // recomputed every frame as content grows/shrinks (ADR-0001 #1, audit #1). The viewport is
        // rebuilt only when its height actually changes so steady-state frames keep their cell-diff.
        // Resize **before** flushing so the committed `insert_before` lines scroll above the
        // correctly-anchored viewport (the active turn's height is unaffected by the flush).
        let size = self.terminal.backend().size().ok();
        // TUI-039 — pi's `$LINES` / `$COLUMNS` step sits between the ioctl and the constant
        // (`tui.ts:1730-1736`). cyrup's own last resort here stays the live viewport height rather
        // than pi's bare `24`, since it is a strictly better guess when one is available.
        let term_h = size
            .map(|s| s.height)
            .or_else(env_rows)
            .unwrap_or(self.viewport_height)
            .max(1);
        let term_w = size.map(|s| s.width).unwrap_or_else(fallback_columns);
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
        let width = self
            .terminal
            .backend()
            .size()
            .map(|s| s.width)
            .unwrap_or_else(|_| fallback_columns()) as usize;
        let output_pad = self.state.transcript.output_pad();
        // Committed tool-result images keep rendering — a half-block raster is ordinary cells, so it
        // survives `insert_before` into native scrollback (see `ImageBlock::halfblock_lines`).
        let images = crate::transcript::ImageOpts {
            show: self.state.transcript.show_images(),
            // TUI-N01 — the committed path reads the same capability the live one does, so a block
            // that scrolled up cannot disagree with the one still on screen.
            graphical: self.state.transcript.graphical_images(),
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
            // TUI-030 — the LIVE `setHiddenThinkingLabel` override, for the same reason
            // `tools_expanded` is read live here: a reasoning block flushed to scrollback must not
            // disagree with the one still on screen it was scrolled up from.
            hidden_thinking_label: Some(self.state.transcript.hidden_thinking_label()),
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
            // TUI-079 — `/export` and `/import` take ONE quote-aware token, not the whole
            // remainder: pi runs both through `getPathCommandArgument`
            // (`interactive-mode.ts:5435`, `:5480` @v0.83.0). Without it
            // `/export "my session.html"` wrote a file whose name contained the quote characters,
            // `/export a.html junk` wrote to the path `a.html junk`, and an unterminated quote —
            // which upstream REFUSES — was accepted as a path. A refusal arrives here as `None`,
            // which is already each arm's no-argument branch: usage for `/import`, and the
            // session-directory default for `/export`, exactly as upstream's `undefined` does.
            "export" => cmd(C::Export(arg.as_deref().and_then(crate::commands::path_command_argument))),
            "import" => cmd(C::Import(arg.as_deref().and_then(crate::commands::path_command_argument))),
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

    /// Rebuild the pending-messages region from BOTH queue sources — Pi's `getAllQueuedMessages`
    /// (`interactive-mode.ts:3942-3953` @v0.83.0), which concatenates the session's steering /
    /// follow-up lists with the `compactionQueuedMessages` of the matching mode, in that order.
    /// TUI-031.
    fn rebuild_pending_messages(&mut self) {
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
    fn queue_compaction_message(&mut self, text: String, follow_up: bool) {
        self.state.compaction_queue.push(CompactionQueued { text, follow_up });
        self.rebuild_pending_messages();
        self.state.transcript.push_status("Queued message for after compaction");
    }

    /// Take the whole compaction queue — Pi's `flushCompactionQueue` opens with
    /// `const queuedMessages = [...this.compactionQueuedMessages]; this.compactionQueuedMessages =
    /// []; this.updatePendingMessagesDisplay();` (`interactive-mode.ts:4038-4041`), and
    /// `clearAllQueues` (`:3959-3971`) drains the same list for the Escape restore. TUI-031.
    fn take_compaction_queue(&mut self) -> Vec<CompactionQueued> {
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
    fn set_tools_expanded(&mut self, expanded: bool) {
        if !self.state.transcript.set_tool_expanded(expanded) {
            return;
        }
        self.state.transcript.set_bash_expanded(expanded);
        self.state.transcript.push_status(format!(
            "Tool output: {}",
            if expanded { "expanded" } else { "collapsed" }
        ));
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

    /// The `/hotkeys` body — Pi `handleHotkeysCommand` (interactive-mode.ts:6090-6205), verbatim: three
    /// `**Section**` headings each over a `| Key | Action |` GFM table, keys backticked and joined with
    /// ` / ` where a row names two bindings.
    ///
    /// Every cell is `keyDisplayText(id)` = `formatKeys(getKeys(id), { capitalize: true })`
    /// (`keybinding-hints.ts:29-39`), i.e. **all** keys bound to the id joined with `/` and each chord
    /// part title-cased — not just the first key, so a rebind that binds two keys shows both. Unbound
    /// ids render as the empty string exactly as upstream's `keys.length === 0 → ""` does (:30).
    ///
    /// The `**Other**` table is upstream's in full. It used to omit three rows behind a
    /// `[CYRUP-DELTA]` — `app.model.select`, `app.thinking.toggle`, `app.message.copy` — on the
    /// grounds that printing them with an empty key cell would advertise a shortcut no key reaches.
    /// That was legitimate only while the bindings were unported; **TUI-008 ported them**, so the
    /// rows are back at upstream's positions (`:5834`, `:5836`, `:5838`) and the delta is deleted
    /// rather than left to make `/hotkeys` permanently three rows short with nothing tracking it.
    ///
    /// The trailing **Extensions** table (`:6186-6197`) IS built. It is gated on
    /// `if (shortcuts.size > 0)` (`:6189`) — no registered shortcut, no section, never an empty
    /// table — and each row is
    /// ``| `${formatKeyText(key, { capitalize: true })}` | ${shortcut.description ?? shortcut.extensionPath} |``
    /// (`:6193-6197`). cyrup's registry is [`AppState::extension_shortcuts`], the very set the input
    /// router already matches presses against (`:1501`), fed from `ExtensionHost::shortcut_specs()`
    /// — so the section is a read of live state, not a fabricated list. EXT-040: it used to be fed
    /// from `shortcut_keys()`, a bare `Vec<String>`, so `description ?? extensionPath` always fell
    /// through to the id and every Action cell repeated its own Key cell.
    ///
    /// The `newLine` row's `" (Ctrl+Enter on Windows Terminal)"` suffix (:6151) is gated on
    /// `process.platform === "win32"`; it is emitted here under the same `cfg(windows)` condition.
    #[cfg(test)]
    pub(crate) fn hotkeys_markdown_for_test(&self) -> String {
        self.hotkeys_markdown()
    }

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
             | `{select_model}` | Open model selector |\n\
             | `{expand_tools}` | Toggle tool output expansion |\n\
             | `{toggle_thinking}` | Toggle thinking block visibility |\n\
             | `{external_editor}` | Edit message in external editor |\n\
             | `{copy_message}` | Copy last assistant message |\n\
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
            select_model = g(Action::ModelSelect),
            expand_tools = g(Action::ToolsExpand),
            toggle_thinking = g(Action::ThinkingToggle),
            external_editor = g(Action::ExternalEditor),
            copy_message = g(Action::MessageCopy),
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

    /// Install the off-task `/compact` channel and hand back its receiver (TUI-055).
    ///
    /// [`App::run`] calls this once at startup, exactly like [`Self::install_tree_nav_channel`].
    /// Without it `C::Compact` awaits the compaction inline and the run loop is frozen for its whole
    /// duration — see [`Self::compact_tx`] for the measurement that made this necessary. `pub` so a
    /// test can drive the spawned path without standing up a run loop.
    pub fn install_compact_channel(
        &mut self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<CompactOutcome> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<CompactOutcome>();
        self.compact_tx = Some(tx);
        rx
    }

    /// Install the off-task queue-drain channel and hand back its receiver (TUI-092 §5b.1).
    ///
    /// [`App::run`] calls this once at startup, exactly like [`Self::install_compact_channel`].
    /// Without it `Escape` and `Alt+Up` await `AgentSession::drain_queue` on the run loop's own
    /// task, and that call ends in an awaited send into the BOUNDED channel the loop itself is the
    /// only drain of — a self-deadlock, not a slow path. See [`Self::queue_drain_tx`] for the full
    /// cycle. `pub` so a test can drive the spawned path without standing up a run loop.
    pub fn install_queue_drain_channel(
        &mut self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<QueueDrain> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<QueueDrain>();
        self.queue_drain_tx = Some(tx);
        rx
    }

    /// Finish a settled [`QueueDrain`] on the run loop's task (TUI-092 §5b.1).
    ///
    /// Everything here was inline in the `Escape` / `Alt+Up` arms and in
    /// [`Self::begin_tree_navigation`] before the split, in exactly this order — only the
    /// `drain_queue().await` itself moved off-task. `take_compaction_queue` and
    /// `restore_queued_to_editor` are `&mut self` and could not move anyway; the abort is
    /// deliberately kept AFTER the restore, which is Pi's own order
    /// (`restoreQueuedMessagesToEditor({abort: true})`, interactive-mode.ts:2636-2637 — restore
    /// first, "and only then abort"), and safely so because the queues were already taken
    /// atomically by the drain that produced this message.
    ///
    /// Shared by every reason so the interleave cannot drift between them, mirroring
    /// [`Self::apply_compact_outcome`].
    pub fn apply_queue_drain(&mut self, drained: QueueDrain, session: &Arc<AgentSession>) {
        let QueueDrain { steering, follow_up, reason } = drained;
        // TUI-031 — Pi's `clearAllQueues` (`interactive-mode.ts:3959-3971`) drains the SESSION's two
        // queues AND `compactionQueuedMessages`, in `[...steering, ...compactionSteering]` /
        // `[...followUp, ...compactionFollowUp]` order. Without the second source an Escape
        // mid-compaction left the compaction queue holding messages the user believed they had just
        // taken back. `/tree` keeps Pi's narrower `[...steering, ...followUp]` (`:4781-4785`).
        let queued: Vec<String> = if reason == QueueDrainReason::TreeNav {
            steering.into_iter().chain(follow_up).collect()
        } else {
            let compaction = self.take_compaction_queue();
            steering
                .into_iter()
                .chain(compaction.iter().filter(|m| !m.follow_up).map(|m| m.text.clone()))
                .chain(follow_up)
                .chain(compaction.iter().filter(|m| m.follow_up).map(|m| m.text.clone()))
                .collect()
        };
        let restored = self.restore_queued_to_editor(&queued);
        match reason {
            QueueDrainReason::Interrupt => {
                session.abort();
                // Also kill a running bash child (the block was already marked cancelled in
                // `apply_action`); the reader task's terminal `Done` clears `bash_rx`.
                session.abort_bash();
            }
            QueueDrainReason::Dequeue => match restored {
                0 => self.state.transcript.push_status("No queued messages to restore"),
                n => self.state.transcript.push_status(format!(
                    "Restored {n} queued message{} to editor",
                    if n > 1 { "s" } else { "" }
                )),
            },
            // The abort already happened in the spawning task, ahead of `navigate_tree`.
            QueueDrainReason::TreeNav => {}
        }
    }

    /// Install the off-task session-lifecycle channel and hand back its receiver (TUI-092 §5b.2).
    ///
    /// [`App::run`] calls this once at startup, exactly like [`Self::install_compact_channel`].
    /// Without it `/new`, `/reload`, `/import`, `/resume` and `/fork` await their runtime op on the
    /// run loop's own task, where a guest session-lifecycle hook that opens a `ui.*` dialog
    /// deadlocks the loop against itself. See [`Self::lifecycle_tx`]. `pub` so a test can drive the
    /// spawned path without standing up a run loop.
    pub fn install_lifecycle_channel(
        &mut self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<LifecycleOutcome> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<LifecycleOutcome>();
        self.lifecycle_tx = Some(tx);
        rx
    }

    /// Apply a settled session-lifecycle op on the run loop's task (TUI-092 §5b.2).
    ///
    /// On failure or cancellation the OPTIMISTIC `pending_swap_status` is cleared before the status
    /// line is shown: no generation bump follows a failed op, so nothing would ever consume it and
    /// it would otherwise surface against the NEXT swap, attributing this command's message to an
    /// unrelated one.
    ///
    /// Shared by the spawned and inline paths so the two cannot drift, mirroring
    /// [`Self::apply_compact_outcome`].
    pub fn apply_lifecycle_outcome(&mut self, outcome: LifecycleOutcome) {
        let effects = match outcome.0 {
            Ok(effects) => effects,
            Err(status) => {
                self.state.pending_swap_status = None;
                self.state.transcript.push_status(status);
                return;
            }
        };
        if let Some(text) = effects.selected_text {
            self.state.editor.set_text(&text);
        }
        if let Some(agent_dir) = effects.reload_keybindings_in {
            // TUI-051 — Pi's ordering: session reload first, THEN `this.keybindings.reload()`
            // (`interactive-mode.ts:5386`). A malformed document must not wipe the live keymap
            // silently, so the error is surfaced; the maps have already been reset to defaults by
            // then, which is also what pi's replace-semantics `rebuild()` leaves behind.
            // CFG-038 — a rejected ENTRY is reported by id and the rest of the document still
            // applies; only an unusable DOCUMENT keeps the old whole-file wording.
            match self.reload_keybindings_from(&agent_dir) {
                Err(e) => self.state.transcript.push_status(format!("keybindings error: {e}")),
                Ok(issues) => {
                    for issue in issues {
                        self.state.transcript.push_status(format!("keybindings: ignoring {issue}"));
                    }
                }
            }
        }
    }

    /// Run a session-lifecycle op off the run loop's task whenever one is servicing
    /// [`Self::lifecycle_tx`], and finish it through [`Self::apply_lifecycle_outcome`]
    /// (TUI-092 §5b.2).
    ///
    /// The caller sets `pending_swap_status` OPTIMISTICALLY before calling this, because the
    /// runtime's generation bump and this channel are two independent paths: once the op is spawned,
    /// the `session_swapped` arm can fire BEFORE the outcome message arrives, and it reads
    /// `pending_swap_status` to caption the swap. Setting it after the fact would leave that arm
    /// painting an unattributed swap. [`Self::apply_lifecycle_outcome`] clears it if the op turns
    /// out to have failed or been cancelled.
    ///
    /// `None` — an embedder or a test with no run loop — awaits inline, exactly as `/compact` does.
    async fn dispatch_lifecycle(
        &mut self,
        op: impl std::future::Future<Output = LifecycleOutcome> + Send + 'static,
    ) {
        match self.lifecycle_tx.clone() {
            Some(tx) => {
                tokio::spawn(async move {
                    let _ = tx.send(op.await);
                });
            }
            None => {
                let outcome = op.await;
                self.apply_lifecycle_outcome(outcome);
            }
        }
    }

    /// Take-all both session queues and finish through [`Self::apply_queue_drain`], off the run
    /// loop's task whenever one is servicing [`Self::queue_drain_tx`] (TUI-092 §5b.1).
    ///
    /// `None` — an embedder or a test driving the action directly — awaits inline, exactly as
    /// `/compact` and `/tree` do. That is correct there and only there: with no run loop there is no
    /// `events` subscription for `drain_queue`'s fan-out to block against.
    async fn dispatch_queue_drain(
        &mut self,
        session: &Arc<AgentSession>,
        reason: QueueDrainReason,
    ) {
        match self.queue_drain_tx.clone() {
            Some(tx) => {
                let session = session.clone();
                tokio::spawn(async move {
                    let (steering, follow_up) = session.drain_queue().await;
                    let _ = tx.send(QueueDrain { steering, follow_up, reason });
                });
            }
            None => {
                let (steering, follow_up) = session.drain_queue().await;
                self.apply_queue_drain(QueueDrain { steering, follow_up, reason }, session);
            }
        }
    }

    /// Render a settled `/compact` — the summary message on success, Pi's reason string on refusal.
    ///
    /// Shared by the inline and spawned paths so the two cannot drift; the run loop calls it from
    /// the `compact_rx` arm.
    pub fn apply_compact_outcome(&mut self, outcome: CompactOutcome) {
        match outcome {
            // Render the compaction-summary message (`compaction-summary-message.ts`): the
            // `[compaction]` label + `**Compacted from N tokens**` markdown body produced by the
            // op (Pi appends a `CompactionSummaryMessage` after a manual `/compact`).
            Ok(result) => {
                self.state.transcript.push_compaction_summary(result.tokens_before, result.summary);
            }
            // SESS-040 — the failure half of the MANUAL compaction surface, which this path owns
            // in full (see the `[CYRUP-DELTA]` on the `CompactionEnd` arm: the event renders the
            // automatic reasons only, because upstream's `/compact` handler renders nothing at all
            // and cyrup's returns an outcome here instead).
            //
            // Pi's manual `compaction_end` branches are BOTH `showError`
            // (`interactive-mode.ts:3099-3100` aborted, `:3116-3117` `errorMessage`), never the dim
            // `showStatus` (`:3200-3213`); and pi classifies the abort by comparing the thrown
            // message to the bare `"Compaction cancelled"` (`agent-session.ts:1911`) — the same
            // test on the same string, because `SessionServiceError::CompactionCancelled`'s
            // `Display` is that message verbatim (`cyrup-session-svc/src/error.rs:92`).
            //
            // Before this: pressing the Escape the band advertises produced the dim status line
            // `compact error: Compaction cancelled` — a cyrup-invented prefix that reports the
            // user's own deliberate cancel as an error, in the wrong channel. A genuine failure
            // took the same dim line, where pi shows `Compaction failed: …` in error styling (the
            // wrapper its catch applies at `agent-session.ts:1908-1917`, which cyrup already emits
            // verbatim on the `compaction_end` event — this path was the one that disagreed).
            Err(e) if e == "Compaction cancelled" => self.state.transcript.push_error(e),
            Err(e) => self.state.transcript.push_error(format!("Compaction failed: {e}")),
        }
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
        let opts = NavigateTreeOptions {
            summarize,
            custom_instructions,
            ..NavigateTreeOptions::default()
        };
        let entry = cyrup_core::EntryId::from(target.as_str());
        let Some(tx) = self.tree_nav_tx.clone() else {
            // No run loop (unit/embedder driving `execute_command` directly): await inline. Safe
            // for the non-summarizing path, which makes no model call — and safe for the drain,
            // because with no run loop there is no `events` subscription for its fan-out to block
            // against (see [`Self::queue_drain_tx`]).
            // Pi `:4781-4785` — `restoreQueuedMessagesToEditor()` then `session.abort()`.
            if session.is_streaming().await {
                self.dispatch_queue_drain(session, QueueDrainReason::TreeNav).await;
                session.abort();
            }
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
        // TUI-092 §5b.1 — Pi's pre-step (`:4781-4785`, "the user committed to navigating: stop the
        // active response") moves INTO this task rather than staying on the loop. Both of its awaits
        // belong off-task: `is_streaming` is cheap but `drain_queue` ends in an awaited send into
        // this loop's own BOUNDED `events` channel, so awaiting it on the loop is the §5b.1
        // self-deadlock. Sequencing is preserved exactly — the drain and the abort still complete
        // BEFORE `navigate_tree` starts, because they are statements in this same task. Only the
        // editor restore travels back to the loop, over `queue_drain_tx`.
        let drain_tx = self.queue_drain_tx.clone();
        tokio::spawn(async move {
            if session.is_streaming().await {
                let (steering, follow_up) = session.drain_queue().await;
                if let Some(drain_tx) = drain_tx {
                    let _ = drain_tx.send(QueueDrain {
                        steering,
                        follow_up,
                        reason: QueueDrainReason::TreeNav,
                    });
                }
                session.abort();
            }
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
        let opened_at = tokio::time::Instant::now();
        let deadline =
            opts.timeout_ms.filter(|&ms| ms > 0).map(|ms| opened_at + Duration::from_millis(ms));
        if let Some(deadline) = deadline {
            inner.set_title(countdown_title(&base_title, deadline, opened_at));
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

    /// Bind the two INTERACTIVE READ-BACK seams an extension asks the host for — the editor buffer
    /// (SEAM-T02) and the theme family (SEAM-T01).
    ///
    /// Separate from [`Self::install_ui_sinks`] for the reason [`Self::install_overlay_sink`] is:
    /// these are interactive-only in pi. `getEditorText` and all four theme members are bound only
    /// inside `createExtensionUIContext` (`interactive-mode.ts:2393`, `:2401-2417` @v0.84.2); every
    /// other mode gets `noOpUIContext`'s `""` / `[]` / `undefined` /
    /// `{success: false, error: "UI not available"}` (`core/extensions/runner.ts:253`, `:261-263`)
    /// or, for RPC, the same answers hard-coded (`modes/rpc/rpc-mode.ts:248-252`, `:290-300`).
    /// Leaving them unattached elsewhere is what reproduces that, which is why the theme switch
    /// does NOT ride the `UiEffect` sink RPC also drains.
    ///
    /// Both were dead before this call existed: `LiveHostServices` overrode neither `editor_text`
    /// nor any of `theme`/`theme_list`/`theme_by_name`/`set_theme`, so they took the trait defaults
    /// in EVERY mode — including this one, and including for WASM guests, since
    /// `cyrup-ext/src/host/live.rs` forwards `get-editor-text`, `theme-get`, `theme-get-json`,
    /// `theme-list`, `theme-get-by-name` and `theme-set` to exactly these trait methods.
    ///
    /// Must be re-run against every swapped-in session, for the reason [`Self::install_ui_sinks`]
    /// must — and additionally because [`crate::theme_access::TuiThemeAccess`] holds THAT session's
    /// resource snapshot, so a `/reload` that discovers a new theme has to rebuild it.
    pub fn install_extension_readbacks(
        &mut self,
        services: &cyrup_session_svc::LiveHostServices,
        resources: Arc<cyrup_resources::ResourceRegistry>,
        switch: crate::theme_access::ThemeSwitchSink,
    ) {
        services.attach_editor_mirror(self.state.editor_mirror.clone());
        let access = Arc::new(crate::theme_access::TuiThemeAccess::new(
            resources,
            &self.state.theme.name,
            switch,
        ));
        services.attach_theme_access(Arc::clone(&access) as Arc<dyn cyrup_session_svc::ThemeAccess>);
        self.state.theme_access = Some(access);
        // Seed both cells before the first extension can ask, rather than waiting for the first
        // frame: a boot-time `onSessionStart` handler runs before any draw.
        self.publish_extension_readbacks();
    }

    /// Republish the state behind the interactive read-back seams (SEAM-T01/T02).
    ///
    /// Called from [`Self::draw`], which is the one choke point every run-loop arm that can have
    /// changed the editor or the theme passes through — the same reasoning that puts
    /// `flush_terminal_progress` on the frame path. An extension therefore reads the buffer and the
    /// theme AS DRAWN, which is upstream's guarantee too: pi's getters read the live component the
    /// same render tree drew.
    ///
    /// The editor value is [`crate::InputEditor::expanded_text`], not `text()` — pi hands the
    /// extension `getExpandedText?.() ?? getText()` (`interactive-mode.ts:2393` @v0.84.2), i.e. with
    /// `[paste #N …]` markers substituted back to their full content.
    pub fn publish_extension_readbacks(&mut self) {
        self.state.editor_mirror.publish(self.state.editor.expanded_text());
        if let Some(access) = self.state.theme_access.as_ref() {
            access.publish_active(&self.state.theme.name);
        }
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
            UiEffect::SetToolsExpanded { expanded } => self.set_tools_expanded(expanded),
            UiEffect::SetTitle { title } => self.state.terminal_title = Some(title),
            // TUI-033 — an EMPTY string is the clear. Pi's `setHeader(factory)` /
            // `setFooter(factory)` restore the built-in when the factory is `undefined`
            // (`interactive-mode.ts:2245-2254`, `:2273-2290`); cyrup's WIT signature is
            // `set-header(content: string)` (`world.wit:272`), which has no `undefined`, so the
            // empty string is the only value that can carry "restore the built-in".
            UiEffect::SetHeader { content } => {
                self.state.extension_header = (!content.is_empty()).then_some(content)
            }
            UiEffect::SetFooter { content } => {
                self.state.extension_footer = (!content.is_empty()).then_some(content)
            }
            UiEffect::SetWidget { widget } => {
                // Pi keys widgets and UPDATES IN PLACE: `removeExisting(this.extensionWidgetsAbove);
                // removeExisting(this.extensionWidgetsBelow);` then `targetMap.set(key, component)`
                // (`interactive-mode.ts:1926-1958`), and a widget whose `content` is `undefined` is
                // removed rather than re-mounted (`:1935-1938`). TUI-014.
                let parsed = ExtensionWidget::from_json(&widget);
                self.state.extension_widgets.retain(|w| w.key != parsed.key);
                if !parsed.lines.is_empty() {
                    self.state.extension_widgets.push(parsed);
                }
            }
            // TUI-030 — the working-indicator family. All four used to be unreachable: the
            // `HostServices` methods had no `UiEffect` to push, so `LiveHostServices` kept the
            // trait's empty defaults and an extension calling any of them changed nothing, in
            // silence. Pi binds every one to real interactive state (`createExtensionUIContext`,
            // `interactive-mode.ts:2377-2385` @v0.84.2 — every pi line in this arm and in
            // `reset_extension_ui` is that tag, not this file's older @v0.83.0 cites). The state
            // itself lives on [`crate::status_indicator::StatusIndicator`] and [`TranscriptView`],
            // whose setters carry the per-verb citations and the branch logic.
            UiEffect::SetWorkingMessage { message } => {
                self.state.indicator.set_working_message(message)
            }
            UiEffect::SetWorkingVisible { visible } => {
                // `this.session.isStreaming` (`interactive-mode.ts:2098`) — cyrup mirrors it on the
                // status line, set by the `AgentStart`/`AgentEnd` arms.
                let streaming = self.state.status.streaming;
                self.state.indicator.set_working_visible(visible, streaming);
            }
            UiEffect::SetWorkingIndicator { options } => {
                self.state
                    .indicator
                    .set_working_indicator(options.as_ref().map(WorkingIndicator::from_json));
            }
            UiEffect::SetHiddenThinkingLabel { label } => {
                self.state.transcript.set_hidden_thinking_label(label)
            }
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
        self.tick_extension_dialog_countdown_at(tokio::time::Instant::now());
    }

    /// [`Self::tick_extension_dialog_countdown`] with an INJECTED instant — TUI-N09.
    ///
    /// `tests/extension_dialog_countdown.rs` used to `std::thread::sleep(1_100ms)` and then assert
    /// the literal `"Proceed? (2s)"` against a 3 s budget, i.e. a wall-clock-exact assertion with
    /// ~900 ms of scheduler slack in a suite of thousands of tests. A CI or loaded-laptop stall past
    /// 900 ms turned it red with a message pointing at the countdown logic rather than at the
    /// scheduler. Pi drives the same countdown from an injected timer rather than a wall-clock sleep
    /// (`components/countdown-timer.ts:21-30`), and this crate already has the pattern in
    /// `StatusIndicator::retry_message`, which recomputes from a stored `Instant`.
    pub fn tick_extension_dialog_countdown_at(&mut self, now: tokio::time::Instant) {
        let Some((base_title, deadline)) = self
            .state
            .pending_ui_reply
            .as_ref()
            .and_then(|p| p.deadline.map(|d| (p.base_title.clone(), d)))
        else {
            return;
        };
        if now >= deadline {
            if let Some(pending) = self.state.pending_ui_reply.take() {
                let _ = pending.reply.send(default_ui_reply(pending.kind));
            }
            self.close_selector(true);
        } else if let Some(active) = self.state.selector.as_mut() {
            active.inner.set_title(countdown_title(&base_title, deadline, now));
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
                match id.as_str() {
                    "theme" => self.open_selector(SelectorKind::Theme),
                    // TUI-032 — `thinking` opens the picker cyrup already had and could not reach:
                    // Pi's `SelectSubmenu("Thinking Level", …, config.availableThinkingLevels, …,
                    // callbacks.onThinkingLevelChange)` (`settings-selector.ts:591-611`).
                    "thinking" => self.open_selector(SelectorKind::Thinking),
                    // `warnings` is a nested toggle LIST, not a picker — Pi's
                    // `WarningSettingsSubmenu` (`settings-selector.ts:120-160`) is a `SettingsList`
                    // over one item, `anthropic-extra-usage`, whose `onChange` writes straight
                    // through. cyrup reuses the same `SettingsSelector` component and the same
                    // `Apply("id\u{1f}value")` → `AppCommand::ApplySetting` persist path the parent
                    // grid rides, so the nested row writes the global layer with no new plumbing.
                    "warnings" => {
                        let rows = vec![SettingRow::toggle(
                            "warnings.anthropicExtraUsage",
                            "Anthropic extra usage",
                            self.state.warn_anthropic_extra_usage,
                        )
                        .with_description(
                            "Warn when Anthropic subscription auth may use paid extra usage",
                        )];
                        let inner: Box<dyn Selector> =
                            Box::new(SettingsSelector::new("Warnings", rows));
                        self.open_boxed_selector(SelectorKind::Settings, inner);
                    }
                    _ => {}
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
            // TUI-N03 — this arm used to return `None`, so a theme chosen in `/settings` repainted
            // the UI and then died with the process: no `ApplySetting` ever reached the persist arm.
            // Pi distinguishes PREVIEW from CONFIRM — `onThemePreview: (name) =>
            // themeController.preview(name)` versus `onThemeChange: (t) => {
            // this.settingsManager.setTheme(t); void this.themeController.applyFromSettings(); }`
            // (`interactive-mode.ts:4226-4231`) — and cyrup treated confirm as a preview that stuck
            // until exit. Worse in combination with TUI-004: `ThemeController::sync_with_terminal`
            // persists an OSC-11 detection only when `settings.theme` is UNSET, which is exactly the
            // state a never-persisted user choice leaves behind, so the next launch overwrote it.
            //
            // `set_theme` still runs for the immediate repaint; the persist arm (`C::ApplySetting`)
            // pushes the `theme → {value}` status, so this arm no longer pushes its own.
            SelectorKind::Theme => {
                self.set_theme(UiTheme::builtin(value));
                Some(AppCommand::ApplySetting {
                    id: "theme".to_string(),
                    value: value.to_string(),
                })
            }
            // TUI-032 — the level is applied to the SESSION, not written to the settings layer:
            // Pi's `onThinkingLevelChange` is `this.session.setThinkingLevel(level);
            // this.footer.invalidate(); this.updateEditorBorderColor();`
            // (`interactive-mode.ts:4222-4226`). The optimistic local mirror below keeps the footer
            // and the editor rule in lockstep on the frame the picker closes; the session's
            // `ThinkingLevelChanged` event then confirms (or clamps) it.
            SelectorKind::Thinking => {
                self.state.thinking_level = value.to_string();
                self.state.status.set_thinking_level(value);
                // The editor's rule color is the always-visible thinking-level signal (spec/tui/03
                // §3.3) — keep it in lockstep with the selected level.
                self.state.editor.set_thinking_level(value);
                Some(AppCommand::SetThinking(value.to_string()))
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
    /// This is still CALLED inline from `App::run`'s `select!` loop, but it no longer AWAITS
    /// guest-reentrant work there: every arm that drives a session-lifecycle op
    /// (`Reload`/`NewSession`/`Import`/the `Session`/`UserMessage` `ConfirmSelection` switch+fork
    /// paths/`Compact`) now runs that `.await` off-task and applies its `self.state` mutation from a
    /// channel-back arm — [`Self::dispatch_lifecycle`] and [`Self::lifecycle_tx`], which is the
    /// restructuring TUI-092 §5b.2 prescribed and the L4 review §2.1 residual deferred. The hazard
    /// it closed is real and worth keeping in view: those ops dispatch
    /// `HostEvent::Session{Start,Shutdown,BeforeSwitch,BeforeFork,Compact}` to every live
    /// extension's hook (`session.rs` `dispatch_notify`/`vetoed`), a guest SDK hook handler is
    /// handed the SAME `Ctx` a tool/shortcut handler gets (`cyrup-ext-sdk/src/ctx.rs`), and a
    /// `ctx.ui().*` call from inside one parks its task in `block_in_place` until the run loop
    /// answers `ui_rx` — which the run loop could not do while awaiting the op that was waiting for
    /// it. Any NEW arm added here that awaits a runtime or session-lifecycle op must go through
    /// [`Self::dispatch_lifecycle`] for the same reason.
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
                    &self.state.thinking_level,
                    // TUI-036 — `supportsImages` gates the two image rows upstream.
                    self.state.image_renderer.is_graphical(),
                    // TUI-041 — the PROCESS env, the same surface the runtime resolves against.
                    &cyrup_session_svc::EnvVars::from_process(),
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
                // `set_label` takes pi's optional label (`setLabel(entryId, label?)`): an EMPTY
                // edit clears it, which is pi's `value || undefined`.
                session.services().host_services.set_label(
                    &entry_id,
                    (!label.is_empty()).then_some(label.as_str()),
                );
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
            // TUI-032 — the `/settings` → `Thinking level` submenu's confirm. Pi's
            // `onThinkingLevelChange` (`interactive-mode.ts:4222-4226`) calls
            // `session.setThinkingLevel(level)`, which clamps to the model's capabilities and emits
            // `ThinkingLevelChanged`; the footer + editor rule re-color off that event exactly as
            // they do for Shift+Tab.
            C::SetThinking(level) => {
                let parsed = match level.as_str() {
                    "off" => Some(ModelThinkingLevel::Off),
                    "minimal" => Some(ModelThinkingLevel::Minimal),
                    "low" => Some(ModelThinkingLevel::Low),
                    "medium" => Some(ModelThinkingLevel::Medium),
                    "high" => Some(ModelThinkingLevel::High),
                    "xhigh" => Some(ModelThinkingLevel::Xhigh),
                    "max" => Some(ModelThinkingLevel::Max),
                    _ => None,
                };
                match parsed {
                    Some(l) => match session.set_thinking_level(l).await {
                        Ok(applied) => {
                            let label = match applied {
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
                        Err(e) => {
                            self.state.transcript.push_status(format!("thinking error: {e}"))
                        }
                    },
                    None => self
                        .state
                        .transcript
                        .push_status(format!("thinking error: unknown level {level}")),
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
                    // TUI-092 §5b.2 — SPAWNED: `fork` dispatches `HostEvent::SessionBeforeFork` to
                    // every live extension, and a guest hook that opens a `ui.*` dialog parks its
                    // task until this loop answers `ui_rx`. Awaited here that is a self-deadlock.
                    Some(rt) => {
                        self.state.pending_swap_status = Some("forked from message".into());
                        let rt = Arc::clone(rt);
                        self.dispatch_lifecycle(async move {
                            LifecycleOutcome(match rt.fork(entry, ForkPosition::Before).await {
                                Ok(r) if r.cancelled => Err("fork cancelled".to_string()),
                                Ok(r) => Ok(LifecycleEffects {
                                    selected_text: r.selected_text,
                                    ..LifecycleEffects::default()
                                }),
                                Err(e) => Err(format!("fork error: {e}")),
                            })
                        })
                        .await;
                    }
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
                    // TUI-092 §5b.2 — SPAWNED: `switch_session` dispatches
                    // `HostEvent::SessionBeforeSwitch` to every live extension; see the `/fork` arm
                    // above for the deadlock awaiting it here would reintroduce.
                    Some(rt) => {
                        self.state.pending_swap_status = Some(format!("resumed {value}"));
                        let rt = Arc::clone(rt);
                        self.dispatch_lifecycle(async move {
                            LifecycleOutcome(match rt.switch_session(value).await {
                                Ok(r) if r.cancelled => Err("resume cancelled".to_string()),
                                Ok(_) => Ok(LifecycleEffects::default()),
                                Err(e) => Err(format!("resume error: {e}")),
                            })
                        })
                        .await;
                    }
                    None => self
                        .state
                        .transcript
                        .push_status(format!("resume {value} (/reload to switch)")),
                }
            }
            C::DeleteSession(path) => {
                // `/resume` in-list delete (`onDeleteSession`): remove the persisted JSONL via the
                // additive `delete_session_file` seam (refuses the active session).
                // SEAM-063 — the seam now routes through the OS `trash` CLI first and reports
                // WHICH happened, so the status line is pi's own:
                // `result.method === "trash" ? "Session moved to trash" : "Session deleted"`
                // (`modes/interactive/components/session-selector.ts:846` @v0.83.0) and
                // `Failed to delete: ${error}` (`:849`). It used to say "deleted session" whether
                // or not the file went.
                match session.delete_session_file(std::path::Path::new(&path)) {
                    Ok(method) => self
                        .state
                        .transcript
                        .push_status(method.status_message().to_string()),
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("Failed to delete: {e}")),
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
                // TUI-041 — `terminal.clearOnShrink` was in neither the live-apply list nor the
                // grid's resolved read, so it did not take effect until the next launch. Pi's
                // `onClearOnShrinkChange` calls `this.ui.setClearOnShrink(clearOnShrink)`
                // immediately (`interactive-mode.ts`, and `handleReloadCommand` re-applies it at
                // `:5401-5405`). cyrup's counterpart is the reserved idle status band.
                if id == "terminal.clearOnShrink" {
                    self.state.reserve_status_rows = value == "true";
                }
                // TUI-009 — the Escape chain reads the cached copy, so the row has to push into it
                // or a flip would not take effect until the next session bind. Pi re-reads
                // `getDoubleEscapeAction()` inside `onEscape` itself (`:2580`), which is the same
                // liveness.
                if id == "doubleEscapeAction" {
                    self.state.double_escape_action = value.clone();
                }
                // TUI-032 — same reason: the submenu is rebuilt from the cache each time it opens.
                if id == "warnings.anthropicExtraUsage" {
                    self.state.warn_anthropic_extra_usage = value == "true";
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
            // TUI-055 — SPAWNED when a run loop is servicing `compact_tx`. `session.compact` is a
            // 10–20 s provider call; awaiting it here froze the loop for its whole duration, so the
            // `compaction_start` event that arms `IndicatorKind::Compaction` was never read and the
            // 80 ms spinner arm never fired — the screen was simply blank. Pi keeps its band on
            // screen for the entire operation (`interactive-mode.ts:3075-3087`); this is what lets
            // cyrup's reach the frame. The outcome comes back over the channel and is applied by
            // [`Self::apply_compact_outcome`], which the inline fallback below also uses.
            C::Compact(arg) => match self.compact_tx.clone() {
                Some(tx) => {
                    let session = session.clone();
                    tokio::spawn(async move {
                        let outcome = session.compact(arg).await.map_err(|e| e.to_string());
                        let _ = tx.send(outcome);
                    });
                }
                None => {
                    let outcome = session.compact(arg).await.map_err(|e| e.to_string());
                    self.apply_compact_outcome(outcome);
                }
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
                            // TUI-082 — one status string for BOTH branches, pi's:
                            // `Session exported to: ${filePath}` (`interactive-mode.ts:5440`
                            // jsonl / `:5443` html @v0.83.0). The two branches used to disagree
                            // with each other as well as with upstream.
                            let label = arg.unwrap_or_default();
                            self.state
                                .transcript
                                .push_status(format!("Session exported to: {label}"));
                        }
                        Err(e) => self.state.transcript.push_status(format!("export error: {e}")),
                    }
                } else {
                    // Pull the transcript as JSONL (no path ⇒ returned as text), render to HTML, write.
                    match session.export_to_jsonl(None).await {
                        Ok(Some(jsonl)) => {
                            let html = crate::export::session_jsonl_to_html(&jsonl);
                            // TUI-082 — bare `/export` WRITES A FILE. It used to `push_block` the
                            // raw HTML into the transcript, so the single most likely invocation
                            // produced no artifact and flooded scrollback with markup the user
                            // could not do anything with. There is no upstream branch that
                            // corresponds to it: pi always writes and always reports a path
                            // (`handleExportCommand`, `interactive-mode.ts:5434-5447` @v0.83.0).
                            //
                            // The default name is pi's LITERAL mechanism, and it is NOT what
                            // `agent-session.ts:3213`'s doc comment says ("defaults to session
                            // directory") — the code is `exportSessionToHtml`
                            // (`core/export-html/index.ts:274-278` @v0.83.0):
                            //   `outputPath = `${APP_NAME}-session-${basename(sessionFile,".jsonl")}.html``
                            // i.e. a RELATIVE name resolved against the process cwd, not a path
                            // under the session directory. Ported as written, with `cyrup` for
                            // `APP_NAME` (TUI-083 — cyrup has no config-name override).
                            let target = match &arg {
                                Some(path) => Some(std::path::PathBuf::from(path)),
                                None => session.session_file().await.map(|f| {
                                    let stem = f
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| "session".into());
                                    std::path::PathBuf::from(format!("cyrup-session-{stem}.html"))
                                }),
                            };
                            match target {
                                Some(path) => match std::fs::write(&path, &html) {
                                    Ok(()) => self.state.transcript.push_status(format!(
                                        "Session exported to: {}",
                                        path.display()
                                    )),
                                    Err(e) => self
                                        .state
                                        .transcript
                                        .push_status(format!("export error: {e}")),
                                },
                                // pi throws "Cannot export in-memory session to HTML"
                                // (`export-html/index.ts:243-245`) when there is no session file;
                                // an unpersisted session has no basename to build the name from.
                                None => self.state.transcript.push_status(
                                    "export error: cannot export an in-memory session to HTML",
                                ),
                            }
                        }
                        Ok(None) => self.state.transcript.push_status("exported session"),
                        Err(e) => self.state.transcript.push_status(format!("export error: {e}")),
                    }
                }
            }
            // `handleNameCommand`'s SETTER half (`interactive-mode.ts:5645-5653` @v0.83.0).
            // TUI-080: the echo is the STORED name, re-read after the write, not the input — the
            // store normalizes, and echoing the input told the user a name was set that `/resume`
            // would not show. When the two differ upstream warns first (`:5648-5650`), verbatim
            // including the JSON quoting of both values.
            C::SetName(name) => match session.set_session_name(&name).await {
                Ok(()) => {
                    let stored = session.session_name().await;
                    if stored.as_deref() != Some(name.as_str()) {
                        self.state.transcript.push_warning(format!(
                            "Session name was normalized from {} to {}",
                            serde_json::to_string(&name).unwrap_or_else(|_| format!("{name:?}")),
                            match &stored {
                                Some(s) => serde_json::to_string(s)
                                    .unwrap_or_else(|_| format!("{s:?}")),
                                None => "null".to_string(),
                            },
                        ));
                    }
                    let shown = stored.unwrap_or(name);
                    self.state.transcript.push_status(format!("Session name set: {shown}"));
                }
                Err(e) => self.state.transcript.push_status(format!("name error: {e}")),
            },
            // TUI-080 / TUI-084 — the getter, and pi's severity CHANNEL for the usage line: a
            // `showWarning` (`interactive-mode.ts:5638`), not a neutral status, and pi's exact
            // string `Usage: /name <name>` rather than cyrup's `usage: /name <session name>`.
            C::ShowName => match session.session_name().await {
                Some(name) => {
                    self.state.transcript.push_status(format!("Session name: {name}"))
                }
                None => self.state.transcript.push_warning("Usage: /name <name>"),
            },
            C::Copy => match session.last_assistant_text().await {
                Some(text) => {
                    let n = text.chars().count();
                    // Pi's `handleCopyCommand` (interactive-mode.ts:6002-6019) wraps the write in a
                    // `try`: success shows a status, a THROW shows `showError(...)`. Reporting
                    // "copied" unconditionally is what let the old `#[cfg(not(unix))]` no-op tell a
                    // Windows user their message was on the clipboard when nothing had been written.
                    if crate::clipboard::copy_to_clipboard(&text).await {
                        self.state
                            .transcript
                            .push_status(format!("copied last message ({n} chars)"));
                    } else {
                        // The message Pi throws when every branch failed (`clipboard.ts:171-173`),
                        // surfaced through the same error channel as its `showError`.
                        self.state.transcript.push_error("Failed to copy to clipboard");
                    }
                }
                None => self.state.transcript.push_status("no assistant message to copy"),
            },
            C::SessionInfo => {
                // Pi's `/session` renderer (`handleSessionCommand`, interactive-mode.ts:5656-5717
                // @v0.83.0) reads exactly these fields off `getSessionStats()`; cyrup renders them
                // as its own markdown table.
                let stats = session.session_stats().await;
                // PROV-036 / PROV-035 — the two things pi computes here that cyrup did not:
                // `getUsageCostBreakdown(entries)` (`:5665`) and
                // `computeCacheWaste(entries, this.session.modelRuntime)` (`:5660`).
                let breakdown = session.usage_cost_breakdown().await;
                let cache_waste = session.cache_waste().await;
                let mut body = format!(
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
                // `if (stats.cost > 0 || cacheWaste.missedTokens > 0) { … }` (`:5696`). Both
                // additions live under pi's one guard, so a zero-cost session gains no rows.
                if stats.cost > 0.0 || cache_waste.missed_tokens > 0 {
                    // `if (usageBreakdown.length > 1)` (`:5699`) — a single-model session shows the
                    // total only, because a one-row breakdown restates it.
                    if breakdown.len() > 1 {
                        for entry in &breakdown {
                            body.push_str(&format!(
                                "| {} | ${:.3} ({} tokens) |\n",
                                entry.key, entry.cost, entry.tokens
                            ));
                        }
                    }
                    // `if (cacheWaste.missedTokens > 0)` (`:5704-5711`): the `$` figure only when
                    // `missedCost >= 0.0001`, else tokens + miss count alone. The label and the
                    // singular/plural of "miss" are pi's strings verbatim.
                    if cache_waste.missed_tokens > 0 {
                        let miss_label = if cache_waste.miss_count == 1 {
                            "1 miss".to_string()
                        } else {
                            format!("{} misses", cache_waste.miss_count)
                        };
                        let detail = format!("{} tokens, {miss_label}", cache_waste.missed_tokens);
                        body.push_str(&if cache_waste.missed_cost >= 0.0001 {
                            format!(
                                "| Cache Re-billed | ${:.3} ({detail}) |\n",
                                cache_waste.missed_cost
                            )
                        } else {
                            format!("| Cache Re-billed | {detail} |\n")
                        });
                    }
                }
                self.state.transcript.push_block("Session", body);
            }
            // Session-lifecycle ops drive the `AgentSessionRuntime` (arch-11 §3.4): the op rebuilds
            // the active session + bumps the generation, and the run loop's generation-watch arm
            // re-binds the UI (re-subscribe + reset transcript) → `pending_swap_status`. Without a
            // runtime (SDK/embedder), surface the request so the path is real (no silent drop).
            C::NewSession => match runtime {
                // `/new` (handleClearCommand): start a fresh session in the same cwd (Pi `newSession`).
                //
                // TUI-092 §5b.2 — SPAWNED: `new_session` dispatches `HostEvent::SessionShutdown`
                // then `SessionStart` to every live extension; see the `/fork` arm for the
                // self-deadlock awaiting it on this task would reintroduce.
                Some(rt) => {
                    self.state.pending_swap_status = Some("started a new session".into());
                    let rt = Arc::clone(rt);
                    self.dispatch_lifecycle(async move {
                        LifecycleOutcome(match rt.new_session().await {
                            Ok(r) if r.cancelled => Err("new session cancelled".to_string()),
                            Ok(_) => Ok(LifecycleEffects::default()),
                            Err(e) => Err(format!("new session error: {e}")),
                        })
                    })
                    .await;
                }
                None => self.state.transcript.push_status("starting new session…"),
            },
            C::Reload => match runtime {
                // `/reload` (handleReloadCommand): rebuild the active session in place (Pi `reload`,
                // agent-session.ts:2451) — re-reads settings/resources/keybindings, resets the
                // provider, preserves the persisted transcript.
                // TUI-092 §5b.2 — SPAWNED: `reload` dispatches `HostEvent::SessionShutdown` then
                // `SessionStart` to every live extension; see the `/fork` arm for the self-deadlock
                // awaiting it on this task would reintroduce. The keybinding rebuild is `&mut self`
                // and stays on the loop, carried back as an effect so TUI-051's ordering — session
                // reload FIRST, then `this.keybindings.reload()` — is preserved exactly.
                Some(rt) => {
                    let agent_dir = session.services().agent_dir.clone();
                    // TUI-025 — Pi's own sentence, `interactive-mode.ts:5418-5423` @v0.83.0.
                    // cyrup's `"reloaded resources"` said nothing about WHAT was reloaded, and the
                    // `/` menu's own help string for the command was a second, different wording.
                    // The `; saved project trust` variant is TUI-037's — it needs the implicit-trust
                    // write, which lives in `crates/cyrup`.
                    self.state.pending_swap_status = Some(
                        "Reloaded keybindings, extensions, skills, prompts, themes, and \
                         context files"
                            .into(),
                    );
                    let rt = Arc::clone(rt);
                    self.dispatch_lifecycle(async move {
                        LifecycleOutcome(match rt.reload(None).await {
                            Ok(()) => Ok(LifecycleEffects {
                                reload_keybindings_in: Some(agent_dir),
                                ..LifecycleEffects::default()
                            }),
                            Err(e) => Err(format!("reload error: {e}")),
                        })
                    })
                    .await;
                }
                None => self.state.transcript.push_status("reloading resources…"),
            },
            C::Import(p) => match (runtime, p) {
                // `/import <path>` (handleImportCommand): copy + resume a JSONL session (Pi
                // `importFromJsonl`, agent-session-runtime.ts:353).
                //
                // TUI-092 §5b.2 — SPAWNED: `import_from_jsonl` dispatches `HostEvent::SessionStart`
                // to every live extension; see the `/fork` arm for the self-deadlock awaiting it on
                // this task would reintroduce.
                (Some(rt), Some(path)) => {
                    self.state.pending_swap_status = Some(format!("imported session {path}"));
                    let rt = Arc::clone(rt);
                    self.dispatch_lifecycle(async move {
                        LifecycleOutcome(match rt.import_from_jsonl(path, None).await {
                            Ok(r) if r.cancelled => Err("import cancelled".to_string()),
                            Ok(_) => Ok(LifecycleEffects::default()),
                            Err(e) => Err(format!("import error: {e}")),
                        })
                    })
                    .await;
                }
                // TUI-084 — pi's string and pi's CHANNEL: `Usage: /import <path.jsonl>` through
                // `showError` (`interactive-mode.ts:5482` @v0.83.0). cyrup dropped the `.jsonl`
                // constraint, lowercased the word, and routed a real error to the neutral status
                // line, where it is neither coloured nor prefixed as a problem.
                (Some(_), None) => {
                    self.state.transcript.push_error("Usage: /import <path.jsonl>")
                }
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
                // TUI-063 — pi does NOT surface the raw gist URL on its own. It peels the gist ID
                // off the URL `gh` printed and renders the VIEWER link built from it:
                //
                // ```ts
                // const gistUrl = result.stdout?.trim();
                // const gistId = gistUrl?.split("/").pop();                 // :5599
                // if (!gistId) { this.showError("Failed to parse gist ID from gh output"); return; }
                // const previewUrl = getShareViewerUrl(gistId);             // :5606
                // this.showStatus(`Share URL: ${previewUrl}\nGist: ${gistUrl}`);
                // ```
                //
                // (`interactive-mode.ts:5597-5608` @v0.83.0.) cyrup printed only the gist URL, so
                // [`share_viewer_url`] — and therefore `CYRUP_SHARE_VIEWER_URL`, which
                // `cyrup --help` advertises as "Base URL for /share command" — had no consumer at
                // all: setting it changed nothing and said nothing.
                let gist_url = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let gist_id = gist_id_from_url(&gist_url);
                if gist_id.is_empty() {
                    // pi `showError` (`:5601`) — cyrup's `"gist created (no URL returned by gh)"`
                    // covered only the empty-stdout half of the same failure. `showError` builds
                    // `Error: ${errorMessage}` INSIDE itself (`interactive-mode.ts:3878-3882`)
                    // while cyrup's `Entry::Error` renders verbatim, so the prefix is the caller's
                    // to supply here (TUI-062's shape, same as `Warning: `).
                    self.state
                        .transcript
                        .push_error("Error: Failed to parse gist ID from gh output");
                } else {
                    let preview = share_viewer_url(gist_id);
                    self.state
                        .transcript
                        .push_status(format!("Share URL: {preview}\nGist: {gist_url}"));
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
        // TUI-031 — `flushCompactionQueue` (`interactive-mode.ts:4036-4110` @v0.83.0), the last
        // statement of pi's `compaction_end` arm. Runs here because it needs the session; the sync
        // `ingest_event` half only raises the flag.
        if std::mem::take(&mut self.state.compaction_flush_pending) {
            for msg in self.take_compaction_queue() {
                // `if (isExtensionCommand) prompt(text) else if (mode === "followUp")
                // followUp(text) else steer(text)` (`:4055-4062`). Delivered in queue order.
                let ui = UserInput::text(msg.text.clone(), InputSource::Tui);
                if is_extension_command(session, &msg.text) {
                    let _ = session.prompt_accepted(ui).await;
                } else if msg.follow_up {
                    let _ = session.follow_up(ui).await;
                } else {
                    let _ = session.steer(ui).await;
                }
            }
        }
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
                match message_role_from_event(ev).as_deref() {
                    Some("assistant") => self.state.streaming_assistant = true,
                    // Pi `:2915-2918`: `event.message.role === "user"` →
                    // `this.addMessageToChat(event.message)` then
                    // `this.updatePendingMessagesDisplay()`. **This is the only place a user bubble
                    // is written** (TUI-016 / TUI-052) — the submission path deliberately does not,
                    // because the session may queue it instead of sending it, and a queued message
                    // belongs to the pending region until the turn that carries it actually starts.
                    // The `queue_update` that drains the queue arrives around this event, so the row
                    // and the bubble hand off without ever both being on screen.
                    Some("user") => {
                        if let Some(text) = user_message_text_from_event(ev)
                            && !text.trim().is_empty()
                        {
                            self.state.transcript.push_user(text);
                        }
                    }
                    _ => {}
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
            // Pi `case "queue_update"` (`interactive-mode.ts:2888-2891`): rebuild the
            // pending-messages region and re-render. TUI-016 — cyrup used to keep only the COUNT
            // (`status.set_queued`) and, since the fidelity pass deleted the `{n} queued` footer
            // segment, rendered it nowhere; the texts were dropped on the floor here.
            AgentSessionEvent::QueueUpdate { steering, follow_up } => {
                self.state.session_queue = (steering.clone(), follow_up.clone());
                // TUI-031 — the region shows the UNION of the session's queues and the compaction
                // queue, as `getAllQueuedMessages` does (`interactive-mode.ts:3942-3953`).
                self.rebuild_pending_messages();
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
                // X18 — the indicator is a BAND, not a message. `interactive-mode.ts:3075-3087`
                // (citation re-derived at v0.83.0) `case "compaction_start"` calls
                // `showStatusIndicator(new CompactionStatusIndicator(this.ui, event.reason))` at
                // `:3084` and nothing else; `StatusIndicator` extends
                // `Loader` (`status-indicator.ts:9-27`) and is mounted in the fixed status slot, so
                // it disappears the moment `clearStatusIndicator` runs. cyrup was ALSO pushing the
                // identical string into the transcript, which `insert_before` then froze into
                // scrollback as a permanent dim `• Compacting context...` row upstream never writes.
                self.state.indicator.set(IndicatorKind::Compaction, Some(msg));
                // Pi rebinds `defaultEditor.onEscape` to `abortCompaction` here (`:3080-3086`).
                self.state.compacting = true;
            }
            AgentSessionEvent::CompactionEnd { reason, result, aborted, error_message, .. } => {
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
                // TUI-054 — this arm used to end in an unconditional
                // `push_status("compaction complete")`, discarding every field of the event. A
                // refusal ("Nothing to compact") and an outright provider failure (`http 400`) were
                // both followed on screen by a claim that the context had been compacted, which it
                // had not been; the user then reasons about their remaining window from a false
                // premise.
                //
                // Pi branches instead (`interactive-mode.ts:3089-3123` @v0.83.0) and never states
                // success in words: `aborted` ⇒ `showError("Compaction cancelled")` for a manual
                // compaction and `showStatus("Auto-compaction cancelled")` otherwise; `result` ⇒
                // the compaction-summary MESSAGE; `errorMessage` ⇒ `showError(...)` for manual and
                // an error-styled chat line otherwise.
                //
                // **[CYRUP-DELTA]** Pi's `/compact` handler renders nothing at all
                // (`handleCompactCommand`, `:6030-6038`: `await this.session.compact()` inside a
                // `try {} catch {}` whose comment is "Ignore, will be emitted as an event"), so the
                // event is upstream's ONLY renderer. cyrup's `/compact` returns a `CompactOutcome`
                // that [`App::apply_compact_outcome`] renders on the command path — the seam that
                // moved the compaction off the run loop. Both would fire for a manual compaction,
                // so this arm renders the automatic reasons only and leaves `Manual` to the command
                // path — which now renders pi's two manual branches verbatim and through
                // `showError`, so the residual this comment used to record (a manual abort reading
                // `compact error: …` where pi reads `Compaction cancelled`) is closed.
                if !matches!(reason, CompactionReason::Manual) {
                    if *aborted {
                        self.state.transcript.push_status("Auto-compaction cancelled");
                    } else if let Some(msg) = error_message {
                        self.state.transcript.push_error(msg.clone());
                    } else if let Some(res) = result {
                        self.state
                            .transcript
                            .push_compaction_summary(res.tokens_before, res.summary.clone());
                    }
                }
                // TUI-031 — `void this.flushCompactionQueue({ willRetry: event.willRetry })` is the
                // LAST statement of pi's `compaction_end` arm (`interactive-mode.ts:3103`), and it
                // runs on every outcome, aborted and failed included. `ingest_event` cannot await a
                // session call, so the drained batch rides out on `AppState` for the run loop's
                // `ingest_session_event` wrapper to dispatch — see
                // [`App::take_pending_compaction_flush`].
                self.state.compaction_flush_pending = true;
                // Pi restores the previous Escape handler here (`:3094-3097`).
                self.state.compacting = false;
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

// The clipboard WRITE lives in [`crate::clipboard`] — Pi's `utils/clipboard.ts` is likewise its own
// module, and the target-gated pair that used to sit here (a `#[cfg(unix)]` CLI probe beside a
// `#[cfg(not(unix))]` no-op) is documented there as what it replaced.

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

/// The text of the USER message a `message_start` carries — Pi's `event.message` handed to
/// `addMessageToChat` (`interactive-mode.ts:2916`). Returns `None` for any other event or role.
///
/// This is what writes the user bubble into the transcript, and the only thing that does for a live
/// submission: a message the session decides to QUEUE produces no `message_start` until the turn
/// that actually carries it begins, which is precisely the distinction TUI-016/TUI-052 turn on.
fn user_message_text_from_event(ev: &AgentSessionEvent) -> Option<String> {
    let value = serde_json::to_value(ev).ok()?;
    let message = value.get("message")?;
    if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
        return None;
    }
    // Same projection [`assistant_message_from_event`] uses: `cyrup_agent::AgentMessage` is not a
    // direct dependency of this crate, and the serialized form is stable — it is the wire shape the
    // `--json` stream and the session JSONL are both written from.
    let content: Vec<cyrup_core::Content> =
        serde_json::from_value(message.get("content")?.clone()).ok()?;
    Some(crate::transcript::content_text(&content))
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
    thinking_level: &str,
    supports_images: bool,
    env: &cyrup_session_svc::EnvVars,
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
    // TUI-036 — `Show images` / `Image width` are offered ONLY on a terminal that has an image
    // protocol: `// Only show image toggle if terminal supports it` / `if (supportsImages) {
    // items.splice(1, 0, {id:"show-images", …}); items.splice(2, 0, {id:"image-width-cells", …}); }`
    // (`settings-selector.ts:654-671` @v0.83.0), where `supportsImages` comes from
    // `getCapabilities()`. The neighbouring `auto-resize-images` row is deliberately NOT gated — it
    // is spliced at `supportsImages ? 3 : 1` — which is exactly the distinction cyrup lost by
    // pushing all three unconditionally. On a plain xterm the two rows could not change anything,
    // and every row below them sat at a different index from pi's.
    let image_rows: Vec<SettingRow> = if supports_images {
        vec![
            SettingRow::toggle("terminal.showImages", "Show images", eff.show_images())
                .with_description("Render images inline in terminal"),
            SettingRow::choice(
                "terminal.imageWidthCells",
                "Image width",
                eff.image_width_cells().to_string(),
                choices(&["60", "80", "120"]),
            )
            .with_description("Preferred inline image width in terminal cells"),
        ]
    } else {
        Vec::new()
    };
    let mut rows = vec![
        // The "Theme" row opens the theme picker (Pi `SettingItem.submenu` → `ThemeSubmenu`,
        // settings-selector.ts:603-610) — the one in-app path Pi reaches theme switching through.
        SettingRow::submenu("theme", "Theme", current_theme.to_string(), "theme")
            .with_description("Color theme for the interface"),
        SettingRow::toggle("compaction.enabled", "Auto-compact", eff.compaction_enabled())
            .with_description("Automatically compact context when it gets too large"),
    ];
    rows.extend(image_rows);
    rows.extend([
        SettingRow::toggle("images.autoResize", "Auto-resize images", eff.image_auto_resize())
            .with_description("Resize large images to 2000x2000 max for better model compatibility"),
        SettingRow::toggle("images.blockImages", "Block images", eff.block_images())
            .with_description("Prevent images from being sent to LLM providers"),
        SettingRow::toggle("enableSkillCommands", "Skill commands", eff.enable_skill_commands())
            .with_description("Register skills as /skill:name commands"),
        // TUI-041 — these two getters resolve `setting → env → false`
        // (`cyrup-config/src/settings.rs`, `.unwrap_or(env.hardware_cursor)` /
        // `.unwrap_or(env.clear_on_shrink)`, sourced from `CYRUP_HARDWARE_CURSOR`/`PI_HARDWARE_CURSOR`
        // and `CYRUP_CLEAR_ON_SHRINK`/`PI_CLEAR_ON_SHRINK`). The grid used to build both rows against
        // a **default** `EnvVars`, i.e. an env-blind read, while the RUNTIME used
        // `EnvVars::from_process()` (`crates/cyrup/src/main.rs`) — so with either variable set and
        // nothing persisted, `/settings` reported `false` for behaviour that was on and toggling the
        // row looked like a no-op. Pi renders every row from the same resolved value the runtime
        // uses; it has no second, env-blind read path.
        SettingRow::toggle("showHardwareCursor", "Show hardware cursor", eff.show_hardware_cursor(env))
            .with_description("Show the terminal cursor while still positioning it for IME support"),
        SettingRow::toggle("terminal.clearOnShrink", "Clear on shrink", eff.clear_on_shrink(env))
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
        // TUI-032 — the two submenu rows pi ships that cyrup had no counterpart for.
        //
        // `warnings` (`settings-selector.ts:578-590` @v0.83.0): `currentValue: "configure"`,
        // `submenu: … new WarningSettingsSubmenu(currentWarnings, …)` whose single item is
        // `anthropic-extra-usage` (`:130-136`). `warnings.anthropicExtraUsage` is fully parsed and
        // honoured by cyrup (`cyrup-config/src/settings.rs:922-926`) and had **no editor**, so the
        // only way to turn the Anthropic paid-extra-usage warning off was to hand-edit
        // `settings.json`.
        SettingRow::submenu("warnings", "Warnings", "configure", "warnings")
            .with_description("Enable or disable individual warnings"),
        // `thinking` (`:591-611`): `label: "Thinking level"`, a `SelectSubmenu` over
        // `config.availableThinkingLevels`. cyrup already had the picker built —
        // `SelectorKind::Thinking` with a live confirm arm — and no way in: `open_selector` had
        // exactly one call site and it only ever constructed `SelectorKind::Theme`. Shift+Tab
        // cycled blindly with no list of the levels.
        SettingRow::submenu("thinking", "Thinking level", thinking_level.to_string(), "thinking")
            .with_description("Reasoning depth for thinking-capable models"),
    ]);
    rows
}

/// Build the `/settings` grid against default effective settings — the test seam for the two rows
/// TUI-032 adds and the two TUI-036 gates. Production always goes through the `C::OpenSelector`
/// arm, which sources the live session's settings.
#[cfg(test)]
pub(crate) fn settings_rows_for_test_with_images(supports_images: bool) -> Vec<SettingRow> {
    let eff = cyrup_session_svc::EffectiveSettings::default();
    settings_rows(
        &eff,
        "dark",
        &Keymap::default(),
        "medium",
        supports_images,
        &cyrup_session_svc::EnvVars::default(),
    )
}

/// [`settings_rows_for_test_with_images`] with an image-capable terminal.
#[cfg(test)]
pub(crate) fn settings_rows_for_test() -> Vec<SettingRow> {
    settings_rows_for_test_with_images(true)
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

/// Pi's project-trust banner, rebranded (`interactive-mode.ts:3506-3509` @v0.83.0). `${CONFIG_DIR_NAME}`
/// is `.cyrup` here — the directory [`cyrup_config::trust::has_trust_requiring_resources`] probes
/// (`trust.rs:211`) — and the closing `pi` is `cyrup`. TUI-N04.
pub(crate) const PROJECT_UNTRUSTED_WARNING: &str = "This project is not trusted. Project .cyrup resources and packages are ignored. Use /trust to save a trust decision, then restart cyrup.";

/// The environment override for the `/share` viewer base URL — cyrup's rebranding of pi's
/// `PI_SHARE_VIEWER_URL` (`config.ts:506` @v0.83.0), and the name `cyrup --help` already advertises
/// at `crates/cyrup/src/cli.rs:1077` ("Base URL for /share command").
const ENV_SHARE_VIEWER_URL: &str = "CYRUP_SHARE_VIEWER_URL";

/// pi's `DEFAULT_SHARE_VIEWER_URL` (`config.ts:502` @v0.83.0), kept verbatim.
///
/// **Not a rebranding oversight.** The viewer is a pi-operated service that renders any GitHub gist
/// by id, so it works for a cyrup-produced gist unchanged, and this repo already points at that host
/// wherever the service is pi's — `cyrup-provider/src/remote_catalog.rs:68`
/// `DEFAULT_CATALOG_BASE_URL = "https://pi.dev"` and the referer headers at
/// `cyrup-session-svc/src/attribution.rs:82`. Substituting a cyrup host cyrup does not operate would
/// print a dead link on every `/share`.
const DEFAULT_SHARE_VIEWER_URL: &str = "https://pi.dev/session/";

/// pi's `const gistId = gistUrl?.split("/").pop();` (`interactive-mode.ts:5599` @v0.83.0) over the
/// URL `gh gist create` printed (`https://gist.github.com/<user>/<id>`).
///
/// JS `"abc".split("/")` is `["abc"]` and `pop()` returns `"abc"`, so a `gh` that printed a bare id
/// still resolves; only an empty tail (empty stdout, or a trailing `/`) is the failure pi's
/// `if (!gistId)` reports. `rsplit(..).next()` has exactly that shape — `"".rsplit('/').next()` is
/// `Some("")`, not `None`.
pub fn gist_id_from_url(gist_url: &str) -> &str {
    gist_url.rsplit('/').next().unwrap_or_default()
}

/// Port of pi's `getShareViewerUrl(gistId)` (`packages/coding-agent/src/config.ts:504-508`
/// @v0.83.0):
///
/// ```ts
/// export function getShareViewerUrl(gistId: string): string {
///     const baseUrl = process.env.PI_SHARE_VIEWER_URL || DEFAULT_SHARE_VIEWER_URL;
///     return `${baseUrl}#${gistId}`;
/// }
/// ```
///
/// JS `||` treats the empty string as unset, so an exported-but-empty variable falls back to the
/// default rather than producing a bare `#{id}` — hence the `filter(|v| !v.is_empty())`.
///
/// TUI-063: this is the ONLY consumer of [`ENV_SHARE_VIEWER_URL`]. Before it existed, `/share`
/// printed the raw gist URL and the advertised variable was inert.
pub fn share_viewer_url(gist_id: &str) -> String {
    share_viewer_url_from(std::env::var(ENV_SHARE_VIEWER_URL).ok().as_deref(), gist_id)
}

/// [`share_viewer_url`] with the environment already read — the same split
/// [`crate::status::experimental_features_enabled_from`] uses, so the `||` semantics are unit-testable
/// without `std::env::set_var` (`unsafe` in edition 2024, and this crate is `#![forbid(unsafe_code)]`).
#[must_use]
pub fn share_viewer_url_from(env_base: Option<&str>, gist_id: &str) -> String {
    let base = env_base.filter(|v| !v.is_empty()).unwrap_or(DEFAULT_SHARE_VIEWER_URL);
    format!("{base}#{gist_id}")
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

/// The rows an extension widget list occupies, `above` or `below` the editor (TUI-014).
fn widget_rows(state: &AppState, below: bool) -> u16 {
    state
        .extension_widgets
        .iter()
        .filter(|w| w.below == below)
        .map(|w| w.lines.len().min(u16::MAX as usize) as u16)
        .fold(0u16, u16::saturating_add)
}

/// The rows the extension header occupies (TUI-033).
fn header_rows(state: &AppState) -> u16 {
    state
        .extension_header
        .as_deref()
        .map(|h| h.lines().count().min(u16::MAX as usize) as u16)
        .unwrap_or(0)
}

fn region_constraints(state: &AppState, width: u16, avail: u16) -> [u16; 10] {
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
    // TUI-016 — Pi's `pendingMessagesContainer`, docked immediately after `chatContainer` and
    // immediately before `statusContainer` (`interactive-mode.ts:712-714`), i.e. the first
    // live-region row after the message area. Its `VStack` entry is `shrink: 1, minSize: 0`, so it
    // is one of the entries that gives its rows up before the editor does; taking it after the
    // editor/footer floors, the popup, the band and the images reproduces that priority.
    let pending = state.pending_messages.height().min(remaining);
    remaining = remaining.saturating_sub(pending);
    // TUI-014 — Pi's `widgetContainerAbove` / `widgetContainerBelow` are two more `VStack` entries
    // in the dock, at `shrink: 1, minSize: 0`, sitting either side of `editorContainer`
    // (`interactive-mode.ts:876-883`, and the mount order at `:709-719`). Taken after the editor and
    // footer floors for the same reason the pending region is: they yield their rows first.
    let widgets_above = widget_rows(state, false).min(remaining);
    remaining = remaining.saturating_sub(widgets_above);
    let widgets_below = widget_rows(state, true).min(remaining);
    remaining = remaining.saturating_sub(widgets_below);
    // TUI-033 — the custom header replaces `builtInHeader` inside `headerContainer`
    // (`interactive-mode.ts:2273-2290`), which is docked ABOVE the chat container.
    let header = header_rows(state).min(remaining);
    remaining = remaining.saturating_sub(header);
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
    [header, msg, pending, band, images, widgets_above, slot, popup, widgets_below, footer]
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
    let [header_h, msg_h, pending_h, band_h, images_h, wabove_h, slot_h, popup_h, wbelow_h, footer_h] =
        region_constraints(state, area.width, area.height);
    let _ = msg_h; // the message region absorbs the remainder via `Min(0)` below.
    let [
        header_area,
        msg_area,
        pending_area,
        band_area,
        images_area,
        wabove_area,
        slot_area,
        popup_area,
        wbelow_area,
        status_area,
    ] = Layout::vertical([
        // TUI-033 — `headerContainer` is docked above `chatContainer` (`interactive-mode.ts:709`).
        Constraint::Length(header_h),
        // `Min(0)` (not the old `Min(1)`): the empty turn must not balloon the viewport (audit #1).
        Constraint::Min(0),
        Constraint::Length(pending_h),
        Constraint::Length(band_h),
        Constraint::Length(images_h),
        // TUI-014 — `widgetContainerAbove`, immediately before `editorContainer` (`:715-716`).
        Constraint::Length(wabove_h),
        Constraint::Length(slot_h),
        Constraint::Length(popup_h),
        // TUI-014 — `widgetContainerBelow`, immediately after `editorContainer` (`:717`).
        Constraint::Length(wbelow_h),
        Constraint::Length(footer_h),
    ])
    .areas(area);
    if header_h > 0
        && let Some(content) = state.extension_header.as_deref()
    {
        let lines: Vec<Line<'static>> = content
            .lines()
            .map(|l| Line::from(Span::styled(l.to_string(), state.theme.base_style())))
            .collect();
        frame.render_widget(
            Paragraph::new(lines).style(state.theme.base_style()),
            header_area,
        );
    }
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
    if pending_h > 0 {
        // `getAppKeyDisplay("app.message.dequeue")` (`interactive-mode.ts:3987`) — `keyDisplayText`,
        // so ALL bound keys joined with `/` and title-cased (`keybinding-hints.ts:29-40`).
        let dequeue = state
            .keymap
            .keys_label(Action::Dequeue)
            .map(|k| crate::chrome::format_key_text(&k, true));
        state.pending_messages.render(frame, pending_area, &state.theme, dequeue.as_deref());
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
    // Pi gates the hardware cursor globally — `showHardwareCursor` (`tui.ts:344,389-397`), fed from
    // the setting at `interactive-mode.ts:1721-1732` — and cyrup parks that flag on the editor
    // (`editor.rs:277`, "the ONLY component that asks for a cursor position is this editor"), which
    // was true only because the selector half had never been wired. Read before the borrow below.
    // …and only while the slot actually holds focus: a floating overlay draws OVER the live region
    // and captures input, so parking the cursor on a caret the user cannot type into would point at
    // the wrong thing. Pi ties the same decision to its own z-stack (`if (this.overlayStack.length
    // === 0) this.terminal.hideCursor()`, `tui.ts:656`).
    let show_hardware_cursor = state.editor.show_hardware_cursor() && state.overlays.is_empty();
    if let Some(active) = state.selector.as_mut() {
        active.inner.render(frame, slot_area, &state.theme);
        // The selector half of the hardware cursor. While a selector owns the input slot — an
        // extension `ui.input` dialog, `/model`, `/resume`'s search — Pi still positions the real
        // cursor at the typed character, because the focused `Input` inside the dialog emits
        // `CURSOR_MARKER` and `TUI.extractCursorPosition` finds it in the rendered output
        // (`tui.ts:1189-1207`, `input.ts:434`). Cyrup drew the reverse-video caret but left the
        // terminal cursor wherever the previous frame put it, which is what an IME composes
        // against and what a screen reader follows. [`crate::selector::caret_cell`] is the same
        // scan over the rendered CELLS; see its doc for why the reversed caret is the marker.
        if show_hardware_cursor {
            // Bound the buffer borrow to this statement so `set_cursor_position` can take `frame`.
            let caret = crate::selector::caret_cell(frame.buffer_mut(), slot_area);
            if let Some(pos) = caret {
                frame.set_cursor_position(pos);
            }
        }
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
    if wabove_h > 0 {
        render_extension_widgets(frame, wabove_area, state, false);
    }
    if wbelow_h > 0 {
        render_extension_widgets(frame, wbelow_area, state, true);
    }
    // TUI-033 — `setExtensionFooter` CLEARS `footerContainer` and adds the extension component in
    // place of the built-in footer, restoring the built-in when the factory is cleared
    // (`interactive-mode.ts:2245-2254`). So this is a swap, not an overlay.
    match state.extension_footer.as_deref() {
        Some(content) => {
            let lines: Vec<Line<'static>> = content
                .lines()
                .map(|l| Line::from(Span::styled(l.to_string(), state.theme.base_style())))
                .collect();
            frame.render_widget(
                Paragraph::new(lines).style(state.theme.base_style()),
                status_area,
            );
        }
        None => state.status.render(frame, status_area, &state.theme),
    }
    // Floating overlays draw last, on top of the live region, bottom→top (spec/tui/05 §2; arch-10
    // §6.4): each clears its own `Rect` then renders its box.
    for overlay in state.overlays.iter_mut() {
        overlay.render(frame, area, &state.theme);
    }
}

/// Render the attached-image strip inline above the editor (`components/image.ts`): stack each
/// [`ImageBlock`] at its natural cell height, drawing the real protocol when `show_images` is on and a
/// text placeholder when off (spec/tui/06 §6). Honors the live image protocol negotiated at startup.
/// TUI-039 — the terminal-geometry fallback is a **two-step** one upstream, not a constant:
/// `get columns() { return process.stdout.columns || Number(process.env.COLUMNS) || 80; }` and
/// `get rows() { return process.stdout.rows || Number(process.env.LINES) || 24; }`
/// (`packages/tui/src/tui.ts:1730-1736` @v0.83.0). Wherever the ioctl gives no size — a pipe, a CI
/// harness, some container PTY setups — cyrup pinned 80 columns and silently ignored a `COLUMNS=200`
/// the user or harness had set.
///
/// `Number("garbage")` is `NaN`, which is falsy, so pi falls through to the constant; a parse
/// failure, a zero and a negative all do the same here.
fn fallback_columns() -> u16 {
    env_geometry("COLUMNS").unwrap_or(80)
}

/// The `$LINES` half (`tui.ts:1734-1736`). Returned as an `Option` rather than defaulted to pi's
/// bare `24`, because cyrup's one caller has a strictly better last resort available — the live
/// inline-viewport height — and chains onto this.
fn env_rows() -> Option<u16> {
    env_geometry("LINES")
}

/// `Number(process.env.X) || …` — a positive integer, else `None`.
fn env_geometry(var: &str) -> Option<u16> {
    std::env::var(var).ok()?.trim().parse::<u16>().ok().filter(|n| *n > 0)
}

/// Pi's `isExtensionCommand(text)` (`interactive-mode.ts:4022-4030` @v0.83.0): a leading `/`, the
/// word up to the first space, looked up in the extension runner's command registry. An extension
/// command is executed immediately even during a compaction — it is UI work, not a turn — which is
/// why the compaction queue skips it. TUI-031.
fn is_extension_command(session: &AgentSession, text: &str) -> bool {
    let Some(body) = text.strip_prefix('/') else { return false };
    let name = body.split_once(' ').map_or(body, |(n, _)| n);
    session.services().ext_host.registry().has_command(name).unwrap_or(false)
}

/// Draw one placement's extension widgets, in mount order — Pi's `renderWidgets` re-adds every
/// entry of the matching map to its container (`interactive-mode.ts:1920-1960`). Each row is a
/// `Text(line, 1, 0)`, i.e. `paddingX` 1. TUI-014.
fn render_extension_widgets(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    below: bool,
) {
    let lines: Vec<Line<'static>> = state
        .extension_widgets
        .iter()
        .filter(|w| w.below == below)
        .flat_map(|w| w.lines.iter())
        .map(|l| Line::from(Span::styled(format!(" {l}"), state.theme.base_style())))
        .collect();
    frame.render_widget(Paragraph::new(lines).style(state.theme.base_style()), area);
}

fn render_images(frame: &mut Frame, area: ratatui::layout::Rect, state: &AppState) {
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    // TUI-017 — Pi's width rule for the attachment strip is
    // `Math.max(1, Math.min(width - 2, this.options.maxWidthCells ?? 60))`
    // (`packages/tui/src/components/image.ts:65` @v0.83.0), where `maxWidthCells` comes from
    // `terminal.imageWidthCells`. cyrup passed the raw `area.width` with no cap at all, so on a wide
    // terminal the raster was unbounded where Pi stops at 60 cells.
    let max_cells = state.transcript.image_width_cells().max(1);
    let width = area.width.saturating_sub(2).min(max_cells).max(1);
    for block in &state.pending_images {
        if y >= bottom {
            break;
        }
        let want = state.image_renderer.cell_size(block, width).1.max(1);
        let h = want.min(bottom.saturating_sub(y));
        let cell = ratatui::layout::Rect { x: area.x, y, width, height: h };
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
        // TUI-092 — announce a BY-DESIGN block to the input reader's wedge detector for the whole
        // of this call. The run loop stops servicing input here until the user `fg`s us, which is
        // indistinguishable from a wedge by observation alone; the flag is what tells the reader
        // not to escalate a working `Ctrl+Z` into an app exit. Held across the SIGTSTP and the
        // re-entry below, and dropped only once raw mode and the viewport are back.
        let _released = TerminalReleased::enter();
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
        // TUI-092 — a BY-DESIGN block, exactly as in [`Self::suspend`]: `run_editor_over_file` is a
        // blocking `Command::status()` that can own the terminal for minutes, during which the run
        // loop services no input. The flag stops the input reader's wedge detector from escalating
        // a `Ctrl+C` typed inside `$EDITOR` into an app exit. Taken FIRST so the early `return
        // Ok(None)` below is covered too, and dropped by `Drop` on every exit path.
        let _released = TerminalReleased::enter();
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
        // The FIFTH extension seam (SEAM-T01): a guest's `setTheme`. Unlike its `set_*` siblings it
        // does NOT ride `ui_effect_tx` — RPC mode installs that sink, and pi's RPC `setTheme` is a
        // hard-coded failure (`modes/rpc/rpc-mode.ts:298-300`), so routing it there would make the
        // switch succeed in a mode upstream refuses it in. `TuiThemeAccess::set` validates the name
        // against the session's discovered themes first (pi's `loadTheme` throw,
        // `theme/theme.ts:622`) and only a RESOLVED theme reaches this channel. Re-installed on
        // session swap below, for the same reason `ui_tx` is.
        let (theme_switch_tx, mut theme_switch_rx) =
            tokio::sync::mpsc::unbounded_channel::<cyrup_resources::Theme>();
        Self::install_ui_sinks(
            &session.services().host_services,
            ui_tx.clone(),
            ui_effect_tx.clone(),
        );
        Self::install_overlay_sink(&session.services().host_services, overlay_tx.clone());
        self.install_extension_readbacks(
            &session.services().host_services,
            Arc::clone(&session.services().resources),
            theme_switch_tx.clone(),
        );
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
        // TUI-009 — `doubleEscapeAction` had no consumer at all; the Escape chain reads it out of
        // `AppState` because `apply_action` has no session in hand.
        self.state.double_escape_action = eff.double_escape_action();
        // TUI-032 — the `Warnings` submenu is built from this cache.
        self.state.warn_anthropic_extra_usage =
            eff.warnings().anthropic_extra_usage.unwrap_or(true);
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
        // TUI-N04 — Pi's `renderInitialMessages()` runs `renderProjectTrustWarningIfNeeded()`
        // straight after the replay (`interactive-mode.ts:3479-3485`). cyrup's replay is the
        // caller's (`crates/cyrup/src/main.rs` for a `--resume`/`--continue` boot, the
        // `session_swapped` arm below for a `/resume`/`/fork`/`/import`), so the check lands HERE
        // rather than inside `replay_session_*`: pi's call is UNconditional, while cyrup's replay is
        // skipped entirely when `raw_context_messages()` is empty — and a fresh session in an
        // untrusted project is precisely the case that most needs the banner.
        self.render_project_trust_warning_if_needed(&session);
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
        // The `/compact` outcome channel (TUI-055). Installed for exactly the same reason as
        // `tree_nav_rx`: a 10–20 s provider call awaited on THIS task freezes every other arm, so
        // the compaction status band Pi shows for the whole operation never reaches a frame.
        let mut compact_rx = self.install_compact_channel();
        // The queue take-all channel (TUI-092 §5b.1). Installed for the same reason as `compact_rx`
        // and, before it, `tree_nav_rx`: `AgentSession::drain_queue` awaits a send into every live
        // subscription's BOUNDED channel — one of which is the `events` receiver THIS task is the
        // sole drain of — so awaiting it here is a self-deadlock, reachable from an ordinary
        // `Escape` or `Alt+Up` on a busy session.
        let mut queue_drain_rx = self.install_queue_drain_channel();
        // The session-lifecycle channel (TUI-092 §5b.2). Installed for the same reason as the two
        // above: `/new`, `/reload`, `/import`, `/resume` and `/fork` each dispatch a
        // `HostEvent::Session*` hook to every live extension, and a guest that answers one by
        // opening a `ui.*` dialog parks its task until THIS loop services `ui_rx` — which it cannot
        // do while awaiting the op that is waiting for it.
        let mut lifecycle_rx = self.install_lifecycle_channel();
        // The startup package-update check's answer channel, moved out of `self` so the `select!`
        // arm's borrow does not collide with the `&mut self` the other arms take — the same
        // run-loop-local shape as `bash_rx` / `tree_nav_rx`. `None` when the binary wired no channel
        // (offline / `--offline` / `CYRUP_SKIP_VERSION_CHECK`), in which case the arm never resolves.
        let mut package_update_rx = self.package_update_rx.take();
        loop {
            // TUI-092 — surface an arm that blew [`ARM_BUDGET`] on the previous iteration. Recorded
            // by [`ArmGuard`]'s `Drop` (which cannot draw: it runs inside the arm, on a raw-mode
            // terminal the frame owns) and drained HERE, on the first healthy iteration after it,
            // so the diagnostic reaches the user as an ordinary transcript line. `push_warning`
            // queues into `TranscriptView::pending`; every arm below ends in `draw_synchronized`,
            // which paints it.
            if let Ok(mut over) = OVER_BUDGET_ARM.lock()
                && let Some(arm) = over.take()
            {
                self.state.transcript.push_warning(format!(
                    "Warning: run-loop arm `{arm}` exceeded its {ARM_BUDGET:?} budget"
                ));
            }
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
                // REQUIRED, not a micro-optimisation — the same statement `cyrup-tools/src/lock.rs:
                // 178` makes for its own cancel race, and the shape every `select!` in
                // `cyrup-ext/src/host/live.rs` already uses. Without it tokio picks a READY arm at
                // random, so a loop iteration in which teardown was requested AND a keystroke,
                // agent event or ticker is simultaneously ready could service the work arm instead:
                // one more consumed key, one more drawn frame, one more applied event after the
                // token fired. It terminates quickly in expectation, but nothing in the code bounds
                // how much runs after cancellation — and shutdown ordering is exactly what the
                // token is for. `biased;` makes the cancel arm win every such tie, deterministically.
                //
                // Nothing below depends on being polled ahead of the cancel arm: the five ticker
                // arms are all `if`-guarded and idempotent (a skipped tick is re-armed by
                // `MissedTickBehavior::Skip`), and every channel arm keeps its message queued for
                // the next poll. `src/tests/run_loop_cancel_bias.rs` pins this.
                //
                // THE ORDERING RULE IS NOW STRONGER THAN "cancel first" — it is **cancel, then
                // input, then everything else**, and the second half is as load-bearing as the
                // first (TUI-092 §2.5). `biased;` takes the FIRST ready arm, so any arm above
                // `input.next()` that is ready on every poll starves it *permanently*. The spinner
                // ticker is exactly that: armed for the whole of a streaming turn and re-ready
                // every 80 ms (`SPINNER_INTERVAL`, `status_indicator.rs:48`), so as soon as one
                // `draw_synchronized` costs more than a tick — which is what growing transcripts
                // do — the input arm is never reached again and the keyboard dies while the screen
                // keeps animating. Do NOT "tidy" the input arm back down among the tickers.
                biased;
                _ = cancel.cancelled() => break,
                // Input outranks every ticker (TUI-092 §2.5/§5c). `biased;` takes the FIRST
                // ready arm, and the spinner re-arms every `SPINNER_INTERVAL` (80 ms,
                // `status_indicator.rs:48`) for the whole of a streaming turn — so the moment one
                // `draw_synchronized` costs more than a tick, a spinner arm placed ABOVE this one
                // is always ready when the loop comes round and this arm is never polled again.
                // The loop keeps drawing; the keyboard is dead, progressively, exactly as reported.
                // No `.await` has to hang for that to happen.
                //
                // Nothing is lost by demoting the tickers: they are `if`-guarded and idempotent,
                // `MissedTickBehavior::Skip` re-arms a skipped tick, and this arm ends in
                // `draw_synchronized()` anyway — so servicing a key repaints the frame the spinner
                // would have drawn.
                maybe_in = input.next() => {
                    let _arm = ArmGuard::enter("input");
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
                            // TUI-031 — Pi's `clearAllQueues` (`interactive-mode.ts:3959-3971`)
                            // drains the SESSION's two queues AND `compactionQueuedMessages`, in
                            // `[...steering, ...compactionSteering]` /
                            // `[...followUp, ...compactionFollowUp]` order. Without the second
                            // source an Escape mid-compaction left the compaction queue holding
                            // messages the user believed they had just taken back.
                            //
                            // TUI-092 §5b.1 — the take-all is SPAWNED, not awaited here.
                            // `drain_queue` ends in an awaited send into every subscription's
                            // BOUNDED channel, one of which is this loop's own `events` receiver:
                            // awaiting it on this task is a self-deadlock the moment that channel is
                            // full. The interleave, the editor restore and the abort all still
                            // happen, in this exact order, in `apply_queue_drain`.
                            self.dispatch_queue_drain(&session, QueueDrainReason::Interrupt).await;
                        }
                        AppAction::AbortCompaction => {
                            session.abort_compaction();
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
                        AppAction::Submit(text) if session.is_compacting()
                            && !is_extension_command(&session, &text) =>
                        {
                            // TUI-031 — Pi tests `this.session.isCompacting` **before** the
                            // streaming branch (`interactive-mode.ts:2813-2822` @v0.83.0): an
                            // extension command runs immediately, anything else goes to
                            // `queueCompactionMessage(text, "steer")` and returns. cyrup consulted
                            // `is_streaming` only, and the session layer has no compaction guard
                            // either (`AgentSession::prepare` has none, and `is_streaming` reads the
                            // agent snapshot, which compaction does not set — compaction ABORTS the
                            // active run), so a message typed during a 10-20 s compaction was
                            // dispatched as a fresh turn assembled from a context the compaction was
                            // in the middle of rewriting, with no status and no queue.
                            self.queue_compaction_message(text, false);
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
                            // TUI-031 — Pi's follow-up path has the identical compaction gate:
                            // `this.queueCompactionMessage(text, "followUp")`
                            // (`interactive-mode.ts:3744`), ahead of the streaming branch.
                            if session.is_compacting() && !is_extension_command(&session, &text) {
                                self.queue_compaction_message(text, true);
                            } else {
                                let streaming = session.is_streaming().await;
                                // TUI-016 / TUI-052 — no optimistic echo in EITHER branch. The idle
                                // branch is Pi's plain submit, which also writes nothing to the chat
                                // container; the bubble arrives with `message_start`.
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
                            //
                            // TUI-092 §5b.1 — spawned for the same reason as the Escape arm above:
                            // `drain_queue`'s fan-out awaits a send into this loop's own bounded
                            // `events` channel. `apply_queue_drain` does the `clearAllQueues`
                            // interleave, the restore and Pi's status line.
                            self.dispatch_queue_drain(&session, QueueDrainReason::Dequeue).await;
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
                    // TUI-092 — the liveness beacon the input reader's wedge detector watches.
                    // LAST, so it means "serviced", not "started", and deliberately after the draw:
                    // a frame the user never sees is not service. This is the ONLY place it is
                    // bumped — counting loop iterations instead would call a spinner-starved loop
                    // healthy, which is the very state the escape hatch exists for.
                    mark_input_serviced();
                }
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
                    let reframe = matches!(effect, UiEffect::SetWorkingIndicator { .. });
                    self.apply_ui_effect(effect);
                    if reframe {
                        // TUI-030 — pi's `Loader.setIndicator` re-arms its `setInterval` with the
                        // extension's `intervalMs` (`loader.ts:69` → `:77-80` @v0.84.2). cyrup has
                        // no timer per indicator: the run loop's single tick IS the animation
                        // clock, so the
                        // new period has to replace it here, next to the `SetTitle` write above and
                        // for the same reason — it is run-loop state `apply_ui_effect` cannot reach.
                        // Without this a `frames`-heavy indicator with a 40 ms `intervalMs` would
                        // still only be sampled every 80 ms.
                        spinner = tokio::time::interval(self.state.indicator.spinner_period());
                        spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    }
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
                Some(outcome) = compact_rx.recv() => {
                    // A spawned `/compact` settled (TUI-055). The band was cleared by the
                    // `compaction_end` event that preceded this message on the `events` stream.
                    self.apply_compact_outcome(outcome);
                    self.draw_synchronized()?;
                }
                Some(outcome) = lifecycle_rx.recv() => {
                    // A spawned `/new`, `/reload`, `/import`, `/resume` or `/fork` settled
                    // (TUI-092 §5b.2). On success the generation-watch arm has usually already
                    // re-bound the UI off the runtime's bump, captioned by the optimistic
                    // `pending_swap_status`; this arm carries the residue that needs `&mut self`
                    // (the `/fork` editor re-seed, the `/reload` keybinding rebuild) and clears that
                    // caption if the op turned out to have failed.
                    self.apply_lifecycle_outcome(outcome);
                    self.draw_synchronized()?;
                }
                Some(drained) = queue_drain_rx.recv() => {
                    // A spawned take-all settled (TUI-092 §5b.1). Everything that used to follow
                    // `drain_queue().await` inline happens here, in the same order: the
                    // `clearAllQueues` interleave, the editor restore, then the abort.
                    self.apply_queue_drain(drained, &session);
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
                Some(theme) = theme_switch_rx.recv() => {
                    let _arm = ArmGuard::enter("theme_switch");
                    // SEAM-T01 — a guest called `ctx.ui().set_theme(name)` and the name RESOLVED
                    // (`TuiThemeAccess::set` rejected it otherwise, which is where pi's
                    // `{success: false, error}` comes from). Pi's handler does two things
                    // (`interactive-mode.ts:2406-2417` @v0.84.2): `themeController.setThemeName`,
                    // which repaints, and — guarded on the value actually differing —
                    // `settingsManager.setTheme(name)`, which persists. Both are done here, and both
                    // are the SAME pair the `/settings → theme` confirm arm runs
                    // (`SelectorKind::Theme` in `apply_selection`), so an extension switch and a
                    // human switch cannot drift apart.
                    let name = theme.key.as_str().to_string();
                    // `from_theme_data`, not `UiTheme::builtin`: the listing this name came from is
                    // the session's whole discovered set, so a file-backed custom theme is
                    // switchable exactly as upstream's is, and would otherwise silently render as
                    // `dark` (`UiTheme::builtin`'s unknown-name fallback).
                    let projected = UiTheme::from_theme_data(&theme.data, 0);
                    self.set_theme(projected);
                    // [CYRUP-DELTA] vs `interactive-mode.ts:2412`: upstream guards the persist with
                    // `if (this.settingsManager.getTheme() !== themeOrName)`. That guard is a pure
                    // write-avoidance — writing the same value yields the same file — and cyrup
                    // cannot evaluate it correctly here: the session's `SettingsManager` is a boot
                    // snapshot that `ApplySetting` does not refresh (its own arm says the effective
                    // view is re-read on `/reload`), so a stale read would SKIP a write that is
                    // genuinely needed after an earlier switch. Persisting unconditionally is what
                    // the human `/settings → theme` confirm arm already does, for the same reason.
                    self.execute_command(
                        AppCommand::ApplySetting { id: "theme".to_string(), value: name },
                        &session,
                        runtime.as_ref(),
                    )
                    .await;
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
                    let _arm = ArmGuard::enter("tree_nav");
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
                    // TUI-092 — names this arm for its duration; the input reader's wedge detector
                    // reads it, and an overrun is reported on the next healthy iteration. Nothing is
                    // interrupted: a `tokio::time::timeout` here would be inert against the real
                    // wedge (a `block_in_place`d task is never polled again) AND would silently
                    // destroy the compaction queue `ingest_session_event` takes before it awaits.
                    let _arm = ArmGuard::enter("events");
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
                    let _arm = ArmGuard::enter("session_swapped");
                    // A runtime replacement (a `/new`/`/resume`/`/fork`/`/reload`/`/import` op, or a
                    // runtime-side `SessionReplaced`, R-11-021) installed a new active session: drop
                    // the stale subscription, subscribe the NEW session's event stream, and re-bind
                    // the UI. Honors a runtime-driven swap identically to a UI-driven one.
                    if swapped && let Some(rt) = runtime.as_ref() {
                        let new_session = rt.session().await;
                        events = new_session.subscribe();
                        session = new_session;
                        self.rebind_session();
                        // TUI-030 — `rebind_session` ran `reset_extension_ui`, whose
                        // `reset_extension_working_state` drops the outgoing extension's
                        // `setWorkingIndicator` options (pi's `this.setWorkingIndicator()` with no
                        // argument, `interactive-mode.ts:2212` @v0.84.2). Upstream that call
                        // re-arms `Loader`'s own `setInterval` back to `DEFAULT_INTERVAL_MS`
                        // (`loader.ts:67-69` → `:77-80`); cyrup has no per-indicator timer — the run
                        // loop's single tick IS the animation clock — so the period has to be
                        // re-read here, exactly as the `SetWorkingIndicator` effect arm above
                        // re-reads it. Without this a guest that asked for `intervalMs: 1000` would
                        // leave the NEXT session's built-in Braille spinner sampled once a second.
                        spinner = tokio::time::interval(self.state.indicator.spinner_period());
                        spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                        // ...and the two read-back seams (SEAM-T01/T02). The theme half additionally
                        // has to be REBUILT rather than merely re-attached: it answers
                        // `getAllThemes`/`getTheme` out of the session's resource snapshot, and a
                        // swap (`/reload` above all) is exactly when a newly discovered theme
                        // appears — pi re-runs `setRegisteredThemes(resourceLoader.getThemes())` on
                        // the same events (`interactive-mode.ts:1910`, `:5787`).
                        self.install_extension_readbacks(
                            &session.services().host_services,
                            Arc::clone(&session.services().resources),
                            theme_switch_tx.clone(),
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
                        // TUI-009 — same liveness as the rows above: a swap can move the settings
                        // scope, so re-read `doubleEscapeAction` for the swapped-in session.
                        self.state.double_escape_action = eff.double_escape_action();
        // TUI-032 — the `Warnings` submenu is built from this cache.
        self.state.warn_anthropic_extra_usage =
            eff.warnings().anthropic_extra_usage.unwrap_or(true);
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
                        // TUI-N04 — the same statement `renderInitialMessages()` runs after its
                        // replay (`interactive-mode.ts:3485`), and it must run here too: a
                        // `/resume` of a session recorded in a DIFFERENT project swaps the cwd and
                        // the trust decision with it, so the banner's answer changes on the swap.
                        self.render_project_trust_warning_if_needed(&session);
                        // The swapped-in session owns a fresh extension host; re-source its
                        // registered shortcuts (R-08-017) so a post-swap press still routes.
                        // EXT-040: `shortcut_specs()` carries the description `/hotkeys` renders;
                        // `shortcut_keys()` drops it.
                        let shortcuts = session.services().ext_host.shortcut_specs();
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
            cyrup_session_svc::BashOptions {
            exclude_from_context: excluded,
            id: None,
            operations: None,
        };
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

// ------------------------------------------------- TUI-092: the unblockable escape hatch ----
//
// The run loop is one tokio task and the sole drain of the input channel, so any handler that
// stops returning also stops input being read — and the exit keys are downstream of the thing
// that broke. The reader thread below is an `std::thread`: it is the one context in the process
// still running when the loop is wedged, so it is where the escape lives.

/// Bumped by [`App::run`]'s input arm once it has finished servicing one [`InputEvent`]. The reader
/// thread reads it to tell a run loop that is still SERVICING INPUT from one that is merely still
/// ITERATING — a distinction that is not academic here, because `biased;` lets the 80 ms spinner arm
/// starve the input arm indefinitely once a frame costs more than a tick (TUI-092 §2.5, the defect
/// the arm order in `App::run` now fixes). Anything counted outside the input arm would call that
/// state healthy.
///
/// A process-global `static` rather than a threaded-through `Arc`, for the same reason
/// [`crate::terminal_progress`]'s `PROGRESS_ARMED` is one (`terminal_progress.rs:84`): there is
/// exactly one interactive run loop per process, and [`crossterm_input_stream`] has a single
/// production caller (`crates/cyrup/src/main.rs`). Threading a handle through both would change two
/// public signatures — and `EventStream<T>` is `Pin<Box<dyn Stream + Send>>`
/// (`cyrup-core/src/lib.rs:44`), so there is nowhere to smuggle one back — purely to express a
/// singleton. `Relaxed` is sufficient: the reader only asks "is this the value I saw", and never
/// orders other memory against it.
static INPUT_SERVICED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One input event has been fully serviced.
fn mark_input_serviced() {
    INPUT_SERVICED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// How many input events the run loop has serviced, read from the reader thread.
fn input_serviced() -> u64 {
    INPUT_SERVICED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set for as long as the run loop has deliberately handed the terminal to a child process, so the
/// watchdog does not read a by-design block as a wedge.
///
/// A first-party flag owned by the loop, **not** an inference from
/// `crossterm::terminal::is_raw_mode_enabled()`. The inference looks equivalent and is not: it is
/// only true in the steady state, it says nothing on a console editor that keeps raw mode on, and a
/// `Ctrl+Z` suspend re-enables raw mode *before* the loop resumes servicing — so the probe would
/// read "raw, and not servicing" for the whole `fg` resume window and promote a working feature
/// into an app exit.
static TERMINAL_RELEASED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// RAII marker for the two paths that block the run loop by design: [`App::suspend`] and
/// [`App::edit_in_external_editor`]. A guard rather than a pair of calls because both bodies return
/// early on `?`.
struct TerminalReleased;

impl TerminalReleased {
    fn enter() -> Self {
        TERMINAL_RELEASED.store(true, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for TerminalReleased {
    fn drop(&mut self) {
        TERMINAL_RELEASED.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Whether the loop is blocked by design right now.
fn terminal_released() -> bool {
    TERMINAL_RELEASED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The budget an arm body of [`App::run`] is expected to finish inside.
///
/// Sized off the healthy ceiling, not off intuition: the `events` arm can legitimately spend
/// 2 × [`EXTENSION_RENDER_TIMEOUT`] = 4 s on a single event (two `run_renderer` calls per
/// `EntryAppended`). 8 s is twice that, so a working-but-slow guest renderer never files a report.
///
/// This is a REPORTING threshold only. It bounds nothing and cannot promote anything — the escape
/// hatch is driven entirely by unserviced chords, never by elapsed time — so an arm that
/// legitimately runs long (a lifecycle hook fan-out is N extensions × `DEFAULT_INVOKE_BUDGET`,
/// `cyrup-ext/src/dispatch.rs:21`) costs a transcript warning and nothing else.
const ARM_BUDGET: Duration = Duration::from_secs(8);

/// The arm currently executing, and since when — written by [`ArmGuard`], read by the input
/// reader's watchdog so a hard exit can name what the loop was stuck in. `&'static str` only, so the
/// critical section is two assignments and the reader never allocates.
static ACTIVE_ARM: std::sync::Mutex<Option<(&'static str, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// The last arm to exceed [`ARM_BUDGET`], drained by the run loop into the transcript on its next
/// healthy iteration — so the report reaches the user without ever writing to a raw-mode terminal
/// from a `Drop`.
static OVER_BUDGET_ARM: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

/// Marks an arm body as entered for as long as it is held, and records an overrun on the way out.
///
/// A guard rather than a pair of calls precisely because these bodies exit by `break`, `continue`,
/// `return` and `?` as often as they fall off the end — `Drop` covers all five paths.
struct ArmGuard(&'static str, std::time::Instant);

impl ArmGuard {
    fn enter(arm: &'static str) -> Self {
        let now = std::time::Instant::now();
        if let Ok(mut slot) = ACTIVE_ARM.lock() {
            *slot = Some((arm, now));
        }
        Self(arm, now)
    }
}

impl Drop for ArmGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = ACTIVE_ARM.lock() {
            *slot = None;
        }
        if self.1.elapsed() >= ARM_BUDGET
            && let Ok(mut over) = OVER_BUDGET_ARM.lock()
        {
            *over = Some(self.0);
        }
    }
}

/// Unserviced escalate chords that mean "leave now, unconditionally".
///
/// Three, because chord #1 carries its own HEAD meaning and must not be a stage: `Ctrl+C` clears
/// the editor (`Action::Clear`, pi's `handleCtrlC`) and `Ctrl+D` is forward-delete on a non-empty
/// buffer. #2 is the cooperative cancel, #3 is the hard exit — `crates/cyrup/src/signals.rs`'s two
/// deliveries, reproduced on the key path because raw mode means `Ctrl+C` never becomes SIGINT.
const PANIC_PRESSES: u32 = 3;

/// The minimum spacing between chords that [`PANIC_PRESSES`] will count.
///
/// Load-bearing, not a nicety. A terminal's key auto-repeat delivers a held `Ctrl+D` as a stream of
/// ordinary press events at roughly 30 ms intervals — the `KeyEventKind::Press` filter in
/// [`is_escalate_chord`] cannot tell those from real presses, because on unix they ARE real presses
/// (`REPORT_EVENT_TYPES` is not pushed, so `Repeat` never appears). Without a floor, leaning on
/// `Ctrl+D` — which is forward-delete on a non-empty buffer and a delete key inside `/resume` —
/// would spend all three presses in under 100 ms and hard-exit a perfectly healthy app. 250 ms is
/// below any human double-tap (pi's own `Ctrl+C` window is 500 ms) and an order of magnitude above
/// auto-repeat.
const PANIC_MIN_GAP: Duration = Duration::from_millis(250);

/// `Ctrl+C` or `Ctrl+D`, pressed — not auto-repeated, not released.
///
/// The `kind` filter is load-bearing **on Windows**, where crossterm sets `KeyEventKind`
/// unconditionally (`kind` is "Only set if: Unix: `REPORT_EVENT_TYPES` … Windows: always",
/// crossterm 0.29 `event.rs:941-946`), so one physical press would otherwise arrive as a press AND
/// a release and burn two of [`PANIC_PRESSES`]. This check necessarily runs BEFORE [`map_event`],
/// which is where `Release` is normally filtered. On unix `kind` is only populated under
/// `REPORT_EVENT_TYPES`, which [`App::into_stdout`] does not push — it pushes
/// `DISAMBIGUATE_ESCAPE_CODES` alone — so every unix event already arrives as `Press`.
fn is_escalate_chord(ev: &Event) -> bool {
    matches!(
        ev,
        Event::Key(k)
            if k.kind == KeyEventKind::Press
                && k.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d'))
    )
}

/// Leave now, from the one context a wedged run loop cannot block.
///
/// Order is [`App::drain_and_restore`]'s followed by `signals.rs`'s repeat watcher's. The drain must
/// precede the restore — `stdin_is_drainable` (`drain.rs`) requires raw mode to still be on — and it
/// matters here more than anywhere: the user has just pressed the chord three times, and those bytes
/// would otherwise land in the parent shell. `try_lock`, never `lock`: this path must not be able to
/// block on a poisoned or contended mutex.
fn hard_exit_from_reader() -> ! {
    let _ = crate::drain::drain_stdin_before_exit();
    crate::panic_hook::restore_terminal_best_effort();
    // Cooked mode again, so stderr is readable rather than a staircase. This line is the whole
    // diagnostic yield of a wedge: it names the arm that never returned.
    if let Ok(slot) = ACTIVE_ARM.try_lock()
        && let Some((arm, since)) = *slot
    {
        eprintln!("cyrup: run loop wedged in arm `{arm}` for {:?}", since.elapsed());
    }
    cyrup_tools::kill_tracked_detached_children();
    // `ShutdownSignal::Interrupt.exit_code()` — the shell's `128 + SIGINT` (`signals.rs`).
    std::process::exit(130)
}

/// How far up the escalation ladder the unserviced escalate chords have climbed.
///
/// There is no timer in here, deliberately. "Promote once a chord has gone unserviced for N
/// seconds" needs an N above the longest LEGITIMATE inline stall, and no such constant exists: a
/// session-lifecycle hook fan-out is N extensions × `DEFAULT_INVOKE_BUDGET` and a swap replay is M
/// messages × [`EXTENSION_RENDER_TIMEOUT`], both scaling with the user's configuration. Every
/// transition here is instead caused by a chord the run loop was then shown not to have serviced,
/// so the ladder cannot be climbed by a slow-but-working operation no matter how long it takes.
#[derive(Clone, Copy, Debug)]
enum Escalation {
    /// Nothing outstanding.
    Idle,
    /// `presses` chords have been forwarded, each at least [`PANIC_MIN_GAP`] after the last, with
    /// the run loop's serviced count stuck at `serviced` throughout. `last` is the previous counted
    /// chord, for the auto-repeat floor. At `presses == 2` the cooperative cancel has already fired.
    Armed { serviced: u64, last: std::time::Instant, presses: u32 },
}

impl Escalation {
    /// Keep the reader thread alive past `cancel` so the next chord can still reach
    /// [`Self::on_press`] — see the loop condition in [`crossterm_input_stream`].
    const fn holds_open(self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// A chord was just read. The caller forwards it regardless of what this returns.
    fn on_press(self, cancel: &CancelToken) -> Self {
        // Checked here as well as in `tick`: a burst of chords can arrive between two reader
        // iterations, so a by-design block must disarm on the press path too, or the ladder could
        // be climbed from inside `$EDITOR`.
        if terminal_released() {
            return Self::Idle;
        }
        let serviced = input_serviced();
        let now = std::time::Instant::now();
        let Self::Armed { serviced: seen, last, presses } = self else {
            // Chord #1: no evidence of anything yet. Arm and let the normal path handle it.
            return Self::Armed { serviced, last: now, presses: 1 };
        };
        // The loop drained input since the last chord: it IS servicing, and that chord already did
        // its HEAD job (cleared the editor, deleted a char, quit). Back to the bottom of the ladder.
        if seen != serviced {
            return Self::Armed { serviced, last: now, presses: 1 };
        }
        // Auto-repeat floor: a held key is a stream of genuine `Press` events on unix, so only
        // deliberately-spaced chords climb.
        if now.duration_since(last) < PANIC_MIN_GAP {
            return Self::Armed { serviced: seen, last, presses };
        }
        let presses = presses.saturating_add(1);
        if presses >= PANIC_PRESSES {
            // Chord #3 against a loop that has serviced nothing since chord #1.
            hard_exit_from_reader();
        }
        // Chord #2: the cooperative half of `signals.rs`'s escalation. Unblocks the loop's `cancel`
        // arm if it can still run at all; if it cannot, chord #3 leaves.
        cancel.cancel();
        Self::Armed { serviced: seen, last: now, presses }
    }

    /// One reader iteration with no chord. Disarms only — it can never promote.
    fn tick(self) -> Self {
        let Self::Armed { serviced, .. } = self else {
            return self;
        };
        // The loop deliberately released the terminal: `Ctrl+G` external editor
        // ([`App::edit_in_external_editor`], which `restore()`s and then blocks in
        // `Command::status()`) or `Ctrl+Z` suspend ([`App::suspend`], SIGTSTP until `fg`). Both stop
        // the loop servicing input for minutes BY DESIGN, and the chord belongs to the child that
        // now owns the tty.
        //
        // Read from the loop's own flag, NOT from `is_raw_mode_enabled()`: `suspend` re-enables raw
        // mode BEFORE it redraws and resumes servicing, so the probe would report "raw, and not
        // servicing" across the whole `fg` resume. [`TerminalReleased`] is cleared by its `Drop`,
        // i.e. only once the loop is genuinely back.
        if terminal_released() || input_serviced() != serviced {
            return Self::Idle;
        }
        self
    }
}

/// A terminal input stream backed by a blocking `event::read()` reader thread (the async crossterm
/// `EventStream` feature is not enabled in this build; arch-10 §5 fallback). Maps `crossterm::Event`
/// to [`InputEvent`] and forwards over an unbounded channel; stops when `cancel` fires.
///
/// Every event passes through two machines, in this order.
///
/// [`EscapeReassembler`] first — the cyrup half of Pi's `tui/src/stdin-buffer.ts`. crossterm emits a
/// bare `Key(Esc)` and clears its buffer whenever a `read(2)` that did not fill its 1,024-byte
/// buffer ends on `0x1B` (`parse.rs:34-41`), so an escape sequence split at the `ESC` byte reaches
/// the app as `Esc` plus its tail typed as literal characters — and that `Esc` aborts a running turn
/// (`TUI-045`, reproduced live 2026-08-13). The reassembler puts the CSI/SS3 sequence back together
/// and emits the key that was actually pressed.
///
/// Then [`StrayReplyFilter`], the port of Pi's `consumeOsc11BackgroundResponse` guard
/// (`tui/src/tui.ts:788-794`): a terminal that answers the boot-time OSC 11 probe *after*
/// [`crate::terminal_query`]'s deadline would otherwise have its reply decoded by crossterm into
/// keystrokes and typed into the prompt. The filter only ever removes a complete, terminated OSC 11
/// frame; anything it holds is replayed the moment the match fails or the input goes idle — see that
/// module's safety contract.
///
/// Both hold, so both are flushed on the *same* idle tick and in the same order: a lone `Escape`
/// costs one [`HELD_FLUSH_INTERVAL`] in total, not one per machine.
pub fn crossterm_input_stream(cancel: CancelToken) -> EventStream<InputEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InputEvent>();
    std::thread::spawn(move || {
        let mut reassembler = EscapeReassembler::new();
        let mut filter = StrayReplyFilter::new();
        let mut reassembled: Vec<Event> = Vec::new();
        let mut released: Vec<Event> = Vec::new();
        let mut escalation = Escalation::Idle;
        // TUI-092 — NOT `while !cancel.is_cancelled()`. This thread now FIRES that token
        // (`Escalation::on_press`), and the old condition would retire the one reader still able to
        // see the NEXT chord at the exact moment that chord becomes the only way out.
        //
        // `!tx.is_closed()` is the LEADING conjunct, and that ordering is the whole safety
        // argument: the receiver is dropped when `App::run` returns, so a real SIGTERM/SIGHUP
        // teardown (`signals.rs` → the biased cancel arm → `drain_and_restore` → return) still ends
        // this thread even with an escalation armed. `holds_open()` can only extend the reader's
        // life across the window where teardown has been REQUESTED but has not COMPLETED — which is
        // precisely the window a wedged teardown must remain escapable in.
        'reader: while !tx.is_closed() && (!cancel.is_cancelled() || escalation.holds_open()) {
            let wait = if reassembler.is_holding() || filter.is_holding() {
                HELD_FLUSH_INTERVAL
            } else {
                INPUT_POLL_INTERVAL
            };
            match event::poll(wait) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        // TUI-092 — recognised BEFORE `EscapeReassembler`/`StrayReplyFilter`: a
                        // machine mid-hold would otherwise delay the one chord that exists to
                        // escape a wedge by up to `HELD_FLUSH_INTERVAL`, and could swallow it into
                        // a reassembled sequence. Read-only on a borrow; the event is pushed below
                        // unchanged, so neither machine's state is disturbed. It must also run
                        // before the `tx.send` at the foot of this loop, which starts failing the
                        // moment the run loop breaks and drops the receiver.
                        if is_escalate_chord(&ev) {
                            escalation = escalation.on_press(&cancel);
                        }
                        reassembler.push(ev, &mut reassembled);
                        for ev in reassembled.drain(..) {
                            filter.push(ev, &mut released);
                        }
                    }
                    Err(_) => break,
                },
                // Idle: nothing more is coming, so release whatever either machine is holding.
                Ok(false) => {
                    reassembler.flush(&mut reassembled);
                    for ev in reassembled.drain(..) {
                        filter.push(ev, &mut released);
                    }
                    filter.flush(&mut released);
                }
                Err(_) => break,
            }
            // TUI-092 — the disarm tick, on EVERY iteration (at most one `INPUT_POLL_INTERVAL`
            // apart). It never promotes; it only drops a stale ladder once the loop resumes
            // servicing input or announces a by-design block, so a chord pressed before a `Ctrl+Z`
            // is not still armed minutes later.
            escalation = escalation.tick();
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
/// to a burst of raw crossterm events, i.e. [`EscapeReassembler`] then [`StrayReplyFilter`] then
/// [`map_event`], with the idle flush at the end of the burst.
#[cfg(test)]
fn input_pipeline(raw: Vec<Event>) -> Vec<InputEvent> {
    let mut reassembler = EscapeReassembler::new();
    let mut filter = StrayReplyFilter::new();
    let mut reassembled: Vec<Event> = Vec::new();
    let mut released: Vec<Event> = Vec::new();
    let mut out: Vec<InputEvent> = Vec::new();
    for ev in raw {
        reassembler.push(ev, &mut reassembled);
        for ev in reassembled.drain(..) {
            filter.push(ev, &mut released);
        }
        out.extend(released.drain(..).filter_map(map_event));
    }
    // Input has gone quiet: the reader thread's `Ok(false)` poll arm.
    reassembler.flush(&mut reassembled);
    for ev in reassembled.drain(..) {
        filter.push(ev, &mut released);
    }
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

    /// TUI-045's own Verify, at the pipeline level: "drive `input_pipeline` with the two-chunk form
    /// and assert one `Up` arrives rather than `Esc` + `[` + `A`."
    ///
    /// RED before [`crate::escape_reassembly`] existed — this produced exactly `Esc`, `Char('[')`,
    /// `Char('A')`, which at idle types `[A` into the prompt and mid-stream aborts the running turn
    /// (reproduced live on 2026-08-13 with two `tmux send-keys -H` writes 60 ms apart).
    #[test]
    fn an_arrow_key_split_at_the_esc_byte_reaches_the_app_as_one_up() {
        // What crossterm emits when a read ends on `0x1b` and the next read carries `[A`.
        let raw = vec![
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            ch('['),
            Event::Key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
        ];
        let delivered = input_pipeline(raw);
        // `InputEvent` is not `PartialEq`, so match the shape (the same style the sibling test uses).
        match delivered.as_slice() {
            [InputEvent::Key(k)] => {
                assert_eq!(*k, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
            }
            other => panic!("the split arrow must reassemble to one Up, got {other:?}"),
        }

        // And nothing lands in the prompt.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        for ev in &delivered {
            app.handle_input(ev);
        }
        assert_eq!(app.state().editor.text(), "", "no `[A` may be typed into the prompt");
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

    /// **DRIFT-045.** Pi `handleClipboardPaste` (`interactive-mode.ts:2870-2892` @v0.84.2) is
    /// image-first, text-second, and the text read is LAZY — `:2882` returns before the
    /// `readClipboardText()` at `:2884` can run.
    ///
    /// **Red before the fix:** `paste_from_clipboard` did not exist and
    /// `try_paste_clipboard_image_path` consulted the image clipboard only, so the
    /// `image absent + text present` case inserted nothing and returned `false` — the whole defect.
    /// (`grep -rnE 'read_clipboard_text|wl-paste' crates --include='*.rs'` returned 0.)
    #[test]
    fn clipboard_paste_is_image_first_text_second_and_the_text_read_is_lazy() {
        let path = std::env::temp_dir().join("cyrup-clipboard-0198f001-test.png");
        let path_str = path.to_string_lossy().to_string();

        // (a) An image on the clipboard wins, and the TEXT read never happens (`:2882` returns).
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        let text_read = std::cell::Cell::new(false);
        let pasted = app.paste_from_clipboard(
            || Some(path.clone()),
            || {
                text_read.set(true);
                Some("clipboard text".to_string())
            },
        );
        assert!(pasted);
        assert_eq!(app.state().editor.text(), path_str);
        assert!(
            !text_read.get(),
            "pi returns at `:2882`; reading the text clipboard anyway is a second system call \
             upstream never makes and would clobber the path on a clipboard holding both"
        );

        // (b) No image, text present → the text is inserted (`:2884-2888`). This is the case that
        // used to insert nothing at all.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        assert!(app.paste_from_clipboard(|| None, || Some("hello from the clipboard".to_string())));
        assert_eq!(app.state().editor.text(), "hello from the clipboard");

        // (c) Neither → nothing inserted and `false`, so the caller lets Ctrl+V fall through to the
        // editor (a terminal that maps Ctrl+V to a bracketed paste still works).
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        assert!(!app.paste_from_clipboard(|| None, || None));
        assert_eq!(app.state().editor.text(), "");

        // (d) `text || null` (`clipboard.ts:66`): an EMPTY string is falsy upstream, so it must not
        // count as a paste — otherwise Ctrl+V over an empty clipboard would swallow the key.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        assert!(!app.paste_from_clipboard(|| None, || Some(String::new())));
        assert_eq!(app.state().editor.text(), "");
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

/// TUI-063 — `/share`'s viewer link, the only consumer of the `CYRUP_SHARE_VIEWER_URL` that
/// `cyrup --help` advertises at `crates/cyrup/src/cli.rs:1077`.
///
/// The env-var half lives in `tests/share_viewer_url.rs` (its own binary): `std::env::set_var` is
/// `unsafe` in edition 2024 and this crate is `#![forbid(unsafe_code)]`, the same split
/// `experimental_features_enabled_from` + `tests/experimental_marker.rs` already uses.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod share_viewer_url_tests {
    use super::*;

    /// `gistUrl?.split("/").pop()` (`interactive-mode.ts:5599` @v0.83.0) over what `gh gist create`
    /// actually prints, plus the two shapes pi's `if (!gistId)` guard is testing for.
    #[test]
    fn the_gist_id_is_the_last_path_segment_of_gh_s_output() {
        assert_eq!(gist_id_from_url("https://gist.github.com/octocat/abc123def456"), "abc123def456");
        // JS `"abc".split("/")` is `["abc"]`, so `pop()` yields the whole string.
        assert_eq!(gist_id_from_url("abc123def456"), "abc123def456");
        // The two failures `if (!gistId)` catches: nothing on stdout, and a trailing separator.
        assert_eq!(gist_id_from_url(""), "");
        assert_eq!(gist_id_from_url("https://gist.github.com/octocat/"), "");
    }

    /// `${baseUrl}#${gistId}` with `baseUrl = process.env.PI_SHARE_VIEWER_URL || DEFAULT`
    /// (`config.ts:504-508` @v0.83.0). The default is pi's verbatim — see [`DEFAULT_SHARE_VIEWER_URL`].
    #[test]
    fn an_unset_or_empty_override_falls_back_to_pi_s_default_base() {
        assert_eq!(
            share_viewer_url_from(None, "abc123"),
            "https://pi.dev/session/#abc123",
            "`DEFAULT_SHARE_VIEWER_URL` is `https://pi.dev/session/` (`config.ts:502`)"
        );
        assert_eq!(
            share_viewer_url_from(Some(""), "abc123"),
            "https://pi.dev/session/#abc123",
            "JS `||` treats the empty string as unset — an exported-but-empty variable must not \
             produce a bare `#abc123`"
        );
    }

    /// The point of the item: a set variable REACHES the rendered link. Before this landed, `/share`
    /// printed the gist URL alone and the variable had no reader anywhere in `crates/`.
    #[test]
    fn a_set_override_becomes_the_base_of_the_share_url() {
        assert_eq!(
            share_viewer_url_from(Some("https://viewer.example/s/"), "abc123"),
            "https://viewer.example/s/#abc123"
        );
    }
}
