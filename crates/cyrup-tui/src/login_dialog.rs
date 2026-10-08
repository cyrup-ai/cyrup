//! `/login` — the in-slot login dialog and the TUI-backed [`AuthInteraction`] that drives a real
//! OAuth / API-key flow from the interactive front-end (arch-10 §3.3; spec/tui/05 §6).
//!
//! Ports pi v0.83.0:
//!
//! | this module | pi source |
//! |---|---|
//! | [`LoginDialog`] | `coding-agent/src/modes/interactive/components/login-dialog.ts:11-236` (`LoginDialogComponent`) |
//! | [`LoginDialog::show_auth`] | `login-dialog.ts:96-113` |
//! | [`LoginDialog::show_device_code`] | `login-dialog.ts:118-131` |
//! | [`LoginDialog::show_manual_input`] | `login-dialog.ts:136-148` |
//! | [`LoginDialog::show_prompt`] | `login-dialog.ts:154-172` |
//! | [`LoginDialog::show_details`] | `login-dialog.ts:175-182` |
//! | [`LoginDialog::show_info`] | `login-dialog.ts:185-201` |
//! | [`LoginDialog::show_waiting`] | `login-dialog.ts:207-211` |
//! | [`LoginDialog::show_progress`] | `login-dialog.ts:217-220` |
//! | [`LoginDialog::show_select`] | `interactive-mode.ts:5294-5325` (`showAuthSelect`) |
//! | [`TuiAuthInteraction`] | `interactive-mode.ts:5327-5375` (`showAuthPrompt` + `notifyAuthDialog` + `loginProvider`) |
//! | [`notify_auth_dialog`] | `interactive-mode.ts:5350-5360` (`notifyAuthDialog`) |
//! | the auth-URL rows + `app.message.copy` | `components/auth-url.ts` (`AuthUrlComponent`) and `login-dialog.ts:16-17`, `:226-233` @v1.1.0 (pi `ced72c2f0`, v1.0.1) |
//!
//! ## Why the flow runs off-task
//!
//! pi's `loginProvider` is `await`ed inside an `async` command handler while its event loop keeps
//! servicing keystrokes, because the `prompt`/`notify` callbacks it hands `ModelRuntime.login` are
//! plain closures that resolve a `Promise` the *editor component* settles later. Rust's run loop is
//! a single `select!` on one task, so awaiting the login inline in `App::run` would service no key
//! events for the whole flow — no prompt could ever be answered and the login would deadlock. The
//! flow therefore runs on a spawned task and talks to the loop over [`LoginUiMsg`], exactly the
//! channel-back shape `/tree`'s spawned navigation (`TreeNavMsg`) already uses. Behaviour is
//! upstream's; only the transport differs.
//!
//! ## Mechanism divergences (behaviour is the upstream one)
//!
//! * **A `select` prompt renders INSIDE the dialog.** pi swaps an `ExtensionSelectorComponent` into
//!   `editorContainer` and restores the dialog afterwards (`showAuthSelect`,
//!   `interactive-mode.ts:5294-5325`); cyrup's input slot holds exactly one occupant
//!   (`AppState::selector`), so the option list is drawn in the dialog's own body. The observable
//!   contract is identical: the prompt message is the header, the option **labels** are the rows,
//!   confirming answers with the option **id**, and cancelling rejects the login with
//!   `"Login cancelled"` (`:5314-5319`).
//! * **OSC-8 is written at paint time.** pi wraps the auth URL and the click hint in
//!   `hyperlink(text, url)` (`auth-url.ts:19`, `:27` @v1.1.0; `showDeviceCode`/`showInfo`,
//!   `login-dialog.ts:118-131`, `:185-201`); cyrup cannot put the escape in the line text
//!   (`osc.rs` says why), so the rows are tagged through [`crate::osc::LinkSink`] and
//!   [`crate::osc::inject_in`] wraps the painted cells — one open/close pair per wrapped row, so a
//!   long authorize URL is clickable on every row it spans. Gated on the process-wide
//!   `getCapabilities().hyperlinks` exactly as upstream's `hyperlink` is.
//! * **The URL is copyable as a whole.** A PKCE authorize URL is several hundred columns, and once it
//!   wraps a terminal selection over it yields one line per row — useless to paste over SSH or in
//!   tmux. pi v1.0.1 (`ced72c2f0`) answers that with `app.message.copy` (default `ctrl+x`) on the
//!   sign-in screen, which copies the full URL (`copyToClipboard`, OSC 52 over SSH) and swaps the
//!   hint for "Copied URL to clipboard" or the clipboard error. cyrup routes the key out of the
//!   dialog as a tagged [`SelectorOutcome::Apply`] (the clipboard write is `async` and lives on the
//!   run loop, the `/tree` copy's shape) and settles it with
//!   [`LoginDialog::set_auth_url_copy_result`]. [`LoginDialog::show_auth`] **does** call pi's
//!   `openBrowser(url)` (`:111`) through [`crate::open_browser`]; [`LoginDialog::show_device_code`]
//!   deliberately does not, matching `:118-131`.
//! * **`[CYRUP-DELTA]` A `manual_code` prompt keeps its placeholder.** pi's `showAuthPrompt` calls
//!   `showManualInput(prompt.message)` and drops `prompt.placeholder` (`interactive-mode.ts:6282-6283`
//!   @v1.1.0). The flows put the expected shape there — Anthropic's copy-code login says
//!   `code#state` (`anthropic.ts:221`), its browser login the redirect URL — and on a headless host
//!   that hint is the only description of what to paste, so cyrup shows it as the empty field's
//!   muted placeholder. It adds no row and vanishes on the first keystroke.
//! * **Secrets are not masked**, matching upstream — pi's dialog uses a plain `Input` for every
//!   prompt kind including `secret` (`login-dialog.ts:54`, `:154-172`).

use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use cyrup_core::CancelToken;
use cyrup_provider::auth::oauth::{
    AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, OAuthError,
};

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{
    Selector, SelectorOutcome, border_rule, search_input_spans, title_lines, title_wrapped_height,
};
use crate::theme::UiTheme;

/// The click affordance pi appends under an auth URL (`login-dialog.ts:100`, `:123`). pi branches on
/// `process.platform === "darwin"`; cyrup resolves the same branch from `cfg!(target_os)`.
fn click_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+click to open"
    } else {
        "Ctrl+click to open"
    }
}

/// How one accumulated dialog line is coloured. Mirrors pi's `theme.fg(<role>, …)` calls one-for-one
/// (`login-dialog.ts`), so the role — not a hardcoded colour — is what this module records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginLineKind {
    /// `theme.fg("text", …)` — the prompt message / info body.
    Text,
    /// `theme.fg("accent", …)` — URLs and links.
    Accent,
    /// `theme.fg("dim", …)` — click hints, progress, waiting, key hints.
    Dim,
    /// `theme.fg("warning", …)` — instructions and the device user-code line.
    Warning,
    /// A blank spacer (`new Spacer(1)`).
    Spacer,
    /// The auth URL's hint row (`auth-url.ts:27` @v1.1.0): `dim(hyperlink(clickHint, url))`, a
    /// dim `•`, then the copy hint or the copy's outcome. The stored text is the click hint; the
    /// rest is drawn from the dialog's copy state.
    AuthHint,
}

/// How the auth-URL hint row's tail is coloured (`auth-url.ts:22`, `:33`, `:35`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyHint {
    /// `keyHint("app.message.copy", "to copy")` — dim key, muted description.
    Key,
    /// `theme.fg("success", "Copied URL to clipboard")`.
    Copied,
    /// `theme.fg("error", error.message)`.
    Failed,
}

impl LoginLineKind {
    fn style(self, theme: &UiTheme) -> Style {
        match self {
            LoginLineKind::Text => theme.base_style(),
            LoginLineKind::Accent => theme.accent_style(),
            LoginLineKind::Dim | LoginLineKind::AuthHint => theme.dim_style(),
            LoginLineKind::Warning => theme.warning_style(),
            LoginLineKind::Spacer => theme.base_style(),
        }
    }
}

