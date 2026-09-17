---
stage: qa
status: completed
updated: 2026-09-14 06:00
---

# SCOPE_4 — durable completion replay

OBJECTIVE: port
[`completion-replay.ts`](../../tmp/pi-subagents/src/runs/background/completion-replay.ts)
(287 LOC) so a `wait` that arrives **after** a completion has been delivered and its payload
unlinked still resolves. cyrup today has only the in-memory `CompletionBus` plus WORKFLOW_4's
in-process `WaitCompletionStore`, so a `wait` racing a completed-and-deleted run in a LATER PROCESS
(or after `DEDUP_TTL`) hangs until its 30-minute timeout.

Closes `SUBA-056` (`docs/gap-analysis/PARITY-GAPS.md:34`, `:1193`, `:1530`).

---

## §0 — AUG 2026-09-14: PATH MAP + HEADS. Read this before any other citation.

**Upstream is checked out IN THIS REPO.** Every `../../../workspace/pi-subagents/...` link in the
original text is dead in this tree. The real locations are:

| the old link said | the file actually is |
|---|---|
| `../../../workspace/pi-subagents/src/...` | `/home/user/cyrup/tmp/pi-subagents/src/...` |
| `../../../workspace/cyrup/crates/...` | `/home/user/cyrup/crates/...` |

**Upstream HEAD has moved: `47bae7f7`, not `df26ebc8`.** `completion-replay.ts` is still exactly
287 LOC and semantically unchanged, but several of its NEIGHBOUR files shifted by 3-4 lines.
`result-watcher.ts` in particular moved: the ordering comment this task calls "verbatim law" is now
at **`:432-433`**, not `:428-435`. Every upstream line number in this file has been re-read at
`47bae7f7` and corrected in place below.

**cyrup HEAD has moved a very long way from `4ff02210`.** See §0.5 — one prescribed mechanism
(SUBTASK2) no longer exists and has been replaced.

---

## §0.1 — PRECONDITION: WORKFLOW_4 HAS LANDED. Verified, not assumed.

```bash
$ ls -d crates/cyrup-ext-subagents/src/background/wait_completions/
crates/cyrup-ext-subagents/src/background/wait_completions/     # PRESENT

$ grep -rn "SCOPE_4" crates/cyrup-ext-subagents/src/
background/wait_completions/project.rs:47    "…producer — SCOPE_4's durable completion-replay writer — has not landed yet."
background/wait_completions/project.rs:190   "// Left unset until SCOPE_4's producer …"
background/wait_completions/record.rs:97     "// 3. A durable completion-replay seam for SCOPE_4: …"
background/wait_completions/record.rs:100    "…SCOPE_4 is what adds the"
background/wait_completions/collect.rs:105   "…until SCOPE_4 lands `read_completion_replay`"
```

**The original §0 said `wait_completions/` does NOT exist. That is now STALE — it exists, and this
task is unblocked.** The module is 675 LOC across four files:

```text
crates/cyrup-ext-subagents/src/background/wait_completions/
  mod.rs       45 LOC   facade + narrative
  project.rs  365 LOC   WaitCompletion, WaitCompletionChild, CompletionUsage, to_wait_completion
  record.rs   152 LOC   WaitCompletionStore + its CompletionObserver impl
  collect.rs  113 LOC   collect_wait_completions — the three-rung resolution
```

The three seams are all present, at these exact locations:

| seam | file:line NOW | what SCOPE_4 does to it |
|---|---|---|
| the recorder's missing `persistence` | `wait_completions/record.rs:97-101` (a comment between step 2 and step 4 of `record_wait_completion`) | adds the `persistence` parameter — **see SUBTASK2's correction: the shape is not what the original text assumed** |
| the ENOENT arm's missing third rung | `wait_completions/collect.rs:104-106` (a comment at the end of the `Err(error) if errno::is_absent(&error)` arm) | replaces the comment with the third rung |
| `WaitCompletion.archive_path` | `wait_completions/project.rs:41-48` (declaration) and `project.rs:190-192` (the `archive_path: None` in `to_wait_completion`) | becomes populated |

`WaitCompletion` is WORKFLOW_4's type (`project.rs:32`). Every `WaitCompletion` in this file refers
to it. **Its `archive_path` is `Option<String>`, NOT `Option<PathBuf>`** (`project.rs:48`), and its
`run_id` is `String`, not `RunId` (`project.rs:33`). Anything this task assigns to it must be
`Some(path.to_string_lossy().into_owned())`.

---

## §0.5 — AUG 2026-09-14: THE INSTALL PIPELINE WAS REWRITTEN. SUBTASK2's mechanism is gone.

The original spec anchored SUBTASK2 on `watch/install.rs:117-161`'s `DeliveryDisposition::Deliver`
arm, a `sink.deliver(message) -> bool`, and a `watcher.delete_after_notify(&notification)` on the
next line. **None of those identifiers exist any more.**

```bash
$ grep -rn "DeliveryDisposition" --include='*.rs' crates/
# (no output — the type is gone crate-wide)
$ ls crates/cyrup-ext-subagents/src/background/delivery/
custody.rs  gate.rs  mod.rs  ownership.rs  receipt.rs      # no disposition.rs
```

What replaced it (`background/watch/install.rs`, now **761 LOC**):

* **`Attribution`** (`background/delivery/custody.rs:34-46`), re-exported at `delivery/mod.rs:45`,
  with three arms — `Unattributed(ResultFile)` / `Foreign(ObservableCompletion)` /
  `Ours(OwnedCompletion)`.
* **`Attribution::classify`** (`custody.rs:55-70`) — **the `Deliver ⇒ session is Some` guarantee
  the original spec cited at `disposition.rs:68-77` SURVIVES**, now at `custody.rs:60-64`:
  ```rust
  let Some(session_id) = result.session_id.clone() else { return Self::Unattributed(result); };
  if owner.owns(&session_id, result.completion_owner_id.as_ref()) {
      Self::Ours(OwnedCompletion { result, payload })
  ```
  So on the `Ours` path `result.session_id` is `Some` by construction. The required-`SessionId`
  argument in §1.2 still costs the call site nothing — only the citation changes.
* **`deliver_pending_completions`** (`install.rs:302-455`) is now **two-phase** for the `Ours` arm
  (`install.rs:371-425`), because of `ASYNC_NOTIFY_BUG_REPORT` RC1:
  * **Phase 1 — synchronous observation** (`install.rs:375-388`): builds the
    `CompletionNotification` and `await`s `observer.observe(&notification)` INLINE in the scan loop.
  * **Phase 2 — concurrent delivery** (`install.rs:390-424`): spawns
    `sink.deliver(&run_id, message).await` onto a `DeliveryFleet` `JoinSet`.
* **The unlink moved out of this function entirely.** It now happens in `settle_delivery`
  (`install.rs:164-200`), at `install.rs:172` — `watcher.consume(payload, receipt).await` — reached
  only from `reap_finished_deliveries` (`install.rs:146-157`) once the delivery task joins.
* **`CompletionSink::deliver` changed signature** (`watch/sink.rs:31`):
  ```rust
  async fn deliver(&self, run_id: &RunId, message: CompletionMessage) -> CompletionDelivery;
  ```
  It returns `CompletionDelivery` (`Delivered(DeliveryReceipt)` / `Deferred`), **not `bool`**.

**Consequence, and it is good news for this task's correctness argument:** the record-before-unlink
ordering is now *structural*, not merely lexical. Observation runs synchronously, in the scan loop,
before the delivery task is even spawned; the unlink cannot happen until that task joins. The
ordering test in the Tests section still belongs, but it now pins an invariant the architecture
already enforces rather than a line ordering someone could reverse by accident.

---

## §1 — Verified anchors (RE-READ 2026-09-14)

### Upstream (pi HEAD `47bae7f7`, under `tmp/pi-subagents/`)

