//! UW-7 — the raw-terminal-input seam's HOST side: cyrup's port of pi's `TUI.handleInput`
//! listener fold (`packages/tui/src/tui.ts:773-788` @v0.83.0).
//!
//! # What this module is for
//!
//! An extension may subscribe to raw terminal input ([`cyrup_ext::native::InitApi::subscribe_terminal_input`],
//! EXT-021) and then see every keystroke BEFORE the focused component does, with the right to
//! rewrite it or swallow it. The whole extension-host half of that has existed and been tested
//! since EXT-021 closed; what did not exist was a caller. This is the caller.
//!
//! Its first consumer is `cyrup-ext-subagents`' fleet-status widget: `↓`/`←` on an empty editor
//! while subagents are running expands the always-on status line into a selectable roster
//! (`pi-subagents/src/tui/fleet-status.ts:696-753` @v0.68.0).
//!
//! # The three things it has to get right
//!
//! 1. **Position.** The fold runs where upstream's does — ahead of the focused component, and
//!    therefore ahead of [`App::handle_input`]'s overlay / selector / loader guards
//!    (`app/input.rs`). Running it *inside* `handle_input`, below those guards, would be simpler
//!    and would need no focus readback; it would also mean the fleet roster never learns that
//!    focus moved to a selector, so it stays active behind the picker and eats the first key
//!    after it closes. `losing_focus_deactivates` in this module's tests is that difference.
//!
//! 2. **The async/sync boundary.** [`cyrup_ext::ExtensionHost::terminal_input`] is `async`;
//!    [`App::handle_input`] is `fn` and returns an [`crate::AppAction`]. The consume-or-deliver
//!    answer is needed BEFORE the editor sees the key, so the extension-shortcut trick — return an
//!    `AppAction` and let the run loop spawn it (`app/run_action.rs`) — cannot be reused: that
//!    works only because a shortcut's effect is fire-and-forget. The `.await` therefore lands one
//!    level up, at `handle_input`'s only caller, `App::on_input_event`, which is already `async`.
//!    See [`cyrup_ext::NativeExtension::on_terminal_input`] for the deadlock that puts on any
//!    handler reaching a blocking capability from there.
//!
//! 3. **The wire format.** The seam carries `data: string` because pi's does — pi hands its
//!    listeners the bytes it read off the tty. cyrup's reader has already parsed those bytes away
//!    (crossterm's `event::read` in `app/input_reader.rs`), so they are reconstructed here, from
//!    the ONE encode/decode table both ends of the seam share
//!    ([`cyrup_ext::encode_terminal_key`] / [`cyrup_ext::decode_terminal_key`]). One table rather
//!    than two hand-written halves, because the two halves live in different crates and must agree
//!    byte for byte or the roster silently never activates.

use cyrup_ext::{
    ExtensionHost, TerminalInputDecision, TerminalKey, TerminalKeyEvent, TerminalKeyModifiers,
    decode_terminal_key, encode_terminal_key,
};
use ratatui::backend::Backend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::App;
use crate::InputEvent;

/// What the fold tells the input loop to do with one event — the [`InputEvent`]-typed shape of
/// [`TerminalInputDecision`].
///
/// No `PartialEq`: [`InputEvent`] has none (it carries crossterm events, which cyrup never
/// compares), and deriving one here would mean adding it to a type this module has no business
/// widening. Tests match on the variant and assert on the event's fields.
#[derive(Clone, Debug)]
pub(crate) enum TerminalInputOutcome {
    /// Hand this event to [`App::handle_input`]. Equal to the original unless a handler rewrote
    /// the chunk (pi `result?.data !== undefined`, `tui.ts:780-782`).
    Deliver(InputEvent),
    /// Drop the keystroke entirely — a handler answered `consume: true` (`tui.ts:777-779`) or the
    /// fold ended with an empty string (`:784-786`).
    Consume,
}

