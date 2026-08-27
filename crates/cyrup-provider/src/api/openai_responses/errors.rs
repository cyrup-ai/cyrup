//! Stream decoding (Pi processResponsesStream, openai-responses-shared.ts:295-531):
//! the terminal `error` event (Pi catch block).

use super::decoder::RDecoder;
use crate::api::EventSink;
use crate::model::Model;
use crate::stream::StreamEvent;
use cyrup_core::{ApiId, StopReason};

/// Emit a terminal `error` event carrying the live snapshot + message (Pi catch block).
pub(super) async fn emit_error(
    dec: &RDecoder,
    model: &Model,
    api: &ApiId,
    sink: &EventSink,
    message: String,
) {
    let mut msg = dec.snapshot(model, api);
    msg.stop_reason = StopReason::Error;
    msg.error_message = Some(message);
    sink.send(StreamEvent::terminal(msg)).await;
}
