---
stage: qa
status: completed
updated: 2026-09-14 20:00
---

# SCOPE_12 — inspect RPC reads through the session index and replay

OBJECTIVE: port `background/inspect-rpc.ts` (443 LOC) — the RPC surface that reads a run's output.
It is the last consumer of the session-partitioned index, and it consumes the replay record too.

**Depends on SCOPE_3** (`wait_completions`) and **SCOPE_4** (`completion_replay`).

> **[AUG] Both dependencies landed** (PR #137, batch 1). Verified in the tree:
> `crates/cyrup-ext-subagents/src/background/wait_completions/collect.rs` (the 3-rung read,
> `collect_wait_completions` at `:41-141`) and
> `crates/cyrup-ext-subagents/src/background/completion_replay/` (`mod.rs`, `record.rs`,
> `archive.rs`, `store.rs`, `retention.rs`). Nothing in this task is blocked.
>
> **[AUG] `alreadyImplemented = false`.** `grep -rn "inspect_rpc\|INSPECT_REPLY\|PI_SUBAGENT_INSPECT"
> src/` over `cyrup-ext-subagents` returns NOTHING. There is no `background/inspect_rpc/` directory,
> no `"inspect"` arm in `route_action`, and no `"inspect"` entry in `SUBAGENT_ACTIONS`. The only
> "inspect" spellings in the crate are `run_status::inspect_status_by_id`/`_by_dir`
> (`background/run_status.rs:520`/`:594`) — the `action: "status"` renderer, a DIFFERENT surface —
> and prose in `extension/tool/text.rs`.
>
> **[AUG] Upstream pin verified.** Every line number this draft cites is correct at
> `7fe9dee1`, confirmed by
> `git -C /home/user/cyrup/tmp/pi-subagents show 7fe9dee1:src/runs/background/inspect-rpc.ts | grep -n`:
> `:233` `readResultOutput`, `:234` `resultPayloadPathForSessionRun`, `:239`
> `completionReplayPath`, `:247` `readCompletionReplay`, `:255` `rawReplay.sessionId === sessionId`,
> `:261` `readCompletionArchive`, `:284` `readOutputArtifact`, `:287` `readSessionBackedOutput`,
> `:396` the one call site inside `buildInspectReply`. File is 443 lines, as stated.

## SUBTASK1 — `background/inspect_rpc/`

**Ports:** `pi-subagents/src/runs/background/inspect-rpc.ts`

The session-bearing core:

```ts
function readResultOutput(resultsDir, sessionId, runId, stepIndex, trustedRoots,
                          stepAgent, now, trustedSessionFileRoot?) {          // :233
    const resultPath = resultPayloadPathForSessionRun(resultsDir, sessionId, runId);  // :234
    …
    const replay = readCompletionReplay(resultsDir, runId, { sessionId, now: nowMs }); // :247
    …
    && rawReplay.sessionId === sessionId                                       // :255
}
```

Three session uses in one function: resolving the payload through the index (`:234`), reading the
replay with a session filter (`:247`), and re-verifying the raw replay's session (`:255`). `:255`
is belt-and-braces over `:247`'s own OPTIONAL filter — port both, they guard different failure
modes (a filter not supplied vs a record that disagrees).

### [AUG] `:234` — the index read, exactly

`crates/cyrup-ext-subagents/src/background/result_index/locate.rs:361-377`:

```rust
pub async fn result_payload_path_for_session_run(
    results_dir: &Path,
    session_id: &SessionId,
    run_id: &RunId,
) -> std::io::Result<Option<PathBuf>> {
    if let Some(entry) = read_result_index_for_session_run(results_dir, session_id, run_id).await?
        && let Some(location) = result_payload_location_from_index(results_dir, &entry).await
    {
        return Ok(Some(location.path));
    }
    let file = ResultFileName::for_run(run_id);
    Ok(
        pending_result_location(results_dir, session_id, run_id, &file)
            .await
            .map(|location| location.path),
    )
}
```

Re-exported at `background/result_index/mod.rs:112-114`. Three facts the implementor must not
re-derive:

1. **It is `async` and FALLIBLE**, where upstream's is sync and infallible (`string | undefined`,
   `result-files.ts:334-338`). The `Err` arm is real and has an established policy — see the
   `Err` note below.
2. **It never touches the legacy public root directly.** Its two rungs are the session index entry
   (→ `result_payload_location_from_index` → `locate_payload`, `locate.rs:193+`, which itself
   probes owned → legacy root → staged and PROMOTES a staged payload on the way) and
   `pending_result_location`. This is the verbatim twin of upstream `:335-337`. Calling this
   function IS the definition-of-done clause "never by joining the public path directly".
3. **Do NOT copy `collect.rs`'s public-path fallback.** `collect_wait_completions`
   (`wait_completions/collect.rs:56-83`) does
   `Ok(found) => found.unwrap_or_else(|| public.clone())` and, for an unattributed run, addresses
   `file.resolve_in(results_dir)` outright. That rung exists because a `wait` must serve a run with
   NO session (`collect.rs:57-58`). Inspect has no such case: `buildInspectReply` returns
   `no_active_session` before it ever reaches `readResultOutput` (`inspect-rpc.ts:311-313`), and
   upstream's `readResultOutput` has no public-path rung at all. Reproducing `collect.rs`'s
   fallback here would defeat `inspect_of_a_foreign_sessions_run_returns_nothing`.

**The `Err` arm.** `collect.rs:71-79` is the precedent and the only established policy in the crate:

```rust
Err(error) if errno::is_access_denied(&error) => {
    result_index::fallback_result_payload_path_for_session_run(results_dir, session, &run.run_id)
        .await
        .unwrap_or_else(|| public.clone())
}
Err(error) => return Err(SubagentError::Spawn(error)),
```

`fallback_result_payload_path_for_session_run` is `locate.rs:397-406` (the STAGED location alone,
addressed directly so an unreadable index directory does not block recovery; its doc at `:379-396`
explains why). For inspect, the access-denied retry should reuse it and its `None` should fall
through to the replay rung — **not** to a public path. Any other `Err` is upstream's `throw`, which
`buildInspectReply`'s outer `try/catch` (`inspect-rpc.ts:313`/`:434`) turns into
`{ code: "internal", message: "Inspection could not read the async run artifacts." }`.

### [AUG] `:247` — the replay read, exactly

`crates/cyrup-ext-subagents/src/background/completion_replay/store.rs:133-171`:

```rust
pub struct ReplayReadFilter<'a> {          // store.rs:97-102
    pub session_id: Option<&'a SessionId>, // None = NO filter (OPTIONAL, not STRICT)
    pub now: Option<i64>,                  // None = crate::time::now_epoch_millis()
}

pub async fn read_completion_replay(
    results_dir: &Path,
    run_id: &RunId,
    filter: ReplayReadFilter<'_>,
) -> Option<CompletionReplayRecord>
```

Its four `None` cases and which two DELETE are tabulated in its own doc at `store.rs:106-118`:

| case | code | deletes? |
|---|---|---|
| absent / unparseable / unknown version | `store.rs:139-153` | no |
| `validate_replay_record` rejected it | `store.rs:154-159` | **yes** — the record only |
| `session_id` filter supplied and mismatches | `store.rs:160-164` | no |
| `expires_at <= now` | `store.rs:165-169` | **yes** — record AND archive |

Inspect passes `session_id: Some(&current_session_id)` (upstream `:247` passes a session; the call
site at `:396` passes `currentSessionId`) and `now: Some(now_ms)` from the SAME clock the `:255`
re-verification uses — one `now` read, threaded, never two calls to
`crate::time::now_epoch_millis()`.

**`CompletionReplayRecord`** (`completion_replay/record.rs:23-46`):

```rust
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionReplayRecord {
    pub version: ReplayVersion,     // a unit type; DESERIALIZATION rejects any value but 1
    pub run_id: RunId,
    pub session_id: SessionId,      // REQUIRED, not Option — record.rs:14-22 explains why
    pub completed_at: i64,
    pub expires_at: i64,
    pub completion: WaitCompletion,
    pub archive_path: PathBuf,      // canonicalized + mirrored onto completion.archive_path
}
```

### [AUG] `:255` — the raw pre-read, and why it CANNOT use the typed parser

Upstream reads the raw record BEFORE calling `readCompletionReplay` (`inspect-rpc.ts:236-245`)
because that function best-effort DELETES invalid and expired records, so a read after it cannot
distinguish "never existed" from "failed validation". cyrup's `read_completion_replay` has the
identical destructive behaviour (`store.rs:157` and `:166-167`), so the pre-read is required here
for the same reason — it is not a stylistic carry-over.

Two mechanical constraints on that pre-read:

* **`parse_replay` is `pub(super)`** (`record.rs:95`) and `validate_replay_record` is `pub(super)`
  (`record.rs:120`). Neither is reachable from `background/inspect_rpc/`. Do NOT widen them — the
  raw probe deliberately wants the UNvalidated bytes.
* **`serde_json::from_slice::<CompletionReplayRecord>` cannot be used for the probe either.**
  `ReplayVersion`'s `Deserialize` (`record.rs:64-75`) errors on any value but `1`, which is exactly
  the class upstream's probe must be able to SEE in order to decide `version === 1` at `:254`.

So the probe is untyped, mirroring `:241` verbatim:

```rust
let replay_path = completion_replay::completion_replay_path(results_dir, run_id); // mod.rs:100-104
let raw: Option<serde_json::Value> = match tokio::fs::read(&replay_path).await {
    Ok(bytes) => serde_json::from_slice(&bytes).ok(),
    Err(error) if result_index::errno::is_absent(&error) => None,   // pi `:243` ENOENT
    Err(error) => return Err(/* pi rethrows `:243` */),
};
```

and the `:250-257` guard, after `read_completion_replay` returned `None`, is the four-field
conjunction on the RAW value — `version == 1`, `runId == run_id`, `sessionId == session_id`,
`expiresAt` a number `> now_ms` — which, when it holds, is
`throw new Error("Completion replay record failed validation.")`. That is an inspection FAILURE
(`internal`), never a success with no `finalOutput`; upstream's comment at `:249-253` says so and
that sentence should be carried into the Rust doc.

`errno::is_absent` is `background/result_index/errno.rs`, already used for exactly this at
`store.rs:142` and `collect.rs:101`.

### [AUG] `:261-288` — the archive ladder

`read_completion_archive` is `store.rs:190-204`:

```rust
pub async fn read_completion_archive(
    archive_path: &Path,
) -> Result<Option<CompletionArchive>, SubagentError>   // Ok(None) = absent; Err = malformed
```

**Its doc at `store.rs:173-184` currently reads: *"This has no in-crate consumer yet, and that is
expected … Upstream's only caller is `inspect-rpc.ts:262`, which cyrup has not ported."* SCOPE_12
gives it its first consumer. That paragraph MUST be rewritten in the same change or it becomes a
lie the next reader will trust.**

`CompletionArchive` / `CompletionArchiveEntry` (`completion_replay/archive.rs:31-90`):

