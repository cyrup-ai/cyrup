use super::*;

/// Row values of the "Summarize branch?" prompt. Pi compares the returned LABELS
/// (`summaryChoice !== "No summary"`, `=== "Summarize with custom prompt"`,
/// `interactive-mode.ts:4767,4769`); cyrup's [`ListSelector`] carries a separate value column, so
/// the labels stay Pi-exact for display while the routing keys stay stable.
/// The one provider id pi's footer treats as subscription-backed regardless of how it authenticates
/// — *"Kimi Coding is subscription-backed despite using API-key authentication"*
/// (pi v0.84.1 `coding-agent/src/modes/interactive/components/footer.ts:138-140`).
pub(crate) const KIMI_CODING_PROVIDER_ID: &str = "kimi-coding";

impl<B: Backend> App<B> {
    /// Override where `/login` sources its provider registry (default:
    /// `cyrup_provider::all_providers()`).
    ///
    /// This is the offline-test seam mandated by the "tests must never hit real provider APIs"
    /// convention: a test injects a provider whose `OAuthAuth::login` is a pure in-process function,
    /// so the full `/login` path — picker → dialog → `AuthInteraction` → `cyrup_config::login::login`
    /// → credential store — runs end to end with no socket opened. Production never calls it.
    pub fn set_login_provider_source(&mut self, source: LoginProviderSource) {
        self.login_providers = Some(source);
    }

    /// Override the gateway `/share`'s Radius upload posts to (default: pi's hardcoded
    /// `DEFAULT_RADIUS_GATEWAY`, `session-share.ts:112`). DRIFT-053.
    ///
    /// The same offline-test seam as [`Self::set_login_provider_source`] and mandated by the same
    /// convention: pointed at a local address, a test can assert that a session holding a radius
    /// credential takes the Radius path and never shells `gh` — the item's Verify line — without
    /// any request leaving the machine. Production never calls it.
    pub fn set_radius_share_gateway(&mut self, gateway: impl Into<String>) {
        self.radius_gateway = Some(gateway.into());
    }

    /// `this.session.modelRuntime.getProviders()` + `getProviderAuthStatus` + `isUsingOAuth`, the
    /// three registry reads `getLoginProviderOptions` folds together
    /// (`interactive-mode.ts:4943-4947`).
    ///
    /// Pi's `Provider` interface carries `name`; cyrup's does not (the display name lives on the
    /// concrete `WireProvider`), so the name comes from [`crate::provider_display_name`] — the same
    /// `getProviderDisplayName` fallback the picker already used for its labels.
    pub(crate) async fn login_provider_inputs(
        &self,
        session: &Arc<AgentSession>,
    ) -> Vec<ProviderLoginInput> {
        Self::build_login_inputs(session, self.login_providers.as_deref()).await
    }

    /// The `&self`-free form of [`Self::login_provider_inputs`], so the spawned login task can
    /// rebuild the inputs itself — `ProviderLoginInput` is not `Clone`, so the vector cannot be
    /// handed across.
    ///
    /// The stored-credential kinds are read ONCE (`listCredentials()`, `auth-storage.ts:252-254`)
    /// rather than per provider: pi answers `isUsingOAuth` off a single in-memory
    /// `snapshot.auth` map (`model-runtime.ts:368`), and a read-per-provider would be ~31
    /// lock-and-parse round trips through `auth.json` every time `/login` opens.
    async fn build_login_inputs(
        session: &Arc<AgentSession>,
        source: Option<&(dyn Fn() -> Vec<Arc<dyn cyrup_provider::Provider>> + Send + Sync)>,
    ) -> Vec<ProviderLoginInput> {
        let store = &session.services().auth;
        let stored = cyrup_config::login::stored_credentials(store)
            .await
            .unwrap_or_default();
        let providers = match source {
            Some(source) => source(),
            None => cyrup_provider::all_providers(),
        };
        let mut out = Vec::with_capacity(providers.len());
        for provider in providers {
            // `provider.auth` — a provider with no auth strategy at all contributes no row
            // (`:4948`/`:4957` both test a member of it).
            let Some(auth) = provider.provider_auth().cloned() else {
                continue;
            };
            let id = provider.id().clone();
            // `isUsingOAuth(id)`: `snapshot.auth.get(id)?.type === "oauth"` — the STORED
            // credential's kind, not the provider's capability.
            let using_oauth = stored
                .iter()
                .any(|(p, t)| p.as_str() == id.as_str() && *t == AuthType::Oauth);
            out.push(ProviderLoginInput {
                name: crate::provider_display_name(id.as_str()),
                status: cyrup_config::login::provider_auth_status(store, &id, None),
                id,
                auth,
                using_oauth,
            });
        }
        // Extension providers. pi's `getLoginProviderOptions` reads ONE provider list — the
        // composed registry, which holds a native extension's provider beside the built-ins
        // (`this.session.modelRuntime.getProviders()`, `interactive-mode.ts:4943-4947`;
        // `nativeExtensionProviders`, `core/model-runtime.ts:298` @v0.99.2-17) — so a live provider
        // that carries an `auth` strategy gets its row exactly like a built-in: an api-key strategy
        // with a `login` runs the normal prompt flow and the credential it returns is stored under
        // the provider's id. cyrup keeps the two sets apart, so they are joined here.
        //
        // A live provider REPLACES a built-in of the same id (pi's registry holds one provider per
        // id, and `registerProvider` "replaces all models"), and its status is its own strategy's
        // `check` (`getProviderAuthStatus`, `core/model-runtime.ts:428-437`) — the `env_keys` table
        // knows nothing of an id it does not list.
        for provider in session.live_extension_providers() {
            let Some(auth) = provider.provider_auth().cloned() else {
                continue;
            };
            let id = provider.id().clone();
            let using_oauth = stored
                .iter()
                .any(|(p, t)| p.as_str() == id.as_str() && *t == AuthType::Oauth);
            let status = cyrup_config::login::live_provider_auth_status(store, &provider).await;
            out.retain(|existing| existing.id.as_str() != id.as_str());
            out.push(ProviderLoginInput {
                // `provider.name` — an extension's provider carries its own display name, which
                // the id title-caser cannot derive (`llama.cpp`).
                name: provider.name().to_string(),
                status,
                id,
                auth,
                using_oauth,
            });
        }
        out
    }

