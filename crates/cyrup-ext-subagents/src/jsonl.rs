//! The crate's append-only JSONL primitives (R-SA-136/146).
//!
//! Two writers, because upstream caps two different things differently:
//!
//! - [`BoundedJsonlWriter`] — a silent, all-or-nothing byte cap. It backs the per-attempt
//!   child-output tee ([`crate::spawn::SpawnedChild`]'s `jsonl_writer`), the child transcript, and
//!   the maintenance log. It is ONE byte-budget implementation for those call sites, not one per
//!   call site (mirroring `background::atomic`'s "one shared primitive, not two" convention).
//! - [`RunEventLog`] — a background run's `events.jsonl` ([`crate::background::RunPaths::events`]).
//!   Its LIFECYCLE lines (`subagent.run.*`, `subagent.step.*`, steering, process-terminal) are
//!   **never** capped, and only its child DIAGNOSTIC lines are, with a one-shot
//!   `subagent.events.truncated` marker written on the first overflow.
//!
//! # Why `events.jsonl` is not a [`BoundedJsonlWriter`]
//!
//! This module used to say the `events.jsonl` cap was "silent" and "a deliberate, disclosed port of
//! a known `pi-subagents` limitation". Neither half was true. Upstream
//! (`runs/background/subagent-runner.ts:266-330` @v0.43.0, `:317-386` @v0.68.0) caps **only**
//! child diagnostic events (`appendDiagnosticJsonl`, called from `appendChildEvent` at `:590-604`
//! @v0.43.0 / `:1223` @v0.68.0), reserves `TRUNCATION_MARKER_RESERVE_BYTES = 512` of the budget,
//! and on the first overflow writes `{type:"subagent.events.truncated", ts, maxBytes,
//! droppedEventType}` exactly once. Every lifecycle event goes through the **uncapped**
//! `appendJsonl`. cyrup had it backwards: it capped the lifecycle trail through the silent writer
//! and journaled no child events at all, so `CYRUP_SUBAGENT_ASYNC_EVENTS_MAX_BYTES=0` erased every
//! `subagent.run.*`/`subagent.step.*` line without a trace, where upstream's `0` drops only the
//! child diagnostics and says so. [`RunEventLog`] is the port.
//!
//! # The [`BoundedJsonlWriter`] contract (func-SA §5.6 R-SA-129/136; §8 R-SA-146)
//!
//! > JSONL ... logs MUST be append-only, MUST silently cap total bytes written per file at a
//! > fixed budget (target: 50MB) without erroring the run, and MUST NOT attempt to rewrite or
//! > truncate earlier lines.
//!
//! This is cyrup's own contract for the tee-style artifacts above; it is NOT upstream's
//! `events.jsonl` semantics (see the section before this one). Precisely:
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

/// pi `TRUNCATED_EVENT_TYPE` (`subagent-runner.ts:266` @v0.43.0, `:320` @v0.68.0): the one
/// marker line [`RunEventLog::write_diagnostic_line`] writes on the first diagnostic overflow.
pub const TRUNCATED_EVENT_TYPE: &str = "subagent.events.truncated";

/// pi `TRUNCATION_MARKER_RESERVE_BYTES` (`subagent-runner.ts:267` @v0.43.0, `:321` @v0.68.0): the
/// slice of the budget diagnostics may not use, so the truncation marker still fits after them.
pub const TRUNCATION_MARKER_RESERVE_BYTES: u64 = 512;

/// A background run's `events.jsonl`: lifecycle lines uncapped, child diagnostics capped with a
/// one-shot truncation marker — pi `appendJsonl` / `appendDiagnosticJsonl`
/// (`subagent-runner.ts:300-330` @v0.43.0, `:354-386` @v0.68.0). See the module doc for why this
/// is not a [`BoundedJsonlWriter`].
///
/// # Byte accounting across writers
///
/// Several of these can be open on one `events.jsonl` at once (the runner's step loop, its control
/// watcher, its telemetry pump). Upstream shares one in-process byte counter per path through a
/// module-level map; here the diagnostic budget is measured against the file's CURRENT length
/// (`fstat` on the append handle) at each diagnostic write, so every lifecycle line any writer
/// has appended counts against the diagnostic budget exactly as upstream's shared counter makes it
/// count. Lifecycle writes need no accounting: they are never refused.
///
/// The `diagnostics_truncated` latch is per handle. Only one handle — the telemetry pump's —
/// writes diagnostics, so "the marker is written once" holds per run.
pub struct RunEventLog {
    file: tokio::fs::File,
    cap_bytes: u64,
    diagnostics_truncated: bool,
}

