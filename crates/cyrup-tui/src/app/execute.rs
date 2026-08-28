use super::*;

impl<B: Backend> App<B> {
    /// Execute a session/data-bound [`AppCommand`] against the live [`AgentSession`]
    /// (`setupEditorSubmitHandler` command handlers, interactive-mode.ts:2554-2734). Data-bound
    /// selectors source their rows here (spec/tui/05 §8 late-data population) and open via
    /// [`open_data_selector`](Self::open_data_selector); lifecycle/IO commands call the matching
    /// session method and surface a status line / info block. Errors degrade to a status line.
    ///
    /// This is still CALLED inline from `App::run`'s `select!` loop, but it no longer AWAITS
    /// guest-reentrant work there: every arm that drives a session-lifecycle op
    /// (`Reload`/`NewSession`/`Import`/the `Session`/`UserMessage` `ConfirmSelection` switch+fork
    /// paths/`Compact`) now runs that `.await` off-task and applies its `self.state` mutation from a
    /// channel-back arm — [`Self::dispatch_lifecycle`] and [`Self::lifecycle_tx`], which is the
    /// restructuring TUI-092 §5b.2 prescribed and the L4 review §2.1 residual deferred. The hazard
    /// it closed is real and worth keeping in view: those ops dispatch
    /// `HostEvent::Session{Start,Shutdown,BeforeSwitch,BeforeFork,Compact}` to every live
    /// extension's hook (`session.rs` `dispatch_notify`/`vetoed`), a guest SDK hook handler is
    /// handed the SAME `Ctx` a tool/shortcut handler gets (`cyrup-ext-sdk/src/ctx/`), and a
    /// `ctx.ui().*` call from inside one parks its task in `block_in_place` until the run loop
    /// answers `ui_rx` — which the run loop could not do while awaiting the op that was waiting for
    /// it. Any NEW arm added here that awaits a runtime or session-lifecycle op must go through
    /// [`Self::dispatch_lifecycle`] for the same reason.
    pub async fn execute_command(
        &mut self,
        cmd: AppCommand,
        session: &Arc<AgentSession>,
        runtime: Option<&Arc<AgentSessionRuntime>>,
    ) {
        use AppCommand as C;
        match cmd {
            // selector family — arms stay in this file
            C::OpenSelector(_) | C::ConfirmSelection { .. } | C::SetEntryLabel { .. } => {
                self.execute_selector_command(cmd, session, runtime).await
            }
            // session lifecycle — app/execute_session.rs
            C::NewSession | C::Reload | C::Import(_) | C::Compact(_) | C::Clone
            | C::DeleteSession(_) | C::RenameSession { .. } | C::Export(_) | C::SessionInfo => {
                self.execute_session_command(cmd, session, runtime).await
            }
            // app/execute_misc.rs — LISTED, not `_`, so a new variant is a compile error until it
            // is deliberately bucketed. This alone does not prevent the cc19b87 defect: that was a
            // variant routed to a dispatcher with no arm for it, which still compiles. The loud
            // catch-alls in the sub-dispatchers are what catch that.
            C::ApplySetting { .. }
            | C::ConfirmSelectionAsDefault { .. }
            | C::Copy
            | C::CycleModel(_)
            | C::CycleThinking
            | C::LoginCommand(_)
            | C::ModelCommand(_)
            | C::SetModelThinkingLevel { .. }
            | C::SetName(_)
            | C::SetThinking(_)
            | C::Share
            | C::ShowName
            | C::ThinkingCommand(_) => self.execute_misc_command(cmd, session).await,
        }
    }