/// The active free-text prompt (pi's shared `Input`, `login-dialog.ts:54`) — literally the shared
/// [`crate::text_input::Input`] now, so a credential field has the same word motion / kill ring /
/// undo / paste as every search box.
struct LoginInput {
    input: crate::text_input::Input,
    placeholder: Option<String>,
}

/// The active `select` prompt, rendered in-dialog (see the module divergence note).
struct LoginSelect {
    /// `(id, label)` — confirming answers with the **id** (`types.ts:156`, `:5313`).
    options: Vec<(String, String)>,
    index: usize,
}

/// The login dialog occupying the input slot for the whole flow — pi's `LoginDialogComponent`
/// (`login-dialog.ts:11-236`).
///
/// Content **accumulates**: `showPrompt`/`showInfo`/`showProgress`/`showWaiting` append, while
/// `showAuth`/`showDeviceCode`/`showDetails` clear first (`login-dialog.ts:97`, `:119`, `:177`) —
/// which is what keeps a device code on screen while its "Waiting for authentication…" line is
/// added underneath, and what the `showPrompt` doc comment upstream calls out explicitly
/// (`login-dialog.ts:152-153`).
pub struct LoginDialog {
    /// `` `Login to ${providerName}` `` unless overridden (`login-dialog.ts:41`).
    title: String,
    lines: Vec<(LoginLineKind, String)>,
    input: Option<LoginInput>,
    select: Option<LoginSelect>,
    /// The live `tui.select.cancel` label behind pi's `keyHint("tui.select.cancel", …)`
    /// (`login-dialog.ts:141`, `:164`, `:198`, `:209`).
    cancel_hint: String,
    /// The live `tui.select.confirm` label (`login-dialog.ts:164`).
    confirm_hint: String,
    /// The browser launcher [`show_auth`](LoginDialog::show_auth) calls — pi's
    /// `openBrowser(url)` (`login-dialog.ts:111`). A field rather than a direct call so the
    /// invocation is observable in a unit test without a live desktop session; the default is the
    /// real [`crate::open_browser::open_browser`] and nothing but a test replaces it (DRIFT-042).
    launch_browser: std::sync::Arc<dyn Fn(&str) + Send + Sync>,
    /// The href each entry of `lines` links to, index for index — pi's `hyperlink(text, url)` on
    /// the auth URL, its click hint, a device-code URI and an info link. Painted as OSC-8 by
    /// [`Selector::render`] when the terminal supports it.
    line_links: Vec<Option<String>>,
    /// `private authUrl?: AuthUrlComponent` (`login-dialog.ts:16-17` @v1.1.0) — the shown sign-in
    /// URL, which `app.message.copy` copies. Set by `showAuth`, cleared by `showDeviceCode` and
    /// `showDetails`.
    auth_url: Option<String>,
    /// Every key bound to `app.message.copy` (pi `kb.matches(data, "app.message.copy")`,
    /// `login-dialog.ts:230`). Supplied by the app from the LIVE global keymap.
    copy_keys: Vec<crate::keymap::Key>,
    /// `keyText("app.message.copy")` for the `… to copy` hint (`auth-url.ts:22`); `None` when the
    /// action is unbound, in which case no copy hint is drawn.
    copy_label: Option<String>,
    /// What the last copy of `auth_url` did — `AuthUrlComponent.setHint` after `copy()`
    /// (`auth-url.ts:31-38`): `Ok` is "Copied URL to clipboard", `Err` the clipboard's message.
    copy_result: Option<Result<(), String>>,
    /// Test override for the OSC-8 capability; `None` reads the process-wide cache.
    hyperlinks: Option<bool>,
}

impl LoginDialog {
    /// `new LoginDialogComponent(ui, providerId, onComplete, providerName, titleOverride)`
    /// (`login-dialog.ts:29-68`). `title` is already resolved by the caller —
    /// `` `Login to ${name}` `` for a login, `` `${name} setup` `` for the ambient dialog
    /// (`interactive-mode.ts:5245`).
    pub fn new(title: impl Into<String>, keymap: &SelectKeymap) -> Self {
        LoginDialog {
            title: title.into(),
            lines: Vec::new(),
            input: None,
            select: None,
            // `keyHint("tui.select.cancel", "to cancel")` / `keyHint("tui.select.confirm", "to
            // submit")` (`login-dialog.ts:141`, `:163`, `:199`, `:210`) resolve through `keyText`,
            // which joins EVERY bound key with `/` (`keybinding-hints.ts:29-36`). The stock cancel
            // set is `["escape", "ctrl+c"]` (`tui/src/keybindings.ts:149-152`), so the first-key
            // `key_label` printed `esc to cancel` and silently hid the second key the user can press.
            cancel_hint: keymap
                .keys_label(SelectAction::Cancel)
                .unwrap_or_else(|| "escape/ctrl+c".to_string()),
            confirm_hint: keymap
                .keys_label(SelectAction::Confirm)
                .unwrap_or_else(|| "enter".to_string()),
            // Inert in this crate's test build: the App-level login tests (`tests/login_flow.rs`)
            // build dialogs through `App` and cannot reach `with_browser_launcher`, so the real
            // launcher opened a browser tab per OAuth test on a desktop (`open` on macOS; a
            // headless `xdg-open` fails silently, which hid it). DRIFT-042's recording tests
            // still replace it explicitly.
            launch_browser: if cfg!(test) {
                std::sync::Arc::new(|_: &str| {})
            } else {
                std::sync::Arc::new(crate::open_browser::open_browser)
            },
            line_links: Vec::new(),
            auth_url: None,
            copy_keys: Vec::new(),
            copy_label: None,
            copy_result: None,
            hyperlinks: None,
        }
    }

    /// Arm `app.message.copy` on the sign-in URL (`login-dialog.ts:230-233` @v1.1.0): `keys` are
    /// the global bindings of [`crate::keymap::Action::MessageCopy`] and `label` their joined
    /// `keyText`. The app passes the live keymap, so a rebind of the copy key moves this hint too.
    #[must_use]
    pub fn with_copy_keys(mut self, keys: Vec<crate::keymap::Key>, label: Option<String>) -> Self {
        self.copy_keys = keys;
        self.copy_label = label.filter(|l| !l.is_empty());
        self
    }

    /// Force the OSC-8 capability instead of reading the process-wide cache (test seam).
    #[cfg(test)]
    pub(crate) fn with_hyperlinks(mut self, on: bool) -> Self {
        self.hyperlinks = Some(on);
        self
    }

    /// Replace the browser launcher (test seam for DRIFT-042). Production never calls this.
    #[cfg(test)]
    fn with_browser_launcher(mut self, f: std::sync::Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        self.launch_browser = f;
        self
    }

    /// The dialog's title (`` `Login to ${providerName}` ``, `login-dialog.ts:41`).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The accumulated body lines (test/inspection): `(role, text)` in render order.
    pub fn lines(&self) -> &[(LoginLineKind, String)] {
        &self.lines
    }

