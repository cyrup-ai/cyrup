//! The `Provider` abstraction (arch-01 §6 / func-01 §6).

use crate::context::Context;
use crate::model::Model;
use crate::stream::{StreamEvent, StreamOptions};
use cyrup_core::{EventStream, ProviderId};

/// A runtime unit owning a model catalog, auth, and stream behavior (func-01 §6).
///
/// Slice: `stream` + catalog reads. Auth resolution, dynamic `refresh`, and `stream_simple`
/// (thinking-level mapping) land with the concrete providers.
pub trait Provider: Send + Sync {
    fn id(&self) -> &ProviderId;

    /// Last-known catalog; synchronous and non-throwing (func-01 R-01-001).
    fn models(&self) -> &[Model];

    fn get_model(&self, id: &str) -> Option<&Model> {
        self.models().iter().find(|m| m.id.as_str() == id)
    }

    /// Construct the response stream. Returns immediately; setup happens behind the stream and
    /// failures are delivered as a terminal `StreamEvent::Error` (func-01 R-01-009/045) — this
    /// method never returns `Err`.
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent>;
}
