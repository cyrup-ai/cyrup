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
use crate::native::{CtxTier, ExtMode};
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

// --- EXT-054: the manifest capability grant, enforced HOST-SIDE ---------------------------------
//
// The three helpers below are the whole enforcement surface for `capabilities.{exec,net,ui}`. They
// sit at the import boundary rather than inside `HostServices` on purpose: `HostServices` is the
// pluggable BACKEND (what the running host is able to do), while `Capabilities` is the per-extension
// RESTRICTION (what this particular guest was allowed to ask for). Enforcing here means a session
// that injects a fully-capable `LiveHostServices` still cannot hand `exec` to a guest that declared
// `"exec": false` — which is exactly the bypass EXT-054 reproduced, where an all-false manifest ran
// `echo hi` as a real host process and opened a real TLS connection.
//
// `capabilities.fs` needs no helper: its enforcement lives in `FsCaps`, which simply has no root to
// resolve against unless a grant declared one (EXT-055).

/// [`guest_of`] + the `capabilities.exec` gate — `exec.run` and every `proc.*` import.
fn exec_guest_of(state: &HostState) -> Result<&Arc<GuestState>, String> {
    let guest = guest_of(state)?;
    guest.require_exec()?;
    Ok(guest)
}

/// [`guest_of`] + the `capabilities.net` gate — every `http-client.*` import.
fn net_guest_of(state: &HostState) -> Result<&Arc<GuestState>, String> {
    let guest = guest_of(state)?;
    guest.require_net()?;
    Ok(guest)
}

