use super::*;

impl<B: Backend> App<B> {
    /// The session-lifecycle half of [`Self::execute_command`] (§7.1): `NewSession`/`Reload`/
    /// `Import`/`Compact`/`Clone` plus the session-file and session-IO arms (`DeleteSession`,
    /// `RenameSession`, `Export`, `SessionInfo`). Arm bodies moved verbatim.
    pub(crate) async fn execute_session_command(
        &mut self,
        cmd: AppCommand,
        session: &Arc<AgentSession>,
        runtime: Option<&Arc<AgentSessionRuntime>>,
    ) {
        use AppCommand as C;
        match cmd {
            C::DeleteSession(path) => {
                // `/resume` in-list delete (`onDeleteSession`): remove the persisted JSONL via the
                // additive `delete_session_file` seam (refuses the active session).
                // SEAM-063 — the seam now routes through the OS `trash` CLI first and reports
                // WHICH happened, so the status line is pi's own:
                // `result.method === "trash" ? "Session moved to trash" : "Session deleted"`
                // (`modes/interactive/components/session-selector.ts:846` @v0.83.0) and
                // `Failed to delete: ${error}` (`:849`). It used to say "deleted session" whether
                // or not the file went.
                match session.delete_session_file(std::path::Path::new(&path)) {
                    Ok(method) => self
                        .state
                        .transcript
                        .push_status(method.status_message().to_string()),
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("Failed to delete: {e}")),
                }
            }

            C::RenameSession { path, name } => {
                // `/resume` in-list rename (`onRenameSession`): persist a `session_info` name on the
                // target file via the additive `rename_session_file` seam.
                match session
                    .rename_session_file(std::path::Path::new(&path), &name)
                    .await
                {
                    Ok(()) => self
                        .state
                        .transcript
                        .push_status(format!("renamed session → {name}")),
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("rename error: {e}")),
                }
            }

            C::Compact(arg) => match self.compact_tx.clone() {
                Some(tx) => {
                    let session = session.clone();
                    tokio::spawn(async move {
                        let outcome = session.compact(arg).await.map_err(|e| e.to_string());
                        let _ = tx.send(outcome);
                    });
                }
                None => {
                    let outcome = session.compact(arg).await.map_err(|e| e.to_string());
                    self.apply_compact_outcome(outcome);
                }
            },

            C::Clone => match session.clone_at(None).await {
                Ok(id) => self
                    .state
                    .transcript
                    .push_status(format!("cloned session → {id}")),
                Err(e) => self
                    .state
                    .transcript
                    .push_status(format!("clone error: {e}")),
            },

