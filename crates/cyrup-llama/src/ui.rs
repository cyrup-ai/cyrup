//! the `/llama` manager overlay (`ui.ts`)
//!
//! Port of `packages/coding-agent/src/extensions/llama/ui.ts` @v0.99.2-17 (EXT-027): the
//! `LlamaUi` surface the `/llama` command flow drives (`ui.ts:77-88`), the model list, the generic
//! select/confirm/connection-error dialogs, the Hugging Face search box, the progress view and
//! `runWithProgress` (`ui.ts:494-542`).
//!
//! # Shape
//!
//! Upstream builds `pi-tui` components (`Container`, `SelectList`, `Input`, `Text`, `Spacer`,
//! `DynamicBorder`) and hands the view to `ctx.ui.custom(...)`. A native cyrup extension has an
//! [`InteractiveOverlay`] (`render(width, height) -> Vec<OverlayLine>`, `handle_key`, `tick`) and
//! `HostServices::open_overlay`, so this module is three layers:
//!
//! 1. **Components** — the small subset of `pi-tui` that `ui.ts` uses, as pure `state ->
//!    Vec<OverlayLine>` values: [`SelectList`] (`select-list.ts`), [`TextInput`] (`input.ts`), the
//!    `Text` word-wrapper (`text.ts` + `wrapTextWithAnsi`), `fuzzyFilter` (`fuzzy.ts`) and the
//!    `frame()` chrome (`ui.ts:64-75`).
//! 2. **[`LlamaView`]** — `LlamaView` (`ui.ts:276-478`) as a state machine with no terminal and no
//!    task of its own: `handle_key`, `tick(now)`, `render(width)`. Every `await`ed answer of
//!    [`LlamaUi`] is a `oneshot` the view resolves from `handle_key`, exactly as pi's
//!    `resolve(...)` closures are called from `SelectList.onSelect`.
//! 3. **The bridge** — [`LlamaOverlay`] (the [`InteractiveOverlay`] the host drives) and
//!    [`LlamaOverlayUi`] (the [`LlamaUi`] the command flow awaits) share one `Arc<Mutex<LlamaView>>`.
//!    [`show_llama_ui`] is `showLlamaUi` (`ui.ts:480-492`): it opens the overlay, runs the command
//!    flow against the handle, and closes the overlay when the flow ends.
//!
//! # Blocking
//!
//! `HostServices::open_overlay` blocks until the overlay closes, so [`show_llama_ui`] runs it on a
//! blocking thread and polls the command flow beside it. Call it from a command or shortcut task,
//! never from an `on_terminal_input` handler: that one runs on the task that services the keyboard
//! (`cyrup-ext/src/native.rs`, the "MUST NOT reach a blocking capability" note) and would wedge
//! the TUI. Build the [`LlamaKeys`] per open (`LlamaKeys::from_agent_dir`) so a `keybindings.json`
//! rebinding is picked up by the next `/llama`.
//!
//! # Mechanism differences
//!
//! * `[CYRUP-DELTA]` `ui.ts:481` calls `ctx.ui.custom(factory)` **without** `{ overlay: true }`, so
//!   upstream mounts the view in the editor slot. A native extension reaches the terminal only
//!   through `HostServices::open_overlay` (`crates/cyrup-ext/src/host/overlay.rs`), so the same
//!   view is painted as a centred overlay. Same component, same keys, different mount point.
//!   Ledger: EXT-027.
//! * `[CYRUP-DELTA]` pi calls `this.tui.requestRender()` from inside settled promises and timers
//!   (`ui.ts:179`, `:318`, `:454`, `:466`). There is no push channel from an extension to the
//!   host, so the same state change sets a dirty flag and the host's next
//!   [`InteractiveOverlay::tick`] (every [`REFRESH_MS`]) repaints. The 500 ms search debounce
//!   (`ui.ts:211`) is likewise a deadline compared in `tick`, against an injectable [`Clock`], so
//!   it can fire up to one tick late. Ledger: EXT-027 (same mechanism as MCP-350 in `cyrup-mcp`).
//! * `[CYRUP-DELTA]` `controller.abort(new Error("Cancelled"))` (`ui.ts:533`) carries a reason;
//!   `tokio_util`'s `CancellationToken` does not. The reason is unobservable upstream too: the
//!   outcome of a cancelled run is discarded (`ui.ts:535-536`). Ledger: EXT-027.
//!
//! Colours: pi resolves `theme.fg("accent" | "muted" | "dim" | "warning" | "text", ...)` against the
//! user's active theme. The theme lives in `cyrup-tui`, out of reach of an extension, so each run
//! names the ROLE ([`OverlayColor::Theme`]) and the host resolves it at paint time: a `--use-theme`
//! choice, or a theme switched while the view is open, recolours it.
//!
//! Pointer: the model list, the select dialogs and the Hugging Face results answer the wheel (one
//! row, clamped), a press (selects the row) and a click (activates it); the search box places its
//! caret where it is clicked. [`LlamaView`] records, while it renders, what each painted row is
//! ([`RowHit`]), so the pointer is resolved against the rows that were actually drawn.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use async_trait::async_trait;
use cyrup_ext::host::{
    HostServices, InteractiveOverlay, KeySpec, NotifyKind, OverlayColor, OverlayKey,
    OverlayKeyCode, OverlayLine, OverlayMouse, OverlayMouseOutcome, OverlayOutcome, OverlaySpan,
    ThemeRole, key_ids, read_user_bindings,
};
use futures::future::BoxFuture;
use icu_collator::options::CollatorOptions;
use icu_collator::{Collator, CollatorPreferences};
use icu_properties::props::Script;
use icu_properties::script::{ScriptWithExtensions, ScriptWithExtensionsBorrowed};
use icu_provider_blob::BlobDataProvider;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::client::{LlamaModelInfo, LlamaModelStatus, LlamaProgress};
use crate::error::LlamaError;
use crate::huggingface::HuggingFaceModel;

/// `DOWNLOAD_VALUE` (`ui.ts:23`): the sentinel value of the trailing "Download model…" row.
const DOWNLOAD_VALUE: &str = "\0download";

/// How often the host should call [`InteractiveOverlay::tick`]. Bounds how late the 500 ms search
/// debounce and an asynchronous progress update become visible (see the module doc).
pub const REFRESH_MS: u64 = 50;

/// `maxVisible = 10` of the search results (`ui.ts:148`).
const SEARCH_MAX_VISIBLE: i64 = 10;
/// [`SEARCH_MAX_VISIBLE`] as a row count.
const SEARCH_VISIBLE_ROWS: usize = 10;
/// The search debounce, `setTimeout(..., 500)` (`ui.ts:211`).
const SEARCH_DEBOUNCE_MS: u64 = 500;
/// `this.query.length < 2` (`ui.ts:197`).
const SEARCH_MIN_QUERY: usize = 2;
/// `const available = 40` (`ui.ts:438`): the progress bar's cell count.
const PROGRESS_BAR_CELLS: usize = 40;
/// `Math.min(items.length, 12)` (`ui.ts:336`, `:363`).
const LIST_MAX_VISIBLE: usize = 12;

/// `status = "Type at least 2 characters"` (`ui.ts:109`, `:198`).
const STATUS_TOO_SHORT: &str = "Type at least 2 characters";
/// `"Searching Hugging Face…"` (`ui.ts:176`, `:209`).
const STATUS_SEARCHING: &str = "Searching Hugging Face…";
/// `"No GGUF models found"` (`ui.ts:205`, `:223`).
const STATUS_NONE_FOUND: &str = "No GGUF models found";

// =================================================================================================
// Public surface for the command flow (`LlamaUi`, `ui.ts:25-88`)
// =================================================================================================

/// `LlamaManagerAction` (`ui.ts:25`): what the model list resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum LlamaManagerAction {
    /// `{ type: "model", model }`: the user picked a model row. Boxed: a catalog entry is large
    /// next to the other two (unit) variants.
    Model(Box<LlamaModelInfo>),
    /// `{ type: "download" }`: the user picked "Download model…".
    Download,
    /// `{ type: "close" }`: the user cancelled the list.
    Close,
}

/// What [`LlamaUi::connection_error`] resolves to (`"retry" | "close"`, `ui.ts:81`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionChoice {
    /// `"retry"`.
    Retry,
    /// `"close"`.
    Close,
}

/// `ProgressState` (`ui.ts:27-30`): a [`LlamaProgress`] plus the view's title and model line.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressState {
    /// The frame title, e.g. `Loading model`.
    pub title: String,
    /// The model line under the title.
    pub model: String,
    /// `LlamaProgress.message`.
    pub message: String,
    /// `LlamaProgress.ratio`.
    pub ratio: Option<f64>,
    /// `LlamaProgress.detail`.
    pub detail: Option<String>,
}

impl ProgressState {
    /// `Object.assign(state, progress)` (`ui.ts:510`): `Object.assign` copies only the keys the
    /// update *has*, so a message-only update (`onProgress({ message: "Loading model" })`,
    /// `client.ts:283`, `:327`) keeps the `ratio` and `detail` the SSE watcher already delivered,
    /// and a load update (`{ message, ratio }`, `client.ts:107-110`) keeps `detail`.
    /// [`LlamaProgress`]'s [`ProgressField`](crate::client::ProgressField) members say which keys are there.
    pub fn apply(&mut self, progress: LlamaProgress) {
        self.message = progress.message;
        progress.ratio.apply_to(&mut self.ratio);
        progress.detail.apply_to(&mut self.detail);
    }
}

/// The search callback of [`LlamaUi::search_models`] — `(query, signal) => Promise<HuggingFaceModel[]>`
/// (`ui.ts:83`). Build one from a closure with [`search_fn`].
pub type SearchFn = Arc<
    dyn Fn(
            String,
            CancellationToken,
        ) -> BoxFuture<'static, Result<Vec<HuggingFaceModel>, LlamaError>>
        + Send
        + Sync,
>;

/// Box a closure into a [`SearchFn`].
pub fn search_fn<F, Fut>(search: F) -> SearchFn
where
    F: Fn(String, CancellationToken) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Vec<HuggingFaceModel>, LlamaError>> + Send + 'static,
{
    Arc::new(move |query, signal| Box::pin(search(query, signal)))
}

/// `LlamaUi` (`ui.ts:77-88`): everything the `/llama` command flow asks of the screen.
///
/// Each method that returns a value resolves when the user answers. A method whose view is later
/// replaced by another (pi's `setContent`) never resolves, as upstream's promises never do — the
/// command flow is expected to race it (see [`run_with_progress`]) or to have moved on.
#[async_trait]
pub trait LlamaUi: Send + Sync {
    /// `showModels(serverUrl, models)` (`ui.ts:321-358`).
    async fn show_models(&self, server_url: &str, models: &[LlamaModelInfo]) -> LlamaManagerAction;

    /// `select(title, options)` (`ui.ts:360-379`): the chosen option, or `None` when cancelled.
    async fn select(&self, title: &str, options: &[String]) -> Option<String>;

    /// `confirm(title, message)` (`ui.ts:381-383`): a Yes/No select titled `"{title}\n{message}"`.
    async fn confirm(&self, title: &str, message: &str) -> bool {
        let options = ["Yes".to_string(), "No".to_string()];
        self.select(&format!("{title}\n{message}"), &options)
            .await
            .as_deref()
            == Some("Yes")
    }

    /// `connectionError(serverUrl, message)` (`ui.ts:385-388`).
    async fn connection_error(&self, server_url: &str, message: &str) -> ConnectionChoice {
        let options = ["Retry".to_string(), "Close".to_string()];
        let title = format!("llama.cpp unavailable\n{server_url}\n\n{message}");
        match self.select(&title, &options).await.as_deref() {
            Some("Retry") => ConnectionChoice::Retry,
            _ => ConnectionChoice::Close,
        }
    }

    /// `searchModels(search)` (`ui.ts:390-413`): the chosen `owner/repository[:quant]`, or `None`.
    async fn search_models(&self, search: SearchFn) -> Option<String>;

    /// `showStatus(title, message)` (`ui.ts:415-417`).
    fn show_status(&self, title: &str, message: &str);

    /// `progress(state)` (`ui.ts:419-428`): shows the progress view and resolves when the user asks
    /// to stop (`tui.select.cancel`).
    async fn progress(&self, state: &ProgressState);

    /// `updateProgress(state)` (`ui.ts:430-455`): repaints the progress view; a no-op unless it is
    /// the view on screen.
    fn update_progress(&self, state: &ProgressState);
}

/// The strings of one [`run_with_progress`] call (`ui.ts:496-502`, minus the two callbacks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressOptions {
    /// The progress frame's title.
    pub title: String,
    /// The model line.
    pub model: String,
    /// The message shown before the first update.
    pub initial_message: String,
    /// The confirm dialog's title (`Stop loading?`).
    pub cancel_title: String,
    /// The confirm dialog's message.
    pub cancel_message: String,
}

/// `{ cancelled: true } | { cancelled: false, value }` (`ui.ts:505`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProgressOutcome<T> {
    /// The user stopped the run and `cancel()` was called.
    Cancelled,
    /// The run settled with a value.
    Completed(T),
}

/// The `update` callback handed to a [`run_with_progress`] run (`ui.ts:502`).
pub type UpdateFn = Arc<dyn Fn(LlamaProgress) + Send + Sync>;