    /// Every body line's text joined by `\n` — the cheap assertion surface for tests. The auth
    /// URL's hint row reads as drawn, copy hint or copy result included.
    pub fn body_text(&self) -> String {
        self.lines
            .iter()
            .map(|(kind, t)| self.display_text(*kind, t))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The sign-in URL `app.message.copy` would copy, if one is shown.
    pub fn auth_url(&self) -> Option<&str> {
        self.auth_url.as_deref()
    }

    /// The armed free-text prompt's placeholder, if any (test/inspection).
    pub fn input_placeholder(&self) -> Option<&str> {
        self.input.as_ref().and_then(|i| i.placeholder.as_deref())
    }

    /// A line's text as drawn: everything but the auth-URL hint row is stored verbatim.
    fn display_text(&self, kind: LoginLineKind, text: &str) -> String {
        match (kind, self.auth_hint_suffix()) {
            (LoginLineKind::AuthHint, Some((suffix, _))) => format!("{text} \u{2022} {suffix}"),
            _ => text.to_string(),
        }
    }

    /// The part of the auth-URL hint row after the `•` (`auth-url.ts:27`): `keyHint("app.message.copy",
    /// "to copy")` until a copy settles, then the copy's outcome (`:33`, `:35`). `None` when the
    /// copy key is unbound and nothing has been copied.
    fn auth_hint_suffix(&self) -> Option<(String, CopyHint)> {
        match &self.copy_result {
            Some(Ok(())) => Some(("Copied URL to clipboard".to_string(), CopyHint::Copied)),
            Some(Err(message)) => Some((message.clone(), CopyHint::Failed)),
            None => self
                .copy_label
                .as_ref()
                .map(|key| (format!("{key} to copy"), CopyHint::Key)),
        }
    }

    /// Settle a copy of `url` — `AuthUrlComponent.copy()`'s `setHint` (`auth-url.ts:31-38`).
    /// Ignored when the dialog has since moved to a different URL (or none): upstream's update
    /// lands on the component that was copied, which is no longer on screen.
    pub fn set_auth_url_copy_result(&mut self, url: &str, result: Result<(), String>) {
        if self.auth_url.as_deref() == Some(url) {
            self.copy_result = Some(result);
        }
    }

    /// The current free-text buffer, or `None` when no text prompt is armed.
    pub fn input_text(&self) -> Option<&str> {
        self.input.as_ref().map(|i| i.input.value())
    }

    /// Whether a prompt (text or select) is currently awaiting an answer.
    pub fn is_prompting(&self) -> bool {
        self.input.is_some() || self.select.is_some()
    }

    /// The option ids of an armed `select` prompt, in row order (test/inspection).
    pub fn select_option_ids(&self) -> Vec<String> {
        self.select
            .as_ref()
            .map(|s| s.options.iter().map(|(id, _)| id.clone()).collect())
            .unwrap_or_default()
    }

    fn push(&mut self, kind: LoginLineKind, text: impl Into<String>) {
        self.push_linked(kind, text, None);
    }

    /// [`Self::push`] for a row pi wraps in `hyperlink(text, href)`.
    fn push_linked(&mut self, kind: LoginLineKind, text: impl Into<String>, href: Option<&str>) {
        self.lines.push((kind, text.into()));
        self.line_links.push(href.map(str::to_string));
    }

    fn spacer(&mut self) {
        self.push(LoginLineKind::Spacer, String::new());
    }

    /// `contentContainer.clear()`.
    fn clear_lines(&mut self) {
        self.lines.clear();
        self.line_links.clear();
    }

    /// `showAuth(url, instructions)` (`login-dialog.ts:96-113` @v0.84.2): clear, then the URL, the
    /// click hint, any instructions — **and `openBrowser(url)`** (`:111`), which is the last thing
    /// the method does before requesting a render.
    ///
    /// DRIFT-042: the launch was disclosed-but-absent here for the whole port. The OSC-8 wrapping
    /// of the URL and of the click hint is recorded in `line_links` and written at paint time
    /// ([`Selector::render`]); the launch is best-effort and silent when no browser exists
    /// ([`crate::open_browser`]), which is the copy-code login's normal case.
    pub fn show_auth(&mut self, url: &str, instructions: Option<&str>) {
        self.clear_lines();
        self.spacer();
        // `this.authUrl = new AuthUrlComponent(this.tui, url)` (`login-dialog.ts:102` @v1.1.0): the
        // URL as one logical row (`auth-url.ts:19`) — it wraps, but no character is inserted into
        // it — then the click hint with the copy hint after a `•` (`:20-27`).
        self.auth_url = Some(url.to_string());
        self.copy_result = None;
        self.push_linked(LoginLineKind::Accent, url, Some(url));
        self.push_linked(LoginLineKind::AuthHint, click_hint(), Some(url));
        if let Some(instructions) = instructions.filter(|s| !s.is_empty()) {
            self.spacer();
            self.push(LoginLineKind::Warning, instructions);
        }
        // `openBrowser(url)` (`login-dialog.ts:111`) — after the lines are staged, so the URL is on
        // screen even if the launcher wins the race. Best-effort: see `open_browser`.
        (self.launch_browser)(url);
    }

    /// `showDeviceCode(info)` (`login-dialog.ts:118-131`): clear, then the verification URI, the
    /// click hint, a spacer, and `` `Enter code: ${info.userCode}` ``.
    ///
    /// Deliberately does **not** open a browser: pi's `showDeviceCode` applies the same OSC-8
    /// wrapping as `showAuth` but has no `openBrowser` call (`login-dialog.ts:118-131` @v0.84.2).
    /// A device-code flow expects the user to move to another device, so launching here would be a
    /// divergence, not an improvement.
    pub fn show_device_code(&mut self, user_code: &str, verification_uri: &str) {
        // `this.authUrl = undefined` (`login-dialog.ts:118` @v1.1.0): the copy key does nothing on a
        // device-code screen.
        self.auth_url = None;
        self.copy_result = None;
        self.clear_lines();
        self.spacer();
        self.push_linked(
            LoginLineKind::Accent,
            verification_uri,
            Some(verification_uri),
        );
        self.push_linked(LoginLineKind::Dim, click_hint(), Some(verification_uri));
        self.spacer();
        self.push(LoginLineKind::Warning, format!("Enter code: {user_code}"));
    }

    /// `showManualInput(prompt)` (`login-dialog.ts:136-148`): reset the buffer, append the dim
    /// prompt + the input + the cancel hint. Does NOT clear — the auth URL stays visible above it,
    /// which is the whole point of the manual-code escape hatch.
    pub fn show_manual_input(&mut self, prompt: &str) {
        self.show_manual_code(prompt, None);
    }

    /// [`Self::show_manual_input`] plus the prompt's placeholder, drawn muted inside the empty field
    /// — the module's `[CYRUP-DELTA]` note says why (`code#state` is the copy-code login's only
    /// description of what to paste).
    pub fn show_manual_code(&mut self, prompt: &str, placeholder: Option<String>) {
        self.spacer();
        self.push(LoginLineKind::Dim, prompt);
        self.input = Some(LoginInput {
            input: crate::text_input::Input::new(),
            placeholder: placeholder.filter(|p| !p.is_empty()),
        });
        self.select = None;
    }

    /// `showPrompt(message, placeholder)` (`login-dialog.ts:154-172`): append the message, an
    /// `` `e.g., ${placeholder}` `` hint when one is given, the input, and the
    /// cancel/submit hints. Explicitly does not clear (`login-dialog.ts:152-153`).
    pub fn show_prompt(&mut self, message: &str, placeholder: Option<String>) {
        self.spacer();
        self.push(LoginLineKind::Text, message);
        if let Some(hint) = placeholder.as_deref().filter(|s| !s.is_empty()) {
            self.push(LoginLineKind::Dim, format!("e.g., {hint}"));
        }
        self.input = Some(LoginInput {
            input: crate::text_input::Input::new(),
            placeholder,
        });
        self.select = None;
    }

    /// `showAuthSelect` (`interactive-mode.ts:5294-5325`) rendered in-dialog: the prompt message
    /// heads a list of the option **labels**; confirming answers with the matching **id**.
    pub fn show_select(&mut self, message: &str, options: Vec<(String, String)>) {
        self.spacer();
        self.push(LoginLineKind::Text, message);
        self.input = None;
        self.select = Some(LoginSelect { options, index: 0 });
    }

    /// `showDetails(lines)` (`login-dialog.ts:175-182`): clear, then the given lines verbatim.
    pub fn show_details(&mut self, lines: &[String]) {
        // `this.authUrl = undefined` (`login-dialog.ts:180` @v1.1.0).
        self.auth_url = None;
        self.copy_result = None;
        self.clear_lines();
        self.spacer();
        for line in lines {
            self.push(LoginLineKind::Text, line.clone());
        }
    }

    /// `showInfo(message, links, showCloseHint)` (`login-dialog.ts:185-201`).
    pub fn show_info(
        &mut self,
        message: &str,
        links: &[(String, Option<String>)],
        close_hint: bool,
    ) {
        self.spacer();
        self.push(LoginLineKind::Text, message);
        for (url, label) in links {
            let text = match label.as_deref().filter(|s| !s.is_empty()) {
                Some(label) => format!("{label}: {url}"),
                None => url.clone(),
            };
            self.push_linked(LoginLineKind::Accent, text, Some(url.as_str()));
        }
        if close_hint {
            self.spacer();
            let hint = format!("({} to close)", self.cancel_hint);
            self.push(LoginLineKind::Dim, hint);
        }
    }

    /// `showWaiting(message)` (`login-dialog.ts:207-211`).
    pub fn show_waiting(&mut self, message: &str) {
        self.spacer();
        self.push(LoginLineKind::Dim, message);
        let hint = format!("({} to cancel)", self.cancel_hint);
        self.push(LoginLineKind::Dim, hint);
    }

    /// `showProgress(message)` (`login-dialog.ts:217-220`) — a bare dim line, no spacer.
    pub fn show_progress(&mut self, message: &str) {
        self.push(LoginLineKind::Dim, message);
    }

    /// `replaceInputWithSubmittedText(value)` (`login-dialog.ts:76-80`): once a prompt is answered
    /// the live field is replaced by a `> value` echo so the transcript of the login reads back.
    fn commit_input_echo(&mut self, value: &str) {
        self.input = None;
        self.select = None;
        self.push(LoginLineKind::Text, format!("> {value}"));
    }

    /// The hint row rendered under an armed prompt (`login-dialog.ts:141`, `:164`).
    fn hint_line(&self) -> Option<String> {
        if self.select.is_some() {
            return Some(format!(
                "({} to cancel, {} to select)",
                self.cancel_hint, self.confirm_hint
            ));
        }
        if self.input.is_some() {
            return Some(format!(
                "({} to cancel, {} to submit)",
                self.cancel_hint, self.confirm_hint
            ));
        }
        None
    }

    /// Body rows, excluding the top/title/bottom chrome: the accumulated lines, then the option
    /// list or the input, then the hint row.
    ///
    /// `theme` is `None` when the rows are being **measured** rather than drawn
    /// ([`Selector::desired_height`]) — the row texts are identical either way, so measuring
    /// through the same function is what guarantees the reserved height can never disagree with
    /// what renders (the invariant `title_wrapped_height` documents for the title area).
    fn body_lines(
        &self,
        width: u16,
        theme: Option<&UiTheme>,
        links: Option<&crate::osc::LinkSink>,
    ) -> Vec<Line<'static>> {
        let style = |pick: fn(&UiTheme) -> Style| theme.map(pick).unwrap_or_default();
        // `hyperlink(text, href)`: the row's style carries the link marker that
        // `osc::inject_in` turns into the escape once the cells exist.
        let linked = |base: Style, href: Option<&String>| match (links, href) {
            (Some(sink), Some(href)) => base.patch(sink.mark(href.clone())),
            _ => base,
        };
        let mut out: Vec<Line<'static>> = self
            .lines
            .iter()
            .enumerate()
            .map(|(i, (kind, text))| {
                let base = theme.map(|t| kind.style(t)).unwrap_or_default();
                let href = self.line_links.get(i).and_then(Option::as_ref);
                let mut spans = vec![
                    Span::styled(" ", base),
                    Span::styled(text.clone(), linked(base, href)),
                ];
                if *kind == LoginLineKind::AuthHint
                    && let Some((suffix, hint)) = self.auth_hint_suffix()
                {
                    spans.push(Span::styled(" \u{2022} ", base));
                    match hint {
                        // `keyHint`: `theme.fg("dim", key) + theme.fg("muted", " to copy")`.
                        CopyHint::Key => {
                            let key = self.copy_label.clone().unwrap_or_default();
                            let rest = suffix.strip_prefix(key.as_str()).unwrap_or(&suffix);
                            spans.push(Span::styled(key.clone(), base));
                            spans.push(Span::styled(rest.to_string(), style(UiTheme::muted_style)));
                        }
                        CopyHint::Copied => {
                            spans.push(Span::styled(suffix, style(UiTheme::success_style)));
                        }
                        CopyHint::Failed => {
                            spans.push(Span::styled(suffix, style(UiTheme::error_style)));
                        }
                    }
                }
                Line::from(spans)
            })
            .collect();
        if let Some(select) = &self.select {
            for (i, (_, label)) in select.options.iter().enumerate() {
                let selected = i == select.index;
                let marker = if selected { " → " } else { "   " };
                let style = if selected {
                    style(UiTheme::accent_style).add_modifier(Modifier::BOLD)
                } else {
                    style(UiTheme::base_style)
                };
                out.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
            }
        }
        if let Some(input) = &self.input {
            // S31: `LoginDialogComponent` adds its `Input` to `contentContainer` as a bare child
            // (`login-dialog.ts:140`, `:160`) — no `Text` wrapper — so the row is `Input.render`'s
            // shared, unstyled `"> "` at column 0 (`input.ts:380`). cyrup drew an accent `" > "`.
            let mut spans = vec![Span::styled(
                crate::selector::INPUT_PROMPT,
                style(UiTheme::base_style),
            )];
            match input.placeholder.as_deref().filter(|s| !s.is_empty()) {
                Some(hint) if input.input.value().is_empty() => {
                    spans.push(Span::styled(hint.to_string(), style(UiTheme::muted_style)));
                }
                _ => match theme {
                    Some(theme) => {
                        // pi's `availableWidth = width - prompt.length` (`input.ts:381`).
                        let available =
                            usize::from(width).saturating_sub(crate::selector::INPUT_PROMPT.len());
                        spans.extend(search_input_spans(
                            input.input.value(),
                            input.input.cursor(),
                            available,
                            theme,
                        ));
                    }
                    // Measurement: `search_input_spans` always draws a caret cell, so the widest
                    // form is the buffer plus one column — capped at the same `availableWidth` the
                    // render windows to (`input.ts:381,415`), or a value wider than the field would
                    // reserve rows the drawn (windowed) line never fills.
                    None => {
                        let available =
                            usize::from(width).saturating_sub(crate::selector::INPUT_PROMPT.len());
                        let cols = crate::text_width::str_width(input.input.value())
                            .saturating_add(1)
                            .min(available);
                        spans.push(Span::raw(" ".repeat(cols)));
                    }
                },
            }
            out.push(Line::from(spans));
        }
        if let Some(hint) = self.hint_line() {
            out.push(Line::from(Span::styled(
                format!(" {hint}"),
                style(UiTheme::dim_style),
            )));
        }
        out
    }

