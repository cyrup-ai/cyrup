//! The services handle a NATIVE extension is given: the session's [`HostServices`] backend, seen
//! from one extension (EXT-105).
//!
//! pi builds one `ExtensionAPI` per extension (`createExtensionAPI(extension, runtime, …)`,
//! `core/extensions/loader.ts:244-` @f1b2e77f5); the action methods delegate to the shared
//! `runtime`, and the API itself knows which `extension` it belongs to, which is how `getFlag`
//! answers only for that extension's own flags (`:374-378`). cyrup's WASM tier has had the same
//! shape since it was built: each guest's [`super::GuestState`] carries its `owner` and answers
//! `get-flag` from its own flags. A native was handed the session's single backend, which has no
//! notion of who is asking.
//!
//! [`CallerScopedServices`] is that per-extension API for a native. The caller's identity is the
//! [`ExtensionId`] the facade registers the native's flags under and that a guest's `owner` is —
//! one notion of "who is asking" for both tiers, never a second one that must agree with it. It is
//! fixed when the facade hands the handle out ([`crate::ExtensionHost::load_native_with_services`]),
//! so an extension cannot name another one.
//!
//! Every method forwards to the session's backend unchanged except the ones pi scopes by caller.
//! Of the trait's verbs that take a caller-supplied key, that is [`HostServices::flag_value`] alone
//! (the enumeration is in `docs/gap-analysis/06-cyrup-ext.md`, `EXT-105`). A verb added to the trait
//! must be forwarded here too, or a native silently gets its deny default:
//! `every_trait_method_is_forwarded` fails until it is.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{CancelToken, ExtensionId};
use serde_json::Value;

use super::services::*;
use crate::registry::ExtensionRegistry;

/// The session's [`HostServices`] seen from one native extension. See the module docs.
pub struct CallerScopedServices {
    inner: Arc<dyn HostServices>,
    caller: ExtensionId,
    registry: Arc<ExtensionRegistry>,
}

impl CallerScopedServices {
    /// `inner` seen from `caller`, whose declarations `registry` holds.
    #[must_use]
    pub fn new(
        inner: Arc<dyn HostServices>,
        caller: ExtensionId,
        registry: Arc<ExtensionRegistry>,
    ) -> Self {
        Self {
            inner,
            caller,
            registry,
        }
    }

    /// The extension this handle answers for.
    #[must_use]
    pub fn caller(&self) -> &ExtensionId {
        &self.caller
    }
}

impl std::fmt::Debug for CallerScopedServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallerScopedServices")
            .field("caller", &self.caller)
            .finish_non_exhaustive()
    }
}

impl HostServices for CallerScopedServices {
    fn confirm(&self, prompt: &str, message: &str, opts: &DialogOptions) -> bool {
        self.inner.confirm(prompt, message, opts)
    }

    fn input(
        &self,
        prompt: &str,
        placeholder: Option<&str>,
        opts: &DialogOptions,
    ) -> Option<String> {
        self.inner.input(prompt, placeholder, opts)
    }

    fn select(&self, prompt: &str, options: &Value, opts: &DialogOptions) -> Option<String> {
        self.inner.select(prompt, options, opts)
    }

    fn oauth_prompt(
        &self,
        message: &str,
        placeholder: Option<&str>,
        allow_empty: bool,
    ) -> Result<String, String> {
        self.inner.oauth_prompt(message, placeholder, allow_empty)
    }

    fn oauth_select(&self, message: &str, options: &Value) -> Option<String> {
        self.inner.oauth_select(message, options)
    }

    fn editor(&self, title: &str, initial: &str) -> Option<String> {
        self.inner.editor(title, initial)
    }

    fn custom(&self, spec: &Value) -> Option<String> {
        self.inner.custom(spec)
    }

    fn open_overlay(&self, overlay: Box<dyn crate::host::overlay::InteractiveOverlay>) -> bool {
        self.inner.open_overlay(overlay)
    }

    fn editor_text(&self) -> String {
        self.inner.editor_text()
    }

    fn editor_has_focus(&self) -> bool {
        self.inner.editor_has_focus()
    }

    fn theme(&self) -> Option<String> {
        self.inner.theme()
    }

    fn theme_list(&self) -> Value {
        self.inner.theme_list()
    }

    fn theme_by_name(&self, name: &str) -> Option<Value> {
        self.inner.theme_by_name(name)
    }

    fn set_theme(&self, name: &str) -> Result<(), String> {
        self.inner.set_theme(name)
    }

    fn effective_keybindings(&self, namespace: &str) -> Value {
        self.inner.effective_keybindings(namespace)
    }

    fn tools_expanded(&self) -> bool {
        self.inner.tools_expanded()
    }

