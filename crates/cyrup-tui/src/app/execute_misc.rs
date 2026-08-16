use super::*;

impl<B: Backend> App<B> {
    /// The remaining [`Self::execute_command`] arms (§7.1): login, model/thinking cycling,
    /// settings writes, and the small session conveniences. Arm bodies moved verbatim.
    pub(crate) async fn execute_misc_command(&mut self, cmd: AppCommand, session: &Arc<AgentSession>) {
        use AppCommand as C;
        match cmd {
            C::LoginCommand(arg) => self.handle_login_command(session, arg).await,

            C::ModelCommand(search) => {
                // `/model [text]` (`handleModelCommand`, interactive-mode.ts:4175-4196): exact match
                // sets directly; a partial (or bare) opens the picker pre-filtered to `search`.
                self.handle_model_command(session, search).await;
            }

            C::CycleModel(direction) => {
                // The cycle set is the scoped models when a scope is active, else the full available
                // catalog (Pi `cycleModel`: `scopedModels.length > 0 ? scoped : available`). Cycling by
                // model id mirrors the `/model` confirm path (`set_model(&value)` above); the footer
                // re-reads the new model off the `ModelChanged` event this triggers.
                let scoped = session.scoped_models();
                let scoped_active = !scoped.is_empty();
                let cycle: Vec<(String, String, String)> = if scoped_active {
                    scoped
                        .iter()
                        .map(|sm| {
                            (sm.model.id.to_string(), sm.model.provider.to_string(), sm.model.name.clone())
                        })
                        .collect()
                } else {
                    session
                        .available_model_catalog()
                        .iter()
                        .map(|m| (m.id.to_string(), m.provider.to_string(), m.name.clone()))
                        .collect()
                };
                if cycle.len() <= 1 {
                    let msg =
                        if scoped_active { "Only one model in scope" } else { "Only one model available" };
                    self.state.transcript.push_status(msg);
                } else {
                    let current = session.model();
                    let n = cycle.len();
                    // `session.model()` is `Option` (pi `AgentSession.model: Model | undefined`,
                    // agent-session.ts:866-868): a modelless session matches nothing, so the cycle
                    // starts at the head exactly as pi's `findIndex === -1 ⇒ 0` does
                    // (agent-session.ts:1650-1653).
                    let cur = cycle.iter().position(|(id, prov, _)| {
                        current.as_ref().is_some_and(|c| {
                            id == c.model.as_str() && prov == c.provider.as_str()
                        })
                    });
                    let next = match direction {
                        CycleDirection::Forward => cur.map_or(0, |i| (i + 1) % n),
                        CycleDirection::Backward => cur.map_or(0, |i| (i + n - 1) % n),
                    };
                    if let Some((id, _prov, name)) = cycle.get(next) {
                        match session.set_model(id).await {
                            Ok(_) => self.state.transcript.push_status(format!("Switched to {name}")),
                            Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
                        }
                    }
                }
            }

            C::CycleThinking => {
                // Pi `cycleThinkingLevel` (interactive-mode.ts:3606-3614): call the session's cycle,
                // which is gated on `supportsThinking()` (agent-session.ts:1599-1608) and walks the
                // model's OWN supported levels (`getSupportedThinkingLevels`, incl. `xhigh` where
                // mapped). `Ok(None)` ⇒ the model does not reason: show the exact Pi status and change
                // NOTHING. `Ok(Some(level))` ⇒ `set_thinking_level` already emitted
                // `ThinkingLevelChanged`, so the footer + editor rule re-color off that event
                // (mirroring Pi's `footer.invalidate()` + `updateEditorBorderColor()`); here we only
                // surface Pi's `Thinking level: {level}` status line.
                match session.cycle_thinking_level().await {
                    Ok(None) => {
                        self.state.transcript.push_status("Current model does not support thinking");
                    }
                    Ok(Some(level)) => {
                        let label = match level {
                            ModelThinkingLevel::Off => "off",
                            ModelThinkingLevel::Minimal => "minimal",
                            ModelThinkingLevel::Low => "low",
                            ModelThinkingLevel::Medium => "medium",
                            ModelThinkingLevel::High => "high",
                            ModelThinkingLevel::Xhigh => "xhigh",
                            ModelThinkingLevel::Max => "max",
                        };
                        self.state.transcript.push_status(format!("Thinking level: {label}"));
                    }
                    Err(e) => self.state.transcript.push_status(format!("thinking error: {e}")),
                }
            }
            // TUI-032 — the `/settings` → `Thinking level` submenu's confirm. Pi's
            // `onThinkingLevelChange` (`interactive-mode.ts:4222-4226`) calls
            // `session.setThinkingLevel(level)`, which clamps to the model's capabilities and emits
            // `ThinkingLevelChanged`; the footer + editor rule re-color off that event exactly as
            // they do for Shift+Tab.

            C::SetThinking(level) => {
                let parsed = match level.as_str() {
                    "off" => Some(ModelThinkingLevel::Off),
                    "minimal" => Some(ModelThinkingLevel::Minimal),
                    "low" => Some(ModelThinkingLevel::Low),
                    "medium" => Some(ModelThinkingLevel::Medium),
                    "high" => Some(ModelThinkingLevel::High),
                    "xhigh" => Some(ModelThinkingLevel::Xhigh),
                    "max" => Some(ModelThinkingLevel::Max),
                    _ => None,
                };
                match parsed {
                    Some(l) => match session.set_thinking_level(l).await {
                        Ok(applied) => {
                            let label = match applied {
                                ModelThinkingLevel::Off => "off",
                                ModelThinkingLevel::Minimal => "minimal",
                                ModelThinkingLevel::Low => "low",
                                ModelThinkingLevel::Medium => "medium",
                                ModelThinkingLevel::High => "high",
                                ModelThinkingLevel::Xhigh => "xhigh",
                                ModelThinkingLevel::Max => "max",
                            };
                            self.state.transcript.push_status(format!("Thinking level: {label}"));
                        }
                        Err(e) => {
                            self.state.transcript.push_status(format!("thinking error: {e}"))
                        }
                    },
                    None => self
                        .state
                        .transcript
                        .push_status(format!("thinking error: unknown level {level}")),
                }
            }

            C::ApplySetting { id, value } => {
                // Persist a `/settings` toggle/choice live (Global scope; Pi's settings selector
                // writes the global layer). The `/reload` re-reads the effective view.
                let json = parse_setting_value(&value);
                // `outputPad` also takes effect ON SCREEN immediately (Pi `onOutputPadChange` →
                // `this.outputPad = padding` + re-render, interactive-mode.ts:4127-4136), unlike the
                // settings that only rebind on `/reload`: push the new pad into the live transcript so
                // the chat horizontal padding changes the moment the row is cycled.
                if id == "outputPad" {
                    let pad = if value == "0" { 0 } else { 1 };
                    self.state.transcript.set_output_pad(pad);
                }
                // `hideThinkingBlock` likewise takes effect live (Pi `setHideThinkingBlock`,
                // assistant-message.ts:57-62) — on the in-flight reasoning block and on every entry
                // committed after the flip. Pi additionally re-renders the ALREADY-shown assistant
                // messages; cyrup's committed rows have left the render tree for native scrollback
                // (`flush_committed` → `insert_before`), so history keeps the form it committed with.
                if id == "hideThinkingBlock" {
                    self.state.transcript.set_hide_thinking_block(value == "true");
                }
                // The image rows are live too (Pi re-reads them per `ToolExecutionComponent`).
                if id == "terminal.showImages" {
                    self.state.show_images = value == "true";
                    self.state.transcript.set_show_images(self.state.show_images);
                }
                // `terminal.showTerminalProgress` is live in Pi by construction — its gate is
                // `getShowTerminalProgress()` re-read at every call site, so a flip takes effect on
                // the next transition with no handler doing anything but persisting
                // (`onShowTerminalProgressChange`, `interactive-mode.ts:4311-4313`). cyrup caches
                // the gate on `AppState`, so the flip has to be pushed into it here or the row would
                // not take effect until the next session bind. Turning the row OFF while an
                // indicator is lit also parks a clear — see the `[CYRUP-DELTA]` on
                // `TerminalProgress::set_enabled`.
                if id == "terminal.showTerminalProgress" {
                    self.state.terminal_progress.set_enabled(value == "true");
                }
                if id == "terminal.imageWidthCells"
                    && let Ok(cells) = value.parse::<u16>()
                {
                    self.state.transcript.set_image_width_cells(cells);
                }
                // `editorPaddingX` is live in Pi too — `onEditorPaddingChange` writes the setting and
                // then calls `setPaddingX` on the live editor (`settings-selector.ts:687-689` →
                // `interactive-mode.ts:5393-5399`), so the rules re-inset on the very next frame.
                if id == "editorPaddingX"
                    && let Ok(pad) = value.parse::<i64>()
                {
                    self.state.editor.set_padding_x(pad);
                }
                // Same for `showHardwareCursor` (Pi `onShowHardwareCursorChange` →
                // `ui.setShowHardwareCursor(enabled)`, `tui.ts:346-352`, which hides the cursor
                // immediately when turned off rather than waiting for a rebind).
                if id == "showHardwareCursor" {
                    self.state.editor.set_show_hardware_cursor(value == "true");
                }
                // TUI-041 — `terminal.clearOnShrink` was in neither the live-apply list nor the
                // grid's resolved read, so it did not take effect until the next launch. Pi's
                // `onClearOnShrinkChange` calls `this.ui.setClearOnShrink(clearOnShrink)`
                // immediately (`interactive-mode.ts`, and `handleReloadCommand` re-applies it at
                // `:5401-5405`). cyrup's counterpart is the reserved idle status band.
                if id == "terminal.clearOnShrink" {
                    self.state.reserve_status_rows = value == "true";
                }
                // TUI-009 — the Escape chain reads the cached copy, so the row has to push into it
                // or a flip would not take effect until the next session bind. Pi re-reads
                // `getDoubleEscapeAction()` inside `onEscape` itself (`:2580`), which is the same
                // liveness.
                if id == "doubleEscapeAction" {
                    self.state.double_escape_action = value.clone();
                }
                // TUI-032 — same reason: the submenu is rebuilt from the cache each time it opens.
                if id == "warnings.anthropicExtraUsage" {
                    self.state.warn_anthropic_extra_usage = value == "true";
                }
                // `enableSkillCommands` gates the `skill:<name>` half of the `/` menu
                // (`interactive-mode.ts:613`); Pi rebuilds the autocomplete provider on the change,
                // so rebuild the registry from the SAME catalog with the new gate.
                if id == "enableSkillCommands" {
                    self.state
                        .editor
                        .set_registry(crate::commands::CommandRegistry::with_dynamic(
                            crate::commands::dynamic_commands_from_catalog_gated(
                                &session.slash_command_catalog(),
                                value == "true",
                            ),
                        ));
                }
                // `transport` is live in Pi too, and it is the ONLY row whose live half touches the
                // agent rather than the UI: `onTransportChange` persists the setting AND assigns
                // `this.session.agent.transport = transport` (`interactive-mode.ts:4213-4216`), so
                // the very next request streams with the chosen transport. cyrup persisted only,
                // which left `AgentBuilder::transport`'s build-time seed in force until restart.
                if id == "transport" {
                    session.set_transport(&value).await;
                }
                match session.persist_setting(cyrup_session_svc::SettingsScope::Global, &id, json) {
                    Ok(()) => self.state.transcript.push_status(format!("{id} → {value}")),
                    Err(e) => self.state.transcript.push_status(format!("settings error: {e}")),
                }
            }
            // TUI-055 — SPAWNED when a run loop is servicing `compact_tx`. `session.compact` is a
            // 10–20 s provider call; awaiting it here froze the loop for its whole duration, so the
            // `compaction_start` event that arms `IndicatorKind::Compaction` was never read and the
            // 80 ms spinner arm never fired — the screen was simply blank. Pi keeps its band on
            // screen for the entire operation (`interactive-mode.ts:3075-3087`); this is what lets
            // cyrup's reach the frame. The outcome comes back over the channel and is applied by
            // [`Self::apply_compact_outcome`], which the inline fallback below also uses.

            C::SetName(name) => match session.set_session_name(&name).await {
                Ok(()) => {
                    let stored = session.session_name().await;
                    if stored.as_deref() != Some(name.as_str()) {
                        self.state.transcript.push_warning(format!(
                            "Session name was normalized from {} to {}",
                            serde_json::to_string(&name).unwrap_or_else(|_| format!("{name:?}")),
                            match &stored {
                                Some(s) => serde_json::to_string(s)
                                    .unwrap_or_else(|_| format!("{s:?}")),
                                None => "null".to_string(),
                            },
                        ));
                    }
                    let shown = stored.unwrap_or(name);
                    self.state.transcript.push_status(format!("Session name set: {shown}"));
                }
                Err(e) => self.state.transcript.push_status(format!("name error: {e}")),
            },
            // TUI-080 / TUI-084 — the getter, and pi's severity CHANNEL for the usage line: a
            // `showWarning` (`interactive-mode.ts:5638`), not a neutral status, and pi's exact
            // string `Usage: /name <name>` rather than cyrup's `usage: /name <session name>`.

            C::ShowName => match session.session_name().await {
                Some(name) => {
                    self.state.transcript.push_status(format!("Session name: {name}"))
                }
                None => self.state.transcript.push_warning("Usage: /name <name>"),
            },

            C::Copy => match session.last_assistant_text().await {
                Some(text) => {
                    let n = text.chars().count();
                    // Pi's `handleCopyCommand` (interactive-mode.ts:6002-6019) wraps the write in a
                    // `try`: success shows a status, a THROW shows `showError(...)`. Reporting
                    // "copied" unconditionally is what let the old `#[cfg(not(unix))]` no-op tell a
                    // Windows user their message was on the clipboard when nothing had been written.
                    if crate::clipboard::copy_to_clipboard(&text).await {
                        self.state
                            .transcript
                            .push_status(format!("copied last message ({n} chars)"));
                    } else {
                        // The message Pi throws when every branch failed (`clipboard.ts:171-173`),
                        // surfaced through the same error channel as its `showError`.
                        self.state.transcript.push_error("Failed to copy to clipboard");
                    }
                }
                None => self.state.transcript.push_status("no assistant message to copy"),
            },

            C::Share => self.share_session(session).await,

            // unreachable by construction: the dispatcher covers every variant before here.
            _ => {}
        }
    }

