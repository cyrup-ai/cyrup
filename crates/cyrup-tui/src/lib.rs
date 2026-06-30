//! cyrup-tui — the terminal UI front-end (arch-10; conformance: func-10; binds ADR-0001).
//!
//! ratatui + crossterm: an **inline viewport** (NOT the alternate screen) over an injectable
//! `ratatui::backend::Backend`, a retained component layer over ratatui's immediate mode, a
//! transcript/history view driven by `AgentSessionEvent`, a hand-rolled multi-line input editor, a
//! themed status line, synchronized-output frames, and a `tokio::select!` event loop over terminal
//! input + the agent event stream + theme hot-reload + cancellation.
//!
//! # Layering
//! - [`App`] is the shell, generic over the backend (`TestBackend` for tests, `CrosstermBackend`
//!   for the binary). `render` is pure (`state -> frame`) so tests are deterministic.
//! - [`Component`] is the retained-component contract (state + `render`); built-ins are
//!   [`TranscriptView`], [`InputEditor`], and [`StatusLine`].
//! - [`UiTheme`] projects `cyrup-resources` themes (`ResolvedTheme`/`ThemeData`/`builtin_themes`)
//!   onto `ratatui` colors, with a hot-reload hook ([`UiTheme::from_theme_data`]).
//! - [`Keymap`]/[`Action`] resolve global keys (no hardcoded checks, R-10-018).
//!
//! # Built in-crate (no longer deferred)
//! Streaming assistant delta text is **live**: `cyrup-provider` is a direct dependency and
//! [`App::ingest_event`] folds `StreamEvent::TextDelta` into the transcript token-by-token. Markdown
//! ([`markdown`]) + syntax highlight, the **`!`/`!!` bash-execution** block ([`bash`]), the **floating
//! overlay z-stack + hotkeys popup** ([`overlay`]), active-region **page-scroll**, and the
//! **`$EDITOR`/`$VISUAL` external-editor escape** (`Ctrl+G`) are all built and TestBackend-covered.
//!
//! # Remaining gaps (see `spec/gap-analysis/12-cyrup-tui.md`)
//! The six unbuilt data-bound selectors + bespoke selector layouts, `@`-mention `fd` search, inline
//! images (`ratatui-image`), wrap-aware/sticky-column vertical motion + large-paste markers, the
//! message-component/chrome tail, and HTML export — plus the outer-layer ext-UI command protocol and
//! grapheme-cluster motion (dep-gated).
#![forbid(unsafe_code)]

mod app;
mod autocomplete;
mod bash;
mod commands;
mod component;
mod diff;
mod editor;
mod error;
mod fuzzy;
mod keymap;
mod markdown;
mod overlay;
mod select_list;
mod selector;
mod status;
mod status_indicator;
mod theme;
mod transcript;

pub use app::{crossterm_input_stream, render, App, AppAction, AppCommand, AppState};
pub use autocomplete::{Applied, Autocomplete, Completion, CompletionContext};
pub use bash::{BashExecution, BashStatus, PREVIEW_LINES};
pub use overlay::{HotkeysOverlay, Overlay, OverlayOutcome};
pub use commands::{
    CommandRegistry, CommandSource, Dispatch, SlashCommand, BUILTIN_SLASH_COMMANDS, HIDDEN_COMMANDS,
};
pub use component::{Component, InputEvent};
pub use diff::render_diff;
pub use editor::{EditorOutcome, InputEditor};
pub use error::TuiError;
pub use fuzzy::{filter as fuzzy_filter, fuzzy_match, score as fuzzy_score, Match};
pub use keymap::{Action, EditorAction, EditorKeymap, Key, Keymap, SelectAction, SelectKeymap};
pub use markdown::{render as render_markdown, trim_partial_closing_fence};
pub use select_list::{ColumnLayout, SelectItem, SelectList, DEFAULT_MAX_VISIBLE};
pub use selector::{ListSelector, Selector, SelectorKind, SelectorOutcome};
pub use status::{format_tokens, StatusLine};
pub use status_indicator::{IndicatorKind, StatusIndicator, SPINNER_FRAMES, SPINNER_INTERVAL};
pub use theme::{color_of, UiTheme};
pub use transcript::{content_text, Entry, TranscriptView};

/// Re-export the exact crossterm ratatui uses (version-matched; ADR-0001 — never add a direct
/// crossterm dep). Front-ends and tests build key events through this path.
pub use ratatui::crossterm;