    /// Refresh the cached stored-credential kinds ([`AppState::oauth_credential_providers`]) from
    /// the session's `AuthStore`, then recompute the footer's ` (sub)` marker.
    ///
    /// This is cyrup's stand-in for pi keeping `modelRuntime.snapshot.auth` warm: pi's footer reads
    /// the map synchronously on every repaint (`isUsingOAuth`, `model-runtime.ts:458-460`), cyrup
    /// reads `auth.json` once per credential-changing event and answers from the cache.
    ///
    /// A read failure leaves the previous snapshot alone rather than clearing it — an unreadable
    /// `auth.json` is not evidence that the user logged out, and blanking the set would make the
    /// marker flicker off on a transient error.
    pub async fn refresh_auth_snapshot(&mut self, session: &Arc<AgentSession>) {
        if let Ok(stored) = cyrup_config::login::stored_credentials(&session.services().auth).await
        {
            self.state.oauth_credential_providers = stored
                .into_iter()
                .filter(|(_, kind)| *kind == AuthType::Oauth)
                .map(|(id, _)| id.as_str().to_string())
                .collect();
        }
        // EXT-051: an extension provider's `oauth.isSubscription` is pi's `auth.oauth.isSubscription`
        // for that provider (`adaptOAuth`, `core/provider-composer.ts:276-279` @v0.87.1).
        let registry = session.services().ext_host.registry();
        self.state.extension_oauth_subscription = registry
            .provider_ids()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| {
                let reg = registry.provider_registration(&id).ok().flatten()?;
                reg.has_oauth().then(|| (id, reg.oauth_is_subscription()))
            })
            .collect();
        self.refresh_subscription_marker();
        // `available_model_catalog()` is auth-FILTERED (`cyrup-session-svc/src/session/model.rs:235-237`),
        // so a login or a logout changes which models `/model ` may offer. This is the seam every
        // credential change already funnels through, which is why the refresh hangs off it rather
        // than off each caller.
        self.refresh_argument_sources(session);
    }

    /// The provider registry the subscription predicate reads — pi's `this.models.getProvider(id)`
    /// (`model-runtime.ts:463`). Same source [`Self::build_login_inputs`] uses, so a test that
    /// substitutes the registry through [`Self::set_login_provider_source`] substitutes it here too.
    fn provider_oauth_strategy(
        &self,
        provider_id: &str,
    ) -> Option<Arc<dyn cyrup_provider::auth::OAuthAuth>> {
        let providers = match self.login_providers.as_deref() {
            Some(source) => source(),
            None => cyrup_provider::all_providers(),
        };
        providers
            .iter()
            .find(|p| p.id().as_str() == provider_id)
            .and_then(|p| p.provider_auth())
            .and_then(|auth| auth.oauth.clone())
    }

    /// The footer's `usingSubscription` predicate, verbatim from pi v0.84.1
    /// `coding-agent/src/modes/interactive/components/footer.ts:138-141`:
    ///
    /// ```text
    /// // Kimi Coding is subscription-backed despite using API-key authentication.
    /// const usingSubscription = state.model
    ///     ? state.model.provider === "kimi-coding" || this.session.modelRuntime.isUsingSubscription(state.model.provider)
    ///     : false;
    /// ```
    ///
    /// with `isUsingSubscription` expanded from `model-runtime.ts:462-464`:
    ///
    /// ```text
    /// isUsingSubscription(providerId) {
    ///     return this.isUsingOAuth(providerId) && this.models.getProvider(providerId)?.auth.oauth?.isSubscription === true;
    /// }
    /// ```
    ///
    /// **Both conjuncts are load-bearing.** `isUsingOAuth` alone — which is what pi itself called
    /// here until v0.84.0 (`v0.83.0:footer.ts:140`) — prints ` (sub)` for a metered OAuth sign-in
    /// such as OpenRouter; pi's v0.84.0 changelog records fixing exactly that (*"Fixed the footer
    /// showing `(sub)` for generic OAuth/OpenID sign-ins without a known subscription"*,
    /// `coding-agent/CHANGELOG.md:155`). And `isSubscription` alone would print ` (sub)` for an
    /// Anthropic user paying with `ANTHROPIC_API_KEY`, since that provider carries a subscription
    /// OAuth *strategy* whether or not the user signed in with it.
    ///
    /// The `kimi-coding` short-circuit is upstream's, not cyrup's: that provider is
    /// subscription-backed while authenticating with an API key, so neither conjunct can see it.
    fn provider_uses_subscription(&self, provider_id: &str) -> bool {
        if provider_id == KIMI_CODING_PROVIDER_ID {
            return true;
        }
        // An extension provider's own `oauth` block answers for its id — pi's `registerProvider`
        // hands the composed provider an `auth.oauth` adapted from it (`adaptOAuth`,
        // `core/provider-composer.ts:276-279` @v0.87.1), so `models.getProvider(id)` reads the
        // extension's `isSubscription` (EXT-051). Otherwise the built-in strategy's.
        let subscription = match self.state.extension_oauth_subscription.get(provider_id) {
            Some(is_subscription) => *is_subscription,
            None => self
                .provider_oauth_strategy(provider_id)
                .is_some_and(|oauth| oauth.is_subscription()),
        };
        self.state.oauth_credential_providers.contains(provider_id) && subscription
    }

    /// Recompute the footer's ` (sub)` marker for the currently-active provider. pi has no such
    /// method because its footer recomputes the flag on every repaint; cyrup's [`StatusLine`] is a
    /// value struct, so the flag is pushed whenever either of its two inputs moves — the active
    /// provider (`ModelChanged`) or the stored credentials ([`Self::refresh_auth_snapshot`]).
    ///
    /// No active provider ⇒ `false`, which is pi's `state.model ? … : false` (`footer.ts:139-141`).
    pub(crate) fn refresh_subscription_marker(&mut self) {
        let sub = self
            .state
            .status
            .provider
            .clone()
            .is_some_and(|p| self.provider_uses_subscription(&p));
        self.state.status.set_using_subscription(sub);
    }

    /// The accumulated body text of the open `/login` dialog (`None` when no dialog is open) —
    /// test/inspection access to what the flow has drawn so far, the same role
    /// [`Self::active_selector_kind`] plays for the slot itself.
    pub fn login_dialog_body(&mut self) -> Option<String> {
        self.login_dialog_mut().map(|d| d.body_text())
    }

    /// The open `/login` dialog's title (`` `Login to ${providerName}` ``), for the same reason.
    pub fn login_dialog_title(&mut self) -> Option<String> {
        self.login_dialog_mut().map(|d| d.title().to_string())
    }

    /// The `/login` dialog currently in the input slot, if any.
    fn login_dialog_mut(&mut self) -> Option<&mut LoginDialog> {
        self.state
            .selector
            .as_mut()
            .filter(|s| s.kind == SelectorKind::LoginDialog)
            .and_then(|s| s.inner.as_login_dialog())
    }

    /// `handleLoginCommand(providerRef?)` (`interactive-mode.ts:4994-5026`), routed through the
    /// ported [`cyrup_config::login::resolve_login_command`].
    pub(crate) async fn handle_login_command(
        &mut self,
        session: &Arc<AgentSession>,
        arg: Option<String>,
    ) {
        let inputs = self.login_provider_inputs(session).await;
        let options = cyrup_config::login::login_provider_options(&inputs, None);
        match cyrup_config::login::resolve_login_command(arg.as_deref(), &options) {
            // `startProviderLogin(providerOptions[0])` (`:5000-5003`).
            LoginCommand::Start(option) => self.begin_provider_login(session, *option),
            // `showLoginAuthTypeSelector(providerOptions?)` (`:4997`, `:5010`).
            LoginCommand::AuthTypeSelector { options } => {
                self.open_login_auth_type_selector(session, options)
            }
            // `showLoginProviderSelector(undefined, providerRef)` (`:5013`).
            LoginCommand::ProviderSelector {
                auth_type,
                initial_search,
            } => self.open_login_provider_selector(&inputs, auth_type, initial_search),
        }
    }

    /// `showLoginAuthTypeSelector(providerOptions?)` (`interactive-mode.ts:5028-5051`), routed
    /// through the ported [`cyrup_config::login::resolve_auth_type_selector`].
    fn open_login_auth_type_selector(
        &mut self,
        session: &Arc<AgentSession>,
        options: Option<Vec<LoginProviderOption>>,
    ) {
        match cyrup_config::login::resolve_auth_type_selector(options.as_deref()) {
            // `showStatus("No login methods available.")` (`:5046`).
            cyrup_config::login::AuthTypeSelector::Unavailable => {
                self.state
                    .transcript
                    .push_status(cyrup_config::login::NO_LOGIN_METHODS);
            }
            // One provider, one method: the selector is skipped entirely (`:5049-5055`).
            cyrup_config::login::AuthTypeSelector::Start(option) => {
                self.state.login_auth_type_options = None;
                self.begin_provider_login(session, *option);
            }
            cyrup_config::login::AuthTypeSelector::Choose {
                title,
                subscription_label,
                api_key_label,
            } => {
                // `options` in Pi's order: the subscription label first (`:5036-5041`).
                let mut rows: Vec<(String, String, Option<String>)> = Vec::new();
                if let Some(label) = subscription_label {
                    rows.push((AuthType::Oauth.as_str().to_string(), label, None));
                }
                if let Some(label) = api_key_label {
                    rows.push((AuthType::ApiKey.as_str().to_string(), label, None));
                }
                self.state.login_auth_type_options = options;
                self.open_data_selector(SelectorKind::LoginAuthType, rows, 0);
                if let Some(active) = self.state.selector.as_mut() {
                    active.inner.set_title(title);
                }
            }
        }
    }

    /// `showLoginProviderSelector(authType?, initialSearchInput?)`
    /// (`interactive-mode.ts:5085-5124`): the options narrowed to `auth_type`, or the empty-state
    /// status when nothing qualifies.
    pub(crate) fn open_login_provider_selector(
        &mut self,
        inputs: &[ProviderLoginInput],
        auth_type: Option<AuthType>,
        initial_search: Option<String>,
    ) {
        let options = cyrup_config::login::login_provider_options(inputs, auth_type);
        if options.is_empty() {
            self.state.transcript.push_status(
                cyrup_config::login::provider_selector_empty_message(auth_type),
            );
            return;
        }
        // S5/S21: the real `OAuthSelectorComponent` (`oauth-selector.ts`) — search `Input`, fuzzy
        // filter, coloured status runs — in place of the bare `ListSelector`. `initialSearchInput`
        // (`:5124`) now lands where upstream puts it: seeded into the search box (`:99`), not
        // reported as a status line.
        let selector = crate::OAuthSelector::new(crate::OAuthMode::Login, &options, initial_search);
        self.state.login_options = options;
        self.open_boxed_selector(SelectorKind::Login, Box::new(selector));
    }

    /// `startProviderLogin(providerOption)` (`interactive-mode.ts:5017-5025`), routed through the
    /// ported [`cyrup_config::login::start_provider_login`].
    ///
    /// The OAuth and API-key legs are the SAME code here: both open the dialog and spawn
    /// `cyrup_config::login::login` with the matching [`AuthType`]. Upstream splits them into
    /// `showLoginDialog` / `showApiKeyLoginDialog` only because of two cosmetic differences — the
    /// amazon-bedrock `showDetails` block (`:5266-5272`; that provider is unported, see
    /// `providers/all.rs`) and the failure-message wording, which [`LoginFinished::oauth`] carries.
    pub(crate) fn begin_provider_login(
        &mut self,
        session: &Arc<AgentSession>,
        option: LoginProviderOption,
    ) {
        match cyrup_config::login::start_provider_login(&option) {
            // `showAmbientAuthDialog(providerOption)` (`:5023`, `:5229-5250`): a dialog with a
            // single info line and a close hint. Nothing to run, so no task is spawned.
            LoginStep::Ambient { title, message, .. } => {
                self.open_login_dialog(title);
                if let Some(dialog) = self.login_dialog_mut() {
                    dialog.show_info(&message, &[], true);
                }
            }
            LoginStep::Oauth { id, name } | LoginStep::ApiKey { id, name } => {
                let oauth = option.auth_type == AuthType::Oauth;
                let Some(tx) = self.login_tx.clone() else {
                    // No run loop is servicing the channel — refuse rather than spawn a task whose
                    // first prompt can never be answered.
                    self.state
                        .transcript
                        .push_status("login unavailable: no interactive session");
                    return;
                };
                // `new LoginDialogComponent(ui, providerId, …, providerName)` → title
                // `` `Login to ${providerName}` `` (`login-dialog.ts:41`).
                self.open_login_dialog(format!("Login to {name}"));
                // `dialog.signal` — the dialog's own AbortController (`login-dialog.ts:73-75`).
                let cancel = CancelToken::new();
                self.state.login_cancel = Some(cancel.clone());
                let auth_type = option.auth_type;
                // **TUI-105.** `const previousModel = this.session.model` (`:6059`, `:6195`) —
                // captured HERE, at the call site, before the dialog runs. `finish_login` must not
                // re-read it: a `/model` issued while the browser tab was open would then look like
                // the state the login started in, and pi's `session.model === previousModel` guard
                // (`:6016`) exists to respect exactly that.
                self.state.login_previous_model = session.model();
                let store = Arc::clone(&session.services().auth);
                // `getAuthPath()` (`env.rs:236-238`): the path the success status names.
                let auth_path = session.services().agent_dir.join("auth.json");
                let session = Arc::clone(session);
                let login_providers = self.login_providers.clone();
                tokio::spawn(async move {
                    let inputs =
                        Self::build_login_inputs(&session, login_providers.as_deref()).await;
                    let interaction = TuiAuthInteraction::new(tx.clone(), cancel);
                    // `await this.session.modelRuntime.login(providerId, method, {…})`
                    // (`interactive-mode.ts:5368`) — `Models.login` persists into the credential
                    // store itself, so there is no separate write here.
                    let result =
                        cyrup_config::login::login(&*store, &inputs, &id, auth_type, &interaction)
                            .await;
                    let finished = match result {
                        Ok(_) => LoginFinished {
                            provider_id: id.as_str().to_string(),
                            provider_name: name,
                            oauth,
                            result: Ok(()),
                            cancelled: false,
                            auth_path,
                        },
                        Err(e) => LoginFinished {
                            provider_id: id.as_str().to_string(),
                            provider_name: name,
                            oauth,
                            cancelled: e.is_cancelled(),
                            result: Err(e.to_string()),
                            auth_path,
                        },
                    };
                    let _ = tx.send(LoginUiMsg::Finished(Box::new(finished)));
                });
            }
        }
    }

    /// Put a fresh [`LoginDialog`] in the input slot (`editorContainer.clear(); addChild(dialog);
    /// setFocus(dialog)`, `interactive-mode.ts:5273-5276`). The hint text is taken from the LIVE
    /// `tui.select.*` bindings, matching Pi's `keyHint` (`login-dialog.ts:141`, `:164`).
    fn open_login_dialog(&mut self, title: impl Into<String>) {
        let dialog = LoginDialog::new(title, &self.state.select_keymap);
        self.open_boxed_selector(SelectorKind::LoginDialog, Box::new(dialog));
    }

    /// Apply one message from the spawned login flow (`notifyAuthDialog` / `showAuthPrompt` /
    /// the `try`/`catch` around `loginProvider`, `interactive-mode.ts:5285-5296`, `:5327-5360`,
    /// `:5392-5403`).
    ///
    /// `pub` for the same reason as [`Self::apply_tree_nav_outcome`]: `tests/*.rs` drives the
    /// settle half without a live run loop.
    pub async fn apply_login_msg(&mut self, session: &Arc<AgentSession>, msg: LoginUiMsg) {
        match msg {
            LoginUiMsg::Notify(event) => {
                if let Some(dialog) = self.login_dialog_mut() {
                    notify_auth_dialog(dialog, *event);
                }
            }
            LoginUiMsg::Prompt { prompt, reply } => {
                let Some(dialog) = self.login_dialog_mut() else {
                    // The dialog is already gone (cancelled, or the flow raced the teardown):
                    // reject exactly as `cancel()` does (`login-dialog.ts:82-88`).
                    let _ = reply.send(Err(OAuthError::Cancelled));
                    return;
                };
                show_auth_prompt(dialog, &prompt);
                // A previous prompt still pending would be a flow bug, but resolving it as
                // cancelled is strictly better than leaking the sender (which would hang the flow).
                if let Some(stale) = self.state.pending_login_prompt.replace(reply) {
                    let _ = stale.send(Err(OAuthError::Cancelled));
                }
            }
            LoginUiMsg::Finished(finished) => self.finish_login(session, *finished).await,
        }
    }

    /// The `try`/`catch` tail of `showLoginDialog` / `showApiKeyLoginDialog`
    /// (`interactive-mode.ts:5285-5296`, `:5392-5403`): restore the editor, then either the
    /// success status (`completeProviderAuthentication`, `:5176-5227`) or the error banner — and
    /// NOTHING at all when the user cancelled, which is what `errorMsg !== "Login cancelled"`
    /// buys (`:5294`, `:5401`).
    async fn finish_login(&mut self, session: &Arc<AgentSession>, finished: LoginFinished) {
        // `restoreEditor()` (`:5276-5281`).
        if self.active_selector_kind() == Some(SelectorKind::LoginDialog) {
            self.close_selector(true);
        }
        if let Some(reply) = self.state.pending_login_prompt.take() {
            let _ = reply.send(Err(OAuthError::Cancelled));
        }
        self.state.login_cancel = None;
        let name = &finished.provider_name;
        match &finished.result {
            Ok(()) => {
                // The credential the flow just persisted IS the auth snapshot change pi's
                // `completeProviderAuthentication` follows with `this.footer.invalidate()`
                // (`interactive-mode.ts:5448-5449`), which re-answers `usingSubscription` off the
                // now-current `snapshot.auth`. Apply the same delta to the cached map and repaint
                // the marker, so signing in to a Pro/Max plan lights ` (sub)` on the very next
                // frame instead of only after a restart.
                if finished.oauth {
                    self.state
                        .oauth_credential_providers
                        .insert(finished.provider_id.clone());
                } else {
                    // An API-key login REPLACES any stored OAuth credential for that provider
                    // (`auth.json` holds one credential per provider), so the OAuth half of the
                    // snapshot must drop it — otherwise switching Anthropic from Pro/Max to a
                    // metered key would keep the ` (sub)` marker on a metered account.
                    self.state
                        .oauth_credential_providers
                        .remove(&finished.provider_id);
                }
                self.refresh_subscription_marker();
                // `actionLabel` (`:5940`): `Logged in to {name}` | `Saved API key for {name}`.
                let action = if finished.oauth {
                    format!("Logged in to {name}")
                } else {
                    format!("Saved API key for {name}")
                };
                let provider_id = finished.provider_id.clone();
                let auth_path = finished.auth_path.clone();
                let previous_model = self.state.login_previous_model.clone();
                // `deferSelection` (`:5944-5949`) — "Dynamic catalogs may be empty until the first
                // authenticated network refresh" (`:5943`): the credential exists now, but the
                // provider's models may only arrive with the refresh below, so selecting the default
                // this instant would report it missing.
                let default_id = cyrup_config::default_model_per_provider(&provider_id);
                let defer = previous_model.is_none()
                    && default_id.is_some()
                    && !session.available_model_catalog().iter().any(|m| {
                        m.provider.as_str() == provider_id && Some(m.id.as_str()) == default_id
                    });
                if defer {
                    // `` `${actionLabel}. Credentials saved to ${getAuthPath()}. Refreshing model
                    // catalog…` `` (`:6000`) — the ONLY line emitted on this path; the selection and
                    // its status come from the refresh continuation.
                    let path = auth_path.display();
                    self.state.transcript.push_status(format!(
                        "{action}. Credentials saved to {path}. Refreshing model catalog…"
                    ));
                } else {
                    // `await finishAuthentication()` (`:6002`).
                    self.finish_provider_authentication(
                        session,
                        &action,
                        &provider_id,
                        &auth_path,
                        previous_model.is_none(),
                    )
                    .await;
                }
                self.begin_post_login_catalog_refresh(
                    session,
                    action,
                    provider_id,
                    auth_path,
                    defer,
                    previous_model,
                );
            }
            // `if (errorMsg !== "Login cancelled")` (`:5294`, `:5401`) — a cancel is silent.
            Err(_) if finished.cancelled => {}
            Err(message) => {
                let banner = if finished.oauth {
                    format!("Failed to login to {name}: {message}")
                } else {
                    format!("Failed to save API key for {name}: {message}")
                };
                self.state.transcript.push_error(banner);
            }
        }
    }

    /// `dialog.cancel()` (`login-dialog.ts:82-88`): abort the flow's signal AND reject the prompt it
    /// is blocked on with `"Login cancelled"`. Called from the selector `Cancel` arm.
    pub(crate) fn cancel_login(&mut self) {
        if let Some(reply) = self.state.pending_login_prompt.take() {
            let _ = reply.send(Err(OAuthError::Cancelled));
        }
        if let Some(cancel) = self.state.login_cancel.take() {
            cancel.cancel();
        }
    }
}

