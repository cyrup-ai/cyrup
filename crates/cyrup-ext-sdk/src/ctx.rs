//! The handler context wrappers (arch-08 §2.2/§6.3; Pi `ExtensionContext`/`ExtensionUIContext`/
//! `ExtensionCommandContext`, types.ts:124-390). Every event/tool handler receives a [`Ctx`], the
//! safe-Rust front for the `ui`/`session`/`models`/`exec`/`bus` capability imports. Command handlers
//! receive a [`CommandCtx`] which additionally exposes the COMMAND-only `control` ops — the
//! type-level half of the deadlock rule (the host check is authoritative, R-08-008).
//!
//! On `wasm32` each method calls the generated WIT import; on the host target (unit tests) the
//! methods return inert defaults so the ergonomic API is exercisable without a runtime.
//!
//! `needless_return` is allowed: the `#[cfg]`-split dual bodies use an early `return` in the wasm
//! arm so the host arm can be a distinct tail expression.
#![allow(clippy::needless_return)]

use crate::descriptor::{
    DialogOptions, ExecOptions, ForkOptions, NavigateOptions, NewSessionOptions, SwitchSessionOptions,
};
use serde::Serialize;
use serde_json::Value;

/// The capability context handed to every handler (event tier: no session mutation).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ctx;

impl Ctx {
    pub fn new() -> Self {
        Ctx
    }
    /// UI surface (R-08-022).
    pub fn ui(&self) -> Ui {
        Ui
    }
    /// Read-only session view + state persistence (R-08-026/027).
    pub fn session(&self) -> Session {
        Session
    }
    /// Model registry view (read; `set_model` is command-only).
    pub fn models(&self) -> Models {
        Models
    }

    /// Convenience: post a transient notification.
    pub fn notify(&self, message: &str) {
        self.ui().notify(message);
    }

    /// Emit on the inter-extension event bus (R-08-029).
    pub fn emit(&self, topic: &str, payload: impl Serialize) {
        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::bus::emit(topic, &payload);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (topic, payload);
        }
    }

    // --- active-tool / command introspection (Pi getActiveTools/…/getCommands, types.ts:1257-1266) ---

    /// The names of the currently-active tools (Pi `getActiveTools`).
    pub fn get_active_tools(&self) -> Vec<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_active_tools())
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
        }
        #[cfg(not(target_arch = "wasm32"))]
        Vec::new()
    }
    /// All configured tools with metadata (Pi `getAllTools` → `ToolInfo[]`).
    pub fn get_all_tools(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_all_tools());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    /// Restrict the active tool set by name (Pi `setActiveTools`; plan-mode-style restriction).
    pub fn set_active_tools(&self, names: &[&str]) {
        let names_json = serde_json::to_string(names).unwrap_or_else(|_| "[]".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ext_tools::set_active_tools(&names_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = names_json;
    }
    /// Available slash commands (Pi `getCommands` → `SlashCommandInfo[]`).
    pub fn get_commands(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ext_tools::get_commands());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }

    /// Run a capability-scoped command (R-08-030). Denied unless the host granted the exec capability.
    pub fn exec(&self, cmd: &str, args: &[&str], opts: &ExecOptions) -> Result<ExecResult, String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            match crate::guest::bindings::cyrup::ext::exec::run(cmd, &argv, &opts_json) {
                Ok(r) => Ok(ExecResult { code: r.code, stdout: r.stdout, stderr: r.stderr }),
                Err(e) => Err(e),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (cmd, args, opts_json);
            Err("exec unavailable on host target".into())
        }
    }
}

/// Result of [`Ctx::exec`].
#[derive(Clone, Debug, Default)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// The UI capability surface (Pi `ExtensionUIContext`, types.ts:124-275).
#[derive(Clone, Copy, Debug, Default)]
pub struct Ui;