    /// The selector-family half of [`Self::execute_command`] (§7.1): `OpenSelector`,
    /// `ConfirmSelection` and `SetEntryLabel` arms, moved verbatim. The `Session`/`UserMessage`
    /// confirmations are session-lifecycle switch+fork paths (TUI-092 §5b.2) and delegate to
    /// [`Self::execute_session_switch`].
    async fn execute_selector_command(
        &mut self,
        cmd: AppCommand,
        session: &Arc<AgentSession>,
        runtime: Option<&Arc<AgentSessionRuntime>>,
    ) {
        use AppCommand as C;
        match cmd {
            C::OpenSelector(SelectorKind::Model) => {
                // The bare `app.model.select` entry point (no search term) — same as `/model` with no
                // argument: the unfiltered picker.
                self.handle_model_command(session, None).await;
            }

            C::OpenSelector(SelectorKind::ScopedModels) => {
                // The scoped-models picker is the bespoke checkbox+reorder selector over the FULL
                // catalog (`scoped-models-selector.ts`): the catalog is every available model; the
                // current scope is `scoped_models()` (empty ⇒ "all enabled", Pi's `enabledIds = null`).
                let catalog: Vec<(String, String, String, Option<String>)> = session
                    .model_catalog()
                    .iter()
                    .map(|m| {
                        (m.id.to_string(), m.name.clone(), m.provider.to_string(), Some(m.provider.to_string()))
                    })
                    .collect();
                if catalog.is_empty() {
                    self.state.transcript.push_status("no models available (configure providers)");
                } else {
                    let scoped: Vec<String> =
                        session.scoped_models().into_iter().map(|sm| sm.model.id.to_string()).collect();
                    // Empty scope ⇒ "all enabled" (None); otherwise the explicit ordered set.
                    let enabled = if scoped.is_empty() { None } else { Some(scoped) };
                    self.open_checkbox_selector(catalog, enabled);
                }
            }

            C::OpenSelector(SelectorKind::UserMessage) => {
                // S22: the real `UserMessageSelectorComponent` (`user-message-selector.ts`) —
                // three lines per entry (message / `Message i of N` / blank) under a header that
                // sits ABOVE the top rule. The `Some(format!("message {}", i + 1))` description
                // this used to build is gone: the metadata line is the component's own, and its
                // text is `  Message ${position} of ${this.messages.length}` (`:66`).
                let rows: Vec<crate::UserMessageRow> = session
                    .user_messages_for_forking()
                    .await
                    .into_iter()
                    .map(|a| crate::UserMessageRow {
                        id: a.entry_id.to_string(),
                        text: a.text.clone(),
                    })
                    .collect();
                if rows.is_empty() {
                    self.state.transcript.push_status("no user messages to fork from");
                } else {
                    // `initialSelectedId` is unset here, so the constructor preselects the most
                    // recent message (`:24-26`) — the same row the old `last` index picked.
                    let selector = crate::UserMessageSelector::new(rows, None);
                    self.open_boxed_selector(SelectorKind::UserMessage, Box::new(selector));
                }
            }

            C::OpenSelector(SelectorKind::Tree) => {
                // `/tree` (tree-selector.ts): the real session DAG flattened via the new
                // `AgentSession::session_dag` getter (feature #2) — nodes with parent/depth/label/kind/
                // fold/leaf/label/timestamp — feeding the connector/fold/filter engine in
                // `tree_selector.rs`. This replaces the flat user-message spine the audit flagged as
                // "data-starved" so the selector renders the actual branch tree.
                let dag = session.session_dag().await;
                if dag.is_empty() {
                    self.state.transcript.push_status("no session history to navigate");
                } else {
                    let nodes: Vec<TreeNode> =
                        dag.iter().map(tree_node_from_dag).collect();
                    let mut tree = TreeSelector::new(nodes);
                    tree.set_keymap(self.state.tree_keymap.clone());
                    // `treeFilterMode` — the filter `/tree` OPENS with (Pi reads
                    // `settingsManager.getTreeFilterMode()` into `initialFilterMode` at
                    // `interactive-mode.ts:4644` and hands it to `TreeSelectorComponent`, which seeds
                    // `this.filterMode` at `tree-selector.ts:137`). Read per open, not cached, so a
                    // `/settings` change takes effect on the next `/tree` exactly as it does in Pi.
                    tree.set_filter(crate::tree_selector::FilterMode::from_setting(
                        &session.services().settings.effective().tree_filter_mode(),
                    ));
                    // Pi re-shows the tree AT THE SAME ENTRY after an escaped summarize prompt or an
                    // aborted summarization (`showTreeSelector(entryId)`,
                    // `interactive-mode.ts:4763,4807`); both paths park the id here.
                    if let Some(pending) = self.state.pending_tree_nav.take() {
                        tree.select_id(&pending.target);
                    }
                    self.open_boxed_selector(SelectorKind::Tree, Box::new(tree));
                }
            }

            C::OpenSelector(SelectorKind::Login) => {
                // `showOAuthSelector("login")` → `showLoginAuthTypeSelector()`
                // (`interactive-mode.ts:5127-5130`), i.e. exactly a bare `/login`.
                self.handle_login_command(session, None).await;
            }

            C::OpenSelector(SelectorKind::Logout) => {
                // `showOAuthSelector("logout")` (`interactive-mode.ts:5132-5175`) →
                // `getLogoutProviderOptions()`: only providers with a STORED credential are listed,
                // each carrying its credential's `authType` (which picks the confirm message).
                let inputs = self.login_provider_inputs(session).await;
                let stored =
                    match cyrup_config::login::stored_credentials(&session.services().auth).await {
                        Ok(stored) => stored,
                        Err(e) => {
                            self.state.transcript.push_status(format!("logout error: {e}"));
                            return;
                        }
                    };
                let options = cyrup_config::login::logout_provider_options(&stored, &inputs);
                if options.is_empty() {
                    // Pi's verbatim copy (`interactive-mode.ts:5136-5138`).
                    self.state
                        .transcript
                        .push_status(cyrup_config::login::NO_STORED_CREDENTIALS);
                    return;
                }
                // S5/S21 — same component as `/login`, in `logout` mode (`:52`), which changes the
                // title (`:72`) and the empty-catalog copy (`:155-158`).
                let selector =
                    crate::OAuthSelector::new(crate::OAuthMode::Logout, &options, None);
                self.state.logout_options = options;
                self.open_boxed_selector(SelectorKind::Logout, Box::new(selector));
            }

            C::OpenSelector(SelectorKind::Settings) => {
                // `/settings` (settings-selector.ts): the curated toggle/choice grid sourced from the
                // live effective settings. Each row cycles in place on `Enter` and persists via
                // `ApplySetting` (Pi's settings selector applies on `onChange`).
                // Seed the persisted default so the `Thinking level` submenu can badge it
                // ` · default` and offer `Ctrl+S` — `handle_selector_key` (which dispatches
                // `OpenSubmenu`) has no session, and `/settings` is the only route to that submenu.
                // Pi's `?? DEFAULT_THINKING_LEVEL` fallback (`interactive-mode.ts:4814`) is what
                // keeps a badge on screen when the setting is unset.
                // `ModelThinkingLevel` is `serde(rename_all = "camelCase")`
                // (`cyrup-core/src/message/thinking.rs:25-26`), so its wire form is exactly the
                // `off`/`minimal`/…/`max` vocabulary the picker rows use — no separate mapping.
                self.state.default_thinking_level = session
                    .services()
                    .settings
                    .effective()
                    .default_thinking_level()
                    .and_then(|l| serde_json::to_value(l).ok())
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_else(|| "medium".to_string());
                let rows = settings_rows(
                    session.services().settings.effective(),
                    &self.state.theme.name,
                    &self.state.keymap,
                    &self.state.thinking_level,
                    // TUI-036 — `supportsImages` gates the two image rows upstream.
                    self.state.image_renderer.is_graphical(),
                    // TUI-041 — the PROCESS env, the same surface the runtime resolves against.
                    &cyrup_session_svc::EnvVars::from_process(),
                );
                let inner: Box<dyn Selector> = Box::new(SettingsSelector::new("Settings", rows));
                self.open_boxed_selector(SelectorKind::Settings, inner);
            }

            C::OpenSelector(SelectorKind::Trust) => {
                // `/trust` (trust-selector.ts): the yes/parent/no option list under a cwd + saved-
                // decision header. Confirming writes the trust store (`write_project_trust`).
                let options = session.project_trust_options();
                let cwd = session.services().cwd.display().to_string();
                let saved = session.saved_trust_decision().await;
                let saved_label = format_saved_trust(&saved);
                // Pi `isSavedOption` (`trust-selector.ts:92-98`): the option whose trust flag AND
                // saved path both match the persisted decision. `selectedIndex` falls back to 0
                // when there is none (`Math.max(0, findIndex(...))`, `:45-48`), but the ` ✓`
                // marker is driven by the predicate itself (`:109-110`), so keep both (S20).
                let saved_index = options.iter().position(|o| {
                    saved.as_ref().is_some_and(|s| {
                        s.decision.is_trusted() == o.trusted
                            && o.saved_path.as_deref() == Some(s.path.as_path())
                    })
                });
                let selected = saved_index.unwrap_or(0);
                let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();
                let inner: Box<dyn Selector> = Box::new(
                    TrustSelector::new(
                        cwd,
                        saved_label,
                        session.services().project_trusted,
                        labels,
                        selected,
                    )
                    .with_saved_index(saved_index)
                    // `keyHint("tui.select.confirm", "save")` / `…cancel` (`trust-selector.ts:
                    // 78-82`) read `getKeybindings()`, so the row must be built from the app's
                    // merged table — `handle` only adopts it once a key has already been pressed,
                    // which is one paint too late.
                    .with_hints(&self.state.select_keymap),
                );
                self.open_boxed_selector(SelectorKind::Trust, inner);
            }

            C::OpenSelector(SelectorKind::Session) => {
                self.execute_open_session_selector(session).await
            }
            C::OpenSelector(SelectorKind::ModelThinking) => {
                // GAP 3 step 1 rows: every model, each described by its CURRENT override (blank
                // when unset), sorted current-model → persisted-default → provider, which is Pi's
                // comparator (`settings-selector.ts:587-597`).
                let eff = session.services().settings.effective();
                let overrides = eff.all_model_thinking_levels();
                let default_key = match (eff.default_provider(), eff.default_model()) {
                    (Some(p), Some(m)) => Some(format!("{p}/{m}")),
                    _ => None,
                };
                let current_key = session
                    .model()
                    .map(|c| format!("{}/{}", c.provider.as_str(), c.model.as_str()));
                let mut entries: Vec<(String, String, Option<String>)> = session
                    .available_model_catalog()
                    .iter()
                    .map(|m| {
                        let key = format!("{}/{}", m.provider, m.id);
                        // `label: modelItemLabel(model)` = `` `${id} [${provider}]` ``
                        // (`settings-selector.ts:190-192`); `description: override ?? undefined`.
                        let label = format!("{} [{}]", m.id, m.provider);
                        let desc = overrides
                            .get(&key)
                            .map(|l| crate::app::thinking_level_str(*l).to_string());
                        (key, label, desc)
                    })
                    .collect();
                entries.sort_by(|a, b| {
                    let rank = |k: &String| {
                        if current_key.as_ref() == Some(k) {
                            0
                        } else if default_key.as_ref() == Some(k) {
                            1
                        } else {
                            2
                        }
                    };
                    rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0))
                });
                if entries.is_empty() {
                    self.state.transcript.push_status("no models available");
                    return;
                }
                self.open_data_selector(SelectorKind::ModelThinking, entries, 0);
            }

