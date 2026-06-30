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
//! # Deferred (would require deps beyond the pre-staged set; see arch-10 §12 + the residual ledger)
//! Streaming assistant delta text is **live**, not deferred: `cyrup-provider` is a direct dependency
//! and [`App::ingest_event`] folds `StreamEvent::TextDelta` into the transcript token-by-token. The
//! remaining gaps are gated on deps not yet ratified into the workspace or on the outer (L7) binary
//! layer: markdown rendering (`pulldown-cmark`), syntax highlighting (`syntect`), inline images
//! (`ratatui-image`), the overlay/z-order + selector-overlay system, the external-editor escape, and
//! the extension-UI command protocol. See `spec/gap-analysis/12-cyrup-tui.md`.
#![forbid(unsafe_code)]

mod app;
mod autocomplete;
mod commands;
mod component;
mod editor;
mod error;
mod fuzzy;
mod keymap;
mod select_list;
mod selector;
mod status;
mod theme;
mod transcript;

pub use app::{crossterm_input_stream, render, App, AppAction, AppState};
pub use autocomplete::{Applied, Autocomplete, Completion, CompletionContext};
pub use commands::{
    CommandRegistry, CommandSource, Dispatch, SlashCommand, BUILTIN_SLASH_COMMANDS, HIDDEN_COMMANDS,
};
pub use component::{Component, InputEvent};
pub use editor::{EditorOutcome, InputEditor};
pub use error::TuiError;
pub use fuzzy::{filter as fuzzy_filter, fuzzy_match, score as fuzzy_score, Match};
pub use keymap::{Action, EditorAction, EditorKeymap, Key, Keymap, SelectAction, SelectKeymap};
pub use select_list::{ColumnLayout, SelectItem, SelectList, DEFAULT_MAX_VISIBLE};
pub use selector::{ListSelector, Selector, SelectorKind, SelectorOutcome};
pub use status::{format_tokens, StatusLine};
pub use theme::{color_of, UiTheme};
pub use transcript::{content_text, Entry, TranscriptView};

/// Re-export the exact crossterm ratatui uses (version-matched; ADR-0001 — never add a direct
/// crossterm dep). Front-ends and tests build key events through this path.
pub use ratatui::crossterm;
