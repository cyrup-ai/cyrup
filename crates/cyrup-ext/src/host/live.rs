//! Live WASM-guest dispatch (arch-08b, gap-08 #1). Binds the `cyrup:ext` world with
//! `wasmtime::component::bindgen!`, implements every host-import interface against [`GuestState`],
//! and exposes [`LiveExtension`] — a real loaded `.wasm` component that implements the unified
//! [`Extension`] trait. The loader calls the guest's `init` export once (R-08-001), reads back the
//! subscriptions/registrations it declared, then dispatches each subscribed event to the matching
//! `on-*` export, decoding the guest's `hook-outcome` into the host block/mutate/notify reduction.
//!
//! Containment (R-08-036): every guest call is run inside the epoch deadline and any trap/OOM/epoch
//! timeout is mapped to a typed `ExtError` and surfaced — the host never crashes.

use crate::contract::{EventPatch, HandledValue, HookOutcome};
use crate::error::ExtError;
use crate::event::{
    EventKind, HostEvent, InputEventSource, InputStreamingBehavior, Subscriptions,
};
use crate::extension::{ExtKind, Extension};
use crate::host::engine::map_wasm_error;
use crate::host::limits::StoreLimits;
use crate::host::services::{
    ControlOp, DialogOptions, ExecOutput, GuestState, NotifyKind, OAuthEvent,
};
use crate::host::store_state::HostState;
use crate::native::CtxTier;
use crate::registry::{CommandDescriptor, ExecModeWire, ToolDescriptor};
use cyrup_core::{
    CancelToken, Content, ExtensionId, Message, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdate, ToolUpdateSink,
};
use serde_json::Value;
use std::sync::Arc;
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

/// The generated `cyrup:ext` bindings. Wrapped in a module so the world struct (`Extension`) does
/// not collide with our [`crate::Extension`] trait.
pub mod bindings {
    wasmtime::component::bindgen!({
        world: "extension",
        path: "wit",
        imports: { default: async },
        exports: { default: async },
    });
}

use bindings::cyrup::ext::types as wit_types;

// ---------------------------------------------------------------------------
// Host-import implementations: the guest's registration + capability calls land here.
// ---------------------------------------------------------------------------

/// Helper: fetch the guest backing or surface a trap-free error string (imports never panic).
fn guest_of(state: &HostState) -> Result<&Arc<GuestState>, String> {
    state.guest.as_ref().ok_or_else(|| "no guest state in store".to_string())
}

/// Map the WIT `notify-kind` severity onto the host-owned [`NotifyKind`] (keeps bindgen types out
/// of [`GuestState`]'s public surface).
fn notify_kind_from_wit(kind: bindings::cyrup::ext::ui::NotifyKind) -> NotifyKind {
    use bindings::cyrup::ext::ui::NotifyKind as Wit;
    match kind {
        Wit::Info => NotifyKind::Info,
        Wit::Warning => NotifyKind::Warning,
        Wit::Error => NotifyKind::Error,
    }
}

// The `types` interface carries only type definitions; its (empty) Host trait still needs an impl.
impl bindings::cyrup::ext::types::Host for HostState {}

impl bindings::cyrup::ext::registration::Host for HostState {
    async fn register_tool(&mut self, t: wit_types::ToolDescriptor) {
        let Ok(guest) = guest_of(self) else { return };
        let parameters: Value = serde_json::from_str(&t.parameters_json).unwrap_or(Value::Null);
        let desc = ToolDescriptor {
            name: t.name,
            label: t.label,
            description: t.description,
            parameters,
            execution_mode: t.exec_mode.map(|m| match m {
                wit_types::ExecMode::Parallel => ExecModeWire::Parallel,
                wit_types::ExecMode::Sequential => ExecModeWire::Sequential,
            }),
            prompt_snippet: t.prompt_snippet,
            prompt_guidelines: t.prompt_guidelines,
            has_renderer: t.has_renderer,
        };
        // A guest tool is dispatched back across the boundary; register it via the registry's
        // descriptor table so the active-tool set can surface it (R-08-012/014).
        let _ = guest.registry.register_guest_tool(guest.owner.clone(), desc);
    }

    async fn register_command(&mut self, name: String, desc_json: String) {
        let Ok(guest) = guest_of(self) else { return };
        let desc: CommandDescriptor = serde_json::from_str(&desc_json).unwrap_or_default();
        guest.push_command(name, desc);
    }

    async fn register_shortcut(&mut self, key: String, _desc: String) {
        let Ok(guest) = guest_of(self) else { return };
        let _ = guest.registry.register_shortcut(guest.owner.clone(), key);
    }

    async fn register_flag(&mut self, name: String, spec_json: String) {
        let Ok(guest) = guest_of(self) else { return };
        let spec: Value = serde_json::from_str(&spec_json).unwrap_or(Value::Null);
        guest.set_flag(name.clone(), spec.clone());
        let _ = guest.registry.set_flag(name, spec);
    }

    async fn get_flag(&mut self, name: String) -> Option<String> {
        let guest = guest_of(self).ok()?;
        guest.get_flag(&name)
    }

    async fn register_provider(&mut self, id: String, config_json: String) {
        let Ok(guest) = guest_of(self) else { return };
        let config: Value = serde_json::from_str(&config_json).unwrap_or(Value::Null);
        let _ = guest.registry.register_provider(guest.owner.clone(), id, config);
    }

    async fn unregister_provider(&mut self, id: String) {
        let Ok(guest) = guest_of(self) else { return };
        let _ = guest.registry.unregister_provider(&id);
    }

    async fn register_message_renderer(&mut self, custom_type: String) {
        let Ok(guest) = guest_of(self) else { return };
        guest.add_renderer(custom_type);
    }

    async fn add_autocomplete(&mut self, command: String) {
        let Ok(guest) = guest_of(self) else { return };
        guest.add_autocomplete(command);
    }

    async fn add_autocomplete_provider(&mut self) {
        if let Ok(guest) = guest_of(self) {
            guest.add_autocomplete_provider();
        }
    }

    async fn subscribe(&mut self, event_kinds: Vec<u8>) {
        let Ok(guest) = guest_of(self) else { return };
        for k in event_kinds {
            if let Some(kind) = EventKind::from_u8(k) {
                guest.add_subscription(kind);
            }
        }
    }
}