| what | where NOW | original said |
|---|---|---|
| the module being ported | `src/runs/background/completion-replay.ts:1-287` | ✔ unchanged |
| constants | `completion-replay.ts:7-12` | ✔ |
| `lastCleanupByResultsDir` | `completion-replay.ts:13` | ✔ |
| `CompletionArchiveEntry` / its `source` union | `:15-22` / `:18` | ✔ |
| `CompletionArchive` | `:24-29` | — |
| `CompletionReplayRecord` / its `sessionId: string` | `:31-39` / `:34` | ✔ |
| **`safeRunFile`** | **`:41-43`** | ✗ said `:39-41` |
| `completionReplayPath` / `completionArchivePath` | `:45-47` / `:49-51` | ✔ |
| **`existingFile` (the `statSync().isFile()` guard)** | **`:57-65`** | ✗ said `:54-62` |
| `outputArtifactPath` | `:67-70` | — |
| **`writeCompletionArchive`** | **`:73-119`** | ✗ said `:73-113` |
| **the per-child ladder** | **`:76-103`** | ✗ said `:75-102` |
| **the `results.length === 0` session fallback** | **`:104-107`** | ✗ said `:105-108` |
| **the `entries.length === 0` summary fallback** | **`:108-114`** | ✗ said `:109-112` |
| **the archive is written even when empty** | **`:115-118`** | ✗ said `:113` |
| `parseCompletion` / its `completion.runId !== runId` | `:121-126` / **`:124`** | ✗ said `:123` |
| `parseReplay` / its version gate / its `sessionId` gate | `:128-139` / `:131` / `:133` | ✔ |
| `validateReplayRecord` / the mirror onto `completion` | `:141-147` / `:145` | ✔ |
| `runIdFromReplayFile` | `:149-157` | ✔ |
| `removeBestEffort` | `:159-163` | — |
| `parseArchive` | `:165-183` | — |
| **`writeCompletionReplay`** | **`:186-209`** | ✗ said `:186-210` |
| archive-first / record-second / sweep-last | `:195` / `:206` / `:207` | ✔ |
| `readCompletionReplay` | `:212-235` | ✔ |
| its ENOENT arm / its `!parsed` arm | `:218` / `:221` | — |
| **the validate-and-remove arm** | **`:222-226`** | ✗ said `:224-227` |
| the OPTIONAL session filter | `:228` | ✔ |
| the expiry arm (removes record AND archive) | `:229-233` | ✔ |
| `readCompletionArchive` | `:237-246` | ✔ |
| **`cleanupCompletionReplayIfDue`** | **`:248-254`** | ✗ said `:248-255` |
| the throttle / the update-before-sweep | `:249-250` / `:251` | ✔ |
| `cleanupCompletionReplay` | `:257-287` | ✔ |
| **the replay-dir loop** | **`:260-276`**, four-way at **`:264-275`** | ✗ said `:262-276` |
| the archive-dir mtime-only sweep | `:278-286` | ✔ |
| **the ORDERING evidence + its verbatim comment** | **`result-watcher.ts:432-433`**, the call at **`:434-437`** | ✗ said `:428-435` |
| **the session-presence gate the recorder sits behind** | **`result-watcher.ts:411-412`** | ✗ said `:408` |
| **`ownsCompletion`** | **`result-watcher.ts:430-431`** | ✗ said `:428` |
| the write side (`persistence` arg) | `wait-completions.ts:130`, the block **`:137-150`** | ✗ said `:137-146` |
| **the `console.error`-and-continue catch** | **`wait-completions.ts:147-149`** | ✗ said `:141-143` |
| `collectWaitCompletions` | `wait-completions.ts:160-213` | ✔ |
| the READ side (the ENOENT third rung) | **`wait-completions.ts:188-210`**, the replay read at **`:202-209`**, the call at **`:203`** | ✗ said `:200-208` |
| a SECOND reader (not in scope; do not port) | `wait-subscriptions.ts:251` | ✔ exact |
| **a THIRD reader — the only `readCompletionArchive` consumer** | **`inspect-rpc.ts:233-268`**; the raw-first comment at **`:235-237`**; `readCompletionReplay` at **`:246`**; `readCompletionArchive` at **`:262`** | ✗ said `:236-265`, `:246`, `:250` |
| the replay dir as a retention guard (`"replay-reference"`) | `async-retention.ts:505` | ✔ exact |
| `utf8Tail` / `decodeUtf8Tail` | `shared/utf8.ts:1-11` | ✔ |
| **the retention SCHEDULE this task must create** | **`extension/index.ts:445-452`**; its teardown `clearTimeout` at **`:1044`** | ✗ said `:440-447` |

### cyrup (this tree, branch `claude/subagents-scope`)

| what | where NOW | original said |
|---|---|---|
| **the observation seam to modify** | **`background/wait_completions/record.rs:130-152`** (the `CompletionObserver` impl) — **NOT** an install.rs call site | ✗ said `watch/install.rs:117-161` |
| the drain loop's Phase-1 observe | `background/watch/install.rs:375-388` | (new) |
| the drain loop's Phase-2 delivery spawn | `background/watch/install.rs:390-424` | (new) |
| **the unlink (delete-last)** | **`background/watch/install.rs:164-200`, `consume` at `:172`** | ✗ said `install.rs:157` |
| **`Ours` ⇒ session is `Some`** | **`background/delivery/custody.rs:55-70`, the destructure at `:60-64`** | ✗ said `delivery/disposition.rs:68-77` |
| the shared atomic-write primitive | `background/atomic.rs` (438 LOC) | ✔ |
| **`write_private_atomic_json_blocking`** | **`background/atomic.rs:125-186`** (doc `:125-149`, `pub fn` at `:150`) | ≈ said `:129-186` |
| `write_atomic_json` (async, not private-mode, no mkdir) | `background/atomic.rs:75-100` | (new) |
| `write_atomic_json_creating_parent` (async + mkdir, `pub(crate)`) | `background/atomic.rs:102-121` | (new — a closer starting point than the blocking one) |
| `unique_temp_path` / `rename_with_backoff` / `backoff_delay` / `MAX_RENAME_ATTEMPTS` | `atomic.rs:226`/`:195`/`:218`/`:51` | (new) |
| **`encode_uri_component` — PRIVATE, must become `pub(crate)`** | **`identity/path_segment.rs:128-149`** (doc `:128-137`, `fn` at `:138`) | ✔ exact |
| `IndexSegment::encode_bounded` + its `~sha256-` fallback | `identity/path_segment.rs:58-67` | ✔ exact |
| `identity/mod.rs` re-export block | **`identity/mod.rs:60-64`** | ✗ said `:55-58` |
| the UTF-8-boundary tail walk to REUSE | **`exec/child_protocol.rs:134-182`**; `push` at `:151-168`; the continuation-byte advance at **`:156-166`** | ≈ said `:134-178` / `:157-167` |
| `DEDUP_TTL` (10 min) | `background/watch/results_watcher.rs:40` | ≈ said `:39` |
| retention precedent (facade + errno + age policy) | `background/result_index/retention.rs`; `cleanup_result_indexes` at `:43`; `DEFAULT_MAX_AGE_MS` at `:16` | ✔ |
| **errno predicates to reuse, never re-roll** | **`background/result_index/errno.rs`**: `is_unaddressable` `:22`, `is_absent` `:31`, `is_access_denied` `:47`, `is_ignorable_listing_error` `:57` — all `pub(crate) fn`; the module is `pub(crate) mod errno;` at `result_index/mod.rs:95` | ≈ said `:22-68` |
| facade house style | `background/result_index/mod.rs:1-88` (layout diagram at `:17-29`, file map at `:70-88`) | ≈ said `:1-60` |
| `result_index`'s retention re-export | **`background/result_index/mod.rs:118`** | ✗ said `:106` |
| `pub mod result_index;` in the facade | `background/mod.rs:46` | ✔ exact |
| `pub mod wait_completions;` | `background/mod.rs:73` | (new) |
| **where the schedule goes** | **`extension/executor/notices.rs:414-491`** (`install_completion_watcher`); `results_dir` bound at `:422`, moved into the call at `:429`; the `Ok(handle)` arm at **`:483-485`** | ✗ said `:394-450` / `:441-443` / `:405` |
| the composite observer registration | `extension/executor/notices.rs:446-465`; `self.wait_completions()` FIRST at `:453` | (new) |
| `WaitCompletionStore` construction | `extension/executor/mod.rs:268-270` (`::default()`, inside `SubagentExecutor::new`) | (new) |
| the store accessor | `extension/executor/mod.rs:392-397` (`fn wait_completions(&self) -> Arc<WaitCompletionStore>`) | (new) |
| `wait.rs` module docs, the stale-third-rung paragraph | **`background/wait.rs:56-77`**; the "does not exist yet" text at **`:74-77`**; a second stale ref at `:240` | ✗ said "module docs" only |
| `collect_wait_completions`'s call site | `background/wait.rs:1027-1032` (passes `&deps.results_dir`) | (new) |
| `crate::time::now_epoch_millis` | `time.rs:18` | ✔ |
| `ResultFileName::EXTENSION` (`".json"`) | `identity/result_name.rs:44` | ✔ |
| `SubagentError::Spawn(#[from] std::io::Error)` | `error.rs:238` | ✔ |
| `SessionId` — `Serialize` derived, `Deserialize` HAND-WRITTEN through `parse` | `identity/session_id.rs:39-41` / `:85-90` | (new; see §1.2) |
| `RunId` — `Serialize + Deserialize`, `#[serde(transparent)]` | `background/run_id.rs:26-30` | (new) |
| `tempfile` dev-dependency | `crates/cyrup-ext-subagents/Cargo.toml:181` | (new) |

---

## §2 — THE ON-DISK FORMAT. Get this wrong and pi cannot read cyrup's replay dir.

`safeRunFile` is **bare `encodeURIComponent(runId) + ".json"`** (**`:41-43`**, was cited `:39-41`).
It is **NOT** `IndexSegment::encode`.

```ts
function safeRunFile(runId: string): string {
	return `${encodeURIComponent(runId)}.json`;
}
```

This is not an oversight upstream: `async-retention.ts:505` independently rebuilds the same name
inline —
`fs.existsSync(path.join(input.resultsDir, "completion-replay", \`${encodeURIComponent(runId)}.json\`))`
— **verified verbatim at `47bae7f7`** — so two upstream files agree the replay dir uses the raw
encoder.

**Do not reach for [`IndexSegment`](../../crates/cyrup-ext-subagents/src/identity/path_segment.rs).**
`IndexSegment::encode_bounded` (`path_segment.rs:58-67`) adds a `~sha256-…` fallback for over-long
(`> 255` bytes, `path_segment.rs:40`) and non-portable segments. A run id that triggers either rule
would land at a **different file name** than pi writes, so pi could not find cyrup's record and vice
versa — and this record is explicitly a shared, versioned on-disk format (`REPLAY_VERSION`,
`ARCHIVE_VERSION`).