impl<B: Backend> App<B> {
    /// Recompute the footer's available-provider count — pi `updateAvailableProviderCount`
    /// (`interactive-mode.ts:5092-5099`) verbatim: the SCOPED set when one is configured, otherwise
    /// the auth-filtered snapshot, deduped by provider.
    ///
    /// This is the SOLE producer of `StatusLine::set_provider_count`, which is what drives the
    /// footer's `(provider)` prefix gate (`status.rs:597` ← `footer.ts:192-193`). Upstream is built
    /// the same way: every recount in `interactive-mode.ts` — boot, the session rebind, the startup
    /// refresh, each login/logout and each model change — goes through this one method, so boot is
    /// not a second implementation but the same call. pi's `init()` reaches it twice, once via
    /// `rebindCurrentSession` (`:1033` → its tail at `:2042`) and once explicitly at `:1051`;
    /// cyrup's boot seed reaches it from `cyrup/src/interactive.rs::seed_model_footer`.
    ///
    /// Takes `&AgentSession` rather than `&Arc<AgentSession>` so the boot seed — which holds a bare
    /// `&AgentSession`, not the `Arc` — can call it; the in-app call sites pass their `Arc` unchanged
    /// through deref coercion.
    pub fn refresh_provider_count(&mut self, session: &AgentSession) {
        let scoped = session.scoped_models();
        // `session.scopedModels.length > 0 ? … : getAvailableSnapshot()` (`:5094-5096`).
        let providers: std::collections::BTreeSet<String> = if scoped.is_empty() {
            session
                .available_model_catalog()
                .iter()
                .map(|m| m.provider.as_str().to_string())
                .collect()
        } else {
            scoped
                .iter()
                .map(|s| s.model.provider.as_str().to_string())
                .collect()
        };
        self.status_mut().set_provider_count(providers.len());
    }