            C::Export(arg) => {
                // Format chosen **by extension**, matching Pi (`handleExportCommand`,
                // interactive-mode.ts:5106-5112): a `.jsonl` target writes the raw transcript;
                // every other target (including no path) writes a styled HTML document — HTML is the
                // default. cyrup renders the document at the L5 seam
                // (`cyrup_session_svc::session_jsonl_to_html_with_theme`) over the session's own
                // JSONL, carrying the ACTIVE theme's palette as pi's `generateHtml` does
                // (`core/export-html/index.ts:151-157` @v0.84.4).
                let is_jsonl = arg
                    .as_deref()
                    .is_some_and(|p| p.trim_end().to_ascii_lowercase().ends_with(".jsonl"));
                if is_jsonl {
                    let path = arg.as_deref().map(std::path::Path::new);
                    match session.export_to_jsonl(path).await {
                        Ok(_) => {
                            // TUI-082 — one status string for BOTH branches, pi's:
                            // `Session exported to: ${filePath}` (`interactive-mode.ts:5440`
                            // jsonl / `:5443` html @v0.83.0). The two branches used to disagree
                            // with each other as well as with upstream.
                            let label = arg.unwrap_or_default();
                            self.state
                                .transcript
                                .push_status(format!("Session exported to: {label}"));
                        }
                        Err(e) => self
                            .state
                            .transcript
                            .push_status(format!("export error: {e}")),
                    }
                } else {
                    // Pull the transcript as JSONL (no path ⇒ returned as text), render to HTML, write.
                    match session.export_to_jsonl(None).await {
                        Ok(Some(jsonl)) => {
                            let html = cyrup_session_svc::session_jsonl_to_html_with_theme(
                                &jsonl,
                                &session.export_theme(),
                                &session.export_state().await,
                            );
                            // TUI-082 — bare `/export` WRITES A FILE. It used to `push_block` the
                            // raw HTML into the transcript, so the single most likely invocation
                            // produced no artifact and flooded scrollback with markup the user
                            // could not do anything with. There is no upstream branch that
                            // corresponds to it: pi always writes and always reports a path
                            // (`handleExportCommand`, `interactive-mode.ts:5434-5447` @v0.83.0).
                            //
                            // The default name is pi's LITERAL mechanism, and it is NOT what
                            // `agent-session.ts:3213`'s doc comment says ("defaults to session
                            // directory") — the code is `exportSessionToHtml`
                            // (`core/export-html/index.ts:274-278` @v0.83.0):
                            //   `outputPath = `${APP_NAME}-session-${basename(sessionFile,".jsonl")}.html``
                            // i.e. a RELATIVE name resolved against the process cwd, not a path
                            // under the session directory. Ported as written, with `cyrup` for
                            // `APP_NAME` (TUI-083 — cyrup has no config-name override).
                            let target = match &arg {
                                Some(path) => Some(std::path::PathBuf::from(path)),
                                None => session.session_file().await.map(|f| {
                                    let stem = f
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| "session".into());
                                    std::path::PathBuf::from(format!("cyrup-session-{stem}.html"))
                                }),
                            };
                            match target {
                                Some(path) => match std::fs::write(&path, &html) {
                                    Ok(()) => self.state.transcript.push_status(format!(
                                        "Session exported to: {}",
                                        path.display()
                                    )),
                                    Err(e) => self
                                        .state
                                        .transcript
                                        .push_status(format!("export error: {e}")),
                                },
                                // pi throws "Cannot export in-memory session to HTML"
                                // (`export-html/index.ts:243-245`) when there is no session file;
                                // an unpersisted session has no basename to build the name from.
                                None => self.state.transcript.push_status(
                                    "export error: cannot export an in-memory session to HTML",
                                ),
                            }
                        }
                        Ok(None) => self.state.transcript.push_status("exported session"),
                        Err(e) => self
                            .state
                            .transcript
                            .push_status(format!("export error: {e}")),
                    }
                }
            }
            // `handleNameCommand`'s SETTER half (`interactive-mode.ts:5645-5653` @v0.83.0).
            // TUI-080: the echo is the STORED name, re-read after the write, not the input — the
            // store normalizes, and echoing the input told the user a name was set that `/resume`
            // would not show. When the two differ upstream warns first (`:5648-5650`), verbatim
            // including the JSON quoting of both values.
            C::SessionInfo => {
                // Pi's `/session` renderer (`handleSessionCommand`, interactive-mode.ts:5656-5717
                // @v0.83.0) reads exactly these fields off `getSessionStats()`; cyrup renders them
                // as its own markdown table.
                let stats = session.session_stats().await;
                // PROV-036 / PROV-035 — the two things pi computes here that cyrup did not:
                // `getUsageCostBreakdown(entries)` (`:5665`) and
                // `computeCacheWaste(entries, this.session.modelRuntime)` (`:5660`).
                let breakdown = session.usage_cost_breakdown().await;
                let cache_waste = session.cache_waste().await;
                let mut body = format!(
                    "| Field | Value |\n|-------|-------|\n\
                     | file | {} |\n| id | {} |\n\
                     | messages | {} |\n| user | {} |\n| assistant | {} |\n\
                     | tool calls | {} |\n| tool results | {} |\n\
                     | input tokens | {} |\n| output tokens | {} |\n\
                     | cache read | {} |\n| cache write | {} |\n| total tokens | {} |\n\
                     | cost | ${:.3} |\n",
                    stats.session_file.as_deref().unwrap_or("In-memory"),
                    stats.session_id,
                    stats.total_messages,
                    stats.user_messages,
                    stats.assistant_messages,
                    stats.tool_calls,
                    stats.tool_results,
                    stats.tokens.input,
                    stats.tokens.output,
                    stats.tokens.cache_read,
                    stats.tokens.cache_write,
                    stats.tokens.total,
                    stats.cost,
                );
                // `if (stats.cost > 0 || cacheWaste.missedTokens > 0) { … }` (`:5696`). Both
                // additions live under pi's one guard, so a zero-cost session gains no rows.
                if stats.cost > 0.0 || cache_waste.missed_tokens > 0 {
                    // `if (usageBreakdown.length > 1)` (`:5699`) — a single-model session shows the
                    // total only, because a one-row breakdown restates it.
                    if breakdown.len() > 1 {
                        for entry in &breakdown {
                            body.push_str(&format!(
                                "| {} | ${:.3} ({} tokens) |\n",
                                entry.key, entry.cost, entry.tokens
                            ));
                        }
                    }
                    // `if (cacheWaste.missedTokens > 0)` (`:5704-5711`): the `$` figure only when
                    // `missedCost >= 0.0001`, else tokens + miss count alone. The label and the
                    // singular/plural of "miss" are pi's strings verbatim.
                    if cache_waste.missed_tokens > 0 {
                        let miss_label = if cache_waste.miss_count == 1 {
                            "1 miss".to_string()
                        } else {
                            format!("{} misses", cache_waste.miss_count)
                        };
                        let detail = format!("{} tokens, {miss_label}", cache_waste.missed_tokens);
                        body.push_str(&if cache_waste.missed_cost >= 0.0001 {
                            format!(
                                "| Cache Re-billed | ${:.3} ({detail}) |\n",
                                cache_waste.missed_cost
                            )
                        } else {
                            format!("| Cache Re-billed | {detail} |\n")
                        });
                    }
                }
                self.state.transcript.push_block("Session", body);
            }
            // Session-lifecycle ops drive the `AgentSessionRuntime` (arch-11 §3.4): the op rebuilds
            // the active session + bumps the generation, and the run loop's generation-watch arm
            // re-binds the UI (re-subscribe + reset transcript) → `pending_swap_status`. Without a
            // runtime (SDK/embedder), surface the request so the path is real (no silent drop).
            C::NewSession => match runtime {
                // `/new` (handleClearCommand): start a fresh session in the same cwd (Pi `newSession`).
                //
                // TUI-092 §5b.2 — SPAWNED: `new_session` dispatches `HostEvent::SessionShutdown`
                // then `SessionStart` to every live extension; see the `/fork` arm for the
                // self-deadlock awaiting it on this task would reintroduce.
                Some(rt) => {
                    self.state.pending_swap_status =
                        Some(SwapCaption::Receipt("\u{2713} New session started".into()));
                    let rt = Arc::clone(rt);
                    self.dispatch_lifecycle(async move {
                        LifecycleOutcome(match rt.new_session().await {
                            Ok(r) if r.cancelled => Err("new session cancelled".to_string()),
                            Ok(_) => Ok(LifecycleEffects::default()),
                            Err(e) => Err(format!("new session error: {e}")),
                        })
                    })
                    .await;
                }
                None => self.state.transcript.push_status("starting new session…"),
            },

            C::Reload => match runtime {
                // `/reload` (handleReloadCommand): rebuild the active session in place (Pi `reload`,
                // agent-session.ts:2451) — re-reads settings/resources/keybindings, resets the
                // provider, preserves the persisted transcript.
                // TUI-092 §5b.2 — SPAWNED: `reload` dispatches `HostEvent::SessionShutdown` then
                // `SessionStart` to every live extension; see the `/fork` arm for the self-deadlock
                // awaiting it on this task would reintroduce. The keybinding rebuild is `&mut self`
                // and stays on the loop, carried back as an effect so TUI-051's ordering — session
                // reload FIRST, then `this.keybindings.reload()` — is preserved exactly.
                Some(rt) => {
                    let agent_dir = session.services().agent_dir.clone();
                    // TUI-037 — pi's `maybeSaveImplicitProjectTrustAfterReload()`
                    // (`interactive-mode.ts:5995` @v0.84.4): persist a trust that was granted
                    // implicitly at boot once the project has grown trust-requiring resources.
                    // Run BEFORE the rebuild is dispatched, not after it as pi does — cyrup's
                    // reload re-decides trust from the store (see `app/reload_trust.rs`'s
                    // ordering note), so the write has to land where the rebuilt session reads it.
                    let (saved_implicit_project_trust, trust_warning) =
                        match self.maybe_save_implicit_project_trust(session).await {
                            Ok(saved) => (saved, None),
                            // pi's `catch` (`:4938-4940`): the reload proceeds, the status keeps
                            // its plain variant, and `showWarning` frames the message
                            // (`Warning: …`, `:4264-4266`). Surfaced post-swap via the effect.
                            Err(e) => (
                                false,
                                Some(format!(
                                    "Warning: Could not save project trust after reload: {e}"
                                )),
                            ),
                        };
                    // TUI-025 — Pi's own sentence, `interactive-mode.ts:5418-5423` @v0.83.0.
                    // cyrup's `"reloaded resources"` said nothing about WHAT was reloaded, and the
                    // `/` menu's own help string for the command was a second, different wording.
                    // The `; saved project trust` variant is `interactive-mode.ts:6000-6003`
                    // @v0.84.4, selected by the boolean above (TUI-037).
                    self.state.pending_swap_status = Some(SwapCaption::Status(
                        if saved_implicit_project_trust {
                            "Reloaded keybindings, extensions, skills, prompts, themes, and \
                             context files; saved project trust"
                        } else {
                            "Reloaded keybindings, extensions, skills, prompts, themes, and \
                             context files"
                        }
                        .into(),
                    ));
                    let rt = Arc::clone(rt);
                    self.dispatch_lifecycle(async move {
                        LifecycleOutcome(match rt.reload(None).await {
                            Ok(()) => Ok(LifecycleEffects {
                                reload_keybindings_in: Some(agent_dir),
                                warning: trust_warning,
                                ..LifecycleEffects::default()
                            }),
                            Err(e) => Err(format!("reload error: {e}")),
                        })
                    })
                    .await;
                }
                None => self.state.transcript.push_status("reloading resources…"),
            },

            C::Import(p) => match (runtime, p) {
                // `/import <path>` (handleImportCommand): copy + resume a JSONL session (Pi
                // `importFromJsonl`, agent-session-runtime.ts:353).
                //
                // TUI-081 — pi asks FIRST: `await this.showExtensionConfirm("Import session",
                // `Replace current session with ${inputPath}?`)` and shows `Import cancelled` on a
                // decline (`interactive-mode.ts:6069-6072` @v0.84.4). The live session is only
                // replaced from the confirm's `Yes` arm ([`Self::execute_session_switch`]); this arm
                // opens the prompt and parks the path on `pending_import`.
                (Some(_), Some(path)) => self.open_import_confirm(path),
                // TUI-084 — pi's string and pi's CHANNEL: `Usage: /import <path.jsonl>` through
                // `showError` (`interactive-mode.ts:5482` @v0.83.0). cyrup dropped the `.jsonl`
                // constraint, lowercased the word, and routed a real error to the neutral status
                // line, where it is neither coloured nor prefixed as a problem.
                (Some(_), None) => self
                    .state
                    .transcript
                    .push_error("Usage: /import <path.jsonl>"),
                (None, p) => self
                    .state
                    .transcript
                    .push_status(format!("importing session {}", p.unwrap_or_default())),
            },

            // NOT unreachable by construction — nothing enforced that. `execute_command` picks the
            // lifecycle bucket; a variant it names there with no arm above lands here. Report it
            // rather than swallowing it — cc19b87 was exactly this, in `execute_misc_command`.
            other => {
                debug_assert!(
                    false,
                    "unrouted command in execute_session_command: {other:?}"
                );
                self.state
                    .transcript
                    .push_error(format!("internal: unrouted command {other:?}"));
            }
        }
    }

    /// The `Session`/`UserMessage` `ConfirmSelection` switch+fork paths (TUI-092 §5b.2) —
    /// session-lifecycle ops, dispatched here from [`Self::execute_selector_command`]. Arm bodies
    /// moved verbatim.
    pub(crate) async fn execute_session_switch(
        &mut self,
        cmd: AppCommand,
        session: &Arc<AgentSession>,
        runtime: Option<&Arc<AgentSessionRuntime>>,
    ) {
        use AppCommand as C;
        match cmd {
            C::ConfirmSelection {
                kind: SelectorKind::ImportConfirm,
                value,
            } => {
                // TUI-081 — the answer to "Replace current session with {path}?"
                // (`handleImportCommand`, `interactive-mode.ts:6069-6082` @v0.84.4). Only `Yes`
                // reaches `importFromJsonl`; `No` is `showStatus("Import cancelled")` (`:6071`).
                // The stashed path is taken either way so it cannot outlive this answer.
                let Some(pending) = self.state.pending_import.take() else {
                    return;
                };
                if value != CONFIRM_YES {
                    self.state.transcript.push_status("Import cancelled");
                    return;
                }
                let Some(rt) = runtime else {
                    // The prompt only opens with a runtime; a swap-less embedder cannot get here,
                    // but degrade to the same status the bare `/import` path shows without one.
                    self.state
                        .transcript
                        .push_status(format!("importing session {}", pending.path));
                    return;
                };
                self.dispatch_import(rt, pending.path).await;
            }

            C::ConfirmSelection {
                kind: SelectorKind::UserMessage,
                value,
            } => {
                // `/fork` (user-message-selector.ts): fork at the chosen entry. With the runtime
                // threaded in, drive `AgentSessionRuntime::fork` so the runtime swaps to the new
                // branched session and the UI re-binds on the generation bump (Pi `fork`,
                // agent-session-runtime.ts:259); `position:"before"` re-seeds the editor with the
                // anchor text. Without a runtime (SDK/embedder), fall back to the in-place
                // `fork_at_entry` (no swap).
                let entry = cyrup_core::EntryId::from(value.as_str());
                match runtime {
                    // TUI-092 §5b.2 — SPAWNED: `fork` dispatches `HostEvent::SessionBeforeFork` to
                    // every live extension, and a guest hook that opens a `ui.*` dialog parks its
                    // task until this loop answers `ui_rx`. Awaited here that is a self-deadlock.
                    Some(rt) => {
                        self.state.pending_swap_status =
                            Some(SwapCaption::Status("forked from message".into()));
                        let rt = Arc::clone(rt);
                        self.dispatch_lifecycle(async move {
                            LifecycleOutcome(match rt.fork(entry, ForkPosition::Before).await {
                                Ok(r) if r.cancelled => Err("fork cancelled".to_string()),
                                Ok(r) => Ok(LifecycleEffects {
                                    selected_text: r.selected_text,
                                    ..LifecycleEffects::default()
                                }),
                                Err(e) => Err(format!("fork error: {e}")),
                            })
                        })
                        .await;
                    }
                    None => match session.fork_at_entry(&entry, ForkPosition::Before).await {
                        Ok(_) => self.state.transcript.push_status("forked from message"),
                        Err(e) => self
                            .state
                            .transcript
                            .push_status(format!("fork error: {e}")),
                    },
                }
            }

            C::ConfirmSelection {
                kind: SelectorKind::Session,
                value,
            } => {
                // `/resume` swap (handleResumeSession, interactive-mode.ts): switch the runtime to the
                // chosen session file (Pi `switchSession`, agent-session-runtime.ts:193). The runtime
                // asserts the resumed cwd still exists, rebuilds cwd-bound services, and bumps the
                // generation; the UI re-binds on the bump. Without a runtime, surface the path.
                match runtime {
                    // TUI-092 §5b.2 — SPAWNED: `switch_session` dispatches
                    // `HostEvent::SessionBeforeSwitch` to every live extension; see the `/fork` arm
                    // above for the deadlock awaiting it here would reintroduce.
                    Some(rt) => {
                        self.state.pending_swap_status =
                            Some(SwapCaption::Status(format!("resumed {value}")));
                        let rt = Arc::clone(rt);
                        self.dispatch_lifecycle(async move {
                            LifecycleOutcome(match rt.switch_session(value).await {
                                Ok(r) if r.cancelled => Err("resume cancelled".to_string()),
                                Ok(_) => Ok(LifecycleEffects::default()),
                                Err(e) => Err(format!("resume error: {e}")),
                            })
                        })
                        .await;
                    }
                    None => self
                        .state
                        .transcript
                        .push_status(format!("resume {value} (/reload to switch)")),
                }
            }

            // NOT unreachable by construction, and this one's invariant is not even local: what
            // reaches here is decided by the caller's pattern in `execute_selector_command`
            // (`execute.rs:589-591`, `ConfirmSelection` of kind `Session` or `UserMessage`). Widen
            // that arm without adding an arm here and the command vanishes silently — cc19b87's
            // shape, one level deeper. Report it instead.
            other => {
                debug_assert!(
                    false,
                    "unrouted command in execute_session_switch: {other:?}"
                );
                self.state
                    .transcript
                    .push_error(format!("internal: unrouted command {other:?}"));
            }
        }
    }

    /// The `/resume` session-selector open (the front half of the session-switch lifecycle path —
    /// its confirmation is [`Self::execute_session_switch`]). Arm body moved verbatim from
    /// [`Self::execute_command`]'s `OpenSelector(SelectorKind::Session)` arm.
    pub(crate) async fn execute_open_session_selector(&mut self, session: &Arc<AgentSession>) {
        // `/resume` (session-selector.ts): the persisted-session list for this cwd, newest
        // first, sourced via the additive `list_sessions` seam. Confirming carries the chosen
        // session file path; the actual runtime swap is driven by the L7 `SessionRuntime`
        // (`switch_session`) once the runtime is threaded into the run loop (residual gap #3).
        let sessions = session.list_sessions();
        if sessions.is_empty() {
            self.state
                .transcript
                .push_status("no saved sessions to resume");
        } else {
            let current = session.session_id().to_string();
            let rows: Vec<SessionRow> = sessions
                .iter()
                .map(|s| {
                    let label = session_label(s);
                    let is_current = s.id.to_string() == current;
                    let desc = format!(
                        "{} msgs{}",
                        s.message_count,
                        if is_current { " (current)" } else { "" }
                    );
                    // The query-DSL search text (`getSessionSearchText`,
                    // session-selector-search.ts:26): `{id} {name} {allMessagesText} {cwd}`.
                    let search_text = format!(
                        "{} {} {} {}",
                        s.id,
                        s.name.as_deref().unwrap_or(""),
                        s.all_messages_text,
                        s.cwd
                    );
                    SessionRow {
                        path: s.path.display().to_string(),
                        label,
                        name: s.name.clone(),
                        desc: Some(desc),
                        search_text,
                        recency: system_time_nanos(s.modified),
                    }
                })
                .collect();
            // `new SessionSelectorComponent(..., { keybindings }, currentSessionFilePath)`
            // (`interactive-mode.ts:4867-4884`): the picker is handed the live keybindings
            // AND the running session's file path, and each `SessionInfo` carries its
            // `parentSessionPath` (`session-manager.ts` → `session-selector.ts:222`).
            // Without those three the threaded view has no edges to draw, the row you are
            // sitting in is not accented, and the hint rows name stock keys.
            let mut selector = SessionSelector::new(rows)
                .with_keymaps(&self.state.session_keymap, self.state.editor.keymap_ref());
            selector.set_parent_paths(sessions.iter().filter_map(|s| {
                s.parent_session_path
                    .as_ref()
                    .map(|p| (s.path.display().to_string(), p.display().to_string()))
            }));
            // `options?.showRenameHint ?? this.canRename` (`session-selector.ts:772`):
            // upstream's host declares the capability by passing a `renameSession`
            // callback. cyrup's is the `SessionSelectorOutcome::Rename` arm below, which
            // lands in `session.rename_session_file` — so the capability is present and the
            // hint is on. Stated here rather than defaulted in the component, because the
            // component cannot know whether its host wired the apply path.
            selector.set_show_rename_hint(true);
            // `currentSessionFilePath` — resolved from the listing rather than the manager
            // so it is the SAME string the rows carry (a canonicalization mismatch would
            // silently never match).
            selector.set_current_session_path(
                sessions
                    .iter()
                    .find(|s| s.id.to_string() == current)
                    .map(|s| s.path.display().to_string()),
            );
            let inner: Box<dyn Selector> = Box::new(selector);
            self.open_boxed_selector(SelectorKind::Session, inner);
        }
    }
}