impl RunEventLog {
    /// Open (creating if absent) `path` for append with the default 50 MiB diagnostic budget.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` if `path` cannot be opened/created in append mode.
    pub async fn create(path: &Path) -> io::Result<Self> {
        Self::create_with_cap(path, DEFAULT_JSONL_CAP_BYTES).await
    }

    /// Open (creating if absent) `path` for append; `cap_bytes` bounds the file only as far as
    /// DIAGNOSTIC lines are concerned (pi `maxAsyncEventsBytes()`).
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` if `path` cannot be opened/created in append mode.
    pub async fn create_with_cap(path: &Path, cap_bytes: u64) -> io::Result<Self> {
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(Self {
            file,
            cap_bytes,
            diagnostics_truncated: false,
        })
    }

    async fn append(&mut self, line: &str) -> io::Result<()> {
        // One `write_all` for line + newline, so two handles appending to the same file cannot
        // interleave a line with another's newline.
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        self.file.write_all(&bytes).await?;
        self.file.flush().await
    }

    /// Append one LIFECYCLE line (pi `appendJsonl`). Never capped: the run's own trail —
    /// `subagent.run.*`, `subagent.step.*`, steering, process-terminal — survives any
    /// `CYRUP_SUBAGENT_ASYNC_EVENTS_MAX_BYTES`, `0` included.
    ///
    /// # Errors
    ///
    /// A genuine I/O failure. Callers treat the log as best-effort and ignore it.
    pub async fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.append(line).await
    }

    /// Append one child DIAGNOSTIC line (pi `appendDiagnosticJsonl`): written only while it fits
    /// in `cap - 512`; the first line that does not fit is replaced by ONE
    /// `{type:"subagent.events.truncated", ts, maxBytes, droppedEventType}` marker (written only if
    /// the marker itself fits under the full cap), after which every diagnostic line is dropped.
    /// A blank line is ignored, as upstream's `if (!line.trim()) return`.
    ///
    /// # Errors
    ///
    /// A genuine I/O failure (on the size probe or the write). Never for the cap itself.
    pub async fn write_diagnostic_line(
        &mut self,
        line: &str,
        dropped_event_type: Option<&str>,
    ) -> io::Result<()> {
        if line.trim().is_empty() || self.diagnostics_truncated {
            return Ok(());
        }
        let current = self.file.metadata().await?.len();
        let chunk = line.len() as u64 + 1;
        let diagnostic_budget = self
            .cap_bytes
            .saturating_sub(TRUNCATION_MARKER_RESERVE_BYTES);
        if current.saturating_add(chunk) <= diagnostic_budget {
            return self.append(line).await;
        }
        let mut marker = serde_json::Map::new();
        marker.insert("type".into(), TRUNCATED_EVENT_TYPE.into());
        marker.insert("ts".into(), crate::time::now_epoch_millis().into());
        marker.insert("maxBytes".into(), self.cap_bytes.into());
        // pi `droppedEventType` is `undefined` for a typeless event, which `JSON.stringify` omits.
        if let Some(kind) = dropped_event_type {
            marker.insert("droppedEventType".into(), kind.into());
        }
        let marker = serde_json::Value::Object(marker).to_string();
        self.diagnostics_truncated = true;
        if current.saturating_add(marker.len() as u64 + 1) <= self.cap_bytes {
            self.append(&marker).await?;
        }
        Ok(())
    }

    /// `true` once a diagnostic line has overflowed (and the marker was written, if it fit).
    #[must_use]
    pub fn diagnostics_truncated(&self) -> bool {
        self.diagnostics_truncated
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
    /// Lifecycle lines are never capped: a zero cap drops nothing from the run's own trail.
    /// Kills: routing `RunEventLog::write_line` through the diagnostic budget (the pre-fix
    /// behaviour, where `CYRUP_SUBAGENT_ASYNC_EVENTS_MAX_BYTES=0` erased the whole trail).
    #[tokio::test]
    async fn a_zero_cap_still_journals_lifecycle_events() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");
        let mut log = RunEventLog::create_with_cap(&path, 0).await.expect("opens");
        for kind in [
            "subagent.run.started",
            "subagent.step.started",
            "subagent.run.completed",
        ] {
            log.write_line(&format!(r#"{{"type":"{kind}"}}"#))
                .await
                .expect("lifecycle writes succeed");
        }
        let text = std::fs::read_to_string(&path).expect("readable");
        let kinds: Vec<String> = text
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).expect("json")["type"]
                    .as_str()
                    .expect("type")
                    .to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "subagent.run.started",
                "subagent.step.started",
                "subagent.run.completed"
            ]
        );
    }

    /// The first diagnostic line that does not fit in `cap - 512` is replaced by exactly ONE
    /// marker with upstream's keys, the marker fits inside the reserve, and every diagnostic after
    /// it is dropped — while lifecycle lines keep landing. Kills: dropping the reserve (the
    /// diagnostics fill the whole cap and the marker no longer fits), writing the marker on every
    /// overflow (latch removed), and never writing the marker (the pre-fix silent cap).
    #[tokio::test]
    async fn the_first_overflowing_diagnostic_line_writes_one_marker_and_stops() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");
        // Diagnostic budget: 1024 - 512 = 512 bytes. Each diagnostic line is 100 bytes + newline.
        let cap = 1024u64;
        let mut log = RunEventLog::create_with_cap(&path, cap)
            .await
            .expect("opens");
        let diag = |n: usize| {
            let head = format!(r#"{{"type":"tool_execution_end","n":"{n:02}","pad":""#);
            let pad = "x".repeat(100 - head.len() - 2);
            format!("{head}{pad}\"}}")
        };
        assert_eq!(diag(0).len(), 100);
        for n in 0..20 {
            log.write_diagnostic_line(&diag(n), Some("tool_execution_end"))
                .await
                .expect("never errors for the cap");
        }
        assert!(log.diagnostics_truncated());
        log.write_line(r#"{"type":"subagent.run.completed"}"#)
            .await
            .expect("lifecycle still lands");

        let text = std::fs::read_to_string(&path).expect("readable");
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).expect("every line is JSON"))
            .collect();
        let diagnostics = lines
            .iter()
            .filter(|v| v["type"] == "tool_execution_end")
            .count();
        assert_eq!(diagnostics, 5, "5 * 101 = 505 <= 512 < 606");
        let markers: Vec<&serde_json::Value> = lines
            .iter()
            .filter(|v| v["type"] == TRUNCATED_EVENT_TYPE)
            .collect();
        assert_eq!(markers.len(), 1, "exactly one marker: {text}");
        let marker = markers[0].as_object().expect("object");
        let mut keys: Vec<&str> = marker.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["droppedEventType", "maxBytes", "ts", "type"]);
        assert_eq!(marker["maxBytes"], cap);
        assert_eq!(marker["droppedEventType"], "tool_execution_end");
        assert_eq!(
            lines.last().expect("lines")["type"],
            "subagent.run.completed",
            "the lifecycle line after the overflow is not dropped"
        );
        let marker_line = text
            .lines()
            .find(|l| l.contains(TRUNCATED_EVENT_TYPE))
            .expect("marker line");
        assert!(
            (marker_line.len() as u64) < TRUNCATION_MARKER_RESERVE_BYTES,
            "the marker fits in the reserve"
        );
    }

    /// Lifecycle bytes already in the file — written through ANOTHER handle — count against the
    /// diagnostic budget, as upstream's shared per-path counter makes them count. Kills: measuring
    /// the budget against only this handle's own diagnostic bytes (the 11-byte line would then fit
    /// in 0 + 11 <= 188 and be written).
    #[tokio::test]
    async fn another_writers_lifecycle_bytes_count_against_the_diagnostic_budget() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");
        // Diagnostic budget: 700 - 512 = 188 bytes.
        let mut lifecycle = RunEventLog::create_with_cap(&path, 700)
            .await
            .expect("opens");
        let mut diagnostics = RunEventLog::create_with_cap(&path, 700)
            .await
            .expect("opens");
        let head = r#"{"type":"subagent.run.started","p":""#;
        let line = format!("{head}{}\"}}", "y".repeat(179 - head.len() - 2));
        assert_eq!(line.len(), 179);
        lifecycle.write_line(&line).await.expect("lifecycle");
        // 180 already on disk + 11 = 191 > 188: this diagnostic overflows.
        diagnostics
            .write_diagnostic_line(r#"{"n":1234}"#, Some("a"))
            .await
            .expect("diag");
        let text = std::fs::read_to_string(&path).expect("readable");
        assert_eq!(
            text.lines().count(),
            2,
            "the lifecycle line and the marker: {text}"
        );
        assert!(
            text.lines()
                .nth(1)
                .unwrap_or_default()
                .contains(TRUNCATED_EVENT_TYPE)
        );
    }

    #[test]
    fn default_cap_is_fifty_megabytes() {
        assert_eq!(DEFAULT_JSONL_CAP_BYTES, 50 * 1024 * 1024);
    }
}
