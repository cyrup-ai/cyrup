//! [`HandoverLoader`] — the "Generating handover..." modal `/handover` shows while the session's
//! model writes the summary: pi's `BorderedLoader` (`coding-agent/src/modes/interactive/components/
//! bordered-loader.ts`, cancellable) as `performHandover` uses it (`pi-intercom v0.16.1
//! index.ts:3112-3121`):
//!
//! ```text
//! const generated = await ctx.ui.custom<{ text } | { error } | null>((tui, theme, _kb, done) => {
//!   const loader = new BorderedLoader(tui, theme, "Generating handover...");
//!   loader.onAbort = () => done(null);
//!   buildHandoverText(handoverClient, ctx, options, loader.signal).then(
//!     (text) => done({ text }),
//!     (error) => done(loader.signal.aborted ? null : { error: getErrorMessage(error) }),
//!   );
//!   return loader;
//! });
//! ```
//!
//! The generation runs on a spawned task under the loader's [`CancelToken`] (pi's
//! `loader.signal`), so Escape aborts the model call itself, not just the wait. The answer is
//! collected on [`InteractiveOverlay::tick`]; the overlay closes itself through
//! [`InteractiveOverlay::should_close`] once it has one.

use std::sync::{Arc, Mutex};

use cyrup_core::CancelToken;
use cyrup_ext::{InteractiveOverlay, OverlayKey, OverlayLine, OverlayOutcome};

use crate::handover::HandoverError;
use crate::ui::overlay::{OverlayTheme, key_to_data, to_overlay_line};
use crate::ui::{DefaultKeybindings, Keybindings, Theme, truncate_to_width, visible_width};

/// The generation a loader drives: `buildHandoverText(…, loader.signal)`.
pub type HandoverTextFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, HandoverError>> + Send>>;

/// `new BorderedLoader(tui, theme, "Generating handover...")` (`v0.16.1 index.ts:3113`).
pub const HANDOVER_LOADER_MESSAGE: &str = "Generating handover...";

/// pi-tui `Loader`'s `DEFAULT_FRAMES` (`components/loader.ts:12`).
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// pi-tui `Loader`'s `DEFAULT_INTERVAL_MS` (`components/loader.ts:13`).
const FRAME_MS: u64 = 80;

/// What the loader closed with — pi's `{ text } | { error } | null`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoverGeneration {
    /// `{ text }` — the framed handover, ready for the editor.
    Text(String),
    /// `{ error: getErrorMessage(error) }`.
    Failed(String),
    /// `null` — Escape (`onAbort`), or a failure that arrived after the abort.
    Cancelled,
}

/// The live loader overlay.
pub struct HandoverLoader {
    frame: usize,
    cancel: CancelToken,
    answer: tokio::sync::oneshot::Receiver<Result<String, HandoverError>>,
    result: Arc<Mutex<Option<HandoverGeneration>>>,
    closed: bool,
}

impl HandoverLoader {
    /// Spawn `generation` on `runtime` and show the loader until it settles or Escape aborts it.
    /// `cancel` must be the token `generation` was built over.
    #[must_use]
    pub fn spawn(
        generation: HandoverTextFuture,
        cancel: CancelToken,
        runtime: &tokio::runtime::Handle,
        result: Arc<Mutex<Option<HandoverGeneration>>>,
    ) -> Self {
        let (tx, answer) = tokio::sync::oneshot::channel();
        runtime.spawn(async move {
            let _ = tx.send(generation.await);
        });
        Self {
            frame: 0,
            cancel,
            answer,
            result,
            closed: false,
        }
    }

    fn done(&mut self, outcome: HandoverGeneration) {
        let mut slot = self.result.lock().unwrap_or_else(|e| e.into_inner());
        // The first `done` wins, as pi's `ctx.ui.custom` resolves once.
        if slot.is_none() {
            *slot = Some(outcome);
        }
        self.closed = true;
    }

    /// The loader's rows (pi `BorderedLoader`'s children: border, `Loader` (a blank row, then the
    /// spinner and message), a spacer, the cancel hint, a spacer, border).
    fn lines(&self, theme: &dyn Theme, keybindings: &dyn Keybindings, width: usize) -> Vec<String> {
        let width = width.max(1);
        let border = theme.fg("border", &"─".repeat(width));
        // pi-tui `Text(…, paddingX = 1)`: one column of padding each side, padded to the width.
        let text_row = |text: String| {
            let inner = width.saturating_sub(2);
            let clipped = truncate_to_width(&text, inner);
            let pad = inner.saturating_sub(visible_width(&clipped));
            let row = format!(" {clipped}{}{}", theme.reset(), " ".repeat(pad));
            let row = if width >= 2 { format!("{row} ") } else { row };
            truncate_to_width(&row, width)
        };
        let spinner = FRAMES.get(self.frame % FRAMES.len()).copied().unwrap_or("");
        // `keyHint("tui.select.cancel", "cancel")` = dim keys + muted description.
        let hint = format!(
            "{}{}",
            theme.fg("dim", &keybindings.get_keys("tui.select.cancel").join("/")),
            theme.fg("muted", " cancel")
        );
        vec![
            border.clone(),
            " ".repeat(width),
            text_row(format!(
                "{} {}",
                theme.fg("accent", spinner),
                theme.fg("muted", HANDOVER_LOADER_MESSAGE)
            )),
            " ".repeat(width),
            text_row(hint),
            " ".repeat(width),
            border,
        ]
    }
}