> **AUG note on how reachable that actually is.** `RunId::new` (`background/run_id.rs:39-42`) mints
> a 32-char hex UUIDv4 simple form, which encodes to itself and is neither long nor non-portable —
> so in production the two encoders AGREE. The divergence is reachable only through
> `RunId::from_token` (`run_id.rs:50-52`, which validates nothing) — a run id parsed back from a
> directory name or a CLI argument. That is exactly what the
> `the_replay_file_name_is_the_raw_uri_encoding` test must construct: a `from_token` id over 255
> encoded bytes, or one ending in a `.jsonl`-shaped extension (`has_trailing_extension`,
> `path_segment.rs:202`).

### The one shared-primitive edit this forces

`encode_uri_component` (`path_segment.rs:138`, doc from `:128`) is a private `fn` in
`identity/path_segment.rs`. **Widen it to `pub(crate)` and add its inverse there** — do not copy the
encoder into the new module. That file's own doc already states why it is one implementation:
*"this is an on-disk format better pinned by the unit tests below than by a third party's set
definition."* Its unreserved set is `A-Z a-z 0-9 - _ . ! ~ * ' ( )` with **uppercase** `%XX` hex
(`path_segment.rs:130-132`), which is what `decode_uri_component` must round-trip.

```rust
// identity/path_segment.rs — change `fn encode_uri_component` to:
/// JavaScript `encodeURIComponent`.
/// …existing doc…
///
/// `pub(crate)` because [`crate::background::completion_replay`] addresses its files with the RAW
/// encoder rather than through [`IndexSegment`] — pi `completion-replay.ts:41-43` and
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

Re-export both from **`identity/mod.rs:60-64`** (the existing `pub use` block; the original text
said `:55-58`, which is now the `mod` declarations) as:

```rust
pub(crate) use path_segment::{decode_uri_component, encode_uri_component};
```

> **AUG — the file-name round-trip is the real gate, not the decoder.** `runIdFromReplayFile`
> (`:149-157`) decodes and then **re-encodes and compares**: `safeRunFile(runId) === file`. That
> comparison is what rejects `%2f`-lowercase, over-encoded, and otherwise non-canonical names. Port
> the comparison, not just the decode — a decoder that is merely permissive is harmless only
> because of it. (`hex_nibble` above accepts lowercase hex on purpose; the round-trip is what
> rejects it.)

---

## SUBTASK1 — `background/completion_replay/`

**Where:** new module `crates/cyrup-ext-subagents/src/background/completion_replay/`
**Ports:** `tmp/pi-subagents/src/runs/background/completion-replay.ts` in full.

Register in `background/mod.rs` in the **first** `pub mod` block (`background/mod.rs:36-50`), which
is alphabetical — so `pub mod completion_replay;` goes between `child_stop` (`:40`) and `control`
(`:41`). It is a peer subsystem, not a leaf helper, exactly as `pub mod result_index;` (`:46`) is.
(The second block at `:68-73` holds the later-phase modules; either is defensible, but the first
block keeps the alphabetical run intact.)

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
`background/result_index/` is arranged (`result_index/mod.rs:70-88` carries that file map;
`wait_completions/mod.rs:20-26` carries the same shape at smaller scale). `mod.rs` carries the
layout diagram and the "why this exists" narrative; the constants live there because they are the
format.

### 1.1 `mod.rs` — the constants are the format. Port verbatim.

```rust
//! Durable completion replay — the bounded on-disk record that makes a `wait` arriving AFTER
//! delivery still resolve.
//!
//! Ports pi `runs/background/completion-replay.ts` (287 LOC).
//!
//! # The race this closes
//!
//! `deliver_pending_completions` ([`crate::background::watch`]) UNLINKS a payload once its
//! delivery receipt lands (R-SA-099's delete-last, `watch/install.rs:172`'s
//! `watcher.consume(payload, receipt)`). A `wait` that resolves a moment later has no file to
//! read. WORKFLOW_4's in-process [`crate::background::wait_completions::WaitCompletionStore`]
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

Path builders (**`:41-51`**, was cited `:39-49`) go in `mod.rs` next to the constants they compose:

```rust
/// pi `safeRunFile` (`:41-43`) — `<encodeURIComponent(runId)>.json`.
///
/// The RAW encoder, not [`crate::identity::IndexSegment`]: `IndexSegment::encode_bounded`'s
/// `~sha256-` fallback (`identity/path_segment.rs:58-67`) would put a long or non-portable run
/// id's record at a name pi does not look at, and `async-retention.ts:505` independently rebuilds
/// this exact name inline. Two upstream files agree on the raw encoder, so it is the format.
fn replay_file_name(run_id: &RunId) -> String {
    format!(
        "{}{}",
        crate::identity::encode_uri_component(run_id.as_str()),
        ResultFileName::EXTENSION,          // `identity/result_name.rs:44` — ".json"
    )
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
has already passed **`result-watcher.ts:411`**'s session-presence gate
(`if (typeof data.sessionId !== "string" || !data.sessionId) return;`). Model it as `SessionId`,
never `Option<SessionId>`: an unattributable replay record must be **unconstructable**, not merely
unwritten. cyrup's `Attribution::classify` (**`background/delivery/custody.rs:60-64`**) gives the
same guarantee structurally — `Ours` is only produced after `result.session_id` was destructured
out of an `Option` — so the required field costs the call site nothing.

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

> **AUG — every field type is already `Serialize + Deserialize`, verified:**
> * `RunId` — derived, `#[serde(transparent)]` (`background/run_id.rs:26-30`).
> * `SessionId` — `Serialize` derived `#[serde(transparent)]` (`identity/session_id.rs:39-41`);
>   `Deserialize` is **hand-written** and goes through `SessionId::parse`
>   (`identity/session_id.rs:85-90`), so `""` is an `Err`. This is STRICTER than pi, whose
>   `typeof record.sessionId !== "string"` accepts `""` at `:133`. The divergence is invisible
>   because `parse_replay` is `.ok()` — a record with an empty `sessionId` becomes `None` on both
>   sides, pi via the `:228` filter never matching, cyrup at deserialization.
> * `WaitCompletion` — `Clone, Debug, Default, PartialEq, Serialize, Deserialize`,
>   `rename_all = "camelCase"` (`wait_completions/project.rs:30-32`).
> * `PathBuf` — serializes as a string; a non-UTF-8 path FAILS to serialize on unix. `results_dir`
>   is derived from `artifact_roots` and is UTF-8 in practice; a serialization failure here is an
>   `io::Error` the recorder swallows (SUBTASK2 rule 2), so it degrades rather than panics.

Two functions, both pure:

```rust
/// pi `parseReplay` (`:128-139`) composed with `parseCompletion` (`:121-126`).
///
/// **Tolerant by contract** (SCOPE_3.md §A.5): this reads bytes written by a DIFFERENT build.
/// Absent, malformed and wrong-version records all become `None` — never an `Err`, and never a
/// panic.
pub(super) fn parse_replay(bytes: &[u8]) -> Option<CompletionReplayRecord>;

/// pi `validateReplayRecord` (`:141-147`) — the run-id check AND the containment check.
///
/// 1. `record.runId !== runId` (`:142`) — the caller's run id, which in the sweep comes from the
///    FILE NAME. This is what stops a record moved (or copied) between file names from replaying
///    another run's output under this run's id.
/// 2. The stored `archivePath` must CANONICALLY equal [`completion_archive_path`] for this
///    `results_dir`/`run_id` (`:143-144`). A record is otherwise a redirect: a hand-edited or
///    relocated record could name any file on disk and [`read_completion_archive`] would read it.
///    Upstream compares `path.resolve(...)`; cyrup uses [`std::path::Path::canonicalize`] where
///    both sides exist and falls back to a component-normalized compare when the archive is
///    already gone (the expired case, where the record is about to be deleted anyway).
///
/// On success the path is REWRITTEN to the canonical one and mirrored onto
/// `completion.archive_path` (pi `:145`), so a caller never sees the stored value. Note
/// `WaitCompletion::archive_path` is `Option<String>` (`wait_completions/project.rs:48`), so the
/// mirror is `Some(archive_path.to_string_lossy().into_owned())`.
pub(super) fn validate_replay_record(
    results_dir: &Path,
    run_id: &RunId,
    record: CompletionReplayRecord,
) -> Option<CompletionReplayRecord>;
```

