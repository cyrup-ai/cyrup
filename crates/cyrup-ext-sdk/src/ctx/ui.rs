//! The `ui` WIT import: notifications, status keys, dialogs, widgets, chrome, the editor buffer,
//! themes and the working indicator — pi's `ExtensionUIContext` in full.

use serde::Serialize;
use serde_json::Value;

use crate::descriptor::DialogOptions;
use crate::widget::WidgetPlacement;

/// Notification severity (Pi `notify(message, type?)`, `extensions/types.ts:142` @v0.83.0).
/// [`NotifyKind::Info`] is Pi's default when the argument is omitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotifyKind {
    /// Informational — Pi's default when `notify`'s optional type argument is omitted, and the
    /// [`Default`] here.
    #[default]
    Info,
    /// Warning severity.
    Warning,
    /// Error severity — the level the SDK itself uses when it has to report that an author payload
    /// was dropped rather than sent (`Ctx::emit`, `ToolCall::emit_update`, [`Ui::custom`]).
    Error,
}

#[cfg(target_arch = "wasm32")]
impl NotifyKind {
    fn to_wit(self) -> crate::guest::bindings::cyrup::ext::ui::NotifyKind {
        use crate::guest::bindings::cyrup::ext::ui::NotifyKind as Wit;
        match self {
            NotifyKind::Info => Wit::Info,
            NotifyKind::Warning => Wit::Warning,
            NotifyKind::Error => Wit::Error,
        }
    }
}

/// The UI capability surface (Pi `ExtensionUIContext`, `extensions/types.ts:131-282` @v0.83.0 —
/// the interface's closing brace is `:282`; the `:290` this line used to carry is inside
/// `ContextUsage`, two interfaces later).
///
/// # EXT-036 — the citation re-run, now COMPLETE
///
/// The first pass re-derived the `types.ts` citations on this type's methods one by one against
/// `v0.83.0` and found a cluster stale by a consistent ~7 lines: `notify` cited `:135` (that line
/// is `confirm`'s doc comment; `notify` is `:142`), `setStatus` `:141` (it is `:148`), `select`
/// `:127` (`:133`), `editor` `:216` (that is `setEditorText`; `editor` is `:222`), the
/// dialog-options `AbortSignal` `:89-94` (`:95-101`, while the sibling comment four lines below it
/// already said `:95-100` — the file contradicted itself, which is EXT-036's signature), and the
/// chrome trio `:130-150` (they are `:183`/`:190`/`:193`). The uniform offset says these were taken
/// against a pi revision this region is ~7 lines shorter in, not invented.
///
/// That pass could not touch the two `world.wit` copies — the world was frozen for the 0.6 bump —
/// and left a register naming the exact lines to fix. **Those are now fixed**, in both copies and
/// in the host, and re-running the sweep across the whole import surface rather than just this type
/// turned up **three more clusters the register did not know about**:
///
/// 1. **`modes/rpc/rpc-types.ts`, uniform +8.** `confirm` cited `:232` (a BLANK line; it is `:240`)
///    and `input` cited `:233-240` (a banner comment; it is `:241-248`). Five sites: both
///    `world.wit` copies, `cyrup-ext/src/host/services.rs`, this file, and
///    `example/commands_ui.rs`.
/// 2. **`cyrup-ext/src/host/services.rs`'s fire-and-forget block, uniform ~+6.** `notify` `:136`,
///    `setStatus` `:141-142`, `setHeader` `:184`, `setFooter` `:174-177`, `setTitle` `:187`,
///    `setEditorText` `:210`, `setToolsExpanded` `:275`, and the `rpc-mode.ts:149,163,196`
///    fire-and-forget cite (it is `:152`/`:168`, with `:218`/`:238` for the title and editor
///    verbs). The tell that this is import rot and not systematic misreading: every citation ADDED
///    to that same block by the EXT-021/EXT-047 pass (`:151`, `:154`, `:164`, `:167`, `:170-175`)
///    is exact.
/// 3. **`types.ts`'s `pi.*` region — NOT an off-by-N, wrong surface entirely.** `getFlag` cited
///    `:1218` (that is `on("turn_start")`; it is `:1269`), `unregisterProvider` `:1361` (an
///    `@example` line; `:1416`), `getActiveTools`/`getCommands` `:1257-1266` (`:1320`/`:1329`),
///    `setThinkingLevel` `:1288` (`:1342`), the `Models` view `:1273-1279` (that is
///    `registerMessageRenderer`/`registerEntryRenderer`; the surface is `:319`/`:326`/`:341`/
///    `:1336`/`:1342`), `registerEntryRenderer` `:1295` (`sendUserMessage`; it is `:1279`),
///    `registerMessageRenderer` `:1284` (a blank line; `:1276`), and `agent_settled` `:1225`
///    (`tool_execution_end`; it is `:1217`).
///
/// **Two FABRICATIONS, not staleness, were also found** and are recorded where they live rather
/// than here: `pasteEditorText` (`world.wit`'s editor-buffer comment and
/// `services.rs::set_editor_text`) names a function pi has at no version — the upstream name is
/// `pasteToEditor`, `types.ts:213` — and `ui.custom` cited `types.ts:175`, the closing paren of the
/// `setWidget` component-factory overload, where `custom` is `:196`. Both are the same class as the
/// `working-start`/`working-stop` fabrication EXT-021 caught, and both were hidden by a citation
/// wide enough to look plausible (`:200-230`).
///
/// Method for the next auditor: a citation is only checkable if it names ONE line or a range whose
/// endpoints are both meaningful. Every fabrication found in this area so far hid inside a range.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ui;

