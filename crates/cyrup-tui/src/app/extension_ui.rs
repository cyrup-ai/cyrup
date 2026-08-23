use super::*;

impl<B: Backend> App<B> {
    /// Render a loaded extension's `ui.{confirm,select,input}` dialog request in the input slot (L4
    /// review §2.1; `ui.editor` is handled synchronously by the caller, never reaching here — see
    /// [`App::run`]'s `ui_rx` arm). Mirrors Pi's `createExtensionUIContext`
    /// (`interactive-mode.ts:2060-2111`): `confirm` opens a Yes/No [`ListSelector`] exactly like Pi's
    /// confirm-as-select (`:2172-2179`); `select` opens a [`ListSelector`] over the guest's option
    /// strings; `input` opens a [`TextInputSelector`]. The dialog's `reply` one-shot is stashed on
    /// [`AppState::pending_ui_reply`]; [`App::confirm_selector`] and the selector-cancel arm of
    /// [`App::handle_selector_key`] take + resolve it when the user answers.
    ///
    /// The input slot holds at most one occupant: if a selector (first-party or extension) or a
    /// floating overlay is already open when a guest dialog arrives, it is denied immediately with its
    /// per-kind deny default (there is nowhere to render it) rather than queued or silently dropped —
    /// the guest's `ui_roundtrip` never blocks past this call regardless.
    ///
    /// `pub` (not called outside [`App::run`]'s `ui_rx` arm in production) so `tests/*.rs` can drive
    /// it directly with a synthetic [`UiRequest`] — the crate's established pattern for exercising
    /// run-loop-only logic (mirrors [`Self::open_boxed_selector`]/[`Self::active_selector_kind`]).
    pub fn open_extension_dialog(&mut self, req: UiRequest) {
        if self.state.selector.is_some() || !self.state.overlays.is_empty() {
            self.state.transcript.push_status(format!(
                "extension {:?} dialog: another dialog/selector is already open, denied",
                req.kind
            ));
            let _ = req.reply.send(default_ui_reply(req.kind));
            return;
        }
        let UiRequest { kind, prompt, options, message, placeholder, opts, reply } = req;
        let (selector_kind, base_title, mut inner): (SelectorKind, String, Box<dyn Selector>) = match kind
        {
            UiKind::Confirm => {
                // Pi's EXACT join (`showExtensionConfirm`, `interactive-mode.ts:2177`):
                // `` `${title}\n${message}` `` — a real newline, not an em-dash. The title area
                // now auto-sizes + word-wraps (`ListSelector::desired_height`/`render`,
                // `title_wrapped_height`) so a long title and/or multi-line message both render in
                // full instead of being clipped to one row (L4 review §2.6).
                let title = if message.is_empty() { prompt } else { format!("{prompt}\n{message}") };
                let rows = vec![
                    ("yes".to_string(), "Yes".to_string(), None),
                    ("no".to_string(), "No".to_string(), None),
                ];
                (
                    SelectorKind::ExtensionConfirm,
                    title.clone(),
                    Box::new(ListSelector::prompt(title, rows, 0).with_upstream_chrome(
                        SelectorKind::ExtensionConfirm,
                        &self.state.select_keymap,
                    )),
                )
            }
            UiKind::Select => {
                // L4 review §4: an empty `options` list must still OPEN the dialog (Pi's
                // `ExtensionSelectorComponent`, `extension-selector.ts:101-103`, renders whatever
                // it's given including `[]`; Enter is a no-op with nothing selected, and resolution
                // only ever happens via Esc/timeout/signal — same as any other select), not
                // short-circuit to `None` before the guest's dialog is ever shown. `cyrup`'s RPC path
                // (`rpc.rs`) already forwards `options: []` verbatim with no such short-circuit;
                // `ListSelector`/`SelectList` already render an empty list safely (`"No matches"`,
                // `current_value()` never panics) — no special-casing needed here beyond NOT
                // early-returning.
                let picked: Vec<String> = options
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let rows: Vec<(String, String, Option<String>)> =
                    picked.into_iter().map(|o| (o.clone(), o, None)).collect();
                (
                    SelectorKind::ExtensionSelect,
                    prompt.clone(),
                    Box::new(ListSelector::prompt(prompt, rows, 0).with_upstream_chrome(
                        SelectorKind::ExtensionSelect,
                        &self.state.select_keymap,
                    )),
                )
            }
            UiKind::Input => (
                SelectorKind::ExtensionInput,
                prompt.clone(),
                // E6: the hint row is built from the LIVE `tui.select.*` table, so the first paint
                // already names the user's own submit/cancel keys — upstream re-resolves `keyHint`
                // on every render (`keybinding-hints.ts:34-44`) and so never shows stock defaults.
                Box::new(
                    TextInputSelector::new(prompt, placeholder)
                        .with_keymap(&self.state.select_keymap),
                ),
            ),
            // L4 review §3: the DEFAULT is an inline dialog (Pi's `ExtensionEditorComponent`,
            // `extension-editor.ts`), not a teardown to `$EDITOR` — `title` on `prompt`, the seed
            // text (Pi `prefill`) on `message` (L4 review §2's `editor(title, initial)` fix).
            UiKind::Editor => (
                SelectorKind::ExtensionEditor,
                prompt.clone(),
                // E9: the hint row is built from the LIVE `tui.select.*` + app tables, so the first
                // paint already names the user's own keys (upstream re-resolves every `keyHint` on
                // each render, `keybinding-hints.ts:34-44`).
                Box::new(
                    ExtensionEditorSelector::new(prompt, &message)
                        .with_keymaps(&self.state.select_keymap, &self.state.keymap),
                ),
            ),
        };
        // Pi's `CountdownTimer` (`countdown-timer.ts:7-38`, wired by `ExtensionSelectorComponent`/
        // `ExtensionInputComponent`): a guest-set `opts.timeout_ms > 0` arms a live 1s-cadence
        // countdown, shown in the title from the INSTANT the dialog opens (Pi calls `onTick`
        // synchronously in its constructor, `countdown-timer.ts:19`) and ticked forward by
        // [`App::tick_extension_dialog_countdown`] — closing the gap where the dialog otherwise never
        // showed the deadline `LiveHostServices::ui_roundtrip` already enforces host-side, and stayed
        // open on screen (stale) after that host-side timeout had already resolved the guest's call.
        let opened_at = tokio::time::Instant::now();
        let deadline =
            opts.timeout_ms.filter(|&ms| ms > 0).map(|ms| opened_at + Duration::from_millis(ms));
        if let Some(deadline) = deadline {
            inner.set_title(countdown_title(&base_title, deadline, opened_at));
        }
        self.open_boxed_selector(selector_kind, inner);
        self.state.pending_ui_reply = Some(PendingUiReply { kind, reply, base_title, deadline });
    }