impl Ui {
    pub fn notify(&self, message: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::notify(message);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = message;
    }
    pub fn set_status(&self, message: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_status(message);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = message;
    }
    /// Confirmation dialog (Pi `confirm`). Indefinite; use [`Self::confirm_with`] for a timeout/signal.
    pub fn confirm(&self, prompt: &str) -> bool {
        self.confirm_with(prompt, &DialogOptions::default())
    }
    /// Confirmation dialog with a [`DialogOptions`] bag (Pi `confirm(title, msg, {timeout, signal})`).
    pub fn confirm_with(&self, prompt: &str, opts: &DialogOptions) -> bool {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::confirm(prompt, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, opts_json);
            false
        }
    }
    /// Text input dialog (Pi `input`).
    pub fn input(&self, prompt: &str) -> Option<String> {
        self.input_with(prompt, &DialogOptions::default())
    }
    /// Text input dialog with a [`DialogOptions`] bag (Pi `input(title, placeholder, {timeout, signal})`).
    pub fn input_with(&self, prompt: &str, opts: &DialogOptions) -> Option<String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::input(prompt, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, opts_json);
            None
        }
    }
    /// Single-choice select; returns the chosen index.
    pub fn select(&self, prompt: &str, options: &[&str]) -> Option<u32> {
        self.select_with(prompt, options, &DialogOptions::default())
    }
    /// Single-choice select with a [`DialogOptions`] bag (Pi `select(title, options, {timeout, signal})`).
    pub fn select_with(&self, prompt: &str, options: &[&str], opts: &DialogOptions) -> Option<u32> {
        let options_json = serde_json::to_string(options).unwrap_or_else(|_| "[]".into());
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::select(prompt, &options_json, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, options_json, opts_json);
            None
        }
    }
    /// Multiline editor seeded with `initial`; returns the edited text (None = cancelled).
    pub fn editor(&self, initial: &str) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::editor(initial);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = initial;
            None
        }
    }
    pub fn set_widget(&self, widget: impl Serialize) {
        let widget_json = serde_json::to_string(&widget).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_widget(&widget_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = widget_json;
    }

    // --- chrome (Pi setHeader/setFooter/setTitle, types.ts:130-150) ---
    pub fn set_header(&self, content: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_header(content);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = content;
    }
    pub fn set_footer(&self, content: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_footer(content);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = content;
    }
    pub fn set_title(&self, title: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_title(title);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = title;
    }
    /// A custom overlay component; returns an optional serialized result (Pi `custom()`).
    pub fn custom(&self, spec: impl Serialize) -> Option<String> {
        let spec_json = serde_json::to_string(&spec).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::custom(&spec_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = spec_json;
            None
        }
    }

    // --- editor buffer access (Pi getEditorText/setEditorText/pasteEditorText, types.ts:200-230) ---
    pub fn editor_text(&self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::get_editor_text();
        }
        #[cfg(not(target_arch = "wasm32"))]
        String::new()
    }
    pub fn set_editor_text(&self, text: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_editor_text(text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = text;
    }
    pub fn paste_editor_text(&self, text: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::paste_editor_text(text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = text;
    }

    // --- theme get/list/set (Pi getTheme/listThemes/setTheme, types.ts:240-260) ---
    pub fn theme(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::theme_get();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    pub fn theme_list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::ui::theme_list());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    pub fn set_theme(&self, name: &str) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::theme_set(name);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = name;
            Ok(())
        }
    }

    // --- working-indicator controls (Pi startWorking/stopWorking, types.ts:265-275) ---
    pub fn working_start(&self, label: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::working_start(label);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = label;
    }
    pub fn working_stop(&self) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::working_stop();
    }

    // --- tools-expanded get/set (Pi getToolsExpanded/setToolsExpanded) ---
    pub fn tools_expanded(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::get_tools_expanded();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }
    pub fn set_tools_expanded(&self, expanded: bool) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_tools_expanded(expanded);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = expanded;
    }
}

/// The read-only session view + state-persistence surface (Pi `ReadonlySessionManager` + R-08-026).
#[derive(Clone, Copy, Debug, Default)]
pub struct Session;

