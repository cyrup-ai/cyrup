//! The three LIVE attachment traits [`LiveHostServices`] answers from when a snapshot field cannot:
//! [`SessionActivity`] (the running session's idle/pending/abort), [`SessionCatalog`] (its
//! guest-facing command + extension-tool-provenance listings) and [`ThemeAccess`] (the interactive
//! TUI's theme seam).
//!
//! Each is a trait rather than more [`LiveSnapshot`] fields for the same reason, restated in the
//! docs below: the value is live, so a mirrored copy would be stale exactly when a handler asks.
//! The first two are attached by `AgentSession::into_shared` over weak self-handles
//! (`session/adapters.rs`); the third by the interactive TUI only.

use serde_json::Value;

// Doc-only: the docs below name the backend these attach to, the snapshot they are the alternative
// to, and the trait methods they back. Nothing in this file names any of them in code.
#[cfg(doc)]
use super::{LiveHostServices, LiveSnapshot};
#[cfg(doc)]
use cyrup_ext::host::HostServices;

/// The live session's activity readback + interrupt, backing the `ctx-state`/`control` imports that
/// only the running session can answer (EXT-005). Pi binds these straight to the session object:
/// `isIdle: () => this.isIdle`, `hasPendingMessages: () => this.pendingMessageCount > 0` and
/// `abort: () => { void this.abort() }` (agent-session.ts:2409-2419).
///
/// A separate trait rather than more snapshot fields because these are LIVE — a mirrored `is_idle`
/// would be stale exactly when it matters (mid-run, which is when a handler asks). Attached by
/// `AgentSession::into_shared` over a weak self-handle, so it never keeps the session alive.
pub(crate) trait SessionActivity: Send + Sync {
    /// Whether no agent run (including the post-run retry/compaction/continuation loop) is in
    /// flight (Pi `isIdle`).
    fn is_idle(&self) -> bool;
    /// Queued steering + follow-up message count (Pi `pendingMessageCount`).
    fn pending_message_count(&self) -> usize;
    /// Interrupt the in-flight run NOW. Pi runs `void this.abort()` SYNCHRONOUSLY from the handler
    /// that called `ctx.abort()` (agent-session.ts:2412-2418); deferring it to a turn-boundary drain
    /// would abort a run that has already finished, i.e. nothing at all.
    fn abort(&self);
}

/// The live session's guest-facing INTROSPECTION catalog — the two listings only the running
/// session can compose (EXT-037 / EXT-038). Pi binds both straight to the session object in
/// `_bindExtensionCore`: `getAllTools: () => this.getAllTools()` (agent-session.ts:2394) and the
/// `getCommands` closure (`:2332-2354`, bound at `:2397`) @v0.83.0.
///
/// A separate trait for the same reason [`SessionActivity`] is one: these are LIVE reads over state
/// this backend does not own, and a mirrored copy would be stale exactly when a handler asks —
/// `getCommands()` must see a command an extension registered a moment ago, and `getAllTools()`
/// must see a tool `refreshTools` just merged. Attached by `AgentSession::into_shared` over a weak
/// self-handle, so it never keeps the session alive.
pub(crate) trait SessionCatalog: Send + Sync {
    /// pi `getCommands(): SlashCommandInfo[]` — `[...extensionCommands, ...templates, ...skills]`,
    /// extension rows keyed on `command.invocationName` (`core/agent-session.ts:2332-2354`
    /// @v0.83.0; type `SlashCommandInfo`, `core/slash-commands.ts:6-11`). That is exactly
    /// `AgentSession::slash_command_catalog`, which is the ONLY source carrying prompt templates
    /// and skills — the registry fallback in `cyrup-ext` has extension commands and nothing else.
    fn commands(&self) -> Vec<Value>;

