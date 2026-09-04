//! The shared, size-capped append-only JSONL primitive (R-SA-136/146): every `.jsonl` artifact
//! this crate writes — the foreground/background-shared per-attempt child-output tee
//! ([`crate::spawn::SpawnedChild`]'s `jsonl_writer`) and the background async-run event log
//! (`<run_dir>/events.jsonl`, [`crate::background::RunPaths::events`]) — goes through
//! [`BoundedJsonlWriter`], so there is exactly ONE byte-budget-enforcement implementation for the
//! whole crate, not one per call site (mirroring `background::atomic`'s identical "one shared
//! primitive, not two" convention for R-SA-076/135).
//!
//! # The cap contract (func-SA §5.6 R-SA-129/136; §8 R-SA-146)
//!
//! > JSONL run-event logs... MUST be append-only, MUST silently cap total bytes written per file
//! > at a fixed budget (target: 50MB) without erroring the run, and MUST NOT attempt to rewrite or
//! > truncate earlier lines.
//!
//! This is a deliberate, disclosed port of a known `pi-subagents` limitation (func-SA §5.6's own
//! text), not a design flaw this port should "improve on" by, say, rotating files or erroring the
//! run once the cap is hit. The contract this module implements, precisely:
//!
//! - Every line successfully accepted BEFORE the cap is reached is written completely and
//!   durably — never partially, never a torn line.
//! - The FIRST line that would push cumulative bytes-written past the cap (and every line after
//!   it) is silently dropped: not written at all, no error returned, no panic.
//! - Already-written lines are never rewritten, truncated, or otherwise mutated once the cap is
//!   hit — [`BoundedJsonlWriter`] never seeks backward or truncates the file; it only ever
//!   appends (or stops appending).
//! - The run itself is never failed, errored, or interrupted merely because a `.jsonl` artifact
//!   hit its cap — a caller that keeps calling [`BoundedJsonlWriter::write_line`] past the cap
//!   simply gets a continued stream of no-op `Ok(())` returns, exactly as if every subsequent line
//!   were accepted and silently discarded.
//!
//! # Why "bytes written so far" is tracked in-process, not re-derived from a `stat` per call
//!
//! [`BoundedJsonlWriter`] tracks its own running byte count across calls (seeded once from the
//! target file's existing size at construction, so re-opening an already-partially-written file —
//! e.g. a background run resuming — does not reset the budget). This avoids a `stat`-per-line
//! syscall on the hot per-line tee path ([`crate::spawn::SpawnedChild::next_event`] calls
//! [`BoundedJsonlWriter::write_line`] once per NDJSON line read from a child's stdout, which can be
//! a very high-frequency path for a chatty child) while still being exactly as accurate as a
//! `stat`-based check would be, since this type is the SOLE writer of its target path for its own
//! lifetime (never shared/cloned across tasks — mirrors [`crate::spawn::SpawnedChild`]'s own
//! single-owning-task invariant).

use std::io;
use std::path::Path;

use tokio::io::AsyncWriteExt;

/// The default per-file byte budget (func-SA §5.6 R-SA-136's "target: 50MB"), enforced identically
/// for every `.jsonl` artifact this crate writes unless a call site explicitly overrides it via
/// [`BoundedJsonlWriter::create_with_cap`].
pub const DEFAULT_JSONL_CAP_BYTES: u64 = 50 * 1024 * 1024;

/// An append-only JSONL file writer enforcing a maximum total-bytes-written budget
/// (R-SA-136/146): once the budget is reached, further [`BoundedJsonlWriter::write_line`] calls
/// are silent, successful no-ops — never an error, never a panic, and never a rewrite/truncation
/// of the bytes already durably written.
///
/// Owned by exactly one task at a time (never shared bare across threads), matching this crate's
/// established single-owning-task convention for live I/O handles
/// ([`crate::spawn::SpawnedChild`]'s own doc comment states the identical invariant for the child
/// process it wraps).
pub struct BoundedJsonlWriter {
    file: tokio::fs::File,
    cap_bytes: u64,
    bytes_written: u64,
    /// `true` once the cap has been reached at least once — kept only so
    /// [`BoundedJsonlWriter::is_capped`] can answer without re-comparing `bytes_written` against
    /// `cap_bytes` on every call and so a caller (e.g. a diagnostic/doctor check) can observe that
    /// the cap was hit even after `bytes_written` has stopped changing.
    capped: bool,
}

