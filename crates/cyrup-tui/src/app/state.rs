use super::*;

/// All retained UI state (the data half of the `state -> frame` split).
pub struct AppState {
    pub transcript: TranscriptView,
    pub editor: InputEditor,
    pub status: StatusLine,
    /// The working/idle status band (spec/tui/01 §6) — a 2-row spinner+message while a turn/retry/
    /// compaction runs, blank when idle. Driven by `AgentSessionEvent`s in [`App::ingest_event`].
    pub indicator: StatusIndicator,
    pub theme: UiTheme,
    /// The terminal color depth every theme is projected into (feature #3/#4): boot resolves it from
    /// the terminal (`ColorMode::detect` / the `ThemeController`); a live `/theme` switch re-projects
    /// the new theme through it so 256-color terminals keep indexed colors after a switch.
    pub color_mode: ColorMode,
    pub keymap: Keymap,
    /// The selector binding table (`tui.select.*`, spec/tui/05 §10) consulted while a selector owns
    /// the input slot.
    pub select_keymap: SelectKeymap,
    /// The `/tree` bespoke binding table (`app.tree.*`, spec/tui/05 §6.1) handed to each opened
    /// [`TreeSelector`] so JSON rebinds of fold/unfold/label flow through (R-10-018).
    pub tree_keymap: TreeKeymap,
    /// The `/resume` bespoke binding table (`app.session.*`, `core/keybindings.ts:91-94,135-154`)
    /// handed to each opened [`SessionSelector`], so a JSON rebind of sort/named/delete/path/rename
    /// reaches BOTH the handler and the header's hint rows (`session-selector.ts:171-179`).
    pub session_keymap: SessionKeymap,
    /// The `/scoped-models` bespoke binding table (`app.models.*`, `core/keybindings.ts:150-175`)
    /// handed to each opened [`CheckboxSelector`], so a JSON rebind of reorder/all/clear/provider/
    /// save reaches both the handler and the footer row (`scoped-models-selector.ts:199-204`).
    pub models_keymap: ModelsKeymap,
    /// The slash-command registry driving dispatch + autocomplete (rebuilt on `/reload`).
    pub commands: CommandRegistry,
    /// The active editor-swap selector, if any (spec/tui/05 §1.1): when `Some`, it replaces the
    /// editor in the bottom inline region and captures input until it confirms/cancels.
    pub selector: Option<ActiveSelector>,
    /// The floating overlay z-stack (spec/tui/05 §2): hotkeys/help popup (and, later, extension UI).
    /// The topmost overlay captures input; rendered over the live region bottom→top.
    pub overlays: Vec<Box<dyn Overlay>>,
    /// The current reasoning level (`off`…`xhigh`), preselected by the thinking selector and updated
    /// on confirm. The authoritative level lives on the agent/session at the L7 layer.
    pub thinking_level: String,
    /// Whether inline images are shown (vs. a text placeholder), toggled by the show-images selector.
    pub show_images: bool,
    /// The terminal image-protocol renderer (spec/tui/06 §6; `terminal-image.ts`). Defaults to the
    /// portable half-block raster; the production binary upgrades it to the real protocol via
    /// [`App::detect_image_support`]. Drives the inline render of [`AppState::pending_images`].
    pub image_renderer: ImageRenderer,
    /// Images attached to the next prompt (the `@`-mention of an image file / a paste), rendered
    /// inline above the editor in the live region (`components/image.ts`), honoring `show_images`.
    pub pending_images: Vec<ImageBlock>,
    /// Messages the session is HOLDING because a turn is streaming — Pi's
    /// `pendingMessagesContainer` (`interactive-mode.ts:328`, filled by
    /// `updatePendingMessagesDisplay` at `:3974-3991`), docked directly above the status band.
    /// Fed from `queue_update`; see [`crate::pending_messages`] for why it exists and what it
    /// replaced (TUI-016 / TUI-052).
    pub pending_messages: crate::pending_messages::PendingMessages,
    /// The last `queue_update` snapshot from the SESSION's own two queues, kept so the pending
    /// region can be rebuilt whenever either source changes — Pi's `getAllQueuedMessages`
    /// (`interactive-mode.ts:3942-3953`) folds `session.getSteeringMessages()` /
    /// `getFollowUpMessages()` together with `compactionQueuedMessages` every time it renders.
    /// TUI-031.
    pub session_queue: (Vec<String>, Vec<String>),
    /// Raised by the sync `compaction_end` arm and consumed by [`App::ingest_session_event`], which
    /// has the session needed to actually deliver the queue. TUI-031.
    pub compaction_flush_pending: bool,
    /// Whether a compaction is currently running — set by `compaction_start` and cleared by
    /// `compaction_end`, the window in which Pi's Escape handler is rebound to `abortCompaction`.
    pub compacting: bool,
    /// Pi's `compactionQueuedMessages` (`interactive-mode.ts:401`) — prompts submitted WHILE a
    /// compaction is running. The session layer has no compaction guard of its own, so without this
    /// a message typed mid-compaction was dispatched as a fresh turn assembled from a context the
    /// compaction was in the middle of rewriting. TUI-031.
    pub compaction_queue: Vec<CompactionQueued>,
    /// Reserve the 2-row status band even when idle (spec/tui/01 §6.3). Default `false` (Pi's
    /// non-`clearOnShrink` behavior) so the editor/footer never reflow on idle viewports.
    pub reserve_status_rows: bool,
    /// The host TERMINAL's row count — Pi's `this.tui.terminal.rows` (`editor.ts:500`). Refreshed
    /// every [`App::draw`] from the backend; `24` until the first draw, matching the `?? 24` default
    /// pi itself uses when a terminal height is unavailable (`config-selector.ts:264-266`).
    ///
    /// **Not** the live-region height. The editor's row budget is `max(5, floor(terminalRows * 0.3))`
    /// (E3), which must be answered against the SCREEN; `region_constraints` is called once with the
    /// terminal height (from [`live_region_height`]) and once with the resulting viewport height
    /// (from [`render`]), and deriving the budget from its `avail` argument would make those two
    /// calls disagree and the split non-idempotent.
    pub term_rows: u16,
    /// Whether the compact startup keybinding-hints bar is shown (Pi `compactInstructions`,
    /// interactive-mode.ts:697-703): a one-line `interrupt · clear/exit · / commands · ! bash · more`
    /// affordance bar rendered just above the editor at startup, dismissed on the first submission.
    pub show_startup_hints: bool,
    /// A `DynamicBorder` loader occupying the editor slot during a long inline op (Pi
    /// `BorderedLoader`, bordered-loader.ts): `/share`'s gist creation and any extension-UI long op.
    /// When `Some`, it replaces the editor in the live region (the selector still wins if both are set,
    /// which never happens). Cleared when the op completes.
    pub loader: Option<crate::chrome::BorderedLoader>,
    /// The 80 ms phase index for the active [`Self::loader`] / status spinner (advanced by the run-loop
    /// tick). Drives the loader's animated glyph without a timer thread.
    pub loader_tick: usize,
    /// Set when the user requested quit; the run loop observes it.
    pub should_quit: bool,
    /// Timestamp of the last `Ctrl+C` press, for the double-tap-to-exit gate (Pi `handleCtrlC`,
    /// interactive-mode.ts:3797-3805): a second `Ctrl+C` within 500 ms exits; otherwise it clears the
    /// editor and records the press time. `None` until the first press.
    pub(crate) last_sigint: Option<std::time::Instant>,
    /// Timestamp of the last Escape on an EMPTY editor, for Pi's 500 ms double-Escape window
    /// (`interactive-mode.ts:2579-2594`, `private lastEscapeTime = 0` at `:355`). `None` until the
    /// first press, and reset to `None` when a pair fires so a third press starts a new pair.
    /// TUI-009.
    pub(super) last_escape: Option<std::time::Instant>,
    /// The persisted `doubleEscapeAction` setting (`tree` / `fork` / `none`), cached here because
    /// [`App::apply_action`] resolves keys without a session in hand. Seeded at boot and re-seeded
    /// on every session swap alongside the other per-session settings. TUI-009.
    pub double_escape_action: String,
    /// The persisted `warnings.anthropicExtraUsage` value, cached for the `/settings` → `Warnings`
    /// submenu, which is opened from a selector outcome with no session in hand. Pi's default is
    /// `true` (`settings-selector.ts:134` `(this.state.anthropicExtraUsage ?? true)`). TUI-032.
    pub warn_anthropic_extra_usage: bool,
    /// A status line to show **after** the next runtime session-swap re-binds the UI (the swap
    /// resets the transcript, so a pre-swap status would be wiped). Set by the session-lifecycle
    /// command handlers (`/new`/`/resume`/`/fork`/`/reload`/`/import`); consumed by
    /// [`App::rebind_session`] once the generation bump fires and the new session is installed.
    pub pending_swap_status: Option<String>,
    /// Committed lines already emitted to native scrollback via `Terminal::insert_before`
    /// (R-ARCH-TUI-003). Test/inspection only — OFF in production builds (TUI-092 F1).
    #[cfg(any(test, feature = "scrollback-accumulator"))]
    pub scrollback: Vec<Line<'static>>,
    /// Extension-registered keyboard shortcuts (R-08-017; Pi `registerShortcut`): each parsed
    /// [`Key`] spec paired with the [`ShortcutSpec`] the host routes on. Sourced from
    /// `ExtensionHost::shortcut_keys()` at boot and refreshed on session swap; a matching key press
    /// (checked at the global-keymap tier, after built-in bindings) becomes an
    /// [`AppAction::ExtensionShortcut`]. Empty when no extension registered a shortcut.
    ///
    /// This is also the registry `/hotkeys` reads for its **Extensions** table — upstream's
    /// `extensionRunner.getShortcuts(...)` (`interactive-mode.ts:6187-6196`) is the same set from
    /// the same source, so [`App::hotkeys_markdown`] iterates it rather than omitting the section.
    pub extension_shortcuts: Vec<(Key, ShortcutSpec)>,
    /// The env-sniffed terminal capabilities (feature #7/#8; Pi `getCapabilities`): image protocol +
    /// truecolor + OSC-8-hyperlink forwarding. Boot default is conservative (half-block, no
    /// hyperlinks); the binary refines it via [`App::detect_image_support`]. The `hyperlinks` flag
    /// gates OSC-8 emission in rendered links (`osc::hyperlink`).
    pub capabilities: TerminalCapabilities,
    /// The REPLY half of the extension-UI dialog currently occupying [`Self::selector`] (`kind ==
    /// SelectorKind::Extension{Confirm,Select,Input}`), if any (L4 review §2.1). A loaded guest's
    /// synchronous `ui.{confirm,select,input}` call blocks its own tokio task on this one-shot
    /// (`LiveHostServices::ui_roundtrip`) until the selector confirms or cancels; `App::run`'s `ui_rx`
    /// arm sets it when it opens the dialog, and [`App::confirm_selector`] /
    /// [`App::handle_selector_key`]'s `Cancel` arm take + resolve it. `None` whenever no extension
    /// dialog is open (including every ordinary first-party selector).
    pub(super) pending_ui_reply: Option<PendingUiReply>,
    /// The extension-visible mirror of the editor buffer (SEAM-T02) — the cell backing
    /// `HostServices::editor_text`, i.e. pi's `getEditorText: () =>
    /// this.editor.getExpandedText?.() ?? this.editor.getText()` (`interactive-mode.ts:2393`
    /// @v0.84.2). Republished by [`App::publish_extension_readbacks`] on every frame; handed to the
    /// session's `LiveHostServices` by [`App::install_extension_readbacks`], without which
    /// `editor_text` keeps the trait default `""` — the shape the read half shipped in, while its
    /// WRITE half (`set_editor_text`) worked, so a guest's read-modify-write silently discarded its
    /// own edit. Always present here (an unattached mirror is simply never read).
    pub(super) editor_mirror: cyrup_session_svc::EditorTextMirror,
    /// The live theme seam handed to the session's `LiveHostServices` (SEAM-T01) — pi's four
    /// `createExtensionUIContext` theme bindings (`interactive-mode.ts:2401-2417` @v0.84.2). `None`
    /// until a session binds ([`App::install_extension_readbacks`]), and rebuilt on every session
    /// swap because it holds that session's resource snapshot. Kept here so
    /// [`App::publish_extension_readbacks`] can republish the active theme name each frame.
    pub(super) theme_access: Option<Arc<crate::theme_access::TuiThemeAccess>>,
    /// The `/tree` target the user confirmed, held while the "Summarize branch?" prompt (and, on its
    /// third option, the custom-instructions editor) is open — Pi keeps the same values in the
    /// `entryId` / `wantsSummary` / `customInstructions` locals of its `while (true)` prompt loop
    /// (`interactive-mode.ts:4749-4779`). Cleared the moment the navigation is dispatched or the
    /// prompt is escaped back to the tree.
    pub(super) pending_tree_nav: Option<PendingTreeNav>,
    /// The window title currently asked for — either by an extension (Pi `setTitle` →
    /// `ui.terminal.setTitle`, `interactive-mode.ts:2238` → `terminal.ts:504-507`) or by the
    /// automatic session/cwd title ([`App::update_terminal_title`], Pi `updateTerminalTitle`,
    /// `interactive-mode.ts:818-826`). Retained so the value is observable in tests and after a
    /// redraw; the crossterm run loop is what actually writes the OSC 0 sequence.
    pub terminal_title: Option<String>,
    /// The OSC 9;4 taskbar progress indicator — Pi's `terminal.showTerminalProgress` gate plus the
    /// armed bit ([`crate::TerminalProgress`], `tui/src/terminal.ts:509-523`). Held here for the
    /// same reason as [`AppState::terminal_title`] directly above: the session-event fold records
    /// the transition and the crossterm run loop is what writes the escape sequence.
    pub terminal_progress: crate::TerminalProgress,
    /// Pi `this.streamingComponent` (`interactive-mode.ts:435`): the assistant message currently
    /// streaming, as a plain "is one open?" bit — cyrup's transcript owns the buffers, so only the
    /// lifetime matters here.
    ///
    /// Set on `message_start` for an `assistant` message (`:3129-3141`) and cleared the moment that
    /// message is finalized (`this.streamingComponent = undefined`, `:3213`). It is the guard Pi's
    /// `message_end` arm opens with (`if (this.streamingComponent && event.message.role ===
    /// "assistant")`, `:3182`), and it is what keeps a defensively-handled terminal
    /// `StreamEvent::Done` inside `message_update` from committing the same message twice.
    pub streaming_assistant: bool,
    /// The working directory whose basename goes into the automatic terminal title — Pi
    /// `sessionManager.getCwd()` (`interactive-mode.ts:819`). Seeded from the process cwd and
    /// re-pointed at the live session's cwd by [`App::run`] (and on every session swap), since a
    /// `/resume` of a session recorded elsewhere moves it.
    pub title_cwd: PathBuf,
    /// The custom header content an extension published — Pi `setHeader(factory)` →
    /// `setExtensionHeader` (`interactive-mode.ts:2262-2290` @v0.83.0), which splices the custom
    /// header into `headerContainer` in place of `builtInHeader` and restores the built-in when the
    /// factory is `undefined`. TUI-033: rendered as the first rows of the message region.
    pub extension_header: Option<String>,
    /// The custom footer content an extension published — Pi `setFooter(factory)` →
    /// `setExtensionFooter` (`:2235-2257`), which clears `footerContainer` and swaps the extension
    /// component in for the built-in footer. TUI-033: rendered in place of the [`StatusLine`] rows.
    pub extension_footer: Option<String>,
    /// The extension widgets currently mounted, keyed by Pi's `key` — `setExtensionWidget`
    /// (`interactive-mode.ts:1920-1960` @v0.83.0) keeps two maps, `extensionWidgetsAbove` and
    /// `extensionWidgetsBelow`, removes the key from BOTH before re-inserting, and drops it entirely
    /// when `content` is `undefined`. TUI-014.
    ///
    /// Pi's three `setWidget(key, content, options)` arguments arrive separately since SEAM-011
    /// widened the WIT; the in-process [`UiEffect::SetWidget`] carrier re-packs them under pi's own
    /// `key`/`lines`/`placement` names (`host_services.rs:150-161`), which
    /// [`ExtensionWidget::from_json`] reads back field by field.
    pub extension_widgets: Vec<ExtensionWidget>,
    /// Whether a branch summarization spawned by [`App::begin_tree_navigation`] is still in flight.
    /// While set, `Esc` routes to `AgentSession::abort_branch_summary` instead of the turn abort —
    /// Pi's `defaultEditor.onEscape = () => this.session.abortBranchSummary()`
    /// (`interactive-mode.ts:4792-4795`), restored in its `finally`.
    pub(super) branch_summary_in_flight: bool,
    /// The footer's git-branch source (Pi's `FooterDataProvider`, `footer-data-provider.ts`), which
    /// is what fills [`StatusLine::branch`]. Boots as "no repo" and is pointed at the session cwd by
    /// [`App::set_footer_git_cwd`]; the run loop re-polls it so a `git checkout` elsewhere repaints.
    pub git_branch: crate::footer_data::FooterGitBranch,
    /// The `AuthSelectorProvider[]` backing the open `/login` picker — Pi's `providerOptions` local
    /// (`showLoginProviderSelector`, `interactive-mode.ts:5086-5148`). Confirming carries the row
    /// INDEX into this vector, because one provider can contribute two rows (oauth + api key) and
    /// the provider id alone cannot disambiguate them.
    pub(super) login_options: Vec<cyrup_config::login::LoginProviderOption>,
    /// The `/logout` twin of [`Self::login_options`] (`getLogoutProviderOptions`,
    /// `interactive-mode.ts:4970-4979`). Carries each row's `authType`, which picks between Pi's two
    /// logout status messages (`interactive-mode.ts:5159-5162`).
    pub(super) logout_options: Vec<cyrup_config::login::LoginProviderOption>,
    /// The provider options an open [`SelectorKind::LoginAuthType`] selector is choosing BETWEEN —
    /// Pi's `providerOptions?` argument to `showLoginAuthTypeSelector`
    /// (`interactive-mode.ts:5028`). `None` for a bare `/login` (the method choice then opens the
    /// provider picker filtered to it, `:5063-5070`); `Some` when `/login <provider>` already
    /// pinned one provider that offers both methods (`:4998-5009`).
    pub(super) login_auth_type_options: Option<Vec<cyrup_config::login::LoginProviderOption>>,
    /// The REPLY half of the login prompt the flow is currently blocked on — the login twin of
    /// [`Self::pending_ui_reply`]. The spawned login task's `AuthInteraction::prompt` awaits this
    /// one-shot (`login_dialog::TuiAuthInteraction::prompt`, Pi's `inputResolver`/`inputRejecter`
    /// pair, `login-dialog.ts:16-17`); [`App::confirm_selector`] resolves it with the typed answer
    /// and [`App::handle_selector_key`]'s `Cancel` arm rejects it with `"Login cancelled"`.
    pub(super) pending_login_prompt: Option<tokio::sync::oneshot::Sender<Result<String, OAuthError>>>,
    /// The dialog's `AbortController` (`login-dialog.ts:15`, `:73-75`) for the flow currently on
    /// screen: `cancel()` fires it so a flow blocked on something other than a prompt (a callback
    /// server, a device-code poll) also unwinds. `None` whenever no login is in flight.
    pub(super) login_cancel: Option<CancelToken>,
    /// Provider ids whose STORED credential is an OAuth one — cyrup's standing copy of the half of
    /// pi's `modelRuntime.snapshot.auth` that `isUsingOAuth` reads
    /// (`model-runtime.ts:458-460`, pi v0.84.1: `this.snapshot.auth.get(providerId)?.type ===
    /// "oauth"`).
    ///
    /// Pi can answer that question synchronously at footer-render time because the snapshot is an
    /// in-memory map the runtime keeps warm; cyrup's equivalent read
    /// ([`cyrup_config::login::stored_credentials`]) parses `auth.json` and is `async`, while the
    /// footer is folded from a **sync** `&mut self` (`ingest_event_rendered`). So the map is cached
    /// here and refreshed at exactly the points pi's own snapshot moves: session bind/swap and a
    /// settled `/login` or `/logout` (each of which ends in `footer.invalidate()`,
    /// `interactive-mode.ts:5449`, `:5475`). See [`App::refresh_auth_snapshot`].
    pub(super) oauth_credential_providers: std::collections::BTreeSet<String>,
}

