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
//! * **No OSC-8 hyperlink wrapping and no browser launch.** pi wraps the auth URL in OSC-8 and calls
//!   `openBrowser(url)` (`login-dialog.ts:98-110`). cyrup renders the bare URL: the crate already
//!   drops OSC-8 wrapping elsewhere for the same reason (`image.rs:351`), and
//!   `utils/open-browser.ts` has no cyrup counterpart anywhere in the workspace. The URL is shown in
//!   full so the login is completable by copy/paste.
//! * **Secrets are not masked**, matching upstream — pi's dialog uses a plain `Input` for every
//!   prompt kind including `secret` (`login-dialog.ts:54`, `:154-172`).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use cyrup_core::CancelToken;
use cyrup_provider::auth::oauth::{
    AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, OAuthError,
};

use crate::keymap::{SelectAction, SelectKeymap};
use crate::selector::{
    search_input_spans, title_lines, title_wrapped_height, Selector, SelectorOutcome,
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
}

impl LoginLineKind {
    fn style(self, theme: &UiTheme) -> Style {
        match self {
            LoginLineKind::Text => theme.base_style(),
            LoginLineKind::Accent => theme.accent_style(),
            LoginLineKind::Dim => theme.dim_style(),
            LoginLineKind::Warning => theme.warning_style(),
            LoginLineKind::Spacer => theme.base_style(),
        }
    }
}