    /// Bind BOTH extension-UI seams of a session's host services to this run loop — the single place
    /// [`App::run`] and its session-swap arm attach the TUI, mirroring `cyrup-modes`' `run_rpc` /
    /// `rebind_session`, which install the same pair for RPC mode.
    ///
    /// The pair is not optional: [`cyrup_session_svc::UiSink`] carries the request/reply dialogs
    /// (`ui.{confirm,input,select,editor}`) and [`cyrup_session_svc::UiEffectSink`] carries the fire-and-forget mutators
    /// (`ui.{notify,set-status,set-widget,set-header,set-footer,set-title,set-editor-text,
    /// paste-editor-text,set-tools-expanded}`). `LiveHostServices` drops an effect outright when the
    /// effect sink is `None` — its headless (print/json) policy, Pi's `noOpUIContext`
    /// (`extensions/runner.ts:230-265`). Interactive is not headless in Pi: it passes a real
    /// `uiContext` (`interactive-mode.ts:2223-2268`), so installing only the dialog half made every
    /// fire-and-forget extension UI call vanish in the DEFAULT mode while working over RPC (TUI-S01).
    ///
    /// Must be re-run against every swapped-in session (`/new`, `/resume`, `/fork`, `/reload`,
    /// `/import`, or a runtime-side `SessionReplaced`): a replacement session brings a fresh
    /// `LiveHostServices` whose sinks are both `None`.
    pub fn install_ui_sinks(
        services: &cyrup_session_svc::LiveHostServices,
        ui: cyrup_session_svc::UiSink,
        effects: cyrup_session_svc::UiEffectSink,
    ) {
        services.set_ui_sink(ui);
        services.set_ui_effect_sink(effects);
    }