impl<B: Backend> App<B> {
    /// TUI-081 — open pi's `/import` guard: `showExtensionConfirm("Import session", `Replace
    /// current session with ${inputPath}?`)` (`interactive-mode.ts:6069` @v0.84.4), i.e.
    /// `showExtensionSelector(`${title}\n${message}`, ["Yes", "No"])` (`:2557-2565`) — a Yes/No
    /// [`ListSelector`] with `Yes` highlighted, under the `ExtensionSelectorComponent` chrome the
    /// extension-driven confirm uses. The path is parked on [`AppState::pending_import`] until the
    /// prompt is answered; the answer arrives as `ConfirmSelection { kind: ImportConfirm, .. }`
    /// (`Enter`) or as the selector-cancel arm (`Escape`).
    pub(crate) fn open_import_confirm(&mut self, path: String) {
        let title = format!(
            "{}\nReplace current session with {path}?",
            SelectorKind::ImportConfirm.title()
        );
        self.state.pending_import = Some(PendingImport { path });
        let rows = vec![
            (CONFIRM_YES.to_string(), "Yes".to_string(), None),
            ("no".to_string(), "No".to_string(), None),
        ];
        self.open_boxed_selector(
            SelectorKind::ImportConfirm,
            Box::new(
                ListSelector::prompt(title, rows, 0)
                    .with_upstream_chrome(SelectorKind::ImportConfirm, &self.state.select_keymap),
            ),
        );
    }