    /// **Citations in this block and in [`DefaultModelFailure`] / [`default_model_selection`] are
    /// all against ONE revision of `interactive-mode.ts`: pi `@v0.99.2-17` (commit `70c036211`, see
    /// the `cyrup-llama` crate docs), where the llama.cpp rung (`:5957-5958`) already exists, so the
    /// rungs that follow it are cited at the lines they have after it. Other blocks of this file
    /// predate it and cite `@v0.87.1`.**
    ///
    /// **TUI-105.** `finishAuthentication` (`interactive-mode.ts:5950-5998`): select the provider's
    /// curated default model when the session had none, recount providers, then report.
    ///
    /// Returns the selected model id, which is what pi's `if (selectedModel)` (`:5986`) branches on.
    ///
    /// Called twice on the deferred path — once from [`Self::finish_login`] when the default is
    /// already in the cached catalog, and once from [`Self::apply_login_refresh`] when it was not —
    /// which is exactly upstream's closure being invoked from its two sites (`:6002`, `:6017`).
    async fn finish_provider_authentication(
        &mut self,
        session: &Arc<AgentSession>,
        action: &str,
        provider_id: &str,
        auth_path: &std::path::Path,
        previous_model_was_unknown: bool,
    ) -> Option<cyrup_core::ModelId> {
        let mut selected: Option<cyrup_core::ModelId> = None;
        let mut selection_error: Option<String> = None;
        // `if (isUnknownModel(previousModel))` (`:5953`) — a session that ALREADY has a model keeps
        // it. Without this gate the fix would clobber a model the user chose before logging in.
        if previous_model_was_unknown {
            // `getAvailableSnapshot().filter(model => model.provider === providerId)` (`:5954-5955`).
            let provider_models: Vec<cyrup_provider::Model> = session
                .available_model_catalog()
                .into_iter()
                .filter(|m| m.provider.as_str() == provider_id)
                .collect();
            match default_model_selection(provider_id, &provider_models) {
                Ok(model) => {
                    let id = model.id.clone();
                    // `await this.session.setModel(selectedModel, { persist: true })` (`:5973`).
                    match session.set_model_resolved(model).await {
                        Ok(_) => selected = Some(id),
                        // `catch` → `selectedModel = undefined` + the fourth message (`:5974-5977`).
                        Err(e) => {
                            selection_error = Some(
                                DefaultModelFailure::SetModelFailed(e.to_string())
                                    .message(action, provider_id),
                            );
                        }
                    }
                }
                Err(failure) => selection_error = Some(failure.message(action, provider_id)),
            }
        }
        // `await this.updateAvailableProviderCount(); this.footer.invalidate();` — unconditional, on
        // both the selected and the errored paths (`:5983-5985`).
        self.refresh_provider_count(session);
        self.refresh_subscription_marker();
        let path = auth_path.display();
        match &selected {
            // `` `${actionLabel}. Selected ${selectedModel.id}. Credentials saved to ${getAuthPath()}` ``
            // (`:5987`).
            Some(id) => {
                let id = id.as_str();
                self.state.transcript.push_status(format!(
                    "{action}. Selected {id}. Credentials saved to {path}"
                ));
            }
            // `` `${actionLabel}. Credentials saved to ${getAuthPath()}` `` + `showError(selectionError)`
            // (`:5990-5996`).
            None => {
                self.state
                    .transcript
                    .push_status(format!("{action}. Credentials saved to {path}"));
                if let Some(message) = selection_error {
                    self.state.transcript.push_error(message);
                }
            }
        }
        selected
    }