impl bindings::cyrup::ext::ui::Host for HostState {
    async fn notify(&mut self, message: String, kind: bindings::cyrup::ext::ui::NotifyKind) {
        if let Ok(guest) = guest_of(self) {
            guest.notify(message, notify_kind_from_wit(kind));
        }
    }
    async fn set_status(&mut self, key: String, text: Option<String>) {
        if let Ok(guest) = guest_of(self) {
            guest.set_status(key, text);
        }
    }
    async fn abort_signal(&mut self, signal_id: String) {
        if let Ok(guest) = guest_of(self) {
            guest.abort_signal(signal_id);
        }
    }
    async fn confirm(&mut self, prompt: String, message: String, opts_json: String) -> bool {
        let opts = DialogOptions::parse(&opts_json);
        let Ok(guest) = guest_of(self) else { return false };
        // Programmatic dismiss (Pi `signal`): a dialog bound to an aborted signal returns cancelled.
        if guest.dialog_dismissed(&opts) {
            return false;
        }
        // Closes the still-open epoch-budget finding (`GuestState::dialog_extra_ticks`'s doc): the
        // wall-clock duration this blocks waiting on a human is recorded so the
        // `epoch_deadline_callback` (registered on this store, [`LiveExtension::load`]) can forgive
        // EXACTLY that much instead of letting the deadline trap the instant the guest resumes wasm
        // execution right after this call returns.
        let started = std::time::Instant::now();
        let result = guest.services.confirm(&prompt, &message, &opts);
        guest.note_dialog_wait(started);
        result
    }
    async fn input(&mut self, prompt: String, placeholder: Option<String>, opts_json: String) -> Option<String> {
        let opts = DialogOptions::parse(&opts_json);
        let guest = guest_of(self).ok()?;
        if guest.dialog_dismissed(&opts) {
            return None;
        }
        let started = std::time::Instant::now();
        let result = guest.services.input(&prompt, placeholder.as_deref(), &opts);
        guest.note_dialog_wait(started);
        result
    }
    async fn select(&mut self, prompt: String, options_json: String, opts_json: String) -> Option<String> {
        let guest = guest_of(self).ok()?;
        let options: Value = serde_json::from_str(&options_json).unwrap_or(Value::Null);
        let opts = DialogOptions::parse(&opts_json);
        if guest.dialog_dismissed(&opts) {
            return None;
        }
        let started = std::time::Instant::now();
        let result = guest.services.select(&prompt, &options, &opts);
        guest.note_dialog_wait(started);
        result
    }
    async fn editor(&mut self, title: String, initial: String) -> Option<String> {
        let guest = guest_of(self).ok()?;
        // `ui.editor` blocks the same way (tears the TUI down and waits for `$EDITOR` to exit, an
        // equally human-paced wait) — the SAME epoch-budget exemption applies.
        let started = std::time::Instant::now();
        let result = guest.services.editor(&title, &initial);
        guest.note_dialog_wait(started);
        result
    }
    async fn set_widget(&mut self, widget_json: String) {
        if let Ok(guest) = guest_of(self) {
            let v: Value = serde_json::from_str(&widget_json).unwrap_or(Value::Null);
            guest.set_widget(v);
        }
    }
    async fn set_header(&mut self, content: String) {
        if let Ok(guest) = guest_of(self) {
            guest.set_header(content);
        }
    }
    async fn set_footer(&mut self, content: String) {
        if let Ok(guest) = guest_of(self) {
            guest.set_footer(content);
        }
    }
    async fn set_title(&mut self, title: String) {
        if let Ok(guest) = guest_of(self) {
            guest.set_title(title);
        }
    }
    async fn custom(&mut self, spec_json: String) -> Option<String> {
        let guest = guest_of(self).ok()?;
        let spec: Value = serde_json::from_str(&spec_json).unwrap_or(Value::Null);
        guest.services.custom(&spec)
    }
    async fn get_editor_text(&mut self) -> String {
        guest_of(self).map(|g| g.services.editor_text()).unwrap_or_default()
    }
    async fn set_editor_text(&mut self, text: String) {
        if let Ok(guest) = guest_of(self) {
            guest.editor_write(text, false);
        }
    }
    async fn paste_editor_text(&mut self, text: String) {
        if let Ok(guest) = guest_of(self) {
            guest.editor_write(text, true);
        }
    }
    async fn theme_get(&mut self) -> Option<String> {
        guest_of(self).ok().and_then(|g| g.services.theme())
    }
    async fn theme_list(&mut self) -> String {
        guest_of(self).map(|g| g.services.theme_list().to_string()).unwrap_or_else(|_| "[]".into())
    }
    async fn theme_set(&mut self, name: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.services.set_theme(&name)?;
        guest.theme_set(name);
        Ok(())
    }
    async fn working_start(&mut self, label: String) {
        if let Ok(guest) = guest_of(self) {
            guest.working(Some(label));
        }
    }
    async fn working_stop(&mut self) {
        if let Ok(guest) = guest_of(self) {
            guest.working(None);
        }
    }
    async fn get_tools_expanded(&mut self) -> bool {
        guest_of(self).map(|g| g.services.tools_expanded()).unwrap_or(false)
    }
    async fn set_tools_expanded(&mut self, expanded: bool) {
        if let Ok(guest) = guest_of(self) {
            guest.set_tools_expanded(expanded);
        }
    }
}

impl bindings::cyrup::ext::session::Host for HostState {
    async fn entries_json(&mut self) -> String {
        guest_of(self).map(|g| g.services.entries().to_string()).unwrap_or_else(|_| "[]".into())
    }
    async fn branch_json(&mut self) -> String {
        guest_of(self).map(|g| g.services.branch().to_string()).unwrap_or_else(|_| "[]".into())
    }
    async fn tree_json(&mut self) -> String {
        guest_of(self).map(|g| g.services.tree().to_string()).unwrap_or_else(|_| "null".into())
    }
    async fn append_entry(&mut self, custom_type: String, data_json: String) -> Result<String, String> {
        let guest = guest_of(self)?;
        let data: Value = serde_json::from_str(&data_json).map_err(|e| e.to_string())?;
        guest.services.append_entry(&custom_type, &data)
    }
    async fn set_session_name(&mut self, name: String) {
        // Route to the pluggable backend: the default host no-ops, the session service renames the
        // live session tree (Pi `setSessionName`, agent-session.ts:2272-2274).
        if let Ok(guest) = guest_of(self) {
            guest.services.set_session_name(&name);
        }
    }
    async fn get_session_name(&mut self) -> Option<String> {
        guest_of(self).ok().and_then(|g| g.services.session_name())
    }
    async fn set_label(&mut self, entry_id: String, label: String) {
        // Route to the pluggable backend (Pi `setLabel`, agent-session.ts:2276-2279); the session
        // service applies it to the live tree via `append_label`.
        if let Ok(guest) = guest_of(self) {
            guest.services.set_label(&entry_id, &label);
        }
    }
}

impl bindings::cyrup::ext::models::Host for HostState {
    async fn list_models(&mut self) -> String {
        guest_of(self).map(|g| g.services.models().to_string()).unwrap_or_else(|_| "[]".into())
    }
    async fn current(&mut self) -> Option<String> {
        guest_of(self).ok().and_then(|g| g.services.current_model())
    }
    async fn set_model(&mut self, model_json: String) {
        let Ok(guest) = guest_of(self) else { return };
        // COMMAND-only (deadlock rule): silently dropped from an event handler.
        if guest.require_command_tier().is_err() {
            return;
        }
        let v: Value = serde_json::from_str(&model_json).unwrap_or(Value::Null);
        let _ = guest.services.control(ControlOp::SetModel(v));
    }
    async fn context_usage(&mut self) -> String {
        guest_of(self).map(|g| g.services.context_usage().to_string()).unwrap_or_else(|_| "{}".into())
    }
    async fn thinking_level(&mut self) -> Option<String> {
        guest_of(self).ok().and_then(|g| g.services.thinking_level())
    }
    async fn set_thinking_level(&mut self, level: String) {
        let Ok(guest) = guest_of(self) else { return };
        if guest.require_command_tier().is_err() {
            return;
        }
        let _ = guest.services.control(ControlOp::SetThinkingLevel(level));
    }
}

impl bindings::cyrup::ext::exec::Host for HostState {
    async fn run(
        &mut self,
        cmd: String,
        args: Vec<String>,
        opts_json: String,
    ) -> Result<wit_types::ExecResult, String> {
        let guest = guest_of(self)?;
        let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
        // Resolve the Pi `signal` (exec.ts:66-72): a `signalId` that was already aborted (`ui.abort-signal`)
        // yields a pre-cancelled token so the grant kills the process immediately (the guest is
        // wasm-suspended across this call, so the signal cannot be aborted mid-run). No tier guard
        // (exec is ambient at any tier once the load-time trust gate passed, arch-08 §6.3).
        let cancel = CancelToken::new();
        if let Some(id) = opts.get("signalId").and_then(|v| v.as_str())
            && guest.is_signal_aborted(id)
        {
            cancel.cancel();
        }
        // Closes the SAME class of CRITICAL finding `845f707`/`9ffec1a` closed for `ui.*`/
        // `http-client`: `guest.services.exec` blocks synchronously in host Rust code
        // (`block_in_place`+`block_on` over `LocalProc::exec_argv`, `host_services.rs`) for up to
        // the guest-settable `timeoutMs` PLUS the SIGTERM/SIGKILL grace escalation — potentially far
        // longer than the WASM epoch budget. With no `note_dialog_wait` call, a slow/killed command
        // left the epoch deadline already expired by the time the guest resumed wasm execution right
        // after this call returns, tripping the SAME permanent instance-wedging trap `ui.*` dialogs
        // and `http-client` used to. Record the wait exactly like those do.
        let started = std::time::Instant::now();
        let result = guest.services.exec(&cmd, &args, &opts, cancel);
        guest.note_dialog_wait(started);
        let ExecOutput { code, stdout, stderr, killed } = result?;
        Ok(wit_types::ExecResult { code, stdout, stderr, killed })
    }
}

