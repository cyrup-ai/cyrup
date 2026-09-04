use super::*;

impl<B: Backend> App<B> {
    /// Open an editor-swap selector (spec/tui/05 §1.1 `showSelector`): snapshot the editor text, build
    /// the selector for `kind`, and put it in the input slot. The theme picker also stashes the live
    /// theme so a cancel can restore it. Idempotent-ish: opening replaces any already-open selector.
    pub fn open_selector(&mut self, kind: SelectorKind) {
        let saved_editor = self.state.editor.text();
        let (inner, restore_theme) = self.build_list_selector(kind);
        self.state.selector = Some(ActiveSelector {
            kind,
            inner,
            saved_editor,
            restore_theme,
            parent: None,
        });
    }

    /// Build (but do not mount) the dependency-free list selector for `kind`, plus the theme to
    /// restore if it is cancelled. Split out of [`Self::open_selector`] so the same construction
    /// serves both the top-level open and the `/settings` submenu open
    /// ([`Self::open_child_selector`]), which differ only in what they do with the slot.
    fn build_list_selector(&self, kind: SelectorKind) -> (Box<dyn Selector>, Option<UiTheme>) {
        // `with_upstream_chrome` applies the hint row / one-column inset ONLY for the kinds whose
        // pi component builds them (`SelectorKind::draws_hint_row` / `insets_rows`). Show-images and
        // theme are `DynamicBorder` + `SelectList` + `DynamicBorder` upstream and get neither.
        match kind {
            // NOT a `ListSelector`: `ThinkingSelectorComponent` is a titled dialog with a
            // cycle-key sentence, a search `Input` and a dim footer (`thinking-selector.ts:77-97`
            // @0.84.3), so it has its own component. The ladder is the SESSION's
            // (`interactive-mode.ts:4792` `getAvailableThinkingLevels()`), not a hardcoded seven.
            SelectorKind::Thinking => (
                Box::new(crate::thinking_selector::ThinkingSelector::new(
                    &self.state.available_thinking_levels,
                    &self.state.thinking_level,
                    &self.state.default_thinking_level,
                    // `keyDisplayText("app.thinking.cycle")` (`thinking-selector.ts:81`) =
                    // `keyText(..., {capitalize:true})` (`keybinding-hints.ts:37-39`), read from
                    // the app's LIVE table so a rebind changes the sentence — the same idiom the
                    // `/settings` thinking row uses (`settings_rows.rs`).
                    self.state
                        .keymap
                        .keys_label(Action::ThinkingCycle)
                        .map(|k| crate::chrome::format_key_text(&k, true))
                        .unwrap_or_default(),
                    // The live flag and the live `app.thinking.toggle` label, read the same way
                    // and for the same reason as the cycle key above.
                    self.state.transcript.hide_thinking_block(),
                    self.state
                        .keymap
                        .keys_label(Action::ThinkingToggle)
                        .map(|k| crate::chrome::format_key_text(&k, true))
                        .unwrap_or_default(),
                )),
                None,
            ),
            SelectorKind::ShowImages => (
                Box::new(
                    ListSelector::show_images(self.state.show_images)
                        .with_upstream_chrome(kind, &self.state.select_keymap),
                ),
                None,
            ),
            SelectorKind::Theme => (
                Box::new(
                    ListSelector::theme(&self.state.theme.name)
                        .with_upstream_chrome(kind, &self.state.select_keymap),
                ),
                Some(self.state.theme.clone()),
            ),
            // Data-bound selectors must be opened via `open_data_selector` (they need L5 rows);
            // opening one with no data yields an empty-state list rather than a panic.
            other => (
                Box::new(
                    ListSelector::data(other, Vec::new(), 0)
                        .with_upstream_chrome(other, &self.state.select_keymap),
                ),
                None,
            ),
        }
    }

    /// Open `inner` as a **child** of whatever currently occupies the slot (pi
    /// `SettingsList.activateItem`'s `this.submenuComponent = item.submenu(…)`,
    /// `settings-list.ts:214-236`): the displaced selector is kept as the new frame's
    /// [`ActiveSelector::parent`] and comes back when this one closes, instead of being dropped.
    ///
    /// The editor is deliberately NOT re-snapshotted — the parent frame already holds the text the
    /// editor had before the whole stack opened, and only the outermost close re-applies it
    /// ([`Self::close_selector`]). `restore_theme` is per-frame, so the theme submenu still undoes
    /// its own live preview on Esc while leaving `/settings` on screen.
    ///
    /// With no selector open this degenerates to a plain open with an empty saved editor, which is
    /// why every call site is a submenu activation reached from an open selector.
    pub(crate) fn open_child_selector(
        &mut self,
        kind: SelectorKind,
        inner: Box<dyn Selector>,
        restore_theme: Option<UiTheme>,
    ) {
        let parent = self.state.selector.take().map(Box::new);
        self.state.selector = Some(ActiveSelector {
            kind,
            inner,
            saved_editor: String::new(),
            restore_theme,
            parent,
        });
    }