    /// Spawn the post-login catalog refresh — pi's `AbortController` + 15 s `setTimeout` +
    /// `session.modelRuntime.refresh(...)` tail (`interactive-mode.ts:6006-6028`).
    ///
    /// Shaped on [`Self::begin_model_catalog_refresh`] (`app/selectors.rs:348-389`) because it is the
    /// same upstream pattern: one [`CancelToken`] serving as the deadline, the cancel and the
    /// `clearTimeout`, and a worker task posting the settled result back to the run loop.
    ///
    /// SCOPED to the provider just authenticated, which is upstream's own shape: pi calls
    /// `session.modelRuntime.refresh({ providers: [providerId], signal: controller.signal })`
    /// DIRECTLY (`:6008`), under the `AbortController` (`:6005`) and 15 s `setTimeout` (`:6006`).
    /// It deliberately does NOT route this through `refreshModelCatalogs`
    /// (`modes/interactive/model-catalog-refresh.ts:46-51`), the shared coordinator it reserves for
    /// WHOLE-catalog refreshes (`/model`, and `run()`'s startup refresh at `:1121`) — because a
    /// coordinated call may JOIN an in-flight operation, and a joined operation's provider list wins,
    /// which would discard this caller's scope outright. Hence
    /// [`cyrup_session_svc::AgentSession::refresh_provider_catalog`] rather than its whole-catalog
    /// neighbour `refresh_model_catalogs`: the whole 15 s budget is spent on the one provider, and a
    /// slow unrelated provider cannot eat it. The fetch list drops `radius` inside that call, per the
    /// fetch/overlay split `cyrup_provider::refresh_and_install` documents.
    ///
    /// A provider a native extension registered live (`llama.cpp`) is refreshed through the same
    /// call: pi holds it in the one composed collection, so `refresh({ providers: [providerId] })`
    /// reaches its `refreshModels` (`models.ts:546-606`), and
    /// [`cyrup_session_svc::AgentSession::refresh_provider_catalog`] routes it to the guest registry's
    /// engine. The models it publishes are what the continuation below counts and lists.
    fn begin_post_login_catalog_refresh(
        &mut self,
        session: &Arc<AgentSession>,
        action: String,
        provider_id: String,
        auth_path: std::path::PathBuf,
        defer: bool,
        previous_model: Option<cyrup_core::ModelRef>,
    ) {
        let Some(tx) = self.login_refresh_tx.clone() else {
            // No run loop servicing the channel (an embedder, a widget test): the login has already
            // reported and, on the non-deferred path, already selected from the cached catalog.
            return;
        };
        self.state.login_refresh_epoch = self.state.login_refresh_epoch.wrapping_add(1);
        let epoch = self.state.login_refresh_epoch;
        let cancel = CancelToken::new();
        self.state.login_refresh_cancel = Some(cancel.clone());
        // `setTimeout(() => controller.abort(), 15_000)` (`:6006`). The second arm is upstream's
        // `finally { clearTimeout(timeout) }` (`:6028`): once the refresh settles this task exits
        // instead of holding a timer for the full budget.
        let deadline = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(crate::MODEL_REFRESH_TIMEOUT) => deadline.cancel(),
                () = deadline.cancelled() => {}
            }
        });
        let session = Arc::clone(session);
        tokio::spawn(async move {
            let result = session
                .refresh_provider_catalog(cancel.clone(), &provider_id)
                .await;
            cancel.cancel();
            let _ = tx.send(crate::login_dialog::LoginRefreshMsg {
                epoch,
                provider_id,
                action,
                auth_path,
                defer,
                previous_model,
                result,
            });
        });
    }

    /// A settled post-login catalog refresh — pi's `.then` continuation
    /// (`interactive-mode.ts:6011-6026`), in upstream's exact order: the two warnings, the deferred
    /// selection under its guard, then the count and the repaint.
    ///
    /// `pub` for the same reason [`Self::apply_login_msg`] is: `tests/login_flow.rs` drives the
    /// settle half without a live run loop.
    pub async fn apply_login_refresh(
        &mut self,
        session: &Arc<AgentSession>,
        msg: crate::login_dialog::LoginRefreshMsg,
    ) {
        // pi's `this.session === session` identity check (`:6016`), expressed as the epoch guard
        // `apply_model_refresh` already uses (`app/selectors.rs:406`): a refresh belonging to a login
        // that has since been superseded must not select on the current one's behalf.
        if msg.epoch != self.state.login_refresh_epoch {
            return;
        }
        self.state.login_refresh_cancel = None;
        let action = &msg.action;
        if msg.result.aborted || msg.result.timed_out {
            // `` `${actionLabel}, but its model catalog refresh timed out; using cached models.` ``
            // (`:6012`). cyrup's coordinator distinguishes THIS caller's token firing (`timed_out`)
            // from the shared operation aborting (`aborted`, `catalog_refresh.rs:90-95`); pi has only
            // the one flag, and both are its `result.aborted`.
            self.state.transcript.show_warning(format!(
                "{action}, but its model catalog refresh timed out; using cached models."
            ));
        } else if !msg.result.errors.is_empty() {
            // `` `${actionLabel}, but its model catalog could not be refreshed; using cached models.` ``
            // (`:6014`).
            self.state.transcript.show_warning(format!(
                "{action}, but its model catalog could not be refreshed; using cached models."
            ));
        }
        // "Do not replace a model or session selected while the refresh was running" (`:6015-6018`):
        // a `/model` issued while the refresh was in flight WINS, which is why `previous_model` is the
        // value captured before the login rather than a re-read.
        if msg.defer && session.model() == msg.previous_model {
            self.finish_provider_authentication(
                session,
                action,
                &msg.provider_id,
                &msg.auth_path,
                msg.previous_model.is_none(),
            )
            .await;
        }
        // `this.updateAvailableProviderCount(); this.footer.invalidate(); this.ui.requestRender();`
        // (`:6019-6021`) — unconditional, so a refresh that installed a new provider's catalog moves
        // the footer even when no selection happened.
        self.refresh_provider_count(session);
        self.refresh_subscription_marker();
        self.frames.request();
    }
}

