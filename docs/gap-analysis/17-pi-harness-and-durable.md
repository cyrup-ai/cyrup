# 17 — pi `packages/agent/src/harness/**` and `packages/durable`: the v0.85.1 → v0.87.1 window

**This file is new (2026-09-24).** Until now README's pin table said that two things in pi were owned
by no area file: `packages/agent/src/harness/**` in the new window, and the new `packages/durable`.
This file owns both, for the window **`v0.85.1..v0.87.1`**, and it adds the ids **`HARN-001`…`HARN-002`**.

**It does not replace the existing harness trackers, and it does not duplicate them.** `AGENT-028`
(area 02), `SESS-038` (area 03) and `DRIFT-040` (area 12) already hold the harness *scope* question
(OQ-7 in `PARITY-GAPS.md`) for the harness as it stood at v0.84.1/v0.85.1. This file measures what the
window added, decides whether any of it is user-visible in pi, and records per-capability
dispositions. Growth for rows owned by other files is written under *Growth for items other files
own*, for their owners to apply. This pass does not edit those files.

## Provenance and pins

| side | pin | how obtained |
|---|---|---|
| cyrup | **`ea23ca2`** (the code commit every area was re-read against on 2026-09-24) | `git log -1` |
| pi | **`v0.87.1`** (`f07218c4d`, 2026-09-22), the newest tag. Clone HEAD is `v0.87.1-16-gb45597504`. That range was read and is recorded only as *post-tag leads* | `git -C tmp/pi describe --tags` |

Every upstream claim was settled with `git -C tmp/pi show <tag>:<path>` or `git -C tmp/pi diff
v0.85.1..v0.87.1 -- <path>` at a named tag. None was read from the working tree. Every cyrup claim was
read at `ea23ca2`. **No cargo command was run.** This is a static reading.

## Scope

**Existence at the old pin, settled with `git cat-file -e`:**

- `packages/durable` **did not exist at v0.85.1**. `git -C tmp/pi cat-file -e v0.85.1:packages/durable/package.json` fails. It first shipped at **v0.86.0**: `080160162` *feat(durable): move Pico into dedicated package* (2026-09-18), then `a16ccd9be`, `0db565924`, `e80cf1401`, `8158b0321` *versioned document storage*, and `5901c9b9e` *durable SQLite storage backend* (first tagged at v0.87.1).
- `packages/agent/src/harness/**` **did exist at v0.85.1**: 82 paths at v0.85.1, 108 at v0.87.1. `harness/pico3/**` is **entirely new in the window**. It came from `46b66c59a` *feat(agent): add hardened pico3 kernel* (2026-09-14), first tagged at v0.86.0.

| region | window | size | read by this pass |
|---|---|---|---|
| `packages/agent/src/harness/pico3/**` | new | 24 files, **8 016 lines** at v0.87.1 | **In full.** `types.ts`, `harness.ts`, `session.ts` (all 1 497 lines), `scheduler.ts`, `jsonl.ts`, `view.ts`, `chord.ts`, `system.ts`, `context.ts`, `membrane.ts`, `bounded.ts`, `hooks.ts`, `bash.ts`, `index.ts`, all nine `kinds/*.ts`, and `memory.ts` through its `stage()` validator |
| `packages/agent/src/harness/**` outside `pico3` | modified | 25 files, ~1 600 changed lines (`git diff --stat v0.85.1..v0.87.1`: 49 files, +9 642 / −652 in all) | **Every hunk read**: `config.ts`, `messages.ts`, `types.ts`, `env/nodejs.ts`, `execution/tools.ts`, `tools/image.ts`, `runtime/drive/{boundary,response,retry,structural,tool-placement,tools}.ts`, `session/{fork-policy,fork,memory,index,types,in-memory-storage-state}.ts`, `session/jsonl/{repo,storage}.ts`, the new `session/jsonl/{fork,io}.ts` in full, and the `legacy-v3.ts` rewrite (+592 changed lines) in full. `session/testing/conformance/session-repo.ts` (+339, a test suite) was **not** read |
| `packages/durable/src/**` | new | 8 files, 2 087 lines | **In full**: `types.ts`, `index.ts`, `storage/sqlite/{migrations,node,database,index}.ts`, and `storage/sqlite/storage.ts` (lines 1–480; the tail is the commit-validation helpers, read at their throw sites). `storage/memory.ts` was read at its throw sites and its contract, which is the same `Storage` interface |
| `packages/durable/{docs,test}/**` | new | docs 3 320 lines, tests ~2 970 | `README.md`, `CHANGELOG.md` and `package.json` in full. `docs/pico-v5.md` was read at the heading level, plus §1 *Terms and invariants*, §12 *API footguns* and §13 *Non-goals*. `docs/pico-v5-handoff.md` was read through *§4 JSONL publication*. **Tests were not read** |
| `packages/agent/docs/**` | modified | 20 files, +17 757 (pico v2/v3 design docs and a zip) | **Not read** beyond the diffstat. These are design documents. They carry no runtime behaviour, and README cites implementations only |

