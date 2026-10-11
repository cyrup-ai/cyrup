//! The live record of one script's nested calls: the rows the details list shows, the usage of its
//! `models.*` calls, and the count of generated images (pi `calls`, `modelUsage`,
//! `generatedImages`, `snapshot()` and `publish()` in `executeCodemode`,
//! `extensions/codemode/execute.ts:304-326` @v1.0.1).
//!
//! Every nested call and every model call pushes a row when it starts and updates it when it ends;
//! the rows are published to the tool's `on_update` sink as a snapshot, which is how a front end
//! shows calls while they run.
//!
//! # Cost
//!
//! [CYRUP-DELTA] Upstream publishes a snapshot of every row on every change, which costs a script
//! that makes `n` calls O(n²) copies and serialisations, each delivered to every subscriber (the
//! TUI, the json and rpc streams). A JavaScript runtime copes with a few thousand rows; here three
//! thousand reads held the process at 3 GB. Two bounds keep a change O(1) amortised:
//!
//! * a snapshot is published at most once per [`PUBLISH_INTERVAL`]. The first change is published
//!   at once; changes inside the interval only mark the record dirty, and
//!   [`Recorder::publish_until_dropped`] (and the final [`Recorder::flush`]) publish the latest
//!   state, so every call's start and end reaches a subscriber eventually;
//! * only the latest [`MAX_LIVE_ROWS`] rows are kept. Older ones are tallied into one leading row,
//!   `... N earlier calls`, so the list stays bounded for the script's whole life and the
//!   renderer's own "earlier calls" fold still reads naturally. The persisted `nestedCalls` caps in
//!   `cyrup-core` are separate and unchanged.
//!
//! # Production call path
//!
//! [`super::execute::execute_codemode`] creates one [`Recorder`] per script and hands it to the
//! tool callbacks and to the `models` globals ([`super::models::models_globals`]).

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use cyrup_core::{ToolCallId, ToolUpdate, ToolUpdateSink, Usage};

use super::{CodemodeNestedCall, CodemodeNestedCallStatus, CodemodeToolDetails};

/// Characters of a nested call's arguments the details keep (`ARGS_PREVIEW_CHARS`,
/// `execute.ts:46`).
pub const ARGS_PREVIEW_CHARS: usize = 200;
/// Characters of a nested call's error the details keep (`ERROR_PREVIEW_CHARS`, `execute.ts:47`).
pub const ERROR_PREVIEW_CHARS: usize = 500;
/// Rows the live list keeps; older ones are summarised in one row (see the module docs). Well
/// above what the renderer shows folded and above the calls that can be running at once.
pub const MAX_LIVE_ROWS: usize = 500;
/// The least time between two published snapshots (see the module docs).
pub const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

/// `truncateText` (`execute.ts:54-56`): at most `max` UTF-16 units, ending in `...` when cut.
#[must_use]
pub fn truncate_text(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max {
        return text.to_owned();
    }
    let kept = units.get(..max.saturating_sub(3)).unwrap_or_default();
    format!("{}...", String::from_utf16_lossy(kept))
}

/// The calls that fell out of the live list, counted by how they ended.
#[derive(Default)]
struct Evicted {
    total: usize,
    /// Still running when evicted, and not reported since.
    running: usize,
    failed: usize,
    cancelled: usize,
}

impl Evicted {
    fn add(&mut self, status: CodemodeNestedCallStatus) {
        self.total += 1;
        self.tally(status);
    }

    /// Count a call that is no longer running as `status`.
    fn tally(&mut self, status: CodemodeNestedCallStatus) {
        match status {
            CodemodeNestedCallStatus::Running => self.running += 1,
            CodemodeNestedCallStatus::Ok => {}
            CodemodeNestedCallStatus::Error => self.failed += 1,
            CodemodeNestedCallStatus::Cancelled => self.cancelled += 1,
        }
    }