impl Ui {
    /// Show an `info`-severity notification (Pi `notify(message)`; the `type` defaults to `"info"`).
    pub fn notify(&self, message: &str) {
        self.notify_with(message, NotifyKind::Info);
    }
    /// Show a notification with an explicit severity (Pi `notify(message, type)`, `types.ts:142`).
    pub fn notify_with(&self, message: &str, kind: NotifyKind) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::notify(message, kind.to_wit());
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (message, kind);
    }
    /// Set a keyed status segment (Pi `setStatus(key, text)`, `types.ts:148`). Pass [`None`] for
    /// `text` to clear that segment (Pi `setStatus(key, undefined)`).
    pub fn set_status(&self, key: &str, text: Option<&str>) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_status(key, text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (key, text);
    }
    /// Clear a keyed status segment (Pi `setStatus(key, undefined)`).
    pub fn clear_status(&self, key: &str) {
        self.set_status(key, None);
    }
    /// Programmatically dismiss any dialog bound to `signal_id` (Pi `ExtensionUIDialogOptions.signal`
    /// `AbortSignal.abort()`, `types.ts:95-101`; sdk gap #2). A dialog subsequently opened via
    /// [`Self::confirm_with`]/[`Self::input_with`]/[`Self::select_with`] carrying that signal id
    /// returns cancelled.
    pub fn abort_signal(&self, signal_id: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::abort_signal(signal_id);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = signal_id;
    }

    /// Stop receiving raw terminal input — the unsubscribe closure pi's
    /// `onTerminalInput(handler): () => void` returns (`extensions/types.ts:145` @v0.83.0,
    /// "Listen to raw terminal input (interactive mode only). Returns an unsubscribe function").
    /// Idempotent, like upstream's `Set.delete`.
    ///
    /// EXT-M04, the same defect EXT-050 closed for the event bus, left open on this pair by the
    /// same pass. The host half has existed since the 0.6 -> 0.7 bump —
    /// `ui.unsubscribe-terminal-input` is declared in `world.wit`, implemented at
    /// `cyrup-ext/src/host/live.rs` and backed by `ExtensionRegistry::unsubscribe_terminal_input`
    /// — but NOTHING in this SDK ever called it: `guest::init` calls
    /// `ui::subscribe_terminal_input()` when the api declares a handler and there was no way back.
    /// A declared import with no caller on the guest side is the mirror image of the EXT-023 class
    /// (a declared field with no reader on the host side), and it made pi's return value —
    /// the entire point of `onTerminalInput` for an extension that listens only while an overlay
    /// or mode is up — unreachable.
    ///
    /// The handler itself stays registered ([`crate::api::ExtensionApi::on_terminal_input`] is an
    /// init-time factory call, since a closure cannot cross the component boundary); this takes
    /// down the HOST-side subscription, which is what stops the `on-terminal-input` export from
    /// being invoked. Pair with [`Self::subscribe_terminal_input`] to resume.
    pub fn unsubscribe_terminal_input(&self) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::unsubscribe_terminal_input();
    }

    /// Resume receiving raw terminal input after [`Self::unsubscribe_terminal_input`] (EXT-M04).
    ///
    /// `guest::init` already calls this once for an extension whose factory registered a handler,
    /// so a guest that never unsubscribes never needs it. Calling it without a registered handler
    /// subscribes to input that this guest's `on-terminal-input` export will answer with pi's
    /// `undefined` (no-op) — harmless, and the same thing upstream's `onTerminalInput(() => {})`
    /// does.
    pub fn subscribe_terminal_input(&self) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::subscribe_terminal_input();
    }
    /// Confirmation dialog (Pi `confirm`). Indefinite, no message body; use [`Self::confirm_with`]
    /// for a message/timeout/signal.
    pub fn confirm(&self, prompt: &str) -> bool {
        self.confirm_with(prompt, "", &DialogOptions::default())
    }
    /// Confirmation dialog with a message body and a [`DialogOptions`] bag (Pi
    /// `confirm(title, message, {timeout, signal})`, rpc-types.ts:240 @v0.83.0): `prompt` is the short title,
    /// `message` the (often large, formatted) body — e.g. pi-mcp-adapter's sampling handler passes a
    /// label as `title` and the full prompt/conversation text as `message`.
    pub fn confirm_with(&self, prompt: &str, message: &str, opts: &DialogOptions) -> bool {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::confirm(prompt, message, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, message, opts_json);
            false
        }
    }
    /// Text input dialog (Pi `input`). No placeholder; use [`Self::input_with`] to set one.
    pub fn input(&self, prompt: &str) -> Option<String> {
        self.input_with(prompt, None, &DialogOptions::default())
    }
    /// Text input dialog with a placeholder and a [`DialogOptions`] bag (Pi
    /// `input(title, placeholder, {timeout, signal})`, rpc-types.ts:241-248 @v0.83.0); forwarded live to the
    /// renderer. `placeholder = None` omits the wire field entirely, matching Pi's optional field.
    pub fn input_with(
        &self,
        prompt: &str,
        placeholder: Option<&str>,
        opts: &DialogOptions,
    ) -> Option<String> {
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::input(prompt, placeholder, &opts_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, placeholder, opts_json);
            None
        }
    }
    /// Single-choice select; returns the chosen option string (Pi `select(title, options, opts):
    /// Promise<string|undefined>`, `types.ts:133`).
    pub fn select(&self, prompt: &str, options: &[&str]) -> Option<String> {
        self.select_with(prompt, options, &DialogOptions::default())
    }
    /// Single-choice select with a [`DialogOptions`] bag (Pi `select(title, options, {timeout, signal})`).
    pub fn select_with(
        &self,
        prompt: &str,
        options: &[&str],
        opts: &DialogOptions,
    ) -> Option<String> {
        let options_json = serde_json::to_string(options).unwrap_or_else(|_| "[]".into());
        let opts_json = serde_json::to_string(opts).unwrap_or_else(|_| "{}".into());
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::select(
                prompt,
                &options_json,
                &opts_json,
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (prompt, options_json, opts_json);
            None
        }
    }
    /// Multiline editor labeled `title`, seeded with `initial` (Pi `editor(title, prefill):
    /// Promise<string|undefined>`, `types.ts:222`); returns the edited text (None = cancelled).
    pub fn editor(&self, title: &str, initial: &str) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::editor(title, initial);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (title, initial);
            None
        }
    }
    /// Set the widget stored under `key` (Pi `setWidget(key, content, options?)`,
    /// `extensions/types.ts:170-175` @v0.83.0).
    ///
    /// EXT-047: this used to take one opaque payload, which meant two extensions setting a widget
    /// clobbered each other and a widget could not be removed at all. `key` makes the surface the
    /// MAP it is upstream; [`Self::clear_widget`] is upstream's `content: undefined`.
    ///
    /// `placement` is `None` for upstream's default `"aboveEditor"`
    /// (`ExtensionWidgetOptions.placement`, `:107-110`).
    pub fn set_widget(&self, key: &str, lines: &[String], placement: Option<WidgetPlacement>) {
        let content_json = serde_json::to_string(lines).unwrap_or_else(|_| "[]".into());
        let opts_json = match placement {
            Some(p) => format!(r#"{{"placement":"{}"}}"#, p.as_str()),
            None => "{}".to_string(),
        };
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_widget(key, Some(&content_json), &opts_json);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (key, content_json, opts_json);
    }

    /// Remove the widget stored under `key` — Pi's `setWidget(key, undefined)`
    /// (`extensions/types.ts:170` @v0.83.0, `content: string[] | undefined`).
    ///
    /// Before EXT-047 there was no way to express this: the shipped subagents extension hand-rolled
    /// `{"key": …, "content": null}` and the host kept the slot occupied with a null payload.
    pub fn clear_widget(&self, key: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_widget(key, None, "{}");
        #[cfg(not(target_arch = "wasm32"))]
        let _ = key;
    }

    /// Set the chrome header line (Pi `setHeader`, `types.ts:190` @v0.83.0).
    ///
    /// Like its two siblings [`Self::set_footer`] and [`Self::set_title`] this is
    /// fire-and-forget — the WIT import returns nothing, so a host-side failure is not observable
    /// here — and a no-op on the host (non-`wasm32`) target.
    pub fn set_header(&self, content: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_header(content);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = content;
    }
    /// Set the chrome footer line (Pi `setFooter`, `types.ts:183` @v0.83.0); fire-and-forget, see
    /// [`Self::set_header`].
    pub fn set_footer(&self, content: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_footer(content);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = content;
    }
    /// Set the chrome title (Pi `setTitle`, `types.ts:193` @v0.83.0); fire-and-forget, see
    /// [`Self::set_header`].
    pub fn set_title(&self, title: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_title(title);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = title;
    }
    /// A custom overlay component; returns an optional serialized result (Pi `custom()`).
    ///
    /// **On an encode failure NO overlay is opened and this returns [`None`].** `spec` is
    /// author-supplied and its `serde_json` encoding is fallible; rather than opening an overlay
    /// from a `null` spec, the call is skipped and the error is surfaced as an error-severity
    /// [`Self::notify_with`] notification. `None` is already this method's "no result" answer, so
    /// the signature is unchanged.
    pub fn custom(&self, spec: impl Serialize) -> Option<String> {
        let spec_json = match serde_json::to_string(&spec) {
            Ok(s) => s,
            Err(e) => {
                self.notify_with(
                    &format!("ui.custom: overlay not opened, spec failed to encode: {e}"),
                    NotifyKind::Error,
                );
                return None;
            }
        };
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

    /// The editor buffer's current text (Pi `getEditorText`, `types.ts:219` @v0.83.0);
    /// `String::new()` on the host (non-`wasm32`) target.
    ///
    /// This method plus [`Self::set_editor_text`] (Pi `setEditorText`, `:216`) and
    /// [`Self::paste_editor_text`] (Pi `pasteToEditor`, `:213` — NOTE the upstream name is
    /// `pasteToEditor`; there is no `pasteEditorText` in pi, and the WIT import that carries this is
    /// spelled `paste-editor-text`) are the whole editor surface a guest gets.
    ///
    /// CYRUP-DELTA (EXT-021): pi additionally has `setEditorComponent(factory)` /
    /// `getEditorComponent()` (`extensions/types.ts:260`, `:263` @v0.83.0), where `EditorFactory`
    /// (`:125`) returns a live component the host then drives through a draw/handle-input/dispose
    /// protocol. That is a reference, not a value, and ADR-0002 makes extension I/O values — the
    /// full reasoning, and the reason `onTerminalInput` (`:145`) is an open GAP rather than a delta,
    /// is in the register at `crates/cyrup-ext/src/lib.rs`.
    pub fn editor_text(&self) -> String {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::get_editor_text();
        }
        #[cfg(not(target_arch = "wasm32"))]
        String::new()
    }
    /// Replace the editor buffer's text (Pi `setEditorText`, `types.ts:216` @v0.83.0); see
    /// [`Self::editor_text`] for the surface this belongs to.
    pub fn set_editor_text(&self, text: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_editor_text(text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = text;
    }
    /// Paste `text` into the editor buffer (Pi `pasteToEditor`, `types.ts:213` @v0.83.0 — see
    /// [`Self::editor_text`] on the name difference).
    pub fn paste_editor_text(&self, text: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::paste_editor_text(text);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = text;
    }

    /// The active theme's NAME only — [`Self::theme_json`] is what returns its colours (EXT-066).
    /// `None` when the host has no theme to report, and always `None` on the host (non-`wasm32`)
    /// target.
    ///
    /// The head of the theme read/list/switch group: [`Self::theme`], [`Self::theme_json`],
    /// [`Self::theme_list`], [`Self::theme_by_name`], [`Self::set_theme`], mirroring Pi
    /// `theme`/`getAllThemes`/`getTheme`/`setTheme` (`extensions/types.ts:266-275` @v0.83.0).
    pub fn theme(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::theme_get();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    /// The ACTIVE theme's colours, in the same serialized shape [`Self::theme_by_name`] returns
    /// (Pi's `readonly theme: Theme` property, `extensions/types.ts:266` @v0.83.0). `None` when the
    /// host has no theme to report.
    ///
    /// EXT-066: [`Self::theme`] returns only the NAME, so the live theme used to be the one theme a
    /// guest could not read the colours of — a renderer had to call `theme()` then
    /// `theme_by_name()`, which is two round trips and races a theme switch between them.
    ///
    /// `Some(`[`Value::Null`]`)` when the host DID report a theme but sent JSON this SDK could not
    /// parse (`super::parse_json`'s fallback). That is distinct from the `None` above — the
    /// unparseable case never collapses into "no theme".
    pub fn theme_json(&self) -> Option<Value> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::theme_get_json().map(super::parse_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// Every available theme as `{name, path}` (Pi `getAllThemes(): {name, path}[]`,
    /// `extensions/types.ts:269` @v0.83.0). `path` is null for a built-in theme — EXT-021: this
    /// returned bare names, so a guest could neither tell a built-in from a file-backed theme nor
    /// locate the file.
    ///
    /// [`Value::Null`] — NOT an empty array — when the host sent JSON this SDK could not parse
    /// (`super::parse_json`'s fallback), so a caller that treats a non-array as "no themes" cannot
    /// tell the two apart. The WIT import promises a JSON array (`wit/world.wit:716`).
    pub fn theme_list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::ui::theme_list());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }

    /// Load one theme by name WITHOUT switching to it (Pi `getTheme(name): Theme | undefined`,
    /// `extensions/types.ts:272` @v0.83.0). `None` = no such theme (EXT-021).
    ///
    /// `Some(`[`Value::Null`]`)` when the theme EXISTS but the host sent JSON this SDK could not
    /// parse (`super::parse_json`'s fallback). That is distinct from the `None` above — the
    /// unparseable case never collapses into "no such theme".
    pub fn theme_by_name(&self, name: &str) -> Option<Value> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::theme_get_by_name(name)
                .map(super::parse_json);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = name;
            None
        }
    }
    /// Switch the active theme by name (Pi `setTheme`, in the `extensions/types.ts:266-275` theme
    /// block @v0.83.0) — the one member of that group that mutates. `Ok(())` on the host
    /// (non-`wasm32`) target: it is fire-and-forget with nothing to hand back, the first arm of the
    /// module's host-target rule (see [`crate::ctx`]).
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

    // --- working / streaming indicator (Pi `ExtensionUIContext`, extensions/types.ts:151-167
    // @v0.83.0). EXT-021: cyrup had only the invented `working_start`/`working_stop` pair below,
    // whose `types.ts:265-275` citation pointed at getAllThemes/getTheme/setTheme/getToolsExpanded
    // — pi has no `startWorking`/`stopWorking` at all. ---

    /// Pi `setWorkingMessage(message?)` (`extensions/types.ts:151`): the message shown during
    /// streaming. `None` is upstream's no-argument call, "restore default".
    pub fn set_working_message(&self, message: Option<&str>) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_working_message(message);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = message;
    }

    /// Pi `setWorkingVisible(visible)` (`extensions/types.ts:154`): show/hide the built-in working
    /// loader row, INDEPENDENTLY of the message.
    pub fn set_working_visible(&self, visible: bool) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_working_visible(visible);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = visible;
    }

    /// Pi `setWorkingIndicator(options?)` (`extensions/types.ts:164`; the bag is
    /// `{frames?: string[], intervalMs?: number}` at `:116-121`). `None` restores the default
    /// animated spinner; `frames: []` hides the indicator entirely.
    pub fn set_working_indicator(&self, opts: Option<&Value>) {
        let opts_json = opts.map(|v| v.to_string());
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_working_indicator(opts_json.as_deref());
        #[cfg(not(target_arch = "wasm32"))]
        let _ = opts_json;
    }

    /// Pi `setHiddenThinkingLabel(label?)` (`extensions/types.ts:167`). `None` restores the
    /// default.
    pub fn set_hidden_thinking_label(&self, label: Option<&str>) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_hidden_thinking_label(label);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = label;
    }

    /// CYRUP-DELTA: cyrup-original — pi has no `startWorking`/`stopWorking` at v0.83.0. Kept
    /// because guests already call it; equivalent to `set_working_message(Some(label))` followed by
    /// `set_working_visible(true)`.
    pub fn working_start(&self, label: &str) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::working_start(label);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = label;
    }
    /// CYRUP-DELTA: see [`Self::working_start`]. Equivalent to `set_working_visible(false)`.
    pub fn working_stop(&self) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::working_stop();
    }

    /// Whether tool rows are expanded (Pi `getToolsExpanded()`, `types.ts:278` @v0.83.0; WIT
    /// `ui.get-tools-expanded`, `wit/world.wit:750`). `false` on the host (non-`wasm32`) target.
    pub fn tools_expanded(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ui::get_tools_expanded();
        }
        #[cfg(not(target_arch = "wasm32"))]
        false
    }
    /// Expand or collapse tool rows (Pi `setToolsExpanded(expanded)`, `types.ts:281` @v0.83.0; WIT
    /// `ui.set-tools-expanded`, `wit/world.wit:751`). Fire-and-forget; a no-op on the host
    /// (non-`wasm32`) target.
    pub fn set_tools_expanded(&self, expanded: bool) {
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::ui::set_tools_expanded(expanded);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = expanded;
    }
}
