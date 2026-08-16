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

    /// `this.session.modelRuntime.getProviders()` + `getProviderAuthStatus` + `isUsingOAuth`, the
    /// three registry reads `getLoginProviderOptions` folds together
    /// (`interactive-mode.ts:4943-4947`).
    ///
    /// Pi's `Provider` interface carries `name`; cyrup's does not (the display name lives on the
    /// concrete `WireProvider`), so the name comes from [`crate::provider_display_name`] — the same
    /// `getProviderDisplayName` fallback the picker already used for its labels.
    pub(crate) async fn login_provider_inputs(&self, session: &Arc<AgentSession>) -> Vec<ProviderLoginInput> {
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
        self.refresh_subscription_marker();
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
        self.state.oauth_credential_providers.contains(provider_id)
            && self
                .provider_oauth_strategy(provider_id)
                .is_some_and(|oauth| oauth.is_subscription())
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
    pub(crate) async fn handle_login_command(&mut self, session: &Arc<AgentSession>, arg: Option<String>) {
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
            self.state
                .transcript
                .push_status(cyrup_config::login::provider_selector_empty_message(auth_type));
            return;
        }
        // S5/S21: the real `OAuthSelectorComponent` (`oauth-selector.ts`) — search `Input`, fuzzy
        // filter, coloured status runs — in place of the bare `ListSelector`. `initialSearchInput`
        // (`:5124`) now lands where upstream puts it: seeded into the search box (`:99`), not
        // reported as a status line.
        let selector =
            crate::OAuthSelector::new(crate::OAuthMode::Login, &options, initial_search);
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
    pub(crate) fn begin_provider_login(&mut self, session: &Arc<AgentSession>, option: LoginProviderOption) {
        match cyrup_config::login::start_provider_login(&option) {
            // `showAmbientAuthDialog(providerOption)` (`:5023`, `:5229-5250`): a dialog with a
            // single info line and a close hint. Nothing to run, so no task is spawned.
            LoginStep::Ambient {
                title, message, ..
            } => {
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
                    let result = cyrup_config::login::login(
                        &*store,
                        &inputs,
                        &id,
                        auth_type,
                        &interaction,
                    )
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
    pub fn apply_login_msg(&mut self, msg: LoginUiMsg) {
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
            LoginUiMsg::Finished(finished) => self.finish_login(*finished),
        }
    }

    /// The `try`/`catch` tail of `showLoginDialog` / `showApiKeyLoginDialog`
    /// (`interactive-mode.ts:5285-5296`, `:5392-5403`): restore the editor, then either the
    /// success status (`completeProviderAuthentication`, `:5176-5227`) or the error banner — and
    /// NOTHING at all when the user cancelled, which is what `errorMsg !== "Login cancelled"`
    /// buys (`:5294`, `:5401`).
    fn finish_login(&mut self, finished: LoginFinished) {
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
                // `actionLabel` (`:5183`) + `` `${actionLabel}. Credentials saved to ${getAuthPath()}` ``
                // (`:5219`). `getAuthPath()` is `<agent_dir>/auth.json` (`env.rs:236`).
                let action = if finished.oauth {
                    format!("Logged in to {name}")
                } else {
                    format!("Saved API key for {name}")
                };
                let path = finished.auth_path.display();
                self.state
                    .transcript
                    .push_status(format!("{action}. Credentials saved to {path}"));
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
