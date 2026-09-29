//! `ui_prompt_start` / `ui_prompt_end` — the host-wide window during which an extension is blocked
//! on its own UI prompt (EXT-075).
//!
//! pi: `ExtensionRunner.setUIContext` wraps the one `ExtensionUIContext` every extension shares
//! (`core/extensions/runner.ts:522-537` @v0.87.1) so `select`/`confirm`/`input`/`editor`/`custom`
//! run through `withUIPrompt` (`:539-566`): a RUNNER-wide depth counter, `ui_prompt_start` on the
//! outermost call only, `ui_prompt_end` once that outermost call settles or throws, with the OUTER
//! prompt's `kind`/`title`. Both are emitted with `queueMicrotask` (`:568-572`), so they reach every
//! extension's handlers while the prompt is still open. With no UI context the runner keeps
//! `noOpUIContext` UNWRAPPED (`:523`), so nothing is emitted.
//!
//! cyrup has two prompt surfaces rather than one wrapped object: a WASM guest's `ui.*` imports
//! (`host/live.rs`) and the [`crate::host::HostServices`] backend every extension ultimately calls
//! (the session's `LiveHostServices`, which natives reach directly). Both wrap through the ONE
//! [`UiPromptTracker`] the [`crate::ExtensionHost`] owns, so the depth is host-wide as pi's is and
//! a guest prompt — which passes through both — emits one pair, not two.
//!
//! [CYRUP-DELTA] The prompting WASM guest does not receive its own pair. It is suspended inside the
//! import that is showing the prompt, holding its single-instance store, so a delivery to it could
//! only run after the prompt closed — and would fail the dispatch budget for a prompt a human left
//! open. The bus drain skips a suspended guest for the same reason (`Dispatcher::drain_bus`). Every
//! other extension receives both events while the prompt is open.

use crate::dispatch::Dispatcher;
use crate::event::HostEvent;
use cyrup_core::{CancelToken, ExtensionId};
use std::sync::{Arc, Mutex, Weak};

/// pi `UIPromptKind` (`core/extensions/types.ts:827` @v0.87.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiPromptKind {
    Select,
    Confirm,
    Input,
    Editor,
    Custom,
}

impl UiPromptKind {
    /// The wire spelling pi's events carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Confirm => "confirm",
            Self::Input => "input",
            Self::Editor => "editor",
            Self::Custom => "custom",
        }
    }
}

#[derive(Default)]
struct PromptState {
    /// pi `uiPromptDepth`.
    depth: u32,
    /// pi `activeUIPrompt`: the outermost prompt, reported by `ui_prompt_end`.
    active: Option<(UiPromptKind, Option<String>, Option<ExtensionId>)>,
}

type Delivery = (HostEvent, Option<ExtensionId>);

/// The host-wide prompt window (see the module doc).
pub struct UiPromptTracker {
    /// pi `hasUI()`: without a UI the runner never wraps, so no event is emitted.
    has_ui: bool,
    dispatcher: Weak<Dispatcher>,
    state: Mutex<PromptState>,
    /// The ordered delivery queue, created with its drain task on the first emission (the tracker
    /// is built before any runtime is guaranteed). One consumer keeps `ui_prompt_end` behind its
    /// `ui_prompt_start`, which pi's microtask queue guarantees.
    queue: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Delivery>>>,
}

impl UiPromptTracker {
    pub(crate) fn new(has_ui: bool, dispatcher: Weak<Dispatcher>) -> Self {
        Self {
            has_ui,
            dispatcher,
            state: Mutex::new(PromptState::default()),
            queue: Mutex::new(None),
        }
    }

    /// Enter a prompt (pi `withUIPrompt`'s head). `prompter` is the WASM guest showing it, if any —
    /// it is excluded from delivery (module doc). The window closes when the returned guard drops,
    /// on every exit path, as pi's `finally(finish)` does.
    pub fn begin(
        self: &Arc<Self>,
        kind: UiPromptKind,
        title: Option<&str>,
        prompter: Option<ExtensionId>,
    ) -> UiPromptGuard {
        if self.has_ui {
            // pi `...(title ? { title } : {})`: an empty title is absent.
            let title = title.filter(|t| !t.is_empty()).map(str::to_string);
            let outer = match self.state.lock() {
                Ok(mut g) => {
                    g.depth += 1;
                    if g.depth == 1 {
                        g.active = Some((kind, title.clone(), prompter.clone()));
                        true
                    } else {
                        false
                    }
                }
                Err(_) => false,
            };
            if outer {
                self.emit(
                    HostEvent::UiPromptStart {
                        kind: kind.as_str().to_string(),
                        title,
                    },
                    prompter,
                );
            }
        }
        UiPromptGuard {
            tracker: self.clone(),
            active: self.has_ui,
        }
    }

    /// pi `finish`: the outermost settle emits `ui_prompt_end` with the OUTER prompt's fields.
    fn finish(&self) {
        let ended = match self.state.lock() {
            Ok(mut g) => {
                g.depth = g.depth.saturating_sub(1);
                if g.depth == 0 { g.active.take() } else { None }
            }
            Err(_) => None,
        };
        if let Some((kind, title, prompter)) = ended {
            self.emit(
                HostEvent::UiPromptEnd {
                    kind: kind.as_str().to_string(),
                    title,
                },
                prompter,
            );
        }
    }

    fn emit(&self, ev: HostEvent, exclude: Option<ExtensionId>) {
        let Ok(mut queue) = self.queue.lock() else {
            return;
        };
        if queue.is_none() {
            let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                tracing::debug!(
                    event = ev.kind().name(),
                    "no async runtime to deliver an extension ui-prompt event on"
                );
                return;
            };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Delivery>();
            let dispatcher = self.dispatcher.clone();
            runtime.spawn(async move {
                while let Some((ev, exclude)) = rx.recv().await {
                    let Some(dispatcher) = dispatcher.upgrade() else {
                        return;
                    };
                    dispatcher
                        .dispatch_notify_excluding(&ev, &CancelToken::new(), exclude.as_ref())
                        .await;
                }
            });
            *queue = Some(tx);
        }
        if let Some(tx) = queue.as_ref() {
            let _ = tx.send((ev, exclude));
        }
    }
}

/// Closes the prompt window on drop (pi `run().finally(finish)` plus the synchronous-throw
/// `catch`, `runner.ts:559-565`).
pub struct UiPromptGuard {
    tracker: Arc<UiPromptTracker>,
    active: bool,
}

impl Drop for UiPromptGuard {
    fn drop(&mut self) {
        if self.active {
            self.tracker.finish();
        }
    }
}