    /// The answer the currently-armed prompt would produce on `tui.select.confirm`, plus the echo
    /// text to record. `None` when nothing is armed (pi's `input.onSubmit` no-ops without an
    /// `inputResolver`, `login-dialog.ts:56-64`).
    fn confirm_answer(&self) -> Option<(String, String)> {
        if let Some(select) = &self.select {
            let (id, label) = select.options.get(select.index)?;
            return Some((id.clone(), label.clone()));
        }
        let input = self.input.as_ref()?;
        let answer = input.input.value().to_string();
        Some((answer.clone(), answer))
    }
}

impl Selector for LoginDialog {
    fn desired_height(&self, width: u16) -> u16 {
        // Top rule + wrapped title + wrapped body + bottom rule.
        let body = self.body_lines(width, None, None);
        let body_h = crate::transcript::wrapped_height(&body, usize::from(width))
            .min(usize::from(u16::MAX)) as u16;
        title_wrapped_height(&self.title, width)
            .saturating_add(body_h)
            .saturating_add(2)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = title_wrapped_height(&self.title, area.width);
        // `hyperlink()` is gated on `getCapabilities().hyperlinks` upstream; an incapable terminal
        // gets the same rows with no escape.
        let sink = crate::osc::LinkSink::new();
        let hyperlinks = self
            .hyperlinks
            .unwrap_or_else(crate::image::hyperlinks_supported);
        let body = self.body_lines(area.width, Some(theme), hyperlinks.then_some(&sink));
        let body_h = crate::transcript::wrapped_height(&body, usize::from(area.width))
            .min(usize::from(u16::MAX)) as u16;
        let [top, title_area, body_area, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(title_h),
            Constraint::Length(body_h),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(border_rule(top.width, theme), top);
        frame.render_widget(
            Paragraph::new(title_lines(&self.title))
                .style(theme.accent_style().add_modifier(Modifier::BOLD))
                .wrap(Wrap { trim: false }),
            title_area,
        );
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), body_area);
        // After the widget, so `Paragraph` measured plain cells (`osc::inject` says why).
        crate::osc::inject_in(frame.buffer_mut(), body_area, &sink);
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }

    /// The armed prompt's `Input` sits in `contentContainer` as a bare child
    /// (`login-dialog.ts:140`, `:160`), which forwards a press to it (`input.ts:229-243`); every
    /// other row of the dialog is a `Text` and takes nothing. The field's row is the rule, the
    /// title and the wrapped lines above it — the rows [`Self::render`] lays out.
    fn pointer(&mut self, area: Rect, event: crate::app::Pointer) -> SelectorOutcome {
        if self.input.is_none() {
            return SelectorOutcome::Ignored;
        }
        let body = self.body_lines(area.width, None, None);
        let above = body.get(..self.lines.len()).unwrap_or_default();
        let above_h = crate::transcript::wrapped_height(above, usize::from(area.width));
        let row = title_wrapped_height(&self.title, area.width)
            .saturating_add(1)
            .saturating_add(above_h.min(usize::from(u16::MAX)) as u16);
        let Some(field) = self.input.as_mut() else {
            return SelectorOutcome::Ignored;
        };
        if field
            .input
            .pointer_in_row(event, row, crate::selector::INPUT_PROMPT_COLS, area.width)
        {
            SelectorOutcome::Redraw
        } else {
            SelectorOutcome::Ignored
        }
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // `handleInput` (`login-dialog.ts:222-232`): the cancel binding aborts the WHOLE login
        // (`cancel()` → `abortController.abort()` + reject "Login cancelled"); everything else goes
        // to the input. The select prompt's own Esc rejects identically (`:5316-5319`).
        let action = keymap.action_for(key);
        // `if (this.authUrl && kb.matches(data, "app.message.copy")) { void this.authUrl.copy();
        // return; }` (`login-dialog.ts:230-233` @v1.1.0) — after cancel, before the input, so the
        // copy key never types into a paste field that happens to be armed.
        if action != Some(SelectAction::Cancel)
            && let Some(url) = self.auth_url.as_deref()
            && self.copy_keys.iter().any(|k| k.matches(key))
        {
            return SelectorOutcome::Apply(copy_auth_url_payload(url));
        }
        match action {
            Some(SelectAction::Cancel) => return SelectorOutcome::Cancel,
            Some(SelectAction::Confirm) => {
                let Some((answer, echo)) = self.confirm_answer() else {
                    return SelectorOutcome::Ignored;
                };
                self.commit_input_echo(&echo);
                return SelectorOutcome::Confirm(answer);
            }
            Some(SelectAction::Up) if self.select.is_some() => {
                if let Some(select) = self.select.as_mut() {
                    let len = select.options.len();
                    if len > 0 {
                        select.index = (select.index + len - 1) % len;
                    }
                }
                return SelectorOutcome::Redraw;
            }
            Some(SelectAction::Down) if self.select.is_some() => {
                if let Some(select) = self.select.as_mut() {
                    let len = select.options.len();
                    if len > 0 {
                        select.index = (select.index + 1) % len;
                    }
                }
                return SelectorOutcome::Redraw;
            }
            _ => {}
        }
        if self.input.is_none() {
            return SelectorOutcome::Ignored;
        }
        // Everything else is the shared single-line editing surface (`login-dialog.ts:231`
        // hands the key straight to the `Input`). `Left`/`Right`/`Home`/`End`/`Delete`/`Backspace`
        // still resolve here — they are `EditorKeymap::default()`'s `Key::plain(...)` bindings —
        // alongside the word motion, kill ring, undo and paste they never had.
        let Some(field) = self.input.as_mut() else {
            return SelectorOutcome::Ignored;
        };
        match field.input.handle_key(key) {
            crate::text_input::InputOutcome::Ignored => SelectorOutcome::Ignored,
            _ => SelectorOutcome::Redraw,
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn set_editor_keymap(&mut self, keymap: &crate::keymap::EditorKeymap) {
        if let Some(field) = self.input.as_mut() {
            field.input.set_editor_keymap(keymap);
        }
    }

    fn handle_paste(&mut self, text: &str) -> SelectorOutcome {
        let Some(field) = self.input.as_mut() else {
            return SelectorOutcome::Ignored;
        };
        field.input.paste(text);
        SelectorOutcome::Redraw
    }

    fn as_login_dialog(&mut self) -> Option<&mut LoginDialog> {
        Some(self)
    }
}

/// The tag on the [`SelectorOutcome::Apply`] payload the copy key produces — unit-separator
/// delimited like `/tree`'s copy payload, so it can never collide with a typed value.
const COPY_AUTH_URL_TAG: &str = "copy-auth-url";

/// `"\u{1f}copy-auth-url\u{1f}{url}"` — what [`LoginDialog`] hands the run loop when
/// `app.message.copy` is pressed over a sign-in URL.
fn copy_auth_url_payload(url: &str) -> String {
    let sep = crate::FIELD_SEP;
    format!("{sep}{COPY_AUTH_URL_TAG}{sep}{url}")
}

/// The URL a [`LoginDialog`] `Apply` payload asks to copy, or `None` for any other payload.
pub(crate) fn parse_copy_auth_url_payload(payload: &str) -> Option<&str> {
    let sep = crate::FIELD_SEP;
    payload
        .strip_prefix(sep)?
        .strip_prefix(COPY_AUTH_URL_TAG)?
        .strip_prefix(sep)
}

/// One message from the spawned login task to `App::run`'s `select!` loop.
///
/// The three variants are the three things pi's `AuthInteraction` object does: block on a prompt
/// (`prompt`), push progress (`notify`), and — because the whole call is `await`ed rather than
/// polled — settle (`loginProvider`'s `try`/`catch`, `interactive-mode.ts:5285-5296`,
/// `:5392-5403`).
#[derive(Debug)]
pub enum LoginUiMsg {
    /// `notify(event)` (`interactive-mode.ts:5364`) — fire-and-forget progress.
    Notify(Box<AuthEvent>),
    /// `prompt(prompt)` (`interactive-mode.ts:5363`) — the flow is blocked until `reply` is sent.
    Prompt {
        prompt: Box<AuthPrompt>,
        reply: tokio::sync::oneshot::Sender<Result<String, OAuthError>>,
    },
    /// The whole login settled — the `try`/`catch` around `loginProvider`.
    Finished(Box<LoginFinished>),
}

/// A settled login (`showLoginDialog`/`showApiKeyLoginDialog`'s `try`/`catch`,
/// `interactive-mode.ts:5285-5296` / `:5392-5403`).
#[derive(Clone, Debug)]
pub struct LoginFinished {
    /// The provider that was logged into.
    pub provider_id: String,
    /// `providerName` — the display name every status/error message interpolates.
    pub provider_name: String,
    /// Whether this was the `oauth` or the `api_key` leg; picks between pi's two message pairs.
    pub oauth: bool,
    /// `Ok(())` on success; `Err(message)` carries `error.message` verbatim
    /// (`interactive-mode.ts:5293`, `:5400`).
    pub result: Result<(), String>,
    /// Whether the failure was the user cancelling — pi's `errorMsg !== "Login cancelled"` guard
    /// (`interactive-mode.ts:5294`, `:5401`), which suppresses the error banner.
    pub cancelled: bool,
    /// `getAuthPath()` — the file the success status names (`interactive-mode.ts:5219`,
    /// `:5222`). Carried on the message because the settle half runs on the run loop with no
    /// session in scope; it is `<agent_dir>/auth.json` (`cyrup-config/src/env.rs:236-238`).
    pub auth_path: std::path::PathBuf,
}

/// **TUI-105.** A settled post-login model-catalog refresh, travelling from the spawned task back
/// to the run loop — pi's `session.modelRuntime.refresh({providers:[providerId], signal}).then(...)`
/// continuation (`interactive-mode.ts:5953-5971`).
///
/// Everything the `.then` closure reads off its enclosing scope travels on the message, because in
/// cyrup that closure is a separate task: `actionLabel`, `providerId`, `getAuthPath()`,
/// `deferSelection` and `previousModel`.
#[derive(Debug)]
pub struct LoginRefreshMsg {
    /// The `/login` generation this refresh belongs to — pi's `this.session === session` identity
    /// check (`interactive-mode.ts:5961`). A stale epoch is DROPPED.
    pub epoch: u64,
    /// `providerId` (`:5880`), for the deferred selection's catalog filter.
    pub provider_id: String,
    /// `actionLabel` (`:5885`) — every warning and status this refresh emits interpolates it.
    pub action: String,
    /// `getAuthPath()` (`:5954`), for the deferred selection's success status.
    pub auth_path: std::path::PathBuf,
    /// `deferSelection` (`:5889-5895`): whether the selection was postponed until this refresh
    /// landed.
    pub defer: bool,
    /// `previousModel` (`:5883`), for pi's `session.model === previousModel` guard (`:5961`).
    pub previous_model: Option<cyrup_core::ModelRef>,
    /// The refresh outcome — pi's `result` (`:5956-5960`).
    pub result: cyrup_provider::CatalogRefreshResult,
}

/// The TUI's [`AuthInteraction`] — pi's inline `{ signal, prompt, notify }` object
/// (`loginProvider`, `interactive-mode.ts:5367-5374`).
///
/// `signal` is the dialog's own `AbortController` (`login-dialog.ts:73-75`), which the cancel
/// binding fires; here that is the [`CancelToken`] `App` holds for the duration of the flow.
pub struct TuiAuthInteraction {
    tx: tokio::sync::mpsc::UnboundedSender<LoginUiMsg>,
    cancel: CancelToken,
}

impl TuiAuthInteraction {
    /// Bind an interaction to the run loop's login channel and the dialog's cancel token.
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<LoginUiMsg>, cancel: CancelToken) -> Self {
        TuiAuthInteraction { tx, cancel }
    }
}