/// `"llama.cpp"` — matches `LLAMA_PROVIDER_ID` from `extensions/llama/provider.ts`, kept inline to
/// avoid coupling interactive mode to the built-in extension (`interactive-mode.ts:5956`).
const LLAMA_CPP_PROVIDER_ID: &str = "llama.cpp";

/// Why no default model could be selected after a login — pi's `selectionError` ladder
/// (`interactive-mode.ts:5957-5958` and the rungs after it), as a value so the messages are one table
/// the tests compare byte-for-byte and so each rung is reachable without a live provider catalog.
///
/// The FIRST rung is `providerId === "llama.cpp"` → `llamaCppPostLoginGuidance(actionLabel,
/// providerModels.length)` (`:5957-5958`, the helper at `:349-353`). It is a rung, not a special
/// case after the others: it runs before `hasDefaultModelProvider` is consulted, so a provider with
/// no curated default and zero models still gets the llama.cpp text rather than the generic one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefaultModelFailure {
    /// `providerId === "llama.cpp"` (`:5957-5958`): no default model is selected for it; the login
    /// ends in `llamaCppPostLoginGuidance(actionLabel, providerModels.length)` (`:349-353`).
    /// Carries `providerModels.length`, the only input the helper reads.
    LlamaCppGuidance(usize),
    NoDefaultConfigured,
    /// `providerModels.length === 0` (`:5961-5962`).
    NoModelsAvailable,
    /// The default id is configured but absent from the provider's catalog (`:5969-5970`).
    DefaultNotAvailable(&'static str),
    /// `setModel` threw (`:5974-5977`).
    ///
    /// `[CYRUP-DELTA]` pi's `setModel(selectedModel, { persist: true })` (`:5973`) maps to
    /// [`cyrup_session_svc::AgentSession::set_model_resolved`]
    /// (`cyrup-session-svc/src/session/model.rs:43`), which appends the `model_change` entry itself —
    /// cyrup has no separate persist flag.
    SetModelFailed(String),
}