/// `runWithProgress` (`ui.ts:494-542`).
///
/// Starts `run` and shows the progress view; Escape (`tui.select.cancel`) opens a confirm. If the
/// user agrees (and the run has not finished in the meantime) `cancel()` is awaited, the token is
/// cancelled, the run is awaited to its end and its outcome discarded, and
/// [`ProgressOutcome::Cancelled`] is returned. Declining returns to the progress view. A run that
/// fails is returned as `Err`.
///
/// `run` is polled in place rather than spawned, so it may borrow; it keeps running while the
/// confirm dialog is open, as upstream's promise does.
///
/// # Errors
///
/// The run's error, or `cancel()`'s (the token is cancelled first either way, `ui.ts:530-534`).
pub async fn run_with_progress<T, E, R, RF, C, CF>(
    ui: Arc<dyn LlamaUi>,
    options: ProgressOptions,
    run: R,
    cancel: C,
) -> Result<ProgressOutcome<T>, E>
where
    R: FnOnce(CancellationToken, UpdateFn) -> RF,
    RF: Future<Output = Result<T, E>>,
    C: FnOnce() -> CF,
    CF: Future<Output = Result<(), E>>,
{
    let controller = CancellationToken::new();
    let state = Arc::new(Mutex::new(ProgressState {
        title: options.title.clone(),
        model: options.model.clone(),
        message: options.initial_message.clone(),
        ratio: None,
        detail: None,
    }));
    let update: UpdateFn = {
        let state = Arc::clone(&state);
        let ui = Arc::clone(&ui);
        Arc::new(move |progress| {
            let snapshot = {
                let mut guard = lock(&state);
                guard.apply(progress);
                guard.clone()
            };
            ui.update_progress(&snapshot);
        })
    };
    let mut settled = Box::pin(run(controller.clone(), update));
    let mut result: Option<Result<T, E>> = None;

    loop {
        // `while (!completed)`
        if let Some(done) = result.take() {
            return done.map(ProgressOutcome::Completed);
        }
        let snapshot = lock(&state).clone();
        let stop_requested = tokio::select! {
            biased;
            () = ui.progress(&snapshot) => true,
            settled_value = &mut settled => {
                result = Some(settled_value);
                false
            }
        };
        if !stop_requested {
            continue;
        }
        // `const stop = await ui.confirm(...)`: the run keeps going while the dialog is open, and
        // a run that finishes meanwhile is noticed after the answer (`ui.ts:529`).
        let confirm = ui.confirm(&options.cancel_title, &options.cancel_message);
        tokio::pin!(confirm);
        let stop = loop {
            tokio::select! {
                settled_value = &mut settled, if result.is_none() => {
                    result = Some(settled_value);
                }
                answer = &mut confirm => break answer,
            }
        };
        if !stop || result.is_some() {
            continue;
        }
        let cancelled = cancel().await;
        controller.cancel();
        cancelled?;
        // `await settled` — the outcome is discarded (`ui.ts:535-536`).
        let _discarded = settled.await;
        return Ok(ProgressOutcome::Cancelled);
    }
}

// =================================================================================================
// Clock
// =================================================================================================

/// A monotonic millisecond clock. The search debounce is a deadline compared against it, so a test
/// can drive time by hand.
pub trait Clock: Send + Sync {
    /// Milliseconds since an arbitrary fixed origin.
    fn now_ms(&self) -> u64;
}

/// The production [`Clock`]: elapsed wall time since construction.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// A clock whose zero is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

// =================================================================================================
// Locking helper
// =================================================================================================

/// Lock, ignoring poison: a panic in a render cannot be allowed to wedge the keyboard.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// =================================================================================================
// Key bindings (`KeybindingsManager.matches`, `keyHint`)
// =================================================================================================
//
// The `matchesKey` grammar ([`KeySpec`]) and the `keybindings.json` reader are
// `cyrup_ext::host`'s (EXT-103), shared with `cyrup-mcp`'s panel keys; this module owns only the
// ids `ui.ts` reads and their defaults.

/// The keybinding ids `ui.ts` and the components it builds read through
/// `KeybindingsManager.matches` / `keyHint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    /// `tui.select.up` (`ui.ts:244`, `select-list.ts:94`).
    SelectUp,
    /// `tui.select.down` (`ui.ts:251`).
    SelectDown,
    /// `tui.select.confirm` (`ui.ts:258`, hint `:353`).
    SelectConfirm,
    /// `tui.select.cancel` (`ui.ts:264`, `:458`, hint `:353`).
    SelectCancel,
    /// `tui.input.submit` (`input.ts:115`; `Input.onSubmit` is never set by `ui.ts`).
    InputSubmit,
    /// `tui.editor.undo` (`input.ts:109`).
    EditorUndo,
    /// `tui.editor.deleteCharBackward`.
    EditorDeleteCharBackward,
    /// `tui.editor.deleteCharForward`.
    EditorDeleteCharForward,
    /// `tui.editor.deleteWordBackward`.
    EditorDeleteWordBackward,
    /// `tui.editor.deleteWordForward`.
    EditorDeleteWordForward,
    /// `tui.editor.deleteToLineStart`.
    EditorDeleteToLineStart,
    /// `tui.editor.deleteToLineEnd`.
    EditorDeleteToLineEnd,
    /// `tui.editor.yank`.
    EditorYank,
    /// `tui.editor.yankPop`.
    EditorYankPop,
    /// `tui.editor.cursorLeft`.
    EditorCursorLeft,
    /// `tui.editor.cursorRight`.
    EditorCursorRight,
    /// `tui.editor.cursorLineStart`.
    EditorCursorLineStart,
    /// `tui.editor.cursorLineEnd`.
    EditorCursorLineEnd,
    /// `tui.editor.cursorWordLeft`.
    EditorCursorWordLeft,
    /// `tui.editor.cursorWordRight`.
    EditorCursorWordRight,
}

impl Binding {
    const ALL: [Self; 20] = [
        Self::SelectUp,
        Self::SelectDown,
        Self::SelectConfirm,
        Self::SelectCancel,
        Self::InputSubmit,
        Self::EditorUndo,
        Self::EditorDeleteCharBackward,
        Self::EditorDeleteCharForward,
        Self::EditorDeleteWordBackward,
        Self::EditorDeleteWordForward,
        Self::EditorDeleteToLineStart,
        Self::EditorDeleteToLineEnd,
        Self::EditorYank,
        Self::EditorYankPop,
        Self::EditorCursorLeft,
        Self::EditorCursorRight,
        Self::EditorCursorLineStart,
        Self::EditorCursorLineEnd,
        Self::EditorCursorWordLeft,
        Self::EditorCursorWordRight,
    ];

    /// The canonical id as it appears in `keybindings.json`.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::SelectUp => "tui.select.up",
            Self::SelectDown => "tui.select.down",
            Self::SelectConfirm => "tui.select.confirm",
            Self::SelectCancel => "tui.select.cancel",
            Self::InputSubmit => "tui.input.submit",
            Self::EditorUndo => "tui.editor.undo",
            Self::EditorDeleteCharBackward => "tui.editor.deleteCharBackward",
            Self::EditorDeleteCharForward => "tui.editor.deleteCharForward",
            Self::EditorDeleteWordBackward => "tui.editor.deleteWordBackward",
            Self::EditorDeleteWordForward => "tui.editor.deleteWordForward",
            Self::EditorDeleteToLineStart => "tui.editor.deleteToLineStart",
            Self::EditorDeleteToLineEnd => "tui.editor.deleteToLineEnd",
            Self::EditorYank => "tui.editor.yank",
            Self::EditorYankPop => "tui.editor.yankPop",
            Self::EditorCursorLeft => "tui.editor.cursorLeft",
            Self::EditorCursorRight => "tui.editor.cursorRight",
            Self::EditorCursorLineStart => "tui.editor.cursorLineStart",
            Self::EditorCursorLineEnd => "tui.editor.cursorLineEnd",
            Self::EditorCursorWordLeft => "tui.editor.cursorWordLeft",
            Self::EditorCursorWordRight => "tui.editor.cursorWordRight",
        }
    }

    /// `DEFAULT_EDITOR_KEYBINDINGS` (`pi/packages/tui/src/keybindings.ts:81-157`).
    fn defaults(self) -> &'static [&'static str] {
        match self {
            Self::SelectUp => &["up"],
            Self::SelectDown => &["down"],
            Self::SelectConfirm | Self::InputSubmit => &["enter"],
            Self::SelectCancel => &["escape", "ctrl+c"],
            Self::EditorUndo => &["ctrl+-"],
            Self::EditorDeleteCharBackward => &["backspace"],
            Self::EditorDeleteCharForward => &["delete", "ctrl+d"],
            Self::EditorDeleteWordBackward => &["ctrl+w", "alt+backspace"],
            Self::EditorDeleteWordForward => &["alt+d", "alt+delete"],
            Self::EditorDeleteToLineStart => &["ctrl+u"],
            Self::EditorDeleteToLineEnd => &["ctrl+k"],
            Self::EditorYank => &["ctrl+y"],
            Self::EditorYankPop => &["alt+y"],
            Self::EditorCursorLeft => &["left", "ctrl+b"],
            Self::EditorCursorRight => &["right", "ctrl+f"],
            Self::EditorCursorLineStart => &["home", "ctrl+home", "ctrl+a"],
            Self::EditorCursorLineEnd => &["end", "ctrl+end", "ctrl+e"],
            Self::EditorCursorWordLeft => &["alt+left", "ctrl+left", "alt+b"],
            Self::EditorCursorWordRight => &["alt+right", "ctrl+right", "alt+f"],
        }
    }
}

#[derive(Clone, Debug)]
struct KeyEntry {
    /// The spec as configured, for `keyHint` text.
    text: String,
    spec: Option<KeySpec>,
}

/// The resolved `tui.select.*` / `tui.editor.*` / `tui.input.submit` table: `getKeybindings()` as
/// `ui.ts` and the components it builds read it.
///
/// Built from the user's `keybindings.json` like the TUI's own table, so a rebinding moves the
/// overlay and its hints together. Keys are read when the overlay is built; a rebinding made while
/// it is open applies the next time it opens.
#[derive(Clone, Debug)]
pub struct LlamaKeys {
    table: HashMap<Binding, Vec<KeyEntry>>,
}

impl Default for LlamaKeys {
    fn default() -> Self {
        Self::from_user_bindings(&[])
    }
}

impl LlamaKeys {
    /// Resolve over the user's raw (already migrated) bindings: a present id, including an empty
    /// list, replaces the default outright; an absent one keeps it.
    #[must_use]
    pub fn from_user_bindings(bindings: &[(String, serde_json::Value)]) -> Self {
        let mut table = HashMap::new();
        for binding in Binding::ALL {
            let configured = bindings
                .iter()
                .find(|(id, _)| id == binding.id())
                .and_then(|(_, value)| key_ids(value));
            let texts: Vec<String> = configured.unwrap_or_else(|| {
                binding
                    .defaults()
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect()
            });
            let entries = texts
                .into_iter()
                .map(|text| KeyEntry {
                    spec: KeySpec::parse(&text),
                    text,
                })
                .collect();
            table.insert(binding, entries);
        }
        Self { table }
    }

    /// Resolve against the HOST's effective keybinding tables — TUI-126, and the form to prefer
    /// over [`Self::from_agent_dir`] wherever a `HostServices` is in hand.
    ///
    /// Reads `tui.editor` and `tui.select`, the two namespaces these bindings live in (the
    /// `tui.input.*` ids are carried by the editor's map, `keymap.rs`'s `EditorAction`). The host's
    /// table is the user's `keybindings.json` ALREADY merged over the platform-conditional defaults
    /// of `core/keybindings.ts` and already re-merged on a live rebind, so this is both the right
    /// precedence and the live value — where reading the file directly carried neither: it missed
    /// defaults this crate does not reimplement, and it froze whatever was on disk when the overlay
    /// opened.
    ///
    /// Falls back to [`Self::from_agent_dir`] when the host answers nothing, which is every
    /// non-interactive mode (RPC, print/json, a test): there is no live keymap there, so the file is
    /// still the best available answer and behaviour outside the TUI is unchanged.
    #[must_use]
    pub fn from_host(host: &dyn cyrup_ext::HostServices, agent_dir: &Path) -> Self {
        // ONE read of the `tui` prefix, not one per leaf namespace: this overlay's ids span three
        // of them — `tui.editor.*`, `tui.select.*` and `tui.input.*` (`Binding::InputSubmit` is
        // `tui.input.submit`) — and `tui.input.*` is bound in the host's EDITOR keymap, so naming
        // `tui.editor` and `tui.select` would silently miss a `tui.input` rebind. Ids this overlay
        // does not know (`tui.altScreen.*`) are dropped by `from_user_bindings`.
        let mut pairs: Vec<(String, serde_json::Value)> = Vec::new();
        if let serde_json::Value::Object(map) = host.effective_keybindings("tui") {
            pairs.extend(map);
        }
        if pairs.is_empty() {
            return Self::from_agent_dir(agent_dir);
        }
        Self::from_user_bindings(&pairs)
    }

    /// Read `<agent_dir>/keybindings.json`, migrating legacy ids exactly as the TUI does. Any
    /// failure (absent, unreadable, malformed, not an object) yields the defaults.
    #[must_use]
    pub fn from_agent_dir(agent_dir: &Path) -> Self {
        read_user_bindings(agent_dir).map_or_else(Self::default, |migrated| {
            Self::from_user_bindings(&migrated)
        })
    }

    /// `keybindings.matches(data, id)`.
    #[must_use]
    pub fn matches(&self, binding: Binding, key: &OverlayKey) -> bool {
        self.table.get(&binding).is_some_and(|entries| {
            entries
                .iter()
                .any(|e| e.spec.is_some_and(|s| s.matches(key)))
        })
    }

    /// `keyText(id)` (`keybinding-hints.ts:29-36`): every key bound to the id, joined with `/`.
    #[must_use]
    pub fn key_text(&self, binding: Binding) -> String {
        let texts: Vec<&str> = self
            .table
            .get(&binding)
            .map(|entries| entries.iter().map(|e| e.text.as_str()).collect())
            .unwrap_or_default();
        format_key_text(&texts.join("/"))
    }
}