impl bindings::cyrup::ext::proc::Host for HostState {
    async fn spawn(
        &mut self,
        cmd: String,
        args: Vec<String>,
        env_json: String,
        cwd: Option<String>,
        capture_stderr: bool,
    ) -> Result<u32, String> {
        let guest = guest_of(self)?;
        // `env-json` is a serde_json object map (guest `ctx.proc_spawn`'s `env` bag); an unparsable
        // or non-object payload degrades to no overrides rather than erroring (mirrors `exec.run`'s
        // permissive `opts-json` handling — never a trap-worthy host failure over a guest payload
        // shape mistake).
        let env: Vec<(String, String)> = serde_json::from_str::<Value>(&env_json)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .map(|m| {
                m.into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string()))).collect()
            })
            .unwrap_or_default();
        // Resolve a guest-supplied `cwd` HERE — the true guest/config-authored boundary, matching
        // where Pi's own `resolveConfigPath(definition.cwd)` runs (`server-manager.ts:110`) — before
        // it is ever handed to `HostServices::proc_spawn`. That call also injects ITS OWN
        // host-computed default (the session's project cwd) when `cwd` is omitted, a cyrup-only
        // mechanism with no Pi equivalent; resolving here, once, on the RAW guest string only, keeps
        // that trusted default from ever being re-interpolated (see `ProcSpawnSpec`'s doc,
        // `caps/proc.rs`, for the corruption this closes).
        let spec = crate::caps::proc::ProcSpawnSpec {
            cmd,
            args,
            env,
            cwd: cwd.map(|c| crate::caps::proc::resolve_config_path(&c)),
            capture_stderr,
        };
        guest.services.proc_spawn(&spec)
    }
    async fn write_stdin(&mut self, handle: u32, data: Vec<u8>) -> Result<u32, String> {
        let guest = guest_of(self)?;
        // Same rationale as `exec::Host::run`/`http_client::Host::request` above — `ProcCaps::write_stdin`
        // `.await`s a real pipe write (`stdin.write_all`, `caps/proc.rs`), which can legitimately
        // block for a while if the child isn't currently reading its stdin. No `note_dialog_wait`
        // here left a slow write to re-wedge the instance exactly like the `ui.*`/`http-client` bug
        // class this same mechanism closed.
        let started = std::time::Instant::now();
        let result = guest.services.proc_write_stdin(handle, &data);
        guest.note_dialog_wait(started);
        result
    }
    async fn read_stdout(&mut self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        let guest = guest_of(self)?;
        guest.services.proc_read_stdout(handle, max_bytes)
    }
    async fn read_stderr(&mut self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        let guest = guest_of(self)?;
        guest.services.proc_read_stderr(handle, max_bytes)
    }
    async fn poll_exit(&mut self, handle: u32) -> Option<i32> {
        guest_of(self).ok().and_then(|g| g.services.proc_poll_exit(handle))
    }
    async fn kill(&mut self, handle: u32) -> Result<(), String> {
        let guest = guest_of(self)?;
        // Same rationale — `ProcCaps::kill` runs the real stdin-EOF/SIGTERM/SIGKILL escalation
        // (`caps/proc.rs`), up to `DEFAULT_KILL_GRACE`*2 + `KILL_CONFIRM_TIMEOUT` (~6s worst case)
        // of real wall-clock blocking, far past the WASM epoch budget. Without this, the SAME
        // permanent instance-wedging trap the `ui.*`/`http-client` fix closed re-opens here.
        let started = std::time::Instant::now();
        let result = guest.services.proc_kill(handle);
        guest.note_dialog_wait(started);
        result
    }
}

impl bindings::cyrup::ext::http_client::Host for HostState {
    async fn request(
        &mut self,
        req: bindings::cyrup::ext::http_client::HttpRequest,
    ) -> Result<bindings::cyrup::ext::http_client::HttpResponse, String> {
        let guest = guest_of(self)?;
        let request = crate::caps::http::HttpRequest {
            method: req.method,
            url: req.url,
            headers: req.headers,
            body: req.body,
            timeout_ms: req.timeout_ms,
        };
        // Closes the SAME class of CRITICAL finding `845f707`/the epoch-forgiveness fix closed for
        // `ui.*` dialogs: `guest.services.http_request` blocks synchronously in host Rust code
        // (`LiveHostServices::http_request`, `block_in_place`+`block_on` over a real `reqwest` call)
        // for up to the guest-settable `timeoutMs` (WIT `timeout-ms`, `world.wit`) — potentially far
        // longer than the WASM epoch budget (`WASM_EPOCH_BUDGET_TICKS`, `facade.rs`, ~5s). With NO
        // `note_dialog_wait` call, a slow request left the epoch deadline already expired by the time
        // the guest resumed wasm execution right after this call returns, tripping the SAME permanent
        // instance-wedging trap `ui.*` dialogs used to. Record the wait exactly like `ui::Host`'s
        // `confirm`/`input`/`select`/`editor` do.
        let started = std::time::Instant::now();
        let result = guest.services.http_request(&request);
        guest.note_dialog_wait(started);
        let resp = result?;
        Ok(bindings::cyrup::ext::http_client::HttpResponse {
            status: resp.status,
            headers: resp.headers,
            body: resp.body,
        })
    }
    async fn request_stream(
        &mut self,
        req: bindings::cyrup::ext::http_client::HttpRequest,
    ) -> Result<bindings::cyrup::ext::http_client::HttpStreamResponse, String> {
        let guest = guest_of(self)?;
        let request = crate::caps::http::HttpRequest {
            method: req.method,
            url: req.url,
            headers: req.headers,
            body: req.body,
            timeout_ms: req.timeout_ms,
        };
        // Same rationale as `request` above — opening a streaming connection blocks on the initiating
        // response the same way a non-streaming request does.
        let started = std::time::Instant::now();
        let result = guest.services.http_request_stream(&request);
        guest.note_dialog_wait(started);
        let opened = result?;
        Ok(bindings::cyrup::ext::http_client::HttpStreamResponse {
            handle: opened.handle,
            status: opened.status,
            headers: opened.headers,
        })
    }
    async fn poll_stream_chunk(&mut self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        let guest = guest_of(self)?;
        // Same rationale — `HttpCaps::poll_stream_chunk` `.await`s the underlying stream's `next()`,
        // which can legitimately block for a while on a slow/sparse server-sent stream (the real MCP
        // SSE-over-HTTP transport shape `pi-mcp-adapter` targets).
        let started = std::time::Instant::now();
        let result = guest.services.http_poll_stream_chunk(handle);
        guest.note_dialog_wait(started);
        result
    }
    async fn close_stream(&mut self, handle: u32) {
        if let Ok(guest) = guest_of(self) {
            guest.services.http_close_stream(handle);
        }
    }
}

