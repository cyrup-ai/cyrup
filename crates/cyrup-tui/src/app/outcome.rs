use super::*;

/// One entry of Pi's `compactionQueuedMessages` (`interactive-mode.ts:401`, the
/// `CompactionQueuedMessage` record `{ text, mode: "steer" | "followUp" }`). TUI-031.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionQueued {
    /// The submitted text, verbatim.
    pub text: String,
    /// `mode === "followUp"` — Alt+Enter's queue rather than Enter's steering queue.
    pub follow_up: bool,
}

/// One mounted extension widget — Pi's entry in `extensionWidgetsAbove` / `extensionWidgetsBelow`
/// (`interactive-mode.ts:1920-1960` @v0.83.0). TUI-014.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionWidget {
    /// Pi's `key`: the identity a re-emit replaces on.
    pub key: String,
    /// The rendered rows, already capped at [`ExtensionWidget::MAX_WIDGET_LINES`] with pi's
    /// truncation marker appended when the content was longer.
    pub lines: Vec<String>,
    /// `options.placement === "belowEditor"` (`:1925`, `:1957`); the default is `"aboveEditor"`.
    pub below: bool,
}

impl ExtensionWidget {
    /// `InteractiveMode.MAX_WIDGET_LINES` (`interactive-mode.ts:2008`).
    pub const MAX_WIDGET_LINES: usize = 10;

    /// Pi's truncation row, appended verbatim when the content exceeded the cap (`:1948-1950`,
    /// `theme.fg("muted", "... (widget truncated)")`).
    pub const TRUNCATED: &'static str = "... (widget truncated)";