impl AppState {
    /// Fresh state with the given theme.
    pub fn new(theme: UiTheme) -> Self {
        // `if (areExperimentalFeaturesEnabled()) statsParts.push(… "xp" …)` (`footer.ts:162-164`).
        // Upstream re-reads `process.env.PI_EXPERIMENTAL` inside `render()`; cyrup reads it once
        // here, which is the only production writer of the flag — `set_experimental` had no caller
        // outside a test, so the `• xp` marker was unreachable however the user launched.
        let mut status = StatusLine::default();
        status.set_experimental(crate::status::experimental_features_enabled());
        AppState {
            transcript: TranscriptView::new(),
            editor: InputEditor::new(),
            status,
            indicator: StatusIndicator::new(),
            color_mode: theme.color_mode,
            theme,
            keymap: Keymap::default(),
            select_keymap: SelectKeymap::default(),
            tree_keymap: TreeKeymap::default(),
            session_keymap: SessionKeymap::default(),
            models_keymap: ModelsKeymap::default(),
            commands: CommandRegistry::new(),
            selector: None,
            overlays: Vec::new(),
            thinking_level: "medium".to_string(),
            show_images: true,
            image_renderer: ImageRenderer::default(),
            pending_images: Vec::new(),
            pending_messages: crate::pending_messages::PendingMessages::default(),
            session_queue: (Vec::new(), Vec::new()),
            compaction_flush_pending: false,
            compacting: false,
            compaction_queue: Vec::new(),
            reserve_status_rows: false,
            term_rows: 24,
            show_startup_hints: true,
            loader: None,
            loader_tick: 0,
            should_quit: false,
            last_sigint: None,
            last_escape: None,
            // Pi's own default is `"tree"` (`settings.ts` `getDoubleEscapeAction`); the real value
            // is seeded from the session's effective settings before the first frame.
            double_escape_action: "tree".to_string(),
            // Pi's `?? true` default (`settings-selector.ts:134`); re-seeded from the session's
            // effective settings before the first frame.
            warn_anthropic_extra_usage: true,
            pending_swap_status: None,
            #[cfg(any(test, feature = "scrollback-accumulator"))]
            scrollback: Vec::new(),
            extension_shortcuts: Vec::new(),
            capabilities: TerminalCapabilities {
                images: None,
                true_color: true,
                hyperlinks: false,
            },
            pending_ui_reply: None,
            editor_mirror: cyrup_session_svc::EditorTextMirror::new(),
            theme_access: None,
            pending_tree_nav: None,
            terminal_title: None,
            // Off until a session binds and `terminal.showTerminalProgress` is read ([`App::run`]).
            // Pi has no seed at all — it re-reads the setting at each of its five call sites.
            terminal_progress: crate::TerminalProgress::default(),
            streaming_assistant: false,
            // Pi reads `sessionManager.getCwd()` at title time; the process cwd is the same value
            // until a session with a recorded cwd is bound, which re-points it ([`App::run`]).
            title_cwd: std::env::current_dir().unwrap_or_default(),
            extension_header: None,
            extension_footer: None,
            extension_widgets: Vec::new(),
            branch_summary_in_flight: false,
            // Pi constructs its `FooterDataProvider` from the session cwd; the binary points this at
            // the runtime's cwd via [`App::set_footer_git_cwd`] before the first frame. Booting as
            // "no repo" keeps a backend-only `AppState` free of any filesystem probe.
            git_branch: crate::footer_data::FooterGitBranch::none(),
            login_options: Vec::new(),
            logout_options: Vec::new(),
            login_auth_type_options: None,
            pending_login_prompt: None,
            login_cancel: None,
            oauth_credential_providers: std::collections::BTreeSet::new(),
        }
    }