    /// Bind the two INTERACTIVE READ-BACK seams an extension asks the host for — the editor buffer
    /// (SEAM-T02) and the theme family (SEAM-T01).
    ///
    /// Separate from [`Self::install_ui_sinks`] for the reason [`Self::install_overlay_sink`] is:
    /// these are interactive-only in pi. `getEditorText` and all four theme members are bound only
    /// inside `createExtensionUIContext` (`interactive-mode.ts:2393`, `:2401-2417` @v0.84.2); every
    /// other mode gets `noOpUIContext`'s `""` / `[]` / `undefined` /
    /// `{success: false, error: "UI not available"}` (`core/extensions/runner.ts:253`, `:261-263`)
    /// or, for RPC, the same answers hard-coded (`modes/rpc/rpc-mode.ts:248-252`, `:290-300`).
    /// Leaving them unattached elsewhere is what reproduces that, which is why the theme switch
    /// does NOT ride the `UiEffect` sink RPC also drains.
    ///
    /// Both were dead before this call existed: `LiveHostServices` overrode neither `editor_text`
    /// nor any of `theme`/`theme_list`/`theme_by_name`/`set_theme`, so they took the trait defaults
    /// in EVERY mode — including this one, and including for WASM guests, since
    /// `cyrup-ext/src/host/live.rs` forwards `get-editor-text`, `theme-get`, `theme-get-json`,
    /// `theme-list`, `theme-get-by-name` and `theme-set` to exactly these trait methods.
    ///
    /// Must be re-run against every swapped-in session, for the reason [`Self::install_ui_sinks`]
    /// must — and additionally because [`crate::theme_access::TuiThemeAccess`] holds THAT session's
    /// resource snapshot, so a `/reload` that discovers a new theme has to rebuild it.
    pub fn install_extension_readbacks(
        &mut self,
        services: &cyrup_session_svc::LiveHostServices,
        resources: Arc<cyrup_resources::ResourceRegistry>,
        switch: crate::theme_access::ThemeSwitchSink,
    ) {
        services.attach_editor_mirror(self.state.editor_mirror.clone());
        let access = Arc::new(crate::theme_access::TuiThemeAccess::new(
            resources,
            &self.state.theme.name,
            switch,
        ));
        services.attach_theme_access(Arc::clone(&access) as Arc<dyn cyrup_session_svc::ThemeAccess>);
        self.state.theme_access = Some(access);
        // Seed both cells before the first extension can ask, rather than waiting for the first
        // frame: a boot-time `onSessionStart` handler runs before any draw.
        self.publish_extension_readbacks();
    }

    /// Republish the state behind the interactive read-back seams (SEAM-T01/T02).
    ///
    /// Called from [`Self::draw`], which is the one choke point every run-loop arm that can have
    /// changed the editor or the theme passes through — the same reasoning that puts
    /// `flush_terminal_progress` on the frame path. An extension therefore reads the buffer and the
    /// theme AS DRAWN, which is upstream's guarantee too: pi's getters read the live component the
    /// same render tree drew.
    ///
    /// The editor value is [`crate::InputEditor::expanded_text`], not `text()` — pi hands the
    /// extension `getExpandedText?.() ?? getText()` (`interactive-mode.ts:2393` @v0.84.2), i.e. with
    /// `[paste #N …]` markers substituted back to their full content.
    pub fn publish_extension_readbacks(&mut self) {
        self.state.editor_mirror.publish(self.state.editor.expanded_text());
        if let Some(access) = self.state.theme_access.as_ref() {
            access.publish_active(&self.state.theme.name);
        }
    }