    /// Open a **data-bound** selector (`/model`, `/resume`, `/tree`, …) over rows the run loop sourced
    /// from session-svc / resources (spec/tui/05 §6, §8 late-data population). `rows` are
    /// `(value, label, description)`; `selected` preselects a row. Confirming hands the chosen `value`
    /// back to the run loop as [`AppCommand::ConfirmSelection`].
    pub fn open_data_selector(
        &mut self,
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
    ) {
        let saved_editor = self.state.editor.text();
        let inner = self.build_data_selector(kind, rows, selected);
        self.state.selector = Some(ActiveSelector {
            kind,
            inner,
            saved_editor,
            restore_theme: None,
            parent: None,
        });
    }

    /// The `/settings`-submenu form of [`Self::open_data_selector`]: same picker, mounted as a
    /// child of `parent` (the frame the run loop was handed through
    /// [`AppState::pending_selector_parent`]) so closing it pops back rather than reaching the
    /// prompt. `parent: None` is exactly [`Self::open_data_selector`], for the routes that opened
    /// the picker on its own (`/model`, `/resume`, …).
    pub(crate) fn open_data_child_selector(
        &mut self,
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
        parent: Option<Box<ActiveSelector>>,
    ) {
        if parent.is_none() {
            return self.open_data_selector(kind, rows, selected);
        }
        let inner = self.build_data_selector(kind, rows, selected);
        self.state.selector = Some(ActiveSelector {
            kind,
            inner,
            saved_editor: String::new(),
            restore_theme: None,
            parent,
        });
    }