#[async_trait::async_trait]
impl AuthInteraction for TuiAuthInteraction {
    fn cancel(&self) -> Option<&CancelToken> {
        Some(&self.cancel)
    }

    /// `showAuthPrompt` (`interactive-mode.ts:5327-5348`): hand the prompt to the dialog, then race
    /// the answer against the prompt's OWN signal — `if (prompt.signal.aborted) throw new
    /// Error("Login cancelled")` up front, then `Promise.race([response, aborted])`. That race is
    /// load-bearing: `openrouter.ts:274-283` cancels the `manual_code` prompt the moment the
    /// callback server wins, and without it the login would hang on a prompt nobody will answer.
    async fn prompt(&self, prompt: AuthPrompt) -> Result<String, OAuthError> {
        let prompt_cancel = prompt.cancel.clone();
        // `if (prompt.signal.aborted) throw new Error("Login cancelled")` (`:5334`).
        if prompt_cancel
            .as_ref()
            .is_some_and(CancelToken::is_cancelled)
            || self.cancel.is_cancelled()
        {
            return Err(OAuthError::Cancelled);
        }
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if self
            .tx
            .send(LoginUiMsg::Prompt {
                prompt: Box::new(prompt),
                reply: reply_tx,
            })
            .is_err()
        {
            // The run loop is gone: nothing can ever answer, so settle as upstream's cancel.
            return Err(OAuthError::Cancelled);
        }
        match prompt_cancel {
            // `Promise.race([response, aborted])` (`:5344`).
            Some(token) => tokio::select! {
                answer = reply_rx => match answer {
                    Ok(answer) => answer,
                    Err(_) => Err(OAuthError::Cancelled),
                },
                () = token.cancelled() => Err(OAuthError::Cancelled),
            },
            // `if (!prompt.signal) return response;` (`:5333`).
            None => match reply_rx.await {
                Ok(answer) => answer,
                Err(_) => Err(OAuthError::Cancelled),
            },
        }
    }