/// `formatKeyText` (`keybinding-hints.ts:17-27`): split on `/` and `+`, `alt` reads `option` on macOS.
fn format_key_text(key: &str) -> String {
    key.split('/')
        .map(|chord| {
            chord
                .split('+')
                .map(|part| {
                    if cfg!(target_os = "macos") && part.eq_ignore_ascii_case("alt") {
                        "option"
                    } else {
                        part
                    }
                })
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join("/")
}

// =================================================================================================
// Styling
// =================================================================================================

/// The theme roles `ui.ts` passes to `theme.fg` / `theme.bold`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// No theme call at all.
    Plain,
    /// `theme.fg("text", ...)`: the default foreground, but a styled (never blank) string.
    Text,
    /// `theme.fg("accent", ...)`.
    Accent,
    /// `theme.fg("accent", theme.bold(...))` (`ui.ts:67`).
    AccentBold,
    /// `theme.fg("muted", ...)`.
    Muted,
    /// `theme.fg("dim", ...)`.
    Dim,
    /// `theme.fg("warning", ...)`.
    Warning,
}

/// `theme.fg(<role>, …)` as a span: the run names the theme role and the host resolves it against
/// the ACTIVE theme at paint time, so a user's theme (and a live theme switch) recolours the view.
fn span(role: Role, text: impl Into<String>) -> OverlaySpan {
    let mut span = OverlaySpan::raw(text);
    let themed = |role| Some(OverlayColor::Theme(role));
    match role {
        Role::Plain => {}
        Role::Text => span.fg = themed(ThemeRole::Text),
        Role::Accent => span.fg = themed(ThemeRole::Accent),
        Role::AccentBold => {
            span.fg = themed(ThemeRole::Accent);
            span.bold = true;
        }
        Role::Muted => span.fg = themed(ThemeRole::Muted),
        Role::Dim => span.fg = themed(ThemeRole::Dim),
        Role::Warning => span.fg = themed(ThemeRole::Warning),
    }
    span
}

/// A run of text in one role.
#[derive(Clone, Debug)]
struct Seg {
    text: String,
    role: Role,
}

fn seg(role: Role, text: impl Into<String>) -> Seg {
    Seg {
        text: text.into(),
        role,
    }
}

// =================================================================================================
// Width, truncation and wrapping (`utils.ts`)
// =================================================================================================

/// `graphemeWidth` (`utils.ts:182`): a tab costs 3 columns, everything else its display width.
fn grapheme_width(grapheme: &str) -> usize {
    if grapheme == "\t" {
        3
    } else {
        UnicodeWidthStr::width(grapheme)
    }
}

/// `visibleWidth(text)` (`utils.ts:240`) for escape-free text (styling crosses the overlay seam in
/// span fields, so there are no escapes to skip).
pub(crate) fn visible_width(text: &str) -> usize {
    text.graphemes(true).map(grapheme_width).sum()
}

/// `truncateToWidth(text, maxWidth, "")` (`utils.ts:1053`): whole grapheme clusters only.
pub(crate) fn truncate_to_width(text: &str, max: usize) -> String {
    if visible_width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for grapheme in text.graphemes(true) {
        let w = grapheme_width(grapheme);
        if width.saturating_add(w) > max {
            break;
        }
        out.push_str(grapheme);
        width = width.saturating_add(w);
    }
    out
}

/// `sliceByColumn(line, startCol, length, true)` (`utils.ts:1254`): the clusters that start inside
/// `[start, start + length)` and fit entirely.
fn slice_by_column(text: &str, start: usize, length: usize) -> String {
    if length == 0 {
        return String::new();
    }
    let end = start.saturating_add(length);
    let mut out = String::new();
    let mut column = 0usize;
    for grapheme in text.graphemes(true) {
        let w = grapheme_width(grapheme);
        if column >= start && column < end && column.saturating_add(w) <= end {
            out.push_str(grapheme);
        }
        column = column.saturating_add(w);
        if column >= end {
            break;
        }
    }
    out
}

/// Cut an [`OverlayLine`] to `width` columns (`ui.ts:469-473`: `truncateToWidth(line, width, "")`
/// for any line wider than the view).
fn clip_line(line: OverlayLine, width: usize) -> OverlayLine {
    let total: usize = line.spans.iter().map(|s| visible_width(&s.text)).sum();
    if total <= width {
        return line;
    }
    let mut remaining = width;
    let mut spans = Vec::new();
    for mut piece in line.spans {
        if remaining == 0 {
            break;
        }
        let w = visible_width(&piece.text);
        if w > remaining {
            piece.text = truncate_to_width(&piece.text, remaining);
            spans.push(piece);
            break;
        }
        remaining -= w;
        spans.push(piece);
    }
    OverlayLine::new(spans)
}

/// The scripts of `cjkBreakRegex` (`utils.ts:54-55`), each matched by `Script_Extensions`.
const CJK_BREAK_SCRIPTS: [Script; 5] = [
    Script::Han,
    Script::Hiragana,
    Script::Katakana,
    Script::Hangul,
    Script::Bopomofo,
];

/// The `Script_Extensions` property, looked up once rather than per grapheme.
static SCRIPT_EXTENSIONS: LazyLock<ScriptWithExtensionsBorrowed<'static>> =
    LazyLock::new(ScriptWithExtensions::new);

/// `cjkBreakRegex.test(segment)` (`utils.ts:54-55`, `:836`): a cluster with any code point whose
/// `Script_Extensions` include Han, Hiragana, Katakana, Hangul or Bopomofo is its own wrap token.
/// `test` searches the whole cluster, so a combining kana voicing mark after a Latin letter counts,
/// and `Script_Extensions` takes in the shared CJK punctuation (`、`, `。`) and the half-width
/// kana that no fixed block list does.
fn is_cjk_break(grapheme: &str) -> bool {
    let scripts = *SCRIPT_EXTENSIONS;
    grapheme.chars().any(|c| {
        CJK_BREAK_SCRIPTS
            .into_iter()
            .any(|script| scripts.has_script(c, script))
    })
}

struct Token {
    start: usize,
    end: usize,
    width: usize,
    whitespace: bool,
}

/// `splitIntoTokensWithAnsi` (`utils.ts:799`) without the ANSI half: runs of spaces, runs of
/// everything else, and each CJK cluster alone.
fn tokenize(line: &str) -> Vec<Token> {
    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        Space,
        Word,
    }
    let mut tokens: Vec<Token> = Vec::new();
    let mut current: Option<(usize, usize, Kind)> = None;
    let flush = |current: &mut Option<(usize, usize, Kind)>, tokens: &mut Vec<Token>| {
        if let Some((start, end, _)) = current.take() {
            let text = line.get(start..end).unwrap_or("");
            tokens.push(Token {
                start,
                end,
                width: visible_width(text),
                whitespace: text.trim().is_empty(),
            });
        }
    };
    for (offset, grapheme) in line.grapheme_indices(true) {
        let end = offset + grapheme.len();
        let is_space = grapheme == " ";
        if !is_space && is_cjk_break(grapheme) {
            flush(&mut current, &mut tokens);
            tokens.push(Token {
                start: offset,
                end,
                width: visible_width(grapheme),
                whitespace: false,
            });
            continue;
        }
        let kind = if is_space { Kind::Space } else { Kind::Word };
        match &mut current {
            Some((_, token_end, current_kind)) if *current_kind == kind => *token_end = end,
            _ => {
                flush(&mut current, &mut tokens);
                current = Some((offset, end, kind));
            }
        }
    }
    flush(&mut current, &mut tokens);
    tokens
}

/// `breakLongWord` (`utils.ts:1013`): split an over-wide word into grapheme chunks.
fn break_long_word(line: &str, start: usize, end: usize, width: usize) -> Vec<(usize, usize)> {
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut cur_start = start;
    let mut cur_end = start;
    let mut cur_width = 0usize;
    let word = line.get(start..end).unwrap_or("");
    for (offset, grapheme) in word.grapheme_indices(true) {
        let w = visible_width(grapheme);
        if cur_width + w > width {
            lines.push((cur_start, cur_end));
            cur_start = start + offset;
            cur_width = 0;
        }
        cur_end = start + offset + grapheme.len();
        cur_width += w;
    }
    if cur_end > cur_start {
        lines.push((cur_start, cur_end));
    }
    if lines.is_empty() {
        lines.push((start, start));
    }
    lines
}

/// `wrapSingleLine` (`utils.ts:916`), as byte ranges of `line`.
fn wrap_single_line(line: &str, width: usize) -> Vec<(usize, usize)> {
    if line.is_empty() {
        return vec![(0, 0)];
    }
    if visible_width(line) <= width {
        return vec![(0, line.len())];
    }
    let mut wrapped: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<(usize, usize)> = None;
    let mut current_width = 0usize;
    for token in tokenize(line) {
        if token.width > width && !token.whitespace {
            if let Some(done) = current.take() {
                wrapped.push(done);
            }
            let mut pieces = break_long_word(line, token.start, token.end, width);
            let last = pieces.pop();
            wrapped.extend(pieces);
            current = last.filter(|(s, e)| e > s);
            current_width = current
                .and_then(|(s, e)| line.get(s..e))
                .map_or(0, visible_width);
            continue;
        }
        if current_width + token.width > width && current_width > 0 {
            if let Some(done) = current.take() {
                wrapped.push(done);
            }
            if token.whitespace {
                current_width = 0;
            } else {
                current = Some((token.start, token.end));
                current_width = token.width;
            }
        } else {
            current = Some(match current {
                Some((s, _)) => (s, token.end),
                None => (token.start, token.end),
            });
            current_width += token.width;
        }
    }
    if let Some(done) = current {
        wrapped.push(done);
    }
    if wrapped.is_empty() {
        return vec![(0, 0)];
    }
    wrapped
        .into_iter()
        .map(|(s, e)| {
            let kept = line.get(s..e).map_or(0, |text| text.trim_end().len());
            (s, s + kept)
        })
        .collect()
}

/// `wrapTextWithAnsi` (`utils.ts:891`), as byte ranges of `text`: lines split on `\r\n|\r|\n`, each
/// wrapped greedily at word boundaries with over-wide words broken by grapheme.
pub(crate) fn wrap_ranges(text: &str, width: usize) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return vec![(0, 0)];
    }
    let bytes = text.as_bytes();
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'\n') => {
                lines.push((start, i));
                i += 1;
                start = i;
            }
            Some(b'\r') => {
                lines.push((start, i));
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    lines.push((start, bytes.len()));
    let mut out = Vec::new();
    for (line_start, line_end) in lines {
        let line = text.get(line_start..line_end).unwrap_or("");
        for (s, e) in wrap_single_line(line, width) {
            out.push((line_start + s, line_start + e));
        }
    }
    out
}

/// `Text.render(width)` (`text.ts:46-100`) with no background: padded, wrapped, each line padded
/// to `width`. An all-blank unstyled text renders nothing (`text.ts:56`).
fn text_lines(segs: &[Seg], padding_x: usize, padding_y: usize, width: usize) -> Vec<OverlayLine> {
    let mut full = String::new();
    let mut bounds: Vec<(usize, usize, Role)> = Vec::new();
    for seg in segs {
        let normalized = seg.text.replace('\t', "   ");
        let start = full.len();
        full.push_str(&normalized);
        bounds.push((start, full.len(), seg.role));
    }
    if full.trim().is_empty() && segs.iter().all(|s| s.role == Role::Plain) {
        return Vec::new();
    }
    let padding_x = padding_x.min(width.saturating_sub(1) / 2);
    let content_width = width.saturating_sub(padding_x * 2).max(1);
    let mut content: Vec<OverlayLine> = Vec::new();
    for (start, end) in wrap_ranges(&full, content_width) {
        let mut spans: Vec<OverlaySpan> = Vec::new();
        if padding_x > 0 {
            spans.push(OverlaySpan::raw(" ".repeat(padding_x)));
        }
        let mut line_width = padding_x;
        for (seg_start, seg_end, role) in &bounds {
            let from = start.max(*seg_start);
            let to = end.min(*seg_end);
            if from >= to {
                continue;
            }
            let piece = full.get(from..to).unwrap_or("");
            line_width += visible_width(piece);
            spans.push(span(*role, piece));
        }
        line_width += padding_x;
        let pad = width.saturating_sub(line_width) + padding_x;
        if pad > 0 {
            spans.push(OverlaySpan::raw(" ".repeat(pad)));
        }
        content.push(OverlayLine::new(spans));
    }
    let blank = || OverlayLine::new(vec![OverlaySpan::raw(" ".repeat(width))]);
    let mut out = Vec::new();
    out.extend((0..padding_y).map(|_| blank()));
    out.extend(content);
    out.extend((0..padding_y).map(|_| blank()));
    out
}

// =================================================================================================
// Fuzzy matching (`fuzzy.ts`)
// =================================================================================================

/// `fuzzyMatch(query, text)` (`fuzzy.ts:12-93`): the score (lower is better) or `None`.
pub(crate) fn fuzzy_match(query: &str, text: &str) -> Option<f64> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let text: Vec<char> = text.to_lowercase().chars().collect();

    let match_query = |wanted: &[char]| -> Option<f64> {
        if wanted.is_empty() {
            return Some(0.0);
        }
        if wanted.len() > text.len() {
            return None;
        }
        let mut query_index = 0usize;
        let mut score = 0.0f64;
        let mut last: Option<usize> = None;
        let mut consecutive = 0i64;
        while let Some(wanted_char) = wanted.get(query_index) {
            let from = last.map_or(0, |l| l + 1);
            let found = text
                .iter()
                .enumerate()
                .skip(from)
                .find(|(_, c)| *c == wanted_char)
                .map(|(i, _)| i)?;
            let boundary = found == 0
                || text
                    .get(found - 1)
                    .is_some_and(|c| c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | ':'));
            if last.is_some_and(|l| l + 1 == found) {
                consecutive += 1;
                score -= (consecutive * 5) as f64;
            } else {
                consecutive = 0;
                if let Some(l) = last {
                    score += (found - l - 1) as f64 * 2.0;
                }
            }
            if boundary {
                score -= 10.0;
            }
            score += found as f64 * 0.1;
            last = Some(found);
            query_index += 1;
        }
        if wanted == text.as_slice() {
            score -= 100.0;
        }
        Some(score)
    };

    if let Some(primary) = match_query(&query) {
        return Some(primary);
    }
    let split = query
        .iter()
        .position(|c| !c.is_ascii_lowercase())
        .filter(|split| *split > 0)
        .filter(|split| query.iter().skip(*split).all(char::is_ascii_digit))
        .or_else(|| {
            query
                .iter()
                .position(|c| !c.is_ascii_digit())
                .filter(|split| *split > 0)
                .filter(|split| query.iter().skip(*split).all(char::is_ascii_lowercase))
        })?;
    let (head, tail) = query.split_at(split);
    let swapped: Vec<char> = tail.iter().chain(head.iter()).copied().collect();
    match_query(&swapped).map(|score| score + 5.0)
}

/// `fuzzyFilter(items, query, getText)` (`fuzzy.ts:99-137`): indices of the matching items, best
/// first. Whitespace- and slash-separated tokens must all match; scores add; the sort is stable.
pub(crate) fn fuzzy_filter<T>(items: &[T], query: &str, text: impl Fn(&T) -> &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..items.len()).collect();
    }
    let tokens: Vec<&str> = query
        .trim()
        .split(|c: char| c.is_whitespace() || c == '/')
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(usize, f64)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let haystack = text(item);
        let mut total = 0.0;
        let mut all = true;
        for token in &tokens {
            match fuzzy_match(token, haystack) {
                Some(score) => total += score,
                None => {
                    all = false;
                    break;
                }
            }
        }
        if all {
            scored.push((index, total));
        }
    }
    scored.sort_by(|a, b| a.1.total_cmp(&b.1));
    scored.into_iter().map(|(index, _)| index).collect()
}