    fn footer_data(&self) -> Option<String> {
        self.inner.footer_data()
    }

    fn notify(&self, message: &str, kind: NotifyKind) {
        self.inner.notify(message, kind)
    }

    fn set_status(&self, key: &str, text: Option<&str>) {
        self.inner.set_status(key, text)
    }

    fn set_widget(&self, key: &str, lines: Option<&[String]>, placement: WidgetPlacement) {
        self.inner.set_widget(key, lines, placement)
    }

    fn set_working_message(&self, message: Option<&str>) {
        self.inner.set_working_message(message)
    }

    fn set_working_visible(&self, visible: bool) {
        self.inner.set_working_visible(visible)
    }

    fn set_working_indicator(&self, opts: Option<&Value>) {
        self.inner.set_working_indicator(opts)
    }

    fn set_hidden_thinking_label(&self, label: Option<&str>) {
        self.inner.set_hidden_thinking_label(label)
    }

    fn set_header(&self, content: Option<&str>) {
        self.inner.set_header(content)
    }

    fn set_footer(&self, content: Option<&str>) {
        self.inner.set_footer(content)
    }

    fn set_title(&self, title: &str) {
        self.inner.set_title(title)
    }

    fn set_editor_text(&self, text: &str, is_paste: bool) {
        self.inner.set_editor_text(text, is_paste)
    }

    fn set_tools_expanded(&self, expanded: bool) {
        self.inner.set_tools_expanded(expanded)
    }

    fn entries(&self) -> Value {
        self.inner.entries()
    }

    fn branch(&self) -> Value {
        self.inner.branch()
    }

    fn tree(&self) -> Value {
        self.inner.tree()
    }

    fn session_name(&self) -> Option<String> {
        self.inner.session_name()
    }