    /// Whether a `/tree` branch summarization is still running (test/inspection access; drives the
    /// `Esc`→`abort_branch_summary` routing).
    pub fn branch_summary_in_flight(&self) -> bool {
        self.branch_summary_in_flight
    }

    /// Install the extension-registered keyboard shortcuts (R-08-017): each raw key-id is parsed to a
    /// [`Key`] spec (unparseable ids are dropped, never panicking) and retained with its id so a
    /// matching press routes to the owning extension. Called by the binary at boot and after a
    /// session swap, so a `/reload` that changes the registered set takes effect.
    ///
    /// Accepts either a bare key-id (`ExtensionHost::shortcut_keys()`'s `Vec<String>`) or an
    /// `(id, description)` pair — see [`ShortcutSpec`] for why both forms exist.
    pub fn set_extension_shortcuts(&mut self, specs: impl IntoIterator<Item = impl Into<ShortcutSpec>>) {
        self.extension_shortcuts = specs
            .into_iter()
            .map(Into::into)
            .filter_map(|spec| Key::parse(&spec.id).ok().map(|k| (k, spec)))
            .collect();
    }
}

/// The `/tree` navigation awaiting the "Summarize branch?" answer (see
/// [`AppState::pending_tree_nav`]).
#[derive(Clone, Debug)]
pub(crate) struct PendingTreeNav {
    /// The confirmed tree row's entry id.
    pub(crate) target: String,
}