```rust
pub enum ArchiveSource { OutputArtifact, Session, ResultTail }  // serde kebab-case, archive.rs:18-27

pub struct CompletionArchiveEntry {   // archive.rs:33-75, camelCase, omit-when-absent
    pub agent: Option<String>,
    pub result_index: Option<usize>,
    pub source: ArchiveSource,
    pub path: Option<PathBuf>,
    pub text: Option<String>,
    pub truncated: Option<bool>,      // Some(true) only
}

pub struct CompletionArchive {        // archive.rs:78-90
    pub version: ArchiveVersion,
    pub run_id: RunId,
    pub created_at: i64,
    pub entries: Vec<CompletionArchiveEntry>,
}
```

Every field upstream's `matches`/`uniqueLegacyAgent`/`singleChild` selection logic
(`inspect-rpc.ts:262-288`) reads is present with the same meaning, so that block ports
one-for-one — including the two RUN-level fallbacks carrying neither `resultIndex` nor `agent`
(written at `archive.rs:304-322`).

### [AUG] Payload key names — `finalOutput`, not `output`

Upstream's `resultOutput` (`inspect-rpc.ts:170-197`) reads `data.results[i].output`,
`data.results[i].error` and `data.summary`. **Neither `output` nor `summary` exists in a
cyrup-written payload.** `SingleResult` (`exec/run_result.rs:23-25`, `rename_all = "camelCase"`)
declares `final_output: Option<String>` → `finalOutput` (`:50`) and `error: Option<String>`
(`:136`); `ResultFile` (`background/records.rs:483-528`) declares no `summary` at all.

`completion_replay/archive.rs:213-231` already met and solved this exact problem, and its ruling is
binding here:

> *rung 3 reads `finalOutput`, not upstream's `output` — deliberately … the key that actually
> carries a child's delivered text is read FIRST and upstream's spelling is kept as a fallback,
> which is additive.*

So port `resultOutput` as `finalOutput` first, `output` as the foreign-payload fallback
(`archive.rs:283` is the exact idiom), and keep the `summary` rung ported-but-dead for the same
reason `archive.rs:227-231` keeps it. **Getting this wrong makes
`inspect_resolves_output_through_the_session_index` read an empty string from a real payload while
still passing a hand-written fixture** — so the test MUST build its payload from a serialized
`SingleResult`/`ResultFile`, not from a `json!` literal that invents `output`.

### [AUG] `:199-219` `readOutputArtifact` — the failed-output prefix is DEAD here, port it anyway

`FAILED_OUTPUT_ARTIFACT_PREFIX` (`inspect-rpc.ts:67`) is written by upstream's
`formatOutputArtifactContent` (`shared/artifacts.ts:204-215`), which also appends the
`\n\nTranscript: …` / `\nMetadata: …` trailers the reader strips at `:214-216`.

**cyrup does not port `formatOutputArtifactContent`.** `artifacts.rs:494-530`'s `write_run_artifacts`
writes `content.output` verbatim to `paths.output_path`, and
`grep -rn "Subagent run failed before producing output" src/` returns nothing. So the prefix
branch, the metadata trim and the transcript trim are all dead for a cyrup-written artifact.

Port them regardless, and say so in the doc — the same standing this crate already gives
`archive.rs`'s `summary` rung (`archive.rs:227-231`): the input is a file a FOREIGN build may have
written, and a later dead-code sweep must not remove the reader half of a format. Widening cyrup's
artifact writer to emit the prefix is **out of scope** for SCOPE_12.

The tail read itself reuses `crate::exec::child_protocol::utf8_tail` (`exec/child_protocol.rs:219`,
`fn(&str, usize) -> BoundedText { text, truncated }`), which is this crate's
`decodeUtf8Tail`/`trimToUtf8Boundary` primitive — do not write a second boundary walk.

### [AUG] `:221-231` `readSessionBackedOutput` — the one seam that does NOT exist yet

This is the largest hidden cost in this 79-line draft and it is not optional: upstream's
session-backed rung needs the STRUCTURED tail.

```ts
const tail = readSessionMessagesTail(sessionPath, MAX_MESSAGE_LINES, trustedRoots, [sessionPath], trustedSessionFileRoot);
const last = tail.messages.findLast((m) => m.role === "assistant" && m.kind === "text");
const parts = tail.messages.filter((m) => m.recordIndex === last.recordIndex && m.kind === "text");
return { output: parts.map((p) => p.text).join("\n") };
```

Upstream `readSessionMessagesTail` is `fleet-view.ts:230-250`, returning
`{ messages: SessionTranscriptMessage[]; warnings: string[]; truncated: boolean }` where
`SessionTranscriptMessage` (`fleet-view.ts:174-183`) is
`{ role, kind: "text"|"toolCall"|"toolResult", text, name?, isError?, recordIndex? }`, built by
`sessionMessageParts` (`fleet-view.ts:185-218`).

**cyrup has NO counterpart.** `background/fleet_view.rs:711-752`:

```rust
fn read_session_transcript_tail(
    session_file: &Path,
    max_lines: usize,
    trusted_roots: &[PathBuf],
) -> (Vec<String>, Vec<String>)          // (formatted "role: text" lines, warnings)
```

and `session_message_line` (`fleet_view.rs:755+`) is only upstream's COLLAPSED `sessionMessageLine`
(`fleet-view.ts:220-226`). There is no `kind`, no `recordIndex`, no `name`, no `isError`, and no
`truncated` in the return. Consequences, both load-bearing:

* the session-backed `finalOutput` rung cannot be computed from what exists, and
* `InspectReply.messages` (`inspect-rpc.ts:33-39`, built by `toReplyMessage` at `:290-299`) cannot
  be built either.

**Mechanism.** Port `sessionMessageParts` into `background/fleet_view.rs` as the structured parser
(`SessionTranscriptMessage` + `read_session_messages_tail`), then re-express the existing
`session_message_line` ON TOP of it — which is exactly upstream's own layering at
`fleet-view.ts:220-226` (`sessionMessageLine` calls `sessionMessageParts`). ONE parser, not two.
`stringify_json_preview` already exists at `fleet_view.rs:823` for the `toolCall`/`toolResult`
previews. `format_async_run_transcript`'s rendering must come out byte-identical — its existing
tests (`fleet_view.rs:1080`, `:1102`, `:1125`, `:1147`) are the regression net and must stay green
untouched.

**`trustedSessionFileRoot` and the `trustedFiles` allowance have no cyrup counterpart** —
`grep -rn "trusted_session_file_root\|trustedSessionFileRoot" src/` returns nothing, and
`read_contained_text_tail` (`fleet_view.rs:167-172`) takes only `(path, max_lines, trusted_roots,
label)`. Decide explicitly (see `unresolvedQuestions`); do not silently drop the parameter from the
ported signature without a `[CYRUP-DELTA]` note.

**`trustedRoots` / `trustedSessionFileRoot`:** a recorded `sessionFile` is data a *child* wrote, so
it is never dereferenced outside the trusted roots. cyrup has this discipline already in
`extension/executor/status.rs`'s `transcript_session_roots` — reuse it, do not invent a second
containment gate.

> ### [AUG] the citation is off, and "the gate" is a different thing from "the roots"
>
> **Correction to the Research-notes line below.** `transcript_session_roots` is at
> `extension/executor/status.rs:472-484`, NOT `:431-438`. `:441` is its one call site, inside the
> `view: "transcript"` arm. Verified shape:
>
> ```rust
> /// The trusted roots a `view: "transcript"` session-JSONL read is confined to (pi
> /// `trustedSessionRootsForStatus`, `subagent-executor.ts:402-407` @v0.43.0). …
> fn transcript_session_roots(&self, cwd: &Path, roots: &crate::paths::Roots) -> Vec<PathBuf> {
>     let mut session_roots = vec![
>         default_async_root_in(roots, cwd),
>         crate::artifacts::project_subagents_dir(cwd),
>         crate::artifacts::temp_artifacts_dir(cwd),
>     ];
>     session_roots.dedup();
>     session_roots
> }
> ```
>
> It is a PRIVATE method on `SubagentExecutor` (no `pub`), so a module under `background/` cannot
> call it as written.
>
> **The crate has two root LISTS and one GATE. Keep them straight:**
>
> | thing | where | what it is |
> |---|---|---|
> | `transcript_session_roots` | `extension/executor/status.rs:472-484` | pi `trustedSessionRootsForStatus` — async root + project subagents dir + temp artifacts dir |
> | `trusted_session_roots` | `extension/executor/paths.rs:196-212` | pi `state.trustedSessionRoots` (`extension/index.ts:895-898`) — configured `default_session_dir` + `subagent_session_root(parent_session_file)` |
> | `read_contained_text_tail` | `background/fleet_view.rs:167-236` | **THE GATE** |
>
> Upstream's inspect uses the SECOND list (`deps.state?.trustedSessionRoots`,
> `inspect-rpc.ts:378-382`), unioned with `node.status.sessionRoot`. The draft names the FIRST.
> Both are legitimate; the choice is recorded as an open question, and whichever is chosen the
> containment MECHANISM is the same single gate.
>
> **`status.sessionRoot` has no cyrup counterpart.** `RunStatus` (`background/records.rs:210-330`)
> declares `session_file: Option<PathBuf>` (`:268`) but no `session_root` field. That rung of
> upstream's union at `:380-381` is unportable as-is; record it as `[CYRUP-DELTA, unrepresentable]`
> rather than quietly dropping it.
>
> **The gate itself**, `fleet_view.rs:167-236`, is `fn read_contained_text_tail(path, max_lines,
> trusted_roots, label) -> TextTail` and is **private to `fleet_view`**. Its five refusals, in
> order: no trusted root at all (`:173-181`), resolved path outside every root (`:182-194`), the
> path is a SYMLINK (`:195-208`), it is not a regular file (`:209-217`), and a re-check of
> containment against the CANONICALIZED path of both sides (`:218-234`). `path_within` is
> `fleet_view.rs:239-243`. `TextTail` is `fleet_view.rs:89-94`
> (`{ path, lines: Vec<String>, truncated: bool, error: Option<String> }`), private.
>
> "Reuse, do not invent a second gate" therefore means: **widen `read_contained_text_tail` (and
> `TextTail`) to `pub(crate)`** — or expose the new `read_session_messages_tail` from `fleet_view`
> and let inspect call only that — and route every inspect-side `sessionFile` dereference through
> it. It does NOT mean copying the five checks into `inspect_rpc/`.
>
> Nit, observed not fixed: `session_roots.dedup()` (`status.rs:482`) is `Vec::dedup`, which removes
> only ADJACENT duplicates, where pi's `new Set([...])` removes all. Out of scope — do not touch it
> in this change.

**Layout:** `inspect_rpc/{mod,request,read_output,respond}.rs`.