impl DefaultModelFailure {
    /// Pi's five `selectionError` templates, byte-for-byte (`interactive-mode.ts:349-353` for
    /// the llama.cpp pair, then `:5960`, `:5962`, `:5970`, `:5977`). Every one of them points at
    /// `/model` (the llama.cpp pair also at `/llama`), which is what makes the failure actionable
    /// rather than a dead end.
    pub(crate) fn message(&self, action: &str, provider_id: &str) -> String {
        match self {
            // `llamaCppPostLoginGuidance` (`:349-353`): zero loaded models vs. at least one.
            Self::LlamaCppGuidance(0) => format!(
                "{action}. No llama.cpp models are loaded. Use /llama to load a model, then /model to select it."
            ),
            Self::LlamaCppGuidance(_) => format!(
                "{action}. Use /model to select a loaded llama.cpp model, or /llama to manage models."
            ),
            Self::NoDefaultConfigured => format!(
                "{action}, but no default model is configured for provider \"{provider_id}\". Use /model to select a model."
            ),
            Self::NoModelsAvailable => format!(
                "{action}, but no models are available for that provider. Use /model to select a model."
            ),
            Self::DefaultNotAvailable(id) => format!(
                "{action}, but its default model \"{id}\" is not available. Use /model to select a model."
            ),
            Self::SetModelFailed(error) => format!(
                "{action}, but selecting its default model failed: {error}. Use /model to select a model."
            ),
        }
    }
}