            C::OpenSelector(SelectorKind::ModelThinkingLevel) => {
                // GAP 3 step 2 rows: the level ladder, plus a `(clear override)` row that exists
                // ONLY when this model already has one (`settings-selector.ts:634-640`).
                let Some(model_key) = self.state.pending_model_thinking.clone() else {
                    return;
                };
                let eff = session.services().settings.effective();
                let existing = eff.all_model_thinking_levels().get(&model_key).copied();
                const LEVELS: [(&str, &str); 7] = [
                    ("off", "No reasoning"),
                    ("minimal", "Very brief reasoning (~1k tokens)"),
                    ("low", "Light reasoning (~2k tokens)"),
                    ("medium", "Moderate reasoning (~8k tokens)"),
                    ("high", "Deep reasoning (~16k tokens)"),
                    ("xhigh", "Extra-high reasoning (~32k tokens)"),
                    ("max", "Maximum reasoning"),
                ];
                let mut rows: Vec<(String, String, Option<String>)> = LEVELS
                    .iter()
                    .map(|(l, d)| ((*l).to_string(), (*l).to_string(), Some((*d).to_string())))
                    .collect();
                if existing.is_some() {
                    let global = eff
                        .default_thinking_level()
                        .map_or("medium", crate::app::thinking_level_str);
                    rows.push((
                        crate::app::CLEAR_MODEL_THINKING.to_string(),
                        "(clear override)".to_string(),
                        Some(format!("Revert to global default ({global})")),
                    ));
                }
                // `preselect: (ctx) => currentModelThinkingLevels[ctx.model]` (`:643`).
                let selected = existing
                    .and_then(|l| {
                        let s = crate::app::thinking_level_str(l);
                        rows.iter().position(|(v, _, _)| v == s)
                    })
                    .unwrap_or(0);
                self.open_data_selector(SelectorKind::ModelThinkingLevel, rows, selected);
            }