// =================================================================================================
// What each painted row is, for the pointer
// =================================================================================================

/// What a painted row is, as far as the pointer is concerned. Recorded while the view renders, so
/// a pointer event is resolved against the rows that were actually drawn (a wrapped row, a scroll
/// window and a title that wraps all shift them) rather than against a second computation of the
/// layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RowHit {
    /// Chrome: a border, the title, a hint, a spacer.
    #[default]
    None,
    /// A list row; the index is into the list of the current mode.
    Item(usize),
    /// A row that belongs to the list but is not one of its items (the `(i/N)` indicator): the
    /// wheel over it scrolls the list, a click selects nothing.
    ListChrome,
    /// The search box. `first_col` is the text column shown at its left edge once the text has
    /// scrolled horizontally.
    Input { first_col: usize },
}

/// Painted rows together with what each one is.
#[derive(Clone, Debug, Default)]
struct Body {
    lines: Vec<OverlayLine>,
    hits: Vec<RowHit>,
}

impl Body {
    fn push(&mut self, line: OverlayLine, hit: RowHit) {
        self.lines.push(line);
        self.hits.push(hit);
    }

    fn extend(&mut self, lines: Vec<OverlayLine>, hit: RowHit) {
        for line in lines {
            self.push(line, hit);
        }
    }

    fn append(&mut self, other: Body) {
        self.lines.extend(other.lines);
        self.hits.extend(other.hits);
    }
}

impl From<Vec<OverlayLine>> for Body {
    /// Rows that are all chrome.
    fn from(lines: Vec<OverlayLine>) -> Self {
        let mut body = Self::default();
        body.extend(lines, RowHit::None);
        body
    }
}

// =================================================================================================
// `SelectList` (`select-list.ts`)
// =================================================================================================

/// `SelectItem` (`select-list.ts:16`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct SelectItem {
    value: String,
    label: String,
    description: Option<String>,
}

impl SelectItem {
    /// `item.label || item.value`.
    fn display(&self) -> &str {
        if self.label.is_empty() {
            &self.value
        } else {
            &self.label
        }
    }
}

/// What a key did to a [`SelectList`].
enum SelectEvent {
    None,
    Select(SelectItem),
    Cancel,
}

/// What a pointer event did to a list.
enum ListPointer {
    /// Not the list's: the event is not on a row it acts on.
    Ignored,
    /// The list took it (the highlight may have moved) and the frame must be repainted.
    Handled,
    /// A click activated this row.
    Select(SelectItem),
}

/// `SelectList` (`select-list.ts:44-259`): the windowed two-column list with a `(i/N)` indicator.
#[derive(Clone, Debug)]
struct SelectList {
    items: Vec<SelectItem>,
    selected: usize,
    /// The row a press selected and whose release has not arrived: the click activates THAT row.
    pressed: Option<usize>,
    max_visible: usize,
    /// `layout.minPrimaryColumnWidth` / `maxPrimaryColumnWidth`.
    min_primary: Option<usize>,
    max_primary: Option<usize>,
}

impl SelectList {
    fn new(items: Vec<SelectItem>, max_visible: usize) -> Self {
        Self {
            items,
            selected: 0,
            pressed: None,
            max_visible,
            min_primary: None,
            max_primary: None,
        }
    }

    fn with_primary_bounds(mut self, min: usize, max: usize) -> Self {
        self.min_primary = Some(min);
        self.max_primary = Some(max);
        self
    }

    /// `handleInput` (`select-list.ts:151-181`): up and down wrap.
    fn handle_key(&mut self, keys: &LlamaKeys, key: &OverlayKey) -> SelectEvent {
        if keys.matches(Binding::SelectUp, key) {
            if !self.items.is_empty() {
                self.selected = if self.selected == 0 {
                    self.items.len() - 1
                } else {
                    self.selected - 1
                };
            }
        } else if keys.matches(Binding::SelectDown, key) {
            if !self.items.is_empty() {
                self.selected = if self.selected + 1 >= self.items.len() {
                    0
                } else {
                    self.selected + 1
                };
            }
        } else if keys.matches(Binding::SelectConfirm, key) {
            if let Some(item) = self.items.get(self.selected) {
                return SelectEvent::Select(item.clone());
            }
        } else if keys.matches(Binding::SelectCancel, key) {
            return SelectEvent::Cancel;
        }
        SelectEvent::None
    }

    /// The pointer over the list (`select-list.ts` `handleMouse`), `hit` being what the row under
    /// it was painted as.
    ///
    /// * **Wheel** over the list moves the highlight one row, clamped (the keys wrap, the wheel
    ///   does not).
    /// * **Press** on a row selects it and remembers it.
    /// * **Click** activates the row the press went down on, falling back to the one under the
    ///   pointer, and forgets the press.
    ///
    /// Hover never moves the highlight: the window is centred on it, so moving it under a resting
    /// pointer would slide the rows out from under the user.
    fn pointer(&mut self, hit: RowHit, event: OverlayMouse) -> ListPointer {
        if self.items.is_empty() {
            return ListPointer::Ignored;
        }
        let last = self.items.len() - 1;
        match (event, hit) {
            (OverlayMouse::Wheel { lines, .. }, RowHit::Item(_) | RowHit::ListChrome) => {
                let step = if lines < 0 { -1_isize } else { 1 };
                self.selected = self.selected.saturating_add_signed(step).min(last);
                ListPointer::Handled
            }
            (OverlayMouse::Press { .. }, RowHit::Item(index)) if index <= last => {
                self.selected = index;
                self.pressed = Some(index);
                ListPointer::Handled
            }
            (OverlayMouse::Click { .. }, RowHit::Item(index)) => {
                let index = self.pressed.take().unwrap_or(index).min(last);
                self.selected = index;
                self.items.get(index).map_or(ListPointer::Ignored, |item| {
                    ListPointer::Select(item.clone())
                })
            }
            _ => ListPointer::Ignored,
        }
    }

    /// `getVisibleRange` (`select-list.ts:183`): the window centred on the selection.
    fn visible_range(&self, max_visible: usize) -> (usize, usize) {
        let len = i64::try_from(self.items.len()).unwrap_or(i64::MAX);
        let max_visible = i64::try_from(max_visible).unwrap_or(i64::MAX);
        let selected = i64::try_from(self.selected).unwrap_or(i64::MAX);
        let start = 0.max((selected - max_visible / 2).min(len - max_visible));
        let end = (start + max_visible).min(len);
        (
            usize::try_from(start).unwrap_or(0),
            usize::try_from(end).unwrap_or(0),
        )
    }

    /// `getPrimaryColumnWidth` (`select-list.ts:215`).
    fn primary_column_width(&self) -> usize {
        let raw_min = self.min_primary.or(self.max_primary).unwrap_or(32);
        let raw_max = self.max_primary.or(self.min_primary).unwrap_or(32);
        let min = raw_min.min(raw_max).max(1);
        let max = raw_min.max(raw_max).max(1);
        let widest = self
            .items
            .iter()
            .map(|item| visible_width(item.display()) + 2)
            .max()
            .unwrap_or(0);
        min.max(widest.min(max))
    }

    /// `render` (`select-list.ts:119-149`) with the window narrowed to `max_visible` rows (never
    /// wider than the list's own), for a host frame too short for the natural one.
    fn render_with(&self, width: usize, max_visible: usize) -> Body {
        if self.items.is_empty() {
            return Body::from(vec![OverlayLine::new(vec![span(
                Role::Warning,
                "  No matching commands",
            )])]);
        }
        let primary = self.primary_column_width();
        let (start, end) = self.visible_range(max_visible.min(self.max_visible));
        let mut body = Body::default();
        for index in start..end {
            let Some(item) = self.items.get(index) else {
                continue;
            };
            let description = item
                .description
                .as_deref()
                .map(normalize_to_single_line)
                .filter(|d| !d.is_empty());
            body.push(
                Self::render_item(
                    item,
                    index == self.selected,
                    width,
                    description.as_deref(),
                    primary,
                ),
                RowHit::Item(index),
            );
        }
        if start > 0 || end < self.items.len() {
            let text = format!("  ({}/{})", self.selected + 1, self.items.len());
            body.push(
                OverlayLine::new(vec![span(
                    Role::Dim,
                    truncate_to_width(&text, width.saturating_sub(2)),
                )]),
                RowHit::ListChrome,
            );
        }
        body
    }

    /// `renderItem` (`select-list.ts:216-258`).
    fn render_item(
        item: &SelectItem,
        selected: bool,
        width: usize,
        description: Option<&str>,
        primary: usize,
    ) -> OverlayLine {
        let prefix = if selected { "→ " } else { "  " };
        let prefix_width = visible_width(prefix);
        if let Some(description) = description.filter(|_| width > 40) {
            let effective = primary.min(width.saturating_sub(prefix_width + 4)).max(1);
            let max_primary = effective.saturating_sub(2).max(1);
            let value = truncate_to_width(item.display(), max_primary);
            let value_width = visible_width(&value);
            let spacing = " ".repeat(effective.saturating_sub(value_width).max(1));
            let description_start = prefix_width + value_width + spacing.len();
            let remaining = width.saturating_sub(description_start + 2);
            if remaining > 10 {
                let description = truncate_to_width(description, remaining);
                return if selected {
                    OverlayLine::new(vec![span(
                        Role::Accent,
                        format!("{prefix}{value}{spacing}{description}"),
                    )])
                } else {
                    OverlayLine::new(vec![
                        OverlaySpan::raw(format!("{prefix}{value}")),
                        span(Role::Muted, format!("{spacing}{description}")),
                    ])
                };
            }
        }
        let max_width = width.saturating_sub(prefix_width + 2);
        let value = truncate_to_width(item.display(), max_width);
        if selected {
            OverlayLine::new(vec![span(Role::Accent, format!("{prefix}{value}"))])
        } else {
            OverlayLine::new(vec![OverlaySpan::raw(format!("{prefix}{value}"))])
        }
    }
}