impl bindings::cyrup::ext::ext_fs::Host for HostState {
    async fn read_file(&mut self, path: String) -> Result<Vec<u8>, String> {
        let guest = guest_of(self)?;
        let resolved = guest.fs.resolve(&path)?;
        std::fs::read(&resolved).map_err(|e| e.to_string())
    }
    async fn write_file(&mut self, path: String, data: Vec<u8>) -> Result<(), String> {
        let guest = guest_of(self)?;
        let resolved = guest.fs.resolve(&path)?;
        std::fs::write(&resolved, data).map_err(|e| e.to_string())
    }
}

impl bindings::cyrup::ext::bus::Host for HostState {
    async fn emit(&mut self, topic: String, payload_json: String) {
        if let Ok(guest) = guest_of(self) {
            let v: Value = serde_json::from_str(&payload_json).unwrap_or(Value::Null);
            guest.bus_emit(topic, v);
        }
    }
}

impl bindings::cyrup::ext::host_tool::Host for HostState {
    async fn emit_update(&mut self, call_id: String, chunk_json: String) {
        if let Ok(guest) = guest_of(self) {
            let chunk: Value = serde_json::from_str(&chunk_json).unwrap_or(Value::Null);
            guest.push_tool_update(call_id, chunk);
        }
    }
    async fn is_cancelled(&mut self, call_id: String) -> bool {
        // The tool `signal` poll (Pi `signal.aborted`): the executing tool's `CancelToken` fired, or
        // a named signal for this `call-id` was aborted (sdk gap #1).
        guest_of(self).map(|g| g.tool_is_cancelled(&call_id)).unwrap_or(false)
    }
}

impl bindings::cyrup::ext::oauth::Host for HostState {
    async fn on_auth(&mut self, url: String, instructions: Option<String>) {
        if let Ok(guest) = guest_of(self) {
            guest.record_oauth_event(OAuthEvent::Auth { url, instructions });
        }
    }
    async fn on_device_code(
        &mut self,
        user_code: String,
        verification_uri: String,
        _interval_seconds: Option<u32>,
        _expires_in_seconds: Option<u32>,
    ) {
        if let Ok(guest) = guest_of(self) {
            guest.record_oauth_event(OAuthEvent::DeviceCode { user_code, verification_uri });
        }
    }
    async fn on_prompt(
        &mut self,
        message: String,
        placeholder: Option<String>,
        allow_empty: bool,
    ) -> Result<String, String> {
        let guest = guest_of(self)?;
        guest.record_oauth_event(OAuthEvent::Prompt { message: message.clone() });
        guest.services.oauth_prompt(&message, placeholder.as_deref(), allow_empty)
    }
    async fn on_progress(&mut self, message: String) {
        if let Ok(guest) = guest_of(self) {
            guest.record_oauth_event(OAuthEvent::Progress { message });
        }
    }
    async fn on_select(&mut self, message: String, options_json: String) -> Option<String> {
        let guest = guest_of(self).ok()?;
        guest.record_oauth_event(OAuthEvent::Select { message: message.clone() });
        let options: Value = serde_json::from_str(&options_json).unwrap_or(Value::Null);
        guest.services.oauth_select(&message, &options)
    }
}

impl bindings::cyrup::ext::provider_stream::Host for HostState {
    async fn emit_event(&mut self, stream_id: String, event_json: String) {
        if let Ok(guest) = guest_of(self) {
            let event: Value = serde_json::from_str(&event_json).unwrap_or(Value::Null);
            guest.push_stream_event(stream_id, event);
        }
    }
}

impl bindings::cyrup::ext::ext_tools::Host for HostState {
    async fn get_active_tools(&mut self) -> String {
        let Ok(guest) = guest_of(self) else { return "[]".into() };
        // Honour the guest's restriction if set; else surface every registered extension/guest tool.
        let names: Vec<String> = match guest.active_tools_restriction() {
            Some(r) => r,
            None => guest.registry.all_registered_tool_names().unwrap_or_default(),
        };
        serde_json::to_string(&names).unwrap_or_else(|_| "[]".into())
    }
    async fn get_all_tools(&mut self) -> String {
        let Ok(guest) = guest_of(self) else { return "[]".into() };
        serde_json::to_string(&guest.registry.tool_info().unwrap_or_default())
            .unwrap_or_else(|_| "[]".into())
    }
    async fn set_active_tools(&mut self, names_json: String) {
        if let Ok(guest) = guest_of(self) {
            let names: Vec<String> = serde_json::from_str(&names_json).unwrap_or_default();
            guest.set_active_tools_restriction(names);
        }
    }
    async fn get_commands(&mut self) -> String {
        let Ok(guest) = guest_of(self) else { return "[]".into() };
        let names = guest.registry.command_names().unwrap_or_default();
        let infos: Vec<Value> = names.into_iter().map(|n| serde_json::json!({ "name": n })).collect();
        serde_json::to_string(&infos).unwrap_or_else(|_| "[]".into())
    }
}

/// Extract a `withSessionCallbackId` from a `control.*` opts payload and schedule the guest
/// `with-session` re-binding callback to run after the command body returns (Pi
/// `finishSessionReplacement` runs `withSession` after the replacement, agent-session-runtime.ts:184;
/// sdk gap #3). Wasm single-instance reentrancy forbids invoking the export synchronously here.
fn schedule_with_session(guest: &GuestState, opts: &Value) {
    if let Some(id) = opts.get("withSessionCallbackId").and_then(|v| v.as_str()) {
        guest.push_pending_with_session(id.to_string());
    }
}

impl bindings::cyrup::ext::control::Host for HostState {
    async fn new_session(&mut self, opts_json: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
        guest.services.control(ControlOp::NewSession { opts: opts.clone() })?;
        schedule_with_session(guest, &opts);
        Ok(())
    }
    async fn switch(&mut self, session_id: String, opts_json: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
        guest.services.control(ControlOp::Switch { session_id, opts: opts.clone() })?;
        schedule_with_session(guest, &opts);
        Ok(())
    }
    async fn fork(&mut self, entry_id: String, opts_json: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
        guest.services.control(ControlOp::Fork { entry_id, opts: opts.clone() })?;
        schedule_with_session(guest, &opts);
        Ok(())
    }
    async fn navigate(&mut self, entry_id: String, opts_json: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
        guest.services.control(ControlOp::Navigate { entry_id, opts })
    }
    async fn reload(&mut self) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        guest.services.control(ControlOp::Reload)
    }
    async fn compact(&mut self) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        guest.services.control(ControlOp::Compact)
    }
    async fn wait_idle(&mut self) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        guest.services.control(ControlOp::WaitIdle)
    }
    async fn send_message(&mut self, message_json: String, opts_json: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let message: Value = serde_json::from_str(&message_json).unwrap_or(Value::Null);
        let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
        guest.services.control(ControlOp::SendMessage { message, opts })
    }
    async fn send_user_message(&mut self, content: String, opts_json: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
        guest.services.control(ControlOp::SendUserMessage { content, opts })
    }
}

// ---------------------------------------------------------------------------
// LiveExtension: a loaded `.wasm` component as a unified Extension.
// ---------------------------------------------------------------------------

/// A live loaded WASM extension (arch-08b). Holds the instantiated component + its `Store` behind an
/// async `Mutex` (single-thread-per-Store, R-ARCH-EXT-013) and the `GuestState` that backs its
/// imports. Implements [`Extension`]; the dispatcher treats it identically to a native built-in.
pub struct LiveExtension {
    id: ExtensionId,
    subs: Subscriptions,
    epoch_ticks: u64,
    guest: Arc<GuestState>,
    inner: tokio::sync::Mutex<LiveInner>,
}

struct LiveInner {
    store: Store<HostState>,
    instance: bindings::Extension,
}

