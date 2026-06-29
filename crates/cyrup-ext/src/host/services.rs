//! Host-side backing for the guest capability imports (arch-08 §3.6). A loaded WASM extension
//! calls the `ui`/`session`/`models`/`exec`/`ext-fs`/`bus`/`control`/`registration` imports; those
//! land in [`GuestState`], which records registrations/observable effects and delegates interactive
//! capabilities to a pluggable [`HostServices`] backend. The default backend denies all interactive
//! capability (no ambient authority, R-ARCH-EXT-011); the session service injects a real one.

use crate::event::{EventKind, Subscriptions};
use crate::native::CtxTier;
use crate::registry::{CommandDescriptor, ExtensionRegistry};
use cyrup_core::ExtensionId;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Result of a capability-scoped `exec.run` (mirrors the WIT `exec-result`).
#[derive(Clone, Debug, Default)]
pub struct ExecOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// UI dialog options bag (Pi `ExtensionUIDialogOptions`, types.ts:89): a live-countdown `timeout_ms`
/// and/or a programmatic-dismiss `signal_id` for `confirm`/`input`/`select` (host gap-08-sdk #4).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialogOptions {
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub signal_id: Option<String>,
}

impl DialogOptions {
    /// Parse from the guest's `opts-json` (degrades to the empty bag, never panics).
    pub fn parse(opts_json: &str) -> Self {
        serde_json::from_str(opts_json).unwrap_or_default()
    }
}

/// A command-tier session/runtime mutation requested via the `control` import (arch-08 §6.3).
#[derive(Clone, Debug)]
pub enum ControlOp {
    NewSession { opts: Value },
    Switch { session_id: String, opts: Value },
    Fork { entry_id: String, opts: Value },
    Navigate { entry_id: String, opts: Value },
    Reload,
    Compact,
    WaitIdle,
    SendMessage { message: Value, opts: Value },
    SendUserMessage { content: String, opts: Value },
    SetModel(Value),
    SetThinkingLevel(String),
}

/// The pluggable host backend for interactive capabilities (arch-08 §3.6). Every method defaults to
/// "denied / empty" so the default host grants NO ambient authority; the session service overrides
/// the ones it wants to expose. All methods are sync (the host runs them on its own executor; the
/// guest is suspended across the call by Wasmtime's async support).
pub trait HostServices: Send + Sync {
    // --- ui (R-08-022) ---
    fn confirm(&self, _prompt: &str, _opts: &DialogOptions) -> bool {
        false
    }
    fn input(&self, _prompt: &str, _opts: &DialogOptions) -> Option<String> {
        None
    }
    fn select(&self, _prompt: &str, _options: &Value, _opts: &DialogOptions) -> Option<u32> {
        None
    }

    // --- provider OAuth login callbacks (Pi OAuthLoginCallbacks, host gap-08 #1) ---
    /// Prompt for a value during a guest `login` flow (Pi `onPrompt`); `Err` = cancelled.
    fn oauth_prompt(
        &self,
        _message: &str,
        _placeholder: Option<&str>,
        _allow_empty: bool,
    ) -> Result<String, String> {
        Err("oauth prompt capability not granted".into())
    }
    /// Interactive selector during a guest `login` flow (Pi `onSelect`); returns the chosen id.
    fn oauth_select(&self, _message: &str, _options: &Value) -> Option<String> {
        None
    }
    fn editor(&self, _initial: &str) -> Option<String> {
        None
    }
    /// A custom overlay component; returns an optional serialized result (Pi `custom()`).
    fn custom(&self, _spec: &Value) -> Option<String> {
        None
    }
    /// Current editor buffer text (Pi `getEditorText`).
    fn editor_text(&self) -> String {
        String::new()
    }
    /// Active theme name (Pi `getTheme`).
    fn theme(&self) -> Option<String> {
        None
    }
    /// Available theme names (Pi `listThemes`).
    fn theme_list(&self) -> Value {
        json!([])
    }
    /// Switch the active theme (Pi `setTheme`); denied by default.
    fn set_theme(&self, _name: &str) -> Result<(), String> {
        Err("theme capability not granted".into())
    }
    /// Whether tool rows are expanded (Pi `getToolsExpanded`).
    fn tools_expanded(&self) -> bool {
        false
    }

