---
stage: aug
status: done
updated: 2026-09-07 02:14
---

# SCOPE_4 — durable completion replay

OBJECTIVE: port
[`completion-replay.ts`](../../../workspace/pi-subagents/src/runs/background/completion-replay.ts)
(287 LOC, pi HEAD `df26ebc8`) so a `wait` that arrives **after** a completion has been delivered
and its payload unlinked still resolves. cyrup today has only the in-memory `CompletionBus`, so a
`wait` racing a completed-and-deleted run hangs until its 30-minute timeout.

Closes `SUBA-056` (`docs/gap-analysis/PARITY-GAPS.md:34`, `:1193`).

---

## §0 — PRECONDITION: WORKFLOW_4 must land first. Verified, not assumed.

```bash
# Must print the module. If it prints NOT PRESENT, STOP — execute WORKFLOW_4 first.
ls -d crates/cyrup-ext-subagents/src/background/wait_completions/ 2>/dev/null || echo "NOT PRESENT"
# Must print the two seam comments WORKFLOW_4 leaves for this task:
grep -rn "SCOPE_4" crates/cyrup-ext-subagents/src/background/wait_completions/
```

**Verified at augmentation time (cyrup HEAD `4ff02210`): `wait_completions/` does NOT exist.**
[`WORKFLOW_4.md`](WORKFLOW_4.md) is `stage: aug, status: done` — augmented, not executed. SCOPE_4
attaches to three named seams WORKFLOW_4 creates and **cannot be executed before it**:

| seam | file WORKFLOW_4 creates | what SCOPE_4 does to it |
|---|---|---|
| `write_completion_replay` in the recorder | `wait_completions/record.rs` | adds the `persistence` parameter WORKFLOW_4 deliberately omitted |
| `read_completion_replay` in the ENOENT arm | `wait_completions/collect.rs` | replaces the comment with the third rung |
| `WaitCompletion.archive_path` | `wait_completions/project.rs` | becomes populated (was `None` forever) |

`WaitCompletion` is WORKFLOW_4's type. Every `WaitCompletion` in this file refers to it.

---

## §1 — Verified anchors

Every line number below was read on both sides during augmentation. Nothing here is inferred.

### Upstream (pi HEAD `df26ebc8`)

| what | where |
|---|---|
| the module being ported | [`completion-replay.ts`](../../../workspace/pi-subagents/src/runs/background/completion-replay.ts) `:1-287` |
| the ORDERING evidence + its verbatim comment | [`result-watcher.ts:428-435`](../../../workspace/pi-subagents/src/runs/background/result-watcher.ts) |
| the write side (`persistence` arg) | [`wait-completions.ts:130,137-146`](../../../workspace/pi-subagents/src/runs/background/wait-completions.ts) |
| the READ side (the ENOENT third rung) | [`wait-completions.ts:200-208`](../../../workspace/pi-subagents/src/runs/background/wait-completions.ts) |
| a SECOND reader (not in scope; do not port) | [`wait-subscriptions.ts:251`](../../../workspace/pi-subagents/src/runs/background/wait-subscriptions.ts) |
| a THIRD reader — the only `readCompletionArchive` consumer | [`inspect-rpc.ts:236-265`](../../../workspace/pi-subagents/src/runs/background/inspect-rpc.ts) |
| the replay dir as a retention guard (`"replay-reference"`) | [`async-retention.ts:505`](../../../workspace/pi-subagents/src/runs/background/async-retention.ts) |
| `utf8Tail` / `decodeUtf8Tail` | [`shared/utf8.ts:1-11`](../../../workspace/pi-subagents/src/shared/utf8.ts) |
| the retention SCHEDULE this task must create | [`extension/index.ts:440-447`](../../../workspace/pi-subagents/src/extension/index.ts) |

### cyrup (HEAD `4ff02210`)

| what | where |
|---|---|
| the `Deliver` arm to modify | [`watch/install.rs:117-161`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/install.rs) |
| `DeliveryDisposition::Deliver` ⇒ session is `Some` | [`delivery/disposition.rs:68-77`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/delivery/disposition.rs) |
| the shared atomic-write primitive | [`background/atomic.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/atomic.rs) |
| `write_private_atomic_json_blocking` (pi's `writePrivateAtomicJson`) | [`atomic.rs:129-186`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/atomic.rs) |
| `encode_uri_component` — PRIVATE, must become `pub(crate)` | [`identity/path_segment.rs:128-149`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/identity/path_segment.rs) |
| the UTF-8-boundary tail walk to REUSE | [`exec/child_protocol.rs:134-178`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/child_protocol.rs) |
| `DEDUP_TTL` (10 min) — the `ttlMs` at the call site | [`watch/results_watcher.rs:39`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/results_watcher.rs) |
| retention precedent (facade + errno + age policy) | [`result_index/retention.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/result_index/retention.rs) |
| errno predicates to reuse, never re-roll | [`result_index/errno.rs:22-68`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/result_index/errno.rs) |
| facade house style (layout diagram, zero logic) | [`result_index/mod.rs:1-60`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/result_index/mod.rs) |
| where the schedule goes | [`extension/executor/notices.rs:394-450`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs) |

---

## §2 — THE ON-DISK FORMAT. Get this wrong and pi cannot read cyrup's replay dir.

`safeRunFile` is **bare `encodeURIComponent(runId) + ".json"`** (`:39-41`). It is **NOT**
`IndexSegment::encode`.

```ts
function safeRunFile(runId: string): string { return `${encodeURIComponent(runId)}.json`; }
```

This is not an oversight upstream: [`async-retention.ts:505`](../../../workspace/pi-subagents/src/runs/background/async-retention.ts)
independently rebuilds the same name inline —
`path.join(input.resultsDir, "completion-replay", \`${encodeURIComponent(runId)}.json\`)` — so two
upstream files agree the replay dir uses the raw encoder.

**Do not reach for [`IndexSegment`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/identity/path_segment.rs).**
`IndexSegment::encode_bounded` adds a `~sha256-…` fallback for over-long and non-portable segments
(`path_segment.rs:58-67`). A run id that triggers either rule would land at a **different file
name** than pi writes, so pi could not find cyrup's record and vice versa — and this record is
explicitly a shared, versioned on-disk format (`REPLAY_VERSION`, `ARCHIVE_VERSION`).

### The one shared-primitive edit this forces