/// [`guest_of`] + the `capabilities.ui` gate — every `ui.*` import. A refused call falls through to
/// the same no-op/`false`/`none` an untrusted extension already gets from [`crate::DenyServices`],
/// because most of `interface ui` has no error channel in its WIT signature.
fn ui_guest_of(state: &HostState) -> Result<&Arc<GuestState>, String> {
    let guest = guest_of(state)?;
    guest.require_ui()?;
    Ok(guest)
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
        let desc_name_for_log = t.name.clone();
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
            // EXT-023 / EXT-024: both fields used to stop at the SDK struct and never reach the
            // registry, so a guest set a documented field that did nothing and got no diagnostic.
            prepare_arguments: t.prepare_arguments,
            render_shell: t.render_shell,
            // PROV-011 / EXT-024: pi `ToolDefinition.constrainedSampling`
            // (`extensions/types.ts:463` @v0.83.0). Parsed ONCE here so a malformed declaration
            // is a registration-time diagnostic instead of a per-turn surprise; an unparseable
            // value degrades to the omitted field, which upstream is indistinguishable from
            // `false`, rather than failing the whole registration — refusing the tool over a
            // sampling HINT would be strictly worse than running it unconstrained.
            constrained_sampling: t.constrained_sampling.as_deref().and_then(|raw| {
                match serde_json::from_str::<cyrup_core::ConstrainedSampling>(raw) {
                    Ok(cs) => Some(cs),
                    Err(e) => {
                        tracing::warn!(
                            tool = %desc_name_for_log,
                            error = %e,
                            "ignoring unparseable constrainedSampling declaration"
                        );
                        None
                    }
                }
            }),
        };
        // A guest tool is dispatched back across the boundary; register it via the registry's
        // descriptor table so the active-tool set can surface it (R-08-012/014).
        let _ = guest.registry.register_guest_tool(guest.owner.clone(), desc);
    }

    /// EXT-058 — pi `registerCommand(name, options)` writes STRAIGHT into the extension's live
    /// `commands` map (`extensions/loader.ts:270-277` @v0.83.0:
    /// `extension.commands.set(name, {name, sourceInfo, ...options})`), and `getCommand`
    /// re-reads that map at every dispatch (`runner.ts:647-649`), so a post-`init`
    /// `registerCommand` takes effect on the next `/name`.
    ///
    /// This used to stage into a per-guest `GuestState.commands` buffer that
    /// [`LiveExtension::load`] drained ONCE, right after `call_init` — a `std::mem::take`, so a
    /// registration made from any LIVE handler was accepted (`Ok`, no error channel in the WIT
    /// signature) and then invisible forever: `resolved_command_owner` returned `None`,
    /// `AgentSession::try_execute_wasm_command` returned `false` with no log, and `/name` went to
    /// the model as an ordinary prompt. Every sibling import in this block already writes through
    /// to the shared registry; this one now does too, which is also what makes it legal for the
    /// `init`-time call (`register_shortcut` proves the registry is reachable during `init`).
    async fn register_command(&mut self, name: String, desc_json: String) {
        let Ok(guest) = guest_of(self) else { return };
        let desc: CommandDescriptor = serde_json::from_str(&desc_json).unwrap_or_default();
        let _ = guest.registry.register_command(guest.owner.clone(), name, desc);
    }

    async fn register_shortcut(&mut self, key: String, desc: String) {
        let Ok(guest) = guest_of(self) else { return };
        // EXT-040: `desc` crossed the boundary and used to be discarded right here, so `/hotkeys`
        // printed the raw key id as its own label. pi keeps it on `ExtensionShortcut.description`
        // (extensions/types.ts:1524-1529 @v0.83.0) and renders `shortcut.description ??
        // shortcut.extensionPath` (interactive-mode.ts:5856). An EMPTY string is upstream's absent
        // field, which falls back to the extension ID — never to the key.
        let desc = if desc.trim().is_empty() { None } else { Some(desc) };
        let _ = guest.registry.register_shortcut(guest.owner.clone(), key, desc);
    }

    async fn register_flag(&mut self, name: String, spec_json: String) {
        let Ok(guest) = guest_of(self) else { return };
        let spec: Value = serde_json::from_str(&spec_json).unwrap_or(Value::Null);
        guest.set_flag(name.clone(), spec.clone());
        // Owner-attributed so Pi's first-wins flag rule (`getFlags`, runner.ts:473-483) and its
        // `Flag "--x" conflicts with <owner>` diagnostic apply to a guest's `registerFlag`.
        let _ = guest.registry.register_flag(guest.owner.clone(), name, spec);
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

    /// EXT-019 — pi `registerMarkdownTransformer(transformer)` (`extensions/types.ts:1292`
    /// @v0.84.1, impl `loader.ts:309-312`). Argument-less: the closure lives guest-side behind the
    /// `transform-markdown` export, so this only records that this guest HAS one, in load order.
    async fn register_markdown_transformer(&mut self) {
        let Ok(guest) = guest_of(self) else { return };
        let _ = guest.registry.register_markdown_transformer(guest.owner.clone());
    }

    async fn register_message_renderer(&mut self, custom_type: String) {
        let Ok(guest) = guest_of(self) else { return };
        // Record it in the SHARED registry so the host can route a custom type back to its owning
        // guest (Pi `getMessageRenderer`, runner.ts:579-587) — the per-guest `GuestState` vec below
        // is host-side bookkeeping only and no consumer can reach it by custom type (EXT-006).
        let _ = guest.registry.register_message_renderer(guest.owner.clone(), custom_type.clone());
        guest.add_renderer(custom_type);
    }

    /// X15 — the custom-ENTRY renderer surface (Pi `registerEntryRenderer`, types.ts:1295, stored
    /// in `extension.entryRenderers` at loader.ts:314-318 and resolved by
    /// `runner.ts:593-600 getEntryRenderer`). A SEPARATE registry table from the message one above:
    /// the two draw different things when the renderer throws (`custom-entry.ts:47-52` vs
    /// `custom-message.ts:82-84`). Rendering itself still travels over the guest's `render-call`
    /// export — see `ExtensionHost::render_entry` for why the world has no fourth export.
    async fn register_entry_renderer(&mut self, custom_type: String) {
        let Ok(guest) = guest_of(self) else { return };
        let _ = guest.registry.register_entry_renderer(guest.owner.clone(), custom_type.clone());
        guest.add_renderer(custom_type);
    }

    /// [CYRUP-DELTA] EXT-062: cyrup-original CALL for an upstream FIELD —
    /// `RegisteredCommand.getArgumentCompletions?` (`extensions/types.ts:1166` @v0.83.0), which a pi
    /// guest passes inline to `registerCommand`. A WIT record cannot carry a closure, so the flag
    /// crosses here and the closure stays guest-side behind the `get-argument-completions` export.
    async fn add_autocomplete(&mut self, command: String) {
        let Ok(guest) = guest_of(self) else { return };
        // Record it in the SHARED registry as well as in `GuestState`, for the reason spelled out on
        // `register_message_renderer` above: the per-guest bookkeeping is host-side diagnostics and
        // no consumer can reach it. The NATIVE tier already lands in the registry
        // (`facade.rs::load_native_body` -> `add_command_autocomplete`); without this line the WASM
        // tier declared the flag into a counter nothing reads, so the two tiers disagreed about
        // whether a guest had argument completions at all.
        let _ = guest.registry.add_command_autocomplete(guest.owner.clone(), command.clone());
        guest.add_autocomplete(command);
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
    // `notify`/`set_status`/`set_widget`/`set_header`/`set_footer`/`set_title`/`set_editor_text`/
    // `paste_editor_text`/`set_tools_expanded` below ALSO forward to `guest.services` (the pluggable
    // [`HostServices`] backend the running session injects), not just `GuestState`'s own bookkeeping
    // (kept for tests/diagnostics — see [`crate::host::services::UiChrome`]'s doc). Without the
    // `guest.services.*` call, these fire-and-forget `ui.*` effects never reach a live consumer at
    // all (they are Pi's `ExtensionUIContext` mutators, types.ts:130-275) — mirrors how `confirm`/
    // `input`/`select`/`editor` below already route through `guest.services`.
    async fn notify(&mut self, message: String, kind: bindings::cyrup::ext::ui::NotifyKind) {
        if let Ok(guest) = ui_guest_of(self) {
            let kind = notify_kind_from_wit(kind);
            guest.services.notify(&message, kind);
            guest.notify(message, kind);
        }
    }
    async fn set_status(&mut self, key: String, text: Option<String>) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_status(&key, text.as_deref());
            guest.set_status(key, text);
        }
    }
    async fn abort_signal(&mut self, signal_id: String) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.abort_signal(signal_id);
        }
    }
    /// pi `onTerminalInput(handler)` (`extensions/types.ts:145` @v0.83.0) — records that this
    /// guest has a raw-input handler; the handler itself is reached through the
    /// `on-terminal-input` EXPORT. EXT-021.
    async fn subscribe_terminal_input(&mut self) {
        let Ok(guest) = ui_guest_of(self) else { return };
        let _ = guest.registry.subscribe_terminal_input(guest.owner.clone());
    }
    /// The unsubscribe function upstream's `onTerminalInput` returns (`Set.delete`,
    /// `packages/tui/src/tui.ts:652-654`). Idempotent.
    async fn unsubscribe_terminal_input(&mut self) {
        let Ok(guest) = ui_guest_of(self) else { return };
        let _ = guest.registry.unsubscribe_terminal_input(&guest.owner);
    }
    async fn confirm(&mut self, prompt: String, message: String, opts_json: String) -> bool {
        let opts = DialogOptions::parse(&opts_json);
        let Ok(guest) = ui_guest_of(self) else { return false };
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
        let guest = ui_guest_of(self).ok()?;
        if guest.dialog_dismissed(&opts) {
            return None;
        }
        let started = std::time::Instant::now();
        let result = guest.services.input(&prompt, placeholder.as_deref(), &opts);
        guest.note_dialog_wait(started);
        result
    }
    async fn select(&mut self, prompt: String, options_json: String, opts_json: String) -> Option<String> {
        let guest = ui_guest_of(self).ok()?;
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
        let guest = ui_guest_of(self).ok()?;
        // `ui.editor` blocks the same way (tears the TUI down and waits for `$EDITOR` to exit, an
        // equally human-paced wait) — the SAME epoch-budget exemption applies.
        let started = std::time::Instant::now();
        let result = guest.services.editor(&title, &initial);
        guest.note_dialog_wait(started);
        result
    }
    /// EXT-047 — pi's `setWidget(key, content: string[] | undefined, options?)`
    /// (`extensions/types.ts:170-175` @v0.83.0). `content-json` is the LINES array; `none` removes
    /// this key's widget (upstream's `content: undefined`).
    async fn set_widget(
        &mut self,
        key: String,
        content_json: Option<String>,
        opts_json: String,
    ) {
        if let Ok(guest) = ui_guest_of(self) {
            // A malformed lines array is NOT a removal: it is a broken write, and treating it as
            // `undefined` would silently tear down a widget the guest was trying to update. Fall
            // back to an empty line list, which upstream renders as a present-but-blank widget.
            let lines: Option<Vec<String>> = content_json
                .map(|j| serde_json::from_str::<Vec<String>>(&j).unwrap_or_default());
            let opts: Value = serde_json::from_str(&opts_json).unwrap_or(Value::Null);
            let placement = crate::host::WidgetPlacement::from_opts(&opts);
            guest.services.set_widget(&key, lines.as_deref(), placement);
            guest.set_widget(crate::host::WidgetEffect { key, lines, placement });
        }
    }

    /// Pi `setWorkingMessage(message?)` (`extensions/types.ts:151` @v0.83.0); `none` = the
    /// no-argument call, "restore default" (EXT-021).
    async fn set_working_message(&mut self, message: Option<String>) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_working_message(message.as_deref());
        }
    }

    /// Pi `setWorkingVisible(visible)` (`extensions/types.ts:154` @v0.83.0) — visibility
    /// independent of the message (EXT-021).
    async fn set_working_visible(&mut self, visible: bool) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_working_visible(visible);
        }
    }

    /// Pi `setWorkingIndicator(options?)` (`extensions/types.ts:164` @v0.83.0); the bag is
    /// `{frames?, intervalMs?}` (`:116-121`), `none` restores the default spinner (EXT-021).
    async fn set_working_indicator(&mut self, opts_json: Option<String>) {
        if let Ok(guest) = ui_guest_of(self) {
            let opts: Option<Value> =
                opts_json.map(|j| serde_json::from_str(&j).unwrap_or(Value::Null));
            guest.services.set_working_indicator(opts.as_ref());
        }
    }

    /// Pi `setHiddenThinkingLabel(label?)` (`extensions/types.ts:167` @v0.83.0) (EXT-021).
    async fn set_hidden_thinking_label(&mut self, label: Option<String>) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_hidden_thinking_label(label.as_deref());
        }
    }

    /// Pi `getTheme(name): Theme | undefined` (`extensions/types.ts:272` @v0.83.0) — inspect a
    /// theme WITHOUT switching to it (EXT-021).
    async fn theme_get_by_name(&mut self, name: String) -> Option<String> {
        let guest = ui_guest_of(self).ok()?;
        guest.services.theme_by_name(&name).map(|t| t.to_string())
    }
    async fn set_header(&mut self, content: String) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_header(&content);
            guest.set_header(content);
        }
    }
    async fn set_footer(&mut self, content: String) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_footer(&content);
            guest.set_footer(content);
        }
    }
    async fn set_title(&mut self, title: String) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_title(&title);
            guest.set_title(title);
        }
    }
    async fn custom(&mut self, spec_json: String) -> Option<String> {
        let guest = ui_guest_of(self).ok()?;
        let spec: Value = serde_json::from_str(&spec_json).unwrap_or(Value::Null);
        // Same epoch-budget forgiveness its siblings (`confirm`/`input`/`select`/`editor` above)
        // carry: a custom overlay is just as human-paced a wait once a real `HostServices` backend
        // wires it to a live UI (Pi `custom()`), so the wall-clock block must be recorded here too,
        // not only for the four dialog kinds that happened to land first.
        let started = std::time::Instant::now();
        let result = guest.services.custom(&spec);
        guest.note_dialog_wait(started);
        result
    }
    async fn get_editor_text(&mut self) -> String {
        ui_guest_of(self).map(|g| g.services.editor_text()).unwrap_or_default()
    }
    async fn set_editor_text(&mut self, text: String) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_editor_text(&text, false);
            guest.editor_write(text, false);
        }
    }
    async fn paste_editor_text(&mut self, text: String) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_editor_text(&text, true);
            guest.editor_write(text, true);
        }
    }
    /// Pi `addAutocompleteProvider(factory)` (`extensions/types.ts:225` @v0.83.0).
    ///
    /// EXT-065: this impl used to sit in `registration::Host` above and call the UNGATED
    /// [`guest_of`], so an extension whose manifest granted no `ui` could still stack a provider
    /// onto the core input editor. It is a `ui.*` import now, and goes through [`ui_guest_of`] like
    /// every other one — the grant is the whole point of the move, so do not relax it back to
    /// `guest_of`.
    async fn add_autocomplete_provider(&mut self) {
        if let Ok(guest) = ui_guest_of(self) {
            // Same shared-registry rule as `add_autocomplete`: the native tier lands in the registry
            // (`facade.rs::load_native_body`), so the WASM tier must too, or a stacked provider is a
            // number in `GuestState` that no consumer can reach. (The consumer itself — the
            // interactive editor consulting these providers — is TUI-029 and still open; this is the
            // producer half it will read.)
            let _ = guest.registry.add_autocomplete_provider(guest.owner.clone());
            guest.add_autocomplete_provider();
        }
    }
    async fn theme_get(&mut self) -> Option<String> {
        ui_guest_of(self).ok().and_then(|g| g.services.theme())
    }
    /// Pi's `readonly theme: Theme` (`extensions/types.ts:266` @v0.83.0) — the ACTIVE theme's
    /// colours, not just its name (EXT-066). Composed from the backend's own two accessors rather
    /// than a third trait method, so every `HostServices` impl that can already answer
    /// `theme()` + `theme_by_name()` answers this too, with no chance of the three disagreeing.
    async fn theme_get_json(&mut self) -> Option<String> {
        let guest = ui_guest_of(self).ok()?;
        let name = guest.services.theme()?;
        guest.services.theme_by_name(&name).map(|t| t.to_string())
    }
    async fn theme_list(&mut self) -> String {
        ui_guest_of(self).map(|g| g.services.theme_list().to_string()).unwrap_or_else(|_| "[]".into())
    }
    async fn theme_set(&mut self, name: String) -> Result<(), String> {
        let guest = ui_guest_of(self)?;
        guest.services.set_theme(&name)?;
        guest.theme_set(name);
        Ok(())
    }
    async fn working_start(&mut self, label: String) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.working(Some(label));
        }
    }
    async fn working_stop(&mut self) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.working(None);
        }
    }
    async fn get_tools_expanded(&mut self) -> bool {
        ui_guest_of(self).map(|g| g.services.tools_expanded()).unwrap_or(false)
    }
    async fn set_tools_expanded(&mut self, expanded: bool) {
        if let Ok(guest) = ui_guest_of(self) {
            guest.services.set_tools_expanded(expanded);
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
    async fn set_label(&mut self, entry_id: String, label: Option<String>) {
        // Route to the pluggable backend (pi `setLabel(entryId, label: string | undefined)`,
        // extensions/types.ts:1314 @v0.83.0); the session service applies it to the live tree via
        // `append_label`, which has always taken `Option<&str>`. `None` CLEARS (EXT-046).
        if let Ok(guest) = guest_of(self) {
            guest.services.set_label(&entry_id, label.as_deref());
        }
    }
}

impl bindings::cyrup::ext::models::Host for HostState {
    async fn list_models(&mut self) -> String {
        guest_of(self).map(|g| g.services.models().to_string()).unwrap_or_else(|_| "[]".into())
    }
    /// pi `ctx.scopedModels` (extensions/types.ts:326 @v0.83.0) — EXT-045.
    async fn scoped_models(&mut self) -> String {
        guest_of(self)
            .map(|g| g.services.scoped_models().to_string())
            .unwrap_or_else(|_| "[]".into())
    }
    async fn current(&mut self) -> Option<String> {
        guest_of(self).ok().and_then(|g| g.services.current_model())
    }
    async fn set_model(&mut self, model_json: String) {
        let Ok(guest) = guest_of(self) else { return };
        // GAP-11: `setModel` is allowed from ANY handler, matching Pi — Pi binds `setModel` with only
        // `assertActive` (loader.ts:359-362), reachable from any handler, and it takes effect
        // (agent-session.ts:1476-1490). We therefore do NOT gate this on the command tier. The op is
        // QUEUED (`control` is a synchronous mpsc push that touches no wasm store, host_services.rs
        // `control`) and applied at the store-free turn-boundary drain
        // (`AgentSession::apply_pending_control`), where its state mutation lands as a fresh top-level
        // effect — never re-entering the suspended event-hook store. So an event-tier `setModel` now
        // TAKES EFFECT on the subsequent turn instead of being silently dropped (the former no-op).
        let v: Value = serde_json::from_str(&model_json).unwrap_or(Value::Null);
        let _ = guest.services.control(ControlOp::SetModel(v));
    }
    async fn context_usage(&mut self) -> String {
        guest_of(self).map(|g| g.services.context_usage().to_string()).unwrap_or_else(|_| "{}".into())
    }
    async fn thinking_level(&mut self) -> Option<String> {
        guest_of(self).ok().and_then(|g| g.services.thinking_level())
    }
    async fn set_thinking_level(&mut self, level: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        // GAP-11: `setThinkingLevel` is allowed from ANY handler, matching Pi — Pi binds it with only
        // `assertActive` (loader.ts:369-372; the runner emit is void/non-awaited,
        // agent-session.ts:1541-1572) and it takes effect. We no longer gate on the command tier.
        //
        // The op is QUEUED (`control` is a synchronous mpsc push that touches no wasm store,
        // host_services.rs `control`) and applied at the store-free turn-boundary drain
        // (`AgentSession::apply_pending_control`). Its `thinking_level_select` re-emit
        // (agent-session.ts:1560-1567) fires THERE as a FRESH top-level guest call — after the event
        // hook's `LiveExtension.inner` store guard has already been released — so it never re-enters
        // the suspended single-instance store. That deferral is what dissolves the R-08-008 deadlock
        // the old command-tier gate guarded against (the same reason `with_session`/`bus_deliver`
        // callbacks defer instead of running inline). An event-tier `setThinkingLevel` therefore TAKES
        // EFFECT on the subsequent turn and the guest observes `Ok(())` — no longer an honest deadlock
        // `Err`.
        guest.services.control(ControlOp::SetThinkingLevel(level))
    }
}

impl bindings::cyrup::ext::exec::Host for HostState {
    async fn run(
        &mut self,
        cmd: String,
        args: Vec<String>,
        opts_json: String,
    ) -> Result<wit_types::ExecResult, String> {
        let guest = exec_guest_of(self)?;
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
        let guest = exec_guest_of(self)?;
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
        //
        // A resolved value of `""` (the guest passed `cwd: ""`, or a `${VAR}`/`$env:VAR` placeholder
        // that interpolated to empty — `pi-mcp-adapter/utils.ts:78-87`'s `resolveConfigPath("")`
        // returns `""` verbatim) is folded to `None` here rather than threaded through as
        // `Some("")`. Node's real `child_process.spawn` (`server-manager.ts:110` ->
        // `StdioClientTransport` -> `client/stdio.js:72-82`'s `cross_spawn(...,{cwd:
        // this._serverParams.cwd})`, verified live) treats a falsy `cwd` as "no override" — it
        // inherits the parent's own ambient cwd rather than erroring, unlike
        // `std::process::Command::current_dir("")` (verified live: `Os { code: 2, kind: NotFound,
        // .. }`). Folding to `None` here reuses `HostServices::proc_spawn`'s EXISTING omitted-cwd
        // fallback to the session's project cwd (`host_services.rs`) — the cyrup-analog of Node's
        // ambient-cwd fallback, since that comment already establishes the two coincide by
        // construction — rather than letting the empty string reach `ProcCaps::spawn`'s
        // unconditional `cmd.current_dir(cwd)` (`caps/proc.rs`) and hard-fail the spawn.
        let spec = crate::caps::proc::ProcSpawnSpec {
            cmd,
            args,
            env,
            cwd: cwd
                .map(|c| crate::caps::proc::resolve_config_path(&c))
                .filter(|p| !p.as_os_str().is_empty()),
            capture_stderr,
        };
        // Closes the SAME class of CRITICAL finding `845f707`/`9ffec1a` closed for `ui.*`/
        // `http-client`, and this file's own `write_stdin`/`kill` already close for their siblings:
        // an `npx`/`npm`-shaped `spec.cmd` sends `guest.services.proc_spawn` through
        // `ProcCaps::spawn`'s `block_in_place`-wrapped `npx_resolver::resolve_npx_binary` call,
        // which on a cold cache runs a REAL blocking subprocess-spawn-and-wait up to
        // `npx_resolver::FORCE_CACHE_TIMEOUT` (30s — `caps/proc/npx_resolver.rs`) — six times the
        // WASM epoch budget (`facade.rs`'s `WASM_EPOCH_BUDGET_TICKS` * `epoch::DEFAULT_TICK` ≈ 5s).
        // With no `note_dialog_wait` call, that 30s worst case left the epoch deadline already
        // expired by the time the guest resumed wasm execution right after this call returns,
        // tripping the SAME permanent instance-wedging trap `ui.*`/`http-client`/`exec`/
        // `write_stdin`/`kill` used to. Record the wait exactly like those do — including the
        // ordinary (non-npx, non-blocking) spawn path, which costs nothing extra here (an
        // `Instant::now()`/`note_dialog_wait` pair on an already-fast call is negligible, and gating
        // it on `cmd == "npx" || cmd == "npm"` would duplicate `ProcCaps::spawn`'s own gate rather
        // than reuse it).
        let started = std::time::Instant::now();
        let result = guest.services.proc_spawn(&spec);
        guest.note_dialog_wait(started);
        // The minted handle belongs to THIS guest — every `proc.*` call below checks it. See
        // `GuestState::own_proc_handle`: `ProcCaps` is one session-wide registry keyed by a
        // monotonic `u32`, shared by every loaded extension, so an unowned handle is ambient
        // authority over another extension's child process.
        if let Ok(h) = &result {
            guest.own_proc_handle(*h);
        }
        result
    }
    async fn write_stdin(&mut self, handle: u32, data: Vec<u8>) -> Result<u32, String> {
        let guest = exec_guest_of(self)?;
        guest.require_proc_handle(handle)?;
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
        let guest = exec_guest_of(self)?;
        guest.require_proc_handle(handle)?;
        guest.services.proc_read_stdout(handle, max_bytes)
    }
    async fn read_stderr(&mut self, handle: u32, max_bytes: u32) -> Result<Vec<u8>, String> {
        let guest = exec_guest_of(self)?;
        guest.require_proc_handle(handle)?;
        guest.services.proc_read_stderr(handle, max_bytes)
    }
    async fn poll_exit(&mut self, handle: u32) -> Option<i32> {
        // No error channel in the WIT signature, so a foreign handle degrades to `none` — the same
        // answer a never-spawned handle already gives, which is the honest one: from this guest's
        // point of view that process does not exist.
        let guest = exec_guest_of(self).ok()?;
        guest.require_proc_handle(handle).ok()?;
        guest.services.proc_poll_exit(handle)
    }
    async fn kill(&mut self, handle: u32) -> Result<(), String> {
        let guest = exec_guest_of(self)?;
        guest.require_proc_handle(handle)?;
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
        let guest = net_guest_of(self)?;
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
        let guest = net_guest_of(self)?;
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
        // Same ownership rule as `proc.spawn` above: `HttpCaps` is one session-wide stream table
        // shared by every guest, so an unowned handle would let another extension read this
        // extension's response body (`GuestState::own_stream_handle`).
        guest.own_stream_handle(opened.handle);
        Ok(bindings::cyrup::ext::http_client::HttpStreamResponse {
            handle: opened.handle,
            status: opened.status,
            headers: opened.headers,
        })
    }
    async fn poll_stream_chunk(&mut self, handle: u32) -> Result<Option<Vec<u8>>, String> {
        let guest = net_guest_of(self)?;
        guest.require_stream_handle(handle)?;
        // Same rationale — `HttpCaps::poll_stream_chunk` `.await`s the underlying stream's `next()`,
        // which can legitimately block for a while on a slow/sparse server-sent stream (the real MCP
        // SSE-over-HTTP transport shape `pi-mcp-adapter` targets).
        let started = std::time::Instant::now();
        let result = guest.services.http_poll_stream_chunk(handle);
        guest.note_dialog_wait(started);
        result
    }
    async fn close_stream(&mut self, handle: u32) {
        if let Ok(guest) = net_guest_of(self) {
            // Closing ANOTHER guest's stream is the cheapest of the cross-extension attacks and the
            // hardest to notice (the victim's next `poll-stream-chunk` simply ends). No error
            // channel in the signature, so a foreign handle is a silent no-op.
            if guest.require_stream_handle(handle).is_err() {
                return;
            }
            guest.services.http_close_stream(handle);
            guest.release_stream_handle(handle);
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
        // EXT-055: a `read:` grant is not a `write:` grant. The manifest syntax has always had two
        // modes (`manifest.rs`'s `["read:.", "write:.cyrup/todo"]`); nothing read them until now.
        let resolved = guest.fs.resolve_write(&path)?;
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
    async fn subscribe(&mut self, topic: String) {
        // Declare this guest as a listener on `topic` in the SHARED bus (Pi `pi.events.on`,
        // event-bus.ts:18). The host later fans a matching `emit` from any guest out to this guest's
        // `bus-deliver` export (gap-08 §5.3).
        if let Ok(guest) = guest_of(self) {
            guest.bus_subscribe(topic);
        }
    }
    /// EXT-050 — the unsubscribe half. pi's `on()` has always returned an unsubscribe closure
    /// (`core/event-bus.ts:18-27`) and since v0.84.1 the loader tracks it
    /// (`extensions/loader.ts:413-421`); without this a `subscribe` was permanent for the
    /// instance's life and a guest listening only while a mode is active had to filter by hand.
    async fn unsubscribe(&mut self, topic: String) {
        if let Ok(guest) = guest_of(self) {
            guest.bus_unsubscribe(&topic);
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
        // Same epoch-budget forgiveness its `ui::Host` siblings (`confirm`/`input`/`select`/
        // `editor`/`custom`, above) already carry: an OAuth prompt is exactly as human-paced a wait
        // once a real `HostServices` backend wires it to a live UI (Pi `onPrompt`), so the
        // wall-clock block must be recorded here too, not only for the `ui.*` dialog kinds that
        // happened to land first.
        let started = std::time::Instant::now();
        let result = guest.services.oauth_prompt(&message, placeholder.as_deref(), allow_empty);
        guest.note_dialog_wait(started);
        result
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
        // Same epoch-budget forgiveness as `on_prompt` just above (Pi `onSelect` is the identical
        // human-paced OAuth wait).
        let started = std::time::Instant::now();
        let result = guest.services.oauth_select(&message, &options);
        guest.note_dialog_wait(started);
        result
    }
}

impl bindings::cyrup::ext::provider_stream::Host for HostState {
    async fn emit_event(&mut self, stream_id: String, event_json: String) {
        if let Ok(guest) = guest_of(self) {
            let event: Value = serde_json::from_str(&event_json).unwrap_or(Value::Null);
            guest.push_stream_event(stream_id, event);
        }
    }

    /// EXT-052 — the `onPayload` half of pi's must-invoke `streamSimple` contract
    /// (`extensions/types.ts:1452-1457` @v0.84.1: "Implementations must invoke `options.onPayload`
    /// before sending the provider request and use any returned replacement payload").
    ///
    /// Routes into the SAME `before_provider_request` reduction the built-in provider path uses, so
    /// an extension-supplied provider's requests stop being invisible to every other extension.
    /// Returns the replacement payload when a handler mutated it, `None` when nothing changed.
    ///
    /// The reduction cannot run here — re-entering the dispatcher would re-enter THIS guest's
    /// single-instance store while it is borrowed for the `stream-simple` call. So the payload is
    /// queued on the guest state and reduced by the host at the stream seam
    /// ([`crate::host::GuestState::take_stream_payloads`]), the same request/poll bridge every
    /// other long-running capability uses (arch-08 §5.2).
    async fn on_payload(&mut self, _stream_id: String, payload_json: String) -> Option<String> {
        // Clone the two things out of the guest BEFORE awaiting: the reduction re-enters the
        // dispatcher (other stores), and holding a borrow of `self` across it would pin the
        // calling guest's state for the whole round trip.
        let (owner, hooks) = {
            let guest = guest_of(self).ok()?;
            (guest.owner.clone(), guest.provider_reduction()?)
        };
        let payload: Value = serde_json::from_str(&payload_json).unwrap_or(Value::Null);
        hooks.before_provider_request(&owner, payload).await.map(|v| v.to_string())
    }

    /// EXT-052 — the `onResponse` half ("They must invoke `options.onResponse` after receiving the
    /// response and before consuming its body, matching built-in providers",
    /// `extensions/types.ts:1452-1457` @v0.84.1). Notify-only, routed into the same
    /// `after_provider_response` dispatch the built-in path uses.
    async fn on_response(&mut self, _stream_id: String, status: u16, headers_json: String) {
        let Some((owner, hooks)) = ({
            match guest_of(self) {
                Ok(guest) => guest.provider_reduction().map(|h| (guest.owner.clone(), h)),
                Err(_) => None,
            }
        }) else {
            return;
        };
        let headers: Value = serde_json::from_str(&headers_json).unwrap_or(Value::Null);
        hooks.after_provider_response(&owner, u32::from(status), headers).await;
    }
}

impl bindings::cyrup::ext::ext_tools::Host for HostState {
    async fn get_active_tools(&mut self) -> String {
        let Ok(guest) = guest_of(self) else { return "[]".into() };
        // Prefer the LIVE session's real active tool set (Pi `getActiveTools` = `getActiveToolNames`,
        // agent-session.ts:2281,813 — the SAME source the host/CLI tool-toggle reads). Fall back to
        // the guest's own restriction / registry when no session backend is attached (default host).
        let names: Vec<String> = match guest.services.active_tools() {
            Some(live) => live,
            None => match guest.active_tools_restriction() {
                Some(r) => r,
                None => guest.registry.all_registered_tool_names().unwrap_or_default(),
            },
        };
        serde_json::to_string(&names).unwrap_or_else(|_| "[]".into())
    }
    async fn get_all_tools(&mut self) -> String {
        let Ok(guest) = guest_of(self) else { return "[]".into() };
        // EXT-038: prefer the LIVE session's merged registry — pi's `getAllTools()` maps
        // `this._toolDefinitions`, which is built-ins + MCP + extension tools
        // (`core/agent-session.ts:906-914` @v0.83.0). `registry.tool_info()` walks the two
        // EXTENSION tables only, so a plan-mode extension reading it computed a restriction set
        // that silently omitted read/write/edit/bash — and `set_active_tools` (which DOES route to
        // the live backend, just below) then honoured it. Same live-source-with-registry-fallback
        // shape `get_active_tools` above already uses.
        let infos = match guest.services.all_tools() {
            Some(live) => live,
            None => guest.registry.tool_info().unwrap_or_default(),
        };
        serde_json::to_string(&infos).unwrap_or_else(|_| "[]".into())
    }
    async fn set_active_tools(&mut self, names_json: String) {
        if let Ok(guest) = guest_of(self) {
            let names: Vec<String> = serde_json::from_str(&names_json).unwrap_or_default();
            // Route to the pluggable backend so a live session genuinely restricts the agent's tool
            // set (Pi `setActiveTools` = `setActiveToolsByName`, agent-session.ts:2283,840-855). ALSO
            // keep the local mirror for hosts with no session backend (mirrors `set_tools_expanded`).
            guest.services.set_active_tools(&names);
            guest.set_active_tools_restriction(names);
        }
    }
    async fn get_commands(&mut self) -> String {
        let Ok(guest) = guest_of(self) else { return "[]".into() };
        // EXT-037. pi's `getCommands()` is `[...extensionCommands, ...templates, ...skills]`, each
        // row `{name: command.invocationName, description, source, sourceInfo}`
        // (`core/agent-session.ts:2332-2354` @v0.83.0, type `SlashCommandInfo` at
        // `core/slash-commands.ts:6-11`). cyrup emitted `{name}` only, from a `HashMap::keys()`
        // walk: extension commands only, RAW names (so a colliding second `deploy` was uncallable
        // — the guest never saw `deploy:2`), no description, no source, and nondeterministic order.
        //
        // Prefer the live catalog, which is the only source that has prompt templates and skills;
        // fall back to the registry's RESOLVED commands (load order, `name:N` assigned) rather than
        // the raw map.
        let infos = match guest.services.commands() {
            Some(live) => live,
            None => guest
                .registry
                .resolved_commands()
                .unwrap_or_default()
                .into_iter()
                .map(|r| {
                    serde_json::json!({
                        "name": r.invocation_name,
                        "description": r.descriptor.description,
                        "source": "extension",
                        // pi's `SourceInfo` is `{path, source, scope, origin, baseDir?}`
                        // (`core/source-info.ts:6-12`); with no session attached the only thing the
                        // registry knows is the owning extension, so this is the synthetic form
                        // `createSyntheticSourceInfo` produces (`:24-38`: scope "temporary",
                        // origin "top-level").
                        "sourceInfo": {
                            "path": r.owner.to_string(),
                            "source": r.owner.to_string(),
                            "scope": "temporary",
                            "origin": "top-level",
                        },
                    })
                })
                .collect(),
        };
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
    /// Pi `ctx.compact(options?: CompactOptions)` (extensions/types.ts:344). `opts-json` carries
    /// `{customInstructions}` (types.ts:296-300) — the extra guidance the summarizer receives; a
    /// malformed or empty bag degrades to "no instructions" rather than erroring, matching Pi's
    /// optional-argument shape. `onComplete`/`onError` cannot cross the component boundary as
    /// function values; the guest's completion signal is the `session_compact` event.
    async fn compact(&mut self, opts_json: String) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let custom_instructions = serde_json::from_str::<Value>(&opts_json)
            .ok()
            .and_then(|v| {
                v.get("customInstructions").and_then(Value::as_str).map(str::to_string)
            })
            .filter(|s| !s.is_empty());
        guest.services.control(ControlOp::Compact { custom_instructions })
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

    /// Pi `ctx.abort()` (extensions/types.ts:339): "Abort the current agent run. **Available in all
    /// contexts.**" Deliberately NOT `require_command_tier()`-gated — unlike the session-REPLACEMENT
    /// ops above, aborting cannot re-enter the guest, and Pi's canonical use is an event handler (a
    /// `tool_call` gate) deciding the run must stop. Pi binds it at agent-session.ts:2405-2436.
    async fn abort(&mut self) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.services.control(ControlOp::Abort)
    }

    /// Pi `ctx.shutdown()` (extensions/types.ts:344): "Gracefully shutdown pi and exit. **Available
    /// in all contexts.**" (runner entry point runner.ts:656-662). Also untiered, for the same
    /// reason as [`Self::abort`].
    async fn shutdown(&mut self) -> Result<(), String> {
        let guest = guest_of(self)?;
        guest.services.control(ControlOp::Shutdown)
    }
}

/// Read-only base-context state (Pi `ExtensionContext`, extensions/types.ts:305-347). The four
/// session-state answers are served straight off the injected [`crate::host::HostServices`] backend,
/// so a guest reads the LIVE session's answer instead of a hard-coded default (EXT-005);
/// `get-mode`/`has-ui` are host CONFIGURATION rather than session state and come off the
/// [`GuestState`] copy of [`crate::HostConfig`] — the same pair the native path reads from
/// `HostCtx` (`native.rs:91-92`). Untiered: Pi puts all six on the base context, available to every
/// handler.
impl bindings::cyrup::ext::ctx_state::Host for HostState {
    /// Pi `ctx.mode` (types.ts:311). A guest with no live [`GuestState`] gets the default `tui`
    /// rather than an error — `mode` is a plain field in Pi and cannot fail.
    async fn get_mode(&mut self) -> wit_types::ExtMode {
        let mode = guest_of(self).map(|g| g.mode()).unwrap_or_default();
        match mode {
            ExtMode::Tui => wit_types::ExtMode::Tui,
            ExtMode::Rpc => wit_types::ExtMode::Rpc,
            ExtMode::Json => wit_types::ExtMode::Json,
            ExtMode::Print => wit_types::ExtMode::Print,
        }
    }
    /// Pi `ctx.hasUI` (types.ts:313): "Whether dialog-capable UI is available (true in TUI and RPC
    /// modes)". Degrades to `false` with no live guest — claiming UI that is not there would send a
    /// guest into a dialog that can never be answered.
    async fn has_ui(&mut self) -> bool {
        guest_of(self).map(|g| g.has_ui()).unwrap_or(false)
    }
    async fn is_idle(&mut self) -> bool {
        guest_of(self).map(|g| g.services.is_idle()).unwrap_or(true)
    }
    async fn has_pending_messages(&mut self) -> bool {
        guest_of(self).map(|g| g.services.has_pending_messages()).unwrap_or(false)
    }
    async fn is_project_trusted(&mut self) -> bool {
        guest_of(self).map(|g| g.services.is_project_trusted()).unwrap_or(false)
    }
    async fn get_system_prompt(&mut self) -> String {
        guest_of(self).ok().and_then(|g| g.services.system_prompt()).unwrap_or_default()
    }
    /// pi `ctx.getSystemPromptOptions()` (`extensions/types.ts:355` @v0.83.0) — the BAG behind the
    /// string `get_system_prompt` returns (EXT-061).
    ///
    /// The one COMMAND-tier member of this interface, and the gate is upstream's own placement:
    /// `getSystemPrompt()` is on the base `ExtensionContext` (`:346`), `getSystemPromptOptions()`
    /// on `ExtensionCommandContext` (`:353-387`). An event-tier call gets the observable
    /// deadlock-guard error, never a silent empty bag.
    ///
    /// With no backend the answer is pi's own no-backend default, `() => ({ cwd: this.cwd })`
    /// (`core/extensions/runner.ts:287`, re-bound at `:350` @v0.83.0) — NOT an error and not `{}`.
    /// That is what keeps this from being the declared-capability-with-a-dead-implementation shape
    /// (EXT-066's finding): the import answers something upstream-correct on every backend.
    async fn get_system_prompt_options(&mut self) -> Result<String, String> {
        let guest = guest_of(self)?;
        guest.require_command_tier()?;
        let bag = guest.services.system_prompt_options().unwrap_or_else(|| {
            serde_json::json!({ "cwd": guest.cwd().to_string_lossy().into_owned() })
        });
        Ok(bag.to_string())
    }
    /// pi `ctx.cwd` (extensions/types.ts:315 @v0.83.0) — on the BASE `ExtensionContext`, so every
    /// handler and every tool `execute` can read it, not just command handlers (EXT-044). Sourced
    /// from the `HostConfig.cwd` copy `GuestState` takes at load time — the same value the native
    /// tier has always exposed as `HostCtx.cwd`.
    async fn get_cwd(&mut self) -> String {
        guest_of(self).map(|g| g.cwd().to_string_lossy().into_owned()).unwrap_or_default()
    }
    /// The run-scoped cancellation poll (EXT-045; pi `ctx.signal`, extensions/types.ts:334
    /// @v0.83.0). See the `is-run-cancelled` CYRUP-DELTA in `world.wit` for why this is a poll and
    /// not a subscription.
    async fn is_run_cancelled(&mut self) -> bool {
        guest_of(self).map(|g| g.services.is_run_cancelled()).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// LiveExtension: a loaded `.wasm` component as a unified Extension.
// ---------------------------------------------------------------------------

/// Unwinds the two pieces of INSTANCE-scoped state `execute_tool` binds for the duration of ONE
/// call — the bound tool `CancelToken` and the queued `host-tool.emit-update` chunks — when the call
/// ends, **including when the `execute_tool` future is DROPPED mid-await**, which is the case a JS
/// port cannot think of and a Rust one must.
///
/// Upstream, `signal` is a parameter of `ToolDefinition.execute` (`extensions/types.ts:483`
/// @v0.83.0): it is scoped to the call by the language, and an `async function` that has begun
/// ALWAYS settles, so there is no state to unwind. cyrup cannot pass the token through the
/// Component Model — a `CancelToken` is not a WIT value — so it binds it on
/// [`GuestState::set_tool_cancel`] for the duration of the call and the guest polls it through the
/// `host-tool.is-cancelled` import.
///
/// That turns a call-scoped parameter into instance-scoped mutable state, and instance-scoped state
/// must be torn down on EVERY exit path. Clearing it on both arms of the `tokio::select!` covers
/// completion and token-cancellation but NOT the third exit a Rust future has: the whole
/// `execute_tool` future being dropped at its await point by a racing caller (an outer `select!`, a
/// `tokio::time::timeout`, an aborted `JoinHandle`). On that path the old code left the token bound
/// forever, and `host-tool.is-cancelled` is NOT gated to tool calls — a guest may poll it from any
/// handler — so every later poll, from any entry point, read the abandoned call's token and
/// answered `true` until the next `execute_tool` happened to rebind it. A guest that checks
/// `is-cancelled` to decide whether to keep working would quietly stop working.
///
/// EXT-M06 — the `onUpdate` half is the same shape and the same third exit. Upstream `onUpdate` is
/// a CLOSURE field of the very same second argument (`extensions/types.ts:484`), so a chunk is
/// delivered to that call's sink and to nothing else, by construction. cyrup routes
/// `host-tool.emit-update(call-id, chunk)` into an instance-scoped queue on [`GuestState`] that
/// `execute_tool` replays AFTER the call settles, so a call that never reaches its replay — the
/// cancelled arm, or the dropped future — used to leave its chunks queued for whichever tool call
/// drained next, which then emitted another call's partial output into its own `ToolUpdateSink`.
/// Clearing the queue here makes that impossible on every exit path; the replay itself additionally
/// filters by `call-id` (see [`GuestState::take_tool_updates_for`]).
///
/// Declared AFTER the `inner` mutex guard in `execute_tool` so it drops BEFORE it: both are unwound
/// while the instance lock is still held, so no other call can observe the gap.
pub(crate) struct ToolCallBinding<'a>(pub(crate) &'a Arc<GuestState>);

impl Drop for ToolCallBinding<'_> {
    fn drop(&mut self) {
        self.0.set_tool_cancel(None);
        let dropped = self.0.clear_tool_updates();
        if dropped > 0 {
            tracing::debug!(
                chunks = dropped,
                "tool call ended without replaying its streamed updates (cancelled or abandoned); \
                 discarded so they cannot surface in the next call"
            );
        }
    }
}

/// A live loaded WASM extension (arch-08b). Holds the instantiated component + its `Store` behind an
/// async `Mutex` (single-thread-per-Store, R-ARCH-EXT-013) and the `GuestState` that backs its
/// imports. Implements [`Extension`]; the dispatcher treats it identically to a native built-in.
pub struct LiveExtension {
    id: ExtensionId,
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

        // EXT-058: command registrations declared during `init` already landed in the registry —
        // `registration::register-command` writes through like every sibling import, so there is
        // no post-`init` drain (and therefore no init-only window a late registration falls out
        // of). Subscriptions are likewise read LIVE off `GuestState` by
        // `<LiveExtension as Extension>::subscriptions`, so nothing is snapshotted here either.
        Ok(Self {
            id,
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
        // Unbound by [`ToolCallBinding`]'s `Drop`, NOT by hand: this future can be dropped at the
        // `select!` below, and a hand-written clear on each arm does not run on that path. See the
        // guard's own doc for what the leaked token then does to `is-cancelled`, and what the
        // leaked `emit-update` chunks then do to the NEXT tool call (EXT-M06).
        self.guest.set_tool_cancel(Some(cancel.clone()));
        let _tool_call = ToolCallBinding(&self.guest);
        let params_s = params.to_string();
        let api = inner.instance.cyrup_ext_events();
        let call = api.call_execute_tool(&mut inner.store, name, call_id.as_str(), &params_s);
        // EXT-M06: the cancelled arm falls THROUGH to the replay instead of returning from inside
        // the `select!`. Upstream `onUpdate` fires synchronously as the tool runs, so an abort never
        // retracts the partial output already delivered (`extensions/types.ts:484` @v0.83.0); cyrup
        // batches the same chunks and must therefore still hand over what the guest emitted before
        // the cancellation, rather than discarding a tool's whole partial output because it was
        // interrupted.
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            r = call => Some(r),
        };
        // Replay this call's streamed updates (Pi onUpdate) into the runtime sink. Filtered by
        // `call-id`: the queue is instance-scoped, upstream's callback is call-scoped (EXT-M06).
        for chunk in self.guest.take_tool_updates_for(call_id.as_str()) {
            let content = chunk
                .get("content")
                .cloned()
                .and_then(|c| serde_json::from_value::<Vec<Content>>(c).ok())
                .unwrap_or_default();
            on_update(ToolUpdate {
                content,
                details: chunk.get("details").cloned(),
                // Pi's `partialResult` is an `AgentToolResult`, which may carry `terminate`
                // (`packages/agent/src/types.ts:368` @v0.83.0 — EXT-036 corrected
                // `extensions/types.ts:359`, which is `ExtensionContext.compact`'s neighbourhood in
                // the wrong package; `:359` in the RIGHT package is `details: T`); thread it through
                // from the guest's update chunk.
                terminate: chunk.get("terminate").and_then(Value::as_bool),
            });
        }
        let Some(res) = res else {
            return Err(ToolError::new("tool execution cancelled"));
        };
        match res {
            Ok(Ok(out)) => {
                let content = serde_json::from_str::<Vec<Content>>(&out.content_json)
                    .unwrap_or_else(|_| vec![Content::text(out.content_json.clone())]);
                let details = out
                    .details_json
                    .and_then(|d| serde_json::from_str::<Value>(&d).ok());
                Ok(ToolResult { content, details, terminate: out.terminate, ..Default::default() })
            }
            Ok(Err(msg)) => Err(ToolError::new(msg)),
            Err(e) => Err(ToolError::new(map_wasm_error(&e).to_string())),
        }
    }

    /// Execute a guest-registered slash command (R-08-016). Mirrors Pi's
    /// `RegisteredCommand.handler(args, ctx)` (types.ts:1167 @v0.83.0; EXT-072: `:1109` is
    /// `skipConversationRestore`): runs at COMMAND tier (session-control
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

    /// Deliver an inter-extension bus event to this guest's `bus-deliver` export (Pi EventEmitter
    /// listener invocation, event-bus.ts:18-27; gap-08 §5.3). A FRESH top-level notify-style call —
    /// the host invokes it after the emitting guest call unwinds, so it is never re-entrant. Runs at
    /// EVENT tier (a bus listener is not a command-tier session-control handler; Pi hands it only the
    /// data). A guest fault is contained as a typed `ExtError` (the caller reports + skips it).
    pub async fn bus_deliver(
        &self,
        topic: &str,
        payload: &Value,
        cancel: &CancelToken,
    ) -> Result<(), ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Event);
        let payload_s = payload.to_string();
        let api = inner.instance.cyrup_ext_events();
        let call = api.call_bus_deliver(&mut inner.store, topic, &payload_s);
        let res = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ExtError::Cancelled),
            r = call => r,
        };
        res.map_err(|e| map_wasm_error(&e))
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
    /// types.ts:1166 @v0.83.0; EXT-072 corrected `:1108`). A fault is surfaced as a typed `ExtError`.
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

    /// Render a tool call via a guest-registered message renderer (Pi `renderCall`, types.ts:489).
    /// Returns the serialized widget tree, or `None` to fall back to the default renderer.
    pub async fn render_call(
        &self,
        custom_type: &str,
        call: &Value,
    ) -> Result<Option<Value>, ExtError> {
        self.render(custom_type, call, true).await
    }

    /// Render a tool result via a guest-registered renderer (Pi `renderResult`, types.ts:492).
    pub async fn render_result(
        &self,
        custom_type: &str,
        result: &Value,
    ) -> Result<Option<Value>, ExtError> {
        self.render(custom_type, result, false).await
    }

    /// Transform transcript markdown through this guest's registered transformer (EXT-019; pi
    /// `MarkdownTransformer`, `extensions/types.ts:1153` @v0.84.1). EVENT tier — it is a rendering
    /// hook, not a command. `ctx` is `MarkdownTransformContext` (`:1147-1151`).
    pub async fn transform_markdown(
        &self,
        markdown: &str,
        ctx: &Value,
    ) -> Result<String, ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Event);
        let ctx_s = ctx.to_string();
        let api = inner.instance.cyrup_ext_events();
        match api.call_transform_markdown(&mut inner.store, markdown, &ctx_s).await {
            Ok(out) => Ok(out),
            Err(e) => Err(map_wasm_error(&e)),
        }
    }

    /// Offer one raw terminal-input chunk to this guest's `onTerminalInput` handler (EXT-021; pi
    /// `TerminalInputHandler`, `extensions/types.ts:113` @v0.83.0). Runs at EVENT tier: it is a
    /// per-keystroke hook, so it must not be able to hold the draw loop past the epoch budget.
    #[cfg(feature = "wasm-host")]
    pub async fn on_terminal_input(
        &self,
        data: &str,
    ) -> Result<Option<crate::TerminalInputResult>, ExtError> {
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        self.guest.set_tier(CtxTier::Event);
        let api = inner.instance.cyrup_ext_events();
        match api.call_on_terminal_input(&mut inner.store, data).await {
            Ok(out) => Ok(out.map(|r| crate::TerminalInputResult {
                consume: r.consume,
                data: r.data,
            })),
            Err(e) => Err(map_wasm_error(&e)),
        }
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

    /// Invoke the guest's `prepare-arguments` export (EXT-023). `None` = the guest declined to
    /// coerce, which the caller treats as pi's identity default.
    pub async fn prepare_arguments(
        &self,
        name: &str,
        args: &Value,
    ) -> Result<Option<Value>, ExtError> {
        let args_s = args.to_string();
        let mut guard = self.inner.lock().await;
        let inner = &mut *guard;
        inner.store.set_epoch_deadline(self.epoch_ticks);
        self.guest.arm_epoch_deadline_estimate(self.epoch_ticks);
        // EVENT tier: argument preparation runs inside the agent's tool preflight, not from a
        // command handler, so session-replacement ops must stay refused (arch-08 §6.3).
        self.guest.set_tier(CtxTier::Event);
        let api = inner.instance.cyrup_ext_events();
        match api.call_prepare_arguments(&mut inner.store, name, &args_s).await {
            Ok(Some(s)) => Ok(serde_json::from_str::<Value>(&s).ok()),
            Ok(None) => Ok(None),
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

/// The `Tool::label` view of a guest descriptor (EXT-M03). Split out of the trait method purely so
/// it is reachable from a unit test: constructing a `WasmTool` needs an `Arc<LiveExtension>`, i.e. a
/// compiled component and the `wasm32-wasip2` toolchain, which is exactly why the missing
/// delegation went unnoticed for as long as it did.
fn descriptor_label(descriptor: &ToolDescriptor) -> Option<&str> {
    let label = descriptor.label.as_str();
    (!label.is_empty()).then_some(label)
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
    /// The guest's declared `label` (pi `ToolDefinition.label: string`, "Human-readable label for
    /// UI", `extensions/types.ts:452-453` @v0.83.0 — a REQUIRED field upstream, which is why the
    /// WIT `tool-descriptor.label` is a bare `string` and not an `option<string>`).
    ///
    /// EXT-M03, the delegation-omission class sweep 6 generalised from `wrapRegisteredTool`: this
    /// override did not exist, so `<WasmTool as Tool>::label` fell through to the trait DEFAULT
    /// (`None`, `cyrup-core/src/tool.rs`). The field crossed the entire ABI with no reader — the SDK
    /// builder sets it (`cyrup-ext-sdk/src/descriptor.rs:132`, defaulted to the name at `:113`, and
    /// set explicitly by all three `tool_factory` presets), `lower_tool_descriptor` lowers it
    /// (`cyrup-ext-sdk/src/guest.rs:58`), the WIT declares it, and `register_tool` lifts it into
    /// `ToolDescriptor.label` at `:113` above — where it then stopped, write-only. Exactly the
    /// EXT-007 `prompt_guidelines` shape and exactly the hazard that hides from tests: a dropped
    /// delegation returns the same `None` a fixture that never sets a label would.
    ///
    /// The asymmetry this closes is the real defect: a NATIVE tool can express a label (it writes
    /// its own `impl Tool`), a WASM guest could not, no matter what it declared.
    ///
    /// An EMPTY label maps to `None` rather than `Some("")` — the trait documents `None` as "fall
    /// back to the tool name", which is what upstream's always-populated `label` degenerates to,
    /// and a non-SDK guest sending `""` must not blank the UI row.
    fn label(&self) -> Option<&str> {
        descriptor_label(&self.descriptor)
    }
    /// The guest's declared `promptSnippet` (Pi `ToolDefinition.promptSnippet`,
    /// `extensions/types.ts:456-457` @v0.83.0; the `:442-443` this line used to cite is
    /// `isError: boolean` on `ToolRenderContext`, a different interface — re-derived this pass
    /// while adding `label` below). `None` when the descriptor omits it, which keeps the tool out
    /// of the system prompt's "Available tools" section exactly as Pi's
    /// `tools.filter(name => !!toolSnippets?.[name])` does (system-prompt.ts:79-80).
    fn prompt_snippet(&self) -> Option<&str> {
        self.descriptor.prompt_snippet.as_deref()
    }
    /// The guest's declared `promptGuidelines` (pi `ToolDefinition.promptGuidelines`,
    /// `extensions/types.ts:458-459` @v0.83.0; `:444-446` was stale by the same -14 as the
    /// `promptSnippet` cite above, while the `:463`/`:465`/`:468` cites its neighbours carry are
    /// EXACT — the tell that the rot is confined to this pair's original import) — the bullets that
    /// reach the system prompt's
    /// "Guidelines" section.
    ///
    /// TOOL-021 / EXT-007: this override could not be written until `Tool::prompt_guidelines`
    /// returned an OWNED slice. The data was already all the way here — declared in `world.wit`
    /// (`prompt-guidelines`), copied off the descriptor at `register_tool` and stored on
    /// `RegisteredTool` — but the trait's `-> &[&str]` can only be satisfied by a borrow of a
    /// `[&'static str]`, which a `Vec<String>` decoded from a component cannot produce. So the
    /// field crossed the ABI and had NO reader: a guest declaring guidelines contributed nothing,
    /// silently, with no warning on either side.
    fn prompt_guidelines(&self) -> Vec<&str> {
        self.descriptor.prompt_guidelines.iter().map(String::as_str).collect()
    }
    /// The guest's declared `renderShell` (pi `ToolDefinition.renderShell?: "default" | "self"`,
    /// `extensions/types.ts:465` @v0.83.0: "Controls whether ToolExecutionComponent renders the
    /// standard colored shell or the tool renders its own framing"). EXT-024 — the field had no WIT
    /// representation at all, so a self-rendering guest tool still drew the default row chrome.
    /// Anything other than the literal `"self"` is upstream's `"default"`.
    fn render_kind(&self) -> cyrup_core::ToolRenderKind {
        match self.descriptor.render_shell.as_deref() {
            Some("self") => cyrup_core::ToolRenderKind::SelfRendered,
            _ => cyrup_core::ToolRenderKind::Default,
        }
    }
    /// The guest's declared `constrainedSampling` (pi
    /// `ToolDefinition.constrainedSampling?: false | ConstrainedSamplingConfig`,
    /// `extensions/types.ts:463` @v0.83.0). PROV-011 / EXT-024 — the field had no WIT
    /// representation and `cyrup_core::Tool` had no accessor, so a guest tool asking for
    /// grammar- or strict-JSON-schema-constrained sampling was answered with silence at BOTH
    /// ends. Upstream the copy happens in `wrapToolDefinition`
    /// (`core/tools/tool-definition-wrapper.ts:14`); this accessor is that copy.
    fn constrained_sampling(&self) -> Option<&cyrup_core::ConstrainedSampling> {
        self.descriptor.constrained_sampling.as_ref()
    }
    /// pi `ToolDefinition.prepareArguments?: (args: unknown) => Static<TParams>`
    /// (`extensions/types.ts:468` @v0.83.0), run BEFORE `validateToolArguments` in
    /// `packages/agent/src/agent-loop.ts`. EXT-023 — the whole shim was unreachable across the WASM
    /// boundary: the string `prepare` did not occur in `world.wit`, and the SDK accepted a
    /// documented `prepare_arguments` field that `lower_tool_descriptor` dropped.
    ///
    /// Only called across the boundary when the descriptor DECLARED the shim, so a tool that does
    /// not use it costs no round trip. A guest fault or an unparseable return degrades to the
    /// identity — pi's absent `prepareArguments` — rather than failing the call: the arguments are
    /// about to be schema-validated anyway, which is where a genuinely bad shape is reported.
    async fn prepare_arguments(&self, args: Value) -> Value {
        if !self.descriptor.prepare_arguments {
            return args;
        }
        match self.ext.prepare_arguments(&self.descriptor.name, &args).await {
            Ok(Some(v)) => v,
            Ok(None) => args,
            Err(e) => {
                tracing::warn!(
                    tool = %self.descriptor.name,
                    error = %e,
                    "guest `prepare-arguments` faulted; using the raw arguments (pi's identity default)"
                );
                args
            }
        }
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
    /// EXT-058 — read LIVE off [`GuestState`] on every call, never snapshotted. pi's `api.on(event,
    /// handler)` pushes into `extension.handlers` (`extensions/loader.ts:252-258` @v0.83.0) and
    /// every emitter re-reads `ext.handlers.get(event.type)` at dispatch time (`runner.ts:571`,
    /// `:806`, `:841`, `:883`, `:937`, `:959`, `:989`, `:1021`, …), so a `subscribe` from a live
    /// handler takes effect on the NEXT event. Snapshotting this once after `init` made the
    /// `subscribe` import a write with no reader.
    fn subscriptions(&self) -> Subscriptions {
        self.guest.subscriptions()
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
        HostEvent::ToolResult { call_id, name, input, content, details, is_error, usage } => {
            let content_json = serde_json::to_string(content).unwrap_or_else(|_| "[]".into());
            let details_json = details.as_ref().map(|d| d.to_string());
            // Pi `ToolResultEventBase.usage` (types.ts:919-921): absent for every ordinary tool, so
            // an unserializable value degrades to absent rather than to a bogus payload.
            let usage_json = usage.as_ref().and_then(|u| serde_json::to_string(u).ok());
            api.call_on_tool_result(
                store,
                call_id.as_str(),
                name,
                &input.to_string(),
                &content_json,
                *is_error,
                details_json.as_deref(),
                usage_json.as_deref(),
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
        // EXT-009 — pi `emitBeforeProviderHeaders` (extensions/runner.ts:1045 @v0.83.0).
        HostEvent::BeforeProviderHeaders { headers } => {
            api.call_on_before_provider_headers(store, &headers.to_string()).await
        }
        // EXT-016 / EXT-043: both events now carry the cwd they are being asked about.
        HostEvent::ResourcesDiscover { cwd, reason } => {
            api.call_on_resources_discover(store, cwd, reason).await
        }
        HostEvent::ProjectTrust { cwd } => api.call_on_project_trust(store, cwd).await,
        // EXT-015: the four session-lifecycle events keep their discriminating fields.
        HostEvent::SessionBeforeSwitch { reason, target_session_file } => {
            api.call_on_session_before_switch(store, reason, target_session_file.as_deref()).await
        }
        HostEvent::SessionBeforeFork { entry_id, position } => {
            api.call_on_session_before_fork(store, entry_id, position).await
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
        // agent_settled (Pi `_emitAgentSettled`, agent-session.ts:581-588): payload-free, notify-only.
        HostEvent::AgentSettled => {
            api.call_on_agent_settled(store).await.and_then(|()| noop())
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
            .call_on_tool_execution_start(store, call_id.as_str(), name, &args.to_string())
            .await
            .and_then(|()| noop()),
        HostEvent::ToolExecUpdate { call_id, name, args, chunk } => api
            .call_on_tool_execution_update(
                store,
                call_id.as_str(),
                name,
                &args.to_string(),
                &chunk.to_string(),
            )
            .await
            .and_then(|()| noop()),
        HostEvent::ToolExecEnd { call_id, name, result, is_error } => api
            .call_on_tool_execution_end(store, call_id.as_str(), name, &result.to_string(), *is_error)
            .await
            .and_then(|()| noop()),
        HostEvent::SessionStart { reason, previous_session_file } => api
            .call_on_session_start(store, reason, previous_session_file.as_deref())
            .await
            .and_then(|()| noop()),
        HostEvent::SessionShutdown { reason, target_session_file } => api
            .call_on_session_shutdown(store, reason, target_session_file.as_deref())
            .await
            .and_then(|()| noop()),
        // EXT-011 — pi `SessionInfoChangedEvent` (extensions/types.ts:571-575 @v0.83.0).
        HostEvent::SessionInfoChanged { name } => {
            api.call_on_session_info_changed(store, name.as_deref()).await.and_then(|()| noop())
        }
        HostEvent::AfterProviderResponse { status, headers } => api
            .call_on_after_provider_response(store, *status, &headers.to_string())
            .await
            .and_then(|()| noop()),
        // EXT-042: `previousModel`/`source` and `previousLevel` are pi SIBLING fields, not
        // members of the `model` blob.
        HostEvent::ModelSelect { model, previous_model, source } => {
            let prev = previous_model.as_ref().map(|v| v.to_string());
            api.call_on_model_select(store, &model.to_string(), prev.as_deref(), source)
                .await
                .and_then(|()| noop())
        }
        HostEvent::ThinkingLevelSelect { level, previous_level } => api
            .call_on_thinking_level_select(store, level, previous_level.as_deref())
            .await
            .and_then(|()| noop()),
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
        // EXT-049: `block` is a record now — `{reason, terminate}` — so a guest can express pi's
        // `ToolCallEventResult.terminate` (extensions/types.ts:1072-1079 @v0.84.1).
        wit_types::HookOutcome::Block(b) => {
            HookOutcome::Block { reason: b.reason, terminate: b.terminate }
        }
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
            // Pi `ToolResultEventResult.usage` (types.ts:1085-1090). A malformed value is dropped
            // (treated as "not patched") rather than failing the whole patch.
            let usage = v
                .get("usage")
                .cloned()
                .and_then(|u| serde_json::from_value::<cyrup_core::Usage>(u).ok());
            Some(EventPatch::ToolResult { content, details, is_error, usage })
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
        // `before_provider_headers` (EXT-009, pi types.ts:681-685): the guest's mutate value is a
        // header PATCH — set each key, and a `null` value DELETES that header.
        EventKind::BeforeProviderHeaders => Some(EventPatch::ProviderHeaders(v)),
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

    /// EXT-M03 — a guest tool's declared `label` (pi `ToolDefinition.label: string`, "Human-readable
    /// label for UI", `extensions/types.ts:452-453` @v0.83.0) reaches `Tool::label`.
    ///
    /// The defect was a MISSING delegation, not a wrong one: `impl Tool for WasmTool` overrode
    /// every other descriptor-backed accessor (`description`, `prompt_snippet`,
    /// `prompt_guidelines`, `render_kind`, `constrained_sampling`, `prepare_arguments`) and simply
    /// omitted `label`, so it fell through to `Tool::label`'s default `None`. The field crossed the
    /// whole ABI — SDK builder, `lower_tool_descriptor`, WIT record, `register_tool`'s lift into
    /// `ToolDescriptor.label` — and had no reader on the far side.
    ///
    /// This is the vacuous-pass shape the sweep briefs warn about, which is why the assertions
    /// below use DISTINCT non-default values: a label that differs from the name (so a test that
    /// accidentally read `name` would fail), and the empty case asserted separately (so
    /// `Some("") != None` cannot be confused with the fallback).
    #[test]
    fn a_guest_tools_declared_label_reaches_the_tool_surface() {
        let described = ToolDescriptor {
            name: "run_migrations".into(),
            // Deliberately NOT the name, and not the description either.
            label: "Run Migrations".into(),
            description: "applies pending migrations".into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            execution_mode: None,
            prompt_snippet: None,
            prompt_guidelines: Vec::new(),
            has_renderer: false,
            prepare_arguments: false,
            render_shell: None,
            constrained_sampling: None,
        };
        assert_eq!(
            descriptor_label(&described),
            Some("Run Migrations"),
            "the guest's declared label must survive to `Tool::label`, not fall back to the name"
        );
        assert_ne!(
            descriptor_label(&described),
            Some(described.name.as_str()),
            "and it must be the LABEL, not the name — the pre-fix behaviour was `None`, which the \
             runtime renders as the name"
        );

        // The SDK defaults `label` to the name (`cyrup-ext-sdk/src/descriptor.rs:113`), so an EMPTY
        // label only reaches the host from a guest that sent one deliberately. `Tool::label`
        // documents `None` as "fall back to the tool name"; `Some("")` would blank the UI row.
        let blank = ToolDescriptor { label: String::new(), ..described };
        assert_eq!(descriptor_label(&blank), None, "an empty label is the name fallback, not an empty row");
    }

    /// EXT-M03, the half the mapping test above CANNOT cover — and the half that was actually
    /// broken.
    ///
    /// `descriptor_label` is a free function, so a test of it passes whether or not
    /// `impl Tool for WasmTool` calls it: deleting the `fn label` override again would leave the
    /// test above green while `Tool::label` silently returned the trait default. Constructing a
    /// `WasmTool` to check the real vtable needs an `Arc<LiveExtension>` — a compiled component and
    /// the `wasm32-wasip2` toolchain — which is precisely why the omission survived. So the
    /// delegation is pinned at the source level instead, the same way `tests/wit_world_sync.rs`
    /// pins the two `world.wit` copies.
    ///
    /// Generalised deliberately: every accessor listed here is descriptor-backed, and each one that
    /// goes missing degrades to a trait default that a fixture leaving the field unset cannot tell
    /// apart from a correct answer. `render_call`/`render_result` are NOT listed — a guest renderer
    /// is routed by `has_renderer` through the registry's tool-renderer table, not off this impl.
    #[test]
    fn the_wasm_tool_impl_delegates_every_descriptor_backed_accessor() {
        let src = include_str!("live.rs");
        let block = src
            .split_once("impl Tool for WasmTool {")
            .map(|(_, rest)| rest.split("\n}\n").next().unwrap_or(rest))
            .expect("`impl Tool for WasmTool` block present");

        // Non-vacuity: prove the slice really is just that impl body. If the `\n}\n` split ever
        // over-reads, the block would swallow the rest of the file and every assertion below would
        // pass for the wrong reason. `impl Extension for LiveExtension` is the next item in the
        // file, and `fn label(` genuinely does not appear in it.
        assert!(
            !block.contains("impl Extension for LiveExtension"),
            "the extracted block over-ran the `impl Tool for WasmTool` body, so the assertions \
             below would be vacuous"
        );

        for method in [
            "fn name(",
            "fn parameters(",
            "fn execution_mode(",
            "fn description(",
            "fn label(",
            "fn prompt_snippet(",
            "fn prompt_guidelines(",
            "fn render_kind(",
            "fn constrained_sampling(",
            "fn prepare_arguments(",
            "fn execute(",
        ] {
            assert!(
                block.contains(method),
                "`impl Tool for WasmTool` no longer overrides `{method})` — a dropped delegation \
                 falls through to the `Tool` trait default, which is indistinguishable from a \
                 descriptor that left the field unset (EXT-M03; `label` was missing exactly this \
                 way). Delete a line here only together with the reason it is no longer \
                 descriptor-backed."
            );
        }
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

    /// L4 round-12 finding #3: a guest `cwd: ""` must fold to `None` — NOT `Some(PathBuf::from(""))`
    /// — before it reaches `HostServices::proc_spawn`, so that call's EXISTING omitted-cwd fallback
    /// (`host_services.rs`, defaults to the session's project cwd) actually fires. This is the
    /// cyrup-analog of Node's real `child_process.spawn` treating a falsy `cwd` as "no override"
    /// (`pi-mcp-adapter/utils.ts:78-87`'s `resolveConfigPath("")` returns `""` verbatim, consumed at
    /// `server-manager.ts:110` -> `client/stdio.js:72-82`'s `cross_spawn(...,{cwd:""})`, verified live
    /// against Node: exits 0, inherits the parent's ambient cwd) — unlike
    /// `std::process::Command::current_dir("")`, which hard-fails the spawn (verified live: `Os {
    /// code: 2, kind: NotFound, .. }`). Also covers a `${UNDEFINED_VAR}` placeholder that
    /// interpolates to empty, the SAME real-world case Pi's `resolveConfigPath` treats identically to
    /// a literal `""` (both fail the `"~"` checks and return `""` verbatim).
    #[tokio::test]
    async fn spawn_folds_a_cwd_that_resolves_to_empty_to_none_not_some_empty_path() {
        use bindings::cyrup::ext::proc::Host as ProcHost;

        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec.clone());

        // A literal empty string.
        ProcHost::spawn(&mut state, "true".into(), vec![], "{}".into(), Some(String::new()), false)
            .await
            .expect("spawn succeeds");
        // An undefined-var placeholder that interpolates to empty (`${DEFINITELY_NOT_SET_...}`).
        ProcHost::spawn(
            &mut state,
            "true".into(),
            vec![],
            "{}".into(),
            Some("${CYRUP_L4_ROUND12_FINDING3_UNDEFINED_VAR}".into()),
            false,
        )
        .await
        .expect("spawn succeeds");

        let cwds = rec.proc_spawn_cwds();
        assert_eq!(cwds.len(), 2);
        assert_eq!(
            cwds.first().cloned().flatten(),
            None,
            "a literal empty guest cwd must fold to None, letting proc_spawn's own omitted-cwd \
             fallback fire — not reach it as Some(\"\")"
        );
        assert_eq!(
            cwds.get(1).cloned().flatten(),
            None,
            "a cwd that INTERPOLATES to empty (an undefined ${{VAR}}) must fold to None the same way"
        );
    }

    /// The `proc::Host::spawn` WIT handler must record `note_dialog_wait` around
    /// `HostServices::proc_spawn` — the SAME epoch-forgiveness bookkeeping `write_stdin`/`kill`
    /// (this file) already carry for their own occasionally-slow calls. `proc_spawn` can run
    /// `ProcCaps::spawn`'s `block_in_place`-wrapped `npx_resolver::resolve_npx_binary`, which on a
    /// cold cache blocks up to `npx_resolver::FORCE_CACHE_TIMEOUT` (30s) — six times the ~5s WASM
    /// epoch budget — with no way for the guest to observe or interrupt it. Proven by arming a short
    /// epoch budget, calling the REAL production `proc::Host::spawn` once, and observing
    /// `take_dialog_extra_ticks()` return NONZERO (floor-1) forgiveness, which only happens when a
    /// wait was actually recorded: `take_dialog_extra_ticks_is_zero_with_no_recorded_wait`
    /// (`host/services.rs`) proves the converse — no recorded wait means exactly `0`. Before this
    /// fix, `spawn` never called `note_dialog_wait` at all, so this assertion would have failed.
    #[tokio::test]
    async fn spawn_records_a_dialog_wait_for_epoch_forgiveness() {
        use bindings::cyrup::ext::proc::Host as ProcHost;

        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec);
        let guest = guest_of(&state).expect("guest state present").clone();
        guest.arm_epoch_deadline_estimate(20); // a 20-tick (100ms) per-dispatch budget, as elsewhere

        ProcHost::spawn(&mut state, "true".into(), vec![], "{}".into(), None, false)
            .await
            .expect("spawn succeeds");

        assert!(
            guest.take_dialog_extra_ticks() > 0,
            "proc.spawn must record a dialog wait so the epoch trap forgives the call's real \
             wall-clock duration — without it, a slow npx/npm cold-cache resolution would trap the \
             instance the instant the guest resumes execution right after this call returns"
        );
    }

    /// The `ui::Host::custom` WIT handler must record `note_dialog_wait` around
    /// `HostServices::custom` — the SAME epoch-forgiveness bookkeeping its siblings `confirm`/
    /// `input`/`select`/`editor` (this file, `ui::Host` impl above) already carry. A custom overlay
    /// is exactly as human-paced a wait once a real backend answers it (Pi `custom()`); before this
    /// fix `custom` never called `note_dialog_wait` at all, so this assertion would have failed —
    /// same proof shape as `spawn_records_a_dialog_wait_for_epoch_forgiveness` above.
    #[tokio::test]
    async fn custom_records_a_dialog_wait_for_epoch_forgiveness() {
        use bindings::cyrup::ext::ui::Host as UiHost;

        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec);
        let guest = guest_of(&state).expect("guest state present").clone();
        guest.arm_epoch_deadline_estimate(20); // a 20-tick (100ms) per-dispatch budget, as elsewhere

        UiHost::custom(&mut state, "{}".into()).await;

        assert!(
            guest.take_dialog_extra_ticks() > 0,
            "ui.custom must record a dialog wait so the epoch trap forgives the call's real \
             wall-clock duration — without it, a slow custom-overlay answer would trap the instance \
             the instant the guest resumes execution right after this call returns"
        );
    }

    /// L4 round-17 finding #3: the `oauth::Host::on_prompt` WIT handler must record
    /// `note_dialog_wait` around `HostServices::oauth_prompt` — the SAME epoch-forgiveness
    /// bookkeeping every OTHER dialog kind (`ui::Host::confirm`/`input`/`select`/`editor`/`custom`,
    /// this file, above) already carries. An OAuth prompt is exactly as human-paced a wait once a
    /// real backend answers it (Pi `onPrompt`); before this fix `on_prompt` never called
    /// `note_dialog_wait` at all, so this assertion would have failed — same proof shape as
    /// `custom_records_a_dialog_wait_for_epoch_forgiveness` above.
    #[tokio::test]
    async fn oauth_on_prompt_records_a_dialog_wait_for_epoch_forgiveness() {
        use bindings::cyrup::ext::oauth::Host as OauthHost;

        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec);
        let guest = guest_of(&state).expect("guest state present").clone();
        guest.arm_epoch_deadline_estimate(20); // a 20-tick (100ms) per-dispatch budget, as elsewhere

        OauthHost::on_prompt(&mut state, "enter code".into(), None, false)
            .await
            .expect("canned oauth prompt answers Ok");

        assert!(
            guest.take_dialog_extra_ticks() > 0,
            "oauth.on-prompt must record a dialog wait so the epoch trap forgives the call's real \
             wall-clock duration — without it, a slow real OAuth prompt answer would trap the \
             instance the instant the guest resumes execution right after this call returns"
        );
    }

    /// Same finding as `oauth_on_prompt_records_a_dialog_wait_for_epoch_forgiveness` above, for the
    /// sibling `oauth::Host::on_select` WIT handler (Pi `onSelect`) — before this fix, `on_select`
    /// ALSO never called `note_dialog_wait`.
    #[tokio::test]
    async fn oauth_on_select_records_a_dialog_wait_for_epoch_forgiveness() {
        use bindings::cyrup::ext::oauth::Host as OauthHost;

        let rec = Arc::new(RecordingServices::new(CannedResponses::default()));
        let mut state = state_with(rec);
        let guest = guest_of(&state).expect("guest state present").clone();
        guest.arm_epoch_deadline_estimate(20); // a 20-tick (100ms) per-dispatch budget, as elsewhere

        let _ = OauthHost::on_select(&mut state, "pick an account".into(), "[]".into()).await;

        assert!(
            guest.take_dialog_extra_ticks() > 0,
            "oauth.on-select must record a dialog wait so the epoch trap forgives the call's real \
             wall-clock duration — without it, a slow real OAuth selection answer would trap the \
             instance the instant the guest resumes execution right after this call returns"
        );
    }
}