    // --- session read-only view (R-08-027) ---
    fn entries(&self) -> Value {
        json!([])
    }
    fn branch(&self) -> Value {
        json!([])
    }
    fn tree(&self) -> Value {
        Value::Null
    }
    fn session_name(&self) -> Option<String> {
        None
    }

    // --- models ---
    fn models(&self) -> Value {
        json!([])
    }
    fn current_model(&self) -> Option<String> {
        None
    }
    fn context_usage(&self) -> Value {
        json!({})
    }
    fn thinking_level(&self) -> Option<String> {
        None
    }

    // --- exec capability (R-08-030); denied by default ---
    fn exec(&self, _cmd: &str, _args: &[String], _opts: &Value) -> Result<ExecOutput, String> {
        Err("exec capability not granted".into())
    }

    // --- command-tier control (arch-08 §6.3); the deadlock guard is applied BEFORE this is called ---
    fn control(&self, _op: ControlOp) -> Result<(), String> {
        Err("control capability not available".into())
    }

    /// Persist a custom (non-LLM) entry (R-08-026); returns the new entry id.
    fn append_entry(&self, _custom_type: &str, _data: &Value) -> Result<String, String> {
        Err("append_entry not available".into())
    }
}

/// Recorded extended-UI chrome effects (Pi `ExtensionUIContext` mutators, types.ts:124-275). These
/// are observable host-side (tests/diagnostics) and would drive the TUI widget protocol (arch-11).
#[derive(Clone, Debug, Default)]
pub struct UiChrome {
    pub header: Option<String>,
    pub footer: Option<String>,
    pub title: Option<String>,
    /// `set-editor-text`/`paste-editor-text` writes (text, is_paste).
    pub editor_writes: Vec<(String, bool)>,
    /// `theme-set` requests.
    pub theme_sets: Vec<String>,
    /// `working-start`(Some(label)) / `working-stop`(None) toggles.
    pub working: Vec<Option<String>>,
    /// last `set-tools-expanded` value.
    pub tools_expanded: Option<bool>,
}

/// The default backend: grants nothing (no ambient authority, R-ARCH-EXT-011).
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyServices;
impl HostServices for DenyServices {}

/// Canned responses a [`RecordingServices`] returns for the interactive (host→user) capabilities.
#[derive(Clone, Debug)]
pub struct CannedResponses {
    pub confirm: bool,
    pub input: Option<String>,
    pub select: Option<u32>,
    pub editor: Option<String>,
    pub custom: Option<String>,
    pub exec: ExecOutput,
    pub current_model: Option<String>,
    pub models: Value,
    pub theme: Option<String>,
    pub themes: Value,
    pub editor_text: String,
    /// Answer returned from a guest `login` flow's `onPrompt` (Pi OAuth). `None` = cancelled.
    pub oauth_prompt: Option<String>,
    /// Id returned from a guest `login` flow's `onSelect`.
    pub oauth_select: Option<String>,
}

impl Default for CannedResponses {
    fn default() -> Self {
        Self {
            confirm: true,
            input: Some(String::new()),
            select: Some(0),
            editor: Some(String::new()),
            custom: None,
            exec: ExecOutput::default(),
            current_model: None,
            models: json!([]),
            theme: None,
            themes: json!([]),
            editor_text: String::new(),
            oauth_prompt: Some(String::new()),
            oauth_select: None,
        }
    }
}