`encode_uri_component` (`path_segment.rs:138`) is a private `fn` in `identity/path_segment.rs`.
**Widen it to `pub(crate)` and add its inverse there** — do not copy the encoder into the new
module. That file's own doc already states why it is one implementation: *"this is an on-disk
format better pinned by the unit tests below than by a third party's set definition."*

```rust
// identity/path_segment.rs — change `fn encode_uri_component` to:
/// JavaScript `encodeURIComponent`.
/// …existing doc…
///
/// `pub(crate)` because [`crate::background::completion_replay`] addresses its files with the RAW
/// encoder rather than through [`IndexSegment`] — pi `completion-replay.ts:39-41` and
/// `async-retention.ts:505` both do, and the `~sha256-` fallback [`IndexSegment::encode_bounded`]
/// applies would put a long run id's record at a name pi cannot find.
pub(crate) fn encode_uri_component(value: &str) -> String { /* unchanged */ }

/// JavaScript `decodeURIComponent`, returning `None` where JS throws `URIError`.
///
/// The inverse of [`encode_uri_component`], needed by
/// [`crate::background::completion_replay`]'s directory sweep to recover a run id from a file
/// name (pi `runIdFromReplayFile`, `completion-replay.ts:149-157`). Two rejection cases, both of
/// which upstream converts to `undefined` via its `try`/`catch`:
///
/// * a malformed escape (`%`, `%A`, `%ZZ`) — JS throws `URIError`;
/// * an escape sequence that decodes to invalid UTF-8 — JS also throws `URIError`, and Rust
///   cannot construct the `String` either. Decoding into `Vec<u8>` and validating once at the end
///   is what makes a multi-byte sequence spread across several `%XX` escapes decode correctly.
pub(crate) fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'%') => {
                let hi = bytes.get(i + 1).copied().and_then(hex_nibble)?;
                let lo = bytes.get(i + 2).copied().and_then(hex_nibble)?;
                out.push(hi * 16 + lo);
                i += 3;
            }
            Some(&byte) => {
                out.push(byte);
                i += 1;
            }
            None => break,
        }
    }
    String::from_utf8(out).ok()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
```

Re-export both from [`identity/mod.rs:55-58`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/identity/mod.rs)
as `pub(crate) use path_segment::{decode_uri_component, encode_uri_component};`.

---

## SUBTASK1 — `background/completion_replay/`

**Where:** new module `crates/cyrup-ext-subagents/src/background/completion_replay/`
**Ports:** [`completion-replay.ts`](../../../workspace/pi-subagents/src/runs/background/completion-replay.ts) in full.

Register in [`background/mod.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/mod.rs)
beside `pub mod result_index;` (`:46`) — it is a peer subsystem, not a leaf helper.

### Layout (house style: facade with the layout diagram and zero logic)

```text
completion_replay/
  mod.rs        the facade: constants, the narrative, `pub use` — NO logic
  record.rs     CompletionReplayRecord + its tolerant parser + validate_replay_record
  archive.rs    CompletionArchive/Entry + write_completion_archive (the 64 KiB truncation)
  store.rs      write_completion_replay / read_completion_replay / read_completion_archive
  retention.rs  cleanup_completion_replay{,_if_due} + the per-resultsDir throttle
```

Four jobs, four files, exactly as the task's original sketch said and as
[`result_index/`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/result_index/)
is arranged. `mod.rs` carries the layout diagram and the "why this exists" narrative; the constants
live there because they are the format.

### 1.1 `mod.rs` — the constants are the format. Port verbatim.

```rust
//! Durable completion replay — the bounded on-disk record that makes a `wait` arriving AFTER
//! delivery still resolve.
//!
//! Ports pi `runs/background/completion-replay.ts` (287 LOC).
//!
//! # The race this closes
//!
//! `deliver_pending_completions` ([`crate::background::watch`]) UNLINKS a payload once it has been
//! delivered (R-SA-099's delete-last, `watch/install.rs:157`). A `wait` that resolves a moment
//! later has no file to read. WORKFLOW_4's in-process [`crate::background::wait_completions`] store
//! covers that window for the lifetime of ONE process; it does not survive a restart, and it is
//! pruned at `DEDUP_TTL`. This module is the durable tier underneath it — and it is written
//! BEFORE the unlink, which is the whole correctness argument (see [`write_completion_replay`]).
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
//! The record is small and fixed-shape; the archive holds up to
//! [`ARCHIVE_TEXT_LIMIT_BYTES`] of fallback text. Keeping them apart means the hot read
//! ([`read_completion_replay`], once per `wait` miss) never pays for 64 KiB it usually does not
//! need — the record carries the archive's PATH and a caller fetches it only when it wants the
//! text ([`read_completion_archive`]).

/// pi `REPLAY_VERSION` (`completion-replay.ts:7`). An on-disk format version: a record with any
/// other value is SKIPPED, never repaired and never a hard error.
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
```

Path builders (`:39-49`) go in `mod.rs` next to the constants they compose:

```rust
/// pi `safeRunFile` (`:39-41`) — `<encodeURIComponent(runId)>.json`.
///
/// The RAW encoder, not [`crate::identity::IndexSegment`]: `IndexSegment::encode_bounded`'s
/// `~sha256-` fallback (`identity/path_segment.rs:58-67`) would put a long or non-portable run
/// id's record at a name pi does not look at, and `async-retention.ts:505` independently rebuilds
/// this exact name inline. Two upstream files agree on the raw encoder, so it is the format.
fn replay_file_name(run_id: &RunId) -> String {
    format!("{}{}", crate::identity::encode_uri_component(run_id.as_str()), ResultFileName::EXTENSION)
}

/// pi `completionReplayPath` (`:45-47`).
#[must_use]
pub fn completion_replay_path(results_dir: &Path, run_id: &RunId) -> PathBuf {
    results_dir.join(REPLAY_DIR_NAME).join(replay_file_name(run_id))
}

/// pi `completionArchivePath` (`:49-51`).
#[must_use]
pub fn completion_archive_path(results_dir: &Path, run_id: &RunId) -> PathBuf {
    results_dir.join(ARCHIVE_DIR_NAME).join(replay_file_name(run_id))
}
```

### 1.2 `record.rs` — `session_id` is REQUIRED, and the version gate is a type

Upstream's `sessionId` is `string`, not `string | undefined` (`:34`), and the writer's only caller
has already passed `result-watcher.ts:408`'s session-presence gate. Model it as `SessionId`, never
`Option<SessionId>`: an unattributable replay record must be **unconstructable**, not merely
unwritten. cyrup's [`DeliveryDisposition::classify`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/delivery/disposition.rs)
gives the same guarantee structurally — `Deliver` is only produced after `result.session_id` was
destructured out of an `Option` (`:69-74`) — so the required field costs the call site nothing.

```rust
/// pi `CompletionReplayRecord` (`completion-replay.ts:31-39`).
///
/// # `session_id` is required, and that is load-bearing
///
/// Upstream types it `string` (`:34`) and `parseReplay` rejects a record without one (`:133`).
/// The read gate at `:228` compares it against the caller's session, so a record with no session
/// could never be read back by anyone — it would be pure garbage occupying the run's one replay
/// slot. `SessionId` (not `Option<SessionId>`) makes that state unconstructable.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionReplayRecord {
    /// pi `:32`. [`ReplayVersion`] rejects anything but [`REPLAY_VERSION`] at DEserialization,
    /// which is what makes `parse_replay` skip a future version instead of misreading it.
    pub version: ReplayVersion,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub completed_at: i64,
    pub expires_at: i64,
    pub completion: crate::background::wait_completions::WaitCompletion,
    pub archive_path: PathBuf,
}