            C::OpenSelector(other) => {
                // Any remaining kind has no in-crate sourcing yet; surface the request (no silent drop).
                self.state.transcript.push_status(format!("{} selector unavailable", other.title()));
            }

            C::ConfirmSelection { kind: SelectorKind::Tree, value } => {
                // Confirming a tree row ASKS ABOUT SUMMARIZATION FIRST (Pi
                // `interactive-mode.ts:4744-4779`), then navigates. Before SESS-023 this arm called
                // `navigate_tree(.., NavigateTreeOptions::default())` — `summarize` hard-false — so
                // the entire branch-summary stack was unreachable from the shipped binary and
                // `branchSummary.skipPrompt` was a no-op.
                //
                // Pi's `getBranchSummarySkipPrompt()` gate (`:4753`) is a FRONT-END decision: when
                // set, skip the prompt entirely and navigate with `wantsSummary = false`.
                if session.services().settings.effective().branch_summary_skip_prompt() {
                    self.begin_tree_navigation(session, value, false, None).await;
                } else {
                    self.state.pending_tree_nav = Some(PendingTreeNav { target: value });
                    self.open_branch_summary_prompt();
                }
            }

            C::ConfirmSelection { kind: SelectorKind::BranchSummary, value } => {
                // The three-option answer (Pi `:4755-4777`). `custom` opens the instructions editor
                // and keeps the pending target; the other two dispatch the navigation directly.
                // `wantsSummary = summaryChoice !== "No summary"` (`:4767`).
                if value == BRANCH_SUMMARY_CUSTOM {
                    self.open_branch_summary_instructions();
                } else {
                    let Some(pending) = self.state.pending_tree_nav.take() else { return };
                    let summarize = value != BRANCH_SUMMARY_NONE;
                    self.begin_tree_navigation(session, pending.target, summarize, None).await;
                }
            }