/// A concrete NON-deny backend (arch-08 §3.6): it GRANTS the interactive/exec/control/append
/// capabilities, returning canned responses and RECORDING the effects. This is the in-crate analog
/// of the live cyrup-session/cyrup-tui backend the session injects at runtime — unlike
/// [`DenyServices`], it proves every capability seam end-to-end with real (observable) effects.
#[derive(Default)]
pub struct RecordingServices {
    responses: CannedResponses,
    state: Mutex<RecordingState>,
}

#[derive(Default)]
struct RecordingState {
    control_ops: Vec<ControlOp>,
    exec_calls: Vec<(String, Vec<String>)>,
    entries: Vec<(String, Value)>,
    next_entry: u64,
}

impl RecordingServices {
    pub fn new(responses: CannedResponses) -> Self {
        Self { responses, state: Mutex::new(RecordingState::default()) }
    }

    /// The control ops requested via the `control` import (command tier).
    pub fn control_ops(&self) -> Vec<ControlOp> {
        self.state.lock().map(|g| g.control_ops.clone()).unwrap_or_default()
    }

    /// The `(cmd, args)` of each capability-scoped `exec.run`.
    pub fn exec_calls(&self) -> Vec<(String, Vec<String>)> {
        self.state.lock().map(|g| g.exec_calls.clone()).unwrap_or_default()
    }

    /// The persisted custom entries (R-08-026).
    pub fn entries_persisted(&self) -> Vec<(String, Value)> {
        self.state.lock().map(|g| g.entries.clone()).unwrap_or_default()
    }
}

impl HostServices for RecordingServices {
    fn confirm(&self, _prompt: &str, _opts: &DialogOptions) -> bool {
        self.responses.confirm
    }
    fn input(&self, _prompt: &str, _opts: &DialogOptions) -> Option<String> {
        self.responses.input.clone()
    }
    fn select(&self, _prompt: &str, _options: &Value, _opts: &DialogOptions) -> Option<u32> {
        self.responses.select
    }
    fn oauth_prompt(
        &self,
        _message: &str,
        _placeholder: Option<&str>,
        _allow_empty: bool,
    ) -> Result<String, String> {
        self.responses.oauth_prompt.clone().ok_or_else(|| "oauth prompt cancelled".into())
    }
    fn oauth_select(&self, _message: &str, _options: &Value) -> Option<String> {
        self.responses.oauth_select.clone()
    }
    fn editor(&self, _initial: &str) -> Option<String> {
        self.responses.editor.clone()
    }
    fn custom(&self, _spec: &Value) -> Option<String> {
        self.responses.custom.clone()
    }
    fn editor_text(&self) -> String {
        self.responses.editor_text.clone()
    }
    fn theme(&self) -> Option<String> {
        self.responses.theme.clone()
    }
    fn theme_list(&self) -> Value {
        self.responses.themes.clone()
    }
    fn set_theme(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }
    fn models(&self) -> Value {
        self.responses.models.clone()
    }
    fn current_model(&self) -> Option<String> {
        self.responses.current_model.clone()
    }
    fn exec(&self, cmd: &str, args: &[String], _opts: &Value) -> Result<ExecOutput, String> {
        if let Ok(mut g) = self.state.lock() {
            g.exec_calls.push((cmd.to_string(), args.to_vec()));
        }
        Ok(self.responses.exec.clone())
    }
    fn control(&self, op: ControlOp) -> Result<(), String> {
        if let Ok(mut g) = self.state.lock() {
            g.control_ops.push(op);
        }
        Ok(())
    }
    fn append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String> {
        let mut g = self.state.lock().map_err(|_| "recording lock poisoned".to_string())?;
        g.next_entry += 1;
        let id = format!("entry-{}", g.next_entry);
        g.entries.push((custom_type.to_string(), data.clone()));
        Ok(id)
    }
}

/// Capability-scoped filesystem roots for the `ext-fs` import (preopened dirs; no ambient fs).
#[derive(Clone, Debug, Default)]
pub struct FsCaps {
    /// A single granted root the guest may read/write under. `None` => all fs access denied.
    pub root: Option<PathBuf>,
}