    /// Read Pi's three `setWidget` arguments off the [`UiEffect::SetWidget`] carrier.
    ///
    /// SEAM-011/EXT-047: `set-widget` carries pi's `key`, `lines` and `placement` separately now
    /// (`wit/world.wit`, `HostServices::set_widget`), and `LiveHostServices` re-packs exactly those
    /// three under pi's own names for this in-process channel (`host_services.rs:724-737`) — so this
    /// reads `{"key": …, "lines": [...], "placement": "aboveEditor" | "belowEditor"}` and nothing
    /// else. It used to read a cyrup-invented `{"content": …, "options": {"placement": …}}` blob;
    /// after the seam widened, that spelling stopped arriving and every widget was dropped.
    ///
    /// `lines` is Pi's `content: string[]` arm (`:1942-1951`); `null`/absent is Pi's
    /// `content === undefined`, which REMOVES the key (`:1935-1938`) and is read here as an empty
    /// line list. A payload that is not an object at all has no structure to recover, so it renders
    /// as its JSON text under an empty key.
    pub fn from_json(v: &serde_json::Value) -> Self {
        let obj = v.as_object();
        let key = obj
            .and_then(|o| o.get("key"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        // `options?.placement ?? "aboveEditor"` (`interactive-mode.ts:1925` @v0.83.0) — the WIT
        // resolves the default host-side, so the carrier always spells the placement out.
        let placement = obj
            .and_then(|o| o.get("placement"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("aboveEditor");
        let below = placement == "belowEditor";
        let content = obj.and_then(|o| o.get("lines"));
        let mut lines: Vec<String> = match content {
            Some(serde_json::Value::String(text)) => {
                text.lines().map(str::to_string).collect()
            }
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .map(|i| {
                    i.as_str().map(str::to_string).unwrap_or_else(|| i.to_string())
                })
                .collect(),
            // `content === undefined` removes the widget (`:1935-1938`) — an empty line list is
            // what the caller reads as "remove".
            Some(serde_json::Value::Null) | None if obj.is_some() => Vec::new(),
            Some(other) => vec![other.to_string()],
            None => vec![v.to_string()],
        };
        if lines.len() > Self::MAX_WIDGET_LINES {
            lines.truncate(Self::MAX_WIDGET_LINES);
            lines.push(Self::TRUNCATED.to_string());
        }
        ExtensionWidget { key, lines, below }
    }
}

/// The `&mut self` work a settled session-lifecycle op still owes the run loop (TUI-092 §5b.2).
///
/// Deliberately tiny: `pending_swap_status` is set OPTIMISTICALLY before the spawn (see
/// [`App::dispatch_lifecycle`]), so a successful op needs nothing from here in the common case.
#[derive(Debug, Default)]
pub struct LifecycleEffects {
    /// `/fork` with `position: "before"` re-seeds the editor with the anchor text
    /// (`RuntimeForkResult::selected_text`).
    pub selected_text: Option<String>,
    /// `/reload` rebuilds the keymaps from this agent dir. Runs AFTER the session reload, which is
    /// Pi's order (`interactive-mode.ts:5386`, session reload then `this.keybindings.reload()`).
    pub reload_keybindings_in: Option<PathBuf>,
}

/// What a spawned session-lifecycle op hands back (TUI-092 §5b.2).
///
/// `Err` carries an ALREADY-RENDERED status line — the per-command cancellation string or error
/// wording Pi uses — so the run loop needs no context to display it, and the optimistic
/// `pending_swap_status` is cleared with it.
#[derive(Debug)]
pub struct LifecycleOutcome(pub Result<LifecycleEffects, String>);

/// Why a [`QueueDrain`] was requested — it decides what the run loop does with the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueDrainReason {
    /// `Escape` while streaming (`AppAction::InterruptRestoreQueued`, Pi `onEscape`
    /// interactive-mode.ts:2636-2637): restore to the editor, then abort the run and any bash child.
    Interrupt,
    /// `Alt+Up` (`AppAction::Dequeue`, Pi `handleDequeue` interactive-mode.ts:3587-3594): restore to
    /// the editor and report how many came back. No abort.
    Dequeue,
    /// The `/tree` pre-step (Pi `:4781-4785`). The abort is issued by the spawning task itself,
    /// immediately after the drain and before `navigate_tree`, so only the editor restore is left
    /// for the loop.
    TreeNav,
}

/// What a spawned [`AgentSession::drain_queue`](cyrup_session_svc::AgentSession::drain_queue) hands
/// back — Pi's `(steering, followUp)` pair plus the reason, so
/// [`App::apply_queue_drain`] can finish the job on the loop task (TUI-092 §5b.1).
#[derive(Clone, Debug)]
pub struct QueueDrain {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
    pub reason: QueueDrainReason,
}

/// What a spawned `/compact` hands back to the run loop — the `Ok`/`Err` of
/// [`AgentSession::compact`](cyrup_session_svc::AgentSession::compact), with the error already
/// rendered to a string so the message needs no session to interpret.
pub type CompactOutcome = Result<cyrup_session_svc::CompactionResult, String>;

/// Where `/login` reads the provider registry from — Pi's `modelRuntime.getProviders()`
/// (`interactive-mode.ts:4943`). See [`App::set_login_provider_source`].
pub type LoginProviderSource =
    Arc<dyn Fn() -> Vec<Arc<dyn cyrup_provider::Provider>> + Send + Sync>;

/// A spawned `/tree` navigation's outcome, posted back to [`App::run`]'s `select!` so the summarize
/// leg never runs on the loop task (the `bash_rx` / `shortcut_status_rx` channel-back pattern).
/// Keeping it off-task is what makes Pi's Escape→`abortBranchSummary` binding deliverable at all:
/// awaited inline, the loop would service no key events for the whole provider round-trip.
#[derive(Debug)]
pub struct TreeNavMsg {
    /// The navigated-to entry id, so an aborted summarization can re-show the tree there.
    pub(crate) target: String,
    pub(crate) outcome: Result<NavigateTreeOutcome, String>,
}

impl TreeNavMsg {
    /// Pair a settled navigation with the entry it targeted. `pub` so `tests/*.rs` can hand
    /// [`App::apply_tree_nav_outcome`] a synthetic outcome (notably the abort case, which is
    /// otherwise a race to provoke) — the crate's established run-loop-only testing seam.
    pub fn new(target: impl Into<String>, outcome: Result<NavigateTreeOutcome, String>) -> Self {
        TreeNavMsg { target: target.into(), outcome }
    }
}
