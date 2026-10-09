//! The transcript module's in-tree unit tests. They live inside `crate::transcript` (not
//! `crate::tests`) so they can reach the module's private helpers and poison
//! [`TranscriptView`](super::TranscriptView)'s private render-cache fields.

mod bash_duration;
mod call_fallback;
mod js_arg;
mod js_number;
mod osc_hyperlinks;
mod output_pad;
mod progressive_commit;
mod read_null_range;
mod recorded_duration;
mod render_cache;
mod rhythm_followup;
mod skill;
mod vertical_rhythm;
mod x_group;