    /// `notify: (event) => this.notifyAuthDialog(dialog, event)` (`interactive-mode.ts:5373`).
    /// Never blocks and never fails; a dropped receiver means the dialog is already gone.
    fn notify(&self, event: AuthEvent) {
        let _ = self.tx.send(LoginUiMsg::Notify(Box::new(event)));
    }
}

/// `notifyAuthDialog(dialog, event)` (`interactive-mode.ts:5350-5360`) — the exact four-way branch,
/// including the `device_code` case's SECOND call (`showWaiting("Waiting for authentication...")`,
/// `:5355`) that keeps the cancel hint on screen while a device flow polls.
pub fn notify_auth_dialog(dialog: &mut LoginDialog, event: AuthEvent) {
    match event {
        AuthEvent::AuthUrl { url, instructions } => {
            dialog.show_auth(&url, instructions.as_deref());
        }
        AuthEvent::DeviceCode {
            user_code,
            verification_uri,
            ..
        } => {
            dialog.show_device_code(&user_code, &verification_uri);
            dialog.show_waiting("Waiting for authentication...");
        }
        AuthEvent::Info { message, links } => {
            let links: Vec<(String, Option<String>)> =
                links.into_iter().map(|l| (l.url, l.label)).collect();
            dialog.show_info(&message, &links, false);
        }
        AuthEvent::Progress { message } => dialog.show_progress(&message),
    }
}

