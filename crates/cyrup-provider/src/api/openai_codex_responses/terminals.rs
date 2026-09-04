//! Terminals.

use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, AssistantMessage, CancelToken, StopReason};

/// A terminal error carrying pi's exact thrown message. Like pi's own catch block (`:489-499`) the
/// content is empty: nothing had been accumulated when the failure occurred on these paths.
pub(super) fn error_event(
    model: &Model,
    api: &ApiId,
    message: String,
    aborted: bool,
) -> StreamEvent {
    let msg = AssistantMessage::errored(
        model.provider.clone(),
        model.id.as_str(),
        Some(api.clone()),
        if aborted {
            StopReason::Aborted
        } else {
            StopReason::Error
        },
        message,
    );
    StreamEvent::terminal(msg)
}

/// pi's abort terminal: `stopReason = "aborted"` with the `"Request was aborted"` text its
/// `throw` produced (`:397`, `:449-451`, `:495-497`).
pub(super) fn aborted_event(model: &Model, api: &ApiId) -> StreamEvent {
    error_event(model, api, "Request was aborted".to_string(), true)
}

/// Interruptible `sleep(ms, signal)` (pi `:185-197`). `false` means the abort fired.
pub(super) async fn sleep_or_abort(cancel: &CancelToken, delay_ms: u64) -> bool {
    if cancel.is_cancelled() {
        return false;
    }
    cancel
        .run_until_cancelled(tokio::time::sleep(std::time::Duration::from_millis(
            delay_ms,
        )))
        .await
        .is_some()
}