/// The literal `1` of [`REPLAY_VERSION`], as a type.
///
/// pi's version check is `record.version !== REPLAY_VERSION → undefined` (`:131`). Expressing it
/// as a `Deserialize` that rejects every other value moves the check to the boundary (§A.1 rule 1)
/// and means `parse_replay` is `serde_json::from_slice(..).ok()` — one fallible constructor rather
/// than a hand-written field-by-field validator that a future field could be forgotten from.
/// A `u32` field plus a runtime `if` is the shape that rots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayVersion;
// Serialize as `1`; Deserialize accepts only `1` (`Err` otherwise). Same for `ArchiveVersion`.
```

Two functions, both pure:

```rust
/// pi `parseReplay` (`:128-139`) composed with `parseCompletion` (`:120-126`).
///
/// **Tolerant by contract** (SCOPE_3.md §A.5): this reads bytes written by a DIFFERENT build.
/// Absent, malformed, wrong-version and wrong-run records all become `None` — never an `Err`, and
/// never a panic. The `completion.runId != run_id` check is pi `:123` and is not decoration: it
/// is what stops a record moved (or copied) between file names from replaying another run's
/// output under this run's id.
pub(super) fn parse_replay(bytes: &[u8], run_id: &RunId) -> Option<CompletionReplayRecord>;

/// pi `validateReplayRecord` (`:141-147`) — the containment check.
///
/// The stored `archivePath` must CANONICALLY equal [`completion_archive_path`] for this
/// `results_dir`/`run_id`. A record is otherwise a redirect: a hand-edited or relocated record
/// could name any file on disk and [`read_completion_archive`] would read it. Upstream compares
/// `path.resolve(...)`; cyrup uses [`std::path::Path::canonicalize`] where both sides exist and
/// falls back to a component-normalized compare when the archive is already gone (the expired
/// case, where the record is about to be deleted anyway).
///
/// On success the path is REWRITTEN to the canonical one and mirrored onto
/// `completion.archive_path` (pi `:145`), so a caller never sees the stored value.
pub(super) fn validate_replay_record(
    results_dir: &Path,
    run_id: &RunId,
    record: CompletionReplayRecord,
) -> Option<CompletionReplayRecord>;
```

### 1.3 `archive.rs` — the 64 KiB truncation, and the `utf8_tail` it needs

`CompletionArchiveEntry.source` is three mutually exclusive outcomes, so it is an enum with an
exhaustive `match`, never a `String` (§A.1 rule 2):

```rust
/// pi `CompletionArchiveEntry["source"]` (`completion-replay.ts:18`).
///
/// The three are ordered by fidelity, and [`write_completion_archive`] tries them in exactly that
/// order (`:73-104`): a saved artifact is the real output; a session transcript can reconstruct
/// it; a bounded text tail is the last resort and the only one that can be truncated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveSource {
    /// `"output-artifact"` — `child.artifactPaths.outputPath`, verified to be an existing FILE.
    OutputArtifact,
    /// `"session"` — `child.sessionFile` (or the run-level one), verified likewise.
    Session,
    /// `"result-tail"` — bounded inline text; the only variant that carries `truncated`.
    ResultTail,
}

/// pi `CompletionArchiveEntry` (`:15-22`). Every optional field is omit-when-absent so a pi
/// consumer reads a byte-identical object.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionArchiveEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")] pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub result_index: Option<usize>,
    pub source: ArchiveSource,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub text: Option<String>,
    /// `Some(true)` ONLY. pi writes `...(bounded.truncated ? { truncated: true } : {})` (`:100`),
    /// i.e. the key is absent rather than `false` — `skip_serializing_if` reproduces that.
    #[serde(default, skip_serializing_if = "Option::is_none")] pub truncated: Option<bool>,
}
```

`write_completion_archive` (`:73-113`) — port the decision ladder exactly. It is `async` because
it writes:

```rust
/// pi `writeCompletionArchive` (`:73-113`). Returns the archive's path.
///
/// # The ladder, per child (`:75-102`)
///
/// 1. `artifactPaths.outputPath` **that is an existing file** → reference it, no text copied.
/// 2. `sessionFile` **that is an existing file** → reference it.
/// 3. `output`/`error` present → retain a bounded TAIL, `Error: <error>` first then `output`,
///    joined by `\n` (`:96`), truncated to [`ARCHIVE_TEXT_LIMIT_BYTES`].
///
/// The `existsSync`+`isFile` guard (`:54-62`) is not paranoia: an entry pointing at a path that
/// is gone (or is a directory) would make a later read fail rather than degrade, and the whole
/// point of the archive is to be the thing that still works.
///
/// # The two run-level fallbacks
///
/// * `results.length === 0` → the RUN's own `sessionFile`, if it is a file (`:105-108`).
/// * still no entries → the run `summary`, bounded (`:109-112`). `ResultFile` has no `summary`
///   field, so cyrup reads `data["summary"]` off the raw value: this projector's input is the
///   same untyped `serde_json::Value` WORKFLOW_4 projects from, and a payload written by another
///   build may well carry one.
///
/// An archive with zero entries is still written (`:113`) — the record needs a companion file at
/// the canonical path or [`validate_replay_record`] would reject its own writer's output.
pub async fn write_completion_archive(
    results_dir: &Path,
    run_id: &RunId,
    data: &serde_json::Value,
    created_at: i64,
) -> std::io::Result<PathBuf>;
```

### 1.4 `store.rs` — write / read / read-archive

```rust
/// pi `writeCompletionReplay` (`:186-210`). Persist a terminal completion BEFORE its one-shot
/// result file is removed.
///
/// Returns the record whose `completion` now carries `archive_path` — the caller stores THAT copy
/// (pi `:139-146` assigns `writeCompletionReplay(...).completion` back over the projection), so
/// the in-process record and the durable record agree on the archive location.
///
/// # Ordering, and why the archive is written first
///
/// The archive lands before the record (`:195` then `:206`). A record is only ever valid if its
/// archive exists — [`validate_replay_record`] enforces exactly that — so writing the record
/// first would create a window in which a concurrent reader sees a record pointing at nothing and
/// deletes it as invalid. Archive, then record, is the same "never publish a reference before its
/// referent" discipline `result_index`'s stage-index-promote protocol follows
/// (`result_index/mod.rs`'s write protocol).
///
/// The last act is the throttled sweep (`:207`) — upstream drives retention from the WRITE path,
/// not from a timer, and the throttle is what makes that free.
pub async fn write_completion_replay(
    input: &CompletionReplayWrite<'_>,
) -> std::io::Result<CompletionReplayRecord>;