            C::ConfirmSelection { kind: SelectorKind::BranchSummaryInstructions, value } => {
                // Pi `showExtensionEditor` returned a string (`:4769`): a complete choice, so the
                // prompt loop breaks and the navigation runs with `summarize: true`. An EMPTY string
                // is still a value (only `undefined`/Escape loops back), so it is forwarded as
                // `None` custom instructions rather than an empty override.
                let Some(pending) = self.state.pending_tree_nav.take() else { return };
                let instructions = (!value.trim().is_empty()).then_some(value);
                self.begin_tree_navigation(session, pending.target, true, instructions).await;
            }

            C::ConfirmSelection { kind: SelectorKind::Model, value } => {
                match session.set_model(&value).await {
                    Ok(_) => self.state.transcript.push_status(format!("model → {value}")),
                    Err(e) => self.state.transcript.push_status(format!("model error: {e}")),
                }
            }

            C::SetEntryLabel { entry_id, label } => {
                // Persist the `/tree` label edit via the SAME live `set_label` path a loaded
                // extension's `setLabel` uses (`LiveHostServices::set_label` → `manager.append_label`,
                // host_services.rs:866). An empty label removes it (`apply_label` drops empty labels),
                // matching Pi's `value || undefined`. Silently degrades (unknown id / busy), like Pi.
                // `set_label` takes pi's optional label (`setLabel(entryId, label?)`): an EMPTY
                // edit clears it, which is pi's `value || undefined`.
                session.services().host_services.set_label(
                    &entry_id,
                    (!label.is_empty()).then_some(label.as_str()),
                );
                let msg = if label.is_empty() {
                    "label removed".to_string()
                } else {
                    format!("label → {label}")
                };
                self.state.transcript.push_status(msg);
            }