/// `normalizeToSingleLine` (`select-list.ts:9`): `text.replace(/[\r\n]+/g, " ").trim()`.
fn normalize_to_single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_break = false;
    for c in text.chars() {
        if c == '\r' || c == '\n' {
            if !in_break {
                out.push(' ');
                in_break = true;
            }
        } else {
            in_break = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

// =================================================================================================
// `Input` (`input.ts`)
// =================================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LastAction {
    None,
    Kill,
    Yank,
    TypeWord,
}

/// `PUNCTUATION_REGEX` (`utils.ts:821`).
fn is_punctuation(c: char) -> bool {
    matches!(
        c,
        '(' | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | '<'
            | '>'
            | '.'
            | ','
            | ';'
            | ':'
            | '\''
            | '"'
            | '!'
            | '?'
            | '+'
            | '-'
            | '='
            | '*'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '%'
            | '^'
            | '$'
            | '#'
            | '@'
            | '~'
            | '`'
    )
}

/// `Intl.SegmentData.isWordLike`, recomputed as "contains an alphanumeric" (UAX#29 exposes no such
/// bit; the two agree on every segment the algorithm can produce).
fn is_word_like(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}

fn is_whitespace_segment(segment: &str) -> bool {
    segment.chars().any(char::is_whitespace)
}

/// `findWordBackward` (`word-navigation.ts:22`).
fn find_word_backward(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let Some(before) = text.get(..cursor) else {
        return cursor;
    };
    let mut segments: Vec<&str> = before.split_word_bounds().collect();
    let mut new_cursor = cursor;
    while let Some(last) = segments.last() {
        if !is_whitespace_segment(last) {
            break;
        }
        new_cursor = new_cursor.saturating_sub(last.len());
        segments.pop();
    }
    let Some(last) = segments.last() else {
        return new_cursor;
    };
    if is_word_like(last) {
        let keep = last
            .char_indices()
            .rev()
            .find(|(_, c)| is_punctuation(*c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        new_cursor = new_cursor.saturating_sub(last.len().saturating_sub(keep));
    } else {
        while let Some(last) = segments.last() {
            if is_word_like(last) || is_whitespace_segment(last) {
                break;
            }
            new_cursor = new_cursor.saturating_sub(last.len());
            segments.pop();
        }
    }
    new_cursor
}

/// `findWordForward` (`word-navigation.ts:76`).
fn find_word_forward(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let Some(after) = text.get(cursor..) else {
        return cursor;
    };
    let mut segments = after.split_word_bounds().peekable();
    let mut new_cursor = cursor;
    while let Some(next) = segments.peek() {
        if !is_whitespace_segment(next) {
            break;
        }
        new_cursor += next.len();
        segments.next();
    }
    let Some(next) = segments.peek() else {
        return new_cursor;
    };
    if is_word_like(next) {
        new_cursor += next
            .char_indices()
            .find(|(_, c)| is_punctuation(*c))
            .map_or(next.len(), |(i, _)| i);
    } else {
        while let Some(next) = segments.peek() {
            if is_word_like(next) || is_whitespace_segment(next) {
                break;
            }
            new_cursor += next.len();
            segments.next();
        }
    }
    new_cursor
}

/// `Input` (`input.ts:30-376`): the single-line editor of the search box, with horizontal
/// scrolling, the Emacs kill ring, yank-pop and undo with word coalescing.
///
/// The cursor is a byte offset kept on a char boundary; every edit goes through
/// [`Self::replace_range`], which no-ops on a bad index instead of panicking.
#[derive(Clone, Debug)]
pub(crate) struct TextInput {
    value: String,
    cursor: usize,
    kill_ring: Vec<String>,
    last_action: LastAction,
    undo: Vec<(String, usize)>,
}

impl TextInput {
    const PROMPT: &'static str = "> ";

    pub(crate) fn new() -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            kill_ring: Vec::new(),
            last_action: LastAction::None,
            undo: Vec::new(),
        }
    }

    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    fn replace_range(&mut self, from: usize, to: usize, with: &str) {
        let (Some(head), Some(tail)) = (self.value.get(..from), self.value.get(to..)) else {
            return;
        };
        let mut next = String::with_capacity(head.len() + with.len() + tail.len());
        next.push_str(head);
        next.push_str(with);
        next.push_str(tail);
        self.value = next;
    }

    fn push_undo(&mut self) {
        self.undo.push((self.value.clone(), self.cursor));
    }

    fn undo(&mut self) {
        if let Some((value, cursor)) = self.undo.pop() {
            self.value = value;
            self.cursor = cursor;
            self.last_action = LastAction::None;
        }
    }

    fn previous_grapheme_len(&self) -> usize {
        self.value
            .get(..self.cursor)
            .and_then(|before| before.graphemes(true).next_back())
            .map_or(1, str::len)
    }

    fn next_grapheme_len(&self) -> usize {
        self.value
            .get(self.cursor..)
            .and_then(|after| after.graphemes(true).next())
            .map_or(1, str::len)
    }

    fn insert_character(&mut self, c: char) {
        if c.is_whitespace() || self.last_action != LastAction::TypeWord {
            self.push_undo();
        }
        self.last_action = LastAction::TypeWord;
        let mut buffer = [0u8; 4];
        let text: &str = c.encode_utf8(&mut buffer);
        self.replace_range(self.cursor, self.cursor, text);
        self.cursor += c.len_utf8();
    }

    fn handle_backspace(&mut self) {
        self.last_action = LastAction::None;
        if self.cursor > 0 {
            self.push_undo();
            let len = self.previous_grapheme_len();
            let from = self.cursor.saturating_sub(len);
            self.replace_range(from, self.cursor, "");
            self.cursor = from;
        }
    }

    fn handle_forward_delete(&mut self) {
        self.last_action = LastAction::None;
        if self.cursor < self.value.len() {
            self.push_undo();
            let len = self.next_grapheme_len();
            self.replace_range(self.cursor, self.cursor + len, "");
        }
    }

    /// `killRing.push` (`kill-ring.ts:19`).
    fn kill_push(&mut self, text: &str, prepend: bool, accumulate: bool) {
        if text.is_empty() {
            return;
        }
        match self.kill_ring.pop() {
            Some(last) if accumulate => {
                self.kill_ring.push(if prepend {
                    format!("{text}{last}")
                } else {
                    format!("{last}{text}")
                });
            }
            Some(last) => {
                self.kill_ring.push(last);
                self.kill_ring.push(text.to_string());
            }
            None => self.kill_ring.push(text.to_string()),
        }
    }

    fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let deleted = self.value.get(..self.cursor).unwrap_or("").to_string();
        self.kill_push(&deleted, true, self.last_action == LastAction::Kill);
        self.last_action = LastAction::Kill;
        self.replace_range(0, self.cursor, "");
        self.cursor = 0;
    }

    fn delete_to_line_end(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let deleted = self.value.get(self.cursor..).unwrap_or("").to_string();
        self.kill_push(&deleted, false, self.last_action == LastAction::Kill);
        self.last_action = LastAction::Kill;
        self.replace_range(self.cursor, self.value.len(), "");
    }

    fn delete_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let was_kill = self.last_action == LastAction::Kill;
        self.push_undo();
        let old = self.cursor;
        self.move_word_backwards();
        let from = self.cursor;
        self.cursor = old;
        let deleted = self.value.get(from..old).unwrap_or("").to_string();
        self.kill_push(&deleted, true, was_kill);
        self.last_action = LastAction::Kill;
        self.replace_range(from, old, "");
        self.cursor = from;
    }

    fn delete_word_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let was_kill = self.last_action == LastAction::Kill;
        self.push_undo();
        let old = self.cursor;
        self.move_word_forwards();
        let to = self.cursor;
        self.cursor = old;
        let deleted = self.value.get(old..to).unwrap_or("").to_string();
        self.kill_push(&deleted, false, was_kill);
        self.last_action = LastAction::Kill;
        self.replace_range(old, to, "");
    }

    fn yank(&mut self) {
        let Some(text) = self.kill_ring.last().cloned() else {
            return;
        };
        self.push_undo();
        self.replace_range(self.cursor, self.cursor, &text);
        self.cursor += text.len();
        self.last_action = LastAction::Yank;
    }

    fn yank_pop(&mut self) {
        if self.last_action != LastAction::Yank || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo();
        let previous = self.kill_ring.last().cloned().unwrap_or_default();
        let from = self.cursor.saturating_sub(previous.len());
        self.replace_range(from, self.cursor, "");
        self.cursor = from;
        if let Some(last) = self.kill_ring.pop() {
            self.kill_ring.insert(0, last);
        }
        let text = self.kill_ring.last().cloned().unwrap_or_default();
        self.replace_range(self.cursor, self.cursor, &text);
        self.cursor += text.len();
        self.last_action = LastAction::Yank;
    }

    fn move_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.last_action = LastAction::None;
        self.cursor = find_word_backward(&self.value, self.cursor);
    }

    fn move_word_forwards(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.last_action = LastAction::None;
        self.cursor = find_word_forward(&self.value, self.cursor);
    }

    /// `handlePaste` (`input.ts:396-406`): the pasted text with every line break dropped and every
    /// tab turned into four spaces, inserted at the cursor as one undoable step. The host decodes
    /// the bracketed-paste markers (`input.ts:62-98`), so this takes the payload.
    pub(crate) fn handle_paste(&mut self, pasted: &str) {
        self.last_action = LastAction::None;
        self.push_undo();
        let clean = pasted
            .replace("\r\n", "")
            .replace(['\r', '\n'], "")
            .replace('\t', "    ");
        self.replace_range(self.cursor, self.cursor, &clean);
        self.cursor += clean.len();
    }

    /// `handleInput` (`input.ts:60-212`); the bracketed-paste branch is [`Self::handle_paste`].
    pub(crate) fn handle_key(&mut self, keys: &LlamaKeys, key: &OverlayKey) {
        if keys.matches(Binding::EditorUndo, key) {
            self.undo();
        } else if keys.matches(Binding::InputSubmit, key) {
            // `onSubmit` is never assigned by `ui.ts`.
        } else if keys.matches(Binding::EditorDeleteCharBackward, key) {
            self.handle_backspace();
        } else if keys.matches(Binding::EditorDeleteCharForward, key) {
            self.handle_forward_delete();
        } else if keys.matches(Binding::EditorDeleteWordBackward, key) {
            self.delete_word_backwards();
        } else if keys.matches(Binding::EditorDeleteWordForward, key) {
            self.delete_word_forward();
        } else if keys.matches(Binding::EditorDeleteToLineStart, key) {
            self.delete_to_line_start();
        } else if keys.matches(Binding::EditorDeleteToLineEnd, key) {
            self.delete_to_line_end();
        } else if keys.matches(Binding::EditorYank, key) {
            self.yank();
        } else if keys.matches(Binding::EditorYankPop, key) {
            self.yank_pop();
        } else if keys.matches(Binding::EditorCursorLeft, key) {
            self.last_action = LastAction::None;
            if self.cursor > 0 {
                self.cursor = self.cursor.saturating_sub(self.previous_grapheme_len());
            }
        } else if keys.matches(Binding::EditorCursorRight, key) {
            self.last_action = LastAction::None;
            if self.cursor < self.value.len() {
                self.cursor += self.next_grapheme_len();
            }
        } else if keys.matches(Binding::EditorCursorLineStart, key) {
            self.last_action = LastAction::None;
            self.cursor = 0;
        } else if keys.matches(Binding::EditorCursorLineEnd, key) {
            self.last_action = LastAction::None;
            self.cursor = self.value.len();
        } else if keys.matches(Binding::EditorCursorWordLeft, key) {
            self.move_word_backwards();
        } else if keys.matches(Binding::EditorCursorWordRight, key) {
            self.move_word_forwards();
        } else if let OverlayKeyCode::Char(c) = key.code
            && !key.ctrl
            && !key.alt
            && !c.is_control()
        {
            // A control character, or an alt-prefixed key, is rejected (`input.ts:202-210`).
            self.insert_character(c);
        }
    }

    /// `render(width)` (`input.ts:266-376`) with a fake (reverse-video) cursor; the hardware-cursor
    /// marker has no counterpart on the overlay seam.
    #[cfg(test)]
    pub(crate) fn render(&self, width: usize) -> OverlayLine {
        self.render_at(width).0
    }

    /// [`Self::render`] plus the text column shown at the left edge of the field, which a click
    /// needs to turn its screen column back into a position in the text.
    pub(crate) fn render_at(&self, width: usize) -> (OverlayLine, usize) {
        let available = width.saturating_sub(visible_width(Self::PROMPT));
        if width <= visible_width(Self::PROMPT) {
            return (
                OverlayLine::new(vec![OverlaySpan::raw(truncate_to_width(
                    Self::PROMPT,
                    width,
                ))]),
                0,
            );
        }
        let total = visible_width(&self.value);
        let mut first_col = 0;
        let (visible_text, cursor_display) = if total < available {
            (self.value.clone(), self.cursor)
        } else {
            let scroll_width = if self.cursor == self.value.len() {
                available - 1
            } else {
                available
            };
            let cursor_col = visible_width(self.value.get(..self.cursor).unwrap_or(""));
            if scroll_width > 0 {
                let half = scroll_width / 2;
                let start = if cursor_col < half {
                    0
                } else if cursor_col > total.saturating_sub(half) {
                    total.saturating_sub(scroll_width)
                } else {
                    cursor_col.saturating_sub(half)
                };
                first_col = start;
                let visible = slice_by_column(&self.value, start, scroll_width);
                let before = slice_by_column(&self.value, start, cursor_col.saturating_sub(start));
                let cursor_display = before.len().min(visible.len());
                (visible, cursor_display)
            } else {
                (String::new(), 0)
            }
        };
        let tail = visible_text.get(cursor_display..).unwrap_or("");
        let at_cursor = tail.graphemes(true).next().unwrap_or(" ");
        let before = visible_text.get(..cursor_display).unwrap_or("");
        let after = visible_text
            .get(cursor_display + at_cursor.len().min(tail.len())..)
            .unwrap_or("");
        let used = visible_width(before) + visible_width(at_cursor) + visible_width(after);
        let mut cursor_span = OverlaySpan::raw(at_cursor);
        cursor_span.reversed = true;
        let mut spans = vec![OverlaySpan::raw(Self::PROMPT)];
        if !before.is_empty() {
            spans.push(OverlaySpan::raw(before));
        }
        spans.push(cursor_span);
        if !after.is_empty() {
            spans.push(OverlaySpan::raw(after));
        }
        let padding = available.saturating_sub(used);
        if padding > 0 {
            spans.push(OverlaySpan::raw(" ".repeat(padding)));
        }
        (OverlayLine::new(spans), first_col)
    }

    /// A press on the field's row (`input.ts` `handleMouse`, which acts on `press`, not on the
    /// click that follows it): the caret goes to the START of the grapheme whose cells contain the
    /// pointer, or to the end of the text when the pointer is past it.
    ///
    /// `column` is the cell inside the row, `first_col` the text column the field's left edge
    /// showed when it was painted ([`Self::render_at`], the same window `render` draws); the
    /// two-cell prompt is skipped, so a press on the prompt lands on that first visible column.
    /// Always a cursor movement, so it ends an undo-coalescing run.
    pub(crate) fn press_at(&mut self, column: usize, first_col: usize) {
        self.last_action = LastAction::None;
        let target = first_col + column.saturating_sub(visible_width(Self::PROMPT));
        let mut passed = 0;
        for (offset, grapheme) in self.value.grapheme_indices(true) {
            let next = passed + grapheme_width(grapheme);
            if target < next {
                self.cursor = offset;
                return;
            }
            passed = next;
        }
        self.cursor = self.value.len();
    }
}

// =================================================================================================
// Model list text (`ui.ts:32-52`)
// =================================================================================================

