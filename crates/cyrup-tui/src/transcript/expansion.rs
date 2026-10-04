//! **Per-entry expansion** — the click-to-toggle half of pi's expand/collapse model.
//!
//! # Upstream
//! Pi gives every collapsible component a *local* `expanded` field. Two writers reach it:
//!
//! * the global broadcast — `setToolsExpanded` (`Ctrl+O`, `app.tools.expand`) walks every
//!   `isExpandable` child of the chat container and **overwrites** its local flag
//!   (`interactive-mode.ts:4474-4491` @v1.0.0). Tool blocks, the `!` bash block, branch and
//!   compaction summaries, skill invocations and custom messages are expandable; an
//!   `AssistantMessageComponent` is not.
//! * a left click on the component's `MouseRegion`, which flips only that component's flag
//!   (`tool-execution.ts:175-181`, `branch-summary-message.ts`, `compaction-summary-message.ts`,
//!   `skill-invocation-message.ts`). A thinking run is the odd one out: its override lives in
//!   `AssistantMessageComponent.thinkingVisibilityOverrides`, one boolean per run, and only
//!   `setHideThinkingBlock` clears it (`assistant-message.ts:57-62`) — `Ctrl+O` leaves it alone.
//!
//! cyrup keeps no component per entry (ADR-0005 §B-1: a drained turn is an [`Entry`] value), so the
//! local flag becomes an **override map** beside the global one. An entry with no override renders
//! at the global value, which is exactly what pi's "seed `setExpanded(toolOutputExpanded)` at
//! construction, re-broadcast on every toggle" amounts to; an entry with one renders at it. The
//! global writers clear the overrides pi's broadcast would have overwritten, per [`ToggleScope`].
//!
//! # Identity
//! An override is keyed by the entry's **sequence number** — `retained_dropped + index` — so it
//! survives the document's front trim (ADR-0005 §B-1), which moves every index but no sequence
//! number. Entries the trim evicts drop their override with them ([`TranscriptView::prune_expansion`]).

use std::collections::BTreeMap;
use std::ops::Range;

use super::*;

/// One entry's expansion, when it differs from what the global flag would give it.
///
/// A two-state enum rather than a `bool` so a call site reads `Expansion::Open` and a stored value
/// cannot be confused with "no override", which is the `Option` around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expansion {
    /// Rendered expanded, whatever the global flag says.
    Open,
    /// Rendered collapsed, whatever the global flag says.
    Closed,
}

impl Expansion {
    /// `Open` for `true`.
    pub const fn from_open(open: bool) -> Self {
        if open { Self::Open } else { Self::Closed }
    }

    /// Whether this is [`Self::Open`].
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

/// Which broadcast clears an override — the two pi keeps apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToggleScope {
    /// A reasoning run. Cleared by `hideThinkingBlock` changes only
    /// (`assistant-message.ts:57-62`).
    Thinking,
    /// A tool block, branch/compaction summary or skill invocation. Cleared by the tools-expanded
    /// broadcast (`interactive-mode.ts:4474-4491`).
    Block,
}

impl Entry {
    /// Whether a left click on this entry toggles it, and which broadcast owns the result.
    ///
    /// `None` for everything pi does not make clickable: user and plain assistant text, bash
    /// blocks, custom messages, notices — and a tool block that has no result yet
    /// (`if (!this.result …) return undefined`, `tool-execution.ts:176`).
    pub(crate) fn toggle_scope(&self) -> Option<ToggleScope> {
        match self {
            Entry::Thinking { .. } => Some(ToggleScope::Thinking),
            Entry::Tool(run) if run.result.is_some() => Some(ToggleScope::Block),
            Entry::BranchSummary { .. }
            | Entry::CompactionSummary { .. }
            | Entry::SkillInvocation { .. } => Some(ToggleScope::Block),
            _ => None,
        }
    }

    /// Whether the entry renders expanded when it has no override of its own.
    fn open_by_default(&self, tools_expanded: bool) -> bool {
        match self {
            // The reasoning body is shown unless the run committed under `hideThinkingBlock`.
            Entry::Thinking { hidden, .. } => !hidden,
            _ => tools_expanded,
        }
    }
}