impl LiveExtension {
    /// Load + instantiate a component and run its `init` export (R-08-001). After `init` returns,
    /// the declared subscriptions are read back and the registered commands are flushed into the
    /// registry. The `services` backend supplies interactive capabilities (default: deny-all).
    pub async fn load(
        engine: &Engine,
        id: ExtensionId,
        bytes: &[u8],
        limits: StoreLimits,
        guest: Arc<GuestState>,
        epoch_ticks: u64,
    ) -> Result<Self, ExtError> {
        let component =
            Component::from_binary(engine, bytes).map_err(|e| ExtError::Component(e.to_string()))?;

        let mut linker = Linker::<HostState>::new(engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|e| ExtError::Engine(e.to_string()))?;
        bindings::Extension::add_to_linker::<HostState, wasmtime::component::HasSelf<HostState>>(
            &mut linker,
            |s| s,
        )
        .map_err(|e| ExtError::Engine(e.to_string()))?;

        let mut store = Store::new(engine, HostState::with_guest(limits, guest.clone()));
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(epoch_ticks);
        guest.arm_epoch_deadline_estimate(epoch_ticks);
        // Closes the still-open finding that the epoch budget bounds the ENTIRE `ui.*` dialog wait: by
        // default a deadline reached mid-execution traps immediately (`epoch_deadline_trap`, wasmtime's
        // default) — but the epoch check only fires at a WASM checkpoint, which for a call like
        // `ui.confirm` is the instant the guest resumes wasm execution right after the (possibly
        // long, human-paced) blocking host call returns; a real trap there wedges the whole instance
        // for the rest of the session (component-model reentrance bookkeeping never sees a clean
        // completion — reproduced empirically: a 6s-delayed reply still resolved `Ok`, but every
        // later call against the same instance silently no-op'd). Replace the default trap with a
        // callback that forgives EXACTLY the guest's REMAINING (unused) budget at the moment it
        // entered the dialog wait (`GuestState::take_dialog_extra_ticks`'s doc — NOT the wait duration
        // itself, which double-counts: `UpdateDeadline::Continue(delta)` extends from the CURRENT
        // epoch, which has already advanced by the wait duration by the time this callback fires) and
        // extends the deadline by that much instead of trapping; a deadline reached with NO recorded
        // dialog wait (a genuine runaway/looping guest) still traps exactly as before —
        // `UpdateDeadline::Interrupt` is wasmtime's own explicit "halt and trap" variant, so this is
        // not a weaker budget, only a correctly-scoped one.
        store.epoch_deadline_callback(|ctx| {
            let owed =
                ctx.data().guest.as_ref().map(|g| g.take_dialog_extra_ticks()).unwrap_or(0);
            if owed > 0 {
                Ok(wasmtime::UpdateDeadline::Continue(owed))
            } else {
                Ok(wasmtime::UpdateDeadline::Interrupt)
            }
        });

        // init runs at command tier (load time): control ops would be legal here (R-08-008).
        guest.set_tier(CtxTier::Command);
        let instance = bindings::Extension::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(|e| map_wasm_error(&e))?;

        match instance.call_init(&mut store).await {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => return Err(ExtError::Component(format!("init failed: {msg}"))),
            Err(e) => return Err(map_wasm_error(&e)),
        }

        // Flush command registrations declared during init into the registry (R-08-016).
        for (name, desc) in guest.take_commands() {
            guest.registry.register_command(guest.owner.clone(), name, desc)?;
        }

        let subs = guest.subscriptions();
        Ok(Self {
            id,
            subs,
            epoch_ticks,
            guest,
            inner: tokio::sync::Mutex::new(LiveInner { store, instance }),
        })
    }

    /// Read-only access to the guest backing (notifications, bus emits, etc.) for tests/diagnostics.
    pub fn guest(&self) -> &Arc<GuestState> {
        &self.guest
    }