    /// Bind the INTERACTIVE-OVERLAY seam — Pi's `ctx.ui.custom(factory, { overlay: true, … })`
    /// (`interactive-mode.ts:2719`, the only `showOverlay` consumer upstream has).
    ///
    /// Separate from [`Self::install_ui_sinks`] because only THIS mode can service it. A `UiSink`
    /// dialog is one question and one answer, which RPC can carry over its
    /// `extension_ui_request`/`extension_ui_response` pair; an overlay is a component the renderer
    /// must DRIVE — paint it, feed it every keystroke, repaint on its own cadence — which needs a
    /// terminal. So `cyrup-modes`' `run_rpc` deliberately installs nothing here, leaving
    /// `LiveHostServices::open_overlay` to answer `false` so the extension falls back to its own
    /// non-interactive rendering (Pi's `!ctx.hasUI` branch) instead of blocking on a modal nobody
    /// can close.
    ///
    /// Must be re-run against every swapped-in session, for exactly the reason
    /// [`Self::install_ui_sinks`] must.
    pub fn install_overlay_sink(
        services: &cyrup_session_svc::LiveHostServices,
        overlays: cyrup_session_svc::OverlaySink,
    ) {
        services.set_overlay_sink(overlays);
    }

    /// Bind the THIRD extension seam of a session — the contained-fault listener Pi's interactive
    /// mode passes as `bindExtensions({ … onError })` (`interactive-mode.ts:1700-1701`:
    /// `onError: (error) => { this.showExtensionError(error.extensionPath, error.error,
    /// error.stack); }`).
    ///
    /// A guest handler fault is CONTAINED by the dispatcher (R-08-036) — the handler is skipped
    /// (fail open) or the action is blocked (fail closed) and the host survives either way — and is
    /// then reported to every registered listener. `cyrup-modes`' `run_rpc` registers one
    /// (`rpc.rs`'s `error_listener`, emitting an `extension_error` line) and its `rebind_session`
    /// re-registers it on every swap; the interactive TUI registered NONE, so with no listener
    /// `Dispatcher::report` degraded to a `tracing::warn!` that no TUI user ever sees. A broken
    /// extension therefore silently ate its own hook — or silently DENIED a tool — in the DEFAULT
    /// mode while an RPC client on the same session saw the fault (TUI-S02).
    ///
    /// The listener is invoked SYNCHRONOUSLY from whatever worker thread the faulting dispatch ran
    /// on, so it only forwards onto an unbounded channel the run loop drains; the drain arm calls
    /// [`Self::show_extension_error`].
    ///
    /// Must be re-run against every swapped-in session for the same reason
    /// [`Self::install_ui_sinks`] must: a replacement session brings a fresh `ExtensionHost` with an
    /// empty listener list (Pi re-binds `onError` from `rebindSession` too).
    pub fn install_error_listener(
        ext_host: &cyrup_ext::ExtensionHost,
        errors: tokio::sync::mpsc::UnboundedSender<cyrup_ext::ExtensionError>,
    ) {
        ext_host.add_error_listener(std::sync::Arc::new(
            move |err: &cyrup_ext::ExtensionError| {
                let _ = errors.send(err.clone());
            },
        ));
    }

    /// The extension seam HA-1 adds: a command registered from a LIVE handler (an MCP prompt
    /// surfacing when its server connects mid-session).
    ///
    /// The tool leg needs no equivalent — a late tool raises the registry's dirty flag and
    /// `AgentSession`'s turn-boundary refresh polls it. Commands have no such poll: the RPC catalog
    /// reads `resolved_commands()` live, but the TUI `/` menu is a snapshot, so without this the
    /// command is invocable by typing it in full and absent from the menu until a session swap.
    ///
    /// Carries no payload for the same reason: the handler rebuilds from
    /// `session.slash_command_catalog()`, which is already the live truth.
    pub fn install_commands_listener(
        ext_host: &cyrup_ext::ExtensionHost,
        changed: tokio::sync::mpsc::UnboundedSender<()>,
    ) {
        ext_host.add_commands_listener(std::sync::Arc::new(move || {
            let _ = changed.send(());
        }));
    }