/// The named argument object of [`write_completion_replay`] (pi's inline `input:` type, `:186-194`).
/// A struct rather than seven positional parameters because `results_dir`/`archive` paths and
/// `now`/`ttl_ms` are each pairwise transposable at a call site.
pub struct CompletionReplayWrite<'a> {
    pub results_dir: &'a Path,
    pub run_id: &'a RunId,
    pub session_id: &'a SessionId,
    pub completion: &'a WaitCompletion,
    /// The RAW payload the archive projects from — the same untyped value WORKFLOW_4 projects.
    pub data: &'a serde_json::Value,
    pub now: i64,
    /// [`crate::background::watch::DEDUP_TTL`]. Do NOT introduce a second constant.
    pub ttl_ms: i64,
}
```

```rust
/// pi `readCompletionReplay` (`:212-235`). The current record for `run_id`, or `None`.
///
/// # The four `None`s, and the two that DELETE
///
/// | case | pi | deletes? |
/// |---|---|---|
/// | file absent / unparseable / unknown version | `:216-226` | no |
/// | [`validate_replay_record`] rejected it | `:224-227` | **yes** — record only |
/// | `session_id` filter supplied and does not match | `:228` | no |
/// | `expires_at <= now` | `:229-233` | **yes** — record AND archive |
///
/// The two non-deleting rows are not an oversight. A foreign-session record belongs to another
/// session and this caller has no standing to reap it; an unparseable one is swept by
/// [`cleanup_completion_replay`]'s age policy, which is the only place that can distinguish
/// "written by a newer build" from "corrupt".
///
/// # `session_id` is an OPTIONAL filter, not a STRICT one (`:228`)
///
/// `None` means "no filter" and returns the record whatever session wrote it — upstream's
/// `options.sessionId !== undefined` guard, verbatim. This is deliberate: `inspect-rpc.ts:246`
/// passes a session, `wait-completions.ts:203` passes `run.sessionId` which may itself be
/// `undefined`, and collapsing the two would make an unattributed run's wait unable to read its
/// OWN record. Do not "tighten" this into a required parameter.
pub async fn read_completion_replay(
    results_dir: &Path,
    run_id: &RunId,
    filter: ReplayReadFilter<'_>,
) -> Option<CompletionReplayRecord>;

/// The two read options (pi's `{ sessionId?, now? }`, `:212`), as one type so `None`/`None` at a
/// call site cannot be transposed.
pub struct ReplayReadFilter<'a> {
    /// `None` applies NO filter — see [`read_completion_replay`]'s OPTIONAL-not-STRICT note.
    pub session_id: Option<&'a SessionId>,
    /// `None` = [`crate::time::now_epoch_millis`] (pi's `options.now ?? Date.now()`).
    pub now: Option<i64>,
}

/// pi `readCompletionArchive` (`:237-246`).
///
/// # This has no in-crate consumer yet, and that is expected
///
/// Upstream's only caller is `inspect-rpc.ts:250`, which cyrup has not ported. It is in this
/// task's surface because the archive is half the on-disk format: a writer with no reader cannot
/// be validated, and the tests below are its first consumer. Mark it `pub` and cite the upstream
/// caller so a later dead-code sweep does not remove the reader half of a format.
///
/// Upstream distinguishes "absent" (`undefined`) from "malformed" (throws, `:241`). cyrup returns
/// `Result<Option<CompletionArchive>, SubagentError>` so both survive: `Ok(None)` for absent,
/// `Err` for malformed. Collapsing them would let a corrupt archive read as an empty one.
pub async fn read_completion_archive(
    archive_path: &Path,
) -> Result<Option<CompletionArchive>, SubagentError>;
```

### 1.5 `retention.rs` — the throttle is process-global state; make that explicit

```rust
/// pi `lastCleanupByResultsDir` (`completion-replay.ts:13`) — a module-level `Map`.
///
/// Process-global by design: the throttle exists to stop N concurrent completions in one results
/// dir from each walking the directory, so it must be shared across every caller in the process.
/// Keyed by `results_dir` because that is the unit being swept; two cwds throttle independently.
///
/// `std::sync::Mutex` with poison recovery — this crate's convention for a short non-`await`
/// critical section (`runner_main/status.rs:74-78`).
static LAST_CLEANUP_BY_RESULTS_DIR: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, i64>>,
> = std::sync::LazyLock::new(Default::default);

/// pi `cleanupCompletionReplayIfDue` (`:248-255`). `true` if the sweep RAN.
///
/// # `interval_ms` is a throttle, not a timer
///
/// The function returns `false` — cheaply, without touching the filesystem — when called again
/// inside the interval. That is what makes it safe to call on every completion write (`:207`) AND
/// from a schedule: whichever fires first does the work and the other is a map lookup. Nothing
/// here starts a timer or spawns a task.
///
/// The map is updated BEFORE the sweep (`:252`), not after, so a long sweep cannot admit a second
/// concurrent one.
pub async fn cleanup_completion_replay_if_due(
    results_dir: &Path,
    now: i64,
    max_age_ms: i64,
    interval_ms: i64,
) -> bool;