pub(crate) const BRANCH_SUMMARY_NONE: &str = "none";

pub(crate) const BRANCH_SUMMARY_YES: &str = "summarize";

pub(crate) const BRANCH_SUMMARY_CUSTOM: &str = "custom";

/// One extension-registered keyboard shortcut as the TUI holds it — the display-side half of
/// upstream's `ExtensionShortcut` record (`coding-agent/src/core/extensions/types.ts:1547-1552`:
/// `shortcut: KeyId`, `description?: string`, `handler`, `extensionPath: string`). The handler lives
/// on the guest side of the WASM boundary and never crosses into the TUI; the id and the label are
/// what `/hotkeys` and the dispatcher need.
///
/// Both `From` impls exist because the two callers differ: the binary installs
/// `ExtensionHost::shortcut_keys()` (`crates/cyrup/src/main.rs:1634`), a `Vec<String>` of bare ids,
/// while a host that also carries the registered description installs `(id, description)` pairs.
/// A bare id therefore keeps working unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutSpec {
    /// `shortcut: KeyId` (`types.ts:1548`) — the raw id the host routes a fired press back on.
    pub id: String,
    /// `description ?? extensionPath` (`interactive-mode.ts:6193`) — the `/hotkeys` Action cell.
    /// `None` when the registering host supplied neither.
    pub description: Option<String>,
}