    /// Render one contained extension fault into the transcript — Pi `showExtensionError`
    /// (`interactive-mode.ts:2545-2560`), whose copy is
    /// `Extension "${extensionPath}" error: ${error}` in the `error` colour.
    ///
    /// Pi appends a dimmed, indented stack trace when the thrown value carried one; cyrup's
    /// [`cyrup_ext::ExtensionError`] has no `stack` field (a contained fault is an `ExtError`
    /// string, not a JS `Error` object), so only the message line is emitted.
    ///
    /// `pub` for the same reason [`Self::apply_ui_effect`] is: `tests/*.rs` drive the run loop's
    /// drain arm directly, since `App::run` needs a real terminal event source.
    pub fn show_extension_error(&mut self, err: &cyrup_ext::ExtensionError) {
        self.state
            .transcript
            .push_error(format!("Extension \"{}\" error: {}", err.extension.as_str(), err.error));
    }

    /// Apply one fire-and-forget extension UI effect — the interactive-TUI half of the
    /// [`cyrup_session_svc::UiEffectSink`] seam `cyrup-modes`' `run_rpc` already drives for RPC mode.
    ///
    /// Pi builds a real `uiContext` for interactive mode (`interactive-mode.ts:2223-2268`) whose
    /// mutators land on concrete TUI state; only headless modes get `noOpUIContext`
    /// (`extensions/runner.ts:230-265`). Cyrup installed the request/reply [`cyrup_session_svc::UiSink`] here but never
    /// the effect sink, so every `notify`/`setStatus`/`setTitle`/`setEditorText`/`pasteToEditor`/
    /// `setToolsExpanded`/`setWidget`/`setHeader`/`setFooter` call was dropped by
    /// `LiveHostServices::emit_ui_effect` in the DEFAULT mode while working over RPC.
    ///
    /// Per-variant mapping (each cites the Pi interactive handler it ports):
    /// * `Notify` → `showExtensionNotify` (`:2518-2526`): `error` → `showError`, `warning` →
    ///   `showWarning`, otherwise `showStatus`.
    /// * `SetStatus` → `setExtensionStatus` (`:1920-1923`) → the footer's extension-status line.
    /// * `SetEditorText` → `this.editor.setText(text)` (`:2241`); `is_paste` (`pasteToEditor`,
    ///   `:2240`, which wraps the text in bracketed-paste markers and re-feeds the editor) → the
    ///   editor's real paste path, so the same sanitization applies.
    /// * `SetToolsExpanded` → `setToolsExpanded` (`:3887-3903`), including its no-op early-out and
    ///   its `Tool output: expanded|collapsed` status echo.
    /// * `SetTitle` → retained on [`AppState::terminal_title`]; the crossterm run loop writes the
    ///   OSC 0 sequence (`terminal.ts:504-507`), which a `TestBackend` app must not do.
    /// * `SetWidget`/`SetHeader`/`SetFooter` → retained on [`AppState`]. These now ARRIVE (they used
    ///   to be discarded before leaving `LiveHostServices`) but cyrup's TUI has no extension chrome
    ///   slot to draw them in, so TUI-014 stays open — see those fields' docs.
    ///
    /// `pub` for the same reason [`Self::open_extension_dialog`] is: `tests/*.rs` drive it directly.
    pub fn apply_ui_effect(&mut self, effect: UiEffect) {
        match effect {
            UiEffect::Notify { message, kind } => match kind {
                NotifyKind::Error => {
                    // Pi `showError` prefixes the copy (`interactive-mode.ts:3952`).
                    self.state.transcript.push_error(format!("Error: {message}"));
                }
                NotifyKind::Warning => {
                    self.state.transcript.push_warning(format!("Warning: {message}"));
                }
                NotifyKind::Info => self.state.transcript.push_status(message),
            },
            UiEffect::SetStatus { key, text } => {
                // `text: None` clears the key — `StatusLine::set_extension_status` already treats an
                // empty value as a removal (Pi `footer.ts:233`).
                self.state.status.set_extension_status(key, text.unwrap_or_default());
            }
            UiEffect::SetEditorText { text, is_paste } => {
                if is_paste {
                    self.state.editor.handle_paste(&text);
                } else {
                    self.state.editor.set_text(&text);
                }
            }
            UiEffect::SetToolsExpanded { expanded } => self.set_tools_expanded(expanded),
            UiEffect::SetTitle { title } => self.state.terminal_title = Some(title),
            // TUI-033 — an EMPTY string is the clear. Pi's `setHeader(factory)` /
            // `setFooter(factory)` restore the built-in when the factory is `undefined`
            // (`interactive-mode.ts:2245-2254`, `:2273-2290`); cyrup's WIT signature is
            // `set-header(content: string)` (`world.wit:272`), which has no `undefined`, so the
            // empty string is the only value that can carry "restore the built-in".
            UiEffect::SetHeader { content } => {
                self.state.extension_header = (!content.is_empty()).then_some(content)
            }
            UiEffect::SetFooter { content } => {
                self.state.extension_footer = (!content.is_empty()).then_some(content)
            }
            UiEffect::SetWidget { widget } => {
                // Pi keys widgets and UPDATES IN PLACE: `removeExisting(this.extensionWidgetsAbove);
                // removeExisting(this.extensionWidgetsBelow);` then `targetMap.set(key, component)`
                // (`interactive-mode.ts:1926-1958`), and a widget whose `content` is `undefined` is
                // removed rather than re-mounted (`:1935-1938`). TUI-014.
                let parsed = ExtensionWidget::from_json(&widget);
                self.state.extension_widgets.retain(|w| w.key != parsed.key);
                if !parsed.lines.is_empty() {
                    self.state.extension_widgets.push(parsed);
                }
            }
            // TUI-030 — the working-indicator family. All four used to be unreachable: the
            // `HostServices` methods had no `UiEffect` to push, so `LiveHostServices` kept the
            // trait's empty defaults and an extension calling any of them changed nothing, in
            // silence. Pi binds every one to real interactive state (`createExtensionUIContext`,
            // `interactive-mode.ts:2377-2385` @v0.84.2 — every pi line in this arm and in
            // `reset_extension_ui` is that tag, not this file's older @v0.83.0 cites). The state
            // itself lives on [`crate::status_indicator::StatusIndicator`] and [`TranscriptView`],
            // whose setters carry the per-verb citations and the branch logic.
            UiEffect::SetWorkingMessage { message } => {
                self.state.indicator.set_working_message(message)
            }
            UiEffect::SetWorkingVisible { visible } => {
                // `this.session.isStreaming` (`interactive-mode.ts:2098`) — cyrup mirrors it on the
                // status line, set by the `AgentStart`/`AgentEnd` arms.
                let streaming = self.state.status.streaming;
                self.state.indicator.set_working_visible(visible, streaming);
            }
            UiEffect::SetWorkingIndicator { options } => {
                self.state
                    .indicator
                    .set_working_indicator(options.as_ref().map(WorkingIndicator::from_json));
            }
            UiEffect::SetHiddenThinkingLabel { label } => {
                self.state.transcript.set_hidden_thinking_label(label)
            }
        }
    }