            C::ConfirmSelection { kind: SelectorKind::ScopedModels, value } => {
                // The checkbox selector confirms with the ordered enabled ids (`\n`-joined), or the
                // `SCOPED_MODELS_ALL` sentinel for "all enabled". Rebuild the scoped set from the
                // catalog and persist via `set_scoped_models` (`scoped-models-selector.ts onPersist`).
                let catalog = session.model_catalog();
                let ordered_ids: Vec<String> = if value == crate::SCOPED_MODELS_ALL {
                    catalog.iter().map(|m| m.id.to_string()).collect()
                } else {
                    value.split('\n').filter(|s| !s.is_empty()).map(str::to_string).collect()
                };
                let scoped: Vec<cyrup_session_svc::ScopedModel> = ordered_ids
                    .iter()
                    .filter_map(|id| catalog.iter().find(|m| m.id.to_string() == *id))
                    .map(|m| cyrup_session_svc::ScopedModel {
                        model: m.clone(),
                        thinking_level: None,
                    })
                    .collect();
                let n = scoped.len();
                session.set_scoped_models(scoped);
                // GAP 4 — Pi's `Ctrl+S` here is `setEnabledModels(patterns)`
                // (`interactive-mode.ts:5072`), a real settings write; cyrup applied the scope to
                // the session only, which made the selector's own footer
                // ("Session-only. ctrl+s to save to settings.") a false promise. The patterns are
                // the ordered fully-qualified `provider/id` references, and the "all" sentinel
                // passes `undefined` upstream — `Value::Null` here, which `SettingsManager::set`
                // treats as key REMOVAL (`manager.rs:255-256`) rather than writing a literal null.
                let patterns = if value == crate::SCOPED_MODELS_ALL {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(
                        ordered_ids
                            .iter()
                            .filter_map(|id| catalog.iter().find(|m| m.id.to_string() == *id))
                            .map(|m| format!("{}/{}", m.provider, m.id))
                            .collect::<Vec<_>>()
                    )
                };
                // The `/model ` completer ranks the scoped set when one is active, else the whole
                // available catalog (`interactive-mode.ts:689-691` @v0.84.3), so changing a scope
                // changes its candidate list. Refreshed before the persist result is known: the
                // in-session scope has already changed either way, so the completer must follow it
                // even if the settings write fails.
                self.refresh_argument_sources(session);
                match session
                    .persist_setting(
                        cyrup_session_svc::SettingsScope::Global,
                        "enabledModels",
                        patterns,
                    )
                    .await
                {
                    Ok(()) => self
                        .state
                        .transcript
                        .push_status(format!("scoped models → {n} enabled")),
                    Err(e) => self.state.transcript.push_status(format!("settings error: {e}")),
                }
            }

