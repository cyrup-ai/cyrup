# 17 — pi's durable harness: `packages/durable` (and the deleted `packages/agent/src/harness/**`)

> **Window, 2026-10-02.** This file opened against `v0.85.1..v0.87.1` and now also covers
> **`v0.87.1..v1.0.0`**. In that second window pi **deleted `packages/agent/src/harness/**` outright**
> and `packages/durable` absorbed it, so the first half of the original title names a path that no
> longer exists upstream. `HARN-003` records the deletion and the four rows whose pin it moves;
> `HARN-004` measures what `packages/durable` became. Ids are `HARN-NNN` for pi's harness and
> `DUR-NNN` for `packages/durable` and the packages around it.

> **CORRECTED 2026-10-03 (file-level; ledger corrections after PR #173 and ADR-0029/ADR-0030).**
> This file was written before cyrup built a Pico5 durability kernel. Since then
> `docs/adr/ADR-0029-durable-pico5-scope.md` (accepted 2026-10-02, "decided by default under the
> parity rule — overridable") decided the scope question this file's trackers deferred: design to
> the Pico5 specification, build the durability kernel (spec §1–§4 and §10–§11) as Rust-native code,
> do **not** build §5–§9 (scheduler, durable task machine, `ExecutionEnv`, `CodingTools`), and do not
> port the implementation. `docs/adr/ADR-0030-durable-rust-architecture.md` sets the Rust design.
> The ADRs do not use the id "OQ-7", but the question they decide is the harness-scope question this
> file calls OQ-7 (`AGENT-028` / `SESS-038` / `DRIFT-040`), so every "OQ-7 is open / deferred" sentence
> below is stale; ADR-0029 records the decision and names itself owner of `HARN-002` and `HARN-004`.
> The kernel landed in the tree with PR #173 (merge `6bd82cb2`, build commit `d9fe2b05`): crates
> `cyrup-pico`, `cyrup-pico-doc`, `cyrup-pico-store` and `cyrup-pico-store-jsonl`. Consequence: the
> cyrup-side sentences "no durable-task ... concept anywhere in `crates/`" (`HARN-001`), "no
> versioned-document store, no task or submission tables" (`HARN-002`, `HARN-004`) and the
> "`grep -rli 'pico3\|pico5\|pi-durable\|packages/durable' crates` returns 0" claim (`HARN-003`) are
> no longer true; see the CORRECTED notes on those rows. Also corrected below: the sentence that
> "Six of the seven rows are `tracker`s" (there are eight rows and seven trackers), `HARN-004`'s
> "~18 600 lines" and "five task kinds", and the dead `docs/pico-v5.md` path (`DUR-003`).

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
| cyrup | **`fe875569`** (HEAD, 2026-10-02) — was `ea23ca2` for the `v0.85.1..v0.87.1` half | `git log -1` |
| pi | **`v1.0.0`** (2026-10-01), the newest tag — was `v0.87.1` (`f07218c4d`, 2026-09-22) | `git -C tmp/pi describe --tags` |

Every upstream claim was settled with `git -C tmp/pi show <tag>:<path>`,
`git -C tmp/pi ls-tree <tag> -- <path>`, `git -C tmp/pi cat-file -e <tag>:<path>` or
`git -C tmp/pi diff <old>..<new> -- <path>` at a named tag. None was read from the working tree.
Every cyrup claim in the `v0.85.1..v0.87.1` half was read at `ea23ca2`; every claim in the
`v0.87.1..v1.0.0` half at `fe875569`. **No cargo command was run.** This is a static reading.

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
| 14 | **Agent-level retry cap `maxAgentDelayMs`** threaded through the harness drive and pico's `retryDecision` | `harness/config.ts`, `runtime/drive/{boundary,retry,response,structural}.ts`, `kinds/generation.ts` | **yes**, via coding-agent `settings.retry.maxAgentDelayMs` | none | owned by **`CFG-081`** (and `DRIFT-057`). **2026-09-28:** both closed on `claude/lows-next`. The coding-agent cap `retry.maxAgentDelayMs` is now read and applied at cyrup's two retry sites. The harness drive threading itself is still unported |
| 15 | **GIF sniffing tightened to `GIF87a`/`GIF89a`** | `harness/tools/image.ts` | **yes**, the twin in `coding-agent/src/utils/mime.ts` (`47a18e37b`) | `crates/cyrup-tools/src/ops/mod.rs` still tests `b"GIF"` | owned by **`TOOL-048`** |
| 16 | **`convertToLlm` passes `role:"system"` through** | `harness/messages.ts` | yes, as part of `TranscriptContext` | none | owned by **`AGENT-039`** / **`PROV-083`** |
| 17 | **Harness v4 session layer.** Streaming JSONL fork (`session/jsonl/fork.ts`), a fork of a *closed* legacy-v3 file through `LegacyV3Source` (an *open* v3 session is refused until upgraded), a strict LF `TextLineReader` that drops an unterminated final record, and a v3 importer that now **rejects forward parents** | `harness/session/**` | no | `cyrup-session` writes coding-agent v3 | growth on **`DRIFT-040`** (see below) |

Negative results, recorded so the next pass does not repeat them: `grep -rli
'pico3\|pico5\|pi-durable\|packages/durable' crates --include=*.rs` returns **0**. `grep -rn
'pi\.session\.\|pi\.harness\|HARNESS_TELEMETRY' crates` returns **0**. No crate depends on
`opentelemetry`. `crates/cyrup-workflow-runtime` was opened: it is pi-subagents' `workflowScript`
`deno_core` extension, **not** a durable-task counterpart.

## Open items

> **Next free ids: `HARN-005` and `DUR-006`** (2026-10-02, after the pi v1.0.0 pass filed `HARN-003`, `HARN-004` and `DUR-001`…`DUR-004`). **2026-10-03:** `DUR-005` was filed and closed (cyrup-original, the `cyrup-session` rename-durability fix landed with PR #173; it is the only closed row in this table) — `DUR-006` is next; `HARN-005` is unallocated.

> The standard `ID | Severity | Kind | Effort | Title` table, as README's *Item format* requires and
> as `09b` uses. Two id series: `HARN-NNN` for pi's harness, `DUR-NNN` for `packages/durable` and the
> packages around it. **Six of the seven rows are `tracker`s** — they propose no schedulable work,
> because pi still ships none of this to a user of its published binary, and they stay outside every
> tally until their escalation condition fires. The one severity-bearing row is `DUR-003`, which is a
> citation defect in this file and in `HARN-002`, not a port gap.
> `scripts/count_open_items.py` lists this file (added by the 2026-09-24 synthesis pass).
>
> **CORRECTED 2026-10-03:** "Six of the seven rows are `tracker`s" is wrong. The table had eight rows
> (`HARN-001`…`HARN-004`, `DUR-001`…`DUR-004`), of which **seven** are `tracker`s (every row but
> `DUR-003`). It now has nine: `DUR-005` is a closed `cyrup-original` row, filed 2026-10-03 (below the
> `DUR-004` row). Counted set (`count_open_items.py`): 0 critical · 0 high · 0 medium · 1 low = 1 open,
> 7 trackers, 1 closed.

| ID | Severity | Kind | Effort | Title |
|---|---|---|---|---|
| HARN-001 | tracker | not-ported | L | pi's experimental Pico3 durable harness kernel (`@earendil-works/pi-agent-core/experimental/pico3`, new at v0.86.0, 8 016 lines) has no cyrup counterpart. It is library-only: its one consumer is the unpublished `coding-agent/src/experimental/micro` **CORRECTED 2026-10-03:** the cyrup-side sentence "There is no durable-task, checkpoint, scheduler, owned-conversation or replicated-view concept anywhere in `crates/`" (body, **cyrup**) is no longer true. PR #173 (build commit `d9fe2b05`, merge `6bd82cb2`) added `crates/cyrup-pico-store/src/records/{task,submission,conversation}.rs` (task/submission/conversation records), `crates/cyrup-pico/src/fork.rs` (forks) and `crates/cyrup-pico/src/observe.rs` (observation/publication). What is still true: there is no scheduler and no durable task machine, because `docs/adr/ADR-0029-durable-pico5-scope.md` decided not to build spec §5-§9 (`crates/cyrup-pico/src/lib.rs:145-146`: "§5's durable task machine — out of build scope entirely (ADR-0029)"). The subject here (Pico3) is deleted upstream (`HARN-003`) and ADR-0029 records that Pico3 is out of scope (spec §13). Tracker status unchanged. |
| HARN-002 | tracker | not-ported | L | The new `@earendil-works/pi-durable` package (v0.86.0; SQLite backend at v0.87.1) has no cyrup counterpart. At v0.87.1 it is a storage layer with zero `src` consumers anywhere in pi **CORRECTED 2026-10-03:** (1) "No versioned-document store. No task or submission tables" (body, **cyrup**) is no longer true: `crates/cyrup-pico-doc` is a versioned-document core and `crates/cyrup-pico-store/src/records/{task,submission}.rs` define the task and submission records (PR #173, `d9fe2b05`). "No embedded database" is still true (`grep -rE '^(sqlx|rusqlite|redb|sled|libsql) *=' Cargo.toml crates/*/Cargo.toml` matches nothing at `6823f35b`); the storage backend that was built is the JSONL one (`crates/cyrup-pico-store-jsonl`), and ADR-0029 decision 5 defers any engine choice to a measured trigger. (2) The scope question this row defers was decided by `docs/adr/ADR-0029-durable-pico5-scope.md`, which names itself owner of this row; the row stays a tracker. (3) The Fix's `docs/pico-v5.md` path is dead at v1.0.0; read `packages/durable/docs/spec.md` §10 *Storage contract* (line 4188 @v1.0.0) (see `DUR-003`). |
| HARN-003 | tracker | upstream-drift | S | **pi v1.0.0 deleted `packages/agent/src/harness/**` outright, so `HARN-001`'s subject and four rows' upstream pin no longer exist** — `7fd478a2e`, a declared Breaking Change in `packages/agent/CHANGELOG.md:6` @v1.0.0; `git ls-tree v1.0.0 -- packages/agent/src/` has six entries and no `harness`. All four `HARN-001`/`HARN-002` escalation conditions re-tested at v1.0.0 and still negative. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the body's cyrup reading "`grep -rli 'pico3\|pico5\|pi-durable\|packages/durable' crates --include=*.rs` returns 0" is stale: it returns 66 files at `6823f35b` (30 in `cyrup-pico`, 15 in `cyrup-pico-store-jsonl`, 13 in `cyrup-pico-store`, 5 in `cyrup-pico-doc`, 3 in `cyrup-session`), all from PR #173. The upstream half (deletion `7fd478a2e`, `packages/agent/CHANGELOG.md:6`, six files in `packages/agent/src`, `packages/session-backends` absent, 280 files +484/-97 141) was re-run and holds. The four re-pin edits in the body's **Fix** are not applied anywhere in this file. OQ-7, which the body says upstream answered, was decided for cyrup by `docs/adr/ADR-0029-durable-pico5-scope.md`. |
| HARN-004 | tracker | upstream-drift | L | **`packages/durable` became the harness: 8 files and 2 087 lines at v0.87.1, 60 files and ~18 600 lines of complete Pico5 runtime at published `1.0.0`** — so `HARN-002`'s recorded premise, "a storage layer with zero `src` consumers", is stale in both halves. Scheduler, five task kinds, session transactions, forks, live view, prompt extensions, `CodingTools`, an `ExecutionEnv` and three storage backends are all now implemented, and two unshipped frontends consume them. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** (1) "~18 600 lines" should read **17 662** (`git -C tmp/pi ls-tree -r --name-only v1.0.0 packages/durable/src` = 60 files; `git show v1.0.0:<path> | wc -l` summed = 17 662); the 60-file count and per-file counts hold. (2) "five task kinds" should read **three**: `git -C tmp/pi grep -n 'name: "pi\.' v1.0.0 -- packages/durable/src/` returns only `pi.compaction` (`harness/compaction.ts:103`), `pi.generation` (`harness/generation.ts:116`) and `pi.tool` (`harness/tool.ts:51`); five is the number of `TaskState` statuses. Both corrections are also stated in ADR-0029 (Measurement 1). (3) The cyrup reading "no versioned-document store, no task or submission tables" is stale after PR #173 (see `HARN-002`'s note); "no embedded database" still holds. (4) The Fix's "re-word HARN-002" and "fold HARN-001 ... into it" are overtaken by ADR-0029, which names itself owner of this row and `HARN-002`. (5) Impact point 1 cites `docs/pico-v5.md` §10, a dead path; read `docs/spec.md` §10 (see `DUR-003`). |
| DUR-001 | tracker | upstream-drift | S | **`@earendil-works/chord` reached published `1.0.0` and deleted its `src/state/` layer, but all four of `EXT-088`'s escalation conditions are still negative** — `src/state/{diff,draft,value}.ts` (997 lines) are gone, `src/delta/` is canonical, and `ReplicatedState` gained a source/attachment contract; `packages/chord/PLANNING.md:3` still reads "not a stable public API contract yet" verbatim, and no shipped `coding-agent/src` file imports chord. `EXT-088` stays the owner; this row is area 17's chord pin. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** two stale cites in the body: the README table citation `README:360` should be `README:437`, and `EXT-088`'s body is at `:2510`, not `:2508` (the upstream claims, `PLANNING.md:3` unchanged at v1.0.1 and no chord CHANGELOG, re-held). Also, the body's cyrup sentence that chord ("TUI chords only") has no counterpart is false for `crates/cyrup-pico-doc`, which reimplements Chord's change/delta model (`crates/cyrup-pico-doc/src/change.rs:3`; ADR-0029 decision 2: "Chord's seven operation tuples"); that is a document-core reimplementation, not a TUI chord, and `EXT-088` is still the owner of the escalation conditions. Both line cites re-checked: `README.md:437` carries the "pi `packages/chord` line by line" row and `06-cyrup-ext.md:2510` is the `## EXT-088` heading. |
| DUR-002 | tracker | upstream-drift | S | **pi's experimental `client`/`server` tree — `SEAM-058`'s subject — was re-founded on `@earendil-works/pi-durable` at v1.0.0 and now keeps no reducer of its own** — `48dd1e2f0`; `experimental/services/transcript.ts` fell to 9 lines serving `ConversationView` straight from the durable harness, `transcript-provider.ts` −125, `session-worker.ts` 206 changed lines. `SEAM-058`'s own escalation (pi's `main()` referencing `experimentalCli`) is still negative. **FILED 2026-10-02**; body below. |
| DUR-003 | low | upstream-drift | S | **`packages/durable/docs/pico-v5.md` does not exist at v1.0.0 — it is `docs/spec.md` — so `HARN-002`'s Fix and `HARN-004`'s Fix both route the OQ-7 reader to a dead path** — `git cat-file -e v1.0.0:packages/durable/docs/pico-v5.md` fails; `docs/spec.md` is 4 601 lines with §10 *Storage contract* at line 4188. The acceptance artifacts to cite instead are named below. **FILED 2026-10-02**; body below. **CORRECTED 2026-10-03:** the row's Fix edits 1 and 2 are now applied as CORRECTED notes at the dead cites (`HARN-002` Fix and `HARN-004` Impact point 1 point to `docs/spec.md` §10, line 4188 @v1.0.0, verified with `git -C tmp/pi show v1.0.0:packages/durable/docs/spec.md`); the original sentences are kept per the ledger's no-rewrite rule, so a grep of `pico-v5\.md` still hits them. `pico-v5.md` is absent at v1.0.0 (`cat-file -e` fails). `docs/adr/ADR-0029-durable-pico5-scope.md` and `ADR-0030-durable-rust-architecture.md` cite `spec.md` throughout. Edit 3 (name `spec-usage.test.ts`, `test/examples/**` and `./testing` as acceptance artifacts) is not applied. Row left open. |
| DUR-004 | tracker | upstream-drift | S | **The release post's "Pi Durable ships `pi-durable`, `pi-ai` and `chord`" is a re-announcement of three packages already publishable at `v0.87.1`, and `pi-ai` is `packages/ai` itself, not a repackaging** — all three carried `"version": "0.87.1"`, `files`, `prepublishOnly`, no `private`, and a root-`README.md` table entry at the old pin. Recorded as a negative result so no later pass re-derives it, with the three genuinely new packaging surfaces named. **FILED 2026-10-02**; body below. |
| ~~DUR-005~~ | ~~low~~ **CLOSED 2026-10-03** | cyrup-original | S | **Filed and closed 2026-10-03** (fixed outside any ledger batch, in PR #173's build commit `d9fe2b05`, merge `6bd82cb2`; slice S11 of `docs/PICO5-PLAN.md`, ADR-0029 decision 6, ADR-0030 §3). `DiskStore::rewrite` (`crates/cyrup-session/src/store.rs`) replaced a session file by temp-file-and-rename but never fsynced the parent directory: at `d9fe2b05^` it did `f.sync_data()?` then `std::fs::rename(&tmp, &self.path)?` (`store.rs:322-324`), which makes the new inode durable and leaves the directory entry naming it in the page cache, so after power loss a unix filesystem could legally restore the pre-rewrite entry while `rewrite` had returned `Ok(())`. `rewrite` is reached only to persist a format migration or an eager clone seed (`manager/lifecycle.rs`, `manager/branched_session.rs`), i.e. when the only copy of the history is rebuilt from memory; the old file stays intact, so this is a silent lost-rewrite, not lost history (rated low on that basis; the README rubric rates on consequence when reached). No pi basis: pi's `_rewriteFile` truncates in place (`session-manager.ts:979-988` @v0.83.0, already recorded in `03-cyrup-session.md` as `VL-P22` partially addressed), so this is not a parity row. Fixed: `store.rs:329` now calls `crate::durable::durable_rename(&tmp, &self.path)` (`crates/cyrup-session/src/durable.rs:71`: fsync the temp payload, rename, fsync the parent directory; unix arm only, the Windows arm is open as ADR-0030 §14 item 1). Verify: `cyrup-session` `tests::durable_rewrite::{rewrite_fsyncs_the_directory_its_rename_rewrote,rewrite_replaces_a_live_file_and_leaves_no_temp_sibling,rewrite_creates_a_missing_parent_chain}` (the first asserts the syscall-level parent-directory fsync; the power-loss outcome itself cannot be asserted in-process, per `durable.rs`'s module doc). Not re-run here (docs-only pass). |

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
**CORRECTED 2026-10-03:** `docs/pico-v5.md` does not exist at v1.0.0; read `packages/durable/docs/spec.md`
§10 *Storage contract* (line 4188 @v1.0.0) (`DUR-003`). The "if cyrup ever adopts a durable store" and
"decide it with OQ-7" clauses are overtaken by `docs/adr/ADR-0029-durable-pico5-scope.md`, which
adopted the Pico5 contract's guarantees and built the JSONL-backed kernel (PR #173).

**Escalation** — `git -C tmp/pi grep -l '@earendil-works/pi-durable' <tag> -- 'packages/*/src'`
matches a file outside `packages/durable` and outside any `experimental/` directory, or a
coding-agent session opens a `.sqlite` file.

**Verify** — that grep at each new tag.

---

## Findings filed 2026-10-02 — the `v0.87.1..v1.0.0` window

pi v1.0.0 (`2026-10-01`). Upstream read only through `git -C tmp/pi show v1.0.0:<path>`,
`git -C tmp/pi ls-tree v1.0.0 -- <path>`, `git -C tmp/pi cat-file -e v1.0.0:<path>` and
`git -C tmp/pi diff v0.87.1..v1.0.0 -- <paths>`; cyrup read at `fe875569`.

**This window moves the subject of the whole file.** `packages/agent/src/harness/**` — `HARN-001`'s
subject and this file's title — **no longer exists at v1.0.0** (`HARN-003`), and `packages/durable`
absorbed it: 8 files and 2 087 lines at v0.87.1, 60 files and ~18 600 lines of a complete Pico5
runtime at v1.0.0 (`HARN-004`). Neither escalation condition fired — every consumer is still under
`experimental/` — so both stay `tracker`s, and the ids `DUR-NNN` are added for the
`packages/durable` side.

## HARN-003 — pi v1.0.0 deleted `packages/agent/src/harness/**`, so four rows cite a path that no longer exists

**Kind** upstream-drift · **Severity** tracker · **Effort** S · **Confidence** confirmed (upstream
read at both tags; cyrup read at HEAD)

**upstream** — `7fd478a2e` *feat(agent): remove the experimental harness from pi-agent-core*
(2026-10-01, tagged v1.0.0). Its message: *"pi-agent-core now contains only Agent, the agent loop,
the proxy stream, and their types. Removed the harness, sessions and session storage, pico3, harness
tools, compaction, skills, prompt templates, telemetry schemas, search types, and the uuidv7 and
pi-telemetry re-exports, plus the ./node, ./harness/* and ./experimental/pico3 subpath exports.
Durable sessions live in @earendil-works/pi-durable. Also removed packages/session-backends and the
experimental mini and micro coding-agent frontends."* It is recorded as a Breaking Change at
`packages/agent/CHANGELOG.md:6` @v1.0.0.

The measurements:

- `git -C tmp/pi ls-tree --name-only v1.0.0 -- packages/agent/src/` returns `agent-loop.ts`,
  `agent.ts`, `index.ts`, `proxy.ts`, `stream-fn.ts`, `types.ts`. At v0.87.1 the same command also
  returned `harness`, `node.ts` and `search`.
- `packages/agent/src/index.ts` @v1.0.0 is five lines: `agent.ts`, `agent-loop.ts`, `proxy.ts`,
  `setDefaultStreamFn`, `types.ts`.
- `packages/agent/package.json` @v1.0.0 drops the `./node`, `./harness/context`,
  `./harness/env/nodejs`, `./harness/runtime/reducer`, `./harness/session`,
  `./harness/session/testing` and `./experimental/pico3` subpath exports, and its dependency list
  falls from seven to two (`pi-ai`, `typebox`) — `chord`, `pi-telemetry`, `diff`, `ignore` and `yaml`
  are all gone.
- `git -C tmp/pi diff --stat v0.87.1..v1.0.0 -- packages/agent/` is 280 files, **+484 / −97 141**.
  `git diff --numstat` shows insertions in exactly ten files: `CHANGELOG.md` (+17), `README.md`
  (+4/−8), two new `examples/mcp-codemode/*` files (+222), `package.json` (+5/−48), `src/agent-loop.ts`
  (+67/−25), `src/agent.ts` (+4), `src/types.ts` (+31/−2) and two test files. Everything else in the
  280 is removal.

**Area 17's premise re-tested at v1.0.0 — it holds.** `HARN-001`'s four escalation conditions, run
against the new tag:

- (a) *a `pico3`/`pi-durable` match outside `src/experimental/`* — **negative.**
  `git -C tmp/pi grep -l 'pi-durable' v1.0.0 -- packages/` matches 90 paths; every one is under
  `packages/durable/**`, `packages/coding-agent/src/experimental/**`,
  `packages/coding-agent/test/**`, or a CHANGELOG. `pico3` matches nothing outside docs.
- (b) *`coding-agent/package.json` stops excluding `dist/experimental`* — **negative.** Its `files`
  array @v1.0.0 still carries `"!dist/experimental"` and `"!dist/cli/experimental"`, and
  `tsconfig.build.json`'s `exclude` still lists `src/experimental`.
- (c) *`packages/agent/src/index.ts` re-exports `./harness/pico3`* — **negative, permanently.** The
  path is deleted.
- (d) *a coding-agent CHANGELOG entry names pico, durable or micro as user-facing* — **negative.**
  `packages/coding-agent/CHANGELOG.md`'s `[1.0.0]` section (seven New Features, eight Added, ten
  Changed, thirteen Fixed) mentions none of the three. Its headline features are fullscreen TUI,
  leaner codemode, codemode image generation, Radius login, Anthropic copy-code login, MCP OAuth
  hardening and `quietStartup: "header"`.

One new fact strengthens (b): `@earendil-works/pi-durable` **is not in
`packages/coding-agent/package.json`'s `dependencies` or `devDependencies`** @v1.0.0, although
`src/experimental/durable/**` imports it. It resolves only through the npm workspace, so the
published coding-agent tarball could not load that code even if `dist/experimental` were shipped.

**cyrup** — nothing is owed, and that is the point of this row. cyrup never ported the harness:
`grep -rli 'pico3\|pico5\|pi-durable\|packages/durable' crates --include=*.rs` returns 0 at
`fe875569`, as it did at `ea23ca2`. **CORRECTED 2026-10-03:** it returns 66 files at `6823f35b`
(PR #173 built the Pico5 durability kernel, ADR-0029); the deleted harness is still not ported. cyrup's session, skills, compaction and system-prompt ports come
from `packages/coding-agent/src/core/**`, which this commit does not touch — so the deletion
invalidates no cyrup code. The crates that would have been at risk are unaffected:
`crates/cyrup-session/src/store.rs` writes coding-agent v3 JSONL, and `crates/cyrup-tools/src/truncate.rs`
ports `coding-agent/src/core/tools/truncate.ts`, not the harness twin.

**Impact** — bookkeeping only, but it touches five rows. Four ledger rows pin themselves to a path
that no longer exists upstream:

- `HARN-001` (area 17) — its entire subject, `packages/agent/src/harness/pico3/**`, is deleted.
- `HARN-002` (area 17) — see `HARN-004`; its premise moved rather than vanished.
- `AGENT-028` (area 02, tracker) — *"pi v0.84.x's typed telemetry contract has no cyrup
  counterpart — filed to force a scope decision on `packages/agent/src/harness/**`"*. Its target
  (`harness/telemetry.ts`, the telemetry schemas, `scripts/generate-telemetry-docs.ts` and the
  `pi-telemetry` re-export) is deleted, and `packages/agent` no longer depends on
  `@earendil-works/pi-telemetry` at all.
- `SESS-038` (area 03) — the harness session layer and `packages/session-backends/sqlite-node`.
  **Both** are deleted (`packages/session-backends` is absent from `git ls-tree v1.0.0 -- packages/`).
- `DRIFT-040` (area 12) — the harness v4 session layer, including the `legacy-v3.ts` interop
  constraints area 17 resolved into growth on it. The whole file set is gone.

OQ-7 in `PARITY-GAPS.md` asks whether cyrup should port pi's harness. **Upstream answered it:** pi
removed it from the package a user installs and moved the line to `@earendil-works/pi-durable`. The
question is no longer "port the harness?" but `HARN-004`'s "track Pico5?", and it has one target
instead of three.

**Fix** — no code. Four edits, to be made by each row's owner, not by this file:

1. Re-pin `HARN-001` to `packages/durable/src/harness/**` and record that pico3 is deleted, so
   nobody re-reads its 8 016 lines. Area 17's own body already called pico3 *"a superseded
   prototype"*; v1.0.0 makes that upstream's position too.
2. Re-pin `AGENT-028` (area 02) and `SESS-038` (area 03) and `DRIFT-040` (area 12) to
   `packages/durable`, or close all three into `HARN-004` as one scope question. Area 17's
   *Capability census* already notes that all three ask the same thing.
3. Strike area 17's §*Post-tag leads* and §*Still unread* item 4 ("the v0.85.1 bodies of the 27
   `harness/runtime/**` files"). Those files no longer exist; the reading can never be owed.
4. Correct area 17's §*Growth for items other files own* bullet on area 12's
   `AssistantMessageFrameEncoder` evidence: the two harness consumers it names
   (`pico3/kinds/generation.ts`, `runtime/drive/response.ts`) are deleted, so area 12's original
   claim — *"no `src/` consumer outside `packages/ai`"* — is now **true** at v1.0.0 and the
   correction area 17 asked for should not be applied.

**Verify** — at each new tag: `git -C tmp/pi ls-tree --name-only <tag> -- packages/agent/src/` has no
`harness` entry, and the four escalation greps above stay negative.

## HARN-004 — `packages/durable` became the harness: a complete Pico5 runtime at published `1.0.0`

**Kind** upstream-drift · **Severity** tracker · **Effort** L · **Confidence** confirmed (upstream
read at both tags by file census and at the index/export level; cyrup read at HEAD)

**Why this is a row and not a closure of `HARN-002`.** `HARN-002` records *"At v0.87.1 it is a
storage layer with zero `src` consumers anywhere in pi"*, and its escalation condition is a
`pi-durable` import outside `packages/durable` and outside any `experimental/` directory. The
condition did **not** fire — every consumer is still under `experimental/`. But both clauses of the
recorded premise are now wrong: it is no longer a storage layer, and it no longer has zero
consumers. A reader who takes `HARN-002` at face value will size the port against 2 087 lines of
`Storage` contract and be wrong by an order of magnitude.

**upstream** — `git -C tmp/pi ls-tree -r --name-only v1.0.0 -- packages/durable/src` is **60
files**; at v0.87.1 it was 8. Summed with `git show v1.0.0:<path> | wc -l`, `src/**` is ~18 600
lines against 2 087. **CORRECTED 2026-10-03:** the measured sum is 17 662 lines (60 files). (The table row's "five task kinds" is three: `pi.generation`, `pi.tool`, `pi.compaction`.) `packages/durable/package.json` @v1.0.0 is `"version": "1.0.0"` with a
`prepublishOnly`, ten subpath exports and `"files": ["dist", …]`, and its description changed from
record contracts to *"Durable conversation, task, and document runtime for Pi"*. What is implemented
now that was not at v0.87.1:

- **The harness kernel**, `src/harness/**`, 21 files: `scheduler.ts` (1 337), `generation.ts` (677),
  `tool.ts` (488), `compaction.ts` (451), `harness.ts` (433), `events.ts` (425), `output.ts` (288),
  `agent.ts` (251), `view.ts` (237), `task-graph.ts` (222), `submissions.ts` (207), `live.ts`,
  `context.ts`, `prompt.ts`, `inbox.ts`, `registry.ts`, `define.ts`, `usage.ts`, `json.ts`,
  `types.ts` (643), `util.ts`. At v0.87.1 `packages/durable` had no harness directory and the
  handoff doc said *"Packages 1–3 are implemented … later Pico5 runtime packages remain"*.
- **A session layer**, `src/session/**`: `transaction.ts` (1 023), `session.ts` (568),
  `observation.ts` (301), `forks.ts` (97).
- **Three storage backends**: the SQLite one carried over (`storage/sqlite/storage.ts`, 870),
  `storage/memory.ts` (810) and a **new JSONL backend** (`storage/jsonl/storage.ts`, 845) — the
  `main.jsonl`-plus-sidecars design area 17 recorded only as a post-tag lead.
- **Its own `ExecutionEnv`**: `env/node.ts` (957) and `env/index.ts` (174).
- **Its own coding toolset**: `src/tools/**` — `bash.ts`, `read.ts`, `write.ts`, `edit.ts`,
  `edit-diff.ts` (500), `image.ts`, `file-mutation-queue.ts`, `path-utils.ts`, exported as
  `@earendil-works/pi-durable/tools` and consumed as `CodingTools`.
- **An exported conformance suite**: `src/testing/**` — `storage-conformance.ts` (1 583),
  `storage-benchmark.ts` (489), `runner.ts`, `assertions.ts` — published as the `./testing` subpath.

**Consumers — two full frontends, both unshipped.** `packages/coding-agent/src/experimental/durable/**`
(`main.ts`, `runtime.ts`, `harness-setup.ts`, `tui.ts`, `sessions.ts`, `subagent.ts`, `prompt.ts`)
is a complete second coding agent on the durable harness, and
`src/experimental/vacation/**` is a third. Its README names what the kernel now does end to end:
*"every streamed partial, tool output, queue, and turn is committed. Kill the process in the middle
of a tool call and start it again with `--continue`: the interrupted call gets an interrupted result
and the turn finishes"*; sessions live at
`~/.pi/agent/experimental/durable-sessions/<cwd-hash>/<session>/session.sqlite` under a
`proper-lockfile` lock with a 10 s stale timeout; the TUI renders `Conversation.viewState()` over the
built-in documents `pi.live`, `pi.inbox`, `pi.agent`, `pi.usage`; a `subagent` tool runs a task in a
child conversation the call owns; `/tasks` shows `Harness.taskGraph()` live.

**It is still not shipped.** The README's own run instruction is
`node --import ./packages/coding-agent/src/experimental/source-resolver.ts packages/coding-agent/src/experimental/durable/main.ts`
— from source, exactly as `micro`'s was at v0.87.1, one level up. `git grep -n 'durable\|vacation'
v1.0.0 -- packages/coding-agent/src/cli.ts packages/coding-agent/src/main.ts
packages/coding-agent/src/cli/` is **empty**: no flag, no subcommand, no entry point. See
`HARN-003` for the four escalation conditions, all still negative, and for the fact that
`pi-durable` is not even a declared dependency of `coding-agent`.

**cyrup** — unchanged from `HARN-002`'s reading and re-confirmed at `fe875569`: no embedded database
in any workspace `Cargo.toml` (no `rusqlite`, `sqlx` or `libsql`), no versioned-document store, no
task or submission tables, no checkpointed turn. `crates/cyrup-agent/src/agent/run/` is a
process-local loop over an in-memory `RunCtx`; a crash mid-turn loses the turn, as it does in pi's
shipped agent.

**Impact** — none for a pi user; `pi` the binary still cannot reach any of this. Two things change
for cyrup, both strategic:

1. **The port target is now concrete and versioned.** `HARN-002`'s *Fix* says that if cyrup ever
   adopts a durable store the contract to follow is `packages/durable/src/types.ts` plus
   `docs/pico-v5.md` §10 (**CORRECTED 2026-10-03:** dead path; use `docs/spec.md` §10, `DUR-003`). That is now a published `1.0.0` package with a stable export surface and
   an **exported conformance suite** (`./testing`, `storage-conformance.ts`, 1 583 lines) — so the
   decision OQ-7 defers is cheaper to act on than it was, and the spec is executable rather than
   prose.
2. **A divergence watch, not an obligation.** `packages/durable/src/truncate.ts` is a near-fork of
   `packages/coding-agent/src/core/tools/truncate.ts` — same `DEFAULT_MAX_LINES = 2000` and
   `DEFAULT_MAX_BYTES = 50 * 1024`, 233 diff lines apart (durable adds a `Buffer`-free
   `utf8ByteLength`; coding-agent keeps `GREP_MAX_LINE_LENGTH` and the bash tail-truncation
   exception). `crates/cyrup-tools/src/truncate.rs` ports the coding-agent one, which is correct and
   stays correct. `src/tools/**` and `env/node.ts` are likewise second implementations of surfaces
   cyrup already ports from coding-agent. **Nothing is owed** — but a future pass must read the
   coding-agent twin, not the durable one, or it will file phantom drift.

**Fix** — no code. Re-word `HARN-002` so its premise matches v1.0.0: it is a published `1.0.0`
runtime of ~18 600 lines (**CORRECTED 2026-10-03:** 17 662, see this row's table note), not a 2 087-line storage layer, and it has two experimental frontends, not
zero consumers. Keep it a `tracker` — the escalation condition is unchanged and still unfired — and
fold `HARN-001`, `AGENT-028`, `SESS-038` and `DRIFT-040` into it per `HARN-003`'s fix, so OQ-7 has
one target. If the decision is ever "track", the reading order is
`packages/durable/src/harness/types.ts` → `src/types.ts` → `src/harness/harness.ts` →
`src/harness/scheduler.ts`, and the acceptance test is the exported conformance suite.

**Escalation (unchanged in substance, re-pinned)** — any of: a `pi-durable` import in
`packages/coding-agent/src` outside `experimental/`; `pi-durable` appearing in
`coding-agent/package.json`'s `dependencies`; `coding-agent`'s `files` or `tsconfig.build.json`
ceasing to exclude `experimental`; a `durable` flag or subcommand in `cli.ts`/`main.ts`; or a
coding-agent CHANGELOG entry naming it. Re-rate crash recovery first — it is the one a user notices,
and per the durable README it now demonstrably works.

**Verify** — the five greps above at each new tag, plus
`git -C tmp/pi show <tag>:packages/durable/package.json | grep '"version"'` to catch the package
going stable-but-still-unwired again.

### Measured in this window and recorded without a row

Three facts the four rows above do not carry, kept here so a later pass does not re-derive them. None
is a cyrup gap; all three matter only if OQ-7 is ever answered "track".

- **The SQLite backend became asynchronous.** `f3e68e8ea`, `8f89293a2`, `ed391c4f0` *harden async
  SQLite adapter queue and close*, `cd0ba2fab` *execute SQLite queries by SQL text and cache
  statements per connection*, and `fd3af5ee2` *pass a transaction handle to SQLite transaction
  callbacks*. `HARN-002`'s sizing is against the synchronous v0.87.1 shape.
- **`packages/durable` gained an enforced runtime boundary so it can load in a browser** —
  `scripts/check-entry-graphs.mjs`, `scripts/durable-browser-smoke-entry.ts` and
  `test/storage-runtime-boundary.test.ts`. A port that collapses the `env` split loses a property
  upstream now tests for.
- **`packages/session-backends` was removed in this window.** It is `SESS-038`'s subject (area 03),
  and the removal is recorded in `HARN-003` rather than re-measured here.


## DUR-001 — chord reached published `1.0.0` and deleted `src/state/`, but `EXT-088` still does not escalate

**Kind** upstream-drift · **Severity** tracker · **Effort** S · **Confidence** confirmed (upstream
read at both tags; cyrup read at HEAD)

**Why this row exists and does not duplicate `EXT-088`.** `packages/chord` is held by area 06's
watch-only tracker `EXT-088` (`06-cyrup-ext.md:616`, body at `:2508`), and README's
*deliberately-unread* table (`README.md:360`) assigns "pi `packages/chord` line by line" to it,
checked by importer only. `EXT-088`'s four escalation conditions were last tested at `v0.87.1`.
Area 17 is now also the home of chord as `packages/durable`'s hard dependency, so this row carries
the v1.0.0 re-test and the area-17 pin. **`EXT-088` remains the owner of the port decision.**

**upstream** — `git -C tmp/pi diff --stat v0.87.1..v1.0.0 -- packages/chord` is 49 files,
**+10 394 / −4 995**. The structural change, from
`diff <(git ls-tree -r --name-only v0.87.1 -- packages/chord/src) <(… v1.0.0 …)`:

- **Deleted:** `src/state/diff.ts` (249), `src/state/draft.ts` (577), `src/state/value.ts` (171).
- **Added:** `src/delta/tracker.ts`, `src/delta/draft.ts`, `src/delta/diff.ts`,
  `src/delta/revision-validator.ts`, `src/delta/apply-immutable-trusted.ts`, plus
  `src/delta/README.md` (now shipped in `files`). `d5cba1d97` *feat(chord): make immutable delta
  tracker canonical* and `9a139c62b` *feat(chord): consolidate immutable delta tracking*.
- **Rewritten:** `src/services/state.ts` (+431), `src/services/provider.ts` (+70),
  `src/services/consumer.ts` (+57), `src/types.ts` (+98), `src/json.ts` (+80).

The `ReplicatedState` contract changes in `src/types.ts` @v1.0.0 are breaking:

- `ReplicatedState<T>.value` is now "**contract-immutable**, not frozen, and may share containers
  with an in-process provider" (`types.ts:43-47`) — it was documented as plain immutable.
- `subscribe()` gains an async-listener overload with a stated delivery policy: callbacks are
  serialized per subscription, hydration is awaited first, **at most 100 deliveries queue** behind
  the running callback, and overflow **keeps only the newest** pending value — "update sequences may
  skip" (`types.ts:49-55`).
- `MutableReplicatedState.change()` moved from "one synchronous copy-on-write mutation" to "one
  synchronous **overlay** mutation"; draft values are cloned by value, and assigning `undefined` to
  an object property now **deletes** it instead of throwing (`types.ts:62-66`).
- `replace()` now "takes immutable **ownership** of an alias-free strict-JSON replacement. The
  caller must not mutate the transferred root after this call" (`types.ts:68-71`) — it no longer
  copies into a detached snapshot.
- New `ReplicatedStateSource<T>` / `ReplicatedStateSourceAttachment<T>` /
  `ReplicatedStateSourceFrame<T>` / `AttachedReplicatedState<T>` (`types.ts:74-120`): an
  authoritative revision source that Chord "only publishes… it never applies or re-diffs them".
  This is the seam `packages/durable`'s `Conversation.viewState()` publishes through.
- `ServiceProviderUpdate` gains `{ type: "reset"; snapshot }` — "full subscription rebaseline after
  overflow" (`types.ts:229-230`).
- `InvalidJsonPart<T>` now accepts a `ReadonlyJsonValue` (readonly containers and readonly arrays),
  which it rejected before (`types.ts:133-149`).

**`EXT-088`'s four escalation conditions, re-tested at `v1.0.0`. None holds.**

1. *`src/experimental/` loses the `experimental` prefix, or `./experimental/plugin` is renamed to a
   stable subpath* — **negative.** `packages/coding-agent/package.json` @v1.0.0 still exports
   `"./experimental/plugin": { "source": "./src/experimental/plugin.ts" }`, source-only with no
   `dist` condition, and `files` still carries `"!dist/experimental"` and
   `"!dist/cli/experimental"`.
2. *`core/extensions/` is deleted or stops being loaded on the default CLI path* — **negative.**
   `git ls-tree --name-only v1.0.0 -- packages/coding-agent/src/core/extensions` still returns the
   directory.
3. *A shipped release loads a chord facet bundle on the default CLI path* — **negative, and now
   measured directly.** `git grep -l '@earendil-works/chord' v1.0.0 -- packages/coding-agent/src`
   returns **18 paths, every one under `src/experimental/`**. There is no non-experimental
   `coding-agent/src` importer at all. `git grep -n 'experimental' v1.0.0 --
   packages/coding-agent/src/cli.ts packages/coding-agent/src/main.ts` is **empty**: the
   experimental command tree (`src/cli/experimental/cli.ts`, `commands/{client,server}.ts`) is
   reached only through `src/experimental/commands.ts`, which neither entry point imports, and the
   `services/README.md` run line is `PI_EXPERIMENTAL=1 ./pi-test.sh server`, a dev script.
   `@earendil-works/chord` *is* a declared `dependencies` entry of `coding-agent` @v1.0.0 — so it is
   installed into a user's tree — but nothing in the shipped build can import it.
4. *`packages/chord` declares a stable public API* — **negative, and this is the one the version
   number makes look otherwise.** The package is `"version": "1.0.0"`, and the root `README.md`
   calls it a "Standalone application-composition runtime". But
   `git diff v0.87.1..v1.0.0 -- packages/chord/PLANNING.md` is **empty**, and
   `packages/chord/PLANNING.md:3` still reads, verbatim: *"Symmetric RPC and structural generation
   replacement remain planned. This is not a stable public API contract yet."* There is **no
   `packages/chord/CHANGELOG.md`** (`git cat-file -e v1.0.0:packages/chord/CHANGELOG.md` fails),
   and the chord `README.md` makes no SemVer or stability statement. The `1.0.0` is the monorepo's
   release train, not a chord API commitment.

**Citation correction for `EXT-088`.** Its body cites this sentence as `PLANNING.md:3`. There is no
root `PLANNING.md` at **either** tag — `git cat-file -e v0.87.1:PLANNING.md` and the v1.0.0 equivalent
both fail. The file is `packages/chord/PLANNING.md`. `EXT-088`'s claim is correct; only its path is.

**cyrup** — no counterpart, re-confirmed at HEAD. `grep -rniE 'chord' crates/ --include='*.rs'`
matches only TUI **keyboard chords** (`crates/cyrup-tui/src/keymap.rs`, `app/hotkeys.rs`,
`input/decode.rs`, `altscreen/keys.rs` and their tests) — no facet, service-token, keyed-instance or
replicated-state concept. `crates/cyrup-ext/wit/world.wit` is unchanged on this axis, which is
`EXT-088`'s own cyrup reading.

**Impact** — none for a pi user and none for cyrup today. Two things change for the ledger:

1. **A reader who sees `@earendil-works/chord@1.0.0` will assume `EXT-088` escalated.** It did not.
   That assumption is the cheapest way for a later pass to open a large port it does not owe, and
   this row is the written answer.
2. **`EXT-088`'s deliberately-unread grant now hides a breaking rewrite.** `src/state/**` is deleted
   and the `ReplicatedState` contract is materially different. Nothing cyrup ports depends on it, so
   nothing is owed — but if condition (3) ever fires, the contract to read is the v1.0.0
   `src/delta/**` plus `src/types.ts`, **not** the `src/state/**` the census described at v0.85.0.

**Fix** — no code. Two edits, for `EXT-088`'s owner (area 06), not this file:

- Re-date `EXT-088` to v1.0.0 and record the four negative re-tests above, with condition (4)'s
  evidence (the unchanged `PLANNING.md:3` and the absent CHANGELOG) spelled out, because the version
  bump is the thing that reads as escalation.
- Fix the citation to `packages/chord/PLANNING.md:3` and re-pin the "do not read line by line" grant
  from `src/state/**` to `src/delta/**`.

**Escalation** — unchanged in substance. Re-pinned for v1.0.0: any of (1) a non-experimental
`coding-agent/src` importer of `@earendil-works/chord`; (2) `core/extensions/` deleted; (3) `cli.ts`
or `main.ts` referencing `experimental`; (4) `packages/chord/PLANNING.md` dropping the "not a stable
public API contract yet" sentence, **or** a `packages/chord/CHANGELOG.md` appearing with a SemVer
policy.

**Verify** — at each new tag:
`git -C tmp/pi grep -l '@earendil-works/chord' <tag> -- packages/coding-agent/src | grep -v /experimental/`
(must be empty), `git -C tmp/pi grep -c 'not a stable public API contract yet' <tag> -- packages/chord/PLANNING.md`
(must be 1), and `git -C tmp/pi cat-file -e <tag>:packages/chord/CHANGELOG.md` (must fail).

## DUR-002 — pi's experimental client/server was re-founded on `pi-durable` and dropped its own reducer

**Kind** upstream-drift · **Severity** tracker · **Effort** S · **Confidence** confirmed (upstream
read at both tags; cyrup read at HEAD)

**Why this row exists.** Area 17's *Capability census* row 11 says the replicated conversation view
"is the `pi client`/`server` experiment… related to `SEAM-058`; this pass leaves that row alone".
At v1.0.0 that experiment stopped being related to the durable kernel and became a **client of it**.
`SEAM-058` (area 08, tracker, owner of the experimental server/client plus `packages/protocol` and
`packages/client`) does not know that. `HARN-004` measures `experimental/durable/**` and
`experimental/vacation/**` as the two durable frontends; it does not cover the `services/**`
client/server tree, which is `SEAM-058`'s, so this is growth and not a duplicate.

**upstream** — `48dd1e2f0` *feat(coding-agent): port the experimental client/server to pi-durable*.
`git -C tmp/pi diff --stat v0.87.1..v1.0.0 -- packages/coding-agent/src/experimental` is 56 files,
**+3 377 / −4 137** — a net deletion. The load-bearing hunks:

- `experimental/services/transcript.ts` @v1.0.0 is **9 lines in full**: it imports
  `ReplicatedState` from `@earendil-works/chord` and `ConversationView` from
  `@earendil-works/pi-durable`, and declares
  `defineService<Transcript>("pi.transcript")` over `ReplicatedState<ConversationView>`. Its doc
  comment: *"The root conversation's durable view: active entries and its live, inbox, agent, and
  usage documents."*
- `experimental/services/transcript-provider.ts` **−125**. `services/README.md` @v1.0.0 states the
  consequence: *"`Transcript` serves `Conversation.viewState()` directly: the durable Harness
  publishes exact operations per commit, so the worker keeps no reducer of its own."*
- `experimental/session-worker.ts` 206 changed lines, `services/agent-controller-provider.ts` 144,
  `services/models-provider.ts` 132, `services/worker.ts` 19, plus a new
  `experimental/session-catalog.ts` (+87).
- `experimental/micro/**` — the only pico3 consumer area 17 recorded — is **gone**, replaced by
  `experimental/durable/**`. `experimental/mini/**` is gone too (`7fd478a2e`).

**It is still not shipped, and `SEAM-058`'s own trigger has not fired.** `SEAM-058`'s escalation is
*"the moment pi's `main()` references `experimentalCli`"*.
`git -C tmp/pi grep -n 'experimental' v1.0.0 -- packages/coding-agent/src/cli.ts packages/coding-agent/src/main.ts`
is **empty** at v1.0.0, as it was at v0.84.1. `coding-agent`'s `files` still excludes
`dist/experimental` and `dist/cli/experimental`, `tsconfig.build.json` still excludes
`src/experimental`, and `@earendil-works/pi-durable` is **not** in `coding-agent`'s `dependencies` or
`devDependencies` at all, so the published tarball could not resolve this tree even if it shipped.

**cyrup** — nothing is owed. `crates/cyrup-session-svc` and `crates/cyrup-tui` render from cyrup's
own session store; there is no replicated-state transport, no service-token registry and no durable
view. `grep -rli 'pi-durable\|pico3\|pico5\|packages/durable' crates --include='*.rs'` returns 0 at
HEAD. The crate that would have been at risk, `cyrup-session-svc`, ports
`packages/coding-agent/src/core/**`, which `48dd1e2f0` does not touch.

**Impact** — none today. One thing changes for the ledger: `SEAM-058`'s subject is no longer a
self-contained experiment with its own reducer. It is now a thin presentation layer over
`pi-durable`, which means **`SEAM-058` and `HARN-002`/`HARN-004` are now the same scope question**,
exactly as area 17 found for `AGENT-028` / `SESS-038` / `DRIFT-040`. If pi ever wires
`experimentalCli` into `main()`, the port is not "the client/server tree" — it is the durable harness
underneath it, and `SEAM-058` would be the *second* row to escalate, not the first.

**Fix** — no code. For `SEAM-058`'s owner (area 08): record that the tree moved onto `pi-durable` at
v1.0.0, that its reducer is gone, and that its escalation now implies `HARN-004`'s. Fold it into the
single OQ-7 answer that `HARN-003`'s fix proposes, rather than answering it separately.

**Escalation** — unchanged: `main()` or `cli.ts` referencing the experimental command tree. Add one:
`@earendil-works/pi-durable` appearing in `packages/coding-agent/package.json`'s `dependencies`.

**Verify** — at each new tag:
`git -C tmp/pi grep -n 'experimental' <tag> -- packages/coding-agent/src/cli.ts packages/coding-agent/src/main.ts`
(must be empty) and
`git -C tmp/pi show <tag>:packages/coding-agent/package.json | grep -c pi-durable` (must be 0).

## DUR-003 — `packages/durable/docs/pico-v5.md` is gone; two rows' Fix sections route OQ-7 to a dead path

**Kind** upstream-drift · **Severity** low · **Effort** S · **Confidence** confirmed (upstream read
at both tags)

**cyrup** — this is ledger text, not code. No crate is affected.

**upstream** — `git -C tmp/pi cat-file -e v1.0.0:packages/durable/docs/pico-v5.md` **fails**. At
v1.0.0 `packages/durable/docs/` holds `spec.md`, `pico-v5-handoff.md`, `pico-v5-chord-usage.md` and
`chord-delta-findings.md`. `docs/spec.md` is **4 601 lines**, titled *Pico5 specification*, and its
section numbering survived the rename: §1 *Terms and invariants* (line 46), §2 *Core records* (80),
§3 *Documents* (919), §4 *Transactions and storage ownership* (1521), §5 *Tasks* (1568),
§6 *Submissions and inbox* (2120), §7 *Extensions, hooks, tools, and system prompt* (2251),
§8 *Built-in tasks* (3168), §9 *Document observation and Chord* (3747), **§10 *Storage contract*
(4188)**, §11 *Backends* (4371), §12 *API footguns* (4450), §13 *Non-goals* (4587).

**Who cites the dead path.** Two Fix sections, both of which are the reading instruction a future
OQ-7 decision would follow:

- `17-pi-harness-and-durable.md`, `HARN-002`'s **Fix**: *"the contract to follow is
  `packages/durable/src/types.ts` together with `docs/pico-v5.md` §10 Storage contract"*.
- `HARN-004`'s **Impact** point 1 in this file: *"`HARN-002`'s Fix says… the contract to
  follow is `packages/durable/src/types.ts` plus `docs/pico-v5.md` §10"*.

Area 17's §*Still unread* item 1 and §*RE-MEASURE* also cite `docs/pico-v5.md` §2–§11 and §1/§12/§13
as read-or-excluded. Those are provenance records of what a past pass read at `v0.87.1`, where the
file did exist, so they are **correct as history** and should be left alone; only the forward-looking
Fix lines mislead.

**What to cite instead, and why it matters more than a rename.** The durable package now ships its
specification as **executable** artifacts, which is the thing a port would actually be graded on:

- `packages/durable/test/spec-usage.test.ts` (380 lines) — its header: *"The usage examples of
  `docs/spec.md`, compile-checked. Each block is copied as written apart from formatting… Keep the
  two in sync; `test/examples/` runs the same patterns end to end."* It is the spec's own
  type-level conformance check.
- `packages/durable/test/examples/00-conversation.ts` … `31-reload-and-restart.ts` — **32 numbered
  runnable examples**, each run standalone with
  `node --conditions=source --experimental-strip-types test/examples/<n>.ts`, and tabulated in the
  package `README.md`: `14-chat` (one Q&A), `16-real-model` (streaming from OpenAI),
  `17-coding-tools` (a tool-using turn on JSONL storage), `18-print`, `19-json`, `20-inbox`
  (steers, follow-ups, writes and withdrawal while busy), `21-late-join`,
  `22-subagent-foreground`, `23-subagent-background` (spawn, steer, stop, list, restart-safe),
  `24-child-tasks`, `25-compaction`, `26-coding-agent`, `27-plan-mode`, `28-reviewer`,
  `29-sandbox-per-conversation`, `30-tool-override`, `31-reload-and-restart`.
- `packages/durable/src/testing/**`, published as the `./testing` subpath — the exported storage
  conformance suite `HARN-004` already names.

**Impact** — low but real, and it is the cheapest finding in this file to act on. OQ-7 is the
ledger's largest deferred decision; its two Fix sections are the instruction someone will follow on
the day it is answered, and today both open with a `git show` that fails.

**Fix** — no code. Three one-line edits, for area 17's owner:

1. In `HARN-002`'s Fix, replace `docs/pico-v5.md` §10 with
   `packages/durable/docs/spec.md` §10 *Storage contract* (line 4188 @v1.0.0).
2. Make the same replacement in `HARN-004`'s Impact point 1 above.
3. Add the acceptance artifacts above to whichever of the two rows survives the `HARN-003` fold, so
   "if the decision is ever track" names `spec.md` §10, `spec-usage.test.ts`, `test/examples/**` and
   the `./testing` subpath together. Leave area 17's §*Still unread* and §*RE-MEASURE* citations as
   written — they are v0.87.1 provenance.

**Verify** — `git -C tmp/pi cat-file -e v1.0.0:packages/durable/docs/spec.md` succeeds and the
`pico-v5.md` equivalent fails; `grep -rn 'pico-v5\.md' docs/gap-analysis/` returns only provenance
sentences afterwards.

## DUR-004 — the three "newly shipped" packages were already published at v0.87.1, and `pi-ai` is `packages/ai`

**Kind** upstream-drift · **Severity** tracker · **Effort** S · **Confidence** confirmed (both tags
read; npm registry not consulted — see *Blind spot*)

**Why this row exists.** It is a **negative result**, filed so no later pass re-derives it. The
release post launches "Pi Durable" as a separate experimental product *"shipping
`@earendil-works/pi-durable`, `@earendil-works/pi-ai` and `@earendil-works/chord`"*. Read as a map
that reads as three new packages and, for `pi-ai`, as a repackaging of `packages/ai` that would be
material to area 01 and to `crates/cyrup-core`/the provider crate. **It is neither.**

**upstream, at the OLD pin `v0.87.1`** — `git -C tmp/pi show v0.87.1:packages/<p>/package.json` for
all three:

| package | name @v0.87.1 | version | publishable @v0.87.1 |
|---|---|---|---|
| `packages/durable` | `@earendil-works/pi-durable` | `0.87.1` | yes — `files: ["dist","docs","README.md"]`, `prepublishOnly`, no `private` |
| `packages/ai` | `@earendil-works/pi-ai` | `0.87.1` | yes — `main`/`types`/`exports`, no `private` |
| `packages/chord` | `@earendil-works/chord` | `0.87.1` | yes — `files: ["dist","README.md"]`, `prepublishOnly`, no `private` |

And the monorepo root `README.md` **already listed all three in its `## All Packages` table at
`v0.87.1`**, with the same one-line descriptions it uses at v1.0.0:
`@earendil-works/chord` at line **30** (*"Standalone application-composition runtime for services,
replicated state, RPC, and plugins"*), `@earendil-works/pi-ai` at **32** (*"Unified multi-provider
LLM API"*) and `@earendil-works/pi-durable` at **33** (*"Durable conversation, task, and document
runtime"*) — the *same* line numbers as at v1.0.0.

**`pi-ai` is `packages/ai`.** `packages/ai/package.json`'s `"name"` is `@earendil-works/pi-ai` at
both tags. There is no new package directory: `git -C tmp/pi ls-tree --name-only v1.0.0 -- packages/`
contains no `pi-ai` entry, and `packages/durable`'s `dependencies` named
`"@earendil-works/pi-ai": "^0.87.1"` already at the old pin. **So area 01's and area 12's
reading of `packages/ai` is the whole of `pi-ai`, and cyrup's provider crate owes nothing to the
release post's package list.** Any `pi-ai` drift is `packages/ai` drift, which area 01 measured in the
same window (`PROV-113`…`PROV-129`).

**What IS new at v1.0.0, and it is packaging only:**

1. **`packages/durable` gained seven subpath exports** — `./env`, `./env/node`, `./tools`,
   `./storage/jsonl`, `./storage/jsonl/node`, `./testing`, and a `default` condition on every entry;
   it kept `.`, `./storage/memory`, `./storage/sqlite`, `./storage/sqlite/node`. It also gained two
   runtime dependencies, `diff@8.0.4` and `typebox@1.3.27`, and dropped `docs` from `files` while
   adding `CHANGELOG.md` (`eaa8b0d06` *fix(durable): publish README and changelog, link the spec,
   add default export condition*). Its `description` changed from record contracts to *"Durable
   conversation, task, and document runtime for Pi"*, and `CHANGELOG.md` @v1.0.0 has exactly one
   entry: *"Initial release of `@earendil-works/pi-durable`, a durable agent harness."* — the first
   release **note**, not the first publishable version.
2. **`packages/ai` gained one subpath export, `./models`** (`ba7d5fbed` *feat(ai,durable): add
   lightweight pi-ai/models entry and avoid TypeBox in durable*). This is **not new code**:
   `packages/ai/src/models.ts` already existed at `v0.87.1` (`git cat-file -e` succeeds) and is
   1 256 lines at v1.0.0. The export exists so `packages/durable` can `import { createModels } from
   "@earendil-works/pi-ai/models"` without pulling the package index, whose `sideEffects` list
   registers the built-in image providers. **No cyrup gap** — cyrup has no module graph to split —
   but no area-01 row mentions it (`grep -rn 'pi-ai/models' docs/gap-analysis` finds only
   this file), and area 01's owner may want it as a one-line packaging note.
3. **`packages/chord` added `src/delta/README.md` to `files`** and nothing else in its packaging.

**cyrup** — nothing is owed by this row. Re-confirmed at HEAD:
`grep -rli 'pi-durable\|pico3\|pico5\|packages/durable' crates --include='*.rs'` returns 0;
`grep -rn 'rusqlite\|sqlx\|libsql\|turso' --include='Cargo.toml' crates/ Cargo.toml` returns 0, so
there is still no embedded database anywhere in the workspace.

**Impact** — none for code. The point is to stop a cost: a later pass that treats the release post's
package list as evidence will open an area-01 repackaging investigation that has no subject, and may
size `HARN-002` against "a brand-new package" when the package is a year-old directory that merely
grew a runtime inside it (which is `HARN-004`'s measurement, and the right one to use).

**Fix** — no code. Record the table above in area 17 so the negative result is citable, and hand the
`pi-ai/models` packaging note to area 01's owner.

**Blind spot** — publishability here was settled from `package.json` and the root README at each tag.
**The npm registry was not queried**, so the *date* each version first appeared on npm is unknown.
Nothing in this file rests on it: the claim is "already publishable and already advertised in-repo at
v0.87.1", which the two tags settle on their own.

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