/// pi `cleanupCompletionReplay` (`:257-287`). Best-effort; never surfaces an error.
///
/// # The replay dir's four-way per-file decision (`:262-276`)
///
/// 1. the file name does not round-trip through [`replay_file_name`] → SKIP entirely (`:150-156`);
/// 2. parsed but [`validate_replay_record`] rejected → remove, no age check (`:269`);
/// 3. valid and `expires_at <= now` → remove the record AND its archive (`:270-273`);
/// 4. UNparseable and `now - mtime > max_age_ms` → remove (`:274`);
/// 5. otherwise keep.
///
/// Row 4 is the one that must not be simplified to row 2. An unparseable file may be a record
/// written by a NEWER build whose version this one does not know; deleting it on sight would make
/// two cyrup versions sharing a directory destroy each other's records. The age check is the
/// concession that lets them coexist.
///
/// # The archive dir (`:278-286`) sweeps on mtime ALONE
///
/// Every archive older than `max_age_ms` goes, with no record consulted. That is safe only
/// because both writes happen in the same call with the same `ttl_ms` (`:195`,`:200`,`:207`), so a
/// record's `expires_at` and its archive's age-out coincide. If a caller ever passes a
/// `max_age_ms` SHORTER than the `ttl_ms` a record was written with, it would strand a live
/// record pointing at a deleted archive — [`read_completion_archive`] returns `Ok(None)` there
/// and the completion still replays without its text, which is the intended degradation.
///
/// One bad entry must never abort the sweep (`:275`,`:283`): every per-file operation is
/// individually swallowed, using [`crate::background::result_index::errno`]'s named predicates
/// rather than a fresh `matches!` on `ErrorKind`.
pub async fn cleanup_completion_replay(results_dir: &Path, now: i64, max_age_ms: i64);
```

---

## SUBTASK1b — the two shared-primitive additions

Both are additions to EXISTING modules. Neither may be re-implemented inside `completion_replay/`.

### (a) `background/atomic.rs` — an ASYNC private (`0600`) writer

pi persists both files through `writePrivateAtomicJson` (`completion-replay.ts:111`,`:206`), whose
cyrup analog is `write_private_atomic_json_blocking` (`atomic.rs:129-186`) — correct mode, correct
mkdir, but **blocking**. This module's writer runs inside the watcher's async drain loop
(`watch/install.rs`), where a blocking `std::fs::rename` retry loop would stall the reactor for up
to ~0.6 s under contention (`MAX_RENAME_ATTEMPTS` × `backoff_delay`).

Add a third **entry point onto the same implementation** — permitted and in fact required by that
module's own contract (*"one shared primitive, not two"* names the temp-path derivation and the
rename backoff, which this reuses verbatim):

```rust
/// The ASYNC, owner-only (`0600`) sibling of [`write_atomic_json`] — pi `writePrivateAtomicJson`
/// (`shared/atomic-json.ts:62`) for callers already inside an async context.
///
/// Identical semantics to [`write_private_atomic_json_blocking`]: creates the parent, chmods the
/// temp file to `0600` BEFORE the rename so the private mode is in effect the instant the file is
/// visible, and reuses [`unique_temp_path`] + [`rename_with_backoff`]. It exists because
/// [`crate::background::completion_replay`] writes from the completion watcher's drain loop,
/// where the blocking variant's bounded-retry sleep would block the reactor thread.
///
/// `0600` is not cosmetic here: a replay record embeds a child's completion and an archive can
/// embed 64 KiB of its output. `results_dir` is a shared per-cwd temp directory.
///
/// # Errors
///
/// Serialization, `mkdir`, write, chmod, or a rename that exhausts the retry budget. The temp
/// file is best-effort removed on every error path.
pub async fn write_private_atomic_json<T: serde::Serialize + Sync>(
    path: &Path,
    value: &T,
) -> io::Result<()>;
```

### (b) `exec/child_protocol.rs` — `utf8_tail`, over the tail walk that already exists

pi's `utf8Tail` (`shared/utf8.ts:7-11`) is "last N bytes, then advance off leading UTF-8
continuation bytes, plus a `truncated` flag". The advance-off-continuation-bytes walk is **already
written** in [`BoundedByteTail::push`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/child_protocol.rs)
(`:157-167`, itself a port of `trimToUtf8Boundary`). Write the one-shot form as a thin call onto
it — do not hand-roll a second boundary walk:

```rust
/// pi `utf8Tail` (`shared/utf8.ts:7-11`) — the LAST `max_bytes` of `value`, trimmed to a UTF-8
/// boundary, with whether anything was cut.
///
/// Implemented over [`BoundedByteTail`] rather than as a second boundary walk: `BoundedByteTail`
/// is already this crate's port of upstream's `trimToUtf8Boundary` (`child-protocol.ts:370-375`),
/// and two copies of a boundary walk is exactly the duplication SCOPE_3.md §A.0 forbids. The
/// streaming type keeps a tail across many pushes; this is the same thing with one push.
///
/// `truncated` is decided on the INPUT's byte length, before any trimming — a value at exactly
/// `max_bytes` is NOT truncated (`utf8.ts:9`'s `<=`), and the boundary trim that follows may
/// remove a further 1-3 bytes without changing that answer.
#[must_use]
pub fn utf8_tail(value: &str, max_bytes: usize) -> BoundedText {
    if value.len() <= max_bytes {
        return BoundedText { text: value.to_string(), truncated: false };
    }
    let mut tail = BoundedByteTail::new(max_bytes);
    tail.push(value.as_bytes());
    BoundedText { text: tail.text(), truncated: true }
}

/// The `{ text, truncated }` pi returns (`shared/utf8.ts:7`). A named pair, not a tuple: the two
/// fields are a `String` and a `bool` and a tuple destructure has no compiler-checked order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedText { pub text: String, pub truncated: bool }
```

---

## SUBTASK2 — record the replay before dedupe and before the unlink

**Where:** WORKFLOW_4's `record_wait_completion` call in
[`watch/install.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/install.rs)'s
`DeliveryDisposition::Deliver` arm.