impl FsCaps {
    /// Resolve `path` under the granted root, rejecting escapes (`..`). Returns the absolute path or
    /// an error string surfaced to the guest as a WIT `result` error (never a host panic).
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let root = self.root.as_ref().ok_or("filesystem capability not granted")?;
        let candidate = PathBuf::from(path);
        // Reject absolute paths and parent-dir escapes (capability scoping, R-ARCH-EXT-011).
        if candidate.is_absolute() || candidate.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(format!("path `{path}` escapes the granted capability root"));
        }
        Ok(root.join(candidate))
    }
}

/// Host-side state backing one loaded WASM extension's imports (arch-08 §3.5/§3.6). Shared (via
/// `Arc`) between the extension's `Store<HostState>` (so the import Host impls reach it) and the
/// [`crate::host::WasmExtension`] handle (so the loader reads back what `init` registered).
pub struct GuestState {
    pub owner: ExtensionId,
    pub registry: Arc<ExtensionRegistry>,
    pub services: Arc<dyn HostServices>,
    pub fs: FsCaps,
    /// The current dispatch tier; control ops are legal only at [`CtxTier::Command`] (R-08-008).
    tier: Mutex<CtxTier>,
    /// Subscriptions declared via the `subscribe` import (read back after `init`).
    subs: Mutex<Subscriptions>,
    /// Commands registered via `register-command` (drained into the registry after `init`).
    commands: Mutex<Vec<(String, CommandDescriptor)>>,
    /// Flags registered via `register-flag` (name -> spec JSON).
    flags: Mutex<HashMap<String, Value>>,
    /// Autocomplete providers added via `add-autocomplete` (command names).
    autocomplete: Mutex<Vec<String>>,
    /// Message renderers registered via `register-message-renderer` (custom types).
    renderers: Mutex<Vec<String>>,
    /// `ui.notify` log — observable host effect (used by tests + diagnostics).
    notifications: Mutex<Vec<String>>,
    /// `ui.set-status` log.
    statuses: Mutex<Vec<String>>,
    /// `ui.set-widget` payloads.
    widgets: Mutex<Vec<Value>>,
    /// Extended UI chrome effects (header/footer/title/editor/theme/working/tools-expanded).
    chrome: Mutex<UiChrome>,
    /// `bus.emit` topics + payloads (R-08-029).
    bus_emits: Mutex<Vec<(String, Value)>>,
    /// `host-tool.emit-update` chunks emitted during a guest tool's `execute` (call_id, chunk).
    /// Drained to the runtime `ToolUpdateSink` after the execute call settles (Pi `onUpdate`).
    tool_updates: Mutex<Vec<(String, Value)>>,
    /// Count of stacked global autocomplete providers (Pi addAutocompleteProvider, host gap #3).
    autocomplete_providers: Mutex<u32>,
    /// OAuth login-flow callbacks the guest invoked during `provider-login` (observable host-side).
    oauth_events: Mutex<Vec<OAuthEvent>>,
    /// `provider-stream.emit-event` events pushed during a guest `streamSimple` (stream_id, event).
    stream_events: Mutex<Vec<(String, Value)>>,
    /// The active-tool restriction set via `ext-tools.set-active-tools` (Pi `setActiveTools`).
    active_tools_restriction: Mutex<Option<Vec<String>>>,
}

/// An OAuth login-flow callback the guest invoked during `provider-login` (Pi `OAuthLoginCallbacks`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthEvent {
    Auth { url: String, instructions: Option<String> },
    DeviceCode { user_code: String, verification_uri: String },
    Prompt { message: String },
    Progress { message: String },
    Select { message: String },
}

impl GuestState {
    pub fn new(owner: ExtensionId, registry: Arc<ExtensionRegistry>) -> Self {
        Self::with_services(owner, registry, Arc::new(DenyServices))
    }