    /// The `... N earlier calls` row, or `None` while nothing has been evicted.
    fn row(&self, tool_call_id: &ToolCallId) -> Option<CodemodeNestedCall> {
        (self.total > 0).then(|| CodemodeNestedCall {
            id: format!("{tool_call_id}/earlier"),
            name: format!("... {} earlier calls", self.total),
            args: if self.failed > 0 {
                format!("{} failed", self.failed)
            } else {
                String::new()
            },
            status: if self.running > 0 {
                CodemodeNestedCallStatus::Running
            } else if self.failed > 0 {
                CodemodeNestedCallStatus::Error
            } else if self.cancelled > 0 {
                CodemodeNestedCallStatus::Cancelled
            } else {
                CodemodeNestedCallStatus::Ok
            },
            duration_ms: None,
            error: None,
            cost: None,
        })
    }
}

/// When the next snapshot may go out: at most one per `interval`, and one owed for every change
/// the interval held back.
struct Gate {
    interval: Duration,
    last: Option<Instant>,
    /// A change has not been published yet.
    dirty: bool,
}

impl Gate {
    /// Whether a snapshot may be published at `now`. When it may not, the change is remembered
    /// for [`Recorder::flush`].
    fn admit(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval)
        {
            self.last = Some(now);
            self.dirty = false;
            true
        } else {
            self.dirty = true;
            false
        }
    }
}

struct State {
    /// The latest [`MAX_LIVE_ROWS`] rows, oldest first.
    rows: VecDeque<CodemodeNestedCall>,
    /// How many rows came before `rows[0]`: [`RowId`]s count from the first row ever begun.
    first: usize,
    evicted: Evicted,
    /// Usage of the script's `models.*` calls. Nested tool calls report theirs through the session.
    model_usage: Option<Usage>,
    /// Images returned by `models.generateImages()`, to notice a script that never shows them.
    generated_images: usize,
    gate: Gate,
}

/// See the module docs.
pub struct Recorder {
    tool_call_id: ToolCallId,
    state: Mutex<State>,
    on_update: Mutex<ToolUpdateSink>,
}

/// Which row a call updates.
#[derive(Clone, Copy, Debug)]
pub struct RowId(usize);

impl Recorder {
    pub fn new(tool_call_id: ToolCallId, on_update: ToolUpdateSink) -> Self {
        Self::with_publish_interval(tool_call_id, on_update, PUBLISH_INTERVAL)
    }

    pub(crate) fn with_publish_interval(
        tool_call_id: ToolCallId,
        on_update: ToolUpdateSink,
        interval: Duration,
    ) -> Self {
        Self {
            tool_call_id,
            state: Mutex::new(State {
                rows: VecDeque::new(),
                first: 0,
                evicted: Evicted::default(),
                model_usage: None,
                generated_images: 0,
                gate: Gate {
                    interval,
                    last: None,
                    dirty: false,
                },
            }),
            on_update: Mutex::new(on_update),
        }
    }

    /// The tool call every row's id is derived from.
    pub fn tool_call_id(&self) -> &ToolCallId {
        &self.tool_call_id
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn rows_of(&self, state: &State) -> Vec<CodemodeNestedCall> {
        state
            .evicted
            .row(&self.tool_call_id)
            .into_iter()
            .chain(state.rows.iter().cloned())
            .collect()
    }

    /// The details as of now (`snapshot`, `execute.ts:324`).
    pub fn snapshot(&self) -> CodemodeToolDetails {
        CodemodeToolDetails {
            calls: self.rows_of(&self.state()),
            full_output_path: None,
        }
    }

    /// Publish the current rows as a partial result (`publish`, `execute.ts:325`).
    fn publish(&self) {
        // The sink is taken first, so two publishes reach it in the order of their snapshots.
        let mut sink = self
            .on_update
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let details = serde_json::to_value(self.snapshot()).ok();
        (sink)(ToolUpdate {
            content: Vec::new(),
            details,
            ..ToolUpdate::default()
        });
    }

    /// Something changed: publish now if the interval allows, else owe a snapshot.
    fn changed(&self) {
        let due = self.state().gate.admit(Instant::now());
        if due {
            self.publish();
        }
    }

    /// Publish the latest state if a change is still owed a snapshot.
    pub fn flush(&self) {
        {
            let mut state = self.state();
            if !state.gate.dirty {
                return;
            }
            state.gate.dirty = false;
            state.gate.last = Some(Instant::now());
        }
        self.publish();
    }

    /// Flush the owed snapshot once per interval, for as long as this future is polled, so a call
    /// that starts or ends inside an interval is reported at the end of it, not at the next change.
    pub async fn publish_until_dropped(&self) -> Infallible {
        let interval = self.state().gate.interval;
        let mut ticks = tokio::time::interval(interval.max(Duration::from_millis(1)));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            self.flush();
        }
    }