WORKFLOW_4 places the recorder at exactly the right line and deliberately omits the `persistence`
parameter (its SUBTASK4 step 3: *"an always-`None` parameter no caller can populate is worse than
adding it later with its consumer"*). **SCOPE_4 is that consumer.** Add the parameter now.

Upstream's ordering comment is verbatim law here — port it as a comment, do not paraphrase:

```rust
// pi `result-watcher.ts:428-435`, comment verbatim:
//   "Recorded before dedupe and before the unlink below so bg_wait can
//    use the in-memory record or its bounded durable replay after cleanup."
//
// Recording AFTER `sink.deliver` would lose precisely the race this exists to close: the window
// between `delete_after_notify` unlinking the payload (:157) and a `wait` asking about the run.
// The order is the behaviour; a test asserts it (`the_replay_record_is_written_before_the_
// payload_is_deleted`), because both placements compile and both "record the completion".
record_wait_completion(
    &store,
    &notification.result.run_id,
    &payload_value,
    crate::time::now_epoch_millis(),
    ttl_ms,
    // SCOPE_4: `Deliver` already destructured `session_id` out of its `Option`
    // (`delivery/disposition.rs:69-74`), so the required `SessionId` costs nothing here.
    Some(&ReplayPersistence { results_dir: watcher.results_dir(), session_id }),
)
.await;

let message = format_completion_message(&notification.result);
if sink.deliver(message).await {
    let _ = watcher.delete_after_notify(&notification).await;   // the unlink, AFTER the record
}
```

### The signature change on `record_wait_completion`, and why it becomes `async`

```rust
/// pi's `persistence?: { resultsDir, sessionId }` (`wait-completions.ts:130`).
///
/// `session_id` is a required [`SessionId`], not an `Option`: upstream's only call site
/// (`result-watcher.ts:431`) has already passed `:408`'s session-presence gate, and a record
/// without one is unreadable by construction (see [`CompletionReplayRecord`]).
pub struct ReplayPersistence<'a> {
    pub results_dir: &'a Path,
    pub session_id: &'a SessionId,
}

pub async fn record_wait_completion(
    store: &WaitCompletionStore,
    run_id: &RunId,
    data: &serde_json::Value,
    now: i64,
    ttl_ms: i64,
    persistence: Option<&ReplayPersistence<'_>>,
);
```

**Three rules for the body, all of them ordering:**

1. **Prune, project, PERSIST, then insert** — upstream's order (`:132-146`). The stored completion
   must be the archive-path-bearing copy `write_completion_replay` returns, not the pre-persist
   projection, or the in-process and durable tiers disagree about `archive_path`.
2. **A persistence failure is logged and swallowed** (`:141-143`: `console.error`, then the
   pre-persist `completion` is stored anyway). The in-memory record must still land — degrading to
   WORKFLOW_4's behaviour is strictly better than losing both tiers. `tracing::warn!`, then continue.
3. **The `std::sync::Mutex` guard must not be held across the `.await`.** Structure the body as
   prune-and-project (sync) → `write_completion_replay` (await) → lock-and-insert (sync).
   `clippy::await_holding_lock` is denied in this crate and will catch the wrong shape, but write
   it correctly rather than discovering it from the linter.

`ttl_ms` is [`DEDUP_TTL`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/watch/results_watcher.rs)
(10 min) at the call site, which WORKFLOW_4 already establishes. It flows through unchanged as both
the record's TTL and the sweep's `max_age_ms` (pi `:207` passes `input.ttlMs` for both).

---

## SUBTASK3 — the read fallback goes in `collect.rs`, **not** `wait.rs`

> **CORRECTION to this task's original text.** The original said *"**Where:** `background/wait.rs`
> — when the live scan finds no payload for a requested run, consult `read_completion_replay`."*
> That is the wrong file. `wait.rs`'s loop consults
> [`list_active_runs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/run_status.rs)
> for run STATE and never reads a payload at all — there is no "live scan finds no payload" site
> in it. Upstream's replay read is in `collectWaitCompletions`'
> **ENOENT arm** ([`wait-completions.ts:200-208`](../../../workspace/pi-subagents/src/runs/background/wait-completions.ts)),
> which WORKFLOW_4 ports to `wait_completions/collect.rs` and where it leaves the named seam.
> Putting the read in `wait.rs` would add a second, differently-gated reader of the same record.

**Where:** `wait_completions/collect.rs`, the `Err(e) if e.kind() == NotFound` arm.

**Change:** replace WORKFLOW_4's placeholder comment with the third rung. Upstream order in that arm
is fixed and all three rungs are load-bearing:

```rust
Err(e) if e.kind() == io::ErrorKind::NotFound => {
    // pi `:185-207`. The watcher consumed the file between the store check and this read.
    // Rung 2 — the in-process record, re-checked. NOT redundant with the check at the top of
    // the loop: the watcher runs on its own task and the window between them is real (`:195`).
    if let Some(late) = store.get(&run.run_id) {
        out.push(late);
        continue;
    }
    // Rung 3 (SCOPE_4) — the durable replay, written before the unlink so a `wait` landing
    // after delete-last, or in a LATER PROCESS entirely, still reports the completion (`:201`).
    //
    // `session_id` is passed through as-is, including `None`: it is an OPTIONAL filter
    // (`completion-replay.ts:228`), and an unattributed run must still read its own record.
    if let Some(replay) = read_completion_replay(
        results_dir,
        &run.run_id,
        ReplayReadFilter { session_id: run.session_id.as_ref(), now: None },
    ).await {
        out.push(replay.completion);
    }
    // Nothing at all: the payload is gone, the record expired. The run contributes no
    // completion — upstream's own outcome (`:203`'s `if (replay)` has no else).
}
```

**Do not** propagate a `read_completion_replay` miss as an error. Upstream wraps only a genuine
*throw* from the replay read (`:204-207`); an absent/expired/foreign record is `undefined` and the
loop moves on. `read_completion_replay` returns `Option`, which is that behaviour by construction.

---

## SUBTASK4 — the retention schedule. It does not exist; CREATE it.