impl From<String> for ShortcutSpec {
    fn from(id: String) -> Self {
        ShortcutSpec { id, description: None }
    }
}

impl From<&str> for ShortcutSpec {
    fn from(id: &str) -> Self {
        ShortcutSpec { id: id.to_string(), description: None }
    }
}

impl From<(String, String)> for ShortcutSpec {
    fn from((id, description): (String, String)) -> Self {
        ShortcutSpec { id, description: Some(description) }
    }
}

impl From<(String, Option<String>)> for ShortcutSpec {
    fn from((id, description): (String, Option<String>)) -> Self {
        ShortcutSpec { id, description }
    }
}

/// The active editor-swap selector plus the state needed to restore the editor on close (spec/tui/05
/// §7 `ActiveSelector`). Pi snapshots the editor text on open (`interactive-mode.ts:2371`) and, for
/// the theme picker, restores the prior theme on cancel (`theme-selector.ts` caller responsibility).
pub struct ActiveSelector {
    pub(crate) kind: SelectorKind,
    pub(crate) inner: Box<dyn Selector>,
    /// Editor text snapshotted on open, re-applied when the slot closes.
    pub(crate) saved_editor: String,
    /// Theme to restore if a previewing selector is cancelled (theme picker only).
    pub(crate) restore_theme: Option<UiTheme>,
}

