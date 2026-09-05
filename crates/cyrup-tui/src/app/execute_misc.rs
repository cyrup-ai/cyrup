use super::*;

/// The picker/command level vocabulary → [`ModelThinkingLevel`].
///
/// The same strings [`crate::thinking_selector::ThinkingSelector`] rows on and `ModelThinkingLevel`'s
/// `serde(rename_all = "camelCase")` wire form produces (`cyrup-core/src/message/thinking.rs:25`),
/// so a level round-trips settings → picker → settings unchanged. Shared by the session-only
/// `SetThinking` arm and its persisting `ConfirmSelectionAsDefault` sibling, which must agree on
/// what a valid level is or `/thinking` and `Ctrl+S` could disagree.
impl<B: Backend> App<B> {
    /// Pi `_addPersistedDefaultToNonEmptyScope` (`agent-session.ts:1658-1670`), run after a model
    /// is persisted as the default.
    ///
    /// A scoped set that does not contain the new default would make it **uncyclable**: `Ctrl+P`
    /// walks the scoped models when any exist (`cycle_model`), so persisting a default outside
    /// that set leaves the user unable to reach it without clearing the scope. Pi therefore widens
    /// the scope to include it — both the LIVE set and, when the scope came from settings rather
    /// than a `--models` flag, the persisted `enabledModels` list.
    ///
    /// Every early return is Pi's, in Pi's order:
    /// - empty scoped set → nothing is restricted, nothing to widen (`:1659`);
    /// - model already scoped → already reachable (`:1660`);
    /// - `enabledModels` absent/empty → the scope is session-only (a `--models` flag), so the live
    ///   set is widened but nothing is written (`:1664-1665`);
    /// - reference already present, compared case-INSENSITIVELY → no duplicate row (`:1668`).
    pub(crate) async fn add_persisted_default_to_non_empty_scope(
        &mut self,
        session: &Arc<AgentSession>,
        provider: &str,
        id: &str,
    ) {
        let scoped = session.scoped_models();
        if scoped.is_empty() {
            return;
        }
        if scoped
            .iter()
            .any(|s| s.model.provider.as_str() == provider && s.model.id.as_str() == id)
        {
            return;
        }
        // Widen the LIVE set first (`:1662`), so the model is cyclable this session even when the
        // scope is flag-driven and nothing gets written below.
        let Some(model) = session
            .available_model_catalog()
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
            .cloned()
        else {
            return;
        };
        let mut widened = scoped;
        widened.push(cyrup_session_svc::ScopedModel {
            model,
            thinking_level: None,
        });
        session.set_scoped_models(widened);

        let existing = session.services().settings.effective().enabled_models();
        let Some(existing) = existing.filter(|v| !v.is_empty()) else {
            return;
        };
        let reference = format!("{provider}/{id}");
        if existing.iter().any(|p| p.eq_ignore_ascii_case(&reference)) {
            return;
        }
        let mut next = existing;
        next.push(reference);
        if let Err(e) = session
            .persist_setting(
                cyrup_session_svc::SettingsScope::Global,
                "enabledModels",
                serde_json::json!(next),
            )
            .await
        {
            self.state
                .transcript
                .push_status(format!("settings error: {e}"));
        }
    }
}

/// Pi `CLEAR_OVERRIDE_VALUE` (`settings-selector.ts:174`): the sentinel row value that REMOVES a
/// per-model override rather than setting one. Verbatim, so the two stay recognisably the same
/// mechanism.
pub(crate) const CLEAR_MODEL_THINKING: &str = "__clear__";

pub(crate) fn thinking_level_str(level: ModelThinkingLevel) -> &'static str {
    match level {
        ModelThinkingLevel::Off => "off",
        ModelThinkingLevel::Minimal => "minimal",
        ModelThinkingLevel::Low => "low",
        ModelThinkingLevel::Medium => "medium",
        ModelThinkingLevel::High => "high",
        ModelThinkingLevel::Xhigh => "xhigh",
        ModelThinkingLevel::Max => "max",
    }
}

pub(crate) fn parse_thinking_level(level: &str) -> Option<ModelThinkingLevel> {
    match level {
        "off" => Some(ModelThinkingLevel::Off),
        "minimal" => Some(ModelThinkingLevel::Minimal),
        "low" => Some(ModelThinkingLevel::Low),
        "medium" => Some(ModelThinkingLevel::Medium),
        "high" => Some(ModelThinkingLevel::High),
        "xhigh" => Some(ModelThinkingLevel::Xhigh),
        "max" => Some(ModelThinkingLevel::Max),
        _ => None,
    }
}

impl<B: Backend> App<B> {
    /// A command was registered from a live extension handler (HA-1). Re-read the catalog.
    ///
    /// Coalescing is the channel's: several registrations in one burst — an MCP server exposing
    /// eight prompts on connect — each send a `()`, and each rebuild re-reads the same live
    /// catalog, so a late one is never wrong, only redundant. Rebuilding is a map walk over an
    /// already-materialised `Vec`, which is why this is not worth debouncing.
    pub(crate) fn on_commands_changed(&mut self, ctx: &super::run::RunCtx) {
        let gate = ctx
            .session
            .services()
            .settings
            .effective()
            .enable_skill_commands();
        let session = std::sync::Arc::clone(&ctx.session);
        self.rebuild_command_registry(&session, gate);
    }

