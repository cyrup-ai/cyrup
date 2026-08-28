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
//!
//! ## Historical paths: `app.rs:NNNN` citations elsewhere in this crate
//! This tree is what `crates/cyrup-tui/src/app.rs` became. Commit `40821ed`
//! (`refactor(cyrup-tui): split the 10.6k-line app.rs into the app/ module tree`) replaced that
//! single 10,607-line file with the modules listed below, and **`src/app.rs` no longer exists**.
//! Any surviving `app.rs:NNNN` citation in a comment anywhere in this crate is therefore
//! **historical**: it names a line of the pre-split file, not of anything at HEAD.
//!
//! Those citations are annotated here rather than churned one by one — the precedent, and the
//! reason for it, is `docs/gap-analysis/07-cyrup-tui.md:161-162`: re-pointing a line number
//! mechanically produced pointers that matched TEXT but not MEANING, because the citations were
//! written across dozens of revisions of `app.rs` and no single revision maps them all. Re-point
//! one only when it misdirects about a **symbol**, and then only by reading the target.
//!
//! This does **not** apply to the crate's upstream citations (`interactive-mode.ts:3500`,
//! `editor.ts:114`, `truncate.ts:177`, …). Those name a line in a pinned external tag of the
//! TypeScript this crate ports and stay verbatim.

mod action;
mod backend;
mod bash_spawn;
mod channels;
mod crossterm;
mod draw;
mod event_extract;
mod events;
mod events_fold;
mod extension_ui;
mod execute;
mod execute_misc;
// The per-model-thinking helpers `execute.rs` and `selectors.rs` reach for by path
// (`crate::app::thinking_level_str`, `crate::app::CLEAR_MODEL_THINKING`).
pub(crate) use execute_misc::{CLEAR_MODEL_THINKING, thinking_level_str};
mod execute_session;
mod hotkeys;
#[path = "extension_render.rs"]
mod extension_render_impl;
mod input;
mod input_reader;
mod layout;
mod login;
mod mode_switch;
mod outcome;
#[path = "render.rs"]
mod render_impl;
mod run;
mod run_action;
mod run_arms;
mod selectors;
mod session_bind;
mod settings_rows;
mod share;
mod shell;
mod submit;
mod tree_nav;
mod state;

pub use action::{AppAction, AppCommand, CycleDirection};
pub use backend::{InlineBackend, RebuildBackend, reanchor_inline_region};
pub(crate) use bash_spawn::{BashMsg, spawn_session_bash};
pub(crate) use event_extract::{
    assistant_message_from_event, context_usage_may_have_moved,
    custom_message_from_event, custom_message_text, edit_preview, message_role_from_event,
    model_entries, read_clipboard_image_to_temp, stop_reason_notice, tool_result_usage_from_event,
    truncate_summary, user_message_text_from_event,
};
#[cfg(any(test, feature = "scrollback-accumulator"))]
pub(crate) use event_extract::line_text;
pub use extension_render_impl::{
    extension_render, extension_render_entry, extension_render_message,
    should_honor_extension_shutdown,
};
pub(crate) use extension_render_impl::custom_entry_type;
pub use input_reader::{crossterm_input_stream, write_terminal_title};
pub(crate) use input_reader::{
    ARM_BUDGET, ArmGuard, OVER_BUDGET_ARM, TerminalReleased, mark_input_serviced,
};
#[cfg(test)]
pub(crate) use input_reader::{
    ACTIVE_ARM, Escalation, PANIC_MIN_GAP, is_escalate_chord, input_serviced, map_event,
    map_event_on,
};
pub(crate) use layout::{
    live_region_height, max_visible_editor_lines, region_constraints,
};
/// ADR-0005 §Decision B-14 — the live renderer switch (pi `switchTuiMode`,
/// `interactive-mode.ts:842-891` @v0.84.3). Re-exported from `lib.rs` because the eventual caller
/// is the composition root in the `cyrup` crate (`crates/cyrup/src/interactive.rs`), not this one.
/// Both directions of the switch are implemented — [`App::switch_tui_mode`] enters and leaves
/// ADR-0005 §B-3's `AltScreen` — and nothing inside this crate calls it: its two callers are the
/// composition root (merging `--tui-mode` with the persisted `settings.tuiMode`) and the
/// `/settings` `tui-mode` row (`app/settings_rows.rs`). See [`mode_switch`]'s module doc.
pub use mode_switch::{MainScreenRenderState, ModeSwitch, ModeSwitchOptions};
pub use outcome::{
    CompactOutcome, CompactionQueued, ExtensionWidget, LifecycleEffects, LifecycleOutcome,
    LoginProviderSource, QueueDrain, QueueDrainReason, TreeNavMsg,
};
pub(crate) use settings_rows::PROJECT_UNTRUSTED_WARNING;
pub(crate) use settings_rows::{
    format_saved_trust, model_thinking_summary_for_count, parse_setting_value, session_label,
    settings_rows, system_time_nanos,
};
#[cfg(test)]
pub(crate) use settings_rows::{settings_rows_for_test, settings_rows_for_test_with_images};
pub use share::{ShareMsg, gist_id_from_url, share_viewer_url, share_viewer_url_from};
pub(crate) use share::ShareInFlight;