impl<B: Backend> App<B> {
    /// Whether the EDITOR is the component the key path would route this keystroke to — cyrup's
    /// answer to pi's `editorHasFocus()`
    /// (`pi-subagents/src/tui/fleet-status.ts:965-976` @v0.68.0).
    ///
    /// Upstream duck-types `tui.focusedComponent` for five `EditorComponent` methods, excusing
    /// itself with *"pi-tui exposes focus mutation but no focus getter"* and *"instanceof is
    /// unreliable across jiti module boundaries"*. cyrup has a better source: the routing chain
    /// itself. [`App::handle_input`] offers the key to an overlay, then a selector, then a mounted
    /// loader, and only what survives all three reaches the editor (`app/input.rs`, the three
    /// guards `spec/tui/05 §2` calls routing step 2). So "the editor is focused" is exactly "none
    /// of those three is mounted", read off the same state the router reads.
    ///
    /// \[CYRUP-DELTA] Deliberately NOT `self.state.editor.is_focused()`. That flag is DEC `?1004`
    /// terminal-window focus, driven by [`InputEvent::FocusGained`]/[`InputEvent::FocusLost`],
    /// and upstream's `focusedComponent` is independent of it. Answering with window focus would
    /// leave the fleet roster active behind an open selector — the precise state pi's guard exists
    /// to end — while also deactivating it whenever the user alt-tabs away, which upstream never
    /// does.
    pub(crate) fn editor_has_keyboard_focus(&self) -> bool {
        self.state.overlays.is_empty()
            && self.state.selector.is_none()
            && self.state.loader.is_none()
    }

    /// Offer one input event to every terminal-input subscriber and say what to do with it — pi's
    /// `TUI.handleInput` listener fold (`packages/tui/src/tui.ts:773-788` @v0.83.0).
    ///
    /// Called from `App::on_input_event`, ahead of [`App::handle_input`]; see the module doc for
    /// why it lives there and not inside it.
    ///
    /// # The three early exits, each for its own reason
    ///
    /// * **No subscribers.** Gated on [`ExtensionHost::has_terminal_input_subscribers`], which is
    ///   upstream's own guard (`if (this.inputListeners.size > 0)`, `tui.ts:773`). Without it
    ///   every keystroke in an extension-less session would cost an `.await`, a cloned owner list
    ///   and a `String`; with it, one rwlock read.
    /// * **Not a key.** Only [`InputEvent::Key`] is offered. A resize, a focus change and a mouse
    ///   report are not keystrokes, and a [`InputEvent::Paste`] is content rather than a key —
    ///   pi's own key matching refuses bracketed-paste payloads for exactly that reason
    ///   (`packages/tui/src/keys.ts:531-535`).
    /// * **Not encodable.** A [`KeyEvent`] outside the shared table's closed vocabulary
    ///   ([`cyrup_ext::TerminalKey`]) is delivered to [`App::handle_input`] UNTOUCHED, without
    ///   consulting the seam. Not offering a key is safe; dropping one is not.
    ///
    /// # The readback republish
    ///
    /// [`App::publish_extension_readbacks`] normally runs once per frame, from `draw`. That is too
    /// coarse here: `on_input_event` DRAINS every queued key before drawing (TUI-092 F3), so in a
    /// burst of auto-repeat the second key's fold would read the editor buffer and the focus as
    /// they were a frame ago. pi has no such window — its getters read the live component
    /// synchronously inside `handleKey` — so the choke point is re-run here, per key. It is a
    /// `String` clone and an atomic store, and only on the path that already has a subscriber.
    pub(crate) async fn offer_input_to_extensions(
        &mut self,
        host: &ExtensionHost,
        ev: InputEvent,
    ) -> TerminalInputOutcome {
        if !host.has_terminal_input_subscribers() {
            return TerminalInputOutcome::Deliver(ev);
        }
        let InputEvent::Key(key) = ev else {
            return TerminalInputOutcome::Deliver(ev);
        };
        let Some(encoded) = encode_key_event(&key) else {
            return TerminalInputOutcome::Deliver(InputEvent::Key(key));
        };
        self.publish_extension_readbacks();
        match host.terminal_input(&encoded).await {
            TerminalInputDecision::Consume => TerminalInputOutcome::Consume,
            // Unchanged: hand back the ORIGINAL event, not a decode of the echo. The round trip is
            // the identity for everything the encoder emits, but the original also carries the
            // `KeyEventState` and `kind` fields the wire format has no room for.
            TerminalInputDecision::Deliver(data) if data == encoded => {
                TerminalInputOutcome::Deliver(InputEvent::Key(key))
            }
            // Rewritten (pi `result?.data`, `tui.ts:780-782`): the editor must receive what the
            // extension substituted, not what the user pressed. A rewrite this table cannot
            // express falls back to the original rather than dropping the keystroke — the same
            // "never drop a key" direction the encoder's `None` takes.
            TerminalInputDecision::Deliver(data) => TerminalInputOutcome::Deliver(InputEvent::Key(
                decode_key_event(&data).unwrap_or(key),
            )),
        }
    }
}