    /// The `SourceInfo` (`core/source-info.ts:6-12` @v0.83.0) pi stamps on each `_toolDefinitions`
    /// entry, for the EXTENSION-contributed tools only, keyed by tool name.
    ///
    /// pi tags the registry three ways while rebuilding it (`_refreshToolRegistry`,
    /// `agent-session.ts:2455-2488`): a built-in gets `createSyntheticSourceInfo("<builtin:${name}>",
    /// {source: "builtin"})`, an SDK custom tool `("<sdk:${name}>", {source: "sdk"})`, and a
    /// registered extension tool carries the runner's real `tool.sourceInfo`. Only the last is
    /// recoverable here, so a name absent from this map falls back to the builtin synthetic form
    /// (see `json::builtin_tool_source_info`, this module's sibling).
    fn extension_tool_source_info(&self) -> std::collections::HashMap<String, Value>;
}

/// The interactive TUI's live THEME seam — the source behind all four of
/// [`HostServices::theme`], [`HostServices::theme_list`], [`HostServices::theme_by_name`] and
/// [`HostServices::set_theme`] (SEAM-T01).
///
/// One handle for all four because pi gates all four the same way: they are bound only inside
/// `createExtensionUIContext`, which ONLY the interactive mode builds
/// (`modes/interactive/interactive-mode.ts:2404-2415` @v0.84.2 — `getAllThemes: () =>
/// getAvailableThemesWithPaths()`, `getTheme: (name) => getThemeByName(name)`, the `get theme()`
/// accessor at `:2401-2403`, and the `setTheme` closure at `:2406-2417`). Every other mode gets
/// `noOpUIContext`, whose theme members are `getAllThemes: () => []`, `getTheme: () => undefined`
/// and `setTheme: () => ({success: false, error: "UI not available"})`
/// (`core/extensions/runner.ts:261-263` @v0.83.0); pi's RPC mode hard-codes the same three answers
/// (`modes/rpc/rpc-mode.ts:290-300` @v0.83.0, its `setTheme` erroring "Theme switching not
/// supported in RPC mode"). So an UNATTACHED handle here reproduces upstream exactly: the trait
/// defaults `None` / `json!([])` / `None` are already pi's headless answers, and
/// [`LiveHostServices::set_theme`] returns pi's own `"UI not available"` string.
///
/// A trait rather than more [`LiveSnapshot`] fields for the reason the crate-internal
/// `SessionActivity` is one: the
/// active theme is LIVE (an extension asking mid-session must see a `/settings → theme` switch the
/// user made a keystroke ago), and `set` is a real ACTION whose success/failure pi returns
/// synchronously. Attached by the interactive TUI only, over handles that do not keep the app
/// alive.
pub trait ThemeAccess: Send + Sync {
    /// The ACTIVE theme's name — pi's `get theme() { return theme }`
    /// (`interactive-mode.ts:2401-2403`), reduced to the name because cyrup's WIT `theme-get`
    /// returns `option<string>`; the colours travel through [`Self::by_name`], which is how
    /// `live.rs`'s `theme-get-json` (EXT-066) composes pi's whole `Theme` value.
    fn active(&self) -> Option<String>;

    /// pi `getAllThemes(): {name, path}[]` (`core/extensions/types.ts:269` @v0.83.0), implemented
    /// upstream by `getAvailableThemesWithPaths()`
    /// (`modes/interactive/theme/theme.ts:493-520` @v0.83.0): built-ins, then custom themes, then
    /// registered ones, deduped first-wins by name and sorted by name.
    fn list(&self) -> Value;

    /// pi `getTheme(name): Theme | undefined` (`core/extensions/types.ts:272` @v0.83.0) —
    /// `getThemeByName` (`theme.ts:671-677`), which loads WITHOUT switching and swallows a load
    /// failure into `undefined`.
    fn by_name(&self, name: &str) -> Option<Value>;

    /// pi `setTheme(name): {success, error?}` (`core/extensions/types.ts:275` @v0.83.0). `Err` is
    /// upstream's `{success: false, error}`, whose message for an unknown name is
    /// `Theme not found: {name}` (`theme.ts:622`, thrown by `loadThemeJson` and caught into the
    /// result by `setTheme`, `:891-913`).
    fn set(&self, name: &str) -> Result<(), String>;
}