> **CORRECTION to this task's original text.** The original said *"**Where:** wherever
> `cleanup_result_indexes` is scheduled at install time"*. **It is scheduled nowhere.** Verified:
>
> ```bash
> $ grep -rn "cleanup_result_indexes" --include='*.rs' crates/ | grep -v retention.rs
> crates/cyrup-ext-subagents/src/background/result_index/mod.rs:106:pub use retention::{DEFAULT_MAX_AGE_MS, cleanup_result_indexes};
> ```
>
> One re-export, zero call sites outside its own `#[cfg(test)]` module. cyrup's index sweep is
> **dead code today** — a second, pre-existing gap this task closes on the way past.

**Where:** [`extension/executor/notices.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs),
at the end of `SubagentExecutor::install_completion_watcher`, in the `Ok(handle)` arm after the
handle is stored (`:441-443`).

**Port:** [`extension/index.ts:440-447`](../../../workspace/pi-subagents/src/extension/index.ts) —
a 30-second `setTimeout`, `unref`'d, wrapped in `try`/`catch`:

```ts
const resultIndexCleanupTimer = setTimeout(() => {
    try { cleanupResultIndexes(DIRS.results); }
    catch (error) { console.error("Failed to clean stale subagent result indexes:", error); }
}, 30_000);
resultIndexCleanupTimer.unref?.();
```

```rust
/// pi's `resultIndexCleanupTimer` (`extension/index.ts:440-447`) — a one-shot, detached,
/// best-effort retention sweep 30 s after the watcher is installed.
///
/// # Why 30 s and detached, rather than at install or on an interval
///
/// Upstream's `unref` is the point: the sweep must never hold the process open and must never sit
/// in front of session start. `tokio::spawn` + `sleep` is the direct analog — a detached task the
/// runtime abandons at shutdown. It is deliberately NOT retained on `self`: nothing cancels it,
/// nothing awaits it, and a session that ends inside 30 s simply never sweeps, exactly as an
/// `unref`'d timer behaves.
///
/// # Two sweeps, one schedule
///
/// * [`cleanup_result_indexes`] at [`DEFAULT_MAX_AGE_MS`] (24 h) — pi `:442`. This is its FIRST
///   call site in cyrup; it was previously unreachable outside its own tests.
/// * [`cleanup_completion_replay_if_due`] (SCOPE_4). Upstream drives replay retention only from
///   the write path (`completion-replay.ts:207`) and cyrup keeps that too. The schedule is the
///   second driver because the write path only fires when a completion is DELIVERED: an
///   orchestrator that restarts after its runs completed elsewhere would otherwise never sweep
///   the records those runs left behind. The 60 s throttle makes the overlap free — whichever
///   driver fires first does the work and the other is one map lookup
///   ([`cleanup_completion_replay_if_due`] returns `false` without touching the filesystem).
///
/// `max_age_ms` for the replay sweep is [`crate::background::watch::DEDUP_TTL`], the same value
/// every record is written with, so `expires_at` and archive age-out coincide (see
/// [`cleanup_completion_replay`]).
const RETENTION_SWEEP_DELAY: Duration = Duration::from_secs(30);

drop(tokio::spawn(async move {
    tokio::time::sleep(RETENTION_SWEEP_DELAY).await;
    if let Err(error) = crate::background::result_index::cleanup_result_indexes(
        &results_dir, crate::time::now_epoch_millis(),
        crate::background::result_index::DEFAULT_MAX_AGE_MS,
    ).await {
        tracing::warn!("Failed to clean stale subagent result indexes: {error}");
    }
    crate::background::completion_replay::cleanup_completion_replay_if_due(
        &results_dir,
        crate::time::now_epoch_millis(),
        ttl_ms,
        crate::background::completion_replay::CLEANUP_INTERVAL_MS,
    ).await;
}));
```

`results_dir` is moved into `install_completion_watcher_with_observer` at `:405`; clone it before
that call for the task.

---

## SUBTASK5 — doc sweep D18

**Where:** [`background/wait.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/wait.rs)
module docs, and `wait_completions/mod.rs`'s narrative.

**Change:** `wait.rs`'s header currently documents two escape hatches and the SUBA-034 wake, and
says nothing about where a completion's DETAILS come from. Add a short section — and delete
WORKFLOW_4's forward reference to this task, which is now landed code:

```rust
//! # Resolving a completion after cleanup (SUBA-056)
//!
//! A resolved wait reports each terminal run's completion through
//! [`super::wait_completions::collect_wait_completions`], which resolves in three rungs: the
//! in-process record, the payload on disk, and — when the watcher has already delivered and
//! UNLINKED the payload — the durable replay record in
//! [`super::completion_replay`]. The record is written BEFORE the unlink
//! (pi `result-watcher.ts:428-435`), which is the entire reason a `wait` arriving after cleanup
//! resolves instead of reporting a completion it demonstrably observed as absent. It survives a
//! process restart, which the in-process record does not, and expires at
//! [`super::watch::DEDUP_TTL`].
```

Delete every remaining `SCOPE_4`/"until it lands"/"SCOPE_4 owns" placeholder:

```bash
grep -rn "SCOPE_4" crates/cyrup-ext-subagents/src/   # must be empty when this task is done
```

---

## Tests

Location: `#[cfg(test)] mod tests` inside the module that owns each behaviour — this crate does not
use a separate `tests/` tree for unit-level pins.

| test | in | fails before | pins |
|---|---|---|---|
| `a_wait_after_cleanup_resolves_from_the_replay_record` | `wait_completions/collect.rs` | **yes — the whole task** | the race: deliver, unlink, CLEAR the in-process store, then collect |
| `the_replay_record_is_written_before_the_payload_is_deleted` | `watch/install.rs` | **yes** | `result-watcher.ts:430`'s ordering, asserted from inside the sink |
| `reading_a_replay_with_a_foreign_session_returns_none` | `completion_replay/store.rs` | — | `:228`'s OPTIONAL gate |
| `reading_a_replay_with_no_session_filter_returns_it` | `completion_replay/store.rs` | — | OPTIONAL ≠ STRICT — the vacuous-pass guard on the row above |
| `an_expired_replay_record_is_not_returned` | `completion_replay/store.rs` | — | `expires_at`, and that the read DELETES record + archive (`:230-232`) |
| `an_archive_over_64_kib_is_truncated_and_flagged` | `completion_replay/archive.rs` | — | `ARCHIVE_TEXT_LIMIT_BYTES` + `truncated: Some(true)` + a valid UTF-8 boundary |
| `an_archive_prefers_an_existing_artifact_over_inline_text` | `completion_replay/archive.rs` | — | the `:75-102` ladder, including the `isFile` guard |
| `a_future_replay_version_is_ignored_not_a_panic` | `completion_replay/record.rs` | — | `REPLAY_VERSION`; `{"version":2,…}` reads as `None` and is NOT deleted |
| `a_replay_naming_a_foreign_archive_path_is_rejected_and_removed` | `completion_replay/record.rs` | — | `validateReplayRecord` (`:141-147`) — the containment property |
| `cleanup_is_throttled_to_the_interval` | `completion_replay/retention.rs` | — | `:248-251` returns `false` inside the interval |
| `cleanup_removes_records_past_max_age` | `completion_replay/retention.rs` | — | the four-way age policy, incl. that an UNparseable record inside the window survives |
| `the_replay_file_name_is_the_raw_uri_encoding` | `completion_replay/mod.rs` | — | §2 — a run id that `IndexSegment` would hash must still land at `%…json` |
| `the_retention_sweep_is_scheduled_after_the_watcher_installs` | `extension/executor/notices.rs` | **yes** | SUBTASK4 — `cleanup_result_indexes` currently has no call site at all |