/// Map one crossterm [`KeyEvent`] onto the shared wire table's vocabulary, then encode it.
///
/// `None` for any key outside that vocabulary — see [`cyrup_ext::TerminalKey`] for what it holds
/// and why it is small, and [`App::offer_input_to_extensions`] for what the caller does with a
/// `None`.
///
/// `KeyEventKind::Release` maps to the table's release flag, which encodes as the kitty
/// event-type-3 form pi's `isKeyRelease` tests for (`packages/tui/src/keys.ts:527-549` @v0.83.0).
/// cyrup's reader drops releases before they reach the run loop (`app/input_reader.rs`, the
/// `!matches!(k.kind, KeyEventKind::Release)` filter), so this arm is unreachable from the TUI
/// today; it is here because the table is shared and the flag is part of it, not as dead weight.
#[must_use]
pub(crate) fn encode_key_event(ev: &KeyEvent) -> Option<String> {
    let key = match ev.code {
        KeyCode::Up => TerminalKey::Up,
        KeyCode::Down => TerminalKey::Down,
        KeyCode::Left => TerminalKey::Left,
        KeyCode::Right => TerminalKey::Right,
        KeyCode::Esc => TerminalKey::Escape,
        KeyCode::Enter => TerminalKey::Enter,
        KeyCode::Char(c) => TerminalKey::Char(c),
        _ => return None,
    };
    encode_terminal_key(TerminalKeyEvent {
        key,
        modifiers: wire_modifiers(ev.modifiers),
        release: matches!(ev.kind, KeyEventKind::Release),
    })
}

/// The inverse: one rewritten chunk back into a [`KeyEvent`] the editor can be handed.
///
/// `KeyEventState::NONE` because the wire format carries no state bits — a rewrite is a new key,
/// not the user's original one, so there is nothing to preserve.
#[must_use]
pub(crate) fn decode_key_event(data: &str) -> Option<KeyEvent> {
    let ev = decode_terminal_key(data)?;
    let code = match ev.key {
        TerminalKey::Up => KeyCode::Up,
        TerminalKey::Down => KeyCode::Down,
        TerminalKey::Left => KeyCode::Left,
        TerminalKey::Right => KeyCode::Right,
        TerminalKey::Escape => KeyCode::Esc,
        TerminalKey::Enter => KeyCode::Enter,
        TerminalKey::Char(c) => KeyCode::Char(c),
    };
    Some(KeyEvent::new_with_kind_and_state(
        code,
        crossterm_modifiers(ev.modifiers),
        if ev.release {
            KeyEventKind::Release
        } else {
            KeyEventKind::Press
        },
        KeyEventState::NONE,
    ))
}

/// crossterm's modifier flags → pi's wire bit layout (`packages/tui/src/keys.ts:292-297`).
/// `KeyModifiers::HYPER` and `::META` have no pi counterpart and are dropped, which makes such a
/// key read as "some other key" at the far end — the same answer pi gives them.
fn wire_modifiers(mods: KeyModifiers) -> TerminalKeyModifiers {
    let mut out = TerminalKeyModifiers::NONE;
    if mods.contains(KeyModifiers::SHIFT) {
        out = out.union(TerminalKeyModifiers::SHIFT);
    }
    if mods.contains(KeyModifiers::ALT) {
        out = out.union(TerminalKeyModifiers::ALT);
    }
    if mods.contains(KeyModifiers::CONTROL) {
        out = out.union(TerminalKeyModifiers::CTRL);
    }
    if mods.contains(KeyModifiers::SUPER) {
        out = out.union(TerminalKeyModifiers::SUPER);
    }
    out
}