    /// Add a row and publish.
    pub fn begin(&self, row: CodemodeNestedCall) -> RowId {
        let id = {
            let mut state = self.state();
            let id = RowId(state.first + state.rows.len());
            state.rows.push_back(row);
            while state.rows.len() > MAX_LIVE_ROWS {
                if let Some(old) = state.rows.pop_front() {
                    state.first += 1;
                    state.evicted.add(old.status);
                }
            }
            id
        };
        self.changed();
        id
    }

    /// Change a row and publish. A row is updated once, when its call ends; one that has left the
    /// live list is then only counted (`change` runs on a stand-in for it).
    pub fn update(&self, row: RowId, change: impl FnOnce(&mut CodemodeNestedCall)) {
        {
            let mut state = self.state();
            let first = state.first;
            match row.0.checked_sub(first) {
                Some(index) => {
                    if let Some(call) = state.rows.get_mut(index) {
                        change(call);
                    }
                }
                None => {
                    let mut ended = CodemodeNestedCall {
                        id: String::new(),
                        name: String::new(),
                        args: String::new(),
                        status: CodemodeNestedCallStatus::Running,
                        duration_ms: None,
                        error: None,
                        cost: None,
                    };
                    change(&mut ended);
                    if ended.status != CodemodeNestedCallStatus::Running
                        && state.evicted.running > 0
                    {
                        state.evicted.running -= 1;
                        state.evicted.tally(ended.status);
                    }
                }
            }
        }
        self.changed();
    }

    /// Add the usage of a `models.*` call (`addModelUsage`, `execute.ts:316`).
    pub fn add_model_usage(&self, usage: &Usage) {
        let mut state = self.state();
        state.model_usage = Some(match &state.model_usage {
            Some(total) => cyrup_core::combine_usage(total, usage),
            None => usage.clone(),
        });
    }

    /// Count images a `models.generateImages()` call returned (`addGeneratedImages`).
    pub fn add_generated_images(&self, count: usize) {
        self.state().generated_images += count;
    }

    pub fn model_usage(&self) -> Option<Usage> {
        self.state().model_usage.clone()
    }

    pub fn generated_images(&self) -> usize {
        self.state().generated_images
    }

    /// Mark every row still running as cancelled — the script ended, timed out or was aborted
    /// while it ran (`execute.ts:386-388`) — and return the rows. A change is owed a snapshot
    /// ([`Self::flush`]).
    pub fn finish(&self) -> Vec<CodemodeNestedCall> {
        let mut state = self.state();
        let mut changed = false;
        for call in &mut state.rows {
            if call.status == CodemodeNestedCallStatus::Running {
                call.status = CodemodeNestedCallStatus::Cancelled;
                changed = true;
            }
        }
        if state.evicted.running > 0 {
            state.evicted.cancelled += state.evicted.running;
            state.evicted.running = 0;
            changed = true;
        }
        state.gate.dirty |= changed;
        self.rows_of(&state)
    }
}

#[cfg(test)]
mod tests;
