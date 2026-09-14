//! Durable completion replay — the bounded on-disk record that makes a `wait` arriving AFTER
//! delivery still resolve.
//!
//! Ports pi `runs/background/completion-replay.ts` (287 LOC). Closes `SUBA-056`.
//!
//! # The race this closes
//!
//! `deliver_pending_completions` ([`crate::background::watch`]) UNLINKS a payload once its
//! delivery receipt lands (R-SA-099's delete-last, `watch/install.rs`'s
//! `watcher.consume(payload, receipt)` inside `settle_delivery`). A `wait` that resolves a moment
//! later has no file to read. WORKFLOW_4's in-process
//! [`crate::background::wait_completions::WaitCompletionStore`] covers that window for the lifetime
//! of ONE process; it does not survive a restart, and it is pruned at
//! [`crate::background::watch::DEDUP_TTL`]. This module is the durable tier underneath it — and it
//! is written BEFORE the unlink, which is the whole correctness argument (see
//! [`write_completion_replay`]).
//!
//! # Layout
//!
//! ```text
//! <results_dir>/
//!   completion-replay/<encodeURIComponent(runId)>.json   the record: completion + TTL + sessionId
//!   output-archives/<encodeURIComponent(runId)>.json     the archive: artifact refs + bounded text
//! ```
//!
//! `<encodeURIComponent(...)>` is the RAW encoder, deliberately NOT
//! [`crate::identity::IndexSegment`] — see [`replay_file_name`].
//!
//! # Why two files and not one
//!
//! The record is small and fixed-shape; the archive holds up to [`ARCHIVE_TEXT_LIMIT_BYTES`] of
//! fallback text. Keeping them apart means the hot read ([`read_completion_replay`], once per
//! `wait` miss) never pays for 64 KiB it usually does not need — the record carries the archive's
//! PATH and a caller fetches the text only when it wants it ([`read_completion_archive`]).
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs        facade: the constants (they ARE the format), the path builders, this narrative
//! record.rs     CompletionReplayRecord + its tolerant parser + validate_replay_record
//! archive.rs    CompletionArchive/Entry + write_completion_archive (the 64 KiB truncation)
//! store.rs      write_completion_replay / read_completion_replay / read_completion_archive
//! retention.rs  cleanup_completion_replay{,_if_due} + the per-results-dir throttle
//! ```

use std::path::{Path, PathBuf};

use crate::background::RunId;
use crate::identity::ResultFileName;

mod archive;
mod record;
mod retention;
mod store;

pub use archive::{
    ArchiveSource, ArchiveVersion, CompletionArchive, CompletionArchiveEntry,
    write_completion_archive,
};
pub use record::{CompletionReplayRecord, ReplayVersion};
pub use retention::{cleanup_completion_replay, cleanup_completion_replay_if_due};
pub use store::{
    CompletionReplayWrite, ReplayReadFilter, read_completion_archive, read_completion_replay,
    write_completion_replay,
};

/// pi `REPLAY_VERSION` (`completion-replay.ts:7`). An on-disk format version: a record carrying
/// any other value is SKIPPED, never repaired and never a hard error.
pub const REPLAY_VERSION: u32 = 1;
/// pi `ARCHIVE_VERSION` (`:8`).
pub const ARCHIVE_VERSION: u32 = 1;
/// pi `ARCHIVE_TEXT_LIMIT_BYTES` (`:9`) — 64 KiB of retained fallback text per entry.
pub const ARCHIVE_TEXT_LIMIT_BYTES: usize = 64 * 1024;
/// pi `REPLAY_DIR_NAME` (`:10`).
pub const REPLAY_DIR_NAME: &str = "completion-replay";
/// pi `ARCHIVE_DIR_NAME` (`:11`).
pub const ARCHIVE_DIR_NAME: &str = "output-archives";
/// pi `CLEANUP_INTERVAL_MS` (`:12`) — 60 s. A THROTTLE, not a timer; see
/// [`cleanup_completion_replay_if_due`].
pub const CLEANUP_INTERVAL_MS: i64 = 60_000;

/// pi `safeRunFile` (`completion-replay.ts:41-43`) — `<encodeURIComponent(runId)>.json`.
///
/// The RAW encoder, not [`crate::identity::IndexSegment`]: `IndexSegment::encode_bounded`'s
/// `~sha256-` fallback (`identity/path_segment.rs`) would put a long or non-portable run id's
/// record at a name pi does not look at, and `async-retention.ts:505` independently rebuilds this
/// exact name inline. Two upstream files agree on the raw encoder, so it is the format, not an
/// oversight — and this record is explicitly a shared, versioned on-disk shape.
fn replay_file_name(run_id: &RunId) -> String {
    format!(
        "{}{}",
        crate::identity::encode_uri_component(run_id.as_str()),
        // `identity/result_name.rs`'s `".json"` — the same extension every payload file carries.
        ResultFileName::EXTENSION,
    )
}

/// pi `completionReplayPath` (`:45-47`).
#[must_use]
pub fn completion_replay_path(results_dir: &Path, run_id: &RunId) -> PathBuf {
    results_dir
        .join(REPLAY_DIR_NAME)
        .join(replay_file_name(run_id))
}

/// pi `completionArchivePath` (`:49-51`).
#[must_use]
pub fn completion_archive_path(results_dir: &Path, run_id: &RunId) -> PathBuf {
    results_dir
        .join(ARCHIVE_DIR_NAME)
        .join(replay_file_name(run_id))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// §2's format rule: the replay file name is the RAW `encodeURIComponent`, never an
    /// [`crate::identity::IndexSegment`].
    ///
    /// The divergence is only reachable through [`RunId::from_token`], which validates nothing — a
    /// run id parsed back from a directory name or a CLI argument. `RunId::new` mints a 32-char hex
    /// UUID that encodes to itself, so in production the two encoders AGREE; this test constructs
    /// the two ids where they do not, because pi would look for the record at the raw name and find
    /// nothing if cyrup had hashed it.
    #[test]
    fn the_replay_file_name_is_the_raw_uri_encoding() {
        // Over `IndexSegment`'s 255-byte ceiling once encoded — `encode_bounded` would emit
        // `~sha256-<hex>` here.
        let long = RunId::from_token("x".repeat(300));
        let name = replay_file_name(&long);
        assert_eq!(name, format!("{}.json", "x".repeat(300)));
        assert!(!name.starts_with('~'), "never the hashed fallback: {name}");

        // Non-portable: a trailing `.jsonl`-shaped extension, which `is_portable_segment` rejects
        // and `encode_bounded` therefore hashes. The raw encoder does not care.
        let session_shaped = RunId::from_token("run/one.jsonl");
        assert_eq!(
            replay_file_name(&session_shaped),
            "run%2Fone.jsonl.json",
            "uppercase %XX hex, and the extension left intact"
        );

        // And the two path builders compose it under the two documented directory names.
        let dir = Path::new("/results");
        let run = RunId::from_token("a b");
        assert_eq!(
            completion_replay_path(dir, &run),
            Path::new("/results/completion-replay/a%20b.json")
        );
        assert_eq!(
            completion_archive_path(dir, &run),
            Path::new("/results/output-archives/a%20b.json")
        );
    }
}