    /// `/share` (`handleShareCommand`, interactive-mode.ts:5191): export the session to HTML, write a
    /// temp file, then shell `gh gist create --public=false <file>` behind a [`BorderedLoader`] and
    /// surface the resulting gist URL. `gh` missing / logged-out / failing degrades to a status line
    /// (Pi's `showError` paths). Fully in-crate (the HTML body is rendered by [`crate::export`]).
    async fn share_session(&mut self, session: &Arc<AgentSession>) {
        use tokio::process::Command;
        // Render the session HTML over its own JSONL (the same body `/export` writes).
        let html = match session.export_to_jsonl(None).await {
            Ok(Some(jsonl)) => crate::export::session_jsonl_to_html(&jsonl),
            Ok(None) => {
                self.state.transcript.push_status("nothing to share (empty session)");
                return;
            }
            Err(e) => {
                self.state.transcript.push_status(format!("share export error: {e}"));
                return;
            }
        };
        let tmp = std::env::temp_dir().join(format!("cyrup-session-{}.html", session.session_id()));
        if let Err(e) = std::fs::write(&tmp, html.as_bytes()) {
            self.state.transcript.push_status(format!("share write error: {e}"));
            return;
        }
        // Show the bordered loader in the editor slot while gh runs (Pi's `BorderedLoader`).
        // `keyHint("tui.select.cancel", "cancel")` (`bordered-loader.ts:36`) — the SELECT-tier
        // action, and `keyText` joins every bound key with `/` (`keybinding-hints.ts:29-36`), so the
        // stock hint reads `escape/ctrl+c cancel`. This used `Keymap::key_label(Action::Interrupt)`,
        // a different action resolved to its FIRST key only, which both named the wrong binding and
        // silently hid the second key the user can actually press.
        self.state.loader = Some(crate::chrome::BorderedLoader::cancellable(
            "Creating gist...",
            self.state
                .select_keymap
                .keys_label(SelectAction::Cancel)
                .unwrap_or_else(|| "escape/ctrl+c".into()),
        ));
        let result = Command::new("gh")
            .args(["gist", "create", "--public=false"])
            .arg(&tmp)
            .output()
            .await;
        self.state.loader = None;
        let _ = std::fs::remove_file(&tmp);
        match result {
            Ok(out) if out.status.success() => {
                // TUI-063 — pi does NOT surface the raw gist URL on its own. It peels the gist ID
                // off the URL `gh` printed and renders the VIEWER link built from it:
                //
                // ```ts
                // const gistUrl = result.stdout?.trim();
                // const gistId = gistUrl?.split("/").pop();                 // :5599
                // if (!gistId) { this.showError("Failed to parse gist ID from gh output"); return; }
                // const previewUrl = getShareViewerUrl(gistId);             // :5606
                // this.showStatus(`Share URL: ${previewUrl}\nGist: ${gistUrl}`);
                // ```
                //
                // (`interactive-mode.ts:5597-5608` @v0.83.0.) cyrup printed only the gist URL, so
                // [`share_viewer_url`] — and therefore `CYRUP_SHARE_VIEWER_URL`, which
                // `cyrup --help` advertises as "Base URL for /share command" — had no consumer at
                // all: setting it changed nothing and said nothing.
                let gist_url = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let gist_id = gist_id_from_url(&gist_url);
                if gist_id.is_empty() {
                    // pi `showError` (`:5601`) — cyrup's `"gist created (no URL returned by gh)"`
                    // covered only the empty-stdout half of the same failure. `showError` builds
                    // `Error: ${errorMessage}` INSIDE itself (`interactive-mode.ts:3878-3882`)
                    // while cyrup's `Entry::Error` renders verbatim, so the prefix is the caller's
                    // to supply here (TUI-062's shape, same as `Warning: `).
                    self.state
                        .transcript
                        .push_error("Error: Failed to parse gist ID from gh output");
                } else {
                    let preview = share_viewer_url(gist_id);
                    self.state
                        .transcript
                        .push_status(format!("Share URL: {preview}\nGist: {gist_url}"));
                }
            }
            Ok(out) => {
                let err = String::from_utf8_lossy(&out.stderr);
                let msg = err.trim();
                let detail = if msg.is_empty() { "gh gist create failed" } else { msg };
                self.state.transcript.push_status(format!("share error: {detail}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self
                .state
                .transcript
                .push_status("GitHub CLI (gh) is not installed — see https://cli.github.com/"),
            Err(e) => self.state.transcript.push_status(format!("share error: {e}")),
        }
    }

}