/// `Number(text)` for the arguments `contextLabel` reads: decimal, exponent and the `0x`/`0o`/`0b`
/// forms; an empty string is `0`; everything else is `None` (JS `NaN`/`Infinity`, both rejected by
/// the caller's `Number.isFinite`).
pub(crate) fn js_number(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    let radix = |prefix_lower: &str, base: u32| -> Option<f64> {
        let lower = trimmed.to_ascii_lowercase();
        let digits = lower.strip_prefix(prefix_lower)?;
        u64::from_str_radix(digits, base).ok().map(|n| n as f64)
    };
    if let Some(n) = radix("0x", 16)
        .or_else(|| radix("0o", 8))
        .or_else(|| radix("0b", 2))
    {
        return Some(n);
    }
    if trimmed
        .chars()
        .any(|c| c.is_alphabetic() && !matches!(c, 'e' | 'E'))
    {
        return None;
    }
    trimmed.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// `n >= 1000 ? `${Math.round(n / 1000)}k` : String(n)` (`ui.ts:34`, `:39`).
fn compact_context(value: f64) -> String {
    if value >= 1000.0 {
        format!("{}k", (value / 1000.0 + 0.5).floor())
    } else {
        format!("{value}")
    }
}

/// `contextLabel(model)` (`ui.ts:32-42`).
pub(crate) fn context_label(model: &LlamaModelInfo) -> Option<String> {
    let context = model
        .meta
        .as_ref()
        .and_then(|meta| meta.n_ctx.or(meta.n_ctx_train));
    if let Some(context) = context.filter(|c| *c > 0) {
        return Some(if context >= 1000 {
            format!("{}k", (context + 500) / 1000)
        } else {
            context.to_string()
        });
    }
    let args = model.status.args.as_deref().unwrap_or(&[]);
    for pair in args.windows(2) {
        let (Some(flag), Some(value)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        if flag != "--ctx-size" && flag != "-c" && flag != "-ctx" {
            continue;
        }
        if let Some(number) = js_number(value).filter(|n| *n > 0.0) {
            return Some(compact_context(number));
        }
    }
    None
}

/// `modelDescription(model)` (`ui.ts:44-52`): `loaded · 32k context`, or the status name.
pub(crate) fn model_description(model: &LlamaModelInfo) -> String {
    let mut details: Vec<String> = Vec::new();
    let loaded = matches!(
        model.status.value,
        LlamaModelStatus::Loaded | LlamaModelStatus::Sleeping
    );
    if loaded {
        details.push("loaded".to_string());
    } else if model.status.value != LlamaModelStatus::Unloaded {
        details.push(model.status.value.as_str().to_string());
    }
    if loaded && let Some(context) = context_label(model) {
        details.push(format!("{context} context"));
    }
    details.join(" · ")
}

/// ICU4X collation data for the root locale with Han in radical-stroke order, generated from ICU
/// 77.1 (the ICU of Node 22) by `icu4x-datagen` 2.3.0:
///
/// ```text
/// icu4x-datagen --format blob --locales und --collation-root-han unihan \
///   --markers CollationRootV1 CollationTailoringV1 CollationDiacriticsV1 CollationJamoV1 \
///     CollationMetadataV1 CollationReorderingV1 CollationSpecialPrimariesV1 \
///     NormalizerNfdDataV1 NormalizerNfdTablesV1 \
///   --icuexport-root icuexportdata_release-77-1.zip --cldr-root cldr-47.0.0-json-full.zip \
///   -o collation-root-unihan.postcard
/// ```
///
/// ICU4X's own compiled data is built with `--collation-root-han implicit`, which (like `feruca`)
/// orders Han by code point: `一 中 丽 乙 㐀 𠀀` where Node's `localeCompare` gives
/// `一 𠀀 㐀 丽 中 乙`.
static ROOT_COLLATION_DATA: &[u8] = include_bytes!("../data/collation-root-unihan.postcard");

/// The root collator, built once (`None` only if the embedded data fails to load, which
/// `root_collator_loads` rules out).
static ROOT_COLLATOR: LazyLock<Option<Collator>> = LazyLock::new(|| {
    let provider = BlobDataProvider::try_new_from_static_blob(ROOT_COLLATION_DATA).ok()?;
    Collator::try_new_with_buffer_provider(
        &provider,
        CollatorPreferences::default(),
        CollatorOptions::default(),
    )
    .ok()
});

/// Whether the embedded root collation data loaded.
#[cfg(test)]
pub(crate) fn root_collator_loads() -> bool {
    ROOT_COLLATOR.is_some()
}

/// `left.localeCompare(right)` (`ui.ts:324`, `huggingface.ts:149`): ICU root collation at its
/// default tertiary strength, punctuation non-ignorable, Han in radical-stroke order. Strings ICU
/// holds equal (`é` and `e` + U+0301, or a soft hyphen) compare `Equal`, as `localeCompare` returns
/// `0`, and a stable sort keeps their order as `Array.prototype.sort` does.
pub(crate) fn locale_compare(left: &str, right: &str) -> Ordering {
    match ROOT_COLLATOR.as_ref() {
        Some(collator) => collator.as_borrowed().compare(left, right),
        None => left.cmp(right),
    }
}

/// The sort of `showModels` (`ui.ts:322-325`): loaded first, then by id.
fn compare_models(left: &LlamaModelInfo, right: &LlamaModelInfo) -> Ordering {
    let loaded = |model: &LlamaModelInfo| i32::from(model.status.value == LlamaModelStatus::Loaded);
    (loaded(right) - loaded(left))
        .cmp(&0)
        .then_with(|| locale_compare(&left.id, &right.id))
}

/// `compactCount(value)` (`ui.ts:90-94`).
pub(crate) fn compact_count(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!(
            "{}M",
            to_fixed(value / 1_000_000.0, usize::from(value < 10_000_000.0))
        )
    } else if value >= 1_000.0 {
        format!(
            "{}k",
            to_fixed(value / 1_000.0, usize::from(value < 100_000.0))
        )
    } else {
        format!("{value}")
    }
}

/// `Number.prototype.toFixed(digits)`: the exact decimal expansion rounded half **up** (JS picks the
/// larger candidate on a tie, where Rust's `{:.N}` picks the even one: `1.25` is `"1.3"` here).
pub(crate) fn to_fixed(value: f64, digits: usize) -> String {
    let exact = format!("{value:.30}");
    let (integer, fraction) = exact.split_once('.').unwrap_or((exact.as_str(), ""));
    let mut kept: Vec<u8> = integer
        .bytes()
        .chain(fraction.bytes().take(digits))
        .collect();
    let round_up = fraction.as_bytes().get(digits).is_some_and(|d| *d >= b'5');
    if round_up {
        let mut carry = true;
        for digit in kept.iter_mut().rev() {
            if !carry {
                break;
            }
            if *digit == b'9' {
                *digit = b'0';
            } else if digit.is_ascii_digit() {
                *digit += 1;
                carry = false;
            } else {
                // The sign of a negative value: nothing left to carry into.
                break;
            }
        }
        if carry {
            let position = usize::from(kept.first() == Some(&b'-'));
            kept.insert(position, b'1');
        }
    }
    let split = kept.len().saturating_sub(digits);
    let (head, tail) = kept.split_at(split);
    let head = String::from_utf8_lossy(head);
    if digits == 0 {
        head.into_owned()
    } else {
        format!("{head}.{}", String::from_utf8_lossy(tail))
    }
}

// =================================================================================================
// `frame()` and the key hints (`ui.ts:64-75`, `keybinding-hints.ts`)
// =================================================================================================

/// `keyHint(id, description)` (`keybinding-hints.ts:46`): dim key text, muted description.
fn key_hint(keys: &LlamaKeys, binding: Binding, description: &str) -> Vec<Seg> {
    vec![
        seg(Role::Dim, keys.key_text(binding)),
        seg(Role::Muted, format!(" {description}")),
    ]
}

/// Join hints with ` • ` (`ui.ts:353`). The separator is unstyled: it sits after a hint's closing
/// colour reset, inside the outer `theme.fg("dim", ...)` that the reset has already ended.
fn join_hints(hints: Vec<Vec<Seg>>) -> Vec<Seg> {
    let mut out = Vec::new();
    for (index, hint) in hints.into_iter().enumerate() {
        if index > 0 {
            out.push(seg(Role::Plain, " • "));
        }
        out.extend(hint);
    }
    out
}

/// `DynamicBorder` (`dynamic-border.ts:20`): `─` repeated `max(1, width)`, in the accent colour.
fn border(width: usize) -> OverlayLine {
    OverlayLine::new(vec![span(Role::Accent, "─".repeat(width.max(1)))])
}

/// `frame(theme, title, body, footer)` (`ui.ts:64-75`).
fn frame(title: &str, body: impl Into<Body>, footer: Option<Vec<Seg>>, width: usize) -> Body {
    let mut out = Body::default();
    out.push(border(width), RowHit::None);
    out.extend(
        text_lines(&[seg(Role::AccentBold, title)], 1, 0, width),
        RowHit::None,
    );
    out.append(body.into());
    if let Some(footer) = footer {
        out.push(OverlayLine::default(), RowHit::None);
        out.extend(text_lines(&footer, 1, 0, width), RowHit::None);
    }
    out.push(border(width), RowHit::None);
    out
}

fn spacer() -> OverlayLine {
    OverlayLine::default()
}

// =================================================================================================
// `HuggingFaceSearch` (`ui.ts:96-274`)
// =================================================================================================

/// The per-view `searchCache` (`ui.ts:280`): lowercased query to results.
type SearchCache = Arc<Mutex<HashMap<String, Vec<HuggingFaceModel>>>>;

/// `/^[^/\s]+\/[^:\s]+(?::[^\s:]+)?$/u` (`ui.ts:259`): an exact `owner/repository[:quant]`.
static EXACT_MODEL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[^/\s]+/[^:\s]+(?::[^\s:]+)?$")
        .unwrap_or_else(|_| unreachable!("the exact-model pattern is a literal"))
});

struct InFlight {
    id: u64,
    token: CancellationToken,
}

/// One settled search, sent from its task back to the view.
struct Completion {
    id: u64,
    query: String,
    token: CancellationToken,
    result: Result<Vec<HuggingFaceModel>, LlamaError>,
}

/// `HuggingFaceSearch` (`ui.ts:96-274`): a query box over a results list. Typing starts a
/// 500 ms-debounced search; every keystroke aborts the previous request; results are cached per
/// lowercased query and narrowed locally by fuzzy match; an exact `owner/repository[:quant]`
/// bypasses the list on confirm.
struct HuggingFaceSearch {
    search: SearchFn,
    cache: SearchCache,
    clock: Arc<dyn Clock>,
    runtime: Handle,
    input: TextInput,
    results: Vec<HuggingFaceModel>,
    filtered: Vec<HuggingFaceModel>,
    selected: usize,
    /// The result a press selected and whose release has not arrived: the click confirms THAT row.
    pressed: Option<usize>,
    query: String,
    status: String,
    /// `this.debounce`: the deadline of the pending `setTimeout`.
    debounce_at: Option<u64>,
    /// `this.request`.
    request: Option<InFlight>,
    next_request: u64,
    closed: bool,
    answer: Option<oneshot::Sender<Option<String>>>,
    completions_tx: mpsc::UnboundedSender<Completion>,
    completions_rx: mpsc::UnboundedReceiver<Completion>,
}

impl HuggingFaceSearch {
    fn new(
        search: SearchFn,
        cache: SearchCache,
        clock: Arc<dyn Clock>,
        runtime: Handle,
        answer: oneshot::Sender<Option<String>>,
    ) -> Self {
        let (completions_tx, completions_rx) = mpsc::unbounded_channel();
        Self {
            search,
            cache,
            clock,
            runtime,
            input: TextInput::new(),
            results: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            pressed: None,
            query: String::new(),
            status: STATUS_TOO_SHORT.to_string(),
            debounce_at: None,
            request: None,
            next_request: 0,
            closed: false,
            answer: Some(answer),
            completions_tx,
            completions_rx,
        }
    }

    /// `updateResults` (`ui.ts:146-180`), as lines, with the result window narrowed to
    /// `max_visible` rows (never wider than [`SEARCH_MAX_VISIBLE`]) for a host frame too short for
    /// the natural one.
    fn render_with(&self, width: usize, max_visible: i64) -> Body {
        let max_visible = max_visible.clamp(1, SEARCH_MAX_VISIBLE);
        let mut body = Body::default();
        body.extend(
            text_lines(
                &[seg(Role::Dim, "Model name or owner/repository[:quant]")],
                1,
                0,
                width,
            ),
            RowHit::None,
        );
        let (input_line, first_col) = self.input.render_at(width);
        body.push(input_line, RowHit::Input { first_col });
        body.push(spacer(), RowHit::None);

        let len = i64::try_from(self.filtered.len()).unwrap_or(i64::MAX);
        let selected = i64::try_from(self.selected).unwrap_or(i64::MAX);
        let start = 0.max((selected - max_visible / 2).min(len - max_visible));
        let end = (start + max_visible).min(len);
        let (start_index, end_index) = (
            usize::try_from(start).unwrap_or(0),
            usize::try_from(end).unwrap_or(0),
        );
        for index in start_index..end_index {
            let Some(model) = self.filtered.get(index) else {
                continue;
            };
            let prefix = if index == self.selected { "→ " } else { "  " };
            let details = format!("{} downloads", compact_count(model.downloads));
            let segs = if index == self.selected {
                vec![seg(
                    Role::Accent,
                    format!("{prefix}{}  {details}", model.id),
                )]
            } else {
                vec![
                    seg(Role::Plain, format!("{prefix}{}", model.id)),
                    seg(Role::Muted, format!("  {details}")),
                ]
            };
            body.extend(text_lines(&segs, 0, 0, width), RowHit::Item(index));
        }
        if start > 0 || end < len {
            let indicator = format!("  ({}/{})", self.selected + 1, self.filtered.len());
            body.extend(
                text_lines(&[seg(Role::Dim, indicator)], 0, 0, width),
                RowHit::ListChrome,
            );
        }
        if self.filtered.is_empty() || self.status == STATUS_SEARCHING {
            body.extend(
                text_lines(&[seg(Role::Dim, format!("  {}", self.status))], 0, 0, width),
                RowHit::None,
            );
        }
        body
    }