    /// Execute a guest-registered tool (R-08-015). Mirrors Pi's `ToolDefinition.execute`: passes the
    /// `call_id`/`params`, races against `cancel` (the `signal`), and replays the streamed `onUpdate`
    /// chunks into `on_update` once the call settles. A guest fault is contained as a `ToolError`.
    pub async fn execute_tool(
        &self,
        name: &str,
        call_id: &ToolCallId,
        params: &Value,
        cancel: &CancelToken,
        on_update: &mut ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Event);
        // Bind this call's CancelToken so the guest's `signal` poll (`host-tool.is-cancelled`) reads
        // the live cancellation state during a long `execute` (Pi `signal` param, sdk gap #1).
        self.guest.set_tool_cancel(Some(cancel.clone()));
        let params_s = params.to_string();
        let api = inner.instance.cyrup_ext_events();
        let call = api.call_execute_tool(&mut inner.store, name, call_id.as_str(), &params_s);
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.guest.set_tool_cancel(None);
                return Err(ToolError::new("tool execution cancelled"));
            }
            r = call => r,
        };
        self.guest.set_tool_cancel(None);
        // Replay streamed updates (Pi onUpdate) into the runtime sink.
        for (_cid, chunk) in self.guest.take_tool_updates() {
            let content = chunk
                .get("content")
                .cloned()
                .and_then(|c| serde_json::from_value::<Vec<Content>>(c).ok())
                .unwrap_or_default();
            on_update(ToolUpdate {
                content,
                details: chunk.get("details").cloned(),
                // Pi's `partialResult` is an `AgentToolResult`, which may carry `terminate`
                // (types.ts:359); thread it through from the guest's update chunk.
                terminate: chunk.get("terminate").and_then(Value::as_bool),
            });
        }
        match res {
            Ok(Ok(out)) => {
                let content = serde_json::from_str::<Vec<Content>>(&out.content_json)
                    .unwrap_or_else(|_| vec![Content::text(out.content_json.clone())]);
                let details = out
                    .details_json
                    .and_then(|d| serde_json::from_str::<Value>(&d).ok());
                Ok(ToolResult { content, details, terminate: out.terminate })
            }
            Ok(Err(msg)) => Err(ToolError::new(msg)),
            Err(e) => Err(ToolError::new(map_wasm_error(&e).to_string())),
        }
    }

    /// Execute a guest-registered slash command (R-08-016). Mirrors Pi's
    /// `RegisteredCommand.handler(args, ctx)` (types.ts:1109): runs at COMMAND tier (session-control
    /// ops are legal), passes the raw `args` string, returns the optional text result. A guest fault
    /// is contained as a typed `ExtError`.
    pub async fn execute_command(
        &self,
        name: &str,
        args: &str,
        cancel: &CancelToken,
    ) -> Result<Option<String>, ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        // Command tier: control ops are legal from a command handler (R-08-008).
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        let call = api.call_execute_command(&mut inner.store, name, args);
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ExtError::Cancelled),
            r = call => r,
        };
        let out = match res {
            Ok(Ok(out)) => out,
            Ok(Err(msg)) => return Err(ExtError::Component(msg)),
            Err(e) => return Err(map_wasm_error(&e)),
        };
        // Run any `withSession` re-binding callbacks the command scheduled via a `control.*` op (Pi
        // `finishSessionReplacement`, sdk gap #3). Each is a FRESH top-level `with-session` export call
        // at command tier — reentrancy forbids invoking it inside the `control.*` import. A faulting
        // callback is contained as a typed `ExtError` (the command's output is preserved on success).
        for callback_id in self.guest.take_pending_with_session() {
            inner.store.set_epoch_deadline(self.epoch_ticks);
            self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
            self.guest.set_tier(CtxTier::Command);
            let api = inner.instance.cyrup_ext_events();
            let cb = api.call_with_session(&mut inner.store, &callback_id);
            let r = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ExtError::Cancelled),
                r = cb => r,
            };
            match r {
                Ok(Ok(())) => {}
                Ok(Err(msg)) => return Err(ExtError::Component(msg)),
                Err(e) => return Err(map_wasm_error(&e)),
            }
        }
        Ok(out)
    }

    /// Execute a guest-registered keyboard shortcut (R-08-017; Pi `registerShortcut` handler,
    /// types.ts:1199-1205). The host calls this when the registered `KeyId` fires. Runs at COMMAND
    /// tier (Pi hands the shortcut handler the full `ExtensionContext`, so session-control ops are
    /// legal). A guest fault is contained as a typed `ExtError`.
    pub async fn execute_shortcut(&self, key: &str, cancel: &CancelToken) -> Result<(), ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        let call = api.call_execute_shortcut(&mut inner.store, key);
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ExtError::Cancelled),
            r = call => r,
        };
        match res {
            Ok(Ok(())) => Ok(()),
            Ok(Err(msg)) => Err(ExtError::Component(msg)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Dynamic argument completions for a guest command (Pi `getArgumentCompletions(prefix)`,
    /// types.ts:1108). A fault is surfaced as a typed `ExtError`.
    pub async fn argument_completions(
        &self,
        name: &str,
        prefix: &str,
    ) -> Result<Vec<String>, ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        match api.call_get_argument_completions(&mut inner.store, name, prefix).await {
            Ok(v) => Ok(v),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Render a tool call via a guest-registered message renderer (Pi `renderCall`, types.ts:472).
    /// Returns the serialized widget tree, or `None` to fall back to the default renderer.
    pub async fn render_call(
        &self,
        custom_type: &str,
        call: &Value,
    ) -> Result<Option<Value>, ExtError> {
        self.render(custom_type, call, true).await
    }

    /// Render a tool result via a guest-registered renderer (Pi `renderResult`, types.ts:481).
    pub async fn render_result(
        &self,
        custom_type: &str,
        result: &Value,
    ) -> Result<Option<Value>, ExtError> {
        self.render(custom_type, result, false).await
    }

    /// Run the guest provider's `login(callbacks)` flow (Pi `oauth.login`, host gap #1). Runs at
    /// COMMAND tier (user-initiated `/login`); during it the guest drives the `oauth` host imports.
    /// Returns the credentials JSON to persist. A guest fault is contained as a typed `ExtError`.
    pub async fn provider_login(&self, id: &str) -> Result<Value, ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        match api.call_provider_login(&mut inner.store, id).await {
            Ok(Ok(s)) => Ok(serde_json::from_str(&s).unwrap_or(Value::Null)),
            Ok(Err(msg)) => Err(ExtError::Component(msg)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Refresh a guest provider's expired credentials (Pi `oauth.refreshToken`).
    pub async fn provider_refresh_token(
        &self,
        id: &str,
        credentials: &Value,
    ) -> Result<Value, ExtError> {
        let creds = credentials.to_string();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        match api.call_provider_refresh_token(&mut inner.store, id, &creds).await {
            Ok(Ok(s)) => Ok(serde_json::from_str(&s).unwrap_or(Value::Null)),
            Ok(Err(msg)) => Err(ExtError::Component(msg)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Derive a guest provider's API key from its credentials (Pi `oauth.getApiKey`).
    pub async fn provider_get_api_key(
        &self,
        id: &str,
        credentials: &Value,
    ) -> Result<String, ExtError> {
        let creds = credentials.to_string();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        match api.call_provider_get_api_key(&mut inner.store, id, &creds).await {
            Ok(Ok(key)) => Ok(key),
            Ok(Err(msg)) => Err(ExtError::Component(msg)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Rewrite a guest provider's models given its credentials (Pi optional `oauth.modifyModels`).
    pub async fn provider_modify_models(
        &self,
        id: &str,
        models: &Value,
        credentials: &Value,
    ) -> Result<Value, ExtError> {
        let models_s = models.to_string();
        let creds = credentials.to_string();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        match api.call_provider_modify_models(&mut inner.store, id, &models_s, &creds).await {
            Ok(Ok(s)) => Ok(serde_json::from_str(&s).unwrap_or(Value::Null)),
            Ok(Err(msg)) => Err(ExtError::Component(msg)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Run a guest provider's custom `streamSimple` (Pi `streamSimple`, host gap #1). The guest
    /// pushes assistant-message events via the `provider-stream` import (recorded in `GuestState`);
    /// this returns once the stream ends. `stream_id` keys the emitted events.
    pub async fn provider_stream_simple(
        &self,
        id: &str,
        stream_id: &str,
        model: &Value,
        context: &Value,
        options: &Value,
    ) -> Result<(), ExtError> {
        let model_s = model.to_string();
        let context_s = context.to_string();
        let options_s = options.to_string();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        match api
            .call_provider_stream_simple(&mut inner.store, id, stream_id, &model_s, &context_s, &options_s)
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(msg)) => Err(ExtError::Component(msg)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Fold the guest's stacked autocomplete providers over the host's built-in suggestions (Pi the
    /// `AutocompleteProviderFactory` chain, host gap #3). `base` = the built-in `AutocompleteSuggestions`
    /// (none = the host had no suggestion), `query` = `{lines, cursorLine, cursorCol, force}`. Returns
    /// the final suggestions after every stacked provider.
    pub async fn autocomplete_suggest(
        &self,
        base: Option<&Value>,
        query: &Value,
    ) -> Result<Value, ExtError> {
        let base_s = base.map(|v| v.to_string()).unwrap_or_else(|| "null".into());
        let query_s = query.to_string();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Command);
        let api = inner.instance.cyrup_ext_events();
        match api.call_autocomplete_suggest(&mut inner.store, &base_s, &query_s).await {
            Ok(s) => Ok(serde_json::from_str(&s).unwrap_or(Value::Null)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    async fn render(
        &self,
        custom_type: &str,
        payload: &Value,
        is_call: bool,
    ) -> Result<Option<Value>, ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Event);
        let payload_s = payload.to_string();
        let api = inner.instance.cyrup_ext_events();
        let res = if is_call {
            api.call_render_call(&mut inner.store, custom_type, &payload_s).await
        } else {
            api.call_render_result(&mut inner.store, custom_type, &payload_s).await
        };
        match res {
            Ok(Some(s)) => Ok(serde_json::from_str::<Value>(&s).ok()),
            Ok(None) => Ok(None),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }
}

/// A guest-registered tool surfaced to the agent's active-tool set (R-08-012/014). Executing it
/// dispatches `execute-tool` across the boundary into the live component instance.
pub struct WasmTool {
    ext: Arc<LiveExtension>,
    descriptor: ToolDescriptor,
}

impl WasmTool {
    pub fn new(ext: Arc<LiveExtension>, descriptor: ToolDescriptor) -> Self {
        Self { ext, descriptor }
    }
}

#[async_trait::async_trait]
impl Tool for WasmTool {
    fn name(&self) -> &str {
        &self.descriptor.name
    }
    fn parameters(&self) -> &Value {
        &self.descriptor.parameters
    }
    fn execution_mode(&self) -> cyrup_core::ExecMode {
        match self.descriptor.execution_mode {
            Some(ExecModeWire::Sequential) => cyrup_core::ExecMode::Sequential,
            _ => cyrup_core::ExecMode::Parallel,
        }
    }
    fn description(&self) -> &str {
        &self.descriptor.description
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        params: Value,
        cancel: CancelToken,
        mut on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.ext
            .execute_tool(&self.descriptor.name, &call_id, &params, &cancel, &mut on_update)
            .await
    }
}

#[async_trait::async_trait]
impl Extension for LiveExtension {
    fn id(&self) -> &ExtensionId {
        &self.id
    }
    fn kind(&self) -> ExtKind {
        ExtKind::Wasm
    }
    fn subscriptions(&self) -> &Subscriptions {
        &self.subs
    }

    async fn invoke_event(
        &self,
        ev: &HostEvent,
        cancel: &CancelToken,
    ) -> Result<HookOutcome, ExtError> {
        let mut guard = self.inner.lock().await;
        // Re-arm the epoch deadline for this call and run at event tier (control ops illegal).
        guard.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Event);

        let kind = ev.kind();
        let call = invoke(&mut guard, ev);
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ExtError::Cancelled),
            r = call => r,
        };
        match outcome {
            Ok(wit) => Ok(decode_outcome(kind, wit)),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }
}

/// Dispatch one event into the matching guest `on-*` export. Notify-only events return `Noop`.
async fn invoke(
    inner: &mut LiveInner,
    ev: &HostEvent,
) -> Result<wit_types::HookOutcome, wasmtime::Error> {
    let store = &mut inner.store;
    let api = inner.instance.cyrup_ext_events();
    let noop = || Ok(wit_types::HookOutcome::Noop);
    match ev {
        HostEvent::ToolCall { call_id, name, input } => {
            api.call_on_tool_call(store, call_id.as_str(), name, &input.to_string()).await
        }
        HostEvent::ToolResult { call_id, name, input, content, details, is_error } => {
            let content_json = serde_json::to_string(content).unwrap_or_else(|_| "[]".into());
            let details_json = details.as_ref().map(|d| d.to_string());
            api.call_on_tool_result(
                store,
                call_id.as_str(),
                name,
                &input.to_string(),
                &content_json,
                *is_error,
                details_json.as_deref(),
            )
            .await
        }
        HostEvent::Context { messages } => {
            let msgs = serde_json::to_string(messages).unwrap_or_else(|_| "[]".into());
            api.call_on_context(store, &msgs).await
        }
        HostEvent::MessageEnd { message } => {
            let m = serde_json::to_string(message).unwrap_or_else(|_| "null".into());
            api.call_on_message_end(store, &m).await
        }
        HostEvent::BeforeAgentStart { prompt, images, system_prompt, options, .. } => {
            api.call_on_before_agent_start(
                store,
                prompt,
                &images.to_string(),
                system_prompt,
                &options.to_string(),
            )
            .await
        }
        HostEvent::Input { text, images, source, streaming_behavior } => {
            let images_json = serde_json::to_string(images).unwrap_or_else(|_| "[]".into());
            let source = input_source_str(*source);
            let behavior = streaming_behavior.map(streaming_behavior_str);
            api.call_on_input(store, text, &images_json, source, behavior).await
        }
        HostEvent::UserBash { command, exclude_from_context, cwd } => {
            api.call_on_user_bash(store, command, *exclude_from_context, cwd).await
        }
        HostEvent::BeforeProviderRequest { payload } => {
            api.call_on_before_provider_request(store, &payload.to_string()).await
        }
        HostEvent::ResourcesDiscover => api.call_on_resources_discover(store).await,
        HostEvent::ProjectTrust => api.call_on_project_trust(store).await,
        HostEvent::SessionBeforeSwitch { target_id } => {
            api.call_on_session_before_switch(store, target_id).await
        }
        HostEvent::SessionBeforeFork { entry_id } => {
            api.call_on_session_before_fork(store, entry_id).await
        }
        HostEvent::SessionBeforeCompact {
            preparation,
            branch_entries,
            custom_instructions,
            reason,
            will_retry,
            ..
        } => {
            api.call_on_session_before_compact(
                store,
                &preparation.to_string(),
                &branch_entries.to_string(),
                custom_instructions.as_deref(),
                reason,
                *will_retry,
            )
            .await
        }
        HostEvent::SessionBeforeTree { preparation, .. } => {
            api.call_on_session_before_tree(store, &preparation.to_string()).await
        }
        // ---- notify-only: fire-and-forget, return Noop ----
        HostEvent::AgentStart => api.call_on_agent_start(store).await.and_then(|()| noop()),
        HostEvent::AgentEnd { messages } => {
            let m = serde_json::to_string(messages).unwrap_or_else(|_| "[]".into());
            api.call_on_agent_end(store, &m).await.and_then(|()| noop())
        }
        HostEvent::TurnStart { turn_index, timestamp } => {
            api.call_on_turn_start(store, *turn_index, *timestamp).await.and_then(|()| noop())
        }
        HostEvent::TurnEnd { turn_index, message, tool_results } => {
            let m = serde_json::to_string(message).unwrap_or_else(|_| "null".into());
            let tr = serde_json::to_string(tool_results).unwrap_or_else(|_| "[]".into());
            api.call_on_turn_end(store, *turn_index, &m, &tr).await.and_then(|()| noop())
        }
        HostEvent::MessageStart { message } => {
            api.call_on_message_start(store, &message.to_string()).await.and_then(|()| noop())
        }
        HostEvent::MessageUpdate { message, delta } => api
            .call_on_message_update(store, &message.to_string(), &delta.to_string())
            .await
            .and_then(|()| noop()),
        HostEvent::ToolExecStart { call_id, name, args } => api
            .call_on_tool_exec_start(store, call_id.as_str(), name, &args.to_string())
            .await
            .and_then(|()| noop()),
        HostEvent::ToolExecUpdate { call_id, chunk } => api
            .call_on_tool_exec_update(store, call_id.as_str(), &chunk.to_string())
            .await
            .and_then(|()| noop()),
        HostEvent::ToolExecEnd { call_id, result, is_error } => api
            .call_on_tool_exec_end(store, call_id.as_str(), &result.to_string(), *is_error)
            .await
            .and_then(|()| noop()),
        HostEvent::SessionStart { reason } => {
            api.call_on_session_start(store, reason).await.and_then(|()| noop())
        }
        HostEvent::SessionShutdown { reason } => {
            api.call_on_session_shutdown(store, reason).await.and_then(|()| noop())
        }
        HostEvent::AfterProviderResponse { status, headers } => api
            .call_on_after_provider_response(store, *status, &headers.to_string())
            .await
            .and_then(|()| noop()),
        HostEvent::ModelSelect { model } => {
            api.call_on_model_select(store, &model.to_string()).await.and_then(|()| noop())
        }
        HostEvent::ThinkingLevelSelect { level } => {
            api.call_on_thinking_level_select(store, level).await.and_then(|()| noop())
        }
        HostEvent::SessionCompact { compaction_entry, from_extension, reason, will_retry } => api
            .call_on_session_compact(
                store,
                &compaction_entry.to_string(),
                *from_extension,
                reason,
                *will_retry,
            )
            .await
            .and_then(|()| noop()),
        HostEvent::SessionTree { tree } => {
            api.call_on_session_tree(store, &tree.to_string()).await.and_then(|()| noop())
        }
    }
}

/// The Pi `InputSource` wire string (types.ts:797) for the `on-input` seam.
fn input_source_str(s: InputEventSource) -> &'static str {
    match s {
        InputEventSource::Interactive => "interactive",
        InputEventSource::Rpc => "rpc",
        InputEventSource::Extension => "extension",
    }
}

/// The Pi `streamingBehavior` wire string (types.ts:809) for the `on-input` seam.
fn streaming_behavior_str(b: InputStreamingBehavior) -> &'static str {
    match b {
        InputStreamingBehavior::Steer => "steer",
        InputStreamingBehavior::FollowUp => "followUp",
    }
}

/// Decode a guest `hook-outcome` into the host reduction, interpreting the mutate/handled JSON patch
/// by the event kind (the §3.3 reducer contract). An unparseable/shape-mismatched patch degrades to
/// `Noop` (never a panic).
fn decode_outcome(kind: EventKind, wit: wit_types::HookOutcome) -> HookOutcome {
    match wit {
        wit_types::HookOutcome::Noop => HookOutcome::Noop,
        wit_types::HookOutcome::Block(reason) => HookOutcome::Block { reason },
        wit_types::HookOutcome::Handled(s) => {
            let v: Value = serde_json::from_str(&s).unwrap_or(Value::Null);
            HookOutcome::Handled(HandledValue(v))
        }
        wit_types::HookOutcome::Mutate(s) => {
            let v: Value = match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(_) => return HookOutcome::Noop,
            };
            match decode_patch(kind, v) {
                Some(p) => HookOutcome::Mutate(p),
                None => HookOutcome::Noop,
            }
        }
    }
}

/// Interpret a guest mutate-payload as a typed [`EventPatch`] for `kind`.
fn decode_patch(kind: EventKind, v: Value) -> Option<EventPatch> {
    match kind {
        EventKind::ToolCall => Some(EventPatch::ToolInput(v)),
        EventKind::ToolResult => {
            let content = v
                .get("content")
                .cloned()
                .and_then(|c| serde_json::from_value::<Vec<Content>>(c).ok());
            let details = v.get("details").cloned();
            let is_error = v.get("isError").and_then(|b| b.as_bool());
            Some(EventPatch::ToolResult { content, details, is_error })
        }
        EventKind::Context => {
            let messages = serde_json::from_value(v).ok()?;
            Some(EventPatch::Context { messages })
        }
        EventKind::MessageEnd => {
            let m: Message = serde_json::from_value(v).ok()?;
            Some(EventPatch::Message(Box::new(m)))
        }
        EventKind::BeforeAgentStart => {
            let system = v.get("systemPrompt").and_then(|s| s.as_str()).map(|s| s.to_string());
            let inject = v
                .get("message")
                .cloned()
                .and_then(|m| serde_json::from_value::<Message>(m).ok())
                .map(Box::new);
            Some(EventPatch::SystemPromptAndInject { system, inject })
        }
        // `input` (Pi `InputEventResult` `{action:"transform", text, images?}`, types.ts:805): the
        // guest's transform rewrites the submission text and optionally its images. `text` is
        // required; a missing `text` is not a transform (degrade to noop). `images` (absent =
        // keep the folded-so-far images, Pi `result.images ?? currentImages`).
        EventKind::Input => {
            let text = v.get("text").and_then(|s| s.as_str())?.to_string();
            let images = v
                .get("images")
                .cloned()
                .and_then(|i| serde_json::from_value::<Vec<Content>>(i).ok());
            Some(EventPatch::Input { text, images })
        }
        // `before_provider_request` (Pi runner.ts:962): the handler's return value REPLACES the
        // payload wholesale. The guest sends the replacement payload as the mutate value.
        EventKind::BeforeProviderRequest => Some(EventPatch::ProviderRequest(v)),
        // `session_before_compact` / `session_before_tree` (Pi `SessionBeforeCompactResult.compaction`
        // / `SessionBeforeTreeResult`): the guest's `mutate` value is the override bag; the producer
        // interprets its shape (summary/details/label).
        EventKind::SessionBeforeCompact => Some(EventPatch::CompactionOverride(v)),
        EventKind::SessionBeforeTree => Some(EventPatch::TreeOverride(v)),
        // Other kinds have no typed patch shape (notify, or `user_bash`/discovery which use the
        // `handled` channel, not `mutate`); ignore.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::host::services::{CannedResponses, RecordingServices};
    use crate::registry::ExtensionRegistry;
    use bindings::cyrup::ext::exec::Host as ExecHost;

    /// A bare [`HostState`] backed by a [`GuestState`]/[`RecordingServices`] pair — no `wasmtime::
    /// Store`/component instantiation needed, since `exec::Host::run` is a plain async trait method
    /// callable directly. Exercises the REAL production `exec::Host::run` implementation, not a
    /// reimplementation of its logic.
    fn state_with(services: Arc<RecordingServices>) -> HostState {
        let guest =
            GuestState::with_services(ExtensionId::from("test"), Arc::new(ExtensionRegistry::new()), services);
        HostState::with_guest(StoreLimits::default(), Arc::new(guest))
    }

    /// Closes the SDK gap this fix addresses: `ExecOptions::signal_id` (`cyrup-ext-sdk::descriptor`)
    /// now round-trips through `exec::Host::run` into a REAL pre-cancelled `CancelToken` — 1:1 with
    /// Pi's `options.signal.aborted` branch (`exec.ts:66-68`) — proven by actually calling the
    /// production host-import implementation, not just asserting the struct field exists.
    #[tokio::test]
    async fn signal_id_already_aborted_starts_exec_pre_cancelled() {
        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec.clone());
        // The guest already aborted this signal id (Pi `signal.abort()`, mirrors a prior
        // `ctx.abort_signal("my-signal")` call reaching `ui::Host::abort_signal`).
        guest_of(&state).expect("guest state present").abort_signal("my-signal".into());

        let opts_json = serde_json::json!({ "signalId": "my-signal" }).to_string();
        let result = ExecHost::run(&mut state, "echo".into(), vec!["hi".into()], opts_json).await;
        assert!(result.is_ok(), "exec still runs (a pre-cancelled token is not a host error)");

        assert_eq!(
            rec.exec_call_pre_cancelled(),
            vec![true],
            "an already-aborted `signalId` must reach `HostServices::exec` as an ALREADY-cancelled \
             token, not merely be recorded and ignored"
        );
    }

    /// The negative case: an UN-aborted `signalId` (or none at all) must NOT pre-cancel — proves the
    /// check is a real conditional, not a hard-coded always-cancel.
    #[tokio::test]
    async fn signal_id_not_aborted_does_not_pre_cancel() {
        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec.clone());
        // Registered but never aborted.
        let _ = guest_of(&state).expect("guest state present");

        let opts_json = serde_json::json!({ "signalId": "never-aborted" }).to_string();
        ExecHost::run(&mut state, "echo".into(), vec!["hi".into()], opts_json)
            .await
            .expect("exec runs");
        assert_eq!(rec.exec_call_pre_cancelled(), vec![false]);

        // No `signalId` in opts at all — same result.
        ExecHost::run(&mut state, "echo".into(), vec!["hi".into()], "{}".into())
            .await
            .expect("exec runs");
        assert_eq!(rec.exec_call_pre_cancelled(), vec![false, false]);
    }

    /// The `proc::Host::spawn` WIT handler — the true guest/config-authored boundary — must resolve
    /// a raw guest `cwd` string (`${VAR}`/`$env:VAR`/leading `~`) itself, matching where Pi's own
    /// `resolveConfigPath(definition.cwd)` runs (`server-manager.ts:110`), BEFORE `HostServices::
    /// proc_spawn` ever sees it. Proven by calling the REAL production `proc::Host::spawn`
    /// implementation directly (no reimplementation) and inspecting the resolved `cwd`
    /// `RecordingServices::proc_spawn` actually received.
    #[tokio::test]
    async fn spawn_resolves_a_raw_guest_cwd_before_it_reaches_host_services() {
        use bindings::cyrup::ext::proc::Host as ProcHost;

        let real_home = std::env::var("HOME").expect("HOME is set in the test environment");
        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec.clone());

        ProcHost::spawn(&mut state, "true".into(), vec![], "{}".into(), Some("~".into()), false)
            .await
            .expect("spawn succeeds");

        let cwds = rec.proc_spawn_cwds();
        assert_eq!(cwds.len(), 1);
        assert_eq!(
            cwds.first().and_then(|c| c.as_deref()).and_then(|p| p.to_str()),
            Some(real_home.as_str()),
            "a raw guest `~` cwd must already be tilde-expanded by the time it reaches \
             HostServices::proc_spawn — NOT left for a later layer to (possibly never) resolve"
        );
    }
}