/// The cells of an entry's rows a left click toggles it from.
///
/// Pi's `MouseRegion` wraps the component's *content*, inside the tinted `Box` padding, so the
/// padding rows and the padding columns of a boxed block do not respond; a bare `Container`
/// checks only the row. Both are expressed here, relative to the entry's first row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToggleRegion {
    rows: Range<usize>,
    cols: Range<usize>,
}

impl ToggleRegion {
    pub(crate) fn new(rows: Range<usize>, cols: Range<usize>) -> Self {
        Self { rows, cols }
    }

    /// The last row of the region, relative to the entry's first row.
    pub(crate) fn last_row(&self) -> Option<usize> {
        self.rows
            .end
            .checked_sub(1)
            .filter(|&r| r >= self.rows.start)
    }

    /// Whether `(row, col)` — `row` relative to the entry's first row — is inside the region.
    pub(crate) fn contains(&self, row: usize, col: usize) -> bool {
        self.rows.contains(&row) && self.cols.contains(&col)
    }
}

struct Override {
    scope: ToggleScope,
    state: Expansion,
}

/// A block of the in-flight turn a click can toggle: pi wraps a streaming thinking run in its
/// `MouseRegion` from the first delta, and a tool block answers as soon as it has a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveBlock {
    /// The reasoning run still streaming.
    Thinking,
    /// The tool run at this index of the turn's running tools.
    Tool(usize),
}

/// What a click landed on: a retained entry, or a block of the in-flight turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleTarget {
    /// Index into [`TranscriptView::document`].
    Entry(usize),
    /// A block still being drawn from the live buffers.
    Live(LiveBlock),
}

/// The click region of one live block, in the rows of the wrapped in-flight turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LiveToggle {
    /// The block's first row within the in-flight rows.
    pub start: usize,
    /// The cells that toggle it, relative to `start`.
    pub region: ToggleRegion,
    pub block: LiveBlock,
}

/// The overrides, and a counter that moves whenever any of them changes what would be drawn.
#[derive(Default)]
pub(super) struct ExpansionOverrides {
    by_seq: BTreeMap<u64, Override>,
    generation: u64,
    /// The click override of the reasoning run that is still streaming. A tool run carries its own
    /// ([`ToolRun::live_expansion`]); both are moved onto `by_seq` when the block commits.
    pub(super) live_thinking: Option<Expansion>,
}

impl TranscriptView {
    /// The sequence number of the retained entry at `index`: stable across the front trim.
    fn entry_seq(&self, index: usize) -> u64 {
        self.retained_dropped
            .saturating_add(u64::try_from(index).unwrap_or(u64::MAX))
    }

    /// The override on the retained entry at `index`, if a click (or [`Self::toggle_entry`]) set one.
    pub fn entry_expansion(&self, index: usize) -> Option<Expansion> {
        self.expansion
            .by_seq
            .get(&self.entry_seq(index))
            .map(|o| o.state)
    }

    /// Flip the retained entry at `index` — pi's `setExpanded(!expanded)` on a click — and answer
    /// the state it now has, or `None` when the entry is not one a click toggles.
    ///
    /// The flip is relative to what the entry shows now: its override if it has one, otherwise the
    /// global flag (a thinking run: whether it committed hidden).
    pub fn toggle_entry(&mut self, index: usize) -> Option<Expansion> {
        let entry = self.document.get(index)?;
        let scope = entry.toggle_scope()?;
        let seq = self.entry_seq(index);
        let open = match self.expansion.by_seq.get(&seq) {
            Some(o) => o.state.is_open(),
            None => entry.open_by_default(self.tool_expanded),
        };
        let state = Expansion::from_open(!open);
        self.expansion.by_seq.insert(seq, Override { scope, state });
        self.expansion.generation = self.expansion.generation.wrapping_add(1);
        Some(state)
    }