    /// `filterResults` (`ui.ts:182-191`). The narrowed list keeps the API's order, not the fuzzy
    /// score's: upstream collects the matching ids into a `Set` and filters `results` by it.
    fn filter_results(&mut self) {
        if self.query.is_empty() {
            self.filtered = self.results.clone();
        } else {
            let matches: HashSet<&str> =
                fuzzy_filter(&self.results, &self.query, |m| m.id.as_str())
                    .into_iter()
                    .filter_map(|index| self.results.get(index))
                    .map(|m| m.id.as_str())
                    .collect();
            self.filtered = self
                .results
                .iter()
                .filter(|m| matches.contains(m.id.as_str()))
                .cloned()
                .collect();
        }
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    /// `scheduleSearch` (`ui.ts:193-212`).
    fn schedule_search(&mut self) {
        self.debounce_at = None;
        if let Some(request) = self.request.take() {
            request.token.cancel();
        }
        if self.query.encode_utf16().count() < SEARCH_MIN_QUERY {
            self.status = STATUS_TOO_SHORT.to_string();
            self.filter_results();
            return;
        }
        let cached = lock(&self.cache).get(&self.query.to_lowercase()).cloned();
        if let Some(cached) = cached {
            self.status = if cached.is_empty() {
                STATUS_NONE_FOUND.to_string()
            } else {
                String::new()
            };
            self.results = cached;
            self.filter_results();
            return;
        }
        self.status = STATUS_SEARCHING.to_string();
        self.filter_results();
        self.debounce_at = Some(self.clock.now_ms().saturating_add(SEARCH_DEBOUNCE_MS));
    }

    /// The call half of `runSearch` (`ui.ts:214-233`): create the request, call `search`, and let a
    /// task carry the outcome back. The cache write (`ui.ts:219`) happens in that task, before the
    /// staleness checks, so a result nobody is waiting for is still cached.
    fn run_search(&mut self, query: String) {
        let token = CancellationToken::new();
        let id = self.next_request;
        self.next_request += 1;
        self.request = Some(InFlight {
            id,
            token: token.clone(),
        });
        let future = (self.search)(query.clone(), token.clone());
        let cache = Arc::clone(&self.cache);
        let sender = self.completions_tx.clone();
        self.runtime.spawn(async move {
            let result = future.await;
            if let Ok(models) = &result {
                lock(&cache).insert(query.to_lowercase(), models.clone());
            }
            let _ = sender.send(Completion {
                id,
                query,
                token,
                result,
            });
        });
    }

    /// The settle half of `runSearch` (`ui.ts:220-231`).
    fn complete(&mut self, completion: Completion) -> bool {
        let stale =
            self.closed || completion.token.is_cancelled() || self.query != completion.query;
        let mut changed = false;
        if !stale {
            match completion.result {
                Ok(results) => {
                    self.status = if results.is_empty() {
                        STATUS_NONE_FOUND.to_string()
                    } else {
                        String::new()
                    };
                    self.results = results;
                    self.selected = 0;
                }
                Err(error) => {
                    self.results = Vec::new();
                    self.status = error.to_string();
                }
            }
            self.filter_results();
            changed = true;
        }
        if self.request.as_ref().is_some_and(|r| r.id == completion.id) {
            self.request = None;
        }
        changed
    }

    /// `close(model)` (`ui.ts:235-241`).
    fn close(&mut self, model: Option<String>) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.debounce_at = None;
        if let Some(request) = self.request.take() {
            request.token.cancel();
        }
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(model);
        }
    }

    /// `handleInput` (`ui.ts:243-273`).
    fn handle_key(&mut self, keys: &LlamaKeys, key: &OverlayKey) {
        if self.closed {
            return;
        }
        if keys.matches(Binding::SelectUp, key) {
            if !self.filtered.is_empty() {
                self.selected = if self.selected == 0 {
                    self.filtered.len() - 1
                } else {
                    self.selected - 1
                };
            }
            return;
        }
        if keys.matches(Binding::SelectDown, key) {
            if !self.filtered.is_empty() {
                self.selected = if self.selected + 1 >= self.filtered.len() {
                    0
                } else {
                    self.selected + 1
                };
            }
            return;
        }
        if keys.matches(Binding::SelectConfirm, key) {
            let exact = EXACT_MODEL
                .is_match(&self.query)
                .then(|| self.query.clone());
            let selected = exact.or_else(|| self.filtered.get(self.selected).map(|m| m.id.clone()));
            if let Some(selected) = selected.filter(|s| !s.is_empty()) {
                self.close(Some(selected));
            }
            return;
        }
        if keys.matches(Binding::SelectCancel, key) {
            self.close(None);
            return;
        }
        self.input.handle_key(keys, key);
        self.query_changed();
    }

    /// The pointer over the search view, `hit` being what the row under it was painted as. The
    /// results answer as a [`SelectList`] does (wheel one row clamped, press selects, click
    /// confirms the row); a press in the box places the caret. `true` when the event was the
    /// view's and the frame must be repainted.
    fn pointer(&mut self, hit: RowHit, event: OverlayMouse) -> bool {
        if self.closed {
            return false;
        }
        let last = self.filtered.len().saturating_sub(1);
        match (event, hit) {
            (OverlayMouse::Wheel { lines, .. }, RowHit::Item(_) | RowHit::ListChrome)
                if !self.filtered.is_empty() =>
            {
                let step = if lines < 0 { -1_isize } else { 1 };
                self.selected = self.selected.saturating_add_signed(step).min(last);
                true
            }
            (OverlayMouse::Press { .. }, RowHit::Item(index)) if index < self.filtered.len() => {
                self.selected = index;
                self.pressed = Some(index);
                true
            }
            (OverlayMouse::Click { .. }, RowHit::Item(index)) => {
                let index = self.pressed.take().unwrap_or(index);
                self.selected = index.min(last);
                if let Some(model) = self.filtered.get(index) {
                    let id = model.id.clone();
                    self.close(Some(id));
                    return true;
                }
                false
            }
            // The caret is placed on the PRESS; the click that completes the gesture must not move
            // it again (pi's `Input.handleMouse` answers `press` only).
            (OverlayMouse::Press { column, .. }, RowHit::Input { first_col }) => {
                self.input.press_at(usize::from(column), first_col);
                true
            }
            _ => false,
        }
    }

    /// A bracketed paste into the search box: forwarded to the input as upstream's
    /// `this.input.handleInput(data)` does (`ui.ts:271`), then the query change is picked up as
    /// for a typed key.
    fn handle_paste(&mut self, text: &str) {
        if self.closed {
            return;
        }
        self.input.handle_paste(text);
        self.query_changed();
    }

    /// The tail of `handleInput` (`ui.ts:269-272`): re-read the trimmed query and, when it
    /// changed, schedule a search.
    fn query_changed(&mut self) {
        let query = self.input.value().trim().to_string();
        if query == self.query {
            return;
        }
        self.query = query;
        self.schedule_search();
    }

    /// Advance the clock-driven half: the debounce deadline and settled searches. `true` when the
    /// next frame differs.
    fn tick(&mut self) -> bool {
        let mut changed = false;
        while let Ok(completion) = self.completions_rx.try_recv() {
            changed |= self.complete(completion);
        }
        if let Some(at) = self.debounce_at
            && self.clock.now_ms() >= at
        {
            self.debounce_at = None;
            let query = self.query.clone();
            self.run_search(query);
        }
        changed
    }
}

/// A search view that is replaced (or dropped with the overlay) takes its request with it, so no
/// Hugging Face call outlives the box that asked for it. Upstream's timer and request simply leak
/// in that case (`ui.ts` only aborts in `close`); nothing observable depends on it.
impl Drop for HuggingFaceSearch {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            request.token.cancel();
        }
    }
}

// =================================================================================================
// `LlamaView` (`ui.ts:276-478`)
// =================================================================================================

struct ModelsMode {
    server_url: String,
    list: SelectList,
    by_id: HashMap<String, LlamaModelInfo>,
    answer: Option<oneshot::Sender<LlamaManagerAction>>,
}

impl ModelsMode {
    /// Answer the pending `showModels` with what the list decided (`onSelect` / `onCancel`),
    /// whether a key or a click decided it.
    fn settle(&mut self, event: SelectEvent) {
        match event {
            SelectEvent::Select(item) => {
                let action = if item.value == DOWNLOAD_VALUE {
                    Some(LlamaManagerAction::Download)
                } else {
                    self.by_id
                        .get(&item.value)
                        .map(|model| LlamaManagerAction::Model(Box::new(model.clone())))
                };
                if let (Some(action), Some(answer)) = (action, self.answer.take()) {
                    let _ = answer.send(action);
                }
            }
            SelectEvent::Cancel => {
                if let Some(answer) = self.answer.take() {
                    let _ = answer.send(LlamaManagerAction::Close);
                }
            }
            SelectEvent::None => {}
        }
    }
}

struct SelectMode {
    title: String,
    list: SelectList,
    answer: Option<oneshot::Sender<Option<String>>>,
}

impl SelectMode {
    /// Answer the pending `select` with what the list decided.
    fn settle(&mut self, event: SelectEvent) {
        match event {
            SelectEvent::Select(item) => {
                if let Some(answer) = self.answer.take() {
                    let _ = answer.send(Some(item.value));
                }
            }
            SelectEvent::Cancel => {
                if let Some(answer) = self.answer.take() {
                    let _ = answer.send(None);
                }
            }
            SelectEvent::None => {}
        }
    }
}

/// `this.content` and its input handler (`ui.ts:281-282`).
enum Mode {
    /// The constructor's `frame(theme, "llama.cpp models", [Text("Loading…", 1, 1)])`.
    Loading,
    Models(ModelsMode),
    Select(SelectMode),
    Search(Box<HuggingFaceSearch>),
    Status {
        title: String,
        message: String,
    },
    Progress(ProgressState),
}

/// The `/llama` view as a pure state machine: no terminal, no task, no wall clock.
///
/// Every `show_*`/`search`/`progress` call replaces the content (pi's `setContent`) and hands back
/// the receiver its answer arrives on. Drive it with [`Self::handle_key`] and [`Self::tick`], read
/// it with [`Self::render`]. [`LlamaOverlay`] and [`LlamaOverlayUi`] are the two thin wrappers
/// that put it behind the host and behind the command flow.
pub struct LlamaView {
    keys: Arc<LlamaKeys>,
    clock: Arc<dyn Clock>,
    runtime: Handle,
    search_cache: SearchCache,
    mode: Mode,
    /// `progressResolver`/`progressPromise`: every pending `progress()` call, resolved together.
    progress_waiters: Vec<oneshot::Sender<()>>,
    closed: bool,
    dirty: bool,
    attached: Option<oneshot::Sender<()>>,
    /// What each row of the last render was, for the pointer.
    hits: Vec<RowHit>,
}

impl LlamaView {
    /// A view on the "Loading…" screen. `runtime` is where Hugging Face searches are spawned.
    #[must_use]
    pub fn new(keys: Arc<LlamaKeys>, clock: Arc<dyn Clock>, runtime: Handle) -> Self {
        Self {
            keys,
            clock,
            runtime,
            search_cache: Arc::new(Mutex::new(HashMap::new())),
            mode: Mode::Loading,
            progress_waiters: Vec::new(),
            closed: false,
            dirty: true,
            attached: None,
            hits: Vec::new(),
        }
    }

    /// Whether [`Self::finish`] has been called.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Mark the view finished (pi's `done()`): the overlay reports [`InteractiveOverlay::should_close`].
    pub fn finish(&mut self) {
        self.closed = true;
        self.dirty = true;
    }

    /// `setContent` (`ui.ts:305-319`): drop every pending answer of the old content (they never
    /// resolve, as upstream's promises do not) and show the new one.
    fn set_mode(&mut self, mode: Mode) {
        self.progress_waiters.clear();
        self.mode = mode;
        self.dirty = true;
    }

    /// `showModels` (`ui.ts:321-358`).
    pub fn show_models(
        &mut self,
        server_url: &str,
        models: &[LlamaModelInfo],
    ) -> oneshot::Receiver<LlamaManagerAction> {
        let mut sorted: Vec<LlamaModelInfo> = models.to_vec();
        sorted.sort_by(compare_models);
        let by_id: HashMap<String, LlamaModelInfo> = sorted
            .iter()
            .map(|model| (model.id.clone(), model.clone()))
            .collect();
        let mut items: Vec<SelectItem> = sorted
            .iter()
            .map(|model| SelectItem {
                value: model.id.clone(),
                label: model.id.clone(),
                description: Some(model_description(model)),
            })
            .collect();
        items.push(SelectItem {
            value: DOWNLOAD_VALUE.to_string(),
            label: "Download model…".to_string(),
            description: Some("Hugging Face owner/repository[:quant]".to_string()),
        });
        let max_visible = items.len().min(LIST_MAX_VISIBLE);
        let (answer, receiver) = oneshot::channel();
        self.set_mode(Mode::Models(ModelsMode {
            server_url: server_url.to_string(),
            list: SelectList::new(items, max_visible).with_primary_bounds(36, 56),
            by_id,
            answer: Some(answer),
        }));
        receiver
    }

    /// `select` (`ui.ts:360-379`).
    pub fn show_select(
        &mut self,
        title: &str,
        options: &[String],
    ) -> oneshot::Receiver<Option<String>> {
        let items: Vec<SelectItem> = options
            .iter()
            .map(|option| SelectItem {
                value: option.clone(),
                label: option.clone(),
                description: None,
            })
            .collect();
        let max_visible = items.len().min(LIST_MAX_VISIBLE);
        let (answer, receiver) = oneshot::channel();
        self.set_mode(Mode::Select(SelectMode {
            title: title.to_string(),
            list: SelectList::new(items, max_visible),
            answer: Some(answer),
        }));
        receiver
    }

    /// `searchModels` (`ui.ts:390-413`). The cache lives as long as the view, so a second search in
    /// the same session reuses the first one's results.
    pub fn show_search(&mut self, search: SearchFn) -> oneshot::Receiver<Option<String>> {
        let (answer, receiver) = oneshot::channel();
        let component = HuggingFaceSearch::new(
            search,
            Arc::clone(&self.search_cache),
            Arc::clone(&self.clock),
            self.runtime.clone(),
            answer,
        );
        self.set_mode(Mode::Search(Box::new(component)));
        receiver
    }

    /// `showStatus` (`ui.ts:415-417`).
    pub fn show_status(&mut self, title: &str, message: &str) {
        self.set_mode(Mode::Status {
            title: title.to_string(),
            message: message.to_string(),
        });
    }

    /// `progress` (`ui.ts:419-428`): show the progress view; the receiver resolves when the user
    /// presses `tui.select.cancel`. A view replaced before that drops the sender, which the caller
    /// must treat as "never".
    pub fn show_progress(&mut self, state: &ProgressState) -> oneshot::Receiver<()> {
        let (waiter, receiver) = oneshot::channel();
        // `showingProgress = true; updateProgress(state)`: the content changes, but the pending
        // promise (and the other waiters sharing it) survive.
        self.mode = Mode::Progress(state.clone());
        self.progress_waiters.push(waiter);
        self.dirty = true;
        receiver
    }

    /// `updateProgress` (`ui.ts:430-455`): only repaints the progress view.
    pub fn update_progress(&mut self, state: &ProgressState) {
        if let Mode::Progress(shown) = &mut self.mode {
            *shown = state.clone();
            self.dirty = true;
        }
    }

    /// `handleInput` (`ui.ts:457-467`). The host repaints after every key, as pi's
    /// `requestRender()` does.
    pub fn handle_key(&mut self, key: &OverlayKey) {
        self.mark_attached();
        if self.closed {
            return;
        }
        self.dirty = true;
        let keys = Arc::clone(&self.keys);
        if !self.progress_waiters.is_empty() && keys.matches(Binding::SelectCancel, key) {
            for waiter in self.progress_waiters.drain(..) {
                let _ = waiter.send(());
            }
            return;
        }
        match &mut self.mode {
            Mode::Models(models) => {
                let event = models.list.handle_key(&keys, key);
                models.settle(event);
            }
            Mode::Select(select) => {
                let event = select.list.handle_key(&keys, key);
                select.settle(event);
            }
            Mode::Search(search) => search.handle_key(&keys, key),
            Mode::Loading | Mode::Status { .. } | Mode::Progress(_) => {}
        }
    }

    /// A pointer event inside the box the overlay painted, `event` local to it. Resolved against
    /// what the last render drew on that row ([`RowHit`]), then handled exactly as the key that
    /// does the same thing: a click on a row is the Enter on it. `true` when the view took the
    /// event and the frame must be repainted.
    pub fn handle_mouse(&mut self, event: OverlayMouse) -> bool {
        self.mark_attached();
        if self.closed {
            return false;
        }
        let (_, row) = event.cell();
        let hit = self
            .hits
            .get(usize::from(row))
            .copied()
            .unwrap_or(RowHit::None);
        let handled = match &mut self.mode {
            Mode::Models(models) => match models.list.pointer(hit, event) {
                ListPointer::Ignored => false,
                ListPointer::Handled => true,
                ListPointer::Select(item) => {
                    models.settle(SelectEvent::Select(item));
                    true
                }
            },
            Mode::Select(select) => match select.list.pointer(hit, event) {
                ListPointer::Ignored => false,
                ListPointer::Handled => true,
                ListPointer::Select(item) => {
                    select.settle(SelectEvent::Select(item));
                    true
                }
            },
            Mode::Search(search) => search.pointer(hit, event),
            Mode::Loading | Mode::Status { .. } | Mode::Progress(_) => false,
        };
        self.dirty |= handled;
        handled
    }

