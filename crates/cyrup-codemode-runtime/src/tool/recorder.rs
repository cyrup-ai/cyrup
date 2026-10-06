//! The live record of one script's nested calls: the rows the details list shows, the usage of its
//! `models.*` calls, and the count of generated images (pi `calls`, `modelUsage`,
//! `generatedImages`, `snapshot()` and `publish()` in `executeCodemode`,
//! `extensions/codemode/execute.ts:304-326` @v1.0.1).
//!
//! Every nested call and every model call pushes a row when it starts and updates it when it ends;
//! each change is published to the tool's `on_update` sink as a snapshot, which is how a front end
//! shows calls while they run.
//!
//! # Production call path
//!
//! [`super::execute::execute_codemode`] creates one [`Recorder`] per script and hands it to the
//! tool callbacks and to the `models` globals ([`super::models::models_globals`]).

use std::sync::{Mutex, PoisonError};

use cyrup_core::{ToolCallId, ToolUpdate, ToolUpdateSink, Usage};

use super::{CodemodeNestedCall, CodemodeNestedCallStatus, CodemodeToolDetails};

/// Characters of a nested call's arguments the details keep (`ARGS_PREVIEW_CHARS`,
/// `execute.ts:46`).
pub const ARGS_PREVIEW_CHARS: usize = 200;
/// Characters of a nested call's error the details keep (`ERROR_PREVIEW_CHARS`, `execute.ts:47`).
pub const ERROR_PREVIEW_CHARS: usize = 500;

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

#[derive(Default)]
struct State {
    calls: Vec<CodemodeNestedCall>,
    /// Usage of the script's `models.*` calls. Nested tool calls report theirs through the session.
    model_usage: Option<Usage>,
    /// Images returned by `models.generateImages()`, to notice a script that never shows them.
    generated_images: usize,
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
        Self {
            tool_call_id,
            state: Mutex::new(State::default()),
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

    /// The details as of now (`snapshot`, `execute.ts:324`).
    pub fn snapshot(&self) -> CodemodeToolDetails {
        CodemodeToolDetails {
            calls: self.state().calls.clone(),
            full_output_path: None,
        }
    }

    /// Publish the current rows as a partial result (`publish`, `execute.ts:325`).
    fn publish(&self) {
        let details = serde_json::to_value(self.snapshot()).ok();
        let mut sink = self
            .on_update
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        (sink)(ToolUpdate {
            content: Vec::new(),
            details,
            ..ToolUpdate::default()
        });
    }

    /// Add a row and publish.
    pub fn begin(&self, row: CodemodeNestedCall) -> RowId {
        let id = {
            let mut state = self.state();
            state.calls.push(row);
            RowId(state.calls.len() - 1)
        };
        self.publish();
        id
    }

    /// Change a row and publish.
    pub fn update(&self, row: RowId, change: impl FnOnce(&mut CodemodeNestedCall)) {
        if let Some(call) = self.state().calls.get_mut(row.0) {
            change(call);
        }
        self.publish();
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
    /// while it ran (`execute.ts:386-388`) — and return the rows.
    pub fn finish(&self) -> Vec<CodemodeNestedCall> {
        let mut state = self.state();
        for call in &mut state.calls {
            if call.status == CodemodeNestedCallStatus::Running {
                call.status = CodemodeNestedCallStatus::Cancelled;
            }
        }
        state.calls.clone()
    }
}