    pub fn with_services(
        owner: ExtensionId,
        registry: Arc<ExtensionRegistry>,
        services: Arc<dyn HostServices>,
    ) -> Self {
        Self {
            owner,
            registry,
            services,
            fs: FsCaps::default(),
            tier: Mutex::new(CtxTier::Command), // init runs at command tier (load time)
            subs: Mutex::new(Subscriptions::empty()),
            commands: Mutex::new(Vec::new()),
            flags: Mutex::new(HashMap::new()),
            autocomplete: Mutex::new(Vec::new()),
            renderers: Mutex::new(Vec::new()),
            notifications: Mutex::new(Vec::new()),
            statuses: Mutex::new(Vec::new()),
            widgets: Mutex::new(Vec::new()),
            chrome: Mutex::new(UiChrome::default()),
            bus_emits: Mutex::new(Vec::new()),
            tool_updates: Mutex::new(Vec::new()),
            autocomplete_providers: Mutex::new(0),
            oauth_events: Mutex::new(Vec::new()),
            stream_events: Mutex::new(Vec::new()),
            active_tools_restriction: Mutex::new(None),
        }
    }

    pub fn with_fs(mut self, root: PathBuf) -> Self {
        self.fs = FsCaps { root: Some(root) };
        self
    }

    /// Set the dispatch tier (the loader sets `Event` before dispatching an event handler, keeps
    /// `Command` for init/command handlers). A poisoned lock degrades to a no-op (never a panic).
    pub fn set_tier(&self, tier: CtxTier) {
        if let Ok(mut g) = self.tier.lock() {
            *g = tier;
        }
    }

    pub fn tier(&self) -> CtxTier {
        self.tier.lock().map(|g| *g).unwrap_or(CtxTier::Event)
    }

    /// Deadlock guard (R-08-008): control ops require the command tier.
    pub fn require_command_tier(&self) -> Result<(), String> {
        if self.tier() == CtxTier::Command {
            Ok(())
        } else {
            Err("deadlock guard: session-mutating control op from an event handler".into())
        }
    }

    pub fn add_subscription(&self, kind: EventKind) {
        if let Ok(mut g) = self.subs.lock() {
            g.add(kind);
        }
    }

    pub fn subscriptions(&self) -> Subscriptions {
        self.subs.lock().map(|g| *g).unwrap_or_else(|_| Subscriptions::empty())
    }

    pub fn push_command(&self, name: String, desc: CommandDescriptor) {
        if let Ok(mut g) = self.commands.lock() {
            g.push((name, desc));
        }
    }

    /// Drain the commands registered during `init` (the loader writes them into the registry).
    pub fn take_commands(&self) -> Vec<(String, CommandDescriptor)> {
        self.commands.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }

    pub fn set_flag(&self, name: String, spec: Value) {
        if let Ok(mut g) = self.flags.lock() {
            g.insert(name, spec);
        }
    }

    pub fn get_flag(&self, name: &str) -> Option<String> {
        self.flags.lock().ok().and_then(|g| g.get(name).map(|v| v.to_string()))
    }

    pub fn add_autocomplete(&self, command: String) {
        if let Ok(mut g) = self.autocomplete.lock() {
            g.push(command);
        }
    }