    /// A bracketed paste (`handleInput` receiving `\x1b[200~ ... \x1b[201~`, `input.ts:62-98`).
    /// Only the search view has a text field; every other view ignores it, as upstream's select
    /// lists ignore data that is no key.
    pub fn handle_paste(&mut self, text: &str) {
        self.mark_attached();
        if self.closed {
            return;
        }
        self.dirty = true;
        if let Mode::Search(search) = &mut self.mode {
            search.handle_paste(text);
        }
    }

    /// Advance everything driven by time or by a background task. Returns `true` when the next
    /// frame would differ (the host should repaint).
    pub fn tick(&mut self) -> bool {
        self.mark_attached();
        if let Mode::Search(search) = &mut self.mode
            && search.tick()
        {
            self.dirty = true;
        }
        std::mem::take(&mut self.dirty)
    }

    /// `render(width)` (`ui.ts:469-473`): the content, each line cut to `width`.
    #[must_use]
    pub fn render(&mut self, width: usize) -> Vec<OverlayLine> {
        self.render_within(width, None)
    }

    /// [`Self::render`] for a host that clips the frame to `rows`: the list windows shrink (down
    /// to one row) until the whole frame, footer and borders included, fits, so the key hints are
    /// never the rows that get cut. Upstream mounts the view in the editor slot with no clip
    /// (`ui.ts:469-473`); this host's overlay box is at most `OverlayOptions::max_rows` tall.
    #[must_use]
    pub fn render_within(&mut self, width: usize, rows: Option<usize>) -> Vec<OverlayLine> {
        self.mark_attached();
        let body = self.render_content(width, rows);
        self.hits = body.hits;
        body.lines
            .into_iter()
            .map(|line| clip_line(line, width))
            .collect()
    }

    fn mark_attached(&mut self) {
        if let Some(attached) = self.attached.take() {
            let _ = attached.send(());
        }
    }

    fn render_content(&self, width: usize, rows: Option<usize>) -> Body {
        let keys = &self.keys;
        match &self.mode {
            Mode::Loading => frame(
                "llama.cpp models",
                text_lines(&[seg(Role::Muted, "Loading…")], 1, 1, width),
                None,
                width,
            ),
            Mode::Models(models) => fit_rows(rows, models.list.max_visible, |visible| {
                let mut body = Body::from(text_lines(
                    &[seg(Role::Dim, models.server_url.as_str())],
                    1,
                    0,
                    width,
                ));
                body.push(spacer(), RowHit::None);
                body.append(models.list.render_with(width, visible));
                let footer = join_hints(vec![
                    key_hint(keys, Binding::SelectConfirm, "load/unload/download"),
                    key_hint(keys, Binding::SelectCancel, "close"),
                ]);
                frame("llama.cpp models", body, Some(footer), width)
            }),
            Mode::Select(select) => fit_rows(rows, select.list.max_visible, |visible| {
                let mut body = Body::from(vec![spacer()]);
                body.append(select.list.render_with(width, visible));
                let footer = join_hints(vec![
                    key_hint(keys, Binding::SelectConfirm, "select"),
                    key_hint(keys, Binding::SelectCancel, "cancel"),
                ]);
                frame(&select.title, body, Some(footer), width)
            }),
            Mode::Search(search) => fit_rows(rows, SEARCH_VISIBLE_ROWS, |visible| {
                let mut body = Body::from(vec![spacer()]);
                body.append(
                    search.render_with(width, i64::try_from(visible).unwrap_or(SEARCH_MAX_VISIBLE)),
                );
                let footer = join_hints(vec![
                    key_hint(keys, Binding::SelectConfirm, "select"),
                    key_hint(keys, Binding::SelectCancel, "back"),
                ]);
                frame("Download model", body, Some(footer), width)
            }),
            Mode::Status { title, message } => {
                let mut body = vec![spacer()];
                body.extend(text_lines(
                    &[seg(Role::Muted, message.as_str())],
                    1,
                    0,
                    width,
                ));
                frame(title, body, None, width)
            }
            Mode::Progress(state) => {
                let footer = key_hint(keys, Binding::SelectCancel, "stop");
                frame(
                    &state.title,
                    progress_body(state, width),
                    Some(footer),
                    width,
                )
            }
        }
    }
}

/// The frame `render` builds for a list window of `visible` rows, narrowed one row at a time until
/// it fits `rows` (or the window is a single row). `None` is "no clip": the natural window.
fn fit_rows(rows: Option<usize>, natural: usize, render: impl Fn(usize) -> Body) -> Body {
    let mut visible = natural.max(1);
    let Some(rows) = rows else {
        return render(visible);
    };
    loop {
        let body = render(visible);
        if body.lines.len() <= rows || visible <= 1 {
            return body;
        }
        visible -= 1;
    }
}

/// The body of the progress frame (`ui.ts:432-451`): model, message, bar with percent, detail.
fn progress_body(state: &ProgressState, width: usize) -> Vec<OverlayLine> {
    let mut body = text_lines(&[seg(Role::Text, state.model.as_str())], 1, 0, width);
    body.push(spacer());
    body.extend(text_lines(
        &[seg(Role::Muted, state.message.as_str())],
        1,
        0,
        width,
    ));
    if let Some(ratio) = state.ratio {
        body.extend(text_lines(
            &[seg(Role::Accent, progress_bar(ratio))],
            1,
            0,
            width,
        ));
    }
    if let Some(detail) = state.detail.as_deref().filter(|d| !d.is_empty()) {
        body.extend(text_lines(&[seg(Role::Dim, detail)], 1, 0, width));
    }
    body
}

/// `"█".repeat(filled) + "─".repeat(40 - filled) + " " + round(ratio * 100) + "%"` (`ui.ts:438-445`).
///
/// The bar's fill is clamped to `[0, 1]`; the percentage is not, so `1.5` reads `150%`.
pub(crate) fn progress_bar(ratio: f64) -> String {
    if ratio.is_nan() {
        return " NaN%".to_string();
    }
    let filled = (ratio.clamp(0.0, 1.0) * PROGRESS_BAR_CELLS as f64 + 0.5).floor() as usize;
    let filled = filled.min(PROGRESS_BAR_CELLS);
    let percent = (ratio * 100.0 + 0.5).floor();
    format!(
        "{}{} {percent}%",
        "█".repeat(filled),
        "─".repeat(PROGRESS_BAR_CELLS - filled)
    )
}

// =================================================================================================
// The bridge: the overlay the host drives and the `LlamaUi` the command flow awaits
// =================================================================================================

type SharedView = Arc<Mutex<LlamaView>>;

/// The [`InteractiveOverlay`] the host paints: a thin wrapper over the shared [`LlamaView`].
pub struct LlamaOverlay {
    view: SharedView,
}

impl InteractiveOverlay for LlamaOverlay {
    /// `height` is the host frame's; the box is clipped to `OverlayOptions::max_rows` of it (this
    /// overlay keeps the default options), so the list windows shrink to leave the footer and the
    /// bottom border inside it.
    fn render(&mut self, width: usize, height: usize) -> Vec<OverlayLine> {
        let rows = usize::from(
            self.options()
                .max_rows(u16::try_from(height).unwrap_or(u16::MAX)),
        );
        lock(&self.view).render_within(width, Some(rows))
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        let mut view = lock(&self.view);
        view.handle_key(&key);
        if view.is_closed() {
            OverlayOutcome::Close
        } else {
            OverlayOutcome::Redraw
        }
    }

    fn handle_mouse(&mut self, event: OverlayMouse) -> OverlayMouseOutcome {
        let mut view = lock(&self.view);
        let handled = view.handle_mouse(event);
        if view.is_closed() {
            OverlayMouseOutcome::Close
        } else if handled {
            OverlayMouseOutcome::Redraw
        } else {
            // A row of chrome (title, hint, blank) is not the view's: the host may select it.
            OverlayMouseOutcome::Unhandled
        }
    }

    fn handle_paste(&mut self, text: &str) -> OverlayOutcome {
        let mut view = lock(&self.view);
        view.handle_paste(text);
        if view.is_closed() {
            OverlayOutcome::Close
        } else {
            OverlayOutcome::Redraw
        }
    }

    fn refresh_ms(&self) -> u64 {
        REFRESH_MS
    }

    fn tick(&mut self) -> bool {
        lock(&self.view).tick()
    }

    fn should_close(&self) -> bool {
        lock(&self.view).is_closed()
    }
}

/// Await an answer; a dropped sender means the content was replaced, which upstream's promise
/// answers with silence forever.
async fn answered<T>(receiver: oneshot::Receiver<T>) -> T {
    match receiver.await {
        Ok(value) => value,
        Err(_) => std::future::pending().await,
    }
}

/// The [`LlamaUi`] over the shared [`LlamaView`]; clone it freely.
#[derive(Clone)]
pub struct LlamaOverlayUi {
    view: SharedView,
}

impl LlamaOverlayUi {
    fn view(&self) -> MutexGuard<'_, LlamaView> {
        lock(&self.view)
    }

    /// Pi's `done()` (`ui.ts:484`): the overlay reports [`InteractiveOverlay::should_close`] and
    /// every later [`LlamaUi`] call answers at once with its "cancelled" value instead of waiting
    /// for a user who is gone. [`show_llama_ui`] calls this when the command flow ends.
    pub fn finish(&self) {
        self.view().finish();
    }
}

#[async_trait]
impl LlamaUi for LlamaOverlayUi {
    async fn show_models(&self, server_url: &str, models: &[LlamaModelInfo]) -> LlamaManagerAction {
        let receiver = {
            let mut view = self.view();
            if view.is_closed() {
                return LlamaManagerAction::Close;
            }
            view.show_models(server_url, models)
        };
        answered(receiver).await
    }

    async fn select(&self, title: &str, options: &[String]) -> Option<String> {
        let receiver = {
            let mut view = self.view();
            if view.is_closed() {
                return None;
            }
            view.show_select(title, options)
        };
        answered(receiver).await
    }

    async fn search_models(&self, search: SearchFn) -> Option<String> {
        let receiver = {
            let mut view = self.view();
            if view.is_closed() {
                return None;
            }
            view.show_search(search)
        };
        answered(receiver).await
    }

    fn show_status(&self, title: &str, message: &str) {
        let mut view = self.view();
        if !view.is_closed() {
            view.show_status(title, message);
        }
    }

    async fn progress(&self, state: &ProgressState) {
        let receiver = {
            let mut view = self.view();
            if view.is_closed() {
                return;
            }
            view.show_progress(state)
        };
        answered(receiver).await;
    }

    fn update_progress(&self, state: &ProgressState) {
        self.view().update_progress(state);
    }
}

/// Build the overlay and the handle over one fresh [`LlamaView`]. The receiver resolves the first
/// time the host paints, ticks or sends a key to the overlay, i.e. when the host has really taken
/// it.
#[must_use]
pub fn llama_overlay(
    keys: Arc<LlamaKeys>,
    clock: Arc<dyn Clock>,
    runtime: Handle,
) -> (LlamaOverlay, LlamaOverlayUi, oneshot::Receiver<()>) {
    let (attached_tx, attached_rx) = oneshot::channel();
    let mut view = LlamaView::new(keys, clock, runtime);
    view.attached = Some(attached_tx);
    let view = Arc::new(Mutex::new(view));
    (
        LlamaOverlay {
            view: Arc::clone(&view),
        },
        LlamaOverlayUi { view },
        attached_rx,
    )
}

/// Finishes a [`LlamaOverlayUi`] when dropped, so an overlay the host still shows reports
/// `should_close` once [`show_llama_ui`]'s future is gone.
struct FinishOnDrop(LlamaOverlayUi);

impl Drop for FinishOnDrop {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// How [`show_llama_ui`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlamaUiOutcome {
    /// The flow ran to its end (a failure was reported through `notify` first) and the overlay closed.
    Finished,
    /// The host has no interactive surface (`open_overlay` answered `false`); the flow did not run.
    NoSurface,
    /// The host tore the overlay down while the flow was still running; the flow was dropped.
    Dismissed,
}

/// `showLlamaUi(ctx, run)` (`ui.ts:480-492`): open the overlay, run the command flow against it,
/// close the overlay when the flow ends. A flow that fails is reported with `notify(message,
/// "error")` before the overlay closes (`ui.ts:485-488`).
///
/// `HostServices::open_overlay` blocks until the overlay closes, so it runs on a blocking thread
/// while the flow is polled here. The flow starts only once the host has painted the overlay: a
/// host with no terminal answers `false` at once and the flow is never started (pi's
/// `ctx.ui.custom` is likewise not run without a UI).
pub async fn show_llama_ui<R, Fut, E>(
    host: Arc<dyn HostServices>,
    keys: LlamaKeys,
    run: R,
) -> LlamaUiOutcome
where
    R: FnOnce(Arc<dyn LlamaUi>) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    let (overlay, ui, attached) = llama_overlay(
        Arc::new(keys),
        Arc::new(SystemClock::new()),
        Handle::current(),
    );
    // Finishes the view however this future ends. If the caller drops it while the overlay is open
    // (a dispatcher cancel, a session shutting down) the view still reports `should_close`, so the
    // host tears the overlay down and the blocking thread parked in `open_overlay` returns, instead
    // of the overlay staying up with nothing behind it.
    let finisher = FinishOnDrop(ui.clone());
    let open_host = Arc::clone(&host);
    let mut open = tokio::task::spawn_blocking(move || open_host.open_overlay(Box::new(overlay)));

    tokio::select! {
        biased;
        joined = &mut open => {
            return match joined {
                Ok(true) => LlamaUiOutcome::Dismissed,
                Ok(false) | Err(_) => LlamaUiOutcome::NoSurface,
            };
        }
        _ = attached => {}
    }

    let flow = run(Arc::new(ui));
    tokio::pin!(flow);
    tokio::select! {
        outcome = &mut flow => {
            if let Err(error) = outcome {
                host.notify(&error.to_string(), NotifyKind::Error);
            }
            finisher.0.finish();
            let _ = (&mut open).await;
            LlamaUiOutcome::Finished
        }
        _ = &mut open => {
            finisher.0.finish();
            LlamaUiOutcome::Dismissed
        }
    }
}