> **AUG — CORRECTION, and it changes an observable behaviour.** The original text gave
> `parse_replay(bytes, run_id)` and attributed *"the `completion.runId != run_id` check"* to
> pi `:123`. Re-read at `47bae7f7`, upstream splits this in two and the split matters:
>
> * `parseReplay(value)` takes **no** caller run id (`:128`). It calls
>   `parseCompletion(record.completion, record.runId)` (`:137`), so the `:124` check compares the
>   embedded completion's `runId` against the **record's own** `runId` — an internal-consistency
>   check, not an identity check against the caller.
> * The caller/file-name run id is checked **only** in `validateReplayRecord` at **`:142`**.
>
> Collapsing them (passing the caller's run id into `parse_replay`) is NOT equivalent, because
> `cleanupCompletionReplay` (`:265-274`) branches on `record` vs `safeRecord` separately:
>
> | record | safeRecord | pi's action |
> |---|---|---|
> | `Some` | `None` | **remove immediately** (`:267-268`) — no age check |
> | `Some` | `Some`, expired | remove record + archive (`:269-271`) |
> | `None` | — | remove only if `mtime` older than `max_age_ms` (`:272-273`) |
>
> A record whose `runId` disagrees with its file name must land in row 1 (removed on sight). If
> `parse_replay` folded the run-id check in, it would return `None` and the file would land in
> row 3 — surviving inside the age window as if it were a future-version record. **Keep the
> two-function split exactly as upstream has it.** `parse_replay` takes bytes only.

### 1.3 `archive.rs` — the 64 KiB truncation, and the `utf8_tail` it needs

`CompletionArchiveEntry.source` is three mutually exclusive outcomes, so it is an enum with an
exhaustive `match`, never a `String` (§A.1 rule 2):

```rust
/// pi `CompletionArchiveEntry["source"]` (`completion-replay.ts:18`).
///
/// The three are ordered by fidelity, and [`write_completion_archive`] tries them in exactly that
/// order (`:76-103`): a saved artifact is the real output; a session transcript can reconstruct
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

`write_completion_archive` (**`:73-119`**) — port the decision ladder exactly. It is `async`
because it writes:

```rust
/// pi `writeCompletionArchive` (`:73-119`). Returns the archive's path.
///
/// # The ladder, per child (`:76-103`)
///
/// 1. `artifactPaths.outputPath` **that is an existing file** → reference it, no text copied.
/// 2. `sessionFile` **that is an existing file** → reference it.
/// 3. `output`/`error` present → retain a bounded TAIL, `Error: <error>` first then the output,
///    joined by `\n` (`:94`), truncated to [`ARCHIVE_TEXT_LIMIT_BYTES`].
///
/// The `existsSync`+`isFile` guard (`:57-65`) is not paranoia: an entry pointing at a path that
/// is gone (or is a directory) would make a later read fail rather than degrade, and the whole
/// point of the archive is to be the thing that still works.
///
/// # The two run-level fallbacks
///
/// * `results.length === 0` → the RUN's own `sessionFile`, if it is a file (`:104-107`).
///   [`crate::background::ResultFile::session_file`] (`background/records.rs:508`) is the cyrup
///   field; it serializes as `sessionFile`.
/// * still no entries → the run `summary`, bounded (`:108-114`). `ResultFile` has no `summary`
///   field, so cyrup reads `data["summary"]` off the raw value: this projector's input is the
///   same untyped `serde_json::Value` WORKFLOW_4 projects from, and a payload written by another
///   build may well carry one.
///
/// An archive with zero entries is still written (`:115-118`) — the record needs a companion file
/// at the canonical path or [`validate_replay_record`] would reject its own writer's output.
pub async fn write_completion_archive(
    results_dir: &Path,
    run_id: &RunId,
    data: &serde_json::Value,
    created_at: i64,
) -> std::io::Result<PathBuf>;
```

> **AUG — CRITICAL: rung 3's key does not exist. Read this before writing the ladder.**
>
> Upstream `:91` is `const output = nonEmptyString(child.output);`. **Neither pi's `SingleResult`
> nor cyrup's has an `output` field.** Verified:
> * pi `shared/types.ts:1243-1330` — `SingleResult` declares `error?`, `sessionFile?`,
>   `artifactPaths?`, and **`finalOutput?`** (`shared/types.ts:1300`). There is no `output`.
> * cyrup `exec/run_result.rs:25-…` — `#[serde(rename_all = "camelCase")]`, `final_output:
>   Option<String>` (`:50`) → wire key **`finalOutput`**; `error: Option<String>` (`:160`);
>   `session_file: Option<PathBuf>` (`:187`) → `sessionFile`; `artifact_paths:
>   Option<ArtifactPaths>` (`:234`) → `artifactPaths`, whose `output_path` (`artifacts.rs:68`)
>   serializes as `outputPath` under `rename_all = "camelCase"` (`artifacts.rs:61`).
> * Likewise `:109`'s `data.summary`: no `summary` on pi's async result payload and none on cyrup's
>   `ResultFile` (`background/records.rs:483-528`).
>
> So **upstream's rung 3 fires on `error` alone**, and its run-level `summary` fallback is dead.
> Porting `child.output` verbatim would give cyrup a `result-tail` rung that never carries a
> successful child's answer — which is precisely the text the archive exists to preserve.
>
> **Do both, in this order, and say so in the doc:**
> ```rust
> // pi `:91` reads `child.output`, a key `SingleResult` does not declare (pi
> // `shared/types.ts:1243-1330`, cyrup `exec/run_result.rs`) — so upstream's tail rung fires on
> // `error` alone. Read the key that actually carries a child's delivered text FIRST, and keep
> // upstream's spelling as a fallback so a foreign payload that does carry `output` still works.
> let output = non_empty(child.get("finalOutput")).or_else(|| non_empty(child.get("output")));
> ```
> This ADDS to the requirement; it does not replace it. Both keys are read, the join order
> (`Error: <error>` then output, `\n`-separated, `:94`) is unchanged, and a pi consumer reading
> cyrup's archive sees the same `{source:"result-tail", text, truncated?}` shape.
>
> `non_empty` here is the same predicate `wait_completions/project.rs` already uses for pi's
> `nonEmptyString` (`completion-replay.ts:53-55`) — reuse it rather than re-rolling a third copy.
>
> Note also `:76-78`: a `results` element that is not a non-array object is `continue`d, and
> `:75`'s `Array.isArray(data.results) ? data.results : []` means a missing/non-array `results` is
> the empty list, which then triggers the `:104` run-level branch.

### 1.4 `store.rs` — write / read / read-archive

```rust
/// pi `writeCompletionReplay` (`:186-209`). Persist a terminal completion BEFORE its one-shot
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
/// (`result_index/mod.rs:44-55`'s write protocol).
///
/// The last act is the throttled sweep (`:207`) — upstream drives retention from the WRITE path,
/// not from a timer, and the throttle is what makes that free. Note `:207` passes `input.ttlMs`
/// as `maxAgeMs` and omits `intervalMs`, taking the `CLEANUP_INTERVAL_MS` default at `:248`.
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
    /// [`crate::background::watch::DEDUP_TTL`] (`watch/results_watcher.rs:40`, 10 min) as millis.
    /// Do NOT introduce a second constant.
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
/// | file absent / unparseable / unknown version | `:216-221` | no |
/// | [`validate_replay_record`] rejected it | `:222-226` | **yes** — record only |
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
///
/// # Errno policy
///
/// pi returns `undefined` for `ENOENT` and RETHROWS anything else (`:218-219`). cyrup returns
/// `Option`, so a non-absent read fault degrades to `None` too — use
/// [`crate::background::result_index::errno::is_absent`] (`result_index/errno.rs:31`) for the
/// absent test rather than a fresh `matches!` on `ErrorKind`, and `tracing::warn!` the rest.
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
    /// `None` = [`crate::time::now_epoch_millis`] (`time.rs:18`) — pi's `options.now ?? Date.now()`.
    pub now: Option<i64>,
}

/// pi `readCompletionArchive` (`:237-246`), whose tolerant parser is `parseArchive` (`:165-183`).
///
/// # This has no in-crate consumer yet, and that is expected
///
/// Upstream's only caller is `inspect-rpc.ts:262`, which cyrup has not ported. It is in this
/// task's surface because the archive is half the on-disk format: a writer with no reader cannot
/// be validated, and the tests below are its first consumer. Mark it `pub` and cite the upstream
/// caller so a later dead-code sweep does not remove the reader half of a format.
///
/// Upstream distinguishes "absent" (`undefined`, `:243`) from "malformed" (throws, `:240`). cyrup
/// returns `Result<Option<CompletionArchive>, SubagentError>` so both survive: `Ok(None)` for
/// absent, `Err` for malformed. Collapsing them would let a corrupt archive read as an empty one.
/// [`crate::error::SubagentError::Spawn`] (`error.rs:238`) is the `#[from] std::io::Error` arm
/// `collect.rs` already wraps parse failures into.
pub async fn read_completion_archive(
    archive_path: &Path,
) -> Result<Option<CompletionArchive>, SubagentError>;
```

> **AUG — `parseArchive` (`:165-183`) is per-entry tolerant and must be ported that way.** It
> `flatMap`s entries, DROPPING any entry that is not an object or whose `source` is not one of the
> three literals (`:170-172`), and coercing each optional field independently (`:174-179`), with
> `resultIndex` additionally required to be a safe non-negative integer (`:175`). In serde terms:
> deserialize `entries` as `Vec<serde_json::Value>` and `filter_map` each through a
> `serde_json::from_value::<CompletionArchiveEntry>(..).ok()`, rather than letting one bad entry
> fail the whole `Vec<CompletionArchiveEntry>`. A plain derived `Deserialize` on the vector is
> STRICTER than upstream and would turn a partially-corrupt archive into an `Err`.

### 1.5 `retention.rs` — the throttle is process-global state; make that explicit

```rust
/// pi `lastCleanupByResultsDir` (`completion-replay.ts:13`) — a module-level `Map`.
///
/// Process-global by design: the throttle exists to stop N concurrent completions in one results
/// dir from each walking the directory, so it must be shared across every caller in the process.
/// Keyed by `results_dir` because that is the unit being swept; two cwds throttle independently.
///
/// `std::sync::Mutex` with poison recovery — this crate's convention for a short non-`await`
/// critical section (`runner_main/status.rs`'s `lock_status`; `wait_completions/record.rs:52-53`
/// is the nearest in-crate example: `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)`,
/// never `.unwrap()`, because the crate denies `clippy::unwrap_used` at `lib.rs:19-24`).
static LAST_CLEANUP_BY_RESULTS_DIR: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, i64>>,
> = std::sync::LazyLock::new(Default::default);