    /// Advance the open extension-UI dialog's countdown by one tick (Pi's `CountdownTimer`'s 1s
    /// `setInterval`, `countdown-timer.ts:21-30`): live-updates the selector's title with the
    /// remaining seconds, or — once the deadline has passed — auto-resolves the dialog to its
    /// per-kind deny default and closes the slot (Pi's `onExpire` → `onCancelCallback`,
    /// `extension-selector.ts:56`/`extension-input.ts:59`), exactly like an `Esc` cancel
    /// ([`App::handle_selector_key`]'s `Cancel` arm). A stale reply send (the host's OWN independent
    /// `ui_roundtrip` timeout already won the race) is a harmless no-op, same as every other reply
    /// site in this module. A no-op when no extension dialog is open or it has no timeout armed —
    /// callers gate the driving interval on this same condition so it costs nothing otherwise.
    ///
    /// `pub` for the same reason as [`Self::open_extension_dialog`]: `tests/*.rs` calls it directly
    /// to simulate the run loop's 1s tick without needing a real `tokio::time::sleep`.
    pub fn tick_extension_dialog_countdown(&mut self) {
        self.tick_extension_dialog_countdown_at(tokio::time::Instant::now());
    }

    /// [`Self::tick_extension_dialog_countdown`] with an INJECTED instant — TUI-N09.
    ///
    /// `tests/extension_dialog_countdown.rs` used to `std::thread::sleep(1_100ms)` and then assert
    /// the literal `"Proceed? (2s)"` against a 3 s budget, i.e. a wall-clock-exact assertion with
    /// ~900 ms of scheduler slack in a suite of thousands of tests. A CI or loaded-laptop stall past
    /// 900 ms turned it red with a message pointing at the countdown logic rather than at the
    /// scheduler. Pi drives the same countdown from an injected timer rather than a wall-clock sleep
    /// (`components/countdown-timer.ts:21-30`), and this crate already has the pattern in
    /// `StatusIndicator::retry_message`, which recomputes from a stored `Instant`.
    pub fn tick_extension_dialog_countdown_at(&mut self, now: tokio::time::Instant) {
        let Some((base_title, deadline)) = self
            .state
            .pending_ui_reply
            .as_ref()
            .and_then(|p| p.deadline.map(|d| (p.base_title.clone(), d)))
        else {
            return;
        };
        if now >= deadline {
            if let Some(pending) = self.state.pending_ui_reply.take() {
                let _ = pending.reply.send(default_ui_reply(pending.kind));
            }
            self.close_selector(true);
        } else if let Some(active) = self.state.selector.as_mut() {
            active.inner.set_title(countdown_title(&base_title, deadline, now));
        }
    }