    /// Flip whatever a click landed on.
    pub fn toggle(&mut self, target: ToggleTarget) -> Option<Expansion> {
        match target {
            ToggleTarget::Entry(index) => self.toggle_entry(index),
            ToggleTarget::Live(block) => self.toggle_live(block),
        }
    }

    /// Flip a block of the in-flight turn. Its override is parked on the live buffer (the thinking
    /// run) or on the run (a tool), and [`Self::push_pending_with`] moves it onto the entry's
    /// sequence number at the commit, so the committed entry renders as the live block did.
    fn toggle_live(&mut self, block: LiveBlock) -> Option<Expansion> {
        let state = match block {
            LiveBlock::Thinking => {
                self.thinking.as_ref()?;
                let open = self
                    .expansion
                    .live_thinking
                    .map_or(!self.hide_thinking, Expansion::is_open);
                let state = Expansion::from_open(!open);
                self.expansion.live_thinking = Some(state);
                state
            }
            LiveBlock::Tool(index) => {
                let default = self.tool_expanded;
                let run = self.active_tools.get_mut(index)?;
                // `if (!this.result …) return undefined` (`tool-execution.ts:176`).
                run.result.as_ref()?;
                let open = run.live_expansion.map_or(default, Expansion::is_open);
                let state = Expansion::from_open(!open);
                run.live_expansion = Some(state);
                state
            }
        };
        self.bump_render_generation();
        Some(state)
    }

    /// Push a committed entry, carrying the override its live block had onto the sequence number the
    /// entry takes in the retained document.
    ///
    /// That number is `retained_dropped + document.len() + pending.len()` at the push: a drain
    /// appends `pending` in order, and a front trim moves `retained_dropped` and every index by the
    /// same amount, so the sequence is the same at the push and at the drain. The carry only runs
    /// while the document is retained — an inline session has no document to key into.
    pub(super) fn push_pending_with(&mut self, entry: Entry, live: Option<Expansion>) {
        if let Some(state) = live
            && self.retain_document
            && let Some(scope) = entry.toggle_scope()
        {
            let seq = self.entry_seq(self.document.len() + self.pending.len());
            self.expansion.by_seq.insert(seq, Override { scope, state });
            self.expansion.generation = self.expansion.generation.wrapping_add(1);
        }
        self.pending.push(entry);
    }

    /// The override of the committed-but-undrained entry at `index` of `pending`.
    pub(super) fn pending_expansion(&self, index: usize) -> Option<Expansion> {
        self.expansion
            .by_seq
            .get(&self.entry_seq(self.document.len() + index))
            .map(|o| o.state)
    }

    /// How many overrides are held — the observation point for "an evicted entry drops its own".
    #[cfg(test)]
    pub(crate) fn tracked_expansions(&self) -> usize {
        self.expansion.by_seq.len()
    }

    /// A counter that changes whenever an override is set or cleared — the part of the retained
    /// document's render key that the entries themselves do not carry.
    pub fn expansion_generation(&self) -> u64 {
        self.expansion.generation
    }

    /// Clear every override of `scope`: the global broadcast overwriting the local flags.
    pub(super) fn reset_expansion(&mut self, scope: ToggleScope) {
        let before = self.expansion.by_seq.len();
        self.expansion.by_seq.retain(|_, o| o.scope != scope);
        // The live blocks are components too, and the same broadcast overwrites their flags.
        match scope {
            ToggleScope::Thinking => self.expansion.live_thinking = None,
            ToggleScope::Block => {
                for run in &mut self.active_tools {
                    run.live_expansion = None;
                }
            }
        }
        if self.expansion.by_seq.len() != before {
            self.expansion.generation = self.expansion.generation.wrapping_add(1);
        }
    }

    /// Forget the overrides of entries the front trim evicted. Does not move the generation: the
    /// evicted entries are gone from the document, and the trim moves `retained_dropped`, which the
    /// render key already carries.
    pub(super) fn prune_expansion(&mut self) {
        self.expansion.by_seq = self.expansion.by_seq.split_off(&self.retained_dropped);
    }
}
