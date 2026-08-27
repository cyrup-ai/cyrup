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
//! - `app/` is that shell as a module tree (see `app/mod.rs`): the `tokio::select!` skeleton
//!   (`run.rs` → `run_arms.rs` → `run_action.rs`) and the frame path (`draw.rs`/`render.rs`/
//!   `layout.rs`), with the session-event fold, command execution and selector plumbing alongside.
//! - [`Component`] is the retained-component contract (state + `render`); built-ins are
//!   [`TranscriptView`], [`InputEditor`], and [`StatusLine`].
//! - `transcript/` is the history view as a module tree (see `transcript/mod.rs`): entries and
//!   their rendering (`entry.rs`/`render.rs`/`message.rs`), the tool blocks (`tool_*.rs`), the
//!   streaming buffer (`stream.rs`) and the active-region render cache (`cache.rs`).
//! - The other multi-file concerns follow the same shape: `editor/` (the input editor), `selector/`
//!   (Pi's in-place editor-swap selector engine) and `markdown/` (the pulldown-cmark walk); the
//!   crate's one visible-width / truncation primitive set is `text_width.rs`.
//! - [`UiTheme`] projects `cyrup-resources` themes (`ResolvedTheme`/`ThemeData`/`builtin_themes`)
//!   onto `ratatui` colors, with a hot-reload hook ([`UiTheme::from_theme_data`]).
//! - [`Keymap`]/[`Action`] resolve global keys (no hardcoded checks, R-10-018).
//! - [`ViewportRenderer`] is the renderer seam (ADR-0005 §B-2, pi `tui.ts:322-330`): the inline
//!   [`App`] satisfies it with four no-ops, and `altscreen/`'s [`AltScreen`] is the second
//!   implementation. It does NOT make the app swappable behind a `dyn` pointer — see the
//!   trait's own scope note. [`App::switch_tui_mode`] is what installs and removes the alternate
//!   screen; [`App`] holds it as an `Option`, and `None` is regular mode.
//!
//! # Built in-crate (no longer deferred)
//! Streaming assistant delta text is **live**: `cyrup-provider` is a direct dependency and
//! [`App::ingest_event`] folds `StreamEvent::TextDelta` into the transcript token-by-token. Markdown
//! ([`markdown`]) + syntax highlight, the **`!`/`!!` bash-execution** block ([`bash`]), the **floating
//! overlay z-stack + hotkeys popup** ([`overlay`]), active-region **page-scroll**, and the
//! **`$EDITOR`/`$VISUAL` external-editor escape** (`Ctrl+G`) are all built and TestBackend-covered.
//! So are inline images (`image.rs` — `ratatui-image` TTY-protocol probe + half-block fallback +
//! `show_images` placeholder), the **`@`-mention `fd` file search** (`autocomplete.rs`),
//! **grapheme-cluster** editor motion (`editor/` via `unicode-segmentation`), and the bespoke
//! **scoped-models checkbox+reorder** selector ([`CheckboxSelector`] + [`ModelsKeymap`]).
//!
//! # Remaining gaps (see `docs/gap-analysis/07-cyrup-tui.md`)
//! Wrap-aware/sticky-column vertical motion + large-paste markers, the message-component/chrome tail,
//! clipboard-image paste + base64 message-image decode — plus the outer-layer ext-UI command protocol.
#![forbid(unsafe_code)]
// `clippy::indexing_slicing` (workspace-denied) only fires on slice/array receivers; `str` range
// indexing is exempt and panics on a non-char-boundary byte offset. Use `.get(..)`/`split_once`/
// `strip_prefix`/`split_at_checked` instead. Test modules opt out alongside the other four.
#![deny(clippy::string_slice)]

mod altscreen;
mod ansi;
mod app;
mod auth_select;
mod autocomplete;
mod bash;
mod chrome;
mod clipboard;
mod commands;
mod component;
mod config_selector;
mod diff;
mod drain;
mod editor;
mod error;
mod escape_reassembly;
mod export;
mod extension_editor;
mod footer_data;
mod fuzzy;
mod image;
mod keyboard_protocol;
mod keymap;
mod login_dialog;
mod markdown;
mod model_selector;
mod native_modifiers;
mod oauth_selector;
mod open_browser;
mod osc;
mod overlay;
mod panic_hook;
mod pending_messages;
mod resume_hint;
mod select_list;
mod selector;
mod session_search;
mod session_selector;
mod settings_selector;
mod startup;
mod startup_selector;
mod status;
mod status_indicator;
mod stray_reply;
mod terminal_progress;
mod terminal_query;
mod terminal_title;
mod text_input;
mod text_width;
mod theme;
mod theme_access;
mod tmux;
mod transcript;
mod tree_selector;
mod user_message_selector;

