//! A bracketed paste while a floating overlay is topmost goes to the overlay, never to the editor
//! beneath it.
//!
//! pi hands the focused component the raw terminal data, paste markers included (`tui.ts:892-897`),
//! and an `Input` inserts the paste (`input.ts:62-87`). cyrup decodes the paste before it routes,
//! and the paste arm used to ignore the overlay stack its key arm honours: a paste into the
//! `/llama` Hugging Face search box landed in the chat editor underneath the modal, leaving the
//! search box empty.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use cyrup_ext::host::{
    InteractiveOverlay, OverlayKey, OverlayLine, OverlayOutcome as ExtOverlayOutcome,
};
use ratatui::backend::TestBackend;

use crate::overlay::ExtensionOverlay;
use crate::{App, AppAction, InputEvent, UiTheme};

/// An overlay with a text field: it records the pastes it receives and answers `outcome`.
struct Field {
    pasted: Arc<Mutex<Vec<String>>>,
    outcome: ExtOverlayOutcome,
}

impl InteractiveOverlay for Field {
    fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
        Vec::new()
    }
    fn handle_key(&mut self, _key: OverlayKey) -> ExtOverlayOutcome {
        ExtOverlayOutcome::Ignored
    }
    fn handle_paste(&mut self, text: &str) -> ExtOverlayOutcome {
        self.pasted.lock().unwrap().push(text.to_string());
        self.outcome
    }
}

/// An overlay with no text field: it keeps the trait's default `handle_paste`.
struct NoField;

impl InteractiveOverlay for NoField {
    fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
        Vec::new()
    }
    fn handle_key(&mut self, _key: OverlayKey) -> ExtOverlayOutcome {
        ExtOverlayOutcome::Ignored
    }
}

fn app_with(overlay: Box<dyn InteractiveOverlay>) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (done, _released) = tokio::sync::oneshot::channel();
    app.state_mut()
        .overlays
        .push(Box::new(ExtensionOverlay::new(overlay, done)));
    app
}

/// The paste reaches the overlay's field and the editor stays empty.
///
/// **Red** when the paste arm does not consult the overlay stack: the editor holds the text and
/// the overlay saw nothing.
#[test]
fn a_paste_goes_to_the_topmost_overlay_and_not_the_editor() {
    let pasted = Arc::new(Mutex::new(Vec::new()));
    let mut app = app_with(Box::new(Field {
        pasted: Arc::clone(&pasted),
        outcome: ExtOverlayOutcome::Redraw,
    }));

    let action = app.handle_input(&InputEvent::Paste("unsloth/Qwen3-8B-GGUF".to_string()));

    assert_eq!(action, AppAction::Redraw);
    assert_eq!(*pasted.lock().unwrap(), ["unsloth/Qwen3-8B-GGUF"]);
    assert!(
        app.state().editor.is_empty(),
        "the paste must not land in the editor under the modal"
    );
    assert!(app.overlay_open());
}

/// An overlay with no text field still swallows the paste: it must not leak to the editor either.
#[test]
fn an_overlay_without_a_text_field_still_keeps_the_paste_from_the_editor() {
    let mut app = app_with(Box::new(NoField));

    let action = app.handle_input(&InputEvent::Paste("stray text".to_string()));

    assert_eq!(action, AppAction::Redraw);
    assert!(app.state().editor.is_empty());
    assert!(app.overlay_open());
}

/// An overlay that closes itself on the paste is popped, as for a key.
#[test]
fn an_overlay_that_closes_on_the_paste_is_popped() {
    let mut app = app_with(Box::new(Field {
        pasted: Arc::default(),
        outcome: ExtOverlayOutcome::Close,
    }));

    assert_eq!(
        app.handle_input(&InputEvent::Paste("x".to_string())),
        AppAction::Redraw
    );
    assert!(!app.overlay_open());
}

/// With no overlay open a paste is the editor's, as before.
#[test]
fn without_an_overlay_the_paste_still_reaches_the_editor() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();

    app.handle_input(&InputEvent::Paste("hello".to_string()));

    assert!(!app.state().editor.is_empty());
}