### The two tests that carry the task, written against the real pipeline

`the_replay_record_is_written_before_the_payload_is_deleted` must assert ORDERING, not presence.
Presence passes with the record written after delivery. Use a sink that inspects the filesystem at
`deliver` time — the one moment strictly between "record" and "unlink":

```rust
/// The record must exist BEFORE `sink.deliver` is called, because `delete_after_notify` runs
/// immediately after `deliver` returns. A sink that checks from inside `deliver` is the only
/// observer positioned between the two, and it is what makes this test fail for the
/// record-after-delivery placement that "records the completion" just as truthfully.
#[async_trait::async_trait]
impl CompletionSink for OrderingSink {
    async fn deliver(&self, _message: CompletionMessage) -> bool {
        self.replay_existed_at_delivery.store(
            completion_replay_path(&self.results_dir, &self.run_id).exists(),
            Ordering::SeqCst,
        );
        true
    }
}
```

`a_wait_after_cleanup_resolves_from_the_replay_record` must **clear the in-process store** before
collecting. Without that it passes on WORKFLOW_4's `WaitCompletionStore` alone and proves nothing
about this task:

```rust
// 1. install the real watcher over a real results dir, publish a real terminal ResultFile;
// 2. wait for delivery AND for the payload to be unlinked (both, bounded);
// 3. drop the store — this is the "later process" / "TTL elapsed" condition, and without it the
//    in-process record answers and the durable tier is never exercised;
// 4. collect_wait_completions over a fresh, EMPTY store → the completion still comes back,
//    carrying the run id and the archive path.
```

---

## Benchmarks

None. The replay read happens once per `wait` MISS — a path that already lost a filesystem read —
and is bounded by one small JSON parse, far below the 500 ms `RESULTS_DIR_POLL_INTERVAL` floor the
wake itself pays.

---

## Definition of done

* A `wait` issued **after** a run completed, was delivered and had its payload unlinked still
  resolves that run's completion, from the durable record, with an **empty** in-process store.
* The replay record is written before delivery and before the unlink, and a test asserts the
  **ordering** from inside the sink — not merely that the record exists afterwards.
* A replay record carries a required `SessionId`; the type cannot express one without. A foreign
  session cannot read it; **`None` reads it** (OPTIONAL, not STRICT).
* A record whose `archive_path` is not the canonical archive path for its run is rejected and
  removed.
* Archives truncate at 64 KiB, set `truncated: true`, and never split a UTF-8 character.
* Replay files are named `<encodeURIComponent(runId)>.json` — verified for a run id that
  `IndexSegment` would hash.
* Cleanup is throttled to 60 s (returns `false` inside the interval, without filesystem I/O),
  removes expired and invalid records, and leaves an unparseable record inside the age window
  alone.
* `cleanup_result_indexes` has a real call site for the first time, 30 s after the completion
  watcher installs, detached and best-effort.
* `grep -rn "SCOPE_4" crates/cyrup-ext-subagents/src/` is **empty**.
* No second copy of: the URI encoder, the UTF-8 boundary walk, an errno predicate, a TTL constant,
  or the atomic-write temp/rename machinery.
* `SUBA-056` marked closed in `docs/gap-analysis/PARITY-GAPS.md` (`:34`, `:1193`).
* Workspace builds, 0 failed tests, `clippy` exit 0.

---

## Research notes

* Upstream: [`completion-replay.ts`](../../../workspace/pi-subagents/src/runs/background/completion-replay.ts),
  pi HEAD `df26ebc8`. cyrup baseline HEAD `4ff02210`.
* **`wait-subscriptions.ts` is a second consumer of `readCompletionReplay`**
  ([`:251`](../../../workspace/pi-subagents/src/runs/background/wait-subscriptions.ts)) and is
  **not in scope** — it is its own unported file (`SUBA` census line, `PARITY-GAPS.md:34`). Build
  `read_completion_replay` so that port needs no change to it: that is why `ReplayReadFilter`
  carries `now` (`wait-subscriptions.ts` passes an injected clock) even though this task's own
  call sites always pass `None`.
* **`inspect-rpc.ts` is the only `readCompletionArchive` consumer** and is likewise unported. Its
  `:236-244` comment documents a subtlety worth keeping in mind if it is ever ported: it reads the
  RAW record first, because `readCompletionReplay` best-effort DELETES invalid records and a read
  after it cannot distinguish "never existed" from "failed validation".
* **`async-retention.ts:505` treats an existing replay record as a reason not to reap an archive**
  (`"replay-reference"`). cyrup has no async-retention port, so nothing here can be reaped out from
  under a record today — but a future port must reproduce that guard, and this module's file name
  scheme (§2) is what makes the check expressible.
* Existing cleanup-scheduling precedent in cyrup: none. `cleanup_result_indexes` with
  `DEFAULT_MAX_AGE_MS` (24 h) exists and is unreferenced — SUBTASK4 is its first caller.
* pi's `writeCompletionReplay` swallows nothing (`:186-210` can throw); its CALLER
  (`wait-completions.ts:141-143`) is what logs and continues. Keep that split: the writer returns
  `io::Result`, the recorder degrades.

---

/home/d0m17bw/.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_4.md
