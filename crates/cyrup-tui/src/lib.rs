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
//! # Deferred (would require deps beyond the pre-staged set or larger surface; see arch-10 §12)
//! - **Incremental assistant delta text from events.** `AgentSessionEvent`'s message/delta payloads
//!   are `cyrup_agent::AgentMessage` / `cyrup_provider::StreamEvent`, which `cyrup-tui` does not
//!   depend on; only the *terminal* assistant message is recovered (see [`App::ingest_event`]). The
//!   neutral [`TranscriptView::push_assistant_delta`] API is the streaming seam meanwhile.
//! - Markdown rendering, syntax highlighting, inline images (need `pulldown-cmark`/`syntect`/
//!   `ratatui-image`), overlays/z-order, stackable autocomplete, the external-editor escape, and the
//!   extension-UI command protocol — all out of scope for this pass.
#![forbid(unsafe_code)]

mod app;
mod component;
mod editor;
mod error;
mod keymap;
mod status;
mod theme;
mod transcript;

pub use app::{crossterm_input_stream, render, App, AppAction, AppState};
pub use component::{Component, InputEvent};
pub use editor::{EditorOutcome, InputEditor};
pub use error::TuiError;
pub use keymap::{Action, Key, Keymap};
pub use status::StatusLine;
pub use theme::{color_of, UiTheme};
pub use transcript::{content_text, Entry, TranscriptView};

/// Re-export the exact crossterm ratatui uses (version-matched; ADR-0001 — never add a direct
/// crossterm dep). Front-ends and tests build key events through this path.
pub use ratatui::crossterm;