/// The REQUEST/REPLY pairing an open extension-UI dialog (`SelectorKind::Extension{Confirm,Select,
/// Input}`) resolves against (L4 review §2.1): `kind` is retained so a `Cancel` can resolve to the
/// correct per-kind deny default ([`default_ui_reply`]) without re-deriving it from the selector kind
/// at the call site.
pub(crate) struct PendingUiReply {
    pub(crate) kind: UiKind,
    pub(crate) reply: tokio::sync::oneshot::Sender<UiReply>,
    /// The dialog's title WITHOUT any countdown suffix, so each tick recomputes `"{base_title}
    /// ({s}s)"` fresh off the current remaining time rather than accumulating appended text.
    pub(crate) base_title: String,
    /// When this dialog auto-resolves to its per-kind deny default if the user hasn't answered by
    /// then — Pi's `CountdownTimer` (`countdown-timer.ts:7-38`), armed from the guest's
    /// `opts.timeout_ms` exactly like `LiveHostServices::ui_roundtrip`'s OWN independent host-side
    /// timeout race (`host_services.rs`) arms from the SAME value; the two are deliberately separate
    /// clocks (mirroring Pi's `createDialogPromise`'s host-armed `setTimeout` vs. the renderer's own
    /// `CountdownTimer`, `rpc-mode.ts:114-119`) — whichever fires first wins the reply, and the loser
    /// finds it a harmless no-op. `None` when the guest set no timeout (dialog waits indefinitely for
    /// a key, matching `ui_roundtrip`'s own `None` branch).
    pub(crate) deadline: Option<tokio::time::Instant>,
}