> ### [AUG] what lands in each of the four files
>
> The crate's own convention is one narrative `mod.rs` facade plus one file per concern
> (`completion_replay/mod.rs:36-44` and `result_index/mod.rs:74-84` both spell their own layout out
> in a `File layout — one file, one concern` block; do the same here).
>
> * **`mod.rs`** — the facade + the format constants (`INSPECT_REPLY_KIND`,
>   `INSPECT_REPLY_VERSION`, `INSPECT_WIDGET_KEY`, `INSPECT_WIDGET_PREFIX`,
>   `MAX_SERIALIZED_BYTES` at `inspect-rpc.ts:14-17`, `:66`) and the bound constants
>   (`:57-65`: `MAX_ID_LENGTH` 256, `MAX_LABEL_LENGTH` 160, `MAX_TASK_LENGTH` 2_000,
>   `MAX_FINAL_OUTPUT_LENGTH` 8_000, `MAX_MESSAGE_TEXT_LENGTH` 1_000, `DEFAULT_MESSAGE_LINES` 100,
>   `MAX_MESSAGE_LINES` 200). Every constant keeps its `pi …ts:NN` citation.
> * **`request.rs`** — `InspectRequest` (`:26-31`), `InspectErrorCode` (`:18-24`) and
>   `parse_inspect_request` (`:112-138`) IF the slash form is kept (see `unresolvedQuestions` Q1);
>   `REQUEST_ID_PATTERN` is `^[A-Za-z0-9_-]{1,64}$` (`:56`).
> * **`read_output.rs`** — `read_result_output` (`:233-289`), `result_output` (`:170-197`),
>   `read_output_artifact` (`:199-219`), `read_session_backed_output` (`:221-231`). **This is where
>   all three session uses live and where five of the six named tests belong.**
> * **`respond.rs`** — `InspectReply`/`InspectReplyMessage` (`:33-55`), `error_reply` (`:104-113`),
>   `to_reply_message` (`:290-299`), `bound_content` (`:97-100`), `public_text` (`:88-92`),
>   `enforce_byte_budget` (`:303-320`), `build_inspect_reply` (`:322-433`), `encode_inspect_reply`
>   (`:435-437`), `handle_inspect_rpc_args` (`:440-443`).
>
> **Register it** at `background/mod.rs`. The public modules are listed at `:36-51` and `:69-74`;
> `inspect_rpc` is a `pub mod` sibling of `completion_replay`/`fleet_view`.
>
> ### [AUG] the seams `build_inspect_reply` composes from, all of which already exist
>
> | upstream | cyrup | shape |
> |---|---|---|
> | `resolveSubagentRunId` (`:328`) | `background::resolve_async_run_id` | `run_id_resolver.rs:183-209` — `fn(&str, &Path, &Path, Option<&SessionId>) -> Result<Option<AsyncRunLocation>, ResolveRunIdError>`; `AsyncRunLocation { async_dir: Option<PathBuf>, result_path: Option<PathBuf>, resolved_id: RunId }` at `:22-30` |
> | `reconcileAsyncRun` (`:349`) | `run_status::reconcile_by_id` / `reconcile_by_dir` | `run_status.rs:540-549` / `:557-573` — `-> Result<Option<(RunStatus, RunPaths)>, SubagentError>`, both over `control::reconcile_before_control_op` (`control.rs:206`). Use these, not `reconcile::reconcile` directly |
> | `readStatus` (`:346`) | folded into the two above | `reconcile_paths` (`run_status.rs:577-583`) maps `NotFound` → `Ok(None)` |
> | `findChildNode` step arm (`:143-152`) | `child_identity::resolve_async_status_child` | `child_identity.rs:156-161` → `AsyncStatusChildResolution::{Resolved(ResolvedAsyncStatusChild{index,id,state,agent}), NotFound(String), Ambiguous(String)}` (`:41-72`). **Reuse it** — its `NotFound` sentence is already upstream's verbatim `Child '<id>' was not found under async run '<run>'.` (`child_identity.rs:198-200`) |
> | `status.sessionId !== currentSessionId` (`:353`) | `delivery::SessionGate` | `delivery/gate.rs:30-57`. Inspect is **`Strict`** (upstream refuses outright with `no_active_session` at `:311-313`), unlike `view:"transcript"` which is `Permissive` (`status.rs:426`) |
> | `sanitizeDisplayText` | `workflows::display_text::sanitize_display_text` | `workflows/display_text.rs:82` |
> | `truncateDisplayText` | `workflows::display_text::truncate_display` | `workflows/display_text.rs:153`, `fn(&str, max_utf16_units: usize) -> String` — already carries the `[CYRUP-DELTA, unrepresentable]` surrogate note at `:147-151` |
> | `decodeUtf8Tail` | `exec::child_protocol::utf8_tail` | `exec/child_protocol.rs:219` |
>
> **`resolve_async_run_id` applies the session filter INTERNALLY** (`run_id_resolver.rs:192-197`,
> via `location_belongs_to`), which upstream's resolver does not. Passing the current session
> straight through would make upstream's `foreign_session` code UNREACHABLE — every foreign run
> would report `not_found` instead. To keep the two codes distinguishable (the tests table below
> demands it), resolve with `None` and then apply `SessionGate::Strict.admits(current, status.session_id.as_ref())`
> explicitly. State whichever is chosen in the module doc.
>
> ### [AUG] what `RunStatus`/`StepStatus` do and do not carry
>
> Verified against `background/records.rs`:
>
> * `RunStatus` (`:210-330`): `run_id`, `session_id: Option<SessionId>`, `mode`, `state`, `pid`,
>   `cwd`, `session_file: Option<PathBuf>` (`:268`), `started_at`, `ended_at`, `last_update`,
>   `current_step`, `chain_step_count`, `pending_appends`, `steps: Vec<StepStatus>`,
>   `parallel_groups`, `display_dismissed_at`, `error: Option<String>` (`:324`).
>   **No `session_root`. No `context`.**
> * `StepStatus` (`:24-143`): `agent: String` (`:26`), `status: StepState`, `session_file`,
>   `model`, `attempted_models`, `usage`, `turns`, `context_overflow`, `timeout_recovery`,
>   `error: Option<String>` (`:65`), `nested_run_ids: Vec<RunId>` (`:71`), `started_at`,
>   `ended_at`, `stop_requested`, `stopped`, `workflow_key: Option<WorkflowKey>` (`:108`),
>   `run_id: Option<RunId>` (`:115`), `session_name`, `interrupted`, `output_path_mapping`,
>   `telemetry`. **No `label`. No `context`. No `children: Vec<NestedRunSummary>`.**
>
> Three consequences, each to be recorded as a `[CYRUP-DELTA]` in the module doc rather than left
> to inference:
>
> 1. **The nested arm of `findChildNode` (`:155-168`) is unportable.**
>    `child_identity.rs:30-34` already states exactly this: cyrup's per-step nested tracking is
>    `Vec<RunId>`, not `NestedRunSummary { asyncDir, agent, agents, children }`, and
>    `reconcileNestedAsyncDescendants` has no port (`reconcile.rs` exposes only `reconcile` at
>    `:231` and `reconcile_now` at `:331`). So `resolved.kind === "nested"` (`:342-347`) and the
>    nested descent collapse away. **Do not invent `NestedRunSummary` in this task.**
> 2. **`label` collapses to `agent`.** Upstream's `node.label ?? step?.label ?? step?.agent ??
>    node.status.runId` (`:414`) becomes `step.agent` → `run_id`.
> 3. **The task-attribution `context !== "fork"` guard (`:390`) has no field to read.** Upstream
>    suppresses `task` for a forked child because a fork's session begins with inherited parent
>    history. cyrup's `context: "fresh" | "fork"` is a LAUNCH parameter
>    (`extension/tool/params.rs:140`) and is not recorded on either status record. Either omit
>    `task` entirely, or keep only the `!tail.truncated` half of the guard and say in the doc that
>    the fork half is unrepresentable. Chosen behaviour goes in `unresolvedQuestions` Q4.

## SUBTASK2 — wire the RPC surface

**Where:** the extension's tool/RPC routing (`extension/tool/routing.rs`)

**Change:** register the inspect action so it routes to the new module.

> ### [AUG] this is a CYRUP-DELTA, and it has a hard invariant attached
>
> **Upstream has no `action: "inspect"`.** `inspect-rpc.ts` is reached ONLY through a slash command:
> `handleInspectRpcArgs(args)` (`:440-443`) parses
> `/subagents-inspect-rpc <requestId> <asyncId> [childId] [--lines N]` (`:112-138`, whose usage
> string is quoted verbatim at `:130`/`:136`) and `encodeInspectReply` (`:435-437`) emits one line
> `PI_SUBAGENT_INSPECT_JSON:<json>` for the host widget. `inspect` does not appear in upstream's
> `SUBAGENT_ACTIONS`. Routing it as a tool action is cyrup's own decision and must be labelled
> `[CYRUP-DELTA]` in the dispatch arm's comment, the way `routing.rs` labels every other such
> choice.
>
> **The advertise-vs-dispatch invariant.** This crate enforces it explicitly:
> `extension/tool/schema.rs:349-360` builds the JSON-Schema `action` enum FROM
> `text.rs:215`'s `SUBAGENT_ACTIONS` slice, and `text.rs:180-188` records why
> (*"cyrup hand-wrote the list in three places and two of them drifted … A model recovering from a
> typo was therefore steered away from verbs that exist."*), while `routing.rs:2112-2118` /
> `:1604-1606` state the rule at each arm: **the enum entry and the dispatch arm land in the SAME
> change.** So SUBTASK2 is two edits, not one:
>
> 1. `extension/tool/text.rs` — add `"inspect"` to `SUBAGENT_ACTIONS` (`:215+`) with a comment
>    saying it is cyrup's own verb, since the slice's own doc at `:186-188` says every entry is
>    upstream's with a named parity item.
> 2. `extension/tool/routing.rs` — the `route_action` arm (`:1546-1707`). Position it next to
>    `"status"`, i.e. in the `"status" | "interrupt" | "stop" | … => self.route_control_action(…)`
>    band at `:1601-1603`, or as its own arm; inspect is a READ, so it must NOT go through
>    `route_control_action`'s authority consult (`:2044-2082`) — `AuthorityAction::for_tool_action`
>    (`registration/authority.rs`) returns `None` for read verbs and must keep doing so.
>
> **Existing params suffice; no new schema property is needed.** `SubagentToolParams`
> (`extension/tool/params.rs:85-215`) already declares `id: Option<String>` (`:97`),
> `run_id: Option<String>` (`:98`), `dir: Option<String>` (`:99`), `index: Option<u64>` (`:100`),
> `child_id: Option<String>` (`:110`), `lines: Option<i64>` (`:119`). Follow the `"status"` arm's
> own precedence — `p.id.as_deref().or(p.run_id.as_deref())` (`routing.rs:2090`) — and reuse
> `p.child_id` exactly as `"stop"` does (`routing.rs:2119-2124`).
>
> **There is no `requestId` param and no widget channel.** A tool call is self-correlating, so
> `requestId`, `enforceByteBudget`, `encodeInspectReply` and `INSPECT_WIDGET_PREFIX` have no
> consumer on a tool-action surface. Dropping them is scope REDUCTION and is not authorised here —
> see `unresolvedQuestions` Q1/Q2. The default this augmentation recommends: port the whole reply
> shape and `enforce_byte_budget` (they are the format), return the reply as
> `ToolResult { content: vec![Content::text(rendered)], details: Some(reply_json), .. }` following
> the `mission.*` arm's shape (`routing.rs:1651-1656`), and leave the slash/widget encoder for a
> follow-up item rather than deleting it.
>
> **Executor entry point.** `route_action` has `&self` (the `SubagentTool`) and `cwd`; the roots
> come from `self.executor.config_snapshot().await.roots` then
> `default_async_root_in(&roots, cwd)` / `default_results_dir_in(&roots, cwd)` — exactly
> `control_status_view`'s opening (`extension/executor/status.rs:333-337`). The current session is
> `self.executor.host_services().and_then(|s| s.session_id())`, or the executor's own
> `current_session_id()` (`extension/executor/session_state.rs:55-59`, which already filters the
> empty string); parse with `crate::identity::SessionId::parse_opt`
> (`identity/session_id.rs:59-60`). Adding a `control_inspect` method on `SubagentExecutor` beside
> `control_status_view` keeps `routing.rs` a pure dispatcher, which is how every other control verb
> is shaped.

## Tests

| test | pins |
|---|---|
| `inspect_resolves_output_through_the_session_index` | `:234` |
| `inspect_of_a_foreign_sessions_run_returns_nothing` | the partition holds at the RPC boundary |
| `inspect_falls_back_to_the_replay_record_after_cleanup` | `:247` — the SCOPE_4 dependency |
| `a_replay_record_with_a_mismatched_session_is_rejected` | `:255`, distinct from `:247` |
| `a_session_file_outside_the_trusted_roots_is_not_dereferenced` | the containment gate |
| `a_step_index_out_of_range_is_reported_not_panicked` | `stepIndex` bounds |

> ### [AUG] all six as fail-before / pass-after, with the fixture each needs
>
> Placement: five are unit tests in `#[cfg(test)] mod tests` inside
> `background/inspect_rpc/read_output.rs`; the sixth (`foreign_session`) belongs in `respond.rs`
> where the gate is applied. The crate's test-module preamble is mandatory and non-negotiable
> (every `#[cfg(test)] mod tests` in this crate opens with it, e.g. `collect.rs:145-150`):
>
> ```rust
> #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
> ```
>
> (The workspace denies all four in non-test code — `Cargo.toml:101-104`.)
>
> 1. **`inspect_resolves_output_through_the_session_index`** — write the payload with
>    `result_index::write_async_result_file` (the `ResultWrite { results_dir, session_id, run_id,
>    written_at, async_dir, tool_call_id }` shape used at `locate.rs:535-547`), which STAGES then
>    PROMOTES into `result-owned/<enc(session)>/`. Assert the returned output. Then assert the
>    negative that makes it real: nothing exists at `ResultFileName::for_run(&run).resolve_in(results_dir)`
>    (the legacy public path) — so a public-path join could not have produced the answer.
>    **Fails before:** no `read_result_output`. **Fails with a naive port:** if the payload is built
>    from a real `SingleResult`, an `output`-only reader returns `None` (see the `finalOutput` note).
> 2. **`inspect_of_a_foreign_sessions_run_returns_nothing`** — same write under session `s1`; read
>    as `s2`. Pin the CODE, not just emptiness: `error.code == "foreign_session"` when the
>    resolution is permissive, and assert it is NOT `not_found` — that assertion is what catches the
>    `resolve_async_run_id` internal-filter trap described above. `locate.rs:562-583`
>    (`a_foreign_session_cannot_resolve_another_sessions_run`) is the existing sibling to model.
> 3. **`inspect_falls_back_to_the_replay_record_after_cleanup`** — model it on
>    `collect.rs:302-367` (`a_wait_after_cleanup_resolves_from_the_replay_record`), which is the
>    proven recipe: write through a `WaitCompletionStore` with
>    `Some(&ReplayPersistence { results_dir, session_id })`, drop the store, assert the public path
>    is absent, then read. **The store must be dropped** — with it alive the test passes on SCOPE_3
>    alone and proves nothing.
> 4. **`a_replay_record_with_a_mismatched_session_is_rejected`** — this one has to be written with
>    care or it passes for the wrong reason. `:247`'s filter alone already returns `None` for a
>    foreign record, so the test must prove the `:255` arm is what fired: assert the call is an
>    **error** (`Completion replay record failed validation.` → `internal`), not a silent empty
>    output. Construct the record by hand with a `sessionId` that differs from the record's own
>    embedded session — see `store.rs:384-418`
>    (`a_record_failing_validation_is_removed_by_the_reader`) for the hand-written-record idiom,
>    and note `store.rs:248-285` for the foreign-session-does-NOT-delete invariant this test must
>    not break. **Fails before:** with only `:247` ported, the function returns `Ok({})`.
> 5. **`a_session_file_outside_the_trusted_roots_is_not_dereferenced`** — an archive whose
>    `source: "session"` entry `path` points outside every trusted root. Assert no bytes are read
>    and the refusal is the gate's own sentence, `Refusing to read session transcript path outside
>    trusted roots: <path>` (`fleet_view.rs:190-192`). The existing sibling is
>    `fleet_view.rs:1125` (`a_session_transcript_outside_every_trusted_root_is_refused_not_read`).
>    Add the empty-roots row too (`fleet_view.rs:173-181`: zero roots refuses outright), since
>    `readSessionBackedOutput` short-circuits on it at `inspect-rpc.ts:222`.
> 6. **`a_step_index_out_of_range_is_reported_not_panicked`** — upstream `:175-176` is
>    `results[stepIndex] === undefined → return {}`, i.e. an empty result, NOT an error. In Rust use
>    `results.get(step_index)`; `clippy::indexing_slicing` is `deny` workspace-wide
>    (`Cargo.toml:104`), so an indexing port would not even compile clean. Cover `step_index ==
>    results.len()` and a large value, over both the payload path and the archive path (the archive
>    selector at `:274-280` matches on `entry.result_index == Some(step_index)` and simply finds
>    nothing).
>
> Additionally recommended (not in the original table, additive only):
>
> * `the_archive_reader_now_has_an_in_crate_consumer` — or simply the fact that tests 3-5 exercise
>   `read_completion_archive`, which is the trigger to rewrite `store.rs:173-184`'s "no consumer
>   yet" paragraph.
> * a `routing.rs`/`routing_tests.rs` case asserting `"inspect"` both dispatches AND appears in
>   `SUBAGENT_ACTIONS` — the advertise-vs-dispatch invariant, pinned. `extension/tool/text.rs:505-518`
>   shows the existing shape of such an assertion.

## Benchmarks

None. Inspect is a user-triggered read.

## Definition of done

- Inspect resolves output through `result_payload_path_for_session_run`, never by joining the
  public path directly.
- After cleanup, inspect still returns output from the replay record.
- Both replay session checks (`:247` filter, `:255` re-verification) are present.
- A child-recorded `sessionFile` outside the trusted roots is refused, reusing the existing gate.
- Tests pass; workspace 0 failed; clippy exit 0.

> ### [AUG] additional done-conditions implied by the above (none replace a clause)
>
> - `background/inspect_rpc/` is registered as a `pub mod` in `background/mod.rs` and its `mod.rs`
>   carries a `File layout — one file, one concern` block, as `completion_replay/mod.rs:36-44` and
>   `result_index/mod.rs:74-84` both do.
> - `completion_replay/store.rs:173-184`'s *"This has no in-crate consumer yet"* paragraph is
>   rewritten — SCOPE_12 is that consumer.
> - `"inspect"` is in `SUBAGENT_ACTIONS` (`extension/tool/text.rs:215+`) **and** has a dispatch arm
>   in `route_action` (`extension/tool/routing.rs:1546-1707`) — the same change, per the
>   advertise-vs-dispatch invariant.
> - The structured session tail (`read_session_messages_tail` + `SessionTranscriptMessage`) lives in
>   `background/fleet_view.rs` with `session_message_line` re-expressed on top of it, and
>   `format_async_run_transcript`'s existing tests (`fleet_view.rs:1080`, `:1102`, `:1125`, `:1147`)
>   are unchanged and green.
> - Every unportable upstream behaviour is recorded as a `[CYRUP-DELTA]` doc note at the seam, never
>   silently dropped: nested `findChildNode`, `status.sessionRoot`, `trustedSessionFileRoot`,
>   `step.label`, `context === "fork"`, and the dead `FAILED_OUTPUT_ARTIFACT_PREFIX` branch.
> - `cargo fmt -p cyrup-ext-subagents` only (repo-wide `cargo fmt --all` must stay a no-op).

## Research notes

* Upstream: `pi-subagents/src/runs/background/inspect-rpc.ts` (HEAD `7fe9dee1`).
* Consumes: `result_index::result_payload_path_for_session_run` (landed),
  `completion_replay::read_completion_replay` (SCOPE_4).
* Existing containment gate to reuse: `extension/executor/status.rs`'s `transcript_session_roots`
  (`:431-438`) and `background/fleet_view.rs`'s containment check.

> ### [AUG] corrections and additions to the research notes
>
> * `7fe9dee1` is a COMMIT, not HEAD — the clone's HEAD is `4ab1b1b8`. Read every upstream file as
>   `git -C /home/user/cyrup/tmp/pi-subagents show 7fe9dee1:<path>`, never from the working tree.
> * **`transcript_session_roots` is at `extension/executor/status.rs:472-484`, not `:431-438`**
>   (`:441` is its call site). It is a private method on `SubagentExecutor`.
> * The containment CHECK in `background/fleet_view.rs` is `read_contained_text_tail` at
>   `:167-236`, with `path_within` at `:239-243` and the private `TextTail` at `:89-94`. All three
>   are private to the module today.
> * Second roots list, the one upstream's inspect actually uses:
>   `extension/executor/paths.rs:196-212` (`trusted_session_roots`, pi `state.trustedSessionRoots`,
>   `extension/index.ts:895-898`), over `subagent_session_root` at `paths.rs:166-179`.
> * The 3-rung `wait` read this task must mirror (and must NOT copy the public-path rung from):
>   `background/wait_completions/collect.rs:41-141`, doc at `:23-40`.
> * The `finalOutput`-vs-`output` ruling that governs `resultOutput`:
>   `background/completion_replay/archive.rs:213-231`, implemented at `:283`.
> * Upstream files worth reading alongside `inspect-rpc.ts`, all at `7fe9dee1`:
>   `src/runs/background/fleet-view.ts:174-250` (`sessionMessageParts` / `readSessionMessagesTail`),
>   `src/shared/artifacts.ts:204-215` (`formatOutputArtifactContent`, the writer of the failed-output
>   prefix), `src/runs/background/result-files.ts:334-338`
>   (`resultPayloadPathForSessionRun`, which confirms upstream has NO public-root rung either).