    /// Re-bind the UI to a freshly-installed runtime session (arch-11 §3.4 replacement; Pi's
    /// interactive session-swap). Called by the run loop on a generation bump (a `/new`/`/resume`/
    /// `/fork`/`/reload`/`/import` op or a runtime-side `SessionReplaced`): the run loop has already
    /// dropped the stale subscription and re-subscribed the new session's `AgentSessionEvent` stream;
    /// here we reset the per-session UI state (the transcript, the streaming/indicator status, any
    /// open selector/overlay) for the new session and surface the swap status line. Committed
    /// scrollback already lives in the terminal's native history (`insert_before`) and is preserved.
    /// Tear down every EXTENSION-owned UI surface before the old session is invalidated — pi
    /// `resetExtensionUI` (`interactive-mode.ts:1974-2003`), registered on the runtime via
    /// `setBeforeSessionInvalidate` (`:452`).
    ///
    /// The ordering is the point, and it is why this cannot be folded into
    /// [`Self::rebind_session`]. pi's runtime fires `abort → session_shutdown →
    /// beforeSessionInvalidate → dispose` (`agent-session-runtime.ts:167-177`), so this runs while
    /// the OLD session is still alive and its extension host still answerable. `rebind_session`
    /// runs AFTER the swap and resets session-owned surfaces (transcript, selector, overlays,
    /// status flags); an extension header, footer, widget, status row or shortcut binding left
    /// behind by the outgoing session's extensions would otherwise survive into the new one and
    /// keep rendering, owned by a host that no longer exists.
    pub fn reset_extension_ui(&mut self) {
        self.state.extension_header = None;
        self.state.extension_footer = None;
        self.state.extension_widgets.clear();
        self.state.extension_shortcuts.clear();
        self.state.status.extension_statuses.clear();
        // An extension dialog/editor overlay belongs to the outgoing host; leaving it up would
        // present a prompt whose reply channel is about to be dropped.
        self.state.overlays.clear();
        self.state.selector = None;
        // TUI-030 — the working-indicator family, which upstream resets in the SAME function:
        // `this.workingMessage = undefined; this.workingVisible = true; this.setWorkingIndicator();`
        // then `this.setHiddenThinkingLabel()` (`interactive-mode.ts:2210-2218` @v0.84.2). Without
        // this, an extension that hid the working band — or renamed it — would leave the NEXT
        // session with a band owned by a host that no longer exists: the same class of leak the
        // header/footer/widget clears above fix.
        //
        // Upstream's one extra step is the live band's copy: when the band is currently the working
        // one it is re-messaged to `"${defaultWorkingMessage} (${keyText("app.interrupt")} to
        // interrupt)"` (`:2213-2217`) — the ONLY place upstream ever suffixes a working message, and
        // it says "to interrupt", not the "to cancel" the other three kinds bake in.
        let interrupt = self.state.keymap.keys_label(Action::Interrupt);
        self.state.indicator.reset_extension_working_state(interrupt.as_deref());
        self.state.transcript.set_hidden_thinking_label(None);
    }
}