/// `showAuthPrompt`'s kind dispatch (`interactive-mode.ts:5328-5332`): `select` opens the option
/// list, `manual_code` opens the bare manual-entry field (keeping its placeholder — the module's
/// `[CYRUP-DELTA]`), everything else (`text`, `secret`, and an absent `type`) opens the ordinary
/// message+placeholder prompt.
pub fn show_auth_prompt(dialog: &mut LoginDialog, prompt: &AuthPrompt) {
    match prompt.kind {
        Some(AuthPromptKind::Select) => {
            let options = prompt
                .options
                .iter()
                .map(|o| (o.id.clone(), o.label.clone()))
                .collect();
            dialog.show_select(&prompt.message, options);
        }
        Some(AuthPromptKind::ManualCode) => {
            dialog.show_manual_code(&prompt.message, prompt.placeholder.clone())
        }
        _ => dialog.show_prompt(&prompt.message, prompt.placeholder.clone()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use cyrup_provider::auth::oauth::{AuthInfoLink, AuthSelectOption};
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// Every dialog built by a test gets an INERT launcher. Without this, `show_auth` would open a
    /// real browser tab on the developer's desktop on every `cargo test` run (DRIFT-042).
    fn dialog() -> LoginDialog {
        recording_dialog().0
    }

    /// A dialog plus the list of URLs its launcher was handed, in call order.
    fn recording_dialog() -> (LoginDialog, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let calls: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let sink = std::sync::Arc::clone(&calls);
        let d = LoginDialog::new("Login to Anthropic", &SelectKeymap::default())
            .with_browser_launcher(std::sync::Arc::new(move |url: &str| {
                if let Ok(mut v) = sink.lock() {
                    v.push(url.to_string());
                }
            }));
        (d, calls)
    }

    /// **DRIFT-042.** `showAuth` opens the browser; `showDeviceCode` deliberately does not.
    ///
    /// **Red before the fix:** `LoginDialog` had no launcher at all — `login_dialog.rs:212`
    /// carried a comment saying "pi additionally calls `openBrowser(url)`" and nothing called it,
    /// and `grep -rnE 'xdg-open|rundll32|open_browser|FileProtocolHandler' crates --include='*.rs'`
    /// returned 0 across the workspace. `with_browser_launcher` did not exist, so this test did not
    /// compile; once it did, the `show_auth` assertion failed on an empty call list.
    #[test]
    fn show_auth_launches_the_browser_once_and_device_code_never_does() {
        let (mut d, calls) = recording_dialog();
        d.show_auth("https://example.test/auth", None);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["https://example.test/auth".to_string()],
            "`openBrowser(url)` (login-dialog.ts:111 @v0.84.2) runs exactly once, with the auth URL"
        );

        // `showDeviceCode` (`login-dialog.ts:118-131`) wraps the URI in OSC-8 but has NO
        // `openBrowser` call: the user is expected to move to another device.
        let (mut d, calls) = recording_dialog();
        d.show_device_code("WXYZ-1234", "https://example.test/device");
        assert!(
            calls.lock().unwrap().is_empty(),
            "showDeviceCode must not launch a browser"
        );
    }

    #[test]
    fn show_auth_clears_and_shows_url_plus_click_hint() {
        let mut d = dialog();
        d.show_progress("stale");
        d.show_auth("https://example.test/auth", Some("Approve in the browser"));
        let body = d.body_text();
        assert!(!body.contains("stale"), "showAuth clears content: {body}");
        assert!(body.contains("https://example.test/auth"));
        assert!(body.contains("click to open"));
        assert!(body.contains("Approve in the browser"));
    }

    #[test]
    fn device_code_shows_uri_code_and_waiting() {
        let mut d = dialog();
        notify_auth_dialog(
            &mut d,
            AuthEvent::DeviceCode {
                user_code: "WXYZ-1234".to_string(),
                verification_uri: "https://example.test/device".to_string(),
                interval_seconds: Some(5.0),
                expires_in_seconds: Some(900.0),
            },
        );
        let body = d.body_text();
        assert!(body.contains("https://example.test/device"));
        // pi's exact copy (`login-dialog.ts:128`).
        assert!(body.contains("Enter code: WXYZ-1234"), "{body}");
        // The SECOND call `notifyAuthDialog` makes for a device code (`interactive-mode.ts:5355`).
        assert!(body.contains("Waiting for authentication..."), "{body}");
    }

    #[test]
    fn prompt_appends_and_keeps_the_auth_url_visible() {
        let mut d = dialog();
        d.show_auth("https://example.test/auth", None);
        d.show_prompt("Paste the code", Some("abc123".to_string()));
        let body = d.body_text();
        // `showPrompt` does NOT clear (`login-dialog.ts:152-153`).
        assert!(body.contains("https://example.test/auth"), "{body}");
        assert!(body.contains("Paste the code"));
        assert!(body.contains("e.g., abc123"), "{body}");
        assert!(d.is_prompting());
    }

    #[test]
    fn typing_then_enter_confirms_and_echoes() {
        let mut d = dialog();
        d.show_prompt("Key?", None);
        let km = SelectKeymap::default();
        for c in "sk-1".chars() {
            assert_eq!(
                d.handle(&key(KeyCode::Char(c)), &km),
                SelectorOutcome::Redraw
            );
        }
        assert_eq!(d.input_text(), Some("sk-1"));
        assert_eq!(
            d.handle(&key(KeyCode::Enter), &km),
            SelectorOutcome::Confirm("sk-1".to_string())
        );
        // `replaceInputWithSubmittedText` (`login-dialog.ts:76-80`).
        assert!(d.body_text().contains("> sk-1"), "{}", d.body_text());
        assert!(!d.is_prompting(), "the field is retired after a submit");
    }

    #[test]
    fn escape_cancels_the_whole_login() {
        let mut d = dialog();
        d.show_prompt("Key?", None);
        assert_eq!(
            d.handle(&key(KeyCode::Esc), &SelectKeymap::default()),
            SelectorOutcome::Cancel
        );
    }

    #[test]
    fn select_prompt_answers_with_the_option_id_not_the_label() {
        let mut d = dialog();
        let prompt = AuthPrompt::select(
            "Pick an account",
            vec![
                AuthSelectOption {
                    id: "acct-1".to_string(),
                    label: "Personal".to_string(),
                    description: None,
                },
                AuthSelectOption {
                    id: "acct-2".to_string(),
                    label: "Work".to_string(),
                    description: None,
                },
            ],
        );
        show_auth_prompt(&mut d, &prompt);
        assert_eq!(d.select_option_ids(), vec!["acct-1", "acct-2"]);
        let km = SelectKeymap::default();
        assert_eq!(d.handle(&key(KeyCode::Down), &km), SelectorOutcome::Redraw);
        // `resolve(id)` where `id = options.find(o => o.label === optionLabel)?.id` (`:5313`).
        assert_eq!(
            d.handle(&key(KeyCode::Enter), &km),
            SelectorOutcome::Confirm("acct-2".to_string())
        );
    }

    #[test]
    fn enter_with_no_armed_prompt_is_a_no_op() {
        // pi's `input.onSubmit` early-returns without an `inputResolver` (`login-dialog.ts:56-64`).
        let mut d = dialog();
        d.show_info("Configured outside cyrup.", &[], true);
        assert_eq!(
            d.handle(&key(KeyCode::Enter), &SelectKeymap::default()),
            SelectorOutcome::Ignored
        );
    }

    #[test]
    fn info_links_render_label_and_url() {
        let mut d = dialog();
        notify_auth_dialog(
            &mut d,
            AuthEvent::Info {
                message: "Read the docs".to_string(),
                links: vec![AuthInfoLink {
                    url: "https://example.test/docs".to_string(),
                    label: Some("Docs".to_string()),
                }],
            },
        );
        assert!(d.body_text().contains("Docs: https://example.test/docs"));
    }

    // -- pi v1.0.1 `ced72c2f0`: `app.message.copy` on the sign-in URL (`auth-url.ts`) --------------

    /// A sign-in URL as long as a real PKCE authorize URL — pi's own fixture shape
    /// (`test/auth-url-copy.test.ts:14`).
    fn long_url() -> String {
        format!("https://auth.example.invalid/authorize?{}", "x".repeat(300))
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// The dialog the app opens: `app.message.copy` armed with its stock `ctrl+x`.
    fn copy_dialog() -> LoginDialog {
        dialog().with_copy_keys(
            vec![crate::keymap::Key::ctrl('x')],
            Some("ctrl+x".to_string()),
        )
    }

    /// pi `test/auth-url-copy.test.ts:34-43` — "login dialog copies the auth URL instead of typing
    /// into the code input": the hint says `ctrl+x to copy`, the key yields the WHOLE URL (here
    /// as the run loop's copy command), the paste field is left untouched, and the hint then reads
    /// `Copied URL to clipboard`.
    ///
    /// **Red before the fix:** the dialog had no copy key; `ctrl+x` fell through to the paste
    /// field and nothing on screen offered a way to get an unbroken copy of a wrapped URL.
    #[test]
    fn the_copy_key_copies_the_auth_url_instead_of_typing_into_the_code_input() {
        let url = long_url();
        let mut d = copy_dialog();
        d.show_auth(&url, None);
        d.show_manual_input("Paste the code:");
        assert!(
            d.body_text().contains("ctrl+x to copy"),
            "{}",
            d.body_text()
        );

        let outcome = d.handle(&ctrl('x'), &SelectKeymap::default());
        let SelectorOutcome::Apply(payload) = outcome else {
            panic!("expected the copy payload, got {outcome:?}");
        };
        assert_eq!(parse_copy_auth_url_payload(&payload), Some(url.as_str()));
        assert_eq!(d.input_text(), Some(""), "the key must not reach the field");

        d.set_auth_url_copy_result(&url, Ok(()));
        let body = d.body_text();
        assert!(body.contains("Copied URL to clipboard"), "{body}");
        assert!(
            !body.contains("to copy"),
            "the result replaces the hint: {body}"
        );
    }

    /// `auth-url.ts:34-36` — a failing clipboard shows its own message in the hint row.
    #[test]
    fn a_failed_copy_shows_the_clipboard_error_in_the_hint_row() {
        let url = long_url();
        let mut d = copy_dialog();
        d.show_auth(&url, None);
        d.set_auth_url_copy_result(&url, Err("Failed to copy to clipboard".to_string()));
        assert!(
            d.body_text().contains("Failed to copy to clipboard"),
            "{}",
            d.body_text()
        );
    }

    /// pi `test/auth-url-copy.test.ts:45-50` — "login dialog ignores the copy key without an auth
    /// URL": a device-code screen clears `authUrl` (`login-dialog.ts:118`).
    #[test]
    fn the_copy_key_does_nothing_without_an_auth_url() {
        let mut d = copy_dialog();
        d.show_auth("https://example.test/auth", None);
        d.show_device_code("ABCD", "https://example.invalid/device");
        assert_eq!(d.auth_url(), None);
        assert_eq!(
            d.handle(&ctrl('x'), &SelectKeymap::default()),
            SelectorOutcome::Ignored
        );
        let mut d = copy_dialog();
        d.show_auth("https://example.test/auth", None);
        d.show_details(&["Configure it elsewhere".to_string()]);
        assert_eq!(d.auth_url(), None, "showDetails clears it too (`:180`)");
    }

    /// A copy that settles after the dialog moved to a new URL lands on nothing — upstream's
    /// `setHint` updates the component that was copied, which is no longer shown.
    #[test]
    fn a_stale_copy_result_is_ignored() {
        let mut d = copy_dialog();
        d.show_auth("https://example.test/first", None);
        d.show_auth("https://example.test/second", None);
        d.set_auth_url_copy_result("https://example.test/first", Ok(()));
        assert!(
            d.body_text().contains("ctrl+x to copy"),
            "{}",
            d.body_text()
        );
        assert!(!d.body_text().contains("Copied"), "{}", d.body_text());
    }

    /// With the copy action unbound there is no `… to copy` promise to break.
    #[test]
    fn an_unbound_copy_key_draws_no_copy_hint() {
        let mut d = dialog();
        d.show_auth("https://example.test/auth", None);
        let body = d.body_text();
        assert!(body.contains("click to open"), "{body}");
        assert!(!body.contains("to copy"), "{body}");
        assert_eq!(
            d.handle(&ctrl('x'), &SelectKeymap::default()),
            SelectorOutcome::Ignored
        );
    }

    /// Paint the dialog into an 80-column buffer and return each row's raw cell symbols.
    fn painted_rows(d: &mut LoginDialog) -> Vec<String> {
        let width = 80;
        let height = d.desired_height(width);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        let theme = UiTheme::dark();
        terminal
            .draw(|frame| d.render(frame, frame.area(), &theme))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    /// pi `auth-url.ts:19` (`hyperlink(url, url)`) and `:27` (`hyperlink(clickHint, url)`): on a
    /// hyperlink-capable terminal every row the wrapped URL occupies is an OSC-8 link to the FULL
    /// URL, so a click on any row opens the whole thing — and wrapping inserts nothing into it:
    /// the rows' visible text, concatenated, is the URL exactly.
    ///
    /// **Red before the fix:** the dialog emitted no OSC-8 at all (`TUI-020` was cited as the
    /// reason), so on a wrapped URL only a copy-and-repair of each row could reach the browser.
    #[test]
    fn a_wrapped_auth_url_is_one_osc8_link_per_row_and_reassembles_exactly() {
        let url = long_url();
        let mut d = copy_dialog().with_hyperlinks(true);
        d.show_auth(&url, Some("Complete login in your browser."));
        let rows = painted_rows(&mut d);
        let open = crate::osc::open(&url);
        let url_rows: Vec<&String> = rows.iter().filter(|r| r.contains(&open)).collect();
        assert!(
            url_rows.len() >= 5,
            "a {}-char URL spans several 80-column rows, each linked: {rows:#?}",
            url.len()
        );
        // The URL rows plus the click-hint row are all linked to the same full URL.
        let visible: Vec<String> = url_rows
            .iter()
            .map(|r| crate::ansi::strip_ansi(r).trim().to_string())
            .collect();
        let hint_row = visible.last().unwrap().clone();
        assert!(
            hint_row.starts_with("Ctrl+click to open") || hint_row.starts_with("Cmd+click to open"),
            "{hint_row}"
        );
        assert!(hint_row.contains("ctrl+x to copy"), "{hint_row}");
        let reassembled: String = visible[..visible.len() - 1].concat();
        assert_eq!(
            reassembled, url,
            "wrapping must not insert or drop a character"
        );
    }

    /// `hyperlink()` is gated on `getCapabilities().hyperlinks`: an incapable terminal gets plain
    /// cells, never a literal escape.
    #[test]
    fn no_osc8_is_painted_when_the_terminal_lacks_hyperlinks() {
        let mut d = copy_dialog().with_hyperlinks(false);
        d.show_auth(&long_url(), None);
        let rows = painted_rows(&mut d);
        assert!(
            rows.iter().all(|r| !r.contains('\u{1b}')),
            "no escape on an incapable terminal: {rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("https://auth.example.invalid"))
        );
    }

    /// The copy-code (headless) login's paste prompt: Anthropic's `manual_code` prompt carries
    /// `placeholder: "code#state"` (`anthropic.ts:221` @v1.1.0). It is drawn muted inside the empty
    /// field (the module's `[CYRUP-DELTA]`) under the instructions and the URL.
    ///
    /// **Red before the fix:** `show_auth_prompt` routed `manual_code` to `show_manual_input`,
    /// which dropped the placeholder, so nothing on screen said what shape to paste.
    #[test]
    fn a_manual_code_prompt_shows_its_placeholder_in_the_empty_field() {
        let mut d = copy_dialog().with_hyperlinks(false);
        d.show_auth(
            "https://claude.ai/oauth/authorize?code=true",
            Some("Complete login in your browser, then copy the code Anthropic shows and paste it here."),
        );
        show_auth_prompt(
            &mut d,
            &AuthPrompt::manual_code("Paste the code Anthropic shows after you sign in:")
                .with_placeholder("code#state"),
        );
        assert_eq!(d.input_placeholder(), Some("code#state"));
        let rows = painted_rows(&mut d).join("\n");
        assert!(
            rows.contains("then copy the code Anthropic shows"),
            "{rows}"
        );
        assert!(
            rows.contains("Paste the code Anthropic shows after you sign in:"),
            "{rows}"
        );
        assert!(rows.contains("> code#state"), "{rows}");
        // The placeholder is not a value: typing replaces it.
        let km = SelectKeymap::default();
        d.handle(&key(KeyCode::Char('a')), &km);
        assert_eq!(d.input_text(), Some("a"));
        assert!(!painted_rows(&mut d).join("\n").contains("code#state"));
    }

    #[tokio::test]
    async fn interaction_prompt_round_trips_through_the_channel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let interaction = TuiAuthInteraction::new(tx, CancelToken::new());
        let task = tokio::spawn(async move { interaction.prompt(AuthPrompt::text("Key?")).await });
        let msg = rx.recv().await.expect("prompt reaches the loop");
        match msg {
            LoginUiMsg::Prompt { prompt, reply } => {
                assert_eq!(prompt.message, "Key?");
                reply.send(Ok("answer".to_string())).ok();
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
        assert_eq!(task.await.unwrap().unwrap(), "answer");
    }

    #[tokio::test]
    async fn prompt_level_cancel_wins_the_race() {
        // `openrouter.ts:274-283` cancels the `manual_code` prompt when the callback server wins.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let interaction = TuiAuthInteraction::new(tx, CancelToken::new());
        let prompt_cancel = CancelToken::new();
        let armed = prompt_cancel.clone();
        let task = tokio::spawn(async move {
            interaction
                .prompt(AuthPrompt::manual_code("Paste the URL").with_cancel(armed))
                .await
        });
        let msg = rx.recv().await.expect("prompt reaches the loop");
        assert!(matches!(msg, LoginUiMsg::Prompt { .. }));
        prompt_cancel.cancel();
        let err = task
            .await
            .unwrap()
            .expect_err("prompt-level cancel rejects");
        assert!(matches!(err, OAuthError::Cancelled), "{err}");
    }

    #[tokio::test]
    async fn an_already_aborted_prompt_never_reaches_the_dialog() {
        // `if (prompt.signal.aborted) throw new Error("Login cancelled")` (`:5334`).
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let interaction = TuiAuthInteraction::new(tx, CancelToken::new());
        let token = CancelToken::new();
        token.cancel();
        let err = interaction
            .prompt(AuthPrompt::text("Key?").with_cancel(token))
            .await
            .expect_err("aborted up front");
        assert!(matches!(err, OAuthError::Cancelled));
        assert!(rx.try_recv().is_err(), "no prompt should have been sent");
    }
}