## Is any of it user-visible in pi v0.87.1?

**No.** That is the load-bearing finding of this file. It was settled by consumer census, not taken
from changelog wording:

1. **`pico3` is an experimental subpath with one consumer, and that consumer is not shipped.**
   `git -C tmp/pi show v0.87.1:packages/agent/package.json` exports it only as
   `"./experimental/pico3"`. The package root `src/index.ts` does not re-export it.
   `git -C tmp/pi grep -l pico3 v0.87.1 -- packages/coding-agent/src` finds only
   `src/experimental/micro/{api,models,runtime,tools}.ts`. `packages/coding-agent/package.json`
   ships `"files": [..., "!dist/experimental", ...]`, and `tsconfig.build.json` excludes
   `src/experimental` from the build. `experimental/micro/README.md` says to run it with
   `tsx … packages/coding-agent/src/experimental/micro/main.ts`, from source. Neither `main.ts`,
   `cli.ts` nor `experimental/cli.ts` mentions `micro`. **A user of the published `pi` binary cannot
   reach pico3.**
2. **`@earendil-works/pi-durable` has no `src` consumer anywhere in pi.** The only mention outside
   the package is prose in `coding-agent/src/experimental/services/README.md`, which describes the
   docs' canvas and diff-review patterns as "extension patterns, not built-in coding-agent services".
   The package's own README calls its API "the durable record contracts and detached in-memory
   storage implementation". Its handoff doc says **"Packages 1–3 are implemented … later Pico5
   runtime packages remain"**: at v0.87.1 there is a storage layer and no runtime.
3. **The rest of `harness/**` is not on the shipped path either.** `core/agent-session.ts`,
   `core/sdk.ts` and `core/session-manager.ts` import `Agent`/`agent-loop` types from the package
   root. `git -C tmp/pi grep -n "pi-agent-core/harness" v0.87.1 -- packages/coding-agent/src`
   matches only `src/experimental/mini/**`. Both are excluded from the published package.
   `NodeExecutionEnv`, `openTextLineReader` and `readTextLines` have **zero** hits in
   `coding-agent/src` outside `experimental/`.