    /// Rebuild the `/` menu's dynamic half from the session's live catalog.
    ///
    /// ONE implementation, four callers: boot ([`Self::seed_session_ui`]), session swap, the
    /// `enableSkillCommands` toggle, and — since HA-1 — a late command registration arriving from a
    /// live extension handler. The first three set the registry at fixed points in the loop's
    /// control flow; the fourth is the one that made a shared method worth extracting, because a
    /// fourth open-coded copy is how the three drift apart.
    ///
    /// `slash_command_catalog()` reads `resolved_commands()` live, so nothing needs to be
    /// invalidated first — the rebuild simply re-reads the truth.
    pub(crate) fn rebuild_command_registry(
        &mut self,
        session: &Arc<AgentSession>,
        enable_skill_commands: bool,
    ) {
        self.state
            .editor
            .set_registry(crate::commands::CommandRegistry::with_dynamic(
                crate::commands::dynamic_commands_from_catalog_gated(
                    &session.slash_command_catalog(),
                    enable_skill_commands,
                ),
            ));
    }

    /// Re-read the argument-completion sources pi's `createBaseAutocompleteProvider` closes over
    /// (`interactive-mode.ts:685-736` @v0.84.3) and push them into the editor.
    ///
    /// pi's closures read the session live on every keystroke;
    /// [`crate::Autocomplete::compute`] is synchronous and holds no session, so cyrup takes a
    /// SNAPSHOT at the four points where the underlying sets can actually change: boot
    /// ([`Self::seed_session_ui`]), session swap, a credential change (the model catalog is
    /// auth-filtered) and a `/scoped-models` save. It is a snapshot, not a subscription: a provider
    /// or model that appears by some other path — a guest provider registered mid-session — is not
    /// offered until the next refresh.
    ///
    /// Per-keystroke refreshing is deliberately NOT an option: `available_model_catalog()`
    /// (`cyrup-session-svc/src/session/model.rs:235-237`) runs `has_configured_auth` per model.
    pub(crate) fn refresh_argument_sources(&mut self, session: &Arc<AgentSession>) {
        // `scopedModels.length > 0 ? scopedModels.map(s => s.model) :
        // modelRuntime.getAvailableSnapshot()` (`interactive-mode.ts:689-691`).
        // `available_model_catalog()` is the same source the `/model` picker reads
        // (`app/event_extract.rs:282`).
        let scoped = session.scoped_models();
        let models: Vec<crate::autocomplete::ModelArgument> = if scoped.is_empty() {
            session
                .available_model_catalog()
                .iter()
                .map(|m| crate::autocomplete::ModelArgument {
                    id: m.id.to_string(),
                    provider: m.provider.to_string(),
                    name: m.name.clone(),
                })
                .collect()
        } else {
            scoped
                .iter()
                .map(|sm| crate::autocomplete::ModelArgument {
                    id: sm.model.id.to_string(),
                    provider: sm.model.provider.to_string(),
                    name: sm.model.name.clone(),
                })
                .collect()
        };

        // `getLoginProviderCompletionOptions(this.getLoginProviderOptions())`
        // (`interactive-mode.ts:729`). The registry comes through the same seam
        // [`Self::build_login_inputs`] and [`Self::provider_oauth_strategy`] use, so the offline
        // test override ([`Self::set_login_provider_source`]) covers this path too. No auth-store
        // read: the completion row uses only id / name / authTypes, never a status (`:730-734`).
        let registry = match self.login_providers.as_deref() {
            Some(source) => source(),
            None => cyrup_provider::all_providers(),
        };
        let mut login_providers: Vec<crate::autocomplete::LoginProviderArgument> = registry
            .iter()
            .filter_map(|p| {
                // A provider with no auth strategy contributes no row (`:4948`/`:4957` both test a
                // member of `provider.auth`), exactly as `build_login_inputs` filters.
                let auth = p.provider_auth()?;
                // The push order IS `AUTH_TYPE_ORDER` — oauth 0, api_key 1
                // (`interactive-mode.ts:286`), which is what the merge step sorts by.
                let mut auth_types = Vec::with_capacity(2);
                if auth.oauth.is_some() {
                    auth_types.push(AuthType::Oauth);
                }
                if auth.api_key.is_some() {
                    auth_types.push(AuthType::ApiKey);
                }
                let id = p.id().as_str().to_string();
                Some(crate::autocomplete::LoginProviderArgument {
                    name: crate::provider_display_name(&id),
                    id,
                    auth_types,
                })
            })
            .collect();
        // `sort((a, b) => a.name.localeCompare(b.name))` (`interactive-mode.ts:317`). DEVIATION:
        // cyrup-tui carries no collator (`cyrup_config::login::sort_by_name` uses `feruca`, which
        // is not a dependency here), so this is the lowercased-name-then-id ordering
        // `auth_select::provider_rows` (`:115`) already uses for the very same provider list —
        // identical for the ASCII ids every provider actually has.
        login_providers.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });

        // `session.getAvailableThinkingLevels()` (`core/agent-session.ts:1816-1819`): the current
        // model's ladder, or the full `THINKING_LEVEL_OPTIONS` when no model is selected. pi reads
        // it from the session at BOTH consumers — the `/thinking` completer
        // (`interactive-mode.ts:715`) and the picker (`:4792`) — so this one call feeds both here
        // too, instead of the completer re-deriving it from the catalog and drifting from what the
        // picker offers.
        let thinking_levels: Vec<String> = session
            .available_thinking_levels()
            .into_iter()
            .map(|l| thinking_level_str(l).to_string())
            .collect();
        self.state.available_thinking_levels = thinking_levels.clone();

        // `extension_completions` is NOT part of this snapshot — it is fetched per keystroke by
        // [`Self::refresh_extension_completions`] and carried across by `set_argument_sources`.
        self.state
            .editor
            .set_argument_sources(crate::autocomplete::ArgumentSources {
                models,
                login_providers,
                thinking_levels,
                ..crate::autocomplete::ArgumentSources::default()
            });
    }

    /// The async half of an EXTENSION command's argument completion — pi's
    /// `await command.getArgumentCompletions(argumentText)` (`packages/tui/src/autocomplete.ts:355`
    /// @v0.84.3), lifted out of the synchronous popup path.
    ///
    /// Called once per serviced input batch, from the run loop's input arm and BEFORE the frame is
    /// drawn, so a completion fetched for the key just pressed still reaches that key's frame. When
    /// the line under the cursor is not `/<ext-command> <arg>` this is a single string test and a
    /// clear; when it is, and `(command, argument)` has not changed since the last fetch, it is a
    /// comparison. Only a genuinely new pair reaches the guest.
    ///
    /// The guest call is epoch-deadlined at command tier (`cyrup-ext/src/host/live.rs:1592`), which
    /// is what bounds a misbehaving completer's effect on the input loop. An error — no live owner,
    /// a trap, a timeout — is an EMPTY completion set, matching pi's non-array/empty branch
    /// (`autocomplete.ts:356-357`): the popup closes and typing continues. It is not surfaced as a
    /// notification, because it would fire on every keystroke.
    ///
    /// See [`crate::commands::ArgumentCompleter::Extension`] for why the fetch lives here.
    pub(crate) async fn refresh_extension_completions(&mut self, session: &Arc<AgentSession>) {
        let Some((name, argument)) = self.state.editor.pending_extension_argument() else {
            // Left the argument context: forget the query, so re-entering it refetches rather than
            // trusting a set the guest may since have changed its mind about.
            self.state.extension_completion_query = None;
            return;
        };
        if self
            .state
            .extension_completion_query
            .as_ref()
            .is_some_and(|(c, a)| c == &name && a == &argument)
        {
            return;
        }
        self.state.extension_completion_query = Some((name.clone(), argument.clone()));
        let items = session
            .services()
            .ext_host
            .command_completions(&name, &argument)
            .await
            .unwrap_or_default();
        // No staleness check on the way back, and none is needed: this future holds `&mut self`
        // across the await, so no key can be serviced while it is in flight — the pair it answers
        // for is by construction still the pair on screen. The cost is that a slow completer
        // delays the frame; the epoch deadline is what bounds that.
        self.state
            .editor
            .set_extension_completions(&name, &argument, items);
        self.state.editor.refresh_autocomplete();
    }

    /// The remaining [`Self::execute_command`] arms (§7.1): login, model/thinking cycling,
    /// settings writes, and the small session conveniences. Arm bodies moved verbatim.
    pub(crate) async fn execute_misc_command(
        &mut self,
        cmd: AppCommand,
        session: &Arc<AgentSession>,
    ) {
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
                            (
                                sm.model.id.to_string(),
                                sm.model.provider.to_string(),
                                sm.model.name.clone(),
                            )
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
                    let msg = if scoped_active {
                        "Only one model in scope"
                    } else {
                        "Only one model available"
                    };
                    self.state.transcript.push_status(msg);
                } else {
                    let current = session.model();
                    let n = cycle.len();
                    // `session.model()` is `Option` (pi `AgentSession.model: Model | undefined`,
                    // agent-session.ts:866-868): a modelless session matches nothing, so the cycle
                    // starts at the head exactly as pi's `findIndex === -1 ⇒ 0` does
                    // (agent-session.ts:1650-1653).
                    let cur = cycle.iter().position(|(id, prov, _)| {
                        current
                            .as_ref()
                            .is_some_and(|c| id == c.model.as_str() && prov == c.provider.as_str())
                    });
                    let next = match direction {
                        CycleDirection::Forward => cur.map_or(0, |i| (i + 1) % n),
                        CycleDirection::Backward => cur.map_or(0, |i| (i + n - 1) % n),
                    };
                    if let Some((id, _prov, name)) = cycle.get(next) {
                        match session.set_model(id).await {
                            Ok(_) => self
                                .state
                                .transcript
                                .push_status(format!("Switched to {name}")),
                            Err(e) => self
                                .state
                                .transcript
                                .push_status(format!("model error: {e}")),
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
                        self.state
                            .transcript
                            .push_status("Current model does not support thinking");
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
                        self.state
                            .transcript
                            .push_status(format!("Thinking level: {label}"));
                    }
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("thinking error: {e}")),
                }
            }
            // TUI-032 — the `/settings` → `Thinking level` submenu's confirm. Pi's
            // `onThinkingLevelChange` (`interactive-mode.ts:4222-4226`) calls
            // `session.setThinkingLevel(level)`, which clamps to the model's capabilities and emits
            // `ThinkingLevelChanged`; the footer + editor rule re-color off that event exactly as
            // they do for Shift+Tab.
            C::SetThinking(level) => {
                let parsed = parse_thinking_level(&level);
                match parsed {
                    Some(l) => match session.set_thinking_level(l).await {
                        Ok(applied) => {
                            let label = thinking_level_str(applied);
                            self.state
                                .transcript
                                .push_status(format!("Thinking level: {label}"));
                        }
                        Err(e) => self
                            .state
                            .transcript
                            .push_status(format!("thinking error: {e}")),
                    },
                    None => self
                        .state
                        .transcript
                        .push_status(format!("thinking error: unknown level {level}")),
                }
            }

            // Routed here, not to `execute_selector_command`: `execute_command`'s selector
            // bucket lists `OpenSelector | ConfirmSelection | SetEntryLabel` only, so this
            // variant reaches the `_` arm and lands in this file. Its `Thinking` sibling is
            // directly below; both must stay here or the dispatcher silently drops them.
            C::ConfirmSelectionAsDefault {
                kind: SelectorKind::Model,
                value,
            } => {
                // Pi `selectModel(model, true)` → `setModel(model, { persist: true })`
                // (`interactive-mode.ts:4999` → `agent-session.ts:1630-1650`). ORDER IS THE
                // CONTRACT: the session set runs FIRST and a failure returns without writing
                // anything. Pi throws out of `setModel` before its persist when auth is missing
                // (`agent-session.ts:1637-1639`); cyrup's `set_model_resolved` returns
                // `NoConfiguredAuth` first (`model.rs:44-50`), so the same guard falls out of
                // sequencing the persist after a successful set.
                if let Err(e) = session.set_model(&value).await {
                    self.state
                        .transcript
                        .push_status(format!("model error: {e}"));
                    return;
                }
                // `setDefaultModelAndProvider(provider, id)` writes BOTH keys together
                // (`settings-manager.ts:737-744`); never one alone, or a relaunch resolves a model
                // id against the wrong provider. The picker confirms a fully-qualified
                // `provider/id`, so the split is the inverse of how it was built.
                let (provider, id) = match value.split_once('/') {
                    Some((p, i)) => (p.to_string(), i.to_string()),
                    // Unreachable from the picker; degrade rather than write a half pair.
                    None => {
                        self.state
                            .transcript
                            .push_status(format!("model → {value} (not persisted: no provider)"));
                        return;
                    }
                };
                let scope = cyrup_session_svc::SettingsScope::Global;
                let wrote = match session
                    .persist_setting(scope, "defaultProvider", serde_json::json!(provider))
                    .await
                {
                    Ok(()) => {
                        session
                            .persist_setting(scope, "defaultModel", serde_json::json!(id))
                            .await
                    }
                    Err(e) => Err(e),
                };
                match wrote {
                    // `Default model: {provider}/{id}` vs the plain path's `Model: {id}`
                    // (`interactive-mode.ts:4978`).
                    Ok(()) => self
                        .state
                        .transcript
                        .push_status(format!("Default model: {provider}/{id}")),
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("settings error: {e}")),
                }
                self.state.default_model = Some((provider.clone(), id.clone()));
                self.add_persisted_default_to_non_empty_scope(session, &provider, &id)
                    .await;
            }

            C::ConfirmSelectionAsDefault {
                kind: SelectorKind::Thinking,
                value,
            } => {
                // Pi `selectLevel(level, true)` → `setThinkingLevel(level, { persist: true })`
                // (`interactive-mode.ts:4813` → `:4788`). Same ordering contract as the model
                // path: apply to the session first, and persist only if that succeeded.
                let Some(parsed) = parse_thinking_level(&value) else {
                    self.state
                        .transcript
                        .push_status(format!("thinking error: unknown level {value}"));
                    return;
                };
                if let Err(e) = session.set_thinking_level(parsed).await {
                    self.state
                        .transcript
                        .push_status(format!("thinking error: {e}"));
                    return;
                }
                // THE REQUESTED LEVEL, NOT THE CLAMPED ONE. `setThinkingLevel` clamps to what the
                // active model supports and returns the applied value, but Pi persists `level` —
                // the argument — not `effectiveLevel` (`agent-session.ts:1782`). Writing the clamp
                // would silently downgrade the user's stored default the first time they set it
                // while on a weaker model, and it would never recover on a stronger one.
                match session
                    .persist_setting(
                        cyrup_session_svc::SettingsScope::Global,
                        "defaultThinkingLevel",
                        serde_json::json!(value),
                    )
                    .await
                {
                    // `Default thinking level: {level}` vs the plain path's
                    // `Thinking level: {level}` (`interactive-mode.ts:4793`).
                    Ok(()) => {
                        self.state.default_thinking_level = value.clone();
                        self.state
                            .transcript
                            .push_status(format!("Default thinking level: {value}"));
                    }
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("settings error: {e}")),
                }
            }

            C::ThinkingCommand(arg) => {
                // Pi `handleThinkingCommand(searchTerm?)` (`interactive-mode.ts:4771-4785`).
                let levels = session.available_thinking_levels();
                // The picker's ladder is the session's, not a hardcoded seven
                // (`interactive-mode.ts:4792`). Re-seeded here because this is the primary route
                // in and the snapshot that normally fills it may predate a model switch.
                self.state.available_thinking_levels = levels
                    .iter()
                    .copied()
                    .map(thinking_level_str)
                    .map(str::to_string)
                    .collect();
                let Some(term) = arg else {
                    // No argument → the picker. Seed the persisted default first so the row can be
                    // badged and `Ctrl+S` offered — this is the second route in (the other is
                    // `/settings` → Thinking level), and neither reaches a session later.
                    self.state.default_thinking_level = session
                        .services()
                        .settings
                        .effective()
                        .default_thinking_level()
                        .map(thinking_level_str)
                        .unwrap_or("medium")
                        .to_string();
                    self.open_selector(SelectorKind::Thinking);
                    return;
                };
                // `searchTerm.trim().toLowerCase()` matched case-insensitively against the
                // AVAILABLE levels — not the full ladder — so a level the active model cannot do
                // is reported unknown rather than silently clamped (`:4779-4780`).
                let normalized = term.trim().to_lowercase();
                let matched = levels
                    .iter()
                    .copied()
                    .find(|l| thinking_level_str(*l).eq_ignore_ascii_case(&normalized));
                let Some(level) = matched else {
                    // Pi's exact copy, including the quotes around the RAW term (not the
                    // normalized one) and the trailing period (`:4781`).
                    let available: Vec<&str> =
                        levels.iter().copied().map(thinking_level_str).collect();
                    self.state.transcript.push_status(format!(
                        "Unknown thinking level \"{term}\". Available levels: {}.",
                        available.join(", ")
                    ));
                    return;
                };
                // `selectThinkingLevel(level, false)` — SESSION-ONLY, the same as `Enter` in the
                // picker (`:4786`). The persisting sibling is `Ctrl+S`, never this.
                match session.set_thinking_level(level).await {
                    Ok(applied) => {
                        let label = thinking_level_str(applied);
                        self.state.thinking_level = label.to_string();
                        self.state.status.set_thinking_level(label);
                        self.state.editor.set_thinking_level(label);
                        self.state
                            .transcript
                            .push_status(format!("Thinking level: {label}"));
                    }
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("thinking error: {e}")),
                }
            }

            C::SetModelThinkingLevel { model, level } => {
                // GAP 3's write (Pi `setModelThinkingLevel` / `removeModelThinkingLevel`,
                // `settings-manager.ts:800-812`).
                //
                // \[CYRUP-DELTA] The task prescribed
                // `persist_setting(Global, "modelThinkingLevels.{provider}/{id}", …)`. That is
                // WRONG here: `persist_setting` addresses nesting by splitting the key on `.`
                // (`accessors.rs:135`), and **325 model ids in this workspace's catalogs contain a
                // dot** (`claude-opus-4.5`, `gemini-2.5-pro`, …), so the key would split mid-id and
                // write a bogus nested object for the overwhelming majority of models. Pi has no
                // such hazard because its setter indexes the map directly with the composed string
                // (`:804`). The whole map is therefore read, modified, and written back under the
                // single un-dotted key `modelThinkingLevels`, which is the same end state Pi
                // reaches and is dot-safe by construction.
                let eff_map = session
                    .services()
                    .settings
                    .effective()
                    .all_model_thinking_levels();
                let mut map: serde_json::Map<String, serde_json::Value> = eff_map
                    .into_iter()
                    .filter_map(|(k, v)| serde_json::to_value(v).ok().map(|v| (k, v)))
                    .collect();
                if level == CLEAR_MODEL_THINKING {
                    map.remove(&model);
                } else if let Some(parsed) = parse_thinking_level(&level) {
                    let Ok(v) = serde_json::to_value(parsed) else {
                        return;
                    };
                    map.insert(model.clone(), v);
                } else {
                    self.state
                        .transcript
                        .push_status(format!("thinking error: unknown level {level}"));
                    return;
                }
                // `modelThinkingOverridesSummary(currentModelThinkingLevels)`
                // (`settings-selector.ts:184-188`), the value pi's `done(summary())` writes back
                // onto the `model-thinking` row — taken off the map being persisted, so it needs no
                // settings re-read to be current.
                let summary = model_thinking_summary_for_count(map.len());
                // `if (Object.keys(...).length === 0) delete this.globalSettings
                // .modelThinkingLevels` (`settings-manager.ts:810-812`) — an emptied map removes
                // the key rather than persisting `{}`. `Value::Null` is `SettingsManager::set`'s
                // key-removal signal (`manager.rs:255-256`).
                let payload = if map.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::Object(map)
                };
                match session
                    .persist_setting(
                        cyrup_session_svc::SettingsScope::Global,
                        "modelThinkingLevels",
                        payload,
                    )
                    .await
                {
                    Ok(()) => {
                        let msg = if level == CLEAR_MODEL_THINKING {
                            format!("{model} → global default")
                        } else {
                            format!("{model} → {level}")
                        };
                        self.state.transcript.push_status(msg);
                        // The `/settings` list under this submenu keeps showing the summary it was
                        // built with unless it is written through — pi's submenu `done(summary())`
                        // (`settings-selector.ts:665`), which runs `item.currentValue = …` before
                        // the list comes back (`settings-list.ts:222-225`).
                        self.set_settings_row_value("model-thinking", &summary);
                        // pi's `{ loop: true }`: the final step re-enters step 0 rather than
                        // closing (`settings-submenu.ts:226-231`). The step-2 confirm has already
                        // popped back to the retained step-1 list; rebuild it in place, keeping its
                        // parent, so the model whose override just changed shows the new value in
                        // its description.
                        if self.active_selector_kind() == Some(SelectorKind::ModelThinking) {
                            let parent =
                                self.state.selector.take().and_then(|active| active.parent);
                            let entries = super::execute::model_thinking_rows(session);
                            self.open_data_child_selector(
                                SelectorKind::ModelThinking,
                                entries,
                                0,
                                parent,
                            );
                        }
                    }
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("settings error: {e}")),
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
                    let hide = value == "true";
                    self.state.set_hide_thinking(hide);
                    // The `/settings` slot stays open on a cycle and its rows are built ONCE from
                    // the effective settings (`app/execute.rs`'s `C::OpenSelector` arm), so the
                    // sibling `Thinking level` row would keep the marker state it was born with —
                    // reading as "reasoning is visible" one line under the switch that just
                    // suppressed it, and keeping a stale `(hidden)` when the switch goes back off.
                    // Written through exactly as `model-thinking` is above, which is pi's own
                    // `item.currentValue = …` before `onChange` (`tui/src/components/
                    // settings-list.ts:222-225` for a submenu return, `:236-238` for a cycle).
                    // A no-op when no settings list is open, which is the bare `Ctrl+T` case.
                    let shown = thinking_row_value(&self.state.thinking_level, hide);
                    self.set_settings_row_value("thinking", &shown);
                }
                // `markdown.mermaid` is live upstream by construction: `onMermaidRenderingModeChange`
                // persists (`settings-selector.ts:856-857`) and the transformer's
                // `getMode: () => this.settingsManager.getMermaidRenderingMode()` closure
                // (`interactive-mode.ts:484-486`) is re-read on every render. cyrup caches the mode
                // on the transcript, so it has to be pushed in here or the row would not take
                // effect until the next session bind. Already-flushed native scrollback keeps the
                // form it committed with — the same accepted limit `outputPad`/`hideThinkingBlock`
                // carry above.
                if id == "markdown.mermaid" {
                    self.state
                        .transcript
                        .set_mermaid_mode(crate::markdown::mode_from_setting(&value));
                }
                // `showCacheMissNotices` is live in Pi by construction: every emission re-reads
                // `getShowCacheMissNotices()` (`interactive-mode.ts:3803`, `:3821`, `:3694`).
                // cyrup caches the gate on `AppState` — the fold and the replay walk hold no
                // settings view — so the flip has to be pushed in here, or the row would not take
                // effect until the next session bind.
                if id == "showCacheMissNotices" {
                    self.state.show_cache_miss_notices = value == "true";
                }
                // The image rows are live too (Pi re-reads them per `ToolExecutionComponent`).
                if id == "terminal.showImages" {
                    self.state.show_images = value == "true";
                    self.state
                        .transcript
                        .set_show_images(self.state.show_images);
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
                    // The NEW gate value, not the effective one: the setting has not been
                    // committed yet at this point.
                    self.rebuild_command_registry(session, value == "true");
                }
                // CFG-078 — both alt-screen rows are live in pi. `onFullscreenCopyOnSelectChange`
                // persists and then pushes into the running renderer
                // (`interactive-mode.ts:4757-4760`); `onFullscreenExitOutputChange` persists only
                // (`:4750-4752`) because pi re-reads the getter at `stop()` time (`:6556`). cyrup
                // caches BOTH on `App` — the renderer is built per excursion and the exit teardown
                // has no settings view — so each has to be pushed in here, or a row cycled
                // mid-session would not take effect until the next launch.
                if id == "fullscreenCopyOnSelect" {
                    self.set_fullscreen_copy_on_select(value == "true");
                }
                if id == "fullscreenExitOutput" {
                    self.set_fullscreen_exit_output(if value == "resume-hint" {
                        crate::altscreen::FullscreenExitOutput::ResumeHint
                    } else {
                        // The getter's own degrade rule (`settings-manager.ts:1213`), applied to
                        // the row's value so the cached copy and a later `EffectiveSettings` read
                        // of the same document cannot disagree.
                        crate::altscreen::FullscreenExitOutput::Transcript
                    });
                }
                // `transport` is live in Pi too, and it is the ONLY row whose live half touches the
                // agent rather than the UI: `onTransportChange` persists the setting AND assigns
                // `this.session.agent.transport = transport` (`interactive-mode.ts:4213-4216`), so
                // the very next request streams with the chosen transport. cyrup persisted only,
                // which left `AgentBuilder::transport`'s build-time seed in force until restart.
                if id == "transport" {
                    session.set_transport(&value).await;
                }
                match session
                    .persist_setting(cyrup_session_svc::SettingsScope::Global, &id, json)
                    .await
                {
                    Ok(()) => self.state.transcript.push_status(format!("{id} → {value}")),
                    Err(e) => self
                        .state
                        .transcript
                        .push_status(format!("settings error: {e}")),
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
                                Some(s) =>
                                    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}")),
                                None => "null".to_string(),
                            },
                        ));
                    }
                    let shown = stored.unwrap_or(name);
                    self.state
                        .transcript
                        .push_status(format!("Session name set: {shown}"));
                }
                Err(e) => self
                    .state
                    .transcript
                    .push_status(format!("name error: {e}")),
            },
            // TUI-080 / TUI-084 — the getter, and pi's severity CHANNEL for the usage line: a
            // `showWarning` (`interactive-mode.ts:5638`), not a neutral status, and pi's exact
            // string `Usage: /name <name>` rather than cyrup's `usage: /name <session name>`.
            C::ShowName => match session.session_name().await {
                Some(name) => self
                    .state
                    .transcript
                    .push_status(format!("Session name: {name}")),
                None => self.state.transcript.push_warning("Usage: /name <name>"),
            },

            // ADR-0005 §B-11: with an active alternate-screen selection, `/copy` copies THAT
            // rather than the last assistant message — upstream asks
            // `getSelectionBounds() !== undefined` first (`tui-alt-screen.ts:545`) and copies the
            // selection when it answers yes. This arm was specified in B-11 and never wired, which
            // is what left `selection::has_selection` dead: selecting text and running `/copy`
            // silently copied the wrong thing.
            C::Copy
                if self
                    .altscreen
                    .as_ref()
                    .and_then(AltScreen::selection_text)
                    .is_some() =>
            {
                let Some(text) = self.altscreen.as_ref().and_then(AltScreen::selection_text) else {
                    return;
                };
                let n = text.chars().count();
                if crate::clipboard::copy_to_clipboard(&text).await {
                    self.state
                        .transcript
                        .push_status(format!("copied selection ({n} chars)"));
                } else {
                    self.state
                        .transcript
                        .push_error("Failed to copy to clipboard");
                }
            }

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
                        self.state
                            .transcript
                            .push_error("Failed to copy to clipboard");
                    }
                }
                None => self
                    .state
                    .transcript
                    .push_status("no assistant message to copy"),
            },

            // `app.message.copy` inside `/tree` — pi's `selector.onCopy` consumer
            // (`interactive-mode.ts:5297-5308`), whose three outcomes are reproduced verbatim:
            // a falsy text is `showError("Selected entry has no text to copy")`, a successful write
            // is `showStatus("Copied selected message to clipboard")`, and a throwing
            // `copyToClipboard` is surfaced through the same error channel. The clipboard call site
            // is `C::Copy`'s above — [`crate::clipboard::copy_to_clipboard`] — not a second one.
            C::CopyEntry(entry_id) => {
                match session.entry_copy_text(&entry_id.as_str().into()).await {
                    Some(text) => {
                        if crate::clipboard::copy_to_clipboard(&text).await {
                            self.state
                                .transcript
                                .push_status("Copied selected message to clipboard");
                        } else {
                            // The message pi throws when every clipboard branch failed
                            // (`clipboard.ts:171-173`), reaching `showError` via the `catch`.
                            self.state
                                .transcript
                                .push_error("Failed to copy to clipboard");
                        }
                    }
                    None => self
                        .state
                        .transcript
                        .push_error("Selected entry has no text to copy"),
                }
            }

            C::Share => self.share_session(session).await,

            // Was `_ => {}` under a comment asserting this is unreachable. Nothing enforced that
            // assertion, and it was false for `ConfirmSelectionAsDefault { kind: Model, .. }` until
            // cc19b87 — the arm existed, in a function the router never sent this variant to, so the
            // command vanished with no model change, no settings write and no status. Silence is the
            // one failure mode that hides a misrouted arm, so refuse to be silent.
            other => {
                debug_assert!(false, "unrouted command in execute_misc_command: {other:?}");
                self.state
                    .transcript
                    .push_error(format!("internal: unrouted command {other:?}"));
            }
        }
    }

    /// `/share` (`handleShareCommand`, interactive-mode.ts:5191): export the session to HTML, write a
    /// temp file, then shell `gh gist create --public=false <file>` behind a [`crate::chrome::BorderedLoader`] and
    /// surface the resulting gist URL. `gh` missing / logged-out is caught by pi's `gh auth status`
    /// pre-check (`session-share.ts:59-68`) before anything is mounted; a failing upload degrades to
    /// a status line. Fully in-crate (the HTML body is rendered by [`crate::export`]).
    ///
    /// The upload itself is SPAWNED whenever a run loop is present, never awaited on the loop task —
    /// see [`Self::share_tx`] — and settles through [`Self::apply_share_outcome`].
    async fn share_session(&mut self, session: &Arc<AgentSession>) {
        use tokio::process::Command;
        // Render the session HTML over its own JSONL (the same body `/export` writes).
        let html = match session.export_to_jsonl(None).await {
            Ok(Some(jsonl)) => crate::export::session_jsonl_to_html(&jsonl),
            Ok(None) => {
                self.state
                    .transcript
                    .push_status("nothing to share (empty session)");
                return;
            }
            Err(e) => {
                self.state
                    .transcript
                    .push_status(format!("share export error: {e}"));
                return;
            }
        };
        let tmp = std::env::temp_dir().join(format!("cyrup-session-{}.html", session.session_id()));
        if let Err(e) = std::fs::write(&tmp, html.as_bytes()) {
            self.state
                .transcript
                .push_status(format!("share write error: {e}"));
            return;
        }
        // pi's `gh auth status` pre-check, run BEFORE the loader is mounted
        // (`session-share.ts:59-68`): a logged-out `gh` otherwise reaches the transcript as raw
        // stderr, with no hint that `gh auth login` is the fix.
        //
        // Deliberately awaited inline, unlike the upload below: upstream runs it with `spawnSync`
        // (`:60`) — it blocks pi's event loop too — and its ORDER is the point, since a failed
        // check must return before anything is mounted rather than flashing a loader that is
        // immediately torn down. There is nothing on screen to animate while it runs.
        match Command::new("gh").args(["auth", "status"]).output().await {
            Ok(out) if out.status.success() => {}
            // pi's `catch` arm (`:66-68`). Node's `spawnSync` reports a missing binary as
            // `result.error` rather than a throw, so upstream's own ENOENT actually lands in the
            // `status !== 0` branch below and mislabels an uninstalled `gh` as logged out; the
            // message it *intends* for that case is this one, and `ErrorKind::NotFound` is the
            // honest test for it. Same string as the gist path used to carry, now aligned on pi's
            // wording ("Install it from", not cyrup's "— see").
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(&tmp);
                self.state.transcript.push_error(
                    "Error: GitHub CLI (gh) is not installed. Install it from https://cli.github.com/",
                );
                return;
            }
            // pi's `authResult.status !== 0` (`:61-64`). `showError` builds its own `Error: `
            // prefix (`interactive-mode.ts:3878-3882`) while `Entry::Error` renders verbatim, so
            // the prefix is supplied here — the same shape the gist-id arm below uses.
            _ => {
                let _ = std::fs::remove_file(&tmp);
                self.state
                    .transcript
                    .push_error("Error: GitHub CLI is not logged in. Run 'gh auth login' first.");
                return;
            }
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
        // A fresh run is never pre-cancelled (pi builds a new `AbortController` with every loader).
        self.state.share_cancelled = false;
        let Some(tx) = self.share_tx.clone() else {
            // No run loop (an embedder or a test driving `execute_command` directly): await inline,
            // exactly as `/tree` does (`tree_nav.rs:112-116`). Nothing would draw the loader on this
            // path and no keystroke could be delivered, so there is nothing to keep the task free
            // for and no cancel to retain a handle for.
            let result = Command::new("gh")
                .args(["gist", "create", "--public=false"])
                .arg(&tmp)
                .output()
                .await;
            self.apply_share_outcome(ShareMsg { result, tmp });
            return;
        };
        // pi spawns the child and awaits it while the render loop keeps running
        // (`session-share.ts:172-186`); here the await lives on its own task and the settled result
        // comes back over `share_tx`, so this call returns to the `select!` immediately and the very
        // next iteration draws the mounted loader.
        //
        // `kill_on_drop` is the cancel mechanism: `wait_with_output` consumes the child, so the task
        // is its only owner and aborting the task is what kills `gh` (see [`ShareInFlight`]).
        let spawned = Command::new("gh")
            .args(["gist", "create", "--public=false"])
            .arg(&tmp)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let child = match spawned {
            Ok(child) => child,
            // pi's `catch` around the whole promise (`:198-203`) — reported through the same arms
            // the settled result uses.
            Err(e) => {
                self.apply_share_outcome(ShareMsg {
                    result: Err(e),
                    tmp,
                });
                return;
            }
        };
        let waiter_tmp = tmp.clone();
        let waiter = tokio::spawn(async move {
            let result = child.wait_with_output().await;
            let _ = tx.send(ShareMsg {
                result,
                tmp: waiter_tmp,
            });
        });
        self.state.share_in_flight = Some(ShareInFlight {
            task: waiter.abort_handle(),
            tmp,
        });
    }

    /// Finish a settled `/share` on the run loop's task — pi's post-`await` tail
    /// (`session-share.ts:174-197`), which unmounts the loader (`restoreEditor`, `:205-210`) and
    /// then reports.
    ///
    /// Shared by the spawned and the inline path so the two cannot drift, mirroring
    /// [`Self::apply_compact_outcome`].
    pub fn apply_share_outcome(&mut self, msg: ShareMsg) {
        // `restoreEditor(loader, context)` — the loader comes down before anything is printed.
        self.state.loader = None;
        self.state.share_in_flight = None;
        // pi's `finally { fs.unlinkSync(tmpFile) }` (`:76-84`): every path unlinks, cancellation
        // included (the cancel path unlinks too, so this is a no-op there — `remove_file`'s error is
        // deliberately ignored, exactly as upstream's `catch {}` ignores it).
        let _ = std::fs::remove_file(&msg.tmp);
        // pi re-checks `loader.signal.aborted` on EVERY completion path (`:174`, `:191-199`) and
        // returns without touching the UI: `gh` may have settled in the window between the user's
        // Escape and the abort landing, and a cancelled share must print neither a gist URL nor a
        // `share error:` line over the top of `Share cancelled`.
        if std::mem::take(&mut self.state.share_cancelled) {
            return;
        }
        match msg.result {
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
                let detail = if msg.is_empty() {
                    "gh gist create failed"
                } else {
                    msg
                };
                self.state
                    .transcript
                    .push_status(format!("share error: {detail}"));
            }
            // No `ErrorKind::NotFound` arm: a missing `gh` is reported by the `gh auth status`
            // pre-check above, which runs first and returns — this path is only ever reached with a
            // `gh` that exists.
            Err(e) => self
                .state
                .transcript
                .push_status(format!("share error: {e}")),
        }
    }
}
