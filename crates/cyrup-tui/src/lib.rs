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
//! # Built this round (L6 round-4)
//! Inline images (`image.rs` — `ratatui-image` TTY-protocol probe + half-block fallback + `show_images`
//! placeholder), the **`@`-mention `fd` file search** (`autocomplete.rs`), **grapheme-cluster** editor
//! motion (`editor.rs` via `unicode-segmentation`), and the bespoke **scoped-models checkbox+reorder**
//! selector ([`CheckboxSelector`] + [`ModelsKeymap`]).
//!
//! # Remaining gaps (see `spec/gap-analysis/12-cyrup-tui.md`)
//! The five unsourced data-bound selectors + their bespoke layouts (tree/session/settings/trust/oauth),
//! wrap-aware/sticky-column vertical motion + large-paste markers, the message-component/chrome tail,
//! clipboard-image paste + base64 message-image decode — plus the outer-layer ext-UI command protocol.
#![forbid(unsafe_code)]

mod app;
mod auth_select;
mod autocomplete;
mod bash;
mod chrome;
mod commands;
mod component;
mod diff;
mod editor;
mod error;
mod export;
mod fuzzy;
mod image;
mod keymap;
mod markdown;
mod overlay;
mod select_list;
mod selector;
mod session_search;
mod session_selector;
mod settings_selector;
mod startup_selector;
mod status;
mod status_indicator;
mod theme;
mod transcript;
mod tree_selector;

pub use app::{crossterm_input_stream, render, App, AppAction, AppCommand, AppState};
pub use auth_select::{provider_display_name, provider_rows, AuthState};
pub use autocomplete::{
    list_files as mention_list_files, mention_autocomplete, mention_query, Applied, Autocomplete,
    Completion, CompletionContext,
};
pub use bash::{BashExecution, BashStatus, PREVIEW_LINES};
pub use overlay::{HotkeysOverlay, Overlay, OverlayOutcome};
pub use commands::{
    CommandRegistry, CommandSource, Dispatch, SlashCommand, BUILTIN_SLASH_COMMANDS, HIDDEN_COMMANDS,
};
pub use component::{Component, InputEvent};
pub use diff::render_diff;
pub use chrome::{
    compact_hints, format_key_text, key_hint_line, key_hint_spans, render_compact_hints,
    truncate_to_visual_lines, BorderedLoader, VisualTruncate,
};
pub use editor::{EditorOutcome, InputEditor, VisualLine};
pub use export::session_jsonl_to_html;
pub use error::TuiError;
pub use fuzzy::{filter as fuzzy_filter, fuzzy_match, score as fuzzy_score, Match};
pub use image::{ImageBlock, ImageRenderer};
pub use keymap::{
    Action, EditorAction, EditorKeymap, Key, Keymap, ModelsAction, ModelsKeymap, SelectAction,
    SelectKeymap, TreeAction, TreeKeymap,
};
pub use markdown::{render as render_markdown, trim_partial_closing_fence};
pub use select_list::{ColumnLayout, SelectItem, SelectList, DEFAULT_MAX_VISIBLE};
pub use selector::{
    CheckboxSelector, ListSelector, Selector, SelectorKind, SelectorOutcome, SCOPED_MODELS_ALL,
};
pub use session_search::{
    filter_and_sort as filter_and_sort_sessions, match_text as match_session_text,
    parse_search_query, NameFilter, ParsedSearchQuery, QueryMode, SearchRow, SearchToken, SortMode,
    TokenKind,
};
pub use session_selector::{SessionRow, SessionSelector, SessionSelectorOutcome};
pub use settings_selector::{SettingRow, SettingsSelector, TrustSelector, FIELD_SEP};
pub use startup_selector::run_startup_selector;
pub use status::{format_tokens, StatusLine};
pub use tree_selector::{FilterMode, TreeKind, TreeNode, TreeSelector};
pub use status_indicator::{IndicatorKind, StatusIndicator, SPINNER_FRAMES, SPINNER_INTERVAL};
pub use theme::{color_of, UiTheme};
pub use transcript::{
    content_text, parse_skill_block, Entry, ParsedSkillBlock, TranscriptView,
};

/// Re-export the exact crossterm ratatui uses (version-matched; ADR-0001 — never add a direct
/// crossterm dep). Front-ends and tests build key events through this path.
pub use ratatui::crossterm;