/// The per-kind deny default a dialog resolves to when the user cancels it (`Esc`) rather than
/// answering — Pi's `noOpUIContext` shape (`runner.ts:230-261`), the same mapping
/// `crates/cyrup-modes/src/rpc.rs`'s `default_ui_reply` uses for a timed-out/force-resolved RPC dialog.
pub(crate) fn default_ui_reply(kind: UiKind) -> UiReply {
    match kind {
        UiKind::Confirm => UiReply::Confirm(false),
        UiKind::Input | UiKind::Editor | UiKind::Select => UiReply::Text(None),
    }
}

/// Format `base` with a live "(Ns)" countdown suffix — Pi's `CountdownTimer`'s exact title format
/// (`` `${this.baseTitle} (${s}s)` ``, `countdown-timer.ts:14,23,55`). Rounds UP (Pi's own
/// `Math.ceil(timeoutMs / 1000)`, `countdown-timer.ts:18`) so e.g. 4500ms remaining reads "5s", not
/// "4s"; a `deadline` already in the past reads "0s" (the tick loop closes the dialog that same
/// pass, so this is never rendered for more than one frame).
///
/// `now` is the instant of the tick being rendered — Pi's `CountdownTimer` decrements
/// `remainingSeconds` inside the `setInterval` callback (`countdown-timer.ts:22-24`), i.e. the
/// displayed value belongs to the tick, not to whenever the string happens to be formatted. Reading
/// the clock in here instead left [`App::tick_extension_dialog_countdown_at`]'s injected instant
/// governing only the expiry branch, so a ticked-forward countdown still printed its opening value.
pub(crate) fn countdown_title(base: &str, deadline: tokio::time::Instant, now: tokio::time::Instant) -> String {
    let remaining = deadline.saturating_duration_since(now);
    let secs = remaining.as_millis().div_ceil(1000);
    format!("{base} ({secs}s)")
}