/// pi `cleanupCompletionReplayIfDue` (`:248-254`). `true` if the sweep RAN.
///
/// # `interval_ms` is a throttle, not a timer
///
/// The function returns `false` — cheaply, without touching the filesystem — when called again
/// inside the interval. That is what makes it safe to call on every completion write (`:207`) AND
/// from a schedule: whichever fires first does the work and the other is a map lookup. Nothing
/// here starts a timer or spawns a task.
///
/// The map is updated BEFORE the sweep (`:251`), not after, so a long sweep cannot admit a second
/// concurrent one. The guard MUST be dropped before the `.await` on the sweep — take the decision
/// and the insert in one lock acquisition, release, then sweep.
pub async fn cleanup_completion_replay_if_due(
    results_dir: &Path,
    now: i64,
    max_age_ms: i64,
    interval_ms: i64,
) -> bool;

/// pi `cleanupCompletionReplay` (`:257-287`). Best-effort; never surfaces an error.
///
/// # The replay dir's per-file decision (`:260-276`)
///
/// 1. the file name does not round-trip through [`replay_file_name`] → SKIP entirely (`:149-157`,
///    `:261-262`);
/// 2. parsed but [`validate_replay_record`] rejected → remove, no age check (`:267-268`);
/// 3. valid and `expires_at <= now` → remove the record AND its archive (`:269-271`);
/// 4. UNparseable and `now - mtime > max_age_ms` → remove (`:272-273`);
/// 5. otherwise keep.
///
/// Row 4 is the one that must not be simplified to row 2. An unparseable file may be a record
/// written by a NEWER build whose version this one does not know; deleting it on sight would make
/// two cyrup versions sharing a directory destroy each other's records. The age check is the
/// concession that lets them coexist. (This is also why `parse_replay` must NOT take the caller's
/// run id — see §1.2's correction: folding that check in moves row 2's inputs into row 4.)
///
/// # The archive dir (`:278-286`) sweeps on mtime ALONE
///
/// Every archive older than `max_age_ms` goes, with no record consulted. That is safe only
/// because both writes happen in the same call with the same `ttl_ms` (`:195`,`:202`,`:207`), so a
/// record's `expires_at` and its archive's age-out coincide. If a caller ever passes a
/// `max_age_ms` SHORTER than the `ttl_ms` a record was written with, it would strand a live
/// record pointing at a deleted archive — [`read_completion_archive`] returns `Ok(None)` there
/// and the completion still replays without its text, which is the intended degradation.
///
/// One bad entry must never abort the sweep (`:275`,`:284`) and a missing directory is not an
/// error (`:277`,`:286`): every per-file operation is individually swallowed, using
/// [`crate::background::result_index::errno`]'s named predicates
/// (`is_absent` `errno.rs:31`, `is_ignorable_listing_error` `errno.rs:57`) rather than a fresh
/// `matches!` on `ErrorKind`.
pub async fn cleanup_completion_replay(results_dir: &Path, now: i64, max_age_ms: i64);
```

---

## SUBTASK1b — the two shared-primitive additions

Both are additions to EXISTING modules. Neither may be re-implemented inside `completion_replay/`.

### (a) `background/atomic.rs` — an ASYNC private (`0600`) writer

pi persists both files through `writePrivateAtomicJson` (`completion-replay.ts:117`,`:206`), whose
cyrup analog is `write_private_atomic_json_blocking` (**`atomic.rs:125-186`**, `pub fn` at `:150`)
— correct mode, correct mkdir, but **blocking**. This module's writer runs inside the watcher's
async observe path (`watch/install.rs:375-388`, which is INLINE in the scan loop — see §0.5), where
a blocking `std::fs::rename` retry loop would stall the reactor for up to ~0.6 s under contention
(`MAX_RENAME_ATTEMPTS` = 5, `atomic.rs:51`, × `backoff_delay` capped at `RETRY_MAX_DELAY` = 200 ms,
`atomic.rs:59`,`:218-224`).

Add a third **entry point onto the same implementation** — permitted and in fact required by that
module's own contract (*"one shared primitive, not two"*, `atomic.rs:1-4`, which names the temp-path
derivation and the rename backoff, both of which this reuses verbatim):

```rust
/// The ASYNC, owner-only (`0600`) sibling of [`write_atomic_json`] — pi `writePrivateAtomicJson`
/// (`shared/atomic-json.ts:62`) for callers already inside an async context.
///
/// Identical semantics to [`write_private_atomic_json_blocking`]: creates the parent, chmods the
/// temp file to `0600` BEFORE the rename so the private mode is in effect the instant the file is
/// visible, and reuses [`unique_temp_path`] + [`rename_with_backoff`]. It exists because
/// [`crate::background::completion_replay`] writes from the completion watcher's synchronous
/// observe phase, where the blocking variant's bounded-retry sleep would block the reactor thread.
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

> **AUG — the closest existing template is `write_atomic_json_creating_parent`**
> (`atomic.rs:102-121`, `pub(crate) async`), which is already "async + `create_dir_all` + delegate".
> The new function is that plus the unix `chmod 0600` block copied from
> `atomic.rs:170-180`, and it must NOT delegate to `write_atomic_json` (which would rename before
> the chmod) — it needs its own write → chmod → `rename_with_backoff` sequence, using
> `tokio::fs::set_permissions` with `std::fs::Permissions::from_mode(0o600)` under
> `#[cfg(unix)]`. Mark `pub`, not `pub(crate)`, only if a test outside `background` needs it;
> `pub(crate)` matches the sibling and is sufficient.

### (b) `exec/child_protocol.rs` — `utf8_tail`, over the tail walk that already exists

pi's `utf8Tail` (`shared/utf8.ts:7-11`) is "last N bytes, then advance off leading UTF-8
continuation bytes, plus a `truncated` flag". The advance-off-continuation-bytes walk is **already
written** in `BoundedByteTail::push` (`exec/child_protocol.rs:151-168`; the continuation-byte
advance is `:156-166`, itself a port of `trimToUtf8Boundary`). Write the one-shot form as a thin
call onto it — do not hand-roll a second boundary walk:

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

> **AUG — two verified details, both fine, one worth a comment.**
> 1. `value.len()` on a Rust `&str` IS the UTF-8 byte length, so it is exactly
>    `Buffer.from(value, "utf-8").length` (`utf8.ts:8`). ✔
> 2. `BoundedByteTail::new` **clamps `max_bytes` to at least 1** (`exec/child_protocol.rs:145`,
>    `max_bytes.max(1)`). So `utf8_tail(x, 0)` returns one byte's worth of tail where pi returns
>    `""`. Unreachable for this task's only caller ([`ARCHIVE_TEXT_LIMIT_BYTES`] = 65536), but say
>    so in the doc rather than leaving a silent divergence: *"`max_bytes == 0` yields a 1-byte tail
>    where upstream yields `""` — `BoundedByteTail`'s positive-integer precondition
>    (`child_protocol.rs:145`); no caller passes 0."*
> 3. `BoundedByteTail::text` is `String::from_utf8_lossy` (`child_protocol.rs:171-174`), matching
>    `Buffer.toString("utf8")`'s replacement-char behaviour (`utf8.ts:4`). ✔

---

## SUBTASK2 — record the replay before dedupe and before the unlink

> ### AUG 2026-09-14 — **THE PRESCRIBED MECHANISM NO LONGER EXISTS.** Read §0.5 first.
>
> The original text placed the change at a `record_wait_completion(...)` call in
> `watch/install.rs`'s `DeliveryDisposition::Deliver` arm, followed by
> `if sink.deliver(message).await { watcher.delete_after_notify(&notification).await }`.
> **There is no such call site.** WORKFLOW_4 did not put the recorder in `install.rs` at all; it
> wired `WaitCompletionStore` onto the `CompletionObserver` seam. `DeliveryDisposition`,
> `delete_after_notify`, and the `bool`-returning `sink.deliver` are all gone.
>
> **The requirement is unchanged and still fully in scope**: the replay record must be written
> before dedupe and before the unlink, the `persistence` parameter must be added, and
> `ReplayPersistence` must carry a required `SessionId`. Only the *location and plumbing* change.
> Everything below records the mechanism that CAN work, beside the original.

### Where it actually goes

`crates/cyrup-ext-subagents/src/background/wait_completions/record.rs`. Three pieces:

**(1) `record_wait_completion` — `record.rs:68-111`.** Today:

```rust
fn record_wait_completion(
    store: &WaitCompletionStore,
    run_id: &RunId,
    data: &serde_json::Value,
    now_ms: i64,
    ttl_ms: i64,
) {
    let mut entries = store.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    entries.retain(|_, entry| now_ms.saturating_sub(entry.seen_at) <= ttl_ms);   // :82  prune
    let completion = match to_wait_completion(data, run_id) { … };               // :89  project
    // :97-101  <-- THE SEAM. A comment. Replace it.
    entries.insert(run_id.clone(), RecordedCompletion { seen_at: now_ms, completion });  // :104
}
```

It becomes `async` and gains the parameter, exactly as the original specified:

```rust
/// pi's `persistence?: { resultsDir, sessionId }` (`wait-completions.ts:130`).
///
/// `session_id` is a required [`SessionId`], not an `Option`: upstream's only call site
/// (`result-watcher.ts:434-437`) has already passed `:411`'s session-presence gate, and a record
/// without one is unreadable by construction (see [`CompletionReplayRecord`]).
pub struct ReplayPersistence<'a> {
    pub results_dir: &'a Path,
    pub session_id: &'a SessionId,
}

async fn record_wait_completion(
    store: &WaitCompletionStore,
    run_id: &RunId,
    data: &serde_json::Value,
    now_ms: i64,
    ttl_ms: i64,
    persistence: Option<&ReplayPersistence<'_>>,
);
```

**(2) `WaitCompletionStore::record` — `record.rs:61-63`** — the sync `pub fn` wrapper. It becomes
`pub async fn record(&self, …, persistence: Option<&ReplayPersistence<'_>>)`. Its only caller is
the observer below (`record.rs:144-149`); `grep -rn "\.record(" crates/` confirms no other
`WaitCompletionStore::record` call site in the crate.

**(3) `impl CompletionObserver for WaitCompletionStore` — `record.rs:130-152`** — where the
`persistence` argument is actually populated. Today:

```rust
async fn observe(&self, notification: &CompletionNotification) -> bool {
    if notification.band != CompletionBand::Ours { return true; }            // :135
    let Ok(data) = serde_json::to_value(&notification.result) else { return true; };  // :141
    self.record(
        &notification.result.run_id,
        &data,
        crate::time::now_epoch_millis(),
        DEDUP_TTL.as_millis().try_into().unwrap_or(i64::MAX),
    );                                                                       // :144-149
    true
}
```

**`session_id` is already right here**: `notification.result.session_id`
(`background/records.rs:517-518`, `Option<SessionId>`), and on the `CompletionBand::Ours` band it
is `Some` by construction (`delivery/custody.rs:60-64` — the band is only reachable through
`Attribution::Ours`). Build `ReplayPersistence` from `notification.result.session_id.as_ref()`;
where it is `None`, pass `persistence: None` and degrade to WORKFLOW_4's behaviour. That is exactly
pi's `:411` gate, expressed as an `Option` rather than an early `return`.

**`results_dir` is the one thing the observer does not have, and this is the only real design
decision in SUBTASK2.** `CompletionNotification` (`watch/results_watcher.rs:91-108`) carries
`result`, `result_path`, `exhausted`, `band` — no results dir — and
`WaitCompletionStore::default()` is constructed in `SubagentExecutor::new`
(`extension/executor/mod.rs:268-270`), long before any cwd is known. Two workable shapes; pick one
and say why in the doc:

* **(A) Give `WaitCompletionStore` an interior-mutable results dir.** Add
  `replay_dir: std::sync::Mutex<Option<PathBuf>>` (or `ArcSwapOption`) plus
  `pub fn set_replay_dir(&self, dir: PathBuf)`, called from
  `extension/executor/notices.rs` right after `let results_dir = default_results_dir_in(&roots, cwd);`
  (`notices.rs:422`) and before the composite is built (`notices.rs:446`). **Must be overwritable,
  not a `OnceLock`**: `install_completion_watcher` is explicitly re-run on every `SessionStart`
  (`notices.rs:414-419` documents the idempotent-reinstall contract) and a later session may have a
  different cwd. Cheapest change; keeps `CompletionNotification` untouched.
* **(B) Add `results_dir: PathBuf` to `CompletionNotification`.** `watcher.results_dir()`
  (`watch/results_watcher.rs:318-320`) is in scope at all three construction sites
  (`install.rs:319`, `:354`, `:376`); the other four sites are tests/`mission.rs`
  (`watch/observer.rs:234`,`:252`,`:301`, `extension/tool/mission.rs:276`). Fatter notification on
  a `broadcast`-adjacent path, but no hidden mutable state and every observer gets it.

(A) is the smaller blast radius and is recommended; (B) is acceptable if the implementor prefers the
value to travel with the notification. **Do not** thread `results_dir` through by reaching for a
process-global — the crate has none for this and a per-cwd value must not become one.

### The ordering comment — port it verbatim at the seam

Upstream's comment is at **`result-watcher.ts:432-433`** (was cited `:428-435`), immediately above
the `recordWaitCompletion` call at `:434-437`:

```rust
// pi `result-watcher.ts:432-433`, comment verbatim:
//   "Recorded before dedupe and before the unlink below so bg_wait can
//    use the in-memory record or its bounded durable replay after cleanup."
//
// In cyrup this position is structural rather than lexical: `deliver_pending_completions` runs
// Phase 1 (this observer) SYNCHRONOUSLY in the scan loop (`watch/install.rs:375-388`) and only
// then spawns the delivery (`:390-424`), whose receipt is what finally authorises
// `settle_delivery`'s `watcher.consume(payload, receipt)` (`watch/install.rs:172`). The record
// therefore lands before the unlink by construction. A test asserts it anyway
// (`the_replay_record_is_written_before_the_payload_is_deleted`), because a future refactor that
// moved the persist into the spawned task would still compile and would still "record the
// completion".
```

**Three rules for the body, all of them ordering:**

1. **Prune, project, PERSIST, then insert** — upstream's order (`wait-completions.ts:132-151`). The
   stored completion must be the archive-path-bearing copy `write_completion_replay` returns, not
   the pre-persist projection, or the in-process and durable tiers disagree about `archive_path`.
2. **A persistence failure is logged and swallowed** (`wait-completions.ts:147-149` —
   `console.error`, then the pre-persist `completion` is stored anyway; the original text cited
   `:141-143`, which is now the middle of the `writeCompletionReplay` argument object). The
   in-memory record must still land — degrading to WORKFLOW_4's behaviour is strictly better than
   losing both tiers. `tracing::warn!`, then continue.