    /// TUI-081 — the decline half of the `/import` guard: drop the parked path and show pi's
    /// `Import cancelled` (`interactive-mode.ts:6071` @v0.84.4). Reached from the confirm's `No`
    /// row and from Escape on the prompt.
    pub(crate) fn cancel_pending_import(&mut self) {
        self.state.pending_import = None;
        self.state.transcript.push_status("Import cancelled");
    }

    /// Run the confirmed `/import <path>` through the runtime (Pi `importFromJsonl`,
    /// `agent-session-runtime.ts:353`). The swap caption is pi's `Session imported from:
    /// ${inputPath}` (`interactive-mode.ts:6082` @v0.84.4); an extension veto of the switch
    /// (`result.cancelled`) is pi's second `Import cancelled` (`:6079`).
    ///
    /// TUI-092 §5b.2 — SPAWNED: `import_from_jsonl` dispatches `HostEvent::SessionStart` to every
    /// live extension; see the `/fork` arm for the self-deadlock awaiting it on this task would
    /// reintroduce.
    async fn dispatch_import(&mut self, rt: &Arc<AgentSessionRuntime>, path: String) {
        self.state.pending_swap_status = Some(SwapCaption::Status(format!(
            "Session imported from: {path}"
        )));
        let rt = Arc::clone(rt);
        self.dispatch_lifecycle(async move {
            LifecycleOutcome(match rt.import_from_jsonl(path, None).await {
                Ok(r) if r.cancelled => Err("Import cancelled".to_string()),
                Ok(_) => Ok(LifecycleEffects::default()),
                Err(e) => Err(format!("import error: {e}")),
            })
        })
        .await;
    }
}