impl Session {
    pub fn entries(&self) -> Value {
        parse_json(session_call(SessionGet::Entries))
    }
    pub fn branch(&self) -> Value {
        parse_json(session_call(SessionGet::Branch))
    }
    pub fn tree(&self) -> Value {
        parse_json(session_call(SessionGet::Tree))
    }
    /// Persist a custom (non-LLM) entry (R-08-026); returns the new entry id.
    pub fn append_entry(&self, custom_type: &str, data: impl Serialize) -> Result<String, String> {
        let data_json = serde_json::to_string(&data).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::session::append_entry(custom_type, &data_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (custom_type, data_json);
            Err("append_entry unavailable on host target".into())
        }
    }
    pub fn session_name(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::session::get_session_name();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    pub fn set_session_name(&self, name: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::session::set_session_name(name);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = name;
    }
    pub fn set_label(&self, entry_id: &str, label: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::session::set_label(entry_id, label);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (entry_id, label);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum SessionGet {
    Entries,
    Branch,
    Tree,
}

fn session_call(which: SessionGet) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::guest::bindings::cyrup::ext::session as s;
        return match which {
            SessionGet::Entries => s::entries_json(),
            SessionGet::Branch => s::branch_json(),
            SessionGet::Tree => s::tree_json(),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = which;
        "null".into()
    }
}

fn parse_json(s: String) -> Value {
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

/// The model registry view (Pi types.ts:1273-1279).
#[derive(Clone, Copy, Debug, Default)]
pub struct Models;

impl Models {
    pub fn list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::models::list_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    pub fn current(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::current();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    pub fn context_usage(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return parse_json(crate::guest::bindings::cyrup::ext::models::context_usage());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Null
    }
    pub fn thinking_level(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::thinking_level();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
}

/// The command-tier context (Pi `ExtensionCommandContext`, types.ts:339-373). Adds the COMMAND-only
/// `control` ops to [`Ctx`]; the host rejects any control op from an event handler (R-08-008).
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandCtx {
    base: Ctx,
}

impl CommandCtx {
    pub fn new() -> Self {
        Self { base: Ctx }
    }
    pub fn ctx(&self) -> &Ctx {
        &self.base
    }
    pub fn ui(&self) -> Ui {
        self.base.ui()
    }
    pub fn session(&self) -> Session {
        self.base.session()
    }
    pub fn models(&self) -> Models {
        self.base.models()
    }

    pub fn new_session(&self) -> Result<(), String> {
        self.new_session_with(&NewSessionOptions::default())
    }
    /// Start a new session with typed options (Pi `newSession({parentSession, withSession})`).
    pub fn new_session_with(&self, opts: &NewSessionOptions) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::NewSession(&opts))
    }
    pub fn switch_session(&self, session_id: &str) -> Result<(), String> {
        self.switch_session_with(session_id, &SwitchSessionOptions::default())
    }
    /// Switch sessions with typed options (Pi `switchSession({withSession})`).
    pub fn switch_session_with(
        &self,
        session_id: &str,
        opts: &SwitchSessionOptions,
    ) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Switch(session_id, &opts))
    }
    pub fn fork(&self, entry_id: &str) -> Result<(), String> {
        self.fork_with(entry_id, &ForkOptions::default())
    }
    /// Fork with typed options (Pi `fork(entryId, {position, withSession})`).
    pub fn fork_with(&self, entry_id: &str, opts: &ForkOptions) -> Result<(), String> {
        let opts = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        control(Control::Fork(entry_id, &opts))
    }
    pub fn navigate(&self, entry_id: &str, opts: impl Serialize) -> Result<(), String> {
        let opts = serde_json::to_string(&opts).unwrap_or_else(|_| "{}".into());
        control(Control::Navigate(entry_id, &opts))
    }
    /// Navigate the session tree with typed options (Pi `navigateTree(targetId, {summarize, …})`).
    pub fn navigate_with(&self, entry_id: &str, opts: &NavigateOptions) -> Result<(), String> {
        self.navigate(entry_id, opts)
    }
    pub fn reload(&self) -> Result<(), String> {
        control(Control::Reload)
    }
    pub fn compact(&self) -> Result<(), String> {
        control(Control::Compact)
    }
    pub fn wait_idle(&self) -> Result<(), String> {
        control(Control::WaitIdle)
    }
    pub fn set_model(&self, model: impl Serialize) -> Result<(), String> {
        let m = serde_json::to_string(&model).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            crate::guest::bindings::cyrup::ext::models::set_model(&m);
            return Ok(());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = m;
            Ok(())
        }
    }
    pub fn send_message(&self, message: impl Serialize, opts: impl Serialize) -> Result<(), String> {
        let m = serde_json::to_string(&message).unwrap_or_else(|_| "null".into());
        let o = serde_json::to_string(&opts).unwrap_or_else(|_| "{}".into());
        control(Control::SendMessage(&m, &o))
    }
    pub fn send_user_message(&self, content: &str, opts: impl Serialize) -> Result<(), String> {
        let o = serde_json::to_string(&opts).unwrap_or_else(|_| "{}".into());
        control(Control::SendUserMessage(content, &o))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum Control<'a> {
    NewSession(&'a str),
    Switch(&'a str, &'a str),
    Fork(&'a str, &'a str),
    Navigate(&'a str, &'a str),
    Reload,
    Compact,
    WaitIdle,
    SendMessage(&'a str, &'a str),
    SendUserMessage(&'a str, &'a str),
}

fn control(op: Control<'_>) -> Result<(), String> {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::guest::bindings::cyrup::ext::control as c;
        return match op {
            Control::NewSession(o) => c::new_session(o),
            Control::Switch(id, o) => c::switch(id, o),
            Control::Fork(id, o) => c::fork(id, o),
            Control::Navigate(id, o) => c::navigate(id, o),
            Control::Reload => c::reload(),
            Control::Compact => c::compact(),
            Control::WaitIdle => c::wait_idle(),
            Control::SendMessage(m, o) => c::send_message(m, o),
            Control::SendUserMessage(co, o) => c::send_user_message(co, o),
        };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = op;
        Ok(())
    }
}

/// The call passed to a guest tool's `execute` (Pi `ToolDefinition.execute` args, types.ts:464).
/// Carries the `toolCallId`, parsed `params`, and a [`Ctx`]; `emit_update` streams partial output
/// back to the runtime (Pi `onUpdate`).
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub call_id: String,
    pub params: Value,
    pub ctx: Ctx,
}

impl ToolCall {
    pub fn new(call_id: impl Into<String>, params: Value) -> Self {
        Self { call_id: call_id.into(), params, ctx: Ctx }
    }
    /// Stream a partial-output chunk (Pi `onUpdate`).
    pub fn emit_update(&self, chunk: impl Serialize) {
        let chunk_json = serde_json::to_string(&chunk).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::host_tool::emit_update(&self.call_id, &chunk_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = chunk_json;
    }
}