3. **The `std::sync::Mutex` guard must not be held across the `.await`.** Today the body holds one
   guard from `record.rs:75` through `:110`, spanning the seam. Restructure as:
   **prune-and-project (one lock acquisition, released) → `write_completion_replay` (await) →
   lock-and-insert (second acquisition)**. Note this splits WORKFLOW_4's deliberate
   single-acquisition prune+insert (`record.rs:80-81` explains why it was one) — say in the comment
   that the split is forced by the `await` and that the observable difference is only a reader
   seeing a pruned-but-not-yet-inserted map, which `collect_wait_completions`' own ENOENT
   re-check (`collect.rs:101`) already tolerates.

   > **AUG — the lint claim in the original text is wrong, but the gate still catches it.**
   > `clippy::await_holding_lock` is **not** denied in this crate: `lib.rs:19-24` denies only
   > `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, and `Cargo.toml:100-105` adds
   > `return_self_not_must_use`. `await_holding_lock` is a default-**warn** clippy lint, which the
   > repo's gate (`cargo clippy --workspace --all-targets -- -D warnings`) promotes to an error —
   > so it does fail the build, just not for the stated reason. Write it correctly anyway.

`ttl_ms` at the observer is `DEDUP_TTL.as_millis().try_into().unwrap_or(i64::MAX)`
(`record.rs:148`, from `watch/results_watcher.rs:40` = 10 min), which WORKFLOW_4 already
establishes. It flows through unchanged as both the record's TTL and the sweep's `max_age_ms`
(pi `:207` passes `input.ttlMs` for both).

---

## SUBTASK3 — the read fallback goes in `collect.rs`, **not** `wait.rs`

> **CORRECTION to this task's original text (still valid, and now confirmed against landed code).**
> The original said *"**Where:** `background/wait.rs` — when the live scan finds no payload for a
> requested run, consult `read_completion_replay`."* That is the wrong file. `wait.rs`'s loop
> consults `list_active_runs` (`background/run_status.rs`) for run STATE and calls
> `collect_wait_completions` once at `wait.rs:1027-1032`; it never reads a payload itself.
> Upstream's replay read is in `collectWaitCompletions`' **ENOENT arm**
> (`wait-completions.ts:188-210`, the replay read at `:202-209`), which WORKFLOW_4 ported to
> `wait_completions/collect.rs` and where it left the named seam.

**Where:** `wait_completions/collect.rs:97-107` — the `Err(error) if errno::is_absent(&error)` arm.
(Note the real predicate is `errno::is_absent` (`result_index/errno.rs:31`), **not**
`e.kind() == io::ErrorKind::NotFound` as the original sketch wrote — `is_absent` also covers
`ENOTDIR` and `ENAMETOOLONG`.)

**Change:** replace the placeholder comment at `collect.rs:104-106` with the third rung. Upstream
order in that arm is fixed and all three rungs are load-bearing. The landed code today:

```rust
Err(error) if errno::is_absent(&error) => {
    // The watcher may have consumed the file between the store check above and this
    // read: its drain loop runs independently of this one, so the in-process record
    // can appear in the meantime.
    if let Some(late) = store.get(&run.run_id) {          // collect.rs:101 — rung 2
        out.push(late);
    }
    // collect.rs:104-106 — THE SEAM. Replace with rung 3.
}
```

becomes:

```rust
Err(error) if errno::is_absent(&error) => {
    // pi `:194-209`. The watcher consumed the file between the store check and this read.
    // Rung 2 — the in-process record, re-checked. NOT redundant with the check at the top of
    // the loop (`collect.rs:45`): the watcher runs on its own task and the window between them
    // is real (pi `:194-196`).
    if let Some(late) = store.get(&run.run_id) {
        out.push(late);
        continue;                    // pi `:200` — `continue`, do not fall through
    }
    // Rung 3 (SCOPE_4) — the durable replay, written before the unlink so a `wait` landing
    // after delete-last, or in a LATER PROCESS entirely, still reports the completion
    // (pi `:203`).
    //
    // `session_id` is passed through as-is, including `None`: it is an OPTIONAL filter
    // (`completion-replay.ts:228`), and an unattributed run must still read its own record.
    if let Some(replay) = crate::background::completion_replay::read_completion_replay(
        results_dir,
        &run.run_id,
        ReplayReadFilter { session_id: run.session_id.as_ref(), now: None },
    ).await {
        out.push(replay.completion);
    }
    // Nothing at all: the payload is gone, the record expired. The run contributes no
    // completion — upstream's own outcome (pi `:204`'s `if (replay)` has no else).
}
```

> **AUG — two mechanical notes.**
> * **The `continue` at rung 2 is missing from the landed code** (`collect.rs:101-103` pushes and
>   falls through into the placeholder comment, which is harmless only because the comment does
>   nothing). Once rung 3 is real, the `continue` becomes load-bearing — without it a run would
>   contribute TWO completions. Upstream has it at `:200`. Add it.
> * `run.session_id` is `Option<SessionId>` on `RunStatus` and is already used at
>   `collect.rs:52` — `.as_ref()` is the right borrow and needs no clone.

**Do not** propagate a `read_completion_replay` miss as an error. Upstream wraps only a genuine
*throw* from the replay read (`:205-208`); an absent/expired/foreign record is `undefined` and the
loop moves on. `read_completion_replay` returns `Option`, which is that behaviour by construction.

---

## SUBTASK4 — the retention schedule. It does not exist; CREATE it.

> **CORRECTION to this task's original text — RE-VERIFIED 2026-09-14 and STILL TRUE.** The original
> said *"**Where:** wherever `cleanup_result_indexes` is scheduled at install time"*. **It is
> scheduled nowhere.**
>
> ```bash
> $ grep -rn "cleanup_result_indexes" --include='*.rs' crates/
> …/result_index/retention.rs:43      pub async fn cleanup_result_indexes(     # the definition
> …/result_index/retention.rs:194,220,256,288,324,341,366,388,420             # its own #[cfg(test)]
> …/result_index/mod.rs:118           pub use retention::{DEFAULT_MAX_AGE_MS, cleanup_result_indexes};
> ```
>
> One re-export (now at `:118`, was cited `:106`), zero call sites outside its own `#[cfg(test)]`
> module. cyrup's index sweep is **dead code today** — a second, pre-existing gap this task closes
> on the way past.

**Where:** `extension/executor/notices.rs`, at the end of `SubagentExecutor::install_completion_watcher`
(**`notices.rs:414-491`**), in the `Ok(handle) => { … }` arm after the handle is stored
(**`notices.rs:483-485`**). The original cited `:394-450` / `:441-443`; both have shifted.

**Port:** **`extension/index.ts:445-452`** (was cited `:440-447`) — a 30-second `setTimeout`,
`unref`'d, wrapped in `try`/`catch`:

```ts
const resultIndexCleanupTimer = setTimeout(() => {
	try {
		cleanupResultIndexes(DIRS.results);
	} catch (error) {
		console.error("Failed to clean stale subagent result indexes:", error);
	}
}, 30_000);
resultIndexCleanupTimer.unref?.();
```

```rust
/// pi's `resultIndexCleanupTimer` (`extension/index.ts:445-452`) — a one-shot, detached,
/// best-effort retention sweep 30 s after the watcher is installed.
///
/// # Why 30 s and detached, rather than at install or on an interval
///
/// Upstream's `unref` (`:452`) is the point: the sweep must never hold the process open and must
/// never sit in front of session start. `tokio::spawn` + `sleep` is the direct analog — a
/// detached task the runtime abandons at shutdown. It is deliberately NOT retained on `self`:
/// nothing cancels it, nothing awaits it, and a session that ends inside 30 s simply never
/// sweeps, exactly as an `unref`'d timer behaves.
///
/// # Two sweeps, one schedule
///
/// * [`cleanup_result_indexes`] at [`DEFAULT_MAX_AGE_MS`] (24 h) — pi `:447`. This is its FIRST
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
        &results_dir,
        crate::time::now_epoch_millis(),
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

`results_dir` is bound at **`notices.rs:422`** and moved into
`install_completion_watcher_with_observer` at **`notices.rs:429`** (it is the first parameter and
is taken by value — `watch/install.rs:70-75`). **Clone it before that call** for the task.
`cleanup_result_indexes` returns `std::io::Result<usize>` (`result_index/retention.rs:43-47`), so
the `if let Err` above is correct; upstream's one-argument `cleanupResultIndexes(DIRS.results)`
defaults `now`/`maxAge`, which cyrup must pass explicitly.

`ttl_ms` for the replay sweep: `crate::background::watch::DEDUP_TTL.as_millis().try_into().unwrap_or(i64::MAX)`
— the same conversion `wait_completions/record.rs:148` already performs. `DEDUP_TTL` is re-exported
at `background::watch` (`record.rs:8-10` imports it from there).

> **AUG — one upstream detail the original text contradicts, recorded beside it, not instead.**
> Upstream DOES retain the timer handle and clears it at teardown:
> `clearTimeout(resultIndexCleanupTimer)` at **`extension/index.ts:1044`**. So "deliberately NOT
> retained on `self`" is a cyrup choice, not upstream's. It is a defensible one — `unref` already
> means the timer cannot hold the process open, and a detached `tokio` task dies with the runtime
> — but the doc should say it is a delta rather than imply parity. If the implementor prefers
> parity, store the `JoinHandle` next to `self.completion_watcher` (`notices.rs:484`) and `abort()`
> it on reinstall; that also stops a rapid SessionStart loop from stacking sweeps. Either is
> acceptable; the delta must be named in the comment.

---

## SUBTASK5 — doc sweep D18

**Where:** `background/wait.rs` module docs and `wait_completions/mod.rs`'s narrative.

**Change:** `wait.rs`'s `# Payload resolution` section (**`wait.rs:56-77`**) currently ends with a
forward reference that this task makes false:

```rust
//! A third rung — durable replay across a **process restart** — does not exist yet; its producer   // :74
//! has not landed. Until it does, a run whose payload is gone AND whose in-process record has      // :75
//! expired contributes only its records-only fallback (steps + an artifacts pointer), never the    // :76
//! child's own answer.                                                                             // :77
```

Delete `wait.rs:74-77` and put this in its place:

```rust
//! # Resolving a completion after cleanup (SUBA-056)
//!
//! A resolved wait reports each terminal run's completion through
//! [`super::wait_completions::collect_wait_completions`], which resolves in three rungs: the
//! in-process record, the payload on disk, and — when the watcher has already delivered and
//! UNLINKED the payload — the durable replay record in
//! [`super::completion_replay`]. The record is written BEFORE the unlink
//! (pi `result-watcher.ts:432-433`), which is the entire reason a `wait` arriving after cleanup
//! resolves instead of reporting a completion it demonstrably observed as absent. It survives a
//! process restart, which the in-process record does not, and expires at
//! [`super::watch::DEDUP_TTL`].
```

Also re-check the two stale composite-observer citations while in this file —
`wait.rs:70` and `wait.rs:240` both say `extension/executor/notices.rs:419-431`; the composite is
now at **`notices.rs:446-465`** with `self.wait_completions()` first at **`:453`**. Correcting them
is in scope for this doc sweep (they are the same forward-reference rot).

Delete every remaining `SCOPE_4`/"until it lands"/"SCOPE_4 owns" placeholder — there are exactly
five, all listed in §0.1:

```bash
grep -rn "SCOPE_4" crates/cyrup-ext-subagents/src/   # must be empty when this task is done
```

Specifically: `wait_completions/project.rs:47` and `:190-191` (the `archive_path` field doc and the
`archive_path: None` comment — the field is still set to `None` by `to_wait_completion`, which is
CORRECT: the projector does not know the archive path; `write_completion_replay` fills it in
afterwards, pi `:196`. Reword the comments to say that rather than deleting the `None`),
`wait_completions/record.rs:97-101` (replaced by the real `persistence` block), and
`wait_completions/collect.rs:104-106` (replaced by rung 3).

---

## Tests

Location: `#[cfg(test)] mod tests` inside the module that owns each behaviour — this crate does not
use a separate `tests/` tree for unit-level pins. `tempfile` is available
(`crates/cyrup-ext-subagents/Cargo.toml:181`). Test modules in this crate open with
`#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`
(e.g. `delivery/custody.rs:133`, `identity/session_id.rs:94-99`).

| test | in | fails before | pins |
|---|---|---|---|
| `a_wait_after_cleanup_resolves_from_the_replay_record` | `wait_completions/collect.rs` | **yes — the whole task** | the race: deliver, unlink, CLEAR the in-process store, then collect |
| `the_replay_record_is_written_before_the_payload_is_deleted` | `watch/install.rs` | **yes** | `result-watcher.ts:432-433`'s ordering, asserted from inside the sink |
| `reading_a_replay_with_a_foreign_session_returns_none` | `completion_replay/store.rs` | — | `:228`'s OPTIONAL gate |
| `reading_a_replay_with_no_session_filter_returns_it` | `completion_replay/store.rs` | — | OPTIONAL ≠ STRICT — the vacuous-pass guard on the row above |
| `an_expired_replay_record_is_not_returned` | `completion_replay/store.rs` | — | `expires_at`, and that the read DELETES record + archive (`:229-233`) |
| `an_archive_over_64_kib_is_truncated_and_flagged` | `completion_replay/archive.rs` | — | `ARCHIVE_TEXT_LIMIT_BYTES` + `truncated: Some(true)` + a valid UTF-8 boundary |
| `an_archive_prefers_an_existing_artifact_over_inline_text` | `completion_replay/archive.rs` | — | the `:76-103` ladder, including the `isFile` guard (`:57-65`) |
| **`an_archive_retains_a_successful_childs_final_output`** | `completion_replay/archive.rs` | — | **NEW (AUG)** — rung 3 reads `finalOutput`, not the nonexistent `output`; without this the tail rung is dead for every successful child |
| `a_future_replay_version_is_ignored_not_a_panic` | `completion_replay/record.rs` | — | `REPLAY_VERSION`; `{"version":2,…}` reads as `None` and is NOT deleted |
| `a_replay_naming_a_foreign_archive_path_is_rejected_and_removed` | `completion_replay/record.rs` | — | `validateReplayRecord` (`:141-147`) — the containment property |
| **`a_replay_whose_run_id_disagrees_with_its_file_name_is_removed_on_sight`** | `completion_replay/retention.rs` | — | **NEW (AUG)** — the `parse_replay`/`validate_replay_record` split (§1.2): it must hit row 2 (remove now), not row 4 (age-gated) |
| `cleanup_is_throttled_to_the_interval` | `completion_replay/retention.rs` | — | `:249-250` returns `false` inside the interval |
| `cleanup_removes_records_past_max_age` | `completion_replay/retention.rs` | — | the four-way age policy, incl. that an UNparseable record inside the window survives |
| `the_replay_file_name_is_the_raw_uri_encoding` | `completion_replay/mod.rs` | — | §2 — a run id that `IndexSegment` would hash must still land at `%…json`; build it with `RunId::from_token` (see §2's AUG note) |
| `the_retention_sweep_is_scheduled_after_the_watcher_installs` | `extension/executor/notices.rs` | **yes** | SUBTASK4 — `cleanup_result_indexes` currently has no call site at all |

### The two tests that carry the task, written against the real pipeline

`the_replay_record_is_written_before_the_payload_is_deleted` must assert ORDERING, not presence.
Presence passes with the record written after delivery. Use a sink that inspects the filesystem at
`deliver` time — the one moment strictly between "record" and "unlink":

```rust
/// The record must exist BEFORE `sink.deliver` is called, because the delivery's receipt is what
/// authorises `settle_delivery`'s `watcher.consume(payload, receipt)` (`watch/install.rs:172`).
/// A sink that checks from inside `deliver` is the only observer positioned between the two, and
/// it is what makes this test fail for the record-after-delivery placement that "records the
/// completion" just as truthfully.
#[async_trait::async_trait]
impl CompletionSink for OrderingSink {
    // AUG: the trait signature is now `(&self, run_id: &RunId, message: CompletionMessage)
    // -> CompletionDelivery` (`watch/sink.rs:31`) — NOT the `(message) -> bool` the original
    // sketch used.
    async fn deliver(&self, run_id: &RunId, _message: CompletionMessage) -> CompletionDelivery {
        self.replay_existed_at_delivery.store(
            completion_replay_path(&self.results_dir, run_id).exists(),
            Ordering::SeqCst,
        );
        CompletionDelivery::delivered(run_id.clone())
    }
}
```

`install.rs`'s own test module already has this exact shape to copy from: a `CompletionSink` impl at
`watch/install.rs:492-499` and three end-to-end watcher tests
(`two_instances_sharing_a_directory_each_receive_only_their_own_completions` at `:508`,
`a_stolen_payload_is_announced_and_its_dangling_index_retired` at `:620`,
`install_completion_watcher_fires_exactly_one_notify_and_deletes_the_result` at `:694`) that build a
real results dir and a real `ResultDeliveryOwnership`.

`a_wait_after_cleanup_resolves_from_the_replay_record` must **clear the in-process store** before
collecting. Without that it passes on WORKFLOW_4's `WaitCompletionStore` alone and proves nothing
about this task:

```rust
// 1. install the real watcher over a real results dir, publish a real terminal ResultFile;
// 2. wait for delivery AND for the payload to be unlinked (both, bounded — the unlink is now in
//    `settle_delivery` and lands only after the spawned delivery task joins, so poll for the
//    payload's absence rather than assuming it is synchronous with `deliver`);
// 3. drop the store — this is the "later process" / "TTL elapsed" condition, and without it the
//    in-process record answers and the durable tier is never exercised;
// 4. collect_wait_completions over a fresh, EMPTY WaitCompletionStore → the completion still
//    comes back, carrying the run id and the archive path.
```

---

## Benchmarks

None. The replay read happens once per `wait` MISS — a path that already lost a filesystem read —
and is bounded by one small JSON parse, far below the 500 ms `RESULTS_DIR_POLL_INTERVAL`
(`watch/results_watcher.rs:33`) floor the wake itself pays.

---

## Definition of done

* A `wait` issued **after** a run completed, was delivered and had its payload unlinked still
  resolves that run's completion, from the durable record, with an **empty** in-process store.
* The replay record is written before delivery and before the unlink, and a test asserts the
  **ordering** from inside the sink — not merely that the record exists afterwards.
* A replay record carries a required `SessionId`; the type cannot express one without. A foreign
  session cannot read it; **`None` reads it** (OPTIONAL, not STRICT).
* A record whose `archive_path` is not the canonical archive path for its run is rejected and
  removed; so is one whose `run_id` disagrees with its file name, **immediately and without an age
  check** (the `parse_replay`/`validate_replay_record` split, §1.2).
* Archives truncate at 64 KiB, set `truncated: true`, and never split a UTF-8 character.
* **An archive's `result-tail` rung actually carries a successful child's delivered text** — it
  reads `finalOutput` (with upstream's `output` accepted as a fallback), because neither pi's nor
  cyrup's `SingleResult` declares `output`.
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
* No `std::sync::MutexGuard` held across an `.await` anywhere in the new or edited code.
* `SUBA-056` marked closed in `docs/gap-analysis/PARITY-GAPS.md` (`:34`, `:1193`, `:1530`).
* Workspace builds, 0 failed tests, `cargo clippy --workspace --all-targets -- -D warnings` exit 0,
  `cargo fmt -p cyrup-ext-subagents` a no-op.

---

## Research notes

* Upstream: `tmp/pi-subagents/src/runs/background/completion-replay.ts`, pi HEAD **`47bae7f7`**
  (the spec was written against `df26ebc8`; the module is byte-stable, its neighbours are not).
  cyrup baseline: branch `claude/subagents-scope`, cut from `main` at `d53763b` — a long way past
  the `4ff02210` the original text cites.
* **`wait-subscriptions.ts` is a second consumer of `readCompletionReplay`** (`:251`, verified
  exact) and is **not in scope** — it is its own unported file (`PARITY-GAPS.md:34`, which now
  gives it 348 LOC). Build `read_completion_replay` so that port needs no change to it: that is why
  `ReplayReadFilter` carries `now` (`wait-subscriptions.ts:251` passes an injected `now()`) even
  though this task's own call sites always pass `None`. It also reads
  `completion?.archivePath` (`wait-subscriptions.ts:255`) to build its settle detail, which is the
  second reason `WaitCompletion::archive_path` must actually be populated.
* **`inspect-rpc.ts` is the only `readCompletionArchive` consumer** (`:262`) and is likewise
  unported. Its `:235-237` comment documents a subtlety worth keeping in mind if it is ever ported:
  it reads the RAW record first, because `readCompletionReplay` best-effort DELETES invalid records
  and a read after it cannot distinguish "never existed" from "failed validation". Its `:248-256`
  block then re-derives the version/run/session/expiry checks by hand off that raw value.
* **`async-retention.ts:505` treats an existing replay record as a reason not to reap an archive**
  (`"replay-reference"`, verified verbatim at `47bae7f7`). cyrup has no async-retention port, so
  nothing here can be reaped out from under a record today — but a future port must reproduce that
  guard, and this module's file name scheme (§2) is what makes the check expressible.
* Existing cleanup-scheduling precedent in cyrup: **still none.** `cleanup_result_indexes` with
  `DEFAULT_MAX_AGE_MS` (24 h) exists and is unreferenced — SUBTASK4 is its first caller.
* pi's `writeCompletionReplay` swallows nothing (`:186-209` can throw); its CALLER
  (`wait-completions.ts:147-149`) is what logs and continues. Keep that split: the writer returns
  `io::Result`, the recorder degrades.
* **AUG — upstream quirks this port must decide about, all recorded above rather than silently
  inherited:** `child.output` does not exist (§1.3), `data.summary` does not exist (§1.3),
  `parseReplay` deliberately does NOT take the caller's run id (§1.2), `parseArchive` is per-entry
  tolerant (§1.4), `BoundedByteTail` clamps `max_bytes` to ≥ 1 (SUBTASK1b(b)), and upstream's
  retention timer IS cleared at teardown (SUBTASK4).