    pub fn autocomplete_commands(&self) -> Vec<String> {
        self.autocomplete.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn add_renderer(&self, custom_type: String) {
        if let Ok(mut g) = self.renderers.lock() {
            g.push(custom_type);
        }
    }

    pub fn renderers(&self) -> Vec<String> {
        self.renderers.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn notify(&self, message: String) {
        if let Ok(mut g) = self.notifications.lock() {
            g.push(message);
        }
    }

    pub fn notifications(&self) -> Vec<String> {
        self.notifications.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_status(&self, message: String) {
        if let Ok(mut g) = self.statuses.lock() {
            g.push(message);
        }
    }

    pub fn statuses(&self) -> Vec<String> {
        self.statuses.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_widget(&self, widget: Value) {
        if let Ok(mut g) = self.widgets.lock() {
            g.push(widget);
        }
    }

    // --- extended UI chrome (Pi ExtensionUIContext mutators) ---

    fn with_chrome<R>(&self, f: impl FnOnce(&mut UiChrome) -> R) -> Option<R> {
        self.chrome.lock().ok().map(|mut g| f(&mut g))
    }

    /// Snapshot of the recorded chrome effects (tests/diagnostics).
    pub fn chrome(&self) -> UiChrome {
        self.chrome.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_header(&self, content: String) {
        self.with_chrome(|c| c.header = Some(content));
    }
    pub fn set_footer(&self, content: String) {
        self.with_chrome(|c| c.footer = Some(content));
    }
    pub fn set_title(&self, title: String) {
        self.with_chrome(|c| c.title = Some(title));
    }
    pub fn editor_write(&self, text: String, is_paste: bool) {
        self.with_chrome(|c| c.editor_writes.push((text, is_paste)));
    }
    pub fn theme_set(&self, name: String) {
        self.with_chrome(|c| c.theme_sets.push(name));
    }
    pub fn working(&self, label: Option<String>) {
        self.with_chrome(|c| c.working.push(label));
    }
    pub fn set_tools_expanded(&self, expanded: bool) {
        self.with_chrome(|c| c.tools_expanded = Some(expanded));
    }

    pub fn bus_emit(&self, topic: String, payload: Value) {
        if let Ok(mut g) = self.bus_emits.lock() {
            g.push((topic, payload));
        }
    }

    pub fn bus_emits(&self) -> Vec<(String, Value)> {
        self.bus_emits.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn push_tool_update(&self, call_id: String, chunk: Value) {
        if let Ok(mut g) = self.tool_updates.lock() {
            g.push((call_id, chunk));
        }
    }

    /// Drain the streamed updates for a settled tool execution.
    pub fn take_tool_updates(&self) -> Vec<(String, Value)> {
        self.tool_updates.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }

    // --- global autocomplete provider stacking (Pi addAutocompleteProvider, host gap #3) ---

    pub fn add_autocomplete_provider(&self) {
        if let Ok(mut g) = self.autocomplete_providers.lock() {
            *g += 1;
        }
    }

    /// How many global autocomplete providers the guest stacked (drives the host fold).
    pub fn autocomplete_provider_count(&self) -> u32 {
        self.autocomplete_providers.lock().map(|g| *g).unwrap_or(0)
    }

    // --- OAuth login-flow callbacks (Pi OAuthLoginCallbacks, host gap #1) ---

    pub fn record_oauth_event(&self, ev: OAuthEvent) {
        if let Ok(mut g) = self.oauth_events.lock() {
            g.push(ev);
        }
    }

    /// The OAuth login callbacks the guest invoked during `provider-login` (tests/diagnostics).
    pub fn oauth_events(&self) -> Vec<OAuthEvent> {
        self.oauth_events.lock().map(|g| g.clone()).unwrap_or_default()
    }

    // --- provider streamSimple events (Pi createAssistantMessageEventStream, host gap #1) ---

    pub fn push_stream_event(&self, stream_id: String, event: Value) {
        if let Ok(mut g) = self.stream_events.lock() {
            g.push((stream_id, event));
        }
    }

    /// The assistant-message stream events a guest `streamSimple` pushed (stream_id, event).
    pub fn stream_events(&self) -> Vec<(String, Value)> {
        self.stream_events.lock().map(|g| g.clone()).unwrap_or_default()
    }

    // --- active-tool restriction (Pi setActiveTools, host gap-08-sdk #7) ---

    pub fn set_active_tools_restriction(&self, names: Vec<String>) {
        if let Ok(mut g) = self.active_tools_restriction.lock() {
            *g = Some(names);
        }
    }

    /// The active-tool restriction the guest set, if any (the merge is applied host-side).
    pub fn active_tools_restriction(&self) -> Option<Vec<String>> {
        self.active_tools_restriction.lock().ok().and_then(|g| g.clone())
    }
}