impl BoundedJsonlWriter {
    /// Opens (creating if absent) `path` for append, using [`DEFAULT_JSONL_CAP_BYTES`] as the byte
    /// budget.
    ///
    /// If `path` already has content (e.g. a background run's `events.jsonl` being re-opened by a
    /// resumed runner), the existing file size seeds `bytes_written` so the budget accounts for
    /// bytes written in a PRIOR process lifetime too — the cap is a per-file lifetime budget, not
    /// a per-writer-instance one.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` if `path` cannot be opened/created in append mode, or if its current
    /// size cannot be determined.
    pub async fn create(path: &Path) -> io::Result<Self> {
        Self::create_with_cap(path, DEFAULT_JSONL_CAP_BYTES).await
    }

    /// Identical to [`BoundedJsonlWriter::create`], with an explicit `cap_bytes` override instead
    /// of [`DEFAULT_JSONL_CAP_BYTES`] — primarily for tests that need a small, fast-to-exceed
    /// budget rather than genuinely writing 50MB of scripted output.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` under the identical conditions as [`BoundedJsonlWriter::create`].
    pub async fn create_with_cap(path: &Path, cap_bytes: u64) -> io::Result<Self> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let bytes_written = file.metadata().await?.len();
        let capped = bytes_written >= cap_bytes;
        Ok(Self {
            file,
            cap_bytes,
            bytes_written,
            capped,
        })
    }

    /// Writes `line` plus a trailing newline, unmodified, IFF doing so would not push cumulative
    /// bytes-written past this writer's cap; otherwise this is a silent no-op (R-SA-136's "silently
    /// cap... without erroring the run").
    ///
    /// The check is all-or-nothing per line: a line that would only PARTIALLY fit under the
    /// remaining budget is dropped in its entirety, never truncated mid-line — R-SA-136 explicitly
    /// forbids "rewrite or truncate," and a half-written JSON line would itself be a corrupt line
    /// even if no earlier line were touched. Once the cap is hit, every subsequent call becomes an
    /// immediate no-op without even attempting a partial write.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` only for a genuine I/O failure on the underlying write/flush WHILE
    /// still under the cap — never for the cap itself being reached (that path returns `Ok(())`).
    pub async fn write_line(&mut self, line: &str) -> io::Result<()> {
        if self.capped {
            return Ok(());
        }

        // +1 for the trailing newline this call always appends alongside `line`'s own bytes.
        let line_bytes = line.len() as u64 + 1;
        if self.bytes_written.saturating_add(line_bytes) > self.cap_bytes {
            self.capped = true;
            return Ok(());
        }

        self.file.write_all(line.as_bytes()).await?;
        self.file.write_all(b"\n").await?;
        self.file.flush().await?;
        self.bytes_written += line_bytes;
        if self.bytes_written >= self.cap_bytes {
            self.capped = true;
        }
        Ok(())
    }

    /// `true` once this writer has dropped (or would drop) a line for being over budget — useful
    /// for diagnostics/tests that want to assert the cap was actually exercised, without needing
    /// to re-derive that from the file's own size.
    #[must_use]
    pub fn is_capped(&self) -> bool {
        self.capped
    }

    /// Total bytes this writer has actually written to `path` so far (including any bytes already
    /// present at construction time, per [`BoundedJsonlWriter::create`]'s doc comment) — never
    /// exceeds the configured cap.
    #[must_use]
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// The configured byte budget for this writer.
    #[must_use]
    pub fn cap_bytes(&self) -> u64 {
        self.cap_bytes
    }
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

    /// Every line written while comfortably under the cap lands verbatim, in order, and the file
    /// remains fully valid line-delimited JSONL.
    #[tokio::test]
    async fn writes_under_the_cap_all_land_correctly() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("under-cap.jsonl");

        let mut writer = BoundedJsonlWriter::create_with_cap(&path, 1024)
            .await
            .expect("opens");
        for i in 0..10 {
            let line = format!(r#"{{"type":"unknown","n":{i}}}"#);
            writer
                .write_line(&line)
                .await
                .expect("write under cap succeeds");
        }
        assert!(
            !writer.is_capped(),
            "10 short lines must not exceed a 1KB cap"
        );

        let contents = tokio::fs::read_to_string(&path).await.expect("readable");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 10, "every line must be present");
        for (i, line) in lines.iter().enumerate() {
            let parsed: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line {i} must be valid JSON: {e}: {line}"));
            assert_eq!(
                parsed["n"], i,
                "lines must land in write order, uncorrupted"
            );
        }
    }

    /// Writes that would cross the cap stop cleanly: earlier lines remain fully intact and
    /// individually parseable, the line that would cross the boundary (and everything after it)
    /// is dropped entirely rather than truncated mid-line, and no error/panic ever surfaces.
    #[tokio::test]
    async fn writes_crossing_the_cap_stop_cleanly_leaving_a_valid_prefix() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("cross-cap.jsonl");

        // Each line is exactly 20 bytes (a fixed-width, zero-padded STRING `n` field — never a
        // bare JSON number, so a leading zero like "05" stays valid JSON — keeps every line's
        // byte length identical regardless of `i`'s own digit count) + 1 newline = 21 bytes. A
        // 100-byte cap fits exactly 4 full lines (84 bytes) with 16 bytes of remaining budget —
        // not enough for a 5th 21-byte line, so the 5th (and every subsequent) line must be
        // dropped whole, never split.
        let cap = 100u64;
        let mut writer = BoundedJsonlWriter::create_with_cap(&path, cap)
            .await
            .expect("opens");

        for i in 0..50 {
            let line = format!(r#"{{"n":"{i:02}","pad":"x"}}"#); // fixed 20-byte body
            assert_eq!(
                line.len(),
                20,
                "test fixture line must be exactly 20 bytes: {line:?}"
            );
            writer
                .write_line(&line)
                .await
                .expect("write_line never errors, even over cap");
        }

        assert!(
            writer.is_capped(),
            "50 lines against a 100-byte cap must have triggered the cap"
        );
        assert!(
            writer.bytes_written() <= cap,
            "writer must never report more bytes written than the configured cap: {} > {cap}",
            writer.bytes_written()
        );

        let contents = tokio::fs::read_to_string(&path).await.expect("readable");
        assert!(
            contents.len() as u64 <= cap,
            "on-disk file must never exceed the cap: {} bytes on disk, cap {cap}",
            contents.len()
        );

        // The critical corruption check: every byte on disk must form a complete, valid,
        // individually-parseable sequence of whole lines — no partial/torn trailing line from a
        // write that got cut off mid-line.
        assert!(
            contents.ends_with('\n') || contents.is_empty(),
            "the file must end on a clean line boundary, never mid-line: {contents:?}"
        );
        let mut expected_n = 0u32;
        for line in contents.lines() {
            let parsed: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("every on-disk line must be valid JSON: {e}: {line:?}"));
            assert_eq!(
                parsed["n"],
                serde_json::Value::String(format!("{expected_n:02}")),
                "lines on disk must be an unbroken, in-order prefix of what was submitted"
            );
            expected_n += 1;
        }
        assert_eq!(
            expected_n, 4,
            "exactly 4 of the 20-byte lines fit in the 100-byte cap (4*21=84 <= 100 < 5*21=105)"
        );
    }

    /// Calling `write_line` far past the point the cap was reached never panics and never returns
    /// an `Err` — the defining "silently dropped without corrupting already-written lines"
    /// behavior, exercised with many repeated post-cap calls (not just the one call that first
    /// crosses the boundary).
    #[tokio::test]
    async fn no_panic_or_error_propagates_from_exceeding_the_cap() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("post-cap.jsonl");

        let cap = 32u64;
        let mut writer = BoundedJsonlWriter::create_with_cap(&path, cap)
            .await
            .expect("opens");

        // Push well past the cap with many further calls; every single one must return Ok(()).
        for i in 0..500u32 {
            let line = format!(r#"{{"type":"unknown","payload":"line-{i}-padded-out-long"}}"#);
            let result = writer.write_line(&line).await;
            assert!(
                result.is_ok(),
                "write_line must never return Err merely for being over the cap (call {i})"
            );
        }

        assert!(writer.is_capped());
        assert!(writer.bytes_written() <= cap);

        // The file itself must still be intact and fully parseable up to whatever prefix landed.
        let contents = tokio::fs::read_to_string(&path)
            .await
            .expect("readable, not corrupted");
        for line in contents.lines() {
            let _: serde_json::Value =
                serde_json::from_str(line).expect("every persisted line remains valid JSON");
        }
    }

    /// A line that would land EXACTLY at the cap boundary is accepted (the cap is "at most this
    /// many bytes," not "strictly fewer than").
    #[tokio::test]
    async fn a_line_landing_exactly_at_the_cap_boundary_is_accepted() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("exact-boundary.jsonl");

        // `{"n":0}` is 7 bytes; +1 newline = 8 bytes total. Cap the budget at exactly 8.
        let cap = 8u64;
        let mut writer = BoundedJsonlWriter::create_with_cap(&path, cap)
            .await
            .expect("opens");

        writer
            .write_line(r#"{"n":0}"#)
            .await
            .expect("first line fits exactly at the cap");
        assert_eq!(writer.bytes_written(), cap);
        assert!(
            writer.is_capped(),
            "reaching the cap exactly still marks the writer as capped"
        );

        // A second line must now be dropped entirely, not partially written.
        writer
            .write_line(r#"{"n":1}"#)
            .await
            .expect("post-cap call is a clean no-op");
        assert_eq!(
            writer.bytes_written(),
            cap,
            "no further bytes accepted once at the cap"
        );

        let contents = tokio::fs::read_to_string(&path).await.expect("readable");
        assert_eq!(contents, "{\"n\":0}\n");
    }

    /// Re-opening a path that already has content (simulating a background run resuming into an
    /// existing `events.jsonl`) seeds the budget from the file's existing size, rather than
    /// resetting the cap to "0 bytes written" and allowing a full fresh 50MB (or test-cap-sized)
    /// budget on top of what a prior process lifetime already wrote.
    #[tokio::test]
    async fn reopening_an_existing_file_seeds_the_budget_from_its_current_size() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("resumed.jsonl");

        tokio::fs::write(&path, b"0123456789\n")
            .await
            .expect("seed existing content");

        let cap = 15u64;
        let mut writer = BoundedJsonlWriter::create_with_cap(&path, cap)
            .await
            .expect("reopens");
        assert_eq!(
            writer.bytes_written(),
            11,
            "must start from the file's existing size"
        );

        // Only 4 bytes of budget remain (15 - 11); a 3-byte line + 1 newline = 4 bytes fits
        // exactly.
        writer
            .write_line("abc")
            .await
            .expect("fits in the remaining 4 bytes");
        assert_eq!(writer.bytes_written(), 15);
        assert!(writer.is_capped());

        let contents = tokio::fs::read_to_string(&path).await.expect("readable");
        assert_eq!(contents, "0123456789\nabc\n");
    }

    /// The default cap constant matches R-SA-136's documented target (50MB) exactly, so a reader
    /// of this test can confirm the crate-wide default without needing to trust the doc comment
    /// alone.
    #[test]
    fn default_cap_is_fifty_megabytes() {
        assert_eq!(DEFAULT_JSONL_CAP_BYTES, 50 * 1024 * 1024);
    }
}