    fn session_id(&self) -> Option<String> {
        self.inner.session_id()
    }

    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        self.inner.human_interaction_lock()
    }

    fn emit_event(&self, topic: &str, payload: &Value) {
        self.inner.emit_event(topic, payload)
    }

    fn emit_typed_event(&self, topic: &str, event: &dyn std::any::Any) {
        self.inner.emit_typed_event(topic, event)
    }

    fn session_file(&self) -> Option<PathBuf> {
        self.inner.session_file()
    }

    fn inject_message(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        details: Option<&Value>,
        trigger_turn: bool,
    ) -> Result<(), String> {
        self.inner
            .inject_message(content, custom_type, display, details, trigger_turn)
    }

    fn inject_message_ack(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        details: Option<&Value>,
        trigger_turn: bool,
    ) -> Result<tokio::sync::oneshot::Receiver<InjectOutcome>, String> {
        self.inner
            .inject_message_ack(content, custom_type, display, details, trigger_turn)
    }

    fn inject_message_steer(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        details: Option<&Value>,
    ) -> Result<(), String> {
        self.inner
            .inject_message_steer(content, custom_type, display, details)
    }

    fn wake_user_prompt(&self, text: &str) -> Result<(), String> {
        self.inner.wake_user_prompt(text)
    }

    fn send_user_message(
        &self,
        text: &str,
        deliver_as: Option<crate::event::InputStreamingBehavior>,
    ) -> Result<(), String> {
        self.inner.send_user_message(text, deliver_as)
    }

    fn models(&self) -> Value {
        self.inner.models()
    }

    fn current_model(&self) -> Option<String> {
        self.inner.current_model()
    }

    fn current_model_info(&self) -> Option<Value> {
        self.inner.current_model_info()
    }

    fn context_usage(&self) -> Value {
        self.inner.context_usage()
    }

    fn thinking_level(&self) -> Option<String> {
        self.inner.thinking_level()
    }

    fn registered_provider(&self, provider_id: &str) -> Option<Arc<dyn cyrup_provider::Provider>> {
        self.inner.registered_provider(provider_id)
    }

    fn refresh_provider<'a>(
        &'a self,
        provider_id: &'a str,
        allow_network: bool,
        force: bool,
        cancel: CancelToken,
    ) -> futures::future::BoxFuture<'a, cyrup_provider::ModelsRefreshResult> {
        self.inner
            .refresh_provider(provider_id, allow_network, force, cancel)
    }

    /// pi `getFlag`'s gate (`core/extensions/loader.ts:374-378` @f1b2e77f5, the check at `:376`:
    /// `if (!extension.flags.has(name)) return undefined;`): the session's answer, but only for a
    /// flag THIS native declared. See the trait method for the value it answers.
    fn flag_value(&self, name: &str) -> Option<serde_json::Value> {
        if !self.registry.declares_flag(&self.caller, name) {
            return None;
        }
        self.inner.flag_value(name)
    }

    fn provider_auth<'a>(
        &'a self,
        provider_id: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<Option<HostProviderAuth>, String>> {
        self.inner.provider_auth(provider_id)
    }

    fn provider_credentials(
        &self,
        provider_id: &str,
    ) -> Option<Arc<dyn cyrup_provider::CredentialStore>> {
        self.inner.provider_credentials(provider_id)
    }

    fn session_context_messages(
        &self,
    ) -> futures::future::BoxFuture<'_, Vec<cyrup_session::AgentMessage>> {
        self.inner.session_context_messages()
    }

    fn complete_standalone<'a>(
        &'a self,
        request: StandaloneCompletion,
        cancel: CancelToken,
    ) -> futures::future::BoxFuture<
        'a,
        Result<cyrup_core::AssistantMessage, StandaloneCompletionRefusal>,
    > {
        self.inner.complete_standalone(request, cancel)
    }

    fn model_stream(
        &self,
        call: crate::host::ModelCall,
    ) -> cyrup_core::EventStream<cyrup_provider::StreamEvent> {
        self.inner.model_stream(call)
    }

    fn model_complete<'a>(
        &'a self,
        call: crate::host::ModelCall,
    ) -> futures::future::BoxFuture<'a, cyrup_core::AssistantMessage> {
        self.inner.model_complete(call)
    }

    fn exec(
        &self,
        cmd: &str,
        args: &[String],
        opts: &Value,
        cancel: CancelToken,
    ) -> Result<ExecOutput, String> {
        self.inner.exec(cmd, args, opts, cancel)
    }

    fn http_request(&self, req: &HttpRequest) -> Result<HttpResponse, String> {
        self.inner.http_request(req)
    }

    fn http_request_stream(&self, req: &HttpRequest) -> Result<HttpStreamResponse, String> {
        self.inner.http_request_stream(req)
    }

    fn http_poll_stream_chunk(&self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        self.inner.http_poll_stream_chunk(handle)
    }

    fn http_close_stream(&self, handle: u32) {
        self.inner.http_close_stream(handle)
    }

    fn proc_spawn(&self, spec: &ProcSpawnSpec) -> Result<u32, String> {
        self.inner.proc_spawn(spec)
    }

    fn proc_write_stdin(&self, handle: u32, data: &[u8]) -> Result<u32, String> {
        self.inner.proc_write_stdin(handle, data)
    }

    fn proc_read_stdout(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.inner.proc_read_stdout(handle, max_bytes)
    }

    fn proc_read_stderr(&self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.inner.proc_read_stderr(handle, max_bytes)
    }

    fn proc_poll_exit(&self, handle: u32) -> Option<i32> {
        self.inner.proc_poll_exit(handle)
    }

    fn proc_kill(&self, handle: u32) -> Result<(), String> {
        self.inner.proc_kill(handle)
    }

    fn control(&self, op: ControlOp) -> Result<(), String> {
        self.inner.control(op)
    }

    fn is_idle(&self) -> bool {
        self.inner.is_idle()
    }

    fn has_pending_messages(&self) -> bool {
        self.inner.has_pending_messages()
    }

    fn is_project_trusted(&self) -> bool {
        self.inner.is_project_trusted()
    }

    fn system_prompt(&self) -> Option<String> {
        self.inner.system_prompt()
    }

    fn system_prompt_options(&self) -> Option<Value> {
        self.inner.system_prompt_options()
    }

    fn render_system_prompt(&self, options: &Value) -> Result<String, String> {
        self.inner.render_system_prompt(options)
    }

    fn preview_boundary(&self, boundary: &str, entries: &Value) -> Result<Value, String> {
        self.inner.preview_boundary(boundary, entries)
    }

    fn append_entry(&self, custom_type: &str, data: &Value) -> Result<String, String> {
        self.inner.append_entry(custom_type, data)
    }

    fn set_session_name(&self, name: &str) {
        self.inner.set_session_name(name)
    }

    fn set_label(&self, entry_id: &str, label: Option<&str>) {
        self.inner.set_label(entry_id, label)
    }

    fn scoped_models(&self) -> Value {
        self.inner.scoped_models()
    }

    fn is_run_cancelled(&self) -> bool {
        self.inner.is_run_cancelled()
    }

    fn active_tools(&self) -> Option<Vec<String>> {
        self.inner.active_tools()
    }

    fn all_tool_names(&self) -> Option<Vec<String>> {
        self.inner.all_tool_names()
    }

    fn set_active_tools(&self, names: &[String]) {
        self.inner.set_active_tools(names)
    }

    fn all_tools(&self) -> Option<Vec<Value>> {
        self.inner.all_tools()
    }

    fn commands(&self) -> Option<Vec<Value>> {
        self.inner.commands()
    }
}

#[cfg(test)]
mod tests;
