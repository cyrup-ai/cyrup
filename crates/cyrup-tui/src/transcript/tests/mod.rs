//! The transcript module's in-tree unit tests. They live inside `crate::transcript` (not
//! `crate::tests`) so they can reach the module's private helpers and poison
//! [`TranscriptView`](super::TranscriptView)'s private render-cache fields.

mod output_pad;
mod progressive_commit;
mod render_cache;
mod rhythm_followup;
mod skill;
mod vertical_rhythm;
mod x_group;