pub use render_impl::render;
pub(crate) use render_impl::{env_rows, fallback_columns, is_extension_command};
pub(crate) use crossterm::resolve_external_editor;
#[cfg(test)]
pub(crate) use crossterm::run_editor_over_file;
pub use tree_nav::tree_node_from_dag;
pub use state::{ActiveSelector, AppState, ShortcutSpec, SwapCaption};
pub(crate) use state::{
    BRANCH_SUMMARY_CUSTOM, BRANCH_SUMMARY_NONE, BRANCH_SUMMARY_YES, PendingTreeNav,
    PendingUiReply, countdown_title, default_ui_reply,
};

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
use futures::{FutureExt, StreamExt};
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
use crate::altscreen::AltScreen;
use crate::keymap::{
    Action, AltScreenKeymap, EditorAction, Key, KeybindingIssue, Keymap, ModelsKeymap, SelectAction,
    SelectKeymap, SessionKeymap, TreeKeymap,
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

/// The interactive front-end over an injectable backend.
pub struct App<B: Backend> {
    terminal: Terminal<B>,
    state: AppState,
    /// The ADR-0005 §B-3 alternate-screen renderer while fullscreen mode is live — `None` for
    /// regular mode, which is every session that never calls [`App::switch_tui_mode`].
    ///
    /// An `Option` field rather than a `Box<dyn ViewportRenderer>` swap, for the reason
    /// [`crate::ViewportRenderer`]'s own scope note gives: `ratatui::Terminal` exposes no consuming
    /// accessor, so a backend cannot be moved out of one `Terminal` into another and the second
    /// renderer is built over `RebuildBackend::rebuild()` instead — the same mechanism
    /// `App::draw`'s `resize_viewport` already uses (`app/draw.rs:113-118`). `App` therefore keeps
    /// its inline `Terminal` untouched for the whole excursion and puts it back into use by
    /// dropping this.
    ///
    /// Every consumer reads it through one of four seams and never inline: [`App::render_mode`] and
    /// [`App::renderer_mut`] (`app/mode_switch.rs`), the fullscreen branch of [`App::draw`]
    /// (`app/draw.rs`), and the §B-9 key offer in [`App::handle_input`] (`app/input.rs`).
    altscreen: Option<AltScreen<B>>,
    /// The `tui.altScreen.*` table §B-9's key routing resolves against — the eight ids of pi's
    /// `keybindings.ts:159-209`, held here rather than on [`AppState`] because it is the one keymap
    /// whose resolution is gated on which renderer is live
    /// ([`AltScreenKeymap::action_in_mode`]).
    ///
    /// **Not yet reachable from a `keybindings.json`.** The five other maps are merged by
    /// [`App::load_keybindings_json`] (`app/shell.rs:159-172`) and reset by
    /// [`App::reload_keybindings_from`]; adding this one is a line in each, and until then the
    /// table is upstream's defaults for the session's life.
    alt_keymap: AltScreenKeymap,
    /// The current inline-viewport height (the live region's content height). Recomputed each
    /// [`draw`](Self::draw); the viewport is rebuilt only when it changes (audit #1).
    viewport_height: u16,
    /// Grow-only high-water mark for the live-region height WHILE a turn is active (streaming or a
    /// live `!` bash block). During a turn the viewport pins at this floor and stops tracking
    /// per-tool content churn, so the terminal is reconstructed (`resize_viewport` → `reanchor_inline`)
    /// only on GENUINE geometry changes (terminal resize, editor multi-line growth, selector/overlay/
    /// band), the two idle↔active transitions per turn, and COMMIT-FLUSH frames — never per tool
    /// event. That stable height is what lets ratatui cell-diff the message churn inside a fixed
    /// viewport with no full repaint, eliminating the per-tool-call FLICKER.
    ///
    /// The floor is RELEASED back down to the remaining content height on any frame that will flush a
    /// commit (TUI-090): a mid-turn commit moves content OUT of the live region, and a viewport left
    /// pinned above the remainder makes `Terminal::insert_before` send the flush directly to native
    /// scrollback invisibly (ratatui-core `inline.rs:66-67` — "if the viewport takes up the whole
    /// screen, all lines will be inserted directly into the scrollback buffer"). The release is what
    /// keeps pi's scroll-into-history VISIBLE. Reset to `0` the instant the turn goes idle so the
    /// region collapses back to the compact editor/footer (the void-fix is preserved).
    live_floor: u16,
    /// Where a spawned `/tree` navigation posts its outcome back to the run loop. Installed by
    /// [`App::install_tree_nav_channel`], which [`App::run`] calls once at startup. `None` when no
    /// run loop is present (an embedder or a test driving `execute_command` directly), in which case
    /// [`App::begin_tree_navigation`] falls back to awaiting the navigation inline — correct for a
    /// non-summarizing navigation (no model call, so no abort to deliver and nothing to keep the
    /// loop free for) and the only thing a caller without a loop can do.
    tree_nav_tx: Option<tokio::sync::mpsc::UnboundedSender<TreeNavMsg>>,
    /// Where a spawned `/share` gist upload posts its outcome back to the run loop. Installed by
    /// [`App::install_share_channel`], which [`App::run`] calls once at startup — the same shape as
    /// [`Self::tree_nav_tx`].
    ///
    /// Installing it is what makes `/share`'s [`crate::chrome::BorderedLoader`] *work at all*: pi
    /// mounts the loader and then awaits `gh` while its render loop keeps running
    /// (`session-share.ts:152-186`), whereas an inline `.output().await` on this loop's own task
    /// reaches no other `select!` arm for the whole upload — so no frame is ever produced with the
    /// loader set, the 80 ms spinner never advances, and the `escape/ctrl+c cancel` hint the loader
    /// renders is not even read until `gh` has already exited.
    ///
    /// `None` (an embedder or a test driving `execute_command` directly) falls back to awaiting the
    /// upload inline, exactly as `/tree` does: with no run loop there is nothing to keep free and no
    /// keystroke could be delivered anyway.
    share_tx: Option<tokio::sync::mpsc::UnboundedSender<ShareMsg>>,
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