            C::ConfirmSelection { kind: SelectorKind::Logout, value } => {
                // `/logout` onSelect (`interactive-mode.ts:5149-5166`): `modelRuntime.logout(id)` —
                // the ported `cyrup_config::login::logout`, which wraps a store failure as
                // `Credential store delete failed for …` (`ai/src/models.ts:446-452`). Env vars and
                // `models.json` are untouched, which is what the second message spells out.
                let Some(option) = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| self.state.logout_options.get(i))
                    .cloned()
                else {
                    // `if (!providerOption) return;` (`:5151-5153`).
                    return;
                };
                match cyrup_config::login::logout(&*session.services().auth, &option.id).await {
                    Ok(()) => {
                        // The credential is gone, so the auth snapshot pi's footer reads has lost
                        // this provider (`modelRuntime.logout` updates it before the repaint at
                        // `interactive-mode.ts:5388-5394`). Drop it here too, or ` (sub)` would
                        // survive the logout that removed the subscription credential.
                        self.state
                            .oauth_credential_providers
                            .remove(option.id.as_str());
                        self.refresh_subscription_marker();
                        let name = &option.name;
                        // Pi's two verbatim messages (`:5157-5161`).
                        let message = if option.auth_type == AuthType::Oauth {
                            format!("Logged out of {name}")
                        } else {
                            format!(
                                "Removed stored API key for {name}. Environment variables and \
                                 models.json config are unchanged."
                            )
                        };
                        self.state.transcript.push_status(message);
                    }
                    // `showError(\`Logout failed: ${message}\`)` (`:5163-5165`).
                    Err(e) => self
                        .state
                        .transcript
                        .push_error(format!("Logout failed: {e}")),
                }
            }

            C::ConfirmSelection { kind: SelectorKind::Login, value } => {
                // `OAuthSelectorComponent`'s onSelect (`interactive-mode.ts:5106-5117`): re-find the
                // chosen option and `startProviderLogin(providerOption)`. The value is the row INDEX
                // (see `SelectorKind::Login`), which is what `(providerId, authType)` collapses to.
                let Some(option) = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| self.state.login_options.get(i))
                    .cloned()
                else {
                    // `if (!providerOption) return;` (`:5113-5115`).
                    return;
                };
                self.begin_provider_login(session, option);
            }

            C::ConfirmSelection { kind: SelectorKind::LoginAuthType, value } => {
                // `showLoginAuthTypeSelector`'s onSelect (`interactive-mode.ts:5063-5073`): with a
                // pinned provider, start ITS option of the chosen kind; otherwise open the provider
                // picker filtered to that kind.
                let auth_type = if value == AuthType::Oauth.as_str() {
                    AuthType::Oauth
                } else {
                    AuthType::ApiKey
                };
                match self.state.login_auth_type_options.take() {
                    Some(options) => {
                        // `providerOptions.find(p => p.authType === authType)` (`:5066`).
                        if let Some(option) =
                            options.iter().find(|o| o.auth_type == auth_type).cloned()
                        {
                            self.begin_provider_login(session, option);
                        }
                    }
                    // `this.showLoginProviderSelector(authType)` (`:5071`).
                    None => {
                        let inputs = self.login_provider_inputs(session).await;
                        self.open_login_provider_selector(&inputs, Some(auth_type), None);
                    }
                }
            }

            C::ConfirmSelection { kind: SelectorKind::Trust, value } => {
                // The trust selector confirms with the chosen option INDEX; re-derive the options and
                // persist that option's store updates (Pi `/trust` `onSelect` → trust-store write).
                let options = session.project_trust_options();
                match value.parse::<usize>().ok().and_then(|i| options.get(i)) {
                    Some(opt) => match session.write_project_trust(&opt.updates).await {
                        Ok(()) => {
                            let label = if opt.trusted { "trusted" } else { "untrusted" };
                            self.state.transcript.push_status(format!(
                                "project trust → {label} (/reload to apply to this session)"
                            ));
                        }
                        Err(e) => self.state.transcript.push_status(format!("trust error: {e}")),
                    },
                    None => self.state.transcript.push_status("trust selection cancelled"),
                }
            }

            C::ConfirmSelection { kind: SelectorKind::Session, .. }
            | C::ConfirmSelection { kind: SelectorKind::UserMessage, .. } => {
                self.execute_session_switch(cmd, session, runtime).await
            }

            C::ConfirmSelection { kind, value } => {
                self.state.transcript.push_status(format!("{} → {value}", kind.title()));
            }

            // NOT unreachable by construction — nothing enforced that. `execute_command` decides
            // what arrives here, and a variant it routes to this function with no arm above lands in
            // this branch. Silence is the one failure mode that hides a misrouted arm, so report it.
            // See the `execute_misc_command` twin for the defect that made this necessary (cc19b87).
            other => {
                debug_assert!(false, "unrouted command in execute_selector_command: {other:?}");
                self.state
                    .transcript
                    .push_error(format!("internal: unrouted command {other:?}"));
            }
        }
    }
}