/// The inverse of [`wire_modifiers`].
fn crossterm_modifiers(mods: TerminalKeyModifiers) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if mods.contains(TerminalKeyModifiers::SHIFT) {
        out |= KeyModifiers::SHIFT;
    }
    if mods.contains(TerminalKeyModifiers::ALT) {
        out |= KeyModifiers::ALT;
    }
    if mods.contains(TerminalKeyModifiers::CTRL) {
        out |= KeyModifiers::CONTROL;
    }
    if mods.contains(TerminalKeyModifiers::SUPER) {
        out |= KeyModifiers::SUPER;
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    //! UW-7's TUI half.
    //!
    //! These live INLINE rather than in `crate::tests` because they read `AppState`'s
    //! `pub(super)` readback mirrors — the very cells the fold publishes — and `crate::tests` is
    //! not a descendant of `crate::app`. That is the case `crate::tests`' own module doc names for
    //! an inline test module: "when the test needs private items".

    use std::sync::{Arc, Mutex};

    use cyrup_core::ExtensionId;
    use cyrup_ext::event::HostEvent;
    use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
    use cyrup_ext::{ExtError, ExtensionHost, HookOutcome, HostConfig, TerminalInputResult};
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::{App, UiTheme};

    /// What the stub subscriber answers for every chunk.
    #[derive(Clone, Copy)]
    enum Answer {
        /// pi `undefined` — "I looked at it and did nothing".
        Decline,
        /// pi `{ consume: true }`.
        Consume,
        /// pi `{ data: "x" }` — rewrite the buffer for whoever comes next.
        RewriteTo(&'static str),
    }

    /// A native extension that SUBSCRIBES to raw terminal input, records every chunk it is
    /// offered, and answers as configured. The minimum real subscriber the host will fold.
    struct KeyWatcher {
        answer: Answer,
        seen: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl NativeExtension for KeyWatcher {
        fn id(&self) -> ExtensionId {
            "key-watcher".into()
        }
        async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
            api.subscribe_terminal_input();
            Ok(())
        }
        async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
            HookOutcome::Noop
        }
        fn on_terminal_input(&self, data: &str) -> Option<TerminalInputResult> {
            self.seen.lock().unwrap().push(data.to_string());
            match self.answer {
                Answer::Decline => None,
                Answer::Consume => Some(TerminalInputResult {
                    consume: Some(true),
                    data: None,
                }),
                Answer::RewriteTo(next) => Some(TerminalInputResult {
                    consume: None,
                    data: Some(next.to_string()),
                }),
            }
        }
    }

    fn host() -> ExtensionHost {
        ExtensionHost::new(HostConfig {
            mode: cyrup_ext::ExtMode::Tui,
            has_ui: true,
            cwd: std::env::temp_dir(),
        })
    }

    /// A host with one subscriber attached, plus the record of what it was offered.
    async fn host_with(answer: Answer) -> (ExtensionHost, Arc<Mutex<Vec<String>>>) {
        let host = host();
        let seen = Arc::new(Mutex::new(Vec::new()));
        host.load_native(Arc::new(KeyWatcher {
            answer,
            seen: Arc::clone(&seen),
        }))
        .await
        .expect("load the stub subscriber");
        (host, seen)
    }

    fn app() -> App<TestBackend> {
        App::new(TestBackend::new(80, 24), UiTheme::dark()).expect("app")
    }

    fn key(code: KeyCode) -> InputEvent {
        InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// Exactly what `App::on_input_event` does with one event, so these tests exercise the
    /// composition the run loop performs rather than a paraphrase of it. The structural guard
    /// below is what ties this helper to the real arm.
    async fn service(app: &mut App<TestBackend>, host: &ExtensionHost, ev: InputEvent) {
        match app.offer_input_to_extensions(host, ev).await {
            TerminalInputOutcome::Consume => {}
            TerminalInputOutcome::Deliver(ev) => {
                app.handle_input(&ev);
            }
        }
    }

    /// **T1** — a key a subscriber CONSUMES never reaches the editor. This is the whole observable
    /// of UW-7: `↓` on an empty editor expands the fleet roster instead of moving the cursor.
    ///
    /// MUTATION: make the fold answer `Deliver` where the host said `Consume` (or delete the
    /// `Consume` arm from `on_input_event`'s `match`) — the `j` lands in the editor buffer and
    /// this fails. Observed RED.
    #[tokio::test]
    async fn a_consumed_key_never_reaches_the_editor() {
        let (host, seen) = host_with(Answer::Consume).await;
        let mut app = app();
        service(&mut app, &host, key(KeyCode::Char('j'))).await;
        assert_eq!(seen.lock().unwrap().as_slice(), ["j"]);
        assert!(
            app.state().editor.is_empty(),
            "a consumed keystroke must not reach the editor, got {:?}",
            app.state().editor.text()
        );
    }

    /// **T2** — a key the subscriber DECLINES (pi's `undefined`) reaches the editor unchanged.
    /// The complement of T1, and the reason a broken extension cannot swallow the keyboard.
    ///
    /// MUTATION: make the fold's unchanged-`Deliver` arm answer `Consume` — every keystroke in a
    /// session with any subscriber vanishes and this fails. Observed RED.
    #[tokio::test]
    async fn a_declined_key_reaches_the_editor() {
        let (host, seen) = host_with(Answer::Decline).await;
        let mut app = app();
        service(&mut app, &host, key(KeyCode::Char('a'))).await;
        assert_eq!(seen.lock().unwrap().as_slice(), ["a"]);
        assert_eq!(app.state().editor.text(), "a");
    }

    /// **T3** — a REWRITTEN chunk is what the editor receives, not what the user pressed (pi
    /// `result?.data !== undefined` replaces the buffer, `packages/tui/src/tui.ts:780-782`).
    ///
    /// MUTATION: drop the decode-back arm and always deliver the original `ev` — the editor gets
    /// `a` instead of `x` and this fails. Observed RED.
    #[tokio::test]
    async fn a_rewritten_chunk_is_what_the_editor_receives() {
        let (host, _) = host_with(Answer::RewriteTo("x")).await;
        let mut app = app();
        service(&mut app, &host, key(KeyCode::Char('a'))).await;
        assert_eq!(app.state().editor.text(), "x");
    }

    /// A rewrite the shared table cannot express falls back to the ORIGINAL key rather than
    /// dropping it. Same direction as the encoder's `None`: never offer, never drop.
    ///
    /// MUTATION: replace `unwrap_or(key)` with a `Consume` (or with a `return`) — the user's
    /// keystroke disappears because an extension answered with something unparsable, and this
    /// fails. Observed RED.
    #[tokio::test]
    async fn an_inexpressible_rewrite_delivers_the_original_key() {
        // `\x1b[5~` is PageUp: a real sequence, deliberately outside the shared table.
        let (host, _) = host_with(Answer::RewriteTo("\x1b[5~")).await;
        let mut app = app();
        service(&mut app, &host, key(KeyCode::Char('a'))).await;
        assert_eq!(app.state().editor.text(), "a");
    }

    /// **T4** — with NO subscriber the fold is skipped entirely, which is upstream's own guard
    /// (`if (this.inputListeners.size > 0)`, `tui.ts:773`).
    ///
    /// Observed through the readback republish: the fold's first act after the gate is to
    /// republish the editor mirror, so an unpublished mirror proves the gate short-circuited
    /// ahead of it. A key that reaches `handle_input` still lands in the editor either way, so
    /// the editor alone could not tell the two paths apart.
    ///
    /// MUTATION: delete the `has_terminal_input_subscribers()` gate — the mirror is published,
    /// the first assertion fails, and every keystroke in an extension-less session starts paying
    /// for an `.await` and a `String`. Observed RED.
    #[tokio::test]
    async fn with_no_subscriber_the_fold_is_skipped_entirely() {
        let bare = host();
        assert!(!bare.has_terminal_input_subscribers());
        let mut app = app();
        app.handle_input(&key(KeyCode::Char('a')));
        service(&mut app, &bare, key(KeyCode::Char('b'))).await;
        assert_eq!(
            app.state.editor_mirror.text(),
            "",
            "the gate must return before the readback republish"
        );

        // The contrast, on the same app: one subscriber and the very same key.
        let (host, _) = host_with(Answer::Decline).await;
        assert!(host.has_terminal_input_subscribers());
        service(&mut app, &host, key(KeyCode::Char('c'))).await;
        assert_eq!(app.state.editor_mirror.text(), "ab");
    }

    /// **T7** — `editor_has_focus` tracks the ROUTING CHAIN, which is what pi's
    /// `editorHasFocus()` means, and is published on the key path so the fold reads it live.
    ///
    /// MUTATION: make `editor_has_keyboard_focus` answer `self.state.editor.is_focused()` (the
    /// plan's own suggestion) — with a selector mounted and the window focused it reads `true`,
    /// the fleet roster stays active behind the picker, and the second assertion fails. Observed
    /// RED.
    ///
    /// Note the mutation is on the FUNCTION, not on its publish site. A first pass named the
    /// publish site — `publish_extension_readbacks`'s `let has_focus = …` line — and this test
    /// stayed GREEN under it, because the test reads the function directly and the mutated
    /// publish line only changed what reached the mirror. (It also made the function dead code,
    /// which is the tell.) The publish site is covered by
    /// [`the_fold_still_runs_and_reports_no_focus_behind_a_selector`], which asserts on the
    /// mirror; both are needed, and neither alone is enough.
    #[tokio::test]
    async fn editor_focus_follows_the_routing_chain_not_the_window() {
        let mut app = app();
        assert!(app.editor_has_keyboard_focus());

        app.open_selector(crate::SelectorKind::Theme);
        assert!(
            !app.editor_has_keyboard_focus(),
            "a mounted selector takes the key before the editor (app/input.rs)"
        );
        app.state_mut().selector = None;
        assert!(app.editor_has_keyboard_focus());

        // Window focus is a DIFFERENT question and must not move this one — upstream's
        // `focusedComponent` is unaffected by DEC `?1004`.
        app.handle_input(&InputEvent::FocusLost);
        assert!(!app.state().editor.is_focused());
        assert!(
            app.editor_has_keyboard_focus(),
            "losing the terminal window must not tell an extension the editor lost focus"
        );
    }

    /// **T10, the TUI half** — the fold runs AHEAD of `handle_input`'s guards, so a subscriber is
    /// still offered the key while a selector is mounted, and is told the editor does not hold
    /// focus. That pair is what lets the fleet roster deactivate itself; without it the roster
    /// stays expanded behind the picker and eats the first key after it closes.
    ///
    /// This is the test that REJECTS the simpler alternative of folding inside `handle_input` at
    /// the extension-shortcut tier, where the selector guard has already returned.
    ///
    /// MUTATION 1: give the fold `handle_input`'s own selector guard (`if
    /// self.state.selector.is_some() { return Deliver(ev); }`, which is what folding at the
    /// extension-shortcut tier amounts to) — `seen` is empty, the first assertion fails, and the
    /// widget can never learn that focus moved. Observed RED.
    ///
    /// MUTATION 2: make `publish_extension_readbacks` publish `self.state.editor.is_focused()`
    /// instead of [`App::editor_has_keyboard_focus`] — the mirror reads window focus, the second
    /// assertion fails, and an extension is told the editor holds focus while a selector has it.
    /// This is the test that covers the PUBLISH SITE; see
    /// [`editor_focus_follows_the_routing_chain_not_the_window`] for why the function's own test
    /// cannot. Observed RED.
    #[tokio::test]
    async fn the_fold_still_runs_and_reports_no_focus_behind_a_selector() {
        let (host, seen) = host_with(Answer::Decline).await;
        let mut app = app();
        app.open_selector(crate::SelectorKind::Theme);
        service(&mut app, &host, key(KeyCode::Down)).await;
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["\x1b[B"],
            "the subscriber must still see the key with a selector mounted"
        );
        assert!(
            !app.state.editor_focus_mirror.has_focus(),
            "and must be told the editor is not the focused component"
        );
    }

    /// The crossterm half of the shared table: every [`cyrup_ext::TerminalKey`] variant survives
    /// `KeyEvent` → bytes → `KeyEvent`. `cyrup-ext`'s own `terminal_keys.rs` pins the bytes; this
    /// pins the mapping onto crossterm's vocabulary, which is the part that can drift when
    /// crossterm adds a `KeyCode`.
    ///
    /// MUTATION: swap the `KeyCode::Left`/`KeyCode::Right` arms in `encode_key_event` — `←` and
    /// `→` trade places on the wire and this fails. Observed RED.
    #[test]
    fn crossterm_key_events_round_trip_through_the_wire() {
        let codes = [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char('/'),
        ];
        for code in codes {
            for mods in [
                KeyModifiers::NONE,
                KeyModifiers::CONTROL,
                KeyModifiers::ALT | KeyModifiers::SHIFT,
            ] {
                let ev = KeyEvent::new(code, mods);
                let encoded =
                    encode_key_event(&ev).unwrap_or_else(|| panic!("{code:?}+{mods:?} encodable"));
                let back = decode_key_event(&encoded)
                    .unwrap_or_else(|| panic!("{encoded:?} must decode back"));
                assert_eq!(back.code, code, "{encoded:?}");
                assert_eq!(back.modifiers, mods, "{encoded:?}");
            }
        }
    }

    /// A key outside the table is delivered WITHOUT consulting the seam — never dropped, never
    /// offered. The conservative direction of the encoder's `None`.
    ///
    /// MUTATION: make `offer_input_to_extensions` answer `Consume` when `encode_key_event`
    /// returns `None` — Tab, Backspace and every function key stop reaching the editor. Observed
    /// RED.
    #[tokio::test]
    async fn an_unencodable_key_bypasses_the_seam_and_still_reaches_the_editor() {
        let (host, seen) = host_with(Answer::Consume).await;
        let mut app = app();
        // Backspace on an empty buffer is a no-op, so type something first and delete it: the
        // edit is the observable that the key was delivered.
        service(&mut app, &host, key(KeyCode::Char('a'))).await;
        assert!(app.state().editor.is_empty(), "precondition: 'a' consumed");
        app.handle_input(&key(KeyCode::Char('a')));
        assert_eq!(app.state().editor.text(), "a");
        service(&mut app, &host, key(KeyCode::Backspace)).await;
        assert_eq!(
            app.state().editor.text(),
            "",
            "Backspace is outside the table, so the consuming subscriber never saw it"
        );
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            ["a"],
            "and it was never offered"
        );
    }

    /// The seam must be reached from the REAL input arm, not only from these tests' `service`
    /// helper. Nothing in this crate can construct a `RunCtx` (it owns a runtime, an event stream
    /// and thirteen channels), so the call site is read from the source — the same answer
    /// `extension_renderers.rs::the_action_arm_still_refreshes_extension_renders` and
    /// `run_loop_input_priority.rs` give for the same constraint.
    ///
    /// MUTATION: delete the `offer_input_to_extensions` call from `on_input_event` — every test
    /// above keeps passing (they call the fold directly) and this one fails, which is exactly why
    /// it exists. Observed RED.
    #[test]
    fn the_input_arm_folds_every_key_before_handle_input() {
        const SRC: &str = include_str!("run_action.rs");
        let offset = SRC
            .find("pub(crate) async fn on_input_event")
            .expect("run_action.rs must still define `on_input_event`");
        let arm = SRC.get(offset..).unwrap_or("");
        let end = arm
            .find("pub(crate) async fn on_session_event")
            .unwrap_or(arm.len());
        let arm = arm.get(..end).unwrap_or("");
        let fold = arm.find("self.offer_input_to_extensions(").expect(
            "the input arm no longer folds keys through the extension seam — a subscribed \
             extension would see no keystrokes at all (UW-7)",
        );
        let dispatch = arm
            .find("self.handle_input(&ev)")
            .expect("the input arm must still dispatch each drained event through `handle_input`");
        assert!(
            fold < dispatch,
            "the fold must run BEFORE `handle_input`: pi's listener loop completes before the \
             focused component is offered the key (`packages/tui/src/tui.ts:773-788`), and a \
             fold that ran after could not consume one"
        );
    }
}