impl Drop for HandoverLoader {
    /// A loader torn down without an answer (the host went away, the session was replaced) must not
    /// leave the model call running: abort it, as pi's abandoned `loader.signal` would be.
    fn drop(&mut self) {
        if !self.closed {
            self.cancel.cancel();
        }
    }
}

impl InteractiveOverlay for HandoverLoader {
    fn render(&mut self, width: usize, _height: usize) -> Vec<OverlayLine> {
        self.lines(&OverlayTheme, &DefaultKeybindings, width)
            .iter()
            .map(|line| to_overlay_line(line))
            .collect()
    }

    /// pi-tui `CancellableLoader.handleInput` (`cancellable-loader.ts:30-36`): only cancel does
    /// anything — it aborts the signal, then `onAbort` → `done(null)`.
    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        let Some(data) = key_to_data(key) else {
            return OverlayOutcome::Ignored;
        };
        if !DefaultKeybindings.matches(&data, "tui.select.cancel") {
            return OverlayOutcome::Ignored;
        }
        self.cancel.cancel();
        self.done(HandoverGeneration::Cancelled);
        OverlayOutcome::Close
    }

    fn refresh_ms(&self) -> u64 {
        FRAME_MS
    }

    fn tick(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.frame = self.frame.wrapping_add(1);
        let settled = match self.answer.try_recv() {
            Ok(settled) => settled,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return true,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => Err(HandoverError::Aborted),
        };
        let outcome = match settled {
            Ok(text) => HandoverGeneration::Text(text),
            // `done(loader.signal.aborted ? null : { error })`.
            Err(_) if self.cancel.is_cancelled() => HandoverGeneration::Cancelled,
            Err(error) => HandoverGeneration::Failed(error.to_string()),
        };
        self.done(outcome);
        true
    }

    fn should_close(&self) -> bool {
        self.closed
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;
    use crate::ui::PlainTheme;
    use cyrup_ext::OverlayKeyCode;

    fn loader(
        generation: HandoverTextFuture,
        cancel: CancelToken,
    ) -> (HandoverLoader, Arc<Mutex<Option<HandoverGeneration>>>) {
        let result = Arc::new(Mutex::new(None));
        let loader = HandoverLoader::spawn(
            generation,
            cancel,
            &tokio::runtime::Handle::current(),
            result.clone(),
        );
        (loader, result)
    }

    async fn settle(loader: &mut HandoverLoader) {
        for _ in 0..200 {
            loader.tick();
            if loader.should_close() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the loader never settled");
    }

    #[tokio::test]
    async fn the_generated_text_closes_the_loader() {
        let (mut loader, result) = loader(
            Box::pin(async { Ok("# Handover".to_string()) }),
            CancelToken::new(),
        );
        let text = loader
            .lines(&PlainTheme, &DefaultKeybindings, 40)
            .join("\n");
        assert!(text.contains("Generating handover..."), "{text}");
        assert!(text.contains("escape/ctrl+c cancel"), "{text}");
        for line in loader.lines(&PlainTheme, &DefaultKeybindings, 40) {
            assert_eq!(visible_width(&line), 40, "{line:?}");
        }
        settle(&mut loader).await;
        assert_eq!(
            result.lock().unwrap().clone(),
            Some(HandoverGeneration::Text("# Handover".to_string()))
        );
    }

    #[tokio::test]
    async fn a_failure_is_reported_with_its_sentence() {
        let (mut loader, result) = loader(
            Box::pin(async { Err(HandoverError::NoConversation) }),
            CancelToken::new(),
        );
        settle(&mut loader).await;
        assert_eq!(
            result.lock().unwrap().clone(),
            Some(HandoverGeneration::Failed(
                "No conversation to hand over.".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn escape_aborts_the_generation_itself() {
        let cancel = CancelToken::new();
        let observed = cancel.clone();
        let (mut loader, result) = loader(
            Box::pin(async move {
                observed.cancelled().await;
                Err(HandoverError::Aborted)
            }),
            cancel.clone(),
        );
        assert_eq!(
            loader.handle_key(OverlayKey::plain(OverlayKeyCode::Char('x'))),
            OverlayOutcome::Ignored
        );
        assert_eq!(
            loader.handle_key(OverlayKey::plain(OverlayKeyCode::Escape)),
            OverlayOutcome::Close
        );
        assert!(cancel.is_cancelled(), "Escape reaches the model call");
        assert_eq!(
            result.lock().unwrap().clone(),
            Some(HandoverGeneration::Cancelled)
        );
    }

    #[tokio::test]
    async fn a_dropped_loader_cancels_its_generation() {
        let cancel = CancelToken::new();
        let (loader, _result) = loader(Box::pin(std::future::pending()), cancel.clone());
        drop(loader);
        assert!(cancel.is_cancelled());
    }
}