    /// Mount one step of a `/settings` submenu (pi `SteppedSubmenu.buildStep` →
    /// `new SelectSubmenu(title, `${stepLabel}${desc}`, items, preselect, …)`,
    /// `settings-submenu.ts:204-243`), as a child of `parent` exactly like
    /// [`Self::open_data_child_selector`].
    ///
    /// `step`/`steps` produce upstream's `Step i/N · ` description prefix, drawn only when there is
    /// more than one step (`:206`). `searchable` is the step's own flag — `true` on the model step
    /// of the per-model thinking flow (`settings-selector.ts:611`) and unset on its level step.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_submenu_child_selector(
        &mut self,
        kind: SelectorKind,
        title: String,
        description: &str,
        step: usize,
        steps: usize,
        searchable: bool,
        layout: crate::ColumnLayout,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
        parent: Option<Box<ActiveSelector>>,
    ) {
        // `const stepLabel = total > 1 ? `Step ${stepIndex + 1}/${total} · ` : ""` (`:206`) — the
        // prefix lands on the DESCRIPTION, not on the title (`:214`).
        let description = if steps > 1 {
            format!("Step {step}/{steps} \u{b7} {description}")
        } else {
            description.to_string()
        };
        let inner: Box<dyn Selector> = Box::new(crate::submenu_selector::SubmenuSelector::new(
            title,
            description,
            rows,
            selected,
            searchable,
            layout,
        ));
        let saved_editor = if parent.is_some() {
            String::new()
        } else {
            self.state.editor.text()
        };
        self.state.selector = Some(ActiveSelector {
            kind,
            inner,
            saved_editor,
            restore_theme: None,
            parent,
        });
    }

    /// Build (but do not mount) the data-bound list picker for `kind`.
    fn build_data_selector(
        &self,
        kind: SelectorKind,
        rows: Vec<(String, String, Option<String>)>,
        selected: usize,
    ) -> Box<dyn Selector> {
        Box::new(
            ListSelector::data(kind, rows, selected)
                .with_upstream_chrome(kind, &self.state.select_keymap),
        )
    }

    /// Open the bespoke scoped-models checkbox+reorder selector (`scoped-models-selector.ts`,
    /// spec/tui/05 §6) over the full `catalog` `(id, label, provider, desc)` with the current scope
    /// (`None` = all enabled). Confirming (Ctrl+S) yields an [`AppCommand::ConfirmSelection`] the run
    /// loop applies via `set_scoped_models`.
    pub fn open_checkbox_selector(
        &mut self,
        catalog: Vec<(String, String, String, Option<String>)>,
        enabled: Option<Vec<String>>,
    ) {
        let saved_editor = self.state.editor.text();
        let mut selector = CheckboxSelector::scoped_models(catalog, enabled);
        // `getFooterText` resolves the toggle key through `keyText("tui.select.confirm")`
        // (`scoped-models-selector.ts:198`), so the footer has to read the app's merged table, not
        // the stock one. Same for the bespoke `app.models.*` keys the rest of the row names
        // (`:199-204`), which is what `set_models_keymap` is for.
        selector.set_select_keymap(self.state.select_keymap.clone());
        selector.set_models_keymap(self.state.models_keymap.clone());
        let inner: Box<dyn Selector> = Box::new(selector);
        self.state.selector = Some(ActiveSelector {
            kind: SelectorKind::ScopedModels,
            inner,
            saved_editor,
            restore_theme: None,
            parent: None,
        });
    }

    /// Open the `/model` selector (feature #1): the full [`ModelSelector`] with fuzzy search, the
    /// `all | scoped` scope toggle, `[provider]` badges, and a `✓` on the active model — over the live
    /// model catalog `(id, name, provider, current, scoped)`. Replaces the bare titled list the audit
    /// flagged. Snapshots the editor like every editor-swap selector. When `search` is `Some`, the
    /// picker opens **pre-filtered** to it (Pi `showModelSelector(initialSearchInput)`,
    /// interactive-mode.ts:4307,4333).
    pub fn open_model_selector(&mut self, models: Vec<ModelEntry>, search: Option<String>) {
        let saved_editor = self.state.editor.text();
        let mut selector = ModelSelector::new(models);
        // Pi passes `defaultProvider && defaultModel` into the component and wires
        // `onSelectAsDefault` alongside it (`interactive-mode.ts:4999-5000`). Absent here means the
        // picker was opened on a path with no session to persist through (the test constructors),
        // and it correctly renders no `Ctrl+S` hint and binds no key.
        if let Some((provider, id)) = self.state.default_model.clone() {
            selector = selector.with_default_model(&provider, &id);
        }
        // `getScopeHintText` is `keyHint("tui.input.tab", "scope") + …` (`model-selector.ts:229`),
        // resolved through the live table; cyrup's editor tier owns that binding.
        selector.set_editor_keymap(self.state.editor.keymap_ref());
        if let Some(term) = search {
            selector.set_search(term);
        }
        let inner: Box<dyn Selector> = Box::new(selector);
        self.state.selector = Some(ActiveSelector {
            kind: SelectorKind::Model,
            inner,
            saved_editor,
            restore_theme: None,
            parent: None,
        });
    }

    /// Handle `/model [text]` (Pi `handleModelCommand`, interactive-mode.ts:4175-4196): with no term the
    /// unfiltered picker opens; with a term, an EXACT catalog match sets the model directly (no picker,
    /// `findExactModelReferenceMatch` → `session.setModel`), while a partial opens the picker
    /// pre-filtered to it. The catalog is the live available multi-provider catalog the picker itself
    /// sources (`model_entries`).
    pub(crate) async fn handle_model_command(
        &mut self,
        session: &Arc<AgentSession>,
        search: Option<String>,
    ) {
        // Seed the persisted default before opening: `open_model_selector` is also reachable from
        // session-less test paths, so the picker reads it off state rather than taking it as an
        // argument. `Some` even when both are unset — that is Pi's "callback wired, nothing
        // default yet" state, which still offers `Ctrl+S`.
        let eff = session.services().settings.effective();
        self.state.default_model = Some((
            eff.default_provider().unwrap_or_default(),
            eff.default_model().unwrap_or_default(),
        ));
        let models = model_entries(session);
        if models.is_empty() {
            self.state
                .transcript
                .push_status("no models available (configure providers)");
            return;
        }
        if let Some(term) = search.as_deref()
            && let Some(model) =
                crate::model_selector::find_exact_model_reference_match(&models, term)
        {
            // Exact match → set the fully-qualified `provider/id` directly (mirrors the confirm path),
            // no picker (`handleModelCommand` early-returns after `setModel`).
            let id = format!("{}/{}", model.provider, model.id);
            match session.set_model(&id).await {
                Ok(_) => self.state.transcript.push_status(format!("model → {id}")),
                Err(e) => self
                    .state
                    .transcript
                    .push_status(format!("model error: {e}")),
            }
            return;
        }
        // No term, or a partial with no exact match → the picker, pre-filtered to the term if any.
        self.open_model_selector(models, search);
    }

    /// Open an arbitrary boxed [`Selector`] in the input slot under `kind` (the seam for the bespoke
    /// non-list selectors — `/tree`'s [`TreeSelector`] — that are not a plain [`ListSelector`] yet need
    /// the same editor-swap lifecycle as the data selectors). Snapshots the editor like the others.
    pub fn open_boxed_selector(&mut self, kind: SelectorKind, inner: Box<dyn Selector>) {
        let saved_editor = self.state.editor.text();
        self.state.selector = Some(ActiveSelector {
            kind,
            inner,
            saved_editor,
            restore_theme: None,
            parent: None,
        });
    }

    /// The kind of the currently-open selector, if any (test/inspection access).
    pub fn active_selector_kind(&self) -> Option<SelectorKind> {
        self.state.selector.as_ref().map(|s| s.kind)
    }

    /// Route one key to the active selector and act on the outcome (spec/tui/05 §3.1). `Confirm`
    /// applies the selection by kind and closes the slot; `Cancel` restores the prior theme (if any)
    /// and closes; `Preview` re-themes live without closing. A no-op if no selector is open.
    pub(crate) fn handle_selector_key(&mut self, key: &event::KeyEvent) -> AppAction {
        // Hand the selector the LIVE `tui.editor.*` table before routing the key: pi's `Input`
        // re-reads `getKeybindings()` on every keystroke (`input.ts:86`), so a rebound Ctrl+W or
        // Alt+B reaches a search box without the dialog being reopened. Destructured so the editor
        // borrow does not collide with the selector one.
        let AppState {
            selector,
            select_keymap,
            editor,
            ..
        } = &mut self.state;
        let Some(active) = selector.as_mut() else {
            return AppAction::None;
        };
        active.inner.set_editor_keymap(editor.keymap_ref());
        let outcome = active.inner.handle(key, select_keymap);
        let kind = active.kind;
        self.apply_selector_outcome(kind, outcome)
    }

    /// Offer a bracketed paste to the active selector's embedded `Input` (pi `Input.handlePaste`,
    /// `input.ts:362-372`). A selector that owns no input answers [`SelectorOutcome::Ignored`],
    /// which becomes [`AppAction::None`] — the preserved "the chrome drops the paste" fallback.
    pub(crate) fn handle_selector_paste(&mut self, text: &str) -> AppAction {
        let Some(active) = self.state.selector.as_mut() else {
            return AppAction::None;
        };
        let outcome = active.inner.handle_paste(text);
        let kind = active.kind;
        self.apply_selector_outcome(kind, outcome)
    }

    /// Act on a [`SelectorOutcome`], whatever produced it — the key path and the paste path share
    /// this verbatim.
    fn apply_selector_outcome(
        &mut self,
        kind: SelectorKind,
        outcome: SelectorOutcome,
    ) -> AppAction {
        match outcome {
            SelectorOutcome::Ignored => AppAction::None,
            SelectorOutcome::Redraw => AppAction::Redraw,
            SelectorOutcome::Preview(value) => {
                // Theme live preview: re-theme the whole UI as the highlight moves
                // (`theme-selector.ts:54-56`). Other kinds never emit `Preview`.
                if kind == SelectorKind::Theme {
                    self.set_theme(UiTheme::builtin(&value));
                }
                AppAction::Redraw
            }
            SelectorOutcome::Confirm(value) => {
                // The login dialog is the one selector that does NOT close on confirm: submitting
                // answers the flow's in-flight `AuthInteraction::prompt` and the flow runs on —
                // Pi's `input.onSubmit` resolves `inputResolver` and leaves `editorContainer`
                // alone (`login-dialog.ts:56-64`), so the URL/device code stays on screen and a
                // second prompt can follow. The dialog is torn down by `finish_login` (the login
                // settled) or by the `Cancel` arm below.
                if kind == SelectorKind::LoginDialog {
                    if let Some(reply) = self.state.pending_login_prompt.take() {
                        let _ = reply.send(Ok(value));
                    }
                    return AppAction::Redraw;
                }
                let command = self.confirm_selector(kind, &value);
                self.close_selector(false);
                // pi applies the chosen value to the parent row BEFORE closing the submenu
                // (`settings-list.ts:222-225`); doing it just after the pop is the same frame on
                // screen, and keeps the "did this close pop a settings list?" question in one place.
                if let Some(row) = Self::submenu_row_for(kind) {
                    self.set_submenu_row_value(kind, row, &value);
                }
                match command {
                    Some(c) => AppAction::Command(c),
                    None => AppAction::Redraw,
                }
            }
            SelectorOutcome::ConfirmDefault(value) => {
                // Pi's `Ctrl+S` sibling of the confirm key: apply to the session AND persist as the
                // default. Both pickers close exactly as `Enter` does — the model picker disposes
                // before its callback (`model-selector.ts:406-407`), the thinking picker's callback
                // is `(level) => selectLevel(level, true)` whose `selectLevel` calls `done()`
                // (`interactive-mode.ts:4803`, `:4813`) — so the close is unconditional here.
                let command = self.confirm_selector_as_default(kind, &value);
                self.close_selector(false);
                if let Some(row) = Self::submenu_row_for(kind) {
                    self.set_submenu_row_value(kind, row, &value);
                }
                match command {
                    Some(c) => AppAction::Command(c),
                    None => AppAction::Redraw,
                }
            }
            SelectorOutcome::Apply(payload) => {
                // A `/tree` label save (`e` → `LabelInput` submit, tree_selector.rs) rides an
                // `"{entry_id}\u{1f}{label}"` `Apply` payload; the entry id is a UUID (never contains
                // the separator) so the split is unambiguous. Persist it via the session `set_label`
                // path and keep the slot open (the tree already refreshed its own `has_label` star).
                if kind == SelectorKind::Tree {
                    // `app.message.copy` inside the tree rides a unit-separator-TAGGED payload
                    // (`"\u{1f}copy\u{1f}{entry_id}"`, `TreeSelector::copy_payload`), decoded
                    // first so it can never be read as the untagged label form below. The slot
                    // stays open — pi does not close the tree on copy (`tree-selector.ts:1029`).
                    let sep = crate::FIELD_SEP;
                    if let Some(entry_id) = payload.strip_prefix(&format!("{sep}copy{sep}")) {
                        return AppAction::Command(AppCommand::CopyEntry(entry_id.to_string()));
                    }
                    return match payload.split_once(crate::FIELD_SEP) {
                        Some((entry_id, label)) => AppAction::Command(AppCommand::SetEntryLabel {
                            entry_id: entry_id.to_string(),
                            label: label.to_string(),
                        }),
                        None => AppAction::Redraw,
                    };
                }
                // A `/resume` in-list delete/rename rides a unit-separator-*tagged* `Apply` payload
                // (`session_selector.rs`); decode it first so it never mis-routes to the settings
                // handler. The slot stays open (the selector already mutated its own row list).
                if let Some(action) = SessionSelectorOutcome::parse_apply(&payload) {
                    return match action {
                        SessionSelectorOutcome::Delete(path) => {
                            AppAction::Command(AppCommand::DeleteSession(path))
                        }
                        SessionSelectorOutcome::Rename { path, name } => {
                            AppAction::Command(AppCommand::RenameSession { path, name })
                        }
                        // `Resume` never arrives via `Apply` (it is a `Confirm`); ignore defensively.
                        SessionSelectorOutcome::Resume(_) => AppAction::Redraw,
                    };
                }
                // Otherwise a `/settings` row cycled in place: persist it live, keep the slot open
                // (Pi's settings selector applies on each `onChange`). The payload is `"id\u{1f}value"`.
                match payload.split_once(crate::FIELD_SEP) {
                    Some((id, value)) => AppAction::Command(AppCommand::ApplySetting {
                        id: id.to_string(),
                        value: value.to_string(),
                    }),
                    None => AppAction::Redraw,
                }
            }
            SelectorOutcome::Cancel => {
                // A cancelled extension-UI dialog resolves to its per-kind deny default (Pi's
                // `Esc`-cancelled select yields `undefined`, which `confirm`'s `result === Yes` then
                // reads as `false` — `interactive-mode.ts:2172-2179`) rather than hanging the
                // wasm-suspended guest until `ui_roundtrip`'s timeout (or forever, with none set).
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(default_ui_reply(pending.kind));
                }
                // `LoginDialogComponent.cancel()` (`login-dialog.ts:82-88`): abort the flow's signal
                // AND reject whatever prompt it is blocked on with `"Login cancelled"`. Without the
                // signal half, a flow parked on a callback server or a device-code poll (neither of
                // which is a prompt) would keep running with no dialog to talk to.
                if kind == SelectorKind::LoginDialog {
                    self.cancel_login();
                }
                self.close_selector(true);
                // The two `/tree` summarization prompts each have their OWN Escape destination in Pi
                // (`interactive-mode.ts:4761-4765`, `:4770-4773`), not a plain dismiss:
                match kind {
                    // "Summarize branch?" → back to the tree selector, same selection (`:4763`).
                    // `pending_tree_nav` is deliberately LEFT SET: the tree-open arm consumes it as
                    // the initial selection, which is what `showTreeSelector(entryId)` means.
                    SelectorKind::BranchSummary => {
                        return AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree));
                    }
                    // The custom-instructions editor → back to the prompt (Pi's `continue`, `:4772`),
                    // NOT out of the flow: the pending target is deliberately kept.
                    SelectorKind::BranchSummaryInstructions => {
                        self.open_branch_summary_prompt();
                        return AppAction::Redraw;
                    }
                    // TUI-081 — Escape on the `/import` guard is a decline: `showExtensionSelector`
                    // resolves `undefined` on Esc, `showExtensionConfirm` reads that as `false`
                    // (`interactive-mode.ts:2564` @v0.84.4) and `handleImportCommand` shows
                    // `Import cancelled` (`:6071`). The stashed path is dropped with it.
                    SelectorKind::ImportConfirm => self.cancel_pending_import(),
                    _ => {}
                }
                AppAction::Redraw
            }
            // A `/settings` submenu row (Pi `SettingItem.submenu`, settings-selector.ts:603-610):
            // open the nested picker as a CHILD of the settings selector, which stays alive
            // underneath it (`settings-list.ts:214-236` stores it in `submenuComponent` and
            // `closeSubmenu` `:242-256` brings it back, cursor row and search query intact).
            SelectorOutcome::OpenSubmenu(id) => {
                match id.as_str() {
                    "theme" => {
                        // `restore_theme` rides the CHILD frame, so Esc out of the picker undoes the
                        // live preview and lands back on the settings list — pi's
                        // `ThemeSubmenu.cancel()`, which restores `originalThemeSetting` and then
                        // calls `onDone()` with no value (`settings-selector.ts:283-330`).
                        let (inner, restore_theme) = self.build_list_selector(SelectorKind::Theme);
                        self.open_child_selector(SelectorKind::Theme, inner, restore_theme);
                    }
                    // TUI-032 — `thinking` opens the picker cyrup already had and could not reach:
                    // Pi's `SelectSubmenu("Thinking Level", …, config.availableThinkingLevels, …,
                    // callbacks.onThinkingLevelChange)` (`settings-selector.ts:591-611`).
                    "thinking" => {
                        let (inner, restore_theme) =
                            self.build_list_selector(SelectorKind::Thinking);
                        self.open_child_selector(SelectorKind::Thinking, inner, restore_theme);
                    }
                    // GAP 3 step 1. Unlike `theme`/`thinking` this picker is DATA-BOUND (its rows
                    // are the model catalog plus each model's current override), and this handler
                    // has no session — so it rides a command the run loop resolves, the same shape
                    // the `BranchSummary` cancel arm above uses.
                    "model-thinking" => {
                        // The settings frame cannot be the child's parent yet — the child does not
                        // exist until the run loop has built its rows — so it rides
                        // `pending_selector_parent` across the command, and the `OpenSelector`
                        // arm re-parents the picker onto it (execute.rs).
                        self.state.pending_selector_parent =
                            self.state.selector.take().map(Box::new);
                        return AppAction::Command(AppCommand::OpenSelector(
                            SelectorKind::ModelThinking,
                        ));
                    }
                    // `warnings` is a nested toggle LIST, not a picker — Pi's
                    // `WarningSettingsSubmenu` (`settings-selector.ts:120-160`) is a `SettingsList`
                    // over one item, `anthropic-extra-usage`, whose `onChange` writes straight
                    // through. cyrup reuses the same `SettingsSelector` component and the same
                    // `Apply("id\u{1f}value")` → `AppCommand::ApplySetting` persist path the parent
                    // grid rides, so the nested row writes the global layer with no new plumbing.
                    "warnings" => {
                        let rows =
                            vec![SettingRow::toggle(
                            "warnings.anthropicExtraUsage",
                            "Anthropic extra usage",
                            self.state.warn_anthropic_extra_usage,
                        )
                        .with_description(
                            "Warn when Anthropic subscription auth may use paid extra usage",
                        )];
                        let inner: Box<dyn Selector> =
                            Box::new(SettingsSelector::new("Warnings", rows));
                        self.open_child_selector(SelectorKind::Settings, inner, None);
                    }
                    _ => {}
                }
                AppAction::Redraw
            }
            // `Ctrl+G` inside the extension `ui.editor` dialog (L4 review §3) — the actual
            // teardown+spawn+restore needs `&mut self: &mut App` (terminal access), which
            // `Selector::handle` doesn't have; bubble it up as an `AppAction` the run loop's
            // fallible `match` dispatches (mirrors the plain `Ctrl+G`/`AppAction::OpenExternalEditor`
            // arm right next to it).
            SelectorOutcome::OpenExternalEditor => AppAction::OpenExternalEditorForSelector,
        }
    }

    /// Apply a confirmed selection. The three dependency-free selectors (theme/thinking/show-images)
    /// are applied fully in-crate and return `None`; the data-bound selectors return an
    /// [`AppCommand::ConfirmSelection`] so the run loop applies the effect at the session layer (set
    /// model, switch branch, login…).
    /// The `Ctrl+S` sibling of [`Self::confirm_selector`]: apply the choice to the session AND
    /// persist it as the global default (Pi `selectModel(m, true)` / `selectLevel(l, true)`).
    ///
    /// Only the two pickers that opt into the binding can reach this — every other kind falls back
    /// to the plain confirm, so a stray `Ctrl+S` can never write settings for a selector Pi does
    /// not persist from.
    fn confirm_selector_as_default(
        &mut self,
        kind: SelectorKind,
        value: &str,
    ) -> Option<AppCommand> {
        match kind {
            SelectorKind::Model | SelectorKind::Thinking => {
                // The same optimistic local mirror the `Enter` path applies, so the footer and the
                // editor rule are correct on the frame the picker closes; the session event then
                // confirms or clamps. The persist rides the command, after the session set lands.
                if kind == SelectorKind::Thinking {
                    self.state.thinking_level = value.to_string();
                    self.state.status.set_thinking_level(value);
                    self.state.editor.set_thinking_level(value);
                }
                Some(AppCommand::ConfirmSelectionAsDefault {
                    kind,
                    value: value.to_string(),
                })
            }
            other => self.confirm_selector(other, value),
        }
    }

    fn confirm_selector(&mut self, kind: SelectorKind, value: &str) -> Option<AppCommand> {
        match kind {
            // TUI-N03 — this arm used to return `None`, so a theme chosen in `/settings` repainted
            // the UI and then died with the process: no `ApplySetting` ever reached the persist arm.
            // Pi distinguishes PREVIEW from CONFIRM — `onThemePreview: (name) =>
            // themeController.preview(name)` versus `onThemeChange: (t) => {
            // this.settingsManager.setTheme(t); void this.themeController.applyFromSettings(); }`
            // (`interactive-mode.ts:4226-4231`) — and cyrup treated confirm as a preview that stuck
            // until exit. Worse in combination with TUI-004: `ThemeController::sync_with_terminal`
            // persists an OSC-11 detection only when `settings.theme` is UNSET, which is exactly the
            // state a never-persisted user choice leaves behind, so the next launch overwrote it.
            //
            // `set_theme` still runs for the immediate repaint; the persist arm (`C::ApplySetting`)
            // pushes the `theme → {value}` status, so this arm no longer pushes its own.
            SelectorKind::Theme => {
                self.set_theme(UiTheme::builtin(value));
                Some(AppCommand::ApplySetting {
                    id: "theme".to_string(),
                    value: value.to_string(),
                })
            }
            // TUI-032 — on the `Enter` path the level is applied to the SESSION, not written to
            // the settings layer: Pi's `onThinkingLevelChange` is `this.session.setThinkingLevel(
            // level); this.footer.invalidate(); this.updateEditorBorderColor();`
            // (`interactive-mode.ts:4222-4226`). The optimistic local mirror below keeps the footer
            // and the editor rule in lockstep on the frame the picker closes; the session's
            // `ThinkingLevelChanged` event then confirms (or clamps) it.
            //
            // "Session op, not a settings write" is true HERE and only here. Pi has a second
            // confirm key in this picker — `Ctrl+S` → `selectLevel(level, true)` → `{persist:true}`
            // (`interactive-mode.ts:4813`) — and reading the `Enter` callback as the whole story is
            // exactly what kept `defaultThinkingLevel` write-only-by-the-launcher. That path is
            // [`Self::confirm_selector_as_default`].
            SelectorKind::Thinking => {
                self.state.thinking_level = value.to_string();
                self.state.status.set_thinking_level(value);
                // The editor's rule color is the always-visible thinking-level signal (spec/tui/03
                // §3.3) — keep it in lockstep with the selected level.
                self.state.editor.set_thinking_level(value);
                Some(AppCommand::SetThinking(value.to_string()))
            }
            // GAP 3 step 1 → step 2: stash the chosen `provider/id` and open the level picker
            // (Pi's `SteppedSubmenu` advancing with `selections.model` set,
            // `settings-selector.ts:653`). Data-bound, so it rides a command.
            SelectorKind::ModelThinking => {
                self.state.pending_model_thinking = Some(value.to_string());
                // Step 1 is not being left behind — pi's `SteppedSubmenu` keeps ONE submenu alive
                // and swaps its active step (`settings-submenu.ts:216-232`), and its Esc at step 2
                // rebuilds step 1 rather than closing (`:234-241`). Moving the whole step-1 frame
                // (settings list still hanging off it) into `pending_selector_parent` makes step 2
                // its child, so Esc pops model-list → settings list. The `close_selector(false)`
                // that follows this call finds an empty slot and correctly does nothing: this frame
                // is not closing, it is being re-parented.
                self.state.pending_selector_parent = self.state.selector.take().map(Box::new);
                Some(AppCommand::OpenSelector(SelectorKind::ModelThinkingLevel))
            }
            // GAP 3 step 2: apply. The pending model is consumed here, so an Escape at step 2
            // leaves it set but harmless — the next step-1 confirm overwrites it.
            SelectorKind::ModelThinkingLevel => {
                self.state.pending_model_thinking.take().map(|model| {
                    AppCommand::SetModelThinkingLevel {
                        model,
                        level: value.to_string(),
                    }
                })
            }
            SelectorKind::ShowImages => {
                self.state.show_images = value == "yes";
                // TUI-007: the toggle governs TOOL-RESULT images too (Pi passes `showImages` into
                // every `ToolExecutionComponent`, interactive-mode.ts:3449), not just the editor's
                // attachment strip. Off ⇒ Pi's `[Image: …]` text stand-in.
                self.state
                    .transcript
                    .set_show_images(self.state.show_images);
                let label = if self.state.show_images {
                    "inline"
                } else {
                    "placeholder"
                };
                self.state
                    .transcript
                    .push_status(format!("images → {label}"));
                None
            }
            SelectorKind::ExtensionConfirm => {
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(UiReply::Confirm(value == "yes"));
                }
                None
            }
            SelectorKind::ExtensionSelect
            | SelectorKind::ExtensionInput
            | SelectorKind::ExtensionEditor => {
                if let Some(pending) = self.state.pending_ui_reply.take() {
                    let _ = pending.reply.send(UiReply::Text(Some(value.to_string())));
                }
                None
            }
            // Unreachable: [`Self::handle_selector_key`] intercepts a login-dialog confirm before
            // it gets here (the dialog must NOT close on submit). Explicit rather than falling into
            // the `other` arm, which would emit a bogus `ConfirmSelection` command.
            SelectorKind::LoginDialog => {
                if let Some(reply) = self.state.pending_login_prompt.take() {
                    let _ = reply.send(Ok(value.to_string()));
                }
                None
            }
            other => Some(AppCommand::ConfirmSelection {
                kind: other,
                value: value.to_string(),
            }),
        }
    }

    /// Close the topmost selector frame (spec/tui/05 §7 `done()`). A previewed theme is restored
    /// first when `cancelled`, for the frame that is closing, whatever its depth.
    ///
    /// With a [`ActiveSelector::parent`] this is pi's `closeSubmenu` (`settings-list.ts:242-256`):
    /// the parent goes back in the slot and the editor is left alone — the parent still owns the
    /// input slot, and its retained component carries its own cursor row and search query, so the
    /// user returns to the settings list exactly where they left it. Only the outermost frame
    /// re-applies the editor text, so a stack of submenus restores it exactly once.
    pub(crate) fn close_selector(&mut self, cancelled: bool) {
        if let Some(active) = self.state.selector.take() {
            if cancelled && let Some(theme) = active.restore_theme {
                self.set_theme(theme);
            }
            match active.parent {
                Some(parent) => self.state.selector = Some(*parent),
                None => self.state.editor.set_text(&active.saved_editor),
            }
        }
    }

    /// Write `value` into the `id` row of every `/settings` list still on the stack (pi
    /// `SettingsList.updateValue`, `settings-list.ts:74-80`, which `activateItem`'s `done()`
    /// callback runs — as `item.currentValue = selectedValue` — BEFORE `closeSubmenu()`,
    /// `:216-225`). Without it the row the user pops back to still shows the value it was built
    /// with at `/settings` time: `AppCommand::ApplySetting` persists and mirrors onto `AppState`
    /// but never touches the open selector.
    ///
    /// The whole chain is walked (not just the frame in the slot) because the settings list is the
    /// PARENT of the picker that produced the value, and for the two-step per-model flow the
    /// grandparent. A frame that is not a [`SettingsSelector`], or has no such row, is skipped —
    /// `update_value` is a no-op for an unknown id, exactly like upstream's `find`.
    /// Write a confirmed submenu pick back into its `/settings` row. The thinking row carries the
    /// `hideThinkingBlock` marker, so it goes through the formatter the row was BUILT with — a bare
    /// `set_settings_row_value` here would drop the marker on confirm and leave the row claiming
    /// reasoning is visible while it is suppressed.
    fn set_submenu_row_value(&mut self, kind: SelectorKind, id: &str, value: &str) {
        let shown = if kind == SelectorKind::Thinking {
            thinking_row_value(value, self.state.transcript.hide_thinking_block())
        } else {
            value.to_string()
        };
        self.set_settings_row_value(id, &shown);
    }

    pub(crate) fn set_settings_row_value(&mut self, id: &str, value: &str) {
        let mut frame = self.state.selector.as_mut();
        while let Some(active) = frame {
            if let Some(settings) = active.inner.as_settings_mut() {
                settings.update_value(id, value);
            }
            frame = active.parent.as_deref_mut();
        }
    }

    /// The `id` of the `/settings` row a confirmed submenu picker feeds back into, if any — the
    /// row-to-submenu mapping of [`fn@crate::app::settings_rows`] read backwards. `warnings` is
    /// deliberately absent: its submenu calls `done()` with no value upstream
    /// (`settings-selector.ts:560-569`), so the parent row keeps its literal `"configure"`.
    fn submenu_row_for(kind: SelectorKind) -> Option<&'static str> {
        match kind {
            SelectorKind::Theme => Some("theme"),
            SelectorKind::Thinking => Some("thinking"),
            _ => None,
        }
    }
}