/// Pick the model a fresh login selects — pi's `selectedModel` expression (`:5966-5970`) with its
/// guard rungs (`:5957-5958`, then `:5959-5962`) folded in front.
///
/// The `radius` special case is upstream's, comment included: "Radius catalogs vary by account;
/// prefer balanced, then use catalog order" (`:5965`), i.e. the default id first and the first
/// catalog entry as that one provider's fallback.
pub(crate) fn default_model_selection(
    provider_id: &str,
    provider_models: &[cyrup_provider::Model],
) -> Result<cyrup_provider::Model, DefaultModelFailure> {
    // `if (providerId === "llama.cpp")` (`:5957-5958`), ahead of every other rung. The comment at
    // `:5956` — "Matches LLAMA_PROVIDER_ID from extensions/llama/provider.ts; kept inline to avoid
    // coupling interactive mode to the built-in extension" — applies unchanged: the id is spelled
    // here rather than imported from the `cyrup-llama` crate.
    if provider_id == LLAMA_CPP_PROVIDER_ID {
        return Err(DefaultModelFailure::LlamaCppGuidance(provider_models.len()));
    }
    let Some(default_id) = cyrup_config::default_model_per_provider(provider_id) else {
        return Err(DefaultModelFailure::NoDefaultConfigured);
    };
    if provider_models.is_empty() {
        return Err(DefaultModelFailure::NoModelsAvailable);
    }
    provider_models
        .iter()
        .find(|m| m.id.as_str() == default_id)
        .or_else(|| {
            (provider_id == "radius")
                .then(|| provider_models.first())
                .flatten()
        })
        .cloned()
        .ok_or(DefaultModelFailure::DefaultNotAvailable(default_id))
}