/// The crate's headless render / keymap / selector suites. They lived one-file-per-binary under
/// `tests/`; compiled here they are a single unit-test target instead of ~77 linked processes.
#[cfg(test)]
mod tests;

pub use altscreen::{AltScreen, PointerOutcome, ScrollbarMode, TuiRenderMode, ViewportRenderer};
pub use app::{
    crossterm_input_stream, extension_render, gist_id_from_url, reanchor_inline_region, render,
    share_viewer_url, share_viewer_url_from, should_honor_extension_shutdown, tree_node_from_dag,
    App, AppAction, AppCommand, AppState, CompactionQueued, ExtensionWidget, InlineBackend,
    LifecycleEffects, LifecycleOutcome, LoginProviderSource, MainScreenRenderState, ModeSwitch,
    ModeSwitchOptions, QueueDrain, QueueDrainReason, RebuildBackend, TreeNavMsg,
};
pub use auth_select::{
    format_auth_selector_provider_type, format_status_indicator, login_selector_rows,
    provider_display_name, provider_rows, status_indicator_runs, AuthState, StatusTone,
};
pub use autocomplete::{
    list_files as mention_list_files, mention_autocomplete, mention_query, Applied, Autocomplete,
    Completion, CompletionContext,
};
pub use bash::{BashExecution, BashStatus, PREVIEW_LINES};
pub use overlay::{Overlay, OverlayOutcome};
pub use panic_hook::{install_panic_hook, restore_terminal_best_effort};
pub use resume_hint::{
    format_resume_command, quote_if_needed, resume_hint_line, ResumeTarget, APP_NAME,
};
pub use commands::{
    dynamic_commands_from_catalog, dynamic_commands_from_catalog_gated, CommandRegistry,
    CommandSource, Dispatch, SlashCommand, BUILTIN_SLASH_COMMANDS, HIDDEN_COMMANDS,
};
pub use component::{Component, InputEvent};
pub use diff::render_diff;
pub use drain::{
    drain_count, drain_input, drain_stdin_before_exit, InputDrain, DRAIN_IDLE, DRAIN_MAX,
};
pub use chrome::{
    compact_hint_height, compact_hints, compact_onboarding, format_key_text, key_hint_line,
    key_hint_spans, render_compact_hints, truncate_to_visual_lines, BorderedLoader, VisualTruncate,
    COMPACT_HINT_ROWS, STARTUP_ONBOARDING,
};
pub use editor::{CommandHighlight, EditorOutcome, InputEditor, VisualLine};
pub use footer_data::{
    find_git_paths, resolve_branch as resolve_git_branch, FooterGitBranch, GitPaths,
    POLL_INTERVAL as GIT_BRANCH_POLL_INTERVAL,
};
pub use export::session_jsonl_to_html;
pub use error::TuiError;
pub use fuzzy::{filter as fuzzy_filter, fuzzy_match, score as fuzzy_score, Match};
pub use image::{
    cached_capabilities, detect_capabilities, detect_capabilities_from,
    detect_capabilities_on_platform, hyperlinks_supported, image_fallback_text,
    reset_capabilities_cache, seed_capabilities, seed_hyperlink_support, set_capabilities,
    ImageBlock, ImageProtocol, ImageRenderer, TerminalCapabilities,
};
pub use keyboard_protocol::{
    current as keyboard_protocol, decide as decide_keyboard_protocol, find_kitty_flags,
    is_negotiation_prefix, negotiate as negotiate_keyboard_protocol, parse_negotiation_sequence,
    set_current as set_keyboard_protocol, KeyboardProtocol, NegotiationSequence,
    KITTY_FLAGS_QUERY, MODIFY_OTHER_KEYS_DISABLE, MODIFY_OTHER_KEYS_ENABLE, NEGOTIATION_TIMEOUT,
};
pub use keymap::{
    Action, AltScreenAction, AltScreenKeymap, AutocompleteAction, AutocompleteKeymap, EditorAction,
    EditorKeymap, Key, KeybindingIssue, Keymap, ModelsAction, ModelsKeymap, SelectAction,
    SelectKeymap, SessionAction, SessionKeymap, TreeAction, TreeKeymap,
};
pub use login_dialog::{
    notify_auth_dialog, show_auth_prompt, LoginDialog, LoginFinished, LoginLineKind, LoginUiMsg,
    TuiAuthInteraction,
};
pub use markdown::{
    render as render_markdown, render_with_hyperlink_support as render_markdown_with_hyperlinks,
    render_with_text_color as render_markdown_with_text_color, trim_partial_closing_fence,
};
pub use model_selector::{ModelEntry, ModelSelector};
pub use native_modifiers::{
    clear_native_modifier_probe, host_platform, is_apple_terminal_session,
    is_native_modifier_pressed, normalize_native_shift_enter, rescue_native_shift_enter,
    set_native_modifier_probe, should_detect_native_shift_enter,
    ModifierKey, ModifierProbe,
};
pub use select_list::{ColumnLayout, SelectItem, SelectList, DEFAULT_MAX_VISIBLE};
pub use config_selector::{
    ConfigKind, ConfigRow, ConfigScope, ConfigSelector, ConfigToggle, ConfigWriteScope,
    ProjectOverrideState,
};
pub use oauth_selector::{OAuthMode, OAuthSelector};
pub use selector::{
    input_line_spans, search_input_spans, CheckboxSelector, ListSelector, Selector, SelectorKind,
    SelectorOutcome, INPUT_PROMPT, SCOPED_MODELS_ALL,
};
pub use user_message_selector::{UserMessageRow, UserMessageSelector};
pub use session_search::{
    filter_and_sort as filter_and_sort_sessions, match_text as match_session_text,
    parse_search_query, NameFilter, ParsedSearchQuery, QueryMode, SearchRow, SearchToken, SortMode,
    TokenKind,
};
pub use session_selector::{SessionRow, SessionSelector, SessionSelectorOutcome};
pub use settings_selector::{SettingRow, SettingsSelector, TrustSelector, FIELD_SEP};
pub use startup_selector::run_startup_selector;
pub use status::{
    experimental_features_enabled, experimental_features_enabled_from, format_tokens, StatusLine,
};
pub use text_input::TextInputSelector;
pub use tree_selector::{FilterMode, TreeEntryRole, TreeKind, TreeNode, TreeSelector};
pub use status_indicator::{
    IndicatorKind, StatusIndicator, WorkingIndicator, SPINNER_FRAMES, SPINNER_INTERVAL,
};
pub use startup::{
    build_startup_lines, display_path, extension_diagnostics, resource_diagnostics,
    DiagnosticCollision, DiagnosticSeverity, StartupDiagnostic, StartupLine, StartupReport,
    StartupRole, StartupSpan,
};
pub use terminal_query::{
    find_cell_size_report, find_color_scheme_report, find_osc11_background_color,
    parse_cell_size_report, parse_color_scheme_report, parse_osc11_background_color,
    saw_device_attributes, NoTerminalProbe, StdinTerminalProbe, TerminalProbe, CELL_SIZE_QUERY,
    CELL_SIZE_TIMEOUT, COLOR_SCHEME_QUERY, OSC11_BACKGROUND_QUERY,
};
pub use terminal_progress::{
    progress_is_armed, write_terminal_progress, TerminalProgress,
    TERMINAL_PROGRESS_ACTIVE_SEQUENCE, TERMINAL_PROGRESS_CLEAR_SEQUENCE,
    TERMINAL_PROGRESS_KEEPALIVE,
};
pub use terminal_title::{session_terminal_title, APP_TITLE};
pub use theme::{
    color_of, detect_terminal_background_from_env, detect_terminal_background_theme,
    detect_terminal_theme_for_auto, rgb_to_256, theme_for_rgb, BackgroundTheme, ColorMode,
    DetectionConfidence, TerminalTheme, TerminalThemeDetection, TerminalThemeSource,
    ThemeController, ThinkingTheme, UiTheme,
};
pub use tmux::{
    check_keyboard_setup as check_tmux_keyboard_setup, in_tmux,
    keyboard_warning as tmux_keyboard_warning, EXTENDED_KEYS_FORMAT_WARNING,
    EXTENDED_KEYS_OFF_WARNING, TMUX_QUERY_TIMEOUT,
};
pub use transcript::{
    content_text, parse_skill_block, thinking_text, Entry, ParsedSkillBlock, ResultImage,
    TranscriptView, DEFAULT_IMAGE_WIDTH_CELLS, HIDDEN_THINKING_LABEL,
};

/// Re-export the exact crossterm ratatui uses (version-matched; ADR-0001 — never add a direct
/// crossterm dep). Front-ends and tests build key events through this path.
pub use ratatui::crossterm;