4. **Neither package is in a user-facing changelog.** `packages/coding-agent/CHANGELOG.md` for
   0.86.0–0.87.1 does not mention pico, durable or micro. `packages/agent/CHANGELOG.md` does not
   mention pico3. `packages/durable/CHANGELOG.md` has one line (0.86.0, *"initial Pico durable record
   contracts and detached in-memory storage"*).

**So there is no parity gap a pi user can observe.** Each capability below is recorded as a scope
decision (`tracker`), not as a defect. Where a harness-side change *also* landed on the shipped path,
it is already filed by the area that owns that path. The census says which item.

## Capability census

For each capability: does pi ship it, does cyrup have a counterpart (checked by `grep` over
`crates/`, and by opening the file where one was found), and what is the disposition.

| # | capability | upstream @v0.87.1 | shipped in `pi`? | cyrup at `ea23ca2` | disposition |
|---|---|---|---|---|---|
| 1 | **Durable task kernel.** Every turn is a persisted state machine: `pi.generation`, `pi.tool`, `pi.post_tools`, `pi.collapse`, `pi.job`, `pi.plugin` tasks, each with a JSON checkpoint written *before* the external effect (`inflight` phases). A scheduler resumes `running` tasks after reopen | `harness/pico3/{scheduler,session,types}.ts`, `kinds/*.ts` | no | none. `crates/cyrup-agent/src/agent/run/` is a process-local loop. `grep -rni checkpoint crates/cyrup-agent/src` returns 0 | `HARN-001` |
| 2 | **Crash recovery of an in-flight request.** `requesting` is counted as a failed attempt and retried by policy (`kinds/generation.ts` phase `requesting`). A `started` tool whose declaration is `replay: "unsafe"` gets a synthetic `interrupted` result; a `replay: "safe"` one is re-invoked (`kinds/tool.ts` phase `started`) | pico3 | no | none. After a crash, cyrup, like pi's shipped coding agent, loses the in-flight turn | `HARN-001` |
| 3 | **Deferred provider responses.** A `deferred` phase polls `models.fetchDeferred` on a durable handle and `cancelDeferred` on abort | `kinds/generation.ts` | no (the pi-ai half is `ModelRuntime.streamDeferred`) | `DeferredHandle` exists in `crates/cyrup-core/src/message/assistant.rs`; no poll loop | pi-ai half owned by **`PROV-040`**; harness half is `HARN-001` |
| 4 | **Multi-conversation sessions.** Task-owned child conversations, subtree abort and idle waits, `fork(at)` with a rewindable config document resolved as-of the fork entry (`docAsOf`) | `harness.ts`, `session.ts` `fork`/`createOwnedConversation` | no | subagent children are separate sessions (`cyrup-ext-subagents`); `cyrup-session` forks by file copy (`manager/lifecycle.rs::fork_from`) | `HARN-001` |
| 5 | **Durable inbox and idempotent admission.** `send()` deduplicates by `requestId`; queued inputs are stored in the sticky document; steer and follow-up are placed at `postTools`/`final` boundaries; `write` entries with `head:"self"` make older queued inputs stale | `session.ts` `send`/`boundary` | no | steer/follow-up queues are in memory (area 02). No `requestId` dedup: `grep request_id crates/cyrup-session-svc/src` returns 0 | `HARN-001` |
| 6 | **Durable background jobs.** `pi.job`: spawn through a `ProcessHost`, `notBefore`, recurring `every`, `notify` notices written into the transcript, reconciliation after restart (`rerun`) | `kinds/job.ts` | no | `crates/cyrup-ext-subagents/src/background/scheduled_runs/` ports pi-subagents' scheduled runs. That is a different upstream and a different contract | `HARN-001` |
| 7 | **Transcript system sections.** A managed `pi.system` entry carries `toolsAdded`/`toolsRemoved`, with baseline/delta rendering and a `systemInstructions` hook | `system.ts` | the pi-ai `SystemMessage` / `TranscriptContext` half **is** shipped (v0.86.0) | none | shipped half owned by **`PROV-083`** / **`AGENT-039`**; pico half is `HARN-001` |
| 8 | **Context edits** (`omit`/`replace` by target entry id, newest edit wins) | `context.ts` `deriveContext` | shipped separately as coding-agent's `context_edit` entry (v0.87.0) | none | shipped half owned by **`SESS-052`** |
| 9 | **Kernel-bounded tool output.** Byte budget plus line budget, head or tail retention, dropped-byte and dropped-line accounting (`Bounded`), throttled 100 ms flushes into the live slot | `bounded.ts`, `kinds/tool.ts` `invoke`/`bound` | no (coding-agent keeps its own `truncate.ts`) | `crates/cyrup-tools/src/truncate.rs` `truncate_head`/`truncate_tail` port the shipped path | no action: parity is against the shipped truncation, not the kernel's |
| 10 | **Tool control.** `terminate`, `handoff` (writes a `pi.handoff` head entry) and `addTools` from a tool result | `types.ts` `ToolControl`, `kinds/post-tools.ts` | no. The shipped `addedToolNames` was *removed* at v0.86.0 | cyrup still carries `added_tool_names` | shipped-path removal owned by **`AGENT-039`** |
| 11 | **Replicated conversation view.** `watch()` gives commit-granular `Envelope`s of Chord ops; `attachChordView` and `PicoConversationService` form a remote keyed service | `view.ts`, `chord.ts` | no (it is the `pi client`/`server` experiment, gated on `PI_EXPERIMENTAL=1`) | none | related to **`SEAM-058`** (pi's experimental server/client); this pass leaves that row alone |
| 12 | **Pico3 JSONL storage.** `main.jsonl` plus `sticky-<c>.jsonl` and `task-<id>.jsonl` sidecars, main-marker publication, torn-tail truncation on open, `fsync` on by default | `jsonl.ts` | no | `crates/cyrup-session/src/store.rs` writes one coding-agent JSONL file with a background `sync_data` worker | `HARN-001`. The torn-tail half converges on **`VL-P22`** |
| 13 | **Durable storage package.** `Storage` contract (conversations, entries, tasks, submissions, versioned documents with base/delta revisions and as-of reads), `MemoryStorage`, and a SQLite backend (`node:sqlite`, WAL, `synchronous = NORMAL`, contiguous schema migrations, `busy_timeout` 5 s) | `packages/durable/src/**` | no | none. No `rusqlite`/`sqlx`/`libsql` in any workspace `Cargo.toml` | `HARN-002` |
| 14 | **Agent-level retry cap `maxAgentDelayMs`** threaded through the harness drive and pico's `retryDecision` | `harness/config.ts`, `runtime/drive/{boundary,retry,response,structural}.ts`, `kinds/generation.ts` | **yes**, via coding-agent `settings.retry.maxAgentDelayMs` | none | owned by **`CFG-081`** (and `DRIFT-057`) |
| 15 | **GIF sniffing tightened to `GIF87a`/`GIF89a`** | `harness/tools/image.ts` | **yes**, the twin in `coding-agent/src/utils/mime.ts` (`47a18e37b`) | `crates/cyrup-tools/src/ops/mod.rs` still tests `b"GIF"` | owned by **`TOOL-048`** |
| 16 | **`convertToLlm` passes `role:"system"` through** | `harness/messages.ts` | yes, as part of `TranscriptContext` | none | owned by **`AGENT-039`** / **`PROV-083`** |
| 17 | **Harness v4 session layer.** Streaming JSONL fork (`session/jsonl/fork.ts`), a fork of a *closed* legacy-v3 file through `LegacyV3Source` (an *open* v3 session is refused until upgraded), a strict LF `TextLineReader` that drops an unterminated final record, and a v3 importer that now **rejects forward parents** | `harness/session/**` | no | `cyrup-session` writes coding-agent v3 | growth on **`DRIFT-040`** (see below) |

Negative results, recorded so the next pass does not repeat them: `grep -rli
'pico3\|pico5\|pi-durable\|packages/durable' crates --include=*.rs` returns **0**. `grep -rn
'pi\.session\.\|pi\.harness\|HARNESS_TELEMETRY' crates` returns **0**. No crate depends on
`opentelemetry`. `crates/cyrup-workflow-runtime` was opened: it is pi-subagents' `workflowScript`
`deno_core` extension, **not** a durable-task counterpart.

## Open items

> The standard `ID | Severity | Kind | Effort | Title` table, as README's *Item format* requires and
> as `09b` uses. Both rows are `tracker`s. Neither proposes schedulable work, because pi ships none of
> this to users. Both stay outside every tally until their escalation condition fires.
> `scripts/count_open_items.py` lists this file (added by the 2026-09-24 synthesis pass).

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| HARN-001 | tracker | not-ported | L | pi's experimental Pico3 durable harness kernel (`@earendil-works/pi-agent-core/experimental/pico3`, new at v0.86.0, 8 016 lines) has no cyrup counterpart. It is library-only: its one consumer is the unpublished `coding-agent/src/experimental/micro` |
| HARN-002 | tracker | not-ported | L | The new `@earendil-works/pi-durable` package (v0.86.0; SQLite backend at v0.87.1) has no cyrup counterpart. At v0.87.1 it is a storage layer with zero `src` consumers anywhere in pi |

---

## HARN-001 — pi's experimental Pico3 durable harness kernel has no cyrup counterpart

**Kind** not-ported · **Severity** tracker · **Effort** L · **Confidence** confirmed (both sides read; not observed live)

**cyrup** — There is no durable-task, checkpoint, scheduler, owned-conversation or replicated-view
concept anywhere in `crates/`. The turn loop is `crates/cyrup-agent/src/agent/run/` over an in-memory
`RunCtx`. Sessions are coding-agent v3 JSONL (`crates/cyrup-session/src/store.rs`,
`manager/{append,lifecycle,branched_session}.rs`). The census rows 1–7, 9–12 give the per-capability
cyrup reading.

**upstream** — `git -C tmp/pi show v0.87.1:packages/agent/src/harness/pico3/index.ts`, exported only
as `"./experimental/pico3"` in `packages/agent/package.json`. The kernel pieces:

- `Harness.open(storage, {models, tools, taskKinds, sections, plugins, processHost})` in `harness.ts`.
- One serialized mutation *line* with atomic multi-table commits, capability-scoped transactions and
  revocable document membranes (`session.ts`, `membrane.ts`).
- The task scheduler, which resumes `running` tasks into their checkpointed phase and marks unknown
  kinds `orphaned` (`scheduler.ts`, `harness.ts` `reconcileOrphans`).
- Six built-in kinds (`kinds/*.ts`).
- Durable inbox admission (`session.ts` `send`/`boundary`).
- Managed system sections (`system.ts`).
- The per-conversation view/watch (`view.ts`) and the Chord bridge (`chord.ts`).
- Multi-file JSONL storage (`jsonl.ts`).

The only consumer is `packages/coding-agent/src/experimental/micro/runtime.ts`
(`JsonlStorage.open`, `Harness.open`). It is excluded from the published package, and its README runs
it via `tsx` from source.

**Impact** — None for a pi user: pi's shipped `pi` binary never reaches this code (*Is any of it
user-visible*, points 1 and 4). For cyrup it is strategic. pi is prototyping a successor harness in
which a crash mid-turn resumes instead of losing the turn, admission is idempotent, and a turn is a
set of persisted tasks. `packages/durable/docs/pico-v5-handoff.md` @v0.87.1 already calls Pico3
"reference material only" and moves the design to Pico5 in `packages/durable`, so porting Pico3 itself
would chase a superseded prototype.

**Fix** — **Do not port.** Answer OQ-7 (`AGENT-028` / `SESS-038` / `DRIFT-040`) once, for the whole
harness line (the v0.84 harness, Pico3 and Pico5 together), because all three ask the same question.
If the answer is "track", the target is the Pico5 contract in `packages/durable`, not Pico3.

**Escalation (what turns this into counted work)** — any one of these:

- (a) `git -C tmp/pi grep -l 'pico3\|pi-durable' <tag> -- packages/coding-agent/src` matches a
  file outside `src/experimental/`.
- (b) `packages/coding-agent/package.json` at a tag stops excluding `dist/experimental`.
- (c) `packages/agent/src/index.ts` re-exports `./harness/pico3`.
- (d) a coding-agent CHANGELOG entry names pico, durable or micro as a user-facing feature.

Any of these makes the census rows user-visible. Re-rate each then, starting with row 2 (crash
recovery), because it is the one a user would notice.

**Verify** — the escalation greps above, run at each new tag.

## HARN-002 — `@earendil-works/pi-durable` has no cyrup counterpart

**Kind** not-ported · **Severity** tracker · **Effort** L · **Confidence** confirmed (both sides read)

**cyrup** — No SQLite or other embedded database in the workspace (no `rusqlite`/`sqlx`/`libsql` in
any `Cargo.toml`). No versioned-document store. No task or submission tables. Session persistence is
the coding-agent v3 JSONL file only.

**upstream** — `git -C tmp/pi show v0.87.1:packages/durable/src/types.ts` defines the `Storage`
contract:

- `commit(writes)` is atomic across conversations, entries, tasks, submissions and document
  create/change/retire.
- `mintId` allocates from a session-global id space.
- Fork-aware `scanEntries` and `findLatestHeadMarker` walk parent conversations up to `parent.at`.
- `submissionByRequest` looks up a submission by request id.
- Document incarnations are session-, conversation- or task-scoped and read with
  `findDocument`/`document(id, at)`, from a base plus later deltas, with a version boundary that
  requires a new base.
- `ROOT_CONVERSATION_ID = 1`.

The backends:

- `storage/memory.ts` is the in-memory backend.
- `storage/sqlite/{storage,migrations,node}.ts` is the SQLite backend: `STRICT` tables, one
  contiguous migration list, `BEGIN IMMEDIATE` transactions, WAL with `synchronous = NORMAL`,
  `wal_checkpoint(TRUNCATE)` on close, `busy_timeout` 5 000 ms. It rejects a schema newer than it
  supports.

The package README says it "contains the Pico runtime". The handoff doc says only packages 1–3
(records, memory documents, SQLite) are implemented at v0.87.1. There is no Session or scheduler
yet. **No `src` file anywhere in pi imports the package.**

**Impact** — None for a pi user. For cyrup this is the storage half of the same strategic question
as `HARN-001`, and it is the second SQLite session backend pi has grown without wiring it in. The
first was `packages/session-backends/sqlite-node` (`SESS-038`).

**Fix** — Do not port. Decide it with `HARN-001` / OQ-7. If cyrup ever adopts a durable store, the
contract to follow is `packages/durable/src/types.ts` together with `docs/pico-v5.md` §10 *Storage
contract*, and not Pico3's `Storage` in `harness/pico3/types.ts`, which the handoff doc supersedes.

**Escalation** — `git -C tmp/pi grep -l '@earendil-works/pi-durable' <tag> -- 'packages/*/src'`
matches a file outside `packages/durable` and outside any `experimental/` directory, or a
coding-agent session opens a `.sqlite` file.

**Verify** — that grep at each new tag.

---

## Growth for items other files own

These are findings for rows this pass may not edit. Each names its owner.

- **`DRIFT-040` (area 12) — harness v4 session layer, `v0.85.1..v0.87.1`.**
  - `git -C tmp/pi diff v0.85.1..v0.87.1 -- packages/agent/src/harness/session` replaces
    snapshot-then-rewrite forking with a streaming `runJsonlFork` (`session/jsonl/fork.ts`, new, 328
    lines).
  - It adds `LegacyV3Source` (`session/jsonl/legacy-v3.ts`), so a *closed* coding-agent v3 file can
    be forked into v4 without materializing it. An *open* v3 session is refused: *"Cannot fork an open
    legacy v3 JSONL session; commit a non-empty transaction to upgrade it to format 4 first"*
    (`session/jsonl/repo.ts` `resolveForkInput`).
  - It reads v3 through a strict-LF `TextLineReader` (`harness/types.ts`, `env/nodejs.ts`
    `NodeTextLineReader`) that **ignores an unterminated final line**.
  - Its v3 importer now **throws on a forward or missing parent** (*"Legacy v3 entry … has a missing
    or forward parent at line …"*, `indexLegacyV3Entry`).
  - **cyrup side, read to settle interop:** `crates/cyrup-session/src/manager/branched_session.rs::create_branched_session`
    writes the retained path root-to-leaf, re-chaining each parent to the previous entry and
    appending labels after it. `manager/lifecycle.rs::fork_from` copies source entries verbatim, in
    file order. Appends only ever follow an existing leaf. **So a cyrup-written v3 file satisfies
    the parent-order constraint.** That is a negative result, and nothing is owed.
- **`VL-P22` (`PARITY-GAPS.md`)** — Pico3's `jsonl.ts` `truncateTornTail` and the harness
  `JsonlStorage.openV4` both repair a torn tail. That is two more upstream implementations of the
  behaviour `VL-P22` says cyrup lacks. It does not change the row.
- **Area 12's `AssistantMessageFrameEncoder` strike (its 2026-09-24 second-pass disposition block)
  cites false evidence.** It says the encoder's "only references outside `packages/ai` are
  `packages/agent/docs/**` (no `src/` consumer)". At v0.87.1, `git -C tmp/pi grep -l
  AssistantMessageFrameEncoder v0.87.1 -- packages` also matches
  **`packages/agent/src/harness/pico3/kinds/generation.ts`** and
  **`packages/agent/src/harness/runtime/drive/response.ts`**. **The disposition itself still
  stands**: both consumers are harness code off the shipped path, so it is still not a port target
  on its own. Only the stated evidence is wrong, and area 12's owner should correct it.

## Leads assigned to this pass by area 12, resolved

Area 12's *UNVERIFIED — 2026-09-14 census* marks three harness bullets as "owned by the
harness/durable pass". This file is that pass.

- **"Harness telemetry schema reshaped"** (`v0.84.1..v0.85.1`, *"the cyrup side was NOT read"*) —
  **struck, not applicable.** The cyrup side has now been read. There is no telemetry emitter to
  mirror: `grep -rn 'pi\.session\.\|pi\.harness\|before_drive\|HARNESS_TELEMETRY' crates` returns 0
  and no crate depends on `opentelemetry`. That is exactly what `AGENT-028`'s body already records.
  `harness/telemetry.ts` is unchanged in `v0.85.1..v0.87.1` (absent from the diffstat). The rename
  set is therefore subsumed by the `AGENT-028` tracker, and no item is owed.
- **"New `src/harness/runtime/**` durable drive layer"** (`v0.85.1`) — **its window delta is
  resolved; its v0.85.1 body is not.** Every `runtime/drive/*` hunk in `v0.85.1..v0.87.1` was read.
  They are the retry-cap threading (census row 14 → `CFG-081`) and the deletion of
  `addedToolNames`-driven `activeTools` growth in `tool-placement.ts` (census row 10 →
  `AGENT-039`). The lead's premise, "cyrup has no counterpart", holds (`HARN-001`'s cyrup reading).
  **The 27 runtime files as they stood at v0.85.1 were still not read line by line.** That is left
  inside the OQ-7 decision rather than read speculatively, because none of it is shipped.
- **"`legacy-v3.ts` — upstream declares cyrup's format legacy"** — **resolved into growth on
  `DRIFT-040`** (above). The v0.87.1 rewrite adds three interop constraints (forward parents, torn
  final line, open-v3 fork refusal). The only one cyrup could violate is parent order, and it does
  not.

The fourth harness bullet, *"`Context` threaded through every FileSystem / Shell / harness async
method"*, is explicitly owned by area 12 (it says so itself) and is not taken here.

## Post-tag leads (pi `v0.87.1..b45597504`)

**Untagged, so these are leads and not items**, per README. Read with `git -C tmp/pi show <sha>` and
`git -C tmp/pi diff --stat v0.87.1..b45597504 -- packages/agent/src/harness packages/durable`
(30 files, +5 320 / −412, six non-merge commits):

- `898ab8040` *feat: add durable JSONL storage backend* and `b313731b8` *feat: add JSONL sidecar
  reclamation*. `packages/durable/src/storage/jsonl/storage.ts` (821 lines) implements Pico5
  handoff §4: `main.jsonl` plus one sidecar per document incarnation and per live task.
- `b45597504` *feat(durable): export scoped storage conformance suite (#9977)*. `test/` →
  `src/testing/`, exported.
- `packages/durable/src/env/{index,node}.ts` (+1 152) and `env/utils/{output-capture,truncate,
  adaptive-publisher}.ts`. The package gains its own `ExecutionEnv` (`FileSystem` + `Shell`,
  `truncate`/`fsync` file operations).
- `a32782520` *docs: propose Pico5 live extension registries* (`docs/pico-v5-live-registries.md`,
  +614) and `967246214` *docs: refine Pico5 JSONL publication design*.
- `packages/agent/src/harness/pico3/legacy-tracker.ts` (+90) plus small `session.ts`/`view.ts`
  edits (`9a139c62b` *feat(chord): consolidate immutable delta tracking*).

**None of it is wired into the shipped path.** At `b45597504`, `git grep -l 'pi-durable\|pico3' --
packages/coding-agent/src` still matches only `src/experimental/micro/**`, and `coding-agent`'s
`files` still excludes `dist/experimental`. So `HARN-001`/`HARN-002` escalation conditions (a)–(d)
have not fired past the tag either.

## RE-MEASURE — 2026-09-24

**This pass read:**

- `harness/pico3/**` in full: 24 files, 8 016 lines.
- Every `harness/**` hunk outside pico3 in `v0.85.1..v0.87.1`, except the test-support conformance
  suite.
- `packages/durable/src/**` in full, except the tail of the SQLite commit-validation helpers and the
  body of `storage/memory.ts`, which were read at their throw sites. Both implement the same
  `types.ts` contract, which was read in full.
- The durable README, CHANGELOG, `package.json`, handoff doc (through §4), and the spec's §1, §12 and
  §13.
- The consumer census for both packages at v0.87.1 and at `b45597504`.
- On the cyrup side: `cyrup-session`'s fork and branch writers and `cyrup-workflow-runtime`'s crate
  doc, plus the greps listed under *Capability census*.

**Still unread, and why:**

1. `packages/durable/docs/pico-v5.md` §2–§11 (~2 000 lines of normative design for a runtime that
   does not exist at v0.87.1), `docs/pico-v5-chord-usage.md` and `docs/chord-delta-findings.md`.
   They describe unimplemented Pico5 packages, so there is nothing to measure against yet.
2. `packages/durable/test/**` and `harness/session/testing/conformance/session-repo.ts`. These are
   test suites, and no item here rests on test behaviour.
3. `packages/agent/docs/**` (+17 757). These are Pico v2/v3 design documents and a zip. Pico3 is
   superseded by its own authors.
4. The v0.85.1 bodies of the 27 `harness/runtime/**` files. They are pre-window, off the shipped
   path, and belong to the OQ-7 decision (see *Leads … resolved*).

**If any `HARN` escalation condition fires, items 1 and 4 stop being excludable.**

## Blind spots — read before the next pass

- **Everything here rests on "not shipped", and "not shipped" was settled by grep and packaging
  config at two points (v0.87.1 and `b45597504`).** The tests were not run and the published npm
  tarball was not inspected. If the published `@earendil-works/pi-coding-agent` ever carries
  `dist/experimental`, the dispositions invert. The escalation greps are the tripwire.
- **This file measures the window, not the whole harness.** The v0.84 harness (`AGENT-028`,
  `DRIFT-040`) is still owned by those rows and still unaudited line by line.
- **cyrup's `cyrup-herdr` crate is out of scope here.** It is a *client* for herdr
  (`github.com/herdrdev/herdr`), not a port of herdr, and nothing in pi's harness or durable
  packages touches it.