/// The active free-text prompt (pi's shared `Input`, `login-dialog.ts:54`).
struct LoginInput {
    buffer: String,
    /// Byte offset into `buffer` (always a char boundary).
    cursor: usize,
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
        }
    }

    /// The dialog's title (`` `Login to ${providerName}` ``, `login-dialog.ts:41`).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The accumulated body lines (test/inspection): `(role, text)` in render order.
    pub fn lines(&self) -> &[(LoginLineKind, String)] {
        &self.lines
    }

    /// Every body line's text joined by `\n` — the cheap assertion surface for tests.
    pub fn body_text(&self) -> String {
        self.lines
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The current free-text buffer, or `None` when no text prompt is armed.
    pub fn input_text(&self) -> Option<&str> {
        self.input.as_ref().map(|i| i.buffer.as_str())
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
        self.lines.push((kind, text.into()));
    }

    fn spacer(&mut self) {
        self.lines.push((LoginLineKind::Spacer, String::new()));
    }

    /// `showAuth(url, instructions)` (`login-dialog.ts:96-113`): clear, then the URL, the click
    /// hint, and any instructions. pi additionally calls `openBrowser(url)` — see the module note.
    pub fn show_auth(&mut self, url: &str, instructions: Option<&str>) {
        self.lines.clear();
        self.spacer();
        self.push(LoginLineKind::Accent, url);
        self.push(LoginLineKind::Dim, click_hint());
        if let Some(instructions) = instructions.filter(|s| !s.is_empty()) {
            self.spacer();
            self.push(LoginLineKind::Warning, instructions);
        }
    }

    /// `showDeviceCode(info)` (`login-dialog.ts:118-131`): clear, then the verification URI, the
    /// click hint, a spacer, and `` `Enter code: ${info.userCode}` ``.
    pub fn show_device_code(&mut self, user_code: &str, verification_uri: &str) {
        self.lines.clear();
        self.spacer();
        self.push(LoginLineKind::Accent, verification_uri);
        self.push(LoginLineKind::Dim, click_hint());
        self.spacer();
        self.push(LoginLineKind::Warning, format!("Enter code: {user_code}"));
    }

    /// `showManualInput(prompt)` (`login-dialog.ts:136-148`): reset the buffer, append the dim
    /// prompt + the input + the cancel hint. Does NOT clear — the auth URL stays visible above it,
    /// which is the whole point of the manual-code escape hatch.
    pub fn show_manual_input(&mut self, prompt: &str) {
        self.spacer();
        self.push(LoginLineKind::Dim, prompt);
        self.input = Some(LoginInput {
            buffer: String::new(),
            cursor: 0,
            placeholder: None,
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
            buffer: String::new(),
            cursor: 0,
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
        self.lines.clear();
        self.spacer();
        for line in lines {
            self.push(LoginLineKind::Text, line.clone());
        }
    }

    /// `showInfo(message, links, showCloseHint)` (`login-dialog.ts:185-201`).
    pub fn show_info(&mut self, message: &str, links: &[(String, Option<String>)], close_hint: bool) {
        self.spacer();
        self.push(LoginLineKind::Text, message);
        for (url, label) in links {
            let text = match label.as_deref().filter(|s| !s.is_empty()) {
                Some(label) => format!("{label}: {url}"),
                None => url.clone(),
            };
            self.push(LoginLineKind::Accent, text);
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
    fn body_lines(&self, theme: Option<&UiTheme>) -> Vec<Line<'static>> {
        let style = |pick: fn(&UiTheme) -> Style| theme.map(pick).unwrap_or_default();
        let mut out: Vec<Line<'static>> = self
            .lines
            .iter()
            .map(|(kind, text)| {
                let style = theme.map(|t| kind.style(t)).unwrap_or_default();
                Line::from(Span::styled(format!(" {text}"), style))
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
            let mut spans =
                vec![Span::styled(crate::selector::INPUT_PROMPT, style(UiTheme::base_style))];
            match input.placeholder.as_deref().filter(|s| !s.is_empty()) {
                Some(hint) if input.buffer.is_empty() => {
                    spans.push(Span::styled(hint.to_string(), style(UiTheme::muted_style)));
                }
                _ => match theme {
                    Some(theme) => {
                        spans.extend(search_input_spans(&input.buffer, input.cursor, theme));
                    }
                    // Measurement: `search_input_spans` always draws a caret cell, so the widest
                    // form is the buffer plus one column.
                    None => spans.push(Span::raw(format!("{} ", input.buffer))),
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

    fn insert_char(&mut self, c: char) {
        if let Some(input) = self.input.as_mut() {
            input.buffer.insert(input.cursor, c);
            input.cursor += c.len_utf8();
        }
    }

    fn backspace(&mut self) {
        let Some(input) = self.input.as_mut() else { return };
        let Some(ch) = input.buffer.get(..input.cursor).and_then(|s| s.chars().next_back()) else {
            return;
        };
        let start = input.cursor - ch.len_utf8();
        input.buffer.replace_range(start..input.cursor, "");
        input.cursor = start;
    }

    fn delete_forward(&mut self) {
        let Some(input) = self.input.as_mut() else { return };
        let Some(ch) = input.buffer.get(input.cursor..).and_then(|s| s.chars().next()) else {
            return;
        };
        let end = input.cursor + ch.len_utf8();
        input.buffer.replace_range(input.cursor..end, "");
    }

    fn cursor_left(&mut self) {
        let Some(input) = self.input.as_mut() else { return };
        if let Some(ch) = input.buffer.get(..input.cursor).and_then(|s| s.chars().next_back()) {
            input.cursor -= ch.len_utf8();
        }
    }

    fn cursor_right(&mut self) {
        let Some(input) = self.input.as_mut() else { return };
        if let Some(ch) = input.buffer.get(input.cursor..).and_then(|s| s.chars().next()) {
            input.cursor += ch.len_utf8();
        }
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
        Some((input.buffer.clone(), input.buffer.clone()))
    }
}

impl Selector for LoginDialog {
    fn desired_height(&self, width: u16) -> u16 {
        // Top rule + wrapped title + wrapped body + bottom rule.
        let body = self.body_lines(None);
        let body_h = crate::transcript::wrapped_height(&body, usize::from(width))
            .min(usize::from(u16::MAX)) as u16;
        title_wrapped_height(&self.title, width)
            .saturating_add(body_h)
            .saturating_add(2)
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let title_h = title_wrapped_height(&self.title, area.width);
        let body = self.body_lines(Some(theme));
        let body_h = crate::transcript::wrapped_height(&body, usize::from(area.width))
            .min(usize::from(u16::MAX)) as u16;
        let [top, title_area, body_area, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(title_h),
            Constraint::Length(body_h),
            Constraint::Length(1),
        ])
        .areas(area);
        let rule = |w: u16| "─".repeat(w.max(1) as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(rule(top.width), theme.border_style()))),
            top,
        );
        frame.render_widget(
            Paragraph::new(title_lines(&self.title))
                .style(theme.accent_style().add_modifier(Modifier::BOLD))
                .wrap(Wrap { trim: false }),
            title_area,
        );
        frame.render_widget(
            Paragraph::new(body).wrap(Wrap { trim: false }),
            body_area,
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                rule(bottom.width),
                theme.border_style(),
            ))),
            bottom,
        );
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        // `handleInput` (`login-dialog.ts:222-232`): the cancel binding aborts the WHOLE login
        // (`cancel()` → `abortController.abort()` + reject "Login cancelled"); everything else goes
        // to the input. The select prompt's own Esc rejects identically (`:5316-5319`).
        match keymap.action_for(key) {
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
        match key.code {
            KeyCode::Backspace => {
                self.backspace();
                SelectorOutcome::Redraw
            }
            KeyCode::Delete => {
                self.delete_forward();
                SelectorOutcome::Redraw
            }
            KeyCode::Left => {
                self.cursor_left();
                SelectorOutcome::Redraw
            }
            KeyCode::Right => {
                self.cursor_right();
                SelectorOutcome::Redraw
            }
            KeyCode::Home => {
                if let Some(input) = self.input.as_mut() {
                    input.cursor = 0;
                }
                SelectorOutcome::Redraw
            }
            KeyCode::End => {
                if let Some(input) = self.input.as_mut() {
                    input.cursor = input.buffer.len();
                }
                SelectorOutcome::Redraw
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_char(c);
                SelectorOutcome::Redraw
            }
            _ => SelectorOutcome::Ignored,
        }
    }

    fn set_title(&mut self, title: String) {
        self.title = title;
    }

    fn as_login_dialog(&mut self) -> Option<&mut LoginDialog> {
        Some(self)
    }
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
    pub fn new(
        tx: tokio::sync::mpsc::UnboundedSender<LoginUiMsg>,
        cancel: CancelToken,
    ) -> Self {
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
        if prompt_cancel.as_ref().is_some_and(CancelToken::is_cancelled) || self.cancel.is_cancelled()
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
/// list, `manual_code` opens the bare manual-entry field, everything else (`text`, `secret`, and an
/// absent `type`) opens the ordinary message+placeholder prompt.
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
        Some(AuthPromptKind::ManualCode) => dialog.show_manual_input(&prompt.message),
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
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn dialog() -> LoginDialog {
        LoginDialog::new("Login to Anthropic", &SelectKeymap::default())
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
            assert_eq!(d.handle(&key(KeyCode::Char(c)), &km), SelectorOutcome::Redraw);
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

    #[tokio::test]
    async fn interaction_prompt_round_trips_through_the_channel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let interaction = TuiAuthInteraction::new(tx, CancelToken::new());
        let task = tokio::spawn(async move {
            interaction.prompt(AuthPrompt::text("Key?")).await
        });
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
        let err = task.await.unwrap().expect_err("prompt-level cancel rejects");
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
