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
mod submenu_selector;
mod terminal_progress;
mod terminal_query;
mod terminal_title;
mod text_input;
mod text_width;
mod theme;
mod theme_access;
mod thinking_selector;
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
    App, AppAction, AppCommand, AppState, CompactionQueued, ExtensionWidget, ImplicitTrustReload,
    InlineBackend, LifecycleEffects, LifecycleOutcome, LoginProviderSource, MainScreenRenderState,
    ModeSwitch, ModeSwitchOptions, QueueDrain, QueueDrainReason, RebuildBackend, TreeNavMsg,
    crossterm_input_stream, extension_render, gist_id_from_url, implicit_trust_after_reload,
    reanchor_inline_region, render, share_viewer_url, share_viewer_url_from,
    should_honor_extension_shutdown, tree_node_from_dag,
};
pub use auth_select::{
    AuthState, StatusTone, format_auth_selector_provider_type, format_status_indicator,
    login_selector_rows, provider_display_name, provider_rows, status_indicator_runs,
};
pub use autocomplete::{
    Applied, ArgumentSources, Autocomplete, Completion, CompletionContext, ExtensionCompletions,
    LoginProviderArgument, ModelArgument, list_files as mention_list_files, mention_autocomplete,
    mention_query,
};
pub use bash::{BashExecution, BashStatus, PREVIEW_LINES};
pub use chrome::{
    BorderedLoader, COMPACT_HINT_ROWS, STARTUP_ONBOARDING, VisualTruncate, compact_hint_height,
    compact_hints, compact_onboarding, format_key_text, key_hint_line, key_hint_spans,
    render_compact_hints, truncate_to_visual_lines,
};
pub use commands::{
    ArgumentCompleter, BUILTIN_SLASH_COMMANDS, CommandRegistry, CommandSource, Dispatch,
    HIDDEN_COMMANDS, SlashCommand, dynamic_commands_from_catalog,
    dynamic_commands_from_catalog_gated,
};
pub use component::{Component, InputEvent};
pub use config_selector::{
    ConfigKind, ConfigRow, ConfigScope, ConfigSelector, ConfigToggle, ConfigWriteScope,
    ProjectOverrideState,
};
pub use diff::render_diff;
pub use drain::{
    DRAIN_IDLE, DRAIN_MAX, InputDrain, drain_count, drain_input, drain_stdin_before_exit,
};
pub use editor::{CommandHighlight, EditorOutcome, InputEditor, VisualLine};
pub use error::TuiError;
pub use export::session_jsonl_to_html;
pub use footer_data::{
    FooterGitBranch, GitPaths, POLL_INTERVAL as GIT_BRANCH_POLL_INTERVAL, find_git_paths,
    resolve_branch as resolve_git_branch,
};
pub use fuzzy::{Match, filter as fuzzy_filter, fuzzy_match, score as fuzzy_score};
pub use image::{
    ImageBlock, ImageProtocol, ImageRenderer, TerminalCapabilities, cached_capabilities,
    detect_capabilities, detect_capabilities_from, detect_capabilities_on_platform,
    detect_capabilities_with_overrides, hyperlinks_supported, image_fallback_text,
    reset_capabilities_cache, seed_capabilities, seed_hyperlink_support, set_capabilities,
};
pub use keyboard_protocol::{
    KITTY_FLAGS_QUERY, KeyboardProtocol, MODIFY_OTHER_KEYS_DISABLE, MODIFY_OTHER_KEYS_ENABLE,
    NEGOTIATION_TIMEOUT, NegotiationSequence, current as keyboard_protocol,
    decide as decide_keyboard_protocol, find_kitty_flags, is_negotiation_prefix,
    negotiate as negotiate_keyboard_protocol, parse_negotiation_sequence,
    set_current as set_keyboard_protocol,
};
pub use keymap::{
    Action, AltScreenAction, AltScreenKeymap, AutocompleteAction, AutocompleteKeymap, EditorAction,
    EditorKeymap, Key, KeybindingIssue, Keymap, ModelsAction, ModelsKeymap, SelectAction,
    SelectKeymap, SessionAction, SessionKeymap, TreeAction, TreeKeymap,
};
pub use login_dialog::{
    LoginDialog, LoginFinished, LoginLineKind, LoginUiMsg, TuiAuthInteraction, notify_auth_dialog,
    show_auth_prompt,
};
pub use markdown::{
    render as render_markdown, render_with_hyperlink_support as render_markdown_with_hyperlinks,
    render_with_text_color as render_markdown_with_text_color, trim_partial_closing_fence,
};
pub use model_selector::{ModelEntry, ModelSelector};
pub use native_modifiers::{
    ModifierKey, ModifierProbe, clear_native_modifier_probe, host_platform,
    is_apple_terminal_session, is_native_modifier_pressed, normalize_native_shift_enter,
    rescue_native_shift_enter, set_native_modifier_probe, should_detect_native_shift_enter,
};
pub use oauth_selector::{OAuthMode, OAuthSelector};
pub use overlay::{Overlay, OverlayOutcome};
pub use panic_hook::{install_panic_hook, restore_terminal_best_effort};
pub use resume_hint::{
    APP_NAME, ResumeTarget, format_resume_command, quote_if_needed, resume_hint_line,
};
pub use select_list::{ColumnLayout, DEFAULT_MAX_VISIBLE, SelectItem, SelectList};
pub use selector::{
    CheckboxSelector, INPUT_PROMPT, ListSelector, SCOPED_MODELS_ALL, Selector, SelectorKind,
    SelectorOutcome, input_line_spans, search_input_spans,
};
pub use session_search::{
    NameFilter, ParsedSearchQuery, QueryMode, SearchRow, SearchToken, SortMode, TokenKind,
    filter_and_sort as filter_and_sort_sessions, match_text as match_session_text,
    parse_search_query,
};
pub use session_selector::{SessionRow, SessionSelector, SessionSelectorOutcome};
pub use settings_selector::{FIELD_SEP, SettingRow, SettingsSelector, TrustSelector};
pub use startup::{
    DiagnosticCollision, DiagnosticSeverity, StartupDiagnostic, StartupLine, StartupReport,
    StartupRole, StartupSpan, build_startup_lines, display_path, extension_diagnostics,
    resource_diagnostics,
};
pub use startup_selector::run_startup_selector;
pub use status::{
    StatusLine, experimental_features_enabled, experimental_features_enabled_from, format_tokens,
};
pub use status_indicator::{
    IndicatorKind, SPINNER_FRAMES, SPINNER_INTERVAL, StatusIndicator, WorkingIndicator,
};
pub use submenu_selector::SubmenuSelector;
pub use terminal_progress::{
    TERMINAL_PROGRESS_ACTIVE_SEQUENCE, TERMINAL_PROGRESS_CLEAR_SEQUENCE,
    TERMINAL_PROGRESS_KEEPALIVE, TerminalProgress, progress_is_armed, write_terminal_progress,
};
pub use terminal_query::{
    CELL_SIZE_QUERY, CELL_SIZE_TIMEOUT, COLOR_SCHEME_QUERY, NoTerminalProbe,
    OSC11_BACKGROUND_QUERY, StdinTerminalProbe, TerminalProbe, find_cell_size_report,
    find_color_scheme_report, find_osc11_background_color, parse_cell_size_report,
    parse_color_scheme_report, parse_osc11_background_color, saw_device_attributes,
};
pub use terminal_title::{APP_TITLE, session_terminal_title};
pub use text_input::{Input, InputOutcome, TextInputSelector};
pub use theme::{
    BackgroundTheme, ColorMode, DetectionConfidence, TerminalTheme, TerminalThemeDetection,
    TerminalThemeSource, ThemeController, ThinkingTheme, UiTheme, color_of,
    detect_terminal_background_from_env, detect_terminal_background_theme,
    detect_terminal_theme_for_auto, rgb_to_256, theme_for_rgb,
};
pub use thinking_selector::ThinkingSelector;
pub use tmux::{
    EXTENDED_KEYS_FORMAT_WARNING, EXTENDED_KEYS_OFF_WARNING, TMUX_QUERY_TIMEOUT,
    check_keyboard_setup as check_tmux_keyboard_setup, in_tmux,
    keyboard_warning as tmux_keyboard_warning,
};
pub use transcript::{
    DEFAULT_IMAGE_WIDTH_CELLS, Entry, HIDDEN_THINKING_LABEL, ParsedSkillBlock, ResultImage,
    TranscriptView, content_text, parse_skill_block, thinking_text,
};
pub use tree_selector::{FilterMode, TreeEntryRole, TreeKind, TreeNode, TreeSelector};
pub use user_message_selector::{UserMessageRow, UserMessageSelector};

/// Re-export the exact crossterm ratatui uses (version-matched; ADR-0001 — never add a direct
/// crossterm dep). Front-ends and tests build key events through this path.
pub use ratatui::crossterm;
